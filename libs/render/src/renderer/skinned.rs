//! Skin rigs, SDF shadow atlases and the GPU-skinned character lane.

use super::*;

impl Renderer {
    /// Is this rig's rest mesh resident? See [`Self::upload_skin_rig`].
    pub fn skin_rig_loaded(&self, rig: u64) -> bool {
        self.skin_rig_geometries.iter().any(|(k, _, _)| *k == rig)
    }

    /// Upload a rig's GPU rest bundle once (`SkinnedModel::rest_gpu`): the
    /// chart-split rest mesh plus its rest-pose AO atlas as an R8 texture.
    /// Every character wearing the rig shares both; the per-frame upload for
    /// a character is its joint palette. Idempotent per rig key.
    pub fn upload_skin_rig(&mut self, cx: &mut Cx, rig: u64, rest: crate::skin::SkinRestGpu) {
        if self.skin_rig_loaded(rig) {
            return;
        }
        self.install_uploaded_skin_rig(rig, UploadedSkinRig::upload(cx, rest));
    }

    pub fn install_uploaded_skin_rig(&mut self, rig: u64, uploaded: UploadedSkinRig) {
        if !self.skin_rig_loaded(rig) {
            self.skin_lods.insert(rig,uploaded.lods);
            if let Some(morph)=uploaded.morph{self.skin_morphs.insert(rig,morph);}
            if let Some(sdf)=uploaded.sdf{self.skin_prepared_sdf.insert(rig,sdf);}
            self.skin_material_draws.insert(rig,uploaded.materials);
            self.skin_rig_geometries.push((rig, uploaded.geometry, uploaded.ao_map));
        }
    }

    /// Upload one delivered SDF atlas as an R8 texture + its addressing
    /// meta. The runtime cost of a caster's whole shadow tier is this
    /// texture plus one instanced quad per caster per frame.
    pub(super) fn upload_sdf_atlas(
        cx: &mut Cx,
        atlas: crate::shadow_sdf::ShadowSdfAtlas,
    ) -> (Texture, SdfMeta) {
        let (w, h) = (atlas.width(), atlas.height());
        let meta = SdfMeta {
            rect: vec4(atlas.rect.0, atlas.rect.1, atlas.rect.2, atlas.rect.3),
            rows: atlas.rows,
            band_world: atlas.band_world,
            len_per_unit: atlas.len_per_unit,
        };
        let tex = Texture::new_with_format(
            cx,
            TextureFormat::VecRu8 {
                width: w,
                height: h,
                data: Some(atlas.pixels),
                unpack_row_length: None,
                updated: TextureUpdated::Full,
            },
        );
        (tex, meta)
    }

    /// The offline `.shadowsdf` sidecar for `glb` (tools/ao_bake), if it can
    /// be trusted. Three gates, all falling back to the off-thread bake
    /// rather than erroring:
    ///  - FRESH: sidecar mtime newer than the glb's (the `.aomesh` rule) —
    ///    a re-exported model must not keep its old silhouette.
    ///  - KEYED: `expect_hash` (a rig's rest hash, the `.skinao` scheme)
    ///    must match when the caller has one.
    ///  - THIS SUN: the bake projects at the sun's elevation
    ///    (`len_per_unit`), so a sidecar baked for a different sun would
    ///    draw every shadow at the wrong length. The tolerance only absorbs
    ///    normalisation ulps, not a different sky.
    /// A baked silhouette serves ANY sun whose shadow length is within a
    /// sane stretch of the baked one — the instance build stretches the
    /// sample window along the sun axis by the ratio (play-session-1 entry
    /// 18: exact-length matching rejected every sidecar the moment a level
    /// authored its own `time_of_day`, and the whole dynamic tier fell to
    /// blobs). Past the band the stretch distorts (a noon bake pulled to a
    /// sunset length smears) — those keep the blob tier.
    pub(super) fn sun_len_compatible(baked: f32, now: f32) -> bool {
        baked > 0.0 && now > 0.0 && (0.2..=5.0).contains(&(now / baked))
    }

