//! The world scene pass, moved verbatim from gamemaker's game_view.rs:
//! sky dome → terrain mesh → opaque per-shape batches → alpha per-shape
//! batches. Statics come from packed instance slabs cached against
//! `world.render_rev`; dynamics re-pack every frame.

use makepad_draw::*;
use makepad_game_sim::{entity_index_sorted, BodyKind, Entity, GameWorld, Shape, Terrain};

use crate::bake::{BakeSettings, BakeStats, LightBake};
use crate::geometry::shape_geometry_data;
use crate::shaders::{
    DrawGameAlpha, DrawGameCube, DrawGameShadow, DrawGameSkinned, DrawGameSky, DrawGameTerrain,
};
use crate::shadow_mesh::{caster_points, skinned_proxy_points, Receiver, ShadowMeshBuilder};
use crate::model::StaticModel;
use crate::stage::Stage;
use crate::particles::ParticleInstance;
use crate::sun::GameSun;

/// The host widget's themed draw structs, lent to the renderer per frame.
/// They stay `#[live]` fields on the widget so script-side styling applies.
pub struct GameDraws<'a> {
    pub cube: &'a mut DrawGameCube,
    pub alpha: &'a mut DrawGameAlpha,
    pub sky: &'a mut DrawGameSky,
    pub terrain: &'a mut DrawGameTerrain,
    /// Silhouette shadow mesh (shadow_mesh.rs). Optional so a host that has
    /// not adopted it yet keeps the blob tier.
    pub shadow: Option<&'a mut DrawGameShadow>,
}

/// Per-frame render counters, handed back for the host's profiler.
#[derive(Default, Clone, Copy)]
pub struct RenderStats {
    pub slab_rebuilds: u64,
    pub slab_us: u64,
    pub static_instances: u64,
    pub dyn_instances: u64,
    /// Sky dome drawn this frame (suppressed on an MR stage — the room is
    /// the environment). Tests assert the suppression.
    pub sky_drawn: bool,
    /// Terrain mesh drawn this frame (suppressed on an MR stage).
    pub terrain_drawn: bool,
    /// Shadow-catcher quad drawn under the diorama (MR only).
    pub shadow_catcher_drawn: bool,
    /// Cast shadows drawn this frame (both tiers).
    pub shadows: u64,
    /// How many of those were full projected silhouettes rather than blobs.
    pub projected_shadows: u64,
    /// Device-local particles drawn this frame.
    pub particles: u64,
    /// Stock props placed this frame, and how many draw items they cost.
    /// `model_draws` < `model_instances` is the batching working: copies of
    /// one prop share a draw item.
    pub model_instances: u64,
    pub model_draws: u64,
    pub model_triangles: usize,
    /// What the CPU light bake cost (bake.rs). Zero on frames it skipped.
    pub bake: BakeStats,
    /// Floats per cube instance, read from the compiled shader rather than
    /// counted by hand — this is the number the bandwidth budget is about.
    pub instance_floats: u32,
}

/// GPU-side caches for one view family: unit shape geometries, the packed
/// static instance slabs, and the terrain mesh. Owns no draw structs — see
/// [`GameDraws`].
pub struct GameRenderer {
    // PERF: unit shape geometries, built once (index = Shape::index()).
    shape_geometries: [Option<Geometry>; 5],
    // PERF: packed static instance data per shape (opaque / alpha passes),
    // valid while slab_rev == world.render_rev.
    static_slab: [Vec<f32>; 5],
    static_slab_alpha: [Vec<f32>; 5],
    /// `(world.render_rev, bake.generation())` — the slabs carry baked light
    /// in their colours, so a rebake invalidates them exactly like a world
    /// edit does.
    slab_key: Option<(u64, u64)>,
    slab_instance_count: u64,
    /// GPU mesh for the smooth terrain, rebuilt when the revision changes.
    terrain_geometry: Option<Geometry>,
    terrain_revision: u64,
    /// GPU meshes for CPU-skinned characters, keyed by caller id, re-uploaded
    /// every frame (the skinning happens CPU-side; see skin.rs).
    skinned_geometries: Vec<(u64, Geometry)>,
    /// Static stock props, uploaded ONCE and keyed by asset id — the opposite
    /// of `skinned_geometries` above, which re-uploads per frame because CPU
    /// skinning changes the vertices. A prop's vertices never change, so its
    /// per-frame cost is one instance. See model.rs.
    static_models: Vec<(String, LoadedModel)>,
    /// Stock props placed for this frame, set by the host before drawing.
    placed_models: Vec<ModelInstance>,
    /// How this device projects the world (flat / VR 1:1 / MR diorama).
    /// Applied as the scene draw list's view transform, so it costs one
    /// uniform and never invalidates the static slabs. See stage.rs.
    stage: Stage,
    /// How many casters get a projected silhouette before the rest fall
    /// back to blobs. A per-device dial: a Quest can afford fewer than a PC
    /// even though both cost one instance, because the projection math runs
    /// per caster per frame on the CPU.
    shadow_budget: usize,
    /// Device-local particles for this frame (particles.rs). Set by the
    /// host before drawing; never simulation state, never replicated.
    particle_instances: Vec<ParticleInstance>,
    /// Scratch for this frame's silhouette shadow mesh; reused so a steady
    /// scene does not reallocate. Uploaded as one geometry, drawn once.
    shadow_mesh: ShadowMeshBuilder,
    shadow_geometry: Option<Geometry>,
    shadow_points: Vec<Vec3f>,
    /// Statics do not move, so their silhouettes are built once per
    /// (world edit, sun position) and memcpy'd into the frame mesh instead
    /// of being re-projected every frame. This is what makes static cast
    /// shadows affordable at all.
    static_shadow_mesh: ShadowMeshBuilder,
    static_shadow_key: Option<(u64, u64, u64)>,
    /// Bumped when the placed-prop list changes, so it can join the static
    /// shadow cache key.
    models_rev: u64,
    static_shadow_count: u64,
    /// CPU-baked occlusion (bake.rs), folded into instance colours. Renderer
    /// state by construction: the sim has no field for it, so a device may
    /// bake at a different quality than its peers without diverging.
    bake: LightBake,
}

/// One CPU-skinned mesh instance for [`GameRenderer::draw_scene_full`].
/// `vertices` is the packed GameMeshVertex layout `SkinnedModel::skin_to_packed`
/// emits (6 floats/vertex).
pub struct SkinnedDraw {
    pub key: u64,
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
    pub transform: Mat4f,
}

/// The skinned characters for one frame, drawn between the opaque and alpha
/// passes (so blob shadows and sensor ghosts blend over them correctly).
pub struct SkinnedBatch<'a> {
    pub skinned: &'a mut DrawGameSkinned,
    pub texture: &'a Texture,
    pub items: Vec<SkinnedDraw>,
}

/// A stock prop resident on the GPU: geometry uploaded once, plus the pack
/// atlas it samples. Thousands of models share a few dozen atlases, which is
/// what keeps a whole pack's worth of props cheap to draw.
struct LoadedModel {
    geometry: Geometry,
    texture: Texture,
    triangles: usize,
    /// Model-space bounds, kept so a caller can build a collider without
    /// re-parsing the GLB. A prop the player walks through is not in the
    /// world, it is painted on it.
    min: Vec3f,
    max: Vec3f,
    /// Decimated model-space points for the shadow hull, computed once at
    /// load. A tree's shadow should read as a tree rather than as its
    /// bounding rectangle, so these come from the mesh — but the projector
    /// convex-hulls whatever it is given, so a few dozen well-spread points
    /// carry the silhouette as faithfully as thousands would.
    shadow_points: Vec<Vec3f>,
    /// Low-res multi-box collider in model space (model.rs collider_parts).
    collider_parts: Vec<(Vec3f, Vec3f)>,
}

