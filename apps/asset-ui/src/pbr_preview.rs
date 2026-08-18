//! Static PBR preview — MeshView's material-bearing branch.
//!
//! A Hunyuan/TRELLIS painted model arrives as ONE self-contained GLB whose
//! materials carry embedded baseColor / metallicRoughness(+occlusion) /
//! normal textures. Rendering that through the base-color statue path throws
//! the material away; this branch instead reuses the engine's existing glTF
//! PBR path — [`makepad_xr::render::GltfRenderer`] driving makepad-draw's
//! [`DrawPbr`] — so metallic, roughness, occlusion and normal maps light as
//! real materials (GGX/Schlick-Smith direct light, tangent-space normal
//! mapping with authored-or-generated tangents, glTF ORM channel semantics,
//! sRGB base-color decode, hemisphere/env-cube ambient). No second renderer,
//! no Renderer extension.
//!
//! Contract: bytes in, pixels out, immutable and role-addressed by the GLB's
//! own material graph. `base_dir` stays `None` for the whole life of a load,
//! so relative-URI texture resolution — filesystem sibling probing — is
//! impossible by construction; a texture either resolves from an embedded
//! bufferView or the load honestly fails back to the statue path. A channel
//! the material does not declare stays absent: DrawPbr then uses its neutral
//! behavior (geometric normal, factor-only metallic/roughness, occlusion 1)
//! instead of guessing.
//!
//! This file is a child module of mesh_view.rs (declared there via
//! `#[path]`) because main.rs is owned by another lane and must not change.

use makepad_gltf::{load_gltf_from_bytes, LoadedGltf};
use makepad_widgets::*;
use makepad_xr::render::{GltfDrawObject, GltfMaterialState, GltfRenderer};

/// Same fit rule as the statue path: normalize by the LARGEST dimension so
/// wide/flat models don't blow past the view, feet on the ground plane.
const FIT_EXTENT: f32 = 1.75;
/// The statue's presentation yaw, kept so both static branches pose alike.
const FIT_YAW: f32 = 0.35;

const DEFAULT_LIGHT_DIR: Vec3f = Vec3f { x: 0.35, y: 0.8, z: 0.45 };
/// Per-keypress light orbit steps (radians); key repeat makes them smooth.
const LIGHT_AZIMUTH_STEP: f32 = 0.12;
const LIGHT_ELEVATION_STEP: f32 = 0.08;
/// Quarter-stop multiplicative step for exposure / env intensity keys.
const STEP_FACTOR: f32 = 1.189_207_1;
const EXPOSURE_MIN: f32 = 1.0 / 64.0;
const EXPOSURE_MAX: f32 = 64.0;
/// Env floor stays above zero because the step is multiplicative — from an
/// exact zero no number of `]` presses could ever bring it back.
const ENV_INTENSITY_MIN: f32 = 0.05;
const ENV_INTENSITY_MAX: f32 = 8.0;

/// Host-facing lighting controls for the PBR branch. Exposure is a plain
/// pre-tonemap multiplier: every light source (key, ambient, environment)
/// scales by it, which for a linear pipeline is exactly camera exposure —
/// and exposure 0 honestly blacks the model out instead of clamping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PbrDisplayControls {
    /// Direction TOWARD the key light, world space (normalized on resolve).
    pub light_dir: Vec3f,
    pub light_color: Vec3f,
    pub light_intensity: f32,
    /// Flat ambient fill (DrawPbr's `u_ambient`).
    pub ambient: f32,
    /// Environment reflection strength (DrawPbr's `u_env_intensity`); this
    /// is what makes metal read as metal, so it has its own control.
    pub env_intensity: f32,
    pub exposure: f32,
}

impl Default for PbrDisplayControls {
    fn default() -> Self {
        Self {
            light_dir: DEFAULT_LIGHT_DIR,
            light_color: vec3(1.0, 1.0, 1.0),
            light_intensity: 1.0,
            ambient: 0.42,
            env_intensity: 1.2,
            exposure: 1.15,
        }
    }
}