    pub(super) fn load_shadow_sdf_sidecar(
        sidecar: &std::path::Path,
        glb: &std::path::Path,
        expect_hash: Option<u64>,
        sun: &SunLight,
    ) -> Option<crate::shadow_sdf::ShadowSdfAtlas> {
        // Staleness by mtime is only meaningful for checkout files with no
        // recorded identity. A caller that KNOWS the expected content hash
        // (store-streamed rigs — the cache writes both files at arbitrary
        // times) must not lose its shadows to write ordering.
        if expect_hash.is_none() {
            let stamp =
                |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            if stamp(sidecar)? <= stamp(glb)? {
                return None;
            }
        }
        let (atlas, hash) =
            crate::shadow_sdf::ShadowSdfAtlas::from_shadowsdf(&std::fs::read(sidecar).ok()?)?;
        if expect_hash.is_some_and(|h| h != hash) {
            return None;
        }
        if !Self::sun_len_compatible(atlas.len_per_unit, sun.shadow_len_per_unit()) {
            return None;
        }
        Some(atlas)
    }

    /// Resolve each rig's SDF atlas from its offline `.shadowsdf` sidecar
    /// (tools/ao_bake), once per rig per sun era. There is NO runtime
    /// silhouette baking — the sidecar IS the tier: a fresh, hash-matched,
    /// same-sun file uploads; anything else caches a `None` and the rig's
    /// characters keep the blob tier (re-tried only when the sun changes —
    /// [`Self::sdf_baked_sun_len`]).
    pub(super) fn seed_skinned_sdf(&mut self, cx: &mut Cx, items: &[SkinnedDraw], sun: &SunLight) {
        for item in items {
            if self.sdf_atlas_tex.iter().any(|(r, _)| *r == item.rig) {
                continue;
            }
            if let Some(prepared)=self.skin_prepared_sdf.get(&item.rig){
                let payload=prepared.as_ref().filter(|(_,meta)|Self::sun_len_compatible(meta.len_per_unit,sun.shadow_len_per_unit())).cloned();
                self.sdf_atlas_tex.push((item.rig,payload));continue;
            }
            let payload = item.sdf_sidecar.as_ref().and_then(|(glb, hash)| {
                let sidecar = std::path::PathBuf::from(format!("{glb}.shadowsdf"));
                Self::load_shadow_sdf_sidecar(
                    &sidecar,
                    std::path::Path::new(glb),
                    Some(*hash),
                    sun,
                )
            });
            let payload = match payload {
                Some(atlas) => {
                    log!(
                        "shadow sdf: rig {} atlas from sidecar ({}x{} R8, {} rows, {} bytes)",
                        item.rig,
                        atlas.width(),
                        atlas.height(),
                        atlas.rows,
                        atlas.pixels.len()
                    );
                    Some(Self::upload_sdf_atlas(cx, atlas))
                }
                None => {
                    log!(
                        "shadow sdf: rig {} has no loadable sidecar for this sun — blob tier \
                         (bake one with tools/ao_bake)",
                        item.rig
                    );
                    None
                }
            };
            self.sdf_atlas_tex.push((item.rig, payload));
        }
    }

