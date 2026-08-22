//! Half-edge mesh, face groups, triangulator, uniform grid.
//! From `vendor/xatlas.cpp` (Mesh ~2393, UniformGrid2 ~3555).

use crate::math::*;
use crate::util::*;

pub const MESH_HAS_IGNORED_FACES: u32 = 1 << 0;
pub const MESH_HAS_NORMALS: u32 = 1 << 1;
pub const MESH_HAS_MATERIALS: u32 = 1 << 2;

pub struct Mesh {
    epsilon: f32,
    flags: u32,
    id: u32,
    face_ignore: Vec<u8>, // not Vec<bool> — C++ Array<bool> is 1 byte
    face_materials: Vec<u32>,
    indices: Vec<u32>,
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    texcoords: Vec<Vec2>,
    next_colocal_vertex: Vec<u32>,
    first_colocal_vertex: Vec<u32>,
    is_boundary_vertex: BitArray,
    boundary_edges: Vec<u32>,
    opposite_edges: Vec<u32>,
    edge_map: HashMap<EdgeKey>,
}

impl Mesh {
    pub fn new(
        epsilon: f32,
        approx_vertex_count: u32,
        approx_face_count: u32,
        flags: u32,
        id: u32,
    ) -> Self {
        let mut m = Self {
            epsilon,
            flags,
            id,
            face_ignore: Vec::new(),
            face_materials: Vec::new(),
            indices: Vec::new(),
            positions: Vec::new(),
            normals: Vec::new(),
            texcoords: Vec::new(),
            next_colocal_vertex: Vec::new(),
            first_colocal_vertex: Vec::new(),
            is_boundary_vertex: BitArray::new(),
            boundary_edges: Vec::new(),
            opposite_edges: Vec::new(),
            edge_map: HashMap::new(approx_face_count * 3, hash_edge, eq_edge),
        };
        m.indices.reserve((approx_face_count * 3) as usize);
        m.positions.reserve(approx_vertex_count as usize);
        m.texcoords.reserve(approx_vertex_count as usize);
        if m.flags & MESH_HAS_IGNORED_FACES != 0 {
            m.face_ignore.reserve(approx_face_count as usize);
        }
        if m.flags & MESH_HAS_NORMALS != 0 {
            m.normals.reserve(approx_vertex_count as usize);
        }
        if m.flags & MESH_HAS_MATERIALS != 0 {
            m.face_materials.reserve(approx_face_count as usize);
        }
        m
    }

    pub fn flags(&self) -> u32 {
        self.flags
    }
    pub fn id(&self) -> u32 {
        self.id
    }
    pub fn epsilon(&self) -> f32 {
        self.epsilon
    }

    pub fn add_vertex(&mut self, pos: Vec3, normal: Vec3, texcoord: Vec2) {
        self.positions.push(pos);
        if self.flags & MESH_HAS_NORMALS != 0 {
            self.normals.push(normal);
        }
        self.texcoords.push(texcoord);
    }

    pub fn add_face(&mut self, indices: [u32; 3], ignore: bool, material: u32) {
        if self.flags & MESH_HAS_IGNORED_FACES != 0 {
            self.face_ignore.push(if ignore { 1 } else { 0 });
        }
        if self.flags & MESH_HAS_MATERIALS != 0 {
            self.face_materials.push(material);
        }
        let first_index = self.indices.len() as u32;
        self.indices.extend_from_slice(&indices);
        for i in 0..3 {
            let vertex0 = self.indices[first_index as usize + i];
            let vertex1 = self.indices[first_index as usize + (i + 1) % 3];
            self.edge_map.add(EdgeKey::new(vertex0, vertex1));
        }
    }

    pub fn create_colocals_bvh(&mut self) {
        let vertex_count = self.positions.len() as u32;
        let aabbs: Vec<Aabb> = self
            .positions
            .iter()
            .map(|&p| Aabb::from_point_radius(p, self.epsilon))
            .collect();
        let bvh = Bvh::new(&aabbs, 4);
        let mut colocals = Vec::new();
        let mut potential = Vec::new();
        self.next_colocal_vertex = vec![UINT32_MAX; vertex_count as usize];
        self.first_colocal_vertex = vec![UINT32_MAX; vertex_count as usize];
        for i in 0..vertex_count {
            if self.next_colocal_vertex[i as usize] != UINT32_MAX {
                continue;
            }
            colocals.clear();
            colocals.push(i);
            bvh.query(Aabb::from_point_radius(self.positions[i as usize], self.epsilon), &mut potential);
            for &other_vertex in &potential {
                if other_vertex != i
                    && equal3(
                        self.positions[i as usize],
                        self.positions[other_vertex as usize],
                        self.epsilon,
                    )
                    && self.next_colocal_vertex[other_vertex as usize] == UINT32_MAX
                {
                    colocals.push(other_vertex);
                }
            }
            if colocals.len() == 1 {
                self.next_colocal_vertex[i as usize] = i;
                self.first_colocal_vertex[i as usize] = i;
                continue;
            }
            insertion_sort(&mut colocals);
            for j in 0..colocals.len() {
                self.next_colocal_vertex[colocals[j] as usize] = colocals[(j + 1) % colocals.len()];
                self.first_colocal_vertex[colocals[j] as usize] = colocals[0];
            }
        }
    }

