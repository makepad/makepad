//! Chart growth / segmentation from `vendor/xatlas.cpp:4934`.

use crate::atlas::ChartOptions;
use crate::math::*;
use crate::mesh::*;
use crate::util::*;

// xatlas.cpp:4939 — smallest element at the end so pop is O(1).
pub struct CostQueue {
    max_size: u32,
    pairs: Vec<(f32, u32)>,
}

impl CostQueue {
    pub fn new(size: u32) -> Self {
        Self {
            max_size: size,
            pairs: Vec::new(),
        }
    }

    pub fn peek_cost(&self) -> f32 {
        self.pairs.last().unwrap().0
    }
    pub fn peek_face(&self) -> u32 {
        self.pairs.last().unwrap().1
    }
    pub fn push(&mut self, cost: f32, face: u32) {
        let p = (cost, face);
        if self.pairs.is_empty() || cost < self.peek_cost() {
            self.pairs.push(p);
        } else {
            let mut i = 0usize;
            let count = self.pairs.len();
            while i < count {
                if self.pairs[i].0 < cost {
                    break;
                }
                i += 1;
            }
            self.pairs.insert(i, p);
            if self.pairs.len() as u32 > self.max_size {
                self.pairs.remove(0);
            }
        }
    }
    pub fn pop(&mut self) -> u32 {
        self.pairs.pop().unwrap().1
    }
    pub fn clear(&mut self) {
        self.pairs.clear();
    }
    pub fn count(&self) -> u32 {
        self.pairs.len() as u32
    }
}

pub struct AtlasData {
    pub options: ChartOptions,
    pub mesh: *const Mesh,
    pub edge_dihedral_angles: Vec<f32>,
    pub edge_lengths: Vec<f32>,
    pub face_areas: Vec<f32>,
    pub face_uv_areas: Vec<f32>,
    pub face_normals: Vec<Vec3>,
    pub is_face_in_chart: BitArray,
}

impl Default for AtlasData {
    fn default() -> Self {
        Self {
            options: ChartOptions::default(),
            mesh: std::ptr::null(),
            edge_dihedral_angles: Vec::new(),
            edge_lengths: Vec::new(),
            face_areas: Vec::new(),
            face_uv_areas: Vec::new(),
            face_normals: Vec::new(),
            is_face_in_chart: BitArray::new(),
        }
    }
}

impl AtlasData {
    fn mesh(&self) -> &Mesh {
        unsafe { &*self.mesh }
    }

    pub fn compute(&mut self) {
        let mesh = unsafe { &*self.mesh };
        let face_count = mesh.face_count();
        let edge_count = mesh.edge_count();
        self.edge_dihedral_angles.resize(edge_count as usize, 0.0);
        self.edge_lengths.resize(edge_count as usize, 0.0);
        self.face_areas.resize(face_count as usize, 0.0);
        if self.options.use_input_mesh_uvs {
            self.face_uv_areas.resize(face_count as usize, 0.0);
        }
        self.face_normals.resize(face_count as usize, Vec3::splat(0.0));
        self.is_face_in_chart.resize(face_count);
        self.is_face_in_chart.zero_out_memory();
        for f in 0..face_count {
            for i in 0..3 {
                let edge = f * 3 + i;
                let p0 = mesh.position(mesh.vertex_at(mesh_edge_index0(edge)));
                let p1 = mesh.position(mesh.vertex_at(mesh_edge_index1(edge)));
                self.edge_lengths[edge as usize] = length3(p1 - p0);
            }
            self.face_areas[f as usize] = mesh.compute_face_area(f);
            if self.options.use_input_mesh_uvs {
                self.face_uv_areas[f as usize] = mesh.compute_face_parametric_area(f);
            }
            self.face_normals[f as usize] = mesh.compute_face_normal(f);
        }
        for face in 0..face_count {
            for i in 0..3 {
                let edge = face * 3 + i;
                let oedge = mesh.opposite_edge(edge);
                if oedge == UINT32_MAX {
                    self.edge_dihedral_angles[edge as usize] = f32::MAX;
                } else {
                    let oface = mesh_edge_face(oedge);
                    let d = dot3(self.face_normals[face as usize], self.face_normals[oface as usize]);
                    self.edge_dihedral_angles[edge as usize] = d;
                    self.edge_dihedral_angles[oedge as usize] = d;
                }
            }
        }
    }
}

struct OriginalUvCharts {
    data: *mut AtlasData,
    charts: Vec<(u32, u32)>,
    chart_basis: Vec<Basis>,
    chart_faces: Vec<u32>,
}

