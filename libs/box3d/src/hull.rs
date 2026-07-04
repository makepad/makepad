// Port of box3d/src/hull.c
// Dirk Gregorius contributed portions of the original C code.
//
// Convex hull builder (quickhull with face merging), box hulls, and hull queries.
//
// Port notes:
// - The C builder carves pointer-linked arenas out of one allocation; the port
//   uses index-based arenas (Vec) with i32 links. The intrusive circular lists
//   (vertex list, orphan list, face list, per-face conflict lists) keep the C
//   sentinel representation: sentinels are arena entries and list order is
//   preserved operation for operation.
// - The C b3HullData is one allocation with trailing arrays; the Rust HullData
//   (crate::types) has Vec fields, so byteCount/offsets do not exist.
// - Hull constructors return Arc<HullData>; b3DestroyHull is not ported.
// - The content hash is computed over a canonical little-endian serialization
//   (see hull_content_bytes) instead of raw struct bytes. Values differ from C
//   but are self-consistent, which is all the dedup map and determinism need.

use std::sync::Arc;

use crate::b3_assert;
use crate::b3_validate;
use crate::constants::linear_slop;
use crate::constants::overlap_slop;
use crate::core::{log, non_zero_hash, HASH_INIT, NULL_INDEX};
use crate::math_functions::*;
use crate::math_internal::*;
use crate::types::*;

const MARK_VISIBLE: i32 = 0;
const MARK_DELETE: i32 = 1;

// Final hull is index-encoded with u8, so vertex/edge/face counts are capped at u8::MAX.
const HULL_LIMIT: i32 = u8::MAX as i32;

struct QHVertex {
    // Intrusive list link. NULL_INDEX when detached.
    prev: i32,
    next: i32,

    conflict_face: i32,
    position: Vec3,

    // Index in the finalized hull, stamped during emit. NULL_INDEX until then.
    final_index: i32,
    reachable: bool,
}

struct QHHalfEdge {
    // Edge ring (CCW) around the owning face. Not an external list.
    // `next` doubles as the free-list link when retired (as in C).
    prev: i32,
    next: i32,

    origin: i32, // vertex index
    face: i32,   // face index
    twin: i32,   // edge index

    // Index in the finalized hull, stamped during emit. NULL_INDEX until then.
    final_index: i32,
}

struct QHFace {
    // Intrusive face-list link. `link_next` doubles as the free-list link when
    // retired (as in C, where it overlays link.next).
    link_prev: i32,
    link_next: i32,

    edge: i32,

    mark: i32,
    area: f32,
    plane: Plane,
    centroid: Vec3,
    max_conflict_distance: f32,

    // Sentinel vertex (in the vertex arena) heading this face's conflict list.
    conflict_head: i32,

    // Cached farthest conflict vertex (above HullBuilder::min_outside).
    // NULL_INDEX when no conflict above threshold.
    max_conflict: i32,

    // Index in the finalized hull, stamped during emit. NULL_INDEX until then.
    final_index: i32,
    flipped: bool,
}

// One frame of the iterative horizon DFS.
struct HorizonFrame {
    face: i32,
    start_edge: i32, // ring termination sentinel
    edge: i32,       // next edge to process
    started: bool,   // false until the first edge of this ring has been processed
}

// All working memory for one hull build.
struct HullBuilder {
    tolerance: f32,
    min_radius: f32,
    min_outside: f32,

    interior_point: Vec3,

    verts: Vec<QHVertex>,
    edges: Vec<QHHalfEdge>,
    faces: Vec<QHFace>,

    edge_free_head: i32,
    face_free_head: i32,

    // List sentinels (arena indices).
    orphaned_head: i32,
    vertex_head: i32,
    face_head: i32,

    // Reusable scratch buffers.
    horizon: Vec<i32>,      // edge indices
    cone: Vec<i32>,         // face indices
    merged_faces: Vec<i32>, // face indices

    // Final counts of the constructed hull. Populated by clean_hull.
    final_vertex_count: i32,
    final_half_edge_count: i32,
    final_face_count: i32,
}

impl HullBuilder {
    fn new() -> HullBuilder {
        let mut b = HullBuilder {
            tolerance: 0.0,
            min_radius: 0.0,
            min_outside: 0.0,
            interior_point: Vec3::ZERO,
            verts: Vec::new(),
            edges: Vec::new(),
            faces: Vec::new(),
            edge_free_head: NULL_INDEX,
            face_free_head: NULL_INDEX,
            orphaned_head: NULL_INDEX,
            vertex_head: NULL_INDEX,
            face_head: NULL_INDEX,
            horizon: Vec::new(),
            cone: Vec::new(),
            merged_faces: Vec::new(),
            final_vertex_count: 0,
            final_half_edge_count: 0,
            final_face_count: 0,
        };

        b.orphaned_head = b.alloc_vertex_slot(Vec3::ZERO);
        b.vlist_init(b.orphaned_head);
        b.vertex_head = b.alloc_vertex_slot(Vec3::ZERO);
        b.vlist_init(b.vertex_head);

        b.face_head = b.alloc_face_slot();
        let fh = b.face_head as usize;
        b.faces[fh].link_prev = b.face_head;
        b.faces[fh].link_next = b.face_head;

        b
    }

    // ------------------------------------------------------------------
    // Vertex intrusive list (shared by vertex list, orphan list, and the
    // per-face conflict lists, exactly like the C link field).
    // ------------------------------------------------------------------

    fn vlist_init(&mut self, head: i32) {
        self.verts[head as usize].prev = head;
        self.verts[head as usize].next = head;
    }

    fn vlist_contains(&self, node: i32) -> bool {
        self.verts[node as usize].prev != NULL_INDEX && self.verts[node as usize].next != NULL_INDEX
    }

    fn vlist_empty(&self, head: i32) -> bool {
        self.verts[head as usize].next == head
    }

    // Insert node before `where_`.
    fn vlist_insert(&mut self, node: i32, where_: i32) {
        b3_assert!(!self.vlist_contains(node) && self.vlist_contains(where_));

        let prev = self.verts[where_ as usize].prev;
        self.verts[node as usize].prev = prev;
        self.verts[node as usize].next = where_;

        self.verts[prev as usize].next = node;
        self.verts[where_ as usize].prev = node;
    }

    fn vlist_remove(&mut self, node: i32) {
        b3_assert!(self.vlist_contains(node));

        let prev = self.verts[node as usize].prev;
        let next = self.verts[node as usize].next;
        self.verts[prev as usize].next = next;
        self.verts[next as usize].prev = prev;

        self.verts[node as usize].prev = NULL_INDEX;
        self.verts[node as usize].next = NULL_INDEX;
    }

    fn vlist_push_back(&mut self, head: i32, node: i32) {
        // Faithful to C: b3QHList_PushBack inserts before head->prev.
        let where_ = self.verts[head as usize].prev;
        self.vlist_insert(node, where_);
    }

    // ------------------------------------------------------------------
    // Face intrusive list.
    // ------------------------------------------------------------------

    fn flist_contains(&self, node: i32) -> bool {
        self.faces[node as usize].link_prev != NULL_INDEX && self.faces[node as usize].link_next != NULL_INDEX
    }

    fn flist_insert(&mut self, node: i32, where_: i32) {
        b3_assert!(!self.flist_contains(node) && self.flist_contains(where_));

        let prev = self.faces[where_ as usize].link_prev;
        self.faces[node as usize].link_prev = prev;
        self.faces[node as usize].link_next = where_;

        self.faces[prev as usize].link_next = node;
        self.faces[where_ as usize].link_prev = node;
    }

    fn flist_remove(&mut self, node: i32) {
        b3_assert!(self.flist_contains(node));

        let prev = self.faces[node as usize].link_prev;
        let next = self.faces[node as usize].link_next;
        self.faces[prev as usize].link_next = next;
        self.faces[next as usize].link_prev = prev;

        self.faces[node as usize].link_prev = NULL_INDEX;
        self.faces[node as usize].link_next = NULL_INDEX;
    }

    fn flist_push_back(&mut self, head: i32, node: i32) {
        let where_ = self.faces[head as usize].link_prev;
        self.flist_insert(node, where_);
    }

    // ------------------------------------------------------------------
    // Arena allocation
    // ------------------------------------------------------------------

    fn alloc_vertex_slot(&mut self, position: Vec3) -> i32 {
        let index = self.verts.len() as i32;
        self.verts.push(QHVertex {
            prev: NULL_INDEX,
            next: NULL_INDEX,
            conflict_face: NULL_INDEX,
            position,
            final_index: NULL_INDEX,
            reachable: false,
        });
        index
    }

    fn alloc_face_slot(&mut self) -> i32 {
        // Each face slot owns a conflict-list sentinel vertex, mirroring the
        // b3QHVertex embedded in the C b3QHFace.
        let sentinel = self.alloc_vertex_slot(Vec3::ZERO);
        let index = self.faces.len() as i32;
        self.faces.push(QHFace {
            link_prev: NULL_INDEX,
            link_next: NULL_INDEX,
            edge: NULL_INDEX,
            mark: MARK_VISIBLE,
            area: 0.0,
            plane: Plane::default(),
            centroid: Vec3::ZERO,
            max_conflict_distance: 0.0,
            conflict_head: sentinel,
            max_conflict: NULL_INDEX,
            final_index: NULL_INDEX,
            flipped: false,
        });
        index
    }

    fn new_vertex(&mut self, position: Vec3) -> i32 {
        self.alloc_vertex_slot(position)
    }

    fn new_edge(&mut self) -> i32 {
        let edge;
        if self.edge_free_head != NULL_INDEX {
            edge = self.edge_free_head;
            self.edge_free_head = self.edges[edge as usize].next;
        } else {
            edge = self.edges.len() as i32;
            self.edges.push(QHHalfEdge {
                prev: NULL_INDEX,
                next: NULL_INDEX,
                origin: NULL_INDEX,
                face: NULL_INDEX,
                twin: NULL_INDEX,
                final_index: NULL_INDEX,
            });
        }
        // All other fields are written by new_face immediately after.
        self.edges[edge as usize].final_index = NULL_INDEX;
        edge
    }

    fn retire_edge(&mut self, edge: i32) {
        self.edges[edge as usize].next = self.edge_free_head;
        self.edge_free_head = edge;
    }

