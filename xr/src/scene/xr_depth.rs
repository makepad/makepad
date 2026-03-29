#![allow(dead_code)]

use super::xr_physics::{makepad_pose, RapierScene};
use super::*;
use crate::depth_debug_mesh::{push_debug_depth_plane, DebugDepthMeshChunk};
use crate::depth_debug_mesh_worker::{
    XrDepthDebugMeshVisibleSet, XrDepthDebugMeshWorker, XrDepthDebugMeshWorkerResult,
};
use crate::tsdf_query::{
    depth_query_might_need_impact_refresh, depth_query_plane_supports_body, evaluate_tsdf_query,
    DepthQuery, DepthQueryCollider, DepthQueryColliderGeometry, DepthQueryColliderRole,
    DepthQueryHit, DepthQueryResult, DepthQuerySupportPlane,
};
use makepad_widgets::makepad_platform::XrTsdfStore;

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
pub(super) struct DepthQuerySurfaceTarget {
    pub(super) collider: DepthQueryCollider,
}

#[derive(Clone)]
struct RetainedDepthQuerySurface {
    target: DepthQuerySurfaceTarget,
    misses_left: u8,
}

fn depth_query_plane_edge_slack(plane: DepthQuerySupportPlane, body_position: Vec3f) -> f32 {
    let offset = body_position - plane.point;
    let tangent_slack = plane.half_extent_tangent - offset.dot(plane.tangent).abs();
    let bitangent_slack = plane.half_extent_bitangent - offset.dot(plane.bitangent).abs();
    tangent_slack.min(bitangent_slack)
}

fn depth_query_support_refresh_edge_margin(query_radius: f32) -> f32 {
    (query_radius * XR_DEPTH_QUERY_SUPPORT_REFRESH_EDGE_MARGIN_SCALE).clamp(
        XR_DEPTH_QUERY_SUPPORT_REFRESH_EDGE_MARGIN_MIN,
        XR_DEPTH_QUERY_SUPPORT_REFRESH_EDGE_MARGIN_MAX,
    )
}

fn depth_query_support_refresh_margin_scale(body_speed: f32) -> f32 {
    (body_speed / XR_DEPTH_QUERY_SUPPORT_REFRESH_SPEED_MIN).clamp(0.25, 1.0)
}

fn depth_query_should_refresh_from_tsdf(
    body_sleeping: bool,
    can_skip_support_refresh: bool,
    body_speed: f32,
    needs_impact_refresh: bool,
) -> bool {
    !body_sleeping
        && (!can_skip_support_refresh
            || needs_impact_refresh
            || body_speed >= XR_DEPTH_QUERY_SUPPORT_REFRESH_SPEED_MIN)
}

impl RetainedDepthQuerySurface {
    fn sticky_supports(&self, body_position: Vec3f, query_radius: f32) -> bool {
        if self.target.collider.role != DepthQueryColliderRole::Support {
            return false;
        }
        let DepthQueryColliderGeometry::HalfSpace(plane) = self.target.collider.geometry;
        depth_query_plane_supports_body(
            plane,
            body_position,
            query_radius,
            XR_DEPTH_QUERY_STICKY_KEEP_MARGIN,
        )
    }

    fn safely_supports(&self, body_position: Vec3f, query_radius: f32, edge_margin: f32) -> bool {
        if self.target.collider.role != DepthQueryColliderRole::Support {
            return false;
        }
        let DepthQueryColliderGeometry::HalfSpace(plane) = self.target.collider.geometry;
        depth_query_plane_supports_body(plane, body_position, query_radius, 0.0)
            && depth_query_plane_edge_slack(plane, body_position) > edge_margin
    }
}

#[derive(Clone)]
pub(super) struct RetainedDepthQueryHit {
    surfaces: [Option<RetainedDepthQuerySurface>; XR_DEPTH_QUERY_SURFACES_PER_BODY],
}

impl RetainedDepthQueryHit {
    fn new(hit: &DepthQueryHit) -> Self {
        let mut retained = Self {
            surfaces: std::array::from_fn(|_| None),
        };
        retained.refresh_from_hit(hit);
        retained
    }

