//! Realm lifecycle, stage/quality, effects, placed models and custom materials.

use super::*;

impl Renderer {
    /// Begin rendering a different realm into this view.
    ///
    /// World-local revision counters restart when a script game is loaded,
    /// so they cannot by themselves distinguish (for example) town terrain
    /// at revision 1 from FPS terrain at revision 1. Call this exactly once
    /// at the successful world-replacement boundary, before submitting the
    /// new realm's first frame. It invalidates every cache derived from the
    /// old world while retaining uploaded asset geometry/textures, shader
    /// and pass pools, stage policy, bake settings, shadow budget, and the
    /// adaptive-quality history owned by this device.
    pub fn enter_realm(&mut self) {
        self.custom_draws.clear();
        self.static_chunks.clear();
        self.chunk_visible.clear();
        self.slab_key = None;
        self.slab_instance_count = 0;

        self.terrain_tiles.clear();
        self.terrain_revision = 0;
        self.voxel_tiles.clear();
        self.water_tiles.clear();
        self.water_rev = None;

        // Keep resident rig geometry and the palette texture allocation;
        // only their frame-to-instance mapping belongs to the old realm.
        self.skin_joint_bases.clear();
        self.placed_models.clear();
        self.csm_static_casters.clear();
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

        self.lightmap = None;
        self.lm_remaps.clear();
        self.lm_ground = None;
        self.lm_top = None;
        self.gpu_baker.enter_realm();
        self.csm_scene_bounds = None;
        self.lm_lights.clear();
        self.frame_lights.clear();
        self.frame_baked_count = 0;
        self.host_lights.clear();self.host_asset_lights.clear();self.asset_light_error=None;
        self.lamp_cache.clear();
        self.lamp_cache_rev = None;
        self.light_grid = LightGrid::default();
        let cluster_config = self.clustered.config();
        let shadow_config = self.clustered.shadows.config();
        let soft_filter = self.clustered.shadows.soft_filter;
        let shadow_source_radius = self.clustered.shadows.source_radius;
        self.clustered = crate::clustered::ClusteredLights::default();
        self.clustered.set_config(cluster_config);
        self.clustered.shadows.set_config(shadow_config);
        self.clustered.shadows.soft_filter=soft_filter;
        self.clustered.shadows.source_radius=shadow_source_radius;
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

    /// How this device projects the world. Changing it is free — the stage
    /// is a draw-list uniform, so nothing cached needs rebuilding, and the
    /// simulation never learns it happened.
    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn set_stage(&mut self, stage: Stage) {
        self.stage = stage;
    }

    /// How many casters may use the projected-silhouette tier. Lower it on
    /// mobile/standalone XR; see apps/sandbox/BUDGETS.md.
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
            "/../../../apps/sandbox/resources/models"
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

    /// Rebuild the static half of Realtime's CSM caster list.
    ///
    /// With a world atlas, charted models already become `BakeState` meshes;
    /// without one this list owns those casters too, including every layer. Keeping it
    /// next to placed-scene identity makes a scene upload the registration
    /// boundary instead of turning static architecture into a per-frame
    /// "mover" merely to get a shadow.
    pub(super) fn rebuild_csm_static_casters(&mut self) {
        self.csm_static_casters.clear();
        for inst in self.placed_models.iter().filter(|instance| !instance.dynamic) {
            let Some((_, model)) = self
                .static_models
                .iter()
                .find(|(id, _)| *id == inst.model)
            else {
                continue;
            };
            if model.morph.is_some(){continue;}
            if model.lm_source.is_some() && self.world_atlas_required() {
                continue;
            }
            let (min, max) = crate::lightmap::world_bounds(&inst.transform, (model.min, model.max));
            if model.lm_source.is_none() && !casts_as_caster_only(
                self.model_casts_shadow.get(&inst.model).copied(),
                model.prelit,
                min,
                max,
            ) {
                continue;
            }
            if self.model_casts_shadow.get(&inst.model) == Some(&false) { continue; }
            for geometry in std::iter::once(model.geometry.as_ref())
                .chain(model.extra_draws.iter().map(|(geometry, ..)| geometry.as_ref()))
            {
                self.csm_static_casters.push(crate::gpu_lightmap::GpuBakeMesh {
                    geometry: geometry.geometry_id(),
                    transform: inst.transform,
                    min,
                    max,
                });
            }
        }
    }

    /// Hand this frame's stock props to the renderer. Draw submission is
    /// batched by model, but list order is still scene identity: lightmap
    /// remaps are indexed by placed slot. Producers should therefore keep a
    /// stable order so a harmless reorder does not request a new bake.
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
        let signature = placed_scene_signature(&instances);
        let scene_changed = self.placed_scene_signature != Some(signature);
        // Statics are cached against a key; any meaningful placed-scene
        // change must break it or an equal-length replacement would retain
        // the previous realm's lamps, lightmap remaps, and shadows.
        if scene_changed {
            self.models_rev = self.models_rev.wrapping_add(1);
            // The lightmap remaps are indexed by placed position — a changed
            // list makes them point at the wrong props. Drop them; the
            // models_rev bump re-kicks the bake once the world settles.
            self.lm_remaps.clear();
            self.placed_scene_signature = Some(signature);
            // Per-slot door commands are indexed the same way and go stale
            // the same way. Per-model ones do not — an imported level keeps
            // its doors open across a harmless list rebuild.
            self.model_anim_state.forget_slots();
        }
        self.placed_models = instances;
        if scene_changed {
            self.rebuild_csm_static_casters();
        }
        Ok(())
    }

