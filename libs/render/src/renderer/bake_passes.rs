//! GPU light-map/CSM/local-shadow casters and the per-frame bake passes; texture bindings.

use super::*;

impl Renderer {
    /// Local lights must see all opaque architecture, not just the subset
    /// owned by CSM when a world atlas is present. Reuse resident geometry;
    /// terrain/voxel conversions are cached with each tile's revision.
    pub(super) fn collect_local_static_casters(&mut self, cx: &mut Cx, world: &World) -> Vec<crate::gpu_lightmap::GpuBakeMesh> {
        use crate::gpu_lightmap::GpuBakeMesh;
        let mut out = Vec::new();
        for inst in self.placed_models.iter().filter(|i| !i.dynamic) {
            if self.model_casts_shadow.get(&inst.model) == Some(&false) { continue; }
            let Some((_,model)) = self.static_models.iter().find(|(id,_)|id == &inst.model) else {continue;};
            if model.morph.is_some(){continue;}
            let (min,max) = crate::lightmap::world_bounds(&inst.transform,(model.min,model.max));
            for geometry in std::iter::once(model.geometry.as_ref()).chain(model.extra_draws.iter().map(|(g,..)|g.as_ref())) {
                out.push(GpuBakeMesh {geometry:geometry.geometry_id(),transform:inst.transform,min,max, cutout: None, band: Default::default() });
            }
        }
        fn shadow_geometry(cx:&mut Cx,source:&Geometry,cached:&mut Option<Geometry>)->Option<GeometryId> {
            if cached.is_none() {
                let (indices,vertices)=source.cpu_buffers(cx);
                let vertices=vertices.as_f32()?;
                let indices=indices.as_u32()?.to_vec();
                let mut packed=Vec::with_capacity(vertices.len()/16*crate::model::MODEL_VERTEX_FLOATS);
                for v in vertices.chunks_exact(16) {
                    packed.extend_from_slice(&v[..3]);
                    packed.resize(packed.len()+crate::model::MODEL_VERTEX_FLOATS-3,0.0);
                }
                let geometry=Geometry::new(cx); geometry.update(cx,indices,packed); *cached=Some(geometry);
            }
            cached.as_ref().map(Geometry::geometry_id)
        }
        if self.stage.shows_environment() {
            if let Some(terrain)=world.terrain.as_deref() {
                self.ensure_terrain_tiles(cx,terrain,world.terrain_materials.as_deref());
                for tile in &mut self.terrain_tiles {
                    if let Some(geometry)=shadow_geometry(cx,&tile.geometry,&mut tile.shadow_geometry) {
                        out.push(GpuBakeMesh{geometry,transform:Mat4f::identity(),min:tile.min,max:tile.max, cutout: None, band: Default::default() });
                    }
                }
            }
            self.ensure_voxel_tiles(cx,world.voxel.as_deref());
            for tile in &mut self.voxel_tiles {
                if let Some(geometry)=shadow_geometry(cx,&tile.geometry,&mut tile.shadow_geometry) {
                    out.push(GpuBakeMesh{geometry,transform:Mat4f::identity(),min:tile.min,max:tile.max, cutout: None, band: Default::default() });
                }
            }
        }
        out
    }

    /// World primitives used to enter CSM through the realized atlas. Keep
    /// them as ordinary caster instances when clustered lighting skips it.
    pub(super) fn append_unbaked_world_casters(
        &mut self, cx: &mut Cx, world: &World,
        out: &mut Vec<crate::gpu_lightmap::GpuLmMover>,
    ) {
        for e in &world.entities {
            if e.kind != BodyKind::Static || e.hidden || primitive_bucket(e) != Some(PrimitiveBucket::Opaque) { continue; }
            let size = vec3f(e.half.x*2.0*e.scale.x,e.half.y*2.0*e.scale.y,e.half.z*2.0*e.scale.z);
            let transform = Self::rigid_transform(e);
            self.append_primitive_caster(cx, e.shape, transform, size, out);
        }
        for p in &world.parts {
            let Some(owner) = entity_index_sorted(&world.entities,p.owner).map(|i|&world.entities[i]) else { continue; };
            if owner.hidden || p.color.w < 0.999 { continue; }
            let size=vec3f(p.half.x*2.0*owner.scale.x,p.half.y*2.0*owner.scale.y,p.half.z*2.0*owner.scale.z);
            self.append_primitive_caster(cx,p.shape,Self::part_transform(owner,p),size,out);
        }
    }

