//! World-model lanes: diffuse/PBR/custom model draws, sky faces, view models.

use super::*;
use super::realm::ModelList;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LayerPhase {
    Opaque,
    Blended,
}

/// Only the world-model kinds visit draw_models_inner; City and Grass use
/// ModelDraw for lane binding alone.
enum LayerLane {
    Diffuse,
    Pbr,
    Splat,
    Foliage,
    Custom(String),
}

impl LayerLane {
    fn matches(&self, draw: &ModelDraw<'_>) -> bool {
        match (self, draw) {
            (Self::Diffuse, ModelDraw::Diffuse(_))
            | (Self::Pbr, ModelDraw::Pbr(_))
            | (Self::Splat, ModelDraw::Splat(_))
            | (Self::Foliage, ModelDraw::Foliage(_)) => true,
            (Self::Custom(name), ModelDraw::Custom(other, _)) => name.as_str() == *other,
            _ => false,
        }
    }

    fn of(draw: &ModelDraw<'_>) -> Self {
        match draw {
            ModelDraw::Diffuse(_) => Self::Diffuse,
            ModelDraw::Pbr(_) => Self::Pbr,
            ModelDraw::Splat(_) => Self::Splat,
            ModelDraw::Foliage(_) => Self::Foliage,
            ModelDraw::Custom(name, _) => Self::Custom((*name).to_string()),
            ModelDraw::City(_) | ModelDraw::Grass(_) | ModelDraw::Traced(..) => unreachable!("not a deferred model lane"),
        }
    }
}

/// A draw in one model LOD. Replay uses the same indices as the opaque walk.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelLayer {
    Body(usize),
    Animated(usize, usize),
    Driven(usize, usize),
}

impl LoadedModel {
    fn part_layers(&self) -> impl Iterator<Item = ModelLayer> + '_ {
        self.anim_parts.iter().enumerate().flat_map(|(part, p)| {
            (0..p.draws.len()).map(move |layer| ModelLayer::Animated(part, layer))
        }).chain(self.driven_parts.iter().enumerate().flat_map(|(part, p)| {
            (0..p.draws.len()).map(move |layer| ModelLayer::Driven(part, layer))
        }))
    }

    fn layer_draw(&self, layer: ModelLayer) -> Option<(GeometryId, &Texture, &Texture, [f32; 2], &LayerMaterial)> {
        let primary = usize::from(self.triangles > 0);
        let draw = match layer {
            ModelLayer::Body(0) if primary == 1 => return Some((
                self.geometry.geometry_id(), &self.texture, &self.detail, self.detail_scale, &self.material,
            )),
            ModelLayer::Body(i) => self.extra_draws.get(i - primary)?,
            ModelLayer::Animated(part, i) => &self.anim_parts[part].draws[i],
            ModelLayer::Driven(part, i) => &self.driven_parts[part].draws[i],
        };
        Some((draw.0.geometry_id(), &draw.1, &draw.2, draw.3, &draw.4))
    }
}

struct BlendedLayer {
    entry: (usize, usize, f32),
    layer: ModelLayer,
    sequence: usize,
}

struct BlendedLane {
    list: WorldModelLane,
    lane: LayerLane,
    layers: Vec<BlendedLayer>,
}

/// Buckets and their strings/entry storage survive frames. Active indices
/// retain the opaque calls' order, including custom programs' name order.
#[derive(Default)]
pub(super) struct BlendedModels {
    lanes: Vec<BlendedLane>,
    active: Vec<usize>,
    /// The ordered custom lanes and their installed names during the opaque walk.
    pub(super) programs: super::realm::ProgramWalk,
}

impl BlendedModels {
    pub(super) fn clear(&mut self) {
        for &at in &self.active {
            self.lanes[at].layers.clear();
        }
        self.active.clear();
    }

    pub(super) fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    fn find(&self, list: WorldModelLane, draw: &ModelDraw<'_>) -> Option<usize> {
        self.lanes.iter().position(|lane| lane.list == list && lane.lane.matches(draw))
    }

    fn record(&mut self, at: &mut Option<usize>, list: WorldModelLane, draw: &ModelDraw<'_>, entry: (usize, usize, f32), layer: ModelLayer) {
        let at = *at.get_or_insert_with(|| self.find(list, draw).unwrap_or_else(|| {
            self.lanes.push(BlendedLane { list, lane: LayerLane::of(draw), layers: Vec::new() });
            self.lanes.len() - 1
        }));
        let layers = &mut self.lanes[at].layers;
        if layers.is_empty() {
            self.active.push(at);
        }
        layers.push(BlendedLayer { entry, layer, sequence: layers.len() });
    }
}

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

/// The model lanes' names for their draws ("model gen/mdl/valley-oak lod1 layer2", "impostor
/// gen/mdl/valley-oak"), which the GPU profiler and frame captures show as the draws' owners.
/// Asked only while owners are recorded (`Cx::draw_owners_armed`); each name is formatted and
/// interned once, after that naming a draw is a lookup.
#[derive(Default)]
pub(super) struct DrawNames {
    /// model id -> ((LOD, layer, stand-in) packed, label)
    models: std::collections::HashMap<String, Vec<(u32, DrawLabel)>>,
}

impl DrawNames {
    pub(super) fn model(&mut self, cx: &mut Cx, model: &str, lod: usize, layer: usize, stand_in: bool) -> DrawLabel {
        let key = (lod as u32) << 16 | (layer as u32 & 0x7fff) << 1 | stand_in as u32;
        if let Some((_, label)) = self.models.get(model).and_then(|v| v.iter().find(|(k, _)| *k == key)) {
            return *label;
        }
        let mut name = format!("{} {model}", if stand_in { "impostor" } else { "model" });
        if lod > 0 {
            name.push_str(&format!(" lod{lod}"));
        }
        if layer > 0 && !stand_in {
            name.push_str(&format!(" layer{layer}"));
        }
        let label = cx.draw_label(&name);
        self.models.entry(model.to_string()).or_default().push((key, label));
        label
    }
}

/// One instance list's draw order for one frame: each copy's model slot and
/// LOD, and the copies grouped by (model, LOD).
#[derive(Default)]
pub(super) struct ModelOrder {
    key: (usize, usize, u32),
    slots: Vec<Option<usize>>,
    lods: Vec<usize>,
    distances: Vec<f32>,
    /// The opaque phase's shared transient-light anchor, reused by deferred layers.
    anchor_all: Vec3f,
    /// Draw entries: (copy, LOD, screen-door window). A copy crossfading
    /// between two chain levels has an entry for each, their windows
    /// complementary (the shader's `color_adjust_ctl.w` dither code); 0 is
    /// no window.
    order: Vec<(usize, usize, f32)>,
    /// Each model slot's pack occlusion atlas (index into `ao_textures`),
    /// resolved on first use this frame.
    ao: Vec<Option<Option<usize>>>,
    /// Scratch kept across frames: the drawn entries with their layer counts,
    /// and the later layer passes' entry list.
    layered: Vec<((usize, usize, f32), usize)>,
    pass_buf: Vec<(usize, usize, f32)>,
}

/// Loose slots are inserted in ascending order. Vacating their indexed cells
/// preserves that order without shifting an arbitrarily large loose tail.
#[derive(Default, Debug)]
pub(super) struct LooseSlots(Vec<Option<u32>>);

impl LooseSlots {
    pub(super) fn iter(&self) -> impl Iterator<Item = u32> + '_ { self.0.iter().flatten().copied() }
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool { self.iter().next().is_none() }
}

#[cfg(test)]
impl<const N: usize> PartialEq<[u32; N]> for LooseSlots {
    fn eq(&self, other: &[u32; N]) -> bool { self.iter().eq(other.iter().copied()) }
}

#[derive(Clone, Copy)]
enum PlacedMember {
    Block(usize),
    Loose(usize),
}

/// The placed list's static copies grouped by place (64 m cells) with
/// their world bounds, so the once-per-frame cull skips a whole patch of a
/// forest at once. Rebuilt when the placed scene, the loaded models or the
/// set of Splash-material copies changes; each member keeps its model slot.
#[derive(Default)]
pub(super) struct PlacedBlocks {
    key: Option<(u64, u64, u64, usize)>,
    /// (world min, world max, members as (copy, model slot)).
    blocks: Vec<(Vec3f, Vec3f, Vec<(u32, u32)>)>,
    /// Copies no block holds (dynamic, Splash material, parts or a morph,
    /// model not loaded): tested one by one every frame. Ascending.
    loose: LooseSlots,
    membership: Vec<Option<PlacedMember>>,
}

impl PlacedBlocks {
    /// The loose copies, when the blocks were built for this very list (a
    /// list of `len` copies with this signature); None otherwise.
    pub(super) fn loose_for(&self, len: usize, signature: Option<u64>) -> Option<&LooseSlots> {
        self.key.filter(|k| Some(k.0) == signature && k.3 == len).map(|_| &self.loose)
    }