impl OriginalUvCharts {
    fn new(data: *mut AtlasData) -> Self {
        Self {
            data,
            charts: Vec::new(),
            chart_basis: Vec::new(),
            chart_faces: Vec::new(),
        }
    }
    fn data(&self) -> &AtlasData {
        unsafe { &*self.data }
    }
    fn data_mut(&mut self) -> &mut AtlasData {
        unsafe { &mut *self.data }
    }
    fn chart_count(&self) -> u32 {
        self.charts.len() as u32
    }
    fn chart_basis(&self, i: u32) -> Basis {
        self.chart_basis[i as usize]
    }
    fn chart_faces(&self, i: u32) -> &[u32] {
        let (first, count) = self.charts[i as usize];
        &self.chart_faces[first as usize..(first + count) as usize]
    }
    fn compute(&mut self) {
        self.charts.clear();
        self.chart_faces.clear();
        let face_count = {
            let data = self.data();
            data.mesh().face_count()
        };
        for f in 0..face_count {
            if self.data().is_face_in_chart.get(f) {
                continue;
            }
            if is_zero(self.data().face_uv_areas[f as usize], AREA_EPSILON) {
                continue;
            }
            let first_face = self.chart_faces.len() as u32;
            self.chart_faces.push(f);
            self.data_mut().is_face_in_chart.set(f);
            let mut face_count_c = 1u32;
            self.floodfill(&mut face_count_c, first_face);
            self.charts.push((first_face, face_count_c));
        }
        self.chart_basis.resize(self.charts.len(), Basis::default());
        for c in 0..self.charts.len() {
            let (first, count) = self.charts[c];
            let mut temp_points = Vec::with_capacity(count as usize * 3);
            for f in 0..count {
                let face = self.chart_faces[(first + f) as usize];
                for i in 0..3 {
                    temp_points.push(
                        self.data()
                            .mesh()
                            .position(self.data().mesh().vertex_at(face * 3 + i)),
                    );
                }
            }
            Fit::compute_basis(&temp_points, &mut self.chart_basis[c]);
        }
    }
    fn floodfill(&mut self, chart_face_count: &mut u32, first_face: u32) {
        let is_face_area_negative =
            self.data().face_uv_areas[self.chart_faces[first_face as usize] as usize] < 0.0;
        loop {
            let mut new_face_added = false;
            let face_count = *chart_face_count;
            for f in 0..face_count {
                let source_face = self.chart_faces[(first_face + f) as usize];
                let mesh = self.data().mesh();
                let mut edge_it = mesh.face_edge_iter(source_face);
                let mut to_add = Vec::new();
                while !edge_it.is_done() {
                    let face = edge_it.opposite_face();
                    if face != UINT32_MAX
                        && !self.data().is_face_in_chart.get(face)
                        && !is_zero(self.data().face_uv_areas[face as usize], AREA_EPSILON)
                        && (self.data().face_uv_areas[face as usize] < 0.0) == is_face_area_negative
                    {
                        let uv0 = mesh.texcoord(edge_it.vertex0());
                        let uv1 = mesh.texcoord(edge_it.vertex1());
                        let ouv0 = mesh.texcoord(mesh.vertex_at(mesh_edge_index0(edge_it.opposite_edge())));
                        let ouv1 = mesh.texcoord(mesh.vertex_at(mesh_edge_index1(edge_it.opposite_edge())));
                        if equal2(uv0, ouv1, mesh.epsilon()) && equal2(uv1, ouv0, mesh.epsilon()) {
                            to_add.push(face);
                        }
                    }
                    edge_it.advance();
                }
                for face in to_add {
                    if self.data().is_face_in_chart.get(face) {
                        continue;
                    }
                    self.chart_faces.push(face);
                    *chart_face_count += 1;
                    self.data_mut().is_face_in_chart.set(face);
                    new_face_added = true;
                }
            }
            if !new_face_added {
                break;
            }
        }
    }
}

struct PlanarCharts {
    data: *mut AtlasData,
    region_first_face: Vec<u32>,
    next_region_face: Vec<u32>,
    face_to_region_id: Vec<u32>,
    region_areas: Vec<f32>,
    charts: Vec<(u32, u32)>,
    chart_faces: Vec<u32>,
    chart_basis: Vec<Basis>,
}

impl PlanarCharts {
    fn new(data: *mut AtlasData) -> Self {
        Self {
            data,
            region_first_face: Vec::new(),
            next_region_face: Vec::new(),
            face_to_region_id: Vec::new(),
            region_areas: Vec::new(),
            charts: Vec::new(),
            chart_faces: Vec::new(),
            chart_basis: Vec::new(),
        }
    }
    fn data(&self) -> &AtlasData {
        unsafe { &*self.data }
    }
    fn data_mut(&mut self) -> &mut AtlasData {
        unsafe { &mut *self.data }
    }
    pub fn chart_count(&self) -> u32 {
        self.charts.len() as u32
    }
    pub fn chart_basis(&self, i: u32) -> Basis {
        self.chart_basis[i as usize]
    }
    pub fn chart_faces(&self, i: u32) -> &[u32] {
        let (first, count) = self.charts[i as usize];
        &self.chart_faces[first as usize..(first + count) as usize]
    }
    pub fn region_id_from_face(&self, face: u32) -> u32 {
        self.face_to_region_id[face as usize]
    }
    pub fn next_region_face(&self, face: u32) -> u32 {
        self.next_region_face[face as usize]
    }
    pub fn region_area(&self, region: u32) -> f32 {
        self.region_areas[region as usize]
    }

    fn compute(&mut self) {
        let face_count = self.data().mesh().face_count();
        self.region_first_face.clear();
        self.next_region_face = (0..face_count).collect();
        self.face_to_region_id = vec![UINT32_MAX; face_count as usize];
        let mut face_stack = Vec::new();
        face_stack.reserve(face_count.min(16) as usize);
        let mut region_count = 0u32;
        for f in 0..face_count {
            if self.next_region_face[f as usize] != f {
                continue;
            }
            if self.data().is_face_in_chart.get(f) {
                continue;
            }
            face_stack.clear();
            face_stack.push(f);
            loop {
                if face_stack.is_empty() {
                    break;
                }
                let face = *face_stack.last().unwrap();
                self.face_to_region_id[face as usize] = region_count;
                face_stack.pop();
                let mut to_push = Vec::new();
                let mesh = unsafe { &*(*self.data).mesh };
                let mut it = mesh.face_edge_iter(face);
                let mut neighbors = Vec::new();
                while !it.is_done() {
                    neighbors.push((
                        it.opposite_face(),
                        it.is_boundary(),
                        it.edge(),
                    ));
                    it.advance();
                }
                for (oface, is_boundary, _) in neighbors {
                    if !is_boundary
                        && self.next_region_face[oface as usize] == oface
                        && !unsafe { &*self.data }.is_face_in_chart.get(oface)
                        && equal_f(
                            dot3(
                                unsafe { &*self.data }.face_normals[face as usize],
                                unsafe { &*self.data }.face_normals[oface as usize],
                            ),
                            1.0,
                            EPSILON,
                        )
                    {
                        let next = self.next_region_face[face as usize];
                        self.next_region_face[face as usize] = oface;
                        self.next_region_face[oface as usize] = next;
                        self.face_to_region_id[oface as usize] = region_count;
                        to_push.push(oface);
                    }
                }
                face_stack.extend(to_push);
            }
            self.region_first_face.push(f);
            region_count += 1;
        }
        self.region_areas = vec![0.0; region_count as usize];
        for f in 0..face_count {
            if self.face_to_region_id[f as usize] == UINT32_MAX {
                continue;
            }
            self.region_areas[self.face_to_region_id[f as usize] as usize] +=
                self.data().face_areas[f as usize];
        }
        self.charts.clear();
        self.chart_faces.clear();
        for region in 0..region_count {
            let first_region_face = self.region_first_face[region as usize];
            let mut face = first_region_face;
            let mut create_chart = true;
            loop {
                let mut it = self.data().mesh().face_edge_iter(face);
                while !it.is_done() {
                    if !it.is_boundary() {
                        let oface = it.opposite_face();
                        if self.face_to_region_id[oface as usize] != region {
                            let angle = self.data().edge_dihedral_angles[it.edge() as usize];
                            if angle > 0.0 && angle < f32::MAX {
                                create_chart = false;
                                break;
                            }
                        }
                    }
                    it.advance();
                }
                if !create_chart {
                    break;
                }
                face = self.next_region_face[face as usize];
                if face == first_region_face {
                    break;
                }
            }
            if create_chart {
                let first_face = self.chart_faces.len() as u32;
                let mut count = 0u32;
                face = first_region_face;
                loop {
                    self.data_mut().is_face_in_chart.set(face);
                    self.chart_faces.push(face);
                    count += 1;
                    face = self.next_region_face[face as usize];
                    if face == first_region_face {
                        break;
                    }
                }
                self.charts.push((first_face, count));
            }
        }
        self.chart_basis.resize(self.charts.len(), Basis::default());
        for c in 0..self.charts.len() {
            let face = self.chart_faces[self.charts[c].0 as usize];
            let mut basis = Basis::default();
            basis.normal = self.data().face_normals[face as usize];
            basis.tangent = Basis::compute_tangent(basis.normal);
            basis.bitangent = Basis::compute_bitangent(basis.normal, basis.tangent);
            self.chart_basis[c] = basis;
        }
    }
}

