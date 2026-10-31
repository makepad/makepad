//! Static and frame lights: lamp harvest, daylight rails, light-map bake kick.

use super::*;

/// HDR lane light gains (see `Renderer::scale_frame_lights_hdr`).
const HDR_LAMP_GAIN: f32 = 4.0;
const HDR_CANDELA_GAIN: f32 = 0.02;

impl Renderer {
    /// Last frame's dynamic shadow-mesh triangles (entity hull drapes +
    /// pre-sidecar blobs) — the budget the unattended debug cycle watches.
    pub fn dynamic_shadow_triangles(&self) -> usize {
        self.last_dynamic_shadow_tris
    }

    /// Explicit static lights for the light baker. Empty (the default)
    /// harvests lamp props automatically at bake time.
    pub fn set_static_lights(&mut self, lights: Vec<crate::lightmap::LmLight>) {
        self.lm_lights = lights;
        // The lamp cache and its selection grid mirror this set.
        self.lamp_cache_rev = None;
    }

    /// Street lights from the placed props: any static model whose id reads
    /// as a lamp gets a warm downlight at its head — the bulb sits near the
    /// top of the model, so the anchor is measured from its bounds rather
    /// than assumed.
    ///
    /// # Photometry comes from the FIXTURE, not from the mesh scale
    ///
    /// Only the bulb's POSITION follows the placement transform: the mesh was
    /// scaled, so the bulb really did move. Strength and reach are solved
    /// from the mount height instead
    /// ([`lamp_photometry`](crate::lightmap::lamp_photometry)) so the pool on
    /// the street is the same size and the same brightness whether the kit
    /// was placed at ×1, ×2 or the road kits' canonical ×8.
    ///
    /// The flat `color: 2.0, radius: 8.0` this replaced looked
    /// scale-independent and was not: the gather's falloff is
    /// `(1 - d/radius)²`, so pinning the reach while the transform lifted the
    /// bulb made delivered brightness a function of mesh scale. A 1.56 m
    /// lantern at ×2 put 0.87 on the ground — MORE than the noon sun's 0.72
    /// direct term — which is the white plaza pool this rewrite fixes; the
    /// same lantern at ×8 lit nothing at all, its bulb hanging outside its
    /// own 8 m reach.
    pub(super) fn harvest_lamps(&self) -> Vec<crate::lightmap::LmLight> {
        use crate::lightmap::lamp_photometry;
        /// Warm street-lamp tint, normalised so its brightest component is 1
        /// — `lamp_photometry` supplies the strength it is scaled by.
        const TINT: Vec3f = Vec3f { x: 1.0, y: 0.775, z: 0.475 };
        let mut out = Vec::new();
        for inst in &self.placed_models {
            if inst.dynamic {
                continue;
            }
            let name = inst.model.rsplit('/').next().unwrap_or(&inst.model);
            if !(name.contains("lamp") || name.contains("lantern") || name.contains("light")) {
                continue;
            }
            let Some(at) = self.static_models.iter().position(|(k, _)| *k == inst.model)
            else {
                continue;
            };
            let m = &self.static_models[at].1;
            let mid_x = (m.min.x + m.max.x) * 0.5;
            let mid_z = (m.min.z + m.max.z) * 0.5;
            let head = m.max.y - (m.max.y - m.min.y) * 0.12;
            let at_world = |y: f32| {
                inst.transform
                    .transform_vec4(Vec4f { x: mid_x, y, z: mid_z, w: 1.0 })
                    .to_vec3f()
            };
            let p = at_world(head);
            // The pole's own foot IS the ground it stands on, whatever the
            // terrain does — measured under the same transform, so the mount
            // height is exact for any scale, yaw or tilt.
            let mount = p.y - at_world(m.min.y).y;
            let (radius, strength) = lamp_photometry(mount);
            out.push(crate::lightmap::LmLight {
                pos: p,
                color: TINT * strength,
                radius,
                // A street light is a downlight: full spot kills the glow
                // it was painting on the roof BESIDE its own head.
                dir: vec3f(0.0, -1.0, 0.0),
                spot: 1.0,
                ..Default::default()
            });
        }
        out
    }