    pub(super) fn invalidate(&mut self) { self.key = None; }

    #[cfg(test)]
    pub(super) fn snapshot(&self, renderer: &Renderer) -> (Vec<(Vec3f, Vec3f, Vec<(u32, u32)>)>, Vec<u32>, Vec<usize>) {
        let mut order = ModelOrder::default();
        order.build(&renderer.static_models, &renderer.placed_models, Vec3f::default(), 1.0,
            renderer.placed_models.view().key(), &|_| false, None, None, Some(self));
        let visible = order.order.iter().map(|entry| entry.0).collect();
        (self.blocks.clone(), self.loose.iter().collect(), visible)
    }

    fn custom_slot_key(slot: usize) -> u64 {
        use std::hash::Hasher;
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        hash.write_usize(slot);
        hash.finish()
    }

    pub(super) fn remove(&mut self, slot: usize, custom: bool) {
        let Some(member) = self.membership.get_mut(slot).and_then(Option::take) else { return };
        if custom {
            if let Some(key) = &mut self.key { key.2 ^= Self::custom_slot_key(slot); }
        }
        match member {
            PlacedMember::Block(index) => {
                let members = &mut self.blocks[index].2;
                if let Ok(at) = members.binary_search_by_key(&(slot as u32), |m| m.0) { members.remove(at); }
            }
            PlacedMember::Loose(index) => self.loose.0[index] = None,
        }
    }

    pub(super) fn refresh(&mut self, models: &[(String, LoadedModel)], instances: &(impl ModelList + ?Sized), signature: Option<u64>) {
        let instances = instances.view();
        let fingerprint = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for (_, m) in models {
                m.geometry.geometry_id().hash(&mut h);
                m.lods.len().hash(&mut h);
                for (_, l) in &m.lods {
                    l.geometry.geometry_id().hash(&mut h);
                }
            }
            h.finish()
        };
        let custom = instances.live().filter(|(_, inst)| inst.has_custom())
            .fold(0u64, |h, (i, _)| h ^ Self::custom_slot_key(i));
        let key = signature.map(|signature| (signature, fingerprint, custom, instances.len()));
        if key.is_some() && key == self.key {
            return;
        }
        self.key = key;
        self.blocks.clear();
        self.loose.0.clear();
        self.membership.clear();
        self.membership.resize(instances.len(), None);
        if key.is_none() {
            for (i, _) in instances.live() {
                self.membership[i] = Some(PlacedMember::Loose(self.loose.0.len()));
                self.loose.0.push(Some(i as u32));
            }
            return;
        }
        let index: std::collections::HashMap<&str, usize> =
            models.iter().enumerate().map(|(i, (k, _))| (k.as_str(), i)).collect();
        let mut cells: std::collections::HashMap<(i32, i32), usize> = std::collections::HashMap::new();
        for (i, inst) in instances.live() {
            let slot = index.get(inst.model.as_str()).copied();
            let root = slot.map(|at| &models[at].1);
            let Some((at, root)) = slot.zip(root).filter(|(_, root)| {
                !inst.dynamic && !inst.has_custom()
                    && std::iter::once(*root).chain(root.lods.iter().map(|(_, m)| m)).all(|m| m.anim_parts.is_empty() && m.driven_parts.is_empty() && m.morph.is_none())
            }) else {
                self.membership[i] = Some(PlacedMember::Loose(self.loose.0.len()));
                self.loose.0.push(Some(i as u32));
                continue;
            };
            // The copy's world box, padded as the lanes' own test pads it.
            let pad = vec3f(0.05, 0.05, 0.05);
            let (mid, half) = ((root.min + root.max) * 0.5, (root.max - root.min) * 0.5 + pad);
            let t = &inst.transform.v;
            let c = vec3f(t[0] * mid.x + t[4] * mid.y + t[8] * mid.z + t[12], t[1] * mid.x + t[5] * mid.y + t[9] * mid.z + t[13], t[2] * mid.x + t[6] * mid.y + t[10] * mid.z + t[14]);
            let e = vec3f(
                t[0].abs() * half.x + t[4].abs() * half.y + t[8].abs() * half.z,
                t[1].abs() * half.x + t[5].abs() * half.y + t[9].abs() * half.z,
                t[2].abs() * half.x + t[6].abs() * half.y + t[10].abs() * half.z,
            );
            let (lo, hi) = (c - e, c + e);
            let cell = ((c.x / 64.0).floor() as i32, (c.z / 64.0).floor() as i32);
            let b = *cells.entry(cell).or_insert_with(|| {
                self.blocks.push((lo, hi, Vec::new()));
                self.blocks.len() - 1
            });
            self.membership[i] = Some(PlacedMember::Block(b));
            let block = &mut self.blocks[b];
            block.0 = vec3f(block.0.x.min(lo.x), block.0.y.min(lo.y), block.0.z.min(lo.z));
            block.1 = vec3f(block.1.x.max(hi.x), block.1.y.max(hi.y), block.1.z.max(hi.z));
            block.2.push((i as u32, at as u32));
        }
    }
}

