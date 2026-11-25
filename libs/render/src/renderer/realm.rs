//! Realm lifecycle, stage/quality, effects, placed models and custom materials.

use super::*;
use super::draw_models::LayerPhase;
use super::model_instance::refill_from_slice;
use crate::custom_material::{TracedScene, TracedSurfacePasses};

struct TracedScenes {
    caster: TracedScene,
    outside: TracedScene,
}

impl TracedScenes {
    fn for_program(&self, program: &TracedProgram) -> TracedScene {
        if program.wants_sun() { self.caster } else { self.outside }
    }
}

/// Authoritative slots keep their identity across identical submissions. Transient
/// item instances may follow them for the duration of a frame.
#[derive(Default)]
pub(super) struct PlacedModels {
    copies: Vec<ModelInstance>,
    retired: Vec<bool>,
    retired_count: usize,
    /// Static slots by resident model. The mover walk can visit the copies of a
    /// traced level even when their ordinary geometry belongs to a static block.
    pub(super) static_by_model: Vec<Vec<usize>>,
    retired_lamp: bool,
    retirement_revision: u64,
    warned_dynamic: bool,
    warned_missing: bool,
}

impl PlacedModels {
    pub(super) fn len(&self) -> usize { self.copies.len() }
    pub(super) fn get(&self, slot: usize) -> Option<&ModelInstance> {
        self.view().get(slot)
    }
    pub(super) fn live(&self) -> impl Iterator<Item = (usize, &ModelInstance)> {
        self.view().live()
    }
    // The temporary item tail belongs to this draw, outside the host's submitted list.
    fn all_including_retired(&self) -> &[ModelInstance] { &self.copies[..self.retired.len()] }

    pub(super) fn append_items(&mut self, items: Vec<ModelInstance>) { self.copies.extend(items); }
    pub(super) fn truncate_items(&mut self, keep: usize) { self.copies.truncate(keep); }

    fn take_all_including_retired(&mut self) -> Vec<ModelInstance> { std::mem::take(&mut self.copies) }

    fn replace(&mut self, copies: Vec<ModelInstance>, restore: bool) {
        if restore {
            self.retired_count = 0;
            self.retired_lamp = false;
            self.retirement_revision = self.retirement_revision.wrapping_add(1);
            self.retired.fill(false);
        }
        self.retired.resize(copies.len(), false);
        self.copies = copies;
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool { self.copies.is_empty() }
    #[cfg(test)]
    pub(super) fn last(&self) -> Option<&ModelInstance> { self.live().last().map(|(_, m)| m) }
}

#[cfg(test)]
impl std::ops::Index<usize> for PlacedModels {
    type Output = ModelInstance;
    fn index(&self, slot: usize) -> &ModelInstance { &self.copies[slot] }
}

/// Both draw lanes address copies by their original slot. Attachment lists
/// have no retired slots and always form a single contiguous live run.
#[derive(Clone, Copy)]
pub(super) struct CopyView<'a> {
    copies: &'a [ModelInstance],
    retired: &'a [bool],
}

pub(super) trait ModelList {
    fn view(&self) -> CopyView<'_>;
}

impl<T: AsRef<[ModelInstance]> + ?Sized> ModelList for T {
    fn view(&self) -> CopyView<'_> { CopyView { copies: self.as_ref(), retired: &[] } }
}

impl ModelList for PlacedModels {
    fn view(&self) -> CopyView<'_> {
        CopyView { copies: &self.copies, retired: if self.retired_count == 0 { &[] } else { &self.retired } }
    }
}

impl ModelList for CopyView<'_> {
    fn view(&self) -> CopyView<'_> { *self }
}

impl<'a> CopyView<'a> {
    pub(super) fn len(self) -> usize { self.copies.len() }
    pub(super) fn key(self) -> (usize, usize) { (self.copies.as_ptr() as usize, self.len()) }

    fn is_live(self, slot: usize) -> bool { !self.retired.get(slot).copied().unwrap_or(false) }

    pub(super) fn get(self, slot: usize) -> Option<&'a ModelInstance> {
        self.copies.get(slot).filter(|_| self.is_live(slot))
    }

    fn live_from(self, start: usize) -> impl Iterator<Item = (usize, &'a ModelInstance)> {
        self.copies.iter().enumerate().skip(start)
            .filter(move |(slot, _)| self.is_live(*slot))
    }

    pub(super) fn live(self) -> impl Iterator<Item = (usize, &'a ModelInstance)> { self.live_from(0) }

    /// Slice-only consumers can take the dense path while every slot is live.
    fn contiguous_live(self) -> Option<&'a [ModelInstance]> {
        self.retired.is_empty().then_some(self.copies)
    }
}

impl std::ops::Index<usize> for CopyView<'_> {
    type Output = ModelInstance;
    fn index(&self, slot: usize) -> &ModelInstance { &self.copies[slot] }
}

#[derive(Default)]
pub(super) struct StaticCopy {
    casters: std::ops::Range<usize>,
    lamp: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StaticMaterialSource {
    pub(super) caster: usize,
    pub(super) instance: usize,
    /// An instance-wide source uses the empty lookup name. Keep the mesh layer for later overrides.
    instance_wide: bool,
    layer: std::sync::Arc<str>,
    program: String,
}

#[derive(Clone, Copy)]
pub(super) struct ResolvedCaster<'a> {
    name: &'a String,
    shader: DrawShaderId,
    params: Vec4f,
    cutoff: f32,
    bounds_pad: f32,
}

impl ResolvedCaster<'_> {
    pub(super) fn material(self, texture: &Texture) -> crate::gpu_lightmap::MoverMaterial {
        crate::gpu_lightmap::MoverMaterial {
            shader: self.shader, params: self.params, cutoff: self.cutoff, texture: texture.clone(),
        }
    }
}

/// Program names in registry order and the phase in which each was selected.
#[derive(Default)]
pub(super) struct ProgramWalk {
    entries: Vec<(String, Option<LayerPhase>)>,
    walking: usize,
}

impl ProgramWalk {
    pub(super) fn begin(
        &mut self, programs: &std::collections::BTreeMap<String, Box<CustomMaterial>>,
        instances: &(impl ModelList + ?Sized), stock_ibl_on: bool,
    ) {
        for (index, (name, material)) in programs.iter().enumerate() {
            let selected = (stock_ibl_on && name == stock_ibl::STOCK_IBL_MATERIAL)
                || instances.view().live().any(|(_, inst)| inst.material_names().any(|program| program == name.as_str()));
            // A draw can change its blend state as it binds layers. Decide the order
            // before drawing any of them, and keep the stock IBL lane in the opaque group.
            let order = selected.then(|| {
                if name != stock_ibl::STOCK_IBL_MATERIAL && material.draw.draw_vars.options.alpha_blend {
                    LayerPhase::Blended
                } else {
                    LayerPhase::Opaque
                }
            });
            if let Some((key, phase)) = self.entries.get_mut(index) {
                if key.as_str() != name.as_str() {
                    key.clone_from(name);
                }
                *phase = order;
            } else {
                self.entries.push((name.clone(), order));
            }
        }
        self.walking = programs.len();
    }

    pub(super) fn phase(&self, index: usize) -> Option<LayerPhase> {
        self.entries[index].1
    }

    pub(super) fn end(&mut self) {
        self.walking = 0;
    }

    /// During the opaque walk the draws are borrowed from the detached registry.
    pub(super) fn contains(&self, name: &str) -> bool {
        self.entries[..self.walking].binary_search_by(|(key, _)| key.as_str().cmp(name)).is_ok()
    }
}

impl crate::gpu_lightmap::StaticCasterBindings {
    /// Retiring a copy leaves its sorted casters and binding record in place.
    /// Compare the resubmitted suffix through the same live mask on both sides.
    fn live_suffix_matches(&self, instances: &PlacedModels, start: usize) -> bool {
        let view = instances.view();
        let base = self.copies.partition_point(|entry| entry.slot < start);
        let old = self.copies[base..].iter().filter(|c| view.get(c.slot).is_some())
            .map(|c| (c.slot, &c.material, &c.layers));
        let new = view.live_from(start).filter(|(_, m)| !m.dynamic && m.has_custom())
            .map(|(i, m)| (i, &m.custom_material, &m.layer_materials));
        old.eq(new)
    }

    /// Inspect only the submitted part. A retained prefix costs no comparisons, and
    /// a repeated submission reuses its records without hashing or allocating.
    fn update(&mut self, instances: &PlacedModels, start: usize, model_index: impl Fn(&str) -> Option<usize>) -> bool {
        if !self.tracking { return false; }
        let mut next = self.copies.partition_point(|entry| entry.slot < start);
        let mut changed = false;
        for (slot, inst) in instances.view().live_from(start).filter(|(_, m)| !m.dynamic && m.has_custom()) {
            if let Some(copy) = self.copies.get_mut(next) {
                if copy.slot != slot {
                    copy.slot = slot;
                    copy.model = model_index(&inst.model);
                    changed = true;
                }
                if copy.material != inst.custom_material || copy.layers != inst.layer_materials {
                    copy.material.clone_from(&inst.custom_material);
                    copy.layers.clone_from(&inst.layer_materials);
                    changed = true;
                }
            } else {
                self.copies.push(crate::gpu_lightmap::SubmittedCasterBinding {
                    slot, model: model_index(&inst.model),
                    material: inst.custom_material.clone(), layers: inst.layer_materials.clone(),
                });
                changed = true;
            }
            next += 1;
        }
        changed |= next != self.copies.len();
        self.copies.truncate(next);
        changed
    }
}