    pub(super) fn append_primitive_caster(
        &mut self,cx:&mut Cx,shape:Shape,mut transform:Mat4f,size:Vec3f,
        out:&mut Vec<crate::gpu_lightmap::GpuLmMover>,
    ) {
        let geometry=self.ensure_shadow_shape_geometry(cx,shape);
        for j in 0..3 { transform.v[j]*=size.x; transform.v[4+j]*=size.y; transform.v[8+j]*=size.z; }
        out.push(crate::gpu_lightmap::GpuLmMover {
                    material: None,
            geometry,
            transform,min:vec3f(-0.5,-0.5,-0.5),max:vec3f(0.5,0.5,0.5),skin:None,morph:None,
        });
    }

    /// The depth passes' unit caster for one primitive shape, in the packed
    /// model layout (position lanes only — the depth shaders read nothing
    /// else), built once per shape.
    pub(super) fn ensure_shadow_shape_geometry(&mut self,cx:&mut Cx,shape:Shape)->GeometryId {
        let slot=shape.index();
        if self.shadow_shape_geometries[slot].is_none() {
            let (source,indices)=shape_geometry_data(shape);
            let mut vertices=Vec::with_capacity(source.len()/12*crate::model::MODEL_VERTEX_FLOATS);
            for p in source.chunks_exact(12) {
                vertices.extend_from_slice(&p[..3]);
                vertices.resize(vertices.len()+crate::model::MODEL_VERTEX_FLOATS-3,0.0);
            }
            let geometry=Geometry::new(cx);
            geometry.update(cx,indices,vertices);
            self.shadow_shape_geometries[slot]=Some(geometry);
        }
        self.shadow_shape_geometries[slot].as_ref().unwrap().geometry_id()
    }

    /// Every primitive shape's caster geometry, so [`Self::collect_lm_movers`]
    /// (which has no `Cx`) can cast each entity in its OWN shape.
    pub(super) fn ensure_entity_caster_geometries(&mut self, cx: &mut Cx) {
        for shape in Shape::ALL {
            self.ensure_shadow_shape_geometry(cx, shape);
        }
    }

