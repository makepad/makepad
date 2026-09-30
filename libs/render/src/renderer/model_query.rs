//! Model queries, sky time, model states and animated-part colliders.

use super::*;

impl Renderer {
    pub fn model_is_loaded(&self, id: &str) -> bool {
        self.static_models.iter().any(|(k, _)| k == id)
    }

    /// The prop's low-res multi-box collider in model space. A house comes
    /// back as walls and roof rather than one box, so its doorway is a gap.
    /// Light-bake occluder boxes for a loaded prop — few, face-aligned.
    pub fn model_occluder_parts(&self, id: &str) -> Option<&[(Vec3f, Vec3f)]> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, m)| m.occluder_parts.as_slice())
    }

    pub fn model_authored_collisions(&self, id: &str) -> Option<&std::sync::Arc<Vec<crate::asset_metadata::PreparedAssetCollision>>> {
        self.static_models.iter().find(|(key, _)| key == id).map(|(_, model)| &model.authored_collisions).filter(|values| !values.is_empty())
    }

    pub fn model_has_authored_lights(&self, id: &str) -> bool {
        self.static_models.iter().any(|(key, model)| key == id && !model.emitters.is_empty())
    }

    /// Replace the live ownership set so removing/swapping an exterior
    /// restores ordinary vehicle headlights without mutating authored state.
    pub fn set_model_headlight_owners(&mut self, owners: Vec<u64>) {
        self.model_headlight_owners = owners;
    }

    pub fn model_collider_parts(&self, id: &str) -> Option<&[(Vec3f, Vec3f)]> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, m)| m.collider_parts.as_slice())
    }

    /// Model-space bounds of a loaded prop, for building its collider.
    pub fn model_bounds(&self, id: &str) -> Option<(Vec3f, Vec3f)> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, m)| (m.min, m.max))
    }

    /// Generic externally-driven connection points declared by this model.
    /// Empty is a valid ordinary model; no source-pack naming is consulted.
    pub fn model_driven_parts(&self, id: &str) -> Vec<DrivenPartInfo> {
        self.static_models
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, model)| {
                model
                    .driven_parts
                    .iter()
                    .map(|part| DrivenPartInfo {
                        connection: part.def.connection.clone(),
                        pivot: part.def.pivot,
                        anchor: part.def.anchor,
                        radius: part.def.radius,
                        width: part.def.width,
                        visual: part.def.visual,
                        rest_transform: part.def.rest_transform(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Advance the sky clock — what the scrolling layers of a Quake sky
    /// ride. The host ticks it beside its other presentation clocks; a
    /// paused game keeps a still sky, and a capture that sets the time
    /// explicitly ([`Self::set_sky_time`]) is reproducible.
    pub fn tick_sky(&mut self, dt: f32) {
        if dt.is_finite() {
            // Wrapped well beyond any layer's period so a long session never
            // loses precision in the scroll offset.
            self.sky_time = (self.sky_time + dt) % 4096.0;
        }
    }

    pub fn set_sky_time(&mut self, time: f32) {
        self.sky_time = time;
    }

    pub fn sky_time(&self) -> f32 {
        self.sky_time
    }

    /// The map's sky definition (projection, layer count, repeat, speeds),
    /// for a host that wants to report or drive it.
    pub fn model_sky(&self, id: &str) -> Option<&crate::model::SkyPart> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .and_then(|(_, m)| m.sky.as_ref())
            .map(|s| s.part.as_ref())
    }

    /// The sky faces' triangles in MODEL space.
    ///
    /// Kept separate from [`Self::model_mesh`] because the two answers differ
    /// by consumer: the RENDERER must not shade sky faces like walls, but a
    /// WALKER usually must collide with them — a Doom sky brush is solid, and
    /// dropping it from collision lets the player walk out of the map.
    pub fn model_sky_mesh(&self, id: &str) -> Option<(&[Vec3f], &[u32])> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .and_then(|(_, m)| m.sky.as_ref())
            .map(|s| (s.positions.as_slice(), s.indices.as_slice()))
    }

    /// Say whether a model casts into the baked sun shadows and Realtime CSM.
    ///
    /// Only matters for a static that owns no AO layout of its own, which is
    /// the lane that otherwise falls back to a size heuristic
    /// ([`CASTER_ONLY_MAX_SPAN`]). A host loading an imported LEVEL — one
    /// enormous GLB that IS the world — should pass `false`: a level that
    /// casts into the bake shadows its own interior and every room goes
    /// dark. Props default to casting, so a fence still throws a shadow.
    ///
    /// CSM registration updates immediately; the pending atlas bake is also
    /// re-kicked for OnChange.
    /// Draw `base` as the models of `chain` from their distances on: a
    /// hand-built mid-detail model and a far proxy, say, where decimating
    /// the base cannot give the shape wanted. The members are ordinary
    /// models built and installed on their own; once all are resident they
    /// become the base's LOD levels (the draw picks by distance like any
    /// authored LOD) and a chained base casts its shadow from its LAST
    /// level. An empty chain removes it.
    pub fn set_model_lod_chain(&mut self, base: &str, chain: Vec<(f32, String)>) {
        let mut chain = chain;
        chain.sort_by(|a, b| a.0.total_cmp(&b.0));
        let changed = if chain.is_empty() { self.model_lod_chains.remove(base).is_some() } else { self.model_lod_chains.insert(base.to_string(), chain.clone()) != Some(chain) };
        if changed {
            if let Some((_, m)) = self.static_models.iter_mut().find(|(k, _)| k == base) { m.lods.clear(); }
            self.model_lod_chains_dirty = true;
        }
    }

    /// Whether `id` is the base of a LOD chain (its shadow uses the last level).
    pub(super) fn is_lod_chain_base(&self, id: &str) -> bool { self.model_lod_chains.contains_key(id) }

    /// Fold resident chain members into their bases' `lods` (after any
    /// install or chain change; free otherwise).
    pub(super) fn apply_model_lod_chains(&mut self) {
        if !self.model_lod_chains_dirty { return; }
        self.model_lod_chains_dirty = false;
        for (base, chain) in &self.model_lod_chains {
            let Some(at) = self.static_models.iter().position(|(k, _)| k == base) else { continue };
            let levels: Option<Vec<(f32, super::prepared::LoadedModel)>> = chain.iter().map(|(d, id)| {
                self.static_models.iter().find(|(k, _)| k == id).map(|(_, m)| { let mut m = m.clone(); m.lods.clear(); (*d, m) })
            }).collect();
            if let Some(levels) = levels { self.static_models[at].1.lods = levels; }
        }
    }

    pub fn set_model_casts_shadow(&mut self, id: &str, casts: bool) {
        if self.model_casts_shadow.insert(id.to_string(), casts) != Some(casts) {
            self.placed_scene_signature = None;
            self.models_rev = self.models_rev.wrapping_add(1);
            self.rebuild_csm_static_casters();
        }
    }

    /// The loaded model's own triangles in MODEL space — positions and
    /// indices, the pair a collision structure is built from.
    ///
    /// Kept from the load so a walker never re-parses the GLB (`level.rs`
    /// builds its BVH straight off this). ANIM PARTS ARE NOT IN IT: a door
    /// moves, so it belongs to [`Self::anim_part_boxes`], not to a static
    /// acceleration structure that would go stale the moment it opened.
    pub fn model_mesh(&self, id: &str) -> Option<(&[Vec3f], &[u32])> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, m)| (m.mesh_positions.as_slice(), m.mesh_indices.as_slice()))
    }

    /// One named part's definition (states, clip, local bounds). The
    /// definitions are interleaved with their GPU handles, so a model's parts
    /// are enumerated by name ([`Self::model_anim_part_names`]) and fetched
    /// one at a time rather than handed out as a slice.
    pub fn model_anim_part(&self, id: &str, part: &str) -> Option<&crate::model::AnimPart> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .and_then(|(_, m)| m.anim_parts.iter().find(|p| p.def.name == part))
            .map(|p| p.def.as_ref())
    }

    /// Every part name a loaded model exposes, in file order.
    pub fn model_anim_part_names(&self, id: &str) -> Vec<String> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, m)| m.anim_parts.iter().map(|p| p.def.name.clone()).collect())
            .unwrap_or_default()
    }

    /// Drive one part toward a named state — the game's whole handle on a
    /// door.
    ///
    /// `target` is a model id (every placed copy; an imported level is one
    /// copy, so this is the usual form) or a placed slot index. `blend_secs`
    /// is how long THIS move takes: 0 snaps, and a command that lands
    /// mid-move retargets from where the part currently is, so a door caught
    /// half open reverses smoothly instead of jumping.
    ///
    /// Returns false — and changes nothing — when the model is not resident,
    /// has no such part, or the part has no such state; the caller has asked
    /// for something that does not exist, and silently doing nothing would
    /// hide an importer/contract mismatch.
    /// Select an imported clip at an explicit simulation time. None stops
    /// the clip in its authored rest pose; remove_model_clip restores autoplay.
    pub fn set_model_clip(&mut self,target:impl Into<ModelTarget>,name:Option<String>,time:f32,looping:bool)->Result<(),String>{
        self.set_model_clip_weighted(target,name,time,looping,1.0)
    }
    pub fn set_model_clip_weighted(&mut self,target:impl Into<ModelTarget>,name:Option<String>,time:f32,looping:bool,weight:f32)->Result<(),String>{
        if !time.is_finite()||time<0.0{return Err("model clip time must be finite and nonnegative".into())}
        if !weight.is_finite()||!(0.0..=1.0).contains(&weight){return Err("model clip weight must be finite and within0..1".into())}
        let target=target.into();let id=self.target_model_id(&target).ok_or("model clip target is not resident")?;
        let model=self.static_models.iter().find(|(key,_)|key==&id).map(|(_,m)|m).ok_or("model clip target is not resident")?;
        if let Some(name)=&name{if !model.anim_parts.iter().any(|p|p.def.clip.hierarchy.as_ref().is_some_and(|h|h.has_clip(name)))&&!model.morph.as_ref().is_some_and(|m|m.source.clips.iter().any(|c|&c.name==name)){return Err(format!("model has no imported clip '{name}'"))}}
        if !self.model_anim_state.clips.contains_key(&target)&&self.model_anim_state.clips.len()>=1024{return Err("model clip bindings exceed1024".into())}
        self.model_anim_state.clips.insert(target,ModelClipPlayback{name,time,looping,weight});Ok(())
    }
    pub fn remove_model_clip(&mut self,target:impl Into<ModelTarget>){self.model_anim_state.clips.remove(&target.into());}

    pub fn set_model_state(
        &mut self,
        target: impl Into<ModelTarget>,
        part: &str,
        state: &str,
        blend_secs: f32,
    ) -> bool {
        let target = target.into();
        let Some(model_id) = self.target_model_id(&target) else {
            return false;
        };
        // Fields, not accessors: the definition borrows `static_models` while
        // the clock map is written, and those are disjoint.
        let Some(def) = self
            .static_models
            .iter()
            .find(|(k, _)| *k == model_id)
            .and_then(|(_, m)| m.anim_parts.iter().find(|p| p.def.name == part))
            .map(|p| p.def.as_ref())
        else {
            return false;
        };
        self.model_anim_state.set(target, def, state, blend_secs)
    }

    /// Advance every triggered part by `dt` seconds of wall clock. Motion is
    /// LINEAR in time along the clip — the host ticks this once a frame,
    /// exactly where it ticks the rest of its presentation.
    pub fn tick_model_states(&mut self, dt: f32) {
        self.model_anim_state.tick(dt);
    }

    /// What every part of `target` is doing right now. Parts nobody has
    /// triggered report their model's default state.
    pub fn model_states(&self, target: impl Into<ModelTarget>) -> Vec<ModelPartState> {
        let target = target.into();
        let Some(model_id) = self.target_model_id(&target) else {
            return Vec::new();
        };
        let Some((_, loaded)) = self.static_models.iter().find(|(k, _)| *k == model_id) else {
            return Vec::new();
        };
        loaded
            .anim_parts
            .iter()
            .map(|p| {
                let (state, time, goal) = self.model_anim_state.clock(&target, &model_id, &p.def);
                ModelPartState {
                    part: p.def.name.clone(),
                    state,
                    state_name: p.def.states.get(state).cloned().unwrap_or_default(),
                    time,
                    target_time: goal,
                    settled: (time - goal).abs() <= 1.0e-6,
                }
            })
            .collect()
    }

    /// One part's state, or `None` when the model or part is unknown.
    pub fn model_part_state(
        &self,
        target: impl Into<ModelTarget>,
        part: &str,
    ) -> Option<ModelPartState> {
        let target = target.into();
        self.model_states(target).into_iter().find(|s| s.part == part)
    }

    /// The model id a target resolves to: itself, or the model in that slot.
    pub(super) fn target_model_id(&self, target: &ModelTarget) -> Option<String> {
        match target {
            ModelTarget::Model(id) => Some(id.clone()),
            ModelTarget::Instance(i) => self.placed_models.get(*i).map(|m| m.model.clone()),
            ModelTarget::Attachment(i) => self.world_attachments.get(*i).map(|m|m.model.clone()),
        }
    }


    /// Every placed instance's anim parts as WORLD-space collider boxes for
    /// this moment — a closed door is a wall, an open one is a hole, and a
    /// door caught halfway is exactly where it looks.
    ///
    /// This is the mover/walker query: it is not a broad-phase structure, so
    /// a caller with many doors should reject on `min`/`max` first.
    pub fn anim_part_boxes(&self) -> Vec<AnimPartBox> {
        let mut out = Vec::new();
        for (slot, inst) in self.placed_models.iter().enumerate() {
            let Some((_, loaded)) = self.static_models.iter().find(|(k, _)| *k == inst.model)
            else {
                continue;
            };
            for part in &loaded.anim_parts {
                let (state, _, _) =
                    self.model_anim_state.clock(&ModelTarget::Instance(slot), &inst.model, &part.def);
                let m = Mat4f::mul(&inst.transform, &self.model_anim_state.transform(&ModelTarget::Instance(slot),&inst.model,&part.def));
                let (boxes, min, max) = world_boxes(&m, &part.collider);
                out.push(AnimPartBox {
                    instance: slot,
                    model: inst.model.clone(),
                    part: part.def.name.clone(),
                    kind: part.def.kind.clone(),
                    state,
                    state_name: part.def.states.get(state).cloned().unwrap_or_default(),
                    boxes,
                    min,
                    max,
                });
            }
        }
        out
    }

    /// One part's world-space collider boxes. `Model` targets answer for the
    /// FIRST placed copy of that id (an imported level has exactly one).
    pub fn anim_part_collider(
        &self,
        target: impl Into<ModelTarget>,
        part: &str,
    ) -> Option<Vec<(Vec3f, Vec3f)>> {
        let target = target.into();
        let slot = match &target {
            ModelTarget::Instance(i)|ModelTarget::Attachment(i) => *i,
            ModelTarget::Model(id) => self.placed_models.iter().position(|m| m.model == *id)?,
        };
        let inst = if matches!(target,ModelTarget::Attachment(_)){self.world_attachments.get(slot)?}else{self.placed_models.get(slot)?};
        let (_, loaded) = self.static_models.iter().find(|(k, _)| *k == inst.model)?;
        let found = loaded.anim_parts.iter().find(|p| p.def.name == part)?;
        let (_, _, _) = self.model_anim_state.clock(&target, &inst.model, &found.def);
        let m = Mat4f::mul(&inst.transform, &self.model_anim_state.transform(&target,&inst.model,&found.def));
        Some(world_boxes(&m, &found.collider).0)
    }

    /// Triangle count of a loaded prop, so a caller can budget a scene before
    /// drawing it. Counted from the index buffer rather than stored, because
    /// this is a reporting path, not a hot one.
    pub fn model_triangles(&self, id: &str) -> Option<usize> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, m)| m.triangles)
    }

    /// Distance LOD levels below full detail a loaded prop switches to
    /// (authored or the builder's automatic ones); a reporting path.
    pub fn model_lod_levels(&self, id: &str) -> Option<usize> {
        self.static_models
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, m)| m.lods.len())
    }
}
