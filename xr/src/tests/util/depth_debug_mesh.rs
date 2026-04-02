fn depth_tsd_distance_meters(voxel_size_meters: f32) -> f32 {
    voxel_size_meters * 2.0
}

mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Arc};

    fn packed_position(vertices: &[f32], vertex_index: usize) -> Vec3f {
        let base = vertex_index * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX;
        vec3f(vertices[base], vertices[base + 1], vertices[base + 2])
    }

    fn packed_barycentric(vertices: &[f32], vertex_index: usize) -> Vec3f {
        let base = vertex_index * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX;
        vec3f(vertices[base + 4], vertices[base + 5], vertices[base + 6])
    }

    fn set_normalized_distance(
        chunks: &mut HashMap<ChunkKey, Arc<SparseTsdReadChunk>>,
        chunk_edge: i32,
        coord: VoxelCoord,
        normalized_distance: f32,
        confidence: u8,
    ) {
        let chunk_key = ChunkKey::new(
            coord.x.div_euclid(chunk_edge),
            coord.y.div_euclid(chunk_edge),
            coord.z.div_euclid(chunk_edge),
        );
        let lx = coord.x.rem_euclid(chunk_edge) as usize;
        let ly = coord.y.rem_euclid(chunk_edge) as usize;
        let lz = coord.z.rem_euclid(chunk_edge) as usize;
        let edge = chunk_edge as usize;
        let id = lx + ly * edge + lz * edge * edge;

        let chunk = Arc::make_mut(
            chunks
                .entry(chunk_key)
                .or_insert_with(|| Arc::new(SparseTsdReadChunk::new(edge * edge * edge))),
        );
        chunk.set_value(id, normalized_distance, confidence, 1);
    }

    fn make_flat_floor_snapshot_with_confidence(
        voxel_size: f32,
        confidence: u8,
    ) -> TsdfPublishedSnapshot {
        let chunk_edge = 8;
        let mut chunks = HashMap::new();
        let tsd_distance_meters = depth_tsd_distance_meters(voxel_size);
        let mut active_value_count = 0usize;
        for z in -6..=6 {
            for y in -6..=6 {
                for x in -6..=6 {
                    let world_y = (y as f32 + 0.5) * voxel_size;
                    let normalized = (world_y / tsd_distance_meters).clamp(-1.0, 1.0);
                    set_normalized_distance(
                        &mut chunks,
                        chunk_edge,
                        VoxelCoord::new(x, y, z),
                        normalized,
                        confidence,
                    );
                    active_value_count += 1;
                }
            }
        }
        TsdfPublishedSnapshot {
            generation: 1,
            latest_topology_generation: 1,
            update_sequence: 1,
            grid: Arc::new(SparseTsdGridReadSnapshot {
                voxel_size,
                chunk_edge,
                chunk_edge_shift: Some(chunk_edge.trailing_zeros() as u8),
                chunk_edge_mask: chunk_edge - 1,
                chunk_volume: (chunk_edge as usize).pow(3),
                active_value_count,
                active_bounds: Some((
                    vec3f(-6.0 * voxel_size, -6.0 * voxel_size, -6.0 * voxel_size),
                    vec3f(7.0 * voxel_size, 7.0 * voxel_size, 7.0 * voxel_size),
                )),
                chunks,
            }),
            height_map: None,
        }
    }

    fn make_flat_floor_snapshot(voxel_size: f32) -> TsdfPublishedSnapshot {
        make_flat_floor_snapshot_with_confidence(voxel_size, 8)
    }

    #[test]
    fn pack_surface_mesh_debug_vertices_expands_to_triangle_barycentrics() {
        let mesh = SurfaceMesh32 {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
        };

        let (indices, vertices) = pack_surface_mesh_debug_vertices(&mesh);

        assert_eq!(indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(vertices.len(), 6 * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX);
        assert_eq!(packed_position(&vertices, 0), vec3f(0.0, 0.0, 0.0));
        assert_eq!(packed_position(&vertices, 2), vec3f(1.0, 1.0, 0.0));
        assert_eq!(packed_position(&vertices, 5), vec3f(0.0, 1.0, 0.0));
        assert_eq!(packed_barycentric(&vertices, 0), vec3f(1.0, 0.0, 0.0));
        assert_eq!(packed_barycentric(&vertices, 1), vec3f(0.0, 1.0, 0.0));
        assert_eq!(packed_barycentric(&vertices, 2), vec3f(0.0, 0.0, 1.0));
    }

    #[test]
    fn push_debug_depth_plane_emits_single_quad_mesh() {
        let plane = DepthQuerySupportPlane {
            point: vec3f(0.0, 0.5, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            tangent: vec3f(1.0, 0.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.25,
            half_extent_bitangent: 0.10,
        };
        let mut indices = Vec::new();
        let mut vertices = Vec::new();

        push_debug_depth_plane(&mut indices, &mut vertices, plane);

        assert_eq!(indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(vertices.len(), 6 * XR_DEBUG_DEPTH_FLOATS_PER_VERTEX);
        let first = packed_position(&vertices, 0);
        let sixth = packed_position(&vertices, 5);
        assert_eq!(first, vec3f(-0.25, 0.5, -0.10));
        assert_eq!(sixth, vec3f(-0.25, 0.5, 0.10));
        assert_eq!(packed_barycentric(&vertices, 3), vec3f(1.0, 0.0, 0.0));
        assert_eq!(packed_barycentric(&vertices, 4), vec3f(0.0, 1.0, 0.0));
        assert_eq!(packed_barycentric(&vertices, 5), vec3f(0.0, 0.0, 1.0));
    }

    #[test]
    fn debug_depth_mesh_view_plan_extracts_visible_chunk_meshes() {
        let snapshot = make_flat_floor_snapshot(0.05);
        let view_plan =
            debug_depth_mesh_view_plan(&snapshot, Pose::new(Quat::default(), vec3f(0.0, 1.4, 0.0)));

        assert!(
            !view_plan.visible_chunks.is_empty(),
            "expected at least one visible debug chunk"
        );

        let mut triangulator = DebugDepthMeshTriangulator::default();
        let built_chunks: Vec<_> = view_plan
            .visible_chunks
            .iter()
            .filter_map(|plan| triangulator.build_chunk(&snapshot, view_plan.layout, plan))
            .collect();

        assert!(
            !built_chunks.is_empty(),
            "expected triangulated visible chunks for a flat floor snapshot"
        );
        assert!(built_chunks
            .iter()
            .all(|chunk| !chunk.indices.is_empty() && !chunk.vertices.is_empty()));
    }

    #[test]
    fn debug_depth_mesh_includes_low_confidence_valid_voxels() {
        let snapshot = make_flat_floor_snapshot_with_confidence(0.05, 1);
        let view_plan =
            debug_depth_mesh_view_plan(&snapshot, Pose::new(Quat::default(), vec3f(0.0, 1.4, 0.0)));

        let mut triangulator = DebugDepthMeshTriangulator::default();
        let built_chunks: Vec<_> = view_plan
            .visible_chunks
            .iter()
            .filter_map(|plan| triangulator.build_chunk(&snapshot, view_plan.layout, plan))
            .collect();

        assert!(
            !built_chunks.is_empty(),
            "expected triangulated visible chunks even from low-confidence valid TSDF voxels"
        );
    }

    #[test]
    fn debug_depth_mesh_focus_cube_plan_limits_chunks_to_focus_volume() {
        let snapshot = make_flat_floor_snapshot(0.05);
        let focus_center = vec3f(0.12, 0.0, -0.08);
        let focus_plan = debug_depth_mesh_focus_cube_plan(&snapshot, focus_center, 1.0);

        assert!(
            !focus_plan.visible_chunks.is_empty(),
            "expected at least one visible chunk in the focus cube plan"
        );

        let cube_half_extent = vec3f(0.5, 0.5, 0.5);
        let cube_min = focus_center - cube_half_extent;
        let cube_max = focus_center + cube_half_extent;
        for chunk in &focus_plan.visible_chunks {
            let (world_min, world_max) = mesh_chunk_world_bounds(
                snapshot.grid.voxel_size,
                chunk.chunk_key,
                focus_plan.layout,
            );
            assert!(
                aabb_intersects(world_min, world_max, cube_min, cube_max),
                "focus chunk {:?} should intersect the 1m focus cube",
                chunk.chunk_key
            );
        }

        let head_plan =
            debug_depth_mesh_view_plan(&snapshot, Pose::new(Quat::default(), vec3f(0.0, 1.4, 0.0)));
        assert!(
            focus_plan.visible_chunks.len() <= head_plan.visible_chunks.len(),
            "focus cube mode should not expand the visible chunk set beyond the head-view plan"
        );
    }

    #[test]
    fn snapshot_debug_mesh_layout_targets_thirty_two_centimeter_chunks() {
        let snapshot = make_flat_floor_snapshot(0.02);
        let layout = snapshot_debug_mesh_layout(&snapshot);

        assert_eq!(layout.chunk_edge_voxels, 16);
        assert!((layout.chunk_world_size_meters - 0.32).abs() <= 1.0e-6);
    }
}