    /// Every dynamic caster for the Realtime bake's depth passes: dynamic
    /// placed models (driven cars), skinned CHARACTERS (rest mesh + this
    /// frame's palette — [`Self::pack_skin_palettes`] must already have
    /// run), and Rigid/Mover primitive entities in their own shape (a
    /// marble casts a round shadow, a crate a square one).
    pub(super) fn collect_lm_movers(
        &self,
        world: &World,
        eye:Vec3f,
        skinned_items: Option<&[SkinnedDraw]>,
    ) -> Vec<crate::gpu_lightmap::GpuLmMover> {
        let mut out = Vec::new();
        for (target,inst) in self.placed_models.iter().enumerate().map(|(i,m)|(ModelTarget::Instance(i),m))
            .chain(self.world_attachments.iter().enumerate().map(|(i,m)|(ModelTarget::Attachment(i),m))) {
            let Some(at) = self.static_models.iter().position(|(k, _)| *k == inst.model)
            else {
                continue;
            };
            if self.model_casts_shadow.get(&inst.model)==Some(&false){continue;}
            let root=&self.static_models[at].1;
            let distance=crate::asset_lod::instance_distance(&inst.transform,eye);
            // A chained base (hand-built LOD models) casts from its last,
            // cheapest level: a car's shadow is its silhouette on the road.
            let lod=if self.is_lod_chain_base(&inst.model)&&!root.lods.is_empty(){root.lods.len()}else{root.lods.partition_point(|(threshold,_)|*threshold<=distance)};
            let m=if lod==0{root}else{&root.lods[lod-1].1};
            let morph=m.morph.as_ref().map(|m|m.depth(self.model_anim_state.morph_weights(&target,&inst.model,&m.source)));
            // Anim parts cast as MOVERS even when their level is static: the
            // static atlas was baked without them (they are not in its
            // stream), so a door's shadow can only come from the cascades —
            // and it has to follow the door, which is what a mover is.
            for part in &m.anim_parts {
                let pose = Mat4f::mul(&inst.transform, &self.model_anim_state.transform(&target,&inst.model,&part.def));
                for (g, ..) in &part.draws {
                    out.push(crate::gpu_lightmap::GpuLmMover {
                    material: None,
                        geometry: g.geometry_id(),
                        transform: pose,
                        min: part.def.min,
                        max: part.def.max,
                        skin: None,
                        morph:morph.clone(),
                    });
                }
            }
            for part in &m.driven_parts {
                let local = inst
                    .part_poses
                    .iter()
                    .find(|pose| pose.connection == part.def.connection)
                    .map(|pose| pose.transform)
                    .unwrap_or_else(|| part.def.rest_transform());
                let pose = Mat4f::mul(&inst.transform, &local);
                for (g, ..) in &part.draws {
                    out.push(crate::gpu_lightmap::GpuLmMover {
                    material: None,
                        geometry: g.geometry_id(),
                        transform: pose,
                        min: part.def.min,
                        max: part.def.max,
                        skin: None,
                        morph:morph.clone(),
                    });
                }
            }
            if !inst.dynamic && !matches!(target,ModelTarget::Attachment(_)) && morph.is_none() {
                continue;
            }
            // A multi-material GLB owns one resident geometry per layer.
            // The main geometry alone would make only layer zero cast; walk
            // every visible layer so "dynamic model" means the whole model.
            // A Splash material with a derived caster (a vertex hook or a
            // mask) casts through it: displaced geometry, cut-out holes.
            let caster = inst.custom_material.as_ref().and_then(|cm| {
                let m = self.custom_draws.get(&cm.name)?;
                Some((m.shadow?, cm.params, m.cutoff, m.bounds_pad))
            });
            let pad = vec3f(1.0, 1.0, 1.0) * caster.map_or(0.0, |c| c.3);
            for (geometry, texture) in std::iter::once((m.geometry.as_ref(), &m.texture))
                .chain(m.extra_draws.iter().map(|(geometry, texture, ..)| (geometry.as_ref(), texture)))
            {
                out.push(crate::gpu_lightmap::GpuLmMover {
                    material: caster.map(|(shader, params, cutoff, _)| crate::gpu_lightmap::MoverMaterial { shader, params, cutoff, texture: texture.clone() }),
                    geometry: geometry.geometry_id(),
                    transform: inst.transform,
                    min: m.min - pad,
                    max: m.max + pad,
                    skin: None,
                    morph:morph.clone(),
                });
            }
        }
        if let (Some(items), Some(palette_tex)) = (skinned_items, &self.skin_palette_tex) {
            for (i, item) in items.iter().enumerate() {
                let Some(base) = self.skin_joint_bases.get(i).copied().filter(|b| *b >= 0.0)
                else {
                    continue;
                };
                let Some(at) = self
                    .skin_rig_geometries
                    .iter()
                    .position(|(k, _, _)| *k == item.rig)
                else {
                    continue;
                };
                // Posed bounds when the host measured them; a generous
                // character-sized default otherwise (bounds only cull lamp
                // faces — the sun pass draws every caster regardless).
                let (min, max) = item
                    .bounds
                    .unwrap_or((vec3f(-1.0, 0.0, -1.0), vec3f(1.0, 2.2, 1.0)));
                let distance=crate::asset_lod::instance_distance(&item.transform,eye);
                let lod=self.skin_lods.get(&item.rig).and_then(|levels|{let n=levels.partition_point(|(threshold,_)|*threshold<=distance);n.checked_sub(1).map(|i|&levels[i].1)});
                let morph=if let Some(lod)=lod{lod.morph.as_ref()}else{self.skin_morphs.get(&item.rig)};
                let morph=morph.map(|m|m.depth(m.source.sample_playback(item.morph_clip.as_deref(),item.morph_time,item.morph_looping)));
                out.push(crate::gpu_lightmap::GpuLmMover {
                    material: None,
                    geometry: lod.map_or_else(||self.skin_rig_geometries[at].1.geometry_id(),|lod|lod.geometry.geometry_id()),
                    transform: item.transform,
                    min,
                    max,
                    morph,
                    skin: Some(crate::gpu_lightmap::GpuLmSkin {
                        joint_tex: palette_tex.clone(),
                        joint_base: base,
                    }),
                });
            }
        }
        {
            for e in world.entities.iter() {
                if !matches!(e.kind, BodyKind::Mover | BodyKind::Rigid | BodyKind::Kinematic)
                    || e.alpha_primitive
                    || e.hidden
                    || e.parent != 0
                {
                    continue;
                }
                if std::env::var_os("MAKEPAD_CSM_CASTER_LOG").is_some() {
                    log!(
                        "csm caster {:?}: id {} kind {:?} pos ({:.1},{:.1},{:.1}) half ({:.2},{:.2},{:.2}) scale ({:.2},{:.2},{:.2}) alpha {:.2}",
                        e.shape, e.id, e.kind, e.pos.x, e.pos.y, e.pos.z,
                        e.half.x, e.half.y, e.half.z,
                        e.scale.x, e.scale.y, e.scale.z, e.color.w
                    );
                }
                let s = vec3f(
                    e.half.x * e.scale.x * 2.0,
                    e.half.y * e.scale.y * 2.0,
                    e.half.z * e.scale.z * 2.0,
                );
                let mut t = Self::entity_rotation(e);
                for c in 0..3 {
                    t.v[c] *= s.x;
                    t.v[4 + c] *= s.y;
                    t.v[8 + c] *= s.z;
                }
                t.v[12] = e.pos.x;
                t.v[13] = e.pos.y;
                t.v[14] = e.pos.z;
                // Built by ensure_entity_caster_geometries before this runs;
                // a shape not yet resident skips one frame's shadow.
                let Some(geometry) = self.shadow_shape_geometries[e.shape.index()].as_ref() else { continue };
                out.push(crate::gpu_lightmap::GpuLmMover {
                    material: None,
                    geometry: geometry.geometry_id(),
                    transform: t,
                    min: vec3f(-0.5, -0.5, -0.5),
                    max: vec3f(0.5, 0.5, 0.5),
                    skin: None,
                    morph:None,
                });
            }
        }
        out
    }

