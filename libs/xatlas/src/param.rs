//! Parameterization (LSCM / ortho / piecewise) from `vendor/xatlas.cpp:6330`.

use crate::atlas::{ChartOptions, ChartType};
use crate::math::*;
use crate::mesh::*;
use crate::opennl;
use crate::segment;
use crate::util::*;

// xatlas.cpp:6333 — loop starts at v=1, skipping vertex 0. Preserve that.
fn find_approximate_diameter_vertices(mesh: &Mesh) -> Option<(u32, u32)> {
    let vertex_count = mesh.vertex_count();
    let mut min_vertex = [UINT32_MAX; 3];
    let mut max_vertex = [UINT32_MAX; 3];
    for v in 1..vertex_count {
        if mesh.is_boundary_vertex(v) {
            min_vertex = [v; 3];
            max_vertex = [v; 3];
            break;
        }
    }
    if min_vertex[0] == UINT32_MAX {
        return None;
    }
    for v in 1..vertex_count {
        if !mesh.is_boundary_vertex(v) {
            continue;
        }
        let pos = mesh.position(v);
        if pos.x < mesh.position(min_vertex[0]).x {
            min_vertex[0] = v;
        } else if pos.x > mesh.position(max_vertex[0]).x {
            max_vertex[0] = v;
        }
        if pos.y < mesh.position(min_vertex[1]).y {
            min_vertex[1] = v;
        } else if pos.y > mesh.position(max_vertex[1]).y {
            max_vertex[1] = v;
        }
        if pos.z < mesh.position(min_vertex[2]).z {
            min_vertex[2] = v;
        } else if pos.z > mesh.position(max_vertex[2]).z {
            max_vertex[2] = v;
        }
    }
    let lengths = [
        length3(mesh.position(min_vertex[0]) - mesh.position(max_vertex[0])),
        length3(mesh.position(min_vertex[1]) - mesh.position(max_vertex[1])),
        length3(mesh.position(min_vertex[2]) - mesh.position(max_vertex[2])),
    ];
    if lengths[0] > lengths[1] && lengths[0] > lengths[2] {
        Some((min_vertex[0], max_vertex[0]))
    } else if lengths[1] > lengths[2] {
        Some((min_vertex[1], max_vertex[1]))
    } else {
        Some((min_vertex[2], max_vertex[2]))
    }
}

fn project_triangle(p0: Vec3, p1: Vec3, p2: Vec3) -> (Vec2, Vec2, Vec2) {
    let x = normalize3(p1 - p0);
    let z = normalize3(cross(x, p2 - p0));
    let y = cross(z, x);
    let o = p0;
    (
        Vec2::new(0.0, 0.0),
        Vec2::new(length3(p1 - o), 0.0),
        Vec2::new(dot3(p2 - o, x), dot3(p2 - o, y)),
    )
}

fn vec_angle_cos(v1: Vec3, v2: Vec3, v3: Vec3) -> f32 {
    let d1 = v1 - v2;
    let d2 = v3 - v2;
    clamp(dot3(d1, d2) / (length3(d1) * length3(d2)), -1.0, 1.0)
}

fn vec_angle(v1: Vec3, v2: Vec3, v3: Vec3) -> f32 {
    c_acos(vec_angle_cos(v1, v2, v3))
}

fn triangle_angles(v1: Vec3, v2: Vec3, v3: Vec3) -> (f32, f32, f32) {
    let a1 = vec_angle(v3, v1, v2);
    let a2 = vec_angle(v1, v2, v3);
    let a3 = PI - a2 - a1;
    (a1, a2, a3)
}

fn setup_abf_relations(
    ctx: &mut opennl::NlContext,
    mut id0: i32,
    mut id1: i32,
    mut id2: i32,
    p0: Vec3,
    p1: Vec3,
    p2: Vec3,
) -> bool {
    let (mut a0, mut a1, mut a2) = triangle_angles(p0, p1, p2);
    if a0 == 0.0 || a1 == 0.0 || a2 == 0.0 {
        return false;
    }
    let mut s0 = c_sin(a0);
    let mut s1 = c_sin(a1);
    let mut s2 = c_sin(a2);
    if s1 > s0 && s1 > s2 {
        std::mem::swap(&mut s1, &mut s2);
        std::mem::swap(&mut s0, &mut s1);
        std::mem::swap(&mut a1, &mut a2);
        std::mem::swap(&mut a0, &mut a1);
        std::mem::swap(&mut id1, &mut id2);
        std::mem::swap(&mut id0, &mut id1);
    } else if s0 > s1 && s0 > s2 {
        std::mem::swap(&mut s0, &mut s2);
        std::mem::swap(&mut s0, &mut s1);
        std::mem::swap(&mut a0, &mut a2);
        std::mem::swap(&mut a0, &mut a1);
        std::mem::swap(&mut id0, &mut id2);
        std::mem::swap(&mut id0, &mut id1);
    }
    let c0 = c_cos(a0);
    let ratio = if s2 == 0.0 { 1.0 } else { s1 / s2 };
    let cosine = c0 * ratio;
    let sine = s0 * ratio;
    let u0 = (2 * id0) as u32;
    let v0 = (2 * id0 + 1) as u32;
    let u1 = (2 * id1) as u32;
    let v1 = (2 * id1 + 1) as u32;
    let u2 = (2 * id2) as u32;
    let v2 = (2 * id2 + 1) as u32;
    opennl::nl_begin(ctx, opennl::NL_ROW);
    opennl::nl_coefficient(ctx, u0, (cosine - 1.0) as f64);
    opennl::nl_coefficient(ctx, v0, (-sine) as f64);
    opennl::nl_coefficient(ctx, u1, (-cosine) as f64);
    opennl::nl_coefficient(ctx, v1, sine as f64);
    opennl::nl_coefficient(ctx, u2, 1.0);
    opennl::nl_end(ctx, opennl::NL_ROW);
    opennl::nl_begin(ctx, opennl::NL_ROW);
    opennl::nl_coefficient(ctx, u0, sine as f64);
    opennl::nl_coefficient(ctx, v0, (cosine - 1.0) as f64);
    opennl::nl_coefficient(ctx, u1, (-sine) as f64);
    opennl::nl_coefficient(ctx, v1, (-cosine) as f64);
    opennl::nl_coefficient(ctx, v2, 1.0);
    opennl::nl_end(ctx, opennl::NL_ROW);
    true
}

