//! Image-based lighting for Splash materials (KERNELS.md §3.3.3). Opt-in:
//! `World::environment.ibl` names an environment; the renderer prepares it
//! off the UI thread (hdri/prepare.rs: render-material's lane texture, a
//! meta row with SH9, then the prefiltered atlas; a full-resolution dome;
//! and the numbers the sun rig, the exposure and the fog take from the
//! map) and binds the lane texture on the detail slot of
//! the materials compiled with IBL once it lands. Until then, and without
//! an environment, nothing is bound and every stock lane keeps its
//! analytic sky reflection. `Background::Environment` draws the dome as the
//! sky (`DrawEnvBackground`); once it is up the frame skips its analytic
//! dome.
//!
//! Preparations land at the renderer's pace (K1): one job at a time, and a
//! job runs to its end even when the world declares another key meanwhile
//! (a running clock re-declares its sun every quarter hour, every 7.5
//! minutes while the sun is low, or every frame); its result is adopted
//! when it lands, newer than what is bound,
//! and then the latest key prepares. So light and sky are at most one
//! preparation behind the world and always come from the same preparation.
//! A host that re-bakes the map it registered (`register_rebaked_environment`:
//! the sandbox's `params:` sky bakes again every quarter hour of game time,
//! every 7.5 minutes while the sun is low; every 0.3 s on a 30 s day, half
//! that then) is the map's form of a key that moves (K7):
//! the job preparing an earlier bake runs on and lands, then the newest
//! bake prepares. Only a change of map cancels the job in flight (another
//! map registered under its handle, another environment named, an
//! unregister): the job polls a token between its stages and stops, and
//! whatever it still returns is rejected when it is taken.
//!
//! A fork of the renderer (a split-screen pane, the AI world review) draws
//! the live renderer's environment (I4): it mirrors the maps and the bound
//! preparation (`Renderer::mirror_environment_from`) and resolves nothing.
use super::*;
use crate::hdri::envmap::GeneratedEnvMap;
use crate::hdri::prepare::{prepare_ibl_until, KeyRemoval, PrepareSizes, PreparedIbl, ATLAS_WIDTH, DOME_MAX_WIDTH, LIGHTING_MAX_WIDTH};
use makepad_draw::makepad_platform::thread::{CancellationToken, Lane, SubmitError, TaskHandle, TaskPool};
use makepad_render_material::ibl::{EnvMap, EnvPreset};
use makepad_scene::{Background, EnvSun, Environment, IblSource, TextureRef};
use std::collections::HashMap;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.geom

    // The environment as the background (`Background::Environment`): a
    // fullscreen quad drawn first in the scene pass. Sharp (blur 0, dome
    // up): the full-resolution map in `env_tex`, turned like the IBL and at
    // its intensity. Blurred: the prefiltered atlas in `detail_map` through
    // the IBL material's own lookups (render-material builtin.rs). Both
    // show the map at its own scale x Ibl.intensity (x the background's
    // intensity), exactly what the IBL reflections see: HDR_SKY_GAIN is
    // the analytic sky's and never applies here. HDR lane: that linear
    // radiance, the composite tone maps. Legacy lane: the analytic dome's
    // tone map (sky_dome.rs), metered on the luminance the dome shows, and a
    // hash dither against banding, as the analytic dome has.
    mod.draw.DrawEnvBackground = mod.std.set_type_default() do #(DrawEnvBackground::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        detail_map: texture_2d(float)
        env_tex: texture_2d(float)
        v_ndc: varying(vec2f)
        // render-material's lookups, as the base assigns them. Since plan 1a
        // task A7 the meta row is row 0 and the atlas rows start at 1, both
        // addressed absolutely (normalised by size() only at the sample), so
        // spare rows below the data could not hide them; no backend gives a
        // float texture spare rows now (the HAL allocates every texture at
        // exactly its size). Nothing here overrides them.
        mat_ibl_meta: mod.draw.mat_ibl_meta
        mat_ibl_dir: mod.draw.mat_ibl_dir
        mat_ibl_level: mod.draw.mat_ibl_level
        mat_ibl_sky_env: mod.draw.mat_ibl_sky_env
        // The dome, bilinear by hand (float textures are unfiltered): u
        // wraps, v clamps, texel centres. The texel coordinates are in the
        // LOGICAL size (bg2.zw) and size() only converts a texel index to
        // the sampler's coordinates (never wraps, clamps or scales by it),
        // so the lookup reads the map's texels whatever the allocation (an
        // exact one on every backend under the HAL). The test
        // `the_dome_lookup_never_reads_the_padding_of_a_float_allocation`
        // runs this function's transcription over a NaN-padded allocation.
        env_texel: fn(uv: vec2) -> vec3 {
            let size = self.env_tex.size()
            let w = self.bg2.z
            let h = self.bg2.w
            let px = uv.x * w - 0.5
            let py = clamp(uv.y * h - 0.5, 0.0, h - 1.0)
            let x0 = floor(px)
            let y0 = floor(py)
            let fx = px - x0
            let fy = py - y0
            let xa = (x0 - floor(x0 / w) * w + 0.5) / size.x
            let xb = (x0 + 1.0 - floor((x0 + 1.0) / w) * w + 0.5) / size.x
            let ya = (y0 + 0.5) / size.y
            let yb = (min(y0 + 1.0, h - 1.0) + 0.5) / size.y
            let a = mix(self.env_tex.sample_nearest(vec2(xa, ya)).xyz, self.env_tex.sample_nearest(vec2(xb, ya)).xyz, fx)
            let b = mix(self.env_tex.sample_nearest(vec2(xa, yb)).xyz, self.env_tex.sample_nearest(vec2(xb, yb)).xyz, fx)
            return mix(a, b, fy)
        }
        vertex: fn() {
            let p = self.geom.pos * 2.0 - vec2(1.0, 1.0)
            self.v_ndc = p
            self.vertex_pos = vec4(p.x, p.y, 0.9999, 1.0)
        }
        pixel: fn() {
            // The view ray through this pixel, in world space.
            let v = vec4(self.v_ndc.x * self.bg.z, self.v_ndc.y * self.bg.w, -1.0, 0.0)
            let d = normalize((self.draw_pass.camera_inv * v).xyz)
            // Sharp with the dome up: the map itself, turned by the IBL's
            // rotation (meta texel 9) and at its intensity; else the atlas
            // at the blur (which carries the intensity already).
            let sharp = step(self.bg.x, 0.0) * step(0.5, self.bg2.z)
            var c = vec3(0.0, 0.0, 0.0)
            if sharp > 0.5 {
                let r = self.mat_ibl_dir(d)
                let uv = vec2(fract(0.5 + atan2(r.x, 0.0 - r.z) / 6.2831853), acos(clamp(r.y, -1.0, 1.0)) / 3.14159265)
                c = self.env_texel(uv) * self.mat_ibl_meta(9.0).z
            } else {
                c = self.mat_ibl_sky_env(d, self.bg.x)
            }
            c = max(c * self.bg.y, vec3(0.0, 0.0, 0.0))
            // Legacy lane (bg2.y = 0): Reinhard on the luminance at the
            // metered exposure, normalised, gamma 1/2.2 — the analytic
            // dome's own steps. HDR lane (bg2.y = 1): the linear radiance
            // as it is (no gain: the environment's own scale).
            let hdr = step(0.5, self.bg2.y)
            let lum = dot(c, vec3(0.2126, 0.7152, 0.0722))
            let yt = max(lum * self.bg2.x, 0.0)
            let ldr0 = c * ((yt / (1.0 + yt)) / max(lum, 0.000001))
            let m = max(max(ldr0.x, ldr0.y), max(ldr0.z, 1.0))
            let ldr = pow(ldr0 * (1.0 / m), vec3(0.4545454, 0.4545454, 0.4545454))
            let out = mix(ldr, c, hdr)
            // The dither is the legacy lane's alone: its 8-bit output bands.
            // The HDR lane writes linear radiance, which the composite
            // dithers after its tone map; a fixed +-0.004 there would swamp
            // a dim map.
            let hash = fract(sin(dot(d.xy + d.zz, vec2(12.9898, 78.233))) * 43758.5453)
            let dither = (hash - 0.5) * 0.008 * (1.0 - hdr)
            return vec4(out + vec3(1.0, 1.0, 1.0) * dither, 1.0)
        }
        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// The environment background's draw.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawEnvBackground {
    #[deref]
    pub draw_vars: DrawVars,
    /// x = blur (0..1), y = intensity over the IBL's, zw = the projection's
    /// inverse x and y scales (view ray from the NDC).
    #[live(vec4(0.0, 1.0, 1.0, 1.0))]
    pub bg: Vec4f,
    /// x = the legacy lane's exposure (metered on the luminance the dome
    /// shows, the map's mean x Ibl.intensity, x 2^exposure_ev; 0 in the
    /// HDR lane), y = the lane (1 = HDR: linear out at the environment's
    /// own scale; 0 = legacy: the shader tone maps), zw = the dome
    /// texture's LOGICAL size (0 = no dome yet: the atlas draws at every
    /// blur).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub bg2: Vec4f,
}

/// How a draw takes part in the renderer's environment (`draw_scene_full`,
/// `draw_scene_aux`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EnvScope {
    /// The scene's own draw: its world's environment is what the renderer
    /// prepares, binds and drops.
    Scene,
    /// A draw of the same frame that is not the scene's (a contact or id
    /// map): its world's environment is not looked at, so a world that
    /// names none cannot drop what the scene's draws prepared.
    Aux,
}

/// What a preparation is for: the source, the sun its lighting copy is
/// made without, and the map's own sun the world reports for the daylight
/// switches (`Environment.daylight_sun`: it changes nothing in the
/// preparation, but it is the map's, so it binds with it and the switches
/// follow the sky that is drawn). Intensity and rotation are not here: they
/// are meta texels the shader reads, rewritten in place when they change.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PrepareKey {
    source: IblSource,
    sun: Option<EnvSun>,
    daylight_sun: Option<Vec3f>,
}

/// What the Heavy job hands back.
struct IblJobResult {
    generation: u64,
    key: PrepareKey,
    /// The intensity and rotation the meta row was packed with.
    intensity: f32,
    rotation_deg: f32,
    prepared: PreparedIbl,
    /// A procedural hdri preset's own sun (a registered map has none here;
    /// its sun is the world's `Environment.sun`).
    baked_sun: Option<EnvSun>,
    /// ... and its sky's own sun for the daylight switches (`hdri::Env::sun_dir`).
    baked_daylight: Option<Vec3f>,
}

/// A host-registered environment: the map the dome shows and, for a
/// generated map, the same map without exactly its key's light, which the
/// lighting copy is made from while the world declares a key (N3; without
/// it the declared key's covering cone is filled, as for a file's sun).
#[derive(Clone)]
struct RegisteredMap {
    map: Arc<EnvMap>,
    keyless: Option<Arc<EnvMap>>,
    /// Which registration this map continues: a change of map
    /// (`register_generated_environment`) starts a new one, a re-bake
    /// (`register_rebaked_environment`) continues the one registered under
    /// the handle. A job's result is adopted while its map's registration
    /// is still the handle's (`Renderer::job_is_current`).
    bakes: u64,
}

impl RegisteredMap {
    /// The same allocations: registering them again is no change.
    fn same(&self, other: &RegisteredMap) -> bool {
        let keyless = match (&self.keyless, &other.keyless) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        Arc::ptr_eq(&self.map, &other.map) && keyless
    }
}

/// The Heavy job in flight. Kept until it ends, also once cancelled, so
/// no second job ever runs beside it.
struct IblJob {
    handle: TaskHandle<Option<IblJobResult>>,
    /// What it prepares.
    key: PrepareKey,
    /// The registered map it prepares (`None` for a procedural index): its
    /// result is adopted only while this map, or a later bake of it, is the
    /// map registered under the handle (`Renderer::job_is_current`).
    /// Holding it also keeps the allocation from being reused by a new map
    /// while the job runs.
    map: Option<RegisteredMap>,
    /// Set on a change of map; the job polls it between its stages.
    cancel: CancellationToken,
}

/// How a finished job stands to what the world names now
/// (`Renderer::job_is_current`).
struct JobStanding {
    /// Its result may be adopted: the source is named, and the job prepared
    /// the map registered under it or an earlier bake of that map.
    current: bool,
    /// It prepared the map registered now (not an earlier bake).
    registered: bool,
}

#[derive(Default)]
pub(super) struct IblState {
    /// Host-registered HDR environments, by the handle documents name.
    maps: HashMap<TextureRef, RegisteredMap>,
    /// The last registration handed out (`RegisteredMap::bakes`).
    registrations: u64,
    /// What `texture`'s meta row holds: (source, intensity bits, rotation bits).
    key: Option<(IblSource, u32, u32)>,
    /// The lane texture (meta row 0, then the atlas: plan 1a task A7) and
    /// the full-resolution dome, with the dome's LOGICAL size (the shader
    /// addresses texels in it, never in the allocation's `size()`).
    texture: Option<Texture>,
    dome: Option<Texture>,
    dome_size: (usize, usize),
    /// What the world asked for on the last `resolve_ibl` (None: no
    /// environment, one that cannot be built, or one unregistered since).
    /// What `environment_pending` compares the prepared key and the failed
    /// key with.
    wanted: Option<PrepareKey>,
    /// What the textures and numbers below were prepared for, while they
    /// are of the map registered now: a re-registration clears it (the new
    /// map then prepares while these stay bound), and an earlier bake that
    /// lands after a re-bake leaves it clear (the newest bake prepares next).
    prepared_for: Option<PrepareKey>,
    /// The bound preparation's key and generation (`None`: nothing bound).
    /// A result is adopted only when it is newer than this one.
    bound: Option<(PrepareKey, u64)>,
    /// The bound lighting copy's numbers. `sh` is `Some` exactly while a
    /// preparation is bound (set in `adopt_ibl`, cleared in `unbind_ibl`);
    /// it outlives `prepared_for`, which a re-registration clears.
    sh: Option<[[f32; 3]; 9]>,
    mean_luminance: f32,
    horizon_rgb: Vec3f,
    /// The sun the bound lighting copy was made without: the declared one,
    /// else a procedural hdri preset's baked one.
    filled_sun: Option<EnvSun>,
    /// The bound map's own sun for the daylight switches: the one the world
    /// reported, else a procedural hdri preset's.
    daylight_sun: Option<Vec3f>,
    /// The job in flight, and the last generation submitted.
    job: Option<IblJob>,
    generation: u64,
    /// A key whose job failed (a worker panic) is not retried every frame.
    failed_for: Option<PrepareKey>,
    queue_full_logged: bool,
    background: Option<Box<DrawEnvBackground>>,
    /// This renderer draws another renderer's environment
    /// (`Renderer::mirror_environment_from`): it resolves nothing, so it
    /// never prepares, binds or drops one of its own.
    mirror: bool,
}