    /// Independent camera/lighting caches for an on-demand scene review.
    /// Meshes, textures and immutable metadata share resident handles; the
    /// live renderer's GI, camera and simulation state remain untouched.
    pub fn fork_scene_for_review(&self) -> Self {
        let mut review = Self::default();
        review.static_models = self.static_models.clone();
        review.model_casts_shadow = self.model_casts_shadow.clone();
        review.model_anim_state = self.model_anim_state.clone();
        review.ao_textures = self.ao_textures.clone();
        review.model_pack = self.model_pack.clone();
        review.skin_rig_geometries = self.skin_rig_geometries.clone();
        review.skin_material_draws = self.skin_material_draws.clone();
        review.skin_lods = self.skin_lods.clone();
        review.skin_morphs = self.skin_morphs.clone();
        review.skin_prepared_sdf = self.skin_prepared_sdf.clone();
        review.set_models(self.placed_models.clone());
        review.set_world_attachments(self.world_attachments.clone());
        review.host_lights = self.host_lights.clone();
        review.host_asset_lights = self.host_asset_lights.clone();
        review.sky_time = self.sky_time;
        review.set_gpu_lightmap_mode(crate::GpuLightmapMode::Realtime);
        review.set_clustered_lighting(true);
        review.set_gi_mode(crate::GiMode::Off);
        review
    }

    /// Custom draw handles stay UI-owned; lend them only while encoding a
    /// review pass, then restore them before encoding the player's pass.
    pub fn swap_review_materials(&mut self, other: &mut Self) {
        std::mem::swap(&mut self.custom_draws, &mut other.custom_draws);
    }

    pub fn review_models_ready(&self) -> bool {
        self.placed_models.iter().chain(&self.world_attachments).all(|instance|self.model_is_loaded(&instance.model))
    }

    /// Visible resident model bounds, including rotation and instance scale.
    pub fn placed_scene_bounds(&self) -> Option<(Vec3f, Vec3f)> {
        let mut min = vec3f(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = vec3f(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for instance in self.placed_models.iter().chain(&self.world_attachments) {
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
    pub fn install_custom_material(&mut self, name: String, draw: DrawSceneCustom) -> bool {
        if !draw.draw_vars.can_instance() { return false; }
        self.custom_draws.insert(name, Box::new(draw));
        true
    }

    pub fn retain_custom_materials(&mut self, names: &[String]) {
        self.custom_draws.retain(|name, _| names.contains(name));
    }

    pub fn custom_material_shader(&self, name: &str) -> Option<DrawShaderId> {
        self.custom_draws.get(name).and_then(|draw| draw.draw_shader_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_custom_models(
        &mut self, cx: &mut Cx3d, eye: Vec3f, instances: &[ModelInstance],
        lane: WorldModelLane, fog: (Vec3f, f32), sun: &SunLight,
        frustum: Option<&Frustum>, stats: &mut RenderStats,
    ) {
        let names: Vec<String> = self.custom_draws.keys().filter(|name| {
            instances.iter().any(|i| i.custom_material.as_ref().is_some_and(|m| &m.name == *name))
        }).cloned().collect();
        for name in names {
            let Some(mut draw) = self.custom_draws.remove(&name) else { continue; };
            self.draw_models_inner(cx, ModelDraw::Custom(&name, &mut draw), eye,
                instances, lane, fog, sun, frustum, stats);
            self.custom_draws.insert(name, draw);
        }
    }

    /// Hand this frame's actor-attached world props to the renderer.
    ///
    /// Attachments receive ordinary world depth, fog, sunlight, AO and
    /// dynamic lights. Their transforms may change every frame without
    /// touching placed-scene identity, lightmap scheduling, CSM caster
    /// capture, shadow-mesh generation, lamp harvesting or replication.
    /// They are always lit as dynamic geometry regardless of the supplied
    /// [`ModelInstance::dynamic`] value; that flag remains meaningful only
    /// in the placed-model lane.
    pub fn set_world_attachments(&mut self, instances: Vec<ModelInstance>) {
        self.model_anim_state.clips.retain(|target,_|!matches!(target,ModelTarget::Attachment(_)));
        self.world_attachments = instances;
    }

    /// Hand this view's transient presentation meshes to the renderer.
    /// These are visible geometry only: unlike [`Self::set_models`], this
    /// list never participates in scene revision, baking, CSM mover capture,
    /// blob/SDF shadows, collision, or replication.
    pub fn set_view_models(&mut self, instances: Vec<ModelInstance>) {
        self.view_models = instances;
    }

    /// How much CPU the light bake may spend (bake.rs). Lower `ao_rays` and
    /// `max_probes` on standalone XR; see apps/sandbox/BUDGETS.md. Setting
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