    /// THE static light list for this sun: harvested fixtures (or the
    /// host's hand-set lights) with both rails applied, exactly once.
    ///
    /// One entry point on purpose — [`Self::rail_lamp_pools`]'s daylight
    /// scale is a MULTIPLIER, so applying it twice would square it. The
    /// bake and the per-frame analytic list must both come through here, or
    /// a static and a character standing on the same texel disagree about
    /// how bright the lamp above them is.
    pub(super) fn static_lights_for(&self, sun: &SunLight) -> Vec<crate::lightmap::LmLight> {
        let mut lights = if self.lm_lights.is_empty() {
            self.harvest_lamps()
        } else {
            self.lm_lights.clone()
        };
        if self.clustered_enabled {
            // Preserve authored day/night dimming, but there is no 8-bit
            // lamp atlas whose saturation should cap a realtime light. The
            // HDR lane has no headroom to protect at all: a lamp is simply
            // switched by daylight, like a photocell, by the frame's own
            // sun (`light_daylight`), not by whatever key lights the rig.
            let day = if self.hdr_output { Self::lamp_photocell(self.light_daylight) } else { Self::lamp_daylight_scale(sun) };
            for light in &mut lights { light.color = light.color * day; }
        } else {
            Self::rail_lamp_pools(&mut lights, sun);
        }
        lights
    }

    /// The sanity rails on static lights, applied wherever a lamp list is
    /// built so the baked atlas and the analytic per-frame term never
    /// disagree. Two of them, in order:
    ///
    /// 1. **Daylight headroom** — a lamp may only add the light the sky is
    ///    not already delivering
    ///    ([`lamp_daylight_scale`](crate::lightmap::lamp_daylight_scale)).
    ///    This is the rail on the SUM that reaches the screen, and the one
    ///    that stops a 0.30 pool painting a near-white plaza to 1.26 under a
    ///    noon sun.
    /// 2. **Atlas saturation** — no single light may clip the light atlas
    ///    over more than
    ///    [`LM_LAMP_SAT_TEXELS`](crate::lightmap::LM_LAMP_SAT_TEXELS) ground
    ///    texels. `harvest_lamps` sizes its fixtures so this never fires;
    ///    hand-set lights (`set_static_lights`) go through no such solve, and
    ///    one of those must still not be able to paint a plaza white.
    ///
    /// A light's implied mount is what its reach leaves over the pool it is
    /// meant to cover — exact for anything `lamp_photometry` sized.
    pub(super) fn rail_lamp_pools(lights: &mut [crate::lightmap::LmLight], sun: &SunLight) {
        use crate::lightmap::{cap_lamp_pool, LM_LAMP_POOL_RADIUS, LM_LAMP_SAT_DENSITY};
        let day = Self::lamp_daylight_scale(sun);
        if day < 1.0 {
            for l in lights.iter_mut() {
                l.color = l.color * day;
            }
        }
        let mut capped = 0usize;
        for l in lights.iter_mut() {
            let mount = (l.radius - LM_LAMP_POOL_RADIUS).max(0.25);
            if cap_lamp_pool(l, mount, LM_LAMP_SAT_DENSITY) < 1.0 {
                capped += 1;
            }
        }
        if capped > 0 {
            log!(
                "lamp bake: {capped} of {} static lights dimmed — their pool clipped the light atlas over more than {} texels",
                lights.len(),
                crate::lightmap::LM_LAMP_SAT_TEXELS as u32
            );
        }
    }

    /// This sun's daylight headroom factor for every static lamp — the one
    /// number that ties the lamp list to the sky. Read by the rail and by
    /// the bake/lamp-cache keys, so a sun change that MOVES it re-kicks the
    /// bake and a sun change that does not costs nothing.
    pub(super) fn lamp_daylight_scale(sun: &SunLight) -> f32 {
        crate::lightmap::lamp_daylight_scale(crate::lightmap::daylight_on_ground(
            sun.dir, sun.color, sun.sky,
        ))
    }