/// The values actually written into DrawPbr each frame. Split out so the
/// exposure/normalization math is testable without a GPU context.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedLightRig {
    pub light_dir: Vec3f,
    pub light_color: Vec3f,
    pub ambient: f32,
    pub env_intensity: f32,
}

impl PbrDisplayControls {
    pub fn resolve(&self) -> ResolvedLightRig {
        let light_dir = if self.light_dir.length() > 1.0e-6 {
            self.light_dir.normalize()
        } else {
            DEFAULT_LIGHT_DIR
        };
        let exposure = self.exposure.max(0.0);
        ResolvedLightRig {
            light_dir,
            light_color: self.light_color * (self.light_intensity.max(0.0) * exposure),
            ambient: self.ambient.max(0.0) * exposure,
            env_intensity: self.env_intensity.max(0.0) * exposure,
        }
    }
}

/// Which material roles the loaded GLB actually declares — the honest
/// loaded/absent report for the HUD. A role is "present" when any material
/// references a texture for it; factors-only materials list none.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PbrRoles {
    pub base_color: bool,
    pub metallic_roughness: bool,
    pub normal: bool,
    pub occlusion: bool,
    pub emissive: bool,
}

impl PbrRoles {
    fn from_materials(materials: &[GltfMaterialState]) -> Self {
        let mut roles = Self::default();
        for material in materials {
            roles.base_color |= material.base_color_texture.is_some();
            roles.metallic_roughness |= material.metallic_roughness_texture.is_some();
            roles.normal |= material.normal_texture.is_some();
            roles.occlusion |= material.occlusion_texture.is_some();
            roles.emissive |= material.emissive_texture.is_some();
        }
        roles
    }

    /// Compact ASCII role list for the one-line HUD ("base mr nrm"), or
    /// "factors" when the materials carry no textures at all.
    pub fn hud(&self) -> String {
        let mut out = Vec::new();
        for (present, name) in [
            (self.base_color, "base"),
            (self.metallic_roughness, "mr"),
            (self.normal, "nrm"),
            (self.occlusion, "occ"),
            (self.emissive, "emi"),
        ] {
            if present {
                out.push(name);
            }
        }
        if out.is_empty() {
            "factors".into()
        } else {
            out.join(" ")
        }
    }
}

/// Honest load report: what is resident, what is still decoding, which
/// roles the material declares, which environment is lighting it.
#[derive(Clone, Debug, PartialEq)]
pub struct PbrStatus {
    pub draw_objects: usize,
    pub materials: usize,
    pub vertices: usize,
    /// Total texture slots the GLB declares vs how many have finished the
    /// async decode. Unsupported images stay un-ready forever — visible
    /// here rather than silently painted white.
    pub textures_total: usize,
    pub textures_ready: usize,
    pub roles: PbrRoles,
    /// True when a host-supplied equirect replaced the procedural env cube.
    pub custom_env: bool,
}

/// Proof-carrying GLB: constructed only by [`parse_material_bearing_glb`],
/// so a `load` call can't be handed an unclassified byte blob.
pub struct MaterialBearingGltf(LoadedGltf);

/// Route test: does this GLB carry a renderable mesh AND at least one
/// material that references a texture? Anything else (unparseable, empty,
/// factors-only) returns None and stays on the legacy statue path, whose
/// vertex-tint rendering is already correct for it. Parsed with
/// `base_dir = None` — in-memory only, never the filesystem.
pub fn parse_material_bearing_glb(glb: &[u8]) -> Option<MaterialBearingGltf> {
    let loaded = load_gltf_from_bytes(glb, None).ok()?;
    let document = &loaded.document;
    if !document
        .meshes_slice()
        .iter()
        .any(|mesh| !mesh.primitives.is_empty())
    {
        return None;
    }
    let material_bearing = document.materials_slice().iter().any(|material| {
        let pbr = material.pbr_metallic_roughness.as_ref();
        pbr.is_some_and(|pbr| {
            pbr.base_color_texture.is_some() || pbr.metallic_roughness_texture.is_some()
        }) || material.normal_texture.is_some()
            || material.occlusion_texture.is_some()
            || material.emissive_texture.is_some()
    });
    if !material_bearing {
        return None;
    }
    Some(MaterialBearingGltf(loaded))
}

