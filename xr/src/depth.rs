use super::*;
use super::physics::{makepad_pose, RapierScene};
use std::{
    collections::{hash_map::DefaultHasher, HashSet},
    hash::{Hash, Hasher},
};

#[derive(Clone, Copy)]
pub(super) struct DepthSurfaceMeshChunkHandle {
    geometry_id: GeometryId,
    fingerprint: u64,
}

#[derive(Clone, Copy)]
pub(super) struct DepthQuerySurfaceCollider {
    pub(super) collider: ColliderHandle,
    pub(super) fingerprint: u64,
}

#[derive(Clone, Copy)]
pub(super) enum DepthQuerySurfaceShape {
    Triangle([Vec3f; 3]),
    Quad([Vec3f; 4]),
}

#[derive(Clone, Copy)]
pub(super) struct DepthQuerySurfaceTarget {
    pub(super) shape: DepthQuerySurfaceShape,
    pub(super) fingerprint: u64,
}

#[derive(Clone, Copy)]
struct DepthQuerySurfaceCandidate {
    key: u64,
    distance: f32,
    shape: DepthQuerySurfaceShape,
    fingerprint: u64,
}

#[derive(Clone)]
pub(super) struct RetainedDepthQueryHit {
    hit: XrDepthMeshQueryHit,
    misses_left: u8,
}

impl RetainedDepthQueryHit {
    fn new(hit: XrDepthMeshQueryHit) -> Self {
        Self {
            hit,
            misses_left: XR_DEPTH_QUERY_HIT_MISS_GRACE_FRAMES,
        }
    }

    fn reuse_result(&mut self) -> Option<XrDepthMeshQueryResult> {
        if self.misses_left == 0 {
            return None;
        }
        self.misses_left -= 1;
        Some(XrDepthMeshQueryResult::Hit(self.hit.clone()))
    }
}

fn quantize_depth_query_value(value: f32) -> i32 {
    (value / XR_DEPTH_QUERY_FINGERPRINT_QUANTIZATION_METERS).round() as i32
}

fn depth_query_triangle_fingerprint(triangle: [Vec3f; 3]) -> u64 {
    let mut vertices = triangle.map(|vertex| {
        [
            quantize_depth_query_value(vertex.x),
            quantize_depth_query_value(vertex.y),
            quantize_depth_query_value(vertex.z),
        ]
    });
    vertices.sort_unstable();
    let mut hasher = DefaultHasher::new();
    vertices.hash(&mut hasher);
    hasher.finish()
}

fn depth_query_quad_fingerprint(quad: [Vec3f; 4]) -> u64 {
    let mut vertices = quad.map(|vertex| {
        [
            quantize_depth_query_value(vertex.x),
            quantize_depth_query_value(vertex.y),
            quantize_depth_query_value(vertex.z),
        ]
    });
    vertices.sort_unstable();
    let mut hasher = DefaultHasher::new();
    vertices.hash(&mut hasher);
    hasher.finish()
}

fn depth_query_patch_is_degenerate(patch: [Vec3f; 4]) -> bool {
    let epsilon = 1.0e-4;
    (patch[1] - patch[0]).length() <= epsilon
        && (patch[2] - patch[0]).length() <= epsilon
        && (patch[3] - patch[0]).length() <= epsilon
}

fn depth_query_surface_candidate(
    key: u64,
    distance: f32,
    from_planar_patch: bool,
    triangle: [Vec3f; 3],
    patch: [Vec3f; 4],
) -> DepthQuerySurfaceCandidate {
    let (shape, fingerprint) = if from_planar_patch && !depth_query_patch_is_degenerate(patch) {
        (
            DepthQuerySurfaceShape::Quad(patch),
            depth_query_quad_fingerprint(patch),
        )
    } else {
        (
            DepthQuerySurfaceShape::Triangle(triangle),
            depth_query_triangle_fingerprint(triangle),
        )
    };
    DepthQuerySurfaceCandidate {
        key,
        distance,
        shape,
        fingerprint,
    }
}

fn extend_depth_query_surface_candidates(
    candidates: &mut Vec<DepthQuerySurfaceCandidate>,
    hit: &XrDepthMeshQueryHit,
) {
    candidates.push(depth_query_surface_candidate(
        hit.key,
        hit.distance,
        hit.from_planar_patch,
        hit.triangle,
        hit.patch,
    ));
    candidates.extend(hit.additional_hits.iter().map(|extra| {
        depth_query_surface_candidate(
            hit.key,
            extra.distance,
            extra.from_planar_patch,
            extra.triangle,
            extra.patch,
        )
    }));
}

