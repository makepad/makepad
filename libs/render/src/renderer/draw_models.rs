//! World-model lanes: diffuse/PBR/custom model draws, sky faces, view models.

use super::*;

/// How much of a model LOD chain's switch distance its two levels crossfade
/// over, just before it: a copy in the band draws both levels through
/// complementary screen-door windows, so a car that drives away changes
/// level as a short dissolve instead of a pop.
const CHAIN_FADE: f32 = 0.1;

/// The two screen-door windows (near level, next level) of a copy at
/// `distance` from a chain switch at `threshold`, or `None` outside the band.
/// A pixel's noise n in [0, 1) shows the near level when n >= q and the next
/// when n < q, q rising from 0 to 1 across the band.
fn chain_fade_windows(threshold: f32, distance: f32) -> Option<(f32, f32)> {
    let t = (distance - threshold * (1.0 - CHAIN_FADE)) / (threshold * CHAIN_FADE);
    if !(t > 0.0 && t < 1.0) { return None; }
    let q = (t * 255.0).round().clamp(1.0, 254.0);
    Some((1.0 + q * 256.0 + 255.0, 1.0 + q))
}

/// One instance list's draw order for one frame: each copy's model slot and
/// LOD, and the copies grouped by (model, LOD).
#[derive(Default)]
pub(super) struct ModelOrder {
    key: (usize, usize),
    slots: Vec<Option<usize>>,
    lods: Vec<usize>,
    distances: Vec<f32>,
    /// Draw entries: (copy, LOD, screen-door window). A copy crossfading
    /// between two chain levels has an entry for each, their windows
    /// complementary (the shader's `color_adjust_ctl.w` dither code); 0 is
    /// no window.
    order: Vec<(usize, usize, f32)>,
    /// Each model slot's pack occlusion atlas (index into `ao_textures`),
    /// resolved on first use this frame.
    ao: Vec<Option<Option<usize>>>,
}

impl ModelOrder {
    fn build(&mut self, models: &[(String, LoadedModel)], instances: &[ModelInstance], eye: Vec3f, key: (usize, usize), chained: &dyn Fn(&str) -> bool, frustum: Option<&Frustum>, occluders: Option<&crate::stream::OcclusionRaster>) -> u64 {
        // Model slot per instance, resolved once through a map: a linear
        // search per instance is O(instances x models), which a streamed or
        // prop-heavy world turns into milliseconds.
        let index: std::collections::HashMap<&str, usize> =
            models.iter().enumerate().map(|(i, (k, _))| (k.as_str(), i)).collect();
        self.slots.clear();
        // Copies of one model often follow each other: those skip the hash.
        let mut last: Option<(&str, Option<usize>)> = None;
        self.slots.extend(instances.iter().map(|inst| {
            let id = inst.model.as_str();
            match last {
                Some((prev, slot)) if prev == id => slot,
                _ => {
                    let slot = index.get(id).copied();
                    last = Some((id, slot));
                    slot
                }
            }
        }));
        // Whether each model is an authored chain, once per model.
        let chained: Vec<bool> = models.iter().map(|(k, _)| chained(k)).collect();
        self.distances.clear();
        self.distances.extend(instances.iter().map(|inst| crate::asset_lod::instance_distance(&inst.transform, eye)));
        self.lods.clear();
        self.lods.extend(self.slots.iter().zip(&self.distances).map(|(slot, distance)| slot.map_or(0, |at| {
            models[at].1.lods.partition_point(|(threshold, _)| *threshold <= *distance)
        })));
        self.order.clear();
        for (i, (slot, (&lod, &distance))) in self.slots.iter().zip(self.lods.iter().zip(&self.distances)).enumerate() {
            // Only authored chains (m.lod_models) fade: their few copies
            // (vehicles) are worth a second draw in the band; a decimated
            // prop's levels differ by less than the dither would show.
            let fade = slot.filter(|at| chained[*at]).and_then(|at| {
                chain_fade_windows(models[at].1.lods.get(lod)?.0, distance)
            });
            match fade {
                Some((near, next)) => {
                    self.order.push((i, lod, near));
                    self.order.push((i, lod + 1, next));
                }
                None => self.order.push((i, lod, 0.0)),
            }
        }
        // Culled once for every lane that walks this list: an entry with no
        // model, or offscreen by the lanes' own test (a copy with a
        // Splash material keeps its lane's test, which pads its bounds).
        // A crossfading copy's two entries are tested on their own levels.
        let mut culled = 0;
        {
            let slots = &self.slots;
            self.order.retain(|&(i, lod, _)| {
                let Some(at) = slots[i] else { return false };
                let (Some(frustum), None) = (frustum, instances[i].custom_material.as_ref()) else { return true };
                let root = &models[at].1;
                let loaded = if lod == 0 { root } else { &root.lods[lod - 1].1 };
                let m = vec3f(0.05, 0.05, 0.05);
                let t = &instances[i].transform;
                let shown = !loaded.anim_parts.is_empty() || frustum.intersects_obb(root.min - m, root.max + m, t) && !occluders.is_some_and(|r| {
                    // Hidden behind the streamed city's occluders (the
                    // raster its own chunks and props are tested against):
                    // the copy's world box, tested the same way.
                    let (mid, half) = ((root.min + root.max) * 0.5, (root.max - root.min) * 0.5 + m);
                    let t = &t.v;
                    let c = vec3f(t[0] * mid.x + t[4] * mid.y + t[8] * mid.z + t[12], t[1] * mid.x + t[5] * mid.y + t[9] * mid.z + t[13], t[2] * mid.x + t[6] * mid.y + t[10] * mid.z + t[14]);
                    let e = vec3f(
                        t[0].abs() * half.x + t[4].abs() * half.y + t[8].abs() * half.z,
                        t[1].abs() * half.x + t[5].abs() * half.y + t[9].abs() * half.z,
                        t[2].abs() * half.x + t[6].abs() * half.y + t[10].abs() * half.z,
                    );
                    r.occluded(c - e, c + e)
                });
                culled += !shown as u64;
                shown
            });
        }
        // Nearest first within each (model, LOD) group: a batch keeps its
        // geometry, and a cut-out layer (leaves) that cannot use hidden-
        // surface removal still has its hidden fragments rejected by the
        // early depth test instead of shaded and then covered. Distances
        // are non-negative, so their bits order like the floats.
        let (slots, distances) = (&self.slots, &self.distances);
        self.order.sort_unstable_by_key(|&(i, lod, _)| (slots[i], lod, distances[i].to_bits(), i));
        self.ao.clear();
        self.ao.resize(models.len(), None);
        self.key = key;
        culled
    }
}