    pub fn create_colocals_hash(&mut self) {
        let vertex_count = self.positions.len() as u32;
        let mut position_to_vertex_map = HashMap::new(vertex_count, hash_vec3, eq_vec3);
        for i in 0..vertex_count {
            position_to_vertex_map.add(self.positions[i as usize]);
        }
        let mut colocals = Vec::new();
        self.next_colocal_vertex = vec![UINT32_MAX; vertex_count as usize];
        self.first_colocal_vertex = vec![UINT32_MAX; vertex_count as usize];
        for i in 0..vertex_count {
            if self.next_colocal_vertex[i as usize] != UINT32_MAX {
                continue;
            }
            colocals.clear();
            colocals.push(i);
            let mut other_vertex = position_to_vertex_map.get(&self.positions[i as usize]);
            while other_vertex != UINT32_MAX {
                if other_vertex != i
                    && equal3(
                        self.positions[i as usize],
                        self.positions[other_vertex as usize],
                        self.epsilon,
                    )
                    && self.next_colocal_vertex[other_vertex as usize] == UINT32_MAX
                {
                    colocals.push(other_vertex);
                }
                other_vertex =
                    position_to_vertex_map.get_next(&self.positions[i as usize], other_vertex);
            }
            if colocals.len() == 1 {
                self.next_colocal_vertex[i as usize] = i;
                self.first_colocal_vertex[i as usize] = i;
                continue;
            }
            insertion_sort(&mut colocals);
            for j in 0..colocals.len() {
                self.next_colocal_vertex[colocals[j] as usize] = colocals[(j + 1) % colocals.len()];
                self.first_colocal_vertex[colocals[j] as usize] = colocals[0];
            }
        }
    }

    pub fn create_colocals(&mut self) {
        if self.epsilon <= f32::EPSILON {
            self.create_colocals_hash();
        } else {
            self.create_colocals_bvh();
        }
    }

    pub fn create_boundaries(&mut self) {
        let edge_count = self.indices.len() as u32;
        let vertex_count = self.positions.len() as u32;
        self.opposite_edges = vec![UINT32_MAX; edge_count as usize];
        self.boundary_edges.clear();
        self.boundary_edges
            .reserve((edge_count as f32 * 0.1) as usize);
        self.is_boundary_vertex.resize(vertex_count);
        self.is_boundary_vertex.zero_out_memory();
        let face_count = self.indices.len() as u32 / 3;
        for i in 0..face_count {
            if self.is_face_ignored(i) {
                continue;
            }
            for j in 0..3 {
                let edge = i * 3 + j;
                let vertex0 = self.indices[edge as usize];
                let vertex1 = self.indices[(i * 3 + (j + 1) % 3) as usize];
                let opposite_edge = self.find_edge(vertex1, vertex0);
                if opposite_edge != UINT32_MAX {
                    self.opposite_edges[edge as usize] = opposite_edge;
                } else {
                    self.boundary_edges.push(edge);
                    self.is_boundary_vertex.set(vertex0);
                    self.is_boundary_vertex.set(vertex1);
                }
            }
        }
    }

    pub fn find_edge(&self, vertex0: u32, vertex1: u32) -> u32 {
        {
            let key = EdgeKey::new(vertex0, vertex1);
            let mut edge = self.edge_map.get(&key);
            while edge != UINT32_MAX {
                if !self.is_face_ignored(mesh_edge_face(edge)) {
                    return edge;
                }
                edge = self.edge_map.get_next(&key, edge);
            }
        }
        if !self.next_colocal_vertex.is_empty() {
            let mut colocal_vertex0 = vertex0;
            loop {
                let mut colocal_vertex1 = vertex1;
                loop {
                    let key = EdgeKey::new(colocal_vertex0, colocal_vertex1);
                    let mut edge = self.edge_map.get(&key);
                    while edge != UINT32_MAX {
                        if !self.is_face_ignored(mesh_edge_face(edge)) {
                            return edge;
                        }
                        edge = self.edge_map.get_next(&key, edge);
                    }
                    colocal_vertex1 = self.next_colocal_vertex[colocal_vertex1 as usize];
                    if colocal_vertex1 == vertex1 {
                        break;
                    }
                }
                colocal_vertex0 = self.next_colocal_vertex[colocal_vertex0 as usize];
                if colocal_vertex0 == vertex0 {
                    break;
                }
            }
        }
        UINT32_MAX
    }