struct ClusteredChart {
    id: i32,
    basis: Basis,
    area: f32,
    boundary_length: f32,
    centroid_sum: Vec3,
    centroid: Vec3,
    faces: Vec<u32>,
    failed_planar_regions: Vec<u32>,
    candidates: CostQueue,
    seed: u32,
}

impl ClusteredChart {
    fn new() -> Self {
        Self {
            id: -1,
            basis: Basis::default(),
            area: 0.0,
            boundary_length: 0.0,
            centroid_sum: Vec3::splat(0.0),
            centroid: Vec3::splat(0.0),
            faces: Vec::new(),
            failed_planar_regions: Vec::new(),
            candidates: CostQueue::new(UINT32_MAX),
            seed: 0,
        }
    }
}

struct ClusteredCharts {
    data: *mut AtlasData,
    planar: *const PlanarCharts,
    texcoords: Vec<Vec2>,
    faces_left: u32,
    face_charts: Vec<i32>,
    charts: Vec<Option<Box<ClusteredChart>>>,
    best_triangles: CostQueue,
    temp_points: Vec<Vec3>,
    boundary_grid: UniformGrid2,
    shared_boundary_lengths: Vec<f32>,
    shared_boundary_lengths_no_seams: Vec<f32>,
    shared_boundary_edge_count_no_seams: Vec<u32>,
    placing_seeds: bool,
}

impl ClusteredCharts {
    fn new(data: *mut AtlasData, planar: *const PlanarCharts) -> Self {
        Self {
            data,
            planar,
            texcoords: Vec::new(),
            faces_left: 0,
            face_charts: Vec::new(),
            charts: Vec::new(),
            best_triangles: CostQueue::new(10),
            temp_points: Vec::new(),
            boundary_grid: UniformGrid2::default(),
            shared_boundary_lengths: Vec::new(),
            shared_boundary_lengths_no_seams: Vec::new(),
            shared_boundary_edge_count_no_seams: Vec::new(),
            placing_seeds: false,
        }
    }
    fn data(&self) -> &AtlasData {
        unsafe { &*self.data }
    }
    fn data_mut(&mut self) -> &mut AtlasData {
        unsafe { &mut *self.data }
    }
    fn planar(&self) -> &PlanarCharts {
        unsafe { &*self.planar }
    }
    fn chart_count(&self) -> u32 {
        self.charts.len() as u32
    }
    fn chart_faces(&self, i: u32) -> &[u32] {
        self.charts[i as usize].as_ref().unwrap().faces.as_slice()
    }
    fn chart_basis(&self, i: u32) -> Basis {
        self.charts[i as usize].as_ref().unwrap().basis
    }

    fn compute(&mut self) {
        let face_count = self.data().mesh().face_count();
        self.faces_left = 0;
        for i in 0..face_count {
            if !self.data().is_face_in_chart.get(i) {
                self.faces_left += 1;
            }
        }
        self.charts.clear();
        self.face_charts = vec![-1; face_count as usize];
        self.texcoords = vec![Vec2::splat(0.0); (face_count * 3) as usize];
        if self.faces_left == 0 {
            return;
        }
        let max_cost = self.data().options.max_cost;
        let max_iterations = self.data().options.max_iterations;
        self.place_seeds(max_cost * 0.5);
        if max_iterations == 0 {
            return;
        }
        self.relocate_seeds();
        self.reset_charts();
        let mut iteration = 0u32;
        loop {
            self.grow_charts(max_cost);
            self.fill_holes(max_cost * 0.5);
            self.merge_charts();
            iteration += 1;
            if iteration == max_iterations {
                break;
            }
            if !self.relocate_seeds() {
                break;
            }
            self.reset_charts();
        }
    }

    fn place_seeds(&mut self, threshold: f32) {
        self.placing_seeds = true;
        while self.faces_left > 0 {
            self.create_chart(threshold);
        }
        self.placing_seeds = false;
    }

