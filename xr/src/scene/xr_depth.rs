#![allow(dead_code)]

use super::xr_physics::{makepad_pose, RapierScene};
use super::*;
use makepad_widgets::makepad_platform::XrDepthMeshStore;
use std::collections::HashSet;

const XR_DEBUG_DEPTH_TRIANGLE_SHRINK: f32 = 0.90;

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

#[derive(Clone)]
pub(super) struct DepthQuerySurfaceTarget {
    pub(super) collider: XrDepthMeshQueryCollider,
}

#[derive(Clone)]
struct RetainedDepthQuerySurface {
    target: DepthQuerySurfaceTarget,
    misses_left: u8,
}

pub(super) fn depth_query_plane_supports_body(
    plane: XrDepthMeshQuerySupportPlane,
    body_position: Vec3f,
    query_radius: f32,
    lateral_margin: f32,
) -> bool {
    let offset = body_position - plane.point;
    let signed_height = offset.dot(plane.normal);
    if signed_height < -query_radius.max(0.0005) {
        return false;
    }
    if signed_height > query_radius + XR_DEPTH_QUERY_MAX_DISTANCE + lateral_margin {
        return false;
    }

    let tangent_limit = plane.half_extent_tangent + lateral_margin;
    let bitangent_limit = plane.half_extent_bitangent + lateral_margin;
    offset.dot(plane.tangent).abs() <= tangent_limit
        && offset.dot(plane.bitangent).abs() <= bitangent_limit
}

fn depth_query_plane_edge_slack(plane: XrDepthMeshQuerySupportPlane, body_position: Vec3f) -> f32 {
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

impl RetainedDepthQuerySurface {
    fn sticky_supports(&self, body_position: Vec3f, query_radius: f32) -> bool {
        if self.target.collider.role != XrDepthMeshQueryColliderRole::Support {
            return false;
        }
        let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) = self.target.collider.geometry;
        depth_query_plane_supports_body(
            plane,
            body_position,
            query_radius,
            XR_DEPTH_QUERY_STICKY_KEEP_MARGIN,
        )
    }

    fn safely_supports(&self, body_position: Vec3f, query_radius: f32, edge_margin: f32) -> bool {
        if self.target.collider.role != XrDepthMeshQueryColliderRole::Support {
            return false;
        }
        let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) = self.target.collider.geometry;
        depth_query_plane_supports_body(plane, body_position, query_radius, 0.0)
            && depth_query_plane_edge_slack(plane, body_position) > edge_margin
    }
}

#[derive(Clone)]
pub(super) struct RetainedDepthQueryHit {
    surfaces: [Option<RetainedDepthQuerySurface>; XR_DEPTH_QUERY_SURFACES_PER_BODY],
}

impl RetainedDepthQueryHit {
    fn new(hit: &XrDepthMeshQueryHit) -> Self {
        let mut retained = Self {
            surfaces: std::array::from_fn(|_| None),
        };
        retained.refresh_from_hit(hit);
        retained
    }