    pub fn destroy_edge_map(&mut self) {
        self.edge_map.destroy();
    }

    pub fn compute_surface_area(&self) -> f32 {
        let mut area = 0.0;
        for f in 0..self.face_count() {
            area += self.compute_face_area(f);
        }
        area
    }

    pub fn compute_parametric_area(&self) -> f32 {
        let mut area = 0.0;
        for f in 0..self.face_count() {
            area += self.compute_face_parametric_area(f).abs();
        }
        area
    }

    pub fn compute_face_area(&self, face: u32) -> f32 {
        let p0 = self.positions[self.indices[face as usize * 3] as usize];
        let p1 = self.positions[self.indices[face as usize * 3 + 1] as usize];
        let p2 = self.positions[self.indices[face as usize * 3 + 2] as usize];
        length3(cross(p1 - p0, p2 - p0)) * 0.5
    }

    pub fn compute_face_centroid(&self, face: u32) -> Vec3 {
        let mut sum = Vec3::splat(0.0);
        for i in 0..3 {
            sum += self.positions[self.indices[face as usize * 3 + i] as usize];
        }
        sum / 3.0
    }

    pub fn compute_face_center(&self, face: u32) -> Vec3 {
        let i0 = self.indices[face as usize * 3];
        let i1 = self.indices[face as usize * 3 + 1];
        let i2 = self.indices[face as usize * 3 + 2];
        let p0 = self.positions[i0 as usize];
        let p1 = self.positions[i1 as usize];
        let p2 = self.positions[i2 as usize];
        let l0 = length3(p1 - p0);
        let l1 = length3(p2 - p1);
        let l2 = length3(p0 - p2);
        let s = l0 + l1 + l2;
        let m0 = (p0 + p1) * l0 * (1.0 / s);
        let m1 = (p1 + p2) * l1 * (1.0 / s);
        let m2 = (p2 + p0) * l2 * (1.0 / s);
        m0 + m1 + m2
    }

    pub fn compute_face_normal(&self, face: u32) -> Vec3 {
        let p0 = self.positions[self.indices[face as usize * 3] as usize];
        let p1 = self.positions[self.indices[face as usize * 3 + 1] as usize];
        let p2 = self.positions[self.indices[face as usize * 3 + 2] as usize];
        let e0 = p2 - p0;
        let e1 = p1 - p0;
        normalize_safe3(cross(e0, e1), Vec3::new(0.0, 0.0, 1.0))
    }

    pub fn compute_face_parametric_area(&self, face: u32) -> f32 {
        let t0 = self.texcoords[self.indices[face as usize * 3] as usize];
        let t1 = self.texcoords[self.indices[face as usize * 3 + 1] as usize];
        let t2 = self.texcoords[self.indices[face as usize * 3 + 2] as usize];
        triangle_area2(t0, t1, t2)
    }

    pub fn is_seam(&self, edge: u32) -> bool {
        let opposite_edge = self.opposite_edges[edge as usize];
        if opposite_edge == UINT32_MAX {
            return false;
        }
        let e0 = mesh_edge_index0(edge);
        let e1 = mesh_edge_index1(edge);
        let oe0 = mesh_edge_index0(opposite_edge);
        let oe1 = mesh_edge_index1(opposite_edge);
        self.indices[e0 as usize] != self.indices[oe1 as usize]
            || self.indices[e1 as usize] != self.indices[oe0 as usize]
    }

    pub fn is_texture_seam(&self, edge: u32) -> bool {
        let opposite_edge = self.opposite_edges[edge as usize];
        if opposite_edge == UINT32_MAX {
            return false;
        }
        let e0 = mesh_edge_index0(edge);
        let e1 = mesh_edge_index1(edge);
        let oe0 = mesh_edge_index0(opposite_edge);
        let oe1 = mesh_edge_index1(opposite_edge);
        self.texcoords[self.indices[e0 as usize] as usize]
            != self.texcoords[self.indices[oe1 as usize] as usize]
            || self.texcoords[self.indices[e1 as usize] as usize]
                != self.texcoords[self.indices[oe0 as usize] as usize]
    }

    pub fn first_colocal_vertex(&self, vertex: u32) -> u32 {
        self.first_colocal_vertex[vertex as usize]
    }