    fn grow_charts(&mut self, threshold: f32) {
        loop {
            if self.faces_left == 0 {
                break;
            }
            let mut best_face = UINT32_MAX;
            let mut best_chart = UINT32_MAX;
            let mut lowest_cost = f32::MAX;
            for i in 0..self.charts.len() {
                if self.charts[i].is_none() {
                    continue;
                }
                let mut face = UINT32_MAX;
                let mut cost = f32::MAX;
                loop {
                    if self.charts[i].as_ref().unwrap().candidates.count() == 0 {
                        break;
                    }
                    cost = self.charts[i].as_ref().unwrap().candidates.peek_cost();
                    face = self.charts[i].as_ref().unwrap().candidates.peek_face();
                    if !self.data().is_face_in_chart.get(face) {
                        break;
                    } else {
                        self.charts[i].as_mut().unwrap().candidates.pop();
                        face = UINT32_MAX;
                    }
                }
                if face == UINT32_MAX {
                    continue;
                }
                if cost < lowest_cost {
                    lowest_cost = cost;
                    best_face = face;
                    best_chart = i as u32;
                }
            }
            if best_face == UINT32_MAX || lowest_cost > threshold {
                break;
            }
            self.charts[best_chart as usize]
                .as_mut()
                .unwrap()
                .candidates
                .pop();
            if !self.add_face_to_chart(best_chart, best_face) {
                let region = self.planar().region_id_from_face(best_face);
                self.charts[best_chart as usize]
                    .as_mut()
                    .unwrap()
                    .failed_planar_regions
                    .push(region);
            }
        }
    }

    fn reset_charts(&mut self) {
        let face_count = self.data().mesh().face_count();
        for i in 0..face_count {
            if self.face_charts[i as usize] != -1 {
                self.data_mut().is_face_in_chart.unset(i);
            }
            self.face_charts[i as usize] = -1;
        }
        self.faces_left = 0;
        for i in 0..face_count {
            if !self.data().is_face_in_chart.get(i) {
                self.faces_left += 1;
            }
        }
        for i in 0..self.charts.len() {
            if self.charts[i].is_none() {
                continue;
            }
            {
                let chart = self.charts[i].as_mut().unwrap();
                chart.area = 0.0;
                chart.boundary_length = 0.0;
                chart.basis = Basis::default();
                chart.centroid_sum = Vec3::splat(0.0);
                chart.centroid = Vec3::splat(0.0);
                chart.faces.clear();
                chart.candidates.clear();
                chart.failed_planar_regions.clear();
            }
            let seed = self.charts[i].as_ref().unwrap().seed;
            self.add_face_to_chart(i as u32, seed);
        }
    }

    fn relocate_seeds(&mut self) -> bool {
        let mut any = false;
        for i in 0..self.charts.len() {
            if self.charts[i].is_some() && self.relocate_seed(i as u32) {
                any = true;
            }
        }
        any
    }

    fn fill_holes(&mut self, threshold: f32) {
        while self.faces_left > 0 {
            self.create_chart(threshold);
        }
    }

    fn merge_charts(&mut self) {
        let chart_count = self.charts.len();
        loop {
            let mut merged = false;
            for c in (0..chart_count as i32).rev() {
                if self.charts[c as usize].is_none() {
                    continue;
                }
                let mut external_boundary_length = 0.0f32;
                self.shared_boundary_lengths = vec![0.0; chart_count];
                self.shared_boundary_lengths_no_seams = vec![0.0; chart_count];
                self.shared_boundary_edge_count_no_seams = vec![0; chart_count];
                let face_count = self.charts[c as usize].as_ref().unwrap().faces.len();
                for i in 0..face_count {
                    let f = self.charts[c as usize].as_ref().unwrap().faces[i];
                    let mesh = unsafe { &*(*self.data).mesh };
                    let mut it = mesh.face_edge_iter(f);
                    let mut edges = Vec::new();
                    while !it.is_done() {
                        edges.push((
                            it.edge(),
                            it.is_boundary(),
                            it.opposite_face(),
                            it.is_seam(),
                            it.is_texture_seam(),
                        ));
                        it.advance();
                    }
                    for (edge, is_boundary, opposite_face, is_seam, is_tex_seam) in edges {
                        let l = unsafe { &*self.data }.edge_lengths[edge as usize];
                        if is_boundary {
                            external_boundary_length += l;
                        } else {
                            let neighbor_chart = self.face_charts[opposite_face as usize];
                            if neighbor_chart == -1 {
                                external_boundary_length += l;
                            } else if neighbor_chart != c {
                                if is_seam && (self.is_normal_seam(edge) || is_tex_seam) {
                                    external_boundary_length += l;
                                } else {
                                    self.shared_boundary_lengths[neighbor_chart as usize] += l;
                                }
                                self.shared_boundary_lengths_no_seams[neighbor_chart as usize] += l;
                                self.shared_boundary_edge_count_no_seams[neighbor_chart as usize] += 1;
                            }
                        }
                    }
                }
                for cc in (0..chart_count as i32).rev() {
                    if cc == c || self.charts[cc as usize].is_none() {
                        continue;
                    }
                    if self.shared_boundary_lengths[cc as usize] <= 0.0 {
                        continue;
                    }
                    let n0 = self.charts[cc as usize].as_ref().unwrap().basis.normal;
                    let n1 = self.charts[c as usize].as_ref().unwrap().basis.normal;
                    if dot3(n0, n1) < MERGE_CHARTS_MIN_NORMAL_DEVIATION {
                        continue;
                    }
                    let area = self.charts[c as usize].as_ref().unwrap().area
                        + self.charts[cc as usize].as_ref().unwrap().area;
                    if self.data().options.max_chart_area > 0.0
                        && area > self.data().options.max_chart_area
                    {
                        continue;
                    }
                    let bl = self.charts[c as usize].as_ref().unwrap().boundary_length
                        + self.charts[cc as usize].as_ref().unwrap().boundary_length
                        - self.shared_boundary_lengths_no_seams[cc as usize];
                    if self.data().options.max_boundary_length > 0.0
                        && bl > self.data().options.max_boundary_length
                    {
                        continue;
                    }
                    let do_merge = {
                        let chart = self.charts[c as usize].as_ref().unwrap();
                        let chart2 = self.charts[cc as usize].as_ref().unwrap();
                        let shared_ns = self.shared_boundary_lengths_no_seams[cc as usize];
                        let shared = self.shared_boundary_lengths[cc as usize];
                        let edge_count = self.shared_boundary_edge_count_no_seams[cc as usize];
                        (shared_ns > 0.0
                            && chart.faces.len() > 1
                            && chart2.faces.len() == 1
                            && chart2.area <= chart.area * 0.1)
                            || (chart2.faces.len() == 2 && edge_count >= 2)
                            || (shared_ns > 0.0
                                && equal_f(shared_ns, chart2.boundary_length, EPSILON))
                            || shared > 0.2 * (chart.boundary_length - external_boundary_length).max(0.0)
                            || shared > 0.75 * chart2.boundary_length
                    };
                    if !do_merge {
                        continue;
                    }
                    if self.merge_chart(c as u32, cc as u32, self.shared_boundary_lengths_no_seams[cc as usize])
                    {
                        merged = true;
                        break;
                    }
                }
                if merged {
                    break;
                }
            }
            if !merged {
                break;
            }
        }
        let mut c = 0i32;
        while c < self.charts.len() as i32 {
            if self.charts[c as usize].is_none() {
                self.charts.remove(c as usize);
                for i in 0..self.face_charts.len() {
                    if self.face_charts[i] > c {
                        self.face_charts[i] -= 1;
                    }
                }
            } else {
                self.charts[c as usize].as_mut().unwrap().id = c;
                c += 1;
            }
        }
    }