impl IblState {
    /// Take `live`'s registered maps and bound preparation, exactly as they
    /// are: the same `Arc`s and texture handles (nothing is copied but
    /// pointers and numbers), the numbers, the key and the daylight report
    /// bound with it. Not its job, its wishes or its failures: a mirror
    /// prepares nothing, so it awaits nothing. The destructuring names
    /// every field, so a new one has to be placed on one side or the other.
    fn mirror_from(&mut self, live: &IblState) {
        let IblState {
            maps,
            key,
            texture,
            dome,
            dome_size,
            prepared_for,
            bound,
            sh,
            mean_luminance,
            horizon_rgb,
            filled_sun,
            daylight_sun,
            registrations: _,
            wanted: _,
            job: _,
            generation: _,
            failed_for: _,
            queue_full_logged: _,
            background: _,
            mirror: _,
        } = live;
        self.maps = maps.clone();
        self.key = *key;
        self.texture = texture.clone();
        self.dome = dome.clone();
        self.dome_size = *dome_size;
        self.prepared_for = *prepared_for;
        self.bound = *bound;
        self.sh = *sh;
        self.mean_luminance = *mean_luminance;
        self.horizon_rgb = *horizon_rgb;
        self.filled_sun = *filled_sun;
        self.daylight_sun = *daylight_sun;
        self.mirror = true;
    }
}

/// The built-in environments `IblSource::Procedural` indexes, in order:
/// render-material's seven looks, then the HDRI generator's presets
/// (`hdri::presets::PRESET_NAMES`) as `hdri_<name>`, lowercase with `_` for
/// spaces and dashes (see `hdri_procedural_name`). Indices are stable, so
/// `material_lab` keeps working; Scene3D's `@names` are these names (its
/// `@hdri_golden_hour` is the generator's preset baked at
/// `PROCEDURAL_HDRI_WIDTH`, as here, and registered whole: Scene3D lights
/// with the key left in the map, its own lights replacing the rig's).
pub const PROCEDURAL_ENVIRONMENTS: &[&str] = &[
    "studio", "softbox", "sunset", "overcast", "night", "neon", "gradient",
    "hdri_clear_noon", "hdri_golden_hour", "hdri_sunset", "hdri_overcast", "hdri_blue_hour", "hdri_moonlit_night", "hdri_starry_night",
    "hdri_three_point", "hdri_top_softbox", "hdri_rim_pair", "hdri_overcast_dome", "hdri_ring_light",
];
/// The prefix of the hdri presets in `PROCEDURAL_ENVIRONMENTS`.
pub const HDRI_PREFIX: &str = "hdri_";
/// A procedural hdri preset bakes at this width (its dome and atlas source).
pub const PROCEDURAL_HDRI_WIDTH: usize = 1024;
/// The synchronous fallback (a closed task pool: wasm without atomics)
/// prepares small so it cannot stall a frame for long.
const SYNC_DOME_WIDTH: usize = 512;
const SYNC_ATLAS_WIDTH: usize = 64;
const SYNC_HDRI_WIDTH: usize = 256;
const SYNC_SIZES: PrepareSizes = PrepareSizes { dome: SYNC_DOME_WIDTH, atlas: SYNC_ATLAS_WIDTH, lighting: SYNC_DOME_WIDTH };
/// What a job prepares at: the engine's sizes.
const JOB_SIZES: PrepareSizes = PrepareSizes { dome: DOME_MAX_WIDTH, atlas: ATLAS_WIDTH, lighting: LIGHTING_MAX_WIDTH };

/// Runs `f(i)` for every `i in 0..n`, however it likes: `fan_out` on the
/// pool inside a job, a plain loop in the synchronous fallback. A bake takes
/// it to spread its rows.
type RowRunner<'a> = &'a dyn Fn(usize, &(dyn Fn(usize) + Sync));

/// A row runner over `run` that runs no more rows once `cancel` is set: a
/// cancelled job's bake ends within a row (`build_ibl` then stops).
fn rows_until_cancelled<'a>(run: RowRunner<'a>, cancel: &'a CancellationToken) -> impl Fn(usize, &(dyn Fn(usize) + Sync)) + 'a {
    move |n, f| {
        run(n, &|i| {
            if !cancel.is_cancelled() {
                f(i)
            }
        })
    }
}

/// The closed-pool path prepares on the UI thread: the source (and a
/// generated map's keyless copy) is shrunk to the small dome's width before
/// anything else reads it, so the sanity scan, the dome, the lighting copy,
/// its key removal, the meter and the band all run on the small map. A
/// malformed map (fewer texels than its size says) is left to the
/// preparation's own sanity check: `resized` would index past its data.
fn shrunk_for_sync(map: Arc<EnvMap>) -> Arc<EnvMap> {
    let well_formed = map.height >= 1 && map.data.len() >= map.width * map.height;
    if well_formed && map.width > SYNC_DOME_WIDTH {
        Arc::new(map.resized(SYNC_DOME_WIDTH))
    } else {
        map
    }
}

/// The name `IblSource::Procedural(i)` stands for.
pub fn procedural_environment_name(i: u32) -> Option<&'static str> {
    PROCEDURAL_ENVIRONMENTS.get(i as usize).copied()
}

/// An hdri preset name ("Three-point") as it appears in
/// `PROCEDURAL_ENVIRONMENTS` ("hdri_three_point"). `hdri::presets::preset`
/// ignores case and treats '-', '_' and ' ' alike, so the suffix resolves.
/// The table above is written out by hand so its indices read at a glance;
/// this is the rule it follows, and a test holds the two together.
#[cfg(test)]
pub fn hdri_procedural_name(preset: &str) -> String {
    let mut out = String::from(HDRI_PREFIX);
    for ch in preset.chars() {
        out.push(match ch {
            ' ' | '-' => '_',
            c => c.to_ascii_lowercase(),
        });
    }
    out
}

/// The environment a procedural index names, baked or painted: an hdri
/// preset with its key, the map without that key and its sky's own sun
/// (`hdri::envmap::bake_generated_env_map`); render-material's looks have
/// none of them. `run` spreads a bake's rows (the pool inside a job). A bake
/// stops in the first build of a seed's cloud noise when `stop` says so
/// (`bake_generated_env_map_cancellable`, D15).
fn procedural_env_map(i: u32, run: RowRunner, bake_width: usize, stop: &dyn Fn() -> bool) -> Option<GeneratedEnvMap> {
    let name = procedural_environment_name(i)?;
    if let Some(hdri_name) = name.strip_prefix(HDRI_PREFIX) {
        let params = crate::hdri::presets::preset(hdri_name)?;
        crate::hdri::envmap::bake_generated_env_map_cancellable(&params, &crate::hdri::ImageSet::default(), bake_width, run, stop)
    } else {
        let preset = EnvPreset::by_name(name)?;
        Some(GeneratedEnvMap { map: EnvMap::procedural(&preset, 256, 1.0, 0.0), keyless: None, sun: None, daylight: None })
    }
}

/// Build everything for one key: the map (registered, or baked/painted for
/// a procedural index), then `prepare_ibl_until` without the key the
/// directional light carries. Pure apart from `run` and `stop`, so the job
/// and the synchronous fallback share it. `None` when the source cannot be
/// built or `stop` said so.
#[allow(clippy::too_many_arguments)]
fn build_ibl(
    generation: u64,
    key: PrepareKey,
    map: Option<RegisteredMap>,
    intensity: f32,
    rotation_deg: f32,
    run: RowRunner,
    sizes: PrepareSizes,
    bake_width: usize,
    stop: &dyn Fn() -> bool,
) -> Option<IblJobResult> {
    // The keyless copy is the map without one key: a preset's own, or the
    // one the host declares for the copy it registered.
    let (map, keyless, keyless_for, baked_sun, baked_daylight) = match key.source {
        IblSource::Hdri(_) => {
            let r = map?;
            (r.map, r.keyless, key.sun, None, None)
        }
        IblSource::Procedural(i) => {
            let g = procedural_env_map(i, run, bake_width, stop)?;
            (Arc::new(g.map), g.keyless.map(Arc::new), g.sun, g.sun, g.daylight)
        }
    };
    if stop() {
        return None;
    }
    // The key the lighting copy is made without: the one the world declares,
    // else the one a procedural hdri preset baked (a host that names a preset
    // by index has no map to declare a sun from). Either way the directional
    // light carries it (`ibl_sun`, renderer/env_sun.rs) and the SH, the atlas
    // and the meter do not. A generated map loses exactly that key's light
    // (N3); a key the map has no keyless copy for (a file's detected sun, a
    // world that declares another key over a preset) has its covering cone
    // filled.
    let fill = key.sun.or(baked_sun);
    let removal = match (fill.as_ref(), keyless.as_deref()) {
        (Some(_), Some(keyless)) if fill == keyless_for => KeyRemoval::Exact(keyless),
        (Some(sun), _) => KeyRemoval::Fill(sun),
        (None, _) => KeyRemoval::Nothing,
    };
    let prepared = prepare_ibl_until(&map, removal, intensity, rotation_deg, sizes, stop)?;
    Some(IblJobResult { generation, key, intensity, rotation_deg, prepared, baked_sun, baked_daylight })
}

/// The quad's two instance vectors for this frame. Pure, so the lane
/// switch, the meter and the dome gate are testable without a device.
/// `metered_luminance` is the luminance the dome shows: the bound map's
/// whole-sphere mean x `Ibl.intensity` (the caller multiplies).
pub(super) fn env_dome_controls(
    blur: f32,
    intensity: f32,
    projection: &Mat4f,
    hdr_output: bool,
    metered_luminance: f32,
    exposure_ev: f32,
    dome_size: Option<(usize, usize)>,
) -> (Vec4f, Vec4f) {
    let clean = |v: f32, fallback: f32| if v.is_finite() { v } else { fallback };
    let (px, py) = (projection.v[0], projection.v[5]);
    let bg = vec4(
        clean(blur, 0.0).clamp(0.0, 1.0),
        clean(intensity, 0.0).max(0.0),
        1.0 / if px.abs() > 1e-6 { px } else { 1.0 },
        1.0 / if py.abs() > 1e-6 { py } else { 1.0 },
    );
    // Legacy lane: the same reflected-light meter as sky::exposure, on the
    // dome's whole-sphere mean at the environment's scale; the floor keeps
    // a black map finite. The HDR lane has no exposure of its own (the
    // composite's) and no gain: the dome is the map x Ibl.intensity,
    // exactly what the IBL lights with (HDR_SKY_GAIN is the analytic
    // sky's).
    let (exposure, lane) = if hdr_output {
        (0.0, 1.0)
    } else {
        let metered = clean(metered_luminance, 0.0).max(crate::sky::EXPOSURE_LUMINANCE_FLOOR);
        ((crate::sky::EXPOSURE_KEY / metered) * 2.0f32.powf(clean(exposure_ev, 0.0).clamp(-12.0, 12.0)), 0.0)
    };
    let (dw, dh) = dome_size.map(|(w, h)| (w as f32, h as f32)).unwrap_or((0.0, 0.0));
    (bg, vec4(exposure, lane, dw, dh))
}

impl Renderer {
    /// Make an HDR environment available to `IblSource::Hdri(texture)`.
    /// Decoding (and its budget) is the host's: render-material's
    /// `ibl::load_hdr` confines it. Registering the map that is registered
    /// under the handle already (the same `Arc`) changes nothing: a host may
    /// do it every frame. Registering another map is a change of map: a job
    /// preparing the old one is cancelled and never lands, and a world that
    /// names the handle prepares the new map in the background (a restart),
    /// while the old textures and numbers stay bound until it lands (the
    /// next bake of the same environment is not a change of map:
    /// [`Self::register_rebaked_environment`]). A key
    /// the world declares for this map has its covering cone filled in the
    /// lighting copy (a file's detected sun); a generated map registers its
    /// keyless copy too ([`Self::register_generated_environment`]).
    pub fn register_environment(&mut self, texture: TextureRef, env: Arc<EnvMap>) {
        self.register_generated_environment(texture, env, None);
    }

    /// [`Self::register_environment`] for a generated map (an hdri bake):
    /// `keyless` is the same map without exactly the key the world declares
    /// on `Environment.sun` (`hdri::envmap::bake_generated_env_map`), and
    /// the lighting copy is made from it while the world declares a key, so
    /// only that key's own light leaves the SH, the atlas, the meter and the
    /// fog band (N3); the dome shows `env`. `None` is `register_environment`.
    /// Registering the same two `Arc`s again changes nothing; a new pair is a
    /// change of map. A host that bakes the same environment again (a clock's
    /// next bake) registers the bake with
    /// [`Self::register_rebaked_environment`], which lets the preparation in
    /// flight land.
    pub fn register_generated_environment(&mut self, texture: TextureRef, env: Arc<EnvMap>, keyless: Option<Arc<EnvMap>>) {
        self.register_map(texture, env, keyless, false);
    }

    /// The next bake of the environment registered under `texture`, with its
    /// keyless copy as for [`Self::register_generated_environment`]: a host
    /// clock that re-bakes its map (the sandbox's `params:` sky bakes again
    /// every quarter hour of game time, every 7.5 minutes while the sun is
    /// low). A re-bake is not a change of map
    /// but the map's form of a key that moves (K1, K7): the job preparing an
    /// earlier bake runs to its end and is adopted when it lands (newer than
    /// what is bound, the bakes registered meanwhile notwithstanding), and
    /// then the newest bake prepares; the bakes in between never do. So a
    /// clock that re-bakes faster than the renderer prepares shows at the
    /// renderer's pace, at most one preparation behind its newest bake,
    /// with light and sky from the same preparation (M1). The world declares
    /// the newest bake's key, which its preparation is made without. With
    /// nothing registered under the handle this is a first registration;
    /// the same two `Arc`s again change nothing.
    pub fn register_rebaked_environment(&mut self, texture: TextureRef, env: Arc<EnvMap>, keyless: Option<Arc<EnvMap>>) {
        self.register_map(texture, env, keyless, true);
    }

    /// Register `env` (and its keyless copy) under `texture`, as the next
    /// bake of the environment registered there (`rebake`) or as a change of
    /// map, which cancels a job preparing the handle.
    fn register_map(&mut self, texture: TextureRef, env: Arc<EnvMap>, keyless: Option<Arc<EnvMap>>, rebake: bool) {
        let mut registered = RegisteredMap { map: env, keyless, bakes: 0 };
        let now = self.ibl.maps.get(&texture);
        if now.is_some_and(|m| m.same(&registered)) {
            return;
        }
        let continued = if rebake { now.map(|m| m.bakes) } else { None };
        registered.bakes = continued.unwrap_or_else(|| {
            self.ibl.registrations = self.ibl.registrations.wrapping_add(1);
            self.ibl.registrations
        });
        let names = |k: Option<IblSource>| k == Some(IblSource::Hdri(texture));
        if names(self.ibl.key.map(|k| k.0)) {
            self.ibl.key = None;
        }
        if names(self.ibl.prepared_for.map(|p| p.source)) {
            self.ibl.prepared_for = None;
        }
        if names(self.ibl.failed_for.map(|p| p.source)) {
            self.ibl.failed_for = None;
        }
        if continued.is_none() && names(self.ibl.job.as_ref().map(|j| j.key.source)) {
            self.cancel_ibl_job();
        }
        self.ibl.maps.insert(texture, registered);
    }