/// Any GLB with a mesh primitive, including Kenney unlit / factors-only.
/// Thumbnails use this so a mesh always has a DrawPbr subject.
pub fn parse_mesh_glb(glb: &[u8]) -> Option<MaterialBearingGltf> {
    let loaded = load_gltf_from_bytes(glb, None).ok()?;
    let has_mesh = loaded
        .document
        .meshes_slice()
        .iter()
        .any(|mesh| !mesh.primitives.is_empty());
    has_mesh.then_some(MaterialBearingGltf(loaded))
}

/// World AABB over every draw object (local bounds through the node's world
/// transform). None for empty scenes or any non-finite corner — a malformed
/// mesh is rejected rather than fitted to garbage.
pub(crate) fn world_bounds(objects: &[GltfDrawObject]) -> Option<(Vec3f, Vec3f)> {
    let mut bounds: Option<(Vec3f, Vec3f)> = None;
    for object in objects {
        let (lo, hi) = (object.local_bounds_min, object.local_bounds_max);
        for corner in 0..8usize {
            let p = object.world_transform.transform_vec4(vec4(
                if corner & 1 == 0 { lo.x } else { hi.x },
                if corner & 2 == 0 { lo.y } else { hi.y },
                if corner & 4 == 0 { lo.z } else { hi.z },
                1.0,
            ));
            if !(p.x.is_finite() && p.y.is_finite() && p.z.is_finite()) {
                return None;
            }
            bounds = Some(match bounds {
                None => (vec3f(p.x, p.y, p.z), vec3f(p.x, p.y, p.z)),
                Some((mn, mx)) => (
                    vec3f(mn.x.min(p.x), mn.y.min(p.y), mn.z.min(p.z)),
                    vec3f(mx.x.max(p.x), mx.y.max(p.y), mx.z.max(p.z)),
                ),
            });
        }
    }
    bounds
}

/// The statue fit rule, verbatim: uniform scale by the largest dimension,
/// centered in x/z, lowest point on the ground plane, presentation yaw.
/// Returns (transform, scale); None when the bounds are degenerate.
pub(crate) fn fit_transform(min: Vec3f, max: Vec3f) -> Option<(Mat4f, f32)> {
    // Checked per component: f32::max IGNORES NaN (NaN.max(1.0) == 1.0), so
    // a poisoned bound would otherwise slide through the extent reduction
    // and surface as a NaN translation instead of an honest rejection.
    for v in [min.x, min.y, min.z, max.x, max.y, max.z] {
        if !v.is_finite() {
            return None;
        }
    }
    let size = max - min;
    let extent = size.x.max(size.y).max(size.z);
    if extent <= 1.0e-6 {
        return None;
    }
    let scale = FIT_EXTENT / extent;
    if !scale.is_finite() {
        return None;
    }
    let center = (min + max) * 0.5;
    Some((
        super::trs_yaw(
            vec3f(-center.x * scale, -min.y * scale, -center.z * scale),
            FIT_YAW,
            scale,
        ),
        scale,
    ))
}

/// Orbit the key light by azimuth/elevation deltas, staying unit length.
/// Elevation clamps just short of the poles so azimuth never degenerates.
pub(crate) fn orbit_light_dir(dir: Vec3f, d_azimuth: f32, d_elevation: f32) -> Vec3f {
    let dir = if dir.length() > 1.0e-6 {
        dir.normalize()
    } else {
        DEFAULT_LIGHT_DIR
    };
    let elevation = (dir.y.clamp(-1.0, 1.0).asin() + d_elevation).clamp(-1.45, 1.45);
    let azimuth = dir.x.atan2(dir.z) + d_azimuth;
    let radius = elevation.cos();
    vec3f(radius * azimuth.sin(), elevation.sin(), radius * azimuth.cos())
}