    /// The daylight scale as a cache key: quantized to 1/32 so a day cycle
    /// re-bakes when the lamps CHANGE STRENGTH and never on a
    /// floating-point wobble of the sun. One step is 3% of the pool peak —
    /// 0.009 of light, under a byte on the brightest albedo there is.
    pub(super) fn lamp_daylight_key(&self, sun: &SunLight) -> u32 {
        if self.hdr_output && self.clustered_enabled {
            return 1000 + (Self::lamp_photocell(self.light_daylight) * 32.0).round() as u32;
        }
        Self::legacy_daylight_key(sun)
    }

    pub(super) fn legacy_daylight_key(sun: &SunLight) -> u32 {
        (Self::lamp_daylight_scale(sun) * 32.0).round() as u32
    }

    /// The HDR lane's street-lamp switch: fully on once the sun is 2 degrees
    /// below the horizon, off by 8 degrees above it, a smooth dusk ramp
    /// between — the photocell behaviour of a real lamp. `daylight` points
    /// at the SUN (renderer/env_sun.rs `daylight_dir`), never at a moon that
    /// lights the frame: under a moonlit sky the lamps are on.
    pub(super) fn lamp_photocell(daylight: Vec3f) -> f32 {
        let elev = daylight.y.clamp(-1.0, 1.0).asin().to_degrees();
        let x = ((8.0 - elev) / 10.0).clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    }

    /// HDR lane: lift this frame's light list into linear, scene-referred
    /// units. Street lamps, entity lights and flashes were sized as
    /// display-space pools (a 0.30 ground peak); glTF punctual lights are
    /// candela (inverse-square) and are brought onto the same scale.
    pub(super) fn scale_frame_lights_hdr(&mut self) {
        for l in &mut self.frame_lights {
            let gain = if l.spot < 0.0 { HDR_CANDELA_GAIN } else { HDR_LAMP_GAIN };
            l.color = l.color * gain;
        }
    }

    /// Transient lights for THIS frame, on top of the per-frame list the
    /// renderer builds itself (street lamps + firework flashes). For hosts:
    /// muzzle flashes, spell impacts, anything that lives a few frames.
    /// Consumed by the next `draw_scene` call; never baked, so statics
    /// receive these analytically too.
    pub fn add_asset_frame_lights(&mut self,lights:Vec<crate::lightmap::LmLight>)->Result<(),String>{
        // Never refused: the frame budget keeps the lights that matter.
        self.host_asset_lights.extend(lights);Ok(())
    }
    pub fn add_frame_lights(&mut self, lights: Vec<crate::lightmap::LmLight>) {
        self.host_lights.extend(lights);
    }

    /// Rebuild this frame's dynamic light list: harvested lamps first (they
    /// are the `frame_baked_count` prefix — already in the baked atlas, so
    /// only dynamic geometry may add them analytically), then transients.
    pub(super) fn build_frame_lights(&mut self, sun: &SunLight) {
        // The SUN is part of the key: the daylight-headroom rail makes a
        // lamp's strength a function of the sky, so a day cycle that dims
        // the pools must dim them for dynamics too — the analytic term and
        // the baked atlas are the same lamp seen twice and may never
        // disagree.
        let key = (self.models_rev, self.lamp_daylight_key(sun));
        if self.lamp_cache_rev != Some(key) {
            // The same list the bake snapshots, through the same rails.
            self.lamp_cache = self.static_lights_for(sun);
            self.lamp_cache_rev = Some(key);
            // The static-light selection grid lives and dies with the lamp
            // set — rebuilt HERE, on the settle path, never per frame. This
            // is what keeps runtime selection O(1) at any light count.
            if !self.clustered_enabled {
                self.light_grid = LightGrid::build(&self.lamp_cache, LIGHT_GRID_CELL);
            }
            self.light_cell_memory.clear();
        }
        self.frame_lights.clear();
        self.frame_lights.extend(self.lamp_cache.iter().cloned());
        self.frame_baked_count = self.frame_lights.len();
        for f in &self.firework_instances {
            if let Some(l) = firework_flash_light(f) {
                self.frame_lights.push(l);
            }
        }
        // Authored emitters are analytic lights even on static geometry;
        // their glTF intensity is not part of the legacy AO bake.
        let authored_start=self.frame_lights.len();
        if self.clustered_enabled{
        // Only models that carry emitters are looked up (most carry none,
        // and a scattered forest is thousands of copies a frame).
        let emitting: Vec<&(String, LoadedModel)> = self.static_models.iter().filter(|(_, m)| !m.emitters.is_empty()).collect();
        if !emitting.is_empty() {
        for(target,instance)in self.placed_models.iter().enumerate().map(|(i,m)|(ModelTarget::Instance(i),m)).chain(self.world_attachments.iter().enumerate().map(|(i,m)|(ModelTarget::Attachment(i),m))){
            if let Some((_, model)) = emitting.iter().copied().find(|(id,_)|id == &instance.model) {
                for emitter in model.emitters.iter() {
                    let node=emitter.animation.as_ref().and_then(|hierarchy|self.model_anim_state.clip(&target,&instance.model).map(|playback|hierarchy.transform_named_weighted(playback.name.as_deref(),playback.name.as_ref().map(|_|playback.time),playback.looping,playback.weight)));
                    self.frame_lights.push(emitter.placed_at(&instance.transform,node,self.model_anim_state.idle_time));
                }
            }
        }
        }
        self.frame_lights.append(&mut self.host_asset_lights);
        }else{self.host_asset_lights.clear();}
        let authored=self.frame_lights.len()-authored_start;
        match self.check_asset_light_count(authored) {
            Err(error)=>self.report_asset_light_error(error),
            Ok(())=>self.asset_light_error=None,
        }
        budget_authored_lights(&mut self.frame_lights, authored_start, self.light_eye, crate::asset_lights::MAX_ASSET_FRAME_LIGHTS);
        // The streamed city's own lights (street lamps, stand-in cars'
        // headlights) that survived the budget, for the host's cost report.
        self.frame_stream_lights = self.stream_light_count.min(self.frame_lights.len() - authored_start);
        self.frame_lights.append(&mut self.host_lights);
    }