fn compute_least_squares_conformal_map(mesh: &mut Mesh) -> bool {
    let (locked0, locked1) = match find_approximate_diameter_vertices(mesh) {
        Some(p) => p,
        None => return false,
    };
    let vertex_count = mesh.vertex_count();
    let mut ctx = opennl::NlContext::new();
    opennl::nl_solver_parameteri(&mut ctx, opennl::NL_NB_VARIABLES, (2 * vertex_count) as i32);
    opennl::nl_solver_parameteri(
        &mut ctx,
        opennl::NL_MAX_ITERATIONS,
        (5 * vertex_count) as i32,
    );
    opennl::nl_begin(&mut ctx, opennl::NL_SYSTEM);
    for i in 0..vertex_count {
        let uv = mesh.texcoord(i);
        opennl::nl_set_variable(&mut ctx, 2 * i, uv.x as f64);
        opennl::nl_set_variable(&mut ctx, 2 * i + 1, uv.y as f64);
        if i == locked0 || i == locked1 {
            opennl::nl_lock_variable(&mut ctx, 2 * i);
            opennl::nl_lock_variable(&mut ctx, 2 * i + 1);
        }
    }
    opennl::nl_begin(&mut ctx, opennl::NL_MATRIX);
    let face_count = mesh.face_count();
    for f in 0..face_count {
        let v0 = mesh.vertex_at(f * 3);
        let v1 = mesh.vertex_at(f * 3 + 1);
        let v2 = mesh.vertex_at(f * 3 + 2);
        if !setup_abf_relations(
            &mut ctx,
            v0 as i32,
            v1 as i32,
            v2 as i32,
            mesh.position(v0),
            mesh.position(v1),
            mesh.position(v2),
        ) {
            let (z0, z1, z2) = project_triangle(mesh.position(v0), mesh.position(v1), mesh.position(v2));
            let a = (z1.x - z0.x) as f64;
            let b = (z1.y - z0.y) as f64;
            let c = (z2.x - z0.x) as f64;
            let d = (z2.y - z0.y) as f64;
            let u0 = 2 * v0;
            let vv0 = 2 * v0 + 1;
            let u1 = 2 * v1;
            let vv1 = 2 * v1 + 1;
            let u2 = 2 * v2;
            let vv2 = 2 * v2 + 1;
            opennl::nl_begin(&mut ctx, opennl::NL_ROW);
            opennl::nl_coefficient(&mut ctx, u0, -a + c);
            opennl::nl_coefficient(&mut ctx, vv0, b - d);
            opennl::nl_coefficient(&mut ctx, u1, -c);
            opennl::nl_coefficient(&mut ctx, vv1, d);
            opennl::nl_coefficient(&mut ctx, u2, a);
            opennl::nl_end(&mut ctx, opennl::NL_ROW);
            opennl::nl_begin(&mut ctx, opennl::NL_ROW);
            opennl::nl_coefficient(&mut ctx, u0, -b + d);
            opennl::nl_coefficient(&mut ctx, vv0, -a + c);
            opennl::nl_coefficient(&mut ctx, u1, -d);
            opennl::nl_coefficient(&mut ctx, vv1, -c);
            opennl::nl_coefficient(&mut ctx, vv2, a);
            opennl::nl_end(&mut ctx, opennl::NL_ROW);
        }
    }
    opennl::nl_end(&mut ctx, opennl::NL_MATRIX);
    opennl::nl_end(&mut ctx, opennl::NL_SYSTEM);
    if !opennl::nl_solve(&mut ctx) {
        return false;
    }
    for i in 0..vertex_count {
        let u = opennl::nl_get_variable(&ctx, 2 * i);
        let v = opennl::nl_get_variable(&ctx, 2 * i + 1);
        *mesh.texcoord_mut(i) = Vec2::new(u as f32, v as f32);
    }
    true
}

struct PiecewiseCandidate {
    face: u32,
    vertex: u32,
    prev: Option<usize>,
    next: Option<usize>,
    position: Vec2,
    cost: f32,
    max_cost: f32,
    patch_edge: u32,
    patch_vertex_orient: f32,
}

#[derive(Default)]
pub struct PiecewiseParam {
    mesh: *const Mesh,
    texcoords: Vec<Vec2>,
    face_in_any_patch: BitArray,
    candidates: Vec<usize>,
    candidate_store: Vec<PiecewiseCandidate>,
    face_to_candidate: Vec<Option<usize>>,
    patch: Vec<u32>,
    face_in_patch: BitArray,
    vertex_in_patch: BitArray,
    face_invalid: BitArray,
    boundary_grid: UniformGrid2,
    new_boundary_edges: Vec<u32>,
    ignore_boundary_edges: Vec<u32>,
}

impl PiecewiseParam {
    fn mesh(&self) -> &Mesh {
        unsafe { &*self.mesh }
    }

    pub fn reset(&mut self, mesh: &Mesh) {
        self.mesh = mesh as *const Mesh;
        let face_count = mesh.face_count();
        let vertex_count = mesh.vertex_count();
        self.texcoords.resize(vertex_count as usize, Vec2::splat(0.0));
        self.patch.clear();
        self.patch.reserve(face_count as usize);
        self.candidates.clear();
        self.candidates.reserve(face_count as usize);
        self.candidate_store.clear();
        self.face_in_any_patch.resize(face_count);
        self.face_in_any_patch.zero_out_memory();
        self.face_invalid.resize(face_count);
        self.face_in_patch.resize(face_count);
        self.vertex_in_patch.resize(vertex_count);
        self.face_to_candidate = vec![None; face_count as usize];
    }

    pub fn chart_faces(&self) -> &[u32] {
        &self.patch
    }
    pub fn texcoords(&self) -> &[Vec2] {
        &self.texcoords
    }