/// One multiplicative quarter-stop up or down, clamped.
pub(crate) fn scaled_step(value: f32, up: bool, lo: f32, hi: f32) -> f32 {
    let stepped = if up {
        value * STEP_FACTOR
    } else {
        value / STEP_FACTOR
    };
    stepped.clamp(lo, hi)
}

/// The static PBR branch state MeshView owns: the retained glTF renderer,
/// its fitted transform, host controls and the honest status. The DrawPbr
/// shader itself stays a `#[live]` field on MeshView (script registration);
/// this struct only borrows it per call.
#[derive(Default)]
pub struct PbrPreview {
    renderer: Option<GltfRenderer>,
    fit: Option<Mat4f>,
    bounds: Option<(Vec3f, Vec3f)>,
    pub controls: PbrDisplayControls,
    pub status: Option<PbrStatus>,
    /// Host-supplied equirect environment, applied at the next draw (env
    /// cube building needs a CxDraw). Survives model reloads on purpose.
    pending_env: Option<Vec<u8>>,
    custom_env: bool,
}

impl PbrPreview {
    /// Drop the current model and its GPU meshes. Controls and environment
    /// persist across loads — they are viewer state, not model state.
    pub fn clear(&mut self, draw: &mut DrawPbr) {
        // Renderer first: its mesh handles index into draw.meshes.
        self.renderer = None;
        self.fit = None;
        self.bounds = None;
        self.status = None;
        draw.clear_meshes();
    }

    /// Make a classified GLB resident and fit it to the pane. `tag` (the
    /// host's load generation) keeps image-cache keys unique per load, so a
    /// regenerated model can never be served a predecessor's decoded maps.
    pub fn load(
        &mut self,
        draw: &mut DrawPbr,
        cx: &mut CxDraw,
        gltf: MaterialBearingGltf,
        tag: u64,
    ) -> Result<(), String> {
        self.clear(draw);
        let mut loaded = gltf.0;
        // Cache identity only — base_dir stays None, so this synthetic name
        // can never turn into a filesystem lookup.
        loaded.source_path = Some(std::path::PathBuf::from(format!("mem:aiapp-pbr-{tag}.glb")));
        debug_assert!(loaded.base_dir.is_none());
        let renderer = match GltfRenderer::from_loaded(draw, cx, &loaded) {
            Ok(renderer) => renderer,
            Err(e) => {
                self.clear(draw);
                return Err(e.to_string());
            }
        };
        let Some((min, max)) = world_bounds(&renderer.draw_objects) else {
            self.clear(draw);
            return Err("empty or non-finite geometry".into());
        };
        let Some((fit, _scale)) = fit_transform(min, max) else {
            self.clear(draw);
            return Err("degenerate bounds".into());
        };
        self.status = Some(PbrStatus {
            draw_objects: renderer.draw_objects.len(),
            materials: renderer.materials.len(),
            vertices: renderer
                .draw_objects
                .iter()
                .map(|o| o.local_vertex_count)
                .sum(),
            textures_total: renderer.textures.len(),
            textures_ready: renderer.textures.iter().filter(|t| t.is_some()).count(),
            roles: PbrRoles::from_materials(&renderer.materials),
            custom_env: self.custom_env,
        });
        self.bounds = Some((min, max));
        self.fit = Some(fit);
        self.renderer = Some(renderer);
        Ok(())
    }

    pub fn bounds(&self) -> Option<(Vec3f, Vec3f)> {
        self.bounds
    }

    pub fn set_fit(&mut self, fit: Mat4f) {
        self.fit = Some(fit);
    }

    /// One-line load summary for the host status field.
    pub fn summary(&self) -> String {
        match &self.status {
            Some(s) => format!(
                "PBR: {} objects, {} materials, {} verts",
                s.draw_objects, s.materials, s.vertices
            ),
            None => "PBR".into(),
        }
    }