/// Pick a bounded, well-spread subset of a mesh's vertices for shadow casting.
///
/// Stride-sampling alone can miss the extremes that define a silhouette — the
/// tip of a roof, the ends of a branch — so the per-axis extremes are always
/// included and the stride fills in the shape between them.
fn shadow_hull_points(vertices: &[f32], stride: usize) -> Vec<Vec3f> {
    const TARGET: usize = 48;
    let count = vertices.len() / stride;
    if count == 0 {
        return Vec::new();
    }
    let at = |i: usize| {
        let b = i * stride;
        vec3f(vertices[b], vertices[b + 1], vertices[b + 2])
    };
    let mut out = Vec::with_capacity(TARGET + 6);
    let (mut lo, mut hi) = ([0usize; 3], [0usize; 3]);
    for i in 1..count {
        let p = at(i);
        for (a, v) in [p.x, p.y, p.z].iter().enumerate() {
            if *v < [at(lo[0]).x, at(lo[1]).y, at(lo[2]).z][a] {
                lo[a] = i;
            }
            if *v > [at(hi[0]).x, at(hi[1]).y, at(hi[2]).z][a] {
                hi[a] = i;
            }
        }
    }
    for i in lo.iter().chain(hi.iter()) {
        out.push(at(*i));
    }
    let step = (count / TARGET).max(1);
    let mut i = 0;
    while i < count {
        out.push(at(i));
        i += step;
    }
    out
}

/// One placed stock prop. `model` is the asset id it was loaded under, e.g.
/// `kenney/car-kit/ambulance`.
#[derive(Clone)]
pub struct ModelInstance {
    pub model: String,
    pub transform: Mat4f,
}

fn perf_us(t0: std::time::Instant) -> u64 {
    t0.elapsed().as_micros() as u64
}

/// Fold a baked shade multiplier into an instance colour.
///
/// Emissive surfaces opt out in proportion to their glow: a beacon in a
/// tunnel is the light source, so darkening it reads as a bug rather than
/// as occlusion. Alpha is never touched — for the alpha batch that would
/// change coverage, not brightness.
fn shade_color(color: Vec4f, shade: f32, glow: f32) -> Vec4f {
    let k = shade + (1.0 - shade) * glow.clamp(0.0, 1.0);
    vec4(color.x * k, color.y * k, color.z * k, color.w)
}

/// Write one [`GameSun`] into every game shader. This is the whole of the
/// T7 unification on the game side: before it, cube/terrain/skinned each
/// hardcoded their own ambient/direct split and five script blocks set the
/// light direction by hand. Skinned is applied in `draw_skinned_inner`,
/// which owns that struct.
pub(crate) fn apply_sun(cx: &Cx, draws: &mut GameDraws, sun: &GameSun, fog_color: Vec3f) {
    // The cube family batches thousands of instances, so its sun and fog
    // colour ride in uniforms; only the direction stays per-instance (it
    // belongs to DrawCube, shared with consumers outside this crate).
    draws.cube.cube.light_dir = sun.dir;
    sun.write_uniforms(cx, &mut draws.cube.cube.draw_vars);
    draws
        .cube
        .cube
        .draw_vars
        .set_uniform(cx, live_id!(fog_color), &[fog_color.x, fog_color.y, fog_color.z]);
    draws.alpha.cube.cube.light_dir = sun.dir;
    sun.write_uniforms(cx, &mut draws.alpha.cube.cube.draw_vars);
    draws.alpha.cube.cube.draw_vars.set_uniform(
        cx,
        live_id!(fog_color),
        &[fog_color.x, fog_color.y, fog_color.z],
    );
    // Terrain and skinned draw one instance each, so there is nothing to
    // save by moving their sun off the instance stream.
    sun.write_into(
        &mut draws.terrain.light_dir,
        &mut draws.terrain.sun_color,
        &mut draws.terrain.sun_sky,
        &mut draws.terrain.sun_ground,
    );
}

/// Desktop-class default; see [`GameRenderer::set_shadow_budget`].
pub const DEFAULT_SHADOW_BUDGET: usize = 24;

impl Default for GameRenderer {
    fn default() -> Self {
        Self {
            shape_geometries: Default::default(),
            static_slab: Default::default(),
            static_slab_alpha: Default::default(),
            slab_key: None,
            slab_instance_count: 0,
            terrain_geometry: None,
            terrain_revision: 0,
            skinned_geometries: Vec::new(),
            static_models: Vec::new(),
            placed_models: Vec::new(),
            stage: Stage::default(),
            shadow_budget: DEFAULT_SHADOW_BUDGET,
            particle_instances: Vec::new(),
            shadow_mesh: ShadowMeshBuilder::default(),
            shadow_geometry: None,
            shadow_points: Vec::new(),
            static_shadow_mesh: ShadowMeshBuilder::default(),
            static_shadow_key: None,
            models_rev: 0,
            static_shadow_count: 0,
            bake: LightBake::default(),
        }
    }
}

impl GameRenderer {
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
    /// mobile/standalone XR; see apps/arcade/BUDGETS.md.
    pub fn shadow_budget(&self) -> usize {
        self.shadow_budget
    }

    pub fn set_shadow_budget(&mut self, casters: usize) {
        self.shadow_budget = casters;
    }

    /// Hand this frame's particles to the renderer. They join the alpha
    /// batch, so any number of them still costs zero extra draw calls.
    pub fn set_particles(&mut self, instances: Vec<ParticleInstance>) {
        self.particle_instances = instances;
    }

    /// Hand this frame's stock props to the renderer. Order does not matter —
    /// `draw_models_inner` sorts by model so copies batch together.
    pub fn set_models(&mut self, instances: Vec<ModelInstance>) {
        // Statics are cached against a key; a changed prop list must break it
        // or the village would keep last frame's shadows forever.
        if instances.len() != self.placed_models.len() {
            self.models_rev = self.models_rev.wrapping_add(1);
        }
        self.placed_models = instances;
    }

    /// How much CPU the light bake may spend (bake.rs). Lower `ao_rays` and
    /// `max_probes` on standalone XR; see apps/arcade/BUDGETS.md. Setting
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

    /// Ground height under a caster: the terrain, or the tallest static box
    /// top it stands over. `None` when it is over a hole.
    fn ground_under(world: &GameWorld, e: &Entity) -> Option<f32> {
        let mut ground: Option<f32> = world
            .terrain
            .as_ref()
            .and_then(|t| t.floor_under(e.pos, e.half));
        let feet = e.pos.y - e.half.y;
        for s in world.entities.iter() {
            if s.sensor || s.hidden || !matches!(s.kind, BodyKind::Static | BodyKind::Kinematic) {
                continue;
            }
            let top = s.pos.y + s.half.y;
            if top <= feet + 0.01
                && (e.pos.x - s.pos.x).abs() < s.half.x
                && (e.pos.z - s.pos.z).abs() < s.half.z
            {
                ground = Some(ground.map_or(top, |g: f32| g.max(top)));
            }
        }
        ground
    }