    pub fn compute_chart(&mut self) -> bool {
        self.patch.clear();
        self.candidates.clear();
        self.candidate_store.clear();
        for s in &mut self.face_to_candidate {
            *s = None;
        }
        self.face_invalid.zero_out_memory();
        self.face_in_patch.zero_out_memory();
        self.vertex_in_patch.zero_out_memory();
        let face_count = self.mesh().face_count();
        let mut seed = UINT32_MAX;
        for f in 0..face_count {
            if self.face_in_any_patch.get(f) {
                continue;
            }
            seed = f;
            let mut texcoords = [Vec2::splat(0.0); 3];
            self.ortho_project_face(seed, &mut texcoords);
            for i in 0..3 {
                let vertex = self.mesh().vertex_at(seed * 3 + i);
                self.vertex_in_patch.set(vertex);
                self.texcoords[vertex as usize] = texcoords[i as usize];
            }
            self.add_face_to_patch(seed);
            let tcs = self.texcoords.clone();
            let idxs = self.mesh().indices().to_vec();
            self.boundary_grid.reset(&tcs, &idxs, 0);
            let mesh = unsafe { &*self.mesh };
            let mut it = mesh.face_edge_iter(seed);
            let mut seed_edges = Vec::new();
            while !it.is_done() {
                seed_edges.push(it.edge());
                it.advance();
            }
            for e in seed_edges {
                self.boundary_grid.append(e);
            }
            break;
        }
        if seed == UINT32_MAX {
            return false;
        }
        loop {
            let mut lowest_cost = f32::MAX;
            let mut best: Option<usize> = None;
            for &ci in &self.candidates {
                let c = &self.candidate_store[ci];
                if c.max_cost < lowest_cost {
                    lowest_cost = c.max_cost;
                    best = Some(ci);
                }
            }
            let best_idx = match best {
                Some(i) => i,
                None => break,
            };
            debug_assert!(self.candidate_store[best_idx].prev.is_none());
            let mut position = Vec2::splat(0.0);
            let mut n = 0u32;
            let mut cur = Some(best_idx);
            while let Some(i) = cur {
                position += self.candidate_store[i].position;
                n += 1;
                cur = self.candidate_store[i].next;
            }
            position *= 1.0 / n as f32;
            let free_vertex = self.candidate_store[best_idx].vertex;
            self.texcoords[free_vertex as usize] = position;
            let mut invalid = false;
            cur = Some(best_idx);
            while let Some(i) = cur {
                let c = &self.candidate_store[i];
                let vertex0 = self.mesh().vertex_at(mesh_edge_index0(c.patch_edge));
                let vertex1 = self.mesh().vertex_at(mesh_edge_index1(c.patch_edge));
                let free_orient = Self::orient_to_edge(
                    self.texcoords[vertex0 as usize],
                    self.texcoords[vertex1 as usize],
                    position,
                );
                if (c.patch_vertex_orient < 0.0 && free_orient < 0.0)
                    || (c.patch_vertex_orient > 0.0 && free_orient > 0.0)
                {
                    invalid = true;
                    break;
                }
                cur = c.next;
            }
            if !invalid {
                cur = Some(best_idx);
                while let Some(i) = cur {
                    let face = self.candidate_store[i].face;
                    let a = self.texcoords[self.mesh().vertex_at(face * 3) as usize];
                    let b = self.texcoords[self.mesh().vertex_at(face * 3 + 1) as usize];
                    let c = self.texcoords[self.mesh().vertex_at(face * 3 + 2) as usize];
                    if triangle_area2(a, b, c) <= 0.0 {
                        invalid = true;
                        break;
                    }
                    cur = self.candidate_store[i].next;
                }
            }
            if !invalid {
                self.new_boundary_edges.clear();
                self.ignore_boundary_edges.clear();
                cur = Some(best_idx);
                let mesh = unsafe { &*self.mesh };
                while let Some(i) = cur {
                    let face = self.candidate_store[i].face;
                    let mut it = mesh.face_edge_iter(face);
                    let mut edges = Vec::new();
                    while !it.is_done() {
                        edges.push((it.edge(), it.opposite_face(), it.opposite_edge()));
                        it.advance();
                    }
                    for (edge, oface, oedge) in edges {
                        if oface == UINT32_MAX || !self.face_in_patch.get(oface) {
                            self.new_boundary_edges.push(edge);
                        }
                        if oface != UINT32_MAX && self.face_in_patch.get(oface) {
                            self.ignore_boundary_edges.push(oedge);
                        }
                    }
                    cur = self.candidate_store[i].next;
                }
                let eps = mesh.epsilon();
                let new_e = self.new_boundary_edges.clone();
                let ign = self.ignore_boundary_edges.clone();
                invalid = self.boundary_grid.intersect(eps, Some(&new_e), &ign);
            }
            if invalid {
                cur = Some(best_idx);
                while let Some(i) = cur {
                    self.face_invalid.set(self.candidate_store[i].face);
                    cur = self.candidate_store[i].next;
                }
                self.remove_linked_candidates(best_idx);
            } else {
                self.vertex_in_patch.set(free_vertex);
                cur = Some(best_idx);
                let mut faces = Vec::new();
                while let Some(i) = cur {
                    faces.push(self.candidate_store[i].face);
                    cur = self.candidate_store[i].next;
                }
                for f in faces {
                    self.add_face_to_patch(f);
                }
                self.remove_linked_candidates(best_idx);
                let tcs = self.texcoords.clone();
                let idxs = self.mesh().indices().to_vec();
                self.boundary_grid.reset(&tcs, &idxs, 0);
                let patch = self.patch.clone();
                for &pf in &patch {
                    let mesh = unsafe { &*self.mesh };
                    let mut it = mesh.face_edge_iter(pf);
                    let mut edges = Vec::new();
                    while !it.is_done() {
                        edges.push((it.edge(), it.opposite_face()));
                        it.advance();
                    }
                    for (edge, oface) in edges {
                        if oface == UINT32_MAX || !self.face_in_patch.get(oface) {
                            self.boundary_grid.append(edge);
                        }
                    }
                }
            }
        }
        true
    }

    fn add_face_to_patch(&mut self, face: u32) {
        self.patch.push(face);
        self.face_in_patch.set(face);
        self.face_in_any_patch.set(face);
        let mut pending: Vec<(u32, f32, u32, u32, u32)> = Vec::new();
        {
            let mut it = self.mesh().face_edge_iter(face);
            while !it.is_done() {
                let oface = it.opposite_face();
                if oface != UINT32_MAX
                    && !self.face_in_any_patch.get(oface)
                    && self.face_to_candidate[oface as usize].is_none()
                {
                    let mut free_vertex = UINT32_MAX;
                    let mut orient = 0.0f32;
                    for j in 0..3 {
                        let vertex = self.mesh().vertex_at(oface * 3 + j);
                        if vertex != it.vertex0() && vertex != it.vertex1() {
                            free_vertex = vertex;
                            orient = Self::orient_to_edge(
                                self.texcoords[it.vertex0() as usize],
                                self.texcoords[it.vertex1() as usize],
                                self.texcoords[self.mesh().vertex_at(face * 3 + j) as usize],
                            );
                            break;
                        }
                    }
                    if free_vertex != UINT32_MAX {
                        if self.vertex_in_patch.get(free_vertex) {
                            it.advance();
                            continue;
                        }
                        if !self.face_invalid.get(oface) {
                            pending.push((it.edge(), orient, oface, it.opposite_edge(), free_vertex));
                        }
                    }
                }
                it.advance();
            }
        }
        for (pe, orient, oface, oedge, fv) in pending {
            self.add_candidate_face(pe, orient, oface, oedge, fv);
        }
    }