    fn new_face(&mut self, v1: i32, v2: i32, v3: i32) -> i32 {
        let face;
        if self.face_free_head != NULL_INDEX {
            face = self.face_free_head;
            // link_next was used as free-list pointer; recover next head before we clobber.
            self.face_free_head = self.faces[face as usize].link_next;
        } else {
            face = self.alloc_face_slot();
        }

        {
            let f = &mut self.faces[face as usize];
            f.link_prev = NULL_INDEX;
            f.link_next = NULL_INDEX;
            f.max_conflict = NULL_INDEX;
            f.max_conflict_distance = 0.0;
            f.final_index = NULL_INDEX;
        }

        let edge1 = self.new_edge();
        let edge2 = self.new_edge();
        let edge3 = self.new_edge();

        let p1 = self.verts[v1 as usize].position;
        let p2 = self.verts[v2 as usize].position;
        let p3 = self.verts[v3 as usize].position;

        let mut plane = Plane::default();
        plane.normal = cross(sub(p2, p1), sub(p3, p1));
        let mut edge_length = 0.0f32;
        plane.normal = get_length_and_normalize(&mut edge_length, plane.normal);
        plane.offset = dot(plane.normal, p1);

        let area = 0.5 * edge_length;

        let interior_point = self.interior_point;
        let conflict_head = self.faces[face as usize].conflict_head;
        {
            let f = &mut self.faces[face as usize];
            f.edge = edge1;
            f.mark = MARK_VISIBLE;
            f.area = area;
            f.centroid = mul_sv(1.0 / 3.0, add(p1, add(p2, p3)));
            f.plane = plane;
            f.flipped = plane_separation(plane, interior_point) > 0.0;
        }
        self.vlist_init(conflict_head);

        {
            let e = &mut self.edges[edge1 as usize];
            e.prev = edge3;
            e.next = edge2;
            e.origin = v1;
            e.face = face;
            e.twin = NULL_INDEX;
        }
        {
            let e = &mut self.edges[edge2 as usize];
            e.prev = edge1;
            e.next = edge3;
            e.origin = v2;
            e.face = face;
            e.twin = NULL_INDEX;
        }
        {
            let e = &mut self.edges[edge3 as usize];
            e.prev = edge2;
            e.next = edge1;
            e.origin = v3;
            e.face = face;
            e.twin = NULL_INDEX;
        }

        face
    }

    // Remove face from face list if still linked, clear its edge pointer, then
    // push onto the face free list (link_next holds the free-list pointer).
    fn retire_face(&mut self, face: i32) {
        if self.flist_contains(face) {
            self.flist_remove(face);
        }
        self.faces[face as usize].edge = NULL_INDEX;
        self.faces[face as usize].link_next = self.face_free_head;
        self.face_free_head = face;
    }

    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    fn compute_tolerance(&mut self, points: &[Vec3]) {
        let bounds = build_bounds(points);
        let max_abs = max(abs(bounds.lower_bound), abs(bounds.upper_bound));

        let max_sum = max_abs.x + max_abs.y + max_abs.z;
        let max_coord = max_float(max_abs.x, max_float(max_abs.y, max_abs.z));
        let max_distance = min_float(SQRT3 * max_coord, max_sum);

        let tolerance = (3.0 * max_distance * 1.01 + max_coord) * f32::EPSILON;

        self.tolerance = tolerance;
        self.min_radius = 4.0 * self.tolerance;
        self.min_outside = 2.0 * self.min_radius;
        b3_assert!(self.min_radius < self.min_outside + 3.0 * f32::EPSILON);
    }

    fn build_initial_hull(&mut self, points: &[Vec3]) -> bool {
        let point_count = points.len() as i32;

        let mut index1 = NULL_INDEX;
        let mut index2 = NULL_INDEX;
        find_farthest_points_along_cardinal_axes(&mut index1, &mut index2, self.tolerance, points);
        if index1 < 0 || index2 < 0 {
            return false;
        }

        let mut index3 = find_farthest_point_from_line(index1, index2, self.tolerance, points);
        if index3 < 0 {
            return false;
        }

        let index4 = find_farthest_point_from_plane(index1, index2, index3, self.tolerance, points);
        if index4 < 0 {
            return false;
        }

        let v1 = sub(points[index1 as usize], points[index4 as usize]);
        let v2 = sub(points[index2 as usize], points[index4 as usize]);
        let v3 = sub(points[index3 as usize], points[index4 as usize]);

        let mut index2 = index2;
        if scalar_triple_product(v1, v2, v3) < 0.0 {
            std::mem::swap(&mut index2, &mut index3);
        }

        self.interior_point = Vec3::ZERO;
        self.interior_point = add(self.interior_point, points[index1 as usize]);
        self.interior_point = add(self.interior_point, points[index2 as usize]);
        self.interior_point = add(self.interior_point, points[index3 as usize]);
        self.interior_point = add(self.interior_point, points[index4 as usize]);
        self.interior_point = mul_sv(0.25, self.interior_point);

        let vertex1 = self.new_vertex(points[index1 as usize]);
        self.vlist_push_back(self.vertex_head, vertex1);
        let vertex2 = self.new_vertex(points[index2 as usize]);
        self.vlist_push_back(self.vertex_head, vertex2);
        let vertex3 = self.new_vertex(points[index3 as usize]);
        self.vlist_push_back(self.vertex_head, vertex3);
        let vertex4 = self.new_vertex(points[index4 as usize]);
        self.vlist_push_back(self.vertex_head, vertex4);

        let face1 = self.new_face(vertex1, vertex2, vertex3);
        self.flist_push_back(self.face_head, face1);
        let face2 = self.new_face(vertex4, vertex2, vertex1);
        self.flist_push_back(self.face_head, face2);
        let face3 = self.new_face(vertex4, vertex3, vertex2);
        self.flist_push_back(self.face_head, face3);
        let face4 = self.new_face(vertex4, vertex1, vertex3);
        self.flist_push_back(self.face_head, face4);

        self.link_faces(face1, 0, face2, 1);
        self.link_faces(face1, 1, face3, 1);
        self.link_faces(face1, 2, face4, 1);

        self.link_faces(face2, 0, face3, 2);
        self.link_faces(face3, 0, face4, 2);
        self.link_faces(face4, 0, face2, 2);

        b3_assert!(self.check_consistency(face1));
        b3_assert!(self.check_consistency(face2));
        b3_assert!(self.check_consistency(face3));
        b3_assert!(self.check_consistency(face4));

        for index in 0..point_count {
            if index == index1 || index == index2 || index == index3 || index == index4 {
                continue;
            }

            let point = points[index as usize];

            let mut max_distance = self.min_outside;
            let mut max_face = NULL_INDEX;

            let mut node = self.faces[self.face_head as usize].link_next;
            while node != self.face_head {
                let face = node;
                let distance = plane_separation(self.faces[face as usize].plane, point);
                if distance > max_distance {
                    max_distance = distance;
                    max_face = face;
                }
                node = self.faces[node as usize].link_next;
            }

            if max_face != NULL_INDEX {
                let vertex = self.new_vertex(point);
                self.verts[vertex as usize].conflict_face = max_face;
                let head = self.faces[max_face as usize].conflict_head;
                self.vlist_push_back(head, vertex);
                if max_distance > self.faces[max_face as usize].max_conflict_distance {
                    self.faces[max_face as usize].max_conflict_distance = max_distance;
                    self.faces[max_face as usize].max_conflict = vertex;
                }
            }
        }

        true
    }

    fn link_face(&mut self, face: i32, index: i32, twin: i32) {
        b3_assert!(face != self.edges[twin as usize].face);

        let mut edge = self.faces[face as usize].edge;
        let mut index = index;
        while index > 0 {
            b3_assert!(self.edges[edge as usize].face == face);
            edge = self.edges[edge as usize].next;
            index -= 1;
        }

        b3_assert!(edge != twin);
        self.edges[edge as usize].twin = twin;
        self.edges[twin as usize].twin = edge;
    }

    fn link_faces(&mut self, face1: i32, index1: i32, face2: i32, index2: i32) {
        b3_assert!(face1 != face2);

        let mut edge1 = self.faces[face1 as usize].edge;
        let mut index1 = index1;
        while index1 > 0 {
            edge1 = self.edges[edge1 as usize].next;
            index1 -= 1;
        }

        let mut edge2 = self.faces[face2 as usize].edge;
        let mut index2 = index2;
        while index2 > 0 {
            edge2 = self.edges[edge2 as usize].next;
            index2 -= 1;
        }

        b3_assert!(edge1 != edge2);
        self.edges[edge1 as usize].twin = edge2;
        self.edges[edge2 as usize].twin = edge1;
    }

    fn vertex_count_of_face(&self, face: i32) -> i32 {
        let mut count = 0;
        let start = self.faces[face as usize].edge;
        let mut edge = start;
        loop {
            count += 1;
            edge = self.edges[edge as usize].next;
            if edge == start {
                break;
            }
        }
        count
    }

    fn is_edge_convex(&self, edge: i32, tolerance: f32) -> bool {
        let e = &self.edges[edge as usize];
        let twin_face = self.edges[e.twin as usize].face;
        let distance = plane_separation(self.faces[e.face as usize].plane, self.faces[twin_face as usize].centroid);
        distance < -tolerance
    }

    fn is_edge_concave(&self, edge: i32, tolerance: f32) -> bool {
        let e = &self.edges[edge as usize];
        let twin_face = self.edges[e.twin as usize].face;
        let distance = plane_separation(self.faces[e.face as usize].plane, self.faces[twin_face as usize].centroid);
        distance > tolerance
    }

    fn newell_plane(&mut self, face: i32) {
        let mut count = 0;
        let mut centroid = Vec3::ZERO;
        let mut normal = Vec3::ZERO;

        let start = self.faces[face as usize].edge;
        b3_assert!(self.edges[start as usize].face == face);

        // Use the first vertex as the origin to reduce round-off
        let origin = self.verts[self.edges[start as usize].origin as usize].position;

        let mut edge = start;
        loop {
            let twin = self.edges[edge as usize].twin;
            b3_assert!(self.edges[twin as usize].twin == edge);

            let v1 = sub(self.verts[self.edges[edge as usize].origin as usize].position, origin);
            let v2 = sub(self.verts[self.edges[twin as usize].origin as usize].position, origin);

            count += 1;
            centroid = add(centroid, v1);
            normal.x += (v1.y - v2.y) * (v1.z + v2.z);
            normal.y += (v1.z - v2.z) * (v1.x + v2.x);
            normal.z += (v1.x - v2.x) * (v1.y + v2.y);

            edge = self.edges[edge as usize].next;
            if edge == start {
                break;
            }
        }

        b3_assert!(count > 0);
        centroid = mul_sv(1.0 / count as f32, centroid);
        centroid = add(centroid, origin);

        let len = length(normal);
        b3_validate!(len > 0.0);
        normal = mul_sv(1.0 / len, normal);

        let f = &mut self.faces[face as usize];
        f.centroid = centroid;
        f.plane = make_plane_from_normal_and_point(normal, centroid);
        f.area = 0.5 * len;
    }