    /// Casters for this frame, tagged with which shadow tier they get.
    ///
    /// Hero rule: rigid bodies (crates, vehicle chassis) and anything
    /// person-sized or larger reads as a real object and earns a projected
    /// silhouette; small scurrying movers get blobs, which is all a blob was
    /// ever good for. Ties are broken by camera distance so the budget
    /// spends itself on what fills the screen.
    fn shadow_casters<'w>(
        world: &'w GameWorld,
        camera_pos: Vec3f,
        budget: usize,
    ) -> Vec<(&'w Entity, f32, bool)> {
        const HERO_HEIGHT: f32 = 0.5;
        let mut heroes: Vec<(&Entity, f32, f32)> = Vec::new();
        let mut small: Vec<(&Entity, f32)> = Vec::new();
        for e in world.entities.iter() {
            if !matches!(e.kind, BodyKind::Mover | BodyKind::Rigid)
                || e.sensor
                || e.hidden
                || e.attached_to != 0
            {
                continue;
            }
            let Some(ground) = Self::ground_under(world, e) else {
                continue;
            };
            let hero = e.kind == BodyKind::Rigid || e.half.y * e.scale.y * 2.0 >= HERO_HEIGHT;
            if hero {
                let d = e.pos - camera_pos;
                heroes.push((e, ground, d.length_squared()));
            } else {
                small.push((e, ground));
            }
        }
        heroes.sort_by(|a, b| a.2.total_cmp(&b.2));
        let mut out: Vec<(&Entity, f32, bool)> = Vec::with_capacity(heroes.len() + small.len());
        for (i, (e, ground, _)) in heroes.into_iter().enumerate() {
            out.push((e, ground, i < budget));
        }
        out.extend(small.into_iter().map(|(e, ground)| (e, ground, false)));
        out
    }

    /// Ground-plane extent of the world's content, in world units: the
    /// centre and half-width of everything the diorama's shadow should
    /// catch. `None` when there is nothing to stand on.
    fn world_footprint(world: &GameWorld) -> Option<(Vec3f, f32)> {
        let mut min = vec3f(f32::MAX, f32::MAX, f32::MAX);
        let mut max = vec3f(f32::MIN, f32::MIN, f32::MIN);
        let mut any = false;
        for e in world.entities.iter() {
            let (p, h) = (e.pos, e.half);
            min.x = min.x.min(p.x - h.x);
            min.y = min.y.min(p.y - h.y);
            min.z = min.z.min(p.z - h.z);
            max.x = max.x.max(p.x + h.x);
            max.z = max.z.max(p.z + h.z);
            any = true;
        }
        if let Some(t) = world.terrain.as_ref() {
            let half = t.cell_size * (t.cells.saturating_sub(1)) as f32 * 0.5;
            let c = t.origin + half;
            min.x = min.x.min(c - half);
            min.z = min.z.min(c - half);
            max.x = max.x.max(c + half);
            max.z = max.z.max(c + half);
            any = true;
        }
        if !any {
            return None;
        }
        let center = vec3f((min.x + max.x) * 0.5, min.y, (min.z + max.z) * 0.5);
        let radius = ((max.x - min.x).max(max.z - min.z) * 0.5).max(0.5);
        Some((center, radius))
    }

    /// Rebuild the terrain GPU mesh when the world's terrain revision moved.
    /// Godot-style: two triangles per cell, verts duplicated per triangle so
    /// normals are flat, per-tri color = average of its corners.
    fn ensure_terrain_geometry(&mut self, cx: &mut Cx, terrain: &Terrain) -> GeometryId {
        if self.terrain_geometry.is_some() && self.terrain_revision == terrain.revision {
            return self.terrain_geometry.as_ref().unwrap().geometry_id();
        }
        let n = terrain.cells;
        let mut vertices: Vec<f32> = Vec::with_capacity((n - 1) * (n - 1) * 2 * 3 * 16);
        let mut indices: Vec<u32> = Vec::with_capacity((n - 1) * (n - 1) * 6);
        let world_pos = |gx: usize, gz: usize| -> Vec3f {
            vec3f(
                terrain.origin + gx as f32 * terrain.cell_size,
                terrain.heights[gz * n + gx],
                terrain.origin + gz as f32 * terrain.cell_size,
            )
        };
        let push_tri = |vertices: &mut Vec<f32>, indices: &mut Vec<u32>, a: Vec3f, b: Vec3f, c: Vec3f, color: Vec4f| {
            let normal = Vec3f::cross(b - a, c - a).normalize();
            for p in [a, b, c] {
                let base = vertices.len() as u32 / 16;
                let _ = base;
                // PbrVertex: pos_nx, ny_nz_uv, color, tangent — 16 floats.
                vertices.extend_from_slice(&[
                    p.x, p.y, p.z, normal.x, normal.y, normal.z, 0.0, 0.0, color.x, color.y,
                    color.z, color.w, 1.0, 0.0, 0.0, 1.0,
                ]);
                indices.push(vertices.len() as u32 / 16 - 1);
            }
        };
        for gz in 0..n - 1 {
            for gx in 0..n - 1 {
                let a = world_pos(gx, gz);
                let b = world_pos(gx + 1, gz);
                let c = world_pos(gx, gz + 1);
                let d = world_pos(gx + 1, gz + 1);
                let color_at = |gx: usize, gz: usize| terrain.colors[gz * n + gx];
                let c0 = color_at(gx, gz);
                let c1 = color_at(gx + 1, gz);
                let c2 = color_at(gx, gz + 1);
                let c3 = color_at(gx + 1, gz + 1);
                let avg3 = |x: Vec4f, y: Vec4f, z: Vec4f| {
                    vec4(
                        (x.x + y.x + z.x) / 3.0,
                        (x.y + y.y + z.y) / 3.0,
                        (x.z + y.z + z.z) / 3.0,
                        1.0,
                    )
                };
                // Same diagonal split as Terrain::height_at, CCW seen from +y.
                push_tri(&mut vertices, &mut indices, a, c, b, avg3(c0, c2, c1));
                push_tri(&mut vertices, &mut indices, b, c, d, avg3(c1, c2, c3));
            }
        }
        let geometry = Geometry::new(cx);
        geometry.update(cx, indices, vertices);
        let id = geometry.geometry_id();
        self.terrain_geometry = Some(geometry);
        self.terrain_revision = terrain.revision;
        id
    }

    /// Unit geometry for a shape, built once and shared by every instance
    /// (index = Shape::index()). All shapes span [-0.5, 0.5] so `cube_size`
    /// scales them exactly like the built-in cube.
    fn ensure_shape_geometry(&mut self, cx: &mut Cx, shape: Shape) -> GeometryId {
        let slot = &mut self.shape_geometries[shape.index()];
        if let Some(geometry) = slot {
            return geometry.geometry_id();
        }
        let (vertices, indices) = shape_geometry_data(shape);
        let geometry = Geometry::new(cx);
        geometry.update(cx, indices, vertices);
        let id = geometry.geometry_id();
        *slot = Some(geometry);
        id
    }

    /// Rotation part of an entity's transform. Rigids carry a full box3d
    /// orientation quat (M1a); everything else rotates by visual yaw exactly
    /// as before. Column-major, same layout as Mat4f::rotation.
    fn entity_rotation(e: &Entity) -> Mat4f {
        if e.kind == BodyKind::Rigid {
            let (x, y, z, w) = (e.orient.x, e.orient.y, e.orient.z, e.orient.w);
            let mut m = Mat4f::identity();
            m.v[0] = 1.0 - 2.0 * (y * y + z * z);
            m.v[1] = 2.0 * (x * y + w * z);
            m.v[2] = 2.0 * (x * z - w * y);
            m.v[4] = 2.0 * (x * y - w * z);
            m.v[5] = 1.0 - 2.0 * (x * x + z * z);
            m.v[6] = 2.0 * (y * z + w * x);
            m.v[8] = 2.0 * (x * z + w * y);
            m.v[9] = 2.0 * (y * z - w * x);
            m.v[10] = 1.0 - 2.0 * (x * x + y * y);
            m
        } else {
            Mat4f::rotation(vec3f(0.0, e.yaw, 0.0))
        }
    }

    /// PERF: pack one instance in the exact slice layout `DrawCube::draw`
    /// emits (DrawVars::as_slice covers the trailing glow/fog instance
    /// fields), so slab content and immediate draws are indistinguishable.
    fn pack_cube_instance(
        &mut self,
        draws: &mut GameDraws,
        alpha: bool,
        out_index: usize,
        transform: Mat4f,
        size: Vec3f,
        color: Vec4f,
        glow: f32,
    ) {
        if alpha {
            draws.alpha.cube.cube.transform = transform;
            draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
            draws.alpha.cube.cube.cube_size = size;
            draws.alpha.cube.cube.color = color;
            draws.alpha.cube.cube.depth_clip = 1.0;
            draws.alpha.cube.glow = glow;
            let slice = draws.alpha.cube.cube.draw_vars.as_slice();
            self.static_slab_alpha[out_index].extend_from_slice(slice);
            self.slab_instance_count += 1;
        } else {
            draws.cube.cube.transform = transform;
            draws.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
            draws.cube.cube.cube_size = size;
            draws.cube.cube.color = color;
            draws.cube.cube.depth_clip = 1.0;
            draws.cube.glow = glow;
            let slice = draws.cube.cube.draw_vars.as_slice();
            self.static_slab[out_index].extend_from_slice(slice);
            self.slab_instance_count += 1;
        }
    }

    /// PERF: rebuild the packed static instance slabs. Only runs when
    /// `world.render_rev` moved — the world bumps it on every mutation that
    /// changes what static content looks like (see mark_render_dirty).
    fn rebuild_static_slabs(&mut self, draws: &mut GameDraws, world: &GameWorld) {
        for slab in self.static_slab.iter_mut() {
            slab.clear();
        }
        for slab in self.static_slab_alpha.iter_mut() {
            slab.clear();
        }
        self.slab_instance_count = 0;
        // Static entities (opaque and sensor/alpha).
        for e in world
            .entities
            .iter()
            .filter(|e| e.kind == BodyKind::Static && !e.hidden)
        {
            let mut transform = Mat4f::rotation(vec3f(0.0, e.yaw, 0.0));
            transform.v[12] = e.pos.x;
            transform.v[13] = e.pos.y;
            transform.v[14] = e.pos.z;
            let size = vec3(
                e.half.x * 2.0 * e.scale.x,
                e.half.y * 2.0 * e.scale.y,
                e.half.z * 2.0 * e.scale.z,
            );
            let mut color = e.color;
            if e.sensor && color.w >= 0.99 {
                color.w = 0.35;
            }
            // Baked occlusion rides in the colour we were already sending —
            // no extra instance field, no shader work, no draw call.
            color = shade_color(color, self.bake.static_shade(e.id), e.glow);
            self.pack_cube_instance(draws, e.sensor, e.shape.index(), transform, size, color, e.glow);
        }
        // Settled parts of static owners.
        for p in world.parts.iter().filter(|p| !p.anim_active) {
            // Entity ids are spawn-ordered, so the list stays sorted; the
            // shared sim helper owns (and debug-asserts) that invariant.
            let Some(owner) = entity_index_sorted(&world.entities, p.owner)
                .map(|i| &world.entities[i])
                .filter(|e| e.kind == BodyKind::Static)
            else {
                continue;
            };
            let mut owner_frame = Mat4f::rotation(vec3f(0.0, owner.yaw, 0.0));
            owner_frame.v[12] = owner.pos.x;
            owner_frame.v[13] = owner.pos.y;
            owner_frame.v[14] = owner.pos.z;
            let mut local = Mat4f::rotation(p.rot);
            local.v[12] = p.offset.x * owner.scale.x;
            local.v[13] = p.offset.y * owner.scale.y;
            local.v[14] = p.offset.z * owner.scale.z;
            let transform = Mat4f::mul(&owner_frame, &local);
            let size = vec3(
                p.half.x * 2.0 * owner.scale.x,
                p.half.y * 2.0 * owner.scale.y,
                p.half.z * 2.0 * owner.scale.z,
            );
            // A part inherits its owner's bake — it is bolted to it.
            let color = shade_color(p.color, self.bake.static_shade(owner.id), p.glow);
            self.pack_cube_instance(draws, false, p.shape.index(), transform, size, color, p.glow);
        }
    }

    /// Build the silhouette shadows every static casts. Cached against
    /// (render_rev, bake generation) — statics do not move, so this runs on
    /// world edits and sun swings only.
    ///
    /// Two guards decide what casts. A floor is not a caster: an entity whose
    /// top barely clears what it stands on has nothing to throw, and a slab
    /// wider than the whole scene would hull to a shadow over everything.
    fn rebuild_static_shadows(&mut self, world: &GameWorld, sun: &GameSun) {
        self.static_shadow_mesh.clear();
        self.static_shadow_count = 0;
        for e in world.entities.iter() {
            if e.kind != BodyKind::Static || e.sensor || e.hidden || e.color.w < 0.99 {
                continue;
            }
            let half = vec3f(
                e.half.x * e.scale.x,
                e.half.y * e.scale.y,
                e.half.z * e.scale.z,
            );
            let Some(ground) = Self::ground_under(world, e) else {
                continue;
            };
            // Stands up enough to cast, and is not itself the ground.
            if e.pos.y + half.y - ground < 0.3 {
                continue;
            }
            if half.x.max(half.z) > 24.0 {
                continue;
            }
            let mut transform = Mat4f::rotation(vec3f(0.0, e.yaw, 0.0));
            transform.v[12] = e.pos.x;
            transform.v[13] = e.pos.y;
            transform.v[14] = e.pos.z;
            caster_points(
                e.shape,
                &transform,
                vec3f(half.x * 2.0, half.y * 2.0, half.z * 2.0),
                &mut self.shadow_points,
            );
            let receiver = Receiver {
                base_y: ground,
                terrain: world.terrain.as_ref(),
            };
            if crate::shadow_mesh::build_caster_shadow(
                &self.shadow_points,
                sun,
                &receiver,
                &mut self.static_shadow_mesh,
            ) {
                self.static_shadow_count += 1;
            }
        }

        // Stock props are drawn as meshes, not entities, so they never reach
        // the loop above — which is why a village of trees and houses cast
        // nothing at all. They are static by nature, so they belong in this
        // same baked layer: projected once per (world, sun, prop list) and
        // merged into ONE geometry, never re-projected per frame.
        //
        // Points come from the model's own mesh rather than its bounds: a
        // pine's shadow should taper like a pine. The projector convex-hulls
        // them anyway, so the decimated set carries the silhouette.
        let ground = world
            .entities
            .iter()
            .filter(|e| e.kind == BodyKind::Static && !e.sensor)
            .map(|e| e.pos.y + e.half.y * e.scale.y)
            .fold(f32::MIN, f32::max);
        let ground = if ground == f32::MIN { 0.0 } else { ground };
        for inst in &self.placed_models {
            let Some((_, m)) = self.static_models.iter().find(|(k, _)| *k == inst.model) else {
                continue;
            };
            if m.shadow_points.len() < 3 {
                continue;
            }
            let t = &inst.transform;
            self.shadow_points.clear();
            self.shadow_points.extend(m.shadow_points.iter().map(|l| {
                vec3f(
                    t.v[0] * l.x + t.v[4] * l.y + t.v[8] * l.z + t.v[12],
                    t.v[1] * l.x + t.v[5] * l.y + t.v[9] * l.z + t.v[13],
                    t.v[2] * l.x + t.v[6] * l.y + t.v[10] * l.z + t.v[14],
                )
            }));
            let receiver = Receiver {
                base_y: ground,
                terrain: world.terrain.as_ref(),
            };
            if crate::shadow_mesh::build_caster_shadow(
                &self.shadow_points,
                sun,
                &receiver,
                &mut self.static_shadow_mesh,
            ) {
                self.static_shadow_count += 1;
            }
        }
    }

    /// Load a stock prop onto the GPU under `id`, idempotent. `png` is the
    /// pack atlas, shared by every model in that pack.
    pub fn load_model(
        &mut self,
        cx: &mut Cx,
        id: &str,
        glb: &[u8],
        png: Option<&[u8]>,
    ) -> Result<usize, String> {
        if let Some(at) = self.static_models.iter().position(|(k, _)| k == id) {
            return Ok(self.static_models[at].1.triangles);
        }
        let model = StaticModel::parse_glb(glb)?;
        let triangles = model.triangle_count();
        let (min, max) = (model.min, model.max);
        let shadow_points = shadow_hull_points(&model.vertices, crate::model::MODEL_VERTEX_FLOATS);
        let collider_parts = model.collider_parts();
        let geometry = Geometry::new(cx);
        geometry.update(cx, model.indices, model.vertices);
        // Two Kenney conventions, one path. A pack that UV-maps into an atlas
        // needs that atlas — missing it would render white, which reads as a
        // broken model, so it is an error. A pack that carries no texture at
        // all (nature-kit: flat per-material colours) is correct with a white
        // 1x1, because model.rs baked those colours into the vertex tint and
        // the shader multiplies the two.
        let texture = match (png, model.texture_uri.as_deref()) {
            (Some(bytes), _) => ImageBuffer::from_png(bytes)
                .map_err(|e| format!("{id}: atlas decode failed: {e:?}"))?
                .into_new_texture(cx),
            (None, None) => {
                let mut white = ImageBuffer::default();
                white.width = 1;
                white.height = 1;
                white.data = vec![0xFFFF_FFFF];
                white.into_new_texture(cx)
            }
            (None, Some(uri)) => {
                return Err(format!(
                    "{id}: atlas {uri} missing — run apps/arcade/download_assets.sh"
                ))
            }
        };
        self.static_models.push((
            id.to_string(),
            LoadedModel {
                geometry,
                texture,
                triangles,
                min,
                max,
                shadow_points,
                collider_parts,
            },
        ));
        Ok(triangles)
    }

    pub fn model_is_loaded(&self, id: &str) -> bool {
        self.static_models.iter().any(|(k, _)| k == id)
    }

    /// The prop's low-res multi-box collider in model space. A house comes
    /// back as walls and roof rather than one box, so its doorway is a gap.
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

    /// Draw the placed stock props. Instances are grouped by model, so N
    /// copies of one prop cost ONE draw item with N instances rather than N
    /// draws — which is what makes a scene full of stock trees affordable.
    fn draw_models_inner(
        &mut self,
        cx: &mut Cx3d,
        draw: &mut DrawGameSkinned,
        instances: &[ModelInstance],
        fog: (Vec3f, f32),
        sun: &GameSun,
        stats: &mut RenderStats,
    ) {
        if instances.is_empty() {
            return;
        }
        sun.write_into(
            &mut draw.light_dir,
            &mut draw.sun_color,
            &mut draw.sun_sky,
            &mut draw.sun_ground,
        );
        draw.fog_color = fog.0;
        draw.fog_density = fog.1;
        draw.depth_clip = 1.0;
        // Sort by model so equal geometry+texture land adjacent: consecutive
        // add_instance calls with unchanged geometry and texture accumulate
        // into a single draw item.
        let mut order: Vec<usize> = (0..instances.len()).collect();
        order.sort_by(|a, b| instances[*a].model.cmp(&instances[*b].model));
        let mut last: Option<String> = None;
        for i in order {
            let inst = &instances[i];
            let Some(at) = self.static_models.iter().position(|(k, _)| *k == inst.model) else {
                continue;
            };
            let loaded = &self.static_models[at];
            draw.draw_vars.geometry_id = Some(loaded.1.geometry.geometry_id());
            draw.draw_vars.set_texture(0, &loaded.1.texture);
            draw.transform = inst.transform;
            if last.as_deref() != Some(inst.model.as_str()) {
                stats.model_draws += 1;
                last = Some(inst.model.clone());
            }
            stats.model_instances += 1;
            stats.model_triangles += loaded.1.triangles;
            if draw.draw_vars.can_instance() {
                let new_area = cx.add_instance(&draw.draw_vars);
                draw.draw_vars.area = cx.update_area_refs(draw.draw_vars.area, new_area);
            }
        }
    }

    /// Upload + draw the skinned batch inside the already-open scene pass.
    fn draw_skinned_inner(
        &mut self,
        cx: &mut Cx3d,
        batch: SkinnedBatch,
        fog: (Vec3f, f32),
        sun: &GameSun,
    ) {
        sun.write_into(
            &mut batch.skinned.light_dir,
            &mut batch.skinned.sun_color,
            &mut batch.skinned.sun_sky,
            &mut batch.skinned.sun_ground,
        );
        for item in batch.items {
            let geometry_id = match self
                .skinned_geometries
                .iter()
                .position(|(key, _)| *key == item.key)
            {
                Some(at) => {
                    let geometry = &self.skinned_geometries[at].1;
                    geometry.update(cx.cx, item.indices, item.vertices);
                    geometry.geometry_id()
                }
                None => {
                    let geometry = Geometry::new(cx.cx);
                    geometry.update(cx.cx, item.indices, item.vertices);
                    let id = geometry.geometry_id();
                    self.skinned_geometries.push((item.key, geometry));
                    id
                }
            };
            batch.skinned.draw_vars.geometry_id = Some(geometry_id);
            batch.skinned.transform = item.transform;
            batch.skinned.depth_clip = 1.0;
            batch.skinned.fog_color = fog.0;
            batch.skinned.fog_density = fog.1;
            batch.skinned.draw_vars.set_texture(0, batch.texture);
            if batch.skinned.draw_vars.can_instance() {
                let new_area = cx.add_instance(&batch.skinned.draw_vars);
                batch.skinned.draw_vars.area =
                    cx.update_area_refs(batch.skinned.draw_vars.area, new_area);
            }
        }
    }

    /// Encode the whole 3D scene for one view. `draw_list` is the host's
    /// scene draw list (begun/ended here, exactly as before the move).
    pub fn draw_scene(
        &mut self,
        cx: &mut Cx3d,
        draw_list: &mut DrawList,
        draws: &mut GameDraws,
        world: &GameWorld,
        scene_state: SceneState3D,
    ) -> RenderStats {
        self.draw_scene_full(cx, draw_list, draws, world, scene_state, None, None)
    }

    /// [`draw_scene`] plus an optional skinned-character batch.
    pub fn draw_scene_full(
        &mut self,
        cx: &mut Cx3d,
        draw_list: &mut DrawList,
        draws: &mut GameDraws,
        world: &GameWorld,
        scene_state: SceneState3D,
        skinned: Option<SkinnedBatch>,
        models_draw: Option<&mut DrawGameSkinned>,
    ) -> RenderStats {
        let mut stats = RenderStats::default();
        let camera_pos = scene_state.camera_pos;
        draw_list.begin_always(cx);
        cx.begin_scene_3d(scene_state);
        let stage_matrix = self.stage.matrix();
        let previous_world = cx.set_scene_world_transform_3d(stage_matrix);
        // The stage rides on the draw list's view transform: every game
        // shader computes `draw_list.view_transform * transform`, so one
        // uniform scales and anchors sky, terrain, cubes and characters
        // together. The instance transforms below stay in world units, which
        // is why the cached static slabs survive a stage change untouched.
        // (The view matrix cannot carry this — in XR the runtime overwrites
        // camera_view/_r with its own eye matrices every frame.)
        cx.cx.draw_lists[draw_list.id()]
            .draw_list_uniforms
            .view_transform = stage_matrix;
        // MR puts the game on your real floor: the room supplies the
        // horizon, so the game's own environment would only paint over the
        // passthrough feed.
        let shows_environment = self.stage.shows_environment();

        // Fog only exists once the script asked for a sky.
        let (fog_color, fog_density) = match &world.sky {
            Some(sky) if shows_environment => {
                (vec3(sky.horizon.x, sky.horizon.y, sky.horizon.z), sky.fog)
            }
            _ => (vec3(0.75, 0.87, 0.96), 0.0),
        };

        // One sun for every shader this frame (sun.rs). Written before any
        // batch begins, because instance fields are snapshotted per draw and
        // uniforms are captured when the draw item opens.
        let sun = crate::sun::resolve_sun(&world.sun);
        apply_sun(cx.cx, draws, &sun, fog_color);

        // 1. Sky dome around the camera (depth-tested at radius, drawn first).
        if let Some(sky) = world.sky.as_ref().filter(|_| shows_environment) {
            let mut transform = Mat4f::identity();
            transform.v[12] = camera_pos.x;
            transform.v[13] = camera_pos.y;
            transform.v[14] = camera_pos.z;
            draws.sky.cube.transform = transform;
            draws.sky.cube.cube_pos = vec3(0.0, 0.0, 0.0);
            draws.sky.cube.cube_size = vec3(800.0, 800.0, 800.0);
            draws.sky.cube.color = vec4(1.0, 1.0, 1.0, 1.0);
            draws.sky.cube.depth_clip = 1.0;
            draws.sky.sky_top = vec3(sky.top.x, sky.top.y, sky.top.z);
            draws.sky.sky_horizon = vec3(sky.horizon.x, sky.horizon.y, sky.horizon.z);
            draws.sky.sky_ground = vec3(sky.ground.x, sky.ground.y, sky.ground.z);
            draws.sky.sky_bottom =
                vec3(sky.ground_bottom.x, sky.ground_bottom.y, sky.ground_bottom.z);
            draws.sky.cube.draw(cx);
            stats.sky_drawn = true;
        }

        // 2. The smooth terrain mesh.
        if let Some(terrain) = world.terrain.clone().filter(|_| shows_environment) {
            let geometry_id = self.ensure_terrain_geometry(cx.cx, &terrain);
            draws.terrain.draw_vars.geometry_id = Some(geometry_id);
            draws.terrain.transform = Mat4f::identity();
            draws.terrain.depth_clip = 1.0;
            draws.terrain.fog_color = fog_color;
            draws.terrain.fog_density = fog_density;
            if draws.terrain.draw_vars.can_instance() {
                let new_area = cx.add_instance(&draws.terrain.draw_vars);
                draws.terrain.draw_vars.area =
                    cx.update_area_refs(draws.terrain.draw_vars.area, new_area);
            }
            stats.terrain_drawn = true;
        }

        // PERF: sections 3+4 batch per shape through many_instances. Statics
        // come from packed slabs rebuilt only when world.render_rev moves
        // (bump it — mark_render_dirty — or your static edit won't show);
        // dynamics (movers, their parts, beams, blob shadows) re-pack every
        // frame. One draw call per shape per pass; empty batches are skipped.
        // fog_color is a uniform now (set in apply_sun); only the density
        // stays per-instance, because shadows switch it off individually.
        draws.cube.fog_density = fog_density;
        draws.alpha.cube.fog_density = fog_density;

        // CPU light bake (bake.rs). Geometry-dependent occlusion is baked
        // once per world edit; the sun term is one ray per target and
        // refreshes whenever the sun swings, so a day cycle moves the baked
        // shadows instead of freezing them at dawn. Both land in the colours
        // packed below — the GPU never learns this happened.
        self.bake.update(world, &sun);
        stats.bake = self.bake.stats();

        let vars_ready = draws.cube.cube.draw_vars.can_instance()
            && draws.alpha.cube.cube.draw_vars.can_instance();
        let slab_key = (world.render_rev, self.bake.generation());
        if vars_ready && self.slab_key != Some(slab_key) {
            let t0 = std::time::Instant::now();
            self.rebuild_static_slabs(draws, world);
            stats.slab_us += perf_us(t0);
            stats.slab_rebuilds += 1;
            self.slab_key = Some(slab_key);
        }
        stats.static_instances = self.slab_instance_count;
        stats.dyn_instances = 0;
        stats.instance_floats = draws.cube.cube.draw_vars.as_slice().len() as u32;

        // Silhouette shadows accumulate into one mesh across the alpha loop
        // below and are drawn once at the end. A host that has not adopted
        // the shadow shader keeps the old blob-only behaviour.
        let shadow_mesh_enabled = draws.shadow.is_some();
        self.shadow_mesh.clear();
        if shadow_mesh_enabled {
            // Static casters: a pillar's shadow is the same every frame until
            // the world or the sun changes, so build it on that key and splice
            // the cache in. Without this, static shadows would cost a full
            // hull projection per pillar per frame for no new information.
            let key = (world.render_rev, self.bake.generation(), self.models_rev);
            if self.static_shadow_key != Some(key) {
                self.rebuild_static_shadows(world, &sun);
                self.static_shadow_key = Some(key);
            }
            self.shadow_mesh.append(&self.static_shadow_mesh);
            stats.shadows += self.static_shadow_count;
            stats.projected_shadows += self.static_shadow_count;
        }

        // PERF: resolve dynamic parts and shape membership ONCE per frame —
        // the per-shape loops below must not re-scan entities per part.
        let mut dyn_parts: Vec<(usize, usize)> = Vec::new();
        for (part_index, part) in world.parts.iter().enumerate() {
            let Some(owner_index) = entity_index_sorted(&world.entities, part.owner) else {
                continue;
            };
            if world.entities[owner_index].kind != BodyKind::Static || part.anim_active {
                dyn_parts.push((part_index, owner_index));
            }
        }
        let mut dyn_entity_shapes = [false; 5];
        let mut dyn_sensor_shapes = [false; 5];
        for e in world
            .entities
            .iter()
            .filter(|e| e.kind != BodyKind::Static && !e.hidden)
        {
            if e.sensor {
                dyn_sensor_shapes[e.shape.index()] = true;
            } else {
                dyn_entity_shapes[e.shape.index()] = true;
            }
        }
        let mut dyn_part_shapes = [false; 5];
        for (part_index, _) in &dyn_parts {
            dyn_part_shapes[world.parts[*part_index].shape.index()] = true;
        }

        // 3. Opaque pass, one batch per shape.
        for shape in Shape::ALL {
            let shape_index = shape.index();
            let has_static = !self.static_slab[shape_index].is_empty();
            let has_dynamic_entity = dyn_entity_shapes[shape_index];
            let has_dynamic_part = dyn_part_shapes[shape_index];
            let has_beams = shape == Shape::Box && !world.beams.is_empty();
            if !has_static && !has_dynamic_entity && !has_dynamic_part && !has_beams {
                continue;
            }
            let geometry_id = self.ensure_shape_geometry(cx.cx, shape);
            draws.cube.cube.draw_vars.geometry_id = Some(geometry_id);
            draws.cube.cube.many_instances =
                cx.begin_many_instances(&draws.cube.cube.draw_vars);
            if has_static {
                if let Some(mi) = &mut draws.cube.cube.many_instances {
                    mi.instances
                        .extend_from_slice(&self.static_slab[shape_index]);
                }
            }
            // Dynamic entities: movers/kinematics/projectiles of this shape.
            for e in world
                .entities
                .iter()
                .filter(|e| !e.sensor && !e.hidden && e.kind != BodyKind::Static && e.shape == shape)
            {
                let mut transform = Self::entity_rotation(e);
                transform.v[12] = e.pos.x;
                transform.v[13] = e.pos.y;
                transform.v[14] = e.pos.z;
                draws.cube.cube.transform = transform;
                draws.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                draws.cube.cube.cube_size = vec3(
                    e.half.x * 2.0 * e.scale.x,
                    e.half.y * 2.0 * e.scale.y,
                    e.half.z * 2.0 * e.scale.z,
                );
                // Movers sample the baked probe lattice, so a crate rolling
                // under a bridge darkens without a shadow map or a pass.
                draws.cube.cube.color =
                    shade_color(e.color, self.bake.dynamic_shade(e.pos), e.glow);
                draws.cube.cube.depth_clip = 1.0;
                draws.cube.glow = e.glow;
                draws.cube.cube.draw(cx);
                stats.dyn_instances += 1;
            }
            // Parts that are NOT in the slab: dynamic owner, or mid-animation.
            for (part_index, owner_index) in dyn_parts.iter().copied() {
                let part = &world.parts[part_index];
                if part.shape != shape {
                    continue;
                }
                let owner = &world.entities[owner_index];
                let mut owner_frame = Self::entity_rotation(owner);
                owner_frame.v[12] = owner.pos.x;
                owner_frame.v[13] = owner.pos.y;
                owner_frame.v[14] = owner.pos.z;
                let mut local = Mat4f::rotation(part.rot);
                local.v[12] = part.offset.x * owner.scale.x;
                local.v[13] = part.offset.y * owner.scale.y;
                local.v[14] = part.offset.z * owner.scale.z;
                draws.cube.cube.transform = Mat4f::mul(&owner_frame, &local);
                draws.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                draws.cube.cube.cube_size = vec3(
                    part.half.x * 2.0 * owner.scale.x,
                    part.half.y * 2.0 * owner.scale.y,
                    part.half.z * 2.0 * owner.scale.z,
                );
                draws.cube.cube.color =
                    shade_color(part.color, self.bake.dynamic_shade(owner.pos), part.glow);
                draws.cube.cube.depth_clip = 1.0;
                draws.cube.glow = part.glow;
                draws.cube.cube.draw(cx);
                stats.dyn_instances += 1;
            }
            // Immediate-mode beams (box batch): a box stretched between two
            // points (grapple cables, lasers). Cable axis on local z.
            if has_beams {
                for beam in &world.beams {
                    let d = beam.to - beam.from;
                    let len = d.length();
                    if len < 1.0e-4 {
                        continue;
                    }
                    let f = d * (1.0 / len);
                    let upv = if f.y.abs() > 0.99 {
                        vec3f(1.0, 0.0, 0.0)
                    } else {
                        vec3f(0.0, 1.0, 0.0)
                    };
                    let r = Vec3f::cross(upv, f).normalize();
                    let u = Vec3f::cross(f, r);
                    let mid = beam.from + d * 0.5;
                    let mut m = Mat4f::identity();
                    m.v[0] = r.x;
                    m.v[1] = r.y;
                    m.v[2] = r.z;
                    m.v[4] = u.x;
                    m.v[5] = u.y;
                    m.v[6] = u.z;
                    m.v[8] = f.x;
                    m.v[9] = f.y;
                    m.v[10] = f.z;
                    m.v[12] = mid.x;
                    m.v[13] = mid.y;
                    m.v[14] = mid.z;
                    draws.cube.cube.transform = m;
                    draws.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    draws.cube.cube.cube_size = vec3(beam.size, beam.size, len);
                    draws.cube.cube.color = beam.color;
                    draws.cube.cube.depth_clip = 1.0;
                    draws.cube.glow = beam.glow;
                    draws.cube.cube.draw(cx);
                    stats.dyn_instances += 1;
                }
            }
            if let Some(mi) = draws.cube.cube.many_instances.take() {
                cx.end_many_instances(mi);
            }
        }

        // 3.5 Skinned characters — after all opaque, before alpha blending.
        if let Some(batch) = skinned {
            // A character's shadow comes from its POSED vertices, decimated
            // to ~64 samples: person-shaped, and it walks when they walk.
            // Projecting all 3716 would cost more than the shadow is worth.
            if shadow_mesh_enabled {
                let ground = world
                    .terrain
                    .as_ref()
                    .and_then(|t| {
                        let p = batch.items.first()?;
                        t.height_at(p.transform.v[12], p.transform.v[14])
                    })
                    .unwrap_or(0.0);
                for item in batch.items.iter() {
                    skinned_proxy_points(&item.vertices, &item.transform, &mut self.shadow_points);
                    let receiver = Receiver {
                        base_y: ground,
                        terrain: world.terrain.as_ref(),
                    };
                    if crate::shadow_mesh::build_caster_shadow(
                        &self.shadow_points,
                        &sun,
                        &receiver,
                        &mut self.shadow_mesh,
                    ) {
                        stats.shadows += 1;
                        stats.projected_shadows += 1;
                    }
                }
            }
            self.draw_skinned_inner(cx, batch, (fog_color, fog_density), &sun);
        }

        // Stock props: the same shader as the skinned path (both are textured
        // packed meshes), but with geometry uploaded once instead of per frame.
        if let Some(draw) = models_draw {
            let instances = std::mem::take(&mut self.placed_models);
            self.draw_models_inner(cx, draw, &instances, (fog_color, fog_density), &sun, &mut stats);
            self.placed_models = instances;
        }

        // 4. Alpha pass, one batch per shape: static sensors from the slab,
        // then blob shadows (box batch) and dynamic sensors — drawn after all
        // opaque geometry so blending sees depth.
        for shape in Shape::ALL {
            let shape_index = shape.index();
            let has_static = !self.static_slab_alpha[shape_index].is_empty();
            let has_dynamic_sensor = dyn_sensor_shapes[shape_index];
            let has_shadows = shape == Shape::Box
                && world.entities.iter().any(|e| {
                    matches!(e.kind, BodyKind::Mover | BodyKind::Rigid)
                        && !e.sensor
                        && !e.hidden
                        && e.attached_to == 0
                });
            let has_particles = shape == Shape::Box && !self.particle_instances.is_empty();
            if !has_static && !has_dynamic_sensor && !has_shadows && !has_particles {
                continue;
            }
            let geometry_id = self.ensure_shape_geometry(cx.cx, shape);
            draws.alpha.cube.cube.draw_vars.geometry_id = Some(geometry_id);
            draws.alpha.cube.cube.many_instances =
                cx.begin_many_instances(&draws.alpha.cube.cube.draw_vars);
            if has_static {
                if let Some(mi) = &mut draws.alpha.cube.cube.many_instances {
                    mi.instances
                        .extend_from_slice(&self.static_slab_alpha[shape_index]);
                }
            }
            if has_shadows {
                // Tiered cast shadows: the nearest casters get a real
                // silhouette MESH (shadow_mesh.rs, accumulated below and
                // drawn as one geometry); everything else falls back to a
                // blob quad in this batch. So the budget buys fidelity, and
                // the whole shadow layer is at most two draw calls.
                for (e, ground, projected) in
                    Self::shadow_casters(world, camera_pos, self.shadow_budget)
                {
                    let half = vec3f(
                        e.half.x * e.scale.x,
                        e.half.y * e.scale.y,
                        e.half.z * e.scale.z,
                    );
                    if projected && shadow_mesh_enabled {
                        // Silhouette tier: hull of the caster's own points,
                        // draped over whatever it lands on.
                        let mut transform = Self::entity_rotation(e);
                        transform.v[12] = e.pos.x;
                        transform.v[13] = e.pos.y;
                        transform.v[14] = e.pos.z;
                        caster_points(
                            e.shape,
                            &transform,
                            vec3f(half.x * 2.0, half.y * 2.0, half.z * 2.0),
                            &mut self.shadow_points,
                        );
                        let receiver = Receiver {
                            base_y: ground,
                            terrain: world.terrain.as_ref(),
                        };
                        if crate::shadow_mesh::build_caster_shadow(
                            &self.shadow_points,
                            &sun,
                            &receiver,
                            &mut self.shadow_mesh,
                        ) {
                            stats.shadows += 1;
                            stats.projected_shadows += 1;
                        }
                        continue;
                    }
                    let quad = crate::shadow::blob_shadow(e.pos, half, ground, &sun);
                    let Some(quad) = quad else { continue };
                    // No fog on a shadow. It lies ON ground that is already
                    // fogged, so fogging it again mixes its RGB toward the
                    // bright horizon colour and a distant shadow comes out
                    // LIGHTER than the surface it darkens.
                    draws.alpha.cube.fog_density = 0.0;
                    draws.alpha.cube.cube.transform = quad.transform();
                    draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    draws.alpha.cube.cube.cube_size = quad.size();
                    // The pipeline blends PREMULTIPLIED (src*1 + dst*(1-a)),
                    // so a shadow must be premultiplied black: RGB 0 leaves
                    // exactly ground*(1-a) — a true multiplicative shadow.
                    // Unpremultiplied dark RGB adds light instead of removing
                    // it, which is why the old blob shadows read as pale.
                    draws.alpha.cube.cube.color = vec4(0.0, 0.0, 0.0, quad.alpha);
                    draws.alpha.cube.cube.depth_clip = 1.0;
                    draws.alpha.cube.glow = 0.0;
                    draws.alpha.cube.cube.draw(cx);
                    stats.dyn_instances += 1;
                    stats.shadows += 1;
                }
                draws.alpha.cube.fog_density = fog_density;
            }
            if has_particles {
                // Small unlit-ish cubes: cheap, and they read fine at the
                // sizes particles live at. Emission carries the colour so a
                // spark stays bright regardless of where the sun is.
                for p in self.particle_instances.iter() {
                    let mut transform = Mat4f::identity();
                    transform.v[12] = p.pos.x;
                    transform.v[13] = p.pos.y;
                    transform.v[14] = p.pos.z;
                    draws.alpha.cube.cube.transform = transform;
                    draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    draws.alpha.cube.cube.cube_size = vec3(p.size, p.size, p.size);
                    // Premultiplied, same blend rule as the shadows above —
                    // otherwise a fading particle stays at full brightness
                    // and smoke reads as a blown-out white plume.
                    draws.alpha.cube.cube.color = vec4(
                        p.color.x * p.color.w,
                        p.color.y * p.color.w,
                        p.color.z * p.color.w,
                        p.color.w,
                    );
                    draws.alpha.cube.cube.depth_clip = 1.0;
                    draws.alpha.cube.glow = 0.8;
                    draws.alpha.cube.cube.draw(cx);
                    stats.dyn_instances += 1;
                    stats.particles += 1;
                }
                draws.alpha.cube.glow = 0.0;
            }
            for e in world
                .entities
                .iter()
                .filter(|e| e.sensor && !e.hidden && e.kind != BodyKind::Static && e.shape == shape)
            {
                let mut transform = Self::entity_rotation(e);
                transform.v[12] = e.pos.x;
                transform.v[13] = e.pos.y;
                transform.v[14] = e.pos.z;
                draws.alpha.cube.cube.transform = transform;
                draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                draws.alpha.cube.cube.cube_size = vec3(
                    e.half.x * 2.0 * e.scale.x,
                    e.half.y * 2.0 * e.scale.y,
                    e.half.z * 2.0 * e.scale.z,
                );
                let mut color = e.color;
                if color.w >= 0.99 {
                    // Sensors are see-through by default; explicit alpha wins.
                    color.w = 0.35;
                }
                draws.alpha.cube.cube.color = color;
                draws.alpha.cube.cube.depth_clip = 1.0;
                draws.alpha.cube.glow = e.glow;
                draws.alpha.cube.cube.draw(cx);
                stats.dyn_instances += 1;
            }
            if let Some(mi) = draws.alpha.cube.cube.many_instances.take() {
                cx.end_many_instances(mi);
            }
        }

        // 4.5 The silhouette shadow layer: every caster's hull, one geometry,
        // ONE draw call. Drawn after the alpha batches so it lies over the
        // ground it darkens; depth test on / depth write off means
        // overlapping shadows can never fight each other for the buffer.
        if !self.shadow_mesh.is_empty() {
            if let Some(shadow) = draws.shadow.as_deref_mut() {
                let geometry = self.shadow_geometry.get_or_insert_with(|| Geometry::new(cx.cx));
                geometry.update(
                    cx.cx,
                    std::mem::take(&mut self.shadow_mesh.indices),
                    std::mem::take(&mut self.shadow_mesh.vertices),
                );
                shadow.draw_vars.geometry_id = Some(geometry.geometry_id());
                if shadow.draw_vars.can_instance() {
                    let new_area = cx.add_instance(&shadow.draw_vars);
                    shadow.draw_vars.area = cx.update_area_refs(shadow.draw_vars.area, new_area);
                }
            }
        }

        // 5. MR shadow catcher: a dark translucent slab just under the
        // diorama's footprint. Without it the world reads as floating
        // stickers over the passthrough feed; with it, planted on the floor.
        // Drawn last so it blends over everything it sits beneath.
        if !shows_environment {
            if let Some(footprint) = Self::world_footprint(world) {
                let (center, radius) = footprint;
                let mut transform = Mat4f::identity();
                transform.v[12] = center.x;
                transform.v[13] = center.y - 0.02 / self.stage.scale.max(1.0e-4);
                transform.v[14] = center.z;
                draws.alpha.cube.cube.transform = transform;
                draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                // Flat slab: thin in y, footprint-sized in x/z.
                let thickness = 0.04 / self.stage.scale.max(1.0e-4);
                draws.alpha.cube.cube.cube_size =
                    vec3(radius * 2.0, thickness, radius * 2.0);
                draws.alpha.cube.cube.color = vec4(0.0, 0.0, 0.0, 0.25);
                draws.alpha.cube.cube.depth_clip = 1.0;
                draws.alpha.cube.glow = 0.0;
                draws.alpha.cube.cube.draw(cx);
                stats.dyn_instances += 1;
                stats.shadow_catcher_drawn = true;
            }
        }

        if let Some(previous_world) = previous_world {
            let _ = cx.set_scene_world_transform_3d(previous_world);
        }
        cx.end_scene_3d();
        draw_list.end(cx);
        stats
    }
}