    fn add_candidate_face(
        &mut self,
        patch_edge: u32,
        patch_vertex_orient: f32,
        face: u32,
        edge: u32,
        free_vertex: u32,
    ) {
        let mut texcoords = [Vec2::splat(0.0); 3];
        self.ortho_project_face(face, &mut texcoords);
        let vertex0 = self.mesh().vertex_at(mesh_edge_index0(patch_edge));
        let vertex1 = self.mesh().vertex_at(mesh_edge_index1(patch_edge));
        let mut local_vertex0 = UINT32_MAX;
        let mut local_vertex1 = UINT32_MAX;
        let mut local_free_vertex = UINT32_MAX;
        for i in 0..3 {
            let vertex = self.mesh().vertex_at(face * 3 + i);
            if vertex == self.mesh().vertex_at(mesh_edge_index1(edge)) {
                local_vertex0 = i;
            } else if vertex == self.mesh().vertex_at(mesh_edge_index0(edge)) {
                local_vertex1 = i;
            } else {
                local_free_vertex = i;
            }
        }
        let patch_edge_vec = self.texcoords[vertex1 as usize] - self.texcoords[vertex0 as usize];
        let local_edge_vec = texcoords[local_vertex1 as usize] - texcoords[local_vertex0 as usize];
        let len1 = length2(patch_edge_vec);
        let len2 = length2(local_edge_vec);
        if len1 <= 0.0 || len2 <= 0.0 {
            return;
        }
        let scale = len1 / len2;
        for t in &mut texcoords {
            *t = *t * scale;
        }
        let translate = self.texcoords[vertex0 as usize] - texcoords[local_vertex0 as usize];
        for t in &mut texcoords {
            *t += translate;
        }
        let angle = c_atan2(patch_edge_vec.y, patch_edge_vec.x) - c_atan2(local_edge_vec.y, local_edge_vec.x);
        for i in 0..3 {
            if i == local_vertex0 {
                continue;
            }
            let mut uv = texcoords[i as usize];
            uv -= texcoords[local_vertex0 as usize];
            let c = c_cos(angle);
            let s = c_sin(angle);
            let x = uv.x * c - uv.y * s;
            let y = uv.y * c + uv.x * s;
            texcoords[i as usize].x = x + texcoords[local_vertex0 as usize].x;
            texcoords[i as usize].y = y + texcoords[local_vertex0 as usize].y;
        }
        if is_nan_f(texcoords[local_free_vertex as usize].x)
            || is_nan_f(texcoords[local_free_vertex as usize].y)
        {
            self.face_invalid.set(face);
            return;
        }
        let free_orient = Self::orient_to_edge(
            self.texcoords[vertex0 as usize],
            self.texcoords[vertex1 as usize],
            texcoords[local_free_vertex as usize],
        );
        if (patch_vertex_orient < 0.0 && free_orient < 0.0)
            || (patch_vertex_orient > 0.0 && free_orient > 0.0)
        {
            self.face_invalid.set(face);
            return;
        }
        let stretch = self.compute_stretch(
            self.mesh().position(vertex0),
            self.mesh().position(vertex1),
            self.mesh().position(free_vertex),
            texcoords[0],
            texcoords[1],
            texcoords[2],
        );
        if stretch >= f32::MAX {
            self.face_invalid.set(face);
            return;
        }
        let cost = (stretch - 1.0).abs();
        if cost > 0.5 {
            self.face_invalid.set(face);
            return;
        }
        let idx = self.candidate_store.len();
        self.candidate_store.push(PiecewiseCandidate {
            face,
            vertex: free_vertex,
            prev: None,
            next: None,
            position: texcoords[local_free_vertex as usize],
            cost,
            max_cost: cost,
            patch_edge,
            patch_vertex_orient,
        });
        self.candidates.push(idx);
        self.face_to_candidate[face as usize] = Some(idx);
        for &other in self.candidates.iter().take(self.candidates.len() - 1) {
            if self.candidate_store[other].vertex == free_vertex {
                let mut tail = other;
                loop {
                    match self.candidate_store[tail].next {
                        Some(n) => tail = n,
                        None => break,
                    }
                }
                self.candidate_store[idx].prev = Some(tail);
                self.candidate_store[idx].next = None;
                self.candidate_store[tail].next = Some(idx);
                break;
            }
        }
        let mut head = idx;
        while let Some(p) = self.candidate_store[head].prev {
            head = p;
        }
        let mut max_cost = 0.0f32;
        let mut cur = Some(head);
        while let Some(i) = cur {
            max_cost = max_cost.max(self.candidate_store[i].cost);
            cur = self.candidate_store[i].next;
        }
        cur = Some(head);
        while let Some(i) = cur {
            self.candidate_store[i].max_cost = max_cost;
            cur = self.candidate_store[i].next;
        }
    }

    fn remove_linked_candidates(&mut self, head: usize) {
        let mut current = Some(head);
        while let Some(i) = current {
            let next = self.candidate_store[i].next;
            let face = self.candidate_store[i].face;
            self.face_to_candidate[face as usize] = None;
            if let Some(pos) = self.candidates.iter().position(|&x| x == i) {
                self.candidates.remove(pos);
            }
            current = next;
        }
    }

    fn ortho_project_face(&self, face: u32, texcoords: &mut [Vec2; 3]) {
        let normal = -self.mesh().compute_face_normal(face);
        let tangent = normalize3(
            self.mesh().position(self.mesh().vertex_at(face * 3 + 1))
                - self.mesh().position(self.mesh().vertex_at(face * 3)),
        );
        let bitangent = cross(normal, tangent);
        for i in 0..3 {
            let pos = self.mesh().position(self.mesh().vertex_at(face * 3 + i as u32));
            texcoords[i] = Vec2::new(dot3(tangent, pos), dot3(bitangent, pos));
        }
    }

    fn compute_stretch(&self, p1: Vec3, p2: Vec3, p3: Vec3, t1: Vec2, t2: Vec2, t3: Vec2) -> f32 {
        let mut parametric_area =
            ((t2.y - t1.y) * (t3.x - t1.x) - (t3.y - t1.y) * (t2.x - t1.x)) * 0.5;
        if is_zero(parametric_area, AREA_EPSILON) {
            return f32::MAX;
        }
        if parametric_area < 0.0 {
            parametric_area = parametric_area.abs();
        }
        let geometric_area = length3(cross(p2 - p1, p3 - p1)) * 0.5;
        if parametric_area <= geometric_area {
            parametric_area / geometric_area
        } else {
            geometric_area / parametric_area
        }
    }

    fn orient_to_edge(edge_vertex0: Vec2, edge_vertex1: Vec2, point: Vec2) -> f32 {
        (edge_vertex0.x - point.x) * (edge_vertex1.y - point.y)
            - (edge_vertex0.y - point.y) * (edge_vertex1.x - point.x)
    }
}

#[derive(Clone, Default)]
pub struct Quality {
    pub boundary_intersection: bool,
    pub total_triangle_count: u32,
    pub flipped_triangle_count: u32,
    pub zero_area_triangle_count: u32,
    pub total_parametric_area: f32,
    pub total_geometric_area: f32,
    pub stretch_metric: f32,
    pub max_stretch_metric: f32,
    pub conformal_metric: f32,
    pub authalic_metric: f32,
}