    /// Resolve a dynamic MODEL's yaw-only SDF atlas from its offline
    /// `.shadowsdf` sidecar, once per model per sun era — the rig rule
    /// above, keyed by asset id ([`Self::models_root`]) instead of rest
    /// hash. No sidecar = its instances keep the blob tier.
    pub(super) fn seed_model_sdf(&mut self, cx: &mut Cx, key: &str, sun: &SunLight) {
        if self.model_sdf_tex.contains_key(key) {
            return;
        }
        if let Some((_,model))=self.static_models.iter().find(|(id,_)|id==key){
            if let Some(prepared)=&model.prepared_sdf{
                let payload=prepared.as_ref().filter(|(_,meta)|Self::sun_len_compatible(meta.len_per_unit,sun.shadow_len_per_unit())).cloned();
                self.model_sdf_tex.insert(key.to_string(),payload);return;
            }
        }
        // Bytes that arrived with the model (asset store) outrank the
        // checkout sidecar; they carry no mtime, only the sun gate applies.
        let streamed = self.model_sdf_bytes.get(key).and_then(|bytes| {
            let (atlas, _hash) = crate::shadow_sdf::ShadowSdfAtlas::from_shadowsdf(bytes)?;
            Self::sun_len_compatible(atlas.len_per_unit, sun.shadow_len_per_unit())
                .then_some(atlas)
        });
        let glb = Self::models_root().join(format!("{key}.glb"));
        let sidecar = Self::models_root().join(format!("{key}.shadowsdf"));
        let payload = match streamed.or_else(|| Self::load_shadow_sdf_sidecar(&sidecar, &glb, None, sun)) {
            Some(atlas) => {
                log!(
                    "shadow sdf: model {} atlas from sidecar ({}x{} R8, {} rows, {} bytes)",
                    key,
                    atlas.width(),
                    atlas.height(),
                    atlas.rows,
                    atlas.pixels.len()
                );
                Some(Self::upload_sdf_atlas(cx, atlas))
            }
            None => {
                log!(
                    "shadow sdf: model {key} has no loadable sidecar for this sun — blob tier \
                     (bake one with tools/ao_bake)"
                );
                None
            }
        };
        self.model_sdf_tex.insert(key.to_string(), payload);
    }