#[cfg(test)]
mod sun_tests {
    use super::*;

    /// T7 on the game side: every game shader must read ONE sun. Before the
    /// unification each shader carried its own ambient/direct constants and
    /// five script blocks set the light direction by hand, so changing one
    /// silently left the others behind. `write_into` is the single write
    /// path — apply_sun calls it once per shader and draw_skinned_inner
    /// calls it for the skinned struct, so uniformity is compiler-enforced
    /// and this asserts the payload rather than eyeballing a capture.
    #[test]
    fn write_into_sets_every_sun_field() {
        let sun = GameSun::from_time_of_day(8.0, 52.0);
        let (mut dir, mut color, mut sky, mut ground) = (
            Vec3f::default(),
            Vec3f::default(),
            Vec3f::default(),
            Vec3f::default(),
        );
        sun.write_into(&mut dir, &mut color, &mut sky, &mut ground);
        assert_eq!(dir, sun.dir);
        assert_eq!(color, sun.color);
        assert_eq!(sky, sun.sky);
        assert_eq!(ground, sun.ground);
    }

    /// Two shaders fed by the same sun end up with identical values — the
    /// property that used to fail silently.
    #[test]
    fn two_targets_receive_identical_values() {
        let sun = GameSun::from_time_of_day(17.0, 52.0);
        let mut a = [Vec3f::default(); 4];
        let mut b = [Vec3f::default(); 4];
        let [a0, a1, a2, a3] = &mut a;
        sun.write_into(a0, a1, a2, a3);
        let [b0, b1, b2, b3] = &mut b;
        sun.write_into(b0, b1, b2, b3);
        assert_eq!(a, b);
    }