impl Renderer {
    /// Begin rendering a different realm into this view.
    ///
    /// World-local revision counters restart when a script game is loaded,
    /// so they cannot by themselves distinguish (for example) town terrain
    /// at revision 1 from FPS terrain at revision 1. Call this exactly once
    /// at the successful world-replacement boundary, before submitting the
    /// new realm's first frame. It invalidates every cache derived from the
    /// old world while retaining uploaded asset geometry/textures, shader
    /// and pass pools, stage policy, bake settings, shadow budget, the
    /// local shadow atlas (its cache is keyed on content, below), the
    /// adaptive-quality history owned by this device, and the grass field
    /// (the host's to replace or remove with [`Self::set_grass`]).
    pub fn enter_realm(&mut self) {
        self.volume_fog.reset();
        self.ssr.reset();
        self.custom_draws.clear();
        self.traced_programs.clear();
        // The stock IBL program went with them: a build of it that failed
        // may be made again (renderer/stock_ibl.rs).
        self.stock_ibl_forget_failure();
        self.static_chunks.clear();
        self.chunk_visible.clear();
        self.slab_key = None;
        self.slab_instance_count = 0;

        self.terrain_tiles.clear();
        self.terrain_revision = 0;
        self.voxel_tiles.clear();
        self.water.enter_realm();

        // Keep resident rig geometry and the palette texture allocation;
        // only their frame-to-instance mapping belongs to the old realm.
        self.skin_joint_bases.clear();
        // Preserve the monotonic bake key across realms, including any retirement edits.
        self.placed_models.replace(Vec::new(), self.placed_models.retired_count != 0);
        self.placed_models.static_by_model.clear();
        self.placed_blocks = Default::default();
        self.placed_static_slots.clear();
        self.placed_caster_indices.clear();
        self.placed_prefix = None;
        self.csm_static_casters.clear();
        self.csm_material_sources.clear();
        self.csm_static_blocks.clear();
        // The grass stays: the host replaces or removes it (set_grass), so a
        // level re-evaluated in place keeps drawing its field instead of
        // showing none until the same field is prepared again.
        self.world_attachments.clear();
        self.view_models.clear();
        self.placed_scene_signature = None;
        self.vfx.records.clear();
        self.vfx.decals.clear();
        self.firework_instances.clear();
        self.smoke_volumes.clear();

        self.shadow_mesh.clear();
        self.receiver_boxes.clear();
        self.occluder_boxes.clear();
        self.shadow_geometry = None;
        self.last_dynamic_shadow_tris = 0;
        self.shadow_points.clear();
        self.shadow_gate = ShadowRebuildGate::default();
        self.sky_hour = None;
        self.sky_clock = false;

        self.clear_delivered_lightmap();
        self.gpu_baker.enter_realm();
        self.csm_scene_bounds = None;
        self.lm_lights.clear();
        self.frame_lights.clear();
        self.frame_baked_count = 0;
        self.host_lights.clear();self.host_asset_lights.clear();self.asset_light_error=None;
        self.lamp_cache.clear();
        self.lamp_cache_rev = None;
        self.light_grid = LightGrid::default();
        // The local shadow atlas stays. A face is kept only for the same
        // light in the same tile with the same camera, the same static
        // casters (geometry ids and transforms) and the same movers inside,
        // so a new world's faces miss by themselves and a restart of the same
        // one finds its static faces; the atlas targets, pass and depth
        // shaders are the device's. Rebuilding them cost every new world's
        // first frame 1-2 ms.
        let cluster_config = self.clustered.config();
        let shadows = std::mem::take(&mut self.clustered.shadows);
        self.clustered = crate::clustered::ClusteredLights::default();
        self.clustered.set_config(cluster_config);
        self.clustered.shadows = shadows;
        self.clustered_frames = 0;
        self.light_rank.clear();
        self.gi.reset();
        // A fixed volume belongs to the departing world, not device quality.
        self.gi.set_config(crate::GiConfig{anchor:None,..self.gi.config()});
        self.light_sel.clear();
        self.light_block_scratch.fill(0.0);
        self.light_cell_memory.clear();
        self.char_ground.clear();
        self.model_ground.clear();
        self.world_attachment_ground.clear();
        self.sdf_instances.clear();
        self.lm_kick_key = None;
        self.lm_kick_sun = None;

        // Keep the counter monotonic even though all its consumers above
        // were cleared; this prevents a future cache from reintroducing the
        // same cross-realm numeric-alias bug.
        self.models_rev = self.models_rev.wrapping_add(1);
        self.bake.enter_realm();
    }

    /// Begin a revision of the current realm. Keep its baked static lighting
    /// on screen until the revised realm's first bake delivers. A placed copy
    /// keeps its atlas region when its chart and transform still match.
    pub fn enter_revised_realm(&mut self) {
        let atlas = self.lightmap.take();
        let remaps = std::mem::take(&mut self.lm_remaps);
        let owners = std::mem::take(&mut self.lm_remap_owners);
        let ground = self.lm_ground.take();
        let top = self.lm_top.take();
        self.enter_realm();
        self.lightmap = atlas;
        self.lm_remaps = remaps;
        self.lm_remap_owners = owners;
        self.lm_ground = ground;
        self.lm_top = top;
    }

    /// How this device projects the world. Changing it is free — the stage
    /// is a draw-list uniform, so nothing cached needs rebuilding, and the
    /// simulation never learns it happened.
    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn set_stage(&mut self, stage: Stage) {
        self.stage = stage;
    }

    /// The room box an `MrBox` stage clips the world to (other stages ignore it).
    pub fn set_clip_box(&mut self, clip_box: Option<crate::stage::ClipBox>) {
        self.clip_box = clip_box;
    }

    /// The stage the scene draw list carries as its view transform: the stage itself, or
    /// identity on an `MrBox` stage (the XR eye views carry it, `Cx::xr_set_scene_to_room`).
    pub(crate) fn draw_stage_matrix(&self) -> Mat4f {
        if self.stage.mode == StageMode::MrBox { Mat4f::identity() } else { self.stage.matrix() }
    }

    /// An `MrBox` stage's box, world -> the unit box.
    pub(crate) fn clip_box_from_world(&self) -> Option<Mat4f> {
        let b = self.clip_box.filter(|_| self.stage.mode == StageMode::MrBox)?;
        Some(Mat4f::mul(&b.unit_from_room(), &self.stage.matrix()))
    }

    /// How many casters may use the projected-silhouette tier. Lower it on
    /// mobile/standalone XR; see apps/commercial/sandbox/BUDGETS.md.
    pub fn shadow_budget(&self) -> usize {
        self.shadow_budget
    }

    pub fn set_shadow_budget(&mut self, casters: usize) {
        self.shadow_budget = casters;
    }

    /// Feed the governor one frame's cost, in milliseconds. Returns true on
    /// the frames where the quality level actually moved, so a host can log
    /// or surface it without polling.
    ///
    /// This is OPT-IN: a host that never calls it never gets cut, which is
    /// the right default for a tool that removes scenery. Pass the platform's
    /// real frame time — an XR runtime hands you one, and it is the number
    /// that matters because the Quest is fill-bound, not CPU-bound.
    ///
    /// One trap worth naming: do not pass a vsync-locked frame-to-frame
    /// interval. That signal is quantised to the refresh rate, so it reads
    /// ~16.6ms whether the frame took 3ms or 16ms of real work, and a
    /// governor targeting 80% of budget would cut forever without ever
    /// seeing an improvement. If a real measurement is unavailable, leave
    /// this uncalled rather than feeding it a quantised one.
    pub fn report_frame_ms(&mut self, ms: f32) -> bool {
        self.thermometer.frame(ms)
    }

    /// Tell the governor what this device's display budget is. Call on
    /// startup and whenever the refresh changes — a Quest at 72Hz and the
    /// same Quest at 120Hz want budgets nearly twice apart.
    pub fn set_refresh_hz(&mut self, hz: f32) {
        self.thermometer.set_refresh_hz(hz);
    }

    /// The current cuts. The renderer applies `particle_scale`,
    /// `shadow_caster_scale` and `projected_shadows` itself; the remaining
    /// dials (`decor_distance_scale`, `foliage_scale`, `draw_distance_scale`)
    /// describe scenery only the world builder can identify, so a host that
    /// places decoration should read them when deciding what to emit.
    pub fn quality(&self) -> Quality {
        self.thermometer.quality()
    }

    /// 0 = everything on. Higher = leaner. Pair with [`Self::quality_reason`]
    /// when showing this to a player, so a suddenly emptier world reads as a
    /// deliberate trade rather than a bug.
    pub fn quality_level(&self) -> usize {
        self.thermometer.level()
    }