    #[cfg(debug_assertions)]
    fn check_consistency(&self, face: i32) -> bool {
        if self.faces[face as usize].mark == MARK_DELETE {
            return false;
        }

        if self.vertex_count_of_face(face) < 3 {
            return false;
        }

        let start = self.faces[face as usize].edge;
        let mut edge = start;

        loop {
            let twin = self.edges[edge as usize].twin;

            if twin == NULL_INDEX {
                return false;
            }
            let twin_face = self.edges[twin as usize].face;
            if twin_face == NULL_INDEX {
                return false;
            }
            if twin_face == face {
                return false;
            }
            if self.faces[twin_face as usize].mark == MARK_DELETE {
                return false;
            }
            if self.edges[twin as usize].twin != edge {
                return false;
            }
            let next = self.edges[edge as usize].next;
            if self.edges[next as usize].origin != self.edges[twin as usize].origin {
                return false;
            }
            let twin_next = self.edges[twin as usize].next;
            if self.edges[edge as usize].origin != self.edges[twin_next as usize].origin {
                return false;
            }
            if self.edges[edge as usize].face != face {
                return false;
            }

            edge = next;
            if edge == start {
                break;
            }
        }

        true
    }

    #[cfg(not(debug_assertions))]
    fn check_consistency(&self, _face: i32) -> bool {
        true
    }

    // Recompute the farthest-conflict cache after a face's plane changes.
    fn recache_conflicts(&mut self, face: i32) {
        let mut max_vertex = NULL_INDEX;
        let mut max_distance = self.min_outside;

        let head = self.faces[face as usize].conflict_head;
        let plane = self.faces[face as usize].plane;
        let mut node = self.verts[head as usize].next;
        while node != head {
            let distance = plane_separation(plane, self.verts[node as usize].position);
            if distance > max_distance {
                max_distance = distance;
                max_vertex = node;
            }
            node = self.verts[node as usize].next;
        }

        self.faces[face as usize].max_conflict = max_vertex;
        self.faces[face as usize].max_conflict_distance = max_distance;
    }

    fn next_conflict_vertex(&self) -> i32 {
        let mut max_vertex = NULL_INDEX;
        let mut max_distance = self.min_outside;

        let mut face_node = self.faces[self.face_head as usize].link_next;
        while face_node != self.face_head {
            let face = &self.faces[face_node as usize];
            if face.max_conflict != NULL_INDEX && face.max_conflict_distance > max_distance {
                max_distance = face.max_conflict_distance;
                max_vertex = face.max_conflict;
            }
            face_node = face.link_next;
        }

        max_vertex
    }

    // Move every conflict vertex of `face` onto the orphaned list and clear their conflict_face.
    fn drain_conflict_list(&mut self, face: i32) {
        let head = self.faces[face as usize].conflict_head;
        let mut node = self.verts[head as usize].next;
        while node != head {
            let orphan = node;
            node = self.verts[node as usize].next;

            self.verts[orphan as usize].conflict_face = NULL_INDEX;
            self.vlist_remove(orphan);
            self.vlist_push_back(self.orphaned_head, orphan);
        }
        b3_assert!(self.vlist_empty(head));
    }

    // Mark a face for deletion, drain its conflict list, and produce a fresh DFS frame for it.
    // `entry_edge` is the half-edge in `face` whose twin lies in the just-deleted parent face,
    // or NULL_INDEX for the seed.
    fn enter_horizon_face(&mut self, face: i32, entry_edge: i32) -> HorizonFrame {
        self.faces[face as usize].mark = MARK_DELETE;
        self.drain_conflict_list(face);

        if entry_edge != NULL_INDEX {
            HorizonFrame {
                face,
                started: false,
                start_edge: entry_edge,
                edge: self.edges[entry_edge as usize].next,
            }
        } else {
            let e = self.faces[face as usize].edge;
            HorizonFrame { face, started: false, start_edge: e, edge: e }
        }
    }

    fn build_horizon(&mut self, apex: i32, seed: i32) {
        let apex_position = self.verts[apex as usize].position;

        let mut stack: Vec<HorizonFrame> = Vec::new();
        let frame = self.enter_horizon_face(seed, NULL_INDEX);
        stack.push(frame);

        while !stack.is_empty() {
            let top = stack.len() - 1;
            {
                let f = &stack[top];
                if f.started && f.edge == f.start_edge {
                    stack.pop();
                    continue;
                }
            }
            stack[top].started = true;

            let edge = stack[top].edge;
            let twin = self.edges[edge as usize].twin;
            stack[top].edge = self.edges[edge as usize].next;

            let twin_face = self.edges[twin as usize].face;
            if self.faces[twin_face as usize].mark != MARK_VISIBLE {
                continue;
            }

            let distance = plane_separation(self.faces[twin_face as usize].plane, apex_position);
            if distance > self.min_radius {
                let frame = self.enter_horizon_face(twin_face, twin);
                stack.push(frame);
            } else {
                self.horizon.push(edge);
            }
        }
    }

    fn build_cone(&mut self, apex: i32) {
        for i in 0..self.horizon.len() {
            let edge = self.horizon[i];
            let twin = self.edges[edge as usize].twin;
            b3_assert!(self.edges[twin as usize].twin == edge);

            let origin = self.edges[edge as usize].origin;
            let twin_origin = self.edges[twin as usize].origin;
            let face = self.new_face(apex, origin, twin_origin);
            self.cone.push(face);

            self.link_face(face, 1, twin);
        }

        let mut face1 = self.cone[self.cone.len() - 1];
        for i in 0..self.cone.len() {
            let face2 = self.cone[i];
            self.link_faces(face1, 2, face2, 0);
            face1 = face2;
        }
    }

    // Retire half-edges in the half-open ring range [begin, end).
    fn destroy_edges(&mut self, begin: i32, end: i32) {
        let mut edge = begin;
        while edge != end {
            let next = self.edges[edge as usize].next;
            self.retire_edge(edge);
            edge = next;
        }
    }

    fn connect_edges(&mut self, prev: i32, next: i32) {
        b3_assert!(prev != next);
        b3_assert!(self.edges[prev as usize].face == self.edges[next as usize].face);

        let prev_twin = self.edges[prev as usize].twin;
        let next_twin = self.edges[next as usize].twin;

        // If both shared neighbors are the same face, prev and next together would orphan that face.
        if self.edges[prev_twin as usize].face == self.edges[next_twin as usize].face {
            // next is redundant.
            let next_face = self.edges[next as usize].face;
            if self.faces[next_face as usize].edge == next {
                self.faces[next_face as usize].edge = prev;
            }

            let twin;
            let opposing_face = self.edges[prev_twin as usize].face;
            if self.vertex_count_of_face(opposing_face) == 3 {
                // Capture all 3 half-edges of the dead triangle before the rewire overwrites prev->twin.
                let dead_edge0 = prev_twin;
                let dead_edge1 = next_twin;
                let dead_edge2 = self.edges[next_twin as usize].prev;

                twin = self.edges[dead_edge2 as usize].twin;
                b3_assert!(self.faces[self.edges[twin as usize].face as usize].mark != MARK_DELETE);

                self.faces[opposing_face as usize].mark = MARK_DELETE;
                self.merged_faces.push(opposing_face);

                let next_next = self.edges[next as usize].next;
                self.edges[prev as usize].next = next_next;
                self.edges[next_next as usize].prev = prev;

                self.edges[prev as usize].twin = twin;
                self.edges[twin as usize].twin = prev;

                // Drop the redundant vertex (slot abandoned in the arena).
                let next_origin = self.edges[next as usize].origin;
                self.vlist_remove(next_origin);

                // Retire the 3 half-edges of the dead triangle now that the rewire is complete.
                self.retire_edge(dead_edge0);
                self.retire_edge(dead_edge1);
                self.retire_edge(dead_edge2);
            } else {
                twin = next_twin;

                let twin_face = self.edges[twin as usize].face;
                if self.faces[twin_face as usize].edge == prev_twin {
                    self.faces[twin_face as usize].edge = twin;
                }

                let prev_twin_next = self.edges[prev_twin as usize].next;
                self.edges[twin as usize].next = prev_twin_next;
                self.edges[prev_twin_next as usize].prev = twin;
                // prev->twin slot is retired to the edge free list.
                self.retire_edge(prev_twin);

                let next_next = self.edges[next as usize].next;
                self.edges[prev as usize].next = next_next;
                self.edges[next_next as usize].prev = prev;

                self.edges[prev as usize].twin = twin;
                self.edges[twin as usize].twin = prev;

                // Drop the redundant vertex (slot abandoned in the arena).
                let next_origin = self.edges[next as usize].origin;
                self.vlist_remove(next_origin);
            }

            // Twin->face changed shape; recompute its plane and refresh its cached max conflict.
            let twin_face = self.edges[twin as usize].face;
            self.newell_plane(twin_face);
            self.recache_conflicts(twin_face);
        } else {
            self.edges[prev as usize].next = next;
            self.edges[next as usize].prev = prev;
        }
    }

    fn absorb_faces(&mut self, face: i32) {
        for i in 0..self.merged_faces.len() {
            let merged = self.merged_faces[i];
            b3_assert!(self.faces[merged as usize].mark == MARK_DELETE);
            let head = self.faces[merged as usize].conflict_head;

            let mut node = self.verts[head as usize].next;
            while node != head {
                let vertex = node;
                node = self.verts[node as usize].next;

                self.vlist_remove(vertex);

                let distance = plane_separation(self.faces[face as usize].plane, self.verts[vertex as usize].position);
                if distance > self.min_outside {
                    let face_head = self.faces[face as usize].conflict_head;
                    self.vlist_push_back(face_head, vertex);
                    self.verts[vertex as usize].conflict_face = face;
                    if distance > self.faces[face as usize].max_conflict_distance {
                        self.faces[face as usize].max_conflict_distance = distance;
                        self.faces[face as usize].max_conflict = vertex;
                    }
                } else {
                    self.vlist_push_back(self.orphaned_head, vertex);
                    self.verts[vertex as usize].conflict_face = NULL_INDEX;
                }
            }

            b3_assert!(self.vlist_empty(head));

            // Conflict list is now drained. Retire this face to the free list.
            self.retire_face(merged);
        }
    }