    fn refresh_from_hit(&mut self, hit: &DepthQueryHit) {
        let mut matched = [false; XR_DEPTH_QUERY_SURFACES_PER_BODY];
        self.absorb_surface(
            DepthQuerySurfaceTarget {
                collider: hit.collider.clone(),
            },
            true,
            &mut matched,
        );
        for additional_hit in &hit.additional_hits {
            self.absorb_surface(
                DepthQuerySurfaceTarget {
                    collider: additional_hit.collider.clone(),
                },
                true,
                &mut matched,
            );
        }
        for (index, slot) in self.surfaces.iter_mut().enumerate() {
            if matched[index] {
                continue;
            }
            if let Some(retained) = slot.as_mut() {
                if retained.misses_left > 0 {
                    retained.misses_left -= 1;
                } else {
                    *slot = None;
                }
            }
        }
    }

    fn absorb_surface(
        &mut self,
        target: DepthQuerySurfaceTarget,
        allow_replace: bool,
        matched: &mut [bool; XR_DEPTH_QUERY_SURFACES_PER_BODY],
    ) {
        let misses_left = match target.collider.role {
            DepthQueryColliderRole::Support => XR_DEPTH_QUERY_HIT_MISS_GRACE_FRAMES,
            DepthQueryColliderRole::Impact => 0,
        };
        let retained_surface = RetainedDepthQuerySurface {
            target,
            misses_left,
        };

        if let Some(index) = self.surfaces.iter().position(|slot| {
            slot.as_ref().is_some_and(|existing| {
                existing.target.collider.fingerprint == retained_surface.target.collider.fingerprint
            })
        }) {
            self.surfaces[index] = Some(retained_surface);
            matched[index] = true;
            return;
        }

        if let Some(index) = self.surfaces.iter().position(Option::is_none) {
            self.surfaces[index] = Some(retained_surface);
            matched[index] = true;
            return;
        }

        if !allow_replace {
            return;
        }

        if let Some((index, _)) = self
            .surfaces
            .iter()
            .enumerate()
            .filter(|(index, _)| !matched[*index])
            .filter_map(|(index, slot)| slot.as_ref().map(|retained| (index, retained.misses_left)))
            .min_by_key(|(_, misses_left)| *misses_left)
        {
            self.surfaces[index] = Some(retained_surface);
            matched[index] = true;
        }
    }

    fn age_on_miss_with_sticky_support(&mut self, body_position: Vec3f, query_radius: f32) -> bool {
        let mut any_alive = false;
        for slot in &mut self.surfaces {
            if let Some(retained) = slot.as_mut() {
                if retained.sticky_supports(body_position, query_radius) {
                    retained.misses_left = XR_DEPTH_QUERY_HIT_MISS_GRACE_FRAMES;
                    any_alive = true;
                    continue;
                }
                if retained.misses_left > 0 {
                    retained.misses_left -= 1;
                    any_alive = true;
                } else {
                    *slot = None;
                }
            }
        }
        any_alive
    }

    fn fill_targets(
        &self,
        targets: &mut [Option<DepthQuerySurfaceTarget>; XR_DEPTH_QUERY_SURFACES_PER_BODY],
    ) {
        *targets = std::array::from_fn(|_| None);
        for (index, retained) in self.surfaces.iter().enumerate() {
            targets[index] = retained.as_ref().map(|retained| retained.target.clone());
        }
    }

    fn can_skip_refresh(
        &self,
        body_position: Vec3f,
        predicted_body_position: Vec3f,
        query_radius: f32,
        body_speed: f32,
    ) -> bool {
        let edge_margin = depth_query_support_refresh_edge_margin(query_radius)
            * depth_query_support_refresh_margin_scale(body_speed);
        self.surfaces.iter().any(|slot| {
            slot.as_ref().is_some_and(|retained| {
                retained.safely_supports(body_position, query_radius, edge_margin)
                    && retained.safely_supports(predicted_body_position, query_radius, edge_margin)
            })
        })
    }