impl Quality {
    pub fn compute_boundary_intersection(&mut self, mesh: &Mesh, grid: &mut UniformGrid2) {
        let boundary_edges = mesh.boundary_edges().to_vec();
        grid.reset(mesh.texcoords(), mesh.indices(), boundary_edges.len() as u32);
        for &e in &boundary_edges {
            grid.append(e);
        }
        self.boundary_intersection = grid.intersect(mesh.epsilon(), None, &[]);
    }

    pub fn compute_flipped_faces(&mut self, mesh: &Mesh, flipped_faces: Option<&mut Vec<u32>>) {
        self.total_triangle_count = 0;
        self.flipped_triangle_count = 0;
        self.zero_area_triangle_count = 0;
        if let Some(ff) = flipped_faces.as_ref() {
            let _ = ff;
        }
        if let Some(ff) = flipped_faces {
            ff.clear();
            let face_count = mesh.face_count();
            for f in 0..face_count {
                let mut texcoord = [Vec2::splat(0.0); 3];
                for i in 0..3 {
                    texcoord[i] = mesh.texcoord(mesh.vertex_at(f * 3 + i as u32));
                }
                self.total_triangle_count += 1;
                let t1 = texcoord[0].x;
                let s1 = texcoord[0].y;
                let t2 = texcoord[1].x;
                let s2 = texcoord[1].y;
                let t3 = texcoord[2].x;
                let s3 = texcoord[2].y;
                let parametric_area = ((s2 - s1) * (t3 - t1) - (s3 - s1) * (t2 - t1)) * 0.5;
                if is_zero(parametric_area, AREA_EPSILON) {
                    self.zero_area_triangle_count += 1;
                    continue;
                }
                if parametric_area < 0.0 {
                    self.flipped_triangle_count += 1;
                    ff.push(f);
                }
            }
            if self.flipped_triangle_count + self.zero_area_triangle_count == self.total_triangle_count
            {
                ff.clear();
                self.flipped_triangle_count = 0;
            }
            if self.flipped_triangle_count > self.total_triangle_count / 2 {
                self.flipped_triangle_count = self.total_triangle_count - self.flipped_triangle_count;
                let temp = ff.clone();
                ff.clear();
                for f in 0..face_count {
                    if !temp.contains(&f) {
                        ff.push(f);
                    }
                }
            }
        } else {
            let face_count = mesh.face_count();
            for f in 0..face_count {
                let mut texcoord = [Vec2::splat(0.0); 3];
                for i in 0..3 {
                    texcoord[i] = mesh.texcoord(mesh.vertex_at(f * 3 + i as u32));
                }
                self.total_triangle_count += 1;
                let t1 = texcoord[0].x;
                let s1 = texcoord[0].y;
                let t2 = texcoord[1].x;
                let s2 = texcoord[1].y;
                let t3 = texcoord[2].x;
                let s3 = texcoord[2].y;
                let parametric_area = ((s2 - s1) * (t3 - t1) - (s3 - s1) * (t2 - t1)) * 0.5;
                if is_zero(parametric_area, AREA_EPSILON) {
                    self.zero_area_triangle_count += 1;
                    continue;
                }
                if parametric_area < 0.0 {
                    self.flipped_triangle_count += 1;
                }
            }
            if self.flipped_triangle_count + self.zero_area_triangle_count == self.total_triangle_count
            {
                self.flipped_triangle_count = 0;
            }
            if self.flipped_triangle_count > self.total_triangle_count / 2 {
                self.flipped_triangle_count = self.total_triangle_count - self.flipped_triangle_count;
            }
        }
    }

    pub fn compute_metrics(&mut self, mesh: &Mesh) {
        self.total_geometric_area = 0.0;
        self.total_parametric_area = 0.0;
        self.stretch_metric = 0.0;
        self.max_stretch_metric = 0.0;
        self.conformal_metric = 0.0;
        self.authalic_metric = 0.0;
        let face_count = mesh.face_count();
        for f in 0..face_count {
            let mut pos = [Vec3::splat(0.0); 3];
            let mut texcoord = [Vec2::splat(0.0); 3];
            for i in 0..3 {
                let v = mesh.vertex_at(f * 3 + i as u32);
                pos[i] = mesh.position(v);
                texcoord[i] = mesh.texcoord(v);
            }
            let t1 = texcoord[0].x;
            let s1 = texcoord[0].y;
            let t2 = texcoord[1].x;
            let s2 = texcoord[1].y;
            let t3 = texcoord[2].x;
            let s3 = texcoord[2].y;
            let mut parametric_area = ((s2 - s1) * (t3 - t1) - (s3 - s1) * (t2 - t1)) * 0.5;
            if is_zero(parametric_area, AREA_EPSILON) {
                continue;
            }
            if parametric_area < 0.0 {
                parametric_area = parametric_area.abs();
            }
            let geometric_area = length3(cross(pos[1] - pos[0], pos[2] - pos[0])) / 2.0;
            let ss = (pos[0] * (t2 - t3) + pos[1] * (t3 - t1) + pos[2] * (t1 - t2))
                / (2.0 * parametric_area);
            let st = (pos[0] * (s3 - s2) + pos[1] * (s1 - s3) + pos[2] * (s2 - s1))
                / (2.0 * parametric_area);
            let a = dot3(ss, ss);
            let b = dot3(ss, st);
            let c = dot3(st, st);
            let sigma1 = c_sqrt(
                0.5 * (a + c - c_sqrt(square(a - c) + 4.0 * square(b))).max(0.0),
            );
            let sigma2 = c_sqrt(
                0.5 * (a + c + c_sqrt(square(a - c) + 4.0 * square(b))).max(0.0),
            );
            let rms_stretch = c_sqrt((a + c) * 0.5);
            self.stretch_metric += square(rms_stretch) * geometric_area;
            self.max_stretch_metric = self.max_stretch_metric.max(sigma2);
            if !is_zero(sigma1, 0.000001) {
                self.conformal_metric += (sigma2 / sigma1) * geometric_area;
            }
            self.authalic_metric += (sigma1 * sigma2) * geometric_area;
            self.total_geometric_area += geometric_area;
            self.total_parametric_area += parametric_area;
        }
        if self.total_geometric_area > 0.0 {
            let norm_factor = c_sqrt(self.total_parametric_area / self.total_geometric_area);
            self.stretch_metric =
                c_sqrt(self.stretch_metric / self.total_geometric_area) * norm_factor;
            self.max_stretch_metric *= norm_factor;
            self.conformal_metric = c_sqrt(self.conformal_metric / self.total_geometric_area);
            self.authalic_metric = c_sqrt(self.authalic_metric / self.total_geometric_area);
        }
    }
}