    /// One line describing what the current level gave up.
    pub fn quality_reason(&self) -> &'static str {
        self.thermometer.reason()
    }

    /// Measured p90 frame time, once enough frames have been seen. `None`
    /// while the window is still filling.
    pub fn frame_p90_ms(&self) -> Option<f32> {
        self.thermometer.p90_ms()
    }

    /// This frame's volumetric smoke clouds (drawn once, in the late
    /// transparent layer, then consumed: push them every frame).
    pub fn set_smoke_volumes(&mut self, volumes: Vec<crate::smoke::SmokeVolume>) {
        self.smoke_volumes = volumes;
    }

    /// This frame's firework shells: one instance per shell, expanded into
    /// sparks on the GPU (firework.rs).
    pub fn set_fireworks(&mut self, instances: Vec<crate::firework::FireworkInstance>) {
        self.firework_instances = instances;
    }

    /// Build the spark sheet once. Its size is fixed by SPARKS_PER_SHELL, so
    /// it never needs rebuilding the way the terrain mesh does.
    pub(super) fn ensure_spark_geometry(&mut self, cx: &mut Cx) -> GeometryId {
        if let Some(g) = &self.spark_geometry {
            return g.geometry_id();
        }
        let (indices, vertices) = crate::firework::spark_sheet_vertices();
        let geometry = Geometry::new(cx);
        geometry.update(cx, indices, vertices);
        let id = geometry.geometry_id();
        self.spark_geometry = Some(geometry);
        id
    }

    /// The flares' shared billboard: ONE quad in the CubeVertex layout
    /// (geom_pos.xy = corner, geom_uv = 0..1), built once — the per-lamp
    /// data rides in the instance stream.
    pub(super) fn ensure_flare_geometry(&mut self, cx: &mut Cx) -> GeometryId {
        if let Some(g) = &self.flare_geometry {
            return g.geometry_id();
        }
        let mut vertices: Vec<f32> = Vec::with_capacity(4 * 12);
        for (qx, qy) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            // geom_pos(3), geom_id(1), geom_normal(3), geom_pad(1),
            // geom_uv(2), tail_pad(2) — the spark-sheet layout.
            vertices.extend_from_slice(&[
                qx,
                qy,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
                0.0,
                qx + 0.5,
                qy + 0.5,
                0.0,
                0.0,
            ]);
        }
        let indices = vec![0u32, 1, 2, 0, 2, 3];
        let geometry = Geometry::new(cx);
        geometry.update(cx, indices, vertices);
        let id = geometry.geometry_id();
        self.flare_geometry = Some(geometry);
        id
    }

    /// A pack's prebaked `ao_atlas.png`, uploaded as a texture.
    ///
    /// Looked up beside the pack's models, which is where `tools/ao_bake`
    /// writes it. Greyscale is expanded to RGBA because the texture path is
    /// 32-bit; the shader reads .x.
    pub(super) fn models_root() -> std::path::PathBuf {
        std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../apps/commercial/sandbox/resources/models"
        ))
    }

    pub(super) fn load_pack_ao(cx: &mut Cx, pack: &str) -> Option<Texture> {
        let png = std::fs::read(Self::models_root().join(pack).join("ao_atlas.png")).ok()?;
        Self::gray_png_texture(cx, &png).map(|(t, _, _)| t)
    }

    /// This model's OWN AO atlas, from `tools/ao_bake`, if one was baked.
    /// The dimensions ride along: they are the shape of the model's chart
    /// layout, which is what sizes its lightmap region.
    pub(super) fn load_model_ao(cx: &mut Cx, id: &str) -> Option<(Texture, usize, usize)> {
        let png = std::fs::read(Self::models_root().join(format!("{id}.ao.png"))).ok()?;
        Self::gray_png_texture(cx, &png)
    }

    /// Decode ao_bake's greyscale PNG straight into an R8 texture.
    ///
    /// Single channel on the GPU, where the old path inflated grey to RGBA —
    /// 4x the memory for three copies of the same byte, on the device (Quest)
    /// where texture memory is the scarce resource. The shader reads `.x`
    /// either way. The decoder handles exactly what the baker writes — 8-bit
    /// greyscale, filter 0, zlib IDAT — and rejects anything else, which is
    /// how a foreign or corrupt file falls back to "no AO" rather than to a
    /// scrambled one.
    pub(super) fn gray_png_texture(cx: &mut Cx, png: &[u8]) -> Option<(Texture, usize, usize)> {
        if png.len() < 8 + 25 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
            return None;
        }
        let (mut o, mut w, mut h, mut idat) = (8usize, 0usize, 0usize, Vec::new());
        while o + 8 <= png.len() {
            let len = u32::from_be_bytes(png[o..o + 4].try_into().ok()?) as usize;
            let kind = &png[o + 4..o + 8];
            let body = png.get(o + 8..o + 8 + len)?;
            match kind {
                b"IHDR" => {
                    w = u32::from_be_bytes(body.get(..4)?.try_into().ok()?) as usize;
                    h = u32::from_be_bytes(body.get(4..8)?.try_into().ok()?) as usize;
                    // 8-bit greyscale, no interlace: bit depth 8, colour type
                    // 0, compression 0, filter 0, interlace 0.
                    if body.get(8..13)? != [8, 0, 0, 0, 0] {
                        return None;
                    }
                }
                b"IDAT" => idat.extend_from_slice(body),
                b"IEND" => break,
                _ => {}
            }
            o += 8 + len + 4;
        }
        if w == 0 || h == 0 || w > 8192 || h > 8192 {
            return None;
        }
        let raw = makepad_fast_inflate::zlib_decompress_vec(&idat).ok()?;
        if raw.len() != (w + 1) * h {
            return None;
        }
        let mut pixels = Vec::with_capacity(w * h);
        for row in raw.chunks_exact(w + 1) {
            if row[0] != 0 {
                return None;
            }
            pixels.extend_from_slice(&row[1..]);
        }
        Some((
            Texture::new_with_format(
                cx,
                TextureFormat::VecRu8 {
                    width: w,
                    height: h,
                    data: Some(pixels),
                    unpack_row_length: None,
                    updated: TextureUpdated::Full,
                },
            ),
            w,
            h,
        ))
    }

    /// The prebaked mesh for `id`, if `tools/ao_bake` has produced one.
    pub(super) fn load_aomesh(id: &str) -> Option<StaticModel> {
        let path = Self::models_root().join(format!("{id}.aomesh"));
        StaticModel::from_aomesh(&std::fs::read(path).ok()?)
    }

    pub(super) fn caster_for<'a>(
        programs: &'a std::collections::BTreeMap<String, Box<CustomMaterial>>,
        binding: &CustomMaterialInstance,
    ) -> Option<ResolvedCaster<'a>> {
        let (name, program) = programs.get_key_value(&binding.name)?;
        Some(ResolvedCaster {
            name, shader: program.shadow?, params: binding.params, cutoff: program.cutoff, bounds_pad: program.bounds_pad,
        })
    }

    /// Append a static copy's caster layers. Cascades track their material sources and
    /// displacement bounds. The atlas keeps the first layer and any additional derived
    /// casters on the full-detail model. Far stand-ins serve the outer cascades only.
    pub(super) fn push_static_caster_layers(
        programs: &std::collections::BTreeMap<String, Box<CustomMaterial>>,
        model: &LoadedModel,
        inst: &ModelInstance,
        bounds: (Vec3f, Vec3f),
        out: &mut Vec<crate::gpu_lightmap::GpuBakeMesh>,
        mut sources: Option<(usize, &mut Vec<StaticMaterialSource>)>,
    ) {
        use crate::gpu_lightmap::{CasterBand, GpuBakeMesh};
        let Some((local_min, local_max)) = model.fixed_shadow_bounds() else { return };
        let cascades = sources.is_some();
        let layers = || std::iter::once((model.geometry.as_ref(), &model.texture, &model.material))
            .chain(model.extra_draws.iter().map(|(g, t, _, _, m)| (g.as_ref(), t, m)));
        let stand_in = |m: &LayerMaterial| m.surface.as_ref().is_some_and(|s| s.definition.impostor > 0.0);
        let has_stand_in = cascades && layers().any(|(_, _, m)| m.traced_required.is_none() && stand_in(m));
        let resolve = |binding: &CustomMaterialInstance| {
            let caster = Self::caster_for(programs, binding)?;
            let bounds = if cascades && caster.bounds_pad > 0.0 {
                let pad = Vec3f::all(caster.bounds_pad);
                let (lo, hi) = crate::lightmap::world_bounds(&inst.transform, (local_min - pad, local_max + pad));
                (Vec3f::min_componentwise(bounds.0, lo), Vec3f::max_componentwise(bounds.1, hi))
            } else {
                bounds
            };
            Some((caster, bounds))
        };
        let shared = inst.layer_materials.is_empty().then(|| inst.material_of("").and_then(resolve));
        for (index, (geometry, texture, layer)) in layers().filter(|(_, _, m)| m.traced_required.is_none()).enumerate() {
            let caster = shared.unwrap_or_else(|| inst.material_of(&layer.name).and_then(resolve));
            if !cascades && index != 0 && (caster.is_none() || stand_in(layer)) { continue; }
            let (material, (min, max)) = match caster {
                Some((caster, bounds)) => {
                    if let Some((instance, records)) = &mut sources {
                        records.push(StaticMaterialSource {
                            caster: out.len(), instance: *instance, instance_wide: shared.is_some(),
                            layer: layer.name.clone(), program: caster.name.clone(),
                        });
                    }
                    (Some(caster.material(texture)), bounds)
                }
                None => (None, bounds),
            };
            let surface = layer.surface.as_ref().filter(|_| cascades);
            out.push(GpuBakeMesh {
                geometry: geometry.geometry_id(), transform: inst.transform, min, max, material,
                band: if !has_stand_in { CasterBand::All } else if stand_in(layer) { CasterBand::Far } else { CasterBand::Near },
                cutout: surface.is_some_and(|s| s.definition.alpha_mode == 1).then(|| texture.clone()),
                msdf: surface.and_then(|s| s.definition.msdf.map(|text| (text, s.definition.base_alpha, inst.color_adjust.w))),
            });
            if !cascades && matches!(shared, Some(None)) { break; }
        }
    }

    /// Resolve the static material inputs when models or programs are submitted. Keep
    /// the binding layout separate so editing four parameters reuses the cascade blocks.
    fn static_caster_keys(&self) -> (u64, u64) {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut layout = DefaultHasher::new();
        let mut values = DefaultHasher::new();
        let mut bound = false;
        for copy in &self.gpu_baker.caster_bindings.copies {
            if self.placed_models.get(copy.slot).is_none() { continue }
            let Some(index) = copy.model else { continue };
            let model = &self.static_models[index].1;
            if model.morph.is_some() { continue; }
            let mut record = |layer: Option<usize>, binding: Option<&CustomMaterialInstance>| {
                if let Some(caster) = binding.and_then(|b| Self::caster_for(&self.custom_draws, b)) {
                    bound = true;
                    (copy.slot, layer, caster.name, caster.shader.index, caster.cutoff.to_bits()).hash(&mut layout);
                    let p = caster.params;
                    [p.x, p.y, p.z, p.w].map(f32::to_bits).hash(&mut values);
                }
            };
            // Use the submitted binding snapshot. This also lets a refill compare
            // its old and new layouts with exactly the same retirements applied.
            if copy.layers.is_empty() {
                record(None, copy.material.as_ref());
            } else {
                let layers = std::iter::once(&model.material).chain(model.extra_draws.iter().map(|draw| &draw.4));
                for (i, layer) in layers.enumerate() {
                    let binding = copy.layers.iter().find(|entry| !layer.name.is_empty() && entry.layer == layer.name)
                        .map(|entry| &entry.material).or(copy.material.as_ref());
                    record(Some(i), binding);
                }
            }
        }
        if bound { (layout.finish(), (layout.finish() ^ values.finish()).max(1)) } else { (0, 0) }
    }

    /// Remove a static placed copy from rendering, shadow casting, lighting and
    /// bake inputs until the host hands a changed list (a different placed scene
    /// signature or length). Identical refills and tint changes keep it retired.
    pub fn retire_placed_copy(&mut self, slot: usize) {
        let placed = &mut self.placed_models;
        // Only the host's list can retire; the frame's transient item tail cannot.
        let Some(copy) = placed.copies.get(slot).filter(|_| slot < placed.retired.len()) else {
            if !placed.warned_missing {
                log!("cannot retire missing placed slot {slot}");
                placed.warned_missing = true;
            }
            return;
        };
        if copy.dynamic {
            if !placed.warned_dynamic {
                log!("cannot retire dynamic placed slot {slot}");
                placed.warned_dynamic = true;
            }
            return;
        }
        if placed.retired[slot] { return }
        placed.retired[slot] = true;
        placed.retired_count += 1;
        placed.retirement_revision = placed.retirement_revision.wrapping_add(1);
        // A slot the caster table has not yet covered has no casters to empty.
        let statics = self.placed_static_slots.get(slot);
        if statics.is_some_and(|s| s.lamp) {
            placed.retired_lamp = true;
            if self.lm_lights.is_empty() { self.lamp_cache_rev = None; }
        }
        let casters = statics.and_then(|s| self.placed_caster_indices.get(s.casters.clone())).unwrap_or(&[]);
        for &index in casters {
            if let Some(caster) = self.csm_static_casters.get_mut(index) {
                caster.min = Vec3f::all(f32::INFINITY);
                caster.max = Vec3f::all(f32::NEG_INFINITY);
            }
        }
        self.placed_blocks.remove(slot, copy.has_custom());
        self.gpu_baker.csm_casters_changed();
        self.models_gi_rev = self.models_gi_rev.wrapping_add(1);
    }

    pub(super) fn bake_models_revision(&self) -> u64 {
        self.models_rev.wrapping_add(self.placed_models.retirement_revision)
    }

    /// Rebuild the static half of Realtime's CSM caster list.
    ///
    /// With a world atlas, charted models already become `BakeState` meshes;
    /// without one this list owns those casters too, including every layer. Keeping it
    /// next to placed-scene identity makes a scene upload the registration
    /// boundary instead of turning static architecture into a per-frame
    /// "mover" merely to get a shadow.
    pub(super) fn rebuild_csm_static_casters(&mut self) {
        self.gpu_baker.csm_casters_changed();
        self.csm_static_casters.clear();
        self.csm_material_sources.clear();
        // First entry per id, as the linear find it replaces (a forest is
        // thousands of copies of a few models; every model install rebuilds).
        let mut by_id: std::collections::HashMap<&str, usize> = std::collections::HashMap::with_capacity(self.static_models.len());
        for (i, (id, _)) in self.static_models.iter().enumerate() {
            by_id.entry(id.as_str()).or_insert(i);
        }
        self.placed_static_slots.clear();
        self.placed_static_slots.resize_with(self.placed_models.len(), Default::default);
        let mut static_by_model = std::mem::take(&mut self.placed_models.static_by_model);
        let has_traced = self.static_models.iter().any(|(_, root)| std::iter::once(root)
            .chain(root.lods.iter().map(|(_, m)| m)).any(|m| m.ordinary_shadow_bounds.is_some()));
        if has_traced {
            static_by_model.resize_with(self.static_models.len(), Vec::new);
            for slots in &mut static_by_model { slots.clear(); }
        } else {
            static_by_model.clear();
        }
        for (instance, inst) in self.placed_models.live().filter(|(_, inst)| !inst.dynamic) {
            let Some(&index) = by_id.get(inst.model.as_str()) else { continue };
            let model = &self.static_models[index].1;
            if has_traced { static_by_model[index].push(instance); }
            self.placed_static_slots[instance].lamp = Self::harvests_lamp(&inst.model);
            if model.morph.is_some(){continue;}
            if model.lm_source.is_some() && self.world_atlas_required() {
                // This copy casts through its region mesh in the realized atlas.
                continue;
            }
            let Some(bounds) = model.fixed_shadow_bounds() else { continue };
            let (mut min, mut max) = crate::lightmap::world_bounds(&inst.transform, bounds);
            if model.lm_source.is_none() && !casts_as_caster_only(
                self.model_casts_shadow.get(&inst.model).copied(),
                model.prelit,
                min,
                max,
            ) {
                continue;
            }
            if self.model_casts_shadow.get(&inst.model) == Some(&false) { continue; }
            if let Some((lo, hi)) = self.element_reach {
                min = vec3f(min.x.min(lo.x), min.y.min(lo.y), min.z.min(lo.z));
                max = vec3f(max.x.max(hi.x), max.y.max(hi.y), max.z.max(hi.z));
            }
            let first = self.csm_static_casters.len();
            Self::push_static_caster_layers(
                &self.custom_draws, model, inst, (min, max), &mut self.csm_static_casters,
                Some((instance, &mut self.csm_material_sources)),
            );
            self.placed_static_slots[instance].casters = first..self.csm_static_casters.len();
        }
        self.placed_models.static_by_model = static_by_model;
        // Equal geometry adjacent, opaque first: a cascade then appends a
        // forest's copies of one layer into ONE instanced draw instead of
        // alternating trunk, crown, trunk, crown (a draw each). Within a
        // layer the copies go in spatial order (a Z-order of 32 m cells),
        // so the blocks of consecutive casters that the cascades cull
        // first are small patches of ground (depth is order-free).
        let mut first: std::collections::HashMap<GeometryId, usize> = std::collections::HashMap::new();
        for (i, m) in self.csm_static_casters.iter().enumerate() { first.entry(m.geometry).or_insert(i); }
        let cell = |m: &crate::gpu_lightmap::GpuBakeMesh| {
            let c = (m.min + m.max) * 0.5;
            let spread = |v: f32| {
                let mut x = ((v / 32.0).floor() as i64 + (1 << 15)).clamp(0, (1 << 16) - 1) as u64;
                x = (x | (x << 8)) & 0x00ff_00ff;
                x = (x | (x << 4)) & 0x0f0f_0f0f;
                x = (x | (x << 2)) & 0x3333_3333;
                (x | (x << 1)) & 0x5555_5555
            };
            spread(c.x) | spread(c.z) << 1
        };
        // Cache keys before comparing (a lookup per comparison was seconds of a city's load).
        let key = |m: &crate::gpu_lightmap::GpuBakeMesh| {
            (m.cutout.is_some() || m.material.is_some(), first[&m.geometry], cell(m))
        };
        let mut numbered: Vec<_> = self.csm_static_casters.drain(..).enumerate().collect();
        numbered.sort_by_cached_key(|(_, mesh)| key(mesh));
        self.placed_caster_indices.resize(numbered.len(), 0);
        for (index, (old, mesh)) in numbered.into_iter().enumerate() {
            self.placed_caster_indices[old] = index;
            self.csm_static_casters.push(mesh);
        }
        for source in &mut self.csm_material_sources {
            source.caster = self.placed_caster_indices[source.caster];
        }
        self.csm_material_sources.sort_unstable_by_key(|s| s.caster);
        self.csm_static_blocks = crate::gpu_lightmap::caster_blocks(&self.csm_static_casters);
        self.csm_custom_casters_rev = self.custom_casters_rev;
        let bindings = &mut self.gpu_baker.caster_bindings;
        bindings.tracking = self.custom_draws.values().any(|program| program.shadow.is_some());
        if bindings.tracking {
            bindings.update(&self.placed_models, 0, |id| by_id.get(id).copied());
            // A model upload or replacement can also move an existing slot in the catalogue.
            for copy in &mut bindings.copies {
                copy.model = self.placed_models.get(copy.slot).and_then(|m| by_id.get(m.model.as_str()).copied());
            }
        } else {
            bindings.copies.clear();
        }
        let (layout, key) = self.static_caster_keys();
        self.gpu_baker.set_caster_keys(layout, key);
    }

    /// Hand this frame's stock props to the renderer. Draw submission is
    /// batched by model, but list order is still scene identity: lightmap
    /// remaps are indexed by placed slot. Producers should therefore keep a
    /// stable order so a harmless reorder does not request a new bake.
    /// [`Self::set_models`] for a host that rebuilds its list every frame:
    /// `fill` gets last frame's list to overwrite in place (with
    /// `clone_from`, which keeps each copy's strings), and the result is
    /// set exactly as `set_models` would.
    pub fn rebuild_models(&mut self, fill: impl FnOnce(&mut Vec<ModelInstance>)) {
        // The host refills the complete authoritative list, preserving its slot numbers.
        let mut instances = self.placed_models.take_all_including_retired();
        fill(&mut instances);
        self.set_models(instances);
    }

    /// [`Self::rebuild_models`] for a host whose list starts with copies it
    /// can vouch for: while `prefix_key` is last frame's, last frame's
    /// leading copies are kept as they are (neither refilled nor hashed
    /// again) and only `tail` appends this frame's rest; otherwise `prefix`
    /// fills the list first. A level's placed forest is thousands of copies
    /// that change only when the level does. The tail is refilled in place
    /// over last frame's ([`ModelTail::push`]), so a stable tail allocates
    /// nothing either.
    pub fn rebuild_models_with_prefix(&mut self, prefix_key: u64, prefix: impl FnOnce(&mut Vec<ModelInstance>), tail: impl FnOnce(&mut ModelTail)) {
        self.rebuild_model_parts(prefix_key, |instances| {
            instances.clear();
            prefix(instances);
            instances.len()
        }, tail);
    }

    /// Refill changed leading copies and the dynamic tail over their previous slots.
    pub fn refill_models_with_prefix(&mut self, prefix_key: u64, prefix: impl FnOnce(&mut ModelTail), tail: impl FnOnce(&mut ModelTail)) {
        self.rebuild_model_parts(prefix_key, |instances| {
            let mut head = ModelTail { list: instances, start: 0, at: 0 };
            prefix(&mut head);
            head.at
        }, tail);
    }

    fn rebuild_model_parts(&mut self, prefix_key: u64, prefix: impl FnOnce(&mut Vec<ModelInstance>) -> usize, tail: impl FnOnce(&mut ModelTail)) {
        // The host refills the complete authoritative list, preserving its slot numbers.
        let mut instances = self.placed_models.take_all_including_retired();
        let (len, prefix_signatures, kept) = match self.placed_prefix {
            Some((key, len, signatures)) if key == prefix_key && instances.len() >= len => {
                (len, signatures, len)
            }
            _ => {
                let len = prefix(&mut instances);
                (len, extend_placed_signatures((0, 0), &instances[..len]), 0)
            }
        };
        self.placed_prefix = Some((prefix_key, len, prefix_signatures));
        let mut t = ModelTail { list: &mut instances, start: len, at: len };
        tail(&mut t);
        let end = t.at;
        instances.truncate(end);
        let signatures = extend_placed_signatures(prefix_signatures, &instances[len..]);
        if let Err(error) = self.try_set_models_signed(instances, signatures, kept) {
            self.report_asset_light_error(error);
        }
    }

    /// The host's list as handed, in slot order, including retired copies.
    pub fn placed_models(&self) -> &[ModelInstance] {
        self.placed_models.all_including_retired()
    }

    pub(super) fn lm_remap_owner(&self, copy: &ModelInstance) -> Option<LmRemapOwner> {
        if copy.dynamic { return None; }
        Some(LmRemapOwner {
            chart_key: *self.lm_model_charts.get(&copy.model)?,
            transform: copy.transform.v.map(f32::to_bits),
        })
    }

    pub(super) fn rekey_lm_remaps(&mut self) {
        if self.lightmap.is_none() || self.lm_remaps_rev == Some(self.models_rev) {
            return;
        }
        // Coincident copies can have the same owner. Chain their regions in
        // slot order so each old region goes to at most one current copy.
        let mut heads = std::collections::HashMap::new();
        let mut regions = Vec::with_capacity(self.lm_remaps.len());
        for (owner, rect) in self.lm_remap_owners.drain(..).zip(self.lm_remaps.drain(..)).rev() {
            if let Some(owner) = owner {
                let head = heads.entry(owner).or_insert(None);
                let next = head.replace(regions.len());
                regions.push((rect, next));
            }
        }
        self.lm_remaps.resize(self.placed_models.len(), Vec4f::default());
        self.lm_remap_owners.resize(self.placed_models.len(), None);
        for (slot, copy) in self.placed_models.live() {
            let owner = self.lm_remap_owner(copy);
            let rect = owner.and_then(|owner| {
                let head = heads.get_mut(&owner)?;
                let (rect, next) = regions[(*head)?];
                *head = next;
                Some(rect)
            });
            self.lm_remaps[slot] = rect.unwrap_or_default();
            self.lm_remap_owners[slot] = owner.filter(|_| rect.is_some());
        }
        self.lm_remaps_rev = Some(self.models_rev);
        self.lm_remaps_generation = self.lm_remaps_generation.wrapping_add(1);
    }

    pub fn set_models(&mut self, instances: Vec<ModelInstance>) {
        if let Err(error)=self.try_set_models(instances){self.report_asset_light_error(error);}
    }
    pub fn asset_light_error(&self)->Option<&str>{self.asset_light_error.as_deref()}
    pub(super) fn report_asset_light_error(&mut self,error:String){if self.asset_light_error.as_ref()!=Some(&error){log!("asset lights degraded: {error}");}self.asset_light_error=Some(error);}
    /// Authored punctual lights never refuse a scene. Past
    /// [`crate::asset_lights::MAX_ASSET_FRAME_LIGHTS`] the frame keeps the
    /// ones that matter most at the eye and fades the tail out
    /// (`budget_authored_lights`); without the clustered renderer they are
    /// skipped. This only reports the degradation, once per change.
    pub fn check_asset_light_count(&self,count:usize)->Result<(),String>{
        if count>crate::asset_lights::MAX_ASSET_FRAME_LIGHTS{return Err(format!("{count} authored lights: the {} that matter most at the eye are lit, the rest fade out",crate::asset_lights::MAX_ASSET_FRAME_LIGHTS))}
        if count>0&&!self.clustered_enabled{return Err("authored punctual lights need the clustered renderer; they are skipped".into())}Ok(())
    }
    pub fn try_set_models(&mut self, instances: Vec<ModelInstance>)->Result<(),String> {
        let signatures = extend_placed_signatures((0, 0), &instances);
        self.placed_prefix = None;
        self.try_set_models_signed(instances, signatures, 0)
    }

    fn try_set_models_signed(&mut self, instances: Vec<ModelInstance>, (signature, paint_signature): (u64, u64), kept: usize) -> Result<(), String> {
        self.install_placed_list(instances, signature, paint_signature, kept, false);
        Ok(())
    }

    fn install_placed_list(&mut self, instances: Vec<ModelInstance>, signature: u64, paint_signature: u64, kept: usize, restore_review_slots: bool) {
        let scene_changed = self.placed_scene_signature != Some(signature)
            || instances.len() != self.placed_models.retired.len();
        let restoring = self.placed_models.retired_count != 0 && (scene_changed || restore_review_slots);
        if restoring || self.placed_paint_signature != Some(paint_signature) {
            self.models_gi_rev = self.models_gi_rev.wrapping_add(1);
            self.placed_paint_signature = Some(paint_signature);
        }
        // Statics are cached against a key; any meaningful placed-scene
        // change must break it or an equal-length replacement would retain
        // the previous realm's lamps and shadows.
        if scene_changed {
            // The lightmap regions are matched to the new list by owner on the
            // next frame (`rekey_lm_remaps`); the models_rev bump re-kicks the
            // bake once the world settles.
            self.models_rev = self.models_rev.wrapping_add(1);
            self.placed_scene_signature = Some(signature);
            // Per-slot door commands are indexed by placed position and go
            // stale with a changed list. Per-model ones do not — an imported
            // level keeps its doors open across a harmless list rebuild.
            self.model_anim_state.forget_slots();
        }
        if restoring {
            self.placed_blocks.invalidate();
            if self.placed_models.retired_lamp && self.lm_lights.is_empty() { self.lamp_cache_rev = None; }
        }
        self.placed_models.replace(instances, restoring);
        if restoring || scene_changed || self.custom_casters_rev != self.csm_custom_casters_rev {
            self.rebuild_csm_static_casters();
            return;
        }
        let bindings = &self.gpu_baker.caster_bindings;
        if !bindings.tracking { return; }
        let previous_layout = if self.placed_models.retired_count == 0 {
            bindings.layout
        } else {
            if bindings.live_suffix_matches(&self.placed_models, kept) { return; }
            // The cached layout can include a caster that has since retired.
            // Compare live layouts only when a submitted live binding changed.
            self.static_caster_keys().0
        };
        if self.gpu_baker.caster_bindings.update(&self.placed_models, kept, |id| {
            self.static_models.iter().position(|(name, _)| name == id)
        }) {
            let (layout, key) = self.static_caster_keys();
            if layout != previous_layout {
                self.rebuild_csm_static_casters();
            } else if key != self.gpu_baker.caster_bindings.key {
                self.gpu_baker.set_caster_keys(layout, key);
                self.refresh_csm_materials();
            }
        }
    }

    /// Rebuild the static casters when a program with a derived caster was
    /// installed, replaced or dropped since they resolved theirs: once,
    /// before the cascades draw.
    /// A review camera borrows the host's programs only while it draws
    /// (`swap_review_materials`), so its casters resolve them here too.
    pub(super) fn sync_csm_custom_casters(&mut self) {
        if self.csm_custom_casters_rev != self.custom_casters_rev {
            self.rebuild_csm_static_casters();
        }
    }

    fn refresh_csm_materials(&mut self) {
        let mut params_changed = false;
        for source in &self.csm_material_sources {
            let Some(inst) = self.placed_models.get(source.instance) else { continue };
            let layer = if source.instance_wide && inst.layer_materials.is_empty() { "" } else { source.layer.as_ref() };
            let binding = inst.material_of(layer);
            let Some(binding) = binding.filter(|binding| binding.name == source.program) else {
                self.rebuild_csm_static_casters();
                return;
            };
            let material = self.csm_static_casters[source.caster].material.as_mut().expect("registered material caster");
            if material.params != binding.params {
                material.params = binding.params;
                params_changed = true;
            }
        }
        if params_changed {
            self.gpu_baker.csm_casters_changed();
        }
    }

    /// Independent camera/lighting caches for an on-demand scene review.
    /// Meshes, textures and immutable metadata share resident handles; the
    /// live renderer's GI, camera and simulation state remain untouched.
    /// The environment is the live renderer's (I4): the fork is born
    /// mirroring it ([`Self::mirror_environment_from`]) and never prepares
    /// one of its own, so a `game.environment` world draws with its map in
    /// every view; a fork kept across frames mirrors it again each frame.
    pub fn fork_scene_for_review(&self) -> Self {
        let mut review = Self::default();
        // The review draws the same scene: its uploads follow the host's plan too.
        review.texture_first_level = self.texture_first_level.clone();
        review.static_models = self.static_models.clone();
        review.lm_model_charts.clone_from(&self.lm_model_charts);
        review.model_casts_shadow = self.model_casts_shadow.clone();
        review.model_anim_state = self.model_anim_state.clone();
        review.ao_textures = self.ao_textures.clone();
        review.model_pack = self.model_pack.clone();
        review.model_lod_chains = self.model_lod_chains.clone();
        review.model_lod_chains_dirty = self.model_lod_chains_dirty;
        review.skin_rig_geometries = self.skin_rig_geometries.clone();
        review.skin_material_draws = self.skin_material_draws.clone();
        review.skin_lods = self.skin_lods.clone();
        review.skin_morphs = self.skin_morphs.clone();
        review.skin_prepared_sdf = self.skin_prepared_sdf.clone();
        review.copy_review_models(self);
        review.set_world_attachments(self.world_attachments.clone());
        review.mirror_traced_programs_from(self);
        review.host_lights = self.host_lights.clone();
        review.host_asset_lights = self.host_asset_lights.clone();
        review.sky_time = self.sky_time;
        review.wind = self.wind;
        review.wind_clock = self.wind_clock;
        review.sky_clock = self.sky_clock;
        review.sky_hour = self.sky_hour;
        review.set_hdr_output(self.hdr_output);
        review.set_camera_relative(self.camera_relative());
        review.set_graphics_tier(self.graphics_tier);
        let csm = self.csm_config();
        review.set_csm_config(csm.tile_resolution, csm.far_range);
        review.set_cluster_config(self.clustered.config());
        review.set_local_shadow_config(self.clustered.shadows.config());
        review.pbr_materials_enabled = self.pbr_materials_enabled;
        review.lightmap_enabled = self.lightmap_enabled;
        review.display_transform = self.display_transform;
        review.output_dither = self.output_dither;
        review.fxaa = self.fxaa;
        review.bloom = self.bloom;
        review.auto_exposure = self.auto_exposure;
        review.grade = self.grade;
        review.clustered.shadows.soft_filter = self.clustered.shadows.soft_filter;
        review.clustered.shadows.source_radius = self.clustered.shadows.source_radius;
        review.star_texture = self.star_texture.clone();
        review.star_map_loaded = self.star_map_loaded;
        // Scene state the host's lanes rely on in every view: which elements the passes keep and
        // cast (the element lookup with its casters' key) and the silhouette tier's caster budget.
        review.set_element_lookup(self.element_lookup.clone(), self.gpu_baker.element_lookup_casters());
        review.set_shadow_budget(self.shadow_budget);
        review.mirror_stream_from(self);
        review.mirror_review_shadows_from(self, true);
        review.set_gi_mode(crate::GiMode::Off);
        review.mirror_environment_from(self);
        review
    }

    /// Custom draw handles stay UI-owned; lend them only while encoding a
    /// review pass, then restore them before encoding the player's pass.
    pub fn swap_review_materials(&mut self, other: &mut Self) {
        std::mem::swap(&mut self.custom_draws, &mut other.custom_draws);
        std::mem::swap(&mut self.custom_casters_rev, &mut other.custom_casters_rev);
        // UploadedSurface already shares its albedo/normal arrays and
        // weight map. The missing state was the prepared material lane:
        // a fresh fork can fall back while its pipeline is compiling.
        std::mem::swap(&mut self.pbr_draw, &mut other.pbr_draw);
        std::mem::swap(&mut self.splat_draw, &mut other.splat_draw);
        std::mem::swap(&mut self.foliage_draw, &mut other.foliage_draw);
        std::mem::swap(&mut self.lane_variants, &mut other.lane_variants);
        std::mem::swap(&mut self.grass, &mut other.grass);
        std::mem::swap(&mut self.grass_draw, &mut other.grass_draw);
    }

    /// Lend the live GI probe field during a secondary scene draw. An atlas-borrowing
    /// fork also reads the host's baked entity shade and mover probes for this draw.
    /// Call again immediately afterwards to restore both. The secondary camera
    /// keeps its own SSAO and skips relighting, recentering and voxel work.
    pub fn swap_review_gi(&mut self, other: &mut Self) {
        std::mem::swap(&mut self.gi, &mut other.gi);
        if self.atlas_borrowed {
            std::mem::swap(&mut self.bake, &mut other.bake);
        }
        self.review_gi = !self.review_gi;
    }

    /// Whether resident specialized material lanes could actually draw.
    pub fn review_materials_ready(&self) -> bool {
        if !self.pbr_materials_enabled { return true; }
        // A traced exterior draws nothing until its program is ready on this renderer's target.
        // Only programs the live renderer itself has ready can be waited for: one it never
        // gets ready (or has failed) must not hold a review forever.
        if self.traced_programs.iter().any(|(name, program)| {
            program.source_ready && program.passes.failure(self.gpu_baker.realtime_sun_active()).is_none()
                && !matches!(self.traced_surface_state(name), TracedSurfaceState::Ready)
        }) {
            return false;
        }
        let used: std::collections::HashSet<&str> = self.placed_models.live().map(|(_, m)| m).chain(&self.world_attachments)
            .map(|m| m.model.as_str()).collect();
        self.static_models.iter().filter(|(id, _)| used.contains(id.as_str())).all(|(_, model)| {
            std::iter::once(model).chain(model.lods.iter().map(|(_, m)| m)).all(|m| {
                let layers = || std::iter::once(&m.material).chain(m.extra_draws.iter().map(|l| &l.4));
                (!m.wants_pbr || self.pbr_ready)
                    && (self.splat_ready || !layers().any(|m| m.surface.as_ref().is_some_and(|s| s.splat.is_some())))
                    // A model with a far stand-in draws its own layers at
                    // every distance until the stand-in's shader exists: a
                    // picture taken then shows the mesh where the stand-in
                    // belongs, and the stand-in pops in a frame later.
                    && (self.impostor_shader.is_some() || !layers().any(|m| m.surface.as_ref().is_some_and(|s| s.definition.impostor > 0.0)))
            })
        })
    }

    fn mirror_review_shadows_from(&mut self, live: &Self, live_light: bool) {
        use crate::GpuLightmapMode::{OnChange, Realtime};

        // Realtime with clustering off casts charted meshes from the baker's
        // private state. Keep clustered Realtime for that case until it can be shared.
        // The host's atlas holds its own sun's light: a camera lit some other
        // way (a neutral map) draws its own realtime shadows instead.
        let borrow = live_light && live.gpu_lightmap_mode() == OnChange;
        let mode = if borrow { OnChange } else { Realtime };
        let clustered = !borrow || live.clustered_lighting();
        if self.gpu_lightmap_mode() != mode {
            self.set_gpu_lightmap_mode(mode);
        }
        if self.clustered_lighting() != clustered {
            self.set_clustered_lighting(clustered);
        }
        if self.atlas_borrowed != borrow {
            // The two CPU bakes can have the same generation but different slab colours.
            self.slab_key = None;
            self.atlas_borrowed = borrow;
            self.copied_lm_remaps_generation = None;
        }
        if borrow
            && (self.lightmap.as_ref().map(Texture::texture_id)
                != live.lightmap.as_ref().map(Texture::texture_id)
                || self.copied_lm_remaps_generation != Some(live.lm_remaps_generation))
        {
            self.lightmap.clone_from(&live.lightmap);
            self.lm_remaps.clone_from(&live.lm_remaps);
            self.lm_remap_owners.clone_from(&live.lm_remap_owners);
            // The live slots may still index its previous list. This view
            // resolves them against its own copies before drawing.
            self.lm_remaps_rev = None;
            self.lm_remaps_generation = self.lm_remaps_generation.wrapping_add(1);
            self.copied_lm_remaps_generation = Some(live.lm_remaps_generation);
            self.lm_ground = live.lm_ground;
            self.lm_top.clone_from(&live.lm_top);
        }
    }

    fn copy_review_models(&mut self, live: &Self) {
        // Review cameras address animation, pick IDs and atlas regions by the host's original slots.
        let source = live.placed_models.all_including_retired();
        let mut copies = self.placed_models.take_all_including_retired();
        refill_from_slice(&mut copies, source);
        let (scene, paint) = live.placed_scene_signature.zip(live.placed_paint_signature)
            .unwrap_or_else(|| extend_placed_signatures((0, 0), &copies));
        // A review can miss a host edit and its reversal between frames. Restore
        // stale review tombstones if the host now shows any of those slots.
        let restore = self.placed_models.retired_count != 0 && self.placed_models.retired.iter()
            .zip(&live.placed_models.retired).any(|(&old, &new)| old && !new);
        self.placed_prefix = None;
        self.install_placed_list(copies, scene, paint, 0, restore);
        if self.placed_models.retired_count < live.placed_models.retired_count {
            for (slot, &retired) in live.placed_models.retired.iter().enumerate() {
                if retired { self.retire_placed_copy(slot); }
            }
        }
    }

    /// Current poses and resolved lights for another camera. Called only
    /// when that camera renders, after the source scene has resolved them.
    pub fn mirror_review_frame_from(&mut self, live: &Self) {
        self.copy_review_models(live);
        self.mirror_review_shadows_from(live, true);
        let mut attachments = std::mem::take(&mut self.world_attachments);
        refill_from_slice(&mut attachments, &live.world_attachments);
        self.set_world_attachments(attachments);
        self.mirror_traced_programs_from(live);
        self.model_anim_state = live.model_anim_state.clone();
        self.sky_time = live.sky_time;
        self.wind = live.wind;
        self.wind_clock = live.wind_clock;
        self.sky_clock = live.sky_clock;
        self.sky_hour = live.sky_hour;
        self.review_lights = Some(live.frame_lights.clone());
        self.trample.mirror_from(&live.trample);
        self.grade = live.grade;
        self.bloom = live.bloom;
        self.fxaa = live.fxaa;
    }

    /// Refresh uploads that completed between an on-demand camera's warmup
    /// and capture. Geometry/texture handles remain shared; never decode a
    /// second copy or keep a replaced generated surface in the fork.
    /// `live_light`: the capture draws in the host's light, so it follows the
    /// host's shadow tier; otherwise (a neutral map) it casts its own.
    pub fn mirror_review_assets_from(&mut self, live: &Self, live_light: bool) {
        // A neutral map capture refreshes assets but skips the live pose/light mirror.
        self.mirror_review_shadows_from(live, live_light);
        let same = self.static_models.len() == live.static_models.len()
            && self.static_models.iter().zip(&live.static_models).all(|((id, a), (other, b))| {
                id == other && a.geometry.geometry_id() == b.geometry.geometry_id()
                    && a.texture.texture_id() == b.texture.texture_id() && a.same_shadow_layers(b)
            });
        // A change of which models cast does not change the models themselves.
        // Same invalidation as `set_model_casts_shadow`.
        if self.model_casts_shadow != live.model_casts_shadow {
            self.model_casts_shadow.clone_from(&live.model_casts_shadow);
            self.placed_scene_signature = None;
            self.models_rev = self.models_rev.wrapping_add(1);
            self.rebuild_csm_static_casters();
        }
        if same { return; }
        self.static_models.clone_from(&live.static_models);
        self.lm_model_charts.clone_from(&live.lm_model_charts);
        self.model_casts_shadow.clone_from(&live.model_casts_shadow);
        self.ao_textures.clone_from(&live.ao_textures);
        self.model_pack.clone_from(&live.model_pack);
        self.model_lod_chains.clone_from(&live.model_lod_chains);
        self.model_lod_chains_dirty = live.model_lod_chains_dirty;
        self.models_rev = self.models_rev.wrapping_add(1);
        self.models_gi_rev = self.models_gi_rev.wrapping_add(1);
        self.rebuild_csm_static_casters();
    }

    pub fn review_models_ready(&self) -> bool {
        self.placed_models.live().map(|(_, m)| m).chain(&self.world_attachments).all(|instance|self.model_is_loaded(&instance.model))
    }

    /// Visible resident model bounds, including rotation and instance scale.
    pub fn placed_scene_bounds(&self) -> Option<(Vec3f, Vec3f)> {
        let mut min = vec3f(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = vec3f(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for instance in self.placed_models.live().map(|(_, m)| m).chain(&self.world_attachments) {
            let Some((a,b)) = self.model_bounds(&instance.model) else { continue; };
            for x in [a.x,b.x] { for y in [a.y,b.y] { for z in [a.z,b.z] {
                let p = instance.transform.transform_vec4(vec4(x,y,z,1.));
                min.x=min.x.min(p.x);min.y=min.y.min(p.y);min.z=min.z.min(p.z);
                max.x=max.x.max(p.x);max.y=max.y.max(p.y);max.z=max.z.max(p.z);
            }}}
        }
        min.x.is_finite().then_some((min,max))
    }

    /// Install only a successfully frontend-compiled material. Failure keeps
    /// the previous draw (or the stock fallback), never a blank model.
    pub fn install_custom_material(&mut self, name: String, material: CustomMaterial) -> bool {
        if !material.draw.draw_vars.can_instance() { return false; }
        if material.shadow.is_some() || self.custom_draws.get(&name).is_some_and(|old| old.shadow.is_some()) {
            self.custom_casters_rev += 1;
        }
        self.custom_draws.insert(name, Box::new(material));
        true
    }

    /// Build `material`'s shader variants for the `set`th feature set a
    /// world's frames draw it with (renderer/variants.rs) before it
    /// is installed; false past the last set. Its first draw builds them
    /// otherwise, a 10-15 ms step for a lit material in the first frame
    /// that shows it. A host's load prepares one set per staging step, so
    /// that frame only draws.
    pub fn prepare_custom_material(&self, cx: &mut Cx, material: &mut CustomMaterial, set: usize) -> bool {
        let Some(features) = self.world_frame_features().get(set).copied() else { return false };
        material.shaders(cx, features);
        true
    }

    /// Make the light map's draws (its 18 depth, gather, filter and encode
    /// shaders) before a world's first frame needs them: that frame made
    /// them otherwise, about 22 ms of a cold launch's first world frame. A
    /// host's load makes them while it stages. True when they were made by
    /// this call; false when they are there already or the VM is busy (a
    /// world's first frame makes them then).
    pub fn prepare_lightmap_draws(&mut self, cx: &mut Cx) -> bool {
        !self.gpu_baker.has_draws() && self.gpu_baker.ensure_draws(cx)
    }

    /// Make the post chain's and the screen AO's draws, and with `lamp_shadows` (the world
    /// has shadowed lamps) the lamp shadows' depth draws, before a world's first frame needs
    /// them: that frame made them otherwise, 13 shaders and about 7 ms of a cold launch's
    /// first world frame. Their passes and targets wait for that frame. False when they are
    /// there already.
    pub fn prepare_post_draws(&mut self, cx: &mut Cx, lamp_shadows: bool) -> bool {
        let lamp_shadows = lamp_shadows && self.clustered_enabled;
        if self.post.draws_ready() && self.gi.screen_ao_draws_ready() && (!lamp_shadows || self.clustered.shadows.draws_ready()) {
            return false;
        }
        self.post.prepare_draws(cx);
        self.gi.prepare_screen_ao_draws(cx);
        if lamp_shadows {
            self.clustered.shadows.prepare_draws(cx);
        }
        true
    }

    /// Bind `texture` on the `detail_map` of the custom material `name`
    /// (the program samples it as `self.detail_map`: a planar reflection, a
    /// screen) until changed; `None` unbinds it. It replaces the IBL there.
    pub fn set_custom_material_texture(&mut self, name: &str, texture: Option<Texture>) {
        if let Some(m) = self.custom_draws.get_mut(name) {
            m.texture = texture;
        }
    }

    /// Install the error material (hatched magenta) under `name`: what a
    /// preview shows for a material that did not compile, and what a host
    /// draws for one that failed after it was installed
    /// ([`Self::take_custom_material_failures`]), which no last good
    /// material stands in for.
    pub fn install_error_material(&mut self, cx: &mut Cx, name: String) -> bool {
        match cx.try_with_vm(crate::custom_material::DrawSceneCustom::error_material) {
            Some(Ok(material)) => self.install_custom_material(name, material),
            _ => false,
        }
    }

    /// Each installed material that turned out not to draw as written since
    /// the last call, once, with why (a feature variant that did not build,
    /// or a pipeline the backend's compiler refused, in its own words).
    /// Such a material draws its stock look meanwhile; a host shows the
    /// author the reason and may install the error material in its place.
    /// Every frame: a few lookups per installed material, no allocation.
    pub fn take_custom_material_failures(&mut self, cx: &Cx, mut failed: impl FnMut(&str, &str)) {
        for (name, m) in self.custom_draws.iter_mut() {
            if let Some(why) = m.take_failure(cx) {
                failed(name, why);
            }
        }
    }

    /// The installed material by name (its plan, variants and warnings).
    pub fn custom_material(&self, name: &str) -> Option<&CustomMaterial> {
        self.custom_draws.get(name).map(|m| &**m)
    }

    /// Keep only the installed materials `names` lists (a host's reload:
    /// the ones it still declares). The stock IBL program goes with the
    /// rest and the next frame under an environment builds it again, also
    /// when its last build failed (renderer/stock_ibl.rs).
    pub fn retain_custom_materials(&mut self, names: &[String]) {
        let mut casters_dropped = false;
        self.custom_draws.retain(|name, program| {
            let keep = names.contains(name);
            casters_dropped |= !keep && program.shadow.is_some();
            keep
        });
        if casters_dropped {
            self.custom_casters_rev += 1;
        }
        self.stock_ibl_forget_failure();
    }

    /// The installed material's stock shader (its colour program built for
    /// every feature). A frame draws through its variant for that frame's
    /// lighting, made after it, so this pipeline being ready does not mean
    /// the material draws: a host that waits for its look asks
    /// [`Self::items_ready`] and [`Self::drew_fallback`].
    pub fn custom_material_shader(&self, name: &str) -> Option<DrawShaderId> {
        self.custom_draws.get(name).and_then(|m| m.stock_shader())
    }

    #[allow(clippy::too_many_arguments)]
    /// The swaying-foliage pass (shaders/foliage.rs): the same instance
    /// list, taking only the models with a wind layer. Until its pipeline
    /// exists (Metal compiles asynchronously) they stay on the PBR lane.
    pub(super) fn draw_foliage_models(
        &mut self, cx: &mut Cx3d, eye: Vec3f, lod_scale: f32, instances: &(impl ModelList + ?Sized),
        lane: WorldModelLane, fog: (Vec3f, f32), sun: &SunLight,
        frustum: Option<&Frustum>, stats: &mut RenderStats,
    ) {
        if !self.foliage_ready { return; }
        let Some(mut draw) = self.foliage_draw.take() else { return };
        self.draw_models_inner(cx, ModelDraw::Foliage(&mut draw), LayerPhase::Opaque, eye, lod_scale, instances, lane, fog, sun, frustum, stats);
        self.foliage_draw = Some(draw);
    }

    /// The terrain paint pass (shaders/splat.rs): the same instance list,
    /// taking only the models with a painted layer. Until its pipeline
    /// exists they stay on the PBR lane (their main layer). `focal_px`: the
    /// view's, for the grass's far carpet (`set_grass_carpet`).
    pub(super) fn draw_splat_models(
        &mut self, cx: &mut Cx3d, eye: Vec3f, lod_scale: f32, instances: &(impl ModelList + ?Sized),
        lane: WorldModelLane, fog: (Vec3f, f32), sun: &SunLight,
        frustum: Option<&Frustum>, focal_px: f32, stats: &mut RenderStats,
    ) {
        if !self.splat_ready { return; }
        let Some(mut draw) = self.splat_draw.take() else { return };
        self.set_grass_carpet(cx.cx, &mut draw, focal_px);
        self.draw_models_inner(cx, ModelDraw::Splat(&mut draw), LayerPhase::Opaque, eye, lod_scale, instances, lane, fog, sun, frustum, stats);
        self.splat_draw = Some(draw);
    }

    /// Whether a resident model has a layer `holds` says yes to, in any of
    /// its levels of detail: a lane's shaders are built for a model that
    /// draws through them, no sooner.
    pub(super) fn a_resident_layer(&self, holds: impl Fn(&crate::material_surface::UploadedSurface) -> bool) -> bool {
        fn any(model: &LoadedModel, holds: &impl Fn(&crate::material_surface::UploadedSurface) -> bool) -> bool {
            std::iter::once(&model.material).chain(model.extra_draws.iter().map(|l| &l.4))
                .any(|m| m.surface.as_ref().is_some_and(holds))
                || model.lods.iter().any(|(_, lod)| any(lod, holds))
        }
        self.static_models.iter().any(|(_, model)| any(model, &holds))
    }

    /// Decide once per frame, before any model pass, whether painted
    /// terrain takes its own lane this frame. Its shaders are built when a
    /// resident model has a painted layer.
    pub(super) fn prepare_splat_lane(&mut self, cx: &mut Cx) {
        self.splat_ready = false;
        if !self.pbr_materials_enabled || !self.a_resident_layer(|s| s.splat.is_some()) {
            return;
        }
        if self.splat_draw.is_none() {
            self.splat_draw = cx.try_with_vm(|vm| Box::new(crate::shaders::DrawSceneSplat::script_new_with_default(vm)));
        }
        if let Some(d) = self.splat_draw.as_deref_mut() {
            d.pbr.skinned.draw_vars.refresh_after_reload(cx);
        }
        let stock = self.splat_draw.as_ref().and_then(|d| d.pbr.skinned.draw_vars.draw_shader_id);
        let shader = if stock.is_some() { self.lane_shaders(cx, super::variants::ModelLane::Splat, stock).0 } else { None };
        self.splat_ready = shader.is_some_and(|id| self.recorded_shader_ready(cx, id));
    }

    /// Prewarm installed traced surfaces on the current scene target before
    /// the model passes. Ordinary custom materials also check whether their
    /// current variant can take their layers from the stock lanes this frame.
    pub(super) fn prepare_custom_lanes(&mut self, cx: &mut Cx) {
        if let Some(pass) = self.items.recorded_pass.filter(|_| !self.traced_programs.is_empty()) {
            let scenes = self.traced_scenes(cx, pass);
            for (name, program) in &mut self.traced_programs {
                let scene = scenes.for_program(program);
                if !program.prepared_this_draw(cx, scene) { program.prepare(cx, scene, name); }
            }
        }
        let features = self.lane_features();
        let pass = self.items.recorded_pass;
        for material in self.custom_draws.values_mut() {
            let shader = material.built_shader(features);
            if shader.is_some() {
                material.draw.draw_vars.draw_shader_id = shader;
            }
            material.lane_ready = shader.is_some_and(|id| super::stock_ibl::material_pipeline_ready(cx, id, pass, self.hdr_output));
        }
    }

    /// Install a generation that requires all four passes.
    pub fn install_traced_surface(&mut self, name: String, recipe: TracedSurfaceRecipe) {
        self.install_traced_surface_for_passes(name, recipe, TracedSurfacePasses::ALL);
    }

    /// Install a generation with the passes it needs to draw. Use
    /// [`TracedSurfacePasses::OUTSIDE`] for an exterior drawn only in the scene.
    /// The program can be prewarmed before placing any of its layers.
    pub fn install_traced_surface_for_passes(
        &mut self, name: String, recipe: TracedSurfaceRecipe, required: TracedSurfacePasses,
    ) {
        self.install_traced_program(name, recipe, required, false, cfg!(target_arch = "wasm32"));
    }

    /// Inspect the outside pass in isolation while the other passes are being
    /// developed. The host still sees the state of the complete program.
    pub fn install_traced_outside_preview(&mut self, name: String, recipe: TracedSurfaceRecipe) {
        self.install_traced_program(name, recipe, TracedSurfacePasses::ALL, true, cfg!(target_arch = "wasm32"));
    }

    pub(super) fn install_traced_program(
        &mut self, name: String, recipe: TracedSurfaceRecipe, required: TracedSurfacePasses, preview: bool, webgl: bool,
    ) {
        let mut program = TracedProgram::new(recipe, required, preview, webgl);
        program.serial = crate::custom_material::next_traced_serial();
        if let Some(reason) = program.passes.failure(self.gpu_baker.realtime_sun_active()) {
            log!("render: traced surface {name} unavailable: {reason}");
        }
        self.traced_programs.insert(name, program);
    }

    /// Keep this renderer's traced programs in step with the live renderer's: the same names
    /// from the same installs, each one built and prewarmed here for this renderer's own pass
    /// target. A program the live renderer dropped or replaced goes; one it lacks is installed
    /// again from the recipe; a failure it reached is reported here too. An unchanged set costs
    /// one comparison per program.
    pub(super) fn mirror_traced_programs_from(&mut self, live: &Self) {
        let mut same = self.traced_programs.len() == live.traced_programs.len();
        for ((mine, own), (name, theirs)) in self.traced_programs.iter_mut().zip(&live.traced_programs) {
            if mine != name || own.serial != theirs.serial {
                same = false;
                break;
            }
            own.follow_failure_of(theirs);
            own.source_ready = live.traced_program_ready(name);
        }
        if same { return; }
        self.traced_programs.retain(|name, own| live.traced_programs.get(name).is_some_and(|theirs| theirs.serial == own.serial));
        for (name, theirs) in &live.traced_programs {
            if !self.traced_programs.contains_key(name) {
                self.traced_programs.insert(name.clone(), theirs.installed_again());
            }
        }
        for (name, own) in &mut self.traced_programs {
            own.follow_failure_of(&live.traced_programs[name]);
            own.source_ready = live.traced_program_ready(name);
        }
    }

    /// Whether the program is ready here, without building a failure message to say so.
    fn traced_program_ready(&self, name: &str) -> bool {
        let realtime_sun = self.gpu_baker.realtime_sun_active();
        self.traced_programs.get(name).is_some_and(|p| p.passes.failure(realtime_sun).is_none()
            && matches!(p.passes.state(self.traced_features(p), realtime_sun, self.gpu_baker.realtime_cascade_pass()), TracedSurfaceState::Ready))
    }

    /// The value recorded for whether `id` casts shadows, if any was set.
    pub fn model_casts_shadow_value(&self, id: &str) -> Option<bool> {
        self.model_casts_shadow.get(id).copied()
    }

    /// Release a registered program and its bindings after its last user switches.
    /// Layers that still require this name remain on the traced route.
    pub fn remove_traced_surface(&mut self, name: &str) -> bool {
        self.traced_programs.remove(name).is_some()
    }

    /// Readiness for the current scene features on the most recently prewarmed target.
    /// Prewarm again after changing the target or its attachments.
    pub fn traced_surface_state(&self, name: &str) -> TracedSurfaceState {
        self.traced_programs.get(name).map_or_else(|| {
            self.traced_surface_failure(name).map_or(TracedSurfaceState::Waiting, |reason| TracedSurfaceState::Failed(reason.to_owned()))
        }, |p| p.passes.state(self.traced_features(p), self.gpu_baker.realtime_sun_active(), self.gpu_baker.realtime_cascade_pass()))
    }

    /// The terminal reason for this program generation, including backend unavailability.
    pub fn traced_surface_failure(&self, name: &str) -> Option<&str> {
        if cfg!(target_arch = "wasm32") { return Some(crate::custom_material::TRACED_WEBGL_FAILURE); }
        self.traced_programs.get(name)?.passes.failure(self.gpu_baker.realtime_sun_active())
    }

    fn traced_features(&self, program: &TracedProgram) -> super::variants::Features {
        if program.wants_sun() { self.traced_caster_features() } else { self.lane_features() }
    }

    pub(super) fn traced_caster_scene(&self, cx: &Cx, pass: DrawPassId) -> TracedScene {
        TracedScene::new(cx, pass, self.traced_caster_features(), self.gpu_baker.realtime_sun_active(), self.gpu_baker.realtime_cascade_pass())
    }

    fn traced_scenes(&self, cx: &Cx, pass: DrawPassId) -> TracedScenes {
        let caster = self.traced_caster_scene(cx, pass);
        TracedScenes { caster, outside: caster.with_features(self.lane_features()) }
    }

    pub fn prewarm_traced_surfaces(&mut self, cx: &mut Cx, scene: DrawPassId) {
        if self.traced_programs.is_empty() { return; }
        if self.gpu_baker.mode() == crate::gpu_lightmap::GpuLightmapMode::Realtime && self.gpu_baker.realtime_cascade_pass().is_none()
            && self.traced_programs.values().any(TracedProgram::wants_sun) {
            self.gpu_baker.sync_csm_targets(cx);
        }
        let scenes = self.traced_scenes(cx, scene);
        for (name, program) in &mut self.traced_programs {
            program.prepare(cx, scenes.for_program(program), name);
        }
    }

    /// Decide once per frame, before any model pass, whether foliage takes
    /// its own lane this frame. Its shaders are built when a resident model
    /// has a layer in the wind.
    pub(super) fn prepare_foliage_lane(&mut self, cx: &mut Cx) {
        self.foliage_ready = false;
        if !self.pbr_materials_enabled
            || std::env::var_os("MAKEPAD_FOLIAGE_LANE").is_some_and(|v| v == "0")
            || !self.a_resident_layer(|s| s.definition.wind > 0.0)
        {
            return;
        }
        if self.foliage_draw.is_none() {
            self.foliage_draw = cx.try_with_vm(|vm| Box::new(crate::shaders::DrawSceneFoliageLit::script_new_with_default(vm)));
        }
        // Ready: the foliage lane's shader for this frame's features (variants.rs).
        if let Some(d) = self.foliage_draw.as_deref_mut() {
            d.pbr.skinned.draw_vars.refresh_after_reload(cx);
        }
        let stock = self.foliage_draw.as_ref().and_then(|d| d.pbr.skinned.draw_vars.draw_shader_id);
        let shader = if stock.is_some() { self.lane_shaders(cx, super::variants::ModelLane::Foliage, stock).0 } else { None };
        self.foliage_ready = shader.is_some_and(|id| self.recorded_shader_ready(cx, id));
    }

    /// Decide once per frame, before any model pass, whether far stand-ins
    /// draw this frame. Their shader is built as soon as a resident model has
    /// a stand-in (its follow-up installed), in parallel with the other
    /// lanes, not at a stand-in's first draw; until its pipeline exists every
    /// model draws its own layers at every distance, and a review waits for
    /// it (`review_materials_ready`).
    pub(super) fn prepare_impostor_lane(&mut self, cx: &mut Cx) {
        self.impostor_shader = None;
        if !self.pbr_materials_enabled || !self.a_resident_layer(|s| s.definition.impostor > 0.0) {
            return;
        }
        self.impostor_shader = self.lane_shaders(cx, super::variants::ModelLane::Impostor, None).0.filter(|id| self.recorded_shader_ready(cx, *id));
    }

    pub(super) fn draw_custom_models(
        &mut self, cx: &mut Cx3d, eye: Vec3f, lod_scale: f32, instances: &(impl ModelList + ?Sized),
        lane: WorldModelLane, fog: (Vec3f, f32), sun: &SunLight,
        frustum: Option<&Frustum>, stats: &mut RenderStats,
    ) {
        if !self.traced_programs.is_empty() && instances.view().len() != 0 {
            let pass = cx.current_pass_id();
            let scenes = self.traced_scenes(cx.cx, pass);
            let mut programs = std::mem::take(&mut self.traced_programs);
            for (name, program) in &mut programs {
                if let Some(draw) = program.outside(scenes.for_program(program)) {
                    self.draw_models_inner(cx, ModelDraw::Traced(name, draw), LayerPhase::Opaque,
                        eye, lod_scale, instances, lane, fog, sun, frustum, stats);
                }
            }
            self.traced_programs = programs;
        }
        // The stock IBL program draws when the frame routes through it and
        // a shiny stock model is in the list, whether or not an instance
        // names it (renderer/stock_ibl.rs). The list check is the name scan
        // of draw_pbr_models' early-out without its fast path (the diffuse
        // lane's model_orders): while the frame routes, each call hashes
        // the resident shiny models' names and scans the list, and a list
        // with no shiny stock model draws nothing through the program.
        // While the frame does not route (no environment) nothing is
        // scanned.
        let stock_ibl_on = self.stock_ibl_active() && match instances.view().contiguous_live() {
            Some(copies) => self.lists_a_stock_ibl_model(copies),
            None => self.has_pbr_copy(instances, |inst| {
                !inst.layer_materials.is_empty() || !self.has_own_material(inst.material_of(""), None)
            }),
        };
        // Keep opaque programs before blended ones, each group in name
        // order; their deferred layers replay the same program order.
        let mut programs = std::mem::take(&mut self.custom_draws);
        self.blended_models.programs.begin(&programs, instances, stock_ibl_on);
        for phase in [LayerPhase::Opaque, LayerPhase::Blended] {
            for (index, (name, material)) in programs.iter_mut().enumerate() {
                if self.blended_models.programs.phase(index) == Some(phase) {
                    self.draw_models_inner(cx, ModelDraw::Custom(name, material), LayerPhase::Opaque,
                        eye, lod_scale, instances, lane, fog, sun, frustum, stats);
                }
            }
        }
        self.blended_models.programs.end();
        self.custom_draws = programs;
    }

    /// Hand this frame's actor-attached world props to the renderer.
    ///
    /// Attachments receive ordinary world depth, fog, sunlight, AO and
    /// dynamic lights. Realtime cascades collect their current poses each
    /// frame, including layers with an admitted traced Sun draw. Moving an
    /// attachment preserves placed-scene identity and the static lightmap.
    /// They are always lit as dynamic geometry regardless of the supplied
    /// [`ModelInstance::dynamic`] value; that flag remains meaningful only
    /// in the placed-model lane.
    /// Returns the previous frame's list, so a caller that rebuilds it every
    /// frame can reuse its buffers.
    pub fn set_world_attachments(&mut self, instances: Vec<ModelInstance>) -> Vec<ModelInstance> {
        self.model_anim_state.clips.retain(|target,_|!matches!(target,StateTarget::Attachment(_)));
        std::mem::replace(&mut self.world_attachments, instances)
    }

    /// Hand this view's transient presentation meshes to the renderer.
    /// These are visible geometry only: unlike [`Self::set_models`], this
    /// list never participates in scene revision, baking, CSM mover capture,
    /// blob/SDF shadows, collision, or replication.
    /// Whether the sun reaches the first-person camera. The held model has
    /// no shadow map, so the host ray-tests the eye; eased over a few frames
    /// so stepping into shade does not pop.
    pub fn set_view_model_sun(&mut self, lit: bool) {
        let target = if lit { 1.0 } else { 0.0 };
        self.view_model_sun += (target - self.view_model_sun) * 0.25;
    }
    pub fn set_view_models(&mut self, instances: Vec<ModelInstance>) {
        self.view_models = instances;
    }

    /// How much CPU the light bake may spend (bake.rs). Lower `ao_rays` and
    /// `max_probes` on standalone XR; see apps/commercial/sandbox/BUDGETS.md. Setting
    /// `ao_strength` and `shadow_strength` to 0 turns the bake off visually
    /// while leaving the rest of the pipeline untouched.
    pub fn bake_settings(&self) -> BakeSettings {
        self.bake.settings()
    }

    pub fn set_bake_settings(&mut self, settings: BakeSettings) {
        self.bake.set_settings(settings);
    }

    /// Baked shade multiplier for a moving object, from the probe lattice.
    /// Public so a host can tint something the renderer does not own (the
    /// skinned characters go through this).
    pub fn dynamic_shade(&self, p: Vec3f) -> f32 {
        self.bake.dynamic_shade(p)
    }
}

/// Fold copies in host slot order. The cached pair is the state just before
/// the tail, so continuing it gives the same result at every prefix boundary.
/// The list length is checked separately when these signatures are installed.
fn extend_placed_signatures((mut scene, mut paint): (u64, u64), copies: &[ModelInstance]) -> (u64, u64) {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    for copy in copies {
        let (copy_scene, copy_paint) = placed_scene_signatures(std::slice::from_ref(copy));
        scene = scene.wrapping_mul(PRIME) ^ copy_scene;
        if !copy.dynamic {
            paint = paint.wrapping_mul(PRIME) ^ copy_paint;
        }
    }
    (scene, paint)
}

/// A portion of the placed list being refilled. Each push overwrites the
/// previous frame's copy at that position, reusing its string storage.
pub struct ModelTail<'a> {
    list: &'a mut Vec<ModelInstance>,
    start: usize,
    at: usize,
}