    fn connect_faces(&mut self, edge: i32) {
        let face = self.edges[edge as usize].face;

        let twin = self.edges[edge as usize].twin;

        let mut edge_prev = self.edges[edge as usize].prev;
        let mut edge_next = self.edges[edge as usize].next;
        let mut twin_prev = self.edges[twin as usize].prev;
        let mut twin_next = self.edges[twin as usize].next;

        let twin_face = self.edges[twin as usize].face;

        while self.edges[self.edges[edge_prev as usize].twin as usize].face == twin_face {
            b3_assert!(self.edges[edge_prev as usize].twin == twin_next);
            b3_assert!(self.edges[twin_next as usize].twin == edge_prev);

            edge_prev = self.edges[edge_prev as usize].prev;
            twin_next = self.edges[twin_next as usize].next;
        }
        b3_assert!(
            self.edges[edge_prev as usize].face != self.edges[twin_next as usize].face
        );

        while self.edges[self.edges[edge_next as usize].twin as usize].face == twin_face {
            b3_assert!(self.edges[edge_next as usize].twin == twin_prev);
            b3_assert!(self.edges[twin_prev as usize].twin == edge_next);

            edge_next = self.edges[edge_next as usize].next;
            twin_prev = self.edges[twin_prev as usize].prev;
        }
        b3_assert!(
            self.edges[edge_next as usize].face != self.edges[twin_prev as usize].face
        );

        self.faces[face as usize].edge = edge_prev;

        // Discard opposing face. merged_faces is single-buffered: connect_faces does not nest.
        self.merged_faces.clear();
        self.merged_faces.push(twin_face);
        self.faces[twin_face as usize].mark = MARK_DELETE;
        self.faces[twin_face as usize].edge = NULL_INDEX;

        let mut absorbed = twin_next;
        let stop = self.edges[twin_prev as usize].next;
        while absorbed != stop {
            self.edges[absorbed as usize].face = face;
            absorbed = self.edges[absorbed as usize].next;
        }

        let ep_next = self.edges[edge_prev as usize].next;
        self.destroy_edges(ep_next, edge_next);
        let tp_next = self.edges[twin_prev as usize].next;
        self.destroy_edges(tp_next, twin_next);

        self.connect_edges(edge_prev, twin_next);
        self.connect_edges(twin_prev, edge_next);

        self.newell_plane(face);
        // Existing conflicts now have stale distances under the new plane; absorb_faces will then
        // add more incrementally and update the cache as it goes.
        self.recache_conflicts(face);

        b3_assert!(self.check_consistency(face));

        self.absorb_faces(face);
    }

    fn merge_concave(&mut self, face: i32) -> bool {
        let start = self.faces[face as usize].edge;
        let mut edge = start;

        loop {
            let twin = self.edges[edge as usize].twin;

            if self.is_edge_concave(edge, self.min_radius) || self.is_edge_concave(twin, self.min_radius) {
                self.connect_faces(edge);
                return true;
            }

            edge = self.edges[edge as usize].next;
            if edge == start {
                break;
            }
        }

        false
    }

    fn merge_coplanar(&mut self, face: i32) -> bool {
        let start = self.faces[face as usize].edge;
        let mut edge = start;

        loop {
            let twin = self.edges[edge as usize].twin;

            if !self.is_edge_convex(edge, self.min_radius) || !self.is_edge_convex(twin, self.min_radius) {
                self.connect_faces(edge);
                return true;
            }

            edge = self.edges[edge as usize].next;
            if edge == start {
                break;
            }
        }

        false
    }

    fn merge_faces(&mut self) {
        for i in 0..self.cone.len() {
            let face = self.cone[i];
            if self.faces[face as usize].mark == MARK_VISIBLE && self.faces[face as usize].flipped {
                self.faces[face as usize].flipped = false;

                let mut best_area = 0.0f32;
                let mut best_edge = NULL_INDEX;

                let start = self.faces[face as usize].edge;
                let mut edge = start;
                loop {
                    let twin = self.edges[edge as usize].twin;
                    let twin_face = self.edges[twin as usize].face;
                    let area = self.faces[twin_face as usize].area;
                    if area > best_area {
                        best_area = area;
                        best_edge = edge;
                    }

                    edge = self.edges[edge as usize].next;
                    if edge == start {
                        break;
                    }
                }

                b3_assert!(best_edge != NULL_INDEX);
                self.connect_faces(best_edge);
            }
        }

        for i in 0..self.cone.len() {
            let face = self.cone[i];
            if self.faces[face as usize].mark == MARK_VISIBLE {
                while self.merge_concave(face) {}
            }
        }

        for i in 0..self.cone.len() {
            let face = self.cone[i];
            if self.faces[face as usize].mark == MARK_VISIBLE {
                while self.merge_coplanar(face) {}
            }
        }
    }

    fn resolve_vertices(&mut self) {
        let mut node = self.verts[self.orphaned_head as usize].next;
        while node != self.orphaned_head {
            let vertex = node;
            node = self.verts[node as usize].next;
            self.vlist_remove(vertex);

            let mut max_distance = self.min_outside;
            let mut max_face = NULL_INDEX;

            for i in 0..self.cone.len() {
                let cone_face = self.cone[i];
                if self.faces[cone_face as usize].mark == MARK_VISIBLE {
                    let distance =
                        plane_separation(self.faces[cone_face as usize].plane, self.verts[vertex as usize].position);
                    if distance > max_distance {
                        max_distance = distance;
                        max_face = cone_face;
                    }
                }
            }

            if max_face != NULL_INDEX {
                b3_assert!(self.faces[max_face as usize].mark == MARK_VISIBLE);
                let head = self.faces[max_face as usize].conflict_head;
                self.vlist_push_back(head, vertex);
                self.verts[vertex as usize].conflict_face = max_face;
                if max_distance > self.faces[max_face as usize].max_conflict_distance {
                    self.faces[max_face as usize].max_conflict_distance = max_distance;
                    self.faces[max_face as usize].max_conflict = vertex;
                }
            }
            // Otherwise: vertex is interior to the hull. Its slot in the arena is abandoned.
        }

        b3_assert!(self.vlist_empty(self.orphaned_head));
    }

    fn resolve_faces(&mut self) {
        // Splice deleted faces out of the face list. Faces already retired by absorb_faces are no
        // longer on the face list, so we guard with flist_contains before removing.
        let mut node = self.faces[self.face_head as usize].link_next;
        while node != self.face_head {
            let face = node;
            node = self.faces[node as usize].link_next;

            if self.faces[face as usize].mark == MARK_DELETE && self.flist_contains(face) {
                b3_assert!(self.vlist_empty(self.faces[face as usize].conflict_head));
                self.flist_remove(face);
            }
        }

        for i in 0..self.cone.len() {
            let face = self.cone[i];
            if self.faces[face as usize].mark == MARK_DELETE {
                continue;
            }
            self.flist_push_back(self.face_head, face);
        }
    }

    fn add_vertex_to_hull(&mut self, vertex: i32) {
        let face = self.verts[vertex as usize].conflict_face;
        self.verts[vertex as usize].conflict_face = NULL_INDEX;
        self.vlist_remove(vertex);
        self.vlist_push_back(self.vertex_head, vertex);

        self.horizon.clear();
        self.build_horizon(vertex, face);
        b3_assert!(self.horizon.len() >= 3);

        self.cone.clear();
        self.build_cone(vertex);
        b3_assert!(self.cone.len() >= 3);

        self.merge_faces();
        self.resolve_vertices();
        self.resolve_faces();
    }

    fn clean_hull(&mut self, origin: Vec3) {
        let mut face_count = 0;
        let mut half_edge_count = 0;

        let mut face_node = self.faces[self.face_head as usize].link_next;
        while face_node != self.face_head {
            let face = face_node;
            let start = self.faces[face as usize].edge;
            let mut edge = start;

            loop {
                let origin_vertex = self.edges[edge as usize].origin;
                self.verts[origin_vertex as usize].reachable = true;
                edge = self.edges[edge as usize].next;
                half_edge_count += 1;
                if edge == start {
                    break;
                }
            }

            {
                let f = &mut self.faces[face as usize];
                f.plane.offset += dot(f.plane.normal, origin);
                f.centroid = add(f.centroid, origin);
            }
            face_count += 1;

            face_node = self.faces[face as usize].link_next;
        }

        let mut vertex_count = 0;
        let mut node = self.verts[self.vertex_head as usize].next;
        while node != self.vertex_head {
            let vertex = node;
            node = self.verts[node as usize].next;

            if !self.verts[vertex as usize].reachable {
                self.vlist_remove(vertex);
            } else {
                let v = &mut self.verts[vertex as usize];
                v.position = add(v.position, origin);
                vertex_count += 1;
            }
        }

        self.interior_point = add(self.interior_point, origin);

        self.final_vertex_count = vertex_count;
        self.final_half_edge_count = half_edge_count;
        self.final_face_count = face_count;
    }

    #[cfg(debug_assertions)]
    fn is_consistent(&self) -> bool {
        let v = self.final_vertex_count;
        let e = self.final_half_edge_count / 2;
        let f = self.final_face_count;

        if v - e + f != 2 {
            return false;
        }

        let mut face_node = self.faces[self.face_head as usize].link_next;
        while face_node != self.face_head {
            let face = face_node;
            let start = self.faces[face as usize].edge;
            if self.edges[start as usize].face != face {
                return false;
            }

            if !self.check_consistency(face) {
                return false;
            }

            if plane_separation(self.faces[face as usize].plane, self.interior_point) > 0.0 {
                return false;
            }

            if self.faces[face as usize].mark != MARK_VISIBLE {
                return false;
            }

            let mut edge = start;
            loop {
                let next = self.edges[edge as usize].next;
                let prev = self.edges[edge as usize].prev;
                let twin = self.edges[edge as usize].twin;

                if self.edges[next as usize].origin != self.edges[twin as usize].origin {
                    return false;
                }
                if self.edges[prev as usize].next != edge {
                    return false;
                }
                if self.edges[next as usize].prev != edge {
                    return false;
                }
                if self.edges[twin as usize].twin != edge {
                    return false;
                }
                if self.edges[edge as usize].face != face {
                    return false;
                }
                let p1 = self.verts[self.edges[edge as usize].origin as usize].position;
                let p2 = self.verts[self.edges[twin as usize].origin as usize].position;
                if distance_squared(p1, p2) < 1000.0 * f32::MIN_POSITIVE {
                    return false;
                }

                edge = next;
                if edge == start {
                    break;
                }
            }

            face_node = self.faces[face as usize].link_next;
        }

        true
    }

    fn has_hull(&self) -> bool {
        let v = self.final_vertex_count;
        let e = self.final_half_edge_count / 2;
        let f = self.final_face_count;
        v - e + f == 2 && f >= 4
    }

    // Build the entire hull. Returns true iff the result satisfies Euler's identity.
    fn construct(&mut self, points: &[Vec3], max_vertex_count: i32, origin: Vec3) -> bool {
        let point_count = points.len() as i32;
        if point_count < 4 {
            return false;
        }

        let shifted_points: Vec<Vec3> = points.iter().map(|p| sub(*p, origin)).collect();

        self.compute_tolerance(&shifted_points);
        if !self.build_initial_hull(&shifted_points) {
            return false;
        }

        let mut budget = clamp_int(max_vertex_count - 4, 0, HULL_LIMIT - 4);

        let mut vertex = self.next_conflict_vertex();
        while vertex != NULL_INDEX && budget > 0 {
            self.add_vertex_to_hull(vertex);
            vertex = self.next_conflict_vertex();
            budget -= 1;
        }

        self.clean_hull(origin);

        #[cfg(debug_assertions)]
        b3_assert!(self.is_consistent());

        self.has_hull()
    }
}