    pub fn edge_count(&self) -> u32 {
        self.indices.len() as u32
    }
    pub fn opposite_edge(&self, edge: u32) -> u32 {
        self.opposite_edges[edge as usize]
    }
    pub fn is_boundary_edge(&self, edge: u32) -> bool {
        self.opposite_edges[edge as usize] == UINT32_MAX
    }
    pub fn boundary_edges(&self) -> &[u32] {
        &self.boundary_edges
    }
    pub fn is_boundary_vertex(&self, vertex: u32) -> bool {
        self.is_boundary_vertex.get(vertex)
    }
    pub fn vertex_count(&self) -> u32 {
        self.positions.len() as u32
    }
    pub fn vertex_at(&self, i: u32) -> u32 {
        self.indices[i as usize]
    }
    pub fn position(&self, vertex: u32) -> Vec3 {
        self.positions[vertex as usize]
    }
    pub fn positions(&self) -> &[Vec3] {
        &self.positions
    }
    pub fn normal(&self, vertex: u32) -> Vec3 {
        self.normals[vertex as usize]
    }
    pub fn texcoord(&self, vertex: u32) -> Vec2 {
        self.texcoords[vertex as usize]
    }
    pub fn texcoord_mut(&mut self, vertex: u32) -> &mut Vec2 {
        &mut self.texcoords[vertex as usize]
    }
    pub fn texcoords(&self) -> &[Vec2] {
        &self.texcoords
    }
    pub fn texcoords_mut(&mut self) -> &mut [Vec2] {
        &mut self.texcoords
    }
    pub fn face_count(&self) -> u32 {
        self.indices.len() as u32 / 3
    }
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }
    pub fn index_count(&self) -> u32 {
        self.indices.len() as u32
    }
    pub fn is_face_ignored(&self, face: u32) -> bool {
        (self.flags & MESH_HAS_IGNORED_FACES != 0) && self.face_ignore[face as usize] != 0
    }
    pub fn face_material(&self, face: u32) -> u32 {
        if self.flags & MESH_HAS_MATERIALS != 0 {
            self.face_materials[face as usize]
        } else {
            UINT32_MAX
        }
    }

    pub fn face_edge_iter(&self, face: u32) -> FaceEdgeIterator<'_> {
        FaceEdgeIterator {
            mesh: self,
            face,
            edge: face * 3,
            relative_edge: 0,
        }
    }
}

pub struct FaceEdgeIterator<'a> {
    mesh: &'a Mesh,
    face: u32,
    edge: u32,
    relative_edge: u32,
}

impl<'a> FaceEdgeIterator<'a> {
    pub fn advance(&mut self) {
        if self.relative_edge < 3 {
            self.edge += 1;
            self.relative_edge += 1;
        }
    }
    pub fn is_done(&self) -> bool {
        self.relative_edge == 3
    }
    pub fn is_boundary(&self) -> bool {
        self.mesh.opposite_edges[self.edge as usize] == UINT32_MAX
    }
    pub fn is_seam(&self) -> bool {
        self.mesh.is_seam(self.edge)
    }
    pub fn is_texture_seam(&self) -> bool {
        self.mesh.is_texture_seam(self.edge)
    }
    pub fn edge(&self) -> u32 {
        self.edge
    }
    pub fn opposite_edge(&self) -> u32 {
        self.mesh.opposite_edges[self.edge as usize]
    }
    pub fn opposite_face(&self) -> u32 {
        let oedge = self.mesh.opposite_edges[self.edge as usize];
        if oedge == UINT32_MAX {
            UINT32_MAX
        } else {
            mesh_edge_face(oedge)
        }
    }
    pub fn vertex0(&self) -> u32 {
        self.mesh.indices[self.face as usize * 3 + self.relative_edge as usize]
    }
    pub fn vertex1(&self) -> u32 {
        self.mesh.indices[self.face as usize * 3 + (self.relative_edge as usize + 1) % 3]
    }
}

pub const FACE_GROUP_INVALID: u32 = UINT32_MAX;

pub struct MeshFaceGroups {
    groups: Vec<u32>,
    first_face: Vec<u32>,
    next_face: Vec<u32>,
    face_count: Vec<u32>,
}