#[derive(Default)]
pub struct ChartCtorBuffers {
    pub chart_mesh_indices: Vec<u32>,
}

pub struct Chart {
    basis: Basis,
    unified_mesh: Mesh,
    ty: ChartType,
    generator_type: segment::ChartGeneratorType,
    tjunction_count: u32,
    original_vertex_count: u32,
    original_indices: Vec<u32>,
    face_to_source_face_map: Vec<u32>,
    vertex_to_source_vertex_map: Vec<u32>,
    chart_vertex_to_unified_vertex_map: Vec<u32>,
    backup_texcoords: Vec<Vec2>,
    quality: Quality,
    is_invalid: bool,
}

impl Chart {
    pub fn new(
        basis: Basis,
        generator_type: segment::ChartGeneratorType,
        faces: &[u32],
        source_mesh: &Mesh,
    ) -> Self {
        let mut face_to_source = faces.to_vec();
        let approx_vertex_count = (faces.len() as u32 * 3).min(source_mesh.vertex_count());
        let mut unified_mesh = Mesh::new(
            source_mesh.epsilon(),
            approx_vertex_count,
            faces.len() as u32,
            0,
            UINT32_MAX,
        );
        let mut source_vertex_to_unified =
            HashMap::new(approx_vertex_count.max(1), hash_u32, eq_u32);
        let mut source_vertex_to_chart =
            HashMap::new(approx_vertex_count.max(1), hash_u32, eq_u32);
        let mut original_indices = vec![0u32; faces.len() * 3];
        let mut vertex_to_source = Vec::new();
        let mut chart_to_unified = Vec::new();
        let mut original_vertex_count = 0u32;
        for (f, &src_face) in faces.iter().enumerate() {
            let mut unified_indices = [0u32; 3];
            for i in 0..3 {
                let source_vertex = source_mesh.vertex_at(src_face * 3 + i as u32);
                let mut source_unified_vertex = source_mesh.first_colocal_vertex(source_vertex);
                if generator_type == segment::ChartGeneratorType::OriginalUv
                    && source_vertex != source_unified_vertex
                    && !equal2(
                        source_mesh.texcoord(source_vertex),
                        source_mesh.texcoord(source_unified_vertex),
                        source_mesh.epsilon(),
                    )
                {
                    source_unified_vertex = source_vertex;
                }
                let mut unified_vertex = source_vertex_to_unified.get(&source_unified_vertex);
                if unified_vertex == UINT32_MAX {
                    unified_vertex = source_vertex_to_unified.add(source_unified_vertex);
                    unified_mesh.add_vertex(
                        source_mesh.position(source_vertex),
                        Vec3::splat(0.0),
                        source_mesh.texcoord(source_vertex),
                    );
                }
                if source_vertex_to_chart.get(&source_vertex) == UINT32_MAX {
                    source_vertex_to_chart.add(source_vertex);
                    vertex_to_source.push(source_vertex);
                    chart_to_unified.push(unified_vertex);
                    original_vertex_count += 1;
                }
                original_indices[f * 3 + i] = source_vertex_to_chart.get(&source_vertex);
                unified_indices[i] = source_vertex_to_unified.get(&source_unified_vertex);
            }
            unified_mesh.add_face(unified_indices, false, UINT32_MAX);
        }
        unified_mesh.create_boundaries();
        let ty = if generator_type == segment::ChartGeneratorType::Planar {
            ChartType::Planar
        } else {
            ChartType::Lscm
        };
        let mut c = Self {
            basis,
            unified_mesh,
            ty,
            generator_type,
            tjunction_count: 0,
            original_vertex_count,
            original_indices,
            face_to_source_face_map: face_to_source,
            vertex_to_source_vertex_map: vertex_to_source,
            chart_vertex_to_unified_vertex_map: chart_to_unified,
            backup_texcoords: Vec::new(),
            quality: Quality::default(),
            is_invalid: false,
        };
        if generator_type == segment::ChartGeneratorType::Planar {
            return c;
        }
        c
    }

    pub fn from_piecewise(
        parent: &Chart,
        parent_mesh: &Mesh,
        faces: &[u32],
        texcoords: &[Vec2],
        source_mesh: &Mesh,
        buffers: &mut ChartCtorBuffers,
    ) -> Self {
        let face_count = faces.len() as u32;
        let mut face_to_source = vec![0u32; face_count as usize];
        for i in 0..face_count {
            face_to_source[i as usize] = parent.face_to_source_face_map[faces[i as usize] as usize];
        }
        buffers.chart_mesh_indices.clear();
        buffers
            .chart_mesh_indices
            .resize(source_mesh.vertex_count() as usize, UINT32_MAX);
        let mut unified_mesh = Mesh::new(
            source_mesh.epsilon(),
            face_count * 3,
            face_count,
            0,
            UINT32_MAX,
        );
        let mut source_vertex_to_unified = HashMap::new((face_count * 3).max(1), hash_u32, eq_u32);
        let mut original_vertex_count = 0u32;
        let mut vertex_to_source = Vec::new();
        let mut chart_to_unified = Vec::new();
        for f in 0..face_count {
            for i in 0..3 {
                let vertex = source_mesh.vertex_at(face_to_source[f as usize] * 3 + i);
                let source_unified_vertex = source_mesh.first_colocal_vertex(vertex);
                let parent_vertex = parent_mesh.vertex_at(faces[f as usize] * 3 + i);
                let mut unified_vertex = source_vertex_to_unified.get(&source_unified_vertex);
                if unified_vertex == UINT32_MAX {
                    unified_vertex = source_vertex_to_unified.add(source_unified_vertex);
                    unified_mesh.add_vertex(
                        source_mesh.position(vertex),
                        Vec3::splat(0.0),
                        texcoords[parent_vertex as usize],
                    );
                }
                if buffers.chart_mesh_indices[vertex as usize] == UINT32_MAX {
                    buffers.chart_mesh_indices[vertex as usize] = original_vertex_count;
                    original_vertex_count += 1;
                    vertex_to_source.push(vertex);
                    chart_to_unified.push(unified_vertex);
                }
            }
        }
        let mut original_indices = vec![0u32; face_count as usize * 3];
        for f in 0..face_count {
            let mut unified_indices = [0u32; 3];
            for i in 0..3 {
                let vertex = source_mesh.vertex_at(face_to_source[f as usize] * 3 + i);
                original_indices[(f * 3 + i) as usize] = buffers.chart_mesh_indices[vertex as usize];
                let unified_vertex = source_mesh.first_colocal_vertex(vertex);
                unified_indices[i as usize] = source_vertex_to_unified.get(&unified_vertex);
            }
            unified_mesh.add_face(unified_indices, false, UINT32_MAX);
        }
        unified_mesh.create_boundaries();
        let mut c = Self {
            basis: Basis::default(),
            unified_mesh,
            ty: ChartType::Piecewise,
            generator_type: segment::ChartGeneratorType::Piecewise,
            tjunction_count: 0,
            original_vertex_count,
            original_indices,
            face_to_source_face_map: face_to_source,
            vertex_to_source_vertex_map: vertex_to_source,
            chart_vertex_to_unified_vertex_map: chart_to_unified,
            backup_texcoords: Vec::new(),
            quality: Quality::default(),
            is_invalid: false,
        };
        c.backup_texcoords();
        c
    }