    /// Pack EVERY character's joint palette into the shared RGBA32F texture,
    /// once per frame, BEFORE the GPU lightmap encodes its passes: the
    /// bake's skinned depth passes and the visible skinned draw bind this
    /// same texture, so a character shadows in exactly the pose it draws
    /// in. Fills [`Self::skin_joint_bases`] (-1 = no palette).
    ///
    /// Deliberately un-culled: an off-screen character still casts into a
    /// visible region (Realtime), and a palette is ~2 KB — the whole
    /// village's worth is one small upload.
    pub(super) fn pack_skin_palettes(
        &mut self,
        cx: &mut Cx,
        items: &[SkinnedDraw],
        stats: &mut RenderStats,
    ) {
        self.skin_joint_bases.clear();
        // Power-of-two width and height keep texel-centre uvs exact in f32;
        // capacity only grows, so a steady crowd re-uses the allocation.
        let needed: usize = items
            .iter()
            .map(|i| i.palette.len() * crate::skin::PALETTE_TEXELS_PER_JOINT)
            .sum();
        if needed == 0 {
            self.skin_joint_bases.resize(items.len(), -1.0);
            return;
        }
        let rows = needed.div_ceil(JOINT_TEX_WIDTH).next_power_of_two();
        let capacity = JOINT_TEX_WIDTH * rows;
        if self.skin_palette_texels < capacity {
            self.skin_palette_tex = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecRGBAf32 {
                    width: JOINT_TEX_WIDTH,
                    height: rows,
                    data: Some(vec![0.0; capacity * 4]),
                    updated: TextureUpdated::Full,
                },
            ));
            self.skin_palette_texels = capacity;
        }
        let palette_tex = self.skin_palette_tex.clone().unwrap();
        // The texture OBJECT binds per consumer; its DATA uploads at pass
        // encode, after this has finished writing every palette.
        let mut texels = palette_tex.take_vec_f32(cx);
        texels.clear();
        for item in items {
            if item.palette.is_empty() {
                self.skin_joint_bases.push(-1.0);
                continue;
            }
            self.skin_joint_bases.push((texels.len() / 4) as f32);
            crate::skin::palette_texels(&item.palette, &mut texels);
        }
        stats.skin_upload_bytes += (texels.len() * 4) as u64;
        // The texture's storage is its full capacity.
        texels.resize(self.skin_palette_texels * 4, 0.0);
        palette_tex.put_back_vec_f32(cx, texels, None);
    }

    /// Draw the skinned batch inside the already-open scene pass.
    ///
    /// GPU skinning: each rig's rest mesh is resident; every character's
    /// joint palette is packed into ONE RGBA32F texture per frame
    /// ([`Self::pack_skin_palettes`]) and blended in the vertex shader.
    /// Characters of one rig sort adjacent so they accumulate into a single
    /// draw item — the per-frame cost of a crowd is its palettes plus an
    /// instance record each, not its vertices.
    pub(super) fn draw_skinned_inner(
        &mut self,
        cx: &mut Cx3d,
        batch: SkinnedBatch,
        fog: (Vec3f, f32),
        sun: &SunLight,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
        eye:Vec3f,
    ) {
        // The eye, the sun and the fog are uniforms, written once per call:
        // the vertex stage ran out of D3D11 input registers with them on the
        // instance stream, and every character of a call shares them.
        batch.skinned.draw_vars.set_uniform(cx.cx, live_id!(eye), &[eye.x, eye.y, eye.z]);
        self.clustered.bind(cx.cx, &mut batch.skinned.draw_vars, self.clustered_enabled);
        self.gi.bind(cx.cx, &mut batch.skinned.draw_vars);
        sun.write_uniforms(cx.cx, &mut batch.skinned.draw_vars);
        batch.skinned.draw_vars.set_uniform(cx.cx, live_id!(light_dir), &[sun.dir.x, sun.dir.y, sun.dir.z]);
        batch.skinned.draw_vars.set_uniform(cx.cx, live_id!(depth_clip), &[1.0]);
        batch.skinned.draw_vars.set_uniform(cx.cx, live_id!(fog_color), &[fog.0.x, fog.0.y, fog.0.z]);
        batch.skinned.draw_vars.set_uniform(cx.cx, live_id!(fog_density), &[fog.1]);
        // Dynamic lights are PER-CHARACTER: each looks up its own
        // precomputed grid cell (O(1), hysteresis-stable — light_grid.rs)
        // inside the draw loop below, so a villager standing under a lamp
        // holds a byte-identical block frame after frame and cannot
        // flicker. Characters sharing a cell (and rig and atlas) still
        // merge into one draw item — the append test compares uniforms.
        // The baked ground light field: characters walking through a house's
        // shadow darken (A channel gates their direct sun; RGB is never read
        // here — lamps arrive through the dl_* array).
        {
            let lm_tex = self.lightmap_texture(cx.cx);
            batch.skinned.draw_vars.set_texture(3, &lm_tex);
            // The shadow-top plane: heads clear a fence rail's shadow.
            let (top_tex, top_base, top_range) = self.lm_top_binding(cx.cx);
            batch.skinned.draw_vars.set_texture(4, &top_tex);
            {
                let csm = self.gpu_baker.csm_binding();
                Self::write_csm_uniforms(
                    cx.cx,
                    &mut batch.skinned.draw_vars,
                    &csm,
                    &lm_tex,
                    5,
                    self.shadow_debug,
                );
            }
            batch.skinned.draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_top_decode),
                &[top_base, top_range, 0.0, 0.0],
            );
            let (g_rect, g_world) = self.lm_ground.unwrap_or_default();
            batch.skinned.draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_rect),
                &[g_rect.x, g_rect.y, g_rect.z, g_rect.w],
            );
            batch.skinned.draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_world),
                &[g_world.x, g_world.y, g_world.z, g_world.w],
            );
        }
        // Palettes were packed by pack_skin_palettes BEFORE the GPU lightmap
        // ran (its skinned depth passes bind the same texture). Cloned
        // handle: the loop below needs `&mut self` for the cell lookup.
        let Some(palette_tex) = self.skin_palette_tex.clone() else {
            return;
        };

        // Copies of one rig sort adjacent: consecutive add_instance calls
        // with unchanged geometry and textures merge into one draw item.
        let mut order: Vec<usize> = (0..batch.items.len()).collect();
        order.sort_by_key(|i| (batch.items[*i].rig, batch.items[*i].texture));
        for i in order {
            let item = &batch.items[i];
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
            // Cull BEFORE packing the palette — an offscreen character then
            // costs its pose math and nothing else. Bounds come from the
            // joint spheres (posed_bounds), conservative for any pose.
            if let (Some(frustum), Some((min, max))) = (frustum, item.bounds) {
                let fur_margin = vec3f(0.05, 0.05, 0.05);
                if !frustum.intersects_obb(min-fur_margin, max+fur_margin, &item.transform) {
                    stats.skinned_culled += 1;
                    continue;
                }
            }
            // This character's OWN light block (cell lookup + strength
            // merge — light_grid.rs), written BEFORE the draw item opens so
            // the capture is deterministic. Characters sharing a cell (and
            // rig/atlas) still merge into one item.
            if !self.clustered_enabled {
                let (x, z) = (item.transform.v[12], item.transform.v[14]);
                let cell = self.stable_light_cell(0x8000_0000_0000_0000 | item.key, x, z);
                let block = match cell {
                    Some(c) => self.light_grid.block_of(c),
                    // (-1,-1) is out of range: the empty block.
                    None => self.light_grid.block_of((-1, -1)),
                };
                let split = merge_transients_into_block(
                    block,
                    &self.frame_lights,
                    self.frame_baked_count..self.frame_lights.len(),
                    vec3f(x, item.transform.v[13], z),
                    &mut self.light_rank,
                    &mut self.light_block_scratch,
                );
                write_light_block(
                    cx.cx,
                    &mut batch.skinned.draw_vars,
                    &self.light_block_scratch,
                    split,
                );
            }
            // Ground plane under this character, for the sun-ray-projected
            // baked-shadow sample (OnChange; the Realtime cascades need no
            // ground plane and no own-shadow bookkeeping — a character
            // simply is one more caster and receiver in the maps).
            batch.skinned.ground_y = self.char_ground.get(i).copied().unwrap_or(0.0);
            batch.skinned.joint_base = base;
            let distance=crate::asset_lod::instance_distance(&item.transform,eye);
            let mut fur_budget = 12_000usize.min(96_000usize.saturating_sub(stats.fur_triangles));
            let lod=self.skin_lods.get(&item.rig).and_then(|levels|{let n=levels.partition_point(|(threshold,_)|*threshold<=distance);n.checked_sub(1).map(|i|&levels[i].1)});
            batch.skinned.draw_vars.geometry_id =
                Some(lod.map_or_else(||self.skin_rig_geometries[at].1.geometry_id(),|lod|lod.geometry.geometry_id()));
            batch.skinned.transform = item.transform;
            batch.skinned.tint = item.tint;
            batch.skinned.color_adjust_ctl = item.color_adjust;
            // Clamp rather than index blindly: a bad texture index would
            // otherwise panic mid-frame, and a character wearing the wrong
            // atlas is a visible bug worth surviving to see.
            if let Some(tex) = batch
                .textures
                .get(item.texture)
                .or_else(|| batch.textures.first())
            {
                batch.skinned.draw_vars.set_texture(0, tex);
            }
            if let Some(texture)=lod.and_then(|lod|lod.base_texture.as_ref()){batch.skinned.draw_vars.set_texture(0,texture);}
            batch.skinned.draw_vars.set_texture(1, &palette_tex);
            // The rig's rest-pose AO atlas — per rig, so it changes exactly
            // when the geometry does and never breaks the rig batching.
            batch.skinned.draw_vars.set_texture(2, lod.map_or(&self.skin_rig_geometries[at].2,|lod|&lod.ao_map));
            batch.skinned.morph_ctl=Vec4f::default();
            let morph=if let Some(lod)=lod{lod.morph.as_ref()}else{self.skin_morphs.get(&item.rig)};
            if let Some(morph)=morph{
                let weights=morph.source.sample_playback(item.morph_clip.as_deref(),item.morph_time,item.morph_looping);
                batch.skinned.morph_ctl=vec4(morph.source.width as f32,morph.source.height as f32,morph.source.vertices as f32,morph.source.targets as f32);
                batch.skinned.morph_weights0=vec4(weights[0],weights[1],weights[2],weights[3]);
                batch.skinned.morph_weights1=vec4(weights[4],weights[5],weights[6],weights[7]);
                batch.skinned.morph_weights2=vec4(weights[8],weights[9],weights[10],weights[11]);
                batch.skinned.morph_weights3=vec4(weights[12],weights[13],weights[14],weights[15]);
                batch.skinned.morph_weights4=vec4(weights[16],weights[17],weights[18],weights[19]);
                batch.skinned.morph_weights5=vec4(weights[20],weights[21],weights[22],weights[23]);
                batch.skinned.morph_weights6=vec4(weights[24],weights[25],weights[26],weights[27]);
                batch.skinned.morph_weights7=vec4(weights[28],weights[29],weights[30],weights[31]);
                if let Some(shader)=batch.skinned.draw_vars.draw_shader_id{if let Some(slot)=cx.draw_shaders[shader.index].mapping.textures.iter().position(|t|t.id==live_id!(morph_map)){batch.skinned.draw_vars.set_texture(slot,&morph.texture);}}
            }
            let materials=if let Some(lod)=lod{Some(&lod.materials)}else{self.skin_material_draws.get(&item.rig)};
            if let Some(parts)=materials.filter(|parts|!parts.is_empty()) {
                for part in parts {
                    let definition=&part.surface.definition;
                    batch.skinned.fur = crate::material_surface::fur_params(definition.fur);
                    // The part's material: uniforms, written beside the part's
                    // own geometry and textures, so no two characters sharing a
                    // draw item can disagree about them.
                    let vars=&mut batch.skinned.draw_vars;
                    vars.set_uniform(cx.cx,live_id!(surface_on),&[1.0]);vars.set_uniform(cx.cx,live_id!(metallic),&[part.metallic]);vars.set_uniform(cx.cx,live_id!(roughness),&[part.roughness]);
                    // Toy gloss (clear coat and rim) in the spare lane.
                    batch.skinned.fur_layer.y=definition.packed_shading();
                    let vars=&mut batch.skinned.draw_vars;
                    vars.set_uniform(cx.cx,live_id!(material_alpha),&[definition.base_alpha]);vars.set_uniform(cx.cx,live_id!(alpha_mode),&[definition.alpha_mode as f32]);vars.set_uniform(cx.cx,live_id!(alpha_cutoff),&[definition.alpha_cutoff]);
                    vars.set_uniform(cx.cx,live_id!(normal_scale),&[definition.normal_scale]);vars.set_uniform(cx.cx,live_id!(occlusion_strength),&[definition.occlusion_strength]);
                    vars.set_uniform(cx.cx,live_id!(emissive),&[definition.emissive[0],definition.emissive[1],definition.emissive[2]]);vars.set_uniform(cx.cx,live_id!(double_sided),&[if definition.double_sided{1.0}else{0.0}]);
                    batch.skinned.draw_vars.options.alpha_blend=definition.alpha_mode==2;batch.skinned.draw_vars.options.depth_write=definition.alpha_mode!=2;batch.skinned.draw_vars.options.backface_culling=!definition.double_sided;
                    batch.skinned.draw_vars.geometry_id=Some(part.geometry.geometry_id());
                    batch.skinned.draw_vars.set_texture(0,&part.base);
                    if let Some(shader)=batch.skinned.draw_vars.draw_shader_id {
                        for(name,texture)in [(live_id!(orm_map),&part.orm),(live_id!(normal_map),&part.surface.normal),(live_id!(occlusion_map),&part.surface.occlusion),(live_id!(emissive_map),&part.surface.emissive)] {
                            if let Some(slot)=cx.draw_shaders[shader.index].mapping.textures.iter().position(|t|t.id==name){batch.skinned.draw_vars.set_texture(slot,texture);}
                        }
                    }
                    if batch.skinned.draw_vars.can_instance() {
                        let triangles = cx.cx.geometries[part.geometry.geometry_id()].indices.len() / 3;
                        let shells = crate::material_surface::fur_shell_count(batch.skinned.fur.x, &item.transform, distance, triangles, &mut fur_budget);
                        for layer in 0..=shells {
                            batch.skinned.fur_layer.x = layer as f32 / shells.max(1) as f32;
                            let area = cx.add_instance(&batch.skinned.draw_vars);
                            batch.skinned.draw_vars.area = cx.update_area_refs(batch.skinned.draw_vars.area, area);
                        }
                        stats.fur_triangles += shells * triangles;
                        batch.skinned.fur_layer.x = 0.0;
                    }
                }
            } else {
                batch.skinned.fur = Default::default(); batch.skinned.fur_layer.x = 0.0;
                batch.skinned.draw_vars.set_uniform(cx.cx,live_id!(surface_on),&[0.0]);batch.skinned.fur_layer.y=0.0;batch.skinned.draw_vars.options.alpha_blend=false;batch.skinned.draw_vars.options.depth_write=true;batch.skinned.draw_vars.options.backface_culling=true;
            if batch.skinned.draw_vars.can_instance() {
                let new_area = cx.add_instance(&batch.skinned.draw_vars);
                batch.skinned.draw_vars.area =
                    cx.update_area_refs(batch.skinned.draw_vars.area, new_area);
            }
            }
        }
    }

    /// Hysteresis-stable grid cell for a dynamic object: keeps the object's
    /// previous cell while it is still inside it plus a 1-unit margin, so
    /// dithering on a cell line never flaps its light block. `id` must be
    /// stable across frames (character key / placed-model index).
    pub(super) fn stable_light_cell(&mut self, id: u64, x: f32, z: f32) -> Option<(i32, i32)> {
        if let Some(prev) = self.light_cell_memory.get(&id) {
            if self.light_grid.cell_still_fits(*prev, x, z, 1.0) {
                return Some(*prev);
            }
        }
        let cell = self.light_grid.cell_of(x, z)?;
        self.light_cell_memory.insert(id, cell);
        Some(cell)
    }

    /// Where a character's soft shadow ROOTS, which way it points and how
    /// strong it is. ONE function owns the whole anchor policy, so the
    /// GPU-instanced quad and the CPU fallback cannot drift apart:
    ///
    /// * The ROOT — the silhouette's foot end — is the caster's own ground
    ///   contact. A shadow is attached to its caster there, and no light
    ///   direction change can detach it: leans redirect the BODY of the
    ///   shadow, never its contact.
    /// * Airborne (`h` = feet above the local ground) the root slides with
    ///   the height-driven part of the projection — anti-sun
    ///   `-sun.xz / max(sun.y, 0.2) · h`, lamp `ρ·h/(bulb−mid)` — a
    ///   jumping character's shadow moves off their feet, which is the
    ///   whole visual point of a jump shadow.
    /// * A nearby lamp claims the LEAN continuously: dominance is the
    ///   lamp's share of light at the feet (biased ×2 — a near, low light
    ///   owns the local shadow well before it strictly outshines the sky),
    ///   never a hard sun-wins threshold. The lean is the true projection
    ///   from the bulb — `ρ·(mid+h)/(bulb−mid)` — and supplies the quad's
    ///   long-axis DIRECTION; at night the sun term dies and the lamp
    ///   saturates.
    /// * The landing ground is re-sampled AT THE ROOT, so jumping beside a
    ///   slab lands the shadow on the slab.
    /// * Alpha fades and the sprite swells with height (a soft shadow weakens
    ///   and spreads with occluder distance — the M64 treatment); a
    ///   dominant lamp darkens it back and compresses it toward the root.
    ///
    /// `None` = out of shadow range, draw nothing.
    pub(super) fn character_shadow_anchor(
        &self,
        feet: Vec3f,
        receiver: &Receiver,
        sun: &SunLight,
    ) -> Option<ShadowAnchor> {
        character_shadow_anchor(feet, receiver, sun, &self.frame_lights)
    }
}
