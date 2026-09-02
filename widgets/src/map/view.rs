use super::archive::*;
use super::geometry::*;
use super::icons::{icon_mesh_by_slot, ICON_MIN_ZOOM};
use super::label::*;
use super::overlay::*;
use super::style::*;
use super::tile::*;
use crate::{
    makepad_derive_widget::*, makepad_draw::*, widget::*, DrawRotatedText, DrawVector,
    PathGlyphInstance, PathTextPlacement, PreparedTextRun, WidgetMatchEvent,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

/// Tile payload sources only. Navigation data is deliberately outside this
/// contract; applications layer routing datasets over MapView separately.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileSourceConfig {
    LocalArchive {
        mbtiles_path: String,
        detail_mbtiles_path: String,
        overlay_mbtiles_paths: String,
        bridge_dz_path: String,
    },
    HttpArchive {
        root_url: String,
        detail_root_url: String,
        overlay_mbtiles_paths: String,
        bridge_dz_path: String,
    },
}

fn is_mkmap_path_shape(path: &str) -> bool {
    let path = Path::new(path);
    path.extension().is_some_and(|extension| extension == "mkmap")
        || path.file_name().is_some_and(|name| name == "root.mkidx")
}

fn detail_matches_base(config: &TileSourceConfig) -> bool {
    match config {
        TileSourceConfig::LocalArchive {
            mbtiles_path,
            detail_mbtiles_path,
            ..
        } => !detail_mbtiles_path.is_empty() && detail_mbtiles_path == mbtiles_path,
        TileSourceConfig::HttpArchive {
            root_url,
            detail_root_url,
            ..
        } => !detail_root_url.is_empty() && detail_root_url == root_url,
    }
}

fn needs_separate_detail_archive(config: &TileSourceConfig) -> bool {
    match config {
        TileSourceConfig::LocalArchive {
            mbtiles_path,
            detail_mbtiles_path,
            ..
        } => {
            is_mkmap_path_shape(mbtiles_path)
                && is_mkmap_path_shape(detail_mbtiles_path)
                && !detail_mbtiles_path.is_empty()
                && detail_mbtiles_path != mbtiles_path
        }
        TileSourceConfig::HttpArchive {
            root_url,
            detail_root_url,
            ..
        } => !detail_root_url.is_empty() && detail_root_url != root_url,
    }
}

impl TileSourceConfig {
    /// Hosted `.mkmap` archive constructor: base and detail from one root, no
    /// sidecars. Keeping variant construction in the map crate lets callers stay
    /// stable as HTTP sidecar fields come and go.
    pub fn http_archive(root_url: impl Into<String>) -> Self {
        let root_url = root_url.into();
        Self::HttpArchive {
            detail_root_url: root_url.clone(),
            root_url,
            overlay_mbtiles_paths: String::new(),
            bridge_dz_path: String::new(),
        }
    }
}

#[derive(Debug)]
struct ArchiveTileParts {
    generation: u64,
    base: Option<Result<Option<Arc<[u8]>>, String>>,
    detail: Option<Result<Option<Arc<[u8]>>, String>>,
    detail_required: bool,
    reuse_base_as_detail: bool,
}

#[derive(Debug)]
#[cfg(not(target_arch = "wasm32"))]
struct ArchiveWatchResult {
    path: String,
    mtime: Option<u128>,
    zoom_range: Option<(u32, u32)>,
}

#[cfg(not(target_arch = "wasm32"))]
fn archive_mtime(path: &Path) -> Option<u128> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.draw
    use mod.geom
    use mod.math
    use mod.shader

    mod.draw.DrawMapVector = mod.std.set_type_default() do #(DrawMapVector::script_shader(vm)){
        ..mod.draw.DrawVector
        map_scale: uniform(vec2(1.0, 1.0))
        map_offset: uniform(vec2(0.0, 0.0))
        tile_fade: uniform(1.0)
        width_correction: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        // Face variant, clamped >= 1: union faces may only WIDEN (inward
        // morph inverts narrow features); stale cross-band tiles render at
        // keyframe width magnified instead of garbling.
        face_correction: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        // Live view zoom for per-icon zoom floors (param4 on shape 20):
        // stale deeper-bucket tiles must not flash markers on zoom-out.
        icon_zoom: uniform(24.0)
        // 2D->3D transition: scales the per-meter height lift so buildings
        // (and trees/signals) grow out of the ground as their 3D bake fades
        // in.
        height_grow: uniform(1.0)
        // Heading-up camera: cos/sin of the screen rotation and its pivot
        // (the view center). Identity when north-up.
        view_rot: uniform(vec2(1.0, 0.0))
        rot_pivot: uniform(vec2(0.0, 0.0))
        // 2.5D camera: x = cos(tilt) screen-y compression, y = screen px of
        // lift per meter of building height (sin(tilt) baked in), z = depth
        // per screen-y of view-space ground position (hardware occlusion
        // for extruded geometry, rotation-proof), w unused.
        tilt_params: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        // 3D terrain displacement: terrarium-packed elevation texture over
        // a pre-rotation screen rect; span zero = disabled.
        terrain_tex: texture_2d(float)
        shadow_mask: texture_2d(float)
        shadow_mask_on: uniform(0.0)
        shadow_mask_size: uniform(vec2(1.0, 1.0))
        shadow_mask_flip: uniform(0.0)
        shadow_dir: uniform(vec2(0.0, 0.0))
        shadow_cast: uniform(0.0)
        v_screen: varying(vec2f)
        v_lift: varying(float)
        terrain_org: uniform(vec2(0.0, 0.0))
        terrain_span: uniform(vec2(0.0, 0.0))
        // Texel-center remap for the elevation texture (half-texel inset),
        // so bilinear fetch equals the CPU mesh's corner interpolation.
        terrain_uvfit: uniform(vec4(1.0, 1.0, 0.0, 0.0))
        // Regional zooms: big landcover polygons cannot follow the surface
        // between their sparse vertices — leave fills flat and let the
        // (more opaque) surface be the ground; strokes/icons keep riding.
        terrain_fill_lift: uniform(1.0)
        // The Inception mode (close-3D): fold + perspective camera.
        // space_warp:  x = tween amount 0..1, y = fold start r0 (pre-tilt
        //              ground px from the pivot), z = curl radius R, w = sin(tilt).
        // space_warp2: x = kappa (perspective 1/D px^-1, amount-scaled;
        //              0 = ortho), y = px-per-meter (lift px), z = bend cap
        //              angle = tilt rad (wall ends face-on), w unused.
        // Shader twin of overlay.rs SpaceWarp::project — keep in LOCKSTEP.
        // amount 0 short-circuits to the identity path (coherent uniform
        // branch, flat mode untouched).
        space_warp: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        space_warp2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // shiny.md: the one SceneSun + per-feature gates. All gates default
        // 0 -> the material dispatch short-circuits and the frame is
        // identical to the legacy path (uniform branches are coherent).
        // x=water_fx, y=building_sheen(gloss), z=foliage_fx, w=route_glow.
        shiny_gates: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // x=dynamic_sun (reserved), y=shadow_alpha, z/w unused.
        shiny_gates2: uniform(vec4(0.0, 0.22, 0.0, 0.0))
        sun_dir: uniform(vec3(-0.379, -0.575, 0.724))
        sun_color: uniform(vec3(1.0, 0.98, 0.94))
        sun_sky: uniform(vec3(0.55, 0.62, 0.72))

        fragment: fn(){
            if self.shadow_cast > 0.5 {
                if self.shadow_cast < 1.5 {
                    if self.v_lift < 0.05 {
                        discard()
                    }
                    self.fb0 = vec4(1.0, 1.0, 1.0, 1.0)
                    return
                }
                if self.shadow_cast < 2.5 {
                    self.fb0 = vec4(0.0, 0.0, 0.0, 1.0)
                    return
                }
                let c = self.v_color.w
                self.fb0 = vec4(c, c, c, c)
                return
            }
            var color = self.pixel() * self.tile_fade * self.fill_pattern()
            color = self.material_fx(color)
            if self.shadow_mask_on > 0.5 && self.v_lift < 0.05 {
                if self.v_shape_id < 19.5 || self.v_shape_id > 20.5 {
                    var uv = self.v_screen / self.shadow_mask_size
                    if self.shadow_mask_flip > 0.5 {
                        uv.y = 1.0 - uv.y
                    }
                    let m = self.shadow_mask.sample(uv).x
                    color = color * (1.0 - m * self.shiny_gates2.y)
                }
            }
            // Dashed tunnel gaps and the zero-coverage tails of analytic
            // vectors are transparent. Discard them before depth_clip so an
            // invisible carrier cannot occlude the road above it in 3D.
            if color.w <= 0.004 {
                discard()
            }
            self.fb0 = depth_clip(self.v_world_clip, color, self.depth_clip)
        }

        // Textured materials fade to the plain 2D carto look as the camera
        // returns to top-down: tilt drives the blend, so dragging the view
        // flat IS the fade animation (flat green, flat blue, ring trees).
        mat_fade: fn() -> float {
            let ctp = clamp(self.tilt_params.x, 0.0, 1.0)
            let stp = sqrt(max(1.0 - ctp * ctp, 0.0))
            return smoothstep(0.04, 0.32, stp)
        }

        // Cheap value noise for water/foliage (no LUT texture: a handful
        // of ALU on minority-material pixels only).
        mat_hash: fn(p: vec2) -> float {
            return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453)
        }

        mat_noise: fn(p: vec2) -> float {
            let i = floor(p)
            let f = fract(p)
            let u = f * f * (vec2(3.0, 3.0) - 2.0 * f)
            let a = self.mat_hash(i)
            let b = self.mat_hash(i + vec2(1.0, 0.0))
            let c = self.mat_hash(i + vec2(0.0, 1.0))
            let d = self.mat_hash(i + vec2(1.0, 1.0))
            return mix(mix(a, b, u.x), mix(c, d, u.x), u.y)
        }

        // Value noise WITH analytic slope (x = value, yz = d/dxy): the
        // water FBM sums slopes directly — fractal wave normals without
        // finite-difference re-taps.
        mat_noise_d: fn(p: vec2) -> vec3 {
            let i = floor(p)
            let f = fract(p)
            let u = f * f * (vec2(3.0, 3.0) - 2.0 * f)
            let du = 6.0 * f * (vec2(1.0, 1.0) - f)
            let a = self.mat_hash(i)
            let b = self.mat_hash(i + vec2(1.0, 0.0))
            let c = self.mat_hash(i + vec2(0.0, 1.0))
            let d = self.mat_hash(i + vec2(1.0, 1.0))
            let k = a - b - c + d
            return vec3(
                mix(mix(a, b, u.x), mix(c, d, u.x), u.y),
                (b - a + k * u.y) * du.x,
                (c - a + k * u.x) * du.y
            )
        }

        // shiny.md material dispatch on param3 of shape-0 geometry. Every
        // branch sits behind a uniform gate; all-gates-zero returns the
        // input color untouched (screenshot-diffs to zero).
        material_fx: fn(color: vec4) -> vec4 {
            if self.v_shape_id > 0.5 || self.v_shape_id < -0.5 {
                return color
            }
            let mat = self.v_param3;
            if mat < 0.5 {
                return color
            }
            // 7: emissive road (Circuit City) — push the theme's line
            // color toward white-hot by the baked class strength; the
            // dark ground around it does the rest of the glow illusion.
            if mat > 6.5 && mat < 7.5 {
                if self.shiny_gates.w < 0.5 {
                    return color
                }
                let e = clamp(self.v_param1, 0.0, 1.0)
                let wcap = vec3(color.w, color.w, color.w)
                let boosted = color.xyz * (1.0 + 1.5 * e) + wcap * (0.20 * e)
                return vec4(min(boosted, wcap), color.w)
            }
            // 3: water surface (T4), shadertoy-style: a fractal slope FBM
            // (value noise with ANALYTIC derivatives, rotated octaves,
            // each octave shimmering in place on its own drift phase — no
            // sliding-sheet look, no directional seams), chopped into
            // crests and lit with a Blinn-Phong sun glint + fresnel sky.
            // Camera is part of the material: cos(tilt) (tilt_params.x)
            // and the physical zoom factor scale the drama, so top-down /
            // far views stay the flat 2D theme color.
            if mat > 2.5 && mat < 3.5 {
                if self.shiny_gates.x < 0.5 {
                    return color
                }
                let k = self.shiny_gates2.z
                let uv = vec2(self.v_param1, self.v_param2) * k
                let t = self.draw_pass.time
                let ct = clamp(self.tilt_params.x, 0.0, 1.0)
                let st = sqrt(max(1.0 - ct * ct, 0.0))
                let closeness = clamp(1.6 - k, 0.0, 1.0)
                // mat_fade: at top-down the surface returns fully to the
                // flat 2D water color.
                let drama = (0.22 + 0.78 * st) * (0.35 + 0.65 * closeness) * self.mat_fade()
                // High-frequency octaves fade before they go sub-pixel.
                let cut1 = clamp(2.0 - k * 1.2, 0.0, 1.0)
                let cut2 = clamp(2.0 - k * 1.6, 0.0, 1.0)
                var slope = vec2(0.0, 0.0)
                var pp = uv * 0.16
                var nd = self.mat_noise_d(pp + vec2(t * 0.10, t * 0.07))
                slope = slope + nd.yz * 0.16
                pp = vec2(pp.x * 1.6 + pp.y * 1.2, pp.y * 1.6 - pp.x * 1.2) + vec2(4.7, 9.2)
                nd = self.mat_noise_d(pp + vec2(0.0 - t * 0.16, t * 0.12))
                slope = slope + nd.yz * 0.20
                pp = vec2(pp.x * 1.6 + pp.y * 1.2, pp.y * 1.6 - pp.x * 1.2) + vec2(8.1, 2.6)
                nd = self.mat_noise_d(pp + vec2(t * 0.23, 0.0 - t * 0.19))
                slope = slope + nd.yz * 0.26 * cut1
                pp = vec2(pp.x * 1.6 + pp.y * 1.2, pp.y * 1.6 - pp.x * 1.2) + vec2(1.9, 6.3)
                nd = self.mat_noise_d(pp + vec2(0.0 - t * 0.31, 0.0 - t * 0.26))
                slope = slope + nd.yz * 0.32 * cut2
                // Close-zoom octaves: fade in as the physical uv factor
                // shrinks, so the surface keeps gaining real facet detail
                // instead of magnifying into soft blobs.
                let cut3 = clamp(0.9 - k * 2.0, 0.0, 1.0)
                if cut3 > 0.01 {
                    pp = vec2(pp.x * 1.6 + pp.y * 1.2, pp.y * 1.6 - pp.x * 1.2) + vec2(3.3, 5.1)
                    nd = self.mat_noise_d(pp + vec2(t * 0.42, 0.0 - t * 0.35))
                    slope = slope + nd.yz * 0.38 * cut3
                }
                let cut4 = clamp(0.6 - k * 3.0, 0.0, 1.0)
                if cut4 > 0.01 {
                    pp = vec2(pp.x * 1.6 + pp.y * 1.2, pp.y * 1.6 - pp.x * 1.2) + vec2(9.4, 1.7)
                    nd = self.mat_noise_d(pp + vec2(0.0 - t * 0.5, t * 0.44))
                    slope = slope + nd.yz * 0.42 * cut4
                }
                // Chop: steepen the crests so the glint breaks into the
                // irregular sparkle real water has.
                slope = slope * (1.0 + 1.4 * length(slope))
                // Static broad tone patch so big water is not uniform.
                let patch = self.mat_noise(uv * 0.02)
                let n = normalize(vec3(0.0 - slope.x, 0.0 - slope.y, 1.0))
                // Heading rotation into screen space (sun stays
                // geo-anchored); the 2.5D view vector tilts with the
                // camera so grazing angles stretch and brighten the glint.
                let rc = self.view_rot.x
                let rs = self.view_rot.y
                let ns = vec3(n.x * rc - n.y * rs, n.x * rs + n.y * rc, n.z)
                let sd = vec3(
                    self.sun_dir.x * rc - self.sun_dir.y * rs,
                    self.sun_dir.x * rs + self.sun_dir.y * rc,
                    self.sun_dir.z
                )
                let view = normalize(vec3(0.0, st * 0.8, max(ct, 0.30)))
                let h = normalize(sd + view)
                let ndh = max(dot(ns, h), 0.0)
                // The sharp glint belongs to the tilted, grazing view —
                // straight overhead keeps only a whisper of it.
                let spec = (pow(ndh, 90.0) * 1.1 + pow(ndh, 16.0) * 0.20)
                    * (0.18 + 0.82 * st)
                let fres = 0.05 + 0.45 * pow(1.0 - clamp(dot(ns, view), 0.0, 1.0), 3.0)
                let diffuse = 0.90 + 0.13 * clamp(dot(ns, sd), 0.0, 1.0) + 0.05 * patch
                let shade = mix(1.0, diffuse, drama)
                let add = (self.sun_sky * fres * 0.35 + self.sun_color * spec) * drama * color.w
                return vec4(color.xyz * shade + add, color.w)
            }
            // 1/2: building wall/roof specular sheen (T4b). Normals are
            // baked in map space; rotate into screen space so the fixed
            // 2.5D view vector makes highlights sweep during heading
            // rotation — reads as real reflection.
            if mat > 0.5 && mat < 2.5 {
                let gloss = self.shiny_gates.y;
                if gloss < 0.01 {
                    return color
                }
                let c = self.view_rot.x;
                let s = self.view_rot.y;
                var n = vec3(0.0, 0.0, 1.0);
                if mat < 1.5 {
                    let nx = self.v_param1 * c - self.v_param2 * s;
                    let ny = self.v_param1 * s + self.v_param2 * c;
                    // Small constant z-tilt: sun-side facades catch a streak.
                    n = normalize(vec3(nx, ny, 0.30));
                }
                let sd = vec3(
                    self.sun_dir.x * c - self.sun_dir.y * s,
                    self.sun_dir.x * s + self.sun_dir.y * c,
                    self.sun_dir.z
                );
                var h = normalize(sd + vec3(0.0, 0.62, 0.79));
                if mat > 1.5 {
                    // Roofs: nudge the half-vector by screen position — a
                    // fake linear environment gradient instead of one flat
                    // spec value per roof. Clamped: an unbounded nudge let
                    // whole roof fields align with the sun on wide views
                    // and bloom out white.
                    let nudge = clamp(
                        self.v_world * 0.0004,
                        vec2(-0.18, -0.18),
                        vec2(0.18, 0.18)
                    );
                    h = normalize(h + vec3(nudge, 0.0));
                }
                var spec = pow(max(dot(n, h), 0.0), 24.0) * gloss;
                if mat < 1.5 {
                    // Bloom toward rooflines (v_param4 = meters up the wall).
                    spec = spec * clamp(self.v_param4 * 0.05, 0.15, 1.0);
                }
                // Sheen is a close-range material: fade it out toward the
                // regional zooms so distant roof fields stay matte.
                spec = spec * clamp(2.2 - self.shiny_gates2.w, 0.0, 1.0);
                return vec4(color.xyz + self.sun_color * spec * color.w, color.w)
            }
            // 4: tree canopy — leaf-clump noise + sun-side rim (T5),
            // fading to the plain Gouraud ball at top-down.
            if mat > 3.5 && mat < 4.5 {
                if self.shiny_gates.z < 0.5 {
                    return color
                }
                let fade = self.mat_fade()
                if fade < 0.01 {
                    return color
                }
                let n = self.mat_noise(self.v_world * 0.45)
                let clump = 1.0 + (0.24 * n - 0.12) * fade
                let rim = max(
                    dot(normalize(vec3(self.v_param1, self.v_param2, 0.4)), self.sun_dir),
                    0.0
                )
                let add = self.sun_color * rim * rim * 0.06 * fade * color.w
                return vec4(color.xyz * clump + add, color.w)
            }
            // 5: green areas — grass (T5). Grass at map scale is FINE,
            // SHARP speckle (blade clumps catching or losing the light),
            // not an undulating surface: the texture itself stays put and
            // wind reads as slow low-frequency gust bands traveling across
            // the field, modulating how hard the speckle contrast shows.
            // Octaves fade in with the physical zoom factor so close-ups
            // keep gaining detail instead of magnifying blobs.
            if mat > 4.5 && mat < 5.5 {
                if self.shiny_gates.z < 0.5 {
                    return color
                }
                let fade = self.mat_fade()
                if fade < 0.01 {
                    return color
                }
                let k = self.shiny_gates2.z
                let uv = vec2(self.v_param1, self.v_param2) * k
                let t = self.draw_pass.time
                // Broad meadow tone patches (static).
                let patch = self.mat_noise(uv * 0.023) * 0.65 + self.mat_noise(uv * 0.11) * 0.35
                var f = 0.93 + 0.11 * patch
                // Blade-clump speckle, sharpened; per-octave zoom gates.
                let gate1 = clamp(1.5 - k * 1.5, 0.0, 1.0)
                let gate2 = clamp(1.1 - k * 2.2, 0.0, 1.0)
                let gate3 = clamp(0.7 - k * 4.0, 0.0, 1.0)
                if gate1 > 0.01 {
                    var fine = (self.mat_noise(uv * 1.3) - 0.5) * gate1
                    if gate2 > 0.01 {
                        fine = fine + (self.mat_noise(uv * 3.1 + vec2(7.7, 3.9)) - 0.5)
                            * 0.8 * gate2
                    }
                    if gate3 > 0.01 {
                        fine = fine + (self.mat_noise(uv * 7.4 + vec2(2.3, 8.1)) - 0.5)
                            * 0.7 * gate3
                    }
                    // Sharpen: blades are contrasty, not soft gradients.
                    let sharp = clamp(fine * 2.4, -1.0, 1.0)
                    // Wind: gust bands drifting over the field scale the
                    // speckle contrast — the grass itself does not move.
                    let gust = self.mat_noise(uv * 0.03 + vec2(t * 0.05, t * 0.028))
                    f = f + sharp * 0.075 * (0.65 + 0.7 * gust)
                    // Sparse pale speckles: daisies in the lawn.
                    let cell = floor(uv * 0.35)
                    let h = self.mat_hash(cell)
                    if h > 0.986 {
                        let fpos = fract(uv * 0.35) - vec2(0.5, 0.5)
                        let speck = 1.0 - smoothstep(0.05, 0.16, length(fpos))
                        f = f + speck * 0.45 * gate1
                    }
                }
                // Top-down returns to flat carto green.
                f = mix(1.0, f, fade)
                return vec4(color.xyz * f, color.w)
            }
            return color
        }

        vertex: fn() {
            // Packed-vertex preamble: f16 pairs / unorm8x4 unpack once.
            let g_uv = unpack2f16(self.geom.uv)
            let g_color = unpack4u8(self.geom.color)
            let g_p0s = unpack2f16(self.geom.p0s)
            let g_p12 = unpack2f16(self.geom.p12)
            let g_p3c = unpack2f16(self.geom.p3c)
            let pos = vec2(self.geom.x, self.geom.y) + self.inst_anchor;
            var transformed = pos * self.map_scale + self.map_offset;
            var shape_id = g_p0s.y;
            var expanded = 0.0;
            var expand_slack = 0.0;
            var surface_decal = 0.0;
            var terrain_pos = pos * self.map_scale + self.map_offset;
            // shape >= 100: GPU re-expandable stroke — the position is the
            // centerline anchor, param1/2 the baked half-width offset and
            // param3 the width-growth class. The per-class correction turns
            // the baked width into the width the current view zoom calls
            // for, so stale-bucket tiles stay correct through a zoom.
            if shape_id > 99.5 {
                shape_id = shape_id - 100.0;
                expanded = 1.0;
                var cls = g_p3c.x;
                var corr = self.width_correction.x;
                if cls > 3.5 {
                    // Face band (class + 4): clamped corrections.
                    cls = cls - 4.0;
                    corr = self.face_correction.x;
                    if cls > 2.5 {
                        corr = self.face_correction.w;
                    } else if cls > 1.5 {
                        corr = self.face_correction.z;
                    } else if cls > 0.5 {
                        corr = self.face_correction.y;
                    }
                } else if cls > 2.5 {
                    corr = self.width_correction.w;
                } else if cls > 1.5 {
                    corr = self.width_correction.z;
                } else if cls > 0.5 {
                    corr = self.width_correction.y;
                }
                let off = vec2(g_p12.x, g_p12.y);
                transformed = transformed + off * self.map_scale * corr;
                expand_slack = length(off) * (corr + 1.0);
            }
            // shape-20/param3=2 is a zoom-constant road-surface decal
            // (currently oneway arrows). Put its screen-px vertex offset
            // into the MAP plane before camera rotation and tilt. This
            // gives every glyph vertex the same projection/depth basis as
            // the road directly beneath it instead of rotating a flat card
            // around one lifted anchor.
            if shape_id > 19.5 && shape_id < 20.5 && g_p3c.x > 1.5 {
                let off = vec2(g_p12.x, g_p12.y);
                transformed = transformed + off;
                terrain_pos = terrain_pos + off;
                surface_decal = 1.0;
            }
            var feature_lift = self.geom.param4 * self.height_grow;
            if shape_id > 19.5 && shape_id < 20.5 {
                if surface_decal > 0.5 {
                    feature_lift = self.geom.param4 * self.height_grow;
                } else {
                    let icon_floor = modf(self.geom.param4, 100.0);
                    feature_lift = (self.geom.param4 - icon_floor) * 0.0025 * self.height_grow;
                }
            }
            if self.shadow_cast > 0.5 && self.shadow_cast < 1.5 {
                let delta = self.shadow_dir * feature_lift * self.map_scale;
                transformed = transformed + delta;
                terrain_pos = terrain_pos + delta;
            }
            // 3D terrain: every vertex lifts by the ground elevation under
            // it, so roads/fills/buildings ride the displaced surface.
            // Sampled at the centerline anchor (pre width-expansion): the
            // casing and center of one road must lift identically. Surface
            // decals instead sample at their actual offset vertex.
            var ground_m = 0.0;
            if self.terrain_span.x > 0.5 {
                let tuv = (terrain_pos - self.terrain_org) / self.terrain_span;
                if tuv.x > 0.0 && tuv.x < 1.0 && tuv.y > 0.0 && tuv.y < 1.0 {
                    let fit = tuv * self.terrain_uvfit.xy + self.terrain_uvfit.zw;
                    let enc = self.terrain_tex.sample_lod(fit, 0.0);
                    ground_m = max(
                        enc.x * 65280.0 + enc.y * 255.0 + enc.z * 0.99609375 - 32768.0,
                        0.0
                    );
                }
            }
            // Heading-up camera: rotate map geometry (and expanded stroke
            // offsets, which are map-space) about the view center.
            let rel = transformed - self.rot_pivot;
            transformed = self.rot_pivot + vec2(
                rel.x * self.view_rot.x - rel.y * self.view_rot.y,
                rel.x * self.view_rot.y + rel.y * self.view_rot.x
            );
            // 2.5D: axonometric tilt compresses screen y about the pivot;
            // building vertices carry their height in meters in param4 and
            // extrude toward screen-top. The pre-tilt (ground) y doubles as
            // the view depth so the depth buffer resolves occlusion.
            let ground_rel_y = transformed.y - self.rot_pivot.y;
            // param4 is building height in meters — EXCEPT on shape 20
            // icons, where it carries the icon's zoom floor and must not
            // lift the marker off the ground.
            var ground_fill = ground_m;
            if expanded < 0.5 {
                ground_fill = ground_m * self.terrain_fill_lift;
            }
            var lift_m = feature_lift + ground_fill;
            if shape_id > 19.5 && shape_id < 20.5 {
                lift_m = feature_lift + ground_m;
            }
            if self.shadow_cast > 0.5 {
                lift_m = ground_fill;
            }
            // The Inception mode: fold + perspective camera (CPU twin:
            // overlay.rs SpaceWarp::project — keep in LOCKSTEP). Ground
            // beyond r0 curls up along a circle until its tangent is
            // face-on to the camera (cap = tilt), then runs straight, so
            // the far field reads as an undistorted flat map; kappa pulls
            // the ortho camera in to a finite dolly distance (scale 1 at
            // the pivot). Building lift extrudes along the LOCAL surface
            // normal. DEPTH below keeps the UNWARPED ground_rel_y —
            // ground distance stays a monotone proxy for camera z in this
            // camera family, so the ortho depth ladder remains valid.
            if self.space_warp.x > 0.0001 {
                let cos_t = self.tilt_params.x
                let sin_t = self.space_warp.w
                let hpx = lift_m * self.space_warp2.y
                let wg = 0.0 - ground_rel_y
                var wf = wg
                var wu = 0.0
                var wnx = 0.0
                var wny = 1.0
                let wa = wg - self.space_warp.y
                if wa > 0.0 {
                    let wr = max(self.space_warp.z, 1.0)
                    let cap = self.space_warp2.z
                    let th = min(wa / wr, cap)
                    let sth = sin(th)
                    let cth = cos(th)
                    wf = self.space_warp.y + wr * sth
                    wu = wr * (1.0 - cth)
                    let we = wa - wr * cap
                    if we > 0.0 {
                        wf = wf + we * cos_t
                        wu = wu + we * sin_t
                    }
                    wnx = 0.0 - sth
                    wny = cth
                }
                let pf = wf + hpx * wnx
                let pu = wu + hpx * wny
                let bf = wg + (pf - wg) * self.space_warp.x
                let bu = hpx + (pu - hpx) * self.space_warp.x
                let zrel = bf * sin_t - bu * cos_t
                let pw = 1.0 / max(1.0 + self.space_warp2.x * zrel, 0.12)
                transformed = vec2(
                    self.rot_pivot.x + (transformed.x - self.rot_pivot.x) * pw,
                    self.rot_pivot.y - (bf * cos_t + bu * sin_t) * pw
                );
            } else {
                transformed.y = self.rot_pivot.y
                    + ground_rel_y * self.tilt_params.x
                    - lift_m * self.tilt_params.y;
            }
            // shape 20: zoom-constant symbol. POI symbols stay upright and
            // add their offset here; surface decals were already projected
            // through the map plane above.
            if shape_id > 19.5 && shape_id < 20.5 {
                // 0.6 grace below the floor: markers fade out on a zoom
                // gesture instead of vanishing the instant the tier line
                // is crossed (still far above the stale-carpet zone).
                // FAIL-OPEN: if the icon_zoom uniform hasn't landed (reads
                // ~0 — seen when a startup DSL override re-parses this
                // shader), the gate disarms instead of hiding every icon.
                if surface_decal < 0.5
                    && self.icon_zoom > 1.0
                    && modf(self.geom.param4, 100.0) > self.icon_zoom + 0.6 {
                    self.vertex_pos = vec4(0.0, 0.0, 0.0, 0.0);
                    return
                }
                if surface_decal < 0.5 {
                    var off = vec2(g_p12.x, g_p12.y);
                    if g_p3c.x > 0.5 {
                        off = vec2(
                            off.x * self.view_rot.x - off.y * self.view_rot.y,
                            off.x * self.view_rot.y + off.y * self.view_rot.x
                        );
                        off.y = off.y * self.tilt_params.x;
                    }
                    transformed = transformed + off;
                }
            }

            self.v_tcoord = vec2(g_uv.x, g_uv.y);
            self.v_color = vec4(g_color.x, g_color.y, g_color.z, g_color.w);
            self.v_stroke_mult = self.geom.stroke_mult;
            // stroke distances are tile-local; scale so dash patterns stay in screen px
            self.v_stroke_dist = self.geom.stroke_dist * self.map_scale.x;
            self.v_shape_id = shape_id;
            self.v_param0 = g_p0s.x;
            self.v_param5 = self.geom.param5;

            let grad_type = g_p0s.x;
            if expanded > 0.5 {
                self.v_param1 = 0.0;
                self.v_param2 = 0.0;
                self.v_param3 = 0.0;
                self.v_param4 = 0.0;
            } else if grad_type > 0.5 && grad_type < 1.5 {
                let p0 = vec2(g_p12.x, g_p12.y) * self.map_scale + self.map_offset;
                let p1 = vec2(g_p3c.x, self.geom.param4) * self.map_scale + self.map_offset;
                self.v_param1 = p0.x;
                self.v_param2 = p0.y;
                self.v_param3 = p1.x;
                self.v_param4 = p1.y;
            } else if grad_type > 1.5 {
                let center = vec2(g_p12.x, g_p12.y) * self.map_scale + self.map_offset;
                self.v_param1 = center.x;
                self.v_param2 = center.y;
                self.v_param3 = g_p3c.x * self.map_scale.x;
                self.v_param4 = self.geom.param4 * self.map_scale.y;
            } else if shape_id > 0.5 && shape_id < 19.5 {
                let bbox_min = vec2(g_p12.x, g_p12.y) * self.map_scale + self.map_offset;
                let bbox_max = vec2(g_p3c.x, self.geom.param4) * self.map_scale + self.map_offset;
                self.v_param1 = bbox_min.x;
                self.v_param2 = bbox_min.y;
                self.v_param3 = bbox_max.x;
                self.v_param4 = bbox_max.y;
            } else if shape_id > 29.5 && shape_id < 32.5 {
                // Pattern fills: anchor the texture to the MAP, not the
                // screen — tile-local position scaled to view px (stable
                // under pan/rotation; rebakes per zoom like carto).
                let pattern_uv = pos * self.map_scale;
                self.v_param1 = pattern_uv.x;
                self.v_param2 = pattern_uv.y;
                self.v_param3 = 0.0;
                self.v_param4 = 0.0;
            } else if shape_id < 0.5
                && (
                    (g_p3c.x > 2.5 && g_p3c.x < 3.5)
                    || (g_p3c.x > 4.5 && g_p3c.x < 5.5)
                ) {
                // Water/green-area materials: param1/2 are free on these
                // fills — carry a map-anchored UV for the per-pixel noise
                // (same anchoring trick as the pattern fills above).
                let mat_uv = pos * self.map_scale;
                self.v_param1 = mat_uv.x;
                self.v_param2 = mat_uv.y;
                self.v_param3 = g_p3c.x;
                self.v_param4 = self.geom.param4;
            } else {
                self.v_param1 = g_p12.x;
                self.v_param2 = g_p12.y;
                self.v_param3 = g_p3c.x;
                self.v_param4 = self.geom.param4;
            }

            let shifted = transformed + self.draw_list.view_shift;
            self.v_world = shifted;
            self.v_screen = transformed - self.rot_pivot + self.shadow_mask_size * 0.5;
            self.v_lift = feature_lift;

            let cr = (g_p3c.y + expand_slack) * max(self.map_scale.x, self.map_scale.y);
            let clip = vec4(
                max(self.draw_clip.x, self.draw_list.view_clip.x - self.draw_list.view_shift.x),
                max(self.draw_clip.y, self.draw_list.view_clip.y - self.draw_list.view_shift.y),
                min(self.draw_clip.z, self.draw_list.view_clip.z - self.draw_list.view_shift.x),
                min(self.draw_clip.w, self.draw_list.view_clip.w - self.draw_list.view_shift.y)
            )

            if transformed.x + cr < clip.x || transformed.y + cr < clip.y
                || transformed.x - cr > clip.z || transformed.y - cr > clip.w {
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 0.0);
                return
            }

            let world = self.draw_list.view_transform * vec4(
                shifted.x
                shifted.y
                // Flat: classic call-order painting. Tilted: self-contained
                // depth — view-ground y dominates, per-pass offset (in w)
                // keeps casing/center/icon layering, baked feature order
                // shrinks to the smallest scale. draw_call.zbias would
                // otherwise grow with call count and beat small lifts.
                self.draw_depth + self.tilt_params.w
                    + mix(
                        self.draw_call.zbias + self.geom.zbias + self.inst_zbias,
                        // Lifted geometry carries its lift into depth: a
                        // deck 2.5 m up must beat the water/ground drawn at
                        // its DISPLAY position, and the required margin is
                        // exactly lift_px * depth-per-ground-px — constant
                        // bumps can't track zoom.
                        self.geom.param5
                            + (ground_rel_y + lift_m * self.tilt_params.y)
                                * self.tilt_params.z,
                        sign(self.tilt_params.z)
                    )
                1.
            );
            self.v_world_clip = world;
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world)
        }

        fill_pattern: fn() {
            // 30: small staggered dot stipple (courtyard gardens).
            if self.v_shape_id > 29.5 && self.v_shape_id < 30.5 {
                let uv = vec2(self.v_param1, self.v_param2)
                let period = 5.0
                let row = floor(uv.y / period)
                let sx = uv.x + fract(row * 0.5) * period
                let cell = fract(vec2(sx, uv.y) / period) - vec2(0.5, 0.5)
                let d = length(cell) * period
                let dot = 1.0 - smoothstep(0.55, 1.0, d)
                let f = 1.0 - 0.14 * dot
                return vec4(f, f, f, 1.0)
            }
            // 31: diagonal hatch (playgrounds).
            if self.v_shape_id > 30.5 && self.v_shape_id < 31.5 {
                let uv = vec2(self.v_param1, self.v_param2)
                let band = fract((uv.x + uv.y) / 9.0)
                let line = 1.0 - smoothstep(0.10, 0.20, abs(band - 0.5))
                let f = 1.0 - 0.12 * line
                return vec4(f, f, f, 1.0)
            }
            // 32: woods/cemeteries. Legacy: staggered open tree rings.
            // With foliage_fx on and the camera tilted in close: plump
            // shaded canopy blobs, anchored map-PHYSICALLY across the full
            // zoom range (the wide uv factor) so they scale with the map
            // instead of sliding at constant screen size. Far out or
            // top-down the pattern blends back to the classic rings.
            if self.v_shape_id > 31.5 && self.v_shape_id < 32.5 {
                let legacy_uv = vec2(self.v_param1, self.v_param2)
                let lrow = floor(legacy_uv.y / 12.0)
                let lsx = legacy_uv.x + fract(lrow * 0.5) * 12.0
                let lcell = fract(vec2(lsx, legacy_uv.y) / 12.0) - vec2(0.5, 0.5)
                let ld = length(lcell) * 12.0
                let ring = 1.0 - smoothstep(0.45, 0.85, abs(ld - 2.4))
                let legacy_f = 1.0 - 0.15 * ring
                var shrub_amount = 0.0
                if self.shiny_gates.z > 0.5 {
                    let w = self.shiny_gates2.w
                    shrub_amount = clamp((3.2 - w) / 1.2, 0.0, 1.0) * self.mat_fade()
                }
                if shrub_amount < 0.01 {
                    return vec4(legacy_f, legacy_f, legacy_f, 1.0)
                }
                // Full physical anchoring down to the deepest zooms: a
                // frozen clamp here let the lattice swim against the map
                // past z18 (crowns are ~8 m physical objects; they simply
                // get big up close).
                let w = clamp(self.shiny_gates2.w, 0.03, 4.0)
                let uv = vec2(self.v_param1, self.v_param2) * w
                // FOREST, not polkadot: three staggered crown lattices so
                // canopies overlap and cover most of the ground; the
                // nearest crown wins the pixel, its dome shades toward the
                // sun, and only the crevices no crown reaches drop to the
                // dark understory.
                let period = 10.0
                var best_d = 9.0
                var best_lit = 0.0
                var best_rnd = 0.0
                // Layer 1
                var luv = uv
                var row = floor(luv.y / period)
                var sx = luv.x + fract(row * 0.5) * period
                var cell_id = vec2(floor(sx / period), row)
                var jit = vec2(
                    self.mat_hash(cell_id) - 0.5,
                    self.mat_hash(cell_id + vec2(11.7, 3.1)) - 0.5
                ) * (period * 0.30)
                var cell = (fract(vec2(sx, luv.y) / period) - vec2(0.5, 0.5)) * period - jit
                var r = period * (0.42 + self.mat_hash(cell_id + vec2(5.2, 8.8)) * 0.16)
                var d = length(cell) / r
                if d < 1.0 {
                    best_d = d
                    best_lit = clamp(0.55 - dot(cell, vec2(0.13, 0.16)) / r, 0.0, 1.0)
                    best_rnd = self.mat_hash(cell_id + vec2(2.4, 6.6))
                }
                // Layer 2 (offset half period)
                luv = uv + vec2(period * 0.5, period * 0.31)
                row = floor(luv.y / period)
                sx = luv.x + fract(row * 0.5) * period
                cell_id = vec2(floor(sx / period), row) + vec2(37.0, 17.0)
                jit = vec2(
                    self.mat_hash(cell_id) - 0.5,
                    self.mat_hash(cell_id + vec2(11.7, 3.1)) - 0.5
                ) * (period * 0.30)
                cell = (fract(vec2(sx, luv.y) / period) - vec2(0.5, 0.5)) * period - jit
                r = period * (0.42 + self.mat_hash(cell_id + vec2(5.2, 8.8)) * 0.16)
                d = length(cell) / r
                if d < best_d {
                    best_d = d
                    best_lit = clamp(0.55 - dot(cell, vec2(0.13, 0.16)) / r, 0.0, 1.0)
                    best_rnd = self.mat_hash(cell_id + vec2(2.4, 6.6))
                }
                // Layer 3 (offset the other diagonal)
                luv = uv + vec2(period * 0.19, period * 0.67)
                row = floor(luv.y / period)
                sx = luv.x + fract(row * 0.5) * period
                cell_id = vec2(floor(sx / period), row) + vec2(11.0, 53.0)
                jit = vec2(
                    self.mat_hash(cell_id) - 0.5,
                    self.mat_hash(cell_id + vec2(11.7, 3.1)) - 0.5
                ) * (period * 0.30)
                cell = (fract(vec2(sx, luv.y) / period) - vec2(0.5, 0.5)) * period - jit
                r = period * (0.42 + self.mat_hash(cell_id + vec2(5.2, 8.8)) * 0.16)
                d = length(cell) / r
                if d < best_d {
                    best_d = d
                    best_lit = clamp(0.55 - dot(cell, vec2(0.13, 0.16)) / r, 0.0, 1.0)
                    best_rnd = self.mat_hash(cell_id + vec2(2.4, 6.6))
                }
                var f = 0.74
                if best_d < 1.0 {
                    // Crown dome: lit toward the sun, dark toward the rim,
                    // per-crown value variation so the canopy reads as
                    // many individual trees packed together.
                    let dome = 1.0 - best_d * best_d * 0.30
                    f = (0.86 + 0.26 * best_lit + 0.10 * (best_rnd - 0.5)) * dome
                }
                f = mix(legacy_f, f, shrub_amount)
                return vec4(f, f, f, 1.0)
            }
            return vec4(1.0, 1.0, 1.0, 1.0)
        }

        get_stroke_mask: fn() {
            if self.v_shape_id > 9.5 && self.v_shape_id < 10.5 {
                return self.dash(3.2, 2.4)
            }
            if self.v_shape_id > 10.5 && self.v_shape_id < 11.5 {
                return self.dash(2.0, 3.0)
            }
            if self.v_shape_id > 11.5 && self.v_shape_id < 12.5 {
                return self.dash(8.0, 8.0)
            }
            return 1.0
        }
    }

    // Ground polygon fills: 20-byte vertices. Their UVs are map-anchored
    // position, and all other generic-vector channels are constants. The
    // inherited fragment/material/pattern path remains the source of truth.
    mod.draw.DrawMapFill = mod.std.set_type_default() do #(DrawMapFill::script_shader(vm)){
        ..mod.draw.DrawMapVector
        geom: vertex_buffer(geom.FillVertexPacked, geom.FillGeomPacked)

        vertex: fn() {
            let pos = vec2(self.geom.x, self.geom.y)
            let fill = unpack2f16(self.geom.params)
            let color = unpack4u8(self.geom.color)
            let depth = unpack4u8(self.geom.zbias) * 255.0
            let flat_zbias = (depth.x + depth.y * 256.0) * 0.000001
            let param5 = (depth.z + depth.w * 256.0) * 0.00001
            var shape_id = 0.0
            var material = fill.x
            if fill.x > 29.5 {
                shape_id = fill.x
                material = 0.0
            }

            var transformed = pos * self.map_scale + self.map_offset
            let terrain_pos = transformed
            var ground_m = 0.0
            if self.terrain_span.x > 0.5 {
                let tuv = (terrain_pos - self.terrain_org) / self.terrain_span
                if tuv.x > 0.0 && tuv.x < 1.0 && tuv.y > 0.0 && tuv.y < 1.0 {
                    let fit = tuv * self.terrain_uvfit.xy + self.terrain_uvfit.zw
                    let enc = self.terrain_tex.sample_lod(fit, 0.0)
                    ground_m = max(
                        enc.x * 65280.0 + enc.y * 255.0 + enc.z * 0.99609375 - 32768.0,
                        0.0
                    )
                }
            }
            let rel = transformed - self.rot_pivot
            transformed = self.rot_pivot + vec2(
                rel.x * self.view_rot.x - rel.y * self.view_rot.y,
                rel.x * self.view_rot.y + rel.y * self.view_rot.x
            )
            let ground_rel_y = transformed.y - self.rot_pivot.y
            let lift_m = ground_m * self.terrain_fill_lift
            if self.space_warp.x > 0.0001 {
                let cos_t = self.tilt_params.x
                let sin_t = self.space_warp.w
                let hpx = lift_m * self.space_warp2.y
                let wg = 0.0 - ground_rel_y
                var wf = wg
                var wu = 0.0
                var wnx = 0.0
                var wny = 1.0
                let wa = wg - self.space_warp.y
                if wa > 0.0 {
                    let wr = max(self.space_warp.z, 1.0)
                    let cap = self.space_warp2.z
                    let th = min(wa / wr, cap)
                    let sth = sin(th)
                    let cth = cos(th)
                    wf = self.space_warp.y + wr * sth
                    wu = wr * (1.0 - cth)
                    let we = wa - wr * cap
                    if we > 0.0 {
                        wf = wf + we * cos_t
                        wu = wu + we * sin_t
                    }
                    wnx = 0.0 - sth
                    wny = cth
                }
                let pf = wf + hpx * wnx
                let pu = wu + hpx * wny
                let bf = wg + (pf - wg) * self.space_warp.x
                let bu = hpx + (pu - hpx) * self.space_warp.x
                let zrel = bf * sin_t - bu * cos_t
                let pw = 1.0 / max(1.0 + self.space_warp2.x * zrel, 0.12)
                transformed = vec2(
                    self.rot_pivot.x + (transformed.x - self.rot_pivot.x) * pw,
                    self.rot_pivot.y - (bf * cos_t + bu * sin_t) * pw
                )
            } else {
                transformed.y = self.rot_pivot.y
                    + ground_rel_y * self.tilt_params.x
                    - lift_m * self.tilt_params.y
            }

            self.v_tcoord = vec2(fill.y, 1.0)
            self.v_color = color
            self.v_stroke_mult = 1e6
            self.v_stroke_dist = 0.0
            self.v_shape_id = shape_id
            self.v_param0 = 0.0
            if shape_id > 29.5 || (material > 2.5 && material < 3.5)
                || (material > 4.5 && material < 5.5) {
                let map_uv = pos * self.map_scale
                self.v_param1 = map_uv.x
                self.v_param2 = map_uv.y
            } else {
                self.v_param1 = 0.0
                self.v_param2 = 0.0
            }
            self.v_param3 = material
            self.v_param4 = 0.0
            self.v_param5 = param5

            let shifted = transformed + self.draw_list.view_shift
            self.v_world = shifted
            self.v_screen = transformed - self.rot_pivot + self.shadow_mask_size * 0.5
            self.v_lift = 0.0
            let world = self.draw_list.view_transform * vec4(
                shifted.x
                shifted.y
                self.draw_depth + self.tilt_params.w
                    + mix(
                        self.draw_call.zbias + flat_zbias,
                        param5 + (ground_rel_y + lift_m * self.tilt_params.y)
                            * self.tilt_params.z,
                        sign(self.tilt_params.z)
                    )
                1.
            )
            self.v_world_clip = world
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world)
        }
    }

    // Compact road-only path: eight 32-bit lanes instead of DrawMapVector's
    // twelve. params.x is an f16 integer: class + 8*material in the low
    // six bits, then dash (get_stroke_mask 10/11/12 as 1/2/3) and kind
    // (stroke/fill/fringe). params.y carries the one kind-specific pixel
    // scalar (dash distance, route-emissive strength, or coverage); uv
    // preserves both tessellator coordinates.
    mod.draw.DrawMapRoad = mod.std.set_type_default() do #(DrawMapRoad::script_shader(vm)){
        ..mod.draw.DrawMapVector
        geom: vertex_buffer(geom.RoadVertexPacked, geom.RoadGeomPacked)

        vertex: fn() {
            let off = unpack2f16(self.geom.off)
            let road_params = unpack2f16(self.geom.params)
            let road_depth = unpack2f16(self.geom.depth)
            let road_uv = unpack2f16(self.geom.uv)
            let expanded = floor(road_params.x / 1024.0)
            let meta = road_params.x - expanded * 1024.0
            let kind = floor(meta / 256.0)
            let dash_id = floor(modf(meta, 256.0) / 64.0)
            let class_material = modf(meta, 64.0)
            var cls = modf(class_material, 8.0)
            let material = floor(class_material / 8.0)

            let pos = vec2(self.geom.x, self.geom.y)
            var transformed = pos * self.map_scale + self.map_offset
            let terrain_pos = transformed
            var corr = self.width_correction.x
            if cls > 3.5 {
                cls = cls - 4.0
                corr = self.face_correction.x
                if cls > 2.5 {
                    corr = self.face_correction.w
                } else if cls > 1.5 {
                    corr = self.face_correction.z
                } else if cls > 0.5 {
                    corr = self.face_correction.y
                }
            } else if cls > 2.5 {
                corr = self.width_correction.w
            } else if cls > 1.5 {
                corr = self.width_correction.z
            } else if cls > 0.5 {
                corr = self.width_correction.y
            }
            transformed = transformed + off * self.map_scale * corr
            if self.shadow_cast > 0.5 && self.shadow_cast < 1.5 {
                transformed = transformed
                    + self.shadow_dir * self.geom.deck * self.height_grow * self.map_scale
            }

            // Roads sample terrain at their centerline anchor, so casing and
            // center remain on one plane even though their offsets differ.
            var ground_m = 0.0
            if self.terrain_span.x > 0.5 {
                let tuv = (terrain_pos - self.terrain_org) / self.terrain_span
                if tuv.x > 0.0 && tuv.x < 1.0 && tuv.y > 0.0 && tuv.y < 1.0 {
                    let fit = tuv * self.terrain_uvfit.xy + self.terrain_uvfit.zw
                    let enc = self.terrain_tex.sample_lod(fit, 0.0)
                    ground_m = max(
                        enc.x * 65280.0 + enc.y * 255.0 + enc.z * 0.99609375 - 32768.0,
                        0.0
                    )
                }
            }

            let rel = transformed - self.rot_pivot
            transformed = self.rot_pivot + vec2(
                rel.x * self.view_rot.x - rel.y * self.view_rot.y,
                rel.x * self.view_rot.y + rel.y * self.view_rot.x
            )
            let ground_rel_y = transformed.y - self.rot_pivot.y
            var ground_fill = ground_m
            if expanded < 0.5 {
                ground_fill = ground_m * self.terrain_fill_lift
            }
            let lift_m = self.geom.deck * self.height_grow + ground_fill

            // Inception/space-warp projection, in lockstep with the generic
            // vector path. Road meshes use their own packed-stride midpoint
            // subdivision before upload, so long chords follow the fold.
            if self.space_warp.x > 0.0001 {
                let cos_t = self.tilt_params.x
                let sin_t = self.space_warp.w
                let hpx = lift_m * self.space_warp2.y
                let wg = 0.0 - ground_rel_y
                var wf = wg
                var wu = 0.0
                var wnx = 0.0
                var wny = 1.0
                let wa = wg - self.space_warp.y
                if wa > 0.0 {
                    let wr = max(self.space_warp.z, 1.0)
                    let cap = self.space_warp2.z
                    let th = min(wa / wr, cap)
                    let sth = sin(th)
                    let cth = cos(th)
                    wf = self.space_warp.y + wr * sth
                    wu = wr * (1.0 - cth)
                    let we = wa - wr * cap
                    if we > 0.0 {
                        wf = wf + we * cos_t
                        wu = wu + we * sin_t
                    }
                    wnx = 0.0 - sth
                    wny = cth
                }
                let pf = wf + hpx * wnx
                let pu = wu + hpx * wny
                let bf = wg + (pf - wg) * self.space_warp.x
                let bu = hpx + (pu - hpx) * self.space_warp.x
                let zrel = bf * sin_t - bu * cos_t
                let pw = 1.0 / max(1.0 + self.space_warp2.x * zrel, 0.12)
                transformed = vec2(
                    self.rot_pivot.x + (transformed.x - self.rot_pivot.x) * pw,
                    self.rot_pivot.y - (bf * cos_t + bu * sin_t) * pw
                )
            } else {
                transformed.y = self.rot_pivot.y
                    + ground_rel_y * self.tilt_params.x
                    - lift_m * self.tilt_params.y
            }

            // get_stroke_mask distinguishes 10 / 11 / 12; anything else is solid.
            var shape_id = 0.0
            if dash_id > 0.5 && dash_id < 1.5 {
                shape_id = 10.0
            } else if dash_id > 1.5 && dash_id < 2.5 {
                shape_id = 11.0
            } else if dash_id > 2.5 {
                shape_id = 12.0
            }
            let color = unpack4u8(self.geom.color)
            if kind > 1.5 {
                self.v_color = color
                self.v_stroke_mult = 2000000.0
            } else if kind > 0.5 {
                self.v_color = color
                self.v_stroke_mult = 1000000.0
            } else {
                self.v_color = color
                self.v_stroke_mult = 1.0
            }
            self.v_tcoord = road_uv
            self.v_stroke_dist = road_params.y * self.map_scale.x
            self.v_shape_id = shape_id
            self.v_param0 = 0.0
            self.v_param1 = 0.0
            self.v_param2 = 0.0
            self.v_param3 = 0.0
            self.v_param4 = 0.0
            if expanded < 0.5 {
                if material > 6.5 {
                    self.v_param1 = road_params.y
                }
                self.v_param3 = material
                self.v_param4 = self.geom.deck
            }
            self.v_param5 = road_depth.x

            let shifted = transformed + self.draw_list.view_shift
            self.v_world = shifted
            self.v_screen = transformed - self.rot_pivot + self.shadow_mask_size * 0.5
            self.v_lift = self.geom.deck * self.height_grow
            let world = self.draw_list.view_transform * vec4(
                shifted.x
                shifted.y
                self.draw_depth + self.tilt_params.w
                    + mix(
                        self.draw_call.zbias + road_depth.y * 0.000001,
                        road_depth.x
                            + (ground_rel_y + lift_m * self.tilt_params.y)
                                * self.tilt_params.z,
                        sign(self.tilt_params.z)
                    )
                1.
            )
            self.v_world_clip = world
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world)
        }
    }

    // Instanced POI symbols: one shared mesh per symbol slot, an 8-float
    // record per placement (anchor, screen offset, scale, the zoom-floor /
    // pin-lift composite, zbias, colour). This is DrawMapVector's shape-20
    // vertex path with the per-placement slots read from the instance —
    // keep the two in LOCKSTEP; the pixel side is inherited unchanged.
    mod.draw.DrawMapIcon = mod.std.set_type_default() do #(DrawMapIcon::script_shader(vm)){
        ..mod.draw.DrawMapVector
        geom: vertex_buffer(geom.IconVertexPacked, geom.IconGeomPacked)

        vertex: fn() {
            var transformed = self.inst_anchor * self.map_scale + self.map_offset;
            let terrain_pos = transformed;
            var ground_m = 0.0;
            if self.terrain_span.x > 0.5 {
                let tuv = (terrain_pos - self.terrain_org) / self.terrain_span;
                if tuv.x > 0.0 && tuv.x < 1.0 && tuv.y > 0.0 && tuv.y < 1.0 {
                    let fit = tuv * self.terrain_uvfit.xy + self.terrain_uvfit.zw;
                    let enc = self.terrain_tex.sample_lod(fit, 0.0);
                    ground_m = max(
                        enc.x * 65280.0 + enc.y * 255.0 + enc.z * 0.99609375 - 32768.0,
                        0.0
                    );
                }
            }
            let rel = transformed - self.rot_pivot;
            transformed = self.rot_pivot + vec2(
                rel.x * self.view_rot.x - rel.y * self.view_rot.y,
                rel.x * self.view_rot.y + rel.y * self.view_rot.x
            );
            let ground_rel_y = transformed.y - self.rot_pivot.y;
            // param4 = zoom_floor + pin_lift_m*100: markers fly at their
            // encoded height (0 for grounded icons).
            let icon_floor = modf(self.inst_param4, 100.0);
            let lift_m = (self.inst_param4 - icon_floor) * 0.0025 * self.height_grow + ground_m;
            if self.space_warp.x > 0.0001 {
                let cos_t = self.tilt_params.x
                let sin_t = self.space_warp.w
                let hpx = lift_m * self.space_warp2.y
                let wg = 0.0 - ground_rel_y
                var wf = wg
                var wu = 0.0
                var wnx = 0.0
                var wny = 1.0
                let wa = wg - self.space_warp.y
                if wa > 0.0 {
                    let wr = max(self.space_warp.z, 1.0)
                    let cap = self.space_warp2.z
                    let th = min(wa / wr, cap)
                    let sth = sin(th)
                    let cth = cos(th)
                    wf = self.space_warp.y + wr * sth
                    wu = wr * (1.0 - cth)
                    let we = wa - wr * cap
                    if we > 0.0 {
                        wf = wf + we * cos_t
                        wu = wu + we * sin_t
                    }
                    wnx = 0.0 - sth
                    wny = cth
                }
                let pf = wf + hpx * wnx
                let pu = wu + hpx * wny
                let bf = wg + (pf - wg) * self.space_warp.x
                let bu = hpx + (pu - hpx) * self.space_warp.x
                let zrel = bf * sin_t - bu * cos_t
                let pw = 1.0 / max(1.0 + self.space_warp2.x * zrel, 0.12)
                transformed = vec2(
                    self.rot_pivot.x + (transformed.x - self.rot_pivot.x) * pw,
                    self.rot_pivot.y - (bf * cos_t + bu * sin_t) * pw
                );
            } else {
                transformed.y = self.rot_pivot.y
                    + ground_rel_y * self.tilt_params.x
                    - lift_m * self.tilt_params.y;
            }
            // 0.6 grace below the floor: markers fade out on a zoom
            // gesture instead of vanishing the instant the tier line is
            // crossed. FAIL-OPEN when the icon_zoom uniform has not landed.
            if self.icon_zoom > 1.0 && icon_floor > self.icon_zoom + 0.6 {
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 0.0);
                return
            }
            // Zoom-constant symbol: the mesh vertex is a screen-px offset
            // from the anchor, added after the map transform.
            let off = vec2(self.geom.x, self.geom.y) * self.inst_scale + self.inst_offset;
            transformed = transformed + off;

            let g_uv = unpack2f16(self.geom.uv)
            self.v_tcoord = vec2(g_uv.x, g_uv.y);
            self.v_color = unpack4u8(self.inst_color);
            self.v_stroke_mult = 1e6;
            self.v_stroke_dist = self.geom.stroke_dist * self.map_scale.x;
            self.v_shape_id = 20.0;
            self.v_param0 = 0.0;
            self.v_param1 = off.x;
            self.v_param2 = off.y;
            self.v_param3 = 0.0;
            self.v_param4 = self.inst_param4;
            // Tilt depth bias of a free-standing symbol (ICON_INSTANCE_DEPTH_BIAS).
            self.v_param5 = 0.35;

            let shifted = transformed + self.draw_list.view_shift;
            self.v_world = shifted;
            self.v_screen = transformed - self.rot_pivot + self.shadow_mask_size * 0.5;
            self.v_lift = 1000.0;

            // Clip radius of a symbol (ICON_INSTANCE_CLIP_RADIUS) in view px.
            let cr = 24.0 * max(self.map_scale.x, self.map_scale.y);
            let clip = vec4(
                max(self.draw_clip.x, self.draw_list.view_clip.x - self.draw_list.view_shift.x),
                max(self.draw_clip.y, self.draw_list.view_clip.y - self.draw_list.view_shift.y),
                min(self.draw_clip.z, self.draw_list.view_clip.z - self.draw_list.view_shift.x),
                min(self.draw_clip.w, self.draw_list.view_clip.w - self.draw_list.view_shift.y)
            )
            if transformed.x + cr < clip.x || transformed.y + cr < clip.y
                || transformed.x - cr > clip.z || transformed.y - cr > clip.w {
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 0.0);
                return
            }

            let world = self.draw_list.view_transform * vec4(
                shifted.x
                shifted.y
                self.draw_depth + self.tilt_params.w
                    + mix(
                        self.draw_call.zbias + self.inst_zbias,
                        0.35 + (ground_rel_y + lift_m * self.tilt_params.y) * self.tilt_params.z,
                        sign(self.tilt_params.z)
                    )
                1.
            );
            self.v_world_clip = world;
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world)
        }
    }

    // Instanced building walls: the unit quad is extruded per footprint edge
    // in the vertex shader from an 11-float record (edge a/b, base/top metres,
    // outward normal, bottom AO, colour, zbias). Varyings match the vertices
    // `append_wall_quad` used to bake (fill, material MAT_WALL, height in
    // param4, BUILDING_SURFACE_DEPTH in param5) — keep in LOCKSTEP with
    // DrawMapVector's fill path; the pixel side is inherited unchanged.
    mod.draw.DrawMapWall = mod.std.set_type_default() do #(DrawMapWall::script_shader(vm)){
        ..mod.draw.DrawMapVector
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)

        vertex: fn() {
            let along = self.geom.pos.x;
            let up = self.geom.pos.y;
            let pos = mix(self.inst_a, self.inst_b, along);
            let h = mix(self.inst_heights.x, self.inst_heights.y, up);
            let ao = mix(self.inst_ao, 1.0, up);
            var transformed = pos * self.map_scale + self.map_offset;
            let terrain_pos = transformed;
            var ground_m = 0.0;
            if self.terrain_span.x > 0.5 {
                let tuv = (terrain_pos - self.terrain_org) / self.terrain_span;
                if tuv.x > 0.0 && tuv.x < 1.0 && tuv.y > 0.0 && tuv.y < 1.0 {
                    let fit = tuv * self.terrain_uvfit.xy + self.terrain_uvfit.zw;
                    let enc = self.terrain_tex.sample_lod(fit, 0.0);
                    ground_m = max(
                        enc.x * 65280.0 + enc.y * 255.0 + enc.z * 0.99609375 - 32768.0,
                        0.0
                    );
                }
            }
            let rel = transformed - self.rot_pivot;
            transformed = self.rot_pivot + vec2(
                rel.x * self.view_rot.x - rel.y * self.view_rot.y,
                rel.x * self.view_rot.y + rel.y * self.view_rot.x
            );
            let ground_rel_y = transformed.y - self.rot_pivot.y;
            // Fills ride the terrain by terrain_fill_lift; the height grows
            // with the 2D->3D reveal.
            let lift_m = h * self.height_grow + ground_m * self.terrain_fill_lift;
            if self.space_warp.x > 0.0001 {
                let cos_t = self.tilt_params.x
                let sin_t = self.space_warp.w
                let hpx = lift_m * self.space_warp2.y
                let wg = 0.0 - ground_rel_y
                var wf = wg
                var wu = 0.0
                var wnx = 0.0
                var wny = 1.0
                let wa = wg - self.space_warp.y
                if wa > 0.0 {
                    let wr = max(self.space_warp.z, 1.0)
                    let cap = self.space_warp2.z
                    let th = min(wa / wr, cap)
                    let sth = sin(th)
                    let cth = cos(th)
                    wf = self.space_warp.y + wr * sth
                    wu = wr * (1.0 - cth)
                    let we = wa - wr * cap
                    if we > 0.0 {
                        wf = wf + we * cos_t
                        wu = wu + we * sin_t
                    }
                    wnx = 0.0 - sth
                    wny = cth
                }
                let pf = wf + hpx * wnx
                let pu = wu + hpx * wny
                let bf = wg + (pf - wg) * self.space_warp.x
                let bu = hpx + (pu - hpx) * self.space_warp.x
                let zrel = bf * sin_t - bu * cos_t
                let pw = 1.0 / max(1.0 + self.space_warp2.x * zrel, 0.12)
                transformed = vec2(
                    self.rot_pivot.x + (transformed.x - self.rot_pivot.x) * pw,
                    self.rot_pivot.y - (bf * cos_t + bu * sin_t) * pw
                );
            } else {
                transformed.y = self.rot_pivot.y
                    + ground_rel_y * self.tilt_params.x
                    - lift_m * self.tilt_params.y;
            }

            let color = unpack4u8(self.inst_color);
            self.v_tcoord = vec2(0.5, 1.0);
            self.v_color = vec4(color.x * ao, color.y * ao, color.z * ao, color.w);
            self.v_stroke_mult = 1e6;
            self.v_stroke_dist = 0.0;
            self.v_shape_id = 0.0;
            self.v_param0 = 0.0;
            // Outward normal, material MAT_WALL, height, surface depth: the
            // slots the wall pixel path reads.
            self.v_param1 = self.inst_normal.x;
            self.v_param2 = self.inst_normal.y;
            self.v_param3 = 1.0;
            self.v_param4 = h;
            self.v_param5 = 0.5;

            let shifted = transformed + self.draw_list.view_shift;
            self.v_world = shifted;
            self.v_screen = transformed - self.rot_pivot + self.shadow_mask_size * 0.5;
            self.v_lift = h;

            // Clip radius of a wall vertex (90 units) in view px.
            let cr = 90.0 * max(self.map_scale.x, self.map_scale.y);
            let clip = vec4(
                max(self.draw_clip.x, self.draw_list.view_clip.x - self.draw_list.view_shift.x),
                max(self.draw_clip.y, self.draw_list.view_clip.y - self.draw_list.view_shift.y),
                min(self.draw_clip.z, self.draw_list.view_clip.z - self.draw_list.view_shift.x),
                min(self.draw_clip.w, self.draw_list.view_clip.w - self.draw_list.view_shift.y)
            )
            if transformed.x + cr < clip.x || transformed.y + cr < clip.y
                || transformed.x - cr > clip.z || transformed.y - cr > clip.w {
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 0.0);
                return
            }

            let world = self.draw_list.view_transform * vec4(
                shifted.x
                shifted.y
                self.draw_depth + self.tilt_params.w
                    + mix(
                        self.draw_call.zbias + self.inst_zbias,
                        0.5 + (ground_rel_y + lift_m * self.tilt_params.y) * self.tilt_params.z,
                        sign(self.tilt_params.z)
                    )
                1.
            );
            self.v_world_clip = world;
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world)
        }
    }

    // Instanced building-wall shadow casters: the same 11-float wall
    // records, extruded along the sun on the ground (no building lift).
    mod.draw.DrawMapShadow = mod.std.set_type_default() do #(DrawMapShadow::script_shader(vm)){
        ..mod.draw.DrawMapVector
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)

        vertex: fn() {
            let along = self.geom.pos.x;
            let up = self.geom.pos.y;
            let pos = mix(self.inst_a, self.inst_b, along)
                + self.shadow_dir * self.inst_heights.y * self.height_grow * up;
            var transformed = pos * self.map_scale + self.map_offset;
            let terrain_pos = transformed;
            var ground_m = 0.0;
            if self.terrain_span.x > 0.5 {
                let tuv = (terrain_pos - self.terrain_org) / self.terrain_span;
                if tuv.x > 0.0 && tuv.x < 1.0 && tuv.y > 0.0 && tuv.y < 1.0 {
                    let fit = tuv * self.terrain_uvfit.xy + self.terrain_uvfit.zw;
                    let enc = self.terrain_tex.sample_lod(fit, 0.0);
                    ground_m = max(
                        enc.x * 65280.0 + enc.y * 255.0 + enc.z * 0.99609375 - 32768.0,
                        0.0
                    );
                }
            }
            let rel = transformed - self.rot_pivot;
            transformed = self.rot_pivot + vec2(
                rel.x * self.view_rot.x - rel.y * self.view_rot.y,
                rel.x * self.view_rot.y + rel.y * self.view_rot.x
            );
            let ground_rel_y = transformed.y - self.rot_pivot.y;
            let lift_m = ground_m * self.terrain_fill_lift;
            if self.space_warp.x > 0.0001 {
                let cos_t = self.tilt_params.x
                let sin_t = self.space_warp.w
                let hpx = lift_m * self.space_warp2.y
                let wg = 0.0 - ground_rel_y
                var wf = wg
                var wu = 0.0
                var wnx = 0.0
                var wny = 1.0
                let wa = wg - self.space_warp.y
                if wa > 0.0 {
                    let wr = max(self.space_warp.z, 1.0)
                    let cap = self.space_warp2.z
                    let th = min(wa / wr, cap)
                    let sth = sin(th)
                    let cth = cos(th)
                    wf = self.space_warp.y + wr * sth
                    wu = wr * (1.0 - cth)
                    let we = wa - wr * cap
                    if we > 0.0 {
                        wf = wf + we * cos_t
                        wu = wu + we * sin_t
                    }
                    wnx = 0.0 - sth
                    wny = cth
                }
                let pf = wf + hpx * wnx
                let pu = wu + hpx * wny
                let bf = wg + (pf - wg) * self.space_warp.x
                let bu = hpx + (pu - hpx) * self.space_warp.x
                let zrel = bf * sin_t - bu * cos_t
                let pw = 1.0 / max(1.0 + self.space_warp2.x * zrel, 0.12)
                transformed = vec2(
                    self.rot_pivot.x + (transformed.x - self.rot_pivot.x) * pw,
                    self.rot_pivot.y - (bf * cos_t + bu * sin_t) * pw
                );
            } else {
                transformed.y = self.rot_pivot.y
                    + ground_rel_y * self.tilt_params.x
                    - lift_m * self.tilt_params.y;
            }

            self.v_tcoord = vec2(0.5, 1.0);
            self.v_color = vec4(1.0, 1.0, 1.0, 1.0);
            self.v_stroke_mult = 1e6;
            self.v_stroke_dist = 0.0;
            self.v_shape_id = 0.0;
            self.v_param0 = 0.0;
            self.v_param1 = 0.0;
            self.v_param2 = 0.0;
            self.v_param3 = 0.0;
            self.v_param4 = 0.0;
            self.v_param5 = 0.0;

            let shifted = transformed + self.draw_list.view_shift;
            self.v_world = shifted;
            self.v_screen = transformed - self.rot_pivot + self.shadow_mask_size * 0.5;
            self.v_lift = 0.0;

            let cr = 90.0 * max(self.map_scale.x, self.map_scale.y);
            let clip = vec4(
                max(self.draw_clip.x, self.draw_list.view_clip.x - self.draw_list.view_shift.x),
                max(self.draw_clip.y, self.draw_list.view_clip.y - self.draw_list.view_shift.y),
                min(self.draw_clip.z, self.draw_list.view_clip.z - self.draw_list.view_shift.x),
                min(self.draw_clip.w, self.draw_list.view_clip.w - self.draw_list.view_shift.y)
            )
            if transformed.x + cr < clip.x || transformed.y + cr < clip.y
                || transformed.x - cr > clip.z || transformed.y - cr > clip.w {
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 0.0);
                return
            }

            let world = self.draw_list.view_transform * vec4(
                shifted.x
                shifted.y
                self.draw_depth + self.draw_call.zbias + self.inst_zbias
                1.
            );
            self.v_world_clip = world;
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world)
        }

        fragment: fn() {
            self.fb0 = vec4(1.0, 1.0, 1.0, 1.0)
        }
    }

    // Terrain hillshade: plain textured quad (RGBA baked CPU-side),
    // drawn between the land fills and the road network.
    mod.draw.DrawTerrainOverlay = mod.std.set_type_default() do #(DrawTerrainOverlay::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex: texture_2d(float)
        uv: varying(vec2f)
        depth_on: uniform(0.0)
        opacity_boost: uniform(1.0)

        vertex: fn() {
            let top = mix(self.c0, self.c1, self.geom.pos.x)
            let bottom = mix(self.c3, self.c2, self.geom.pos.x)
            let p = mix(top, bottom, self.geom.pos.y)
            self.uv = mix(self.uv0, self.uv1, self.geom.pos)
            let shifted = p + self.draw_list.view_shift
            let gd = mix(
                mix(self.gdepth.x, self.gdepth.y, self.geom.pos.x),
                mix(self.gdepth.w, self.gdepth.z, self.geom.pos.x),
                self.geom.pos.y
            )
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * (
                self.draw_list.view_transform * vec4(
                    shifted.x,
                    shifted.y,
                    mix(self.draw_depth + self.draw_call.zbias, gd, self.depth_on),
                    1.
                )
            ))
        }

        pixel: fn() {
            let color = self.tex.sample_as_bgra(self.uv)
            let a = min(color.w * self.opacity_boost, 1.0)
            return vec4(color.xyz * a, a)
        }
    }

    // Rain radar raster overlay: one textured quad whose four SCREEN-space
    // corners come from the overlay camera (so it pans/zooms/rotates/tilts
    // with the map); texture is a mercator-aligned RGBA nowcast frame.
    mod.draw.DrawRainOverlay = mod.std.set_type_default() do #(DrawRainOverlay::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex: texture_2d(float)
        // c0..c3 + rain_alpha come from the Rust struct's #[live] fields
        // (auto-registered as instance inputs; declaring them here too
        // collides, as with DrawRotatedText.upright).
        uv: varying(vec2f)

        vertex: fn() {
            let top = mix(self.c0, self.c1, self.geom.pos.x)
            let bottom = mix(self.c3, self.c2, self.geom.pos.x)
            let p = mix(top, bottom, self.geom.pos.y)
            self.uv = self.geom.pos
            let shifted = p + self.draw_list.view_shift
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * (
                self.draw_list.view_transform * vec4(
                    shifted.x,
                    shifted.y,
                    self.draw_depth + self.draw_call.zbias,
                    1.
                )
            ))
        }

        pixel: fn() {
            // The texture stores the CONTINUOUS interpolated radar value
            // (0..255 in .x, coverage in .w); band the field HERE so the
            // isolines are screen-resolution crisp at any zoom.
            let sampled = self.tex.sample_as_bgra(self.uv)
            if sampled.w < 0.5 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let value = sampled.x * 255.0
            if value < 1.0 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let dbz = 0.5 * value - 32.0
            // Band index as a continuous function -> fwidth spikes exactly
            // at band boundaries = 1-2 px contour lines.
            var band = 0.0
            var rgb = vec3(0.59, 0.78, 1.0)
            var alpha = 0.33
            if dbz >= 5.0 { band = 1.0; rgb = vec3(0.35, 0.63, 0.98); alpha = 0.49 }
            if dbz >= 15.0 { band = 2.0; rgb = vec3(0.14, 0.41, 0.92); alpha = 0.63 }
            if dbz >= 25.0 { band = 3.0; rgb = vec3(0.10, 0.67, 0.35); alpha = 0.73 }
            if dbz >= 33.0 { band = 4.0; rgb = vec3(0.96, 0.73, 0.14); alpha = 0.80 }
            if dbz >= 40.0 { band = 5.0; rgb = vec3(0.92, 0.31, 0.12); alpha = 0.88 }
            if dbz >= 47.0 { band = 6.0; rgb = vec3(0.80, 0.10, 0.63); alpha = 0.96 }
            let contour = min(length(vec2(dFdx(band), dFdy(band))) * 1.2, 1.0)
            rgb = mix(rgb, rgb * 0.5, contour)
            alpha = min(alpha + contour * 0.18, 0.97) * self.rain_alpha
            return vec4(rgb * alpha, alpha)
        }
    }

    mod.widgets.MapViewBase = #(MapView::register_widget(vm))

    mod.widgets.MapView = set_type_default() do mod.widgets.MapViewBase{
        width: Fill
        height: Fill
        center_lon: 4.8779
        center_lat: 52.3757
        zoom: 17.0
        min_zoom: 11.0
        // Street-level over-zoom: past the archive's deepest level the
        // renderer scales those tiles (request_zoom_level clamps; geometry
        // is vector so it stays crisp) — z21 is the near-first-person
        // envelope the space-warp mode keys on.
        max_zoom: 21.0
        dark_theme: false
        use_network: false
        use_local_mbtiles: true
        // openstreetmap-carto palette; road widths are carto's z14 stops in
        // screen px, scaled per view-zoom bucket by zoom_width_mult().
        style_light: MapThemeStyle{
            background: #xf2efe9
            status_text: #x444444
            label: #x000000

            // shiny.md defaults (screenshot-reviewed): baked AO + shadow
            // geometry are zero GPU cost; water/foliage are a few ALU on
            // minority pixels. Sheen/glow stay opt-in per app.
            shiny: MapShinyStyle{
                bake_ao: true
                bake_shadows: true
                terrain_shadows: true
                water_fx: true
                foliage_fx: true
            }

            MapFillRule{group: "building" color: #xd9d0c9}
            MapFillRule{group: "building_outline" color: #xb5aa9b}
            MapFillRule{group: "street_area" color: #xdddde8}
            MapFillRule{group: "bridge_area" color: #xb8b8b8}
            MapFillRule{group: "water" color: #xaad3df}
            MapFillRule{group: "landuse" value: "residential" color: #xe0dfdf}
            MapFillRule{group: "landuse" value: "commercial" color: #xf2dad9}
            MapFillRule{group: "landuse" value: "retail" color: #xffd6d1}
            MapFillRule{group: "landuse" value: "industrial" color: #xebdbe8}
            MapFillRule{group: "landuse" value: "forest" color: #xadd19e}
            MapFillRule{group: "landuse" value: "grass" color: #xcdebb0}
            MapFillRule{group: "landuse" value: "meadow" color: #xcdebb0}
            MapFillRule{group: "landuse" value: "farmland" color: #xeef0d5}
            MapFillRule{group: "landuse" value: "railway" color: #xece7f1}
            MapFillRule{group: "landuse" value: "cemetery" color: #xaacbaf}
            MapFillRule{group: "landuse" value: "sand" color: #xf2e9cf}
            MapFillRule{group: "landuse" value: "*" color: #xe8e7e2}
            MapFillRule{group: "leisure" value: "park" color: #xc8facc}
            MapFillRule{group: "leisure" value: "garden" color: #xcdebb0}
            MapFillRule{group: "leisure" value: "golf_course" color: #xdef6c0}
            MapFillRule{group: "leisure" value: "pitch" color: #x88e0be}
            MapFillRule{group: "leisure" value: "*" color: #xc8facc}

            MapRoadRule{kind: "motorway" sort_rank: 700 casing_color: #xdc2a67 casing_width: 7.2 center_color: #xe892a2 center_width: 6.0}
            MapRoadRule{kind: "trunk" sort_rank: 640 casing_color: #xc84e2f casing_width: 7.2 center_color: #xf9b29c center_width: 6.0}
            MapRoadRule{kind: "primary" sort_rank: 560 casing_color: #xa06b00 casing_width: 6.4 center_color: #xfcd6a4 center_width: 5.0}
            MapRoadRule{kind: "secondary" sort_rank: 470 casing_color: #x707d05 casing_width: 6.4 center_color: #xf7fabf center_width: 5.0}
            MapRoadRule{kind: "busway" sort_rank: 470 casing_color: #x707d05 casing_width: 6.4 center_color: #xf7fabf center_width: 5.0}
            MapRoadRule{kind: "tertiary" sort_rank: 390 casing_color: #x8f8f8f casing_width: 6.2 center_color: #xffffff center_width: 5.0}
            MapRoadRule{kind: "residential" sort_rank: 310 casing_color: #xbbbbbb casing_width: 4.2 center_color: #xffffff center_width: 3.0}
            MapRoadRule{kind: "unclassified" sort_rank: 310 casing_color: #xbbbbbb casing_width: 4.2 center_color: #xffffff center_width: 3.0}
            MapRoadRule{kind: "living_street" sort_rank: 310 casing_color: #xbbbbbb casing_width: 4.0 center_color: #xededed center_width: 3.0}
            MapRoadRule{kind: "service" sort_rank: 240 casing_color: #xbbbbbb casing_width: 3.0 center_color: #xffffff center_width: 2.0}
            MapRoadRule{kind: "pedestrian" sort_rank: 240 casing_color: #x999999 casing_width: 4.0 center_color: #xdddde8 center_width: 3.0}
            MapRoadRule{kind: "pedestrian" sort_rank: 300 casing_color: #xb5b5b5 casing_width: 4.0 center_color: #xfdfdfd center_width: 2.8 min_zoom: 14.0}
            MapRoadRule{kind: "cycleway" sort_rank: 160 center_color: #x6262ff center_width: 0.9 center_shape_id: 10.0 min_zoom: 14.0}
            MapRoadRule{kind: "footway" sort_rank: 160 center_color: #xaaa8a5 center_width: 0.9 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "path" sort_rank: 160 center_color: #xaaa8a5 center_width: 0.8 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "steps" sort_rank: 160 center_color: #xaaa8a5 center_width: 2.0 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "track" sort_rank: 160 center_color: #xaaa8a5 center_width: 1.0 center_shape_id: 10.0 min_zoom: 14.0}
            MapRoadRule{kind: "*" sort_rank: 280 casing_color: #xbbbbbb casing_width: 3.6 center_color: #xffffff center_width: 2.5}

            MapWaterwayRule{kind: "river" sort_rank: 140 center_color: #xaad3df center_width: 4.0}
            MapWaterwayRule{kind: "canal" sort_rank: 140 center_color: #xaad3df center_width: 3.0 min_zoom: 12.0}
            MapWaterwayRule{kind: "stream" sort_rank: 140 center_color: #xaad3df center_width: 1.4 min_zoom: 13.0}
            MapWaterwayRule{kind: "*" sort_rank: 140 center_color: #xaad3df center_width: 1.2 min_zoom: 13.0}
            MapRailRule{sort_rank: 710 center_color: #x6e6e6e center_width: 1.0}
        }
        style_dark: MapThemeStyle{
            background: #x161b22
            status_text: #xb2c7d8
            label: #xe5eaf1
            label_halo: #x161b22

            shiny: MapShinyStyle{
                bake_ao: true
                // No sun-cast shadows at night; baked AO keeps the
                // buildings grounded.
                water_fx: true
                foliage_fx: true
                // Dark volumes + glossy highlights read as the miniature
                // look; the sheen carries the dark theme.
                building_sheen: true
            }

            MapFillRule{group: "building" color: #x383d46}
            MapFillRule{group: "building_outline" color: #x262a31}
            MapFillRule{group: "tree_canopy" color: #x2c4a33}
            MapFillRule{group: "tree_trunk" color: #x3a2f24}
            MapFillRule{group: "street_area" color: #x3a3f4a}
            MapFillRule{group: "bridge_area" color: #x3a3f47}
            MapFillRule{group: "water" color: #x204f74}
            MapFillRule{group: "landuse" value: "residential" color: #x2a2f36}
            MapFillRule{group: "landuse" value: "commercial" color: #x30343b}
            MapFillRule{group: "landuse" value: "retail" color: #x30343b}
            MapFillRule{group: "landuse" value: "industrial" color: #x282c32}
            MapFillRule{group: "landuse" value: "forest" color: #x243629}
            MapFillRule{group: "landuse" value: "grass" color: #x2a3c2d}
            MapFillRule{group: "landuse" value: "meadow" color: #x2a3c2d}
            MapFillRule{group: "landuse" value: "farmland" color: #x2a3c2d}
            MapFillRule{group: "landuse" value: "railway" color: #x2f2b36}
            MapFillRule{group: "landuse" value: "cemetery" color: #x2b3a2f}
            MapFillRule{group: "landuse" value: "sand" color: #x3a362c}
            MapFillRule{group: "landuse" value: "*" color: #x2d3239}
            MapFillRule{group: "leisure" value: "park" color: #x2f4a34}
            MapFillRule{group: "leisure" value: "garden" color: #x2f4a34}
            MapFillRule{group: "leisure" value: "golf_course" color: #x2f4a34}
            MapFillRule{group: "leisure" value: "pitch" color: #x32553a}
            MapFillRule{group: "leisure" value: "*" color: #x2b4230}

            MapRoadRule{kind: "motorway" sort_rank: 700 casing_color: #x8f6937 casing_width: 7.2 center_color: #xd29b54 center_width: 6.0}
            MapRoadRule{kind: "trunk" sort_rank: 640 casing_color: #x8c7141 casing_width: 7.2 center_color: #xc8a561 center_width: 6.0}
            MapRoadRule{kind: "primary" sort_rank: 560 casing_color: #x706857 casing_width: 6.4 center_color: #xb9aa86 center_width: 5.0}
            MapRoadRule{kind: "secondary" sort_rank: 470 casing_color: #x556170 casing_width: 6.4 center_color: #x95a1b1 center_width: 5.0}
            MapRoadRule{kind: "busway" sort_rank: 470 casing_color: #x556170 casing_width: 6.4 center_color: #x95a1b1 center_width: 5.0}
            MapRoadRule{kind: "tertiary" sort_rank: 390 casing_color: #x4b5765 casing_width: 6.2 center_color: #x7d899a center_width: 5.0}
            MapRoadRule{kind: "residential" sort_rank: 310 casing_color: #x404a57 casing_width: 4.2 center_color: #x677383 center_width: 3.0}
            MapRoadRule{kind: "unclassified" sort_rank: 310 casing_color: #x404a57 casing_width: 4.2 center_color: #x677383 center_width: 3.0}
            MapRoadRule{kind: "living_street" sort_rank: 310 casing_color: #x404a57 casing_width: 4.0 center_color: #x677383 center_width: 3.0}
            MapRoadRule{kind: "service" sort_rank: 240 casing_color: #x3e4753 casing_width: 3.0 center_color: #x5e6a79 center_width: 2.0}
            MapRoadRule{kind: "pedestrian" sort_rank: 240 casing_color: #x3e4753 casing_width: 4.0 center_color: #x5e6a79 center_width: 3.0}
            MapRoadRule{kind: "pedestrian" sort_rank: 300 casing_color: #x3c424a casing_width: 4.0 center_color: #x272b31 center_width: 2.8 min_zoom: 14.0}
            MapRoadRule{kind: "cycleway" sort_rank: 160 center_color: #x4f5966 center_width: 0.9 center_shape_id: 10.0 min_zoom: 14.0}
            MapRoadRule{kind: "footway" sort_rank: 160 center_color: #x4f5966 center_width: 0.9 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "path" sort_rank: 160 center_color: #x4f5966 center_width: 0.8 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "steps" sort_rank: 160 center_color: #x4f5966 center_width: 2.0 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "track" sort_rank: 160 center_color: #x4f5966 center_width: 1.0 center_shape_id: 10.0 min_zoom: 14.0}
            MapRoadRule{kind: "*" sort_rank: 280 casing_color: #x404a57 casing_width: 3.6 center_color: #x606c7b center_width: 2.5}

            MapWaterwayRule{kind: "river" sort_rank: 140 center_color: #x204f74 center_width: 4.0}
            MapWaterwayRule{kind: "canal" sort_rank: 140 center_color: #x204f74 center_width: 3.0 min_zoom: 12.0}
            MapWaterwayRule{kind: "stream" sort_rank: 140 center_color: #x204f74 center_width: 1.4 min_zoom: 13.0}
            MapWaterwayRule{kind: "*" sort_rank: 140 center_color: #x204f74 center_width: 1.2 min_zoom: 13.0}
            MapRailRule{sort_rank: 710 center_color: #x8a919d center_width: 1.0}
        }
        // "Circuit City" (shiny.md showcase): the sci-fi night board —
        // near-black ground, matte charcoal volumes with a hard specular
        // sheen, dark reflective water, and every road an emissive amber
        // filament, class-weighted in width and brightness.
        style_circuit: MapThemeStyle{
            background: #x05070a
            status_text: #x76879a
            label: #x8fa3ba
            label_halo: #x05070a

            shiny: MapShinyStyle{
                bake_ao: true
                // Night board: no sun, no cast shadows — the glow does
                // the depth work.
                water_fx: true
                foliage_fx: true
                building_sheen: true
                gloss: 1.1
                route_glow: true
            }

            MapFillRule{group: "building" color: #x15181d}
            MapFillRule{group: "building_outline" color: #x05070a}
            MapFillRule{group: "tree_canopy" color: #x18332a}
            MapFillRule{group: "tree_trunk" color: #x1c1712}
            MapFillRule{group: "street_area" color: #x0c0f13}
            MapFillRule{group: "bridge_area" color: #x111419}
            MapFillRule{group: "water" color: #x071019}
            MapFillRule{group: "landuse" value: "residential" color: #x090c10}
            MapFillRule{group: "landuse" value: "commercial" color: #x0b0d12}
            MapFillRule{group: "landuse" value: "retail" color: #x0b0d12}
            MapFillRule{group: "landuse" value: "industrial" color: #x0a0c10}
            MapFillRule{group: "landuse" value: "forest" color: #x0a1410}
            MapFillRule{group: "landuse" value: "grass" color: #x0c1712}
            MapFillRule{group: "landuse" value: "meadow" color: #x0c1712}
            MapFillRule{group: "landuse" value: "farmland" color: #x0b100d}
            MapFillRule{group: "landuse" value: "railway" color: #x0c0e14}
            MapFillRule{group: "landuse" value: "cemetery" color: #x0b1410}
            MapFillRule{group: "landuse" value: "sand" color: #x14120c}
            MapFillRule{group: "landuse" value: "*" color: #x0a0d11}
            MapFillRule{group: "leisure" value: "park" color: #x0c1a12}
            MapFillRule{group: "leisure" value: "garden" color: #x0c1712}
            MapFillRule{group: "leisure" value: "golf_course" color: #x0c1712}
            MapFillRule{group: "leisure" value: "pitch" color: #x0e1f16}
            MapFillRule{group: "leisure" value: "*" color: #x0c1a12}

            MapRoadRule{kind: "motorway" sort_rank: 700 center_color: #xffc76a center_width: 6.0 emissive: 1.0}
            MapRoadRule{kind: "trunk" sort_rank: 640 center_color: #xffb95c center_width: 6.0 emissive: 0.95}
            MapRoadRule{kind: "primary" sort_rank: 560 center_color: #xffab4e center_width: 5.0 emissive: 0.85}
            MapRoadRule{kind: "secondary" sort_rank: 470 center_color: #xf29a45 center_width: 5.0 emissive: 0.7}
            MapRoadRule{kind: "busway" sort_rank: 470 center_color: #xf29a45 center_width: 5.0 emissive: 0.7}
            MapRoadRule{kind: "tertiary" sort_rank: 390 center_color: #xd9883f center_width: 5.0 emissive: 0.6}
            MapRoadRule{kind: "residential" sort_rank: 310 center_color: #xa8763f center_width: 3.0 emissive: 0.45}
            MapRoadRule{kind: "unclassified" sort_rank: 310 center_color: #xa8763f center_width: 3.0 emissive: 0.45}
            MapRoadRule{kind: "living_street" sort_rank: 310 center_color: #x976b3b center_width: 3.0 emissive: 0.4}
            MapRoadRule{kind: "service" sort_rank: 240 center_color: #x6e5433 center_width: 2.0 emissive: 0.3}
            MapRoadRule{kind: "pedestrian" sort_rank: 240 center_color: #x4c4f58 center_width: 3.0 emissive: 0.15}
            MapRoadRule{kind: "cycleway" sort_rank: 160 center_color: #x2e6f96 center_width: 0.9 center_shape_id: 10.0 min_zoom: 14.0 emissive: 0.35}
            MapRoadRule{kind: "footway" sort_rank: 160 center_color: #x474f58 center_width: 0.9 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "path" sort_rank: 160 center_color: #x474f58 center_width: 0.8 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "steps" sort_rank: 160 center_color: #x474f58 center_width: 2.0 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "track" sort_rank: 160 center_color: #x474f58 center_width: 1.0 center_shape_id: 10.0 min_zoom: 14.0}
            MapRoadRule{kind: "*" sort_rank: 280 center_color: #x8a6b3f center_width: 2.5 emissive: 0.4}

            MapWaterwayRule{kind: "river" sort_rank: 140 center_color: #x0a2030 center_width: 4.0}
            MapWaterwayRule{kind: "canal" sort_rank: 140 center_color: #x0a2030 center_width: 3.0 min_zoom: 12.0}
            MapWaterwayRule{kind: "stream" sort_rank: 140 center_color: #x0a2030 center_width: 1.4 min_zoom: 13.0}
            MapWaterwayRule{kind: "*" sort_rank: 140 center_color: #x0a2030 center_width: 1.2 min_zoom: 13.0}
            MapRailRule{sort_rank: 710 center_color: #x424c5c center_width: 1.0}
        }

        draw_bg +: {
            color: #xf2efe9
        }
        draw_label +: {
            color: #x000000
            text_style: theme.font_regular{font_size: 7}
        }
        draw_text +: {
            color: #xdee9f4
            text_style: theme.font_regular{font_size: 10}
        }
    }
}

/// Frames after the last zoom change before stale-bucket tiles restyle
/// (~0.3s at 60fps).
const ZOOM_SETTLE_SECONDS: f64 = 0.08;
/// Frames before an archive-absent tile is probed again (~30 s at 60 fps).
const MISSING_RECHECK_FRAMES: u64 = 1800;
const ARCHIVE_REQUEST_TIMEOUT_SECONDS: f64 = 10.0;

fn tile_screen_priority(
    key: TileKey,
    zoom: u32,
    center_norm: Vec2d,
    rotation: (f64, f64),
    tilt_cos: f64,
) -> u64 {
    let tile_count = (1_u64 << zoom.min(30)) as f64;
    let center = center_norm * tile_count;
    let mut dx = key.x as f64 + 0.5 - center.x;
    if dx > tile_count * 0.5 {
        dx -= tile_count;
    } else if dx < -tile_count * 0.5 {
        dx += tile_count;
    }
    let dy = key.y as f64 + 0.5 - center.y;
    let sx = dx * rotation.0 - dy * rotation.1;
    let sy = (dx * rotation.1 + dy * rotation.0) * tilt_cos.clamp(0.001, 1.0);
    ((sx * sx + sy * sy) * 4096.0).max(0.0) as u64
}

fn sort_tiles_center_out(
    tiles: &mut [TileKey],
    zoom: u32,
    center_norm: Vec2d,
    rotation: (f64, f64),
    tilt_cos: f64,
) {
    tiles.sort_unstable_by_key(|key| {
        (
            tile_screen_priority(*key, zoom, center_norm, rotation, tilt_cos),
            key.y,
            key.x,
        )
    });
}

/// Camera tilt ceiling (degrees from top-down). Flat enough to read
/// terrain relief against the horizon without the far plane exploding.
/// This is the BASE cap — the near-ground regime (street-level zoom)
/// unlocks up to TILT_HARD_MAX_DEG via `tilt_max_deg_now()`.
const TILT_MAX_DEG: f64 = 78.0;
/// Absolute tilt ceiling, reachable only at street-level zoom where the
/// visible ground fan is a handful of over-zoomed tiles (the honest
/// 1/cos(tilt) culling reach stays cheap there). Every math-side clamp
/// uses THIS so a legally-steep camera is never silently truncated.
const TILT_HARD_MAX_DEG: f64 = 85.0;
/// Accumulated pan (screen px) before labels are re-placed; must stay under
/// LABEL_VIEW_MARGIN so cached placements keep covering the viewport edge.
// Pure pan shifts the cached placement affinely, so mid-gesture full
// re-places are almost pure waste: every 48px+125ms the collision pass
// (5-20ms) hitched the frame and let label winners flicker — the "labels
// pan async from the map" feel. Ride the cache for most of a viewport
// and re-place at rest (the gesture-end timeout below always fires one).
const LABEL_REPLACE_PAN_PX: f64 = 420.0;
/// Minimum frames between full label re-placements while the cached
/// placement is still usable (a full place costs up to ~20ms — 2-3 dropped
/// frames at 120Hz — and tile arrivals during panning invalidated the cache
/// almost every other frame).
const LABEL_REPLACE_MIN_SECONDS: f64 = 0.30;
/// How long the camera (rotation/tilt/zoom/warp tween) must be QUIET before
/// a full label re-place runs. While it moves, cached labels ride the GPU
/// camera+fold transforms exactly — regenerating mid-gesture cost 5-20ms a
/// beat and landed a frame stale, which read as labels trailing the map.
const LABEL_SETTLE_SECONDS: f64 = 0.15;
/// One shared time-lapse for ALL weather layers: the rain nowcast frame
/// rate AND the wind-particle advection derive from it, so cloud drift and
/// wind streaks move as one physical system (900x real time).
const WEATHER_TIMELAPSE: f64 = 900.0;
/// Rain nowcast frames are 5 minutes of real weather apart.
const RAIN_FRAME_REAL_SECONDS: f64 = 300.0;
/// Cross-fade duration when a tile's new geometry replaces the old.
const TILE_FADE_SECONDS: f64 = 0.25;
/// Hard time budget for one placement pass; labels that don't make it are
/// picked up by the next re-place.
const LABEL_PLACE_BUDGET_MS: f64 = 7.0;

/// Inclusive tile-index span covering the half-open world-space interval
/// `[world_min, world_max)` plus one prefetch tile on either side.
fn tile_span_with_prefetch(world_min: f64, world_max: f64) -> (i32, i32) {
    (
        (world_min / TILE_SIZE).floor() as i32 - 1,
        (world_max / TILE_SIZE).ceil() as i32,
    )
}

// --- Actions ---

/// Widget actions emitted by MapView; the app layer builds search, routing
/// and navigation UX on top of these plus the camera/overlay API.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum MapViewAction {
    /// Camera settled after a gesture, fly-to or programmatic move.
    ViewportChanged {
        lon: f64,
        lat: f64,
        zoom: f64,
    },
    /// Camera tilt changed via the rotate/tilt gesture — lets the app keep
    /// its camera controls in sync with manual tilting.
    TiltChanged {
        tilt: f64,
    },
    /// A charger pin was tapped: position + the attributes we know.
    PinTapped {
        lon: f64,
        lat: f64,
        info: Vec<(String, String)>,
    },
    /// Finger up without drag or long-press, not on a marker.
    Tapped {
        lon: f64,
        lat: f64,
        abs: Vec2d,
    },
    LongPressed {
        lon: f64,
        lat: f64,
        abs: Vec2d,
    },
    MarkerClicked {
        id: u64,
    },
    #[default]
    None,
}

/// Animated camera flight (zoom-out-then-in arc when the target is far).
#[derive(Clone, Copy)]
struct FlyTo {
    started: f64,
    duration: f64,
    from_center: Vec2d,
    to_center: Vec2d,
    from_zoom: f64,
    to_zoom: f64,
    arc: f64,
}

// --- Draw shaders ---

const WIND_TRAIL: usize = 22;

#[derive(Clone)]
struct WindParticle {
    /// Ring buffer of recent positions; head = newest.
    history: Vec<Vec2d>,
    age: u32,
    speed: f32,
}

impl WindParticle {
    fn spawn(pos: Vec2d, age: u32) -> Self {
        Self {
            history: vec![pos; 1],
            age,
            speed: 0.0,
        }
    }
    fn head(&self) -> Vec2d {
        *self.history.last().unwrap()
    }
    fn push(&mut self, pos: Vec2d) {
        if self.history.len() >= WIND_TRAIL {
            self.history.remove(0);
        }
        self.history.push(pos);
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTerrainOverlay {
    #[deref]
    pub draw_super: DrawQuad,
    #[live(vec2(0.0, 0.0))]
    pub c0: Vec2f,
    #[live(vec2(0.0, 0.0))]
    pub c1: Vec2f,
    #[live(vec2(0.0, 0.0))]
    pub c2: Vec2f,
    #[live(vec2(0.0, 0.0))]
    pub c3: Vec2f,
    #[live(vec2(0.0, 0.0))]
    pub uv0: Vec2f,
    #[live(vec2(1.0, 1.0))]
    pub uv1: Vec2f,
    /// Tilt-mode depth per corner (pre-lift ground-y formula, matching the
    /// tile shader): x=c0 y=c1 z=c2 w=c3. Unused when depth_on is 0.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub gdepth: Vec4f,
}

/// One terrain worker render: shaded hillshade texels plus the elevation
/// grid (GPU texels + CPU meters) that drives 3D displacement.
#[derive(Default)]
pub struct TerrainOverlayData {
    pub texels: Vec<u32>,
    pub width: usize,
    pub height: usize,
    pub elev_texels: Vec<u32>,
    pub elev: Vec<f32>,
    pub elev_width: usize,
    pub elev_height: usize,
    pub bbox: (f64, f64, f64, f64),
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawRainOverlay {
    #[deref]
    pub draw_super: DrawQuad,
    #[live(vec2(0.0, 0.0))]
    pub c0: Vec2f,
    #[live(vec2(0.0, 0.0))]
    pub c1: Vec2f,
    #[live(vec2(0.0, 0.0))]
    pub c2: Vec2f,
    #[live(vec2(0.0, 0.0))]
    pub c3: Vec2f,
    #[live(0.85)]
    pub rain_alpha: f32,
    #[live(vec2(0.0015625, 0.00125))]
    pub texel: Vec2f,
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawMapVector {
    #[rust(vec2(1.0, 1.0))]
    pub map_scale: Vec2f,
    #[rust(vec2(0.0, 0.0))]
    pub map_offset: Vec2f,
    #[rust(1.0)]
    pub tile_fade: f32,
    /// shiny.md gates + SceneSun, stamped once per frame from the active
    /// theme; draw_geometry feeds them to the shader uniforms.
    #[rust(ShinyConfig::default())]
    pub shiny: ShinyConfig,
    #[rust]
    shadow_mask: Option<Texture>,
    #[rust([0.0, 0.0])]
    shadow_dir: [f32; 2],
    #[rust(0.0)]
    shadow_cast: f32,
    #[rust(0.0)]
    shadow_mask_on: f32,
    #[rust([1.0, 1.0])]
    shadow_mask_size: [f32; 2],
    #[rust(0.0)]
    shadow_mask_flip: f32,
    #[rust([0.0, 0.0, 0.0, 0.0])]
    space_warp_u: [f32; 4],
    #[rust([0.0, 0.0, 0.0, 0.0])]
    space_warp2_u: [f32; 4],
    #[deref]
    pub draw_super: DrawVector,
    /// Per-instance placement of a shared mesh (street trees): added to
    /// every vertex position before the map transform. Zero for the
    /// ordinary one-instance tile geometry draws.
    #[live]
    pub inst_anchor: Vec2f,
    /// Per-instance zbias shift on top of the vertex's own.
    #[live]
    pub inst_zbias: f32,
}

impl DrawMapVector {
    #[allow(clippy::too_many_arguments)]
    fn draw_geometry(
        &mut self,
        cx: &mut Cx2d,
        geometry_id: GeometryId,
        map_scale: Vec2f,
        map_offset: Vec2f,
        fade: f32,
        width_correction: [f32; 4],
        view_rot: [f32; 2],
        rot_pivot: [f32; 2],
        tilt_params: [f32; 4],
        icon_zoom: f32,
        height_grow: f32,
        terrain_org: [f32; 2],
        terrain_span: [f32; 2],
        terrain_uvfit: [f32; 4],
        terrain_tex: &Texture,
        pass_depth: f32,
        terrain_fill_lift: f32,
    ) {
        self.draw_super.draw_depth = pass_depth;
        self.map_scale = map_scale;
        self.map_offset = map_offset;
        self.tile_fade = fade;
        stamp_map_uniforms(
            &mut self.draw_super.draw_vars,
            cx.cx,
            &MapDrawUniforms {
                map_scale,
                map_offset,
                fade,
                width_correction,
                view_rot,
                rot_pivot,
                tilt_params,
                icon_zoom,
                height_grow,
                terrain_org,
                terrain_span,
                terrain_uvfit,
                terrain_fill_lift,
                shadow_dir: self.shadow_dir,
                shadow_cast: self.shadow_cast,
                shadow_mask_on: self.shadow_mask_on,
                shadow_mask_size: self.shadow_mask_size,
                shadow_mask_flip: self.shadow_mask_flip,
                space_warp: self.space_warp_u,
                space_warp2: self.space_warp2_u,
            },
            &self.shiny,
            terrain_tex,
            self.shadow_mask.as_ref(),
        );
        self.draw_super.draw_vars.geometry_id = Some(geometry_id);
        cx.new_draw_call(&self.draw_super.draw_vars);
        if self.draw_super.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_super.draw_vars);
            self.draw_super.draw_vars.area =
                cx.update_area_refs(self.draw_super.draw_vars.area, new_area);
        }
    }
}

impl DrawMapVector {
    /// Draw one shared mesh once per record (`TREE_INSTANCE_FLOATS`: anchor
    /// xy, zbias shift), with the same uniforms as `draw_geometry`.
    #[allow(clippy::too_many_arguments)]
    fn draw_instanced(
        &mut self,
        cx: &mut Cx2d,
        geometry_id: GeometryId,
        records: &[f32],
        uniforms: &MapDrawUniforms,
        terrain_tex: &Texture,
        pass_depth: f32,
    ) {
        if records.is_empty() || self.draw_super.draw_vars.draw_shader_id.is_none() {
            return;
        }
        self.draw_super.draw_depth = pass_depth;
        self.map_scale = uniforms.map_scale;
        self.map_offset = uniforms.map_offset;
        self.tile_fade = uniforms.fade;
        stamp_map_uniforms(
            &mut self.draw_super.draw_vars,
            cx.cx,
            uniforms,
            &self.shiny,
            terrain_tex,
            self.shadow_mask.as_ref(),
        );
        self.draw_super.draw_vars.geometry_id = Some(geometry_id);
        cx.new_draw_call(&self.draw_super.draw_vars);
        let Some(mut instances) = cx.begin_many_aligned_instances(&self.draw_super.draw_vars)
        else {
            return;
        };
        for record in records.chunks_exact(TREE_INSTANCE_FLOATS) {
            self.inst_anchor = vec2(record[0], record[1]);
            self.inst_zbias = record[2];
            instances
                .instances
                .extend_from_slice(self.draw_super.draw_vars.as_slice());
        }
        self.inst_anchor = vec2(0.0, 0.0);
        self.inst_zbias = 0.0;
        let new_area = cx.end_many_instances(instances);
        self.draw_super.draw_vars.area =
            cx.update_area_refs(self.draw_super.draw_vars.area, new_area);
    }
}

/// Road-only draw path backed by `RoadVertexPacked`. It deliberately has no
/// per-placement instance inputs: each tile road mesh is one ordinary draw.
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawMapRoad {
    #[rust(ShinyConfig::default())]
    pub shiny: ShinyConfig,
    /// Mirrors of DrawMapVector's per-frame shadow / space-warp state; MapView
    /// copies them before every road draw so both shaders see one view.
    #[rust]
    shadow_mask: Option<Texture>,
    #[rust([0.0, 0.0])]
    shadow_dir: [f32; 2],
    #[rust(0.0)]
    shadow_cast: f32,
    #[rust(0.0)]
    shadow_mask_on: f32,
    #[rust([1.0, 1.0])]
    shadow_mask_size: [f32; 2],
    #[rust(0.0)]
    shadow_mask_flip: f32,
    #[rust([0.0, 0.0, 0.0, 0.0])]
    space_warp_u: [f32; 4],
    #[rust([0.0, 0.0, 0.0, 0.0])]
    space_warp2_u: [f32; 4],
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub draw_clip: Vec4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(0.0)]
    pub draw_depth: f32,
}

impl DrawMapRoad {
    #[allow(clippy::too_many_arguments)]
    fn draw_geometry(
        &mut self,
        cx: &mut Cx2d,
        geometry_id: GeometryId,
        map_scale: Vec2f,
        map_offset: Vec2f,
        fade: f32,
        width_correction: [f32; 4],
        view_rot: [f32; 2],
        rot_pivot: [f32; 2],
        tilt_params: [f32; 4],
        icon_zoom: f32,
        height_grow: f32,
        terrain_org: [f32; 2],
        terrain_span: [f32; 2],
        terrain_uvfit: [f32; 4],
        terrain_tex: &Texture,
        pass_depth: f32,
        terrain_fill_lift: f32,
    ) {
        self.draw_depth = pass_depth;
        stamp_map_uniforms(
            &mut self.draw_vars,
            cx.cx,
            &MapDrawUniforms {
                map_scale,
                map_offset,
                fade,
                width_correction,
                view_rot,
                rot_pivot,
                tilt_params,
                icon_zoom,
                height_grow,
                terrain_org,
                terrain_span,
                terrain_uvfit,
                terrain_fill_lift,
                shadow_dir: self.shadow_dir,
                shadow_cast: self.shadow_cast,
                shadow_mask_on: self.shadow_mask_on,
                shadow_mask_size: self.shadow_mask_size,
                shadow_mask_flip: self.shadow_mask_flip,
                space_warp: self.space_warp_u,
                space_warp2: self.space_warp2_u,
            },
            &self.shiny,
            terrain_tex,
            self.shadow_mask.as_ref(),
        );
        self.draw_vars.geometry_id = Some(geometry_id);
        cx.new_draw_call(&self.draw_vars);
        if self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}

/// Casing / stroke / fringe geometry is on the 8-slot road layout and must
/// go through `DrawMapRoad`; everything else through `DrawMapVector`. Both
/// take the same argument list. A macro rather than a method: the call sites
/// hold a borrow of `self.tiles`, so only field-level borrows of `self` work.
macro_rules! draw_map_or_road {
    ($self:ident, $road:expr, $($arg:expr),* $(,)?) => {
        if $road {
            $self.draw_road.draw_geometry($($arg),*)
        } else {
            $self.draw_map.draw_geometry($($arg),*)
        }
    };
}

/// The per-draw view state every map shader stamps as uniforms: one struct
/// so the vector and the instanced-icon draws cannot drift apart.
#[derive(Clone, Copy)]
pub(crate) struct MapDrawUniforms {
    pub map_scale: Vec2f,
    pub map_offset: Vec2f,
    pub fade: f32,
    pub width_correction: [f32; 4],
    pub view_rot: [f32; 2],
    pub rot_pivot: [f32; 2],
    pub tilt_params: [f32; 4],
    pub icon_zoom: f32,
    pub height_grow: f32,
    pub terrain_org: [f32; 2],
    pub terrain_span: [f32; 2],
    pub terrain_uvfit: [f32; 4],
    pub terrain_fill_lift: f32,
    pub shadow_dir: [f32; 2],
    pub shadow_cast: f32,
    pub shadow_mask_on: f32,
    pub shadow_mask_size: [f32; 2],
    pub shadow_mask_flip: f32,
    pub space_warp: [f32; 4],
    pub space_warp2: [f32; 4],
}

/// Orientation of the shadow-mask pass texture when the ground shader samples
/// it by screen position. The pass is a child pass at the window's dpi over
/// the map rect; on Metal it comes back bottom-up (grab-verified: unflipped,
/// every shadow lands on the far side of its building and hides behind it).
fn shadow_mask_y_flip_for_os(os_type: &OsType) -> f32 {
    match os_type {
        OsType::Macos | OsType::Ios(_) => 1.0,
        _ => 0.0,
    }
}

fn stamp_map_uniforms(
    draw_vars: &mut DrawVars,
    cx: &Cx,
    u: &MapDrawUniforms,
    shiny: &ShinyConfig,
    terrain_tex: &Texture,
    shadow_mask: Option<&Texture>,
) {
    draw_vars.set_uniform(cx, live_id!(tile_fade), &[u.fade]);
    draw_vars.set_uniform(cx, live_id!(map_scale), &[u.map_scale.x, u.map_scale.y]);
    draw_vars.set_uniform(cx, live_id!(map_offset), &[u.map_offset.x, u.map_offset.y]);
    draw_vars.set_uniform(cx, live_id!(width_correction), &u.width_correction);
    // Face variant, clamped >= 1: union faces may only WIDEN (inward morph
    // inverts narrow features); stale cross-band tiles render at keyframe
    // width magnified instead of garbling.
    let face_correction: [f32; 4] = [
        u.width_correction[0].max(1.0),
        u.width_correction[1].max(1.0),
        u.width_correction[2].max(1.0),
        u.width_correction[3].max(1.0),
    ];
    draw_vars.set_uniform(cx, live_id!(face_correction), &face_correction);
    draw_vars.set_uniform(cx, live_id!(view_rot), &u.view_rot);
    draw_vars.set_uniform(cx, live_id!(rot_pivot), &u.rot_pivot);
    draw_vars.set_uniform(cx, live_id!(tilt_params), &u.tilt_params);
    draw_vars.set_uniform(cx, live_id!(icon_zoom), &[u.icon_zoom]);
    draw_vars.set_uniform(cx, live_id!(height_grow), &[u.height_grow]);
    draw_vars.set_uniform(cx, live_id!(terrain_org), &u.terrain_org);
    draw_vars.set_uniform(cx, live_id!(terrain_span), &u.terrain_span);
    draw_vars.set_uniform(cx, live_id!(terrain_uvfit), &u.terrain_uvfit);
    draw_vars.set_uniform(cx, live_id!(terrain_fill_lift), &[u.terrain_fill_lift]);
    draw_vars.set_uniform(
        cx,
        live_id!(shiny_gates),
        &[
            if shiny.water_fx { 1.0 } else { 0.0 },
            if shiny.building_sheen { shiny.gloss } else { 0.0 },
            if shiny.foliage_fx { 1.0 } else { 0.0 },
            if shiny.route_glow { 1.0 } else { 0.0 },
        ],
    );
    // Water/green noise anchors physically to the map: scale the baked
    // view-px UV by exp2(16 - view_zoom) so ripple size tracks meters, not
    // screen pixels. The lower bound keeps refining well past z20 (the
    // shaders gate their finest octaves on this value); the upper bound
    // keeps far-out zooms from going sub-pixel.
    let mat_uv_scale = (16.0 - u.icon_zoom).exp2().clamp(0.03, 1.25);
    // Wide-range variant for patterns that stay physical further out
    // (shrub fills) and want to know the true zoom for LOD blending.
    let mat_uv_wide = (16.0 - u.icon_zoom).exp2().clamp(0.03, 8.0);
    draw_vars.set_uniform(
        cx,
        live_id!(shiny_gates2),
        &[
            if shiny.dynamic_sun { 1.0 } else { 0.0 },
            shiny.sun.shadow_alpha,
            mat_uv_scale,
            mat_uv_wide,
        ],
    );
    let sun = &shiny.sun;
    draw_vars.set_uniform(cx, live_id!(sun_dir), &[sun.dir.x, sun.dir.y, sun.dir.z]);
    draw_vars.set_uniform(cx, live_id!(sun_color), &[sun.color.x, sun.color.y, sun.color.z]);
    draw_vars.set_uniform(cx, live_id!(sun_sky), &[sun.sky.x, sun.sky.y, sun.sky.z]);
    draw_vars.set_uniform(cx, live_id!(shadow_dir), &u.shadow_dir);
    draw_vars.set_uniform(cx, live_id!(shadow_cast), &[u.shadow_cast]);
    draw_vars.set_uniform(cx, live_id!(shadow_mask_on), &[u.shadow_mask_on]);
    draw_vars.set_uniform(cx, live_id!(shadow_mask_size), &u.shadow_mask_size);
    draw_vars.set_uniform(cx, live_id!(shadow_mask_flip), &[u.shadow_mask_flip]);
    draw_vars.set_uniform(cx, live_id!(space_warp), &u.space_warp);
    draw_vars.set_uniform(cx, live_id!(space_warp2), &u.space_warp2);
    draw_vars.set_texture(1, terrain_tex);
    draw_vars.set_texture(2, shadow_mask.unwrap_or(terrain_tex));
}

/// Compact ground fills. Unlike the generic vector drawer this has no
/// per-instance channels; every varying is reconstructed from five geometry
/// slots in the script vertex function above.
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawMapFill {
    #[rust(ShinyConfig::default())]
    pub shiny: ShinyConfig,
    /// The frame's shadow mask (ground fills are what the shadows darken).
    #[rust]
    shadow_mask: Option<Texture>,
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub draw_clip: Vec4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(0.0)]
    pub draw_depth: f32,
}

impl DrawMapFill {
    fn draw_geometry(
        &mut self,
        cx: &mut Cx2d,
        geometry_id: GeometryId,
        uniforms: &MapDrawUniforms,
        terrain_tex: &Texture,
        pass_depth: f32,
    ) {
        self.draw_depth = pass_depth;
        stamp_map_uniforms(
            &mut self.draw_vars,
            cx.cx,
            uniforms,
            &self.shiny,
            terrain_tex,
            self.shadow_mask.as_ref(),
        );
        self.draw_vars.geometry_id = Some(geometry_id);
        cx.new_draw_call(&self.draw_vars);
        if self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}

/// Instanced POI symbols: the registry mesh is bound as the geometry (one
/// GPU copy per slot, see `MapView::icon_mesh_geometries`), every placement
/// is one instance of `DrawMapIcon`'s per-instance fields. The shader is
/// `DrawMapVector`'s icon path (see the script twin).
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawMapIcon {
    #[rust(ShinyConfig::default())]
    pub shiny: ShinyConfig,
    #[rust]
    shadow_mask: Option<Texture>,
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub draw_clip: Vec4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(0.0)]
    pub draw_depth: f32,
    #[live]
    pub inst_anchor: Vec2f,
    #[live]
    pub inst_offset: Vec2f,
    #[live(1.0)]
    pub inst_scale: f32,
    #[live]
    pub inst_param4: f32,
    #[live]
    pub inst_zbias: f32,
    #[live]
    pub inst_color: f32,
}

impl DrawMapIcon {
    /// Draw every instance group of one tile: one draw call per symbol mesh.
    fn draw_groups(
        &mut self,
        cx: &mut Cx2d,
        groups: &[IconInstances],
        mesh_geometries: &mut HashMap<u16, Geometry>,
        uniforms: &MapDrawUniforms,
        terrain_tex: &Texture,
        pass_depth: f32,
    ) {
        if self.draw_vars.draw_shader_id.is_none() {
            return;
        }
        self.draw_depth = pass_depth;
        for group in groups {
            if group.data.is_empty() {
                continue;
            }
            let Some(mesh) = icon_mesh_by_slot(group.mesh_slot) else {
                continue;
            };
            let geometry = mesh_geometries.entry(group.mesh_slot).or_insert_with(|| {
                let geometry = Geometry::new(cx.cx);
                geometry.update(
                    cx.cx,
                    mesh.indices.clone(),
                    crate::makepad_draw::vector::pack_icon_vertices(&mesh.verts),
                );
                geometry
            });
            stamp_map_uniforms(
                &mut self.draw_vars,
                cx.cx,
                uniforms,
                &self.shiny,
                terrain_tex,
                self.shadow_mask.as_ref(),
            );
            self.draw_vars.geometry_id = Some(geometry.geometry_id());
            cx.new_draw_call(&self.draw_vars);
            let Some(mut instances) = cx.begin_many_aligned_instances(&self.draw_vars) else {
                continue;
            };
            for record in group.data.chunks_exact(ICON_INSTANCE_FLOATS) {
                self.inst_anchor = vec2(record[0], record[1]);
                self.inst_offset = vec2(record[2], record[3]);
                self.inst_scale = record[4];
                self.inst_param4 = record[5];
                self.inst_zbias = record[6];
                self.inst_color = record[7];
                instances.instances.extend_from_slice(self.draw_vars.as_slice());
            }
            let new_area = cx.end_many_instances(instances);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}


/// Instanced building walls: the shared unit quad is the geometry, every
/// footprint edge is one instance (see `WALL_INSTANCE_FLOATS`); the shader is
/// `DrawMapWall` in the script block.
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawMapWall {
    #[rust(ShinyConfig::default())]
    pub shiny: ShinyConfig,
    #[rust]
    shadow_mask: Option<Texture>,
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub draw_clip: Vec4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(0.0)]
    pub draw_depth: f32,
    #[live]
    pub inst_a: Vec2f,
    #[live]
    pub inst_b: Vec2f,
    /// x = base metres, y = top metres.
    #[live]
    pub inst_heights: Vec2f,
    #[live]
    pub inst_normal: Vec2f,
    #[live(1.0)]
    pub inst_ao: f32,
    #[live]
    pub inst_color: f32,
    #[live]
    pub inst_zbias: f32,
}

impl DrawMapWall {
    /// Draw one tile's wall edges as a single instanced call.
    fn draw_edges(
        &mut self,
        cx: &mut Cx2d,
        records: &[f32],
        uniforms: &MapDrawUniforms,
        terrain_tex: &Texture,
        pass_depth: f32,
    ) {
        if records.is_empty() || self.draw_vars.draw_shader_id.is_none() {
            return;
        }
        self.draw_depth = pass_depth;
        stamp_map_uniforms(
            &mut self.draw_vars,
            cx.cx,
            uniforms,
            &self.shiny,
            terrain_tex,
            self.shadow_mask.as_ref(),
        );
        cx.new_draw_call(&self.draw_vars);
        let Some(mut instances) = cx.begin_many_aligned_instances(&self.draw_vars) else {
            return;
        };
        for record in records.chunks_exact(WALL_INSTANCE_FLOATS) {
            self.inst_a = vec2(record[0], record[1]);
            self.inst_b = vec2(record[2], record[3]);
            self.inst_heights = vec2(record[4], record[5]);
            self.inst_normal = vec2(record[6], record[7]);
            self.inst_ao = record[8];
            self.inst_color = record[9];
            self.inst_zbias = record[10];
            instances.instances.extend_from_slice(self.draw_vars.as_slice());
        }
        let new_area = cx.end_many_instances(instances);
        self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
    }
}

/// Instanced wall-edge shadows: same records as `DrawMapWall`, projected
/// along the sun onto the ground in the vertex shader.
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawMapShadow {
    #[rust(ShinyConfig::default())]
    pub shiny: ShinyConfig,
    #[rust]
    shadow_mask: Option<Texture>,
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub draw_clip: Vec4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(0.0)]
    pub draw_depth: f32,
    #[live]
    pub inst_a: Vec2f,
    #[live]
    pub inst_b: Vec2f,
    #[live]
    pub inst_heights: Vec2f,
    #[live]
    pub inst_normal: Vec2f,
    #[live(1.0)]
    pub inst_ao: f32,
    #[live]
    pub inst_color: f32,
    #[live]
    pub inst_zbias: f32,
}

impl DrawMapShadow {
    fn draw_edges(
        &mut self,
        cx: &mut Cx2d,
        records: &[f32],
        uniforms: &MapDrawUniforms,
        terrain_tex: &Texture,
        pass_depth: f32,
    ) {
        if records.is_empty() || self.draw_vars.draw_shader_id.is_none() {
            return;
        }
        self.draw_depth = pass_depth;
        stamp_map_uniforms(
            &mut self.draw_vars,
            cx.cx,
            uniforms,
            &self.shiny,
            terrain_tex,
            self.shadow_mask.as_ref(),
        );
        cx.new_draw_call(&self.draw_vars);
        let Some(mut instances) = cx.begin_many_aligned_instances(&self.draw_vars) else {
            return;
        };
        for record in records.chunks_exact(WALL_INSTANCE_FLOATS) {
            self.inst_a = vec2(record[0], record[1]);
            self.inst_b = vec2(record[2], record[3]);
            self.inst_heights = vec2(record[4], record[5]);
            self.inst_normal = vec2(record[6], record[7]);
            self.inst_ao = record[8];
            self.inst_color = record[9];
            self.inst_zbias = record[10];
            instances.instances.extend_from_slice(self.draw_vars.as_slice());
        }
        let new_area = cx.end_many_instances(instances);
        self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
    }
}

// --- MapView widget ---

#[derive(Script, Widget)]
pub struct MapView {
    /// The @cam readout in the corner: the exact command to recreate this
    /// view, drawn over the map. On by default — map work is driven by it —
    /// and off for an app shipping the map as a face rather than a bench.
    #[live(true)]
    debug_cam: bool,
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[redraw]
    #[live]
    draw_map: DrawMapVector,
    #[redraw]
    #[live]
    draw_fill: DrawMapFill,
    #[redraw]
    #[live]
    draw_road: DrawMapRoad,
    #[redraw]
    #[live]
    draw_icon: DrawMapIcon,
    #[redraw]
    #[live]
    draw_wall: DrawMapWall,
    #[redraw]
    #[live]
    draw_shadow: DrawMapShadow,
    /// One GPU copy of every symbol mesh the resident tiles reference,
    /// keyed by registry slot; instanced draws bind these.
    #[rust]
    icon_mesh_geometries: HashMap<u16, Geometry>,
    #[rust]
    shadow_mask_pass: Option<DrawPass>,
    #[rust]
    shadow_mask_texture: Option<Texture>,
    #[rust]
    shadow_mask_list: Option<DrawList2d>,
    #[redraw]
    #[live]
    draw_label: DrawRotatedText,
    #[redraw]
    #[live]
    draw_rain: DrawRainOverlay,
    #[redraw]
    #[live]
    draw_terrain: DrawTerrainOverlay,
    #[redraw]
    #[live]
    draw_text: DrawText,

    #[live(4.9041)]
    center_lon: f64,
    #[live(52.3676)]
    center_lat: f64,
    #[live(14.0)]
    zoom: f64,
    #[live(11.0)]
    min_zoom: f64,
    #[live(19.0)]
    max_zoom: f64,
    /// Map bearing that points up, in degrees (0 = north-up). Drives the
    /// heading-up navigation camera.
    #[live(0.0)]
    rotation: f64,
    /// Axonometric camera tilt in degrees (0 = top-down). Compresses the
    /// screen y about the view center and lifts 2.5D building geometry.
    #[live(0.0)]
    tilt: f64,
    /// Bake extruded, shaded buildings from the detail archive while the
    /// camera is tilted (needs `detail_mbtiles_path`).
    #[live(false)]
    buildings_3d: bool,
    #[live(false)]
    dark_theme: bool,
    /// Active theme: 0 = light, 1 = dark, 2 = circuit city. `dark_theme`
    /// stays as the boolean shorthand for 0/1.
    #[live(0)]
    theme_select: u32,
    #[live]
    style_light: MapThemeStyle,
    #[live]
    style_dark: MapThemeStyle,
    /// "Circuit City": the sci-fi night preset — near-black ground,
    /// emissive amber road filaments, glossy dark buildings.
    #[live]
    style_circuit: MapThemeStyle,
    #[live(true)]
    use_network: bool,
    #[live(true)]
    use_local_mbtiles: bool,
    /// Overrides the built-in LOCAL_MBTILES_PATH when non-empty, so each app
    /// can point its MapView at its own tile archive.
    #[live]
    mbtiles_path: String,
    /// Semicolon-separated list of geodata overlay mbtiles (layers.md
    /// track: chargers, transit, nature, districts…). Set at runtime via
    /// set_overlay_paths; tiles rebuild on change.
    #[live]
    overlay_mbtiles_paths: String,
    /// Optional all-tag detail archive (pbf-detail output) composed over the
    /// base for micro-POI symbols: trees, benches, bins, artwork…
    #[live]
    detail_mbtiles_path: String,
    /// Optional bridge-bake overlay (bridge-bake output): constraint-solved
    /// + AHN-measured per-vertex road elevation. Inside its coverage bounds
    /// it replaces every tag-based bridge deck heuristic.
    #[live]
    bridge_dz_mbtiles_path: String,
    /// Declared minzoom/maxzoom of the active archive (from its metadata
    /// table). Single-zoom detail archives (minzoom=maxzoom=14) must not be
    /// probed at z13/z12 — those tiles cannot exist.
    #[rust]
    local_source_zoom_range: Option<(u32, u32)>,
    /// Last non-default range announced for this source installation.
    #[rust]
    local_source_logged_zoom_range: Option<(u32, u32)>,
    #[rust]
    local_source_zoom_range_path: Option<String>,
    /// True when the metadata read ran while the archive file existed.
    #[rust]
    local_source_zoom_range_checked: bool,
    #[rust]
    tile_source_config: Option<TileSourceConfig>,
    #[rust]
    base_archive: Option<MapTileArchive>,
    #[rust]
    detail_archive: Option<MapTileArchive>,
    #[rust]
    archive_generation: u64,
    #[rust]
    archive_pending_tiles: HashMap<TileKey, ArchiveTileParts>,
    #[rust]
    archive_worker_pool: Option<ArchiveWorkerPool>,
    #[cfg(not(target_arch = "wasm32"))]
    #[rust]
    archive_watch_rx: ToUIReceiver<ArchiveWatchResult>,
    #[cfg(not(target_arch = "wasm32"))]
    #[rust]
    archive_watch_in_flight: bool,

    #[rust]
    center_norm: Vec2d,
    #[rust]
    view_rect: Rect,
    #[rust]
    drag_start_abs: Option<Vec2d>,
    #[rust]
    drag_start_center_norm: Vec2d,
    #[rust]
    tiles: HashMap<TileKey, TileEntry>,
    #[rust]
    request_to_tile: HashMap<LiveId, PendingTileRequest>,
    #[rust]
    next_request_id: u64,
    #[rust]
    visible_tiles: Vec<TileKey>,
    #[rust]
    frame_counter: u64,
    #[rust]
    status: String,
    #[rust]
    label_perf: LabelPerfStats,
    #[rust]
    local_source_missing_logged: bool,
    #[rust]
    tile_worker_rx: ToUIReceiver<TileWorkerMessage>,
    #[rust]
    tile_thread_pool: Option<TaskPool>,
    #[rust]
    tile_thread_pool_unavailable: bool,
    #[rust]
    local_requested_tiles: HashMap<TileKey, f64>,
    #[rust]
    archive_request_watchdog_rx: ToUIReceiver<()>,
    #[rust]
    archive_request_watchdog_scheduler: Option<Scheduler>,
    #[rust]
    archive_request_watchdog_handle: Option<TimerHandle>,
    #[rust]
    archive_request_watchdog_unavailable: bool,
    #[rust]
    /// Tiles the archive reported absent, stamped with the frame we learned
    /// it — re-checked after MISSING_RECHECK_FRAMES so a rebuilt/replaced
    /// mbtiles (or a transient read glitch) heals instead of leaving a
    /// permanent hole.
    local_missing_tiles: HashMap<TileKey, u64>,
    #[rust]
    applied_dark_theme: Option<bool>,
    #[rust]
    style_epoch: u64,
    /// Rain radar nowcast: one mercator-aligned RGBA texture per +5 min
    /// frame, animated on a timer while enabled.
    #[rust]
    rain_frames: Vec<Texture>,
    #[rust]
    rain_frame_index: usize,
    #[rust]
    rain_timer: Timer,
    /// (west, south, east, north) of the rain textures in lon/lat.
    #[rust]
    rain_bbox: (f64, f64, f64, f64),
    #[rust]
    rain_tex_size: (usize, usize),
    /// Hi-res dual-radar composite shown in place of animation frame 0 (the
    /// "now" frame): texture + its own pixel size (bbox is rain_bbox).
    #[rust]
    rain_now_hires: Option<(Texture, (usize, usize))>,
    #[rust(0.35)]
    rain_interval_current: f64,
    #[rust]
    terrain_texture: Option<Texture>,
    #[rust]
    terrain_elev_texture: Option<Texture>,
    #[rust]
    terrain_fallback_texture: Option<Texture>,
    #[rust]
    terrain_elev: Vec<f32>,
    #[rust((0, 0))]
    terrain_elev_size: (usize, usize),
    #[rust]
    terrain_elev_max: f32,
    /// Tilt-mode depth per screen px of ground y, rescaled each frame so
    /// the whole ladder stays inside the -24 map-depth budget on tall or
    /// steep views (a fixed 0.01 overflowed into the UI/label domain).
    #[rust(0.01)]
    tilt_depth_slope: f64,
    /// (west, north, east, south) in normalized mercator.
    #[rust]
    terrain_bbox: (f64, f64, f64, f64),
    /// 10 m wind field (u east+, v north+, row 0 = south) for the particle
    /// layer; bbox in lon/lat.
    #[rust]
    wind_field: Option<(usize, usize, Vec<f32>, Vec<f32>, (f64, f64, f64, f64))>,
    #[rust]
    wind_particles: Vec<WindParticle>,
    #[rust]
    wind_timer: Timer,
    #[rust]
    wind_rng: u64,
    /// The 2D/3D mode the current tile set was baked with. Mode transitions
    /// are detected in `ensure_visible_tiles`, the single invalidation owner.
    #[rust]
    baked_3d_mode: bool,
    #[rust]
    compiled_style_light: CompiledMapTheme,
    #[rust]
    compiled_style_dark: CompiledMapTheme,
    #[rust]
    compiled_style_circuit: CompiledMapTheme,
    #[rust]
    path_glyphs: Vec<PathGlyphInstance>,
    // Scratch buffers reused across frames to avoid per-frame allocations
    #[rust]
    scratch_draw_tiles: Vec<TileKey>,
    #[rust]
    scratch_draw_seen: HashSet<TileKey>,
    #[rust]
    scratch_descendant_tiles: Vec<TileKey>,
    #[rust]
    scratch_candidates: Vec<LabelCandidate>,
    #[rust]
    scratch_accepted_centers: HashMap<String, Vec<Vec2d>>,
    #[rust]
    scratch_accepted_bounds: Vec<Rect>,
    #[rust]
    scratch_accepted_plans: Vec<(f64, usize, usize, u8, bool, bool, Vec2f, f32)>,
    // Labels drawn last frame (hashed name+position key); kept to stabilize
    // placement while panning instead of flickering between candidates.
    #[rust]
    prev_label_keys: HashSet<u64>,
    #[rust]
    scratch_accepted_hashes: Vec<u64>,
    // Frame of the last zoom change; zoom-bucket restyles are deferred until
    // the gesture settles so widths don't flicker mid-zoom.
    #[rust]
    last_zoom_change_frame: u64,
    #[rust]
    last_zoom_change_time: Option<f64>,
    #[rust]
    zoom_settle_timer: Timer,
    #[rust]
    tile_fade_timer: Timer,
    #[cfg(not(target_arch = "wasm32"))]
    #[rust]
    archive_watch_timer: Timer,
    #[cfg(not(target_arch = "wasm32"))]
    #[rust]
    archive_watch_mtime: Option<u128>,
    // Label placement cache: while panning at the same zoom over the same
    // tiles, last placement's glyphs are redrawn shifted by the pan delta
    // instead of re-scanning/re-shaping/re-colliding thousands of labels.
    #[rust]
    label_cache_valid: bool,
    #[rust]
    label_cache_offset: Vec2d,
    #[rust]
    label_cache_zoom: f64,
    #[rust]
    label_cache_rotation: f64,
    #[rust]
    label_cache_tilt: f64,
    #[rust]
    label_cache_tiles: Vec<TileKey>,
    #[rust]
    label_cache_generation: u64,
    #[rust((1.0, Vec2f { x: 0.0, y: 0.0 }, 0.0, Vec2f { x: 0.0, y: 0.0 }, 1.0))]
    label_draw_transform: (f32, Vec2f, f32, Vec2f, f32),
    #[rust(1.0)]
    label_cache_tilt_cos_for_delta: f32,
    /// Inception fold setting: the USER'S INTENT (survives leaving close-3D
    /// — un-tilting tweens the fold out, tilting back in tweens it back).
    #[rust]
    space_warp_want: bool,
    /// Tween position 0..1 (raw; the eased value goes to the shader).
    #[rust]
    space_warp_t: f64,
    #[rust]
    space_warp_last_step: Option<f64>,
    /// The effective warp this frame — stamped in draw_walk, read by every
    /// OverlayCamera construction so CPU projections match the GPU tiles.
    #[rust]
    space_warp_eff: SpaceWarp,
    #[rust]
    tiles_generation: u64,
    #[rust]
    last_full_place_time: Option<f64>,
    /// When the camera (rotation/tilt/zoom/warp tween) last CHANGED — the
    /// full label re-place waits for ~a beat of camera quiet, so labels
    /// ride the exact GPU transforms through the whole gesture and settle
    /// once, where the transforms already put them.
    #[rust]
    camera_motion_last: Option<f64>,
    /// Camera signature of the previous draw, for the motion detector.
    #[rust]
    camera_motion_sig: (f64, f64, f64, f64),
    #[rust]
    needs_label_followup: bool,
    // Shaped text runs keyed by (text hash, len, quantized font_scale bits);
    // shaping dominates label placement cost.
    #[rust]
    shaped_runs: HashMap<(u64, u32, u32), Option<PreparedTextRun>>,
    // Finished tile buffers waiting for GPU upload; drained a couple per
    // frame so a 10-tile rebuild batch doesn't stall a single frame with
    // hundreds of MB of buffer creation/upload.
    #[rust]
    pending_ready_tiles: Vec<(TileKey, TileBuffers)>,
    #[rust]
    last_tile_upload_frame: u64,
    // Frame-time instrumentation, aggregated to local/map_perf.log.
    #[rust]
    perf_frames: u32,
    #[rust]
    perf_ms_total: f64,
    #[rust]
    perf_ms_geo: f64,
    #[rust]
    perf_ms_icons: f64,
    #[rust]
    perf_ms_tail: f64,
    #[rust]
    perf_ms_labels: f64,
    #[rust]
    perf_ms_max: f64,
    #[rust]
    perf_label_full_places: u32,
    #[rust]
    perf_last_frame: Option<f64>,
    #[rust]
    perf_ms_gap_max: f64,
    #[rust]
    perf_gap_sum: f64,
    #[rust]
    perf_gap_count: u32,
    #[rust]
    perf_gaps_over_12ms: u32,
    #[rust]
    scratch_screen_path: Vec<Vec2d>,
    #[rust]
    scratch_cumulative: Vec<f64>,
    #[rust]
    scratch_smooth_a: Vec<Vec2d>,
    #[rust]
    scratch_smooth_b: Vec<Vec2d>,
    #[rust]
    prev_status_label_perf: LabelPerfStats,
    #[rust]
    prev_status_counters: (usize, usize, usize, usize, usize, usize),

    // --- Interaction layer (overlay + camera API) ---
    #[live]
    draw_overlay: DrawVector,
    #[rust]
    overlay: MapOverlayState,
    #[rust]
    fly: Option<FlyTo>,
    #[rust]
    fly_timer: Timer,
    #[rust]
    gesture_panned: bool,
    /// Right-button / Option-drag camera gesture: (start abs, start
    /// rotation, start tilt).
    #[rust]
    rotate_drag: Option<(Vec2d, f64, f64)>,
    #[rust]
    last_tap_count: u32,
    #[rust]
    pending_viewport_changed: bool,
    /// shiny.md: gentle idle heartbeat so time-driven materials (water
    /// flow, grass sway) animate without interaction. Only runs while an
    /// animated material is on and the view is close enough to show it.
    #[rust]
    shiny_anim_timer: Timer,
    #[rust]
    shiny_anim_on: bool,
}

impl ScriptHook for MapView {
    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_eval() {
            return;
        }

        super::warm_shared_registries();

        let min_zoom = self.min_zoom.max(0.0);
        let max_zoom = self.max_zoom.max(min_zoom);
        self.zoom = self.zoom.clamp(min_zoom, max_zoom);
        self.center_norm = lon_lat_to_normalized(self.center_lon, self.center_lat);
        self.wrap_and_clamp_center();
        self.normalize_source_mode();

        let previous_light = self.compiled_style_light.clone();
        let previous_dark = self.compiled_style_dark.clone();
        let previous_circuit = self.compiled_style_circuit.clone();
        self.rebuild_compiled_styles();
        let styles_changed = previous_light != self.compiled_style_light
            || previous_dark != self.compiled_style_dark
            || previous_circuit != self.compiled_style_circuit;
        if self.style_epoch == 0 {
            self.style_epoch = 1;
        }

        let theme_changed = self.applied_dark_theme != Some(self.dark_theme);
        if theme_changed || styles_changed {
            self.apply_theme_change();
            self.applied_dark_theme = Some(self.dark_theme);
        } else {
            self.apply_theme_palette();
        }

        if self.next_request_id == 0 {
            self.next_request_id = 1;
        }
        if self.status.is_empty() {
            self.status = "Loading Amsterdam tiles from local cache/mbtiles...".to_string();
        }
    }
}

impl Widget for MapView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.use_local_mbtiles {
            self.ensure_archive_source(cx);
            self.handle_archive_events(cx, event);
        }
        self.handle_tile_worker_messages(cx);
        if self.archive_request_watchdog_rx.try_recv_flush().is_ok() {
            self.archive_request_watchdog_handle = None;
            self.expire_archive_requests(cx);
        }
        self.sync_archive_request_watchdog(cx);
        self.widget_match_event(cx, event, scope);

        if self.wind_timer.is_event(event).is_some() && self.wind_field.is_some() {
            self.tick_wind();
            self.redraw(cx);
        }
        // Animated shiny materials: keep a modest heartbeat while they can
        // actually show (close zoom, feature on); stop it otherwise.
        {
            let shiny = &self.active_style().shiny;
            let want = (shiny.water_fx || shiny.foliage_fx) && self.render_bucket() >= 14;
            if want != self.shiny_anim_on {
                self.shiny_anim_on = want;
                cx.stop_timer(self.shiny_anim_timer);
                if want {
                    self.shiny_anim_timer = cx.start_interval(1.0 / 20.0);
                }
            }
        }
        if self.shiny_anim_timer.is_event(event).is_some() {
            self.redraw(cx);
        }
        if self.rain_timer.is_event(event).is_some() && !self.rain_frames.is_empty() {
            self.rain_frame_index = (self.rain_frame_index + 1) % self.rain_frames.len();
            self.retune_rain_timer(cx);
            self.redraw(cx);
        }
        self.handle_archive_watch(cx, event);
        if self.tile_fade_timer.is_event(event).is_some() {
            self.redraw(cx);
            if self.tiles.values().any(|entry| entry.fade.is_some()) {
                self.tile_fade_timer = cx.start_timeout(0.016);
            }
        }
        if self.zoom_settle_timer.is_event(event).is_some() {
            self.redraw(cx);
            if self.pending_viewport_changed {
                self.pending_viewport_changed = false;
                self.sync_camera_fields();
                self.emit_viewport_changed(cx);
            }
            if self.needs_label_followup || !self.pending_ready_tiles.is_empty() {
                self.zoom_settle_timer = cx.start_timeout(0.08);
            }
        }

        // No global hotkeys: a raw KeyDown match here fires even while the
        // user types in a text input elsewhere (the old 'T' theme toggle).
        // Theme/layer switching is app UI now.

        if self.fly_timer.is_event(event).is_some() {
            self.tick_fly(cx);
        }

        // Respect the handled flag (no capture overload): floating UI panels
        // drawn on top of the map must win the hit test (EventOrder::Up
        // dispatches them first).
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(fe) => {
                let rotate_gesture = fe
                    .device
                    .mouse_button()
                    .is_some_and(|button| button.is_secondary())
                    || (fe.is_primary_hit() && fe.modifiers.alt);
                if rotate_gesture {
                    // Right-drag (or Option-drag): horizontal rotates the
                    // camera, vertical tilts it.
                    self.fly = None;
                    self.rotate_drag = Some((fe.abs, self.rotation, self.tilt));
                } else if fe.is_primary_hit() {
                    self.fly = None;
                    self.gesture_panned = false;
                    self.drag_start_abs = Some(fe.abs);
                    self.drag_start_center_norm = self.center_norm;
                    self.last_tap_count = fe.tap_count;
                    cx.set_cursor(MouseCursor::Grabbing);
                }
            }
            Hit::FingerMove(fe) => {
                if let Some((start_abs, start_rotation, start_tilt)) = self.rotate_drag {
                    let delta = fe.abs - start_abs;
                    self.rotation = (start_rotation - delta.x * 0.35).rem_euclid(360.0);
                    // Snap-to-2D dead zone: dragging the camera back to
                    // straight above lands on EXACTLY 0 so the renderer
                    // re-enters the flat building/tree style.
                    let raw_tilt =
                        (start_tilt + delta.y * 0.25).clamp(0.0, self.tilt_max_deg_now());
                    self.tilt = if raw_tilt < 6.0 { 0.0 } else { raw_tilt };
                    cx.widget_action(self.uid, MapViewAction::TiltChanged { tilt: self.tilt });
                    self.redraw(cx);
                } else if let Some(start_abs) = self.drag_start_abs {
                    let delta = fe.abs - start_abs;
                    if delta.length() > 6.0 {
                        self.gesture_panned = true;
                    }
                    // Screen drag maps to a world pan through the inverse of
                    // the heading-up rotation and camera tilt. Under the
                    // fold that inverse is NON-AFFINE, so a lone screen
                    // delta means nothing: invert BOTH endpoints and pan by
                    // the ground offset between them (grab-pan — the point
                    // under the finger at press stays under it).
                    let world_delta = if self.space_warp_eff.is_on() {
                        let camera = self.overlay_camera();
                        camera.screen_to_world_rel(fe.abs)
                            - camera.screen_to_world_rel(start_abs)
                    } else {
                        self.screen_delta_to_world(delta)
                    };
                    let world_size = tile_world_size_zoom(self.view_zoom());
                    self.center_norm = self.drag_start_center_norm
                        - dvec2(world_delta.x / world_size, world_delta.y / world_size);
                    self.wrap_and_clamp_center();
                    self.redraw(cx);
                }
            }
            Hit::FingerLongPress(lp) => {
                // Long press cancels the pan gesture and reports map coords.
                self.drag_start_abs = None;
                let (lon, lat) = self.screen_to_lon_lat(lp.abs);
                cx.widget_action(self.uid, MapViewAction::LongPressed { lon, lat, abs: lp.abs });
            }
            Hit::FingerUp(fe) => {
                if self.rotate_drag.take().is_some() {
                    // Releasing near straight-above settles on EXACTLY 0:
                    // a 5-10 degree residual reads as "3D mode stuck on".
                    if self.tilt > 0.0 && self.tilt < 10.0 {
                        self.tilt = 0.0;
                        cx.widget_action(self.uid, MapViewAction::TiltChanged { tilt: 0.0 });
                    }
                    // Guarantee a full label re-place AFTER the rate-limit
                    // window: without this, a fast spin that ends inside the
                    // window leaves the cached placement rigidly rotated
                    // (180° = upside-down labels) with no frame scheduled
                    // to true it up.
                    cx.stop_timer(self.zoom_settle_timer);
                    self.zoom_settle_timer =
                        cx.start_timeout(LABEL_REPLACE_MIN_SECONDS + 0.05);
                    self.redraw(cx);
                    self.sync_camera_fields();
                    self.emit_viewport_changed(cx);
                    return;
                }
                self.drag_start_abs = None;
                cx.set_cursor(MouseCursor::Grab);
                if fe.is_primary_hit() && fe.was_tap() {
                    if self.last_tap_count >= 2 {
                        // Double-click acts as the long-press (mouse holds
                        // don't synthesize FingerLongPress on desktop).
                        let (lon, lat) = self.screen_to_lon_lat(fe.abs);
                        cx.widget_action(
                            self.uid,
                            MapViewAction::LongPressed { lon, lat, abs: fe.abs },
                        );
                    } else if let Some((lon, lat, info)) = self.pin_at(fe.abs) {
                        cx.widget_action(self.uid, MapViewAction::PinTapped { lon, lat, info });
                    } else if let Some(id) = self.overlay.marker_at(&self.overlay_camera(), fe.abs)
                    {
                        cx.widget_action(self.uid, MapViewAction::MarkerClicked { id });
                    } else {
                        let (lon, lat) = self.screen_to_lon_lat(fe.abs);
                        cx.widget_action(self.uid, MapViewAction::Tapped { lon, lat, abs: fe.abs });
                    }
                } else if self.gesture_panned {
                    self.gesture_panned = false;
                    self.sync_camera_fields();
                    self.emit_viewport_changed(cx);
                }
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::FingerScroll(fs) => {
                let scroll = if fs.scroll.y.abs() > f64::EPSILON {
                    fs.scroll.y
                } else {
                    fs.scroll.x
                };
                self.fly = None;
                self.zoom_with_anchor(cx, scroll, fs.abs);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let perf_start = cx.seconds_since_app_start();
        if let Some(last_frame) = self.perf_last_frame {
            let gap_ms = (perf_start - last_frame).max(0.0) * 1000.0;
            self.perf_ms_gap_max = self.perf_ms_gap_max.max(gap_ms);
            // only count gaps from continuous animation, not idle pauses
            if gap_ms < 100.0 {
                self.perf_gap_sum += gap_ms;
                self.perf_gap_count += 1;
                if gap_ms > 12.0 {
                    self.perf_gaps_over_12ms += 1;
                }
            }
        }
        self.perf_last_frame = Some(perf_start);

        // The map's own draw list carries the clip: hosted as a pane beside
        // other content, every tile/terrain/label draw clamps against
        // view_clip (the shaders always did — the list just never had a
        // rect narrower than the window until the map was embedded).
        cx.begin_turtle(
            walk,
            Layout {
                clip_x: true,
                clip_y: true,
                ..Layout::default()
            },
        );
        let rect = cx.turtle().rect();
        self.view_rect = rect;
        self.draw_bg.draw_abs(cx, rect);
        self.ensure_visible_tiles(cx, rect);

        let view_zoom = self.view_zoom();
        let world_size = tile_world_size_zoom(view_zoom);
        let center_world = self.center_norm * world_size;
        // Keep the global offset in f64; geometry is tile-local, so the only
        // f32 quantities the GPU sees are small (tile-local coords and a
        // screen-magnitude per-tile offset).
        let map_offset = dvec2(
            rect.pos.x + rect.size.x * 0.5 - center_world.x,
            rect.pos.y + rect.size.y * 0.5 - center_world.y,
        );

        let (rot_cos, rot_sin) = self.screen_rotation();
        let rot_pivot = rect.pos + rect.size * 0.5;
        let view_rot_uniform = [rot_cos as f32, rot_sin as f32];
        let rot_pivot_uniform = [rot_pivot.x as f32, rot_pivot.y as f32];
        // Zoom-coupled tilt ceiling: zooming away from street level eases
        // any extra-steep camera back to the base cap (cap is continuous
        // in zoom, so this follows the zoom tween — no snap).
        let tilt_cap_now = self.tilt_max_deg_now();
        if self.tilt > tilt_cap_now {
            self.tilt = tilt_cap_now;
        }
        let tilt_rad = self.tilt.clamp(0.0, TILT_HARD_MAX_DEG).to_radians();
        let px_per_meter = {
            let (_, lat) = normalized_to_lon_lat(self.center_norm);
            world_size / (40_075_016.686 * lat.to_radians().cos())
        };
        // ---- the Inception mode (fold + perspective camera) ----
        // Only meaningful in the near-ground regime (strong tilt + close
        // zoom): the effect auto-tweens out when the camera un-tilts /
        // zooms away and back in when it returns — the SETTING
        // (space_warp_want) remembers intent throughout.
        let warp_conditions = self.space_warp_available();
        let warp_target = if self.space_warp_want && warp_conditions {
            1.0
        } else {
            0.0
        };
        if (self.space_warp_t - warp_target).abs() > 1e-4 {
            let now = perf_start;
            let dt = self
                .space_warp_last_step
                .map(|t| (now - t).clamp(0.0, 0.1))
                .unwrap_or(1.0 / 60.0);
            let step = dt / 0.6; // ~600ms full travel
            self.space_warp_t = if warp_target > self.space_warp_t {
                (self.space_warp_t + step).min(warp_target)
            } else {
                (self.space_warp_t - step).max(warp_target)
            };
            self.space_warp_last_step = Some(now);
            self.redraw(cx);
        } else {
            self.space_warp_last_step = None;
        }
        // Smoothstep ease on the tween. Geometry law (tuned by eye against
        // the reference): r0 pins the fold start ~0.18·H above the pivot in
        // flat-screen terms — in ground METERS that scales with camera
        // height (1/ppm) and tilt, so a near-first-person view folds a few
        // hundred meters out and a higher camera proportionally further.
        // R = 0.30·H curl radius; kappa_full = 1/H is a ~53° vertical FOV
        // at full effect (the perspective tweens in WITH the fold; ortho is
        // its kappa→0 limit, so amount 0 is today's camera exactly).
        let wt = self.space_warp_t;
        let warp_eased = wt * wt * (3.0 - 2.0 * wt);
        self.space_warp_eff = if warp_eased > 1e-4 {
            let h = rect.size.y.max(1.0);
            SpaceWarp {
                amount: warp_eased,
                start_px: 0.18 * h / tilt_rad.cos().max(0.087),
                radius_px: (0.30 * h).max(1.0),
                cos_t: tilt_rad.cos(),
                sin_t: tilt_rad.sin(),
                cap: tilt_rad,
                kappa: warp_eased / h,
            }
        } else {
            SpaceWarp::default()
        };
        let warp_uniform = [
            self.space_warp_eff.amount as f32,
            self.space_warp_eff.start_px as f32,
            self.space_warp_eff.radius_px as f32,
            self.space_warp_eff.sin_t as f32,
        ];
        let warp2_uniform = [
            self.space_warp_eff.kappa as f32,
            px_per_meter as f32,
            self.space_warp_eff.cap as f32,
            0.0,
        ];
        // One uniform set for the whole frame: every draw_geometry call
        // below shares draw_map's draw_vars.
        self.draw_map.space_warp_u = warp_uniform;
        self.draw_road.space_warp_u = warp_uniform;
        self.draw_map.space_warp2_u = warp2_uniform;
        self.draw_road.space_warp2_u = warp2_uniform;
        self.draw_map
            .draw_super
            .draw_vars
            .set_uniform(cx.cx, live_id!(space_warp), &warp_uniform);
        self.draw_map
            .draw_super
            .draw_vars
            .set_uniform(cx.cx, live_id!(space_warp2), &warp2_uniform);
        self.draw_road
            .draw_vars
            .set_uniform(cx.cx, live_id!(space_warp), &warp_uniform);
        self.draw_road
            .draw_vars
            .set_uniform(cx.cx, live_id!(space_warp2), &warp2_uniform);
        // Tilted map depth lives in a negative domain well below every UI
        // element, so panels/labels/overlay drawn later always win by call
        // order. Within the map, view-ground y dominates for occlusion and
        // the baked sort-rank micro-depth (param5) resolves overlapping
        // layers at the same ground pixel without depth-precision flicker.
        // Depth is a function of the UNWARPED ground rel-y in every mode —
        // with the fold on, the visible (unwarped) ground fan extends past
        // the flat frustum, so the ladder budget must cover the warp's
        // cull reach or far-wall depth overflows the domain.
        let tilt_uniform = if tilt_rad > 1e-4 {
            let mut max_rel = (rect.size.y * 0.5
                + self.terrain_elev_max as f64 * self.terrain_lift_px_per_m())
                / tilt_rad.cos().max(0.05);
            if self.space_warp_eff.is_on() {
                let (reach, _) = self
                    .space_warp_eff
                    .cull_extents(rect.size.y * 0.5, max_rel);
                max_rel = reach.max(max_rel);
            }
            self.tilt_depth_slope = (18.0 / (max_rel.max(1.0) * 2.0)).min(0.01);
            [
                tilt_rad.cos() as f32,
                (px_per_meter * tilt_rad.sin()) as f32,
                self.tilt_depth_slope as f32,
                -24.0,
            ]
        } else {
            // Flat mode stays byte-identical to the classic paint order.
            [tilt_rad.cos() as f32, 0.0, 0.0, 0.0]
        };
        // Below city buckets, fills stay flat (their sparse vertices cannot
        // track the surface) and the hillshade becomes the visible ground.
        let terrain_fill_lift = if self.render_bucket() >= 14 { 1.0f32 } else { 0.0 };
        // shiny.md gates + sun for the material dispatch, per active theme.
        self.draw_map.shiny = self.active_style().shiny;
        self.draw_fill.shiny = self.draw_map.shiny;
        self.draw_road.shiny = self.draw_map.shiny;
        self.draw_icon.shiny = self.draw_map.shiny;
        self.draw_wall.shiny = self.draw_map.shiny;
        self.draw_shadow.shiny = self.draw_map.shiny;
        // Road/symbol clearance over the terrain surface, scaled by the
        // relief actually in view: the margin exists to beat interpolation
        // twist (which grows with relief), but a flat-city boost lets
        // streets depth-beat the buildings in front of them.
        let pass_boost = if self.terrain_elev.is_empty() {
            0.0f32
        } else {
            let ground_margin_px = (self.terrain_elev_max as f64
                * self.terrain_lift_px_per_m()
                / 50.0)
                .clamp(4.0, 30.0);
            (ground_margin_px * self.tilt_depth_slope) as f32
        };
        // 3D terrain displacement inputs: the elevation texture's bbox as a
        // pre-rotation screen rect (the shader's sampling uv space). Span
        // zero disables the lift; a 1x1 fallback keeps the slot bound.
        let mut terrain_org = [0.0f32; 2];
        let mut terrain_span = [0.0f32; 2];
        let terrain_tex = match (tilt_rad > 1e-4, self.terrain_elev_texture.clone()) {
            (true, Some(tex)) => {
                let camera = self.overlay_camera();
                let (west, north, east, south) = self.terrain_bbox;
                let org = dvec2(west, north) * camera.world_size + camera.offset;
                let se = dvec2(east, south) * camera.world_size + camera.offset;
                terrain_org = [org.x as f32, org.y as f32];
                terrain_span = [(se.x - org.x) as f32, (se.y - org.y) as f32];
                tex
            }
            _ => self.terrain_fallback(cx),
        };
        // bbox uv 0..1 -> texel centers (half-texel inset) so GPU bilinear
        // lands exactly on the CPU mesh's linear surface.
        let (ew, eh) = self.terrain_elev_size;
        let terrain_uvfit = if ew > 1 && eh > 1 {
            [
                (ew as f32 - 1.0) / ew as f32,
                (eh as f32 - 1.0) / eh as f32,
                0.5 / ew as f32,
                0.5 / eh as f32,
            ]
        } else {
            [1.0, 1.0, 0.0, 0.0]
        };

        self.fill_draw_tile_keys();
        // Take draw_tiles out so we can pass &[TileKey] while mutating self for labels
        let mut draw_tiles = std::mem::take(&mut self.scratch_draw_tiles);
        // Tiles still fading in from empty draw LAST (on top): their old-zoom
        // stand-ins painted beneath them make zoom transitions a real
        // cross-fade instead of a flash of background color.
        draw_tiles.sort_unstable_by_key(|key| {
            (
                self.tile_fading_from_empty(*key) as u8,
                key.z,
                key.y,
                key.x,
            )
        });
        let draw_tiles = draw_tiles;

        let shadow_mask_live = self.buildings_3d
            && self.tilt > 0.0
            && self.active_style().shiny.bake_shadows;
        self.draw_map.shadow_mask_size = [rect.size.x as f32, rect.size.y as f32];
        self.draw_map.shadow_cast = 0.0;
        self.draw_map.shadow_dir = [0.0, 0.0];
        if shadow_mask_live {
            self.draw_shadow_mask_pass(
                cx,
                &draw_tiles,
                view_zoom,
                map_offset,
                view_rot_uniform,
                rot_pivot_uniform,
                tilt_uniform,
                terrain_org,
                terrain_span,
                terrain_uvfit,
                &terrain_tex,
                terrain_fill_lift,
                rect,
            );
            self.draw_map.shadow_mask = self.shadow_mask_texture.clone();
            self.draw_icon.shadow_mask = self.shadow_mask_texture.clone();
            self.draw_wall.shadow_mask = self.shadow_mask_texture.clone();
            self.draw_fill.shadow_mask = self.shadow_mask_texture.clone();
            self.draw_map.shadow_mask_on = 1.0;
            self.draw_map.shadow_mask_flip = shadow_mask_y_flip_for_os(cx.os_type());
        } else {
            self.draw_map.shadow_mask = None;
            self.draw_icon.shadow_mask = None;
            self.draw_wall.shadow_mask = None;
            self.draw_fill.shadow_mask = None;
            self.draw_map.shadow_mask_on = 0.0;
        }
        self.draw_road.shadow_mask = self.draw_map.shadow_mask.clone();
        self.draw_road.shadow_dir = self.draw_map.shadow_dir;
        self.draw_road.shadow_cast = self.draw_map.shadow_cast;
        self.draw_road.shadow_mask_on = self.draw_map.shadow_mask_on;
        self.draw_road.shadow_mask_size = self.draw_map.shadow_mask_size;
        self.draw_road.shadow_mask_flip = self.draw_map.shadow_mask_flip;

        // Four global passes (carto layer order): every tile's fills, then
        // every tile's road casings, then road centers, then POI symbols.
        // Casings interleaved per tile would stamp over neighbor tiles' road
        // interiors in the clip-padding overlap at tile seams.
        for pass in 0..3 {
            if pass == 1 {
                // Hillshade sits over the land fills but UNDER the roads.
                self.draw_terrain_overlay(cx);
            }
            for key in &draw_tiles {
                let Some(entry) = self.tiles.get(key) else {
                    continue;
                };
                let TileLoadState::Ready {
                    fill_geometry,
                    fill_misc_geometry,
                    casing_geometry,
                    stroke_geometry,
                    icon_geometry,
                    icon_high_geometry,
                    shadow_disc_geometry,
                    fringe_geometry,
                    fill_3d_geometry,
                    wall_geometry,
                    wall_instances,
                    tree_geometry,
                    tree_cross_geometry,
                    tree_template_geometry,
                    tree_cross_template_geometry,
                    tree_instances,
                    ..
                } = &entry.state
                else {
                    continue;
                };
                // Once a backend has uploaded a stream, its CPU staging is
                // dead weight: the GPU copy is the resident one. A tile can
                // always be rebaked from the archive, so nothing needs the
                // vectors back. The free itself happens on a pool worker:
                // on the threaded web build a large free on the UI thread
                // contends the allocator lock with the bakes, and a
                // contended lock there is an `Atomics.wait` the main thread
                // may not make. No pool: the staging simply stays.
                if pass == 0 {
                    for geometry in [
                        fill_geometry,
                        fill_misc_geometry,
                        casing_geometry,
                        stroke_geometry,
                        icon_geometry,
                        icon_high_geometry,
                        shadow_disc_geometry,
                        fringe_geometry,
                        fill_3d_geometry,
                        wall_geometry,
                        tree_geometry,
                        tree_cross_geometry,
                        tree_template_geometry,
                        tree_cross_template_geometry,
                    ]
                    .into_iter()
                    .flatten()
                    {
                        let Some(pool) = self.tile_thread_pool.as_ref() else {
                            break;
                        };
                        let Some((indices, vertices)) =
                            geometry.take_cpu_buffers_if_uploaded(cx.cx)
                        else {
                            continue;
                        };
                        match pool.submit(QueueOrder::Fifo, move || drop((indices, vertices))) {
                            Ok(handle) => handle.detach(),
                            Err(_) => {
                                // The pool is full: keep the staging until a
                                // later frame can hand it over.
                                break;
                            }
                        }
                    }
                }
                // Stale higher-bucket tiles keep their baked symbols until
                // the rebuild lands. Charger pins bake from z9 so the pass
                // itself stays on when zoomed out — but a stale POI-carpet
                // tile (baked at z16+) hides its symbols the moment the
                // view drops below icon level, instead of splattering
                // hundreds of full-size shop icons across the region.
                if pass >= 3
                    && (view_zoom < 7.75
                        || (entry.bucket >= ICON_MIN_ZOOM
                            && view_zoom < ICON_MIN_ZOOM as f64 - 0.25))
                {
                    continue;
                }
                let geometry = match pass {
                    1 => casing_geometry,
                    2 => stroke_geometry,
                    3 => icon_geometry,
                    _ => icon_high_geometry,
                };
                let scale = 2.0_f64.powf(view_zoom - key.z as f64);
                let tile_offset = map_offset
                    + dvec2(
                        key.x as f64 * TILE_SIZE * scale,
                        key.y as f64 * TILE_SIZE * scale,
                    );
                let map_scale = Vec2f {
                    x: scale as f32,
                    y: scale as f32,
                };
                let screen_offset = Vec2f {
                    x: tile_offset.x as f32,
                    y: tile_offset.y as f32,
                };
                let mut fade_alpha = 1.0_f32;
                if let Some(fade) = &entry.fade {
                    fade_alpha = (((perf_start - fade.started).max(0.0) / TILE_FADE_SECONDS)
                        as f32)
                        .clamp(0.0, 1.0);
                    if pass == 0 {
                        let uniforms = MapDrawUniforms {
                            map_scale,
                            map_offset: screen_offset,
                            fade: 1.0,
                            width_correction: stroke_width_correction(fade.bucket, view_zoom),
                            view_rot: view_rot_uniform,
                            rot_pivot: rot_pivot_uniform,
                            tilt_params: tilt_uniform,
                            icon_zoom: view_zoom as f32,
                            height_grow: 1.0,
                            terrain_org,
                            terrain_span,
                            terrain_uvfit,
                            terrain_fill_lift,
                            shadow_dir: self.draw_map.shadow_dir,
                            shadow_cast: self.draw_map.shadow_cast,
                            shadow_mask_on: self.draw_map.shadow_mask_on,
                            shadow_mask_size: self.draw_map.shadow_mask_size,
                            shadow_mask_flip: self.draw_map.shadow_mask_flip,
                            space_warp: self.draw_map.space_warp_u,
                            space_warp2: self.draw_map.space_warp2_u,
                        };
                        if let Some(outgoing) = &fade.fill_geometry {
                            self.draw_fill.draw_geometry(
                                cx,
                                outgoing.geometry_id(),
                                &uniforms,
                                &terrain_tex,
                                0.0,
                            );
                        }
                        if let Some(outgoing) = &fade.fill_misc_geometry {
                            self.draw_map.draw_geometry(
                                cx,
                                outgoing.geometry_id(),
                                map_scale,
                                screen_offset,
                                1.0,
                                uniforms.width_correction,
                                view_rot_uniform,
                                rot_pivot_uniform,
                                tilt_uniform,
                                view_zoom as f32,
                                1.0,
                                terrain_org,
                                terrain_span,
                                terrain_uvfit,
                                &terrain_tex,
                                0.0,
                                terrain_fill_lift,
                            );
                        }
                    } else if let Some(outgoing) = match pass {
                        1 => &fade.casing_geometry,
                        2 => &fade.stroke_geometry,
                        3 => &fade.icon_geometry,
                        _ => &None,
                    } {
                        let outgoing_id = outgoing.geometry_id();
                        let road_pass = matches!(pass, 1 | 2);
                        if road_pass {
                            self.draw_road.draw_geometry(
                                cx,
                                outgoing_id,
                                map_scale,
                                screen_offset,
                                1.0,
                                stroke_width_correction(fade.bucket, view_zoom),
                                view_rot_uniform,
                                rot_pivot_uniform,
                                tilt_uniform,
                                view_zoom as f32,
                                1.0,
                                terrain_org,
                                terrain_span,
                                terrain_uvfit,
                                &terrain_tex,
                                if tilt_rad > 1e-4 { pass_boost + (pass - 1) as f32 * 0.02 } else { 0.0 },
                                terrain_fill_lift,
                            );
                        } else {
                            self.draw_map.draw_geometry(
                            cx,
                            outgoing_id,
                            map_scale,
                            screen_offset,
                            1.0,
                            stroke_width_correction(fade.bucket, view_zoom),
                            view_rot_uniform,
                            rot_pivot_uniform,
                            tilt_uniform,
                            view_zoom as f32,
                            1.0,
                            terrain_org,
                            terrain_span,
                            terrain_uvfit,
                            &terrain_tex,
                            if tilt_rad > 1e-4 && pass != 0 {
                                pass_boost + (pass - 1) as f32 * 0.02
                            } else {
                                0.0
                            },
                            terrain_fill_lift,
                            );
                        }
                    }
                }
                let reused_road_pass = matches!(pass, 1 | 2)
                    && entry
                        .fade
                        .as_ref()
                        .is_some_and(|fade| fade.reuse_road_core);
                let incoming_fade = if reused_road_pass { 1.0 } else { fade_alpha };
                let height_grow = if reused_road_pass {
                    1.0
                } else if entry.fade.as_ref().is_some_and(|fade| fade.grow_heights) {
                    fade_alpha
                } else {
                    1.0
                };
                let width_correction = stroke_width_correction(entry.bucket, view_zoom);
                let pass_depth = if tilt_rad > 1e-4 && pass != 0 {
                    pass_boost + (pass - 1) as f32 * 0.02
                } else {
                    0.0
                };
                if pass == 0 {
                    let uniforms = MapDrawUniforms {
                        map_scale,
                        map_offset: screen_offset,
                        fade: incoming_fade,
                        width_correction,
                        view_rot: view_rot_uniform,
                        rot_pivot: rot_pivot_uniform,
                        tilt_params: tilt_uniform,
                        icon_zoom: view_zoom as f32,
                        height_grow,
                        terrain_org,
                        terrain_span,
                        terrain_uvfit,
                        terrain_fill_lift,
                        shadow_dir: self.draw_map.shadow_dir,
                        shadow_cast: self.draw_map.shadow_cast,
                        shadow_mask_on: self.draw_map.shadow_mask_on,
                        shadow_mask_size: self.draw_map.shadow_mask_size,
                        shadow_mask_flip: self.draw_map.shadow_mask_flip,
                        space_warp: self.draw_map.space_warp_u,
                        space_warp2: self.draw_map.space_warp2_u,
                    };
                    if let Some(geometry) = fill_geometry {
                        self.draw_fill.draw_geometry(
                            cx,
                            geometry.geometry_id(),
                            &uniforms,
                            &terrain_tex,
                            pass_depth,
                        );
                    }
                    if let Some(geometry) = fill_misc_geometry {
                        self.draw_map.draw_geometry(
                            cx,
                            geometry.geometry_id(),
                            map_scale,
                            screen_offset,
                            incoming_fade,
                            width_correction,
                            view_rot_uniform,
                            rot_pivot_uniform,
                            tilt_uniform,
                            view_zoom as f32,
                            height_grow,
                            terrain_org,
                            terrain_span,
                            terrain_uvfit,
                            &terrain_tex,
                            pass_depth,
                            terrain_fill_lift,
                        );
                    }
                } else {
                    let Some(geometry) = geometry else {
                        continue;
                    };
                    draw_map_or_road!(
                        self,
                        matches!(pass, 1 | 2),
                        cx,
                        geometry.geometry_id(),
                        map_scale,
                        screen_offset,
                        incoming_fade,
                        width_correction,
                        view_rot_uniform,
                        rot_pivot_uniform,
                        tilt_uniform,
                        view_zoom as f32,
                        height_grow,
                        terrain_org,
                        terrain_span,
                        terrain_uvfit,
                        &terrain_tex,
                        pass_depth,
                        terrain_fill_lift,
                    );
                }
                // 3D volume rides the fill pass with a ground-circle
                // distance fade from the view focus: the far field under
                // tilt (the blurred zone) skips walls/trees/roofs — the
                // bulk of the fill vertex mass. Flat views sit inside the
                // near radius everywhere, so nothing changes at top-down.
                if pass == 0 {
                    let tile_center_px = dvec2(
                        tile_offset.x + TILE_SIZE * scale * 0.5,
                        tile_offset.y + TILE_SIZE * scale * 0.5,
                    );
                    let focus = rect.pos + rect.size * 0.5;
                    let dist = ((tile_center_px.x - focus.x).powi(2)
                        + (tile_center_px.y - focus.y).powi(2))
                    .sqrt();
                    // Ring radii from the actual frustum extent: the near
                    // ring must contain every visible tile CENTER with
                    // margin (a tile diagonal), under any rotation and the
                    // full tilt stretch — visible geometry never drops
                    // below full detail (perf-never-breaks-the-picture).
                    let half_w = rect.size.x * 0.5;
                    // .max(0.087): honest at the 85° street-level cap (the
                    // clamp was tuned to the old 78° base cap's cos).
                    let mut half_h = rect.size.y * 0.5 / self.tilt_cos().max(0.087);
                    if self.space_warp_eff.is_on() {
                        // The fold sees further than the flat frustum: keep
                        // the bend region inside the full-detail ring.
                        let (reach, _) = self
                            .space_warp_eff
                            .cull_extents(rect.size.y * 0.5, half_h);
                        half_h = reach;
                    }
                    let frustum = (half_w * half_w + half_h * half_h).sqrt();
                    let near = frustum * 1.35 + TILE_SIZE * scale;
                    let far = near * 1.7;
                    let lod = (1.0 - ((dist - near) / (far - near)).clamp(0.0, 1.0)) as f32;
                    // LOD rings: near = full detail; mid = roofs + crossed-
                    // quad trees ("roofs only"); far = heights sink to 0.
                    let bands: [(&Option<Geometry>, f32, f32); 4] = [
                        (fill_3d_geometry, 0.003, 1.01),
                        (wall_geometry, 0.55, 1.01),
                        (tree_geometry, 0.55, 1.01),
                        (tree_cross_geometry, 0.003, 0.55),
                    ];
                    for (band, min_lod, max_lod) in bands {
                        if lod <= min_lod || lod > max_lod {
                            continue;
                        }
                        let Some(volume) = band else { continue };
                        let volume_id = volume.geometry_id();
                        self.draw_map.draw_geometry(
                            cx,
                            volume_id,
                            map_scale,
                            screen_offset,
                            fade_alpha,
                            stroke_width_correction(entry.bucket, view_zoom),
                            view_rot_uniform,
                            rot_pivot_uniform,
                            tilt_uniform,
                            view_zoom as f32,
                            // Distance LOD sinks heights, never alpha:
                            // translucent buildings read as broken.
                            lod * if entry.fade.as_ref().is_some_and(|fade| fade.grow_heights) {
                                fade_alpha
                            } else {
                                1.0
                            },
                            terrain_org,
                            terrain_span,
                            terrain_uvfit,
                            &terrain_tex,
                            0.0,
                            terrain_fill_lift,
                        );
                    }
                    // Instanced street trees: the near ring draws the canopy
                    // template, the mid ring the crossed stand-in, both from
                    // the same records — the LOD gates of the tree bands.
                    if !tree_instances.is_empty() {
                        let template = if lod > 0.55 {
                            tree_template_geometry.as_ref()
                        } else if lod > 0.003 {
                            tree_cross_template_geometry.as_ref()
                        } else {
                            None
                        };
                        if let Some(template) = template {
                            self.draw_map.draw_instanced(
                                cx,
                                template.geometry_id(),
                                tree_instances,
                                &MapDrawUniforms {
                                    map_scale,
                                    map_offset: screen_offset,
                                    fade: fade_alpha,
                                    width_correction: stroke_width_correction(entry.bucket, view_zoom),
                                    view_rot: view_rot_uniform,
                                    rot_pivot: rot_pivot_uniform,
                                    tilt_params: tilt_uniform,
                                    icon_zoom: view_zoom as f32,
                                    height_grow: lod
                                        * if entry.fade.as_ref().is_some_and(|fade| fade.grow_heights) {
                                            fade_alpha
                                        } else {
                                            1.0
                                        },
                                    terrain_org,
                                    terrain_span,
                                    terrain_uvfit,
                                    terrain_fill_lift,
                                    shadow_dir: self.draw_map.shadow_dir,
                                    shadow_cast: self.draw_map.shadow_cast,
                                    shadow_mask_on: self.draw_map.shadow_mask_on,
                                    shadow_mask_size: self.draw_map.shadow_mask_size,
                                    shadow_mask_flip: self.draw_map.shadow_mask_flip,
                                    space_warp: self.draw_map.space_warp_u,
                                    space_warp2: self.draw_map.space_warp2_u,
                                },
                                &terrain_tex,
                                0.0,
                            );
                        }
                    }
                    // Instanced walls follow the wall band's LOD gate.
                    if lod > 0.55 && !wall_instances.is_empty() {
                        self.draw_wall.draw_edges(
                            cx,
                            wall_instances,
                            &MapDrawUniforms {
                                map_scale,
                                map_offset: screen_offset,
                                fade: fade_alpha,
                                width_correction: stroke_width_correction(entry.bucket, view_zoom),
                                view_rot: view_rot_uniform,
                                rot_pivot: rot_pivot_uniform,
                                tilt_params: tilt_uniform,
                                icon_zoom: view_zoom as f32,
                                height_grow: lod
                                    * if entry.fade.as_ref().is_some_and(|fade| fade.grow_heights) {
                                        fade_alpha
                                    } else {
                                        1.0
                                    },
                                terrain_org,
                                terrain_span,
                                terrain_uvfit,
                                terrain_fill_lift,
                                shadow_dir: self.draw_map.shadow_dir,
                                shadow_cast: self.draw_map.shadow_cast,
                                shadow_mask_on: self.draw_map.shadow_mask_on,
                                shadow_mask_size: self.draw_map.shadow_mask_size,
                                shadow_mask_flip: self.draw_map.shadow_mask_flip,
                                space_warp: self.draw_map.space_warp_u,
                                space_warp2: self.draw_map.space_warp2_u,
                            },
                            &terrain_tex,
                            0.0,
                        );
                    }
                }
                // AA fringes ride the casing pass, but only where 1px edge
                // AA is visible: at strong tilt the tilt-shift blur and
                // 3D density hide it, and the fringes are ~2/3 of the
                // casing vertex mass on street tiles.
                if pass == 1 && self.tilt < 25.0 {
                    if let Some(fringe) = fringe_geometry {
                        let fringe_id = fringe.geometry_id();
                        self.draw_road.draw_geometry(
                            cx,
                            fringe_id,
                            map_scale,
                            screen_offset,
                            if reused_road_pass { 1.0 } else { fade_alpha },
                            stroke_width_correction(entry.bucket, view_zoom),
                            view_rot_uniform,
                            rot_pivot_uniform,
                            tilt_uniform,
                            view_zoom as f32,
                            1.0,
                            terrain_org,
                            terrain_span,
                            terrain_uvfit,
                            &terrain_tex,
                            if tilt_rad > 1e-4 { pass_boost } else { 0.0 },
                            terrain_fill_lift,
                        );
                    }
                }
            }
        }

        let geo_ms = (cx.seconds_since_app_start() - perf_start).max(0.0) * 1000.0;

        // Labels place and draw BEFORE the icon pass: charger pins must sit
        // OVER street names (EV navigator), while their own in-bubble kW
        // text redraws after the pins in draw_pin_label_phase.
        let labels_start = cx.seconds_since_app_start();
        let full_place =
            self.place_and_draw_labels(cx, &draw_tiles, view_zoom, map_offset, rect);
        let labels_ms = (cx.seconds_since_app_start() - labels_start).max(0.0) * 1000.0;
        let icons_start = cx.seconds_since_app_start();

        for pass in 3..5 {
            // Pass 4: street-band icons (zoom floor > 16) — whole band
            // skipped below the reveal zoom instead of vertex-processing
            // millions of shader-collapsed glyphs every frame.
            if pass == 4 && view_zoom < 16.25 {
                continue;
            }
            for key in &draw_tiles {
                let Some(entry) = self.tiles.get(key) else {
                    continue;
                };
                let TileLoadState::Ready {
                    fill_geometry,
                    casing_geometry,
                    stroke_geometry,
                    icon_geometry,
                    icon_high_geometry,
                    icon_instances,
                    icon_high_instances,
                    ..
                } = &entry.state
                else {
                    continue;
                };
                // Stale higher-bucket tiles keep their baked symbols until
                // the rebuild lands. Charger pins bake from z9 so the pass
                // itself stays on when zoomed out — but a stale POI-carpet
                // tile (baked at z16+) hides its symbols the moment the
                // view drops below icon level, instead of splattering
                // hundreds of full-size shop icons across the region.
                if pass >= 3
                    && (view_zoom < 7.75
                        || (entry.bucket >= ICON_MIN_ZOOM
                            && view_zoom < ICON_MIN_ZOOM as f64 - 0.25))
                {
                    continue;
                }
                let geometry = match pass {
                    0 => fill_geometry,
                    1 => casing_geometry,
                    2 => stroke_geometry,
                    3 => icon_geometry,
                    _ => icon_high_geometry,
                };
                let scale = 2.0_f64.powf(view_zoom - key.z as f64);
                let tile_offset = map_offset
                    + dvec2(
                        key.x as f64 * TILE_SIZE * scale,
                        key.y as f64 * TILE_SIZE * scale,
                    );
                let map_scale = Vec2f {
                    x: scale as f32,
                    y: scale as f32,
                };
                let screen_offset = Vec2f {
                    x: tile_offset.x as f32,
                    y: tile_offset.y as f32,
                };
                let mut fade_alpha = 1.0_f32;
                if let Some(fade) = &entry.fade {
                    fade_alpha = (((perf_start - fade.started).max(0.0) / TILE_FADE_SECONDS)
                        as f32)
                        .clamp(0.0, 1.0);
                    let outgoing = match pass {
                        0 => &fade.fill_geometry,
                        1 => &fade.casing_geometry,
                        2 => &fade.stroke_geometry,
                        3 => &fade.icon_geometry,
                        _ => &None,
                    };
                    if let Some(outgoing) = outgoing {
                        let outgoing_id = outgoing.geometry_id();
                        draw_map_or_road!(
                            self,
                            matches!(pass, 1 | 2),
                            cx,
                            outgoing_id,
                            map_scale,
                            screen_offset,
                            1.0,
                            stroke_width_correction(fade.bucket, view_zoom),
                            view_rot_uniform,
                            rot_pivot_uniform,
                            tilt_uniform,
                            view_zoom as f32,
                            1.0,
                            terrain_org,
                            terrain_span,
                            terrain_uvfit,
                            &terrain_tex,
                            if tilt_rad > 1e-4 && pass != 0 {
                                pass_boost + (pass - 1) as f32 * 0.02
                            } else {
                                0.0
                            },
                            terrain_fill_lift,
                        );
                    }
                }
                // Instanced POI symbols ride the same pass as the vertex-baked
                // decals: the outgoing generation first (cross-fade), then the
                // resident groups.
                if pass >= 3 {
                    let pass_depth = if tilt_rad > 1e-4 {
                        pass_boost + (pass - 1) as f32 * 0.02
                    } else {
                        0.0
                    };
                    let mut uniforms = MapDrawUniforms {
                        map_scale,
                        map_offset: screen_offset,
                        fade: 1.0,
                        width_correction: stroke_width_correction(entry.bucket, view_zoom),
                        view_rot: view_rot_uniform,
                        rot_pivot: rot_pivot_uniform,
                        tilt_params: tilt_uniform,
                        icon_zoom: view_zoom as f32,
                        height_grow: 1.0,
                        terrain_org,
                        terrain_span,
                        terrain_uvfit,
                        terrain_fill_lift,
                        shadow_dir: self.draw_map.shadow_dir,
                        shadow_cast: self.draw_map.shadow_cast,
                        shadow_mask_on: self.draw_map.shadow_mask_on,
                        shadow_mask_size: self.draw_map.shadow_mask_size,
                        shadow_mask_flip: self.draw_map.shadow_mask_flip,
                        space_warp: self.draw_map.space_warp_u,
                        space_warp2: self.draw_map.space_warp2_u,
                    };
                    if pass == 3 {
                        if let Some(fade) = &entry.fade {
                            if !fade.icon_instances.is_empty() {
                                uniforms.width_correction =
                                    stroke_width_correction(fade.bucket, view_zoom);
                                self.draw_icon.draw_groups(
                                    cx,
                                    &fade.icon_instances,
                                    &mut self.icon_mesh_geometries,
                                    &uniforms,
                                    &terrain_tex,
                                    pass_depth,
                                );
                                uniforms.width_correction =
                                    stroke_width_correction(entry.bucket, view_zoom);
                            }
                        }
                    }
                    let groups = if pass == 3 { icon_instances } else { icon_high_instances };
                    if !groups.is_empty() {
                        uniforms.fade = fade_alpha;
                        uniforms.height_grow =
                            if entry.fade.as_ref().is_some_and(|fade| fade.grow_heights) {
                                fade_alpha
                            } else {
                                1.0
                            };
                        self.draw_icon.draw_groups(
                            cx,
                            groups,
                            &mut self.icon_mesh_geometries,
                            &uniforms,
                            &terrain_tex,
                            pass_depth,
                        );
                    }
                }
                let Some(geometry) = geometry else {
                    continue;
                };
                let geometry_id = geometry.geometry_id();
                self.draw_map.draw_geometry(
                    cx,
                    geometry_id,
                    map_scale,
                    screen_offset,
                    fade_alpha,
                    stroke_width_correction(entry.bucket, view_zoom),
                    view_rot_uniform,
                    rot_pivot_uniform,
                    tilt_uniform,
                    view_zoom as f32,
                    if entry.fade.as_ref().is_some_and(|fade| fade.grow_heights) {
                        fade_alpha
                    } else {
                        1.0
                    },
                    terrain_org,
                    terrain_span,
                    terrain_uvfit,
                    &terrain_tex,
                    if tilt_rad > 1e-4 && pass != 0 {
                                pass_boost + (pass - 1) as f32 * 0.02
                            } else {
                                0.0
                            },
                    terrain_fill_lift,
                );
            }
        }


        // Pin-class label text (white kW numbers) over the pins.
        // Rain radar overlay: over all map content, under labels/UI. The
        // quad's corners go through the overlay camera so it sticks to the
        // map in every projection.
        if !self.rain_frames.is_empty() {
            let camera = self.overlay_camera();
            let (west, south, east, north) = self.rain_bbox;
            let nw = lon_lat_to_normalized(west, north);
            let ne = lon_lat_to_normalized(east, north);
            let se = lon_lat_to_normalized(east, south);
            let sw = lon_lat_to_normalized(west, south);
            // In 3D the rain deck hovers like cloud cover: lift the whole
            // quad by ~650 m of parallax so it visibly sits ABOVE the city.
            let cloud_lift = self.lift_screen_px(650.0, view_zoom);
            let lift = dvec2(0.0, -cloud_lift);
            let c0 = camera.norm_to_screen(nw) + lift;
            let c1 = camera.norm_to_screen(ne) + lift;
            let c2 = camera.norm_to_screen(se) + lift;
            let c3 = camera.norm_to_screen(sw) + lift;
            let frame_index = self.rain_frame_index % self.rain_frames.len();
            // The "now" frame swaps in the hi-res dual-radar composite when
            // one is loaded; forecast frames stay at nowcast resolution.
            let (texture, tex_size) = match (frame_index, &self.rain_now_hires) {
                (0, Some((texture, size))) => (texture.clone(), *size),
                _ => (self.rain_frames[frame_index].clone(), self.rain_tex_size),
            };
            self.draw_rain.draw_super.draw_vars.set_texture(0, &texture);
            self.draw_rain.c0 = Vec2f { x: c0.x as f32, y: c0.y as f32 };
            self.draw_rain.c1 = Vec2f { x: c1.x as f32, y: c1.y as f32 };
            self.draw_rain.c2 = Vec2f { x: c2.x as f32, y: c2.y as f32 };
            self.draw_rain.c3 = Vec2f { x: c3.x as f32, y: c3.y as f32 };
            self.draw_rain.texel = Vec2f {
                x: 1.0 / tex_size.0 as f32,
                y: 1.0 / tex_size.1 as f32,
            };
            let min_x = c0.x.min(c1.x).min(c2.x).min(c3.x);
            let min_y = c0.y.min(c1.y).min(c2.y).min(c3.y);
            let max_x = c0.x.max(c1.x).max(c2.x).max(c3.x);
            let max_y = c0.y.max(c1.y).max(c2.y).max(c3.y);
            self.draw_rain.draw_abs(
                cx,
                Rect {
                    pos: dvec2(min_x, min_y),
                    size: dvec2(max_x - min_x, max_y - min_y),
                },
            );
        }

        self.draw_pin_label_phase(cx);
        self.draw_wind_particles(cx);


        // Put draw_tiles back into scratch buffer (preserves allocation)
        self.scratch_draw_tiles = draw_tiles;

        // Interaction overlay: route polyline, markers, position puck —
        // always on top of tiles and labels.
        if !self.overlay.is_empty() {
            let camera = OverlayCamera {
                world_size,
                offset: map_offset,
                rect,
                meters_per_px: {
                    let (_, lat) = normalized_to_lon_lat(self.center_norm);
                    40_075_016.686 * lat.to_radians().cos() / world_size
                },
                rot: (rot_cos, rot_sin),
                rot_pivot,
                rotation_deg: self.rotation,
                tilt_cos: tilt_rad.cos(),
                warp: self.space_warp_eff,
            };
            let mut overlay = std::mem::take(&mut self.overlay);
            overlay.route_glow = self.active_style().shiny.route_glow;
            draw_map_overlay(cx, &mut self.draw_overlay, &camera, &mut overlay);
            self.overlay = overlay;
        }

        let total_ms = (cx.seconds_since_app_start() - perf_start).max(0.0) * 1000.0;
        // icons_ms spans the icon pass through overlays; tail = whatever
        // the section timers do not cover (uniform churn, overhead).
        let icons_ms = (cx.seconds_since_app_start() - icons_start).max(0.0) * 1000.0;
        self.perf_ms_icons += icons_ms;
        self.perf_ms_tail += (total_ms - geo_ms - labels_ms - icons_ms).max(0.0);
        self.perf_frames += 1;
        self.perf_ms_total += total_ms;
        self.perf_ms_geo += geo_ms;
        self.perf_ms_labels += labels_ms;
        self.perf_ms_max = self.perf_ms_max.max(total_ms);
        if full_place {
            self.perf_label_full_places += 1;
        }
        if self.is_local_archive() && self.perf_frames >= 240 {
            use std::io::Write;
            // GPU time per presented frame from the platform monitor
            // (command-buffer start->end, completion-handler thread).
            cx.cx.perf_monitor.set_enabled(true);
            let mut gpu_frames = Vec::new();
            cx.cx.perf_monitor.read(&mut gpu_frames);
            let mut gpu_sum_us = 0u64;
            let mut gpu_max_us = 0u32;
            let mut gpu_n = 0u64;
            for frame in &gpu_frames {
                let us = frame.channel_us[crate::makepad_draw::makepad_platform::perf_monitor::PERF_CHANNEL_GPU.0];
                if us > 0 {
                    gpu_sum_us += us as u64;
                    gpu_max_us = gpu_max_us.max(us);
                    gpu_n += 1;
                }
            }
            let gpu_avg_ms = if gpu_n > 0 { gpu_sum_us as f64 / gpu_n as f64 / 1000.0 } else { 0.0 };
            let gpu_max_ms = gpu_max_us as f64 / 1000.0;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("local/map_perf.log")
            {
                let frames = self.perf_frames as f64;
                let gap_avg = if self.perf_gap_count > 0 {
                    self.perf_gap_sum / self.perf_gap_count as f64
                } else {
                    0.0
                };
                let _ = writeln!(
                    file,
                    "frames:{} avg_ms:{:.2} geo_ms:{:.2} labels_ms:{:.2} icons_ms:{:.2} tail_ms:{:.2} max_ms:{:.2} gpu_ms:{:.2} gpu_max:{:.2} gap_avg_ms:{:.2} gap_max_ms:{:.2} gaps>12ms:{}/{} full_places:{} glyphs:{} z:{:.2}",
                    self.perf_frames,
                    self.perf_ms_total / frames,
                    self.perf_ms_geo / frames,
                    self.perf_ms_labels / frames,
                    self.perf_ms_icons / frames,
                    self.perf_ms_tail / frames,
                    self.perf_ms_max,
                    gpu_avg_ms,
                    gpu_max_ms,
                    gap_avg,
                    self.perf_ms_gap_max,
                    self.perf_gaps_over_12ms,
                    self.perf_gap_count,
                    self.perf_label_full_places,
                    self.label_perf.drawn_glyphs,
                    view_zoom,
                );
            }
            self.perf_frames = 0;
            self.perf_ms_total = 0.0;
            self.perf_ms_geo = 0.0;
            self.perf_ms_icons = 0.0;
            self.perf_ms_tail = 0.0;
            self.perf_ms_labels = 0.0;
            self.perf_ms_max = 0.0;
            self.perf_ms_gap_max = 0.0;
            self.perf_gap_sum = 0.0;
            self.perf_gap_count = 0;
            self.perf_gaps_over_12ms = 0;
            self.perf_label_full_places = 0;
        }

        self.update_status_text();
        // Viewport debug readout: the exact @cam command for this view, so
        // a screenshot alone is enough to recreate the camera. On by
        // default — it is how map work has always been driven — and
        // gated so an app shipping the map as a face, not a workbench,
        // can turn it off.
        if self.debug_cam {
            let center = self.center_norm;
            let lon = center.x * 360.0 - 180.0;
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * center.y))
                .sinh()
                .atan()
                .to_degrees();
            let cam = format!(
                "@cam {:.5} {:.5} {:.2} {:.0} {:.0}",
                lon,
                lat,
                self.view_zoom(),
                self.rotation,
                self.tilt
            );
            self.draw_text.draw_abs(
                cx,
                dvec2(rect.pos.x + rect.size.x - 260.0, rect.pos.y + rect.size.y - 64.0),
                &cam,
            );
        }
        cx.end_turtle();
        DrawStep::done()
    }
}

impl WidgetMatchEvent for MapView {
    fn handle_http_response(
        &mut self,
        cx: &mut Cx,
        request_id: LiveId,
        response: &HttpResponse,
        _scope: &mut Scope,
    ) {
        let Some(pending) = self.request_to_tile.remove(&request_id) else {
            return;
        };
        let tile_key = pending.tile_key;
        let endpoint = pending.endpoint;

        if response.status_code != 200 {
            let preview = response
                .get_string_body()
                .unwrap_or_default()
                .chars()
                .take(120)
                .collect::<String>();
            self.mark_tile_failed(
                tile_key,
                &format!(
                    "endpoint {} http status {} body: {}",
                    endpoint, response.status_code, preview
                ),
            );
            self.update_status_text();
            self.redraw(cx);
            return;
        }

        let Some(body) = response.get_string_body() else {
            self.mark_tile_failed(
                tile_key,
                &format!("endpoint {} missing utf8 response body", endpoint),
            );
            self.update_status_text();
            self.redraw(cx);
            return;
        };

        // Offload heavy JSON parsing + tessellation to the thread pool
        let sender = self.tile_worker_rx.sender();
        let style_epoch = self.style_epoch;
        let theme_style = self.active_style().clone();
        let bucket = self.render_bucket();

        if let Err(error) = self.submit_tile_job(cx, tile_key, move || {
            match build_tile_buffers_from_body(tile_key, &body, &theme_style, bucket) {
                Ok(buffers) => {
                    store_tile_data_cache_on_disk(tile_key, &body);
                    let _ = sender.send(TileWorkerMessage::NetworkTileParsed {
                        style_epoch,
                        tile_key,
                        buffers,
                    });
                }
                Err(err) => {
                    let _ = sender.send(TileWorkerMessage::NetworkTileParseFailed {
                        style_epoch,
                        tile_key,
                        error: err,
                    });
                }
            }
        }) {
            self.mark_tile_failed(tile_key, &format!("tile worker submission failed: {error}"));
            self.update_status_text();
            self.redraw(cx);
        }
    }

    fn handle_http_request_error(
        &mut self,
        cx: &mut Cx,
        request_id: LiveId,
        err: &HttpError,
        _scope: &mut Scope,
    ) {
        let Some(pending) = self.request_to_tile.remove(&request_id) else {
            return;
        };
        self.mark_tile_failed(
            pending.tile_key,
            &format!(
                "endpoint {} http request error: {:?}",
                pending.endpoint, err
            ),
        );
        self.update_status_text();
        self.redraw(cx);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl MapView {
    fn handle_archive_watch(&mut self, cx: &mut Cx, event: &Event) {
        while let Ok(watch) = self.archive_watch_rx.try_recv() {
            self.archive_watch_in_flight = false;
            if !self.is_local_archive() || watch.path != self.active_mbtiles_path() {
                continue;
            }
            if let Some(range) = watch.zoom_range {
                self.local_source_zoom_range = Some(range);
                self.local_source_zoom_range_checked = true;
            }
            if watch.mtime != self.archive_watch_mtime {
                let had = self.archive_watch_mtime.is_some() || watch.mtime.is_some();
                self.archive_watch_mtime = watch.mtime;
                if had {
                    if let Some(config) = self.tile_source_config.clone() {
                        self.install_archive_source(cx, config);
                    }
                    self.local_requested_tiles.clear();
                    let before = self.tiles.len();
                    self.tiles
                        .retain(|_, entry| matches!(entry.state, TileLoadState::Ready { .. }));
                    if self.tiles.len() != before {
                        log!(
                            "MapView: archive changed — cleared {} pending/failed tiles for reload",
                            before - self.tiles.len()
                        );
                    }
                    self.redraw(cx);
                }
            }
        }

        // Growing-archive watch: both directory probing and metadata stay on
        // the persistent archive pool; the UI only consumes the timestamp.
        if self.is_local_archive()
            && (self.archive_watch_timer.is_event(event).is_some()
                || (self.archive_watch_timer.is_empty() && self.use_local_mbtiles))
        {
            if !self.archive_watch_in_flight {
                self.archive_watch_in_flight = true;
                let path = self.active_mbtiles_path().to_string();
                let sender = self.archive_watch_rx.sender();
                let workers = self.ensure_archive_worker_pool(cx);
                match workers.submit(next_archive_task_token(), move || {
                    let archive_path = std::path::PathBuf::from(&path);
                    let probe = if is_mkmap_path_shape(&path) {
                        if archive_path.file_name().is_some_and(|name| name == "root.mkidx") {
                            archive_path
                        } else {
                            archive_path.join("root.mkidx")
                        }
                    } else {
                        archive_path
                    };
                    let mtime = archive_mtime(&probe);
                    let zoom_range = makepad_mbtile_reader::TileArchiveReader::open(
                        Path::new(&path),
                    )
                    .ok()
                    .and_then(|mut reader| reader.validated_zoom_range());
                    let _ = sender.send(ArchiveWatchResult {
                        path,
                        mtime,
                        zoom_range,
                    });
                }) {
                    Ok(()) => {}
                    Err(error) => {
                        self.archive_watch_in_flight = false;
                        log!("MapView: archive watch submission failed: {error}");
                    }
                }
            }
            self.archive_watch_timer = cx.start_timeout(5.0);
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl MapView {
    fn handle_archive_watch(&mut self, _cx: &mut Cx, _event: &Event) {}
}

// --- MapView impl ---

impl MapView {
    fn rebuild_compiled_styles(&mut self) {
        self.compiled_style_light = self.style_light.compile();
        self.compiled_style_dark = self.style_dark.compile();
        self.compiled_style_circuit = self.style_circuit.compile();
    }

    fn effective_theme(&self) -> u32 {
        if self.theme_select > 0 {
            self.theme_select.min(2)
        } else if self.dark_theme {
            1
        } else {
            0
        }
    }

    fn active_style(&self) -> &CompiledMapTheme {
        match self.effective_theme() {
            2 => &self.compiled_style_circuit,
            1 => &self.compiled_style_dark,
            _ => &self.compiled_style_light,
        }
    }

    /// The Inception fold ON/OFF (the SETTING — remembers intent; the fold
    /// itself tweens in only while the camera is in a close 3D view and
    /// tweens back out when it leaves one).
    pub fn set_space_warp(&mut self, cx: &mut Cx, on: bool) {
        if self.space_warp_want == on {
            return;
        }
        self.space_warp_want = on;
        // Restyle resident tiles both ways: turning ON re-uploads the
        // ground meshes chord-refined against the fold (insert_ready_tile),
        // turning OFF restores the pristine buffers so flat mode stays
        // byte-identical to a session that never warped. Old geometry
        // stays on screen while replacements stream in (bucket sentinel),
        // so the 600ms tween starts immediately on the coarse meshes.
        self.restyle_tiles_keep_stale(cx);
        self.redraw(cx);
    }

    pub fn space_warp(&self) -> bool {
        self.space_warp_want
    }

    /// Whether the near-ground regime for the space-warp mode is active
    /// (strong tilt + close zoom). The UI grays the toggle outside it; the
    /// draw loop uses the same predicate to auto-tween the effect out and
    /// back — ONE source of truth for the regime.
    pub fn space_warp_available(&self) -> bool {
        let tilt_rad = self.tilt.clamp(0.0, TILT_HARD_MAX_DEG).to_radians();
        tilt_rad > 0.55 && self.view_zoom() >= 15.0
    }

    /// Switch the active theme (0 light, 1 dark, 2 circuit city) with the
    /// keep-stale restyle: resident tiles keep drawing in the old palette
    /// and cross-fade per tile as their rebake lands.
    pub fn set_theme(&mut self, cx: &mut Cx, theme: u32) {
        let theme = theme.min(2);
        if self.effective_theme() == theme {
            return;
        }
        self.theme_select = theme;
        // Circuit (2) is a dark background too: label classes, halos and
        // the other dark_theme-gated rendering must use the dark palette
        // (style choice itself follows theme_select, not this flag).
        self.dark_theme = theme != 0;
        self.applied_dark_theme = Some(self.dark_theme);
        self.apply_theme_palette();
        self.restyle_tiles_keep_stale(cx);
        self.update_status_text();
        self.redraw(cx);
    }

    fn normalize_source_mode(&mut self) {
        if self.use_local_mbtiles && self.use_network {
            log!("MapView: both sources enabled; selecting OFFLINE mode (mbtiles only). Set use_local_mbtiles:false for ONLINE mode.");
            self.use_network = false;
        } else if !self.use_local_mbtiles && !self.use_network {
            log!("MapView: no source enabled; selecting OFFLINE mode (mbtiles only).");
            self.use_local_mbtiles = true;
        }
    }



    fn apply_theme_change(&mut self) {
        self.style_epoch = self.style_epoch.wrapping_add(1);
        if self.style_epoch == 0 {
            self.style_epoch = 1;
        }
        self.apply_theme_palette();
        self.tiles.clear();
        self.request_to_tile.clear();
        self.local_requested_tiles.clear();
        self.pending_ready_tiles.clear();
        self.tiles_generation = self.tiles_generation.wrapping_add(1);
        self.label_cache_valid = false;
    }

    fn apply_theme_palette(&mut self) {
        let (background, label) = {
            let style = self.active_style();
            (style.background, style.label)
        };
        self.draw_bg.color = background;
        self.draw_label.draw_super.color = label;
        self.draw_text.color = vec4(0.0, 0.0, 0.0, 1.0);
        // The background floor sits below the tilted map's negative depth
        // domain; everything drawn later (labels, panels, overlay) keeps
        // winning by ordinary call order.
        self.draw_bg.draw_depth = -50.0;
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }

    fn tile_units_per_m(key: TileKey) -> f32 {
        let n = (1u32 << key.z) as f64;
        let lat = (std::f64::consts::PI * (1.0 - 2.0 * (key.y as f64 + 0.5) / n))
            .sinh()
            .atan();
        (TILE_SIZE * n / (40_075_016.686 * lat.cos())) as f32
    }

    fn draw_shadow_mask_pass(
        &mut self,
        cx: &mut Cx2d,
        draw_tiles: &[TileKey],
        view_zoom: f64,
        map_offset: Vec2d,
        view_rot: [f32; 2],
        rot_pivot: [f32; 2],
        tilt_params: [f32; 4],
        terrain_org: [f32; 2],
        terrain_span: [f32; 2],
        terrain_uvfit: [f32; 4],
        terrain_tex: &Texture,
        terrain_fill_lift: f32,
        rect: Rect,
    ) {
        if self.shadow_mask_pass.is_none() {
            let pass = DrawPass::new(cx);
            let tex = Texture::new_with_format(
                cx,
                TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Auto,
                    initial: true,
                },
            );
            pass.set_color_texture(
                cx,
                &tex,
                DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
            );
            self.shadow_mask_pass = Some(pass);
            self.shadow_mask_texture = Some(tex);
            self.shadow_mask_list = Some(DrawList2d::new(cx));
        }
        let pass = self.shadow_mask_pass.as_ref().unwrap();
        pass.set_size(cx, rect.size);
        cx.set_pass_area(pass, self.draw_bg.area());
        cx.make_child_pass(pass);
        let dpi = cx.current_dpi_factor();
        cx.begin_pass(pass, Some(dpi));
        self.shadow_mask_list.as_mut().unwrap().begin_always(cx);

        // Nothing drawn into the mask may sample it: binding the target
        // texture as a sampler is a feedback loop (a WebGL error flood and
        // a blank map; undefined on Metal).
        self.draw_map.shadow_mask = None;
        self.draw_shadow.shadow_mask = None;
        self.draw_map.shadow_mask_on = 0.0;
        self.draw_map.shadow_mask_size = [rect.size.x as f32, rect.size.y as f32];
        let shadow_mask_size = self.draw_map.shadow_mask_size;
        let space_warp = self.draw_map.space_warp_u;
        let space_warp2 = self.draw_map.space_warp2_u;
        let sun_2d = self.draw_map.shiny.sun.dir_2d();
        let len_per_m = self.draw_map.shiny.sun.shadow_len_per_m();
        let (sx, sy) = (-sun_2d.x, -sun_2d.y);

        for key in draw_tiles {
            let Some(entry) = self.tiles.get(key) else {
                continue;
            };
            let TileLoadState::Ready {
                fill_3d_geometry,
                casing_geometry,
                stroke_geometry,
                shadow_disc_geometry,
                wall_instances,
                ..
            } = &entry.state
            else {
                continue;
            };
            let scale = 2.0_f64.powf(view_zoom - key.z as f64);
            let tile_offset = map_offset
                + dvec2(
                    key.x as f64 * TILE_SIZE * scale,
                    key.y as f64 * TILE_SIZE * scale,
                );
            let map_scale = Vec2f {
                x: scale as f32,
                y: scale as f32,
            };
            let screen_offset = Vec2f {
                x: tile_offset.x as f32,
                y: tile_offset.y as f32,
            };
            let units_per_m = Self::tile_units_per_m(*key);
            let shadow_dir = [sx * len_per_m * units_per_m, sy * len_per_m * units_per_m];
            self.draw_map.shadow_dir = shadow_dir;
            let width_correction = stroke_width_correction(entry.bucket, view_zoom);
            let uniforms = |height_grow: f32, shadow_cast: f32| MapDrawUniforms {
                map_scale,
                map_offset: screen_offset,
                fade: 1.0,
                width_correction,
                view_rot,
                rot_pivot,
                tilt_params,
                icon_zoom: view_zoom as f32,
                height_grow,
                terrain_org,
                terrain_span,
                terrain_uvfit,
                terrain_fill_lift,
                shadow_dir,
                shadow_cast,
                shadow_mask_on: 0.0,
                shadow_mask_size,
                shadow_mask_flip: 0.0,
                space_warp,
                space_warp2,
            };

            // a. Wall silhouette quads along the sun.
            if !wall_instances.is_empty() {
                self.draw_shadow.draw_edges(
                    cx,
                    wall_instances,
                    &uniforms(1.0, 0.0),
                    terrain_tex,
                    0.0,
                );
            }

            // b. Roof / deck projections of lifted geometry.
            self.draw_map.shadow_cast = 1.0;
            self.draw_road.shadow_cast = 1.0;
            self.draw_road.shadow_dir = shadow_dir;
            self.draw_road.shadow_mask = None;
            self.draw_road.shadow_mask_on = 0.0;
            for (geometry, road) in [
                (fill_3d_geometry, false),
                (casing_geometry, true),
                (stroke_geometry, true),
            ] {
                let Some(geometry) = geometry else {
                    continue;
                };
                draw_map_or_road!(
                    self,
                    road,
                    cx,
                    geometry.geometry_id(),
                    map_scale,
                    screen_offset,
                    1.0,
                    width_correction,
                    view_rot,
                    rot_pivot,
                    tilt_params,
                    view_zoom as f32,
                    1.0,
                    terrain_org,
                    terrain_span,
                    terrain_uvfit,
                    terrain_tex,
                    0.0,
                    terrain_fill_lift,
                );
            }

            // c. Footprint cut-out at ground.
            self.draw_map.shadow_cast = 2.0;
            if let Some(geometry) = fill_3d_geometry {
                self.draw_map.draw_geometry(
                    cx,
                    geometry.geometry_id(),
                    map_scale,
                    screen_offset,
                    1.0,
                    width_correction,
                    view_rot,
                    rot_pivot,
                    tilt_params,
                    view_zoom as f32,
                    0.0,
                    terrain_org,
                    terrain_span,
                    terrain_uvfit,
                    terrain_tex,
                    0.0,
                    terrain_fill_lift,
                );
            }

            // d. Tree / signal contact discs.
            self.draw_map.shadow_cast = 3.0;
            if let Some(geometry) = shadow_disc_geometry {
                self.draw_map.draw_geometry(
                    cx,
                    geometry.geometry_id(),
                    map_scale,
                    screen_offset,
                    1.0,
                    width_correction,
                    view_rot,
                    rot_pivot,
                    tilt_params,
                    view_zoom as f32,
                    0.0,
                    terrain_org,
                    terrain_span,
                    terrain_uvfit,
                    terrain_tex,
                    0.0,
                    terrain_fill_lift,
                );
            }
            let _ = uniforms;
        }

        self.draw_map.shadow_cast = 0.0;
        self.draw_map.shadow_dir = [0.0, 0.0];
        self.draw_road.shadow_cast = 0.0;
        self.draw_road.shadow_dir = [0.0, 0.0];
        self.shadow_mask_list.as_mut().unwrap().end(cx);
        let pass = self.shadow_mask_pass.as_ref().unwrap();
        cx.end_pass(pass);
    }

    fn insert_ready_tile(&mut self, cx: &mut Cx, tile_key: TileKey, mut buffers: TileBuffers) {
        // An overlay-only result has no roads of its own. Never accept it if
        // eviction, a zoom restyle, or another transition replaced the exact
        // resident core it was built to reuse; leave the current entry
        // untouched so the normal request path schedules a full bake.
        if buffers.mode_overlay_only
            && !self.tiles.get(&tile_key).is_some_and(|entry| {
                entry.bucket == buffers.render_zoom
                    && entry.road_core_cached
                    && matches!(entry.state, TileLoadState::Ready { .. })
            })
        {
            return;
        }
        let old_entry = self.tiles.remove(&tile_key);
        let (
            old_bucket,
            old_baked_3d,
            old_fill,
            old_fill_misc,
            old_casing,
            old_stroke,
            old_icon,
            old_feature_count,
            old_bytes,
            old_road_core_cached,
            old_road_icon_indices,
            old_road_icon_vertices,
            old_icon_instances,
        ) = match old_entry {
            Some(TileEntry {
                state:
                    TileLoadState::Ready {
                        fill_geometry,
                        fill_misc_geometry,
                        casing_geometry,
                        stroke_geometry,
                        icon_geometry,
                        icon_instances,
                feature_count,
                ..
            },
                bytes,
                bucket,
                baked_3d,
                road_core_cached,
                road_icon_indices,
                road_icon_vertices,
                ..
            }) => (
                bucket,
                baked_3d,
                fill_geometry,
                fill_misc_geometry,
                casing_geometry,
                stroke_geometry,
                icon_geometry,
                feature_count,
                bytes,
                road_core_cached,
                road_icon_indices,
                road_icon_vertices,
                icon_instances,
            ),
            _ => (
                buffers.render_zoom,
                false,
                None,
                None,
                None,
                None,
                None,
                0,
                0,
                false,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
        };

        // A mode-only bake is valid only while the exact same render-bucket
        // road core is still resident. Append its cached arrow subset to the
        // replacement POI buffer and move (not duplicate) its GPU meshes.
        let reuse_road_core = buffers.mode_overlay_only
            && old_road_core_cached
            && old_bucket == buffers.render_zoom;
        if reuse_road_core {
            buffers.append_cached_road_icons(
                &old_road_icon_indices,
                &old_road_icon_vertices,
            );
        }
        let tile_bytes = if reuse_road_core {
            buffers.byte_size().max(old_bytes)
        } else {
            buffers.byte_size()
        };
        // Space-warp mode: refine long chords in the ground meshes before
        // upload. A flat triangle with far-apart vertices (full-tile land
        // sheets, long straight road quads) warps only at its corners, so
        // its chord slices straight through the curled fold — both in
        // screen position and in the interpolated ground-rel depth. The
        // triangulator's output is midpoint-split (crack-free, shared
        // midpoints) until edges are short against the curl radius; the
        // toggle restyles resident tiles both ways, so flat mode keeps its
        // pristine (byte-identical) buffers.
        if self.space_warp_want {
            let scale = (self.view_zoom() - tile_key.z as f64).exp2().max(1.0);
            let max_edge = (64.0 / scale).clamp(4.0, 64.0) as f32;
            crate::makepad_draw::vector::subdivide_fill_packed_mesh(
                &mut buffers.fill_indices,
                &mut buffers.fill_vertices,
                max_edge,
            );
            crate::makepad_draw::vector::subdivide_packed_mesh(
                &mut buffers.fill_misc_indices,
                &mut buffers.fill_misc_vertices,
                max_edge,
            );
            crate::makepad_draw::vector::subdivide_road_mesh(
                &mut buffers.casing_indices,
                &mut buffers.casing_vertices,
                max_edge,
            );
            crate::makepad_draw::vector::subdivide_road_mesh(
                &mut buffers.stroke_indices,
                &mut buffers.stroke_vertices,
                max_edge,
            );
            crate::makepad_draw::vector::subdivide_road_mesh(
                &mut buffers.fringe_indices,
                &mut buffers.fringe_vertices,
                max_edge,
            );
        }
        let fill_geometry = if !buffers.fill_indices.is_empty() && !buffers.fill_vertices.is_empty()
        {
            let geometry = Geometry::new(cx);
            geometry.update(cx, buffers.fill_indices, buffers.fill_vertices);
            Some(geometry)
        } else {
            None
        };
        let fill_misc_geometry = if !buffers.fill_misc_indices.is_empty()
            && !buffers.fill_misc_vertices.is_empty()
        {
            let geometry = Geometry::new(cx);
            geometry.update(cx, buffers.fill_misc_indices, buffers.fill_misc_vertices);
            Some(geometry)
        } else {
            None
        };

        let new_casing_geometry =
            if !buffers.casing_indices.is_empty() && !buffers.casing_vertices.is_empty() {
                let geometry = Geometry::new(cx);
                geometry.update(cx, buffers.casing_indices, buffers.casing_vertices);
                Some(geometry)
            } else {
                None
            };
        let (casing_geometry, fade_casing_geometry) = if reuse_road_core {
            (old_casing, None)
        } else {
            (new_casing_geometry, old_casing)
        };

        let new_stroke_geometry =
            if !buffers.stroke_indices.is_empty() && !buffers.stroke_vertices.is_empty() {
                let geometry = Geometry::new(cx);
                geometry.update(cx, buffers.stroke_indices, buffers.stroke_vertices);
                Some(geometry)
            } else {
                None
            };
        let (stroke_geometry, fade_stroke_geometry) = if reuse_road_core {
            (old_stroke, None)
        } else {
            (new_stroke_geometry, old_stroke)
        };

        let icon_geometry = if !buffers.icon_indices.is_empty() && !buffers.icon_vertices.is_empty()
        {
            let geometry = Geometry::new(cx);
            geometry.update(cx, buffers.icon_indices, buffers.icon_vertices);
            Some(geometry)
        } else {
            None
        };
        let icon_high_geometry = if !buffers.icon_high_indices.is_empty()
            && !buffers.icon_high_vertices.is_empty()
        {
            let geometry = Geometry::new(cx);
            geometry.update(cx, buffers.icon_high_indices, buffers.icon_high_vertices);
            Some(geometry)
        } else {
            None
        };
        let shadow_disc_geometry = if !buffers.shadow_disc_indices.is_empty()
            && !buffers.shadow_disc_vertices.is_empty()
        {
            let geometry = Geometry::new(cx);
            geometry.update(cx, buffers.shadow_disc_indices, buffers.shadow_disc_vertices);
            Some(geometry)
        } else {
            None
        };
        let fringe_geometry = if !buffers.fringe_indices.is_empty()
            && !buffers.fringe_vertices.is_empty()
        {
            let geometry = Geometry::new(cx);
            geometry.update(cx, buffers.fringe_indices, buffers.fringe_vertices);
            Some(geometry)
        } else {
            None
        };
        let fill_3d_geometry = if !buffers.fill_3d_indices.is_empty()
            && !buffers.fill_3d_vertices.is_empty()
        {
            let geometry = Geometry::new(cx);
            geometry.update(cx, buffers.fill_3d_indices, buffers.fill_3d_vertices);
            Some(geometry)
        } else {
            None
        };
        let mut band = |indices: Vec<u32>, vertices: Vec<f32>| {
            if indices.is_empty() || vertices.is_empty() {
                None
            } else {
                let geometry = Geometry::new(cx);
                geometry.update(cx, indices, vertices);
                Some(geometry)
            }
        };
        let wall_geometry = band(buffers.wall_indices, buffers.wall_vertices);
        let tree_geometry = band(buffers.tree_indices, buffers.tree_vertices);
        let tree_cross_geometry =
            band(buffers.tree_cross_indices, buffers.tree_cross_vertices);
        let tree_template_geometry =
            band(buffers.tree_template_indices, buffers.tree_template_vertices);
        let tree_cross_template_geometry = band(
            buffers.tree_cross_template_indices,
            buffers.tree_cross_template_vertices,
        );

        // Cross-fade: keep the replaced generation's geometry under the new
        // one for TILE_FADE_SECONDS instead of popping.
        let new_baked_3d = self.baked_3d_mode;
        // The grow-heights animation is the flat->3D MODE reveal. Once 3D
        // is established on screen, arriving tiles (fresh, rebuilt at a new
        // bucket, or re-entering after eviction) must NOT replay it — the
        // city is already standing and a per-tile pop reads as a flash.
        let three_d_established = new_baked_3d
            && self
                .tiles
                .values()
                .any(|entry| entry.baked_3d && matches!(entry.state, TileLoadState::Ready { .. }));
        let fade = if old_fill.is_some()
            || old_fill_misc.is_some()
            || old_icon.is_some()
            || !old_icon_instances.is_empty()
            || fade_casing_geometry.is_some()
            || fade_stroke_geometry.is_some()
        {
            Some(TileFade {
                started: cx.seconds_since_app_start(),
                bucket: old_bucket,
                grow_heights: new_baked_3d && !old_baked_3d && !three_d_established,
                reuse_road_core,
                fill_geometry: old_fill,
                fill_misc_geometry: old_fill_misc,
                // Stable road geometry stays current across a mode switch;
                // drawing it again as outgoing fade would darken the roads.
                casing_geometry: fade_casing_geometry,
                stroke_geometry: fade_stroke_geometry,
                icon_geometry: old_icon,
                icon_instances: old_icon_instances,
            })
        } else {
            Some(TileFade {
                started: cx.seconds_since_app_start(),
                bucket: buffers.render_zoom,
                grow_heights: new_baked_3d && !three_d_established,
                reuse_road_core: false,
                fill_geometry: None,
                fill_misc_geometry: None,
                casing_geometry: None,
                stroke_geometry: None,
                icon_geometry: None,
                icon_instances: Vec::new(),
            })
        };
        // In an established 3D scene tiles snap in whole: any fade of
        // opaque 3D reads as a flash (user call — fades are for 2D and
        // for the one flat->3D mode reveal).
        let fade = if three_d_established { None } else { fade };
        cx.stop_timer(self.tile_fade_timer);
        self.tile_fade_timer = cx.start_timeout(0.016);

        self.tiles.insert(
            tile_key,
            TileEntry {
                state: TileLoadState::Ready {
                    fill_geometry,
                    fill_misc_geometry,
                    casing_geometry,
                    stroke_geometry,
                    icon_geometry,
                    icon_high_geometry,
                    shadow_disc_geometry,
                    icon_instances: buffers.icon_instances,
                    icon_high_instances: buffers.icon_high_instances,
                    fringe_geometry,
                    fill_3d_geometry,
                    wall_geometry,
                    wall_instances: buffers.wall_instances,
                    tree_geometry,
                    tree_cross_geometry,
                    tree_template_geometry,
                    tree_cross_template_geometry,
                    tree_instances: buffers.tree_instances,
                    feature_count: if reuse_road_core {
                        buffers.feature_count.max(old_feature_count)
                    } else {
                        buffers.feature_count
                    },
                    labels: buffers.labels,
                    pin_hits: buffers.pin_hits,
                },
                last_used: self.frame_counter,
                attempts: 0,
                retry_after: 0,
                bytes: tile_bytes,
                bucket: buffers.render_zoom,
                baked_3d: self.baked_3d_mode,
                road_core_cached: !buffers.mode_overlay_only || reuse_road_core,
                road_icon_indices: buffers.road_icon_indices,
                road_icon_vertices: buffers.road_icon_vertices,
                fade,
            },
        );
        self.tiles_generation = self.tiles_generation.wrapping_add(1);
    }

    /// The upload queue drains only 2 tiles per frame; a fast pan across 3D
    /// building tiles can park gigabytes of baked buffers here. Drop the
    /// oldest queued bakes beyond a byte budget — they were about to be
    /// stale anyway.
    fn cap_pending_ready_tiles(&mut self) {
        const PENDING_BYTE_BUDGET: usize = 384_000_000;
        let mut total: usize = self
            .pending_ready_tiles
            .iter()
            .map(|(_, buffers)| buffers.byte_size())
            .sum();
        while total > PENDING_BYTE_BUDGET && self.pending_ready_tiles.len() > 1 {
            let (_, dropped) = self.pending_ready_tiles.remove(0);
            total -= dropped.byte_size();
        }
    }

    fn handle_tile_worker_messages(&mut self, cx: &mut Cx) {
        let mut redraw = false;
        while let Ok(msg) = self.tile_worker_rx.try_recv() {
            match msg {
                TileWorkerMessage::LocalBatchLoaded {
                    style_epoch,
                    requested,
                    loaded,
                    failed,
                } => {
                    if style_epoch != self.style_epoch {
                        for key in &requested {
                            self.local_requested_tiles.remove(key);
                        }
                        continue;
                    }
                    for key in &requested {
                        self.local_requested_tiles.remove(key);
                    }

                    let mut loaded_keys = HashSet::with_capacity(loaded.len());
                    let mut empty_feature_tiles = Vec::<TileKey>::new();
                    let current_bucket = self.render_bucket();
                    for tile in loaded {
                        loaded_keys.insert(tile.tile_key);
                        self.local_missing_tiles.remove(&tile.tile_key);
                        if tile.buffers.feature_count == 0 {
                            empty_feature_tiles.push(tile.tile_key);
                        }
                        // NOTE: do NOT drop results whose bucket moved on:
                        // buffers.render_zoom (request-zoom space, clamped to
                        // the archive max) and render_bucket() disagree at
                        // overzoom, so a != check here discarded EVERY tile
                        // at building zooms — permanent livelock (build 24).
                        // A stale-bucket result is still drawable; the
                        // bucket-restyle path rebuilds it in due course.
                        let _ = current_bucket;
                        self.pending_ready_tiles
                            .retain(|(key, _)| *key != tile.tile_key);
                        self.pending_ready_tiles.push((tile.tile_key, tile.buffers));
                    }
                    self.cap_pending_ready_tiles();
                    if !empty_feature_tiles.is_empty() {
                        empty_feature_tiles.sort_unstable();
                        log!("MapView: local mbtiles loaded {} tile(s) with 0 rendered features sample:{}", empty_feature_tiles.len(), format_tile_key_sample(&empty_feature_tiles, 8));
                    }
                    let failed_keys: HashSet<TileKey> = failed.into_iter().collect();
                    for key in requested {
                        if loaded_keys.contains(&key) {
                            continue;
                        }
                        if failed_keys.contains(&key) {
                            // Data exists but the decode failed — retry with
                            // backoff, never blacklist as missing.
                            self.mark_tile_failed(key, "decode failed");
                            continue;
                        }
                        self.local_missing_tiles.insert(key, self.frame_counter);
                        // KEEP-STALE: a key that reads as absent during a
                        // rebuild (racing archive writer, transient read
                        // error) must not take its stale drawable geometry
                        // off screen; the blacklist above already stops
                        // re-requests until the recheck window elapses.
                        if !self
                            .tiles
                            .get(&key)
                            .is_some_and(|entry| matches!(entry.state, TileLoadState::Ready { .. }))
                        {
                            self.tiles.remove(&key);
                        }
                    }
                    redraw = true;
                }
                TileWorkerMessage::LocalBatchFailed {
                    style_epoch,
                    requested,
                    error,
                } => {
                    if style_epoch != self.style_epoch {
                        for key in &requested {
                            self.local_requested_tiles.remove(key);
                        }
                        continue;
                    }
                    log!("MapView: local mbtiles load failed: {}", error);
                    for key in requested {
                        self.local_requested_tiles.remove(&key);
                        self.mark_tile_failed(key, &error);
                    }
                    redraw = true;
                }
                TileWorkerMessage::NetworkTileParsed {
                    style_epoch,
                    tile_key,
                    buffers,
                } => {
                    if style_epoch != self.style_epoch {
                        continue;
                    }
                    self.pending_ready_tiles.retain(|(key, _)| *key != tile_key);
                    self.pending_ready_tiles.push((tile_key, buffers));
                    self.cap_pending_ready_tiles();
                    redraw = true;
                }
                TileWorkerMessage::NetworkTileParseFailed {
                    style_epoch,
                    tile_key,
                    error,
                } => {
                    if style_epoch != self.style_epoch {
                        continue;
                    }
                    self.mark_tile_failed(tile_key, &format!("parse: {}", error));
                    redraw = true;
                }
            }
        }
        // Drain at most two pending uploads per frame; a bucket-17+ tile can
        // carry tens of MB of vertex data, and creating/uploading a whole
        // 10-tile batch in one frame showed up as 200-550ms frame gaps.
        if !self.pending_ready_tiles.is_empty()
            && self.last_tile_upload_frame != self.frame_counter
        {
            self.last_tile_upload_frame = self.frame_counter;
            let upload_start = cx.seconds_since_app_start();
            // Budget by BYTES, not just count: two 3D/overzoom tiles can
            // carry 60+ MB of buffers each and stall the frame for hundreds
            // of ms; always ship at least one so progress never stops.
            const UPLOAD_BYTE_BUDGET: usize = 24_000_000;
            let mut count = 0usize;
            let mut budget = 0usize;
            for (_, buffers) in self.pending_ready_tiles.iter() {
                if count >= 2 {
                    break;
                }
                let size = buffers.byte_size();
                if count > 0 && budget + size > UPLOAD_BYTE_BUDGET {
                    break;
                }
                budget += size;
                count += 1;
            }
            let batch = self
                .pending_ready_tiles
                .drain(..count)
                .collect::<Vec<_>>();
            for (tile_key, buffers) in batch {
                self.insert_ready_tile(cx, tile_key, buffers);
            }
            let upload_ms = (cx.seconds_since_app_start() - upload_start).max(0.0) * 1000.0;
            if self.is_local_archive() && upload_ms > 4.0 {
                use std::io::Write;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("local/map_perf.log")
                {
                    let _ = writeln!(file, "upload_ms:{:.2} tiles:{}", upload_ms, count);
                }
            }
            redraw = true;
        }
        if !self.pending_ready_tiles.is_empty() {
            redraw = true;
        }
        if redraw {
            self.update_status_text();
            self.redraw(cx);
        }
    }

    /// The active mbtiles source: the widget's `mbtiles_path` property when
    /// set, else the compiled-in default.
    fn active_mbtiles_path(&self) -> &str {
        if self.mbtiles_path.is_empty() {
            LOCAL_MBTILES_PATH
        } else {
            &self.mbtiles_path
        }
    }

    fn ensure_archive_source(&mut self, cx: &mut Cx) {
        if self.tile_source_config.is_some() {
            return;
        }
        let config = self
            .tile_source_config
            .clone()
            .unwrap_or_else(|| TileSourceConfig::LocalArchive {
                mbtiles_path: self.active_mbtiles_path().to_string(),
                detail_mbtiles_path: self.detail_mbtiles_path.clone(),
                overlay_mbtiles_paths: self.overlay_mbtiles_paths.clone(),
                bridge_dz_path: self.bridge_dz_mbtiles_path.clone(),
            });
        self.install_archive_source(cx, config);
    }

    fn ensure_archive_worker_pool(&mut self, cx: &mut Cx) -> ArchiveWorkerPool {
        if self.archive_worker_pool.is_none() {
            self.archive_worker_pool = Some(new_archive_worker_pool(cx));
        }
        self.archive_worker_pool.as_ref().unwrap().clone()
    }

    fn install_archive_source(&mut self, cx: &mut Cx, config: TileSourceConfig) {
        self.archive_generation = self.archive_generation.wrapping_add(1).max(1);
        self.style_epoch = self.style_epoch.wrapping_add(1).max(1);
        if let Some(archive) = self.base_archive.as_mut() {
            archive.reset_generation(cx, self.archive_generation);
        }
        if let Some(archive) = self.detail_archive.as_mut() {
            archive.reset_generation(cx, self.archive_generation);
        }
        let workers = self.ensure_archive_worker_pool(cx);
        match &config {
            TileSourceConfig::LocalArchive {
                mbtiles_path,
                detail_mbtiles_path,
                overlay_mbtiles_paths,
                bridge_dz_path,
                ..
            } => {
                self.mbtiles_path = mbtiles_path.clone();
                self.detail_mbtiles_path = detail_mbtiles_path.clone();
                self.overlay_mbtiles_paths = overlay_mbtiles_paths.clone();
                self.bridge_dz_mbtiles_path = bridge_dz_path.clone();
                self.base_archive = is_mkmap_path_shape(mbtiles_path)
                    .then(|| MapTileArchive::file(mbtiles_path, workers.clone()));
                self.detail_archive = needs_separate_detail_archive(&config)
                    .then(|| MapTileArchive::file(detail_mbtiles_path, workers.clone()));
                self.use_local_mbtiles = true;
                self.use_network = false;
            }
            TileSourceConfig::HttpArchive {
                root_url,
                detail_root_url,
                overlay_mbtiles_paths,
                bridge_dz_path,
            } => {
                self.mbtiles_path.clear();
                self.detail_mbtiles_path = detail_root_url.clone();
                self.overlay_mbtiles_paths = overlay_mbtiles_paths.clone();
                self.bridge_dz_mbtiles_path = bridge_dz_path.clone();
                self.base_archive = Some(MapTileArchive::http(root_url, workers.clone()));
                self.detail_archive = needs_separate_detail_archive(&config)
                    .then(|| MapTileArchive::http(detail_root_url, workers));
                self.use_local_mbtiles = true;
                self.use_network = false;
            }
        }
        self.tile_source_config = Some(config);
        self.archive_pending_tiles.clear();
        self.pending_ready_tiles.clear();
        self.local_source_zoom_range = None;
        self.local_source_logged_zoom_range = None;
        self.local_source_zoom_range_path = None;
        self.local_source_zoom_range_checked = false;
    }

    fn is_local_archive(&self) -> bool {
        matches!(self.tile_source_config, Some(TileSourceConfig::LocalArchive { .. }))
    }

    fn handle_archive_events(&mut self, cx: &mut Cx, event: &Event) {
        let base = self
            .base_archive
            .as_mut()
            .map(|archive| archive.drain(cx, event))
            .unwrap_or_default();
        let detail = self
            .detail_archive
            .as_mut()
            .map(|archive| archive.drain(cx, event))
            .unwrap_or_default();

        if let Some(range) = self.base_archive.as_ref().and_then(MapTileArchive::zoom_range) {
            if self.local_source_zoom_range != Some(range) {
                self.local_source_zoom_range = Some(range);
                self.local_source_zoom_range_checked = true;
                self.redraw(cx);
            }
        }
        for tile in base {
            if tile.generation != self.archive_generation {
                continue;
            }
            if let Some(parts) = self.archive_pending_tiles.get_mut(&tile.key) {
                parts.base = Some(match tile.result {
                    TileBytesResult::Bytes(bytes) => Ok(Some(bytes)),
                    TileBytesResult::Missing => Ok(None),
                    TileBytesResult::Error(error) => Err(error),
                });
            }
        }
        for tile in detail {
            if tile.generation != self.archive_generation {
                continue;
            }
            if let Some(parts) = self.archive_pending_tiles.get_mut(&tile.key) {
                parts.detail = Some(match tile.result {
                    TileBytesResult::Bytes(bytes) => Ok(Some(bytes)),
                    TileBytesResult::Missing => Ok(None),
                    TileBytesResult::Error(error) => Err(error),
                });
            }
        }

        let ready: Vec<TileKey> = self
            .archive_pending_tiles
            .iter()
            .filter(|(_, parts)| {
                parts.base.is_some() && (!parts.detail_required || parts.detail.is_some())
            })
            .map(|(key, _)| *key)
            .collect();
        for key in ready {
            let parts = self.archive_pending_tiles.remove(&key).unwrap();
            if parts.generation != self.archive_generation {
                continue;
            }
            let base = match parts.base.unwrap() {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.local_requested_tiles.remove(&key);
                    self.mark_tile_failed(key, &error);
                    self.update_status_text();
                    self.redraw(cx);
                    continue;
                }
            };
            let detail = if parts.reuse_base_as_detail {
                base.clone()
            } else {
                parts.detail.and_then(Result::ok).flatten()
            };
            self.dispatch_archive_tile_build(cx, key, base, detail);
        }
    }

    fn dispatch_archive_tile_build(
        &mut self,
        cx: &mut Cx,
        key: TileKey,
        base: Option<Arc<[u8]>>,
        detail: Option<Arc<[u8]>>,
    ) {
        let sender = self.tile_worker_rx.sender();
        let style_epoch = self.style_epoch;
        let requested = vec![key];
        let theme_style = self.active_style().clone();
        let bucket = self.render_bucket();
        let buildings_3d = self.buildings_3d && self.tilt > 0.0;
        let build_road_core = !self.tiles.get(&key).is_some_and(|entry| {
            matches!(entry.state, TileLoadState::Ready { .. })
                && entry.bucket == bucket
                && entry.road_core_cached
        });
        let (detail_path, bridge_dz_path, overlay_paths) = match self.tile_source_config.as_ref() {
            Some(TileSourceConfig::LocalArchive {
                detail_mbtiles_path,
                bridge_dz_path,
                overlay_mbtiles_paths,
                ..
            }) => (detail_mbtiles_path, bridge_dz_path, overlay_mbtiles_paths),
            Some(TileSourceConfig::HttpArchive {
                detail_root_url,
                bridge_dz_path,
                overlay_mbtiles_paths,
                ..
            }) => (detail_root_url, bridge_dz_path, overlay_mbtiles_paths),
            None => (&self.detail_mbtiles_path, &self.bridge_dz_mbtiles_path, &self.overlay_mbtiles_paths),
        };
        let detail_path = (!detail_path.is_empty() && !is_mkmap_path_shape(detail_path))
            .then_some(detail_path.clone());
        let bridge_dz_path = (!bridge_dz_path.is_empty()).then_some(bridge_dz_path.clone());
        let overlay_paths = overlay_paths
            .split(';')
            .filter(|path| !path.trim().is_empty())
            .map(|path| path.trim().to_string())
            .collect::<Vec<_>>();
        if let Err(error) = self.submit_tile_job(cx, key, move || {
            let result = build_local_tile_from_archive_bytes(
                key,
                base,
                detail,
                detail_path.as_deref().map(Path::new),
                bridge_dz_path.as_deref().map(Path::new),
                &overlay_paths,
                &theme_style,
                bucket,
                buildings_3d,
                build_road_core,
            );
            match result {
                Ok(tile) => {
                    let _ = sender.send(TileWorkerMessage::LocalBatchLoaded {
                        style_epoch,
                        requested,
                        loaded: tile.into_iter().collect(),
                        failed: Vec::new(),
                    });
                }
                Err(error) => {
                    let _ = sender.send(TileWorkerMessage::LocalBatchLoaded {
                        style_epoch,
                        requested: requested.clone(),
                        loaded: Vec::new(),
                        failed: requested,
                    });
                    log!("MapView: archive tile build failed: {}", error);
                }
            }
        }) {
            self.local_requested_tiles.remove(&key);
            self.mark_tile_failed(key, &format!("tile worker submission failed: {error}"));
            self.update_status_text();
            self.redraw(cx);
        }
    }

    fn cancel_archive_tile(&mut self, cx: &mut Cx, key: TileKey) {
        self.archive_pending_tiles.remove(&key);
        if let Some(archive) = self.base_archive.as_mut() {
            archive.cancel_tile(cx, key);
        }
        if let Some(archive) = self.detail_archive.as_mut() {
            archive.cancel_tile(cx, key);
        }
    }

    fn dispatch_legacy_tile_builds(&mut self, cx: &mut Cx, keys: Vec<TileKey>, bucket: u32) {
        let style_epoch = self.style_epoch;
        let active_path = self.active_mbtiles_path().to_string();
        for key in keys {
            let sender = self.tile_worker_rx.sender();
            let requested = vec![key];
            let mbtiles_path = active_path.clone();
            let detail_path = self.detail_mbtiles_path.clone();
            let bridge_dz_path = self.bridge_dz_mbtiles_path.clone();
            let overlay_paths = self
                .overlay_mbtiles_paths
                .split(';')
                .filter(|path| !path.trim().is_empty())
                .map(|path| path.trim().to_string())
                .collect::<Vec<_>>();
            let buildings_3d = self.buildings_3d && self.tilt > 0.0;
            let theme_style = self.active_style().clone();
            let build_road_core = !self.tiles.get(&key).is_some_and(|entry| {
                matches!(entry.state, TileLoadState::Ready { .. })
                    && entry.bucket == bucket
                    && entry.road_core_cached
            });
            if let Err(error) = self.submit_tile_job(cx, key, move || {
                let detail_path = (!detail_path.is_empty()).then_some(detail_path);
                let bridge_dz_path = (!bridge_dz_path.is_empty()).then_some(bridge_dz_path);
                match load_local_tile_batch(
                    Path::new(&mbtiles_path),
                    detail_path.as_deref().map(Path::new),
                    bridge_dz_path.as_deref().map(Path::new),
                    &overlay_paths,
                    &requested,
                    &theme_style,
                    bucket,
                    buildings_3d,
                    build_road_core,
                ) {
                    Ok((loaded, failed)) => {
                        let _ = sender.send(TileWorkerMessage::LocalBatchLoaded {
                            style_epoch,
                            requested,
                            loaded,
                            failed,
                        });
                    }
                    Err(error) => {
                        let _ = sender.send(TileWorkerMessage::LocalBatchFailed {
                            style_epoch,
                            requested,
                            error,
                        });
                    }
                }
            }) {
                self.local_requested_tiles.remove(&key);
                self.mark_tile_failed(key, &format!("tile worker submission failed: {error}"));
            }
        }
    }

    fn request_visible_tiles_from_local_source(&mut self, cx: &mut Cx) {
        if !self.use_local_mbtiles {
            return;
        }

        self.ensure_archive_source(cx);

        let bucket = self.render_bucket();
        // OBSOLETE-WORK CANCELLATION runs on every viewport pass, including
        // same-zoom pans. Running jobs finish, but queued jobs outside the
        // current visible set may not start or retain an in-flight slot.
        let request_zoom = self.request_zoom_level();
        let visible: HashSet<TileKey> = self.visible_tiles.iter().copied().collect();
        let rotation = self.screen_rotation();
        let tilt_cos = self.tilt_cos();
        let mut priority_order = self.visible_tiles.clone();
        sort_tiles_center_out(
            &mut priority_order,
            request_zoom,
            self.center_norm,
            rotation,
            tilt_cos,
        );
        let priorities = priority_order
            .iter()
            .map(|key| {
                (
                    *key,
                    tile_screen_priority(
                        *key,
                        request_zoom,
                        self.center_norm,
                        rotation,
                        tilt_cos,
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        let obsolete_archive: Vec<TileKey> = self
            .archive_pending_tiles
            .keys()
            .filter(|key| key.z != request_zoom || !visible.contains(key))
            .copied()
            .collect();
        for key in obsolete_archive {
            self.cancel_archive_tile(cx, key);
            self.local_requested_tiles.remove(&key);
            if self
                .tiles
                .get(&key)
                .is_some_and(|entry| matches!(entry.state, TileLoadState::LoadingLocal))
            {
                self.tiles.remove(&key);
            }
        }
        if let Some(archive) = self.base_archive.as_mut() {
            archive.reprioritize_tiles(&priorities);
        }
        if let Some(archive) = self.detail_archive.as_mut() {
            archive.reprioritize_tiles(&priorities);
        }
        if let Some(archive) = self.base_archive.as_mut() {
            archive.flush(cx);
        }
        if let Some(archive) = self.detail_archive.as_mut() {
            archive.flush(cx);
        }
        if let Some(pool) = self.tile_thread_pool.as_ref() {
            let dropped = pool
                .retain_queued::<TileKey>(|key| key.z == request_zoom && visible.contains(key));
            for key in dropped {
                self.local_requested_tiles.remove(&key);
                // A queued-then-dropped placeholder must not linger as
                // Loading forever; keep-stale Ready entries stay.
                if self
                    .tiles
                    .get(&key)
                    .is_some_and(|entry| matches!(entry.state, TileLoadState::LoadingLocal))
                {
                    self.tiles.remove(&key);
                }
            }
        }
        self.sync_archive_request_watchdog(cx);
        let now = self.frame_counter;
        // Absent tiles get re-checked after a while: our mbtiles archives
        // are rebuilt in place during development, and a one-off read
        // glitch must not leave a permanent hole.
        self.local_missing_tiles
            .retain(|_, learned| now.saturating_sub(*learned) < MISSING_RECHECK_FRAMES);
        // Mid-gesture the baked geometry scales geometrically (smooth); only
        // restyle stale buckets once the zoom has settled, or widths flicker
        // tile-batch by tile-batch under the gesture.
        let zoom_settling = self
            .last_zoom_change_time
            .is_some_and(|at| cx.seconds_since_app_start() - at < ZOOM_SETTLE_SECONDS);
        let mut missing = Vec::<TileKey>::new();
        for key in &self.visible_tiles {
            if self.local_requested_tiles.contains_key(key)
                || self.local_missing_tiles.contains_key(key)
            {
                continue;
            }
            if let Some(entry) = self.tiles.get(key) {
                match &entry.state {
                    // Stale geometry stays drawable but gets rebuilt. A
                    // same-bucket mode flip can retain the stable road core;
                    // zoom/style rebuilds cannot.
                    TileLoadState::Ready { .. }
                        if entry.bucket != bucket
                            || entry.baked_3d != self.baked_3d_mode
                            || !entry.road_core_cached =>
                    {
                        if zoom_settling {
                            continue;
                        }
                        // Failed-rebuild backoff for still-drawable entries
                        // (mark_tile_failed keeps them Ready).
                        if now < entry.retry_after {
                            continue;
                        }
                    }
                    // Failed tiles retry once their backoff elapses.
                    TileLoadState::Failed { retry_after } if now >= *retry_after => {}
                    _ => continue,
                }
            }
            missing.push(*key);
        }
        if missing.is_empty() {
            return;
        }
        // Load center-out: visible_tiles is generated row-major, and with
        // only max_in_flight slots per frame the top-left corner otherwise
        // fills before what the user is actually looking at.
        sort_tiles_center_out(
            &mut missing,
            request_zoom,
            self.center_norm,
            rotation,
            tilt_cos,
        );
        // Dispatch each tile as its own worker job so builds run in parallel
        // across the pool; keep enough in flight to cover a viewport restyle.
        // While a zoom gesture is live, dispatch only the innermost tiles:
        // mid-zoom tiles can take seconds to build, and filling every slot
        // with speculative edge tiles leaves no worker free for the center
        // the user is actually zooming towards.
        // A mode/bucket restyle of currently-visible tiles is NOT gesture
        // speculation — everything on screen needs its rebuild, so the
        // 4-slot gesture throttle would only serialize the burst. Detect it
        // by the missing set being dominated by still-drawable stale
        // entries (keep-stale rebuilds).
        let stale_rebuilds = missing
            .iter()
            .filter(|key| {
                self.tiles
                    .get(key)
                    .is_some_and(|entry| matches!(entry.state, TileLoadState::Ready { .. }))
            })
            .count();
        let restyle_burst = stale_rebuilds * 2 >= missing.len();
        let slot_cap = if matches!(
            self.tile_source_config,
            Some(TileSourceConfig::HttpArchive { .. })
        ) {
            64usize
        } else if zoom_settling && !restyle_burst {
            4usize
        } else {
            12usize
        };
        let max_in_flight = slot_cap.saturating_sub(self.local_requested_tiles.len());
        if missing.len() > max_in_flight {
            missing.truncate(max_in_flight);
        }
        if missing.is_empty() {
            return;
        }

        for key in &missing {
            self.local_requested_tiles
                .insert(*key, cx.seconds_since_app_start());
            let prev_attempts = self.tiles.get(key).map_or(0, |entry| entry.attempts);
            let keep_stale = self
                .tiles
                .get(key)
                .is_some_and(|entry| matches!(entry.state, TileLoadState::Ready { .. }));
            if !keep_stale {
                self.tiles.insert(
                    *key,
                    TileEntry {
                        state: TileLoadState::LoadingLocal,
                        last_used: self.frame_counter,
                        attempts: prev_attempts,
                        retry_after: 0,
                        bytes: 0,
                        bucket,
                        baked_3d: self.baked_3d_mode,
                        road_core_cached: false,
                        road_icon_indices: Vec::new(),
                        road_icon_vertices: Vec::new(),
                        fade: None,
                    },
                );
            }
        }
        self.sync_archive_request_watchdog(cx);

        if self.base_archive.is_none() {
            self.dispatch_legacy_tile_builds(cx, missing, bucket);
            return;
        }

        let detail_needed = bucket >= 14;
        let reuse_base_as_detail = detail_needed
            && self
                .tile_source_config
                .as_ref()
                .is_some_and(detail_matches_base);
        let detail_required = detail_needed && self.detail_archive.is_some();
        let generation = self.archive_generation;
        for key in missing {
            let priority = priorities.get(&key).copied().unwrap_or(u64::MAX);
            self.archive_pending_tiles.insert(
                key,
                ArchiveTileParts {
                    generation,
                    base: None,
                    detail: None,
                    detail_required,
                    reuse_base_as_detail: reuse_base_as_detail && key.z >= 14,
                },
            );
            if let Some(archive) = self.base_archive.as_mut() {
                archive.request_tile(cx, key, generation, priority);
            }
            if detail_required {
                if let Some(archive) = self.detail_archive.as_mut() {
                    archive.request_tile(cx, key, generation, priority);
                }
            }
        }
        if let Some(archive) = self.base_archive.as_mut() {
            archive.flush(cx);
        }
        if let Some(archive) = self.detail_archive.as_mut() {
            archive.flush(cx);
        }
        self.sync_archive_request_watchdog(cx);
    }

    fn expire_archive_requests(&mut self, cx: &mut Cx) {
        // Hosted archives have their dispatch-aware stall clock in the web
        // transport. A tile can wait here for a browser connection without
        // becoming eligible for cancellation.
        if !self.is_local_archive() {
            return;
        }
        let now = cx.seconds_since_app_start();
        let timed_out: Vec<TileKey> = self
            .local_requested_tiles
            .iter()
            .filter(|(_, started)| now - **started >= ARCHIVE_REQUEST_TIMEOUT_SECONDS)
            .map(|(key, _)| *key)
            .collect();
        let expired_any = !timed_out.is_empty();
        for key in timed_out {
            self.local_requested_tiles.remove(&key);
            self.cancel_archive_tile(cx, key);
            if self
                .tiles
                .get(&key)
                .is_some_and(|entry| matches!(entry.state, TileLoadState::LoadingLocal))
            {
                self.tiles.remove(&key);
            }
        }
        if expired_any {
            self.redraw(cx);
        }
    }

    fn sync_archive_request_watchdog(&mut self, cx: &mut Cx) {
        if !self.is_local_archive() {
            self.archive_request_watchdog_handle = None;
            return;
        }
        if self.local_requested_tiles.is_empty() {
            self.archive_request_watchdog_handle = None;
        } else if self.archive_request_watchdog_handle.is_none()
            && !self.archive_request_watchdog_unavailable
        {
            if self.archive_request_watchdog_scheduler.is_none() {
                match cx.thread_spawner().scheduler() {
                    Ok(scheduler) => {
                        self.archive_request_watchdog_scheduler = Some(scheduler);
                    }
                    Err(error) => {
                        self.archive_request_watchdog_unavailable = true;
                        log!("MapView: archive watchdog unavailable: {error}");
                        return;
                    }
                }
            }
            let now = cx.seconds_since_app_start();
            let deadline = self
                .local_requested_tiles
                .values()
                .copied()
                .fold(f64::INFINITY, f64::min)
                + ARCHIVE_REQUEST_TIMEOUT_SECONDS;
            let scheduler_deadline = Cx::monotonic_now() + (deadline - now).max(0.001);
            let sender = self.archive_request_watchdog_rx.sender();
            match self.archive_request_watchdog_scheduler.as_ref().unwrap().at(
                scheduler_deadline,
                CancellationToken::new(),
                move || {
                    let _ = sender.send(());
                },
            ) {
                Ok(handle) => self.archive_request_watchdog_handle = Some(handle),
                Err(error) => {
                    self.archive_request_watchdog_unavailable = true;
                    log!("MapView: archive watchdog scheduling failed: {error}");
                }
            }
        }
    }

    fn mark_tile_failed(&mut self, tile_key: TileKey, reason: &str) {
        let attempts = self
            .tiles
            .get(&tile_key)
            .map_or(1, |entry| entry.attempts.saturating_add(1));
        let retry_delay = retry_delay_frames(attempts);
        let retry_after = self.frame_counter.saturating_add(retry_delay);
        let bucket = self.render_bucket();
        // KEEP-STALE: a failed REBUILD of a tile that still has drawable
        // geometry must not replace it with a gray Failed placeholder (the
        // 2D/3D flip gray-out: any transient batch error — e.g. an archive
        // being rewritten — nuked every stale mesh on screen). Keep the
        // entry drawable and only arm the rebuild backoff.
        if let Some(entry) = self.tiles.get_mut(&tile_key) {
            if matches!(entry.state, TileLoadState::Ready { .. }) {
                entry.attempts = attempts;
                entry.retry_after = retry_after;
                log!(
                    "MapView: tile z{} x{} y{} rebuild failed (attempt {}), keeping stale geometry: {}",
                    tile_key.z,
                    tile_key.x,
                    tile_key.y,
                    attempts,
                    reason
                );
                return;
            }
        }
        self.tiles.insert(
            tile_key,
            TileEntry {
                state: TileLoadState::Failed { retry_after },
                last_used: self.frame_counter,
                attempts,
                retry_after,
                bytes: 0,
                bucket,
                baked_3d: self.baked_3d_mode,
                road_core_cached: false,
                road_icon_indices: Vec::new(),
                road_icon_vertices: Vec::new(),
                fade: None,
            },
        );
        log!(
            "MapView: tile z{} x{} y{} failed (attempt {}): {}",
            tile_key.z,
            tile_key.x,
            tile_key.y,
            attempts,
            reason
        );
    }

    fn wrap_and_clamp_center(&mut self) {
        // The renderer draws exactly one world width (visible_tile_keys
        // dedups wrapped x), so a WRAPPING centre was a lie: panning past
        // the antimeridian slid the whole world off-centre and showed void
        // — a drag into nothing that read as a broken pan. The centre is
        // CLAMPED instead: the viewport never leaves the world, and a world
        // narrower than the view sits pinned in its middle.
        let world = tile_world_size_zoom(self.view_zoom()).max(1.0);
        let half_x = (self.view_rect.size.x * 0.5 / world).min(0.5);
        let half_y = (self.view_rect.size.y * 0.5 / world).min(0.5);
        self.center_norm.x = if half_x >= 0.5 { 0.5 } else { self.center_norm.x.clamp(half_x, 1.0 - half_x) };
        self.center_norm.y = if half_y >= 0.5 { 0.5 } else { self.center_norm.y.clamp(half_y, 1.0 - half_y) };
    }

    fn zoom_with_anchor(&mut self, cx: &mut Cx, scroll: f64, anchor_abs: Vec2d) {
        if scroll.abs() <= f64::EPSILON {
            return;
        }
        let current_zoom = self.view_zoom();
        let zoom_delta = (-scroll / 240.0).clamp(-1.0, 1.0);
        let min_zoom = self.min_zoom.max(0.0);
        let max_zoom = self.max_zoom.max(min_zoom);
        let new_zoom = (current_zoom + zoom_delta).clamp(min_zoom, max_zoom);
        if (new_zoom - current_zoom).abs() < 1e-4 {
            return;
        }

        if self.view_rect.size.x <= 0.0 || self.view_rect.size.y <= 0.0 {
            self.zoom = new_zoom;
            self.redraw(cx);
            return;
        }

        let old_world_size = tile_world_size_zoom(current_zoom);
        let new_world_size = tile_world_size_zoom(new_zoom);
        let rect_center = self.view_rect.pos + self.view_rect.size * 0.5;
        // Anchor into world-aligned space (undo rotation + tilt — and the
        // fold, when it is on: the point under the cursor is only where the
        // WARP inverse says it is, otherwise zooming on the wall slides the
        // map out from under the pointer).
        let anchor_rel = if self.space_warp_eff.is_on() {
            self.overlay_camera().screen_to_world_rel(anchor_abs)
        } else {
            self.screen_delta_to_world(anchor_abs - rect_center)
        };

        self.zoom = new_zoom;
        self.center_norm = zoom_anchor_center_norm(
            self.center_norm,
            anchor_rel,
            old_world_size,
            new_world_size,
        );
        self.wrap_and_clamp_center();
        self.last_zoom_change_frame = self.frame_counter;
        self.last_zoom_change_time = Some(cx.seconds_since_app_start());
        self.pending_viewport_changed = true;
        // The paint beat idles when input stops; without a timer wake the
        // settle window would never elapse and stale-bucket restyles only
        // fired once the user wiggled the map again.
        cx.stop_timer(self.zoom_settle_timer);
        self.zoom_settle_timer = cx.start_timeout(0.15);
        self.redraw(cx);
    }

    fn ensure_tile_thread_pool(&mut self, cx: &mut Cx) {
        if self.tile_thread_pool.is_none() && !self.tile_thread_pool_unavailable {
            let spawner = cx.thread_spawner();
            match TaskPool::new(
                spawner.clone(),
                PoolOptions {
                    workers: spawner.worker_count(2, 8),
                    capacity: std::num::NonZeroUsize::new(1024).unwrap(),
                    name: "map-tile".into(),
                },
            ) {
                Ok(pool) => self.tile_thread_pool = Some(pool),
                Err(error) => {
                    log!("MapView: tile pool unavailable, using serial work: {error}");
                    self.tile_thread_pool_unavailable = true;
                }
            }
        }
    }

    fn submit_tile_job<F>(&mut self, cx: &mut Cx, key: TileKey, job: F) -> Result<(), SubmitError>
    where
        F: FnOnce() + Send + 'static,
    {
        self.ensure_tile_thread_pool(cx);
        if let Some(pool) = self.tile_thread_pool.as_ref() {
            match pool.submit_tagged(key, true, QueueOrder::Lifo, job) {
                Ok(task) => {
                    task.detach();
                    Ok(())
                }
                Err(error) => Err(error),
            }
        } else {
            // Pure parsing/tessellation has an explicit serial fallback when
            // wasm was built without atomics.
            job();
            Ok(())
        }
    }

    fn ensure_visible_tiles(&mut self, cx: &mut Cx, rect: Rect) {
        self.frame_counter = self.frame_counter.wrapping_add(1);
        let now_seconds = cx.seconds_since_app_start();
        // This is the sole owner of the 2D/3D tile transition. `set_tilt`
        // only updates the camera and redraws, avoiding duplicate restyles.
        let mode_3d = self.buildings_3d && self.tilt > 0.0;
        if mode_3d != self.baked_3d_mode {
            self.baked_3d_mode = mode_3d;
            self.restyle_mode_overlay_keep_stale(cx);
        }
        // Read the archive's declared zoom range BEFORE computing visible
        // tile keys — request_zoom_level clamps to it, and reading it after
        // meant the very first frame requested impossible zoom levels.
        if self.use_local_mbtiles {
            self.ensure_archive_source(cx);
            self.ensure_local_zoom_range();
        }
        for entry in self.tiles.values_mut() {
            if entry
                .fade
                .as_ref()
                .is_some_and(|fade| now_seconds - fade.started > TILE_FADE_SECONDS)
            {
                entry.fade = None;
            }
        }
        self.visible_tiles = self.visible_tile_keys(rect);
        let target_zoom = self.request_zoom_level();
        // Keep frames coming briefly after a zoom change so the deferred
        // bucket restyle actually fires once the gesture settles.
        if self
            .last_zoom_change_time
            .is_some_and(|at| now_seconds - at < ZOOM_SETTLE_SECONDS + 0.05)
        {
            self.redraw(cx);
        }

        self.ensure_tile_thread_pool(cx);
        self.request_visible_tiles_from_local_source(cx);

        let mut visible_set = HashSet::with_capacity(self.visible_tiles.len());
        for key in &self.visible_tiles {
            visible_set.insert(*key);
            if let Some(entry) = self.tiles.get_mut(key) {
                entry.last_used = self.frame_counter;
            }
        }

        let mut pending = self
            .tiles
            .values()
            .filter(|e| matches!(e.state, TileLoadState::LoadingNetwork))
            .count();

        for key in self.visible_tiles.clone() {
            let retry_attempt = self.tiles.get(&key).and_then(|entry| {
                if let TileLoadState::Failed { retry_after } = entry.state {
                    if entry.attempts < MAX_TILE_RETRIES && self.frame_counter >= retry_after {
                        return Some(entry.attempts);
                    }
                }
                None
            });
            if let Some(attempts) = retry_attempt {
                if pending < MAX_PENDING_REQUESTS && self.request_tile(cx, key, attempts, true) {
                    pending += 1;
                }
                continue;
            }
            if self.tiles.contains_key(&key) {
                continue;
            }
            if self.local_missing_tiles.contains_key(&key) {
                if self.use_network
                    && pending < MAX_PENDING_REQUESTS
                    && self.request_tile(cx, key, 0, true)
                {
                    pending += 1;
                }
                continue;
            }
            if self.request_tile(cx, key, 0, pending < MAX_PENDING_REQUESTS) {
                pending += 1;
            }
        }

        // Tiles at high buckets carry tens of MB of GPU buffers each; keeping
        // hundreds resident causes GPU memory pressure (frame-gap stutter).
        if self.tiles.len() > 48 {
            let frame_counter = self.frame_counter;
            let min_keep_zoom = target_zoom.saturating_sub(2);
            let max_keep_zoom = target_zoom.saturating_add(1);
            self.tiles.retain(|key, entry| {
                if visible_set.contains(key)
                    || matches!(
                        entry.state,
                        TileLoadState::LoadingNetwork | TileLoadState::LoadingLocal
                    )
                {
                    return true;
                }
                if key.z < min_keep_zoom || key.z > max_keep_zoom {
                    return false;
                }
                frame_counter.saturating_sub(entry.last_used) <= 120
            });
        }
        // Byte budget on top of the count heuristic: 3D building bakes run
        // 60-90 MB per tile (CPU floats AND a GPU copy), so even a modest
        // resident set can eat the machine. Evict least-recently-used
        // non-visible tiles until the geometry footprint fits.
        const TILE_CACHE_BYTE_BUDGET: usize = 1_200_000_000;
        const HTTP_TILE_CACHE_BYTE_BUDGET: usize = 240_000_000;
        let total_bytes: usize = self.tiles.values().map(|entry| entry.bytes).sum();
        // Anti-thrash: street-zoom tiles now carry the full icon horizon
        // (50-85 MB each), so a fixed budget can sit BELOW visible+ring —
        // pure LRU then evicts the exact neighbor a pan re-enters seconds
        // later and every circle around a city center rebuilds its ring.
        // Scale the effective budget to hold twice the visible set, and
        // evict by DISTANCE from the view center (farthest first, LRU as
        // the tiebreak): the pan ring survives, the trail behind does not.
        let visible_bytes: usize = self
            .tiles
            .iter()
            .filter(|(key, _)| visible_set.contains(*key))
            .map(|(_, entry)| entry.bytes)
            .sum();
        let byte_budget = if matches!(
            self.tile_source_config,
            Some(TileSourceConfig::HttpArchive { .. })
        ) {
            HTTP_TILE_CACHE_BYTE_BUDGET.max(visible_bytes)
        } else {
            TILE_CACHE_BYTE_BUDGET.max(visible_bytes.saturating_mul(2))
        };
        if total_bytes > byte_budget {
            let center = self.center_norm;
            let mut evictable: Vec<(TileKey, u64, usize)> = self
                .tiles
                .iter()
                .filter(|(key, entry)| {
                    !visible_set.contains(key)
                        && !matches!(
                            entry.state,
                            TileLoadState::LoadingNetwork | TileLoadState::LoadingLocal
                        )
                })
                .map(|(key, entry)| (*key, entry.last_used, entry.bytes))
                .collect();
            let norm_dist = |key: &TileKey| -> f64 {
                let n = (1u64 << key.z.min(30)) as f64;
                let dx = (key.x as f64 + 0.5) / n - center.x;
                let dy = (key.y as f64 + 0.5) / n - center.y;
                dx * dx + dy * dy
            };
            // Farthest first; equal-distance ties fall back to oldest use.
            evictable.sort_unstable_by(|a, b| {
                norm_dist(&b.0)
                    .partial_cmp(&norm_dist(&a.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.1.cmp(&b.1))
            });
            let mut remaining = total_bytes;
            for (key, _, bytes) in evictable {
                if remaining <= byte_budget {
                    break;
                }
                self.tiles.remove(&key);
                remaining -= bytes;
            }
        }
        self.update_status_text();
    }

    fn visible_tile_keys(&self, rect: Rect) -> Vec<TileKey> {
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return Vec::new();
        }
        let zoom = self.request_zoom_level();
        let world_size = tile_world_size(zoom);
        let center_world = self.center_norm * world_size;
        // Screen pixels cover 2^(view-zoom) request-zoom world pixels when
        // overzoomed; without this the viewport requests up to 64x too many
        // source tiles at view z17.
        let overzoom = 2.0_f64.powf(self.view_zoom() - zoom as f64).max(1.0);
        // Under heading-up rotation the viewport covers the rotated AABB in
        // world space.
        let (rot_cos, rot_sin) = self.screen_rotation();
        let mut half_w = rect.size.x * 0.5;
        // Tilt compression means the screen shows more world vertically.
        let mut half_h = rect.size.y * 0.5 / self.tilt_cos().max(1e-3);
        // The space-warp wall advances up-screen slower than the flat
        // ortho compression at moderate tilt, and its perspective shrinks
        // the far field laterally — the fold can SEE FURTHER than the flat
        // frustum. Cull honestly or the wall runs out of city.
        if self.space_warp_eff.is_on() {
            let (reach, widen) = self
                .space_warp_eff
                .cull_extents(rect.size.y * 0.5, half_h);
            half_h = reach;
            half_w *= widen;
        }
        // Terrain displacement pulls geometry up-screen by up to the max
        // elevation: ground past both screen edges still lands in view.
        let lift_pad = self.terrain_elev_max as f64 * self.terrain_lift_px_per_m()
            / self.tilt_cos().max(1e-3);
        half_h += lift_pad;
        let half_size = dvec2(
            (half_w * rot_cos.abs() + half_h * rot_sin.abs()) / overzoom,
            (half_w * rot_sin.abs() + half_h * rot_cos.abs()) / overzoom,
        );
        let top_left = center_world - half_size;
        let bottom_right = center_world + half_size;
        let tile_count = 1_i32 << zoom;

        let (min_tx, max_tx) = tile_span_with_prefetch(top_left.x, bottom_right.x);
        let (min_ty, max_ty) = tile_span_with_prefetch(top_left.y, bottom_right.y);

        let mut out = Vec::new();
        for ty in min_ty..=max_ty {
            if ty < 0 || ty >= tile_count {
                continue;
            }
            for tx in min_tx..=max_tx {
                out.push(TileKey {
                    z: zoom,
                    x: tx.rem_euclid(tile_count),
                    y: ty,
                });
            }
        }
        out.sort_unstable();
        out.dedup();

        let center_tx = (center_world.x / TILE_SIZE).floor() as i32;
        let center_ty = (center_world.y / TILE_SIZE).floor() as i32;
        out.sort_unstable_by_key(|key| {
            let dx = (key.x - center_tx).abs();
            let dy = (key.y - center_ty).abs();
            (dx + dy, key.y, key.x)
        });
        out
    }

    /// A ready tile whose cross-fade started from no previous geometry —
    /// i.e. it is fading in over whatever was on screen before, not over an
    /// older restyle of itself.
    fn tile_fading_from_empty(&self, key: TileKey) -> bool {
        self.tiles.get(&key).is_some_and(|entry| {
            entry.fade.as_ref().is_some_and(|fade| {
                fade.fill_geometry.is_none()
                    && fade.fill_misc_geometry.is_none()
                    && fade.casing_geometry.is_none()
                    && fade.stroke_geometry.is_none()
                    && fade.icon_geometry.is_none()
                    && fade.icon_instances.is_empty()
            })
        })
    }

    fn fill_draw_tile_keys(&mut self) {
        self.scratch_draw_tiles.clear();
        self.scratch_draw_seen.clear();

        for i in 0..self.visible_tiles.len() {
            let key = self.visible_tiles[i];
            if self.tile_is_ready(key) {
                // While this tile fades in from empty (fresh zoom level),
                // keep the previous zoom level's imagery painted beneath it
                // so the transition cross-fades instead of flashing the
                // background: prefer the ready ancestor, else descendants.
                if self.tile_fading_from_empty(key) {
                    if let Some(under) = self.find_ready_ancestor(key) {
                        if self.scratch_draw_seen.insert(under) {
                            self.scratch_draw_tiles.push(under);
                        }
                    } else {
                        self.fill_ready_descendants(key);
                        for j in 0..self.scratch_descendant_tiles.len() {
                            let under = self.scratch_descendant_tiles[j];
                            if !self.tile_fading_from_empty(under)
                                && self.scratch_draw_seen.insert(under)
                            {
                                self.scratch_draw_tiles.push(under);
                            }
                        }
                    }
                }
                if self.scratch_draw_seen.insert(key) {
                    self.scratch_draw_tiles.push(key);
                }
                continue;
            }
            if let Some(draw_key) = self.find_ready_ancestor(key) {
                if self.scratch_draw_seen.insert(draw_key) {
                    self.scratch_draw_tiles.push(draw_key);
                }
                continue;
            }
            self.fill_ready_descendants(key);
            for j in 0..self.scratch_descendant_tiles.len() {
                let draw_key = self.scratch_descendant_tiles[j];
                if self.scratch_draw_seen.insert(draw_key) {
                    self.scratch_draw_tiles.push(draw_key);
                }
            }
        }
    }

    fn tile_is_ready(&self, key: TileKey) -> bool {
        self.tiles.get(&key).is_some_and(|entry| {
            if let TileLoadState::Ready {
                fill_geometry,
                stroke_geometry,
                feature_count,
                ..
            } = &entry.state
            {
                *feature_count > 0 || fill_geometry.is_some() || stroke_geometry.is_some()
            } else {
                false
            }
        })
    }

    fn find_ready_ancestor(&self, mut key: TileKey) -> Option<TileKey> {
        while key.z > 0 {
            key = TileKey {
                z: key.z - 1,
                x: key.x / 2,
                y: key.y / 2,
            };
            if self.tile_is_ready(key) {
                return Some(key);
            }
        }
        None
    }

    fn fill_ready_descendants(&mut self, key: TileKey) {
        self.scratch_descendant_tiles.clear();
        for (candidate, entry) in &self.tiles {
            if !matches!(entry.state, TileLoadState::Ready { .. }) {
                continue;
            }
            if is_descendant_tile(*candidate, key) {
                self.scratch_descendant_tiles.push(*candidate);
            }
        }
    }

    fn request_tile(
        &mut self,
        cx: &mut Cx,
        tile_key: TileKey,
        attempts: u8,
        allow_network: bool,
    ) -> bool {
        if attempts == 0 && !self.use_local_mbtiles {
            let cache_path = tile_data_cache_path_for(tile_key);
            if let Ok(cached_body) = fs::read_to_string(&cache_path) {
                // Offload heavy JSON parsing + tessellation to the thread pool
                let sender = self.tile_worker_rx.sender();
                let style_epoch = self.style_epoch;
                let theme_style = self.active_style().clone();
                let bucket = self.render_bucket();
                self.tiles.insert(
                    tile_key,
                    TileEntry {
                        state: TileLoadState::LoadingLocal,
                        last_used: self.frame_counter,
                        attempts: 0,
                        retry_after: 0,
                        bytes: 0,
                        bucket,
                        baked_3d: self.baked_3d_mode,
                        road_core_cached: false,
                        road_icon_indices: Vec::new(),
                        road_icon_vertices: Vec::new(),
                        fade: None,
                    },
                );
                if let Err(error) = self.submit_tile_job(cx, tile_key, move || {
                    match build_tile_buffers_from_body(tile_key, &cached_body, &theme_style, bucket)
                    {
                        Ok(buffers) => {
                            let _ = sender.send(TileWorkerMessage::NetworkTileParsed {
                                style_epoch,
                                tile_key,
                                buffers,
                            });
                        }
                        Err(_err) => {
                            let _ = fs::remove_file(&cache_path);
                            let _ = sender.send(TileWorkerMessage::NetworkTileParseFailed {
                                style_epoch,
                                tile_key,
                                error: String::new(),
                            });
                        }
                    }
                }) {
                    self.mark_tile_failed(tile_key, &format!("tile worker submission failed: {error}"));
                    self.update_status_text();
                    self.redraw(cx);
                }
                return false;
            }
        }

        if !allow_network || !self.use_network {
            return false;
        }

        let request_id = LiveId(self.next_request_id);
        self.next_request_id = self.next_request_id.wrapping_add(1);
        if self.next_request_id == 0 {
            self.next_request_id = 1;
        }

        let query = overpass_query(tile_key);
        let endpoint = overpass_endpoint(attempts);
        let mut request = HttpRequest::new(endpoint.to_string(), HttpMethod::POST);
        request.set_header("Content-Type".to_string(), "text/plain".to_string());
        request.set_header("Accept".to_string(), "application/json".to_string());
        request.set_header("User-Agent".to_string(), "makepad-map-view".to_string());
        request.set_body_string(&query);

        self.request_to_tile
            .insert(request_id, PendingTileRequest { tile_key, endpoint });
        let bucket = self.render_bucket();
        self.tiles.insert(
            tile_key,
            TileEntry {
                state: TileLoadState::LoadingNetwork,
                last_used: self.frame_counter,
                attempts,
                retry_after: 0,
                bytes: 0,
                bucket,
                baked_3d: self.baked_3d_mode,
                road_core_cached: false,
                road_icon_indices: Vec::new(),
                road_icon_vertices: Vec::new(),
                fade: None,
            },
        );
        cx.http_request(request_id, request);
        true
    }

    fn place_and_draw_labels(
        &mut self,
        cx: &mut Cx2d,
        draw_tiles: &[TileKey],
        view_zoom: f64,
        map_offset: Vec2d,
        rect: Rect,
    ) -> bool {
        let now = cx.seconds_since_app_start();
        // Pan-only frames: redraw the cached placement shifted by the pan
        // delta instead of re-scanning/re-shaping/re-colliding every label.
        let pan_delta = map_offset - self.label_cache_offset;
        let pan_dist = pan_delta.x.abs().max(pan_delta.y.abs());
        let rot_delta = self.rotation - self.label_cache_rotation;
        // The fold does NOT invalidate the cache: glyphs are emitted
        // unwarped and DrawRotatedText folds them per frame from the same
        // uniforms the tiles get, so warp-amount changes (and rotation
        // under the fold) ride the GPU exactly like tile geometry.
        let tilt_delta = self.tilt != self.label_cache_tilt;
        let cache_strict = self.label_cache_valid
            && self.label_cache_zoom == view_zoom
            && rot_delta == 0.0
            && !tilt_delta
            && self.label_cache_generation == self.tiles_generation
            && self.label_cache_tiles.as_slice() == draw_tiles
            && pan_dist < LABEL_REPLACE_PAN_PX;
        // Softly-stale cache is still fine to show briefly; rate-limit the
        // expensive full re-place. This covers active zooming too — labels
        // stay pinned in screen space for up to ~125ms during the gesture
        // (pinch behavior a la Google Maps) instead of re-placing every
        // frame, which was 5-20ms/frame at label-dense zooms. Small
        // rotation deltas reuse the cache RIGIDLY rotated about the pivot —
        // that's what keeps labels from wiggling during heading-up nav —
        // but only at identical zoom (rotation+zoom compose non-affinely
        // with the cached-screen transform below).
        // While the camera is MOVING (rotation, tilt, zoom gesture, warp
        // tween), keep riding the cached placement on the GPU transforms —
        // a mid-gesture re-place costs 5-20ms AND lands on positions a
        // frame stale, which reads as labels trailing the map. The full
        // re-place runs once the camera has been quiet for a beat, and
        // lands where the transforms already put everything.
        let camera_sig = (
            self.rotation,
            self.tilt,
            view_zoom,
            self.space_warp_eff.amount,
        );
        if camera_sig != self.camera_motion_sig {
            self.camera_motion_sig = camera_sig;
            self.camera_motion_last = Some(now);
        }
        let camera_moving = self
            .camera_motion_last
            .is_some_and(|at| now - at < LABEL_SETTLE_SECONDS);
        let cache_soft = self.label_cache_valid
            && ((rot_delta == 0.0 && !tilt_delta)
                || self.label_cache_zoom == view_zoom)
            && (self.label_cache_zoom - view_zoom).abs() < 0.5
            && (camera_moving
                || self
                    .last_full_place_time
                    .is_some_and(|at| now - at < LABEL_REPLACE_MIN_SECONDS));
        if cache_strict || cache_soft {
            if camera_moving {
                // Guarantee the settle re-place: the last gesture frame
                // leaves no other redraw scheduled, so arm one past the
                // quiet window (re-armed each moving frame, fires once).
                cx.cx.stop_timer(self.zoom_settle_timer);
                self.zoom_settle_timer = cx.cx.start_timeout(LABEL_SETTLE_SECONDS + 0.02);
            }
            // Screen positions transform affinely under zoom-about-cursor:
            // s_new = s_old * k + R·(off_new - off_old * k) with the
            // heading-up rotation R applied about the view pivot. A plain
            // offset during zoom flung cached labels thousands of px away.
            let k = 2.0_f64.powf(view_zoom - self.label_cache_zoom);
            let raw_shift = map_offset - self.label_cache_offset * k;
            let camera_vec = |v: Vec2d| {
                let r = self.rotate_screen_vec(v);
                dvec2(r.x, r.y * self.tilt_cos())
            };
            let mut shift = camera_vec(raw_shift);
            if k != 1.0 {
                let pivot = rect.pos + rect.size * 0.5;
                shift += (pivot - camera_vec(pivot)) * (1.0 - k);
            }
            // The GPU camera-delta matrix transforms everything AFTER the
            // CPU offsets are applied — pre-invert the pan shift so it
            // lands where intended: shift_pre = M^-1 * shift.
            {
                let (dc, ds) = ((-rot_delta).to_radians().cos(), (-rot_delta).to_radians().sin());
                let t0 = self
                    .label_cache_tilt
                    .clamp(0.0, TILT_HARD_MAX_DEG)
                    .to_radians()
                    .cos()
                    .max(1e-6);
                let t1 = self.tilt_cos().max(1e-6);
                let (a, b, c, d) = (dc, -ds / t0, t1 * ds, t1 * dc / t0);
                let det = a * d - b * c;
                if det.abs() > 1e-9 {
                    let (sx, sy) = (shift.x, shift.y);
                    shift = dvec2((d * sx - b * sy) / det, (-c * sx + a * sy) / det);
                }
            }
            // Screen-space delta rotation about the view pivot (phi = -rotation);
            // the cached placement's tilt_cos rides along so the draw can
            // build the exact non-commuting delta matrix.
            let rot_rad = (-rot_delta).to_radians() as f32;
            let cached_tilt_cos =
                (self.label_cache_tilt.clamp(0.0, TILT_HARD_MAX_DEG).to_radians().cos() as f32).max(1e-4);
            let pivot = rect.pos + rect.size * 0.5;
            self.draw_label_plans_scaled(
                cx,
                k as f32,
                Vec2f {
                    x: shift.x as f32,
                    y: shift.y as f32,
                },
                rot_rad,
                Vec2f {
                    x: pivot.x as f32,
                    y: pivot.y as f32,
                },
                cached_tilt_cos,
                false,
            );
            return false;
        }
        self.last_full_place_time = Some(now);

        let mut label_perf = LabelPerfStats::default();
        self.collect_label_candidates(draw_tiles, view_zoom, map_offset, rect, &mut label_perf);
        if self.scratch_candidates.is_empty() {
            self.path_glyphs.clear();
            self.scratch_accepted_plans.clear();
            self.store_label_cache(draw_tiles, view_zoom, map_offset);
            self.label_perf = label_perf;
            return true;
        }
        self.scratch_candidates
            .sort_unstable_by(|a, b| {
                b.score
                    .total_cmp(&a.score)
                    .then_with(|| a.name_key.cmp(&b.name_key))
            });
        let candidate_budget = label_candidate_budget(view_zoom);
        if self.scratch_candidates.len() > candidate_budget {
            self.scratch_candidates.truncate(candidate_budget);
        }
        label_perf.candidates_kept = self.scratch_candidates.len();
        label_perf.shape_budget = label_shape_attempt_budget(view_zoom);

        self.path_glyphs.clear();
        // Clear but retain allocations from previous frames
        for v in self.scratch_accepted_centers.values_mut() {
            v.clear();
        }
        self.scratch_accepted_bounds.clear();
        self.scratch_accepted_plans.clear();

        // During gestures the budget keeps re-places to ~a frame; at rest run
        // a full pass, otherwise the tail (house numbers) would never place —
        // each pass restarts from the same highest-scored candidates.
        let at_rest = pan_dist < 1.0
            && (self.label_cache_zoom - view_zoom).abs() < 1e-9
            && self
                .last_zoom_change_time
                .is_none_or(|at| now - at > 0.25);
        let place_budget_ms = if at_rest { 40.0 } else { LABEL_PLACE_BUDGET_MS };
        let place_start = now;
        for candidate_index in 0..self.scratch_candidates.len() {
            if (cx.seconds_since_app_start() - place_start).max(0.0) * 1000.0 > place_budget_ms {
                label_perf.rejected_budget +=
                    label_perf.candidates_kept.saturating_sub(candidate_index);
                break;
            }
            let candidate = &self.scratch_candidates[candidate_index];
            // Every pin needs ITS number: two 120kW sites near each other
            // are different chargers, not a repeated street name — the
            // name-key repeat suppression must not blank the second pin.
            let close_repeat = candidate.color_class != LABEL_CLASS_PIN
                && self
                .scratch_accepted_centers
                .get(&candidate.name_key)
                .is_some_and(|centers| {
                    let r2 = candidate.repeat_distance * candidate.repeat_distance;
                    centers.iter().any(|c| {
                        let dx = c.x - candidate.center.x;
                        let dy = c.y - candidate.center.y;
                        dx * dx + dy * dy < r2
                    })
                });
            if close_repeat {
                label_perf.rejected_repeat += 1;
                continue;
            }

            let estimated_width =
                estimate_label_width_pixels(&candidate.text, candidate.font_scale);
            if candidate.path_length < estimated_width + 4.0 {
                label_perf.rejected_pre_short += 1;
                continue;
            }

            if label_perf.shaped_attempts >= label_perf.shape_budget {
                label_perf.rejected_budget +=
                    label_perf.candidates_kept.saturating_sub(candidate_index);
                break;
            }
            label_perf.shaped_attempts += 1;
            // Build placement needs mutable self for draw_label + path_glyphs,
            // but only reads scratch_candidates[candidate_index] immutably.
            // Safe because build_label_placement doesn't touch scratch_candidates.
            let candidate_ptr = &self.scratch_candidates[candidate_index] as *const LabelCandidate;
            let candidate_ref = unsafe { &*candidate_ptr };
            let Some(placement) = self.build_label_placement(cx, candidate_ref) else {
                label_perf.rejected_plan_none += 1;
                continue;
            };
            label_perf.shaped_ok += 1;
            if rect_outside_rect(placement.bounds, rect, LABEL_VIEW_MARGIN) {
                self.path_glyphs.truncate(placement.glyph_start);
                label_perf.rejected_outside += 1;
                continue;
            }
            // In-pin text never collision-culls: it sits INSIDE the pin
            // bubble (which already icon-collides), so losing to a nearby
            // place/street label just blanked the pin. It still RESERVES
            // its box so street text avoids the area.
            let is_pin_text = self.scratch_candidates[candidate_index].color_class
                == LABEL_CLASS_PIN;
            if !is_pin_text
                && self.scratch_accepted_bounds.iter().any(|placed| {
                    rects_overlap_with_padding(*placed, placement.bounds, LABEL_COLLISION_PADDING)
                })
            {
                self.path_glyphs.truncate(placement.glyph_start);
                label_perf.rejected_collision += 1;
                continue;
            }

            let candidate = &self.scratch_candidates[candidate_index];
            let name_key = &candidate.name_key;
            if let Some(centers) = self.scratch_accepted_centers.get_mut(name_key) {
                centers.push(placement.center);
            } else {
                let key = name_key.clone();
                self.scratch_accepted_centers
                    .entry(key)
                    .or_default()
                    .push(placement.center);
            }
            // Pin text reserves the pin BUBBLE's box (not its own glyph
            // box): POI/street labels then place beside the pin instead of
            // under it, while the brand label below the tail stays legal.
            if is_pin_text {
                let anchor_x = placement.center.x - 3.0;
                let anchor_y = placement.center.y + 12.35;
                self.scratch_accepted_bounds.push(Rect {
                    pos: dvec2(anchor_x - 16.0, anchor_y - 27.0),
                    size: dvec2(32.0, 28.0),
                });
            } else {
                self.scratch_accepted_bounds.push(placement.bounds);
            }
            let glyph_count = placement.glyph_end - placement.glyph_start;
            label_perf.drawn_labels += 1;
            label_perf.drawn_glyphs += glyph_count;
            let score = candidate.score + candidate.source_rank as f64 * 2.0;
            self.scratch_accepted_hashes
                .push(stable_label_key(&candidate.name_key, &candidate.road_kind));
            // Post-icon phase: in-pin text and charger brand draw AFTER
            // the symbol pass so they sit on the pins, not under them.
            let post_icon = candidate.color_class == LABEL_CLASS_PIN
                || candidate.road_kind.starts_with("chb");
            // Billboard pin-phase plans anchor at the SITE point (the pin's
            // baked anchor): back the screen-px layout shift out of the
            // placement center so glyph offsets carry the layout instead.
            let lift_px = candidate.lift_px;
            let layout_shift = if candidate.color_class == LABEL_CLASS_PIN {
                (3.0f32, -12.35f32 - lift_px)
            } else if candidate.road_kind.starts_with("chb") {
                (0.0, 9.0 - lift_px)
            } else if candidate.road_kind.starts_with("poi") && lift_px > 0.0 {
                (0.0, -lift_px - 12.0)
            } else if (candidate.road_kind.starts_with("stS")
                || candidate.road_kind.starts_with("stp"))
                && lift_px > 0.0
            {
                (0.0, -lift_px - 10.0)
            } else {
                (0.0, 0.0)
            };
            self.scratch_accepted_plans.push((
                score,
                placement.glyph_start,
                placement.glyph_end,
                candidate.color_class,
                post_icon,
                candidate.screen_point,
                Vec2f {
                    x: placement.center.x as f32 - layout_shift.0,
                    y: placement.center.y as f32 - layout_shift.1,
                },
                candidate.baked_lift_px,
            ));
        }

        self.prev_label_keys.clear();
        self.prev_label_keys
            .extend(self.scratch_accepted_hashes.drain(..));

        self.scratch_accepted_plans
            .sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        self.draw_label_plans(cx, Vec2f { x: 0.0, y: 0.0 });
        self.store_label_cache(draw_tiles, view_zoom, map_offset);
        // budget-truncated passes need another wake to place the tail
        self.needs_label_followup = label_perf.rejected_budget > 0;
        self.label_perf = label_perf;
        true
    }

    /// Draw the current accepted label plans (halo underdraw + colored text)
    /// as one glyph instance batch, optionally shifted by a screen offset
    /// (used to redraw the cached placement while panning).
    fn draw_label_plans(&mut self, cx: &mut Cx2d, extra_offset: Vec2f) {
        let current_tilt_cos = (self.tilt_cos() as f32).max(1e-4);
        self.draw_label_plans_scaled(
            cx,
            1.0,
            extra_offset,
            0.0,
            Vec2f { x: 0.0, y: 0.0 },
            current_tilt_cos,
            false,
        );
    }

    /// Redraw only the pin-class (in-bubble) label plans — called after
    /// the icon pass so kW text sits on top of the charger pins.
    fn draw_pin_label_phase(&mut self, cx: &mut Cx2d) {
        let (scale, offset, rot, pivot, cached_tilt_cos) = self.label_draw_transform;
        self.draw_label_plans_scaled(cx, scale, offset, rot, pivot, cached_tilt_cos, true);
    }

    fn draw_label_plans_scaled(
        &mut self,
        cx: &mut Cx2d,
        scale: f32,
        extra_offset: Vec2f,
        rot: f32,
        pivot: Vec2f,
        cached_tilt_cos: f32,
        pin_phase: bool,
    ) {
        // Remember the transform so the pin-text phase redraws with the
        // exact same mapping after the icon pass.
        self.label_draw_transform = (scale, extra_offset, rot, pivot, cached_tilt_cos);
        self.label_cache_tilt_cos_for_delta = cached_tilt_cos;
        // 4 diagonal offsets read as a solid halo at map label sizes and
        // halve the glyph volume vs an 8-direction ring
        const HALO_OFFSETS: [(f32, f32); 4] = [
            (-0.8, -0.8),
            (0.8, -0.8),
            (-0.8, 0.8),
            (0.8, 0.8),
        ];
        let dark_theme = self.dark_theme;
        let (label_color, halo_color) = {
            let style = self.active_style();
            (style.label, style.label_halo)
        };
        // Rigid delta-rotation of the cached placement about the pivot
        // (heading-up nav): transform a copy once, draw slices from it.
        // Camera-delta on the GPU: the EXACT delta between the cached
        // placement's camera and now. The placement maps world points as
        // rotate-about-pivot THEN y-compress by tilt_cos; the delta from
        // (r0, t0) to (r1, t1) is S(t1)*R(d)*S(1/t0) — a general 2x2 (S
        // and R do not commute), which is why a plain rotate+scale
        // snapped visibly at every re-place in 2.5D.
        let (dc, ds) = (rot.cos(), rot.sin());
        let t0 = self.label_cache_tilt_cos_for_delta;
        let t1 = self.tilt_cos() as f32;
        let m = [dc, -ds / t0, t1 * ds, t1 * dc / t0];
        self.draw_label.set_camera_delta(cx.cx, m, pivot);
        // The fold, stamped with the SAME values the tile shader carries
        // this frame: labels are emitted unwarped and bend on the GPU, so
        // they track rotation/tilt/warp exactly like tile geometry.
        let w = &self.space_warp_eff;
        self.draw_label.set_space_warp(
            cx.cx,
            [w.amount as f32, w.start_px as f32, w.radius_px as f32, w.sin_t as f32],
            [w.kappa as f32, 0.0, w.cap as f32],
            self.tilt_cos() as f32,
        );
        // Pan/zoom ride uniforms too (same-frame as the tile map_offset):
        // glyphs below emit in CACHED placement space, scale 1, no offset.
        self.draw_label.set_pan_delta(
            cx.cx,
            scale,
            Vec2f {
                x: extra_offset.x,
                y: extra_offset.y,
            },
        );
        self.draw_label.begin_glyph_batch(cx);
        for i in 0..self.scratch_accepted_plans.len() {
            let (_, start, end, color_class, post_icon, upright, anchor, baked_lift) =
                self.scratch_accepted_plans[i];
            if post_icon != pin_phase {
                continue;
            }
            self.draw_label.lift = baked_lift;
            let glyphs = &self.path_glyphs[start..end];
            let billboard = pin_phase && upright;
            // In-pin text sits on a solid pin color: no halo underdraw.
            if color_class != LABEL_CLASS_PIN {
                self.draw_label.draw_super.color = halo_color;
                for offset in HALO_OFFSETS {
                    let off = Vec2f {
                        x: offset.0,
                        y: offset.1,
                    };
                    if billboard {
                        self.draw_label
                            .draw_path_glyphs_billboard(cx, glyphs, 1.0, off, anchor);
                    } else if upright {
                        self.draw_label
                            .draw_path_glyphs_upright(cx, glyphs, 1.0, off, anchor);
                    } else {
                        self.draw_label.draw_path_glyphs_scaled(cx, glyphs, 1.0, off);
                    }
                }
            }
            self.draw_label.draw_super.color =
                label_class_color(color_class, label_color, dark_theme);
            let zero = Vec2f { x: 0.0, y: 0.0 };
            if billboard {
                self.draw_label
                    .draw_path_glyphs_billboard(cx, glyphs, 1.0, zero, anchor);
            } else if upright {
                self.draw_label
                    .draw_path_glyphs_upright(cx, glyphs, 1.0, zero, anchor);
            } else {
                self.draw_label.draw_path_glyphs_scaled(cx, glyphs, 1.0, zero);
            }
        }
        self.draw_label.lift = 0.0;
        self.draw_label.end_glyph_batch(cx);
    }

    fn store_label_cache(&mut self, draw_tiles: &[TileKey], view_zoom: f64, map_offset: Vec2d) {
        self.label_cache_valid = true;
        self.label_cache_offset = map_offset;
        self.label_cache_zoom = view_zoom;
        self.label_cache_rotation = self.rotation;
        self.label_cache_tilt = self.tilt;
        self.label_cache_tiles.clear();
        self.label_cache_tiles.extend_from_slice(draw_tiles);
        self.label_cache_generation = self.tiles_generation;
    }

    fn collect_label_candidates(
        &mut self,
        draw_tiles: &[TileKey],
        view_zoom: f64,
        map_offset: Vec2d,
        rect: Rect,
        label_perf: &mut LabelPerfStats,
    ) {
        // Reuse scratch_candidates: clear but retain per-element heap allocations
        // (String, Vec<Vec2d>) from previous frames so they don't re-allocate.
        for c in self.scratch_candidates.iter_mut() {
            c.text.clear();
            c.name_key.clear();
            c.road_kind.clear();
            c.screen_path.clear();
        }
        let mut write_idx = 0usize;

        let rot = self.screen_rotation();
        let rot_pivot = rect.pos + rect.size * 0.5;
        let tilt_cos = self.tilt_cos();
        let rotated = rot != (1.0, 0.0) || tilt_cos != 1.0;
        // Labels are placed UNWARPED (the fold rides shader uniforms), so
        // the accept window must cover the ground the fold pulls back on
        // screen: the risen far wall lives ABOVE the flat frustum in
        // unwarped coordinates, and the near field spreads slightly with
        // perspective. Same honesty rule as the tile cull.
        let label_rect = if self.space_warp_eff.is_on() {
            let half_h = rect.size.y * 0.5;
            let half_h_flat = half_h / tilt_cos.max(0.05);
            let (reach, widen) = self.space_warp_eff.cull_extents(half_h, half_h_flat);
            let top = (rot_pivot.y - reach * tilt_cos).min(rect.pos.y);
            let half_w = rect.size.x * 0.5 * widen;
            Rect {
                pos: dvec2(rot_pivot.x - half_w, top),
                size: dvec2(half_w * 2.0, rect.pos.y + rect.size.y - top),
            }
        } else {
            rect
        };

        for key in draw_tiles {
            label_perf.draw_tiles += 1;
            let Some(entry) = self.tiles.get(key) else {
                continue;
            };
            let TileLoadState::Ready { labels, .. } = &entry.state else {
                continue;
            };
            if labels.is_empty() {
                continue;
            }
            label_perf.tiles_with_labels += 1;
            label_perf.labels_in_tiles += labels.len();
            let scale64 = 2.0_f64.powf(view_zoom - key.z as f64);
            let scale = scale64 as f32;
            // Label paths are tile-local; add this tile's screen offset.
            let tile_offset = map_offset
                + dvec2(
                    key.x as f64 * TILE_SIZE * scale64,
                    key.y as f64 * TILE_SIZE * scale64,
                );
            let zoom_delta = (view_zoom - key.z as f64).abs();

            for label in labels {
                label_perf.labels_scanned += 1;
                let Some(source_rank) = label_source_rank(&label.source_layer) else {
                    continue;
                };
                let is_address = label.source_layer == "addresses";
                let is_poi = label.source_layer == "pois";
                // carto placenames zoom gates by settlement kind.
                let place = label.road_kind.strip_prefix("place:").map(|rest| {
                    let (kind, population) = rest.split_once(':').unwrap_or((rest, "0"));
                    (kind, population.parse::<u64>().unwrap_or(0))
                });
                if let Some((kind, _)) = place {
                    let min_zoom = match kind {
                        "city" => 4.0,
                        "town" => 7.0,
                        "village" | "suburb" => 11.5,
                        _ => 13.5,
                    };
                    let max_zoom = match kind {
                        "city" => 15.5,
                        "town" => 16.5,
                        _ => 17.0,
                    };
                    if view_zoom < min_zoom || view_zoom > max_zoom {
                        continue;
                    }
                }
                if is_address && view_zoom < ADDRESS_LABEL_MIN_ZOOM {
                    continue;
                }
                // Charger pin text carries the pin's zoom floor in its key
                // ("chp11_..."): stale deeper tiles must not flash numbers
                // for pins the icon shader is hiding at this view zoom.
                if let Some(rest) = label.road_kind.strip_prefix("chp") {
                    let floor: f64 = rest
                        .split('_')
                        .next()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0.0);
                    if view_zoom < floor - 0.6 {
                        continue;
                    }
                }
                if label.road_kind.starts_with("chb") && view_zoom < 12.75 {
                    continue;
                }
                // District names: gemeente wide-out, wijk mid, buurt close.
                if let Some(rest) = label.road_kind.strip_prefix("adm") {
                    let (floor, ceil) = match rest.chars().next() {
                        Some('g') => (8.0, 12.0),
                        Some('w') => (11.5, 14.0),
                        _ => (13.5, 17.0),
                    };
                    if view_zoom < floor || view_zoom > ceil {
                        continue;
                    }
                }
                // Stop names: stations from z13, local tram/bus stops z15+.
                if label.source_layer == "stops" {
                    let floor = if label.road_kind.starts_with("stS") { 13.0 } else { 15.0 };
                    if view_zoom < floor {
                        continue;
                    }
                }
                if is_poi && view_zoom < POI_LABEL_MIN_ZOOM {
                    continue;
                }
                // Cheap precomputed-bbox viewport reject before any path work;
                // most of an overzoomed tile's labels are far offscreen. The
                // bbox is world-aligned, so under rotation widen the margin
                // to the extra reach a rotated viewport corner can have.
                let bbox = label.bbox;
                let rot_margin = LABEL_VIEW_MARGIN
                    + if rotated {
                        (rect.size.x + rect.size.y) * 0.25
                    } else {
                        0.0
                    }
                    // The fold sees further than the flat frustum: widen
                    // the map-space pre-cull by the same extra reach the
                    // accept window got, or wall labels die here first.
                    + (rect.pos.y - label_rect.pos.y).max(0.0)
                    + (label_rect.size.x - rect.size.x).max(0.0) * 0.5;
                if (bbox.2 as f64 * scale64 + tile_offset.x) < rect.pos.x - rot_margin
                    || (bbox.3 as f64 * scale64 + tile_offset.y) < rect.pos.y - rot_margin
                    || (bbox.0 as f64 * scale64 + tile_offset.x)
                        > rect.pos.x + rect.size.x + rot_margin
                    || (bbox.1 as f64 * scale64 + tile_offset.y)
                        > rect.pos.y + rect.size.y + rot_margin
                {
                    continue;
                }
                // precomputed at tile build; no per-frame allocation
                let name_key = &label.name_key;
                let is_exit = label.source_layer == "street_labels_points";
                let is_pin_text = label.color_class == LABEL_CLASS_PIN;
                if name_key.len() < if is_address || is_exit || is_pin_text { 1 } else { 2 } {
                    continue;
                }

                // Build screen_path into scratch buffer, then move it into candidate
                self.scratch_screen_path.clear();
                build_screen_polyline_into(
                    &label.path_points,
                    scale,
                    tile_offset,
                    rot,
                    tilt_cos,
                    rot_pivot,
                    &mut self.scratch_screen_path,
                );
                // Point labels (addresses, POI names) stay upright: keep the
                // rotated anchor but restore a horizontal baseline.
                let is_screen_point = is_address
                    || is_poi
                    || matches!(
                        label.source_layer.as_str(),
                        "chargers"
                            | "charger_brand"
                            | "place_labels"
                            | "micro_pois"
                            | "street_labels_points"
                    );
                if rotated && is_screen_point && self.scratch_screen_path.len() == 2 {
                    let a = self.scratch_screen_path[0];
                    let b = self.scratch_screen_path[1];
                    let mid = (a + b) * 0.5;
                    let half = (b - a).length() * 0.5;
                    self.scratch_screen_path[0] = dvec2(mid.x - half, mid.y);
                    self.scratch_screen_path[1] = dvec2(mid.x + half, mid.y);
                }
                // Charger brand reads just under the billboard pin: a fixed
                // SCREEN-space drop below the site anchor (a map-space offset
                // would tilt-compress and orbit the pin under rotation).
                // Flying-marker labels ride their marker's BAKED stalk
                // height (dynamic: each pin clears its own building).
                let lift_px = self.lift_screen_px(label.lift_m, view_zoom);
                // Total upward screen shift baked into this path — the glyph
                // shader camera-deltas the GROUND anchor and re-applies it.
                let mut baked_lift_px = 0.0f64;
                // Terrain: labels ride the displaced ground like the tiles.
                // The path stays UNWARPED here — the fold + perspective
                // are applied per frame in DrawRotatedText's vertex fn
                // (space_warp uniforms, same values as the tiles), which
                // is what keeps labels glued to the map while the camera
                // rotates under the warp instead of trailing the next
                // CPU re-place. Lifts stay plain screen shifts; the
                // shader perspective-scales them at the label's own
                // ground point, as the CPU bake used to.
                if !self.scratch_screen_path.is_empty() {
                    let ground_px =
                        self.terrain_ground_lift_px_at_screen(self.scratch_screen_path[0]);
                    if ground_px > 0.0 {
                        baked_lift_px += ground_px;
                        for p in self.scratch_screen_path.iter_mut() {
                            p.y -= ground_px;
                        }
                    }
                }
                if is_poi && lift_px > 0.0 {
                    // Above the floating icon.
                    baked_lift_px += lift_px + 12.0;
                    for p in self.scratch_screen_path.iter_mut() {
                        p.y -= lift_px + 12.0;
                    }
                }
                if label.source_layer == "stops" && lift_px > 0.0 {
                    baked_lift_px += lift_px + 10.0;
                    for p in self.scratch_screen_path.iter_mut() {
                        p.y -= lift_px + 10.0;
                    }
                }
                if label.road_kind.starts_with("chb") {
                    baked_lift_px += lift_px - 9.0;
                    for p in self.scratch_screen_path.iter_mut() {
                        p.y += 9.0 - lift_px;
                    }
                }
                // In-pin text: center in the droplet's text zone (right of
                // the bolt, above the tail); rides the stalk in 3D.
                if label.color_class == LABEL_CLASS_PIN {
                    baked_lift_px += 12.35 + lift_px;
                    for p in self.scratch_screen_path.iter_mut() {
                        p.x += 3.0;
                        p.y += -12.35 - lift_px;
                    }
                }
                if self.scratch_screen_path.len() < 2
                    || polyline_outside_rect(&self.scratch_screen_path, label_rect, LABEL_VIEW_MARGIN)
                {
                    continue;
                }
                self.scratch_cumulative.clear();
                polyline_cumulative_lengths_into(
                    &self.scratch_screen_path,
                    &mut self.scratch_cumulative,
                );
                let path_length = *self.scratch_cumulative.last().unwrap_or(&0.0);
                if path_length < LABEL_MIN_PATH_PIXELS {
                    continue;
                }
                let Some(center) = sample_polyline_point_at_distance(
                    &self.scratch_screen_path,
                    &self.scratch_cumulative,
                    path_length * 0.5,
                ) else {
                    continue;
                };
                if point_outside_rect(center, rect, LABEL_VIEW_MARGIN) {
                    continue;
                }

                let repeat_distance = if is_address {
                    20.0
                } else {
                    repeat_distance_for_label(label.priority, source_rank)
                };
                // Use a fixed font_scale per tile zoom level so that labels
                // don't shift along the path during continuous zoom.
                // Grow street text with zoom the way carto does (~9px z14 -> ~12px z17).
                let mut font_scale =
                    0.92_f32 * (1.0 + 0.14 * (view_zoom - 14.0).clamp(0.0, 3.0) as f32);
                font_scale *= match label.priority {
                    1 => 1.08,
                    2 => 1.0,
                    _ => 0.92,
                };
                if is_address {
                    font_scale = 0.60;
                } else if is_poi {
                    font_scale = 0.72;
                } else if label.source_layer == "chargers" {
                    font_scale = 0.78;
                } else if is_exit {
                    font_scale = 0.80;
                } else if let Some((kind, population)) = place {
                    // Kind sets the class, population separates Amsterdam
                    // from Purmerend within it.
                    font_scale = match kind {
                        "city" => match population {
                            p if p >= 500_000 => 1.65,
                            p if p >= 150_000 => 1.4,
                            _ => 1.2,
                        },
                        "town" => 1.05,
                        "village" | "suburb" => 0.95,
                        _ => 0.88,
                    };
                }
                // quantize so the shaped-run cache hits during continuous zoom
                font_scale = (font_scale * 32.0).round() / 32.0;

                // Point-anchored area labels (parks, squares, zoo
                // enclosures) have a ~zero-length path; without a length
                // credit every street name outscores them in dense
                // viewports and they never place.
                let effective_length = if label.path_points.len() <= 2 {
                    path_length.max(420.0)
                } else {
                    path_length
                };
                let mut score = source_rank as f64 * 1000.0
                    + (4_u8.saturating_sub(label.priority) as f64) * 120.0
                    + (220.0 - zoom_delta * 65.0)
                    + effective_length.min(640.0) * 0.35;
                if let Some((_, population)) = place {
                    // log-population tiebreak inside a settlement tier.
                    score += (population.max(1) as f64).log10() * 15.0;
                }
                // Hysteresis: prefer labels that were visible last frame so
                // panning doesn't flicker between competing candidates.
                if self
                    .prev_label_keys
                    .contains(&stable_label_key(name_key, &label.road_kind))
                {
                    score += 350.0;
                }

                // Reuse existing candidate slot or push a new one
                if write_idx < self.scratch_candidates.len() {
                    let c = &mut self.scratch_candidates[write_idx];
                    c.text.push_str(&label.text);
                    c.name_key.push_str(name_key);
                    c.road_kind.push_str(&label.road_kind);
                    c.color_class = label.color_class;
                    c.source_rank = source_rank;
                    c.score = score;
                    c.path_length = path_length;
                    c.center = center;
                    c.repeat_distance = repeat_distance;
                    c.font_scale = font_scale;
                    c.screen_point = is_screen_point;
                    c.lift_px = lift_px as f32;
                    c.baked_lift_px = baked_lift_px as f32;
                    c.screen_path.extend_from_slice(&self.scratch_screen_path);
                } else {
                    self.scratch_candidates.push(LabelCandidate {
                        text: label.text.clone(),
                        name_key: name_key.clone(),
                        road_kind: label.road_kind.clone(),
                        color_class: label.color_class,
                        source_rank,
                        score,
                        path_length,
                        center,
                        repeat_distance,
                        font_scale,
                        screen_point: is_screen_point,
                        lift_px: lift_px as f32,
                        baked_lift_px: baked_lift_px as f32,
                        screen_path: self.scratch_screen_path.clone(),
                    });
                }
                write_idx += 1;
                label_perf.candidates += 1;
            }
        }
        self.scratch_candidates.truncate(write_idx);
    }

    fn build_label_placement(
        &mut self,
        cx: &mut Cx2d,
        candidate: &LabelCandidate,
    ) -> Option<PathTextPlacement> {
        if candidate.screen_path.len() < 2 {
            return None;
        }

        // Smooth the candidate's screen_path into scratch_smooth_a,
        // using scratch_smooth_b and scratch_cumulative as temp buffers.
        let mut smooth_a = std::mem::take(&mut self.scratch_smooth_a);
        let mut smooth_b = std::mem::take(&mut self.scratch_smooth_b);
        let mut cum = std::mem::take(&mut self.scratch_cumulative);

        smooth_label_curve_into(
            &candidate.screen_path,
            &mut smooth_a,
            &mut smooth_b,
            &mut cum,
        );

        if smooth_a.len() < 2 {
            self.scratch_smooth_a = smooth_a;
            self.scratch_smooth_b = smooth_b;
            self.scratch_cumulative = cum;
            return None;
        }

        // Shaping dominates placement cost; cache runs by (text, font_scale).
        let run_key = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            candidate.text.hash(&mut hasher);
            (
                hasher.finish(),
                candidate.text.len() as u32,
                candidate.font_scale.to_bits(),
            )
        };
        if !self.shaped_runs.contains_key(&run_key) {
            if self.shaped_runs.len() > 4096 {
                self.shaped_runs.clear();
            }
            self.draw_label.draw_super.font_scale = candidate.font_scale;
            let shaped = self
                .draw_label
                .draw_super
                .prepare_single_line_run(cx, candidate.text.as_str())
                .filter(|run| !run.glyphs.is_empty());
            self.shaped_runs.insert(run_key, shaped);
        }
        let run = match self.shaped_runs.get(&run_key) {
            Some(Some(run)) => run.clone(),
            _ => {
                self.scratch_smooth_a = smooth_a;
                self.scratch_smooth_b = smooth_b;
                self.scratch_cumulative = cum;
                return None;
            }
        };

        // Build cumulative lengths for the smoothed path
        cum.clear();
        polyline_cumulative_lengths_into(&smooth_a, &mut cum);

        let text_width = run.width_in_lpxs;
        let start_distance = choose_label_start_distance(&smooth_a, &cum, text_width as f64);
        let start_distance = match start_distance {
            Some(d) => d,
            None => {
                self.scratch_smooth_a = smooth_a;
                self.scratch_smooth_b = smooth_b;
                self.scratch_cumulative = cum;
                return None;
            }
        };

        let mid_distance = start_distance + text_width as f64 * 0.5;
        let probe_delta = (text_width as f64 * 0.25).clamp(12.0, 42.0);
        let mid_tangent_angle =
            sample_polyline_tangent_angle_raw(&smooth_a, &cum, mid_distance, probe_delta);
        let mid_tangent_angle = match mid_tangent_angle {
            Some(a) => a,
            None => {
                self.scratch_smooth_a = smooth_a;
                self.scratch_smooth_b = smooth_b;
                self.scratch_cumulative = cum;
                return None;
            }
        };
        // Reading direction from the chord across the whole text span: a
        // single mid-point tangent can point 180 degrees off on zigzag
        // segments (rail-yard paths), flipping the label upside down.
        let span_a = sample_polyline_point_at_distance(&smooth_a, &cum, start_distance);
        let span_b = sample_polyline_point_at_distance(
            &smooth_a,
            &cum,
            start_distance + text_width as f64,
        );
        let reverse = match (span_a, span_b) {
            (Some(a), Some(b)) if (b.x - a.x).abs() + (b.y - a.y).abs() > 6.0 => {
                choose_label_reverse(((b.y - a.y) as f32).atan2((b.x - a.x) as f32))
            }
            _ => choose_label_reverse(mid_tangent_angle),
        };
        let label_angle_bias = if reverse { std::f32::consts::PI } else { 0.0 };

        let baseline_shift = (run.ascender_in_lpxs + run.descender_in_lpxs)
            * 0.5
            * LABEL_BASELINE_SHIFT_FACTOR as f32;

        let mut result = self.draw_label.place_text_along_path(
            &run,
            &smooth_a,
            &cum,
            start_distance,
            reverse,
            baseline_shift,
            label_angle_bias,
            LABEL_MAX_GLYPH_TURN_RADIANS,
            LABEL_GLYPH_ANGLE_BLEND,
            candidate.center,
            &mut self.path_glyphs,
        );
        // HARD invariant instead of trusting the chord heuristic: if the
        // REALIZED glyph run reads leftward (net upside-down — hairpin
        // ramps fool any pre-placement guess, e.g. the inverted "A1"
        // motorway ref), throw it away and place flipped.
        if let Some(placed) = &result {
            if placed.glyph_end > placed.glyph_start + 1 {
                let a = self.path_glyphs[placed.glyph_start].glyph_origin;
                let b = self.path_glyphs[placed.glyph_end - 1].glyph_origin;
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let len = (dx * dx + dy * dy).sqrt();
                // Origin order IS reading order in both walk modes (the
                // reversed walk mirrors distances AND carries the pi bias).
                if len > 1.0 && dx / len < -LABEL_VERTICAL_AXIS_EPSILON {
                    let glyph_start = placed.glyph_start;
                    self.path_glyphs.truncate(glyph_start);
                    // Mirror the start so the flipped walk covers the SAME
                    // stretch of road: reusing start_distance verbatim lands
                    // on a different part of a curved path, whose local
                    // direction can match the original — an un-flippable
                    // "flip" that left hairpin labels upside down.
                    let total_length = cum.last().copied().unwrap_or(0.0);
                    let flipped_start = (total_length
                        - start_distance
                        - run.width_in_lpxs as f64)
                        .max(0.0);
                    result = self.draw_label.place_text_along_path(
                        &run,
                        &smooth_a,
                        &cum,
                        flipped_start,
                        !reverse,
                        baseline_shift,
                        if reverse { 0.0 } else { std::f32::consts::PI },
                        LABEL_MAX_GLYPH_TURN_RADIANS,
                        LABEL_GLYPH_ANGLE_BLEND,
                        candidate.center,
                        &mut self.path_glyphs,
                    );
                }
            }
        }

        self.scratch_smooth_a = smooth_a;
        self.scratch_smooth_b = smooth_b;
        self.scratch_cumulative = cum;
        result
    }

    fn update_status_text(&mut self) {
        let mut ready = 0usize;
        let mut loading = 0usize;
        let mut failed = 0usize;
        let mut retrying = 0usize;
        let mut exhausted = 0usize;
        let mut features = 0usize;

        for key in &self.visible_tiles {
            let Some(entry) = self.tiles.get(key) else {
                continue;
            };
            match &entry.state {
                TileLoadState::LoadingNetwork | TileLoadState::LoadingLocal => loading += 1,
                TileLoadState::Ready { feature_count, .. } => {
                    ready += 1;
                    features += *feature_count;
                }
                TileLoadState::Failed { .. } => {
                    failed += 1;
                    if entry.attempts >= MAX_TILE_RETRIES {
                        exhausted += 1;
                    } else {
                        retrying += 1;
                    }
                }
            }
        }

        let counters = (ready, loading, failed, retrying, exhausted, features);
        let lp = self.label_perf;
        // Skip format! if nothing changed since the last call
        if counters == self.prev_status_counters
            && lp == self.prev_status_label_perf
            && !self.status.is_empty()
        {
            return;
        }
        self.prev_status_counters = counters;
        self.prev_status_label_perf = lp;

        self.status = format!(
            "Amsterdam [{}|{}] z{:.2} (req:{})  ready:{}  loading:{}  failed:{}(retry:{} stuck:{})  features:{}  labels(tile:{} scan:{} cand:{}/{} shape:{}/{}(b:{}) draw:{} glyphs:{} rej:r{} ps{} p{} o{} c{} b{})",
            self.source_mode_label(), self.theme_label(), self.view_zoom(), self.request_zoom_level(),
            ready, loading, failed, retrying, exhausted, features,
            lp.labels_in_tiles, lp.labels_scanned, lp.candidates_kept, lp.candidates,
            lp.shaped_ok, lp.shaped_attempts, lp.shape_budget, lp.drawn_labels, lp.drawn_glyphs,
            lp.rejected_repeat, lp.rejected_pre_short, lp.rejected_plan_none,
            lp.rejected_outside, lp.rejected_collision, lp.rejected_budget,
        );
    }

    fn view_zoom(&self) -> f64 {
        let min = self.min_zoom.max(0.0);
        let max = self.max_zoom.max(min);
        self.zoom.clamp(min, max)
    }

    /// (cos, sin) of the screen rotation φ = -rotation — the transform that
    /// makes the `rotation` bearing point up. Identity when north-up.
    fn screen_rotation(&self) -> (f64, f64) {
        if self.rotation == 0.0 {
            return (1.0, 0.0);
        }
        let phi = -self.rotation.to_radians();
        (phi.cos(), phi.sin())
    }

    /// Rotate a screen vector from unrotated (world-aligned) space into
    /// rotated screen space.
    fn rotate_screen_vec(&self, v: Vec2d) -> Vec2d {
        let (cos, sin) = self.screen_rotation();
        dvec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
    }

    /// Inverse: rotated screen vector back into world-aligned screen space.
    fn unrotate_screen_vec(&self, v: Vec2d) -> Vec2d {
        let (cos, sin) = self.screen_rotation();
        dvec2(v.x * cos + v.y * sin, -v.x * sin + v.y * cos)
    }

    fn tilt_cos(&self) -> f64 {
        self.tilt.clamp(0.0, TILT_HARD_MAX_DEG).to_radians().cos()
    }

    /// Zoom-coupled tilt ceiling: the base 78° everywhere, ramping to the
    /// 85° hard cap across view zoom 18.5→20 — the extra steepness (a
    /// near-first-person camera) only unlocks together with street-level
    /// zoom, where the honest 1/cos(tilt) culling fan is a handful of
    /// over-zoomed tiles. Continuous in zoom, so the per-frame enforcement
    /// in draw_walk eases the camera down as it zooms away, never snaps.
    fn tilt_max_deg_now(&self) -> f64 {
        let t = ((self.view_zoom() - 18.5) / 1.5).clamp(0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        TILT_MAX_DEG + (TILT_HARD_MAX_DEG - TILT_MAX_DEG) * s
    }

    /// Screen-space vector (relative to the view pivot) back into
    /// world-aligned space: undo the tilt compression, then the rotation.
    fn screen_delta_to_world(&self, v: Vec2d) -> Vec2d {
        let tilt_cos = self.tilt_cos().max(1e-3);
        self.unrotate_screen_vec(dvec2(v.x, v.y / tilt_cos))
    }

    fn request_zoom_level(&self) -> u32 {
        let mut zoom = self.view_zoom().round() as u32;
        if self.use_local_mbtiles {
            // Honor the archive's declared zoom range: a single-zoom detail
            // archive (minzoom=maxzoom=14) must never be asked for z13/z12 —
            // those rows cannot exist and only produce missing-tile spam.
            let range = self
                .local_source_zoom_range
                .unwrap_or((LOCAL_MBTILES_MIN_ZOOM, LOCAL_MBTILES_MAX_ZOOM));
            let (min_zoom, max_zoom) = if range.0 <= range.1 && range.1 <= 30 {
                range
            } else {
                (LOCAL_MBTILES_MIN_ZOOM, LOCAL_MBTILES_MAX_ZOOM)
            };
            zoom = zoom.max(min_zoom).min(max_zoom);
        }
        zoom
    }

    /// Adopt minzoom/maxzoom from the asynchronously parsed root index.
    fn ensure_local_zoom_range(&mut self) {
        let range = self.base_archive.as_ref().and_then(MapTileArchive::zoom_range);
        if let Some((min, max)) = range {
            self.local_source_zoom_range = Some((min, max));
            if self.local_source_logged_zoom_range != Some((min, max)) {
                if (min, max) != (LOCAL_MBTILES_MIN_ZOOM, LOCAL_MBTILES_MAX_ZOOM) {
                    log!(
                        "MapView: archive declares zoom range z{}-z{}; clamping tile requests",
                        min,
                        max
                    );
                }
                self.local_source_logged_zoom_range = Some((min, max));
            }
        }
    }

    /// View-zoom bucket the tile styling (widths, AA, outlines) is built for.
    /// Beyond the source max zoom the same z14 tiles are re-styled per bucket.
    fn render_bucket(&self) -> u32 {
        // TWO keyframe buckets above the mid-zooms: 14 (view < 15.5) and
        // 16 (view >= 15.5, through the max zoom). Faces/strokes morph to
        // the live zoom on the GPU, icons carry their zoom floors and
        // reveal by the live icon_zoom uniform, so the only restyle event
        // left on the whole zoom axis is the single 14<->16 crossover —
        // and both keyframes morph to identical widths at that zoom.
        // Integer keyframes, switch-only (user call 2026-08-02): every
        // integer bucket 15-18 is baked and swaps at the half-zoom like
        // classic vector maps; GPU-expanded strokes stay smooth through
        // the crossings and the face morph remains an opt-in experiment
        // (/tmp/mp_face_morph) rather than the shipping path.
        (self.view_zoom().round() as u32).min(18)
    }

    fn source_mode_label(&self) -> &'static str {
        if self.use_local_mbtiles {
            "offline"
        } else if self.use_network {
            "online"
        } else {
            "disabled"
        }
    }

    fn theme_label(&self) -> &'static str {
        if self.dark_theme {
            "dark"
        } else {
            "light"
        }
    }
}

// --- Camera + overlay public API (the M0 interaction surface) ---

impl MapView {
    /// Hit-test the tappable charger pins of ready tiles against a screen
    /// point (billboard rect around the pin anchor, camera-transformed).
    /// Screen-px height of a flying marker above its ground anchor (0 in
    /// 2D) — the baked per-marker lift converted through the current tilt
    /// and meters-per-pixel.
    fn lift_screen_px(&self, lift_m: f32, view_zoom: f64) -> f64 {
        self.lift_ground_px(lift_m, view_zoom)
            * self.tilt.clamp(0.0, TILT_HARD_MAX_DEG).to_radians().sin()
    }

    /// The same lift in GROUND px (pre-tilt, the unit `SpaceWarp::project`
    /// takes): the warp rides it along the fold's LOCAL normal and through
    /// the perspective divide, so a flat `lift·sin(tilt)` screen offset is
    /// only correct when the fold is off.
    fn lift_ground_px(&self, lift_m: f32, view_zoom: f64) -> f64 {
        if !self.buildings_3d || self.tilt <= 0.0 || lift_m <= 0.0 {
            return 0.0;
        }
        let world_size = TILE_SIZE * 2f64.powf(view_zoom);
        let (_, lat) = normalized_to_lon_lat(self.center_norm);
        let px_per_meter = world_size / (40_075_016.686 * lat.to_radians().cos());
        lift_m as f64 * px_per_meter
    }

    fn pin_at(&self, abs: Vec2d) -> Option<(f64, f64, Vec<(String, String)>)> {
        let camera = self.overlay_camera();
        let view_zoom = self.view_zoom();
        let mut best: Option<(f64, &PinHit)> = None;
        for entry in self.tiles.values() {
            let TileLoadState::Ready { pin_hits, .. } = &entry.state else {
                continue;
            };
            for hit in pin_hits {
                let norm = dvec2(hit.norm.0, hit.norm.1);
                // Where the SHADER put this pin. Flying pins lift along the
                // fold normal and shrink with the divide once the warp is
                // on, so hit-testing has to project them the same way —
                // a flat straight-up lift misses by tens of px on the wall.
                let screen = if camera.warp.is_on() {
                    camera
                        .norm_to_screen_lifted(norm, self.lift_ground_px(hit.lift_m, view_zoom))
                        .0
                } else {
                    let ground = camera.norm_to_screen(norm);
                    dvec2(
                        ground.x,
                        ground.y - self.lift_screen_px(hit.lift_m, view_zoom),
                    )
                };
                let dx = abs.x - screen.x;
                let dy = abs.y - screen.y;
                if dx.abs() <= 18.0 && dy >= -26.0 && dy <= 6.0 {
                    let dist = dx * dx + dy * dy;
                    if best.as_ref().is_none_or(|(d, _)| dist < *d) {
                        best = Some((dist, hit));
                    }
                }
            }
        }
        let (_, hit) = best?;
        let (lon, lat) = normalized_to_lon_lat(dvec2(hit.norm.0, hit.norm.1));
        Some((lon, lat, hit.info.clone()))
    }

    /// Wind particle streaks: short comet segments advected by tick_wind,
    /// drawn through the overlay camera (pan/zoom/rotate/tilt aware).
    fn draw_wind_particles(&mut self, cx: &mut Cx2d) {
        if self.wind_field.is_none() || self.wind_particles.is_empty() {
            return;
        }
        let camera = self.overlay_camera();
        let rect = camera.rect;
        // The wind flows at its own altitude in 3D (~150 m — beneath the
        // 650 m rain deck): same screen-parallax as the cloud layer, so
        // rotating the camera shows ground, wind and clouds as three
        // separate planes.
        let wind_lift = self.lift_screen_px(150.0, self.view_zoom());
        let particles = std::mem::take(&mut self.wind_particles);
        // Same turtle-pinning pattern as draw_map_overlay: DrawVector paths
        // land in absolute screen coordinates.
        cx.begin_turtle(
            Walk {
                abs_pos: Some(rect.pos),
                width: Size::Fixed(rect.size.x),
                height: Size::Fixed(rect.size.y),
                margin: Inset::default(),
                metrics: Metrics::default(),
            },
            Layout {
                clip_x: true,
                clip_y: true,
                ..Layout::default()
            },
        );
        let dv = &mut self.draw_overlay;
        dv.begin();
        // Speed buckets share a stroke call each; palette flips with the
        // theme so streaks always run HIGH CONTRAST against the basemap
        // (near-black family on the light map, pale family on dark).
        let buckets: [(f32, f32, u32, f32); 4] = if self.dark_theme {
            [
                (0.0, 3.0, 0xcfd8e3, 0.6),
                (3.0, 7.0, 0x7ab8f5, 0.75),
                (7.0, 12.0, 0x4dd0c4, 0.85),
                (12.0, f32::MAX, 0xff8a65, 0.92),
            ]
        } else {
            [
                (0.0, 3.0, 0x1c2733, 0.65),
                (3.0, 7.0, 0x0d47a1, 0.8),
                (7.0, 12.0, 0x00695c, 0.88),
                (12.0, f32::MAX, 0xb3261e, 0.95),
            ]
        };
        // Nullschool-style comet trails: each particle draws its position
        // HISTORY as a polyline, split into tail/mid/head passes so the
        // trail tapers in alpha and width toward the tail.
        for (min_speed, max_speed, color, alpha) in buckets {
            for (seg_from, seg_to, alpha_mul, width) in [
                (0usize, 10usize, 0.22f32, 0.7f32),
                (10, 16, 0.5, 0.95),
                (16, WIND_TRAIL, 1.0, 1.25),
            ] {
                dv.clear();
                dv.set_color_hex(color, alpha * alpha_mul);
                let mut any = false;
                for particle in &particles {
                    if particle.speed < min_speed || particle.speed >= max_speed {
                        continue;
                    }
                    if particle.age < 4 || particle.history.len() < 2 {
                        continue;
                    }
                    let n = particle.history.len();
                    let from = seg_from.min(n - 1);
                    let to = seg_to.min(n);
                    if to <= from + 1 && from > 0 {
                        continue;
                    }
                    let start = camera.norm_to_screen(particle.history[from]);
                    dv.move_to(start.x as f32, (start.y - wind_lift) as f32);
                    for i in (from + 1)..to {
                        let p = camera.norm_to_screen(particle.history[i]);
                        dv.line_to(p.x as f32, (p.y - wind_lift) as f32);
                    }
                    any = true;
                }
                if any {
                    dv.stroke_opts(
                        width,
                        crate::makepad_draw::vector::LineCap::Butt,
                        crate::makepad_draw::vector::LineJoin::Miter,
                        4.0,
                        1.0,
                    );
                }
            }
        }
        dv.end(cx);
        cx.end_turtle();
        self.wind_particles = particles;
    }

    fn overlay_camera(&self) -> OverlayCamera {
        let world_size = tile_world_size_zoom(self.view_zoom());
        let center_world = self.center_norm * world_size;
        let rect = self.view_rect;
        let offset = dvec2(
            rect.pos.x + rect.size.x * 0.5 - center_world.x,
            rect.pos.y + rect.size.y * 0.5 - center_world.y,
        );
        let (_, lat) = normalized_to_lon_lat(self.center_norm);
        OverlayCamera {
            world_size,
            offset,
            rect,
            meters_per_px: 40_075_016.686 * lat.to_radians().cos() / world_size,
            rot: self.screen_rotation(),
            rot_pivot: rect.pos + rect.size * 0.5,
            rotation_deg: self.rotation,
            tilt_cos: self.tilt_cos(),
            warp: self.space_warp_eff,
        }
    }

    fn sync_camera_fields(&mut self) {
        let (lon, lat) = normalized_to_lon_lat(self.center_norm);
        self.center_lon = lon;
        self.center_lat = lat;
    }

    fn emit_viewport_changed(&mut self, cx: &mut Cx) {
        cx.widget_action(
            self.uid,
            MapViewAction::ViewportChanged {
                lon: self.center_lon,
                lat: self.center_lat,
                zoom: self.view_zoom(),
            },
        );
    }

    /// Screen point → map coordinate: the exact inverse of the camera that
    /// drew the frame. With the Inception fold on this runs the piecewise
    /// warp inverse (`SpaceWarp::unproject`), so taps and long presses land
    /// on the feature under the finger even up on the wall; with it off it
    /// is the legacy rotation + `1/tilt_cos` path, unchanged.
    pub fn screen_to_lon_lat(&self, abs: Vec2d) -> (f64, f64) {
        normalized_to_lon_lat(self.overlay_camera().screen_to_norm(abs))
    }

    pub fn lon_lat_to_screen(&self, lon: f64, lat: f64) -> Vec2d {
        let camera = self.overlay_camera();
        camera.norm_to_screen(lon_lat_to_normalized(lon, lat))
    }

    pub fn center(&self) -> (f64, f64) {
        normalized_to_lon_lat(self.center_norm)
    }

    pub fn map_zoom(&self) -> f64 {
        self.view_zoom()
    }

    pub fn set_center(&mut self, cx: &mut Cx, lon: f64, lat: f64) {
        self.fly = None;
        self.center_norm = lon_lat_to_normalized(lon, lat);
        self.wrap_and_clamp_center();
        self.sync_camera_fields();
        self.emit_viewport_changed(cx);
        self.redraw(cx);
    }

    /// Heading-up camera: the given bearing (degrees, 0 = north) points up.
    pub fn set_rotation(&mut self, cx: &mut Cx, rotation_deg: f64) {
        let rotation = rotation_deg.rem_euclid(360.0);
        if (rotation - self.rotation).abs() < 1e-9 {
            return;
        }
        self.rotation = rotation;
        self.redraw(cx);
    }

    pub fn rotation(&self) -> f64 {
        self.rotation
    }

    /// Axonometric camera tilt (degrees, 0 = top-down). Clamped to the
    /// HARD ceiling here; the zoom-coupled cap (`tilt_max_deg_now`) is
    /// enforced per-frame in draw_walk so a steep persisted tilt settles
    /// smoothly once the actual zoom is known.
    /// The following draw detects a flat/tilted mode transition and re-bakes
    /// once; keeping that invalidation in one place avoids duplicate work.
    pub fn set_tilt(&mut self, cx: &mut Cx, tilt_deg: f64) {
        let tilt = tilt_deg.clamp(0.0, TILT_HARD_MAX_DEG);
        if (tilt - self.tilt).abs() < 1e-9 {
            return;
        }
        self.tilt = tilt;
        self.redraw(cx);
    }

    /// Rebuild every resident tile under the current style/mode while its
    /// previous geometry stays on screen (bucket sentinel → the normal
    /// stale-bucket restyle path picks it up and cross-fades).
    /// Swap the tile source archives at runtime. Unlike the overlay swap
    /// there is nothing worth keeping on screen: geometry from the old
    /// archive is not this archive's, so every tile and both negative
    /// caches go, and the normal request path refills from the new source.
    /// (First-run bake: the app starts with no archive at all and points
    /// the view at the one it just built.) Empty strings clear the
    /// optional detail/bridge-dz sources.
    pub fn set_source_config(&mut self, cx: &mut Cx, config: TileSourceConfig) {
        if self.tile_source_config.as_ref() == Some(&config) {
            return;
        }
        self.install_archive_source(cx, config);
        self.tiles.clear();
        self.local_requested_tiles.clear();
        self.local_missing_tiles.clear();
        self.archive_pending_tiles.clear();
        self.local_source_missing_logged = false;
        // Force the zoom-range probe to re-read: the new archive declares
        // its own minzoom/maxzoom (a city extract is not the planet).
        self.local_source_zoom_range = None;
        self.local_source_logged_zoom_range = None;
        self.local_source_zoom_range_path = None;
        self.local_source_zoom_range_checked = false;
        self.redraw(cx);
    }

    pub fn source_config(&self) -> Option<&TileSourceConfig> {
        self.tile_source_config.as_ref()
    }

    pub fn set_source_paths(&mut self, cx: &mut Cx, base: &str, detail: &str, bridge_dz: &str) {
        self.set_source_config(
            cx,
            TileSourceConfig::LocalArchive {
                mbtiles_path: base.to_string(),
                detail_mbtiles_path: detail.to_string(),
                overlay_mbtiles_paths: self.overlay_mbtiles_paths.clone(),
                bridge_dz_path: bridge_dz.to_string(),
            },
        );
    }

    /// Swap the active geodata overlays; stale tiles keep rendering while
    /// rebuilt ones stream in with the new layer set.
    pub fn set_overlay_paths(&mut self, cx: &mut Cx, paths: &str) {
        if self.overlay_mbtiles_paths == paths {
            return;
        }
        self.overlay_mbtiles_paths = paths.to_string();
        if let Some(config) = self.tile_source_config.as_mut() {
            match config {
                TileSourceConfig::LocalArchive {
                    overlay_mbtiles_paths,
                    ..
                }
                | TileSourceConfig::HttpArchive {
                    overlay_mbtiles_paths,
                    ..
                } => *overlay_mbtiles_paths = paths.to_string(),
            }
        }
        self.restyle_tiles_keep_stale(cx);
    }

    /// Runtime shiny.md config update (feature toggles, time-of-day
    /// slider): mutates both themes' shiny style, recompiles, and restyles
    /// while stale tiles stay drawable — bake-flag flips are glitch-free.
    pub fn update_shiny(&mut self, cx: &mut Cx, update: impl Fn(&mut MapShinyStyle)) {
        update(&mut self.style_light.shiny);
        update(&mut self.style_dark.shiny);
        update(&mut self.style_circuit.shiny);
        self.rebuild_compiled_styles();
        self.restyle_tiles_keep_stale(cx);
    }

    /// The active theme's compiled shiny config.
    pub fn shiny(&self) -> &ShinyConfig {
        &self.active_style().shiny
    }

    fn restyle_tiles_keep_stale(&mut self, cx: &mut Cx) {
        self.style_epoch = self.style_epoch.wrapping_add(1);
        if self.style_epoch == 0 {
            self.style_epoch = 1;
        }
        // Drop Loading/Failed placeholders: their in-flight results are
        // discarded by the epoch check, and a placeholder that survives
        // here is never re-requested — a tile stuck broken forever.
        self.tiles
            .retain(|_, entry| matches!(entry.state, TileLoadState::Ready { .. }));
        for entry in self.tiles.values_mut() {
            entry.bucket = u32::MAX;
        }
        self.local_requested_tiles.clear();
        self.pending_ready_tiles.clear();
        self.label_cache_valid = false;
        self.redraw(cx);
    }

    /// Invalidate only the tilt-dependent tile overlay. Ready tiles retain
    /// their bucket and stable road-core cache, while the epoch bump rejects
    /// any in-flight bake from the previous camera mode.
    fn restyle_mode_overlay_keep_stale(&mut self, cx: &mut Cx) {
        self.style_epoch = self.style_epoch.wrapping_add(1);
        if self.style_epoch == 0 {
            self.style_epoch = 1;
        }
        self.tiles
            .retain(|_, entry| matches!(entry.state, TileLoadState::Ready { .. }));
        self.local_requested_tiles.clear();
        self.pending_ready_tiles.clear();
        self.label_cache_valid = false;
        self.redraw(cx);
    }

    pub fn tilt(&self) -> f64 {
        self.tilt
    }

    /// Install (or clear) the 10 m wind field driving the particle layer.
    pub fn set_wind_field(
        &mut self,
        cx: &mut Cx,
        nx: usize,
        ny: usize,
        u: Vec<f32>,
        v: Vec<f32>,
        bbox: (f64, f64, f64, f64),
    ) {
        cx.stop_timer(self.wind_timer);
        self.wind_particles.clear();
        if nx * ny == 0 || u.len() != nx * ny || v.len() != nx * ny {
            self.wind_field = None;
            self.redraw(cx);
            return;
        }
        self.wind_field = Some((nx, ny, u, v, bbox));
        self.wind_rng = 0x9e3779b97f4a7c15;
        self.wind_timer = cx.start_interval(1.0 / 30.0);
        self.redraw(cx);
    }

    /// The weather clock, slowed at deep zooms so streaks stay readable —
    /// rain playback AND wind advection both use this, keeping them in
    /// lockstep at every zoom.
    fn effective_weather_timelapse(&self) -> f64 {
        let view_zoom = self.view_zoom();
        let world_px = TILE_SIZE * 2f64.powf(view_zoom);
        let (_, lat) = normalized_to_lon_lat(self.center_norm);
        let meters_to_norm = 1.0 / (40_075_016.686 * lat.to_radians().cos());
        // px per tick a 10 m/s wind would cover at full time-lapse
        let px_per_tick = 10.0 * WEATHER_TIMELAPSE * (1.0 / 30.0) * meters_to_norm * world_px;
        if px_per_tick > 3.5 {
            WEATHER_TIMELAPSE * 3.5 / px_per_tick
        } else {
            WEATHER_TIMELAPSE
        }
    }

    /// Restart the rain frame timer if the effective clock moved >15%
    /// (zoom changed) — called from the wind/rain tick paths.
    fn retune_rain_timer(&mut self, cx: &mut Cx) {
        if self.rain_frames.is_empty() {
            return;
        }
        let interval = RAIN_FRAME_REAL_SECONDS / self.effective_weather_timelapse();
        let drift = (interval - self.rain_interval_current).abs()
            / self.rain_interval_current.max(1e-6);
        if drift > 0.15 {
            self.rain_interval_current = interval;
            cx.stop_timer(self.rain_timer);
            self.rain_timer = cx.start_interval(interval);
        }
    }

    fn wind_rand(&mut self) -> f64 {
        // xorshift — deterministic, no Instant/random dependencies.
        let mut x = self.wind_rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.wind_rng = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Advect the particle field one tick: constant SCREEN-space speed per
    /// m/s so the flow reads the same at every zoom.
    fn tick_wind(&mut self) {
        let Some((nx, ny, u, v, bbox)) = self.wind_field.clone() else {
            return;
        };
        let view_zoom = self.view_zoom();
        let world_px = TILE_SIZE * 2f64.powf(view_zoom);
        let rect = self.view_rect;
        let half_w = rect.size.x.max(64.0) * 0.7 / world_px;
        let half_h = rect.size.y.max(64.0) * 0.7 / world_px;
        let center = self.center_norm;
        let target = 2600usize;
        while self.wind_particles.len() < target {
            let px = center.x + (self.wind_rand() - 0.5) * 2.0 * half_w;
            let py = center.y + (self.wind_rand() - 0.5) * 2.0 * half_h;
            let age = (self.wind_rand() * 80.0) as u32;
            self.wind_particles
                .push(WindParticle::spawn(dvec2(px, py), age));
        }
        let sample = |pos: Vec2d| -> Option<(f32, f32)> {
            let (lon, lat) = normalized_to_lon_lat(pos);
            let (west, south, east, north) = bbox;
            let fx = (lon - west) / (east - west) * (nx - 1) as f64;
            let fy = (lat - south) / (north - south) * (ny - 1) as f64;
            if fx < 0.0 || fy < 0.0 || fx > (nx - 1) as f64 || fy > (ny - 1) as f64 {
                return None;
            }
            let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
            let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
            let (x1, y1) = ((x0 + 1).min(nx - 1), (y0 + 1).min(ny - 1));
            let at = |x: usize, y: usize| y * nx + x;
            let lerp2 = |g: &Vec<f32>| {
                let top = g[at(x0, y0)] * (1.0 - tx) + g[at(x1, y0)] * tx;
                let bottom = g[at(x0, y1)] * (1.0 - tx) + g[at(x1, y1)] * tx;
                top * (1.0 - ty) + bottom * ty
            };
            Some((lerp2(&u), lerp2(&v)))
        };
        // Advect in REAL map meters at the shared weather time-lapse — the
        // SAME effective clock the rain playback uses at this zoom, so the
        // two layers never drift apart.
        let (_, lat) = normalized_to_lon_lat(center);
        let meters_to_norm = 1.0 / (40_075_016.686 * lat.to_radians().cos());
        let dt = 1.0 / 30.0;
        let k = self.effective_weather_timelapse() * dt * meters_to_norm;
        let mut respawns: Vec<usize> = Vec::new();
        for i in 0..self.wind_particles.len() {
            let p = self.wind_particles[i].head();
            let Some((wu, wv)) = sample(p) else {
                respawns.push(i);
                continue;
            };
            let particle = &mut self.wind_particles[i];
            let next = dvec2(p.x + wu as f64 * k, p.y - wv as f64 * k);
            particle.push(next);
            particle.speed = (wu * wu + wv * wv).sqrt();
            particle.age += 1;
            let out_of_view = (next.x - center.x).abs() > half_w
                || (next.y - center.y).abs() > half_h;
            if particle.age > 260 || out_of_view {
                respawns.push(i);
            }
        }
        for i in respawns {
            let px = center.x + (self.wind_rand() - 0.5) * 2.0 * half_w;
            let py = center.y + (self.wind_rand() - 0.5) * 2.0 * half_h;
            self.wind_particles[i] = WindParticle::spawn(dvec2(px, py), 0);
        }
    }

    /// Install (or clear) the hillshade overlay for a normalized-mercator
    /// bbox (west, north, east, south).
    pub fn set_terrain_overlay(&mut self, cx: &mut Cx, data: TerrainOverlayData) {
        if data.texels.is_empty() {
            self.terrain_texture = None;
            self.terrain_elev_texture = None;
            self.terrain_elev = Vec::new();
            self.terrain_elev_size = (0, 0);
            self.terrain_elev_max = 0.0;
        } else {
            self.terrain_bbox = data.bbox;
            self.terrain_texture = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    data: Some(data.texels),
                    width: data.width,
                    height: data.height,
                    updated: TextureUpdated::Full,
                },
            ));
            if data.elev_texels.is_empty() {
                self.terrain_elev_texture = None;
                self.terrain_elev = Vec::new();
                self.terrain_elev_size = (0, 0);
            } else {
                self.terrain_elev_texture = Some(Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        data: Some(data.elev_texels),
                        width: data.elev_width,
                        height: data.elev_height,
                        updated: TextureUpdated::Full,
                    },
                ));
                self.terrain_elev_max =
                    data.elev.iter().fold(0.0f32, |acc, e| acc.max(*e));
                self.terrain_elev = data.elev;
                self.terrain_elev_size = (data.elev_width, data.elev_height);
            }
        }
        self.redraw(cx);
    }

    /// Ground elevation (m) at a normalized-mercator point, bilinear over
    /// the CPU copy of the displacement grid; 0 outside coverage.
    fn terrain_elevation_at(&self, norm: Vec2d) -> f64 {
        let (ew, eh) = self.terrain_elev_size;
        if ew == 0 || eh == 0 {
            return 0.0;
        }
        let (west, north, east, south) = self.terrain_bbox;
        let sx = (east - west).abs().max(1e-12);
        let sy = (south - north).abs().max(1e-12);
        let fx = ((norm.x - west) / sx).clamp(0.0, 1.0) * (ew - 1) as f64;
        let fy = ((norm.y - north) / sy).clamp(0.0, 1.0) * (eh - 1) as f64;
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (dx, dy) = (fx - x0 as f64, fy - y0 as f64);
        let x1 = (x0 + 1).min(ew - 1);
        let y1 = (y0 + 1).min(eh - 1);
        let at = |x: usize, y: usize| self.terrain_elev[y * ew + x] as f64;
        let top = at(x0, y0) * (1.0 - dx) + at(x1, y0) * dx;
        let bottom = at(x0, y1) * (1.0 - dx) + at(x1, y1) * dx;
        top * (1.0 - dy) + bottom * dy
    }

    /// Hillshade between the fills and the road network. Flat: one quad.
    /// Tilted with elevation data: a grid mesh whose corners lift by the
    /// ground elevation, so the terrain is a real displaced surface.
    fn draw_terrain_overlay(&mut self, cx: &mut Cx2d) {
        let Some(texture) = self.terrain_texture.clone() else {
            return;
        };
        let camera = self.overlay_camera();
        let (west, north, east, south) = self.terrain_bbox;
        self.draw_terrain.draw_super.draw_vars.set_texture(0, &texture);
        let tilted = self.tilt.clamp(0.0, TILT_HARD_MAX_DEG).to_radians() > 1e-4;
        self.draw_terrain
            .draw_super
            .draw_vars
            .set_uniform(cx.cx, live_id!(depth_on), &[if tilted { 1.0f32 } else { 0.0 }]);
        // Regional 3D: the surface IS the ground (fills stay flat under it).
        let ground_mode = tilted && self.render_bucket() < 14;
        self.draw_terrain.draw_super.draw_vars.set_uniform(
            cx.cx,
            live_id!(opacity_boost),
            &[if ground_mode { 1.7f32 } else { 1.0 }],
        );
        let slope = self.tilt_depth_slope;
        // Depth stays a function of the UNWARPED ground plane (the tile
        // shader computes depth from the unwarped ground_rel_y — with the
        // Inception fold on, un-compressing the warped screen y would
        // diverge from the tiles and the surface would win above the fold).
        let depth_of_rel = move |ground_rel: f64| -> f32 {
            (-24.0 + 0.02 + ground_rel * slope) as f32
        };
        let lift_px_per_m = self.terrain_lift_px_per_m();
        let displace = lift_px_per_m > 0.0 && !self.terrain_elev.is_empty();
        if !displace {
            self.draw_terrain.uv0 = Vec2f { x: 0.0, y: 0.0 };
            self.draw_terrain.uv1 = Vec2f { x: 1.0, y: 1.0 };
            let (c0, r0) = camera.norm_to_screen_with_rel(dvec2(west, north));
            let (c1, r1) = camera.norm_to_screen_with_rel(dvec2(east, north));
            let (c2, r2) = camera.norm_to_screen_with_rel(dvec2(east, south));
            let (c3, r3) = camera.norm_to_screen_with_rel(dvec2(west, south));
            self.draw_terrain.gdepth = Vec4f {
                x: depth_of_rel(r0),
                y: depth_of_rel(r1),
                z: depth_of_rel(r2),
                w: depth_of_rel(r3),
            };
            self.terrain_cell(cx, c0, c1, c2, c3);
            return;
        }
        // Grid resolution balances silhouette quality against per-frame CPU.
        const GX: usize = 288;
        const GY: usize = 216;
        let mut pts = Vec::with_capacity((GX + 1) * (GY + 1));
        let mut ground_depth = Vec::with_capacity((GX + 1) * (GY + 1));
        for gy in 0..=GY {
            let v = gy as f64 / GY as f64;
            let ny = north + (south - north) * v;
            for gx in 0..=GX {
                let u = gx as f64 / GX as f64;
                let nx = west + (east - west) * u;
                let npt = dvec2(nx, ny);
                let elev_m = self.terrain_elevation_at(npt);
                let (p, ground_rel) = if camera.warp.is_on() {
                    // Lift through the SAME camera: ground px along the
                    // fold's local normal, perspective divide included.
                    let h_px = elev_m * lift_px_per_m / camera.warp.sin_t.max(1e-6);
                    camera.norm_to_screen_lifted(npt, h_px)
                } else {
                    let (mut p, r) = camera.norm_to_screen_with_rel(npt);
                    p.y -= elev_m * lift_px_per_m;
                    (p, r)
                };
                ground_depth.push(depth_of_rel(ground_rel));
                pts.push(p);
            }
        }
        for gy in 0..GY {
            for gx in 0..GX {
                let i = gy * (GX + 1) + gx;
                self.draw_terrain.uv0 = Vec2f {
                    x: gx as f32 / GX as f32,
                    y: gy as f32 / GY as f32,
                };
                self.draw_terrain.uv1 = Vec2f {
                    x: (gx + 1) as f32 / GX as f32,
                    y: (gy + 1) as f32 / GY as f32,
                };
                self.draw_terrain.gdepth = Vec4f {
                    x: ground_depth[i],
                    y: ground_depth[i + 1],
                    z: ground_depth[i + GX + 2],
                    w: ground_depth[i + GX + 1],
                };
                self.terrain_cell(cx, pts[i], pts[i + 1], pts[i + GX + 2], pts[i + GX + 1]);
            }
        }
    }

    /// 1x1 zero-elevation texture so the shader's terrain slot is always
    /// bound even when the layer is off.
    fn terrain_fallback(&mut self, cx: &mut Cx2d) -> Texture {
        if self.terrain_fallback_texture.is_none() {
            self.terrain_fallback_texture = Some(Texture::new_with_format(
                cx.cx,
                TextureFormat::VecBGRAu8_32 {
                    data: Some(vec![0u32]),
                    width: 1,
                    height: 1,
                    updated: TextureUpdated::Full,
                },
            ));
        }
        self.terrain_fallback_texture.clone().unwrap()
    }

    /// Ground elevation lift (screen px) under a screen point; 0 when the
    /// terrain layer is off or the camera is flat.
    fn terrain_ground_lift_px_at_screen(&self, p: Vec2d) -> f64 {
        if self.terrain_elev.is_empty() {
            return 0.0;
        }
        let lift = self.terrain_lift_px_per_m();
        if lift <= 0.0 {
            return 0.0;
        }
        let (lon, lat) = self.screen_to_lon_lat(p);
        self.terrain_elevation_at(lon_lat_to_normalized(lon, lat)) * lift
    }

    /// Screen px of lift per meter of elevation (0 when flat / no 3D).
    fn terrain_lift_px_per_m(&self) -> f64 {
        let tilt_rad = self.tilt.clamp(0.0, TILT_HARD_MAX_DEG).to_radians();
        if tilt_rad <= 1e-4 {
            return 0.0;
        }
        let camera = self.overlay_camera();
        tilt_rad.sin() / camera.meters_per_px.max(1e-9)
    }

    fn terrain_cell(&mut self, cx: &mut Cx2d, c0: Vec2d, c1: Vec2d, c2: Vec2d, c3: Vec2d) {
        self.draw_terrain.c0 = Vec2f { x: c0.x as f32, y: c0.y as f32 };
        self.draw_terrain.c1 = Vec2f { x: c1.x as f32, y: c1.y as f32 };
        self.draw_terrain.c2 = Vec2f { x: c2.x as f32, y: c2.y as f32 };
        self.draw_terrain.c3 = Vec2f { x: c3.x as f32, y: c3.y as f32 };
        let min_x = c0.x.min(c1.x).min(c2.x).min(c3.x);
        let min_y = c0.y.min(c1.y).min(c2.y).min(c3.y);
        let max_x = c0.x.max(c1.x).max(c2.x).max(c3.x);
        let max_y = c0.y.max(c1.y).max(c2.y).max(c3.y);
        self.draw_terrain.draw_abs(
            cx,
            Rect {
                pos: dvec2(min_x, min_y),
                size: dvec2(max_x - min_x, max_y - min_y),
            },
        );
    }

    /// Install the rain nowcast animation frames (BGRA u32 texels) covering
    /// the given lon/lat bbox; empty = disable. Frames advance every 220 ms.
    pub fn set_rain_frames(
        &mut self,
        cx: &mut Cx,
        frames: Vec<Vec<u32>>,
        width: usize,
        height: usize,
        bbox: (f64, f64, f64, f64),
    ) {
        cx.stop_timer(self.rain_timer);
        self.rain_frames.clear();
        self.rain_frame_index = 0;
        self.rain_bbox = bbox;
        self.rain_tex_size = (width.max(1), height.max(1));
        for data in frames {
            self.rain_frames.push(Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    data: Some(data),
                    width,
                    height,
                    updated: TextureUpdated::Full,
                },
            ));
        }
        if !self.rain_frames.is_empty() {
            let interval = RAIN_FRAME_REAL_SECONDS / self.effective_weather_timelapse();
            self.rain_interval_current = interval;
            self.rain_timer = cx.start_interval(interval);
        } else {
            self.rain_now_hires = None;
        }
        self.redraw(cx);
    }

    /// Install (or clear) the hi-res dual-radar "now" image, drawn instead of
    /// animation frame 0. Covers the same bbox as the animation frames.
    pub fn set_rain_now_hires(
        &mut self,
        cx: &mut Cx,
        texels: Option<(Vec<u32>, usize, usize)>,
    ) {
        self.rain_now_hires = texels.map(|(data, width, height)| {
            (
                Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        data: Some(data),
                        width,
                        height,
                        updated: TextureUpdated::Full,
                    },
                ),
                (width.max(1), height.max(1)),
            )
        });
        self.redraw(cx);
    }

    pub fn set_map_zoom(&mut self, cx: &mut Cx, zoom: f64) {
        let min_zoom = self.min_zoom.max(0.0);
        let max_zoom = self.max_zoom.max(min_zoom);
        self.fly = None;
        self.zoom = zoom.clamp(min_zoom, max_zoom);
        self.last_zoom_change_frame = self.frame_counter;
        self.last_zoom_change_time = Some(cx.seconds_since_app_start());
        cx.stop_timer(self.zoom_settle_timer);
        self.zoom_settle_timer = cx.start_timeout(0.15);
        self.emit_viewport_changed(cx);
        self.redraw(cx);
    }

    /// Animated camera flight; far targets get a zoom-out-then-in arc so
    /// tiles stay loadable mid-flight and the motion reads like every
    /// mapping app.
    pub fn fly_to(&mut self, cx: &mut Cx, lon: f64, lat: f64, zoom: f64) {
        let min_zoom = self.min_zoom.max(0.0);
        let max_zoom = self.max_zoom.max(min_zoom);
        let to_zoom = zoom.clamp(min_zoom, max_zoom);
        let from_zoom = self.view_zoom();
        let to_center = lon_lat_to_normalized(lon, lat);
        let dist_px = (to_center - self.center_norm).length() * tile_world_size_zoom(from_zoom);
        let viewport = self.view_rect.size.length().max(400.0);
        let arc = if dist_px > viewport * 0.5 {
            ((dist_px / viewport).log2() * 0.9).clamp(0.4, 4.5)
        } else {
            0.0
        };
        let duration = (0.55 + 0.22 * arc + (dist_px / 6000.0).min(0.6)).min(2.4);
        self.fly = Some(FlyTo {
            started: cx.seconds_since_app_start(),
            duration,
            from_center: self.center_norm,
            to_center,
            from_zoom,
            to_zoom,
            arc,
        });
        cx.stop_timer(self.fly_timer);
        self.fly_timer = cx.start_timeout(0.016);
        self.redraw(cx);
    }

    fn tick_fly(&mut self, cx: &mut Cx) {
        let Some(fly) = self.fly else {
            return;
        };
        let min_zoom = self.min_zoom.max(0.0);
        let max_zoom = self.max_zoom.max(min_zoom);
        let now = cx.seconds_since_app_start();
        let t = ((now - fly.started).max(0.0) / fly.duration).clamp(0.0, 1.0);
        let e = t * t * (3.0 - 2.0 * t);
        self.center_norm = fly.from_center + (fly.to_center - fly.from_center) * e;
        let zoom = fly.from_zoom + (fly.to_zoom - fly.from_zoom) * e
            - fly.arc * (std::f64::consts::PI * e).sin();
        self.zoom = zoom.clamp(min_zoom, max_zoom);
        self.wrap_and_clamp_center();
        self.last_zoom_change_frame = self.frame_counter;
        self.last_zoom_change_time = Some(now);
        if t >= 1.0 {
            self.fly = None;
            self.center_norm = fly.to_center;
            self.zoom = fly.to_zoom;
            self.wrap_and_clamp_center();
            self.sync_camera_fields();
            self.emit_viewport_changed(cx);
            cx.stop_timer(self.zoom_settle_timer);
            self.zoom_settle_timer = cx.start_timeout(0.15);
        } else {
            self.fly_timer = cx.start_timeout(0.016);
        }
        self.redraw(cx);
    }

    // --- Overlay content ---

    pub fn set_markers(&mut self, cx: &mut Cx, markers: Vec<MapMarker>) {
        self.overlay.markers = markers;
        self.redraw(cx);
    }

    /// Route polyline as (lon, lat) pairs; resets travel progress.
    pub fn set_route(&mut self, cx: &mut Cx, points: &[(f64, f64)]) {
        self.overlay.route = Some(MapRouteOverlay {
            points_norm: points
                .iter()
                .map(|&(lon, lat)| lon_lat_to_normalized(lon, lat))
                .collect(),
            traveled_index: 0,
        });
        self.redraw(cx);
    }

    pub fn clear_route(&mut self, cx: &mut Cx) {
        self.overlay.route = None;
        self.redraw(cx);
    }

    /// Points before `index` draw dimmed (the already-driven part).
    pub fn set_route_progress(&mut self, cx: &mut Cx, index: usize) {
        if let Some(route) = &mut self.overlay.route {
            if route.traveled_index != index {
                route.traveled_index = index;
                self.redraw(cx);
            }
        }
    }

    pub fn set_puck(&mut self, cx: &mut Cx, puck: Option<MapPuck>) {
        self.overlay.puck = puck;
        self.redraw(cx);
    }
}

impl MapViewRef {
    pub fn tapped(&self, actions: &Actions) -> Option<(f64, f64)> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let MapViewAction::Tapped { lon, lat, .. } = item.cast() {
                return Some((lon, lat));
            }
        }
        None
    }

    pub fn long_pressed(&self, actions: &Actions) -> Option<(f64, f64)> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let MapViewAction::LongPressed { lon, lat, .. } = item.cast() {
                return Some((lon, lat));
            }
        }
        None
    }

    pub fn viewport_changed(&self, actions: &Actions) -> Option<(f64, f64, f64)> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let MapViewAction::ViewportChanged { lon, lat, zoom } = item.cast() {
                return Some((lon, lat, zoom));
            }
        }
        None
    }

    pub fn tilt_changed(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let MapViewAction::TiltChanged { tilt } = item.cast() {
                return Some(tilt);
            }
        }
        None
    }

    pub fn marker_clicked(&self, actions: &Actions) -> Option<u64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let MapViewAction::MarkerClicked { id } = item.cast() {
                return Some(id);
            }
        }
        None
    }

    pub fn center(&self) -> Option<(f64, f64)> {
        self.borrow().map(|inner| inner.center())
    }

    pub fn map_zoom(&self) -> Option<f64> {
        self.borrow().map(|inner| inner.map_zoom())
    }

    /// The active theme's compiled shiny.md config (sun + toggles).
    pub fn shiny(&self) -> Option<ShinyConfig> {
        self.borrow().map(|inner| *inner.shiny())
    }

    /// Runtime shiny.md config update; restyles with stale tiles drawable.
    pub fn update_shiny(&self, cx: &mut Cx, update: impl Fn(&mut MapShinyStyle)) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.update_shiny(cx, update);
        }
    }

    /// Switch theme (0 light, 1 dark, 2 circuit city), per-tile crossfade.
    pub fn set_theme(&self, cx: &mut Cx, theme: u32) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_theme(cx, theme);
        }
    }

    /// The Inception fold on/off (tweens; close-3D only — see MapView).
    pub fn set_space_warp(&self, cx: &mut Cx, on: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_space_warp(cx, on);
        }
    }

    pub fn space_warp(&self) -> bool {
        self.borrow().map(|inner| inner.space_warp()).unwrap_or(false)
    }

    pub fn space_warp_available(&self) -> bool {
        self.borrow()
            .map(|inner| inner.space_warp_available())
            .unwrap_or(false)
    }

    pub fn set_center(&self, cx: &mut Cx, lon: f64, lat: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_center(cx, lon, lat);
        }
    }

    pub fn set_map_zoom(&self, cx: &mut Cx, zoom: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_map_zoom(cx, zoom);
        }
    }

    pub fn pin_tapped(&self, actions: &Actions) -> Option<(f64, f64, Vec<(String, String)>)> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let MapViewAction::PinTapped { lon, lat, info } = item.cast() {
                return Some((lon, lat, info));
            }
        }
        None
    }

    pub fn set_overlay_paths(&self, cx: &mut Cx, paths: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_overlay_paths(cx, paths);
        }
    }

    pub fn set_source_paths(&self, cx: &mut Cx, base: &str, detail: &str, bridge_dz: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_source_paths(cx, base, detail, bridge_dz);
        }
    }

    pub fn set_source_config(&self, cx: &mut Cx, config: TileSourceConfig) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_source_config(cx, config);
        }
    }

    pub fn set_rain_frames(
        &self,
        cx: &mut Cx,
        frames: Vec<Vec<u32>>,
        width: usize,
        height: usize,
        bbox: (f64, f64, f64, f64),
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_rain_frames(cx, frames, width, height, bbox);
        }
    }

    pub fn set_rain_now_hires(&self, cx: &mut Cx, texels: Option<(Vec<u32>, usize, usize)>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_rain_now_hires(cx, texels);
        }
    }

    pub fn set_terrain_overlay(&self, cx: &mut Cx, data: TerrainOverlayData) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_terrain_overlay(cx, data);
        }
    }

    pub fn set_wind_field(
        &self,
        cx: &mut Cx,
        nx: usize,
        ny: usize,
        u: Vec<f32>,
        v: Vec<f32>,
        bbox: (f64, f64, f64, f64),
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_wind_field(cx, nx, ny, u, v, bbox);
        }
    }

    pub fn set_rotation(&self, cx: &mut Cx, rotation_deg: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_rotation(cx, rotation_deg);
        }
    }

    pub fn rotation(&self) -> f64 {
        self.borrow().map(|inner| inner.rotation()).unwrap_or(0.0)
    }

    pub fn set_tilt(&self, cx: &mut Cx, tilt_deg: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_tilt(cx, tilt_deg);
        }
    }
    pub fn tilt(&self) -> f64 {
        self.borrow().map(|inner| inner.tilt()).unwrap_or(0.0)
    }

    pub fn fly_to(&self, cx: &mut Cx, lon: f64, lat: f64, zoom: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.fly_to(cx, lon, lat, zoom);
        }
    }

    pub fn set_markers(&self, cx: &mut Cx, markers: Vec<MapMarker>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_markers(cx, markers);
        }
    }

    pub fn set_route(&self, cx: &mut Cx, points: &[(f64, f64)]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_route(cx, points);
        }
    }

    pub fn clear_route(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_route(cx);
        }
    }

    pub fn set_route_progress(&self, cx: &mut Cx, index: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_route_progress(cx, index);
        }
    }

    pub fn set_puck(&self, cx: &mut Cx, puck: Option<MapPuck>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_puck(cx, puck);
        }
    }
}

/// Viewport-independent identity for a label, used for frame-to-frame
/// placement hysteresis (road_kind embeds the tile-local position for points).
fn stable_label_key(name_key: &str, road_kind: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name_key.hash(&mut hasher);
    road_kind.hash(&mut hasher);
    hasher.finish()
}

/// Carto-style label colors per POI class (orange food, purple shops,
/// brown culture, muted house numbers).
fn label_class_color(color_class: u8, default_color: Vec4f, dark_theme: bool) -> Vec4f {
    match (color_class, dark_theme) {
        (LABEL_CLASS_AMENITY, false) => Vec4f::from_u32(0xc77400ff),
        (LABEL_CLASS_SHOP, false) => Vec4f::from_u32(0xac39acff),
        (LABEL_CLASS_CULTURE, false) => Vec4f::from_u32(0x734a08ff),
        (LABEL_CLASS_MUTED, false) => Vec4f::from_u32(0x66768dff),
        (LABEL_CLASS_HEALTH, false) => Vec4f::from_u32(0xbf0000ff),
        (LABEL_CLASS_GREEN, false) => Vec4f::from_u32(0x267d3fff),
        (LABEL_CLASS_AMENITY, true) => Vec4f::from_u32(0xe09a4aff),
        (LABEL_CLASS_SHOP, true) => Vec4f::from_u32(0xcf7fcfff),
        (LABEL_CLASS_CULTURE, true) => Vec4f::from_u32(0xc9a36cff),
        (LABEL_CLASS_MUTED, true) => Vec4f::from_u32(0x8899aaff),
        (LABEL_CLASS_HEALTH, true) => Vec4f::from_u32(0xe06666ff),
        (LABEL_CLASS_GREEN, true) => Vec4f::from_u32(0x7fc98fff),
        (LABEL_CLASS_WATER, false) => Vec4f::from_u32(0x39688fff),
        (LABEL_CLASS_WATER, true) => Vec4f::from_u32(0x7fb2d9ff),
        (LABEL_CLASS_PIN, _) => Vec4f::from_u32(0xffffffff),
        (LABEL_CLASS_EXIT, false) => Vec4f::from_u32(0x960000ff),
        // Bright rose: exit labels sit ON the amber motorway ribbon in the
        // dark themes — salmon (0xe07070) vanished against it.
        (LABEL_CLASS_EXIT, true) => Vec4f::from_u32(0xffc4c4ff),
        (LABEL_CLASS_ADMIN, false) => Vec4f::from_u32(0x6a5b8eff),
        (LABEL_CLASS_ADMIN, true) => Vec4f::from_u32(0xb3a5d6ff),
        _ => default_color,
    }
}

#[cfg(test)]
// Native map tests use wall-clock values only to make temporary paths unique.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
mod tests {
    use super::*;
    use makepad_mbtile_reader::MbtilesWriter;

    fn test_map(cx: &mut Cx) -> MapView {
        cx.init_cx_os();
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            MapView::script_new_with_default(vm)
        })
    }

    fn temp_mbtiles_with_zoom(
        name: &str,
        min_zoom: &str,
        max_zoom: &str,
        tile_zoom: u8,
    ) -> std::path::PathBuf {
        let nonce = Cx::time_now().to_bits();
        let path = std::path::PathBuf::from(format!("target/{name}-{nonce}.mbtiles"));
        std::fs::create_dir_all("target").unwrap();
        let mut writer = MbtilesWriter::create(&path).unwrap();
        writer.set_metadata("minzoom", min_zoom);
        writer.set_metadata("maxzoom", max_zoom);
        writer
            .write_tile_encoded(tile_zoom, 0, 0, &[])
            .unwrap();
        writer.finish().unwrap();
        path
    }

    fn temp_mbtiles(name: &str) -> std::path::PathBuf {
        temp_mbtiles_with_zoom(name, "0", "0", 0)
    }

    #[test]
    fn archive_priority_is_centre_out_in_tilted_screen_space() {
        let mut tiles = vec![
            TileKey { z: 2, x: 2, y: 1 },
            TileKey { z: 2, x: 1, y: 2 },
            TileKey { z: 2, x: 1, y: 1 },
        ];
        sort_tiles_center_out(&mut tiles, 2, dvec2(0.375, 0.375), (1.0, 0.0), 0.25);
        assert_eq!(
            tiles,
            [
                TileKey { z: 2, x: 1, y: 1 },
                TileKey { z: 2, x: 1, y: 2 },
                TileKey { z: 2, x: 2, y: 1 },
            ]
        );
    }

    #[test]
    fn tile_span_keeps_exactly_one_prefetch_tile_for_partial_edge_tiles() {
        let (min, max) =
            tile_span_with_prefetch(TILE_SIZE * 0.25, TILE_SIZE * 1.75);
        assert_eq!((min, max), (-1, 2));
    }

    #[test]
    fn tile_span_keeps_one_prefetch_tile_at_exact_boundaries() {
        let (min, max) = tile_span_with_prefetch(TILE_SIZE, TILE_SIZE * 2.0);
        assert_eq!((min, max), (0, 2));
    }

    #[test]
    fn source_shape_recognizes_archive_formats() {
        assert!(!is_mkmap_path_shape("local/maps/example-base.mbtiles"));
        assert!(is_mkmap_path_shape("local/maps/world.mkmap"));
        assert!(is_mkmap_path_shape("local/maps/world/root.mkidx"));
    }

    #[test]
    fn malformed_zoom_state_cannot_invert_request_clamping() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut map = test_map(&mut cx);
        map.use_local_mbtiles = true;
        map.zoom = 17.0;
        map.local_source_zoom_range = Some((20, 3));
        assert_eq!(map.request_zoom_level(), LOCAL_MBTILES_MAX_ZOOM);
        map.local_source_zoom_range = Some((0, 31));
        assert_eq!(map.request_zoom_level(), LOCAL_MBTILES_MAX_ZOOM);
    }

    #[test]
    fn archive_watcher_rejects_malformed_mbtiles_zoom_metadata() {
        let path = temp_mbtiles_with_zoom("map-view-invalid-watch-zoom", "20", "3", 0);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut map = test_map(&mut cx);
        map.set_source_config(
            &mut cx,
            TileSourceConfig::LocalArchive {
                mbtiles_path: path.to_string_lossy().into_owned(),
                detail_mbtiles_path: String::new(),
                overlay_mbtiles_paths: String::new(),
                bridge_dz_path: String::new(),
            },
        );
        map.local_source_zoom_range = Some((4, 6));
        map.local_source_zoom_range_checked = true;
        map.archive_watch_mtime = archive_mtime(&path);

        <MapView as Widget>::handle_event(
            &mut map,
            &mut cx,
            &Event::Startup,
            &mut Scope::empty(),
        );
        assert!(map.archive_watch_in_flight);
        for _ in 0..2_000 {
            <MapView as Widget>::handle_event(
                &mut map,
                &mut cx,
                &Event::Startup,
                &mut Scope::empty(),
            );
            if !map.archive_watch_in_flight {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!map.archive_watch_in_flight);
        assert_eq!(map.local_source_zoom_range, Some((4, 6)));
        map.zoom = 5.0;
        assert!((4..=6).contains(&map.request_zoom_level()));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn legacy_mbtiles_installed_on_map_view_dispatches_legacy_worker() {
        let path = temp_mbtiles("map-view-legacy-dispatch");
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut map = test_map(&mut cx);
        map.zoom = 0.0;
        map.set_source_config(
            &mut cx,
            TileSourceConfig::LocalArchive {
                mbtiles_path: path.to_string_lossy().into_owned(),
                detail_mbtiles_path: String::new(),
                overlay_mbtiles_paths: String::new(),
                bridge_dz_path: String::new(),
            },
        );
        map.local_source_zoom_range = Some((0, 0));
        let key = TileKey { z: 0, x: 0, y: 0 };
        map.visible_tiles = vec![key];
        map.request_visible_tiles_from_local_source(&mut cx);
        assert!(map.base_archive.is_none());
        assert!(map.archive_pending_tiles.is_empty());
        let mut message = None;
        for _ in 0..2_000 {
            if let Ok(received) = map.tile_worker_rx.try_recv() {
                message = Some(received);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(matches!(
            message,
            Some(TileWorkerMessage::LocalBatchLoaded { loaded, .. }) if loaded.len() == 1
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn map_view_source_switch_reuses_archive_pool() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut map = test_map(&mut cx);
        map.set_source_config(&mut cx, TileSourceConfig::http_archive("https://one.invalid/map"));
        let first = map.archive_worker_pool.as_ref().unwrap().clone();
        map.set_source_config(&mut cx, TileSourceConfig::http_archive("https://two.invalid/map"));
        let second = map.archive_worker_pool.as_ref().unwrap();
        assert!(first.ptr_eq(second));
    }

    #[test]
    fn detail_equal_to_base_issues_one_archive_request() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut map = test_map(&mut cx);
        map.zoom = 14.0;
        map.set_source_config(
            &mut cx,
            TileSourceConfig::http_archive("https://tiles.invalid/world.mkmap"),
        );
        let key = TileKey { z: 14, x: 0, y: 0 };
        map.visible_tiles = vec![key];
        map.request_visible_tiles_from_local_source(&mut cx);
        assert!(map.detail_archive.is_none());
        assert_eq!(map.base_archive.as_ref().unwrap().source_request_count(), 1);
        assert!(map.archive_pending_tiles[&key].reuse_base_as_detail);
    }

    #[test]
    fn same_zoom_pan_prunes_queued_build_and_loading_placeholder() {
        let path = temp_mbtiles_with_zoom("map-view-same-zoom-prune", "3", "3", 3);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut map = test_map(&mut cx);
        map.zoom = 3.0;
        map.set_source_config(
            &mut cx,
            TileSourceConfig::LocalArchive {
                mbtiles_path: path.to_string_lossy().into_owned(),
                detail_mbtiles_path: String::new(),
                overlay_mbtiles_paths: String::new(),
                bridge_dz_path: String::new(),
            },
        );
        map.local_source_zoom_range = Some((3, 3));
        map.ensure_tile_thread_pool(&mut cx);
        let thread_count = cx.thread_spawner().worker_count(2, 8).get();
        let reached = Arc::new(std::sync::Barrier::new(thread_count + 1));
        let release = Arc::new(std::sync::Barrier::new(thread_count + 1));
        for index in 0..thread_count {
            let reached = reached.clone();
            let release = release.clone();
            map.tile_thread_pool
                .as_ref()
                .unwrap()
                .submit_tagged(
                    TileKey { z: 30, x: index as i32, y: 0 },
                    true,
                    QueueOrder::Lifo,
                    move || {
                    reached.wait();
                    release.wait();
                },
                )
                .unwrap()
                .detach();
        }
        reached.wait();
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(64.0, 64.0),
        };
        map.set_center(&mut cx, -90.0, 0.0);
        map.ensure_visible_tiles(&mut cx, rect);
        let requested_before = map
            .local_requested_tiles
            .keys()
            .copied()
            .collect::<HashSet<_>>();
        assert!(!requested_before.is_empty());

        map.set_center(&mut cx, 0.0, 0.0);
        map.ensure_visible_tiles(&mut cx, rect);
        let stale = requested_before
            .into_iter()
            .filter(|key| !map.visible_tiles.contains(key))
            .collect::<Vec<_>>();
        assert!(!stale.is_empty());
        for key in stale {
            assert!(!map.local_requested_tiles.contains_key(&key));
            assert!(!map.tiles.contains_key(&key));
        }
        assert!(map
            .local_requested_tiles
            .keys()
            .any(|key| map.visible_tiles.contains(key)));

        map.visible_tiles.clear();
        map.request_visible_tiles_from_local_source(&mut cx);
        assert!(map.local_requested_tiles.is_empty());
        release.wait();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn http_archive_waiting_for_dispatch_has_no_enqueue_watchdog() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut map = test_map(&mut cx);
        map.zoom = 1.0;
        map.set_source_config(
            &mut cx,
            TileSourceConfig::http_archive("https://never.invalid/world.mkmap"),
        );
        let key = TileKey { z: 1, x: 0, y: 0 };
        map.visible_tiles = vec![key];
        map.request_visible_tiles_from_local_source(&mut cx);
        assert_eq!(map.base_archive.as_ref().unwrap().waiter_count(), 1);
        assert!(map.archive_request_watchdog_handle.is_none());
        assert!(map.local_requested_tiles.contains_key(&key));
        assert!(map.archive_pending_tiles.contains_key(&key));
    }
}