    /// Snapshot the static scene and schedule the GPU bake. Called on the
    /// same settle-debounced cadence as the static shadow rebuild — a burst
    /// of edits pays one bake, after the world goes still. A newer kick
    /// replaces a pending one wholesale (the baker re-plans the layout).
    pub(super) fn kick_lightmap_bake(
        &mut self,
        world: &World,
        sun: &SunLight,
        trigger: crate::gpu_lightmap::BakeTrigger,
    ) {
        if !self.world_atlas_required() { return; }
        let mut meshes = Vec::new();
        let mut mesh_map = Vec::new();
        let mut mesh_geometry = Vec::new();
        let mut casters_only = Vec::new();
        for (pi, inst) in self.placed_models.iter().enumerate() {
            if inst.dynamic {
                continue;
            }
            let Some(at) = self.static_models.iter().position(|(k, _)| *k == inst.model)
            else {
                continue;
            };
            if self.static_models[at].1.morph.is_some() { continue; }
            let Some(src) = &self.static_models[at].1.lm_source else {
                // No AO layout, no region — but it still casts: sun-depth
                // passes take its render geometry at its transform.
                let m = &self.static_models[at].1;
                let (lo, hi) = crate::lightmap::world_bounds(&inst.transform, (m.min, m.max));
                if casts_as_caster_only(
                    self.model_casts_shadow.get(&inst.model).copied(),
                    m.prelit,
                    lo,
                    hi,
                ) {
                    casters_only.push(crate::gpu_lightmap::GpuBakeMesh {
                        geometry: m.geometry.geometry_id(),
                        transform: inst.transform,
                        min: lo,
                        max: hi, cutout: None, band: Default::default() });
                }
                continue;
            };
            if std::env::var_os("MAKEPAD_GPU_LM_REGIONS").is_some() {
                // TEMP instrumentation: name each region's model and give the
                // exact chart uv -> world map of its two largest up-facing
                // triangles, so an atlas dump's texels map back to world.
                let k = meshes.len();
                let mut tris: Vec<(f32, u32)> = Vec::new();
                for t in 0..src.caster.tri_count() as u32 {
                    let (a, b, c) = src.caster.triangle(t);
                    let ab = b - a;
                    let ac = c - a;
                    let n = Vec3f {
                        x: ab.y * ac.z - ab.z * ac.y,
                        y: ab.z * ac.x - ab.x * ac.z,
                        z: ab.x * ac.y - ab.y * ac.x,
                    };
                    let area2 = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
                    if area2 > 1e-9 && n.y / area2 > 0.9 {
                        tris.push((area2, t));
                    }
                }
                tris.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap());
                for (_, t) in tris.iter().take(24) {
                    let vs = src.caster.triangle_verts(*t);
                    let (a, b, c) = src.caster.triangle(*t);
                    let w = |p: Vec3f| {
                        let q = inst.transform.transform_vec4(Vec4f { x: p.x, y: p.y, z: p.z, w: 1.0 });
                        (q.x, q.y, q.z)
                    };
                    let uvs: Vec<[f32; 2]> =
                        vs.iter().map(|i| src.ao_uv[*i as usize]).collect();
                    let (wa, wb, wc) = (w(a), w(b), w(c));
                    log!(
                        "lmtri {} {} uv({:.4},{:.4})({:.4},{:.4})({:.4},{:.4}) w({:.2},{:.2},{:.2})({:.2},{:.2},{:.2})({:.2},{:.2},{:.2})",
                        k, inst.model,
                        uvs[0][0], uvs[0][1], uvs[1][0], uvs[1][1], uvs[2][0], uvs[2][1],
                        wa.0, wa.1, wa.2, wb.0, wb.1, wb.2, wc.0, wc.1, wc.2,
                    );
                }
            }
            meshes.push(crate::lightmap::LmMeshInstance {
                source: src.clone(),
                transform: inst.transform,
            });
            mesh_map.push(pi);
            // The FLAT-WINDING-normal variant: the gather's backface test
            // must see what the CPU rays saw (lm_source implies it exists).
            let m = &self.static_models[at].1;
            mesh_geometry.push(
                m.bake_geometry
                    .as_ref()
                    .map(|g| g.geometry_id())
                    .unwrap_or_else(|| m.geometry.geometry_id()),
            );
        }
        // ONE ground light field for the whole scene: a synthetic heightfield
        // of terrain ∪ static box tops (roads, slabs, platforms), addressed
        // by world xz. Terrain tiles and every cube top sample this same
        // region, which is what lets a box road receive a house's shadow
        // without cubes carrying any lightmap data at all.
        let mut terrain_world = None;
        let mut planars = Vec::new();
        {
            // Bounds from the STATICS, not the terrain: a big terrain would
            // stretch the single ground region over empty grass and starve
            // the shadows of texels (the village measured ~40cm/texel that
            // way). Terrain outside the field renders fully lit — the
            // shaders test the rect rather than clamp-smearing its border.
            let (mut lo_x, mut lo_z) = (f32::MAX, f32::MAX);
            let (mut hi_x, mut hi_z) = (f32::MIN, f32::MIN);
            for (bmin, bmax) in &self.receiver_boxes {
                lo_x = lo_x.min(bmin.x);
                lo_z = lo_z.min(bmin.z);
                hi_x = hi_x.max(bmax.x);
                hi_z = hi_z.max(bmax.z);
            }
            for m in &meshes {
                let (bmin, bmax) = m.world_bounds();
                lo_x = lo_x.min(bmin.x);
                lo_z = lo_z.min(bmin.z);
                hi_x = hi_x.max(bmax.x);
                hi_z = hi_z.max(bmax.z);
            }
            if lo_x < hi_x && lo_z < hi_z {
                // Pad so shadows can run past the outermost caster, square so
                // one origin/cell pair serves both axes.
                // Pad so shadows can run past the outermost caster; cap so
                // density never collapses on a sprawling world (chunked
                // ground regions are the real fix at that scale).
                let pad = 6.0;
                let (lo_x, lo_z) = (lo_x - pad, lo_z - pad);
                let span = ((hi_x - lo_x).max(hi_z - lo_z) + pad).min(240.0);
                let n = ((span * crate::lightmap::LM_PLANAR_TEXELS_PER_UNIT) as usize + 2)
                    .clamp(2, 1025);
                let cell = span / (n - 1) as f32;
                let mut heights = vec![0.0f32; n * n];
                for gz in 0..n {
                    for gx in 0..n {
                        let x = lo_x + gx as f32 * cell;
                        let z = lo_z + gz as f32 * cell;
                        let mut h = world
                            .terrain
                            .as_ref()
                            .and_then(|t| t.height_at(x, z))
                            .unwrap_or(0.0);
                        for (bmin, bmax) in &self.receiver_boxes {
                            if x >= bmin.x - cell
                                && x <= bmax.x + cell
                                && z >= bmin.z - cell
                                && z <= bmax.z + cell
                                // GROUNDED boxes only: a road slab or crate
                                // resting on the terrain IS the ground there
                                // and should receive shadows at its top. A
                                // FLOATING platform is not — hoisting the
                                // field to its top swallowed the shadow it
                                // casts on the grass below, leaving hollow
                                // rim shadows around a lit footprint.
                                && bmin.y - h <= 0.75
                            {
                                h = h.max(bmax.y);
                            }
                        }
                        heights[gz * n + gx] = h;
                    }
                }
                terrain_world = Some(Vec4f { x: lo_x, y: lo_z, z: span, w: span });
                planars.push(crate::lightmap::LmPlanar {
                    x0: lo_x,
                    z0: lo_z,
                    x1: lo_x + span,
                    z1: lo_z + span,
                    y: 0.0,
                    field: Some(crate::lightmap::LmHeightField {
                        origin_x: lo_x,
                        origin_z: lo_z,
                        cell,
                        n,
                        heights: std::sync::Arc::new(heights),
                    }),
                });
            }
        }
        if meshes.is_empty() && planars.is_empty() {
            self.lightmap = None;
            self.lm_remaps.clear();
            self.lm_ground = None;
            self.lm_top = None;
            return;
        }
        // Clustered lights are evaluated at the receiving fragment. The
        // optional OnChange atlas stores SUN visibility only, never lamps.
        let lights = if self.clustered_enabled { Vec::new() } else { self.static_lights_for(sun) };
        let scene = crate::lightmap::LmScene {
            meshes,
            planars,
            boxes: self.occluder_boxes.clone(),
            lights,
            sun_dir: sun.dir,
            sun_color: sun.color,
            sun_sky: sun.sky,
            // Bounce is the slow luxury tier; off unless asked for until the
            // disk cache lands.
            bounce: std::env::var("LM_BOUNCE").is_ok(),
        };
        // The snapshot becomes render passes on the next frame
        // (gpu_lightmap.rs); delivery is a texture handle swap, no upload.
        self.gpu_baker.schedule(crate::gpu_lightmap::GpuBakeJob {
            world_revision: (world.render_rev, self.models_rev),
            scene,
            mesh_geometry,
            mesh_map,
            casters_only,
            terrain_world,
            trigger,
        });
    }
}

