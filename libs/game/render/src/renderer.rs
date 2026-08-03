//! The world scene pass, moved verbatim from gamemaker's game_view.rs:
//! sky dome → terrain mesh → opaque per-shape batches → alpha per-shape
//! batches. Statics come from packed instance slabs cached against
//! `world.render_rev`; dynamics re-pack every frame.

use makepad_draw::*;
use makepad_game_sim::{entity_index_sorted, BodyKind, Entity, GameWorld, Shape, Terrain};

use crate::geometry::shape_geometry_data;
use crate::shaders::{DrawGameAlpha, DrawGameCube, DrawGameSkinned, DrawGameSky, DrawGameTerrain};
use crate::stage::Stage;

/// The host widget's themed draw structs, lent to the renderer per frame.
/// They stay `#[live]` fields on the widget so script-side styling applies.
pub struct GameDraws<'a> {
    pub cube: &'a mut DrawGameCube,
    pub alpha: &'a mut DrawGameAlpha,
    pub sky: &'a mut DrawGameSky,
    pub terrain: &'a mut DrawGameTerrain,
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
}

/// GPU-side caches for one view family: unit shape geometries, the packed
/// static instance slabs, and the terrain mesh. Owns no draw structs — see
/// [`GameDraws`].
#[derive(Default)]
pub struct GameRenderer {
    // PERF: unit shape geometries, built once (index = Shape::index()).
    shape_geometries: [Option<Geometry>; 5],
    // PERF: packed static instance data per shape (opaque / alpha passes),
    // valid while slab_rev == world.render_rev.
    static_slab: [Vec<f32>; 5],
    static_slab_alpha: [Vec<f32>; 5],
    slab_rev: Option<u64>,
    slab_instance_count: u64,
    /// GPU mesh for the smooth terrain, rebuilt when the revision changes.
    terrain_geometry: Option<Geometry>,
    terrain_revision: u64,
    /// GPU meshes for CPU-skinned characters, keyed by caller id, re-uploaded
    /// every frame (the skinning happens CPU-side; see skin.rs).
    skinned_geometries: Vec<(u64, Geometry)>,
    /// How this device projects the world (flat / VR 1:1 / MR diorama).
    /// Applied as the scene draw list's view transform, so it costs one
    /// uniform and never invalidates the static slabs. See stage.rs.
    stage: Stage,
}

/// One CPU-skinned mesh instance for [`GameRenderer::draw_scene_full`].
/// `vertices` is the PbrVertex float layout `SkinnedModel::skin_to_pbr` emits.
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

fn perf_us(t0: std::time::Instant) -> u64 {
    t0.elapsed().as_micros() as u64
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
        for e in world.entities.iter().filter(|e| e.kind == BodyKind::Static) {
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
            self.pack_cube_instance(draws, false, p.shape.index(), transform, size, p.color, p.glow);
        }
    }

    /// Upload + draw the skinned batch inside the already-open scene pass.
    fn draw_skinned_inner(&mut self, cx: &mut Cx3d, batch: SkinnedBatch, fog: (Vec3f, f32)) {
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
        self.draw_scene_full(cx, draw_list, draws, world, scene_state, None)
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
        draws.cube.fog_color = fog_color;
        draws.cube.fog_density = fog_density;
        draws.alpha.cube.fog_color = fog_color;
        draws.alpha.cube.fog_density = fog_density;

        let vars_ready = draws.cube.cube.draw_vars.can_instance()
            && draws.alpha.cube.cube.draw_vars.can_instance();
        if vars_ready && self.slab_rev != Some(world.render_rev) {
            let t0 = std::time::Instant::now();
            self.rebuild_static_slabs(draws, world);
            stats.slab_us += perf_us(t0);
            stats.slab_rebuilds += 1;
            self.slab_rev = Some(world.render_rev);
        }
        stats.static_instances = self.slab_instance_count;
        stats.dyn_instances = 0;

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
        for e in world.entities.iter().filter(|e| e.kind != BodyKind::Static) {
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
                .filter(|e| !e.sensor && e.kind != BodyKind::Static && e.shape == shape)
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
                draws.cube.cube.color = e.color;
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
                draws.cube.cube.color = part.color;
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
            self.draw_skinned_inner(cx, batch, (fog_color, fog_density));
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
                        && e.attached_to == 0
                });
            if !has_static && !has_dynamic_sensor && !has_shadows {
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
                for e in world.entities.iter().filter(|e| {
                    matches!(e.kind, BodyKind::Mover | BodyKind::Rigid)
                        && !e.sensor
                        && e.attached_to == 0
                }) {
                    // Ground under the mover: terrain, or the tallest static
                    // box top.
                    let mut ground: Option<f32> = world
                        .terrain
                        .as_ref()
                        .and_then(|t| t.floor_under(e.pos, e.half));
                    let feet = e.pos.y - e.half.y;
                    for s in world.entities.iter() {
                        if s.sensor
                            || !matches!(s.kind, BodyKind::Static | BodyKind::Kinematic)
                        {
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
                    let Some(ground) = ground else { continue };
                    let drop = feet - ground;
                    if !(0.0..8.0).contains(&drop) {
                        continue;
                    }
                    let fade = (1.0 - drop / 8.0) * 0.35;
                    let mut transform = Mat4f::identity();
                    transform.v[12] = e.pos.x;
                    transform.v[13] = ground + 0.03;
                    transform.v[14] = e.pos.z;
                    draws.alpha.cube.cube.transform = transform;
                    draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    draws.alpha.cube.cube.cube_size = vec3(
                        e.half.x * 2.2 * e.scale.x,
                        0.02,
                        e.half.z * 2.2 * e.scale.z,
                    );
                    draws.alpha.cube.cube.color = vec4(0.02, 0.02, 0.05, fade);
                    draws.alpha.cube.cube.depth_clip = 1.0;
                    draws.alpha.cube.glow = 0.0;
                    draws.alpha.cube.cube.draw(cx);
                    stats.dyn_instances += 1;
                }
            }
            for e in world
                .entities
                .iter()
                .filter(|e| e.sensor && e.kind != BodyKind::Static && e.shape == shape)
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