    pub fn is_invalid(&self) -> bool {
        self.is_invalid
    }
    pub fn ty(&self) -> ChartType {
        self.ty
    }
    pub fn generator_type(&self) -> segment::ChartGeneratorType {
        self.generator_type
    }
    pub fn tjunction_count(&self) -> u32 {
        self.tjunction_count
    }
    pub fn quality(&self) -> &Quality {
        &self.quality
    }
    pub fn map_face_to_source_face(&self, i: u32) -> u32 {
        self.face_to_source_face_map[i as usize]
    }
    pub fn map_chart_vertex_to_source_vertex(&self, i: u32) -> u32 {
        self.vertex_to_source_vertex_map[i as usize]
    }
    pub fn unified_mesh(&self) -> &Mesh {
        &self.unified_mesh
    }
    pub fn unified_mesh_mut(&mut self) -> &mut Mesh {
        &mut self.unified_mesh
    }
    pub fn original_vertex_count(&self) -> u32 {
        self.original_vertex_count
    }
    pub fn original_vertex_to_unified_vertex(&self, v: u32) -> u32 {
        self.chart_vertex_to_unified_vertex_map[v as usize]
    }
    pub fn original_vertices(&self) -> &[u32] {
        &self.original_indices
    }

    pub fn parameterize(&mut self, options: &ChartOptions, boundary_grid: &mut UniformGrid2) {
        let unified_vertex_count = self.unified_mesh.vertex_count();
        if self.generator_type != segment::ChartGeneratorType::OriginalUv {
            for i in 0..unified_vertex_count {
                let pos = self.unified_mesh.position(i);
                *self.unified_mesh.texcoord_mut(i) =
                    Vec2::new(dot3(self.basis.tangent, pos), dot3(self.basis.bitangent, pos));
            }
            if self.ty != ChartType::Planar
                && self.generator_type != segment::ChartGeneratorType::OriginalUv
            {
                self.quality
                    .compute_boundary_intersection(&self.unified_mesh, boundary_grid);
                self.quality.compute_flipped_faces(&self.unified_mesh, None);
                self.quality.compute_metrics(&self.unified_mesh);
                if !self.quality.boundary_intersection
                    && self.quality.flipped_triangle_count == 0
                    && self.quality.zero_area_triangle_count == 0
                    && self.quality.total_geometric_area > 0.0
                    && self.quality.stretch_metric <= 1.1
                    && self.quality.max_stretch_metric <= 1.25
                {
                    self.ty = ChartType::Ortho;
                }
            }
            if self.ty == ChartType::Lscm {
                compute_least_squares_conformal_map(&mut self.unified_mesh);
                self.quality
                    .compute_boundary_intersection(&self.unified_mesh, boundary_grid);
                self.quality.compute_flipped_faces(&self.unified_mesh, None);
                if self.quality.boundary_intersection
                    || self.quality.flipped_triangle_count > 0
                    || self.quality.zero_area_triangle_count > 0
                {
                    self.is_invalid = true;
                }
            }
        }
        if options.fix_winding && self.unified_mesh.compute_face_parametric_area(0) < 0.0 {
            for i in 0..unified_vertex_count {
                self.unified_mesh.texcoord_mut(i).x *= -1.0;
            }
        }
        self.backup_texcoords();
    }

    pub fn compute_parametric_bounds(&self) -> Vec2 {
        let mut min_corner = Vec2::new(f32::MAX, f32::MAX);
        let mut max_corner = Vec2::new(-f32::MAX, -f32::MAX);
        for v in 0..self.unified_mesh.vertex_count() {
            min_corner = min2(min_corner, self.unified_mesh.texcoord(v));
            max_corner = max2(max_corner, self.unified_mesh.texcoord(v));
        }
        (max_corner - min_corner) * 0.5
    }

    pub fn restore_texcoords(&mut self) {
        let n = self.unified_mesh.vertex_count() as usize;
        self.unified_mesh.texcoords_mut()[..n].copy_from_slice(&self.backup_texcoords[..n]);
    }

    fn backup_texcoords(&mut self) {
        self.backup_texcoords = self.unified_mesh.texcoords().to_vec();
    }
}

pub struct ChartGroup {
    id: u32,
    source_mesh: *const Mesh,
    face_to_source_face_map: Vec<u32>,
    charts: Vec<Chart>,
}

impl ChartGroup {
    pub fn new(id: u32, _source_mesh: &Mesh, _face_group: u32) -> Self {
        Self {
            id,
            source_mesh: _source_mesh as *const Mesh,
            face_to_source_face_map: Vec::new(),
            charts: Vec::new(),
        }
    }

    pub fn chart_count(&self) -> u32 {
        self.charts.len() as u32
    }
    pub fn chart_at(&self, i: u32) -> &Chart {
        &self.charts[i as usize]
    }
    pub fn chart_at_mut(&mut self, i: u32) -> &mut Chart {
        &mut self.charts[i as usize]
    }
    pub fn face_count(&self) -> u32 {
        self.face_to_source_face_map.len() as u32
    }

    fn source(&self) -> &Mesh {
        unsafe { &*self.source_mesh }
    }

    pub fn set_faces(&mut self, faces: Vec<u32>) {
        self.face_to_source_face_map = faces;
    }