    /// Drop the map registered under `texture` (and its keyless copy) and any
    /// preparation of it: a job preparing it is cancelled (it never lands),
    /// and when the bound textures and numbers are its, they go too. This
    /// releases the renderer's references to the maps (a cancelled job's own
    /// go when it winds down). A world that still names the handle names an
    /// unknown source from here on: nothing prepares, nothing is awaited, and
    /// the analytic sky and rig take over, as before it was registered.
    pub fn unregister_environment(&mut self, texture: TextureRef) {
        if self.ibl.maps.remove(&texture).is_none() {
            return;
        }
        let names = |k: Option<IblSource>| k == Some(IblSource::Hdri(texture));
        if names(self.ibl.job.as_ref().map(|j| j.key.source)) {
            self.cancel_ibl_job();
        }
        if names(self.ibl.bound.map(|b| b.0.source)) {
            self.unbind_ibl();
        }
        if names(self.ibl.wanted.map(|w| w.source)) {
            self.ibl.wanted = None;
        }
        if names(self.ibl.failed_for.map(|p| p.source)) {
            self.ibl.failed_for = None;
        }
    }

    /// Draw `live`'s environment (I4): a renderer forked from it (a
    /// split-screen pane, the AI world review: `fork_scene_for_review`
    /// mirrors once at the fork) takes its registered maps and its bound
    /// preparation, the same textures, numbers, key and daylight report, so
    /// its dome, its IBL materials, its rig, meter and fog and its "is it
    /// day?" switches are the live renderer's. From then on this renderer's
    /// draws resolve no environment of their own (as `draw_scene_aux`'s
    /// do): it never prepares, binds or drops one, whatever its world names.
    /// A fork kept across frames calls this every frame, after the live
    /// renderer's scene draw, so it follows the live renderer to each new
    /// preparation (and to none), and lets go of the maps the live renderer
    /// let go. Intensity and rotation ride the lane texture's meta row,
    /// which the live renderer rewrites in place, so the mirror sees those
    /// at once.
    pub fn mirror_environment_from(&mut self, live: &Renderer) {
        self.ibl.mirror_from(&live.ibl);
    }

    /// The IBL lane texture for this frame, when the environment asks for one.
    pub fn ibl_texture(&self) -> Option<&Texture> {
        self.ibl.texture.as_ref()
    }

    /// The full-resolution dome (RGBA f32, at most 2048 wide) the
    /// environment background draws at blur 0.
    pub fn ibl_dome_texture(&self) -> Option<&Texture> {
        self.ibl.dome.as_ref()
    }

    /// A preparation is bound: its textures are up and the frame draws
    /// with them. Not necessarily the world's current source or key: the
    /// previous preparation stays bound while the next one prepares (a new
    /// key, a new map, another environment named), so light and sky never
    /// drop out in between. A host that waits for the world's own
    /// environment waits for [`Self::environment_pending`] to turn false
    /// (not one that re-bakes its map faster than the renderer prepares:
    /// that one sees it true nearly all the time, see there).
    pub fn environment_ready(&self) -> bool {
        self.ibl.bound.is_some() && self.ibl.texture.is_some()
    }

    /// ... and its dome texture with it.
    pub fn environment_dome_ready(&self) -> bool {
        self.environment_ready() && self.ibl.dome.is_some()
    }

    /// The world's environment is not prepared yet: a job is preparing (or
    /// finished, and waits for the next draw to adopt it), or the world
    /// wants a key no job was submitted for yet (the queue was full, or the
    /// job in flight is for an older key: the draw after it lands submits
    /// the latest). A host keeps drawing while this holds, since a draw is
    /// what adopts a result and what submits; `items_ready` is false
    /// meanwhile, so a locked-time host does not take the frame before the
    /// environment is in it. A source that cannot be built (an index past
    /// the table, a handle nobody registered), a failed job, and a job
    /// cancelled by a change of map that is winding down are not pending:
    /// nothing will come from them.
    ///
    /// A host that re-bakes its map faster than the renderer prepares one
    /// (`register_rebaked_environment`: the sandbox's day cycle on a fast
    /// day, a bake every 0.3 s on a 30 s day against a preparation of about
    /// 0.7 s) sees this true nearly all the time: whenever a preparation
    /// lands, a newer bake is already registered and wanted, so the next
    /// one is submitted at once. Such a host must not wait for it to turn
    /// false before it draws or hands the renderer its next bake, nor use
    /// `items_ready` (which is false while this holds) to gate its frames:
    /// it would wait for a moment that does not come.
    /// [`Self::environment_ready`] says an environment is bound; the bakes
    /// land at the renderer's pace meanwhile (K7). The sandbox reads
    /// neither.
    pub fn environment_pending(&self) -> bool {
        let s = &self.ibl;
        let live = s.job.as_ref().is_some_and(|j| !j.cancel.is_cancelled());
        live || s.wanted.is_some_and(|w| s.prepared_for != Some(w) && s.failed_for != Some(w))
    }

    /// How many preparations were submitted so far, the synchronous ones of
    /// a closed pool included (tests: a key change prepares exactly once).
    pub fn environment_preparations(&self) -> u64 {
        self.ibl.generation
    }

    /// The SH9 irradiance coefficients of the BOUND lighting copy. The
    /// three numeric accessors are gated on the bound numbers (`sh` is set
    /// in `adopt_ibl` and cleared in `unbind_ibl`), not on `prepared_for`:
    /// re-registering the bound handle clears `prepared_for`, and the rig,
    /// the exposure and the fog must keep the old map's values until the
    /// new ones land (a day cycle re-bakes on its grid, a quarter hour or less).
    pub fn ibl_sh9(&self) -> Option<&[[f32; 3]; 9]> {
        self.ibl.sh.as_ref()
    }

    /// Solid-angle mean luminance of the bound lighting copy.
    pub fn ibl_mean_luminance(&self) -> Option<f32> {
        self.ibl.sh.map(|_| self.ibl.mean_luminance)
    }

    /// Linear mean colour of the horizon band of the bound lighting copy.
    pub fn ibl_horizon_rgb(&self) -> Option<Vec3f> {
        self.ibl.sh.map(|_| self.ibl.horizon_rgb)
    }

    /// The key the bound preparation was made with, the sun its lighting
    /// copy was prepared without (exactly its light for a generated map, its
    /// covering cone for a file's sun): the world's `Environment.sun` as the
    /// job saw it, else the sun a procedural hdri preset baked. `None` when
    /// the map kept all its light (no declared sun, an engine preset, a bake
    /// without a key). The rig lights with this key, not with the one the
    /// world declares meanwhile (renderer/env_sun.rs): light and sky come
    /// from the same preparation.
    pub fn ibl_sun(&self) -> Option<EnvSun> {
        self.ibl.sh.and(self.ibl.filled_sun)
    }

    /// The bound map's own sun for the daylight switches, in the map's frame:
    /// the `Environment.daylight_sun` the job saw, else a procedural hdri
    /// preset's sky's sun. `None` when the map knows no sun of its own (a
    /// studio, a loaded file, an engine look) or nothing is bound: the
    /// switches then follow the world's own sun (renderer/env_sun.rs).
    pub fn ibl_daylight_sun(&self) -> Option<Vec3f> {
        self.ibl.sh.and(self.ibl.daylight_sun)
    }

    /// `resolve_ibl` for a draw of this scope: the scene's draw resolves its
    /// world's environment, an aux draw leaves the environment as it is, and
    /// so does every draw of a mirror (`mirror_environment_from`): what it
    /// draws is the live renderer's.
    pub(super) fn resolve_ibl_for(&mut self, cx: &mut Cx, env: &Environment, scope: EnvScope) {
        match scope {
            EnvScope::Scene if !self.ibl.mirror => self.resolve_ibl(cx, env),
            EnvScope::Scene | EnvScope::Aux => {}
        }
    }

    /// Prepare (in the background, on change) or drop the environment's
    /// textures. Called once per frame before the items pick materials.
    pub(super) fn resolve_ibl(&mut self, cx: &mut Cx, env: &Environment) {
        let Some(ibl) = env.ibl.filter(|i| i.intensity.is_finite() && i.rotation_deg.is_finite()) else {
            self.drop_ibl();
            return;
        };
        let sun = env.sun.filter(|s| s.validate().is_ok());
        let daylight_sun = env.daylight_sun.filter(|d| d.is_finite() && d.length() >= 1.0e-3);
        let wanted = PrepareKey { source: ibl.source, sun, daylight_sun };
        // Set before the submit: a source that cannot be built drops it again.
        self.ibl.wanted = Some(wanted);
        // Another environment named is a change of map: the job in flight is
        // cancelled (else a world that returns to the bound source while it
        // runs would see the abandoned source's textures flip in, and need a
        // second job to get its own back). A new key for the same source is
        // not: that job runs to its end and lands.
        if self.ibl.job.as_ref().is_some_and(|j| j.key.source != wanted.source) {
            self.cancel_ibl_job();
        }
        self.adopt_finished_ibl(cx);
        // One job at a time: the latest key prepares once none is in flight.
        if self.ibl.job.is_none() && self.ibl.prepared_for != Some(wanted) && self.ibl.failed_for != Some(wanted) {
            self.submit_ibl(cx, wanted, ibl.intensity, ibl.rotation_deg);
        }
        // Intensity and rotation ride the meta row: rewrite texel 9 when
        // they moved and the lane texture is this source's.
        let key = (ibl.source, ibl.intensity.to_bits(), ibl.rotation_deg.to_bits());
        if self.ibl.texture.is_some() && self.ibl.key != Some(key) && self.ibl.prepared_for.map(|p| p.source) == Some(ibl.source) {
            self.rewrite_ibl_meta(cx, ibl.intensity, ibl.rotation_deg);
            self.ibl.key = Some(key);
        }
    }

    /// A change of map: the job in flight stops at its next stage (one not
    /// started yet never starts). It stays the job until it ends, so no
    /// second job runs beside it, and whatever it returns is rejected when
    /// it is taken (`job_is_current`).
    fn cancel_ibl_job(&mut self) {
        if let Some(job) = &self.ibl.job {
            job.cancel.cancel();
            job.handle.cancel();
        }
    }

    /// No environment: nothing bound, nothing kept but the maps (the host
    /// unregisters those) and the background draw, which are what the
    /// world may ask for again.
    fn drop_ibl(&mut self) {
        self.cancel_ibl_job();
        self.ibl.wanted = None;
        self.ibl.failed_for = None;
        self.unbind_ibl();
    }

    /// Release the bound preparation: textures, numbers, key.
    fn unbind_ibl(&mut self) {
        let s = &mut self.ibl;
        s.key = None;
        s.texture = None;
        s.dome = None;
        s.dome_size = (0, 0);
        s.prepared_for = None;
        s.bound = None;
        s.sh = None;
        s.mean_luminance = 0.0;
        s.horizon_rgb = Vec3f::default();
        s.filled_sun = None;
        s.daylight_sun = None;
    }

    /// Submit the Heavy job for `wanted` (only called with no job in
    /// flight). A full queue retries next frame; a closed pool (wasm
    /// without atomics) prepares now, small.
    fn submit_ibl(&mut self, cx: &mut Cx, wanted: PrepareKey, intensity: f32, rotation_deg: f32) {
        let map = match wanted.source {
            IblSource::Hdri(t) => self.ibl.maps.get(&t).cloned(),
            IblSource::Procedural(_) => None,
        };
        let resolvable = match wanted.source {
            IblSource::Hdri(_) => map.is_some(),
            IblSource::Procedural(i) => procedural_environment_name(i).is_some(),
        };
        if !resolvable {
            // An unregistered handle or an index past the table: nothing to
            // build, nothing bound (as before).
            self.drop_ibl();
            return;
        }
        let pool = cx.task_pool();
        match pool.reserve(Lane::Heavy) {
            Ok(slot) => {
                self.ibl.generation = self.ibl.generation.wrapping_add(1);
                self.ibl.queue_full_logged = false;
                let generation = self.ibl.generation;
                let rows: TaskPool = pool.clone();
                let cancel = CancellationToken::new();
                let token = cancel.clone();
                let job_map = map.clone();
                let handle = slot.submit(move || {
                    // Inside a Heavy job: a bake's rows fan out over the pool
                    // and stop once the token is set; the stages poll it too.
                    let pool_rows = move |n: usize, f: &(dyn Fn(usize) + Sync)| rows.fan_out(Lane::Heavy, n, f);
                    let run = rows_until_cancelled(&pool_rows, &token);
                    let stop = || token.is_cancelled();
                    build_ibl(generation, wanted, map, intensity, rotation_deg, &run, JOB_SIZES, PROCEDURAL_HDRI_WIDTH, &stop)
                });
                self.ibl.job = Some(IblJob { handle, key: wanted, map: job_map, cancel });
            }
            Err(SubmitError::QueueFull) => {
                if !self.ibl.queue_full_logged {
                    log!("ibl: worker queue busy; the environment prepares next frame");
                    self.ibl.queue_full_logged = true;
                }
            }
            Err(SubmitError::Closed) => {
                self.ibl.generation = self.ibl.generation.wrapping_add(1);
                let serial = |n: usize, f: &(dyn Fn(usize) + Sync)| {
                    for i in 0..n {
                        f(i);
                    }
                };
                let map = map.map(|r| RegisteredMap { map: shrunk_for_sync(r.map), keyless: r.keyless.map(shrunk_for_sync), bakes: r.bakes });
                let result = build_ibl(self.ibl.generation, wanted, map, intensity, rotation_deg, &serial, SYNC_SIZES, SYNC_HDRI_WIDTH, &|| false);
                match result {
                    Some(r) => self.adopt_ibl(cx, r),
                    None => self.ibl.failed_for = Some(wanted),
                }
            }
        }
    }

    /// Take a finished job. Its result is adopted when the job is current
    /// (`job_is_current`) and the result is newer than the bound
    /// preparation, even if the world declared another key or the host
    /// registered a later bake while it ran: the next submit prepares the
    /// latest one. A result of a replaced map or of a source the world no
    /// longer names is dropped. A failure is remembered (so it is not
    /// retried every frame) only for a current job of the map registered
    /// now that was not cancelled: a later bake gets its own try.
    fn adopt_finished_ibl(&mut self, cx: &mut Cx) {
        let Some(result) = self.ibl.job.as_mut().and_then(|j| j.handle.try_take()) else { return };
        let Some(job) = self.ibl.job.take() else { return };
        let JobStanding { current, registered } = self.job_is_current(&job);
        let cancelled = job.cancel.is_cancelled();
        let bound = self.ibl.bound.map_or(0, |b| b.1);
        match result {
            Ok(Some(r)) => {
                if current && r.generation > bound {
                    self.adopt_ibl(cx, r);
                    if !registered {
                        // An earlier bake: bound, but the newest still prepares.
                        self.ibl.prepared_for = None;
                    }
                }
            }
            Ok(None) => {
                if current && registered && !cancelled {
                    self.ibl.failed_for = Some(job.key);
                }
            }
            Err(e) => {
                if !cancelled {
                    log!("ibl: environment preparation failed: {e:?}");
                    if current && registered {
                        self.ibl.failed_for = Some(job.key);
                    }
                }
            }
        }
    }