    /// Forward events so async texture decodes commit; true = a map just
    /// arrived (or finished failing) and the pane should repaint.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let Some(renderer) = self.renderer.as_mut() else {
            return false;
        };
        let before = renderer.textures.iter().filter(|t| t.is_some()).count();
        renderer.handle_event(cx, event);
        let after = renderer.textures.iter().filter(|t| t.is_some()).count();
        if after == before {
            return false;
        }
        if let Some(status) = &mut self.status {
            status.textures_ready = after;
        }
        true
    }

    /// Queue an equirect (PNG/JPG) to replace the procedural environment
    /// cube. Applied at the next draw; the bytes are immutable input.
    /// Reached only through MeshView's handoff API today.
    #[allow(dead_code)]
    pub fn set_env_equirect(&mut self, bytes: Vec<u8>) {
        self.pending_env = Some(bytes);
    }

    /// Light/exposure/environment keys for the focused pane. Returns false
    /// (untouched) when no PBR model is shown, so the play path's key
    /// handling is never shadowed.
    pub fn control_key(&mut self, key: KeyCode) -> bool {
        if self.renderer.is_none() {
            return false;
        }
        let c = &mut self.controls;
        match key {
            KeyCode::ArrowLeft => {
                c.light_dir = orbit_light_dir(c.light_dir, -LIGHT_AZIMUTH_STEP, 0.0)
            }
            KeyCode::ArrowRight => {
                c.light_dir = orbit_light_dir(c.light_dir, LIGHT_AZIMUTH_STEP, 0.0)
            }
            KeyCode::ArrowUp => {
                c.light_dir = orbit_light_dir(c.light_dir, 0.0, LIGHT_ELEVATION_STEP)
            }
            KeyCode::ArrowDown => {
                c.light_dir = orbit_light_dir(c.light_dir, 0.0, -LIGHT_ELEVATION_STEP)
            }
            KeyCode::Minus => {
                c.exposure = scaled_step(c.exposure, false, EXPOSURE_MIN, EXPOSURE_MAX)
            }
            KeyCode::Equals => {
                c.exposure = scaled_step(c.exposure, true, EXPOSURE_MIN, EXPOSURE_MAX)
            }
            KeyCode::LBracket => {
                c.env_intensity =
                    scaled_step(c.env_intensity, false, ENV_INTENSITY_MIN, ENV_INTENSITY_MAX)
            }
            KeyCode::RBracket => {
                c.env_intensity =
                    scaled_step(c.env_intensity, true, ENV_INTENSITY_MIN, ENV_INTENSITY_MAX)
            }
            KeyCode::Key0 => *c = PbrDisplayControls::default(),
            _ => return false,
        }
        true
    }

    /// HUD tail for the PBR branch: live decode progress, declared roles,
    /// environment source, current exposure/env values, key hints.
    pub fn hud_line(&self) -> Option<String> {
        let status = self.status.as_ref()?;
        Some(format!(
            "maps {}/{} [{}] env:{}  exp {:.2} env {:.2}   arrows light, -/= exposure, [ ] env, 0 reset, drag orbit, wheel zoom",
            status.textures_ready,
            status.textures_total,
            status.roles.hud(),
            if status.custom_env { "equirect" } else { "default" },
            self.controls.exposure,
            self.controls.env_intensity,
        ))
    }

    /// Draw the fitted model into the current 3D pass. Opaque and
    /// depth-tested, so drawing after the world composes correctly.
    pub fn draw(&mut self, draw: &mut DrawPbr, cx: &mut Cx3d) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if let Some(bytes) = self.pending_env.take() {
            match draw.load_default_env_equirect_from_bytes(cx, &bytes, None) {
                Ok(()) => {
                    self.custom_env = true;
                    if let Some(status) = &mut self.status {
                        status.custom_env = true;
                    }
                }
                Err(e) => log!("mesh_view pbr: env equirect rejected: {e}"),
            }
        }
        let rig = self.controls.resolve();
        draw.light_dir = rig.light_dir;
        draw.light_color = rig.light_color;
        draw.ambient = rig.ambient;
        draw.env_intensity = rig.env_intensity;
        draw.reset_matrix();
        let Some(fit) = self.fit else {
            return;
        };
        if let Err(e) = renderer.draw_with_transform(draw, cx, fit) {
            log!("mesh_view pbr: draw failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_applies_exposure_to_every_light_source() {
        let mut controls = PbrDisplayControls::default();
        controls.light_intensity = 2.0;
        controls.ambient = 0.5;
        controls.env_intensity = 1.5;
        controls.exposure = 0.25;
        let rig = controls.resolve();
        assert!((rig.light_color.x - 0.5).abs() < 1.0e-6);
        assert!((rig.ambient - 0.125).abs() < 1.0e-6);
        assert!((rig.env_intensity - 0.375).abs() < 1.0e-6);
        // Exposure zero blacks everything out honestly (DrawPbr then also
        // disables its direct/IBL terms off the zero magnitudes).
        controls.exposure = 0.0;
        let rig = controls.resolve();
        assert_eq!(rig.light_color, vec3(0.0, 0.0, 0.0));
        assert_eq!(rig.ambient, 0.0);
        assert_eq!(rig.env_intensity, 0.0);
    }

    #[test]
    fn resolve_normalizes_and_repairs_the_light_direction() {
        let mut controls = PbrDisplayControls::default();
        controls.light_dir = vec3f(0.0, 10.0, 0.0);
        assert!((controls.resolve().light_dir.length() - 1.0).abs() < 1.0e-5);
        controls.light_dir = vec3f(0.0, 0.0, 0.0);
        assert_eq!(controls.resolve().light_dir, DEFAULT_LIGHT_DIR);
    }

    #[test]
    fn orbit_light_stays_unit_and_clamps_elevation() {
        let mut dir = DEFAULT_LIGHT_DIR;
        for _ in 0..100 {
            dir = orbit_light_dir(dir, 0.3, 0.2);
            assert!((dir.length() - 1.0).abs() < 1.0e-4);
        }
        // 100 upward steps ended at the clamp, not the pole.
        assert!(dir.y <= 1.45_f32.sin() + 1.0e-4);
        let level = orbit_light_dir(vec3f(0.0, 0.0, 1.0), std::f32::consts::TAU, 0.0);
        assert!((level.z - 1.0).abs() < 1.0e-4, "full azimuth turn returns: {level:?}");
        // Degenerate input recovers to the default instead of NaN.
        assert!(orbit_light_dir(vec3f(0.0, 0.0, 0.0), 0.0, 0.0).length() > 0.9);
    }

    #[test]
    fn scaled_step_clamps_at_both_bounds() {
        let mut v = 1.0;
        for _ in 0..100 {
            v = scaled_step(v, true, EXPOSURE_MIN, EXPOSURE_MAX);
        }
        assert_eq!(v, EXPOSURE_MAX);
        for _ in 0..200 {
            v = scaled_step(v, false, EXPOSURE_MIN, EXPOSURE_MAX);
        }
        assert_eq!(v, EXPOSURE_MIN);
    }

    fn draw_object(min: Vec3f, max: Vec3f, transform: Mat4f) -> GltfDrawObject {
        GltfDrawObject {
            mesh_handle: 0,
            node_index: 0,
            mesh_index: 0,
            primitive_index: 0,
            material_index: None,
            world_transform: transform,
            local_bounds_min: min,
            local_bounds_max: max,
            local_centroid: (min + max) * 0.5,
            local_vertex_count: 3,
        }
    }

    #[test]
    fn world_bounds_transforms_boxes_and_rejects_non_finite() {
        assert!(world_bounds(&[]).is_none());
        let shifted = draw_object(
            vec3f(-1.0, 0.0, -1.0),
            vec3f(1.0, 2.0, 1.0),
            Mat4f::translation(vec3f(10.0, 0.0, 0.0)),
        );
        let (min, max) = world_bounds(std::slice::from_ref(&shifted)).unwrap();
        assert!((min.x - 9.0).abs() < 1.0e-5 && (max.x - 11.0).abs() < 1.0e-5);
        assert!((min.y - 0.0).abs() < 1.0e-5 && (max.y - 2.0).abs() < 1.0e-5);
        let broken = draw_object(
            vec3f(0.0, 0.0, 0.0),
            vec3f(1.0, f32::NAN, 1.0),
            Mat4f::identity(),
        );
        assert!(world_bounds(&[broken]).is_none());
    }

    #[test]
    fn fit_transform_grounds_feet_and_normalizes_largest_extent() {
        let (min, max) = (vec3f(1.0, 2.0, 3.0), vec3f(3.0, 4.0, 7.0));
        let (fit, scale) = fit_transform(min, max).unwrap();
        // Largest dimension is z = 4.
        assert!((scale - FIT_EXTENT / 4.0).abs() < 1.0e-6);
        // The lowest vertex lands exactly on the ground plane; yaw about Y
        // cannot move it vertically.
        let low = fit.transform_vec4(vec4(2.0, min.y, 5.0, 1.0));
        assert!(low.y.abs() < 1.0e-5, "feet at {}", low.y);
        // The highest point sits at its scaled height above the plane.
        let high = fit.transform_vec4(vec4(2.0, max.y, 5.0, 1.0));
        assert!((high.y - (max.y - min.y) * scale).abs() < 1.0e-5);
        // Degenerate and non-finite bounds are rejected.
        assert!(fit_transform(vec3f(0.0, 0.0, 0.0), vec3f(0.0, 0.0, 0.0)).is_none());
        assert!(fit_transform(vec3f(0.0, 0.0, 0.0), vec3f(f32::NAN, 1.0, 1.0)).is_none());
    }

    /// Minimal spec-shaped GLB container: JSON chunk (space-padded) plus an
    /// optional BIN chunk (zero-padded).
    fn tiny_glb(json: &str, bin: &[u8]) -> Vec<u8> {
        let mut json_bytes = json.as_bytes().to_vec();
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }
        let mut bin_bytes = bin.to_vec();
        while bin_bytes.len() % 4 != 0 {
            bin_bytes.push(0);
        }
        let mut total = 12 + 8 + json_bytes.len();
        if !bin.is_empty() {
            total += 8 + bin_bytes.len();
        }
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_bytes);
        if !bin.is_empty() {
            out.extend_from_slice(&(bin_bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(b"BIN\0");
            out.extend_from_slice(&bin_bytes);
        }
        out
    }

    #[test]
    fn classifier_requires_a_material_texture_reference() {
        let bearing = tiny_glb(
            r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],
            "nodes":[{"mesh":0}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0},"material":0}]}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":1,"type":"VEC3"}],
            "materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}],
            "textures":[{"source":0}],
            "images":[{"bufferView":1,"mimeType":"image/png"}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":12},
                           {"buffer":0,"byteOffset":12,"byteLength":4}],
            "buffers":[{"byteLength":16}]}"#,
            &[0u8; 16],
        );
        assert!(parse_material_bearing_glb(&bearing).is_some());

        let factors_only = tiny_glb(
            r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],
            "nodes":[{"mesh":0}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0},"material":0}]}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":1,"type":"VEC3"}],
            "materials":[{"pbrMetallicRoughness":{"baseColorFactor":[1,0,0,1]}}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":12}],
            "buffers":[{"byteLength":12}]}"#,
            &[0u8; 12],
        );
        assert!(parse_material_bearing_glb(&factors_only).is_none());

        assert!(parse_material_bearing_glb(b"not a glb at all").is_none());
    }

    #[test]
    fn control_keys_are_inert_without_a_loaded_model() {
        // Guard: no PBR model shown means the keys stay free for the play
        // path and never mutate viewer state.
        let mut preview = PbrPreview::default();
        let before = preview.controls;
        assert!(!preview.control_key(KeyCode::ArrowLeft));
        assert!(!preview.control_key(KeyCode::Equals));
        assert_eq!(preview.controls, before);
    }
}