impl MeshFaceGroups {
    pub fn compute(mesh: &Mesh) -> Self {
        let n = mesh.face_count();
        let mut g = Self {
            groups: vec![FACE_GROUP_INVALID; n as usize],
            first_face: Vec::new(),
            next_face: vec![0; n as usize],
            face_count: Vec::new(),
        };
        let mut first_unassigned_face = 0u32;
        let mut group = 0u32;
        let mut grow_faces: Vec<u32> = Vec::new();
        loop {
            let mut face = UINT32_MAX;
            for f in first_unassigned_face..n {
                if g.groups[f as usize] == FACE_GROUP_INVALID && !mesh.is_face_ignored(f) {
                    face = f;
                    first_unassigned_face = f + 1;
                    break;
                }
            }
            if face == UINT32_MAX {
                break;
            }
            g.groups[face as usize] = group;
            g.next_face[face as usize] = UINT32_MAX;
            g.first_face.push(face);
            grow_faces.clear();
            grow_faces.push(face);
            let mut prev_face = face;
            let mut group_face_count = 1u32;
            loop {
                if grow_faces.is_empty() {
                    break;
                }
                let f = *grow_faces.last().unwrap();
                grow_faces.pop();
                let material = mesh.face_material(f);
                let mut edge_it = mesh.face_edge_iter(f);
                while !edge_it.is_done() {
                    let opposite_edge = mesh.find_edge(edge_it.vertex1(), edge_it.vertex0());
                    if opposite_edge != UINT32_MAX {
                        let opposite_face = mesh_edge_face(opposite_edge);
                        if !mesh.is_face_ignored(opposite_face)
                            && mesh.face_material(opposite_face) == material
                            && g.groups[opposite_face as usize] == FACE_GROUP_INVALID
                        {
                            g.groups[opposite_face as usize] = group;
                            g.next_face[opposite_face as usize] = UINT32_MAX;
                            if prev_face != UINT32_MAX {
                                g.next_face[prev_face as usize] = opposite_face;
                            }
                            prev_face = opposite_face;
                            group_face_count += 1;
                            grow_faces.push(opposite_face);
                        }
                    }
                    edge_it.advance();
                }
            }
            g.face_count.push(group_face_count);
            group += 1;
        }
        g
    }

    pub fn group_at(&self, face: u32) -> u32 {
        self.groups[face as usize]
    }
    pub fn group_count(&self) -> u32 {
        self.face_count.len() as u32
    }
    pub fn next_face(&self, face: u32) -> u32 {
        self.next_face[face as usize]
    }
    pub fn face_count(&self, group: u32) -> u32 {
        self.face_count[group as usize]
    }
    pub fn first_face(&self, group: u32) -> u32 {
        self.first_face[group as usize]
    }
}

#[derive(Default)]
pub struct InvalidMeshGeometry {
    faces: Vec<u32>,
    indices: Vec<u32>,
    vertex_to_source: Vec<u32>,
}

impl InvalidMeshGeometry {
    pub fn extract(&mut self, mesh: &Mesh, face_groups: Option<&MeshFaceGroups>) {
        self.faces.clear();
        let mesh_face_count = mesh.face_count();
        for f in 0..mesh_face_count {
            let invalid = match face_groups {
                Some(g) => g.group_at(f) == FACE_GROUP_INVALID,
                None => mesh.is_face_ignored(f),
            };
            if invalid {
                self.faces.push(f);
            }
        }
        let face_count = self.faces.len() as u32;
        self.indices.resize((face_count * 3) as usize, 0);
        let approx_vertex_count = (face_count * 3).min(mesh.vertex_count());
        self.vertex_to_source.clear();
        self.vertex_to_source.reserve(approx_vertex_count as usize);
        let mut source_vertex_to_vertex =
            HashMap::new(approx_vertex_count.max(1), hash_u32, eq_u32);
        for f in 0..face_count {
            let face = self.faces[f as usize];
            for i in 0..3 {
                let vertex = mesh.vertex_at(face * 3 + i);
                let mut new_vertex = source_vertex_to_vertex.get(&vertex);
                if new_vertex == UINT32_MAX {
                    new_vertex = source_vertex_to_vertex.add(vertex);
                    self.vertex_to_source.push(vertex);
                }
                self.indices[(f * 3 + i) as usize] = new_vertex;
            }
        }
    }

    pub fn faces(&self) -> &[u32] {
        &self.faces
    }
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }
    pub fn vertices(&self) -> &[u32] {
        &self.vertex_to_source
    }
}

#[derive(Default)]
pub struct Triangulator {
    polygon_vertices: Vec<i32>,
    polygon_angles: Vec<f32>,
    polygon_points: Vec<Vec2>,
}