    /// Whether the job prepares the source the world names, from the map
    /// registered under it now or an earlier bake of it (a procedural index
    /// has no map to replace). A change of map starts a new registration, so
    /// an earlier map's job is never current after it, even when the earlier
    /// map's bakes came first.
    fn job_is_current(&self, job: &IblJob) -> JobStanding {
        let s = &self.ibl;
        let named = s.wanted.is_some_and(|w| w.source == job.key.source);
        let (of_the_registration, registered) = match job.key.source {
            IblSource::Hdri(t) => match s.maps.get(&t).zip(job.map.as_ref()) {
                Some((now, prepared)) => (now.same(prepared) || now.bakes == prepared.bakes, now.same(prepared)),
                None => (false, false),
            },
            IblSource::Procedural(_) => (true, true),
        };
        JobStanding { current: named && of_the_registration, registered }
    }

    /// Upload a preparation: both textures replaced at once, the numbers
    /// beside them, the meta key set to what the job packed (resolve_ibl
    /// rewrites it if the world moved on meanwhile).
    fn adopt_ibl(&mut self, cx: &mut Cx, r: IblJobResult) {
        let p = r.prepared;
        let lane = Texture::new_with_format(cx, TextureFormat::VecRGBAf32 {
            width: p.texture.width,
            height: p.texture.height,
            data: Some(p.texture.data),
            updated: TextureUpdated::Full,
        });
        let dome = Texture::new_with_format(cx, TextureFormat::VecRGBAf32 {
            width: p.dome_width,
            height: p.dome_height,
            data: Some(p.dome),
            updated: TextureUpdated::Full,
        });
        let s = &mut self.ibl;
        s.texture = Some(lane);
        s.dome = Some(dome);
        s.dome_size = (p.dome_width, p.dome_height);
        s.key = Some((r.key.source, r.intensity.to_bits(), r.rotation_deg.to_bits()));
        s.prepared_for = Some(r.key);
        s.bound = Some((r.key, r.generation));
        s.failed_for = None;
        s.sh = Some(p.sh);
        s.mean_luminance = p.mean_luminance;
        s.horizon_rgb = p.horizon_rgb;
        s.filled_sun = r.key.sun.or(r.baked_sun);
        s.daylight_sun = r.key.daylight_sun.or(r.baked_daylight);
    }

    /// Rewrite meta texel 9's intensity and rotation in the resident lane
    /// texture, one texel dirty. The meta row is row 0 (render-material's
    /// `pack_ibl` layout since plan 1a task A7), so texel 9 is data[36..40]
    /// whatever the texture's width or height.
    fn rewrite_ibl_meta(&mut self, cx: &mut Cx, intensity: f32, rotation_deg: f32) {
        let Some(texture) = self.ibl.texture.clone() else { return };
        let mut data = texture.take_vec_f32(cx);
        let at = 9 * 4;
        if at + 4 <= data.len() {
            data[at + 2] = intensity.max(0.0);
            data[at + 3] = rotation_deg.to_radians();
        }
        texture.put_back_vec_f32(cx, data, Some(RectUsize::new(PointUsize::new(9, 0), SizeUsize::new(1, 1))));
    }

    /// Draw the environment as the background when the world asks for it
    /// (first in the scene pass: everything after draws over it). Returns
    /// true when the full-resolution dome was drawn: the frame then skips
    /// its analytic or gradient dome. Until the dome lands, the atlas
    /// background draws under the dome exactly as before; an MR stage
    /// never shows an environment (the room supplies the horizon).
    pub(super) fn draw_environment_background(&mut self, cx: &mut Cx3d, env: &Environment, projection: &Mat4f, exposure_ev: f32) -> bool {
        let Background::Environment { blur, intensity } = env.background else { return false };
        if !self.stage.shows_sky() {
            return false;
        }
        let Some(texture) = self.ibl.texture.clone() else { return false };
        if self.ibl.background.is_none() {
            // Held VM (a script-driven draw is mid-apply): try again next
            // frame rather than drawing a sky with no shader.
            self.ibl.background = cx.cx.try_with_vm(|vm| {
                makepad_render_material::builtin::register(vm);
                let draw = vm.value(vm.modules(), LiveId::from_str("draw")).as_object();
                let have = draw.is_some_and(|d| {
                    let v = vm.value(d, LiveId::from_str("DrawEnvBackground"));
                    !v.is_nil() && !v.is_err()
                });
                if !have {
                    script_mod(vm);
                }
                Box::new(DrawEnvBackground::script_new_with_default(vm))
            });
        }
        let dome = self.ibl.dome.clone();
        // The dome's LOGICAL size: the shader addresses texels in it, never
        // in the allocation's size().
        let dome_size = dome.as_ref().map(|_| self.ibl.dome_size);
        // The legacy meter reads the luminance the dome shows: the map's
        // mean at the environment's scale x Ibl.intensity (C5's env_exposure
        // meters the rig on the same product).
        let ibl_scale = env.ibl.map_or(0.0, |i| if i.intensity.is_finite() { i.intensity.max(0.0) } else { 0.0 });
        let (bg, bg2) = env_dome_controls(blur, intensity, projection, self.hdr_output, self.ibl.mean_luminance * ibl_scale, exposure_ev, dome_size);
        let Some(d) = self.ibl.background.as_mut() else { return false };
        d.bg = bg;
        d.bg2 = bg2;
        d.draw_vars.options.depth_write = false;
        d.draw_vars.set_texture(0, &texture);
        // Slot 1 is env_tex; the lane texture stands in until the dome is
        // up (bg2.z = 0 keeps the shader on the atlas path).
        d.draw_vars.set_texture(1, dome.as_ref().unwrap_or(&texture));
        let drawn = d.draw_vars.can_instance();
        if drawn {
            cx.add_instance(&d.draw_vars);
        }
        drawn && dome.is_some()
    }
}

#[cfg(test)]
impl Renderer {
    /// Sibling test modules (`sun_tests`, C6) hand-feed a landed
    /// preparation's numbers: no pool, no device, no textures. The numeric
    /// accessors are gated on `sh`, so this is all they need.
    pub(super) fn feed_environment_numbers_for_tests(&mut self, sh: [[f32; 3]; 9], mean_luminance: f32, horizon_rgb: Vec3f) {
        self.ibl.sh = Some(sh);
        self.ibl.mean_luminance = mean_luminance;
        self.ibl.horizon_rgb = horizon_rgb;
    }

    /// ... and the key the hand-fed preparation was made with (`ibl_sun`).
    pub(super) fn feed_environment_sun_for_tests(&mut self, sun: Option<EnvSun>) {
        self.ibl.filled_sun = sun;
    }

    /// ... and its map's own sun for the daylight switches (`ibl_daylight_sun`).
    pub(super) fn feed_environment_daylight_for_tests(&mut self, daylight: Option<Vec3f>) {
        self.ibl.daylight_sun = daylight;
    }

    /// A job is in flight, live or cancelled and winding down.
    pub(super) fn ibl_job_in_flight(&self) -> bool {
        self.ibl.job.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_scene::Ibl;

    fn env_of(source: IblSource, intensity: f32, rotation_deg: f32) -> Environment {
        Environment {
            ibl: Some(Ibl { source, intensity, rotation_deg }),
            background: Background::Environment { blur: 0.0, intensity: 1.0 },
            ..Default::default()
        }
    }
    fn grey(width: usize, v: f32) -> Arc<EnvMap> {
        Arc::new(EnvMap::constant(width, [v, v, v]))
    }
    /// Resolve every 2 ms until no job is in flight (a real pool job; a
    /// prefilter at 256 x 128 x 6 takes seconds in debug). The clock and the
    /// sleep are the test's: it runs natively, with a real pool.
    #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
    fn settle(renderer: &mut Renderer, cx: &mut Cx, env: &Environment) {
        let start = std::time::Instant::now();
        loop {
            renderer.resolve_ibl(cx, env);
            if renderer.ibl.job.is_none() {
                return;
            }
            assert!(start.elapsed().as_secs() < 180, "the preparation did not finish within 180 s");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    fn size_of(texture: &Texture, cx: &mut Cx) -> (usize, usize) {
        match texture.get_format(cx) {
            TextureFormat::VecRGBAf32 { width, height, .. } => (*width, *height),
            other => panic!("expected an RGBA f32 texture, got {other:?}"),
        }
    }
    /// Texel `i` of the lane texture's meta row, which is row 0 (plan 1a
    /// task A7: `pack_ibl` puts it first).
    fn meta_texel_at(texture: &Texture, cx: &mut Cx, i: usize) -> [f32; 4] {
        match texture.get_format(cx) {
            TextureFormat::VecRGBAf32 { data: Some(data), .. } => {
                let at = i * 4;
                [data[at], data[at + 1], data[at + 2], data[at + 3]]
            }
            other => panic!("expected resident RGBA f32 data, got {other:?}"),
        }
    }
    fn meta_texel(texture: &Texture, cx: &mut Cx) -> [f32; 4] {
        meta_texel_at(texture, cx, 9)
    }

    /// The whole life of one environment: nothing until a world asks; a
    /// job while it prepares (the lane texture stays None: items keep the
    /// analytic sky, exactly as today with no environment); both textures
    /// once it lands; the same source on later frames means no new work;
    /// no environment drops everything.
    #[test]
    fn a_registered_environment_prepares_in_the_background_and_uploads_both_textures() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        renderer.resolve_ibl(&mut cx, &Environment::default());
        assert_eq!(renderer.environment_preparations(), 0);
        assert!(!renderer.environment_pending() && !renderer.environment_ready());

        renderer.register_environment(TextureRef(1), grey(32, 0.25));
        let env = env_of(IblSource::Hdri(TextureRef(1)), 1.0, 0.0);
        renderer.resolve_ibl(&mut cx, &env);
        assert!(renderer.environment_pending(), "a Heavy job was submitted");
        assert!(renderer.ibl_texture().is_none() && !renderer.environment_ready());
        assert_eq!(renderer.environment_preparations(), 1);

        settle(&mut renderer, &mut cx, &env);
        assert!(renderer.environment_ready() && renderer.environment_dome_ready());
        let lane = renderer.ibl_texture().cloned().expect("lane texture");
        let dome = renderer.ibl_dome_texture().cloned().expect("dome texture");
        assert_eq!(size_of(&lane, &mut cx), (256, 1 + 6 * 128), "the meta row (row 0) plus prefilter 256 x 6");
        assert_eq!(size_of(&dome, &mut cx), (32, 16), "the dome is the map's own size");
        assert_eq!(renderer.ibl.dome_size, (32, 16), "the logical size the shader addresses");
        assert!(
            matches!(dome.get_format(&mut cx), TextureFormat::VecRGBAf32 { .. }),
            "the dome never carries mips: a chain would turn the atan2 cut at +Z into a one-pixel line"
        );
        assert_eq!(meta_texel(&lane, &mut cx), [6.0, 128.0, 1.0, 0.0]);
        let sh = renderer.ibl_sh9().expect("sh9");
        let dc = meta_texel_at(&lane, &mut cx, 0);
        assert_eq!([dc[0], dc[1], dc[2]], sh[0], "row 0 holds the SH9 (plan 1a task A7: the meta row first)");
        let e = makepad_render_material::ibl::sh9_irradiance(sh, [0.0, 1.0, 0.0]);
        assert!((e[1] - std::f32::consts::PI * 0.25).abs() < 0.02, "a constant 0.25 map: E = pi L, got {e:?}");
        assert!((renderer.ibl_mean_luminance().unwrap() - 0.25).abs() < 1.0e-3);
        assert!((renderer.ibl_horizon_rgb().unwrap() - vec3f(0.25, 0.25, 0.25)).length() < 1.0e-3);
        assert_eq!(renderer.ibl_sun(), None);

        for _ in 0..3 {
            renderer.resolve_ibl(&mut cx, &env);
        }
        assert_eq!(renderer.environment_preparations(), 1, "the same source on later frames is no new work");
        assert!(!renderer.environment_pending());

        renderer.resolve_ibl(&mut cx, &Environment::default());
        assert!(renderer.ibl_texture().is_none() && renderer.ibl_dome_texture().is_none());
        assert!(!renderer.environment_ready() && renderer.ibl_sh9().is_none());
    }

    /// Intensity and rotation are meta texels the shader reads per pixel:
    /// changing them rewrites texel 9 in place and never re-prefilters.
    #[test]
    fn an_intensity_or_rotation_change_rewrites_the_meta_texel_without_a_new_preparation() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        renderer.register_environment(TextureRef(4), grey(16, 0.5));
        let env = env_of(IblSource::Hdri(TextureRef(4)), 1.0, 0.0);
        settle(&mut renderer, &mut cx, &env);
        assert_eq!(renderer.environment_preparations(), 1);
        let lane = renderer.ibl_texture().cloned().unwrap();

        let turned = env_of(IblSource::Hdri(TextureRef(4)), 2.0, 45.0);
        renderer.resolve_ibl(&mut cx, &turned);
        assert!(!renderer.environment_pending());
        assert_eq!(renderer.environment_preparations(), 1, "no job for a meta change");
        let same = renderer.ibl_texture().cloned().unwrap();
        assert_eq!(same.texture_id(), lane.texture_id(), "the texture object is kept");
        let meta = meta_texel(&same, &mut cx);
        assert_eq!((meta[0], meta[1], meta[2]), (6.0, 128.0, 2.0));
        assert!((meta[3] - 45.0f32.to_radians()).abs() < 1.0e-6);
        // A non-finite intensity drops the environment, as before.
        renderer.resolve_ibl(&mut cx, &env_of(IblSource::Hdri(TextureRef(4)), f32::NAN, 0.0));
        assert!(renderer.ibl_texture().is_none());
    }

    /// A source that changes while a job runs: the old job's result is
    /// stale and dropped; the textures that land are the new source's. The
    /// previous environment's textures stay bound until then.
    #[test]
    fn a_source_change_while_preparing_drops_the_stale_result_and_keeps_the_old_textures() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        renderer.register_environment(TextureRef(1), grey(32, 0.25));
        renderer.register_environment(TextureRef(2), grey(16, 0.75));
        let a = env_of(IblSource::Hdri(TextureRef(1)), 1.0, 0.0);
        let b = env_of(IblSource::Hdri(TextureRef(2)), 1.0, 0.0);
        settle(&mut renderer, &mut cx, &a);
        let dome_a = renderer.ibl_dome_texture().cloned().unwrap();
        assert_eq!(size_of(&dome_a, &mut cx), (32, 16));

        // Switch to B and, before it lands, back to A and to B again: two
        // more jobs: B, then B again after the world went back to A (naming
        // A is a change of map: the first B job is cancelled and never
        // lands; the second waits for it to wind down, one job at a time).
        // One result adopted.
        renderer.resolve_ibl(&mut cx, &b);
        assert!(renderer.environment_pending());
        assert_eq!(renderer.ibl_dome_texture().map(|t| t.texture_id()), Some(dome_a.texture_id()), "A stays up while B prepares");
        assert!(renderer.environment_ready() && renderer.environment_dome_ready(), "A's textures stay bound");
        renderer.resolve_ibl(&mut cx, &a);
        assert!(!renderer.environment_pending(), "back on the bound source: the B job is cancelled, nothing new is needed");
        assert_eq!(renderer.ibl_dome_texture().map(|t| t.texture_id()), Some(dome_a.texture_id()));
        renderer.resolve_ibl(&mut cx, &b);
        assert!(renderer.environment_pending());
        settle(&mut renderer, &mut cx, &b);
        assert_eq!(renderer.environment_preparations(), 3, "the cancelled B, then B; settling submits nothing more");
        let dome_b = renderer.ibl_dome_texture().cloned().unwrap();
        assert_eq!(size_of(&dome_b, &mut cx), (16, 8), "B's dome");
        assert!((renderer.ibl_mean_luminance().unwrap() - 0.75).abs() < 1.0e-3, "B's meter");
        assert_eq!(renderer.ibl.prepared_for.map(|p| p.source), Some(IblSource::Hdri(TextureRef(2))));
    }