    fn create_chart(&mut self, threshold: f32) {
        let id = self.charts.len() as i32;
        let mut chart = ClusteredChart::new();
        chart.id = id;
        let mut seed = 0u32;
        let mut largest_area = 0.0f32;
        let face_count = self.data().mesh().face_count();
        for f in 0..face_count {
            if self.data().is_face_in_chart.get(f) {
                continue;
            }
            let area = self.planar().region_area(self.planar().region_id_from_face(f));
            if area > largest_area {
                largest_area = area;
                seed = f;
            }
        }
        chart.seed = seed;
        self.charts.push(Some(Box::new(chart)));
        let ci = self.charts.len() as u32 - 1;
        self.add_face_to_chart(ci, seed);
        loop {
            if self.charts[ci as usize].as_ref().unwrap().candidates.count() == 0
                || self.charts[ci as usize].as_ref().unwrap().candidates.peek_cost() > threshold
            {
                break;
            }
            let f = self.charts[ci as usize].as_mut().unwrap().candidates.pop();
            if self.data().is_face_in_chart.get(f) {
                continue;
            }
            if !self.add_face_to_chart(ci, f) {
                let region = self.planar().region_id_from_face(f);
                self.charts[ci as usize]
                    .as_mut()
                    .unwrap()
                    .failed_planar_regions
                    .push(region);
            }
        }
    }

    fn is_chart_boundary_edge(&self, chart_id: i32, edge: u32) -> bool {
        let opposite_edge = self.data().mesh().opposite_edge(edge);
        let opposite_face = mesh_edge_face(opposite_edge);
        opposite_edge == UINT32_MAX || self.face_charts[opposite_face as usize] != chart_id
    }

    fn compute_chart_basis(&mut self, chart_i: u32, basis: &mut Basis) -> bool {
        let faces = self.charts[chart_i as usize].as_ref().unwrap().faces.clone();
        self.temp_points.resize(faces.len() * 3, Vec3::splat(0.0));
        for (i, &f) in faces.iter().enumerate() {
            for j in 0..3 {
                self.temp_points[i * 3 + j] = self
                    .data()
                    .mesh()
                    .position(self.data().mesh().vertex_at(f * 3 + j as u32));
            }
        }
        Fit::compute_basis(&self.temp_points, basis)
    }

    fn is_face_flipped(&self, face: u32) -> bool {
        let v1 = self.texcoords[face as usize * 3];
        let v2 = self.texcoords[face as usize * 3 + 1];
        let v3 = self.texcoords[face as usize * 3 + 2];
        let parametric_area = ((v2.x - v1.x) * (v3.y - v1.y) - (v3.x - v1.x) * (v2.y - v1.y)) * 0.5;
        parametric_area < 0.0
    }

    fn parameterize_chart(&mut self, chart_i: u32) {
        let (faces, tangent, bitangent) = {
            let c = self.charts[chart_i as usize].as_ref().unwrap();
            (c.faces.clone(), c.basis.tangent, c.basis.bitangent)
        };
        for &face in &faces {
            for j in 0..3 {
                let offset = face * 3 + j;
                let pos = self.data().mesh().position(self.data().mesh().vertex_at(offset));
                self.texcoords[offset as usize] = Vec2::new(dot3(tangent, pos), dot3(bitangent, pos));
            }
        }
    }

    fn is_chart_parameterization_valid(&mut self, chart_i: u32) -> bool {
        let (faces, id) = {
            let c = self.charts[chart_i as usize].as_ref().unwrap();
            (c.faces.clone(), c.id)
        };
        let mut flipped = 0u32;
        for &f in &faces {
            if self.is_face_flipped(f) {
                flipped += 1;
            }
        }
        if flipped != 0 && flipped != faces.len() as u32 {
            return false;
        }
        self.boundary_grid.reset(&self.texcoords, &[], 0);
        for &f in &faces {
            for j in 0..3 {
                let edge = f * 3 + j;
                if self.is_chart_boundary_edge(id, edge) {
                    self.boundary_grid.append(edge);
                }
            }
        }
        let eps = self.data().mesh().epsilon();
        !self.boundary_grid.intersect(eps, None, &[])
    }