impl ModelOrder {
    fn build(&mut self, models: &[(String, LoadedModel)], instances: &(impl ModelList + ?Sized), eye: Vec3f, lod_scale: f32, key: (usize, usize), chained: &dyn Fn(&str) -> bool, frustum: Option<&Frustum>, occluders: Option<&crate::stream::OcclusionRaster>, blocks: Option<&PlacedBlocks>) -> u64 {
        let instances = instances.view();
        // Model slot per instance, resolved once through a map: a linear
        // search per instance is O(instances x models), which a streamed or
        // prop-heavy world turns into milliseconds.
        let index: std::collections::HashMap<&str, usize> =
            models.iter().enumerate().map(|(i, (k, _))| (k.as_str(), i)).collect();
        let n = instances.len();
        self.slots.clear();
        self.slots.resize(n, None);
        self.distances.clear();
        self.distances.resize(n, 0.0);
        self.lods.clear();
        self.lods.resize(n, 0);
        let mut culled = 0;
        {
            let (slots, distances, lods) = (&mut self.slots, &mut self.distances, &mut self.lods);
            let mut visit = |i: usize, slot: Option<usize>| {
                let distance = crate::asset_lod::instance_distance(&instances[i].transform, eye);
                slots[i] = slot;
                distances[i] = distance;
                let lod_distance = distance * lod_scale;
                lods[i] = slot.map_or(0, |at| models[at].1.lods.partition_point(|(threshold, _)| *threshold <= lod_distance));
            };
            // Copies of one model often follow each other: those skip the hash.
            let mut last: Option<(&str, Option<usize>)> = None;
            let mut slot_of = |i: usize| {
                let id = instances[i].model.as_str();
                match last {
                    Some((prev, slot)) if prev == id => slot,
                    _ => {
                        let slot = index.get(id).copied();
                        last = Some((id, slot));
                        slot
                    }
                }
            };
            match blocks {
                None => instances.live().for_each(|(i, _)| visit(i, slot_of(i))),
                Some(blocks) => {
                    for i in blocks.loose.iter() {
                        visit(i as usize, slot_of(i as usize));
                    }
                    // A block wholly offscreen or hidden holds only copies
                    // the per-copy tests below would drop (their boxes lie
                    // inside it): skipped without a look at them.
                    for (lo, hi, members) in &blocks.blocks {
                        if frustum.is_some_and(|f| !f.intersects_aabb(*lo, *hi)) || occluders.is_some_and(|r| r.occluded(*lo, *hi)) {
                            culled += members.len() as u64;
                            continue;
                        }
                        for &(i, at) in members {
                            visit(i as usize, Some(at as usize));
                        }
                    }
                }
            }
        }
        // Whether each model is an authored chain, once per model.
        let chained: Vec<bool> = models.iter().map(|(k, _)| chained(k)).collect();
        self.order.clear();
        for (i, (slot, (&lod, &distance))) in self.slots.iter().zip(self.lods.iter().zip(&self.distances)).enumerate() {
            if slot.is_none() { continue; }
            // Only authored chains (m.lod_models) fade: their few copies
            // (vehicles) are worth a second draw in the band; a decimated
            // prop's levels differ by less than the dither would show.
            let fade = slot.filter(|at| chained[*at]).and_then(|at| {
                chain_fade_windows(models[at].1.lods.get(lod)?.0, distance * lod_scale)
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
        {
            let slots = &self.slots;
            self.order.retain(|&(i, lod, _)| {
                let Some(at) = slots[i] else { return false };
                let Some(frustum) = frustum.filter(|_| !instances[i].has_custom()) else { return true };
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
        self.key = (key.0, key.1, lod_scale.to_bits());
        culled
    }
}

impl Renderer {
    /// Replay only lanes that deferred a layer, in the opaque phase's order
    /// within each list. Both the queue storage and custom draw keys are reused.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_blended_models(
        &mut self, cx: &mut Cx3d, diffuse: &mut DrawSceneSkinned, eye: Vec3f, lod_scale: f32,
        fog: (Vec3f, f32), sun: &SunLight, frustum: Option<&Frustum>, stats: &mut RenderStats,
    ) {
        enum Copies {
            Placed(super::realm::PlacedModels),
            Attachments(Vec<ModelInstance>),
        }
        let active = std::mem::take(&mut self.blended_models.active);
        for list in [WorldModelLane::Placed, WorldModelLane::Attachment] {
            let held = match list {
                WorldModelLane::Placed => Copies::Placed(std::mem::take(&mut self.placed_models)),
                WorldModelLane::Attachment => Copies::Attachments(std::mem::take(&mut self.world_attachments)),
            };
            let instances = match &held {
                Copies::Placed(placed) => placed.view(),
                Copies::Attachments(attachments) => attachments.view(),
            };
            for &at in &active {
                if self.blended_models.lanes[at].list != list { continue; }
                match &self.blended_models.lanes[at].lane {
                    LayerLane::Diffuse => {
                        self.draw_models_inner(cx, ModelDraw::Diffuse(diffuse), LayerPhase::Blended,
                            eye, lod_scale, &instances, list, fog, sun, frustum, stats);
                    }
                    LayerLane::Pbr => {
                        let Some(mut draw) = self.pbr_draw.take() else { continue };
                        self.draw_models_inner(cx, ModelDraw::Pbr(&mut draw), LayerPhase::Blended,
                            eye, lod_scale, &instances, list, fog, sun, frustum, stats);
                        self.pbr_draw = Some(draw);
                    }
                    LayerLane::Splat => {
                        let Some(mut draw) = self.splat_draw.take() else { continue };
                        self.draw_models_inner(cx, ModelDraw::Splat(&mut draw), LayerPhase::Blended,
                            eye, lod_scale, &instances, list, fog, sun, frustum, stats);
                        self.splat_draw = Some(draw);
                    }
                    LayerLane::Foliage => {
                        let Some(mut draw) = self.foliage_draw.take() else { continue };
                        self.draw_models_inner(cx, ModelDraw::Foliage(&mut draw), LayerPhase::Blended,
                            eye, lod_scale, &instances, list, fog, sun, frustum, stats);
                        self.foliage_draw = Some(draw);
                    }
                    LayerLane::Custom(name) => {
                        let mut programs = std::mem::take(&mut self.custom_draws);
                        // Borrow both the key and its draw so replay keeps the registry nodes.
                        let bound = std::ops::Bound::Included(name.as_str());
                        if let Some((name, draw)) = programs.range_mut::<str, _>((bound, bound)).next() {
                            self.draw_models_inner(cx, ModelDraw::Custom(name, draw), LayerPhase::Blended,
                                eye, lod_scale, &instances, list, fog, sun, frustum, stats);
                        }
                        self.custom_draws = programs;
                    }
                }
            }
            match held {
                Copies::Placed(placed) => self.placed_models = placed,
                Copies::Attachments(attachments) => self.world_attachments = attachments,
            }
        }
        self.blended_models.active = active;
    }

    /// Install the lane's shader for this pass and return the optional no-clip variant. A draw
    /// held from before a live edit takes the edit first, so what follows binds the edited shader.
    fn select_model_shader(
        &mut self, cx: &mut Cx3d, vars: &mut DrawVars, lane: variants::ModelLane,
    ) -> Option<DrawShaderId> {
        vars.refresh_after_reload(cx.cx);
        let held = vars.draw_shader_id;
        let (full, opaque) = self.lane_shaders(cx.cx, lane, held);
        vars.draw_shader_id = self.select_lane_shader(cx.cx, lane, full, held, |id| cx.draw_shader_ready_here(id));
        opaque
    }

    /// Draw one world-model lane. Instances are grouped by model, so N
    /// copies of one prop cost ONE draw item with N instances rather than N
    /// draws — which is what makes a scene full of stock trees affordable.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_models_inner(
        &mut self,
        cx: &mut Cx3d,
        mut draw: ModelDraw<'_>,
        phase: LayerPhase,
        eye: Vec3f,
        lod_scale: f32,
        instances: &(impl ModelList + ?Sized),
        lane: WorldModelLane,
        fog: (Vec3f, f32),
        sun: &SunLight,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        if phase == LayerPhase::Opaque {
            self.apply_model_lod_chains();
        }
        let instances = instances.view();
        if instances.len() == 0 {
            if let (WorldModelLane::Placed, LayerPhase::Opaque, ModelDraw::Diffuse(diffuse)) = (lane, phase, &mut draw) {
                // Request this pass's fallback pipeline before its first model arrives.
                let _ = self.select_model_shader(cx, &mut diffuse.draw_vars, variants::ModelLane::Diffuse);
            }
            return;
        }
        let pbr_lane = draw.is_pbr();
        // Shiny models draw matte until the PBR pipeline exists (Metal
        // compiles asynchronously), rather than vanishing. Decided in the
        // diffuse pass; the PBR pass that follows reads the same answer.
        if phase == LayerPhase::Opaque && !pbr_lane {
            if self.pbr_materials_enabled && self.pbr_draw.is_none() && self.has_pbr_copy(&instances, |_| true) {
                // Held VM (a script-driven draw is mid-apply): try again next
                // frame rather than drawing with no shader.
                self.pbr_draw = cx
                    .cx
                    .try_with_vm(|vm| Box::new(DrawScenePbr::script_new_with_default(vm)));
            }
            // The PBR lane's shader for this frame's features (variants.rs).
            if let Some(d) = self.pbr_draw.as_deref_mut() {
                d.skinned.draw_vars.refresh_after_reload(cx.cx);
            }
            let stock = self.pbr_draw.as_ref().and_then(|d| d.skinned.draw_vars.draw_shader_id);
            let shader = if stock.is_some() { self.lane_shaders(cx.cx, variants::ModelLane::Pbr, stock).0 } else { None };
            if let Some(d) = self.pbr_draw.as_deref_mut() {
                d.skinned.draw_vars.draw_shader_id = shader;
            }
            self.pbr_ready = shader.is_some_and(|id| cx.draw_shader_ready_here(id));
            // Likewise shiny stock models leave the PBR lane for the
            // engine's IBL program only once its variant for this frame's
            // features can draw (renderer/stock_ibl.rs).
            let pass_id = cx.current_pass_id();
            self.confirm_stock_ibl(cx.cx, pass_id);
        }
        let custom_name = match &draw {
            ModelDraw::Custom(name, _) => Some(*name),
            _ => None,
        };
        let traced_name = match &draw { ModelDraw::Traced(name, _) => Some(*name), _ => None };
        // Shiny stock models under an environment leave the PBR lane for
        // the engine's IBL program (renderer/stock_ibl.rs): decided in the
        // diffuse pass above, read here by every lane so exactly one of
        // them draws them.
        let stock_ibl_on = self.stock_ibl_active();
        let stock_ibl_lane = custom_name == Some(stock_ibl::STOCK_IBL_MATERIAL);
        // A program's lane draws its layers only once this frame's decision
        // made it ready (realm.rs `prepare_custom_lanes`).
        let custom_ready = matches!(&draw, ModelDraw::Custom(_, m) if m.lane_ready);
        let screen_ctl = self.bind_model_lane(cx, &mut draw, eye, fog, sun);
        // The lane draws through the shader variant for this frame's
        // features, and draws that cut no pixel through the one without
        // `clip` (variants.rs).
        let model_lane = match &draw {
            ModelDraw::Diffuse(_) => Some(variants::ModelLane::Diffuse),
            ModelDraw::Pbr(_) => Some(variants::ModelLane::Pbr),
            ModelDraw::Foliage(_) => Some(variants::ModelLane::Foliage),
            ModelDraw::Splat(_) => Some(variants::ModelLane::Splat),
            _ => None,
        };
        let opaque = match (&mut draw, model_lane) {
            (draw, Some(model_lane)) => self.select_model_shader(cx, &mut draw.base().draw_vars, model_lane)
                .filter(|id| cx.draw_shader_ready_here(*id)),
            // A Splash material's variants of its own program; the one
            // without `clip` only when no hook of it can cut a pixel
            // (custom_material.rs).
            (ModelDraw::Custom(_, m), None) => {
                let (full, opaque) = m.shaders(cx.cx, self.lane_features());
                m.draw.draw_vars.draw_shader_id = full;
                opaque.filter(|id| cx.draw_shader_ready_here(*id))
            }
            _ => None,
        };
        let lane_shader = draw.base().draw_vars.draw_shader_id;
        let (hoisted, hoisted_opaque) = match (screen_ctl, model_lane, lane_shader) {
            (Some(_), Some(lane), Some(shader)) => self.hoisted_lane_shaders(cx, lane, shader),
            _ => (None, None),
        };
        // The copies that read the GI screen cache draw through the hoisted
        // variant: terrain tiles in any lane, and every copy of the splat
        // lane (the level's surfaces).
        let cache_shaders = (hoisted.or(lane_shader), hoisted_opaque.or(opaque));
        // Swaying models (a layer with wind) draw in the foliage lane once
        // its shader is ready, and in no other.
        let foliage_lane = matches!(draw, ModelDraw::Foliage(_));
        let foliage_ready = self.foliage_ready && matches!(lane, WorldModelLane::Placed);
        // Terrain tiles with paint draw in the splat lane once its shader is
        // ready, and in no other; until then the PBR lane draws their main
        // layer (their base texture).
        let splat_lane = matches!(draw, ModelDraw::Splat(_));
        let splat_ready = self.splat_ready && matches!(lane, WorldModelLane::Placed);
        // A model's far stand-in draws through the impostor shader, which has
        // the foliage/PBR lanes' layout: the lane swaps only the shader id
        // for that layer. Without it (the diffuse lane, or before its
        // pipeline exists) the model draws its own layers at every distance:
        // the stand-in's quad is not a picture on its own. The frame builds
        // it as soon as a resident model has a stand-in (realm.rs
        // `prepare_impostor_lane`).
        let impostor_shader = if matches!(draw, ModelDraw::Foliage(_) | ModelDraw::Pbr(_)) { self.impostor_shader } else { None };
        let impostor_scale = self.impostor_scale;
        let terrain_blend_reach = self.terrain_blend_reach;
        let relief_steps = self.relief_steps;
        let ssr_floor = self.ssr.gloss_floor;
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
        let which = matches!(lane, WorldModelLane::Attachment) as usize;
        let anchor_all = if phase == LayerPhase::Opaque {
            let (mut anch_x, mut anch_z, mut anch_n) = (0.0f64, 0.0f64, 0u32);
            for (_, inst) in instances.live() {
                anch_x += inst.transform.v[12] as f64;
                anch_z += inst.transform.v[14] as f64;
                anch_n += 1;
            }
            let anchor = vec3f(
                (anch_x / anch_n.max(1) as f64) as f32,
                1.0,
                (anch_z / anch_n.max(1) as f64) as f32,
            );
            self.model_orders[which].anchor_all = anchor;
            anchor
        } else {
            self.model_orders[which].anchor_all
        };
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
        let key = instances.key();
        // The same list can be drawn with a different projection or detail bias.
        if phase == LayerPhase::Opaque && (matches!(draw, ModelDraw::Diffuse(_)) || self.model_orders[which].key != (key.0, key.1, lod_scale.to_bits())) {
            let mut built = std::mem::take(&mut self.model_orders[which]);
            let chains = &self.model_lod_chains;
            let occluders = self.stream.as_ref().map(|st| st.occluders());
            // Refreshed for this frame's placed list by draw_scene_inner.
            let blocks = matches!(lane, WorldModelLane::Placed).then_some(&self.placed_blocks);
            let culled = built.build(&self.static_models, &instances, eye, lod_scale, key, &|id| chains.contains_key(id), frustum, occluders, blocks);
            match lane {
                WorldModelLane::Placed => stats.model_culled += culled,
                WorldModelLane::Attachment => stats.world_attachment_culled += culled,
            }
            self.model_orders[which] = built;
        }
        let mut model_order = std::mem::take(&mut self.model_orders[which]);
        let ModelOrder { slots, distances, order, ao: model_ao, layered, pass_buf, .. } = &mut model_order;
        // Layer-major: every copy's first layer, then every copy's second.
        // A multi-layer model (a tree: trunk, then leaves) drawn copy by
        // copy alternates geometry, and the batcher only appends to the
        // previous item of the same geometry: thousands of scattered trees
        // were thousands of draws. Stats and rigid parts count on pass 0.
        let layer_passes = if phase == LayerPhase::Opaque { order.iter().filter_map(|(i, _, _)| slots[*i]).map(|at| {
            let root = &self.static_models[at].1;
            std::iter::once(root).chain(root.lods.iter().map(|(_, m)| m)).map(|m| (m.triangles > 0) as usize + m.extra_draws.len()).max().unwrap_or(1)
        }).max().unwrap_or(1).max(1) } else { 0 };
        // Pass 0 walks every entry; later passes only the drawn entries that
        // have that many layers (entry, layer count).
        // The models are read, not cloned, per copy: set aside for the walk
        // (nothing below reads them through `self`) and put back after it.
        let static_models = std::mem::take(&mut self.static_models);
        // The layer material whose uniforms the draw holds now. Consecutive
        // copies of one model layer (a forest, a row of props) share it, and
        // `set_material` writes a dozen uniforms and up to four textures by
        // name each time: on the flight island that was the hottest program
        // code of the frame. A copy that changed the material's uniforms
        // after it (an item's glow, a fur shell) forces the next write; the
        // per-copy lamp gain is written for every copy anyway.
        let mut held_material: *const LayerMaterial = std::ptr::null();
        let mut record_lane = None;
        let blended_lane = if phase == LayerPhase::Blended {
            self.blended_models.find(lane, &draw)
        } else {
            None
        };
        let mut blended = blended_lane.map(|at| std::mem::take(&mut self.blended_models.lanes[at].layers)).unwrap_or_default();
        // A sequence key preserves the recorded order at equal distances
        // while the in-place sort needs no temporary allocation.
        blended.sort_unstable_by_key(|layer| (std::cmp::Reverse(distances[layer.entry.0].to_bits()), layer.sequence));
        if phase == LayerPhase::Opaque {
            layered.clear();
        }
        let (mut layer_pass, mut entry_index) = (0, 0);
        let mut fur_budget = 0;
        let mut pending_parts = None;
        let mut last: Option<(usize, usize)> = None;
        // One body for the layer-major opaque walk and the distance-major
        // recorded walk. Advancing an opaque pass keeps the old entry order.
        loop {
            let (entry, layer) = match (pending_parts.as_mut().and_then(Iterator::next), phase) {
                (Some(part), _) => part,
                (None, LayerPhase::Opaque) => {
                    pending_parts = None;
                    let pass_order = if layer_pass == 0 { &order[..] } else { &pass_buf[..] };
                    if let Some(&entry) = pass_order.get(entry_index) {
                        entry_index += 1;
                        (entry, ModelLayer::Body(layer_pass))
                    } else {
                        layer_pass += 1;
                        if layer_pass >= layer_passes { break; }
                        pass_buf.clear();
                        pass_buf.extend(layered.iter().filter(|(_, n)| *n > layer_pass).map(|(e, _)| *e));
                        if pass_buf.is_empty() { break; }
                        entry_index = 0;
                        last = None;
                        continue;
                    }
                }
                (None, LayerPhase::Blended) => {
                    let Some(layer) = blended.get(entry_index) else { break };
                    entry_index += 1;
                    (layer.entry, layer.layer)
                }
            };
            let (i, lod_index, window) = entry;
            let count_copy = phase == LayerPhase::Opaque && layer == ModelLayer::Body(0);
            let Some(inst) = instances.get(i) else { continue };
            let dynamic = lane.is_dynamic(inst);
            let Some(at) = slots[i] else {
                continue;
            };
            let root = &static_models[at].1;
            let inv_height = 1.0 / root.max.y.max(0.5);
            let distance=distances[i];
            if phase == LayerPhase::Blended || matches!(layer, ModelLayer::Body(_)) {
                fur_budget = 12_000usize.min(96_000usize.saturating_sub(stats.fur_triangles));
            }
            let loaded=if lod_index==0{root}else{&root.lods[lod_index-1].1};
            if count_copy {
                let count = usize::from(loaded.triangles > 0) + loaded.extra_draws.len();
                if count > 1 {
                    layered.push((entry, count));
                }
                // Schedule the moving layers before any lane can reject the body.
                if !loaded.anim_parts.is_empty() || !loaded.driven_parts.is_empty() {
                    pending_parts = Some(loaded.part_layers().map(move |part| (entry, part)));
                }
            }
            let layer_draw = loaded.layer_draw(layer);
            let layer_material = layer_draw.map(|draw| draw.4);
            // A required layer has a single owner even if its program was never
            // installed. Test this before stock IBL, held shaders or custom lookup.
            let required = layer_material.and_then(|m| m.traced_required.as_ref());
            let owns_layer = match (required, traced_name) {
                (None, None) => true,
                (Some((program, TracedLayerRole::Outside)), Some(name)) => program.as_ref() == name,
                _ => false,
            };
            if !owns_layer { continue; }
            let effective = if traced_name.is_none() {
                inst.material_of(layer_material.map_or("", |m| m.name.as_ref()))
            } else {
                None
            };
            if traced_name.is_none() && phase == LayerPhase::Opaque {
                let sways = foliage_ready && std::iter::once(&root.material).chain(root.extra_draws.iter().map(|l| &l.4))
                    .any(|m| m.traced_required.is_none() && m.surface.as_ref().is_some_and(|s| s.definition.wind > 0.0));
                let paints = splat_ready && layer_material.is_some_and(|m| m.surface.as_ref().is_some_and(|s| s.splat.is_some()));
                // Lane filter. Both passes walk the same instance list — indices
                // address `lm_remaps` / `model_ground` / the light-cell key, so
                // they must not be renumbered — and each takes only the models
                // its shader owns.
                let uses_pbr_lane = self.pbr_materials_enabled && loaded.wants_pbr && self.pbr_ready;
                // The layer's own program takes it once this frame's decision
                // made that program ready, the stock lanes until then: one
                // lane, never both (a draw the HAL skips would stand in for
                // one it shows) and never none.
                let wanted_custom = effective.filter(|m| match custom_name {
                    Some(name) => custom_ready && name == m.name.as_str(),
                    None => self.custom_draws.get(&m.name).is_some_and(|d| d.lane_ready),
                });
                // A shiny stock model with no material of its own, while an
                // environment is registered: the stock IBL lane takes it and
                // the PBR lane leaves it (renderer/stock_ibl.rs). A material
                // it names is its own only while that one is installed: a
                // name that is not draws through the stock lanes anyway.
                let own_material = self.has_own_material(effective, custom_name);
                // A painted terrain tile stays with the splat lane (its layers).
                let takes_stock_ibl = !paints && stock_ibl::takes_stock_ibl(stock_ibl_on, uses_pbr_lane, own_material, sways);
                if let Some(name) = custom_name {
                    if stock_ibl_lane {
                        // Its own instances, plus any that name the program
                        // outright (an engine name, but never a vanished model).
                        if !(takes_stock_ibl || wanted_custom.map(|m| m.name.as_str()) == Some(name)) { continue; }
                    } else {
                        if wanted_custom.map(|m| m.name.as_str()) != Some(name) { continue; }
                    }
                } else if paints != splat_lane || if splat_lane {
                    // A painted terrain tile draws in the splat lane only.
                    wanted_custom.is_some() || takes_stock_ibl || sways
                } else {
                    stock_ibl::stock_lane_skips(wanted_custom.is_some(), takes_stock_ibl, sways, foliage_lane, uses_pbr_lane, pbr_lane)
                } {
                    continue;
                }
                // Drawn short of whole (`drew_fallback`): a shiny model on the
                // diffuse lane while the PBR pipeline compiles, or, on any stock
                // lane, a layer whose own material's pipeline is still being
                // made. A pipeline that failed draws stock for good: nothing to
                // wait for.
                if custom_name.is_none()
                    && ((!pbr_lane && self.pbr_materials_enabled && loaded.wants_pbr && !self.pbr_ready)
                        || (wanted_custom.is_none() && effective.is_some_and(|m| self.custom_draws.get(&m.name)
                            .and_then(|d| d.draw.draw_vars.draw_shader_id).is_some_and(|id| !cx.cx.draw_shader_failed(id)))))
                {
                    self.items.fallback = true;
                }
            }
            if !stock_ibl_lane {
                if let (ModelDraw::Custom(_, d), Some(material)) = (&mut draw, effective) {
                    d.draw.set_params(cx.cx, material.params);
                }
            }
            // Per draw item: the morph source belongs to this model LOD, the
            // same `loaded` that picks the geometry and binds `morph_map`.
            // The per-instance weights stay on the instance stream.
            draw.base().draw_vars.set_uniform(cx.cx,live_id!(morph_ctl),&[0.0,0.0,0.0,0.0]);
            // The host's element lookup, bound by name in whatever shader (lane, variant) this item draws
            // through: a slot-index bind into a shader whose slot 6 is another map reads garbage.
            if let Some((map, ctl)) = &self.element_lookup {
                if let Some(shader) = draw.base().draw_vars.draw_shader_id {
                    if let Some(slot) = cx.draw_shaders[shader.index].mapping.textures.iter().position(|t| t.id == live_id!(elem_map)) {
                        draw.base().draw_vars.set_texture(slot, map);
                        draw.base().draw_vars.set_uniform(cx.cx, live_id!(elem_ctl), ctl);
                    }
                }
            }
            draw.base().draw_vars.set_uniform(cx.cx, live_id!(host_clip), &self.clip_plane.unwrap_or([0.0; 4]));
            let host_box = self.host_box.unwrap_or([[0.0; 4]; 3]);
            draw.base().draw_vars.set_uniform(cx.cx, live_id!(host_box0), &host_box[0]);
            draw.base().draw_vars.set_uniform(cx.cx, live_id!(host_box1), &host_box[1]);
            draw.base().draw_vars.set_uniform(cx.cx, live_id!(host_box2), &host_box[2]);
            if let Some(morph)=&loaded.morph{
                let target=match lane{WorldModelLane::Placed=>ModelTarget::Instance(i),WorldModelLane::Attachment=>ModelTarget::Attachment(i)};
                let weights=self.model_anim_state.morph_weights(&target,&inst.model,&morph.source);
                draw.base().draw_vars.set_uniform(cx.cx,live_id!(morph_ctl),&[morph.source.width as f32,morph.source.height as f32,morph.source.vertices as f32,morph.source.targets as f32]);
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
            if let Some(frustum) = frustum.filter(|_| phase == LayerPhase::Opaque && inst.has_custom()) {
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
                    if count_copy {
                        match lane {
                            WorldModelLane::Placed => stats.model_culled += 1,
                            WorldModelLane::Attachment => stats.world_attachment_culled += 1,
                        }
                    }
                    continue;
                }
            }
            let prelit = loaded.prelit;
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
            if let Some(t) = ao_tex.filter(|_| lod_index == 0 && traced_name.is_none()) {
                draw.base().draw_vars.set_texture(1, t);
                draw.base().draw_vars.set_uniform(cx.cx, live_id!(ao_enabled), &[1.0]);
                stats.ao_bound += count_copy as u64;
            } else {
                draw.base().draw_vars.set_uniform(cx.cx, live_id!(ao_enabled), &[0.0]);
                stats.ao_missing += count_copy as u64;
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
            let terrain = inst.model.starts_with("gen/surface/level-terrain");
            // A terrain tile's own distance: its vertices are in world
            // space under an identity transform, so the copy's origin
            // says nothing; the nearest point of its bounding sphere does.
            let tile_distance = if terrain {
                let (center, radius) = variants::bounding_sphere(root.min, root.max, &inst.transform);
                ((center - eye).length() - radius).max(0.0)
            } else { distance };
            let ground = terrain || inst.model.starts_with("gen/surface/level-road");
            let screen_clip = adjust.w > 0.5
                || !ground && occluder_focus.is_some_and(|focus| {
                    let (center, radius) = variants::bounding_sphere(root.min, root.max, &inst.transform);
                    variants::in_occluder_fade(eye, focus, center, radius)
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
            if count_copy && last.is_none_or(|(slot, level)| slot != at || level != lod_index) {
                match lane {
                    WorldModelLane::Placed => stats.model_draws += 1,
                    WorldModelLane::Attachment => stats.world_attachment_draws += 1,
                }
                last = Some((at, lod_index));
            }
            if count_copy { match lane {
                WorldModelLane::Placed => {
                    stats.model_instances += 1;
                    stats.model_triangles += tri_count;
                }
                WorldModelLane::Attachment => {
                    stats.world_attachment_instances += 1;
                    stats.world_attachment_triangles += tri_count;
                }
            } }
            let Some((geometry_id, texture, detail, dscale, material)) = layer_draw else { continue };
            let body_layer = match layer {
                ModelLayer::Animated(part, _) => {
                    let target = match lane {
                        WorldModelLane::Placed => ModelTarget::Instance(i),
                        WorldModelLane::Attachment => ModelTarget::Attachment(i),
                    };
                    let pose = self.model_anim_state.transform(&target, &inst.model, &loaded.anim_parts[part].def);
                    draw.base().transform = Mat4f::mul(&inst.transform, &pose);
                    None
                }
                ModelLayer::Driven(part, _) => {
                    let def = &loaded.driven_parts[part].def;
                    let pose = inst.part_poses.iter().find(|pose| pose.connection == def.connection)
                        .map(|pose| pose.transform).unwrap_or_else(|| def.rest_transform());
                    draw.base().transform = Mat4f::mul(&inst.transform, &pose);
                    None
                }
                ModelLayer::Body(index) => Some(index),
            };
            if body_layer.is_none() {
                // Moving layers have no chart in the static light atlas.
                draw.base().lm_rect = Vec4f::default();
                last = None;
                if phase == LayerPhase::Opaque {
                    let triangles = cx.cx.geometries[geometry_id].index_count / 3;
                    match lane {
                        WorldModelLane::Placed => stats.model_triangles += triangles,
                        WorldModelLane::Attachment => stats.world_attachment_triangles += triangles,
                    }
                }
            }
            // Only the fixed geometry crossfades to the model's far stand-in.
            let impostor_at = impostor_shader.filter(|_| body_layer.is_some()).map_or(0.0, |_| {
                std::iter::once(&loaded.material).chain(loaded.extra_draws.iter().map(|l| &l.4))
                    .filter(|m| m.traced_required.is_none()).filter_map(|m| m.surface.as_ref().map(|s| s.definition.impostor)).fold(0.0f32, f32::max) * impostor_scale
            });
            'submit: {
                let stand_in = traced_name.is_none() && body_layer.is_some() && material.surface.as_ref().is_some_and(|s| s.definition.impostor > 0.0);
                let mut layer_window = 0.0f32;
                if impostor_at > 0.0 {
                    match chain_fade_windows(impostor_at, distance).filter(|_| window <= 0.0) {
                        Some((near, next)) => layer_window = if stand_in { next } else { near },
                        None => if stand_in != (distance >= impostor_at) { break 'submit; },
                    }
                } else if stand_in {
                    break 'submit;
                }
                draw.base().color_adjust_ctl.w = if layer_window > 0.0 { layer_window } else { adjust.w };
                // A splat-lane surface reads the cache like a terrain tile: a
                // gather per pixel cost a view filled with level surfaces
                // 2-3 ms at the bench Mac's p95.
                let cached = terrain || (splat_lane && !stand_in);
                let (shader, item_opaque) = if stand_in {
                    (impostor_shader, None)
                } else if cached {
                    cache_shaders
                } else {
                    (lane_shader, opaque)
                };
                draw.base().draw_vars.draw_shader_id = shader;
                draw.base().draw_vars.geometry_id = Some(geometry_id);
                // A static copy's own layers take no shadow from its stand-in,
                // which casts for it past the first cascade (shaders/csm.rs
                // `csm_own_limit`); a mover casts its own layers everywhere.
                // The stand-in itself never takes its own shadow either, so
                // mesh and stand-in agree where one takes over from the other
                // (shaders/impostor.rs).
                let own = match root.stand_in { Some(c) if body_layer.is_some() && (stand_in || !dynamic) => [c.x, c.y, c.z, 1.0], _ => [0.0; 4] };
                draw.base().draw_vars.set_uniform(cx.cx, live_id!(csm_own), &own);
                // Terrain tiles and splat-lane surfaces read the GI screen
                // cache; every other copy gathers.
                if let Some(ctl) = screen_ctl {
                    draw.base().draw_vars.set_uniform(cx.cx, live_id!(gi_screen_ctl), &[if cached { ctl[0] } else { 0.0 }, ctl[1], ctl[2], ctl[3]]);
                }
                // The reflection capture finishes only what the reflection passes read (shaders/pbr.rs).
                draw.base().draw_vars.set_uniform(cx.cx, live_id!(ssr_pass_floor), &[ssr_floor]);
                if traced_name.is_none() {
                    draw.base().draw_vars.set_texture(0, texture);
                    draw.base().draw_vars.set_texture(5, detail);
                }
                // A painted tile's slot 0 is its layers' albedo array; a
                // far one paints its heaviest layer only, beyond the
                // tier's reach.
                if let Some(p) = material.surface.as_ref().and_then(|s| s.splat.as_ref()).filter(|_| splat_lane) {
                    draw.base().draw_vars.set_texture(0, &p.albedo);
                    draw.base().draw_vars.set_uniform(cx.cx, live_id!(splat_blend), &[if tile_distance > terrain_blend_reach { 0.0 } else { 1.0 }]);
                    draw.base().draw_vars.set_uniform(cx.cx, live_id!(splat_relief_steps), &[relief_steps]);
                    draw.base().draw_vars.set_uniform(cx.cx, live_id!(splat_ssr_floor), &[ssr_floor]);
                }
                draw.base().draw_vars.set_uniform(cx.cx, live_id!(detail_st), &[dscale[0], dscale[1]]);
                // An IBL material reads its environment on the detail slot.
                if let ModelDraw::Custom(_, m) = &mut draw { if let Some(t) = m.texture.clone() { m.draw.draw_vars.set_texture(5, &t); } else if let (true, Some(t)) = (m.ibl, self.ibl_texture()) { m.draw.draw_vars.set_texture(5, t); } }
                draw.base().prelit = if prelit { 1.0 } else { 0.0 };
                if !std::ptr::eq(held_material, material) || matches!(draw, ModelDraw::Custom(..)) {
                    draw.set_material(cx.cx, material);
                    held_material = material;
                    if splat_lane { self.bind_grass_cover(cx.cx, &mut draw); }
                }
                if traced_name.is_some() {
                    // set_material resets the raster options. This lane keeps
                    // its own draw struct, so a mesh sibling keeps its options.
                    let base = draw.base();
                    base.draw_vars.options.backface_culling = false;
                    base.draw_vars.options.depth_write = true;
                    base.draw_vars.options.alpha_blend = false;
                    base.lm_rect = Vec4f::default();
                    base.draw_vars.set_uniform(cx.cx, live_id!(ao_enabled), &[0.0]);
                    base.draw_vars.set_uniform(cx.cx, live_id!(surface_on), &[1.0]);
                    base.draw_vars.set_uniform(cx.cx, live_id!(alpha_mode), &[0.0]);
                    base.draw_vars.set_uniform(cx.cx, live_id!(fur), &[0.0; 4]);
                }
                draw.light_lamp(cx.cx, material, inst.lamps, self.lamp_level);
                if phase == LayerPhase::Opaque && draw.base().draw_vars.options.alpha_blend {
                    self.blended_models.record(&mut record_lane, lane, &draw, entry, layer);
                    break 'submit;
                }
                // A world item's copy may glow on its own (items.rs).
                if matches!(lane, WorldModelLane::Placed) {
                    if let Some(e) = i.checked_sub(instances.len().saturating_sub(self.items.appended)).and_then(|k| self.items.glow.get(k)).filter(|e| e.x + e.y + e.z > 0.0) {
                        draw.add_emission(cx.cx, *e);
                        held_material = std::ptr::null();
                    }
                }
                if material.surface.as_ref().is_some_and(|s| s.definition.fur.is_some()) {
                    held_material = std::ptr::null();
                }
                if let Some(s) = material.surface.as_ref().filter(|s| s.definition.wind > 0.0) {
                    // Sway in metres at the top at wind 1 in a 6 m/s wind;
                    // leaves (cut-out layers) flutter on top of the sway.
                    let d = &s.definition;
                    let flutter = if d.alpha_mode == 1 { d.wind * 0.035 } else { 0.0 };
                    // A morphing model never sways (the lanes are shared).
                    let morph_ctl = draw.base().uniform_value(cx.cx, live_id!(morph_ctl));
                    if morph_ctl[3] < 0.5 {
                        draw.base().draw_vars.set_uniform(cx.cx, live_id!(morph_ctl), &[morph_ctl[0], morph_ctl[1], morph_ctl[2], -1.0]);
                        let (state, scroll) = (self.wind_clock.state(), self.wind_clock.scroll());
                        draw.base().morph_weights0 = vec4(d.wind * 0.35, inv_height, 0.0, flutter);
                        draw.base().morph_weights1 = vec4(state[0], state[1], state[2], state[3]);
                        draw.base().morph_weights2 = vec4(scroll[0], scroll[1], scroll[2], scroll[3]);
                    }
                }
                let cut = screen_clip || stand_in || layer_window > 0.0 || self.clip_plane.is_some() || self.host_box.is_some() || match &material.surface {
                    // Masked layers discard; deferred blended layers keep
                    // the stock pipeline (the no-discard variant is opaque).
                    Some(surface) if pbr_lane => surface.definition.alpha_mode != 0,
                    _ => material.cutout,
                };
                let owner = if cx.draw_owners_armed() {
                    let label = self.draw_names.model(cx.cx, &inst.model, lod_index, body_layer.unwrap_or(0), stand_in);
                    cx.enter_draw_label(label)
                } else {
                    0
                };
                stats.fur_triangles += if body_layer.is_some() {
                    draw.submit_as(cx, distance, &mut fur_budget, item_opaque.filter(|_| !cut))
                } else {
                    draw.submit(cx, distance, &mut fur_budget)
                };
                cx.exit_draw_label(owner);
            }
            draw.base().draw_vars.draw_shader_id = lane_shader;
            draw.base().color_adjust_ctl.w = adjust.w;
            if root.stand_in.is_some() {
                draw.base().draw_vars.set_uniform(cx.cx, live_id!(csm_own), &[0.0; 4]);
            }
        }
        if let Some(at) = blended_lane {
            self.blended_models.lanes[at].layers = blended;
        }
        drop(pending_parts);
        self.static_models = static_models;
        self.model_orders[which] = model_order;
    }

    /// Everything a world-model lane binds ONCE before its instances: the
    /// cluster and GI tables, sun and fog, shading space, the specular eye,
    /// screen-space AO, the lightmap atlas, the cascades and the ground
    /// light field. Shared by the placed/attachment lanes and the streamed
    /// world lane (stream_draw.rs), so they can never shade differently.
    /// Returns the GI screen cache's switch for the lane's terrain tiles and
    /// splat-lane surfaces
    /// (`FastGi::bind_screen_cache`).
    pub(super) fn bind_model_lane(
        &mut self,
        cx: &mut Cx3d,
        draw: &mut ModelDraw<'_>,
        eye: Vec3f,
        fog: (Vec3f, f32),
        sun: &SunLight,
    ) -> Option<[f32; 4]> {
        let pbr_lane = draw.is_pbr();
        // Mesh lanes share a fixed layout. A traced pass owns its binding
        // layout; resolve the lighting slots once for this list of instances.
        let [light_slot, top_slot, csm_slot, ssao_slot] = if matches!(draw, ModelDraw::Traced(..)) {
            let vars = &mut draw.base().draw_vars;
            let depth_map = match cx.gpu_backend() {
                GpuBackend::OpenGl | GpuBackend::WebGl => [0.5, 0.5],
                _ => [1.0, 0.0],
            };
            vars.set_uniform(cx.cx, live_id!(trace_depth_map), &depth_map);
            let shader = vars.draw_shader_id.expect("ready traced surface");
            let textures = &cx.draw_shaders[shader.index].mapping.textures;
            [live_id!(light_map), live_id!(top_map), live_id!(csm_map), live_id!(ssao_map)]
                .map(|name| textures.iter().position(|t| t.id == name))
        } else {
            [Some(2), Some(3), Some(4), Some(7)]
        };
        self.clustered.bind(cx.cx, &mut draw.base().draw_vars, self.clustered_enabled);
        self.gi.bind(cx.cx, &mut draw.base().draw_vars);
        let screen_ctl = self.gi.bind_screen_cache(cx.cx, &mut draw.base().draw_vars, self.gi.last_scene_origin);
        self.bind_output_dither(cx.cx, &mut draw.base().draw_vars);
        // An IBL program's control of its two lookups: the lane's scale,
        // and for the stock IBL program whether the fill comes from the
        // map (renderer/stock_ibl.rs).
        if let ModelDraw::Custom(name, m) = draw {
            if m.ibl {
                self.bind_ibl_lane(cx.cx, name, &mut m.draw.draw_vars);
            }
        }
        // Sun, fog and the debug switches, once for the whole lane. They are
        // uniforms rather than instance lanes -- the vertex stage ran out of
        // D3D11 input registers -- which is also what they describe: one
        // frame has one sun and one fog, whatever draws under them.
        {
            let lm_debug = self.lm_debug;
            let vars = &mut draw.base().draw_vars;
            sun.write_uniforms(cx.cx, vars);
            vars.set_uniform(cx.cx, live_id!(light_dir), &[sun.dir.x, sun.dir.y, sun.dir.z]);
            vars.set_uniform(cx.cx, live_id!(fog_color), &[fog.0.x, fog.0.y, fog.0.z]);
            vars.set_uniform(cx.cx, live_id!(fog_density), &[fog.1]);
            vars.set_uniform(cx.cx, live_id!(depth_clip), &[1.0]);
            vars.set_uniform(cx.cx, live_id!(lm_debug), &[lm_debug]);
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
        // sampled. The visibility remains in ssao_map.x for both lanes.
        {
            let ssao = self.ssao.clone();
            let vars = &mut draw.base().draw_vars;
            match &ssao {
                Some((tex, strength)) => {
                    if let Some(slot) = ssao_slot { vars.set_texture(slot, tex); }
                    vars.set_uniform(cx.cx, live_id!(ssao_ctl), &[*strength, 0.0, 0.0, 0.0]);
                }
                None => {
                    // Nor bound (by name: the slot differs between programs): no AO texture of an
                    // earlier frame, or of the scene's draw for an aux one, stays the lane's input.
                    super::frame::unbind_texture_named(cx.cx, vars, live_id!(ssao_map));
                    vars.set_uniform(cx.cx, live_id!(ssao_ctl), &[0.0, 0.0, 0.0, 0.0]);
                }
            }
        }
        // One atlas for the whole static scene, so binding it here costs
        // nothing per instance and never breaks batching.
        let lm_tex = self.lightmap_texture(cx.cx);
        if let Some(slot) = light_slot { draw.base().draw_vars.set_texture(slot, &lm_tex); }
        if let Some(slot) = csm_slot {
            let csm = self.gpu_baker.csm_binding();
            Self::write_csm_uniforms(cx.cx, &mut draw.base().draw_vars, &csm, &lm_tex, slot, self.shadow_debug);
        }
        // The ground region, for DYNAMIC instances' baked sun shadow (a
        // driven car crossing a house's shadow darkens). A channel only —
        // their lamp light is analytic. The shadow-top plane rides along so
        // a dynamic lifted above the blocker rejects the ground's shadow.
        {
            let (top_tex, top_base, top_range) = self.lm_top_binding(cx.cx);
            if let Some(slot) = top_slot { draw.base().draw_vars.set_texture(slot, &top_tex); }
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
        screen_ctl
    }

    pub(super) fn has_pbr_copy(&self, instances: &(impl ModelList + ?Sized), eligible: impl Fn(&ModelInstance) -> bool) -> bool {
        let shiny: std::collections::HashSet<&str> = self.static_models.iter()
            .filter(|(_, m)| m.wants_pbr).map(|(k, _)| k.as_str()).collect();
        !shiny.is_empty() && instances.view().live().any(|(_, inst)| shiny.contains(inst.model.as_str()) && eligible(inst))
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
        lod_scale: f32,
        instances: &(impl ModelList + ?Sized),
        lane: WorldModelLane,
        fog: (Vec3f, f32),
        sun: &SunLight,
        frustum: Option<&Frustum>,
        stats: &mut RenderStats,
    ) {
        let instances = instances.view();
        if !self.pbr_materials_enabled {
            return;
        }
        // The diffuse lane's order for this list (built this frame) holds
        // every copy's model slot: no hashing of every model name.
        let which = matches!(lane, WorldModelLane::Attachment) as usize;
        let any = {
            let order = &self.model_orders[which];
            if order.key == (instances.key().0, instances.len(), lod_scale.to_bits()) {
                order.slots.iter().any(|slot| slot.is_some_and(|at| self.static_models[at].1.wants_pbr))
            } else {
                self.has_pbr_copy(&instances, |_| true)
            }
        };
        if !any {
            return;
        }
        if !self.pbr_ready {
            // The diffuse pass drew them this frame.
            return;
        }
        let Some(mut draw) = self.pbr_draw.take() else {
            return;
        };
        // Bound by name: a fixed slot index lands on another map when the shader's texture list
        // changes (slot 6 is `ssao_map`: the lookup shaded the PBR lane and its rows moved parts).
        let elem_slot = draw.skinned.draw_vars.draw_shader_id.and_then(|shader| cx.draw_shaders[shader.index].mapping.textures.iter().position(|t| t.id == live_id!(elem_map)));
        match (&self.element_lookup, elem_slot) {
            (Some((map, ctl)), Some(slot)) => {
                draw.skinned.draw_vars.set_texture(slot, map);
                draw.skinned.draw_vars.set_uniform(cx.cx, live_id!(elem_ctl), ctl);
            }
            _ => draw.skinned.draw_vars.set_uniform(cx.cx, live_id!(elem_ctl), &[0.0, 0.0, 0.0, 0.0]),
        }
        self.draw_models_inner(
            cx,
            ModelDraw::Pbr(&mut draw),
            LayerPhase::Opaque,
            eye,
            lod_scale,
            &instances,
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
                .live()
                .any(|(_, inst)| self.model_sky(&inst.model).is_some())
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
        for (_, inst) in instances.live() {
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
                        data.as_ref().map_or(0, |data| data.len()),
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
                let new_area = cx.add_instance(&mut draw.draw_vars);
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

    fn draw_view_model_layer(
        cx: &mut Cx3d, draw: &mut DrawSceneViewModel, geometry_id: GeometryId,
        texture: &Texture, material: &LayerMaterial,
    ) {
        if material.traced_required.is_some() { return }
        draw.draw_vars.geometry_id = Some(geometry_id);
        draw.draw_vars.set_texture(0, texture);
        Self::bind_view_model_material(cx, draw, material);
        if draw.draw_vars.can_instance() {
            let new_area = cx.add_instance(&mut draw.draw_vars);
            draw.draw_vars.area = cx.update_area_refs(draw.draw_vars.area, new_area);
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
        // SSR reconstructs position from depth. The view model squeezes depth
        // into a near band, so a receiver here would trace from the camera.
        draw.draw_vars.refresh_after_reload(cx.cx);
        let stock = draw.draw_vars.draw_shader_id;
        draw.draw_vars.draw_shader_id = self.no_receiver_shader(cx, stock);
        sun.write_into(
            &mut draw.light_dir,
            &mut draw.sun_color,
            &mut draw.sun_sky,
            &mut draw.sun_ground,
        );
        draw.lin = self.lin_ctl()[0];
        draw.sun_vis = self.view_model_sun;
        draw.fog_color = fog_color;
        let stage_inv = self.draw_stage_matrix().invert();
        for inst in &self.view_models {
            let Some((_, loaded)) = self.static_models.iter().find(|(id, _)| *id == inst.model)
            else {
                continue;
            };
            let layer_draws = std::iter::once((loaded.geometry.geometry_id(), &loaded.texture, &loaded.material))
                .chain(loaded.extra_draws.iter().map(|(g, t, _, _, m)| (g.geometry_id(), t, m)));
            // The scene draw-list carries Stage for every world draw. Cancel
            // it here: a held model is attached to the physical camera, not
            // scaled into an MR diorama or moved onto its floor anchor.
            draw.transform = Mat4f::mul(&stage_inv, &inst.transform);
            for (geometry_id, texture, material) in layer_draws {
                Self::draw_view_model_layer(cx, draw, geometry_id, texture, material);
            }
            // The view pass has no animation target, so use the authored rest pose.
            for part in &loaded.anim_parts {
                draw.transform = Mat4f::mul(&stage_inv, &Mat4f::mul(&inst.transform, &part.def.rest_transform()));
                for (geometry, texture, _, _, material) in &part.draws {
                    let id = geometry.geometry_id();
                    Self::draw_view_model_layer(cx, draw, id, texture, material);
                    stats.view_model_triangles += cx.cx.geometries[id].index_count / 3;
                }
            }
            // Driven geometry lives outside the body layers, even at rest.
            for part in &loaded.driven_parts {
                let pose = inst.part_poses.iter().find(|pose| pose.connection == part.def.connection)
                    .map(|pose| pose.transform).unwrap_or_else(|| part.def.rest_transform());
                draw.transform = Mat4f::mul(&stage_inv, &Mat4f::mul(&inst.transform, &pose));
                for (geometry, texture, _, _, material) in &part.draws {
                    let id = geometry.geometry_id();
                    Self::draw_view_model_layer(cx, draw, id, texture, material);
                    stats.view_model_triangles += cx.cx.geometries[id].index_count / 3;
                }
            }
            stats.view_model_instances += 1;
            stats.view_model_triangles += loaded.triangles;
        }
        draw.draw_vars.draw_shader_id = stock;
    }
}

#[cfg(test)]
mod chain_fade_tests {
    use super::*;

    #[test]
    fn lod_bias_switches_model_and_caster_at_the_same_camera() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let model = StaticModel::parse_glb(&crate::asset_morph::tests::fixture(false, false)).unwrap();
        let base = PreparedStaticPreview::prepare(model).unwrap();
        let prepared = base.clone().with_lods(vec![(30.0, base)]).unwrap();
        let mut renderer = Renderer::default();
        assert_eq!(renderer.lod_bias(), 1.0);
        let uploaded = renderer.upload_static_preview(&mut cx, prepared);
        renderer.install_uploaded_static("lod", uploaded);
        let mut transform = Mat4f::identity();
        transform.v[14] = 30.0;
        renderer.set_models(vec![ModelInstance {
            model: "lod".into(), transform, tint: vec4(1.0, 1.0, 1.0, 1.0),
            color_adjust: vec4(0.0, 1.0, 1.0, 0.0), dynamic: true, depth_order: 0.0,
            part_poses: Vec::new(), custom_material: None, layer_materials: Vec::new(), lamps: Default::default(),
        }]);
        let eye = vec3f(0.0, 0.0, 0.0);
        let view_scale = crate::lod_view::lod_projection_scale(
            &Mat4f::perspective(62.0, 1.5, 0.1, 500.0), crate::lod_view::LOD_REFERENCE_HEIGHT_PX);
        let key = renderer.placed_models.view().key();
        let mut order = ModelOrder::default();
        for (bias, expected) in [(0.995, 0), (1.005, 1), (0.995, 0)] {
            renderer.set_lod_bias(bias);
            let lod_scale = view_scale * renderer.lod_bias();
            assert_ne!(order.key, (key.0, key.1, lod_scale.to_bits()), "bias invalidates the cached selection");
            order.build(&renderer.static_models, &renderer.placed_models, eye, lod_scale,
                key, &|_| false, None, None, None);
            assert_eq!(order.lods, [expected], "LOD at bias {bias}");
            assert_eq!(order.order, [(0, expected, 0.0)], "automatic LOD switches without a fade");
            assert_eq!(order.distances, [30.0], "the camera and world distance stay fixed");
            let root = &renderer.static_models[0].1;
            let selected = if expected == 0 { root } else { &root.lods[expected - 1].1 };
            let mut casters = Vec::new();
            renderer.collect_lm_movers(&World::default(), eye, lod_scale, None, &mut Default::default(), &mut casters);
            assert_eq!(casters.len(), 1);
            assert_eq!(casters[0].geometry, selected.geometry.geometry_id(), "caster follows bias {bias}");
        }
        for bias in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            renderer.set_lod_bias(bias);
            assert_eq!(renderer.lod_bias(), 1.0, "invalid bias {bias} resets to the screen-error rule");
        }
    }

    #[test]
    fn a_narrower_view_keeps_the_model_and_caster_at_the_higher_lod() {
        use crate::lod_view::{lod_projection_scale, LOD_REFERENCE_HEIGHT_PX};
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let model = StaticModel::parse_glb(&crate::asset_morph::tests::fixture(false, false)).unwrap();
        let base = PreparedStaticPreview::prepare(model).unwrap();
        let prepared = base.clone().with_lods(vec![(30.0, base)]).unwrap();
        let mut renderer = Renderer::default();
        let uploaded = renderer.upload_static_preview(&mut cx, prepared);
        renderer.install_uploaded_static("lod", uploaded);
        let mut transform = Mat4f::identity();
        transform.v[14] = 31.0;
        renderer.set_models(vec![ModelInstance {
            model: "lod".into(), transform, tint: vec4(1.0, 1.0, 1.0, 1.0),
            color_adjust: vec4(0.0, 1.0, 1.0, 0.0), dynamic: true, depth_order: 0.0,
            part_poses: Vec::new(), custom_material: None, layer_materials: Vec::new(), lamps: Default::default(),
        }]);
        let scale = |fov| lod_projection_scale(&Mat4f::perspective(fov, 1.5, 0.1, 500.0), LOD_REFERENCE_HEIGHT_PX);
        let eye = vec3f(0.0, 0.0, 0.0);
        let mut order = ModelOrder::default();
        // Reuse the same list and cache key across views, as split panes do.
        for (fov, expected) in [(62.0, 1), (55.0, 0), (20.0, 0), (62.0, 1)] {
            let lod_scale = scale(fov);
            order.build(&renderer.static_models, &renderer.placed_models, eye, lod_scale,
                (0, 1), &|_| true, None, None, None);
            assert_eq!(order.lods, [expected], "LOD at FOV {fov}");
            assert_eq!(order.order, [(0, expected, 0.0)], "no premature crossfade at FOV {fov}");
            assert_eq!(order.distances, [31.0], "sorting and other distance consumers keep world units");
            let root = &renderer.static_models[0].1;
            let selected = if expected == 0 { root } else { &root.lods[expected - 1].1 };
            let mut casters = Vec::new();
            renderer.collect_lm_movers(&World::default(), eye, lod_scale, None, &mut Default::default(), &mut casters);
            assert_eq!(casters.len(), 1);
            assert_eq!(casters[0].geometry, selected.geometry.geometry_id(), "caster follows view at FOV {fov}");
        }
        // The dissolve band moves with the threshold, too.
        renderer.rebuild_models(|copies| copies[0].transform.v[14] = 28.5 / scale(55.0));
        order.build(&renderer.static_models, &renderer.placed_models, eye, scale(55.0),
            (0, 1), &|_| true, None, None, None);
        assert_eq!(order.order.len(), 2, "both levels draw in the scaled crossfade band");
        assert_eq!(order.order.iter().map(|entry| entry.1).collect::<Vec<_>>(), [0, 1]);
    }

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