    pub fn compute_charts(
        &mut self,
        options: &ChartOptions,
        atlas: &mut segment::Atlas,
        boundary_grid: &mut UniformGrid2,
        chart_buffers: &mut ChartCtorBuffers,
        piecewise: &mut PiecewiseParam,
    ) {
        self.charts.clear();
        let mesh = self.create_mesh();
        atlas.reset(&mesh, options);
        atlas.compute();
        let face_count = mesh.face_count();
        let chart_count = atlas.chart_count();
        let mut chart_faces = Vec::new();
        chart_faces.resize((chart_count + face_count) as usize, 0);
        let mut offset = 0usize;
        for i in 0..chart_count {
            let faces = atlas.chart_faces(i);
            chart_faces[offset] = faces.len() as u32;
            offset += 1;
            for &f in faces {
                chart_faces[offset] = self.face_to_source_face_map[f as usize];
                offset += 1;
            }
        }
        // Parameterize each chart in order (ST scheduler).
        let mut results: Vec<(Chart, Vec<Chart>)> = Vec::new();
        offset = 0;
        for i in 0..chart_count {
            let basis = atlas.chart_basis(i);
            let gen = atlas.chart_generator_type(i);
            let nfaces = chart_faces[offset];
            offset += 1;
            let faces = chart_faces[offset..offset + nfaces as usize].to_vec();
            offset += nfaces as usize;
            let mut chart = Chart::new(basis, gen, &faces, self.source());
            chart.parameterize(options, boundary_grid);
            let mut extra = Vec::new();
            if chart.is_invalid() {
                piecewise.reset(chart.unified_mesh());
                loop {
                    if !piecewise.compute_chart() {
                        break;
                    }
                    extra.push(Chart::from_piecewise(
                        &chart,
                        chart.unified_mesh(),
                        piecewise.chart_faces(),
                        piecewise.texcoords(),
                        self.source(),
                        chart_buffers,
                    ));
                }
            }
            results.push((chart, extra));
        }
        for (chart, extra) in results {
            if chart.is_invalid() {
                self.charts.extend(extra);
            } else {
                self.charts.push(chart);
            }
        }
    }

    fn create_mesh(&self) -> Mesh {
        let source = self.source();
        let face_count = self.face_to_source_face_map.len() as u32;
        let approx_vertex_count = (face_count * 3).min(source.vertex_count());
        let mut mesh = Mesh::new(
            source.epsilon(),
            approx_vertex_count,
            face_count,
            source.flags() & MESH_HAS_NORMALS,
            UINT32_MAX,
        );
        let mut source_vertex_to_vertex =
            HashMap::new(approx_vertex_count.max(1), hash_u32, eq_u32);
        for f in 0..face_count {
            let face = self.face_to_source_face_map[f as usize];
            for i in 0..3 {
                let vertex = source.vertex_at(face * 3 + i);
                if source_vertex_to_vertex.get(&vertex) == UINT32_MAX {
                    source_vertex_to_vertex.add(vertex);
                    let normal = if source.flags() & MESH_HAS_NORMALS != 0 {
                        source.normal(vertex)
                    } else {
                        Vec3::splat(0.0)
                    };
                    mesh.add_vertex(source.position(vertex), normal, source.texcoord(vertex));
                }
            }
        }
        for f in 0..face_count {
            let face = self.face_to_source_face_map[f as usize];
            let mut indices = [0u32; 3];
            for i in 0..3 {
                let vertex = source.vertex_at(face * 3 + i);
                indices[i as usize] = source_vertex_to_vertex.get(&vertex);
            }
            mesh.add_face(indices, false, UINT32_MAX);
        }
        mesh.create_colocals();
        mesh.create_boundaries();
        mesh.destroy_edge_map();
        mesh
    }
}

pub struct ParamAtlas {
    meshes: Vec<*const Mesh>,
    invalid_mesh_geometry: Vec<InvalidMeshGeometry>,
    mesh_chart_groups: Vec<Vec<ChartGroup>>,
    charts_computed: bool,
}

impl Default for ParamAtlas {
    fn default() -> Self {
        Self {
            meshes: Vec::new(),
            invalid_mesh_geometry: Vec::new(),
            mesh_chart_groups: Vec::new(),
            charts_computed: false,
        }
    }
}

impl ParamAtlas {
    pub fn add_mesh(&mut self, mesh: &Mesh) {
        self.meshes.push(mesh as *const Mesh);
    }

    pub fn mesh_count(&self) -> u32 {
        self.meshes.len() as u32
    }
    pub fn invalid_mesh_geometry(&self, i: u32) -> &InvalidMeshGeometry {
        &self.invalid_mesh_geometry[i as usize]
    }
    pub fn charts_computed(&self) -> bool {
        self.charts_computed
    }
    pub fn chart_group_count(&self, mesh: u32) -> u32 {
        self.mesh_chart_groups[mesh as usize].len() as u32
    }
    pub fn chart_group_at(&self, mesh: u32, group: u32) -> &ChartGroup {
        &self.mesh_chart_groups[mesh as usize][group as usize]
    }

    pub fn compute_charts(&mut self, options: &ChartOptions) -> bool {
        self.charts_computed = false;
        let mesh_count = self.meshes.len();
        self.mesh_chart_groups.clear();
        self.mesh_chart_groups.resize_with(mesh_count, Vec::new);
        self.invalid_mesh_geometry.clear();
        self.invalid_mesh_geometry
            .resize_with(mesh_count, InvalidMeshGeometry::default);
        let mut mesh_sort_data = vec![0.0f32; mesh_count];
        for i in 0..mesh_count {
            mesh_sort_data[i] = unsafe { &*self.meshes[i] }.index_count() as f32;
        }
        let mut radix = RadixSort::default();
        radix.sort(&mut mesh_sort_data);
        let ranks: Vec<u32> = radix.ranks().to_vec();
        let mut atlas = segment::Atlas::default();
        let mut boundary_grid = UniformGrid2::default();
        let mut chart_buffers = ChartCtorBuffers::default();
        let mut piecewise = PiecewiseParam::default();
        // Largest meshes first (ST).
        for k in 0..mesh_count {
            let i = ranks[mesh_count - 1 - k] as usize;
            let source = unsafe { &*self.meshes[i] };
            let face_groups = MeshFaceGroups::compute(source);
            let chart_group_count = face_groups.group_count();
            self.invalid_mesh_geometry[i].extract(source, Some(&face_groups));
            let mut groups = Vec::new();
            for g in 0..chart_group_count {
                let mut cg = ChartGroup::new(g, source, g);
                let mut faces = Vec::new();
                let mut f = face_groups.first_face(g);
                while f != UINT32_MAX {
                    faces.push(f);
                    f = face_groups.next_face(f);
                }
                cg.set_faces(faces);
                groups.push(cg);
            }
            // Sort chart groups by face count, largest first.
            let mut sort_data: Vec<f32> = groups.iter().map(|g| g.face_count() as f32).collect();
            let mut rs = RadixSort::default();
            rs.sort(&mut sort_data);
            let granks: Vec<u32> = rs.ranks().to_vec();
            let n = groups.len();
            for kk in 0..n {
                let gi = granks[n - 1 - kk] as usize;
                groups[gi].compute_charts(
                    options,
                    &mut atlas,
                    &mut boundary_grid,
                    &mut chart_buffers,
                    &mut piecewise,
                );
            }
            self.mesh_chart_groups[i] = groups;
        }
        self.charts_computed = true;
        true
    }
}
