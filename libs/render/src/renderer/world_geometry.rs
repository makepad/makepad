//! Shadow casters, terrain/voxel tiles, shape geometry and static slabs.

use super::*;

impl Renderer {
    /// Ground height under a caster: the terrain, or the tallest static box
    /// top it stands over. `None` when it is over a hole.
    pub(super) fn ground_under(world: &World, e: &Entity) -> Option<f32> {
        let mut ground: Option<f32> = world
            .terrain
            .as_ref()
            .and_then(|t| t.floor_under(e.pos, e.half));
        let feet = e.pos.y - e.half.y;
        for s in world.entities.iter() {
            if s.alpha_primitive || s.hidden || !matches!(s.kind, BodyKind::Static | BodyKind::Kinematic) {
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
    pub(super) fn shadow_casters<'w>(
        world: &'w World,
        camera_pos: Vec3f,
        budget: usize,
    ) -> Vec<(&'w Entity, f32, bool)> {
        const HERO_HEIGHT: f32 = 0.5;
        let mut heroes: Vec<(&Entity, f32, f32)> = Vec::new();
        let mut small: Vec<(&Entity, f32)> = Vec::new();
        for e in world.entities.iter() {
            if !matches!(e.kind, BodyKind::Mover | BodyKind::Rigid)
                || e.alpha_primitive
                || e.hidden
                || e.parent != 0
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
    pub(super) fn world_footprint(world: &World) -> Option<(Vec3f, f32)> {
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
        if let Some(t) = world.terrain.as_deref() {
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

    /// Rebuild the terrain GPU tiles when the world's terrain revision moved.
    /// Godot-style triangles (terrain_tile_data), regrouped into CHUNK_SIZE
    /// tiles so an offscreen stretch of hills skips its draw item — the same
    /// primitives as the old single mesh, just delivered in pieces.
    pub(super) fn ensure_terrain_tiles(
        &mut self,
        cx: &mut Cx,
        terrain: &Terrain,
        materials: Option<&TerrainMaterials>,
    ) {
        if !self.terrain_tiles.is_empty() && self.terrain_revision == terrain.revision {
            return;
        }
        self.terrain_tiles.clear();
        let n = terrain.cells;
        let cells_per_tile = ((CHUNK_SIZE / terrain.cell_size.max(1.0e-6)) as usize).max(1);
        let mut gz0 = 0;
        while gz0 < n - 1 {
            let gz1 = (gz0 + cells_per_tile).min(n - 1);
            let mut gx0 = 0;
            while gx0 < n - 1 {
                let gx1 = (gx0 + cells_per_tile).min(n - 1);
                let (vertices, indices, min, max) =
                    terrain_tile_data(terrain, materials, gx0, gx1, gz0, gz1);
                if !indices.is_empty() {
                    let geometry = Geometry::new(cx);
                    geometry.update(cx, indices, vertices);
                    self.terrain_tiles.push(TerrainTile { min, max, geometry, shadow_geometry: None });
                }
                gx0 = gx1;
            }
            gz0 = gz1;
        }
        self.terrain_revision = terrain.revision;
    }

    /// Mirror the voxel field's chunk meshes into GPU geometries: a merge
    /// over two sorted sequences, re-uploading only chunks whose mesh
    /// revision moved (a dig re-uploads its own chunks, nothing else).
    pub(super) fn ensure_voxel_tiles(&mut self, cx: &mut Cx, voxel: Option<&VoxelView>) {
        let empty = std::collections::BTreeMap::new();
        let meshes = voxel.map_or(&empty, |v| &v.meshes);
        if self.voxel_tiles.is_empty() && meshes.is_empty() {
            return;
        }
        let mut old: std::collections::BTreeMap<ChunkKey, VoxelTile> = std::mem::take(&mut self.voxel_tiles)
            .into_iter()
            .map(|t| (t.key, t))
            .collect();
        let mut out = Vec::with_capacity(meshes.len());
        for (key, mesh) in meshes {
            match old.remove(key) {
                Some(tile) if tile.rev == mesh.rev => out.push(tile),
                _ => {
                    let geometry = Geometry::new(cx);
                    geometry.update(cx, mesh.indices.clone(), mesh.verts.clone());
                    out.push(VoxelTile {
                        key: *key,
                        rev: mesh.rev,
                        min: mesh.min,
                        max: mesh.max,
                        geometry,
                        shadow_geometry: None,
                    });
                }
            }
        }
        // Whatever is left in `old` lost its chunk; the Geometry drops.
        self.voxel_tiles = out;
    }

    /// Unit geometry for a shape, built once and shared by every instance
    /// (index = Shape::index()). All shapes span [-0.5, 0.5] so `cube_size`
    /// scales them exactly like the built-in cube.
    pub(super) fn ensure_shape_geometry(&mut self, cx: &mut Cx, shape: Shape) -> GeometryId {
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

    /// Inward-wound twin of [`Self::ensure_shape_geometry`]. With the shared
    /// cube shader's ordinary back-face culling this draws only the room side
    /// of an interior shell and disappears when the eye sits outside it.

    /// Rotation part of an entity's transform. Rigids carry a full box3d
    /// orientation quat (M1a); everything else rotates by visual yaw exactly
    /// as before. Column-major, same layout as Mat4f::rotation.
    pub fn rigid_transform(e: &Entity) -> Mat4f {
        let mut m = Self::entity_rotation(e);
        m.v[12] = e.pos.x;
        m.v[13] = e.pos.y;
        m.v[14] = e.pos.z;
        m
    }

    pub(super) fn entity_rotation(e: &Entity) -> Mat4f {
        // The producer resolves an optional local +Z display direction.
        if let Some(f) = e.forward_axis {
            let up_hint = if f.y.abs() > 0.99 {
                vec3f(1.0, 0.0, 0.0)
            } else {
                vec3f(0.0, 1.0, 0.0)
            };
            let r = Vec3f::cross(up_hint, f).normalize();
            let u = Vec3f::cross(f, r);
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
            return m;
        }
        // A rigid body's orientation comes from box3d; a kinematic RIDE body
        // (a coaster car halfway round a loop) writes its own. Either way a
        // set quaternion is the pose — only an unset one falls back to yaw.
        if e.kind == BodyKind::Rigid || e.orient != Quat::default() {
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

    /// Compose one primitive attachment from its owner's live transform and
    /// its fixed owner-local pose. Movers use this same path every frame, so
    /// translating/turning the body carries every part without simulation or
    /// script writes. Procedural gait is a rotation overlay only.
    pub(super) fn part_transform(owner: &Entity, part: &Part) -> Mat4f {
        let mut owner_frame = Self::entity_rotation(owner);
        owner_frame.v[12] = owner.pos.x;
        owner_frame.v[13] = owner.pos.y;
        owner_frame.v[14] = owner.pos.z;
        let mut local = Mat4f::rotation(part.rot);
        local.v[12] = part.offset.x * owner.scale.x;
        local.v[13] = part.offset.y * owner.scale.y;
        local.v[14] = part.offset.z * owner.scale.z;
        let placed = Mat4f::mul(&owner_frame, &local);
        match &part.follow {
            Some(follow) => Mat4f::mul(follow, &placed),
            None => placed,
        }
    }

    /// PERF: pack one instance in the exact slice layout `DrawCube::draw`
    /// emits (DrawVars::as_slice covers the trailing glow/fog instance
    /// fields), so slab content and immediate draws are indistinguishable.
    /// Instances land in the world-grid chunk under their centre; the
    /// chunk's content bounds grow by the rotation-invariant half-diagonal,
    /// so any orientation of the box stays inside them.
    pub(super) fn pack_cube_instance(
        &mut self,
        draws: &mut SceneDraws,
        bucket: PrimitiveBucket,
        out_index: usize,
        transform: Mat4f,
        size: Vec3f,
        color: Vec4f,
        glow: f32,
        color_adjust: Vec4f,
    ) {
        let center = vec3f(transform.v[12], transform.v[13], transform.v[14]);
        let r = size.length() * 0.5;
        let cell = chunk_cell(center.x, center.z);
        let at = match self.static_chunks.iter().position(|c| c.cell == cell) {
            Some(at) => at,
            None => {
                self.static_chunks.push(SlabChunk {
                    cell,
                    min: vec3f(f32::MAX, f32::MAX, f32::MAX),
                    max: vec3f(f32::MIN, f32::MIN, f32::MIN),
                    slab: Default::default(),
                    slab_alpha: Default::default(),
                });
                self.static_chunks.len() - 1
            }
        };
        let chunk = &mut self.static_chunks[at];
        chunk.min = vec3f(
            chunk.min.x.min(center.x - r),
            chunk.min.y.min(center.y - r),
            chunk.min.z.min(center.z - r),
        );
        chunk.max = vec3f(
            chunk.max.x.max(center.x + r),
            chunk.max.y.max(center.y + r),
            chunk.max.z.max(center.z + r),
        );
        if bucket == PrimitiveBucket::Alpha {
            draws.alpha.cube.cube.transform = transform;
            draws.alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
            draws.alpha.cube.cube.cube_size = size;
            draws.alpha.cube.cube.color = color;
            draws.alpha.cube.cube.depth_clip = 1.0;
            draws.alpha.cube.glow = glow;
            draws.alpha.cube.color_adjust_ctl = color_adjust;
            let slice = draws.alpha.cube.cube.draw_vars.as_slice();
            chunk.slab_alpha[out_index].extend_from_slice(slice);
            self.slab_instance_count += 1;
        } else {
            draws.cube.cube.transform = transform;
            draws.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
            draws.cube.cube.cube_size = size;
            draws.cube.cube.color = color;
            draws.cube.cube.depth_clip = 1.0;
            draws.cube.glow = glow;
            draws.cube.color_adjust_ctl = color_adjust;
            let slice = draws.cube.cube.draw_vars.as_slice();
            match bucket {
                PrimitiveBucket::Opaque => chunk.slab[out_index].extend_from_slice(slice),
                PrimitiveBucket::Alpha => unreachable!(),
            }
            self.slab_instance_count += 1;
        }
    }

    /// PERF: rebuild the packed static instance slabs. Only runs when
    /// `world.render_rev` moved — the world bumps it on every mutation that
    /// changes what static content looks like (see mark_render_dirty).
    pub(super) fn rebuild_static_slabs(&mut self, draws: &mut SceneDraws, world: &World) {
        // Chunks are dropped, not cleared: a vacated cell must not linger as
        // an empty entry the per-frame loops keep testing. Rebuilds run at
        // edit cadence, so the reallocation is not a per-frame cost.
        self.static_chunks.clear();
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
            if e.alpha_primitive && color.w >= 0.99 {
                color.w = 0.35;
            }
            // Baked occlusion rides in the colour we were already sending —
            // no extra instance field, no shader work, no draw call.
            color = shade_color(color, self.bake.static_shade(e.id), e.glow);
            let Some(bucket) = primitive_bucket(e) else { continue };
            self.pack_cube_instance(
                draws,
                bucket,
                e.shape.index(),
                transform,
                size,
                color,
                e.glow,
                e.color_adjust.instance(),
            );
        }
        // Settled parts of static owners.
        for p in world
            .parts
            .iter()
            .filter(|p| !p.animated)
        {
            // Entity ids are spawn-ordered, so the list stays sorted; the
            // shared sim helper owns (and debug-asserts) that invariant.
            let Some(owner) = entity_index_sorted(&world.entities, p.owner)
                .map(|i| &world.entities[i])
                .filter(|e| e.kind == BodyKind::Static)
            else {
                continue;
            };
            let transform = Self::part_transform(owner, p);
            let size = vec3(
                p.half.x * 2.0 * owner.scale.x,
                p.half.y * 2.0 * owner.scale.y,
                p.half.z * 2.0 * owner.scale.z,
            );
            // A part inherits its owner's bake — it is bolted to it.
            let color = shade_color(p.color, self.bake.static_shade(owner.id), p.glow);
            self.pack_cube_instance(
                draws,
                PrimitiveBucket::Opaque,
                p.shape.index(),
                transform,
                size,
                color,
                p.glow,
                owner.color_adjust.instance(),
            );
        }
    }

    /// Refresh the receiver/occluder box sets after the world settles. The
    /// receiver boxes place every dynamic shadow (SDF quad landing heights,
    /// blob drapes, character ground samples); the occluder boxes feed the
    /// GPU lightmap's depth passes.
    ///
    /// This is all that remains of the old static shadow-mesh rebuild: sun
    /// shadows for statics live in the baked LIGHTMAP, ambient grounding in
    /// each model's own AO atlas — the merged silhouette geometry this used
    /// to build (30ms per world settle) drew nothing but artifacts on top
    /// of them and was deleted.
    pub(super) fn refresh_shadow_receivers(&mut self, world: &World) {
        // Tops that can catch a draped shadow: visible, solid static boxes —
        // road slabs, platforms, crate lids. TALL hidden colliders stay
        // excluded (their tops are the invisible lids the roof surfaces
        // replaced; draping onto those floats shadows mid-air), but THIN
        // hidden slabs are floor stand-ins for visible model floors — a
        // generated dungeon's tiles. Without them shadows draped onto the
        // terrain UNDER the floors and surfaced coplanar with the visible
        // ground: the arena's floor-wide z-fighting sheets.
        self.occluder_boxes = world
            .entities
            .iter()
            .filter(|e| {
                e.kind == BodyKind::Static
                    && !e.alpha_primitive
                    && !e.hidden
                    && e.shape == Shape::Box
                    && e.color.w >= 0.99
            })
            .map(|e| {
                let h = vec3f(
                    e.half.x * e.scale.x,
                    e.half.y * e.scale.y,
                    e.half.z * e.scale.z,
                );
                (
                    vec3f(e.pos.x - h.x, e.pos.y - h.y, e.pos.z - h.z),
                    vec3f(e.pos.x + h.x, e.pos.y + h.y, e.pos.z + h.z),
                )
            })
            .collect();
        self.receiver_boxes = world
            .entities
            .iter()
            .filter(|e| {
                // Receivers: what a draped shadow may land on. Visible solid
                // statics, PLUS the mesh voxel boxes that are genuinely
                // FLOORS — wide flat slabs (a generated dungeon's walkable
                // surface lives in the mesh at its true height; the old
                // mask stand-ins sat 10cm below the drawn floor and every
                // draped shadow was buried under it). Curbs, decks and
                // braces stay excluded: wide+flat only.
                let h = (
                    e.half.x * e.scale.x,
                    e.half.y * e.scale.y,
                    e.half.z * e.scale.z,
                );
                let flat_slab = h.1 <= 0.25 && h.0 >= 0.8 && h.2 >= 0.8;
                e.kind == BodyKind::Static
                    && !e.alpha_primitive
                    && e.shape == Shape::Box
                    && (!e.hidden || flat_slab)
            })
            .map(|e| {
                let h = vec3f(
                    e.half.x * e.scale.x,
                    e.half.y * e.scale.y,
                    e.half.z * e.scale.z,
                );
                (
                    vec3f(e.pos.x - h.x, e.pos.y - h.y, e.pos.z - h.z),
                    vec3f(e.pos.x + h.x, e.pos.y + h.y, e.pos.z + h.z),
                )
            })
            .collect();
    }
}
