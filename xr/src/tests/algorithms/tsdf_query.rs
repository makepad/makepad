mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Arc};

    fn set_normalized_distance(
        chunks: &mut HashMap<ChunkKey, Arc<SparseTsdReadChunk>>,
        chunk_edge: i32,
        coord: TsdfVoxelCoord,
        normalized_distance: f32,
    ) {
        let cx = coord.x.div_euclid(chunk_edge);
        let cy = coord.y.div_euclid(chunk_edge);
        let cz = coord.z.div_euclid(chunk_edge);
        let lx = coord.x.rem_euclid(chunk_edge) as usize;
        let ly = coord.y.rem_euclid(chunk_edge) as usize;
        let lz = coord.z.rem_euclid(chunk_edge) as usize;
        let edge = chunk_edge as usize;
        let id = lx + ly * edge + lz * edge * edge;
        let chunk = Arc::make_mut(
            chunks
                .entry(ChunkKey::new(cx, cy, cz))
                .or_insert_with(|| Arc::new(SparseTsdReadChunk::new(edge * edge * edge))),
        );
        chunk.set_value(id, normalized_distance, 8, 1);
    }

    fn make_flat_floor_snapshot(voxel_size: f32) -> TsdfPublishedSnapshot {
        let chunk_edge = 8;
        let mut chunks = HashMap::new();
        let tsd_distance_meters = depth_tsd_distance_meters(voxel_size);
        let mut active_value_count = 0usize;
        for z in -6..=6 {
            for y in -6..=6 {
                for x in -6..=6 {
                    let world_y = voxel_center_axis(voxel_size, y);
                    let normalized = (world_y / tsd_distance_meters).clamp(-1.0, 1.0);
                    set_normalized_distance(
                        &mut chunks,
                        chunk_edge,
                        TsdfVoxelCoord::new(x, y, z),
                        normalized,
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

    #[test]
    fn direct_tsdf_query_finds_flat_floor_support() {
        let snapshot = make_flat_floor_snapshot(0.05);
        let result = evaluate_tsdf_query(
            &snapshot,
            DepthQuery {
                key: 1,
                center: vec3f(0.0, 0.09, 0.0),
                predicted_center: vec3f(0.0, 0.07, 0.0),
                velocity: vec3f(0.0, -0.2, 0.0),
                radius: 0.05,
                max_distance: 0.12,
            },
        );

        match result {
            DepthQueryResult::Hit(hit) => {
                let DepthQueryColliderGeometry::HalfSpace(plane) = hit.collider.geometry;
                assert!(
                    plane.normal.y >= 0.98,
                    "expected floor support plane, got {plane:?}"
                );
                assert!(
                    plane.point.y.abs() <= 0.03,
                    "expected plane near y=0, got {:?}",
                    plane.point
                );
            }
            DepthQueryResult::Miss { .. } => panic!("expected flat-floor TSDF query hit"),
        }
    }
}