    fn retained_surfaces(&self) -> impl Iterator<Item = &RetainedDepthQuerySurface> + '_ {
        self.surfaces.iter().filter_map(Option::as_ref)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_query_plane_supports_body_respects_exact_quad_edge() {
        let plane = DepthQuerySupportPlane {
            point: vec3f(0.0, 0.0, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            tangent: vec3f(1.0, 0.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.10,
            half_extent_bitangent: 0.08,
        };

        assert!(depth_query_plane_supports_body(
            plane,
            vec3f(0.099, 0.03, 0.0),
            0.05,
            0.0,
        ));
        assert!(!depth_query_plane_supports_body(
            plane,
            vec3f(0.101, 0.03, 0.0),
            0.05,
            0.0,
        ));
    }

    #[test]
    fn retained_support_refresh_requeries_before_body_reaches_quad_edge() {
        let plane = DepthQuerySupportPlane {
            point: vec3f(0.0, 0.0, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            tangent: vec3f(1.0, 0.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.10,
            half_extent_bitangent: 0.10,
        };
        let retained = RetainedDepthQueryHit {
            surfaces: std::array::from_fn(|index| {
                (index == 0).then_some(RetainedDepthQuerySurface {
                    target: DepthQuerySurfaceTarget {
                        collider: DepthQueryCollider {
                            fingerprint: 1,
                            geometry: DepthQueryColliderGeometry::HalfSpace(plane),
                            role: DepthQueryColliderRole::Support,
                            restitution: 0.0,
                        },
                    },
                    misses_left: XR_DEPTH_QUERY_HIT_MISS_GRACE_FRAMES,
                })
            }),
        };

        assert!(retained.can_skip_refresh(
            vec3f(0.00, 0.03, 0.0),
            vec3f(0.02, 0.03, 0.0),
            0.05,
            0.30,
        ));
        assert!(!retained.can_skip_refresh(
            vec3f(0.06, 0.03, 0.0),
            vec3f(0.09, 0.03, 0.0),
            0.05,
            0.30,
        ));
    }

    #[test]
    fn slow_support_refresh_margin_is_less_aggressive_than_fast_motion() {
        let plane = DepthQuerySupportPlane {
            point: vec3f(0.0, 0.0, 0.0),
            normal: vec3f(0.0, 1.0, 0.0),
            tangent: vec3f(1.0, 0.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.10,
            half_extent_bitangent: 0.10,
        };
        let retained = RetainedDepthQueryHit {
            surfaces: std::array::from_fn(|index| {
                (index == 0).then_some(RetainedDepthQuerySurface {
                    target: DepthQuerySurfaceTarget {
                        collider: DepthQueryCollider {
                            fingerprint: 2,
                            geometry: DepthQueryColliderGeometry::HalfSpace(plane),
                            role: DepthQueryColliderRole::Support,
                            restitution: 0.0,
                        },
                    },
                    misses_left: XR_DEPTH_QUERY_HIT_MISS_GRACE_FRAMES,
                })
            }),
        };

        assert!(retained.can_skip_refresh(
            vec3f(0.082, 0.03, 0.0),
            vec3f(0.088, 0.03, 0.0),
            0.05,
            0.05,
        ));
        assert!(!retained.can_skip_refresh(
            vec3f(0.082, 0.03, 0.0),
            vec3f(0.088, 0.03, 0.0),
            0.05,
            0.30,
        ));
    }

    #[test]
    fn retained_support_does_not_hide_impact_capable_motion() {
        assert!(depth_query_should_refresh_from_tsdf(
            false, true, 0.12, true,
        ));
        assert!(!depth_query_should_refresh_from_tsdf(
            false, true, 0.12, false,
        ));
        assert!(!depth_query_should_refresh_from_tsdf(
            true, false, 0.40, true,
        ));
    }
}

impl XrEnv {
    pub(super) fn depth_debug_enabled(&self) -> bool {
        self.depth_mesh_visible() || self.depth_query_hits_visible()
    }

    pub(super) fn clear_depth_surface_mesh(&mut self) {
        self.depth_surface_mesh_generation = 0;
        self.depth_surface_mesh_update_sequence = 0;
        self.depth_surface_mesh_requested_snapshot_grid = None;
        self.depth_surface_mesh_requested_head_pose = None;
        self.depth_surface_mesh_snapshot_grid = None;
        self.depth_surface_mesh_visible_request_id = 0;
        self.depth_surface_mesh_visible_chunks.clear();
        self.depth_surface_mesh_chunks.clear();
        self.depth_surface_mesh_worker = None;
    }

    pub(super) fn ensure_depth_surface_mesh_worker(&mut self) -> &mut XrDepthDebugMeshWorker {
        self.depth_surface_mesh_worker
            .get_or_insert_with(XrDepthDebugMeshWorker::new)
    }

    fn upsert_depth_query_hit_geometry(
        slot: &mut Option<Geometry>,
        cx: &mut Cx2d,
        indices: Vec<u32>,
        vertices: Vec<f32>,
    ) -> Option<GeometryId> {
        if indices.is_empty() {
            return None;
        }
        let geometry = slot.get_or_insert_with(|| Geometry::new(cx.cx.cx));
        geometry.update(cx.cx.cx, indices, vertices);
        Some(geometry.geometry_id())
    }

    fn build_depth_query_hit_geometry(&mut self, cx: &mut Cx2d) -> Option<GeometryId> {
        let mut quad_indices = Vec::new();
        let mut quad_vertices = Vec::new();

        let mut push_surface = |retained: &RetainedDepthQuerySurface| {
            let DepthQueryColliderGeometry::HalfSpace(plane) = retained.target.collider.geometry;
            push_debug_depth_plane(&mut quad_indices, &mut quad_vertices, plane);
        };

        for retained in self.depth_query_retained_hits.values() {
            for surface in retained.retained_surfaces() {
                push_surface(surface);
            }
        }

        Self::upsert_depth_query_hit_geometry(
            &mut self.depth_query_hit_geometry,
            cx,
            quad_indices,
            quad_vertices,
        )
    }

    fn upsert_depth_surface_mesh_chunk(&mut self, cx: &mut Cx2d, chunk: DebugDepthMeshChunk) {
        let key = chunk.chunk_key;
        if self
            .depth_surface_mesh_chunks
            .get(&key)
            .map(|gpu_chunk| gpu_chunk.1.fingerprint == chunk.fingerprint)
            .unwrap_or(false)
        {
            return;
        }

        if let Some((geometry, handle)) = self.depth_surface_mesh_chunks.get_mut(&key) {
            geometry.update(cx.cx.cx, chunk.indices, chunk.vertices);
            *handle = DepthSurfaceMeshChunkHandle {
                geometry_id: geometry.geometry_id(),
                fingerprint: chunk.fingerprint,
            };
        } else {
            let geometry = Geometry::new(cx.cx.cx);
            geometry.update(cx.cx.cx, chunk.indices, chunk.vertices);
            let handle = DepthSurfaceMeshChunkHandle {
                geometry_id: geometry.geometry_id(),
                fingerprint: chunk.fingerprint,
            };
            self.depth_surface_mesh_chunks
                .insert(key, (geometry, handle));
        }
    }

    fn apply_depth_surface_mesh_visible_set(&mut self, result: XrDepthDebugMeshVisibleSet) {
        if result.request_id < self.depth_surface_mesh_visible_request_id {
            return;
        }
        self.depth_surface_mesh_visible_request_id = result.request_id;
        self.depth_surface_mesh_generation = result.generation;
        self.depth_surface_mesh_update_sequence = result.update_sequence;
        self.depth_surface_mesh_snapshot_grid = Some(result.snapshot_grid);
        self.depth_surface_mesh_visible_chunks = result.visible_chunk_keys.into_iter().collect();
    }

    fn remove_depth_surface_mesh_chunk(&mut self, chunk_key: ChunkKey) {
        self.depth_surface_mesh_chunks.remove(&chunk_key);
    }

    fn apply_depth_surface_mesh_worker_result(
        &mut self,
        cx: &mut Cx2d,
        result: XrDepthDebugMeshWorkerResult,
    ) {
        match result {
            XrDepthDebugMeshWorkerResult::VisibleSet(result) => {
                self.apply_depth_surface_mesh_visible_set(result);
            }
            XrDepthDebugMeshWorkerResult::ChunkUpserts { request_id, chunks } => {
                if request_id != self.depth_surface_mesh_visible_request_id {
                    return;
                }
                for chunk in chunks {
                    self.upsert_depth_surface_mesh_chunk(cx, chunk);
                }
            }
            XrDepthDebugMeshWorkerResult::ChunkRemovals {
                request_id,
                chunk_keys,
            } => {
                if request_id != self.depth_surface_mesh_visible_request_id {
                    return;
                }
                for chunk_key in chunk_keys {
                    self.remove_depth_surface_mesh_chunk(chunk_key);
                }
            }
        }
    }

    pub(super) fn poll_depth_surface_mesh_worker(&mut self, cx: &mut Cx2d) {
        while let Some(result) = self
            .depth_surface_mesh_worker
            .as_mut()
            .and_then(|worker| worker.take_next_result())
        {
            self.apply_depth_surface_mesh_worker_result(cx, result);
        }
    }

    fn sync_retained_depth_query_result(
        retained_hits: &mut HashMap<u64, RetainedDepthQueryHit>,
        key: u64,
        latest_result: Option<DepthQueryResult>,
        body_position: Vec3f,
        query_radius: f32,
        expired_retained_keys: &mut Vec<u64>,
    ) {
        match latest_result {
            Some(DepthQueryResult::Hit(hit)) => {
                if let Some(retained) = retained_hits.get_mut(&key) {
                    retained.refresh_from_hit(&hit);
                } else {
                    retained_hits.insert(key, RetainedDepthQueryHit::new(&hit));
                }
            }
            Some(DepthQueryResult::Miss { .. }) | None => {
                if let Some(retained) = retained_hits.get_mut(&key) {
                    if !retained.age_on_miss_with_sticky_support(body_position, query_radius) {
                        expired_retained_keys.push(key);
                    }
                }
            }
        }
    }

    fn build_depth_query_request(
        key: u64,
        pose: Pose,
        velocity: Vec3f,
        gravity: Vec3f,
        query_radius: f32,
    ) -> DepthQuery {
        let speed = velocity.length();
        let lookahead_seconds = if speed > 1.0e-6 {
            XR_DEPTH_QUERY_LOOKAHEAD_SECONDS.min(XR_DEPTH_QUERY_MAX_LOOKAHEAD_DISTANCE / speed)
        } else {
            XR_DEPTH_QUERY_LOOKAHEAD_SECONDS
        };
        let lookahead = velocity.scale(lookahead_seconds)
            + gravity.scale(0.5 * lookahead_seconds * lookahead_seconds);
        DepthQuery {
            key,
            center: pose.position,
            predicted_center: pose.position + lookahead,
            velocity,
            radius: query_radius.max(0.0005),
            max_distance: XR_DEPTH_QUERY_MAX_DISTANCE,
        }
    }

    pub(super) fn sync_depth_query_surfaces(
        retained_hits: &mut HashMap<u64, RetainedDepthQueryHit>,
        scene: Option<&mut RapierScene>,
        cx: &mut Cx,
    ) {
        if !XR_ENABLE_DEPTH_QUERY_PHYSICS {
            return;
        }
        let Some(scene) = scene else {
            return;
        };
        sync_depth_query_surfaces_with_store(retained_hits, Some(scene), &cx.xr_tsdf());
    }

    pub(super) fn sync_depth_surface_mesh(&mut self, cx: &mut Cx2d) {
        self.poll_depth_surface_mesh_worker(cx);
    }

    pub(super) fn draw_depth_surface_mesh(&mut self, cx: &mut Cx2d) {
        let show_mesh = self.depth_mesh_visible();
        let show_query_hits = self.depth_query_hits_visible();
        if !show_mesh && !show_query_hits {
            return;
        }
        let query_hits = if show_query_hits {
            self.build_depth_query_hit_geometry(cx)
        } else {
            None
        };
        if (!show_mesh || self.depth_surface_mesh_chunks.is_empty()) && query_hits.is_none() {
            return;
        }
        self.draw_depth_mesh.base_color = vec4(0.76, 0.88, 0.98, 1.0);
        if show_mesh
            && !self.depth_surface_mesh_chunks.is_empty()
            && !self.depth_surface_mesh_visible_chunks.is_empty()
        {
            let mut chunk_handles: Vec<_> = self
                .depth_surface_mesh_visible_chunks
                .iter()
                .filter_map(|key| {
                    self.depth_surface_mesh_chunks
                        .get(key)
                        .map(|chunk| (*key, chunk.1.geometry_id))
                })
                .collect();
            chunk_handles.sort_by_key(|(key, _)| (key.x, key.y, key.z));
            for (_, geometry_id) in chunk_handles {
                self.draw_depth_mesh.draw_geometry(cx, geometry_id);
            }
        }

        let mesh_normal_bias = self.draw_depth_mesh.normal_bias;
        self.draw_depth_mesh.normal_bias = mesh_normal_bias + 0.004;
        if show_query_hits {
            if let Some(geometry_id) = query_hits {
                self.draw_depth_mesh.base_color = vec4(1.0, 0.42, 0.08, 1.0);
                self.draw_depth_mesh.draw_geometry(cx, geometry_id);
            }
        }
        self.draw_depth_mesh.normal_bias = mesh_normal_bias;
    }
}

pub(super) fn clear_depth_query_state_for_scene(
    scene: Option<&RapierScene>,
    retained_hits: &mut HashMap<u64, RetainedDepthQueryHit>,
) {
    let _ = scene;
    retained_hits.clear();
}

pub(super) fn sync_depth_query_surfaces_with_store(
    retained_hits: &mut HashMap<u64, RetainedDepthQueryHit>,
    scene: Option<&mut RapierScene>,
    depth_mesh: &XrTsdfStore,
) {
    if !XR_ENABLE_DEPTH_QUERY_PHYSICS {
        return;
    }
    let Some(scene) = scene else {
        return;
    };
    let snapshot = depth_mesh.latest_tsdf_snapshot();

    let mut clear_keys = Vec::new();
    let mut expired_retained_keys = Vec::new();
    scene.begin_depth_query_stats_frame();

    for cube_index in 0..scene.cubes.len() {
        let cube = scene.cubes[cube_index];
        let Some(set_index) = cube.depth_query_surface_set else {
            continue;
        };
        let key = RapierScene::depth_query_key(set_index);
        let Some(body) = scene.bodies.get(cube.body) else {
            clear_keys.push(key);
            continue;
        };
        if !body.is_enabled() {
            scene.sync_depth_query_surface_set(set_index, &std::array::from_fn(|_| None));
            clear_keys.push(key);
            continue;
        }
        let body_sleeping = body.is_sleeping();
        let body_pose = makepad_pose(body.position());
        let linvel = body.linvel();
        let body_velocity = vec3f(linvel.x, linvel.y, linvel.z);
        let gravity = scene.gravity_vector();

        let query_request = XrEnv::build_depth_query_request(
            key,
            body_pose,
            body_velocity,
            gravity,
            cube.query_radius,
        );
        let body_speed = body_velocity.length();
        let can_skip_refresh = retained_hits.get(&key).is_some_and(|retained| {
            retained.can_skip_refresh(
                body_pose.position,
                query_request.predicted_center,
                cube.query_radius,
                body_speed,
            )
        });
        let needs_impact_refresh = depth_query_might_need_impact_refresh(query_request);
        if depth_query_should_refresh_from_tsdf(
            body_sleeping,
            can_skip_refresh,
            body_speed,
            needs_impact_refresh,
        ) {
            let latest_result = snapshot
                .as_ref()
                .map(|snapshot| evaluate_tsdf_query(snapshot, query_request));
            XrEnv::sync_retained_depth_query_result(
                retained_hits,
                key,
                latest_result,
                body_pose.position,
                cube.query_radius,
                &mut expired_retained_keys,
            );
        }

        let mut surface_targets = std::array::from_fn(|_| None);
        if let Some(retained) = retained_hits.get(&key) {
            retained.fill_targets(&mut surface_targets);
        }
        scene.sync_depth_query_surface_set(set_index, &surface_targets);
    }

    for key in clear_keys {
        retained_hits.remove(&key);
    }
    for key in expired_retained_keys {
        retained_hits.remove(&key);
    }
}