fn build_bounds(vertices: &[Vec3]) -> AABB {
    let mut bounds = BOUNDS3_EMPTY;
    for v in vertices {
        bounds.lower_bound = min(bounds.lower_bound, *v);
        bounds.upper_bound = max(bounds.upper_bound, *v);
    }
    bounds
}

const AXIS_X: usize = 0;
const AXIS_Y: usize = 1;
const AXIS_Z: usize = 2;

fn find_farthest_points_along_cardinal_axes(
    index1_out: &mut i32,
    index2_out: &mut i32,
    tolerance: f32,
    points: &[Vec3],
) {
    *index1_out = NULL_INDEX;
    *index2_out = NULL_INDEX;

    let v0 = points[0];
    let mut min_pt = [v0, v0, v0];
    let mut max_pt = [v0, v0, v0];

    let mut min_index = [0i32, 0, 0];
    let mut max_index = [0i32, 0, 0];

    for i in 1..points.len() {
        let v = points[i];

        if v.x < min_pt[AXIS_X].x {
            min_pt[AXIS_X] = v;
            min_index[AXIS_X] = i as i32;
        } else if v.x > max_pt[AXIS_X].x {
            max_pt[AXIS_X] = v;
            max_index[AXIS_X] = i as i32;
        }

        if v.y < min_pt[AXIS_Y].y {
            min_pt[AXIS_Y] = v;
            min_index[AXIS_Y] = i as i32;
        } else if v.y > max_pt[AXIS_Y].y {
            max_pt[AXIS_Y] = v;
            max_index[AXIS_Y] = i as i32;
        }

        if v.z < min_pt[AXIS_Z].z {
            min_pt[AXIS_Z] = v;
            min_index[AXIS_Z] = i as i32;
        } else if v.z > max_pt[AXIS_Z].z {
            max_pt[AXIS_Z] = v;
            max_index[AXIS_Z] = i as i32;
        }
    }

    let distance = vec3(
        max_pt[AXIS_X].x - min_pt[AXIS_X].x,
        max_pt[AXIS_Y].y - min_pt[AXIS_Y].y,
        max_pt[AXIS_Z].z - min_pt[AXIS_Z].z,
    );

    let distance_array = [distance.x, distance.y, distance.z];
    let max_element = max_element_index(distance) as usize;

    if distance_array[max_element] > 2.0 * tolerance {
        *index1_out = min_index[max_element];
        *index2_out = max_index[max_element];
    }
}

fn find_farthest_point_from_line(index1: i32, index2: i32, tolerance: f32, points: &[Vec3]) -> i32 {
    let a = points[index1 as usize];
    let b = points[index2 as usize];

    // |ap x ab|^2 / |ab|^2 is the squared perpendicular distance from p to the line.
    // Compares against (2 * tolerance)^2
    let ab = sub(b, a);
    let ab_length_sqr = dot(ab, ab);
    b3_assert!(ab_length_sqr > 0.0);

    let inv_ab_length_sqr = 1.0 / ab_length_sqr;
    let mut max_distance_sqr = 4.0 * tolerance * tolerance;
    let mut max_index = NULL_INDEX;

    for i in 0..points.len() as i32 {
        if i == index1 || i == index2 {
            continue;
        }

        let ap = sub(points[i as usize], a);
        let c = cross(ap, ab);
        let distance_sqr = dot(c, c) * inv_ab_length_sqr;
        if distance_sqr > max_distance_sqr {
            max_distance_sqr = distance_sqr;
            max_index = i;
        }
    }

    max_index
}

fn find_farthest_point_from_plane(index1: i32, index2: i32, index3: i32, tolerance: f32, points: &[Vec3]) -> i32 {
    let a = points[index1 as usize];
    let b = points[index2 as usize];
    let c = points[index3 as usize];

    let plane = make_plane_from_points(a, b, c);

    let mut max_distance = 2.0 * tolerance;
    let mut max_index = NULL_INDEX;

    for i in 0..points.len() as i32 {
        if i == index1 || i == index2 || i == index3 {
            continue;
        }

        let distance = abs_float(plane_separation(plane, points[i as usize]));
        if distance > max_distance {
            max_distance = distance;
            max_index = i;
        }
    }

    max_index
}

// ---------------------------------------------------------------------------
// Hull support and validation
// ---------------------------------------------------------------------------

pub fn find_hull_support_vertex(hull: &HullData, direction: Vec3) -> i32 {
    let mut best_index = NULL_INDEX;
    let mut best_dot = -f32::MAX;

    for (index, point) in hull.points.iter().enumerate() {
        let d = dot(direction, *point);
        if d > best_dot {
            best_index = index as i32;
            best_dot = d;
        }
    }
    b3_assert!(best_index >= 0);

    best_index
}

pub fn find_hull_support_face(hull: &HullData, direction: Vec3) -> i32 {
    let mut best_index = NULL_INDEX;
    let mut best_dot = -f32::MAX;

    for (index, plane) in hull.planes.iter().enumerate() {
        let d = dot(plane.normal, direction);
        if d > best_dot {
            best_dot = d;
            best_index = index as i32;
        }
    }
    b3_assert!(best_index >= 0);

    best_index
}

// Full structural validation. The C version only runs when built with
// BOX3D_VALIDATE; the port validates in debug builds (matching b3_validate!).
fn is_valid_hull_impl(hull: &HullData) -> bool {
    let v = hull.vertex_count();
    let e = hull.edge_count() / 2;
    let f = hull.face_count();

    if v - e + f != 2 {
        return false;
    }

    for (index, vertex) in hull.vertices.iter().enumerate() {
        let edge = &hull.edges[vertex.edge as usize];
        if edge.origin as usize != index {
            return false;
        }
    }

    let edge_count = hull.edges.len();
    let mut index = 0;
    while index + 1 < edge_count {
        let edge = &hull.edges[index];
        let twin = &hull.edges[index + 1];

        if edge.twin as usize != index + 1 {
            return false;
        }

        if twin.twin as usize != index {
            return false;
        }

        index += 2;
    }

    for face_index in 0..hull.faces.len() {
        let face = &hull.faces[face_index];

        let base_edge_index = face.edge;

        let plane = hull.planes[face_index];
        if plane_separation(plane, hull.center) >= 0.0 {
            return false;
        }

        let mut edge_index = base_edge_index;
        loop {
            let edge = &hull.edges[edge_index as usize];
            let next = &hull.edges[edge.next as usize];
            let twin = &hull.edges[edge.twin as usize];

            if edge.face as usize != face_index {
                return false;
            }

            if twin.twin != edge_index {
                return false;
            }

            if next.origin != twin.origin {
                return false;
            }

            edge_index = edge.next;
            if edge_index == base_edge_index {
                break;
            }
        }
    }

    if hull.volume <= 0.0 {
        return false;
    }

    if hull.surface_area <= 0.0 {
        return false;
    }

    if hull.inner_radius <= 0.0 {
        return false;
    }

    true
}

pub fn is_valid_hull(hull: &HullData) -> bool {
    if cfg!(debug_assertions) {
        is_valid_hull_impl(hull)
    } else {
        true
    }
}

// ---------------------------------------------------------------------------
// Content hash / identity
// ---------------------------------------------------------------------------