    /// The declared sun is part of what was prepared: it is filled in the
    /// lighting copy, so a new sun re-prepares once, and `ibl_sun` reports it.
    #[test]
    fn a_declared_sun_is_removed_from_the_lighting_and_reported() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        let sun_dir = crate::hdri::dir_from_az_el(90.0, 45.0);
        let cos_radius = 3.0f32.to_radians().cos();
        let sun = EnvSun { dir: sun_dir, radiance: vec3f(1.0e4, 1.0e4, 1.0e4), cos_radius, facing: 1.0, cos_cover: cos_radius };
        let map = EnvMap::from_fn(128, |d| if crate::hdri::vec(d).dot(sun_dir) >= sun.cos_radius { [1.0e4; 3] } else { [0.5; 3] });
        renderer.register_environment(TextureRef(9), Arc::new(map));
        let mut env = env_of(IblSource::Hdri(TextureRef(9)), 1.0, 0.0);
        settle(&mut renderer, &mut cx, &env);
        assert!(renderer.ibl_mean_luminance().unwrap() > 2.0, "no declared sun: the disc is metered");
        env.sun = Some(sun);
        settle(&mut renderer, &mut cx, &env);
        assert_eq!(renderer.environment_preparations(), 2, "a new sun prepares once more");
        assert!((renderer.ibl_mean_luminance().unwrap() - 0.5).abs() < 0.01, "the cone is filled in the lighting copy");
        assert_eq!(renderer.ibl_sun(), Some(sun));
        settle(&mut renderer, &mut cx, &env);
        assert_eq!(renderer.environment_preparations(), 2);
    }

    /// Re-registering a handle re-prepares it; a handle nobody registered
    /// builds nothing (today's behaviour).
    #[test]
    fn re_registering_a_handle_prepares_again_and_an_unknown_handle_builds_nothing() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        let env = env_of(IblSource::Hdri(TextureRef(7)), 1.0, 0.0);
        for _ in 0..3 {
            renderer.resolve_ibl(&mut cx, &env);
        }
        assert_eq!(renderer.environment_preparations(), 0);
        assert!(renderer.ibl_texture().is_none());
        renderer.register_environment(TextureRef(7), grey(16, 0.1));
        settle(&mut renderer, &mut cx, &env);
        assert_eq!(renderer.environment_preparations(), 1);
        renderer.register_environment(TextureRef(7), grey(16, 0.9));
        renderer.resolve_ibl(&mut cx, &env);
        assert!(renderer.environment_pending() && renderer.ibl_texture().is_some(), "the old texture stays while the new map prepares");
        assert!(
            (renderer.ibl_mean_luminance().unwrap() - 0.1).abs() < 1.0e-3 && renderer.ibl_sh9().is_some() && renderer.ibl_horizon_rgb().is_some(),
            "the old numbers stay while the new map prepares"
        );
        settle(&mut renderer, &mut cx, &env);
        assert_eq!(renderer.environment_preparations(), 2);
        assert!((renderer.ibl_mean_luminance().unwrap() - 0.9).abs() < 1.0e-3);
    }

    /// A closed pool (wasm without atomics) prepares on the spot and small:
    /// a 64 wide atlas and a dome capped at 512, adopted by the same call.
    #[test]
    fn a_closed_pool_prepares_synchronously_and_small() {
        use makepad_draw::makepad_platform::thread::ShutdownMode;
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.task_pool().close(ShutdownMode::CancelPending);
        let mut renderer = Renderer::default();
        renderer.register_environment(TextureRef(3), grey(1024, 0.5));
        let env = env_of(IblSource::Hdri(TextureRef(3)), 1.5, 0.0);
        renderer.resolve_ibl(&mut cx, &env);
        assert!(!renderer.environment_pending(), "no job: the pool is closed");
        assert_eq!(renderer.environment_preparations(), 1);
        assert!(renderer.environment_dome_ready());
        let lane = renderer.ibl_texture().cloned().unwrap();
        let dome = renderer.ibl_dome_texture().cloned().unwrap();
        assert_eq!(size_of(&lane, &mut cx), (64, 1 + 6 * 32), "the meta row plus prefilter 64 x 6");
        assert_eq!(size_of(&dome, &mut cx), (SYNC_DOME_WIDTH, SYNC_DOME_WIDTH / 2), "a 1024 wide map is resized to the cap");
        assert_eq!(meta_texel(&lane, &mut cx), [6.0, 32.0, 1.5, 0.0], "the shader reads the level height from the meta row");
        assert!((renderer.ibl_mean_luminance().unwrap() - 0.5).abs() < 1.0e-3);
        renderer.resolve_ibl(&mut cx, &env);
        assert_eq!(renderer.environment_preparations(), 1, "and it is not repeated");
    }

    /// M5: on a closed pool the preparation runs on the UI thread, so the
    /// source is shrunk to the dome's 512 FIRST: the sanity scan, the dome,
    /// the lighting copy, its sun removal, the meter and the band all run
    /// on the small map, never on the full-resolution one. The numbers are
    /// exactly those of a preparation of the 512 wide resize.
    #[test]
    fn a_closed_pool_shrinks_the_source_before_preparing_it() {
        use crate::hdri::prepare::prepare_ibl_sized;
        use makepad_draw::makepad_platform::thread::ShutdownMode;
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.task_pool().close(ShutdownMode::CancelPending);
        let mut renderer = Renderer::default();
        let map = Arc::new(EnvMap::from_fn(1024, |d| [0.2 + 0.8 * d[1].max(0.0), 0.5 + 0.25 * d[0], 0.3 + 0.1 * d[2]]));
        renderer.register_environment(TextureRef(3), map.clone());
        renderer.resolve_ibl(&mut cx, &env_of(IblSource::Hdri(TextureRef(3)), 1.0, 0.0));
        let small = prepare_ibl_sized(&map.resized(SYNC_DOME_WIDTH), None, 1.0, 0.0, SYNC_DOME_WIDTH, SYNC_ATLAS_WIDTH);
        let full = prepare_ibl_sized(&map, None, 1.0, 0.0, SYNC_DOME_WIDTH, SYNC_ATLAS_WIDTH);
        assert_ne!(full.sh, small.sh, "premise: the full-resolution passes give other numbers");
        assert_eq!(renderer.ibl_sh9(), Some(&small.sh), "the SH of the shrunk source");
        assert_eq!(renderer.ibl_mean_luminance(), Some(small.mean_luminance));
        assert_eq!(renderer.ibl_horizon_rgb(), Some(small.horizon_rgb));
    }

    /// A locked-time host takes a frame once `items_ready` says so, and it
    /// records again until then: with the environment preparing off the UI
    /// thread, the first frame has no lane texture, so the host has to wait
    /// for it (a scene lit by the studio environment alone renders black
    /// until it lands). The wait covers the job in flight, a result that
    /// finished but is not adopted yet (adopting is the next draw's work),
    /// and an environment the world wants that no job was submitted for.
    #[test]
    fn items_are_not_ready_while_the_environment_prepares() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        renderer.resolve_ibl(&mut cx, &Environment::default());
        assert!(renderer.items_ready(&cx), "no environment, nothing to wait for");

        renderer.register_environment(TextureRef(1), grey(32, 0.25));
        let env = env_of(IblSource::Hdri(TextureRef(1)), 1.0, 0.0);
        renderer.resolve_ibl(&mut cx, &env);
        assert!(renderer.environment_pending());
        assert!(!renderer.items_ready(&cx), "the job is in flight: the frame has no lane texture yet");

        settle(&mut renderer, &mut cx, &env);
        assert!(renderer.environment_ready());
        assert!(!renderer.environment_pending());
        assert!(renderer.items_ready(&cx), "the lane texture is bound");
        // An intensity change is a meta rewrite, not a preparation.
        renderer.resolve_ibl(&mut cx, &env_of(IblSource::Hdri(TextureRef(1)), 2.0, 0.0));
        assert!(renderer.items_ready(&cx));

        // Another source: the old textures stay bound, the new ones are awaited.
        renderer.register_environment(TextureRef(2), grey(16, 0.75));
        let b = env_of(IblSource::Hdri(TextureRef(2)), 1.0, 0.0);
        renderer.resolve_ibl(&mut cx, &b);
        assert!(renderer.environment_ready() && !renderer.items_ready(&cx), "A is bound, B is awaited");
        settle(&mut renderer, &mut cx, &b);
        assert!(renderer.items_ready(&cx));

        // Back to A: B stays bound while A is awaited. Then no environment
        // at all: nothing to wait for, whatever was in flight.
        renderer.resolve_ibl(&mut cx, &env);
        assert!(!renderer.items_ready(&cx));
        renderer.resolve_ibl(&mut cx, &Environment::default());
        assert!(renderer.items_ready(&cx) && !renderer.environment_pending());
    }

    /// A job that finished is still awaited until a draw adopts it: the
    /// host's re-record (reattach) is what picks it up, so the wait must
    /// not end before that draw.
    #[test]
    #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
    fn a_finished_job_is_awaited_until_a_draw_adopts_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        renderer.register_environment(TextureRef(1), grey(16, 0.25));
        let env = env_of(IblSource::Hdri(TextureRef(1)), 1.0, 0.0);
        renderer.resolve_ibl(&mut cx, &env);
        let start = std::time::Instant::now();
        while !renderer.ibl.job.as_ref().is_some_and(|j| j.handle.is_finished()) {
            assert!(start.elapsed().as_secs() < 180, "the preparation did not finish within 180 s");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(renderer.environment_pending() && !renderer.items_ready(&cx), "finished, not adopted: nothing is bound yet");
        assert!(renderer.ibl_texture().is_none());
        renderer.resolve_ibl(&mut cx, &env);
        assert!(renderer.environment_ready() && renderer.items_ready(&cx));
    }

    /// A full Heavy queue means no job was submitted, but the world still
    /// wants the environment: the host keeps waiting (and drawing, which is
    /// the retry), and the next draw with room submits it.
    #[test]
    fn a_full_queue_is_awaited_and_retried() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        renderer.register_environment(TextureRef(1), grey(16, 0.25));
        let env = env_of(IblSource::Hdri(TextureRef(1)), 1.0, 0.0);
        // Hold every Heavy slot (reserved, never submitted).
        let pool = cx.task_pool();
        let mut held = Vec::new();
        while let Ok(slot) = pool.reserve(Lane::Heavy) {
            held.push(slot);
        }
        renderer.resolve_ibl(&mut cx, &env);
        renderer.resolve_ibl(&mut cx, &env);
        assert!(renderer.ibl.job.is_none() && renderer.environment_preparations() == 0, "nothing could be submitted");
        assert!(renderer.environment_pending(), "wanted, not prepared: the host keeps drawing");
        assert!(!renderer.items_ready(&cx));

        drop(held);
        renderer.resolve_ibl(&mut cx, &env);
        assert_eq!(renderer.environment_preparations(), 1, "the retry submitted it");
        settle(&mut renderer, &mut cx, &env);
        assert!(renderer.environment_ready() && !renderer.environment_pending() && renderer.items_ready(&cx));
    }

    /// Nothing to wait for when nothing will come: an index past the table,
    /// a handle nobody registered, a closed pool (prepared on the spot).
    #[test]
    fn items_do_not_wait_for_an_environment_that_cannot_prepare() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        renderer.resolve_ibl(&mut cx, &env_of(IblSource::Procedural(999), 1.0, 0.0));
        assert!(!renderer.environment_pending() && renderer.items_ready(&cx));
        renderer.resolve_ibl(&mut cx, &env_of(IblSource::Hdri(TextureRef(77)), 1.0, 0.0));
        assert!(!renderer.environment_pending() && renderer.items_ready(&cx));

        use makepad_draw::makepad_platform::thread::ShutdownMode;
        let mut closed = Cx::new(Box::new(|_, _| {}));
        closed.task_pool().close(ShutdownMode::CancelPending);
        renderer.register_environment(TextureRef(3), grey(16, 0.5));
        renderer.resolve_ibl(&mut closed, &env_of(IblSource::Hdri(TextureRef(3)), 1.0, 0.0));
        assert!(renderer.environment_ready() && !renderer.environment_pending() && renderer.items_ready(&closed));
    }

    /// A frame that records maps the scene reads (motion3d's contact and id
    /// maps, each with a world of its own and so the default environment)
    /// before the scene's own draw: those draws must not drop what the
    /// scene's draw prepares. Resolved like the scene's, the first map of
    /// every recording cancelled the job and cleared the textures, so
    /// nothing was ever adopted and a locked-time host waited for ever.
    #[test]
    #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
    fn aux_draws_do_not_drop_the_scenes_environment() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        renderer.register_environment(TextureRef(1), grey(16, 0.25));
        let env = env_of(IblSource::Hdri(TextureRef(1)), 1.0, 0.0);
        let none = Environment::default();
        // One recording: the two maps, then the scene.
        let record = |renderer: &mut Renderer, cx: &mut Cx| {
            renderer.resolve_ibl_for(cx, &none, EnvScope::Aux);
            renderer.resolve_ibl_for(cx, &none, EnvScope::Aux);
            renderer.resolve_ibl_for(cx, &env, EnvScope::Scene);
        };
        record(&mut renderer, &mut cx);
        assert!(renderer.environment_pending() && renderer.ibl.job.is_some(), "the scene's draw submitted the job");
        let start = std::time::Instant::now();
        while renderer.environment_pending() {
            assert!(start.elapsed().as_secs() < 60, "the environment never landed: the maps' draws discard it");
            std::thread::sleep(std::time::Duration::from_millis(2));
            record(&mut renderer, &mut cx);
        }
        assert_eq!(renderer.environment_preparations(), 1, "every recording named the same environment: one preparation");
        assert!(renderer.environment_ready() && renderer.items_ready(&cx));

        // Landed: a map's draw leaves it, and shows what is bound.
        renderer.resolve_ibl_for(&mut cx, &none, EnvScope::Aux);
        assert!(renderer.environment_ready() && renderer.ibl_texture().is_some() && renderer.ibl_sh9().is_some());
        record(&mut renderer, &mut cx);
        assert_eq!(renderer.environment_preparations(), 1);
        assert!(!renderer.environment_pending() && renderer.items_ready(&cx));

        // The scene's own draw with no environment still drops it.
        renderer.resolve_ibl_for(&mut cx, &none, EnvScope::Scene);
        assert!(renderer.ibl_texture().is_none() && !renderer.environment_ready() && !renderer.environment_pending());
    }

    /// A finished preparation that waits for the scene's next draw to adopt
    /// it is not thrown away by a map's draw in between.
    #[test]
    #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
    fn an_aux_draw_keeps_a_finished_preparation_for_the_scenes_next_draw() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        renderer.register_environment(TextureRef(1), grey(16, 0.25));
        let env = env_of(IblSource::Hdri(TextureRef(1)), 1.0, 0.0);
        renderer.resolve_ibl_for(&mut cx, &env, EnvScope::Scene);
        let start = std::time::Instant::now();
        while !renderer.ibl.job.as_ref().is_some_and(|j| j.handle.is_finished()) {
            assert!(start.elapsed().as_secs() < 180, "the preparation did not finish within 180 s");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        renderer.resolve_ibl_for(&mut cx, &Environment::default(), EnvScope::Aux);
        assert!(renderer.ibl.job.is_some() && renderer.environment_pending(), "the finished job is still there to adopt");
        renderer.resolve_ibl_for(&mut cx, &env, EnvScope::Scene);
        assert!(renderer.environment_ready() && !renderer.environment_pending());
        assert_eq!(renderer.environment_preparations(), 1);
    }

    /// What a sibling test module (C6's fog, C5's rig) feeds in answers the
    /// numeric accessors with no pool, no device and no texture.
    #[test]
    fn hand_fed_numbers_answer_the_accessors_with_nothing_bound() {
        let mut renderer = Renderer::default();
        assert!(renderer.ibl_sh9().is_none() && renderer.ibl_mean_luminance().is_none() && renderer.ibl_horizon_rgb().is_none());
        let mut sh = [[0.0f32; 3]; 9];
        sh[0] = [0.5, 0.6, 0.7];
        renderer.feed_environment_numbers_for_tests(sh, 0.4, vec3f(0.1, 0.2, 0.3));
        assert_eq!(renderer.ibl_sh9(), Some(&sh));
        assert_eq!(renderer.ibl_mean_luminance(), Some(0.4));
        assert_eq!(renderer.ibl_horizon_rgb(), Some(vec3f(0.1, 0.2, 0.3)));
        assert!(renderer.ibl_texture().is_none() && !renderer.environment_ready() && renderer.ibl_sun().is_none());
    }

    /// The dome is sampled through atan2, whose cut sits at +Z (u jumps by
    /// a whole turn there). Only a mip-less texture with a manually wrapped
    /// lookup hides that cut (the pattern of
    /// `sky_lane_tests::the_longitude_cut_jumps_by_whole_periods`); the
    /// format assert in the first test pins "no mips", this pins the cut.
    #[test]
    fn the_equirect_cut_jumps_by_a_whole_turn() {
        use makepad_render_material::ibl::dir_to_equirect_uv;
        let l = dir_to_equirect_uv([1.0e-4, 0.0, 1.0]);
        let r = dir_to_equirect_uv([-1.0e-4, 0.0, 1.0]);
        assert!(((l[0] - r[0]).abs() - 1.0).abs() < 1.0e-3, "u jumps by one period across +Z: only a mip-less, manually wrapped lookup hides it");
        // Away from the cut the mapping is smooth.
        let a = dir_to_equirect_uv([0.0, 0.0, -1.0]);
        let b = dir_to_equirect_uv([0.017, 0.0, -1.0]);
        assert!((a[0] - b[0]).abs() < 0.01);
    }

    /// The engine's seven presets keep their indices; the hdri presets
    /// follow them by name, and every name resolves.
    #[test]
    fn procedural_indices_past_the_engine_presets_name_the_hdri_presets() {
        let hdri = crate::hdri::presets::PRESET_NAMES;
        assert_eq!(PROCEDURAL_ENVIRONMENTS.len(), 7 + hdri.len());
        assert_eq!(&PROCEDURAL_ENVIRONMENTS[..7], &["studio", "softbox", "sunset", "overcast", "night", "neon", "gradient"]);
        for (i, name) in PROCEDURAL_ENVIRONMENTS.iter().enumerate() {
            assert_eq!(procedural_environment_name(i as u32), Some(*name));
            if i < 7 {
                assert!(EnvPreset::by_name(name).is_some(), "{name}");
            } else {
                let hdri_name = name.strip_prefix(HDRI_PREFIX).expect("hdri_ prefix");
                assert!(crate::hdri::presets::preset(hdri_name).is_some(), "{name} -> {hdri_name}");
                assert_eq!(*name, hdri_procedural_name(hdri[i - 7]), "{name} is PRESET_NAMES[{}] lowered", i - 7);
            }
        }
        assert_eq!(procedural_environment_name(PROCEDURAL_ENVIRONMENTS.len() as u32), None);
        assert_eq!(hdri_procedural_name("Three-point"), "hdri_three_point");
        assert_eq!(hdri_procedural_name("Clear noon"), "hdri_clear_noon");
        let unique: std::collections::HashSet<&str> = PROCEDURAL_ENVIRONMENTS.iter().copied().collect();
        assert_eq!(unique.len(), PROCEDURAL_ENVIRONMENTS.len(), "no duplicate names");
    }

    /// A procedural hdri preset bakes on the job (1024 x 512, its own sun)
    /// and prepares like a registered map. Release only: a debug bake of
    /// half a million texels through the atmosphere takes too long.
    #[test]
    #[ignore]
    fn a_procedural_hdri_preset_bakes_on_the_job_and_reports_its_sun() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        let env = env_of(IblSource::Procedural(7), 1.0, 0.0);
        settle(&mut renderer, &mut cx, &env);
        assert_eq!(renderer.environment_preparations(), 1);
        let dome = renderer.ibl_dome_texture().cloned().unwrap();
        assert_eq!(size_of(&dome, &mut cx), (PROCEDURAL_HDRI_WIDTH, PROCEDURAL_HDRI_WIDTH / 2));
        let sun = renderer.ibl_sun().expect("clear noon has a sun");
        assert!(sun.dir.y > 0.5, "{sun:?}");
        assert!(renderer.ibl_mean_luminance().unwrap() > 0.0);
        // Nobody declared that sun, so the job filled the preset's own: the
        // SH toward it holds the sky, not the disc (the directional light
        // carries the disc, renderer/env_sun.rs).
        let e = makepad_render_material::ibl::sh9_irradiance(renderer.ibl_sh9().unwrap(), crate::hdri::arr(sun.dir));
        let disc = crate::sky::luminance(sun.irradiance());
        assert!(crate::sky::luminance(vec3f(e[0], e[1], e[2])) < 0.5 * disc, "the baked sun is not in the SH: E {e:?} vs the disc's {disc}");
        // An index past the table builds nothing.
        renderer.resolve_ibl(&mut cx, &env_of(IblSource::Procedural(999), 1.0, 0.0));
        assert!(renderer.ibl_texture().is_none() && !renderer.environment_pending());
    }

    /// The quad's two instance vectors: the lane switch and the meter,
    /// decided on the CPU so the shader is one branch per pixel.
    #[test]
    fn the_background_controls_pick_the_lane_the_meter_and_the_dome() {
        let mut proj = Mat4f::identity();
        proj.v[0] = 2.0;
        proj.v[5] = 4.0;
        // HDR lane: the lane flag 1 (linear out at the map's own scale x
        // Ibl.intensity, which the shader reads from meta texel 9; never
        // HDR_SKY_GAIN, the analytic sky's), no exposure of its own (the
        // composite's), the dome's logical size.
        let (bg, bg2) = env_dome_controls(0.3, 1.5, &proj, true, 0.25, 1.0, Some((2048, 1024)));
        assert_eq!(bg, vec4(0.3, 1.5, 0.5, 0.25));
        assert_eq!(bg2, vec4(0.0, 1.0, 2048.0, 1024.0));
        // Legacy lane: exposure metered on the luminance the dome shows (the
        // caller passes the map's mean x Ibl.intensity), times
        // 2^exposure_ev; lane flag 0 (the shader tone maps).
        let (_, bg2) = env_dome_controls(0.0, 1.0, &proj, false, 0.25, 1.0, None);
        assert!((bg2.x - crate::sky::EXPOSURE_KEY / 0.25 * 2.0).abs() < 1.0e-5, "{bg2:?}");
        assert_eq!((bg2.y, bg2.z, bg2.w), (0.0, 0.0, 0.0), "no dome: the atlas path");
        // A black map meters at the floor, not at infinity; NaN clamps.
        let (_, floor) = env_dome_controls(0.0, 1.0, &proj, false, 0.0, 0.0, None);
        assert!((floor.x - crate::sky::EXPOSURE_KEY / crate::sky::EXPOSURE_LUMINANCE_FLOOR).abs() < 1.0e-4);
        let (bg, bg2) = env_dome_controls(f32::NAN, -2.0, &proj, false, f32::NAN, f32::NAN, Some((4, 2)));
        assert_eq!((bg.x, bg.y), (0.0, 0.0), "blur and intensity are clamped");
        assert!(bg2.x.is_finite() && bg2.x > 0.0);
        // A degenerate projection still gives a finite ray scale.
        let (bg, _) = env_dome_controls(0.0, 1.0, &Mat4f::default(), true, 1.0, 0.0, None);
        assert_eq!((bg.z, bg.w), (1.0, 1.0));
    }

    /// The background is not the sky until the full-resolution dome is up;
    /// the accessor the frame gates on says so, and an MR stage never
    /// shows an environment.
    #[test]
    fn the_dome_replaces_the_sky_only_once_it_is_ready_and_never_in_mr() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        assert!(!renderer.environment_dome_ready());
        renderer.register_environment(TextureRef(3), grey(16, 0.4));
        let env = env_of(IblSource::Hdri(TextureRef(3)), 1.0, 0.0);
        renderer.resolve_ibl(&mut cx, &env);
        assert!(!renderer.environment_dome_ready(), "preparing: the analytic dome still draws");
        settle(&mut renderer, &mut cx, &env);
        assert!(renderer.environment_dome_ready());
        assert!(renderer.stage.shows_environment(), "a flat stage shows it");
        renderer.stage.mode = crate::stage::StageMode::MrDiorama;
        assert!(!renderer.stage.shows_environment(), "MR: the room is the horizon");
        // The quad's dome size is the preparation's LOGICAL size (what the
        // shader addresses texels in), not a texture allocation's.
        assert_eq!(renderer.ibl.dome_size, (16, 8));
    }

    /// The shader's `env_texel`, transcribed line for line (Step 3): texel
    /// coordinates in the LOGICAL size (`bg2.zw`), u wrapped, v clamped,
    /// texel centres; only the final sample is normalised by the
    /// allocation (`env_tex.size()`), then read back the way
    /// `sample_nearest` reads a texel.
    fn env_texel_cpu(alloc: &[[f32; 4]], alloc_w: usize, alloc_h: usize, w: f32, h: f32, uv: [f32; 2]) -> [f32; 3] {
        let (sx, sy) = (alloc_w as f32, alloc_h as f32);
        let nearest = |u: f32, v: f32| {
            let x = ((u * sx).floor() as usize).min(alloc_w - 1);
            let y = ((v * sy).floor() as usize).min(alloc_h - 1);
            alloc[y * alloc_w + x]
        };
        let mix = |a: [f32; 4], b: [f32; 4], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, 1.0];
        let px = uv[0] * w - 0.5;
        let py = (uv[1] * h - 0.5).clamp(0.0, h - 1.0);
        let (x0, y0) = (px.floor(), py.floor());
        let (fx, fy) = (px - x0, py - y0);
        let xa = (x0 - (x0 / w).floor() * w + 0.5) / sx;
        let xb = (x0 + 1.0 - ((x0 + 1.0) / w).floor() * w + 0.5) / sx;
        let ya = (y0 + 0.5) / sy;
        let yb = ((y0 + 1.0).min(h - 1.0) + 0.5) / sy;
        let a = mix(nearest(xa, ya), nearest(xb, ya), fx);
        let b = mix(nearest(xa, yb), nearest(xb, yb), fx);
        let c = mix(a, b, fy);
        [c[0], c[1], c[2]]
    }

    /// The dome is a VecRGBAf32 texture. The old D3D11 and desktop GL
    /// backends allocated such textures with spare rows (`ceil128(max(3 h,
    /// 512))`) until makepad 9e9a7ee9b, and `size()` reported the
    /// allocation; the HAL allocates every texture at exactly its size. The
    /// lookup addresses texels in the logical size and normalises only the
    /// sample, so it reads the map whatever the allocation: over one whose
    /// spare rows are NaN it reads exactly the map, `EnvMap::sample_uv`,
    /// finite, at the seam, the poles and in between.
    #[test]
    fn the_dome_lookup_never_reads_the_padding_of_a_float_allocation() {
        let map = EnvMap::from_fn(32, |d| [0.5 + 0.5 * d[0], 0.5 + 0.5 * d[1], 0.5 + 0.5 * d[2]]);
        let (w, h) = (map.width, map.height);
        let alloc_h = (3 * h).max(512).div_ceil(128) * 128;
        let mut alloc = map.data.clone();
        alloc.resize(w * alloc_h, [f32::NAN; 4]);
        for uv in [[0.5f32, 0.5f32], [0.001, 0.02], [0.999, 0.98], [0.0, 0.0], [0.75, 1.0], [0.37, 0.61], [0.016, 0.5]] {
            let got = env_texel_cpu(&alloc, w, alloc_h, w as f32, h as f32, uv);
            let want = map.sample_uv(uv);
            for k in 0..3 {
                assert!(got[k].is_finite() && (got[k] - want[k]).abs() < 1.0e-5, "uv {uv:?}: {got:?} vs {want:?}");
            }
        }
        // The same lookup with size() taken as the map's size (no padding:
        // every backend's allocation now) is the same texels.
        let unpadded = env_texel_cpu(&map.data, w, h, w as f32, h as f32, [0.37, 0.61]);
        let want = map.sample_uv([0.37, 0.61]);
        assert!((0..3).all(|k| (unpadded[k] - want[k]).abs() < 1.0e-5));
        // The controls carry the logical size the lookup needs.
        let (_, bg2) = env_dome_controls(0.0, 1.0, &Mat4f::identity(), true, 1.0, 0.0, Some((w, h)));
        assert_eq!((bg2.z, bg2.w), (w as f32, h as f32));
    }

    /// A key the world declares on one frame: a sun at `az` degrees.
    fn key_at(az: f32) -> EnvSun {
        let cos_radius = 2.0f32.to_radians().cos();
        EnvSun { dir: crate::hdri::dir_from_az_el(az, 40.0), radiance: vec3f(50.0, 50.0, 50.0), cos_radius, facing: 1.0, cos_cover: cos_radius }
    }

    /// One adoption seen while frames run: the adopted preparation's
    /// generation and key, the bound meter, and the key the world declared
    /// on the frame it landed.
    struct Landing {
        generation: u64,
        key: PrepareKey,
        mean: f32,
        declared: PrepareKey,
    }

    /// Run frames, each declaring a new key (the sun a tenth of a degree
    /// further), until `n` more preparations were adopted.
    #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
    fn frames_until_landed(renderer: &mut Renderer, cx: &mut Cx, env: &mut Environment, frame: &mut u32, n: usize) -> Vec<Landing> {
        let start = std::time::Instant::now();
        let mut landed = Vec::new();
        let mut last = renderer.ibl.bound.map(|b| b.1);
        while landed.len() < n {
            assert!(start.elapsed().as_secs() < 180, "{} of {n} preparations landed in 180 s while the key changed every frame", landed.len());
            *frame += 1;
            env.sun = Some(key_at(*frame as f32 * 0.1));
            renderer.resolve_ibl(cx, env);
            if let Some((key, generation)) = renderer.ibl.bound {
                if last != Some(generation) {
                    let source = env.ibl.unwrap().source;
                    landed.push(Landing { generation, key, mean: renderer.ibl_mean_luminance().unwrap(), declared: PrepareKey { source, sun: env.sun, daylight_sun: env.daylight_sun } });
                    last = Some(generation);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        landed
    }

    /// K1: preparations land at the renderer's pace. A host whose declared
    /// key changes every frame (a running clock) does not restart the job:
    /// the job in flight runs to its end, its result is adopted although
    /// the world has moved on meanwhile (it is newer than what is bound),
    /// then the latest key prepares; one job at a time, so the light and
    /// the sky are at most one preparation behind. A new map under the
    /// handle is a change of map: the job in flight is cancelled and
    /// nothing of the replaced map lands after it. Once the key holds, the
    /// last one lands.
    #[test]
    fn a_key_that_changes_every_frame_lands_at_the_renderers_pace() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        renderer.register_environment(TextureRef(5), grey(16, 0.25));
        let mut env = env_of(IblSource::Hdri(TextureRef(5)), 1.0, 0.0);
        let mut frame = 0u32;
        let landed = frames_until_landed(&mut renderer, &mut cx, &mut env, &mut frame, 3);
        for (i, l) in landed.iter().enumerate() {
            assert!(i == 0 || l.generation > landed[i - 1].generation, "each adopted preparation is newer than the last");
            assert_ne!(l.key, l.declared, "adopted although the world declared another key by then");
            assert!((l.mean - 0.25).abs() < 1.0e-3, "the one map: {}", l.mean);
        }
        assert!(
            renderer.environment_preparations() <= landed.last().unwrap().generation + 1,
            "one job at a time and none restarted: {} submitted for {} frames",
            renderer.environment_preparations(),
            frame
        );

        // A new map while the latest key prepares: that job (the last one
        // submitted) is cancelled and only the new map lands from here on.
        let cancel = renderer.ibl.job.as_ref().map(|j| j.cancel.clone()).expect("the latest key prepares");
        let replaced = renderer.environment_preparations();
        renderer.register_environment(TextureRef(5), grey(16, 0.75));
        assert!(cancel.is_cancelled(), "a new map cancels the job in flight");
        for l in frames_until_landed(&mut renderer, &mut cx, &mut env, &mut frame, 2) {
            assert!(l.generation > replaced, "the replaced map's job never lands");
            assert!((l.mean - 0.75).abs() < 1.0e-3, "only the new map lands: {}", l.mean);
        }

        // The key holds: the last declared one lands.
        settle(&mut renderer, &mut cx, &env);
        assert_eq!(renderer.ibl.bound.map(|b| b.0), Some(PrepareKey { source: IblSource::Hdri(TextureRef(5)), sun: env.sun, daylight_sun: env.daylight_sun }));
        assert!(!renderer.environment_pending());
    }

    /// K1: only a change of map cancels the job in flight (a new map under
    /// the handle, another environment named; an unregister, below). A job
    /// that already finished when its map was replaced is still taken, and
    /// the check at adoption rejects it: nothing of a replaced map is ever
    /// bound.
    #[test]
    #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
    fn a_change_of_map_cancels_the_running_job_and_never_adopts_its_result() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        let env = env_of(IblSource::Hdri(TextureRef(6)), 1.0, 0.0);
        renderer.register_environment(TextureRef(6), grey(16, 0.25));
        settle(&mut renderer, &mut cx, &env);
        let mean = |r: &Renderer| r.ibl_mean_luminance().unwrap();

        // A new map while a job runs.
        renderer.register_environment(TextureRef(6), grey(16, 0.5));
        renderer.resolve_ibl(&mut cx, &env);
        let cancel = renderer.ibl.job.as_ref().map(|j| j.cancel.clone()).expect("the new map prepares");
        renderer.register_environment(TextureRef(6), grey(16, 0.75));
        assert!(cancel.is_cancelled(), "a new map cancels the job in flight");
        settle(&mut renderer, &mut cx, &env);
        assert!((mean(&renderer) - 0.75).abs() < 1.0e-3, "{}", mean(&renderer));

        // A job that finished before its map was replaced: taken, rejected.
        renderer.register_environment(TextureRef(6), grey(16, 0.1));
        renderer.resolve_ibl(&mut cx, &env);
        let start = std::time::Instant::now();
        while !renderer.ibl.job.as_ref().is_some_and(|j| j.handle.is_finished()) {
            assert!(start.elapsed().as_secs() < 180, "the preparation did not finish within 180 s");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let bound = renderer.ibl.bound;
        renderer.register_environment(TextureRef(6), grey(16, 0.9));
        renderer.resolve_ibl(&mut cx, &env);
        assert_eq!(renderer.ibl.bound, bound, "the replaced map's finished preparation is not adopted");
        assert!((mean(&renderer) - 0.75).abs() < 1.0e-3, "the bound numbers stay: {}", mean(&renderer));
        assert!(renderer.environment_pending(), "the new map prepares");
        settle(&mut renderer, &mut cx, &env);
        assert!((mean(&renderer) - 0.9).abs() < 1.0e-3, "{}", mean(&renderer));

        // Another environment named while a job runs.
        renderer.register_environment(TextureRef(7), grey(16, 0.6));
        renderer.resolve_ibl(&mut cx, &env_of(IblSource::Hdri(TextureRef(7)), 1.0, 0.0));
        let cancel = renderer.ibl.job.as_ref().map(|j| j.cancel.clone()).expect("7 prepares");
        renderer.resolve_ibl(&mut cx, &env);
        assert!(cancel.is_cancelled(), "naming another environment cancels the job in flight");
        assert!(!renderer.environment_pending(), "6 is bound: nothing to wait for while 7's job winds down");
        settle(&mut renderer, &mut cx, &env);
        assert!((mean(&renderer) - 0.9).abs() < 1.0e-3, "7 never lands: {}", mean(&renderer));
    }

    /// Wait until the job in flight has finished (without taking it).
    #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
    fn until_finished(renderer: &Renderer) {
        let start = std::time::Instant::now();
        while !renderer.ibl.job.as_ref().is_some_and(|j| j.handle.is_finished()) {
            assert!(start.elapsed().as_secs() < 180, "the preparation did not finish within 180 s");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// K7 (K1 for a host that re-bakes its map): a clock that re-bakes the
    /// registered environment faster than the renderer prepares it (the
    /// sandbox's day cycle registers a new bake every quarter hour of game
    /// time, every 7.5 minutes while the sun is low; every 0.3 s on a 30 s
    /// day, half that then) lands at the renderer's pace. A
    /// re-bake is not a change of map: the job in flight runs on and its
    /// bake is adopted, newer than what is bound, although newer bakes are
    /// registered by then; then the newest bake prepares, and the ones in
    /// between never do. Every bake here declares the same key (none), so
    /// the key alone cannot tell the bound bake from the newest. A change of
    /// map after re-bakes still cancels.
    #[test]
    fn a_rebaked_map_lands_at_the_renderers_pace() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        let env = env_of(IblSource::Hdri(TextureRef(10)), 1.0, 0.0);
        let mean = |r: &Renderer| r.ibl_mean_luminance().unwrap_or(-1.0);
        renderer.register_generated_environment(TextureRef(10), grey(16, 0.25), None);
        renderer.resolve_ibl(&mut cx, &env);
        let cancel = renderer.ibl.job.as_ref().map(|j| j.cancel.clone()).expect("the first bake prepares");
        // Two re-bakes while it prepares.
        let (between, newest) = (grey(16, 0.5), grey(16, 0.75));
        renderer.register_rebaked_environment(TextureRef(10), between.clone(), None);
        renderer.resolve_ibl(&mut cx, &env);
        renderer.register_rebaked_environment(TextureRef(10), newest.clone(), None);
        renderer.resolve_ibl(&mut cx, &env);
        assert!(!cancel.is_cancelled(), "a re-bake does not cancel the job in flight");
        assert_eq!(renderer.environment_preparations(), 1, "one job at a time");
        until_finished(&renderer);
        renderer.resolve_ibl(&mut cx, &env);
        assert!((mean(&renderer) - 0.25).abs() < 1.0e-3, "the first bake lands although two newer ones are registered: {}", mean(&renderer));
        assert!(renderer.environment_pending(), "the newest bake prepares next");
        assert_eq!(renderer.environment_preparations(), 2);
        let preparing = renderer.ibl.job.as_ref().and_then(|j| j.map.as_ref()).map(|m| m.map.clone()).expect("a job for the newest bake");
        assert!(Arc::ptr_eq(&preparing, &newest), "the newest bake, not the one in between");
        drop(preparing);
        settle(&mut renderer, &mut cx, &env);
        assert!((mean(&renderer) - 0.75).abs() < 1.0e-3, "{}", mean(&renderer));
        assert!(!renderer.environment_pending());
        assert_eq!(renderer.environment_preparations(), 2, "the bake in between never prepared");
        assert_eq!(Arc::strong_count(&between), 1, "and nothing holds it");

        // A change of map after a re-bake still cancels: the re-bake's job
        // never lands, the new map does.
        renderer.register_rebaked_environment(TextureRef(10), grey(16, 0.1), None);
        renderer.resolve_ibl(&mut cx, &env);
        let cancel = renderer.ibl.job.as_ref().map(|j| j.cancel.clone()).expect("the re-bake prepares");
        renderer.register_generated_environment(TextureRef(10), grey(16, 0.9), None);
        assert!(cancel.is_cancelled(), "a change of map cancels the re-bake's job");
        settle(&mut renderer, &mut cx, &env);
        assert!((mean(&renderer) - 0.9).abs() < 1.0e-3, "{}", mean(&renderer));
        // A finished job of a bake whose map was then replaced is rejected.
        renderer.register_rebaked_environment(TextureRef(10), grey(16, 0.3), None);
        renderer.resolve_ibl(&mut cx, &env);
        until_finished(&renderer);
        renderer.register_generated_environment(TextureRef(10), grey(16, 0.6), None);
        renderer.resolve_ibl(&mut cx, &env);
        assert!((mean(&renderer) - 0.9).abs() < 1.0e-3, "the replaced bake is not adopted: {}", mean(&renderer));
        settle(&mut renderer, &mut cx, &env);
        assert!((mean(&renderer) - 0.6).abs() < 1.0e-3, "{}", mean(&renderer));
        // With nothing registered under the handle a re-bake is a first registration.
        renderer.unregister_environment(TextureRef(10));
        renderer.register_rebaked_environment(TextureRef(10), grey(16, 0.4), None);
        settle(&mut renderer, &mut cx, &env);
        assert!((mean(&renderer) - 0.4).abs() < 1.0e-3, "{}", mean(&renderer));
    }

    /// M3: registering the map that is registered (the same `Arc`) is no
    /// change: the job in flight runs on and a landed preparation stays.
    #[test]
    fn the_identical_map_registered_again_does_not_restart() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        let map = grey(16, 0.25);
        let env = env_of(IblSource::Hdri(TextureRef(8)), 1.0, 0.0);
        renderer.register_environment(TextureRef(8), map.clone());
        renderer.resolve_ibl(&mut cx, &env);
        let cancel = renderer.ibl.job.as_ref().map(|j| j.cancel.clone()).expect("preparing");
        renderer.register_environment(TextureRef(8), map.clone());
        renderer.resolve_ibl(&mut cx, &env);
        assert!(!cancel.is_cancelled() && renderer.ibl_job_in_flight(), "the same job runs on");
        assert_eq!(renderer.environment_preparations(), 1);
        settle(&mut renderer, &mut cx, &env);
        let lane = renderer.ibl_texture().unwrap().texture_id();
        for _ in 0..3 {
            renderer.register_environment(TextureRef(8), map.clone());
            renderer.resolve_ibl(&mut cx, &env);
        }
        assert!(!renderer.environment_pending());
        assert_eq!(renderer.environment_preparations(), 1);
        assert_eq!(renderer.ibl_texture().unwrap().texture_id(), lane);
    }

    /// M8: unregistering drops the map and any preparation of it: nothing
    /// of it stays bound, the renderer keeps no reference to it (the host's
    /// `Arc` is the last), and a world that still names it names an unknown
    /// source (nothing prepares, nothing is awaited). A job in flight for it
    /// is cancelled and never lands.
    #[test]
    #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
    fn unregistering_releases_the_map_and_any_preparation_of_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        let map = grey(16, 0.25);
        let env = env_of(IblSource::Hdri(TextureRef(9)), 1.0, 0.0);
        renderer.register_environment(TextureRef(9), map.clone());
        settle(&mut renderer, &mut cx, &env);
        assert!(renderer.environment_ready());
        renderer.unregister_environment(TextureRef(9));
        assert!(renderer.ibl.maps.is_empty());
        assert!(renderer.ibl_texture().is_none() && renderer.ibl_dome_texture().is_none() && renderer.ibl_sh9().is_none());
        assert!(!renderer.environment_ready() && !renderer.environment_pending() && renderer.items_ready(&cx));
        assert_eq!(Arc::strong_count(&map), 1, "the renderer keeps no reference");
        for _ in 0..3 {
            renderer.resolve_ibl(&mut cx, &env);
        }
        assert_eq!(renderer.environment_preparations(), 1, "an unknown source prepares nothing");
        assert!(!renderer.environment_pending() && renderer.ibl_texture().is_none());

        // While a job prepares it.
        let map = grey(16, 0.75);
        renderer.register_environment(TextureRef(9), map.clone());
        renderer.resolve_ibl(&mut cx, &env);
        let cancel = renderer.ibl.job.as_ref().map(|j| j.cancel.clone()).expect("preparing");
        renderer.unregister_environment(TextureRef(9));
        assert!(cancel.is_cancelled(), "the job in flight is cancelled");
        assert!(!renderer.environment_pending());
        let start = std::time::Instant::now();
        while renderer.ibl_job_in_flight() {
            assert!(start.elapsed().as_secs() < 180, "the cancelled job did not wind down within 180 s");
            std::thread::sleep(std::time::Duration::from_millis(2));
            renderer.resolve_ibl(&mut cx, &env);
        }
        assert!(renderer.ibl_texture().is_none() && renderer.ibl_sh9().is_none(), "it never lands");
        assert_eq!(Arc::strong_count(&map), 1);
        // A handle nobody registered: nothing happens.
        renderer.unregister_environment(TextureRef(42));
    }

    /// K1: the job asks between stages: right after its bake (a procedural
    /// index) and then inside the preparation (hdri/prepare.rs); told to
    /// stop it builds nothing.
    #[test]
    fn a_build_asks_to_stop_after_its_bake() {
        let serial = |n: usize, f: &(dyn Fn(usize) + Sync)| {
            for i in 0..n {
                f(i);
            }
        };
        let key = PrepareKey { source: IblSource::Procedural(0), sun: None, daylight_sun: None };
        let asked = std::cell::Cell::new(0usize);
        let at_once = || {
            asked.set(asked.get() + 1);
            true
        };
        assert!(build_ibl(1, key, None, 1.0, 0.0, &serial, SYNC_SIZES, SYNC_HDRI_WIDTH, &at_once).is_none());
        assert_eq!(asked.get(), 1, "asked once, right after the bake");
        asked.set(0);
        let never = || {
            asked.set(asked.get() + 1);
            false
        };
        assert!(build_ibl(1, key, None, 1.0, 0.0, &serial, SYNC_SIZES, SYNC_HDRI_WIDTH, &never).is_some());
        assert!(asked.get() > 1, "and again inside the preparation");
    }

    /// N1: a procedural hdri preset reports its sky's own sun for the
    /// daylight switches, and it binds with the preparation, as the preset's
    /// key does: the moonlit preset's sun is 30 degrees down while its key
    /// (the moon) is up. A sun the world reports wins over the preset's, an
    /// engine look has none, and an unbound renderer reports none.
    #[test]
    fn a_procedural_presets_own_sun_binds_with_its_preparation() {
        let serial = |n: usize, f: &(dyn Fn(usize) + Sync)| {
            for i in 0..n {
                f(i);
            }
        };
        let moonlit = PROCEDURAL_ENVIRONMENTS.iter().position(|n| *n == "hdri_moonlit_night").unwrap() as u32;
        let key = PrepareKey { source: IblSource::Procedural(moonlit), sun: None, daylight_sun: None };
        let built = build_ibl(1, key, None, 1.0, 0.0, &serial, SYNC_SIZES, 32, &|| false).expect("built");
        let daylight = built.baked_daylight.expect("a sky reports its own sun");
        assert!(daylight.y < -0.4 && built.baked_sun.is_some_and(|k| k.dir.y > 0.4), "{daylight:?} {:?}", built.baked_sun);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        assert_eq!(renderer.ibl_daylight_sun(), None, "nothing bound");
        renderer.adopt_ibl(&mut cx, built);
        assert_eq!(renderer.ibl_daylight_sun(), Some(daylight));
        let reported = vec3f(0.0, -1.0, 0.0);
        let key = PrepareKey { daylight_sun: Some(reported), ..key };
        renderer.adopt_ibl(&mut cx, build_ibl(2, key, None, 1.0, 0.0, &serial, SYNC_SIZES, 32, &|| false).unwrap());
        assert_eq!(renderer.ibl_daylight_sun(), Some(reported), "the world's report wins");
        renderer.unbind_ibl();
        assert_eq!(renderer.ibl_daylight_sun(), None);
        let look = PrepareKey { source: IblSource::Procedural(0), sun: None, daylight_sun: None };
        assert_eq!(build_ibl(3, look, None, 1.0, 0.0, &serial, SYNC_SIZES, 32, &|| false).unwrap().baked_daylight, None);
    }

    /// N3: a generated map's lighting copy is the map without exactly its
    /// key. A procedural preset bakes its keyless copy beside its map; a host
    /// registers one with `register_generated_environment` and declares the
    /// key it was made without. Either way the SH and the meter are the
    /// keyless map's while that key lights (the Overcast dome's side panels
    /// stay, under the cover the cone fill would take), and the full map's
    /// with no key. A key the map has no keyless copy for (a file's detected
    /// sun, or a world that declares another key over a preset) still has
    /// its covering cone filled.
    #[test]
    fn a_generated_map_lights_from_its_keyless_copy() {
        use crate::hdri::envmap::{bake_generated_env_map, remove_sun};
        use makepad_render_material::ibl::sh9;
        let serial = |n: usize, f: &(dyn Fn(usize) + Sync)| {
            for i in 0..n {
                f(i);
            }
        };
        let dome = PROCEDURAL_ENVIRONMENTS.iter().position(|n| *n == "hdri_overcast_dome").unwrap() as u32;
        let g = bake_generated_env_map(&crate::hdri::presets::preset("Overcast dome").unwrap(), 32, serial);
        let (keyless, sun) = (g.keyless.clone().expect("a keyed preset"), g.sun.expect("the dome is the key"));
        let mut filled = g.map.clone();
        remove_sun(&mut filled, &sun);
        assert!(sh9(&filled) != sh9(&keyless), "premise: the cone fill takes more than the key");
        let build = |key: PrepareKey, map: Option<RegisteredMap>| build_ibl(1, key, map, 1.0, 0.0, &serial, SYNC_SIZES, 32, &|| false).expect("built").prepared;

        // A procedural preset: its own key, declared or not, leaves exactly.
        let preset = PrepareKey { source: IblSource::Procedural(dome), sun: None, daylight_sun: None };
        assert_eq!(build(preset, None).sh, sh9(&keyless), "the preset's own key leaves exactly");
        assert_eq!(build(PrepareKey { sun: Some(sun), ..preset }, None).sh, sh9(&keyless), "declared as it is");
        let other = EnvSun { dir: vec3f(1.0, 0.0, 0.0), ..sun };
        let mut other_filled = g.map.clone();
        remove_sun(&mut other_filled, &other);
        assert_eq!(build(PrepareKey { sun: Some(other), ..preset }, None).sh, sh9(&other_filled), "another key: its cone");

        // A registered generated map: the keyless copy while a key is declared.
        let hdri = PrepareKey { source: IblSource::Hdri(TextureRef(5)), sun: Some(sun), daylight_sun: None };
        let generated = RegisteredMap { map: Arc::new(g.map.clone()), keyless: Some(Arc::new(keyless.clone())), bakes: 1 };
        let lit = build(hdri, Some(generated.clone()));
        assert_eq!((lit.sh, lit.mean_luminance), (sh9(&keyless), crate::hdri::image::mean_luminance(&keyless)));
        assert_eq!(lit.dome.len(), 32 * 16 * 4, "the dome is the map");
        assert_eq!(lit.dome[..4], [g.map.data[0][0], g.map.data[0][1], g.map.data[0][2], 1.0]);
        assert_eq!(build(PrepareKey { sun: None, ..hdri }, Some(generated)).sh, sh9(&g.map), "no key: nothing leaves");
        // A plain registration (a file): the declared key's cone is filled.
        let file = RegisteredMap { map: Arc::new(g.map.clone()), keyless: None, bakes: 2 };
        assert_eq!(build(hdri, Some(file)).sh, sh9(&filled));
    }

    /// The keyless copy is registered with its map: the same two `Arc`s again
    /// are no change, a new copy is a change of map (the job in flight is
    /// cancelled), and unregistering releases both.
    #[test]
    #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
    fn a_keyless_copy_registers_and_unregisters_with_its_map() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut renderer = Renderer::default();
        let (map, keyless) = (grey(16, 0.25), grey(16, 0.2));
        let env = env_of(IblSource::Hdri(TextureRef(4)), 1.0, 0.0);
        renderer.register_generated_environment(TextureRef(4), map.clone(), Some(keyless.clone()));
        renderer.resolve_ibl(&mut cx, &env);
        let cancel = renderer.ibl.job.as_ref().map(|j| j.cancel.clone()).expect("preparing");
        renderer.register_generated_environment(TextureRef(4), map.clone(), Some(keyless.clone()));
        renderer.resolve_ibl(&mut cx, &env);
        assert!(!cancel.is_cancelled(), "the same pair: the same job runs on");
        let other = grey(16, 0.1);
        renderer.register_generated_environment(TextureRef(4), map.clone(), Some(other.clone()));
        assert!(cancel.is_cancelled(), "a new keyless copy is a change of map");
        renderer.unregister_environment(TextureRef(4));
        let start = std::time::Instant::now();
        while renderer.ibl_job_in_flight() {
            assert!(start.elapsed().as_secs() < 180, "the cancelled job did not wind down within 180 s");
            std::thread::sleep(std::time::Duration::from_millis(2));
            renderer.resolve_ibl(&mut cx, &env);
        }
        assert!(renderer.ibl.maps.is_empty());
        for (name, a) in [("map", &map), ("first copy", &keyless), ("second copy", &other)] {
            assert_eq!(Arc::strong_count(a), 1, "the renderer keeps no reference to the {name}");
        }
    }

    /// A cancelled job's bake runs no more rows: the row runner the job
    /// hands the bake skips every row once the token is set.
    #[test]
    fn a_cancelled_bake_runs_no_more_rows() {
        let serial = |n: usize, f: &(dyn Fn(usize) + Sync)| {
            for i in 0..n {
                f(i);
            }
        };
        let cancel = CancellationToken::new();
        let rows = rows_until_cancelled(&serial, &cancel);
        let ran = std::sync::atomic::AtomicUsize::new(0);
        rows(10, &|i| {
            ran.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if i == 3 {
                cancel.cancel();
            }
        });
        assert_eq!(ran.load(std::sync::atomic::Ordering::Relaxed), 4, "rows 0..=3 ran, the rest were skipped");
    }

    /// I4: a fork of the live renderer (a split-screen pane, the AI world
    /// review) draws the live renderer's environment: it is born with the
    /// bound preparation mirrored (the textures, the numbers, the key and
    /// the daylight report, so the rig, the meter, the fog and the sky's
    /// switches answer as the live renderer's), its draws prepare nothing
    /// of their own whatever their world names, and mirrored again it
    /// follows the live renderer to its next preparation and to none,
    /// releasing the maps the live renderer let go.
    #[test]
    fn a_fork_draws_the_live_renderers_environment_and_never_prepares_its_own() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut live = Renderer::default();
        let first = grey(32, 0.25);
        live.register_environment(TextureRef(1), first.clone());
        let key = EnvSun { dir: vec3f(0.0, 0.6, -0.8), radiance: vec3f(50.0, 45.0, 40.0), cos_radius: 0.999, facing: 1.0, cos_cover: 0.998 };
        let mut env = env_of(IblSource::Hdri(TextureRef(1)), 1.0, 0.0);
        env.sun = Some(key);
        env.daylight_sun = Some(key.dir);
        settle(&mut live, &mut cx, &env);
        assert!(live.environment_ready() && live.ibl_sun() == Some(key), "premise: the live renderer's environment landed");
        let mut world = World::new();
        world.environment = env;
        let same = |fork: &Renderer, live: &Renderer, at: &str| {
            let id = |t: Option<&Texture>| t.map(|t| t.texture_id());
            assert_eq!(fork.ibl_sh9(), live.ibl_sh9(), "{at}: the SH");
            assert_eq!(fork.environment_ready(), live.environment_ready(), "{at}: ready");
            assert_eq!(fork.environment_dome_ready(), live.environment_dome_ready(), "{at}: the dome ready");
            assert_eq!(id(fork.ibl_texture()), id(live.ibl_texture()), "{at}: the lane texture");
            assert_eq!(id(fork.ibl_dome_texture()), id(live.ibl_dome_texture()), "{at}: the dome texture");
            assert_eq!(fork.ibl.dome_size, live.ibl.dome_size, "{at}: the dome's size");
            assert_eq!(fork.ibl.key, live.ibl.key, "{at}: the meta row's key");
            assert_eq!(fork.ibl.prepared_for, live.ibl.prepared_for, "{at}: what it was prepared for");
            assert_eq!(fork.ibl_mean_luminance(), live.ibl_mean_luminance(), "{at}: the meter");
            assert_eq!(fork.ibl_horizon_rgb(), live.ibl_horizon_rgb(), "{at}: the fog band");
            assert_eq!(fork.ibl_sun(), live.ibl_sun(), "{at}: the key");
            assert_eq!(fork.ibl_daylight_sun(), live.ibl_daylight_sun(), "{at}: the daylight report");
            assert_eq!(fork.env_lighting(&world), live.env_lighting(&world), "{at}: what the rig takes");
            assert_eq!(fork.env_key_is_its_sun(&world), live.env_key_is_its_sun(&world), "{at}: the sky's sun");
            assert_eq!(fork.daylight_dir(&world), live.daylight_dir(&world), "{at}: the daylight switches");
            let maps = |r: &Renderer| {
                let mut m: Vec<_> = r.ibl.maps.iter().map(|(t, m)| (t.0, Arc::as_ptr(&m.map), m.keyless.as_ref().map(Arc::as_ptr))).collect();
                m.sort();
                m
            };
            assert_eq!(maps(fork), maps(live), "{at}: the registered maps");
        };

        let mut fork = live.fork_scene_for_review();
        same(&fork, &live, "forked");
        // The fork's draws resolve nothing: its world names the live
        // renderer's environment, another one, or none.
        for named in [env, env_of(IblSource::Procedural(7), 1.0, 0.0), Environment::default()] {
            for _ in 0..3 {
                fork.resolve_ibl_for(&mut cx, &named, EnvScope::Scene);
            }
        }
        assert_eq!(fork.environment_preparations(), 0, "the fork submitted no preparation");
        assert!(!fork.ibl_job_in_flight() && !fork.environment_pending(), "and awaits none");
        same(&fork, &live, "after the fork's draws");

        // The live renderer moves on to another map, a generated one with its
        // keyless copy: the fork follows once mirrored again (a kept fork
        // mirrors every frame), and lets go of the replaced map.
        let (second, keyless) = (grey(32, 0.5), grey(32, 0.4));
        live.register_generated_environment(TextureRef(1), second.clone(), Some(keyless.clone()));
        settle(&mut live, &mut cx, &env);
        assert_ne!(fork.ibl_sh9(), live.ibl_sh9(), "premise: the live renderer prepared the new map");
        fork.mirror_environment_from(&live);
        same(&fork, &live, "mirrored after the new map");
        assert_eq!(Arc::strong_count(&first), 1, "nobody holds the replaced map");

        // No environment, and the map unregistered: the fork has none either.
        live.resolve_ibl(&mut cx, &Environment::default());
        live.unregister_environment(TextureRef(1));
        fork.mirror_environment_from(&live);
        same(&fork, &live, "mirrored after none");
        assert!(!fork.environment_ready() && fork.ibl_texture().is_none() && fork.ibl_sh9().is_none());
        assert_eq!((Arc::strong_count(&second), Arc::strong_count(&keyless)), (1, 1), "nobody holds the unregistered map or its copy");
        assert_eq!(fork.environment_preparations(), 0);
    }
}