    fn add_face_to_chart(&mut self, chart_i: u32, face: u32) -> bool {
        debug_assert!(!self.data().is_face_in_chart.get(face));
        let old_face_count = self.charts[chart_i as usize].as_ref().unwrap().faces.len();
        let first_face = old_face_count == 0;
        self.charts[chart_i as usize]
            .as_mut()
            .unwrap()
            .faces
            .push(face);
        let mut coplanar = self.planar().next_region_face(face);
        while coplanar != face {
            self.charts[chart_i as usize]
                .as_mut()
                .unwrap()
                .faces
                .push(coplanar);
            coplanar = self.planar().next_region_face(coplanar);
        }
        let face_count = self.charts[chart_i as usize].as_ref().unwrap().faces.len();
        let mut basis = Basis::default();
        if first_face {
            let mesh = self.data().mesh();
            basis.normal = self.data().face_normals[face as usize];
            basis.tangent = normalize3(
                mesh.position(mesh.vertex_at(face * 3)) - mesh.position(mesh.vertex_at(face * 3 + 1)),
            );
            basis.bitangent = cross(basis.normal, basis.tangent);
        } else {
            if !self.compute_chart_basis(chart_i, &mut basis) {
                self.charts[chart_i as usize]
                    .as_mut()
                    .unwrap()
                    .faces
                    .truncate(old_face_count);
                return false;
            }
            if dot3(basis.normal, self.data().face_normals[face as usize]) < 0.0 {
                basis.normal = -basis.normal;
            }
        }
        if !first_face {
            self.parameterize_chart(chart_i);
            let id = self.charts[chart_i as usize].as_ref().unwrap().id;
            for i in old_face_count..face_count {
                let f = self.charts[chart_i as usize].as_ref().unwrap().faces[i];
                self.face_charts[f as usize] = id;
            }
            if !self.is_chart_parameterization_valid(chart_i) {
                for i in old_face_count..face_count {
                    let f = self.charts[chart_i as usize].as_ref().unwrap().faces[i];
                    self.face_charts[f as usize] = -1;
                }
                self.charts[chart_i as usize]
                    .as_mut()
                    .unwrap()
                    .faces
                    .truncate(old_face_count);
                return false;
            }
        }
        self.charts[chart_i as usize].as_mut().unwrap().basis = basis;
        let new_area = self.compute_area(chart_i, face);
        let new_bl = self.compute_boundary_length(chart_i, face);
        self.charts[chart_i as usize].as_mut().unwrap().area = new_area;
        self.charts[chart_i as usize].as_mut().unwrap().boundary_length = new_bl;
        let id = self.charts[chart_i as usize].as_ref().unwrap().id;
        for i in old_face_count..face_count {
            let f = self.charts[chart_i as usize].as_ref().unwrap().faces[i];
            self.face_charts[f as usize] = id;
            self.faces_left -= 1;
            self.data_mut().is_face_in_chart.set(f);
            let center = self.data().mesh().compute_face_center(f);
            self.charts[chart_i as usize].as_mut().unwrap().centroid_sum += center;
        }
        let nfaces = self.charts[chart_i as usize].as_ref().unwrap().faces.len() as f32;
        let sum = self.charts[chart_i as usize].as_ref().unwrap().centroid_sum;
        self.charts[chart_i as usize].as_mut().unwrap().centroid = sum / nfaces;
        self.charts[chart_i as usize].as_mut().unwrap().candidates.clear();
        let faces = self.charts[chart_i as usize].as_ref().unwrap().faces.clone();
        let failed = self.charts[chart_i as usize]
            .as_ref()
            .unwrap()
            .failed_planar_regions
            .clone();
        let mut cands: Vec<(f32, u32)> = Vec::new();
        for &f in &faces {
            for j in 0..3 {
                let edge = f * 3 + j;
                let oedge = self.data().mesh().opposite_edge(edge);
                if oedge == UINT32_MAX {
                    continue;
                }
                let oface = mesh_edge_face(oedge);
                if self.data().is_face_in_chart.get(oface) {
                    continue;
                }
                let region = self.planar().region_id_from_face(oface);
                if failed.contains(&region) {
                    continue;
                }
                let cost = self.compute_cost(chart_i, oface);
                if cost < f32::MAX {
                    cands.push((cost, oface));
                }
            }
        }
        for (cost, oface) in cands {
            self.charts[chart_i as usize]
                .as_mut()
                .unwrap()
                .candidates
                .push(cost, oface);
        }
        true
    }

    fn relocate_seed(&mut self, chart_i: u32) -> bool {
        let faces = self.charts[chart_i as usize].as_ref().unwrap().faces.clone();
        self.best_triangles.clear();
        for &f in &faces {
            let cost = self.compute_normal_deviation_metric(chart_i, f);
            self.best_triangles.push(cost, f);
        }
        let mut most_central = 0u32;
        let mut min_distance = f32::MAX;
        let centroid = self.charts[chart_i as usize].as_ref().unwrap().centroid;
        loop {
            if self.best_triangles.count() == 0 {
                break;
            }
            let face = self.best_triangles.pop();
            let face_centroid = self.data().mesh().compute_face_center(face);
            let distance = length3(centroid - face_centroid);
            if distance < min_distance {
                min_distance = distance;
                most_central = face;
            }
        }
        let old_seed = self.charts[chart_i as usize].as_ref().unwrap().seed;
        if most_central == old_seed {
            return false;
        }
        self.charts[chart_i as usize].as_mut().unwrap().seed = most_central;
        true
    }

    fn compute_cost(&self, chart_i: u32, face: u32) -> f32 {
        let new_chart_area = self.compute_area(chart_i, face);
        let new_boundary_length = self.compute_boundary_length(chart_i, face);
        let opt = &self.data().options;
        if opt.max_chart_area > 0.0 && new_chart_area > opt.max_chart_area {
            return f32::MAX;
        }
        if opt.max_boundary_length > 0.0 && new_boundary_length > opt.max_boundary_length {
            return f32::MAX;
        }
        let mut cost = 0.0;
        let normal_deviation = self.compute_normal_deviation_metric(chart_i, face);
        if normal_deviation >= 0.707 {
            return f32::MAX;
        }
        cost += opt.normal_deviation_weight * normal_deviation;
        let normal_seam = self.compute_normal_seam_metric(chart_i, face);
        if opt.normal_seam_weight >= 1000.0 && normal_seam > 0.0 {
            return f32::MAX;
        }
        cost += opt.normal_seam_weight * normal_seam;
        cost += opt.roundness_weight
            * self.compute_roundness_metric(chart_i, new_boundary_length, new_chart_area);
        cost += opt.straightness_weight * self.compute_straightness_metric(chart_i, face);
        cost += opt.texture_seam_weight * self.compute_texture_seam_metric(chart_i, face);
        cost
    }

    fn compute_normal_deviation_metric(&self, chart_i: u32, face: u32) -> f32 {
        let face_normal = self.data().face_normals[face as usize];
        let n = self.charts[chart_i as usize].as_ref().unwrap().basis.normal;
        (1.0 - dot3(face_normal, n)).min(1.0)
    }