    /// Per-frame GPU bake step: realizes scheduled jobs and encodes this
    /// frame's passes — the whole atlas once per dirty kick (both modes,
    /// statics only), plus the cascade depth pass every Realtime frame.
    /// Pass ordering guarantees everything renders before the scene pass
    /// that samples it — same-frame delivery.
    pub(super) fn run_gpu_lightmap(
        &mut self,
        cx: &mut CxDraw,
        world: &World,
        movers: &[crate::gpu_lightmap::GpuLmMover],
        csm_view: Option<&crate::shadow_csm::CsmView>,
        eye: Vec3f,
    ) {
        // The materials' `csm_map` slot is a depth texture in every mode.
        self.gpu_baker.ensure_csm_fallback(cx.cx);
        // A realized atlas is not a precondition for the cascade tier. In
        // Realtime the cascades ARE the dynamic-shadow contract, and worlds
        // that own no static lightmap at all — a flat starter terrain with
        // no props, so `kick_lightmap_bake` finds no AO mesh and no receiver
        // box and never schedules a job — must still get them. Gating this
        // on `has_state` is what made F8 read as "realtime deletes every
        // dynamic shadow" in exactly those worlds.
        if !self.gpu_baker.has_state()
            && !crate::gpu_lightmap::dynamic_shadow_tiers(self.gpu_baker.mode()).csm
        {
            return;
        }
        // DEBUG (macOS): MAKEPAD_GPU_LM_DUMP=<prefix> writes the settled
        // GPU atlas as `<prefix>.a.pgm` (A = sun SDF) + `<prefix>.rgb.ppm`
        // (lamps) — the byte-level counterpart of the CPU bake's old
        // host lightmap dump, for numeric parity comparison.
        #[cfg(target_os = "macos")]
        {
            static DUMPED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            static DUMP_FRAME: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            if let Ok(prefix) = std::env::var("MAKEPAD_GPU_LM_DUMP") {
                // MAKEPAD_GPU_LM_DUMP_FRAME=<n> delays the capture n bake
                // frames so a streamed-in world (or Realtime, which is idle
                // from its first frame) dumps the SETTLED scene, not the
                // first half-loaded bake.
                let wait = std::env::var("MAKEPAD_GPU_LM_DUMP_FRAME")
                    .ok()
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                if self.gpu_baker.is_idle()
                    && DUMP_FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= wait
                    && !DUMPED.swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    if let Some(atlas) = self.lightmap.clone() {
                        if let Some((w, h, bytes)) = cx.cx.debug_read_render_texture(&atlas) {
                            let mut pgm = format!("P5\n{w} {h}\n255\n").into_bytes();
                            pgm.extend(bytes.chunks_exact(4).map(|px| px[3]));
                            let _ = std::fs::write(format!("{prefix}.a.pgm"), pgm);
                            let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
                            for px in bytes.chunks_exact(4) {
                                // Metal readback of BGRA8 is BGRA byte order.
                                ppm.extend([px[2], px[1], px[0]]);
                            }
                            let _ = std::fs::write(format!("{prefix}.rgb.ppm"), ppm);
                            log!("gpu lightmap: dumped {w}x{h} atlas to {prefix}.a.pgm/.rgb.ppm");
                        }
                    }
                    // Intermediates: coverage (R=lit-frac, G=covered) — the
                    // gather truth before any distance transform.
                    for (tex, name) in self.gpu_baker.debug_stage_textures() {
                        if let Some((w, h, bytes)) = cx.cx.debug_read_render_texture(&tex) {
                            if name.contains("depth") {
                                // R32F scratch: raw little-endian floats,
                                // header-free — `w`/`h` ride in the name.
                                let _ = std::fs::write(
                                    format!("{prefix}.{name}.{w}x{h}.f32"),
                                    &bytes,
                                );
                                continue;
                            }
                            let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
                            for px in bytes.chunks_exact(4) {
                                ppm.extend([px[2], px[1], px[0]]);
                            }
                            let _ = std::fs::write(format!("{prefix}.{name}.ppm"), ppm);
                        }
                    }
                }
            }
        }
        let mut sun = crate::sun::resolve_sun(&world.sun);
        // A world's own Sun steers the cascades and the bake too.
        if let Some(dir) = crate::world_lights::world_sun_dir(world) { sun.dir = dir; }
        // The re-bake idempotence probe (macOS readback): with
        // MAKEPAD_GPU_LM_REBAKE set, every settled bake reports its atlas
        // signature, and each bake after the first reports its DIFF against
        // the first — the instrument that measured the accumulator leak
        // (gpu_lightmap.rs, section 1). `=<n>` also re-bakes the same world n
        // times by itself: MODE=redirty re-bakes the realized layout into its
        // own targets, anything else re-kicks the whole job. MIN_REGIONS /
        // MIN_LAMPS hold the probe until the streamed world has arrived.
        #[cfg(target_os = "macos")]
        if let Some(n) = std::env::var("MAKEPAD_GPU_LM_REBAKE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static SETTLED_FRAMES: AtomicUsize = AtomicUsize::new(0);
            static BAKES: AtomicUsize = AtomicUsize::new(0);
            let env_usize = |k: &str| {
                std::env::var(k).ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0)
            };
            let (regions, lamps) = self.gpu_baker.debug_scene_size();
            if self.gpu_baker.is_idle()
                && regions >= env_usize("MAKEPAD_GPU_LM_REBAKE_MIN_REGIONS")
                && lamps >= env_usize("MAKEPAD_GPU_LM_REBAKE_MIN_LAMPS")
            {
                // Edge-triggered: one report per bake, not one per idle
                // frame — the readback and the diff are not free.
                if SETTLED_FRAMES.fetch_add(1, Ordering::Relaxed) == 2 {
                    let k = BAKES.fetch_add(1, Ordering::Relaxed);
                    if let Some(atlas) = self.lightmap.clone() {
                        if let Some((w, h, bytes)) = cx.cx.debug_read_render_texture(&atlas) {
                            lm_probe_report(
                                k,
                                w,
                                h,
                                regions,
                                lamps,
                                &bytes,
                                &self.gpu_baker.debug_region_rects(),
                            );
                        }
                    }
                    if k < n {
                        if std::env::var("MAKEPAD_GPU_LM_REBAKE_MODE").as_deref() == Ok("redirty") {
                            self.gpu_baker.debug_redirty();
                        } else {
                            self.kick_lightmap_bake(
                                world,
                                &sun,
                                crate::gpu_lightmap::BakeTrigger::WorldEdit,
                            );
                        }
                    }
                }
            } else {
                SETTLED_FRAMES.store(0, Ordering::Relaxed);
            }
        }
        let csm_scene_bounds = self.csm_scene_bounds.or_else(|| {
            if self.world_atlas_required() { return None; }
            // An atlas no longer supplies the world bound. Include every
            // caster (and the eye) so elevated/offscreen sun casters cannot
            // disappear from the cascades' light-space depth window.
            let mut min = eye;
            let mut max = eye;
            for (lo, hi) in self.csm_static_casters.iter().chain(&self.stream_casters).map(|m| (m.min, m.max))
                .chain(movers.iter().map(|m| crate::lightmap::world_bounds(&m.transform, (m.min, m.max))))
            {
                min = vec3f(min.x.min(lo.x), min.y.min(lo.y), min.z.min(lo.z));
                max = vec3f(max.x.max(hi.x), max.y.max(hi.y), max.z.max(hi.z));
            }
            Some((min, max))
        });
        // Streamed casters ride along with the placed ones, in that order
        // (the two lists as they are: a city's 20k placed casters were
        // copied into a scratch list every frame).
        if let Some(d) = self.gpu_baker.run_frame(
            cx,
            sun.dir,
            &[self.csm_static_casters.as_slice(), self.stream_casters.as_slice()],
            movers,
            csm_view,
            eye,
            csm_scene_bounds,
        ) {
            // Geometry can change during the settle debounce while an older
            // atlas finishes. Its remaps index the old model list.
            if d.world_revision != (world.render_rev, self.models_rev) {
                return;
            }
            self.lightmap = Some(d.atlas);
            self.lm_remaps = vec![Vec4f::default(); self.placed_models.len()];
            for (k, pi) in d.mesh_map.iter().enumerate() {
                if let Some(slot) = self.lm_remaps.get_mut(*pi) {
                    *slot = d.mesh_rects[k].uv_remap(d.size);
                }
            }
            self.lm_ground = match (d.planar_rects.first(), d.terrain_world) {
                (Some(r), Some(w)) => {
                    // Density in the log so a dump's texels map back to
                    // world coordinates without spelunking.
                    log!(
                        "gpu lightmap: ground region {}x{} px at ({},{}) over world ({:.1},{:.1}) span {:.1} — {:.2} texels/unit",
                        r.w, r.h, r.x, r.y, w.x, w.y, w.z,
                        r.w as f32 / w.z.max(0.0001)
                    );
                    Some((r.uv_remap(d.size), w))
                }
                _ => None,
            };
            self.lm_top = Some(d.top);
        }
    }

    /// Is the baked lightmap being SAMPLED? (The bake itself always runs.)
    pub fn lightmap_enabled(&self) -> bool {
        self.lightmap_enabled
    }

    /// Turn baked-lightmap sampling on/off; returns the new state. Off, the
    /// world renders on the analytic path alone: full sun everywhere the
    /// cascades do not shadow, and no baked lamp pools. Nothing is
    /// invalidated, so the toggle is instant in both directions.
    pub fn set_lightmap_enabled(&mut self, on: bool) -> bool {
        self.lightmap_enabled = on;
        on
    }

    /// Refresh every shadow cascade every frame (false) instead of on
    /// staggered frames (true, the realtime default): a locked-time host
    /// renders frames that must each be whole.
    pub fn set_csm_stagger(&mut self, stagger: bool) {
        self.gpu_baker.set_csm_stagger(stagger);
    }

    /// Which SPACE the model lanes shade in.
    ///
    /// Off (the default) is the game's, and it is right for a game: its
    /// texels are sRGB bytes used as-is, its [`crate::sun::SunLight`] rig is
    /// display-referred (0.72 direct plus 0.28 ambient, one for a fully lit
    /// white surface), and their product goes straight into the 8-bit
    /// target. Nothing in that path is linear and nothing needs mapping.
    ///
    /// On, the lanes decode their texels to linear reflectance, shade there,
    /// and finish through the same ACES fit and display gamma the analytic
    /// sky, the fog colour and the path tracer already use. That is what a
    /// host shading with real light needs — and, just as much, what any host
    /// with DARK materials needs: multiplying a cosine by an sRGB-encoded
    /// albedo and writing it raw crushes the midtones, so a fully sunlit
    /// charcoal wall lands at a tenth of the value it should and reads as a
    /// silhouette against a sky that WAS tone mapped.
    ///
    /// Only the placed/attached model lanes carry it. A world that also
    /// drives the cube, terrain or water lanes is a game world, and games
    /// leave this off.
    /// Shadow debug view: every lit lane shows its sun cascade (red, green,
    /// blue; grey outside) scaled by the filtered shadow visibility.
    pub fn set_shadow_debug(&mut self, on: bool) {
        self.shadow_debug = on;
    }

    /// Apply the host's diagnostic settings (the host reads its own
    /// environment or flags; the engine reads none).
    pub fn set_host_settings(&mut self, settings: &HostSettings) {
        self.lm_debug = if settings.lm_debug { 1.0 } else { 0.0 };
        self.vfx.stats = settings.vfx_stats;
    }

    pub fn set_display_transform(&mut self, on: bool) {
        self.display_transform = if on { 1.0 } else { 0.0 };
    }

    /// Is the model lanes' linear + ACES lane on?
    pub fn display_transform(&self) -> bool {
        self.display_transform > 0.5
    }

    /// Flip it. The sandbox binds this to F9.
    pub fn toggle_lightmap(&mut self) -> bool {
        self.lightmap_enabled = !self.lightmap_enabled;
        self.lightmap_enabled
    }

    /// The lightmap atlas to bind: the real one, else a 1x1 "fully sunlit,
    /// no lamps" stand-in so shaders sample unconditionally. The stand-in
    /// is also what the kill switch binds — an atlas that says "lit, no
    /// lamps" everywhere IS the analytic path.
    pub(super) fn lightmap_texture(&mut self, cx: &mut Cx) -> Texture {
        if let (true, Some(t)) = (self.lightmap_enabled, &self.lightmap) {
            return t.clone();
        }
        if self.lm_fallback.is_none() {
            self.lm_fallback = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: 1,
                    height: 1,
                    // A=255 (lit), RGB=0 (no lamp light).
                    data: Some(vec![0xFF00_0000]),
                    updated: TextureUpdated::Full,
                },
            ));
        }
        self.lm_fallback.clone().unwrap()
    }

    pub(super) fn upload_detail(
        &mut self,
        cx: &mut Cx,
        png: Option<&[u8]>,
        scale: [f32; 2],
    ) -> (Texture, [f32; 2]) {
        if scale[0].abs() <= 1e-4 || png.is_none() {
            return (self.detail_neutral(cx), [0.0, 0.0]);
        }
        match crate::texture_pack::image_buffer(png.unwrap()) {
            Ok(buf) => (buf.into_new_mip_repeat_texture(cx), scale),
            Err(_) => (self.detail_neutral(cx), [0.0, 0.0]),
        }
    }

    /// Make one draw layer's material resident.
    ///
    /// A material with no shininess is flattened to the neutral one on the
    /// way in, so nothing downstream has to remember that glTF's default
    /// `metallicFactor` is 1 — see [`LayerMaterial`]. A metallicRoughness
    /// image that fails to decode (a GLB embedding JPEG rather than PNG)
    /// degrades to the factors instead of failing the model: a prop that
    /// loses its roughness MAP still looks like the prop, and a prop that
    /// fails to load does not.
    ///
    /// The decoded pixels are consumed by the upload — `ImageBuffer` is moved
    /// into the texture — so the only host copy that outlives this call is
    /// the encoded PNG inside the `StaticModel`, which the caller drops.
    pub(super) fn upload_material(&mut self, cx: &mut Cx, pbr: &crate::model::PbrMaterial) -> LayerMaterial {
        if !pbr.is_shiny() {
            return LayerMaterial {
                surface: None,
                metallic: 0.0,
                roughness: 1.0,
                orm: self.orm_neutral(cx),
                orm_on: false,
                mag_nearest: pbr.mag_nearest,
                // Base texture unknown here: keep the discarding shader.
                cutout: true,
            };
        }
        let orm = pbr
            .orm_png
            .as_deref()
            .and_then(|bytes| crate::texture_pack::image_buffer(bytes).ok())
            .map(|buf| buf.into_new_mip_repeat_texture(cx));
        match orm {
            Some(tex) => LayerMaterial {
                surface: None,
                metallic: pbr.metallic,
                roughness: pbr.roughness,
                orm: tex,
                orm_on: true,
                mag_nearest: pbr.mag_nearest,
                // Base texture unknown here: keep the discarding shader.
                cutout: true,
            },
            None => LayerMaterial {
                surface: None,
                metallic: pbr.metallic,
                roughness: pbr.roughness,
                orm: self.orm_neutral(cx),
                orm_on: false,
                mag_nearest: pbr.mag_nearest,
                // Base texture unknown here: keep the discarding shader.
                cutout: true,
            },
        }
    }

    /// White 1x1: sampled as an ORM it multiplies both factors by 1, so a
    /// material that binds it reads exactly as its factors even if `orm_on`
    /// were ever set by mistake.
    pub(super) fn orm_neutral(&mut self, cx: &mut Cx) -> Texture {
        if self.orm_fallback.is_none() {
            self.orm_fallback = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: 1,
                    height: 1,
                    data: Some(vec![0xFFFF_FFFF]),
                    updated: TextureUpdated::Full,
                },
            ));
        }
        self.orm_fallback.clone().unwrap()
    }

    pub(super) fn detail_neutral(&mut self, cx: &mut Cx) -> Texture {
        if self.detail_fallback.is_none() {
            self.detail_fallback = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: 1,
                    height: 1,
                    data: Some(vec![0xFF80_8080]),
                    updated: TextureUpdated::Full,
                },
            ));
        }
        self.detail_fallback.clone().unwrap()
    }

    /// The shadow-top plane to bind plus its (base, range) decode: the real
    /// one, else a 1x1 "no blocker measured" stand-in (byte 255) so shaders
    /// sample unconditionally.
    pub(super) fn lm_top_binding(&mut self, cx: &mut Cx) -> (Texture, f32, f32) {
        if let (true, Some((t, base, range))) = (self.lightmap_enabled, &self.lm_top) {
            return (t.clone(), *base, *range);
        }
        if self.lm_top_fallback.is_none() {
            self.lm_top_fallback = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecRu8 {
                    width: 1,
                    height: 1,
                    data: Some(vec![255]),
                    unpack_row_length: None,
                    updated: TextureUpdated::Full,
                },
            ));
        }
        (self.lm_top_fallback.clone().unwrap(), 0.0, 8.0)
    }

    /// Write this frame's cascade binding into one material family:
    /// `csm_map` (a sampled depth texture) at `slot` plus the shared `csm_*`
    /// uniforms. An off frame (OnChange / no scene / sun down) writes the
    /// tier off and parks the baker's 1x1 depth fallback in the slot —
    /// `csm_vis` early-outs before sampling it, but the pipeline still
    /// wants a texture of the slot's kind bound. `fallback` only serves the
    /// frames before the baker has created that (it is never sampled).
    pub(super) fn write_csm_uniforms(
        cx: &mut Cx,
        dv: &mut DrawVars,
        binding: &Option<(crate::shadow_csm::CsmFrame, Texture, f32)>,
        fallback: &Texture,
        slot: usize,
        debug: bool,
    ) {
        dv.set_uniform(cx, live_id!(csm_debug), &[if debug { 1.0 } else { 0.0 }]);
        let Some((frame, depth, inv_res)) = binding.as_ref() else {
            dv.set_texture(slot, fallback);
            dv.set_uniform(cx, live_id!(csm_p), &[0.0, 0.001, 0.0, 0.0]);
            return;
        };
        dv.set_texture(slot, depth);
        if !frame.on {
            dv.set_uniform(cx, live_id!(csm_p), &[0.0, 0.001, 0.0, 0.0]);
            return;
        }
        let c = &frame.cascades;
        let (clip_scale, clip_bias) = cx.clip_depth_scale_bias();
        let mut da = [0.0f32; 4];
        let mut db = [0.0f32; 4];
        for i in 0..crate::shadow_csm::CSM_CASCADES {
            let (gs, go) = crate::shadow_csm::depth_generation_window(frame.generation[i]);
            da[i] = gs * clip_scale;
            db[i] = go * clip_scale + clip_bias;
        }
        dv.set_uniform(cx, live_id!(csm_p), &[1.0, *inv_res, 0.0, 0.0]);
        dv.set_uniform(cx, live_id!(csm_da), &da);
        dv.set_uniform(cx, live_id!(csm_db), &db);
        dv.set_uniform(
            cx,
            live_id!(csm_zw),
            &[c[0].z_per_world, c[1].z_per_world, c[2].z_per_world, c[3].z_per_world],
        );
        dv.set_uniform(
            cx,
            live_id!(csm_texel),
            &[c[0].texel_world, c[1].texel_world, c[2].texel_world, c[3].texel_world],
        );
        let rows = |v: Vec4f| [v.x, v.y, v.z, v.w];
        for (i, (rx, ry, rz)) in [
            (live_id!(csm_rx0), live_id!(csm_ry0), live_id!(csm_rz0)),
            (live_id!(csm_rx1), live_id!(csm_ry1), live_id!(csm_rz1)),
            (live_id!(csm_rx2), live_id!(csm_ry2), live_id!(csm_rz2)),
            (live_id!(csm_rx3), live_id!(csm_ry3), live_id!(csm_rz3)),
        ]
        .into_iter()
        .enumerate()
        {
            dv.set_uniform(cx, rx, &rows(c[i].rx));
            dv.set_uniform(cx, ry, &rows(c[i].ry));
            dv.set_uniform(cx, rz, &rows(c[i].rz));
        }
    }
}