    /// A default world must light exactly as the pre-unification shaders
    /// did, so adopting SceneSun did not restyle every existing game.
    #[test]
    fn the_default_sun_is_the_legacy_look() {
        let sun = crate::sun::resolve_sun(&makepad_game_sim::SunConfig::default());
        assert_eq!(sun, GameSun::default());
        // Flat hemisphere collapses mix(ground, sky, h) to the old constant.
        assert_eq!(sun.sky, sun.ground);
    }

    /// The silhouette lives at the extremes: a stride-sample alone can miss a
    /// roof ridge or a branch tip, and a shadow that loses them reads as the
    /// wrong object. Also caps the count, since this runs per prop per rebake.
    #[test]
    fn shadow_hull_keeps_the_extremes_and_stays_bounded() {
        // A tall spike among low noise — exactly the case a plain stride misses.
        let stride = crate::model::MODEL_VERTEX_FLOATS;
        let mut verts = Vec::new();
        for i in 0..500 {
            let x = (i % 10) as f32 * 0.1;
            let z = (i / 10) as f32 * 0.1;
            verts.extend_from_slice(&[x, 0.0, z, 0.0, 0.0, 0.0]);
        }
        // The spike sits at an index a stride of 10 would skip.
        let spike = 253;
        verts[spike * stride + 1] = 9.0;

        let pts = shadow_hull_points(&verts, stride);
        assert!(pts.len() <= 64, "unbounded hull: {}", pts.len());
        assert!(
            pts.iter().any(|p| p.y > 8.9),
            "dropped the tallest point, so the silhouette is wrong"
        );
        // And the ground-plane corners survive, or the footprint shrinks.
        assert!(pts.iter().any(|p| p.x > 0.85));
        assert!(pts.iter().any(|p| p.z > 4.8));
    }

    #[test]
    fn shadow_hull_handles_an_empty_mesh() {
        assert!(shadow_hull_points(&[], crate::model::MODEL_VERTEX_FLOATS).is_empty());
    }
}