    fn compute_roundness_metric(&self, chart_i: u32, new_bl: f32, new_area: f32) -> f32 {
        let chart = self.charts[chart_i as usize].as_ref().unwrap();
        let old_r = square(chart.boundary_length) / chart.area;
        let new_r = square(new_bl) / new_area;
        1.0 - old_r / new_r
    }

    fn compute_straightness_metric(&self, chart_i: u32, first_face: u32) -> f32 {
        let mut l_out = 0.0f32;
        let mut l_in = 0.0f32;
        let planar_region_id = self.planar().region_id_from_face(first_face);
        let chart_id = self.charts[chart_i as usize].as_ref().unwrap().id;
        let mut face = first_face;
        loop {
            let mut it = self.data().mesh().face_edge_iter(face);
            while !it.is_done() {
                let l = self.data().edge_lengths[it.edge() as usize];
                if it.is_boundary() {
                    l_out += l;
                } else if self.planar().region_id_from_face(it.opposite_face()) != planar_region_id {
                    if self.face_charts[it.opposite_face() as usize] != chart_id {
                        l_out += l;
                    } else {
                        l_in += l;
                    }
                }
                it.advance();
            }
            face = self.planar().next_region_face(face);
            if face == first_face {
                break;
            }
        }
        let ratio = (l_out - l_in) / (l_out + l_in);
        ratio.min(0.0)
    }

    fn is_normal_seam(&self, edge: u32) -> bool {
        let opposite_edge = self.data().mesh().opposite_edge(edge);
        if opposite_edge == UINT32_MAX {
            return false;
        }
        if self.data().mesh().flags() & MESH_HAS_NORMALS != 0 {
            let mesh = self.data().mesh();
            let v0 = mesh.vertex_at(mesh_edge_index0(edge));
            let v1 = mesh.vertex_at(mesh_edge_index1(edge));
            let ov0 = mesh.vertex_at(mesh_edge_index0(opposite_edge));
            let ov1 = mesh.vertex_at(mesh_edge_index1(opposite_edge));
            if v0 == ov1 && v1 == ov0 {
                return false;
            }
            return !equal3(mesh.normal(v0), mesh.normal(ov1), NORMAL_EPSILON)
                || !equal3(mesh.normal(v1), mesh.normal(ov0), NORMAL_EPSILON);
        }
        let f0 = mesh_edge_face(edge);
        let f1 = mesh_edge_face(opposite_edge);
        if self.planar().region_id_from_face(f0) == self.planar().region_id_from_face(f1) {
            return false;
        }
        !equal3(
            self.data().face_normals[f0 as usize],
            self.data().face_normals[f1 as usize],
            NORMAL_EPSILON,
        )
    }

    fn compute_normal_seam_metric(&self, chart_i: u32, first_face: u32) -> f32 {
        let mut seam_factor = 0.0f32;
        let mut total_length = 0.0f32;
        let chart_id = self.charts[chart_i as usize].as_ref().unwrap().id;
        let mut face = first_face;
        loop {
            let mut it = self.data().mesh().face_edge_iter(face);
            while !it.is_done() {
                if !it.is_boundary() && self.face_charts[it.opposite_face() as usize] == chart_id {
                    let mut l = self.data().edge_lengths[it.edge() as usize];
                    total_length += l;
                    if it.is_seam() && self.is_normal_seam(it.edge()) {
                        let d = if self.data().mesh().flags() & MESH_HAS_NORMALS != 0 {
                            let mesh = self.data().mesh();
                            let n0 = mesh.normal(it.vertex0());
                            let n1 = mesh.normal(it.vertex1());
                            let on0 = mesh.normal(mesh.vertex_at(mesh_edge_index0(it.opposite_edge())));
                            let on1 = mesh.normal(mesh.vertex_at(mesh_edge_index1(it.opposite_edge())));
                            let d0 = clamp(dot3(n0, on1), 0.0, 1.0);
                            let d1 = clamp(dot3(n1, on0), 0.0, 1.0);
                            (d0 + d1) * 0.5
                        } else {
                            clamp(
                                dot3(
                                    self.data().face_normals[face as usize],
                                    self.data().face_normals[mesh_edge_face(it.opposite_edge()) as usize],
                                ),
                                0.0,
                                1.0,
                            )
                        };
                        l *= 1.0 - d;
                        seam_factor += l;
                    }
                }
                it.advance();
            }
            face = self.planar().next_region_face(face);
            if face == first_face {
                break;
            }
        }
        if seam_factor <= 0.0 {
            0.0
        } else {
            seam_factor / total_length
        }
    }

    fn compute_texture_seam_metric(&self, chart_i: u32, first_face: u32) -> f32 {
        let mut seam_length = 0.0f32;
        let mut total_length = 0.0f32;
        let chart_id = self.charts[chart_i as usize].as_ref().unwrap().id;
        let mut face = first_face;
        loop {
            let mut it = self.data().mesh().face_edge_iter(face);
            while !it.is_done() {
                if !it.is_boundary() && self.face_charts[it.opposite_face() as usize] == chart_id {
                    let l = self.data().edge_lengths[it.edge() as usize];
                    total_length += l;
                    if it.is_seam() && it.is_texture_seam() {
                        seam_length += l;
                    }
                }
                it.advance();
            }
            face = self.planar().next_region_face(face);
            if face == first_face {
                break;
            }
        }
        if seam_length <= 0.0 {
            0.0
        } else {
            seam_length / total_length
        }
    }

    fn compute_area(&self, chart_i: u32, first_face: u32) -> f32 {
        let mut area = self.charts[chart_i as usize].as_ref().unwrap().area;
        let mut face = first_face;
        loop {
            area += self.data().face_areas[face as usize];
            face = self.planar().next_region_face(face);
            if face == first_face {
                break;
            }
        }
        area
    }