impl ModelTail<'_> {
    pub fn push(&mut self, m: &ModelInstance) {
        match self.list.get_mut(self.at) {
            Some(slot) => slot.clone_from(m),
            None => self.list.push(m.clone()),
        }
        self.at += 1;
    }
    /// The copies pushed in this portion of the list so far.
    pub fn pushed(&mut self) -> &mut [ModelInstance] {
        &mut self.list[self.start..self.at]
    }
}

#[cfg(test)]
mod traced_fork_tests {
    use super::*;
    use crate::custom_material::TracedSurfaceRecipe;

    #[test]
    fn review_fork_follows_traced_programs() {
        let mut live = Renderer::default();
        live.install_traced_surface_for_passes("body".into(), TracedSurfaceRecipe::tiles(0.05), TracedSurfacePasses::OUTSIDE);
        let mut fork = live.fork_scene_for_review();
        assert!(fork.traced_programs.contains_key("body"), "a fork carries the live renderer's traced program");
        assert!(matches!(fork.traced_surface_state("body"), TracedSurfaceState::Waiting),
            "the fork waits until it has prewarmed the program on its own target");
        assert!(fork.review_materials_ready(), "a program the live renderer has not got ready does not hold a review");
        assert!(live.traced_programs["body"].serial == fork.traced_programs["body"].serial, "the fork holds the same install");

        // Serials never repeat across renderers, so a rebuilt renderer cannot pass for the one a fork follows.
        let mut rebuilt = Renderer::default();
        rebuilt.install_traced_surface_for_passes("body".into(), TracedSurfaceRecipe::tiles(0.2), TracedSurfacePasses::OUTSIDE);
        assert!(rebuilt.traced_programs["body"].serial != live.traced_programs["body"].serial, "install serials are process-wide");
        let mut stale = rebuilt.fork_scene_for_review();
        stale.mirror_review_frame_from(&live);
        assert!(stale.traced_programs["body"].serial == live.traced_programs["body"].serial, "a fork takes the live renderer's program even under the same name");

        // Replacing a program under the same name replaces the fork's copy; a new name is added.
        live.install_traced_surface_for_passes("body".into(), TracedSurfaceRecipe::tiles(0.1), TracedSurfacePasses::OUTSIDE);
        live.install_traced_surface_for_passes("wheel".into(), TracedSurfaceRecipe::tiles(0.05), TracedSurfacePasses::OUTSIDE);
        fork.mirror_review_frame_from(&live);
        assert_eq!(fork.traced_programs.len(), 2, "a program the fork lacked is installed");
        assert!(fork.traced_programs["body"].serial == live.traced_programs["body"].serial, "a replaced program is installed again");

        // A program the live renderer dropped goes from the fork on the next mirror.
        assert!(live.remove_traced_surface("wheel"));
        fork.mirror_review_frame_from(&live);
        assert!(!fork.traced_programs.contains_key("wheel") && fork.traced_programs.len() == 1, "a dropped program leaves the fork");

        // A program with a terminal failure reports the same failure in the fork.
        live.install_traced_program("lost".into(), TracedSurfaceRecipe::tiles(0.05), TracedSurfacePasses::OUTSIDE, false, true);
        fork.mirror_review_frame_from(&live);
        assert!(matches!(fork.traced_surface_state("lost"), TracedSurfaceState::Failed(_)), "a failed program fails in the fork too");
        assert_eq!(fork.traced_surface_failure("lost"), live.traced_surface_failure("lost"), "with the same reason");

        // An unchanged set costs no allocation.
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            use crate::shaders::shader_registration_tests::allocations;
            let before = allocations::count();
            fork.mirror_traced_programs_from(&live);
            assert_eq!(allocations::count(), before, "mirroring an unchanged set allocated");
        }
    }
}