impl Renderer {
    /// Draw one world-model lane. Instances are grouped by model, so N
    /// copies of one prop cost ONE draw item with N instances rather than N
    /// draws — which is what makes a scene full of stock trees affordable.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_models_inner(
        &mut self,
        cx: &mut Cx3d,
        mut draw: ModelDraw<'_>,
        eye: Vec3f,
        instances: &[ModelInstance],
        lane: WorldModelLane,
        fog: (Vec3f, f32),
        sun: &SunLight,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        self.apply_model_lod_chains();
        if instances.is_empty() {
            return;
        }
        let pbr_lane = draw.is_pbr();
        // Shiny models draw matte until the PBR pipeline exists (Metal
        // compiles asynchronously), rather than vanishing. Decided in the
        // diffuse pass; the PBR pass that follows reads the same answer.
        if !pbr_lane {
            let hdr = self.hdr_output;
            self.pbr_ready = self.pbr_draw.as_ref().and_then(|d| d.skinned.draw_vars.draw_shader_id).is_some_and(|id| cx.cx.draw_shader_ready(id, hdr));
        }
        let custom_name = match &draw {
            ModelDraw::Custom(name, _) => Some((*name).to_string()),
            _ => None,
        };
        self.bind_model_lane(cx, &mut draw, eye, fog, sun);
        // The lane's no-discard variant for draws that cut no pixel (opaque.rs).
        let opaque = match &draw {
            ModelDraw::Diffuse(_) => self.opaque_shader(cx.cx, opaque::OpaqueLane::Diffuse),
            ModelDraw::Pbr(_) => self.opaque_shader(cx.cx, opaque::OpaqueLane::Pbr),
            ModelDraw::Foliage(_) => self.opaque_shader(cx.cx, opaque::OpaqueLane::Foliage),
            // A Splash material's own discard-free program, when no hook of
            // it can cut a pixel (custom_material.rs).
            ModelDraw::Custom(_, m) => m.opaque_variant.filter(|id| cx.cx.draw_shader_ready(*id, self.hdr_output)),
            _ => None,
        };
        // Swaying models (a layer with wind) draw in the foliage lane once
        // its shader is ready, and in no other.
        let foliage_lane = matches!(draw, ModelDraw::Foliage(_));
        let foliage_ready = self.foliage_ready && matches!(lane, WorldModelLane::Placed);
        let occluder_focus = self.occluder.focus();
        // Dynamic lights. STATICS share one transient-only block (their
        // lamp light is baked; only the dl_split prefix reaches them, so a
        // zeroed lamp region costs nothing and keeps their per-model
        // batching whole). DYNAMICS are per-INSTANCE: each looks up ITS OWN
        // precomputed grid cell (O(1), light_grid.rs) with a positional
        // dead-band, so a car parked under a lamp holds a byte-identical
        // block frame after frame — the no-flicker contract. Different
        // blocks split draw items on purpose (the append test compares
        // uniforms); a handful of extra items beats a light popping.
        let (mut anch_x, mut anch_z, mut anch_n) = (0.0f64, 0.0f64, 0u32);
        for inst in instances {
            anch_x += inst.transform.v[12] as f64;
            anch_z += inst.transform.v[14] as f64;
            anch_n += 1;
        }
        let anchor_all = vec3f(
            (anch_x / anch_n.max(1) as f64) as f32,
            1.0,
            (anch_z / anch_n.max(1) as f64) as f32,
        );
        let empty_block = LightBlock::default();
        let mut static_block = [0.0f32; LIGHT_BLOCK_FLOATS];
        let static_split = if self.clustered_enabled { 0 } else { merge_transients_into_block(
            &empty_block,
            &self.frame_lights,
            self.frame_baked_count..self.frame_lights.len(),
            anchor_all,
            &mut self.light_rank,
            &mut static_block,
        ) };
        // Which block the draw_vars currently carry; statics rewrite only
        // after a dynamic instance changed it.
        let mut dynamic_block_active = true;
        // Sort by model so equal geometry+texture land adjacent: consecutive
        // add_instance calls with unchanged geometry and texture accumulate
        // into a single draw item; and by LOD within a model: copies at
        // different distances draw different geometry, and interleaved they
        // split into one draw item per copy (a forest of LOD'd trees:
        // thousands of draws a frame). Resolved once per list per frame:
        // the diffuse lane builds it and the PBR, foliage and custom lanes
        // that walk the same list after it reuse it.
        let which = matches!(lane, WorldModelLane::Attachment) as usize;
        let key = (instances.as_ptr() as usize, instances.len());
        if matches!(draw, ModelDraw::Diffuse(_)) || self.model_orders[which].key != key {
            let mut built = std::mem::take(&mut self.model_orders[which]);
            let chains = &self.model_lod_chains;
            let occluders = self.stream.as_ref().map(|st| st.occluders());
            let culled = built.build(&self.static_models, instances, eye, key, &|id| chains.contains_key(id), frustum, occluders);
            match lane {
                WorldModelLane::Placed => stats.model_culled += culled,
                WorldModelLane::Attachment => stats.world_attachment_culled += culled,
            }
            self.model_orders[which] = built;
        }
        let mut model_order = std::mem::take(&mut self.model_orders[which]);
        let ModelOrder { slots, distances, order, ao: model_ao, .. } = &mut model_order;
        // Layer-major: every copy's first layer, then every copy's second.
        // A multi-layer model (a tree: trunk, then leaves) drawn copy by
        // copy alternates geometry, and the batcher only appends to the
        // previous item of the same geometry: thousands of scattered trees
        // were thousands of draws. Stats and rigid parts count on pass 0.
        let layer_passes = order.iter().filter_map(|(i, _, _)| slots[*i]).map(|at| {
            let root = &self.static_models[at].1;
            std::iter::once(root).chain(root.lods.iter().map(|(_, m)| m)).map(|m| (m.triangles > 0) as usize + m.extra_draws.len()).max().unwrap_or(1)
        }).max().unwrap_or(1).max(1);
        // Pass 0 walks every entry; later passes only the drawn entries that
        // have that many layers (entry, layer count).
        // The models are read, not cloned, per copy: set aside for the walk
        // (nothing below reads them through `self`) and put back after it.
        let static_models = std::mem::take(&mut self.static_models);
        let mut layered: Vec<((usize, usize, f32), usize)> = Vec::new();
        for layer_pass in 0..layer_passes {
            let mut last: Option<(usize, usize)> = None;
            let pass_order: std::borrow::Cow<[(usize, usize, f32)]> = if layer_pass == 0 {
                std::borrow::Cow::Borrowed(&order[..])
            } else {
                std::borrow::Cow::Owned(layered.iter().filter(|(_, n)| *n > layer_pass).map(|(e, _)| *e).collect())
            };
            if pass_order.is_empty() {
                break;
            }
            for &(i, lod_index, window) in pass_order.iter() {
                let inst = &instances[i];
                let dynamic = lane.is_dynamic(inst);
                let Some(at) = slots[i] else {
                    continue;
                };
                let root = &static_models[at].1;
                let inv_height = 1.0 / root.max.y.max(0.5);
                let sways = foliage_ready && std::iter::once(&root.material).chain(root.extra_draws.iter().map(|l| &l.4))
                    .any(|m| m.surface.as_ref().is_some_and(|s| s.definition.wind > 0.0));
                let distance=distances[i];
                let mut fur_budget = 12_000usize.min(96_000usize.saturating_sub(stats.fur_triangles));
                let loaded=if lod_index==0{root}else{&root.lods[lod_index-1].1};
                // Lane filter. Both passes walk the same instance list — indices
                // address `lm_remaps` / `model_ground` / the light-cell key, so
                // they must not be renumbered — and each takes only the models
                // its shader owns.
                let uses_pbr_lane = self.pbr_materials_enabled && loaded.wants_pbr && self.pbr_ready;
                let wanted_custom = inst.custom_material.as_ref().filter(|m| {
                    custom_name.as_deref() == Some(m.name.as_str())
                        || self.custom_draws.get(&m.name).is_some_and(|d| d.draw.draw_vars.draw_shader_id.is_some_and(|id| cx.cx.draw_shader_ready(id, self.hdr_output)))
                });
                if let Some(name) = custom_name.as_deref() {
                    if wanted_custom.map(|m| m.name.as_str()) != Some(name) { continue; }
                    if let (ModelDraw::Custom(_, d), Some(material)) = (&mut draw, wanted_custom) {
                        d.draw.params = material.params;
                    }
                } else if wanted_custom.is_some() || sways != foliage_lane || (!foliage_lane && uses_pbr_lane != pbr_lane) {
                    continue;
                }
                // Hoisted: `loaded` borrows self, and the per-instance light
                // block below needs `&mut self` (cell hysteresis).
                draw.base().morph_ctl=Vec4f::default();
                if let Some(morph)=&loaded.morph{
                    let target=match lane{WorldModelLane::Placed=>ModelTarget::Instance(i),WorldModelLane::Attachment=>ModelTarget::Attachment(i)};
                    let weights=self.model_anim_state.morph_weights(&target,&inst.model,&morph.source);
                    draw.base().morph_ctl=vec4(morph.source.width as f32,morph.source.height as f32,morph.source.vertices as f32,morph.source.targets as f32);
                    draw.base().morph_weights0=vec4(weights[0],weights[1],weights[2],weights[3]);
                    draw.base().morph_weights1=vec4(weights[4],weights[5],weights[6],weights[7]);
                    draw.base().morph_weights2=vec4(weights[8],weights[9],weights[10],weights[11]);
                    draw.base().morph_weights3=vec4(weights[12],weights[13],weights[14],weights[15]);
                    draw.base().morph_weights4=vec4(weights[16],weights[17],weights[18],weights[19]);
                    draw.base().morph_weights5=vec4(weights[20],weights[21],weights[22],weights[23]);
                    draw.base().morph_weights6=vec4(weights[24],weights[25],weights[26],weights[27]);
                    draw.base().morph_weights7=vec4(weights[28],weights[29],weights[30],weights[31]);
                    if let Some(shader)=draw.base().draw_vars.draw_shader_id{if let Some(slot)=cx.draw_shaders[shader.index].mapping.textures.iter().position(|t|t.id==live_id!(morph_map)){draw.base().draw_vars.set_texture(slot,&morph.texture);}}
                }
                let tri_count = loaded.triangles;
                // Offscreen copies are skipped BEFORE packing, and a model whose
                // copies all fall outside never opens a draw item at all. The
                // shadow a prop casts is not affected: prop shadows live in the
                // merged static shadow mesh, which is drawn whole regardless.
                if let Some(frustum) = frustum.filter(|_| inst.custom_material.is_some()) {
                    // Generic node clips can leave the authored rest bounds.
                    // Their small, bounded part sets remain visible until posed
                    // bounds are available; a rest-only cull would hide motion.
                    // Fur can extend beyond the authored base geometry. The
                    // validated recipe is bounded to 5 cm in model space.
                    // A Splash material's vertex hook may move geometry up to
                    // its declared bounds_pad.
                    let pad = match &draw { ModelDraw::Custom(_, m) => m.bounds_pad, _ => 0.0 };
                    let fur_margin = vec3f(0.05 + pad, 0.05 + pad, 0.05 + pad);
                    if loaded.anim_parts.is_empty() && !frustum.intersects_obb(root.min-fur_margin, root.max+fur_margin, &inst.transform) {
                        if layer_pass == 0 {
                            match lane {
                                WorldModelLane::Placed => stats.model_culled += 1,
                                WorldModelLane::Attachment => stats.world_attachment_culled += 1,
                            }
                        }
                        continue;
                    }
                }
                let (layer_draws, prelit) = {
                    let m = loaded;
                    let mut layers = Vec::with_capacity(1 + m.extra_draws.len());
                    // A model that is ALL anim parts (a lone door asset) has an
                    // empty static stream and nothing to draw for layer 0.
                    if m.triangles > 0 {
                        layers.push((
                            m.geometry.geometry_id(),
                            &m.texture,
                            &m.detail,
                            m.detail_scale,
                            &m.material,
                        ));
                    }
                    for (g, t, d, s, mat) in &m.extra_draws {
                        layers.push((g.geometry_id(), t, d, *s, mat));
                    }
                    (layers, m.prelit)
                };
                if layer_pass == 0 && layer_draws.len() > 1 {
                    layered.push(((i, lod_index, window), layer_draws.len()));
                }
                // Rigid parts ride the PARENT's material: a door is cut from the
                // level tile it sits in, so its metal/roughness is the model's.
                let has_lightmap_source = loaded.lm_source.is_some();
                // The pack's baked occlusion, on slot 1. Packs share atlases;
                // a model's copies are adjacent (the order above), so the
                // binding changes at most once per model group. Resolved once
                // per model and frame, not searched per copy.
                let ao_at = *model_ao[at].get_or_insert_with(|| self
                    .model_pack
                    .iter()
                    .find(|(m, _)| *m == inst.model)
                    .and_then(|(_, pack)| self.ao_textures.iter().position(|(k, _)| k == pack)));
                let ao_tex = ao_at.map(|k| &self.ao_textures[k].1);
                if let Some(t) = ao_tex.filter(|_|lod_index==0) {
                    draw.base().draw_vars.set_texture(1, t);
                    draw.base().ao_enabled = 1.0;
                    stats.ao_bound += (layer_pass == 0) as u64;
                } else {
                    draw.base().ao_enabled = 0.0;
                    stats.ao_missing += (layer_pass == 0) as u64;
                }
                draw.base().transform = inst.transform;
                draw.base().tint = inst.tint;
                // A crossfading copy's window replaces its w (a plate's
                // number blanks for the few frames of the dissolve).
                let adjust = if window > 0.0 { vec4(inst.color_adjust.x, inst.color_adjust.y, inst.color_adjust.z, window) } else { inst.color_adjust };
                draw.base().color_adjust_ctl = adjust;
                // Screen-door cuts: the streamed-LOD dither, and the occluder
                // fade wherever its cone can reach this copy.
                // The level's ground (terrain tiles, road ribbons) never
                // fades: a dithered hole in the ground shows the void under
                // it, and a tile's bounding sphere (hundreds of metres) is
                // always inside the fade cone, which kept every ground tile
                // on the discarding pipeline: hidden ground under a city
                // then shaded in full (gpuperf lane, citydrive).
                let ground = inst.model.starts_with("gen/surface/level-terrain") || inst.model.starts_with("gen/surface/level-road");
                let screen_clip = adjust.w > 0.5
                    || !ground && occluder_focus.is_some_and(|focus| {
                        let (center, radius) = opaque::bounding_sphere(root.min, root.max, &inst.transform);
                        opaque::in_occluder_fade(eye, focus, center, radius)
                    });
                // This copy's window into the light atlas; zero disables — a
                // dynamic prop or an unbaked model lights analytically as before.
                draw.base().lm_rect = if dynamic || !has_lightmap_source {
                    Vec4f::default()
                } else {
                    self.lm_remaps.get(i).copied().unwrap_or_default()
                };
                // Dynamic-light gate: dynamics sum every uniform slot (lamps
                // included), statics only the transient prefix — their lamp
                // light is already in the atlas RGB.
                draw.base().dl_apply = if dynamic { 1.0 } else { 0.0 };
                draw.base().depth_bias = inst.depth_order * 1.0e-3;
                if dynamic && !self.clustered_enabled {
                    // This instance's OWN cell block + transients, and its
                    // ground plane for the sun-ray-projected shadow sample.
                    let (x, z) = (inst.transform.v[12], inst.transform.v[14]);
                    let cell = self.stable_light_cell(lane.light_key(i), x, z);
                    let block = match cell {
                        Some(c) => self.light_grid.block_of(c),
                        None => &empty_block,
                    };
                    let split = merge_transients_into_block(
                        block,
                        &self.frame_lights,
                        self.frame_baked_count..self.frame_lights.len(),
                        vec3f(x, inst.transform.v[13], z),
                        &mut self.light_rank,
                        &mut self.light_block_scratch,
                    );
                    write_light_block(cx.cx, &mut draw.base().draw_vars, &self.light_block_scratch, split);
                    draw.base().ground_y = match lane {
                        WorldModelLane::Placed => self.model_ground.get(i),
                        WorldModelLane::Attachment => self.world_attachment_ground.get(i),
                    }
                    .copied()
                    .unwrap_or(0.0);
                    dynamic_block_active = true;
                } else if dynamic_block_active {
                    write_light_block(cx.cx, &mut draw.base().draw_vars, &static_block, static_split);
                    draw.base().ground_y = 0.0;
                    dynamic_block_active = false;
                }
                if dynamic && self.clustered_enabled {
                    draw.base().ground_y = match lane {
                        WorldModelLane::Placed => self.model_ground.get(i),
                        WorldModelLane::Attachment => self.world_attachment_ground.get(i),
                    }.copied().unwrap_or(0.0);
                }
                if layer_pass == 0 && last.is_none_or(|(slot, level)| slot != at || level != lod_index) {
                    match lane {
                        WorldModelLane::Placed => stats.model_draws += 1,
                        WorldModelLane::Attachment => stats.world_attachment_draws += 1,
                    }
                    last = Some((at, lod_index));
                }
                if layer_pass == 0 { match lane {
                    WorldModelLane::Placed => {
                        stats.model_instances += 1;
                        stats.model_triangles += tri_count;
                    }
                    WorldModelLane::Attachment => {
                        stats.world_attachment_instances += 1;
                        stats.world_attachment_triangles += tri_count;
                    }
                } }
                // A model with a far stand-in (impostor.rs) draws that layer
                // from its distance on and every other layer before it.
                let impostor_at = layer_draws.iter().filter_map(|l| l.4.surface.as_ref().map(|s| s.definition.impostor)).fold(0.0f32, f32::max);
                for (geometry_id, texture, detail, dscale, material) in layer_draws.iter().skip(layer_pass).take(1) {
                    if impostor_at > 0.0 {
                        let stand_in = material.surface.as_ref().is_some_and(|s| s.definition.impostor > 0.0);
                        if stand_in != (distance >= impostor_at) { continue; }
                    }
                    draw.base().draw_vars.geometry_id = Some(*geometry_id);
                    draw.base().draw_vars.set_texture(0, texture);
                    draw.base().draw_vars.set_texture(5, detail);
                    draw.base().detail_st = vec2f(dscale[0], dscale[1]);
                    // An IBL material reads its environment on the detail slot.
                    if let (ModelDraw::Custom(_, m), Some(t)) = (&mut draw, self.ibl_texture()) { if m.ibl { m.draw.draw_vars.set_texture(5, t); } }
                    draw.base().prelit = if prelit { 1.0 } else { 0.0 };
                    draw.set_material(cx.cx, material);
                    // A far stand-in's cards in the foliage lane skip the
                    // cascade tap (tex_mag.y < 0: foliage.rs).
                    if foliage_lane && material.surface.as_ref().is_some_and(|s| s.definition.impostor > 0.0) {
                        draw.base().tex_mag.y = -1.0;
                    }
                    if let Some(s) = material.surface.as_ref().filter(|s| s.definition.wind > 0.0) {
                        // Sway in metres at the top at wind 1; leaves (cut-out
                        // layers) flutter on top of the sway.
                        let d = &s.definition;
                        let flutter = if d.alpha_mode == 1 { d.wind * 0.035 } else { 0.0 };
                        // A morphing model never sways (the lanes are shared).
                        if draw.base().morph_ctl.w < 0.5 {
                            draw.base().morph_ctl.w = -1.0;
                            draw.base().morph_weights0 = vec4(d.wind * 0.35, inv_height, self.sky_time, flutter);
                        }
                    }
                    let cut = screen_clip || match &material.surface {
                        // Masked layers discard; blended ones need the stock
                        // pipeline too (the no-discard variant draws opaque).
                        Some(surface) if pbr_lane => surface.definition.alpha_mode != 0,
                        _ => material.cutout,
                    };
                    stats.fur_triangles += draw.submit_as(cx, distance, &mut fur_budget, opaque.filter(|_| !cut));
                }
                if layer_pass > 0 {
                    continue;
                }
                // Rigid parts (doors, lifts). Each is one extra draw on the
                // PARENT's material — same shader, same textures, usually the
                // level tile it was cut from — placed at the instance transform
                // times where the part's state machine has it right now.
                //
                // A part carries no baked chart (lm_rect zeroed): it moves, so
                // the static atlas never had a window for it. Its shadow comes
                // from the realtime cascades, which see it as a mover.
                let parts: Vec<(Mat4f, usize, Vec<(GeometryId, Texture, Texture, [f32; 2],LayerMaterial)>)> = {
                    let root=&static_models[at].1;
                    let m = if lod_index==0{root}else{&root.lods[lod_index-1].1};
                    let mut parts = Vec::with_capacity(m.anim_parts.len() + m.driven_parts.len());
                    if !m.anim_parts.is_empty() {
                        let key = match lane {
                            WorldModelLane::Placed => ModelTarget::Instance(i),
                            // Attachments are not placed slots — a slot index
                            // would address someone else's instance — so their
                            // parts follow the per-MODEL command only.
                            WorldModelLane::Attachment => ModelTarget::Attachment(i),
                        };
                        parts.extend(m.anim_parts.iter().map(|p| {
                                let (_, _, _) =
                                    self.model_anim_state.clock(&key, &inst.model, &p.def);
                                (
                                    self.model_anim_state.transform(&key,&inst.model,&p.def),
                                    p.def.indices.len() / 3,
                                    p.draws
                                        .iter()
                                        .map(|(g, t, d, s,m)| {
                                            (g.geometry_id(), t.clone(), d.clone(), *s,m.clone())
                                        })
                                        .collect(),
                                )
                            }));
                    }
                    parts.extend(m.driven_parts.iter().map(|part| {
                        let pose = inst
                            .part_poses
                            .iter()
                            .find(|pose| pose.connection == part.def.connection)
                            .map(|pose| pose.transform)
                            .unwrap_or_else(|| part.def.rest_transform());
                        (
                            pose,
                            part.def.indices.len() / 3,
                            part
                                .draws
                                .iter()
                                .map(|(g, t, d, s,m)| (g.geometry_id(), t.clone(), d.clone(), *s,m.clone()))
                                .collect(),
                        )
                    }));
                    parts
                };
                for (pose, part_tris, part_draws) in &parts {
                    draw.base().transform = Mat4f::mul(&inst.transform, pose);
                    draw.base().lm_rect = Vec4f::default();
                    for (geometry_id, texture, detail, dscale,material) in part_draws {
                        draw.base().draw_vars.geometry_id = Some(*geometry_id);
                        draw.base().draw_vars.set_texture(0, texture);
                        draw.base().draw_vars.set_texture(5, detail);
                        draw.base().detail_st = vec2f(dscale[0], dscale[1]);
                        // An IBL material reads its environment on the detail slot.
                        if let (ModelDraw::Custom(_, m), Some(t)) = (&mut draw, self.ibl_texture()) { if m.ibl { m.draw.draw_vars.set_texture(5, t); } }
                        draw.base().prelit = if prelit { 1.0 } else { 0.0 };
                        draw.set_material(cx.cx, material);
                        stats.fur_triangles += draw.submit(cx, distance, &mut fur_budget);
                    }
                    match lane {
                        WorldModelLane::Placed => stats.model_triangles += part_tris,
                        WorldModelLane::Attachment => {
                            stats.world_attachment_triangles += part_tris
                        }
                    }
                }
                // A part opened its own draw item; the next instance of the same
                // model must re-bind its own transform rather than accumulate
                // into the part's.
                if !parts.is_empty() {
                    last = None;
                }
            }
        }
        self.static_models = static_models;
        self.model_orders[which] = model_order;
    }

    /// Everything a world-model lane binds ONCE before its instances: the
    /// cluster and GI tables, sun and fog, shading space, the specular eye,
    /// screen-space AO, the lightmap atlas, the cascades and the ground
    /// light field. Shared by the placed/attachment lanes and the streamed
    /// world lane (stream_draw.rs), so they can never shade differently.
    pub(super) fn bind_model_lane(
        &mut self,
        cx: &mut Cx3d,
        draw: &mut ModelDraw<'_>,
        eye: Vec3f,
        fog: (Vec3f, f32),
        sun: &SunLight,
    ) {
        let pbr_lane = draw.is_pbr();
        self.clustered.bind(cx.cx, &mut draw.base().draw_vars, self.clustered_enabled);
        self.gi.bind(cx.cx, &mut draw.base().draw_vars);
        {
            let draw = draw.base();
            sun.write_into(
                &mut draw.light_dir,
                &mut draw.sun_color,
                &mut draw.sun_sky,
                &mut draw.sun_ground,
            );
            draw.fog_color = fog.0;
            draw.fog_density = fog.1;
            draw.depth_clip = 1.0;
            draw.lm_debug = self.lm_debug;
        }
        // Shading space, once for the whole lane: written before anything
        // binds, so every draw item of the frame reads the same value.
        {
            let display = self.display_transform;
            let vars = &mut draw.base().draw_vars;
            vars.set_uniform(cx.cx, live_id!(display), &[display, 0.0, 0.0, 0.0]);
        }
        // The specular lobe is the one term in this renderer that needs the
        // camera. TRUE world position, matching v_csm.xyz / v_csm_n.
        if pbr_lane {
            let vars = &mut draw.base().draw_vars;
            vars.set_uniform(cx.cx, live_id!(eye), &[eye.x, eye.y, eye.z, 0.0]);
        }
        // The follow-camera occluder fade, once for the whole lane (both
        // model shaders declare it; w = 0 switches it off).
        {
            let (push, focus, clear) = self.occluder.uniforms();
            let vars = &mut draw.base().draw_vars;
            vars.set_uniform(cx.cx, live_id!(occ_eye), &[eye.x, eye.y, eye.z, push]);
            vars.set_uniform(cx.cx, live_id!(occ_focus), &[focus.x, focus.y, focus.z, clear]);
        }
        // Screen-space AO, once for the whole lane (both lanes pass through
        // here). Ambient-only by shader law; strength 0 = the slot is never
        // sampled, which is every game host's path.
        {
            let ssao = self.ssao.clone();
            let vars = &mut draw.base().draw_vars;
            match &ssao {
                Some((tex, strength)) => {
                    vars.set_texture(7, tex);
                    vars.set_uniform(cx.cx, live_id!(ssao_ctl), &[*strength, 0.0, 0.0, 0.0]);
                }
                None => {
                    vars.set_uniform(cx.cx, live_id!(ssao_ctl), &[0.0, 0.0, 0.0, 0.0]);
                }
            }
        }
        // One atlas for the whole static scene, so binding it here costs
        // nothing per instance and never breaks batching.
        let lm_tex = self.lightmap_texture(cx.cx);
        draw.base().draw_vars.set_texture(2, &lm_tex);
        {
            let csm = self.gpu_baker.csm_binding();
            Self::write_csm_uniforms(cx.cx, &mut draw.base().draw_vars, &csm, &lm_tex, 4, self.shadow_debug);
        }
        // The ground region, for DYNAMIC instances' baked sun shadow (a
        // driven car crossing a house's shadow darkens). A channel only —
        // their lamp light is analytic. The shadow-top plane rides along so
        // a dynamic lifted above the blocker rejects the ground's shadow.
        {
            let (top_tex, top_base, top_range) = self.lm_top_binding(cx.cx);
            draw.base().draw_vars.set_texture(3, &top_tex);
            draw.base().draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_top_decode),
                &[top_base, top_range, 0.0, 0.0],
            );
            let (g_rect, g_world) = self.lm_ground.unwrap_or_default();
            draw.base().draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_ground_rect),
                &[g_rect.x, g_rect.y, g_rect.z, g_rect.w],
            );
            draw.base().draw_vars.set_uniform(
                cx.cx,
                live_id!(lm_ground_world),
                &[g_world.x, g_world.y, g_world.z, g_world.w],
            );
        }
    }

    /// The specular half of the model pass: the same instance list, walked
    /// again for the models whose material carries shininess.
    ///
    /// The shader is the renderer's own, built on first use rather than lent
    /// through `SceneDraws` (the `sky_draw` pattern). That is deliberate: a
    /// host does not opt into PBR, a MODEL does, and every existing host —
    /// VJ, sandbox, the thumbnailer — gets the lane the moment it loads a
    /// shiny GLB without touching its widget or its script.
    ///
    /// Costs nothing when the scene has no shiny models: `wants_pbr` is a
    /// bool on an already-resident struct, so the early-out below is one
    /// linear scan of the instance list and no GPU work at all.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_pbr_models(
        &mut self,
        cx: &mut Cx3d,
        eye: Vec3f,
        instances: &[ModelInstance],
        lane: WorldModelLane,
        fog: (Vec3f, f32),
        sun: &SunLight,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        if !self.pbr_materials_enabled {
            return;
        }
        // The diffuse lane's order for this list (built this frame) holds
        // every copy's model slot: no hashing of every model name.
        let which = matches!(lane, WorldModelLane::Attachment) as usize;
        let any = {
            let order = &self.model_orders[which];
            if order.key == (instances.as_ptr() as usize, instances.len()) {
                order.slots.iter().any(|slot| slot.is_some_and(|at| self.static_models[at].1.wants_pbr))
            } else {
                let shiny: std::collections::HashSet<&str> = self.static_models.iter()
                    .filter(|(_, m)| m.wants_pbr).map(|(k, _)| k.as_str()).collect();
                !shiny.is_empty() && instances.iter().any(|inst| shiny.contains(inst.model.as_str()))
            }
        };
        if !any {
            return;
        }
        if self.pbr_draw.is_none() {
            // Held VM (a script-driven draw is mid-apply): try again next
            // frame rather than drawing with no shader.
            self.pbr_draw = cx
                .cx
                .try_with_vm(|vm| Box::new(DrawScenePbr::script_new_with_default(vm)));
        }
        if !self.pbr_ready {
            // The diffuse pass drew them this frame.
            return;
        }
        let Some(mut draw) = self.pbr_draw.take() else {
            return;
        };
        self.draw_models_inner(
            cx,
            ModelDraw::Pbr(&mut draw),
            eye,
            instances,
            lane,
            fog,
            sun,
            frustum,
            stats,
        );
        self.pbr_draw = Some(draw);
    }

    /// Draw every placed map's sky surfaces.
    ///
    /// One draw item per map: the faces are already one geometry, and the
    /// whole point of the lane is that a sky costs the same whether it is
    /// two brushes or two hundred. Depth is written normally — the faces sit
    /// where the level put them — but nothing else about the world reaches
    /// them: no lightmap window, no cascades, no fog, no sun.
    pub(super) fn draw_sky_faces(
        &mut self,
        cx: &mut Cx3d,
        eye: Vec3f,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        // Models first: most worlds load no sky model at all, and asking
        // per placed copy is a search of every model for every copy.
        if !self.static_models.iter().any(|(_, m)| m.sky.is_some())
            || !self
                .placed_models
                .iter()
                .any(|inst| self.model_sky(&inst.model).is_some())
        {
            return;
        }
        if self.sky_draw.is_none() {
            // Held VM (a script-driven draw is mid-apply): try again next
            // frame rather than drawing a sky with no shader.
            self.sky_draw = cx
                .cx
                .try_with_vm(|vm| Box::new(DrawSceneSkyMap::script_new_with_default(vm)));
        }
        let Some(mut draw) = self.sky_draw.take() else {
            if sky_trace_enabled() && !self.sky_draw_wait_logged {
                log!("map sky draw: waiting for DrawSceneSkyMap shader construction");
                self.sky_draw_wait_logged = true;
            }
            return;
        };
        self.sky_draw_wait_logged = false;
        let time = self.sky_time;
        let lin = self.lin_ctl();
        let instances = std::mem::take(&mut self.placed_models);
        for inst in &instances {
            let Some((_, loaded)) = self.static_models.iter_mut().find(|(k, _)| *k == inst.model)
            else {
                continue;
            };
            let Some(sky) = &mut loaded.sky else { continue };
            // A map whose sky is entirely behind the camera pays nothing.
            // Per-FACE culling is deliberately not attempted: the faces are
            // one geometry precisely so a sky costs one draw.
            if let Some(frustum) = frustum {
                if !frustum.intersects_obb(sky.part.min, sky.part.max, &inst.transform) {
                    if sky_trace_enabled() && sky.draw_trace & 1 == 0 {
                        log!(
                            "map sky draw: model='{}' frustum-culled bounds \
                             ({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2})",
                            inst.model,
                            sky.part.min.x,
                            sky.part.min.y,
                            sky.part.min.z,
                            sky.part.max.x,
                            sky.part.max.y,
                            sky.part.max.z,
                        );
                        sky.draw_trace |= 1;
                    }
                    stats.model_culled += 1;
                    continue;
                }
            }
            draw.transform = inst.transform;
            draw.depth_clip = 1.0;
            draw.eye = vec4(eye.x, eye.y, eye.z, 0.0);
            draw.sky_p = vec4(
                sky.part.projection.code(),
                sky.part.repeat,
                sky.part.scroll(0, time),
                sky.part.scroll(1, time),
            );
            draw.sky_q = vec4(sky.part.v_span, 1.0, 0.0, 0.0);
            draw.draw_vars.geometry_id = Some(sky.geometry.geometry_id());
            draw.draw_vars.set_texture(0, &sky.tex0);
            draw.draw_vars.set_texture(1, &sky.tex1);
            draw.draw_vars.set_uniform(cx.cx, live_id!(lin_ctl), &lin);
            let shader_ready = draw.draw_vars.can_instance();
            if sky_trace_enabled() && sky.draw_trace & 2 == 0 {
                let resident0 = match sky.tex0.get_format(cx.cx) {
                    TextureFormat::VecBGRAu8_32 {
                        width,
                        height,
                        data,
                        updated,
                    } => format!(
                        "VecBGRAu8_32({width}x{height}, texels={}, first={:#010x}, {updated:?})",
                        data.as_ref().map_or(0, Vec::len),
                        data.as_ref().and_then(|pixels| pixels.first()).copied().unwrap_or(0),
                    ),
                    TextureFormat::VecMipBGRAu8_32 {
                        width,
                        height,
                        data,
                        max_level,
                        wrap,
                        updated,
                    } => format!(
                        "VecMipBGRAu8_32({width}x{height}, texels={}, first={:#010x}, \
                         max_level={max_level:?}, wrap={wrap:?}, {updated:?})",
                        data.as_ref().map_or(0, Vec::len),
                        data.as_ref()
                            .and_then(|pixels| pixels.first())
                            .copied()
                            .unwrap_or(0),
                    ),
                    other => format!("{other:?}"),
                };
                let shader_textures = draw
                    .draw_vars
                    .draw_shader_id
                    .map(|shader| {
                        cx.cx.draw_shaders[shader.index]
                            .mapping
                            .textures
                            .iter()
                            .map(|input| format!("{}", input.id))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                log!(
                    "map sky draw: model='{}' texture='{}' shader_ready={} cull={} brightness={:.3} \
                     layer0={:?} layer1={:?} bound0={:?} bound1={:?} inputs={:?} \
                     resident0={} tris={}",
                    inst.model,
                    sky.part.texture.as_deref().unwrap_or("<unnamed>"),
                    shader_ready,
                    draw.draw_vars.options.backface_culling,
                    draw.brightness,
                    sky.tex0.texture_id(),
                    sky.tex1.texture_id(),
                    draw.draw_vars.texture_slots[0]
                        .as_ref()
                        .map(Texture::texture_id),
                    draw.draw_vars.texture_slots[1]
                        .as_ref()
                        .map(Texture::texture_id),
                    shader_textures,
                    resident0,
                    sky.part.triangle_count(),
                );
                sky.draw_trace |= 2;
            }
            if shader_ready {
                let new_area = cx.add_instance(&draw.draw_vars);
                draw.draw_vars.area = cx.update_area_refs(draw.draw_vars.area, new_area);
            }
            stats.model_draws += 1;
            stats.model_triangles += sky.part.triangle_count();
        }
        self.placed_models = instances;
        self.sky_draw = Some(draw);
    }

    /// One layer's material on the held-model shader: factors, the ORM,
    /// normal and emissive maps (bound by name; a missing map binds the
    /// ORM's white 1x1 and its strength is 0), the mask cutout.
    fn bind_view_model_material(cx: &Cx3d, draw: &mut DrawSceneViewModel, m: &LayerMaterial) {
        draw.metallic = m.metallic;
        draw.roughness = m.roughness;
        draw.orm_on = if m.orm_on { 1.0 } else { 0.0 };
        draw.normal_scale = 0.0;
        draw.alpha_mode = 0.0;
        draw.alpha_cutoff = 0.5;
        draw.emissive = vec3f(0.0, 0.0, 0.0);
        if let Some(surface) = &m.surface {
            let d = &surface.definition;
            draw.normal_scale = d.normal_scale;
            draw.alpha_mode = if d.alpha_mode == 1 { 1.0 } else { 0.0 };
            draw.alpha_cutoff = d.alpha_cutoff;
            draw.emissive = vec3f(d.emissive[0], d.emissive[1], d.emissive[2]);
        }
        let Some(id) = draw.draw_vars.draw_shader_id else { return };
        let maps = [
            (live_id!(orm_map), Some(&m.orm)),
            (live_id!(normal_map), m.surface.as_ref().map(|s| &s.normal)),
            (live_id!(emissive_map), m.surface.as_ref().map(|s| &s.emissive)),
        ];
        for (name, texture) in maps {
            if let Some(slot) = cx.draw_shaders[id.index].mapping.textures.iter().position(|t| t.id == name).filter(|slot| *slot < draw.draw_vars.texture_slots.len()) {
                draw.draw_vars.set_texture(slot, texture.unwrap_or(&m.orm));
            }
        }
    }

    /// Draw the private FPS presentation list. This path deliberately does
    /// not share world-model setup: no AO/lightmap/top-map/CSM bindings, no
    /// dynamic-light selection, no frustum/caster/receiver work. A typical
    /// list is one 70-triangle pistol and therefore one draw item.
    pub(super) fn draw_view_models_inner(
        &mut self,
        cx: &mut Cx3d,
        draw: &mut DrawSceneViewModel,
        sun: &SunLight,
        fog_color: Vec3f,
        stats: &mut RenderStats,
    ) {
        if self.view_models.is_empty() {
            return;
        }
        sun.write_into(
            &mut draw.light_dir,
            &mut draw.sun_color,
            &mut draw.sun_sky,
            &mut draw.sun_ground,
        );
        draw.lin = self.lin_ctl()[0];
        draw.sun_vis = self.view_model_sun;
        draw.fog_color = fog_color;
        let stage_inv = self.stage.matrix().invert();
        for inst in &self.view_models {
            let Some((_, loaded)) = self.static_models.iter().find(|(id, _)| *id == inst.model)
            else {
                continue;
            };
            let layer_draws = {
                let mut layers = Vec::with_capacity(1 + loaded.extra_draws.len());
                layers.push((loaded.geometry.geometry_id(), loaded.texture.clone(), loaded.material.clone()));
                for (g, t, _, _, m) in &loaded.extra_draws {
                    layers.push((g.geometry_id(), t.clone(), m.clone()));
                }
                layers
            };
            // The scene draw-list carries Stage for every world draw. Cancel
            // it here: a held model is attached to the physical camera, not
            // scaled into an MR diorama or moved onto its floor anchor.
            draw.transform = Mat4f::mul(&stage_inv, &inst.transform);
            for (geometry_id, texture, material) in &layer_draws {
                draw.draw_vars.geometry_id = Some(*geometry_id);
                draw.draw_vars.set_texture(0, texture);
                Self::bind_view_model_material(cx, draw, material);
                if draw.draw_vars.can_instance() {
                    let new_area = cx.add_instance(&draw.draw_vars);
                    draw.draw_vars.area = cx.update_area_refs(draw.draw_vars.area, new_area);
                }
            }
            stats.view_model_instances += 1;
            stats.view_model_triangles += loaded.triangles;
        }
    }
}

#[cfg(test)]
mod chain_fade_tests {
    use super::chain_fade_windows;

    /// The shader's test (skinned.rs / pbr.rs): shown while lo <= n*255 < hi.
    fn shows(window: f32, n: f32) -> bool {
        let dv = window - 1.0;
        let lo = (dv / 256.0).floor();
        let hi = dv - lo * 256.0;
        !(n < lo / 255.0 || n >= hi / 255.0)
    }

    #[test]
    fn a_copy_in_the_band_shows_each_pixel_in_exactly_one_level() {
        for d in [27.1f32, 28.0, 28.5, 29.2, 29.9] {
            let (near, next) = chain_fade_windows(30.0, d).expect("inside the band");
            for k in 0..1000 {
                let n = k as f32 / 1000.0;
                assert!(shows(near, n) != shows(next, n), "d {d} n {n}");
            }
        }
    }

    #[test]
    fn the_next_level_takes_over_across_the_band() {
        let cover = |d: f32| { let (_, next) = chain_fade_windows(90.0, d).unwrap(); (0..1000).filter(|k| shows(next, *k as f32 / 1000.0)).count() };
        assert!(cover(81.5) < 100 && cover(85.5) > 400 && cover(85.5) < 600 && cover(89.5) > 900);
        assert!(chain_fade_windows(90.0, 80.0).is_none() && chain_fade_windows(90.0, 90.0).is_none() && chain_fade_windows(90.0, 120.0).is_none());
    }
}