impl Triangulator {
    pub fn triangulate_polygon(
        &mut self,
        vertices: &[Vec3],
        input_indices: &[u32],
        output_indices: &mut Vec<u32>,
    ) {
        self.polygon_vertices.clear();
        self.polygon_vertices.reserve(input_indices.len());
        output_indices.clear();
        if input_indices.len() == 3 {
            output_indices.extend_from_slice(input_indices);
            return;
        }
        let mut basis = Basis::default();
        basis.normal = normalize3(cross(
            vertices[input_indices[1] as usize] - vertices[input_indices[0] as usize],
            vertices[input_indices[2] as usize] - vertices[input_indices[1] as usize],
        ));
        basis.tangent = Basis::compute_tangent(basis.normal);
        basis.bitangent = Basis::compute_bitangent(basis.normal, basis.tangent);
        let edge_count = input_indices.len();
        self.polygon_points.clear();
        self.polygon_points.reserve(edge_count);
        self.polygon_angles.clear();
        self.polygon_angles.reserve(edge_count);
        for &idx in input_indices {
            self.polygon_vertices.push(idx as i32);
            let pos = vertices[idx as usize];
            self.polygon_points
                .push(Vec2::new(dot3(basis.tangent, pos), dot3(basis.bitangent, pos)));
        }
        self.polygon_angles.resize(edge_count, 0.0);
        while self.polygon_vertices.len() > 2 {
            let size = self.polygon_vertices.len();
            let mut min_angle = PI2;
            let mut best_ear = 0usize;
            let mut best_is_valid = false;
            for i in 0..size {
                let i0 = i;
                let i1 = (i + 1) % size;
                let i2 = (i + 2) % size;
                let p0 = self.polygon_points[i0];
                let p1 = self.polygon_points[i1];
                let p2 = self.polygon_points[i2];
                let d = clamp(
                    dot2(p0 - p1, p2 - p1) / (length2(p0 - p1) * length2(p2 - p1)),
                    -1.0,
                    1.0,
                );
                let mut angle = c_acos(d);
                let area = triangle_area2(p0, p1, p2);
                if area < 0.0 {
                    angle = PI2 - angle;
                }
                self.polygon_angles[i1] = angle;
                if angle < min_angle || !best_is_valid {
                    let mut valid = true;
                    for j in 0..size {
                        if j == i0 || j == i1 || j == i2 {
                            continue;
                        }
                        if point_in_triangle(self.polygon_points[j], p0, p1, p2) {
                            valid = false;
                            break;
                        }
                    }
                    if valid || !best_is_valid {
                        min_angle = angle;
                        best_ear = i1;
                        best_is_valid = valid;
                    }
                }
            }
            let i0 = (best_ear + size - 1) % size;
            let i1 = best_ear;
            let i2 = (best_ear + 1) % size;
            output_indices.push(self.polygon_vertices[i0] as u32);
            output_indices.push(self.polygon_vertices[i1] as u32);
            output_indices.push(self.polygon_vertices[i2] as u32);
            self.polygon_vertices.remove(i1);
            self.polygon_points.remove(i1);
            self.polygon_angles.remove(i1);
        }
    }
}

fn point_in_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    triangle_area2(a, b, p) >= AREA_EPSILON
        && triangle_area2(b, c, p) >= AREA_EPSILON
        && triangle_area2(c, a, p) >= AREA_EPSILON
}

#[derive(Default)]
pub struct UniformGrid2 {
    edges: Vec<u32>,
    cell_size: f32,
    grid_origin: Vec2,
    grid_width: u32,
    grid_height: u32,
    cell_data_offsets: Vec<u32>,
    cell_data: Vec<u32>,
    potential_edges: Vec<u32>,
    traversed_cell_offsets: Vec<u32>,
}

/// Vertex of edge `index` under an optional index buffer (empty = identity),
/// same as xatlas.cpp `UniformGrid2::vertexAt`.
#[inline]
fn grid_vertex_at(indices: &[u32], index: u32) -> u32 {
    if indices.is_empty() {
        index
    } else {
        indices[index as usize]
    }
}

#[inline]
fn grid_edge_position0(positions: &[Vec2], indices: &[u32], edge: u32) -> Vec2 {
    positions[grid_vertex_at(indices, mesh_edge_index0(edge)) as usize]
}

#[inline]
fn grid_edge_position1(positions: &[Vec2], indices: &[u32], edge: u32) -> Vec2 {
    positions[grid_vertex_at(indices, mesh_edge_index1(edge)) as usize]
}

/// The positions/indices are BORROWED per query instead of copied into the
/// grid: xatlas.cpp keeps pointers, and the clustered segmentation resets
/// this grid once per added face over the whole mesh's texcoord array — a
/// copy there made chart growth O(faces²) in memmove alone.
impl UniformGrid2 {
    pub fn reset(&mut self, reserve_edge_count: u32) {
        self.edges.clear();
        if reserve_edge_count > 0 {
            self.edges.reserve(reserve_edge_count as usize);
        }
        self.cell_data_offsets.clear();
    }

    pub fn append(&mut self, edge: u32) {
        debug_assert!(self.cell_data_offsets.is_empty());
        self.edges.push(edge);
    }

    pub fn intersect_segment(
        &mut self,
        positions: &[Vec2],
        indices: &[u32],
        v1: Vec2,
        v2: Vec2,
        epsilon: f32,
    ) -> bool {
        let edge_count = self.edges.len() as u32;
        let mut brute_force = edge_count <= 20;
        if !brute_force && self.cell_data_offsets.is_empty() {
            brute_force = !self.create_grid(positions, indices);
        }
        if brute_force {
            for j in 0..edge_count {
                let edge = self.edges[j as usize];
                if lines_intersect(
                    v1,
                    v2,
                    grid_edge_position0(positions, indices, edge),
                    grid_edge_position1(positions, indices, edge),
                    epsilon,
                ) {
                    return true;
                }
            }
        } else {
            self.compute_potential_edges(v1, v2);
            let mut prev_edge = UINT32_MAX;
            for j in 0..self.potential_edges.len() {
                let edge = self.potential_edges[j];
                if edge == prev_edge {
                    continue;
                }
                if lines_intersect(
                    v1,
                    v2,
                    grid_edge_position0(positions, indices, edge),
                    grid_edge_position1(positions, indices, edge),
                    epsilon,
                ) {
                    return true;
                }
                prev_edge = edge;
            }
        }
        false
    }