/// How many ranks at the end of the authored-light budget fade out, so a
/// light crossing the cut dims instead of switching off.
const LIGHT_BUDGET_FADE: usize = 32;

/// Keep at most `max` of `lights[start..]`: the ones delivering the most
/// light at `eye` (strength over squared distance, reach-limited). The last
/// [`LIGHT_BUDGET_FADE`] ranks kept are dimmed linearly toward the cut, so
/// as the eye moves and ranks swap, a light fades rather than pops.
pub(super) fn budget_authored_lights(lights: &mut Vec<crate::lightmap::LmLight>, start: usize, eye: Vec3f, max: usize) {
    let n = lights.len().saturating_sub(start);
    if n <= max { return; }
    let score = |l: &crate::lightmap::LmLight| {
        let d2 = (l.pos - eye).length_squared();
        let strength = l.color.x.max(l.color.y).max(l.color.z).max(0.0);
        let reach = if l.radius > 0.0 { (1.0 - d2.sqrt() / (l.radius * 4.0)).max(0.05) } else { 1.0 };
        strength * reach / (1.0 + d2)
    };
    let mut ranked: Vec<(f32, crate::lightmap::LmLight)> = lights.drain(start..).map(|l| (score(&l), l)).collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked.truncate(max);
    let fade_from = max.saturating_sub(LIGHT_BUDGET_FADE);
    for (rank, (_, mut l)) in ranked.into_iter().enumerate() {
        if rank >= fade_from {
            l.color = l.color * ((max - rank) as f32 / (LIGHT_BUDGET_FADE + 1) as f32);
        }
        lights.push(l);
    }
}