// Canonical little-endian serialization of the hull content, used in place of
// the C raw-struct-bytes hash and memcmp identity. Field order: counts, aabb,
// surface_area, volume, inner_radius, center, central_inertia, then vertices,
// points, edges, faces, planes. The hash field itself is excluded.
pub(crate) fn hull_content_bytes(hull: &HullData) -> Vec<u8> {
    let mut bytes: Vec<u8> =
        Vec::with_capacity(96 + hull.points.len() * 13 + hull.edges.len() * 4 + hull.planes.len() * 17);

    fn push_f32(bytes: &mut Vec<u8>, v: f32) {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    fn push_vec3(bytes: &mut Vec<u8>, v: Vec3) {
        bytes.extend_from_slice(&v.x.to_le_bytes());
        bytes.extend_from_slice(&v.y.to_le_bytes());
        bytes.extend_from_slice(&v.z.to_le_bytes());
    }

    bytes.extend_from_slice(&hull.vertex_count().to_le_bytes());
    bytes.extend_from_slice(&hull.edge_count().to_le_bytes());
    bytes.extend_from_slice(&hull.face_count().to_le_bytes());

    push_vec3(&mut bytes, hull.aabb.lower_bound);
    push_vec3(&mut bytes, hull.aabb.upper_bound);
    push_f32(&mut bytes, hull.surface_area);
    push_f32(&mut bytes, hull.volume);
    push_f32(&mut bytes, hull.inner_radius);
    push_vec3(&mut bytes, hull.center);
    push_vec3(&mut bytes, hull.central_inertia.cx);
    push_vec3(&mut bytes, hull.central_inertia.cy);
    push_vec3(&mut bytes, hull.central_inertia.cz);

    for vertex in &hull.vertices {
        bytes.push(vertex.edge);
    }
    for point in &hull.points {
        push_vec3(&mut bytes, *point);
    }
    for edge in &hull.edges {
        bytes.push(edge.next);
        bytes.push(edge.twin);
        bytes.push(edge.origin);
        bytes.push(edge.face);
    }
    for face in &hull.faces {
        bytes.push(face.edge);
    }
    for plane in &hull.planes {
        push_vec3(&mut bytes, plane.normal);
        push_f32(&mut bytes, plane.offset);
    }

    bytes
}

fn compute_hull_hash(hull: &HullData) -> u32 {
    non_zero_hash(crate::core::hash(HASH_INIT, &hull_content_bytes(hull)))
}

/// The baked content hash spread across 64 bits (for hash maps).
pub fn hash_hull_data(hull: &HullData) -> u64 {
    (hull.hash as u64).wrapping_mul(0x9E3779B97F4A7C15)
}

/// Bit-exact structural equality (the C version memcmps the raw allocation).
pub fn compare_hull_data(hull1: &HullData, hull2: &HullData) -> bool {
    if std::ptr::eq(hull1, hull2) {
        return true;
    }

    if hull1.vertices.len() != hull2.vertices.len()
        || hull1.points.len() != hull2.points.len()
        || hull1.edges.len() != hull2.edges.len()
        || hull1.faces.len() != hull2.faces.len()
        || hull1.planes.len() != hull2.planes.len()
    {
        return false;
    }

    hull_content_bytes(hull1) == hull_content_bytes(hull2)
}

// ---------------------------------------------------------------------------
// Creation
// ---------------------------------------------------------------------------

/// Create a tessellated cylinder as a hull.
pub fn create_cylinder(height: f32, radius: f32, y_offset: f32, sides: i32) -> Arc<HullData> {
    b3_assert!(height > 0.0);
    b3_assert!(radius > 0.0);
    b3_assert!((3..=32).contains(&sides));

    let point_count = 2 * sides;
    let mut points = vec![Vec3::ZERO; point_count as usize];

    let mut alpha = 0.0f32;
    let delta_alpha = 2.0 * PI / sides as f32;

    for index in 0..sides {
        let sin_alpha = sin(alpha);
        let cos_alpha = cos(alpha);

        points[(2 * index) as usize] = vec3(radius * cos_alpha, y_offset, radius * sin_alpha);
        points[(2 * index + 1) as usize] = vec3(radius * cos_alpha, y_offset + height, radius * sin_alpha);

        alpha += delta_alpha;
    }

    let hull = create_hull(&points, point_count).expect("cylinder hull construction failed");
    b3_assert!(hull.vertex_count() == point_count);
    b3_assert!(hull.edge_count() == 6 * sides);
    b3_assert!(hull.face_count() == sides + 2);

    hull
}

/// Create a tessellated cone as a hull.
pub fn create_cone(height: f32, radius1: f32, radius2: f32, slices: i32) -> Arc<HullData> {
    b3_assert!(height > 0.0);
    b3_assert!(radius1 > 0.0);
    b3_assert!(radius2 > 0.0);
    b3_assert!((4..=32).contains(&slices));

    let point_count = 2 * slices;
    let mut points = vec![Vec3::ZERO; point_count as usize];

    let mut alpha = 0.0f32;
    let delta_alpha = 2.0 * PI / slices as f32;

    for index in 0..slices {
        let sin_alpha = sin(alpha);
        let cos_alpha = cos(alpha);

        points[(2 * index) as usize] = vec3(radius1 * cos_alpha, 0.0, radius1 * sin_alpha);
        points[(2 * index + 1) as usize] = vec3(radius2 * cos_alpha, height, radius2 * sin_alpha);

        alpha += delta_alpha;
    }

    let hull = create_hull(&points, point_count).expect("cone hull construction failed");
    b3_assert!(hull.vertex_count() == point_count);
    b3_assert!(hull.edge_count() == 6 * slices);
    b3_assert!(hull.face_count() == slices + 2);

    hull
}

/// Create a rock shaped hull.
pub fn create_rock(radius: f32) -> Arc<HullData> {
    let point_count = 10usize;

    // Golden ratio
    let phi = (1.0 + 5.0f32.sqrt()) / 2.0;

    // Fibonacci lattice
    let mut points = [Vec3::ZERO; 10];

    // Azimuthal angle
    let theta = 2.0 * PI / phi;

    let mut cs = CosSin { cosine: 1.0, sine: 0.0 };
    let delta_cs = compute_cos_sin(theta);

    for i in 0..point_count {
        // Z coordinate
        let z = 1.0 - (2.0 * i as f32 + 1.0) / point_count as f32;
        // Radius in xy-plane
        let radius_xy = (1.0 - z * z).sqrt();

        points[i].x = radius * radius_xy * cs.cosine;
        points[i].y = radius * radius_xy * cs.sine;
        points[i].z = radius * z;

        let cs0 = cs;
        cs.cosine = delta_cs.cosine * cs0.cosine - delta_cs.sine * cs0.sine;
        cs.sine = delta_cs.sine * cs0.cosine + delta_cs.cosine * cs0.sine;
    }

    create_hull(&points, point_count as i32).expect("rock hull construction failed")
}

fn update_hull_bounds(hull: &mut HullData) {
    let vertex_count = hull.vertex_count();

    b3_assert!(vertex_count > 0);
    let mut bounds = AABB { lower_bound: hull.points[0], upper_bound: hull.points[0] };

    for i in 1..vertex_count as usize {
        let p = hull.points[i];
        bounds.lower_bound = min(bounds.lower_bound, p);
        bounds.upper_bound = max(bounds.upper_bound, p);
    }

    hull.aabb = bounds;
}

// M. Kallay - "Computing the Moment of Inertia of a Solid Defined by a Triangle Mesh"
fn update_hull_bulk_properties(hull: &mut HullData) -> bool {
    let mut area = 0.0f32;
    let mut volume = 0.0f32;
    let mut center = Vec3::ZERO;

    // Use the first vertex to reduce round-off errors.
    let origin = hull.points[0];

    let mut xx = 0.0f32;
    let mut xy = 0.0f32;
    let mut yy = 0.0f32;
    let mut xz = 0.0f32;
    let mut zz = 0.0f32;
    let mut yz = 0.0f32;

    let face_count = hull.face_count();

    for face_index in 0..face_count as usize {
        let face = &hull.faces[face_index];
        let edge1 = face.edge as usize;
        let mut edge2 = hull.edges[edge1].next as usize;
        let mut edge3 = hull.edges[edge2].next as usize;

        b3_assert!(edge1 != edge3);
        b3_assert!((hull.edges[edge1].origin as i32) < hull.vertex_count());

        let v1 = sub(hull.points[hull.edges[edge1].origin as usize], origin);

        loop {
            b3_assert!((hull.edges[edge2].origin as i32) < hull.vertex_count());
            b3_assert!((hull.edges[edge3].origin as i32) < hull.vertex_count());

            let v2 = sub(hull.points[hull.edges[edge2].origin as usize], origin);
            let v3 = sub(hull.points[hull.edges[edge3].origin as usize], origin);

            area += length(cross(sub(v2, v1), sub(v3, v1)));

            let det = scalar_triple_product(v1, v2, v3);

            volume += det;

            let v4 = add(v1, add(v2, v3));
            center = add(center, mul_sv(det, v4));

            xx += det * (v1.x * v1.x + v2.x * v2.x + v3.x * v3.x + v4.x * v4.x);
            yy += det * (v1.y * v1.y + v2.y * v2.y + v3.y * v3.y + v4.y * v4.y);
            zz += det * (v1.z * v1.z + v2.z * v2.z + v3.z * v3.z + v4.z * v4.z);
            xy += det * (v1.x * v1.y + v2.x * v2.y + v3.x * v3.y + v4.x * v4.y);
            xz += det * (v1.x * v1.z + v2.x * v2.z + v3.x * v3.z + v4.x * v4.z);
            yz += det * (v1.y * v1.z + v2.y * v2.z + v3.y * v3.z + v4.y * v4.z);

            edge2 = edge3;
            edge3 = hull.edges[edge3].next as usize;

            if edge1 == edge3 {
                break;
            }
        }
    }

    b3_validate!(volume > 0.0);

    let local_center = if volume > 0.0 { mul_sv(0.25 / volume, center) } else { Vec3::ZERO };
    center = add(local_center, origin);

    let mut radius = f32::MAX;
    for face_index in 0..face_count as usize {
        let plane = hull.planes[face_index];
        let distance = plane_separation(plane, center);
        b3_validate!(distance < 0.0);

        radius = min_float(radius, -distance);
    }

    b3_validate!(0.0 < radius && radius < f32::MAX);

    let inertia = Matrix3 {
        cx: vec3(yy + zz, -xy, -xz),
        cy: vec3(-xy, xx + zz, -yz),
        cz: vec3(-xz, -yz, xx + yy),
    };

    let mass = volume / 6.0;

    let mut central_inertia = mul_sm(1.0 / 120.0, inertia);
    central_inertia = sub_mm(central_inertia, steiner(mass, local_center));

    hull.center = center;
    hull.central_inertia = central_inertia;
    hull.volume = mass;
    hull.surface_area = 0.5 * area;
    hull.inner_radius = radius;

    if mass <= 0.0 {
        return false;
    }

    if volume <= 0.0 {
        return false;
    }

    if area <= 0.0 {
        return false;
    }

    if radius <= 0.0 {
        return false;
    }

    true
}

/// Create a generic convex hull. Returns None if the input is degenerate.
pub fn create_hull(points: &[Vec3], max_vertex_count: i32) -> Option<Arc<HullData>> {
    let point_count = points.len() as i32;
    if point_count < 4 {
        return None;
    }

    let origin = points[0];
    let clamped_max_count = clamp_int(max_vertex_count, 4, HULL_LIMIT);

    let mut builder = HullBuilder::new();

    let ok = builder.construct(points, clamped_max_count, origin);
    if !ok {
        return None;
    }

    if builder.final_vertex_count >= HULL_LIMIT {
        log(&format!(
            "hull final vertex count of {} exceeds limit of {}",
            builder.final_vertex_count, HULL_LIMIT
        ));
        return None;
    }

    if builder.final_face_count >= HULL_LIMIT {
        log(&format!(
            "hull final face count of {} exceeds limit of {}",
            builder.final_face_count, HULL_LIMIT
        ));
        return None;
    }

    if builder.final_half_edge_count >= HULL_LIMIT {
        log(&format!(
            "hull final half edge count of {} exceeds limit of {}",
            builder.final_half_edge_count, HULL_LIMIT
        ));
        return None;
    }

    // Walk lists into temp arrays, stamping final_index on each node so the
    // resolution pass below is O(E + F).
    let mut temp_vertices: Vec<i32> = Vec::new();
    let mut node = builder.verts[builder.vertex_head as usize].next;
    while node != builder.vertex_head {
        b3_assert!((temp_vertices.len() as i32) <= HULL_LIMIT - 1);
        builder.verts[node as usize].final_index = temp_vertices.len() as i32;
        temp_vertices.push(node);
        node = builder.verts[node as usize].next;
    }

    // Collect edges in twin-paired order (i, i+1) by stamping each pair as we discover it.
    let mut temp_faces: Vec<i32> = Vec::new();
    let mut temp_edges: Vec<i32> = Vec::new();

    let mut face_node = builder.faces[builder.face_head as usize].link_next;
    while face_node != builder.face_head {
        b3_assert!((temp_faces.len() as i32) <= HULL_LIMIT - 1);

        let face = face_node;
        builder.faces[face as usize].final_index = temp_faces.len() as i32;
        temp_faces.push(face);

        let start = builder.faces[face as usize].edge;
        let mut edge = start;
        loop {
            if builder.edges[edge as usize].final_index < 0 {
                b3_assert!((temp_edges.len() as i32) + 1 <= HULL_LIMIT - 1);

                builder.edges[edge as usize].final_index = temp_edges.len() as i32;
                temp_edges.push(edge);
                let twin = builder.edges[edge as usize].twin;
                builder.edges[twin as usize].final_index = temp_edges.len() as i32;
                temp_edges.push(twin);
            }
            edge = builder.edges[edge as usize].next;
            if edge == start {
                break;
            }
        }

        face_node = builder.faces[face as usize].link_next;
    }

    let vertex_count = temp_vertices.len();
    let edge_count = temp_edges.len();
    let face_count = temp_faces.len();

    let mut hull = HullData {
        hash: 0,
        aabb: AABB::default(),
        surface_area: 0.0,
        volume: 0.0,
        inner_radius: 0.0,
        center: Vec3::ZERO,
        central_inertia: Matrix3::ZERO,
        vertices: vec![HullVertex { edge: 0 }; vertex_count],
        points: vec![Vec3::ZERO; vertex_count],
        edges: vec![HullHalfEdge::default(); edge_count],
        faces: vec![HullFace::default(); face_count],
        planes: vec![Plane::default(); face_count],
    };

    for index in 0..vertex_count {
        hull.vertices[index].edge = 0;
        hull.points[index] = builder.verts[temp_vertices[index] as usize].position;
    }

    for index in 0..edge_count {
        let edge_arena_index = temp_edges[index] as usize;
        let (next, twin, face, origin) = {
            let edge = &builder.edges[edge_arena_index];
            (edge.next, edge.twin, edge.face, edge.origin)
        };
        let next_final = builder.edges[next as usize].final_index;
        let twin_final = builder.edges[twin as usize].final_index;
        let face_final = builder.faces[face as usize].final_index;
        let origin_final = builder.verts[origin as usize].final_index;

        b3_assert!(0 <= next_final && next_final <= u8::MAX as i32);
        b3_assert!(0 <= twin_final && twin_final <= u8::MAX as i32);
        b3_assert!(0 <= face_final && face_final <= u8::MAX as i32);
        b3_assert!(0 <= origin_final && origin_final <= u8::MAX as i32);

        hull.edges[index].next = next_final as u8;
        hull.edges[index].twin = twin_final as u8;
        hull.edges[index].face = face_final as u8;
        hull.edges[index].origin = origin_final as u8;

        hull.vertices[origin_final as usize].edge = index as u8;
    }

    for index in 0..face_count {
        let face = &builder.faces[temp_faces[index] as usize];
        let edge_final = builder.edges[face.edge as usize].final_index;
        b3_assert!(0 <= edge_final && edge_final <= u8::MAX as i32);

        hull.faces[index].edge = edge_final as u8;
        hull.planes[index] = face.plane;
    }

    update_hull_bounds(&mut hull);
    let success = update_hull_bulk_properties(&mut hull);
    if !success {
        return None;
    }

    if !is_valid_hull(&hull) {
        return None;
    }

    hull.hash = 0;
    hull.hash = compute_hull_hash(&hull);

    Some(Arc::new(hull))
}

/// Deep clone a hull.
pub fn clone_hull(hull: &HullData) -> Option<Arc<HullData>> {
    if !is_valid_hull(hull) {
        return None;
    }

    Some(Arc::new(hull.clone()))
}

/// Clone and transform a hull. Supports non-uniform and mirroring scale.
pub fn clone_and_transform_hull(original: &HullData, transform: Transform, scale: Vec3) -> Option<Arc<HullData>> {
    if !is_valid_hull(original) {
        return None;
    }

    let mut hull = original.clone();

    let safe_scale_v = safe_scale(scale);

    let face_count = hull.face_count();
    let vertex_count = hull.vertex_count();

    if safe_scale_v.x * safe_scale_v.y * safe_scale_v.z < 0.0 {
        // Reflected: reverse edge winding for each face.
        for i in 0..face_count as usize {
            let start_edge_index = hull.faces[i].edge;
            let mut current_edge_index = start_edge_index;
            let mut prev_edge_index = u8::MAX;

            loop {
                let edge = &hull.edges[current_edge_index as usize];

                if edge.next == start_edge_index {
                    prev_edge_index = current_edge_index;
                    break;
                }

                current_edge_index = edge.next;
                if current_edge_index == start_edge_index {
                    break;
                }
            }

            b3_assert!(prev_edge_index != u8::MAX);

            current_edge_index = start_edge_index;

            loop {
                let next_index = hull.edges[current_edge_index as usize].next;
                hull.edges[current_edge_index as usize].next = prev_edge_index;

                let twin_index = hull.edges[current_edge_index as usize].twin;
                if current_edge_index < twin_index {
                    let temp = hull.edges[current_edge_index as usize].origin;
                    hull.edges[current_edge_index as usize].origin = hull.edges[twin_index as usize].origin;
                    hull.edges[twin_index as usize].origin = temp;
                }

                prev_edge_index = current_edge_index;
                current_edge_index = next_index;

                if current_edge_index == start_edge_index {
                    break;
                }
            }
        }

        for i in 0..vertex_count as usize {
            let edge_index = hull.vertices[i].edge;
            let twin = hull.edges[edge_index as usize].twin;
            hull.vertices[i].edge = twin;
        }
    }

    let matrix = make_matrix_from_quat(transform.q);

    for i in 0..vertex_count as usize {
        hull.points[i] = add(mul_mv(matrix, mul(safe_scale_v, hull.points[i])), transform.p);
    }

    for i in 0..face_count as usize {
        let mut count = 0;
        let mut centroid = Vec3::ZERO;
        let mut normal = Vec3::ZERO;

        let start_edge_index = hull.faces[i].edge;
        let mut current_edge_index = start_edge_index;

        {
            let start_edge = &hull.edges[current_edge_index as usize];
            b3_assert!(start_edge.face as usize == i);
            b3_assert!((start_edge.origin as i32) < vertex_count);
        }

        let origin = hull.points[hull.edges[current_edge_index as usize].origin as usize];

        loop {
            let edge = &hull.edges[current_edge_index as usize];
            let twin = &hull.edges[edge.twin as usize];
            b3_assert!(twin.twin == current_edge_index);

            let v1 = sub(hull.points[edge.origin as usize], origin);
            let v2 = sub(hull.points[twin.origin as usize], origin);

            count += 1;
            centroid = add(centroid, v1);
            normal.x += (v1.y - v2.y) * (v1.z + v2.z);
            normal.y += (v1.z - v2.z) * (v1.x + v2.x);
            normal.z += (v1.x - v2.x) * (v1.y + v2.y);

            current_edge_index = edge.next;
            if current_edge_index == start_edge_index {
                break;
            }
        }

        b3_assert!(count > 0);
        centroid = mul_sv(1.0 / count as f32, centroid);
        centroid = add(centroid, origin);

        let area = length(normal);
        b3_assert!(area > 0.0);
        normal = mul_sv(1.0 / area, normal);

        hull.planes[i] = make_plane_from_normal_and_point(normal, centroid);
    }

    update_hull_bounds(&mut hull);
    let success = update_hull_bulk_properties(&mut hull);
    if !success {
        return None;
    }

    hull.hash = 0;
    hull.hash = compute_hull_hash(&hull);

    b3_validate!(is_valid_hull(&hull));

    Some(Arc::new(hull))
}

// ---------------------------------------------------------------------------
// Mass, bounds, and queries
// ---------------------------------------------------------------------------

/// Compute mass properties of a hull.
pub fn compute_hull_mass(shape: &HullData, density: f32) -> MassData {
    MassData {
        mass: density * shape.volume,
        center: shape.center,
        // Inertia about the center of mass
        inertia: mul_sm(density, shape.central_inertia),
    }
}

/// Compute the bounding box of a transformed hull.
pub fn compute_hull_aabb(shape: &HullData, transform: Transform) -> AABB {
    aabb_transform(transform, shape.aabb)
}

pub fn compute_swept_hull_aabb(shape: &HullData, xf1: Transform, xf2: Transform) -> AABB {
    let aabb1 = aabb_transform(xf1, shape.aabb);
    let aabb2 = aabb_transform(xf2, shape.aabb);
    aabb_union(aabb1, aabb2)
}

/// Overlap shape versus hull.
pub fn overlap_hull(shape: &HullData, shape_transform: Transform, proxy: &ShapeProxy) -> bool {
    let input = DistanceInput {
        proxy_a: ShapeProxy { points: &shape.points, radius: 0.0 },
        proxy_b: *proxy,
        transform: inv_mul_transforms(shape_transform, Transform::IDENTITY),
        use_radii: true,
    };

    let mut cache = SimplexCache::default();
    let output = crate::distance::shape_distance(&input, &mut cache, None);
    output.distance < overlap_slop()
}

/// Ray cast versus hull shape in local space. A zero length ray is a point query.
/// Initial overlap reports a hit at the ray origin with zero fraction and zero normal.
pub fn ray_cast_hull(shape: &HullData, input: &RayCastInput) -> CastOutput {
    b3_assert!(is_valid_ray(input));
    let mut output = CastOutput::default();

    let mut lower = 0.0f32;
    let mut upper = input.max_fraction;
    let mut best_face = NULL_INDEX;

    for face_index in 0..shape.face_count() {
        let plane = shape.planes[face_index as usize];

        let distance = plane.offset - dot(plane.normal, input.origin);
        let denominator = dot(plane.normal, input.translation);

        if denominator == 0.0 {
            if distance < 0.0 {
                return output;
            }
        } else {
            let fraction = distance / denominator;

            if denominator < 0.0 {
                if fraction > lower {
                    best_face = face_index;
                    lower = fraction;
                }
            } else if fraction < upper {
                upper = fraction;
            }

            if upper < lower {
                return output;
            }
        }
    }

    if best_face >= 0 {
        output.point = add(input.origin, mul_sv(lower, input.translation));
        output.normal = shape.planes[best_face as usize].normal;
        output.fraction = lower;
        output.hit = true;
    } else {
        output.point = input.origin;
        output.hit = true;
    }

    output
}

/// Shape cast versus a hull. Initial overlap is treated as a miss.
pub fn shape_cast_hull(shape: &HullData, input: &ShapeCastInput) -> CastOutput {
    let pair_input = ShapeCastPairInput {
        proxy_a: ShapeProxy { points: &shape.points, radius: 0.0 },
        proxy_b: input.proxy,
        transform: Transform::IDENTITY,
        translation_b: input.translation,
        max_fraction: input.max_fraction,
        can_encroach: input.can_encroach,
    };

    crate::distance::shape_cast(&pair_input)
}

pub fn collide_mover_and_hull(result: &mut PlaneResult, shape: &HullData, mover: &Capsule) -> i32 {
    let mover_points = [mover.center1, mover.center2];
    let distance_input = DistanceInput {
        proxy_a: ShapeProxy { points: &shape.points, radius: 0.0 },
        proxy_b: ShapeProxy { points: &mover_points, radius: mover.radius },
        transform: Transform::IDENTITY,
        use_radii: false,
    };

    let total_radius = mover.radius;

    let mut cache = SimplexCache::default();
    let distance_output = crate::distance::shape_distance(&distance_input, &mut cache, None);

    if distance_output.distance == 0.0 {
        // No deep overlap handling on hulls (matches mesh behavior).
        return 0;
    }

    if distance_output.distance <= total_radius {
        let plane = Plane { normal: distance_output.normal, offset: total_radius - distance_output.distance };
        *result = PlaneResult { plane, point: distance_output.point_a };
        return 1;
    }

    0
}

pub fn compute_hull_extent(hull: &HullData, origin: Vec3) -> ShapeExtent {
    let mut extent = ShapeExtent { min_extent: hull.inner_radius, max_extent: Vec3::ZERO };
    for point in &hull.points {
        extent.max_extent = max(extent.max_extent, abs(sub(*point, origin)));
    }

    extent
}

pub fn compute_hull_projected_area(hull: &HullData, direction: Vec3) -> f32 {
    let mut area = 0.0f32;

    for face in &hull.faces {
        let base_edge = face.edge;
        let mut edge = &hull.edges[base_edge as usize];
        let p1 = hull.points[edge.origin as usize];

        let mut edge_index = edge.next;
        edge = &hull.edges[edge_index as usize];
        let mut p2 = hull.points[edge.origin as usize];

        edge_index = edge.next;

        loop {
            edge = &hull.edges[edge_index as usize];
            let p3 = hull.points[edge.origin as usize];

            let e1 = sub(p2, p1);
            let e2 = sub(p3, p1);
            let n = cross(e1, e2);
            let a = dot(n, direction);
            area += max_float(a, 0.0);

            p2 = p3;
            edge_index = edge.next;

            if edge_index == base_edge {
                break;
            }
        }
    }

    0.5 * area
}

// ---------------------------------------------------------------------------
// Box hulls
// ---------------------------------------------------------------------------

// Constant template box topology (vertex/edge/face). make_transformed_box_hull
// fills in the runtime-dependent fields (points, planes, aabb, mass properties, hash).
const BOX_VERTICES: [HullVertex; 8] = [
    HullVertex { edge: 8 },
    HullVertex { edge: 1 },
    HullVertex { edge: 0 },
    HullVertex { edge: 9 },
    HullVertex { edge: 13 },
    HullVertex { edge: 3 },
    HullVertex { edge: 5 },
    HullVertex { edge: 11 },
];

// { next, twin, origin, face }
const BOX_EDGES: [HullHalfEdge; 24] = [
    HullHalfEdge { next: 2, twin: 1, origin: 2, face: 0 },
    HullHalfEdge { next: 17, twin: 0, origin: 1, face: 5 },
    HullHalfEdge { next: 4, twin: 3, origin: 1, face: 0 },
    HullHalfEdge { next: 20, twin: 2, origin: 5, face: 3 },
    HullHalfEdge { next: 6, twin: 5, origin: 5, face: 0 },
    HullHalfEdge { next: 23, twin: 4, origin: 6, face: 4 },
    HullHalfEdge { next: 0, twin: 7, origin: 6, face: 0 },
    HullHalfEdge { next: 18, twin: 6, origin: 2, face: 2 },
    HullHalfEdge { next: 10, twin: 9, origin: 0, face: 1 },
    HullHalfEdge { next: 21, twin: 8, origin: 3, face: 5 },
    HullHalfEdge { next: 12, twin: 11, origin: 3, face: 1 },
    HullHalfEdge { next: 16, twin: 10, origin: 7, face: 2 },
    HullHalfEdge { next: 14, twin: 13, origin: 7, face: 1 },
    HullHalfEdge { next: 19, twin: 12, origin: 4, face: 4 },
    HullHalfEdge { next: 8, twin: 15, origin: 4, face: 1 },
    HullHalfEdge { next: 22, twin: 14, origin: 0, face: 3 },
    HullHalfEdge { next: 7, twin: 17, origin: 3, face: 2 },
    HullHalfEdge { next: 9, twin: 16, origin: 2, face: 5 },
    HullHalfEdge { next: 11, twin: 19, origin: 6, face: 2 },
    HullHalfEdge { next: 5, twin: 18, origin: 7, face: 4 },
    HullHalfEdge { next: 15, twin: 21, origin: 1, face: 3 },
    HullHalfEdge { next: 1, twin: 20, origin: 0, face: 5 },
    HullHalfEdge { next: 3, twin: 23, origin: 4, face: 3 },
    HullHalfEdge { next: 13, twin: 22, origin: 5, face: 4 },
];

const BOX_FACES: [HullFace; 6] = [
    HullFace { edge: 0 },
    HullFace { edge: 8 },
    HullFace { edge: 16 },
    HullFace { edge: 20 },
    HullFace { edge: 19 },
    HullFace { edge: 21 },
];

/// Make a transformed box as a hull.
/// hx, hy, hz are positive half widths.
pub fn make_transformed_box_hull(hx: f32, hy: f32, hz: f32, transform: Transform) -> Arc<HullData> {
    let min_h = 0.2 * linear_slop();
    let h = max(vec3(min_h, min_h, min_h), vec3(hx, hy, hz));

    let mut hull = HullData {
        hash: 0,
        aabb: aabb_transform(transform, AABB { lower_bound: neg(h), upper_bound: h }),
        surface_area: 8.0 * (h.x * h.y + h.x * h.z + h.y * h.z),
        volume: 8.0 * h.x * h.y * h.z,
        inner_radius: min_float(h.x, min_float(h.y, h.z)),
        center: transform.p,
        central_inertia: Matrix3::ZERO,
        vertices: BOX_VERTICES.to_vec(),
        points: vec![Vec3::ZERO; 8],
        edges: BOX_EDGES.to_vec(),
        faces: BOX_FACES.to_vec(),
        planes: vec![Plane::default(); 6],
    };

    let box_inertia_m = box_inertia(hull.volume, neg(h), h);
    hull.central_inertia = rotate_inertia(transform.q, box_inertia_m);

    let lower = neg(h);
    let upper = h;

    hull.planes[0] = transform_plane(transform, make_plane_from_normal_and_point(neg(Vec3::AXIS_X), lower));
    hull.planes[1] = transform_plane(transform, make_plane_from_normal_and_point(Vec3::AXIS_X, upper));
    hull.planes[2] = transform_plane(transform, make_plane_from_normal_and_point(neg(Vec3::AXIS_Y), lower));
    hull.planes[3] = transform_plane(transform, make_plane_from_normal_and_point(Vec3::AXIS_Y, upper));
    hull.planes[4] = transform_plane(transform, make_plane_from_normal_and_point(neg(Vec3::AXIS_Z), lower));
    hull.planes[5] = transform_plane(transform, make_plane_from_normal_and_point(Vec3::AXIS_Z, upper));

    hull.points[0] = transform_point(transform, vec3(h.x, h.y, h.z));
    hull.points[1] = transform_point(transform, vec3(-h.x, h.y, h.z));
    hull.points[2] = transform_point(transform, vec3(-h.x, -h.y, h.z));
    hull.points[3] = transform_point(transform, vec3(h.x, -h.y, h.z));
    hull.points[4] = transform_point(transform, vec3(h.x, h.y, -h.z));
    hull.points[5] = transform_point(transform, vec3(-h.x, h.y, -h.z));
    hull.points[6] = transform_point(transform, vec3(-h.x, -h.y, -h.z));
    hull.points[7] = transform_point(transform, vec3(h.x, -h.y, -h.z));

    hull.hash = 0;
    hull.hash = compute_hull_hash(&hull);

    Arc::new(hull)
}

/// Make a cube as a hull.
pub fn make_cube_hull(half_width: f32) -> Arc<HullData> {
    make_box_hull(half_width, half_width, half_width)
}

/// Make an offset box as a hull.
pub fn make_offset_box_hull(hx: f32, hy: f32, hz: f32, offset: Vec3) -> Arc<HullData> {
    let transform = Transform { p: offset, q: Quat::IDENTITY };
    make_transformed_box_hull(hx, hy, hz, transform)
}

/// Make a box as a hull.
pub fn make_box_hull(hx: f32, hy: f32, hz: f32) -> Arc<HullData> {
    make_transformed_box_hull(hx, hy, hz, Transform::IDENTITY)
}

/// This takes a box with a transform and post scale and converts it into a box
/// with the post scale resolved with new half-widths and transform. This accepts
/// non-uniform and negative scale. This is approximate if there is shear.
pub fn scale_box(half_widths: &mut Vec3, transform: &mut Transform, post_scale: Vec3, min_half_width: f32) {
    b3_assert!(is_valid_float(min_half_width) && min_half_width > 0.0);

    let mut q = transform.q;

    if post_scale.x < 0.0 || post_scale.y < 0.0 || post_scale.z < 0.0 {
        let mut m = make_matrix_from_quat(q);
        m.cx.x *= post_scale.x;
        m.cy.x *= post_scale.x;
        m.cz.x *= post_scale.x;
        m.cx.y *= post_scale.y;
        m.cy.y *= post_scale.y;
        m.cz.y *= post_scale.y;
        m.cx.z *= post_scale.z;
        m.cy.z *= post_scale.z;
        m.cz.z *= post_scale.z;
        m.cx = normalize(m.cx);
        m.cy = normalize(m.cy);
        m.cz = normalize(m.cz);
        m.cx = if post_scale.x < 0.0 { neg(m.cx) } else { m.cx };
        m.cy = if post_scale.y < 0.0 { neg(m.cy) } else { m.cy };
        m.cz = if post_scale.z < 0.0 { neg(m.cz) } else { m.cz };
        q = make_quat_from_matrix(&m);
    }

    let abs_scale = abs(post_scale);

    let h = *half_widths;
    let p1 = mul(abs_scale, rotate_vector(q, neg(h)));
    let p2 = mul(abs_scale, rotate_vector(q, h));

    let local_p1 = inv_rotate_vector(q, p1);
    let local_p2 = inv_rotate_vector(q, p2);

    let lower = min(local_p1, local_p2);
    let upper = max(local_p1, local_p2);

    let scaled_half_width = mul_sv(0.5, sub(upper, lower));

    let m_limit = vec3(min_half_width, min_half_width, min_half_width);
    *half_widths = max(scaled_half_width, m_limit);
    transform.p = mul(post_scale, transform.p);
    transform.q = q;
}

/// This makes a transformed box hull with post scaling.
pub fn make_scaled_box_hull(half_widths: Vec3, transform: Transform, post_scale: Vec3) -> Arc<HullData> {
    let mut h = half_widths;
    let mut xf = transform;
    scale_box(&mut h, &mut xf, post_scale, 4.0 * linear_slop());
    make_transformed_box_hull(h.x, h.y, h.z, xf)
}