    fn refresh_from_hit(&mut self, hit: &XrDepthMeshQueryHit) {
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
            XrDepthMeshQueryColliderRole::Support => XR_DEPTH_QUERY_HIT_MISS_GRACE_FRAMES,
            XrDepthMeshQueryColliderRole::Impact => 0,
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

fn pack_depth_mesh_debug_triangles(chunk: &XrDepthMeshChunk) -> (Vec<u32>, Vec<f32>) {
    const FLOATS_PER_VERTEX: usize = 16;
    let triangle_count = chunk.indices.len() / 3;
    let mut indices = Vec::with_capacity(chunk.indices.len());
    let mut vertices = Vec::with_capacity(triangle_count * 3 * FLOATS_PER_VERTEX);

    for triangle in chunk.indices.chunks_exact(3) {
        let a = chunk.vertices[triangle[0] as usize];
        let b = chunk.vertices[triangle[1] as usize];
        let c = chunk.vertices[triangle[2] as usize];
        let centroid = (a + b + c).scale(1.0 / 3.0);
        let raw_normal = Vec3f::cross(b - a, c - a);
        let normal = if raw_normal.length() > 1.0e-6 {
            raw_normal.normalize()
        } else {
            vec3f(0.0, 1.0, 0.0)
        };
        let base = (vertices.len() / FLOATS_PER_VERTEX) as u32;
        for position in [a, b, c] {
            let shrunken = centroid + (position - centroid).scale(XR_DEBUG_DEPTH_TRIANGLE_SHRINK);
            vertices.extend_from_slice(&[
                shrunken.x, shrunken.y, shrunken.z, normal.x, normal.y, normal.z, 0.0, 0.0, 1.0,
                1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0,
            ]);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    (indices, vertices)
}

fn push_debug_depth_vertex(vertices: &mut Vec<f32>, position: Vec3f, normal: Vec3f) {
    vertices.extend_from_slice(&[
        position.x, position.y, position.z, normal.x, normal.y, normal.z, 0.0, 0.0, 1.0, 1.0, 1.0,
        1.0, 1.0, 0.0, 0.0, 1.0,
    ]);
}

fn push_debug_depth_triangle(
    indices: &mut Vec<u32>,
    vertices: &mut Vec<f32>,
    triangle: [Vec3f; 3],
) {
    let base = (vertices.len() / 16) as u32;
    let raw_normal = Vec3f::cross(triangle[1] - triangle[0], triangle[2] - triangle[0]);
    let normal = if raw_normal.length() > 1.0e-6 {
        raw_normal.normalize()
    } else {
        vec3f(0.0, 1.0, 0.0)
    };
    for position in triangle {
        push_debug_depth_vertex(vertices, position, normal);
    }
    indices.extend_from_slice(&[base, base + 1, base + 2]);
}

fn push_debug_depth_quad(indices: &mut Vec<u32>, vertices: &mut Vec<f32>, quad: [Vec3f; 4]) {
    let base = (vertices.len() / 16) as u32;
    let raw_normal = Vec3f::cross(quad[1] - quad[0], quad[2] - quad[0]);
    let normal = if raw_normal.length() > 1.0e-6 {
        raw_normal.normalize()
    } else {
        vec3f(0.0, 1.0, 0.0)
    };
    for position in quad {
        push_debug_depth_vertex(vertices, position, normal);
    }
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

fn push_debug_depth_plane(
    indices: &mut Vec<u32>,
    vertices: &mut Vec<f32>,
    plane: XrDepthMeshQuerySupportPlane,
) {
    let center = plane.point;
    let tangent = plane.tangent.scale(plane.half_extent_tangent);
    let bitangent = plane.bitangent.scale(plane.half_extent_bitangent);
    push_debug_depth_quad(
        indices,
        vertices,
        [
            center - tangent - bitangent,
            center + tangent - bitangent,
            center + tangent + bitangent,
            center - tangent + bitangent,
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_query_plane_supports_body_respects_exact_quad_edge() {
        let plane = XrDepthMeshQuerySupportPlane {
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
        let plane = XrDepthMeshQuerySupportPlane {
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
                        collider: XrDepthMeshQueryCollider {
                            fingerprint: 1,
                            geometry: XrDepthMeshQueryColliderGeometry::HalfSpace(plane),
                            role: XrDepthMeshQueryColliderRole::Support,
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
        let plane = XrDepthMeshQuerySupportPlane {
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
                        collider: XrDepthMeshQueryCollider {
                            fingerprint: 2,
                            geometry: XrDepthMeshQueryColliderGeometry::HalfSpace(plane),
                            role: XrDepthMeshQueryColliderRole::Support,
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
}

impl XrEnv {
    pub(super) fn depth_debug_enabled(&self) -> bool {
        self.depth_mesh_visible() || self.depth_query_hits_visible()
    }

    pub(super) fn clear_depth_surface_mesh(&mut self) {
        self.depth_surface_mesh_generation = 0;
        self.depth_surface_mesh_update_sequence = 0;
        self.depth_surface_mesh_chunks.clear();
        self.depth_surface_mesh_upload_count = 0;
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
            let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) =
                retained.target.collider.geometry;
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

        let (indices, vertices) = pack_depth_mesh_debug_triangles(chunk);
        if let Some((geometry, handle)) = self.depth_surface_mesh_chunks.get_mut(&key) {
            geometry.update(cx.cx.cx, indices, vertices);
            *handle = DepthSurfaceMeshChunkHandle {
                geometry_id: geometry.geometry_id(),
                fingerprint: chunk.fingerprint,
            };
        } else {
            let geometry = Geometry::new(cx.cx.cx);
            geometry.update(cx.cx.cx, indices, vertices);
            let handle = DepthSurfaceMeshChunkHandle {
                geometry_id: geometry.geometry_id(),
                fingerprint: chunk.fingerprint,
            };
            self.depth_surface_mesh_chunks
                .insert(key, (geometry, handle));
            self.depth_surface_mesh_upload_count =
                self.depth_surface_mesh_upload_count.saturating_add(1);
        }
    }

    fn sync_retained_depth_query_result(
        retained_hits: &mut HashMap<u64, RetainedDepthQueryHit>,
        key: u64,
        latest_result: Option<XrDepthMeshQueryResult>,
        body_position: Vec3f,
        query_radius: f32,
        expired_retained_keys: &mut Vec<u64>,
    ) {
        match latest_result {
            Some(XrDepthMeshQueryResult::Hit(hit)) => {
                if let Some(retained) = retained_hits.get_mut(&key) {
                    retained.refresh_from_hit(&hit);
                } else {
                    retained_hits.insert(key, RetainedDepthQueryHit::new(&hit));
                }
            }
            Some(XrDepthMeshQueryResult::Miss { .. }) | None => {
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
    ) -> XrDepthMeshQuery {
        let speed = velocity.length();
        let lookahead_seconds = if speed > 1.0e-6 {
            XR_DEPTH_QUERY_LOOKAHEAD_SECONDS.min(XR_DEPTH_QUERY_MAX_LOOKAHEAD_DISTANCE / speed)
        } else {
            XR_DEPTH_QUERY_LOOKAHEAD_SECONDS
        };
        let lookahead = velocity.scale(lookahead_seconds)
            + gravity.scale(0.5 * lookahead_seconds * lookahead_seconds);
        XrDepthMeshQuery {
            key,
            center: pose.position,
            predicted_center: pose.position + lookahead,
            velocity,
            radius: query_radius.max(0.0005),
            max_distance: XR_DEPTH_QUERY_MAX_DISTANCE,
            include_planar_patches: XR_DEPTH_QUERY_INCLUDE_PLANAR_PATCHES,
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
        sync_depth_query_surfaces_with_store(retained_hits, Some(scene), &cx.xr_depth_mesh());
    }

    pub(super) fn sync_depth_surface_mesh(&mut self, cx: &mut Cx2d) {
        if !self.depth_mesh_visible() {
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
        if show_mesh && !self.depth_surface_mesh_chunks.is_empty() {
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
    depth_mesh: &XrDepthMeshStore,
    scene: Option<&RapierScene>,
    retained_hits: &mut HashMap<u64, RetainedDepthQueryHit>,
) {
    if let Some(scene) = scene {
        for index in 0..scene.depth_query_surface_set_count() {
            depth_mesh.clear_query(RapierScene::depth_query_key(index));
        }
    }
    retained_hits.clear();
}

pub(super) fn sync_depth_query_surfaces_with_store(
    retained_hits: &mut HashMap<u64, RetainedDepthQueryHit>,
    scene: Option<&mut RapierScene>,
    depth_mesh: &XrDepthMeshStore,
) {
    if !XR_ENABLE_DEPTH_QUERY_PHYSICS {
        return;
    }
    let Some(scene) = scene else {
        return;
    };

    let mut clear_keys = Vec::new();
    let mut query_requests = Vec::new();
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

        let latest_result = if body_sleeping {
            None
        } else {
            depth_mesh.latest_query_result(key)
        };
        XrEnv::sync_retained_depth_query_result(
            retained_hits,
            key,
            latest_result,
            body_pose.position,
            cube.query_radius,
            &mut expired_retained_keys,
        );
        let mut surface_targets = std::array::from_fn(|_| None);
        if let Some(retained) = retained_hits.get(&key) {
            retained.fill_targets(&mut surface_targets);
        }
        scene.sync_depth_query_surface_set(set_index, &surface_targets);

        if body_sleeping {
            continue;
        }
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
        if can_skip_refresh && body_speed < XR_DEPTH_QUERY_SUPPORT_REFRESH_SPEED_MIN {
            continue;
        }
        query_requests.push(query_request);
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
}