fn build_depth_query_surface_targets(
    results: &[XrDepthMeshQueryResult],
) -> Vec<DepthQuerySurfaceTarget> {
    let mut hits = Vec::new();
    for result in results {
        if let XrDepthMeshQueryResult::Hit(hit) = result {
            extend_depth_query_surface_candidates(&mut hits, hit);
        }
    }
    hits.sort_by(|a, b| {
        matches!(b.shape, DepthQuerySurfaceShape::Quad(_))
            .cmp(&matches!(a.shape, DepthQuerySurfaceShape::Quad(_)))
            .then_with(|| {
                a.distance
                    .partial_cmp(&b.distance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.key.cmp(&b.key))
    });

    let mut seen = HashSet::new();
    let mut targets = Vec::with_capacity(XR_DEPTH_QUERY_SHARED_SURFACE_POOL_SIZE);
    for hit in hits {
        if !seen.insert(hit.fingerprint) {
            continue;
        }
        targets.push(DepthQuerySurfaceTarget {
            shape: hit.shape,
            fingerprint: hit.fingerprint,
        });
        if targets.len() >= XR_DEPTH_QUERY_SHARED_SURFACE_POOL_SIZE {
            break;
        }
    }
    targets
}

fn pack_depth_mesh_vertices(chunk: &XrDepthMeshChunk) -> Vec<f32> {
    const FLOATS_PER_VERTEX: usize = 16;
    let mut vertices = Vec::with_capacity(chunk.vertices.len() * FLOATS_PER_VERTEX);
    for (position, normal) in chunk.vertices.iter().zip(chunk.normals.iter()) {
        vertices.extend_from_slice(&[
            position.x, position.y, position.z, normal.x, normal.y, normal.z, 0.0, 0.0, 1.0, 1.0,
            1.0, 1.0, 1.0, 0.0, 0.0, 1.0,
        ]);
    }
    vertices
}

impl XrScene {
    pub(super) fn clear_depth_surface_mesh(&mut self) {
        self.depth_surface_mesh_generation = 0;
        self.depth_surface_mesh_update_sequence = 0;
        self.depth_surface_mesh_chunks.clear();
        self.depth_surface_mesh_upload_count = 0;
    }

    fn upsert_depth_surface_mesh_chunk(&mut self, cx: &mut Cx2d, chunk: &XrDepthMeshChunk) {
        let key = (chunk.chunk_key.x, chunk.chunk_key.y, chunk.chunk_key.z);
        if self
            .depth_surface_mesh_chunks
            .get(&key)
            .map(|gpu_chunk| gpu_chunk.1.fingerprint == chunk.fingerprint)
            .unwrap_or(false)
        {
            return;
        }

        let vertices = pack_depth_mesh_vertices(chunk);
        if let Some((geometry, handle)) = self.depth_surface_mesh_chunks.get_mut(&key) {
            geometry.update(cx.cx.cx, chunk.indices.clone(), vertices);
            *handle = DepthSurfaceMeshChunkHandle {
                geometry_id: geometry.geometry_id(),
                fingerprint: chunk.fingerprint,
            };
        } else {
            let geometry = Geometry::new(cx.cx.cx);
            geometry.update(cx.cx.cx, chunk.indices.clone(), vertices);
            let handle = DepthSurfaceMeshChunkHandle {
                geometry_id: geometry.geometry_id(),
                fingerprint: chunk.fingerprint,
            };
            self.depth_surface_mesh_chunks.insert(key, (geometry, handle));
            self.depth_surface_mesh_upload_count =
                self.depth_surface_mesh_upload_count.saturating_add(1);
        }
    }

    fn resolve_depth_query_result(
        retained_hits: &mut HashMap<u64, RetainedDepthQueryHit>,
        key: u64,
        latest_result: Option<XrDepthMeshQueryResult>,
        expired_retained_keys: &mut Vec<u64>,
    ) -> Option<XrDepthMeshQueryResult> {
        match latest_result {
            Some(XrDepthMeshQueryResult::Hit(hit)) => {
                retained_hits.insert(key, RetainedDepthQueryHit::new(hit.clone()));
                Some(XrDepthMeshQueryResult::Hit(hit))
            }
            Some(XrDepthMeshQueryResult::Miss { .. }) | None => retained_hits
                .get_mut(&key)
                .and_then(|retained| retained.reuse_result())
                .or_else(|| {
                    if retained_hits.contains_key(&key) {
                        expired_retained_keys.push(key);
                    }
                    None
                }),
        }
    }

    fn build_depth_query_request(
        key: u64,
        pose: Pose,
        velocity: Vec3f,
        half_extents: Vec3f,
    ) -> XrDepthMeshQuery {
        let mut lookahead = velocity.scale(XR_DEPTH_QUERY_LOOKAHEAD_SECONDS);
        let lookahead_length = lookahead.length();
        if lookahead_length > XR_DEPTH_QUERY_MAX_LOOKAHEAD_DISTANCE && lookahead_length > 1.0e-6 {
            lookahead =
                lookahead.scale(XR_DEPTH_QUERY_MAX_LOOKAHEAD_DISTANCE / lookahead_length);
        }
        XrDepthMeshQuery {
            key,
            center: pose.position,
            predicted_center: pose.position + lookahead,
            velocity,
            radius: half_extents.length(),
            max_distance: XR_DEPTH_QUERY_MAX_DISTANCE,
            include_planar_patches: false,
        }
    }

    pub(super) fn sync_depth_query_surfaces(&mut self, cx: &mut Cx) {
        if !XR_ENABLE_DEPTH_QUERY_PHYSICS {
            return;
        }
        let Some(scene) = self.scene.as_mut() else {
            return;
        };
        let depth_mesh = cx.xr_depth_mesh();
        let mut clear_keys = Vec::new();
        let mut query_requests = Vec::new();
        let mut query_results = Vec::new();
        let mut expired_retained_keys = Vec::new();
        let retained_hits = &mut self.depth_query_retained_hits;

        for (index, cube) in scene.cubes.iter().enumerate() {
            let key = RapierScene::depth_query_key(index);
            let Some(body) = scene.bodies.get(cube.body) else {
                clear_keys.push(key);
                continue;
            };

            if let Some(result) = Self::resolve_depth_query_result(
                retained_hits,
                key,
                depth_mesh.latest_query_result(key),
                &mut expired_retained_keys,
            ) {
                query_results.push(result);
            }

            if body.is_sleeping() {
                continue;
            }

            let pose = makepad_pose(body.position());
            let linvel = body.linvel();
            let velocity = vec3f(linvel.x, linvel.y, linvel.z);
            query_requests.push(Self::build_depth_query_request(
                key,
                pose,
                velocity,
                cube.half_extents,
            ));
        }

        for key in clear_keys {
            depth_mesh.clear_query(key);
            retained_hits.remove(&key);
        }
        for key in expired_retained_keys {
            retained_hits.remove(&key);
        }

        for query in query_requests {
            let _ = depth_mesh.submit_query(query);
        }

        let targets = build_depth_query_surface_targets(&query_results);
        scene.sync_depth_query_surface_pool(&targets);
    }

    pub(super) fn sync_depth_surface_mesh(&mut self, cx: &mut Cx2d) {
        if !self.depth_debug_enabled() {
            return;
        }

        let Some(depth_mesh) = cx.cx.xr_depth_mesh().latest_mesh() else {
            self.clear_depth_surface_mesh();
            return;
        };
        let previous_mesh_generation = self.depth_surface_mesh_generation;
        let previous_update_sequence = self.depth_surface_mesh_update_sequence;
        if self.depth_surface_mesh_generation == depth_mesh.mesh_generation
            && self.depth_surface_mesh_update_sequence == depth_mesh.update_sequence
        {
            return;
        }

        self.depth_surface_mesh_generation = depth_mesh.mesh_generation;
        self.depth_surface_mesh_update_sequence = depth_mesh.update_sequence;
        if depth_mesh.mesh_chunks.is_empty() {
            self.clear_depth_surface_mesh();
            return;
        }

        let active_chunk_count = depth_mesh.mesh_chunks.len();
        if self.depth_surface_mesh_upload_count > active_chunk_count.saturating_mul(3) + 64 {
            self.clear_depth_surface_mesh();
        }

        let needs_full_resync = previous_mesh_generation == 0
            || self.depth_surface_mesh_chunks.is_empty()
            || depth_mesh.update_sequence != previous_update_sequence.saturating_add(1);

        if needs_full_resync {
            let mut desired_keys = HashSet::with_capacity(depth_mesh.mesh_chunks.len());
            for chunk in &depth_mesh.mesh_chunks {
                desired_keys.insert((chunk.chunk_key.x, chunk.chunk_key.y, chunk.chunk_key.z));
                self.upsert_depth_surface_mesh_chunk(cx, chunk);
            }
            self.depth_surface_mesh_chunks
                .retain(|key, _| desired_keys.contains(key));
            return;
        }

        for key in &depth_mesh.removed_chunk_keys {
            self.depth_surface_mesh_chunks
                .remove(&(key.x, key.y, key.z));
        }
        for key in &depth_mesh.dirty_chunk_keys {
            if let Some(chunk) = depth_mesh
                .mesh_chunks
                .iter()
                .find(|chunk| chunk.chunk_key == *key)
            {
                self.upsert_depth_surface_mesh_chunk(cx, chunk);
            }
        }
    }

    pub(super) fn draw_depth_surface_mesh(&mut self, cx: &mut Cx2d) {
        if !self.depth_debug_enabled() {
            return;
        }
        if self.depth_surface_mesh_chunks.is_empty() {
            return;
        }
        self.draw_depth_mesh.base_color = vec4(0.76, 0.88, 0.98, 1.0);
        let mut chunk_handles: Vec<_> = self
            .depth_surface_mesh_chunks
            .iter()
            .map(|(key, chunk)| (*key, chunk.1.geometry_id))
            .collect();
        chunk_handles.sort_by_key(|(key, _)| *key);
        for (_, geometry_id) in chunk_handles {
            self.draw_depth_mesh.draw_geometry(cx, geometry_id);
        }
    }
}