    fn compute_boundary_length(&self, chart_i: u32, first_face: u32) -> f32 {
        let mut boundary_length = self.charts[chart_i as usize].as_ref().unwrap().boundary_length;
        let planar_region_id = self.planar().region_id_from_face(first_face);
        let chart_id = self.charts[chart_i as usize].as_ref().unwrap().id;
        let mut face = first_face;
        loop {
            let mut it = self.data().mesh().face_edge_iter(face);
            while !it.is_done() {
                let edge_length = self.data().edge_lengths[it.edge() as usize];
                if it.is_boundary() {
                    boundary_length += edge_length;
                } else if self.planar().region_id_from_face(it.opposite_face()) != planar_region_id {
                    if self.face_charts[it.opposite_face() as usize] != chart_id {
                        boundary_length += edge_length;
                    } else {
                        boundary_length -= edge_length;
                    }
                }
                it.advance();
            }
            face = self.planar().next_region_face(face);
            if face == first_face {
                break;
            }
        }
        boundary_length.max(0.0)
    }

    fn merge_chart(&mut self, owner_i: u32, chart_i: u32, shared_boundary_length: f32) -> bool {
        let old_owner_face_count = self.charts[owner_i as usize].as_ref().unwrap().faces.len();
        let chart_faces = self.charts[chart_i as usize].as_ref().unwrap().faces.clone();
        let chart_id = self.charts[chart_i as usize].as_ref().unwrap().id;
        let owner_id = self.charts[owner_i as usize].as_ref().unwrap().id;
        self.charts[owner_i as usize]
            .as_mut()
            .unwrap()
            .faces
            .extend_from_slice(&chart_faces);
        for &f in &chart_faces {
            self.face_charts[f as usize] = owner_id;
        }
        let mut basis = Basis::default();
        if !self.compute_chart_basis(owner_i, &mut basis) {
            self.charts[owner_i as usize]
                .as_mut()
                .unwrap()
                .faces
                .truncate(old_owner_face_count);
            for &f in &chart_faces {
                self.face_charts[f as usize] = chart_id;
            }
            return false;
        }
        let first_owner_face = self.charts[owner_i as usize].as_ref().unwrap().faces[0];
        if dot3(basis.normal, self.data().face_normals[first_owner_face as usize]) < 0.0 {
            basis.normal = -basis.normal;
        }
        self.charts[owner_i as usize].as_mut().unwrap().basis = basis;
        self.parameterize_chart(owner_i);
        if !self.is_chart_parameterization_valid(owner_i) {
            self.charts[owner_i as usize]
                .as_mut()
                .unwrap()
                .faces
                .truncate(old_owner_face_count);
            for &f in &chart_faces {
                self.face_charts[f as usize] = chart_id;
            }
            return false;
        }
        let failed = self.charts[chart_i as usize]
            .as_ref()
            .unwrap()
            .failed_planar_regions
            .clone();
        self.charts[owner_i as usize]
            .as_mut()
            .unwrap()
            .failed_planar_regions
            .extend_from_slice(&failed);
        let add_area = self.charts[chart_i as usize].as_ref().unwrap().area;
        let add_bl = self.charts[chart_i as usize].as_ref().unwrap().boundary_length;
        self.charts[owner_i as usize].as_mut().unwrap().area += add_area;
        self.charts[owner_i as usize].as_mut().unwrap().boundary_length +=
            add_bl - shared_boundary_length;
        self.charts[chart_i as usize] = None;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartGeneratorType {
    OriginalUv,
    Planar,
    Clustered,
    Piecewise,
}

pub struct Atlas {
    data: AtlasData,
    original_uv: OriginalUvCharts,
    planar: PlanarCharts,
    clustered: ClusteredCharts,
}

impl Default for Atlas {
    fn default() -> Self {
        // Placeholder pointers; reset() fills data and the child structs point at it.
        // We rebuild children in reset/compute via a safer approach below.
        let data = AtlasData::default();
        // Leak-free: store data first, then reconstruct children after we have a stable address.
        // We'll use Box for data so the address is stable.
        Self::from_data(data)
    }
}

impl Atlas {
    fn from_data(data: AtlasData) -> Self {
        // Temporary dummy; real wiring happens in reset.
        let mut s = Self {
            data,
            original_uv: OriginalUvCharts::new(std::ptr::null_mut()),
            planar: PlanarCharts::new(std::ptr::null_mut()),
            clustered: ClusteredCharts::new(std::ptr::null_mut(), std::ptr::null()),
        };
        s.rewire();
        s
    }

    fn rewire(&mut self) {
        let data_ptr = &mut self.data as *mut AtlasData;
        self.original_uv.data = data_ptr;
        self.planar.data = data_ptr;
        self.clustered.data = data_ptr;
        self.clustered.planar = &self.planar as *const PlanarCharts;
    }

    pub fn chart_count(&self) -> u32 {
        self.original_uv.chart_count() + self.planar.chart_count() + self.clustered.chart_count()
    }

    pub fn chart_faces(&self, mut chart_index: u32) -> &[u32] {
        if chart_index < self.original_uv.chart_count() {
            return self.original_uv.chart_faces(chart_index);
        }
        chart_index -= self.original_uv.chart_count();
        if chart_index < self.planar.chart_count() {
            return self.planar.chart_faces(chart_index);
        }
        chart_index -= self.planar.chart_count();
        self.clustered.chart_faces(chart_index)
    }

    pub fn chart_basis(&self, mut chart_index: u32) -> Basis {
        if chart_index < self.original_uv.chart_count() {
            return self.original_uv.chart_basis(chart_index);
        }
        chart_index -= self.original_uv.chart_count();
        if chart_index < self.planar.chart_count() {
            return self.planar.chart_basis(chart_index);
        }
        chart_index -= self.planar.chart_count();
        self.clustered.chart_basis(chart_index)
    }

    pub fn chart_generator_type(&self, mut chart_index: u32) -> ChartGeneratorType {
        if chart_index < self.original_uv.chart_count() {
            return ChartGeneratorType::OriginalUv;
        }
        chart_index -= self.original_uv.chart_count();
        if chart_index < self.planar.chart_count() {
            return ChartGeneratorType::Planar;
        }
        ChartGeneratorType::Clustered
    }

    pub fn reset(&mut self, mesh: &Mesh, options: &ChartOptions) {
        self.data.options = options.clone();
        self.data.mesh = mesh as *const Mesh;
        self.data.compute();
        self.rewire();
    }

    pub fn compute(&mut self) {
        self.rewire();
        if self.data.options.use_input_mesh_uvs {
            self.original_uv.compute();
        }
        self.planar.compute();
        self.rewire();
        self.clustered.compute();
    }
}