    pub fn intersect(
        &mut self,
        positions: &[Vec2],
        indices: &[u32],
        epsilon: f32,
        edges: Option<&[u32]>,
        ignore_edges: &[u32],
    ) -> bool {
        let mut brute_force = self.edges.len() <= 20;
        if !brute_force && self.cell_data_offsets.is_empty() {
            brute_force = !self.create_grid(positions, indices);
        }
        // Iterate the caller's edge list, or the grid's own edges (taken
        // out for the duration so no clone is needed while the grid mutates
        // its potential-edge scratch).
        let own_edges = std::mem::take(&mut self.edges);
        let checking_self = edges.map_or(true, |e| e.is_empty());
        let edges1: &[u32] = if checking_self { &own_edges } else { edges.unwrap() };
        let edges1_count = edges1.len();
        let mut hit = false;
        'outer: for i in 0..edges1_count {
            let edge1 = edges1[i];
            let edge1_vertex = [
                grid_vertex_at(indices, mesh_edge_index0(edge1)),
                grid_vertex_at(indices, mesh_edge_index1(edge1)),
            ];
            let edge1_position1 = positions[edge1_vertex[0] as usize];
            let edge1_position2 = positions[edge1_vertex[1] as usize];
            let edge1_extents = Extents2::from_points(edge1_position1, edge1_position2);
            let mut j = 0usize;
            let edges2: &[u32] = if brute_force {
                if checking_self {
                    j = i + 1;
                    if j == edges1_count {
                        break;
                    }
                }
                &own_edges
            } else {
                self.compute_potential_edges(edge1_position1, edge1_position2);
                &self.potential_edges
            };
            let edges2_count = edges2.len();
            let mut prev_edge = UINT32_MAX;
            while j < edges2_count {
                let edge2 = edges2[j];
                j += 1;
                if edge1 == edge2 {
                    continue;
                }
                if edge2 == prev_edge {
                    continue;
                }
                prev_edge = edge2;
                if ignore_edges.iter().any(|&e| e == edge2) {
                    continue;
                }
                let edge2_vertex = [
                    grid_vertex_at(indices, mesh_edge_index0(edge2)),
                    grid_vertex_at(indices, mesh_edge_index1(edge2)),
                ];
                if edge1_vertex[0] == edge2_vertex[0]
                    || edge1_vertex[0] == edge2_vertex[1]
                    || edge1_vertex[1] == edge2_vertex[0]
                    || edge1_vertex[1] == edge2_vertex[1]
                {
                    continue;
                }
                let edge2_position1 = positions[edge2_vertex[0] as usize];
                let edge2_position2 = positions[edge2_vertex[1] as usize];
                if !Extents2::intersect(
                    edge1_extents,
                    Extents2::from_points(edge2_position1, edge2_position2),
                ) {
                    continue;
                }
                if lines_intersect(
                    edge1_position1,
                    edge1_position2,
                    edge2_position1,
                    edge2_position2,
                    epsilon,
                ) {
                    hit = true;
                    break 'outer;
                }
            }
        }
        self.edges = own_edges;
        hit
    }

    fn create_grid(&mut self, positions: &[Vec2], indices: &[u32]) -> bool {
        let edge_count = self.edges.len() as u32;
        let mut edge_extents = Extents2::default();
        edge_extents.reset();
        for i in 0..edge_count {
            let edge = self.edges[i as usize];
            edge_extents.add(grid_edge_position0(positions, indices, edge));
            edge_extents.add(grid_edge_position1(positions, indices, edge));
        }
        self.grid_origin = edge_extents.min;
        let extents_size = edge_extents.max - edge_extents.min;
        self.cell_size =
            extents_size.x.max(extents_size.y) / (clamp(edge_count, 32, 512) as f32);
        if self.cell_size <= 0.0 {
            return false;
        }
        self.grid_width = c_ceil(extents_size.x / self.cell_size) as u32;
        self.grid_height = c_ceil(extents_size.y / self.cell_size) as u32;
        if self.grid_width <= 1 || self.grid_height <= 1 {
            return false;
        }
        self.cell_data_offsets
            .resize((self.grid_width * self.grid_height) as usize, UINT32_MAX);
        self.cell_data.clear();
        self.cell_data.reserve((edge_count * 2) as usize);
        for i in 0..edge_count {
            let edge = self.edges[i as usize];
            self.traverse(
                grid_edge_position0(positions, indices, edge),
                grid_edge_position1(positions, indices, edge),
            );
            debug_assert!(!self.traversed_cell_offsets.is_empty());
            for k in 0..self.traversed_cell_offsets.len() {
                let cell = self.traversed_cell_offsets[k];
                let mut offset = self.cell_data_offsets[cell as usize];
                if offset == UINT32_MAX {
                    self.cell_data_offsets[cell as usize] = self.cell_data.len() as u32;
                } else {
                    loop {
                        let next = self.cell_data[offset as usize + 1];
                        if next == UINT32_MAX {
                            self.cell_data[offset as usize + 1] = self.cell_data.len() as u32;
                            break;
                        }
                        offset = next;
                    }
                }
                self.cell_data.push(edge);
                self.cell_data.push(UINT32_MAX);
            }
        }
        true
    }

    fn compute_potential_edges(&mut self, p1: Vec2, p2: Vec2) {
        self.potential_edges.clear();
        self.traverse(p1, p2);
        for k in 0..self.traversed_cell_offsets.len() {
            let cell = self.traversed_cell_offsets[k];
            let mut offset = self.cell_data_offsets[cell as usize];
            while offset != UINT32_MAX {
                let edge2 = self.cell_data[offset as usize];
                self.potential_edges.push(edge2);
                offset = self.cell_data[offset as usize + 1];
            }
        }
        if self.potential_edges.is_empty() {
            return;
        }
        insertion_sort(&mut self.potential_edges);
    }

    fn traverse(&mut self, p1: Vec2, p2: Vec2) {
        let dir = p2 - p1;
        let normal = normalize_safe2(dir, Vec2::splat(0.0));
        let step_x: i32 = if dir.x >= 0.0 { 1 } else { -1 };
        let step_y: i32 = if dir.y >= 0.0 { 1 } else { -1 };
        let first_cell = [self.cell_x(p1.x), self.cell_y(p1.y)];
        let last_cell = [self.cell_x(p2.x), self.cell_y(p2.y)];
        let dist_to_next_cell_x = if step_x == 1 {
            (first_cell[0] + 1) as f32 * self.cell_size - (p1.x - self.grid_origin.x)
        } else {
            (p1.x - self.grid_origin.x) - first_cell[0] as f32 * self.cell_size
        };
        let dist_to_next_cell_y = if step_y == 1 {
            (first_cell[1] + 1) as f32 * self.cell_size - (p1.y - self.grid_origin.y)
        } else {
            (p1.y - self.grid_origin.y) - first_cell[1] as f32 * self.cell_size
        };
        let (mut t_max_x, t_delta_x) = if normal.x > EPSILON || normal.x < -EPSILON {
            (
                (dist_to_next_cell_x * step_x as f32) / normal.x,
                (self.cell_size * step_x as f32) / normal.x,
            )
        } else {
            (f32::MAX, f32::MAX)
        };
        let (mut t_max_y, t_delta_y) = if normal.y > EPSILON || normal.y < -EPSILON {
            (
                (dist_to_next_cell_y * step_y as f32) / normal.y,
                (self.cell_size * step_y as f32) / normal.y,
            )
        } else {
            (f32::MAX, f32::MAX)
        };
        self.traversed_cell_offsets.clear();
        self.traversed_cell_offsets
            .push(first_cell[0] + first_cell[1] * self.grid_width);
        let mut current_cell = first_cell;
        while !(current_cell[0] == last_cell[0] && current_cell[1] == last_cell[1]) {
            if t_max_x < t_max_y {
                t_max_x += t_delta_x;
                current_cell[0] = (current_cell[0] as i32 + step_x) as u32;
            } else {
                t_max_y += t_delta_y;
                current_cell[1] = (current_cell[1] as i32 + step_y) as u32;
            }
            if current_cell[0] >= self.grid_width || current_cell[1] >= self.grid_height {
                break;
            }
            if step_x == -1 && current_cell[0] < last_cell[0] {
                break;
            }
            if step_x == 1 && current_cell[0] > last_cell[0] {
                break;
            }
            if step_y == -1 && current_cell[1] < last_cell[1] {
                break;
            }
            if step_y == 1 && current_cell[1] > last_cell[1] {
                break;
            }
            self.traversed_cell_offsets
                .push(current_cell[0] + current_cell[1] * self.grid_width);
        }
    }

    fn cell_x(&self, x: f32) -> u32 {
        let v = ((x - self.grid_origin.x) / self.cell_size).max(0.0) as u32;
        v.min(self.grid_width - 1)
    }

    fn cell_y(&self, y: f32) -> u32 {
        let v = ((y - self.grid_origin.y) / self.cell_size).max(0.0) as u32;
        v.min(self.grid_height - 1)
    }
}
