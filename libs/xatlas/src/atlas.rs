//! Public `parametrize` surface and Generate/AddMesh pipeline.

use crate::math::*;
use crate::mesh::*;
use crate::pack::PackAtlas;
use crate::param::ParamAtlas;
use crate::util::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddMeshError {
    Error,
    IndexOutOfRange,
    InvalidFaceVertexCount,
    InvalidIndexCount,
}

impl std::fmt::Display for AddMeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error => write!(f, "Unspecified error"),
            Self::IndexOutOfRange => write!(f, "Index out of range"),
            Self::InvalidFaceVertexCount => write!(f, "Invalid face vertex count"),
            Self::InvalidIndexCount => write!(f, "Invalid index count"),
        }
    }
}

impl std::error::Error for AddMeshError {}

#[derive(Clone, Debug)]
pub struct ChartOptions {
    pub max_chart_area: f32,
    pub max_boundary_length: f32,
    pub normal_deviation_weight: f32,
    pub roundness_weight: f32,
    pub straightness_weight: f32,
    pub normal_seam_weight: f32,
    pub texture_seam_weight: f32,
    pub max_cost: f32,
    pub max_iterations: u32,
    pub use_input_mesh_uvs: bool,
    pub fix_winding: bool,
}

impl Default for ChartOptions {
    fn default() -> Self {
        Self {
            max_chart_area: 0.0,
            max_boundary_length: 0.0,
            normal_deviation_weight: 2.0,
            roundness_weight: 0.01,
            straightness_weight: 6.0,
            normal_seam_weight: 4.0,
            texture_seam_weight: 0.5,
            max_cost: 2.0,
            max_iterations: 1,
            use_input_mesh_uvs: false,
            fix_winding: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PackOptions {
    pub max_chart_size: u32,
    pub padding: u32,
    pub texels_per_unit: f32,
    pub resolution: u32,
    pub bilinear: bool,
    pub block_align: bool,
    pub brute_force: bool,
    pub create_image: bool,
    pub rotate_charts_to_axis: bool,
    pub rotate_charts: bool,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            max_chart_size: 0,
            padding: 0,
            texels_per_unit: 0.0,
            resolution: 0,
            bilinear: true,
            block_align: false,
            brute_force: false,
            create_image: false,
            rotate_charts_to_axis: true,
            rotate_charts: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartType {
    Planar,
    Ortho,
    Lscm,
    Piecewise,
    Invalid,
}

/// One output vertex, matching `xatlas::Vertex`.
#[derive(Clone, Debug)]
pub struct Vertex {
    pub xref: u32,
    pub atlas_index: i32,
    pub chart_index: i32,
    /// Raw atlas-space UV (not divided by width/height).
    pub uv: [f32; 2],
}

#[derive(Clone, Debug)]
pub struct Parametrize {
    pub width: u32,
    pub height: u32,
    pub atlas_count: u32,
    pub chart_count: u32,
    pub texels_per_unit: f32,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<[u32; 3]>,
}

impl Parametrize {
    /// Hunyuan / xatlas-python UVs: `vertex.uv / (width, height)`.
    pub fn normalized_uvs(&self) -> Vec<[f32; 2]> {
        let w = self.width.max(1) as f32;
        let h = self.height.max(1) as f32;
        self.vertices
            .iter()
            .map(|v| [v.uv[0] / w, v.uv[1] / h])
            .collect()
    }

    pub fn vmapping(&self) -> Vec<u32> {
        self.vertices.iter().map(|v| v.xref).collect()
    }
}

/// Official Hunyuan wrap: split-seam mesh + unique `[0,1]` UVs.
/// `indices` is a flat triangle list (`3 * face_count`).
pub fn unwrap_mesh(
    positions: &[[f32; 3]],
    indices: &[u32],
) -> Result<(Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>), AddMeshError> {
    if indices.len() % 3 != 0 {
        return Err(AddMeshError::InvalidIndexCount);
    }
    let faces: Vec<[u32; 3]> = indices
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    let out = parametrize(positions, &faces)?;
    let new_positions: Vec<[f32; 3]> = out
        .vertices
        .iter()
        .map(|v| positions[v.xref as usize])
        .collect();
    let uvs = out
        .normalized_uvs()
        .into_iter()
        .map(|uv| [uv[0].clamp(0.0, 1.0), uv[1].clamp(0.0, 1.0)])
        .collect();
    let new_indices: Vec<u32> = out.indices.iter().flatten().copied().collect();
    Ok((new_positions, uvs, new_indices))
}

/// Official `xatlas.parametrize(positions, faces)` with default options.
pub fn parametrize(
    positions: &[[f32; 3]],
    faces: &[[u32; 3]],
) -> Result<Parametrize, AddMeshError> {
    parametrize_with_options(
        positions,
        faces,
        &ChartOptions::default(),
        &PackOptions::default(),
    )
}

pub fn parametrize_with_options(
    positions: &[[f32; 3]],
    faces: &[[u32; 3]],
    chart: &ChartOptions,
    pack: &PackOptions,
) -> Result<Parametrize, AddMeshError> {
    if positions.is_empty() || faces.is_empty() {
        return Err(AddMeshError::Error);
    }
    let vertex_count = positions.len() as u32;
    let index_count = (faces.len() * 3) as u32;
    if index_count % 3 != 0 {
        return Err(AddMeshError::InvalidIndexCount);
    }
    for face in faces {
        for &idx in face {
            if idx >= vertex_count {
                return Err(AddMeshError::IndexOutOfRange);
            }
        }
    }

    // MeshDecl defaults: epsilon = FLT_EPSILON, no normals/uvs, HasIgnoredFaces.
    const EPSILON: f32 = 1.192092896e-07;
    let mut mesh = Mesh::new(
        EPSILON,
        vertex_count,
        faces.len() as u32,
        MESH_HAS_IGNORED_FACES,
        0,
    );
    for p in positions {
        mesh.add_vertex(Vec3::new(p[0], p[1], p[2]), Vec3::splat(0.0), Vec2::splat(0.0));
    }
    const K_MAX_WARNINGS: u32 = 50;
    let mut _warning_count = 0u32;
    for (face_i, face) in faces.iter().enumerate() {
        let polygon = *face;
        let mut ignore = false;
        for i in 0..3 {
            let index1 = polygon[i];
            let index2 = polygon[(i + 1) % 3];
            if index1 == index2 {
                ignore = true;
                _warning_count += 1;
                break;
            }
            let pos1 = mesh.position(index1);
            let pos2 = mesh.position(index2);
            if length3(pos2 - pos1) <= 0.0 {
                ignore = true;
                _warning_count += 1;
                break;
            }
        }
        if !ignore {
            for i in 0..3 {
                let pos = mesh.position(polygon[i]);
                if is_nan_f(pos.x) || is_nan_f(pos.y) || is_nan_f(pos.z) {
                    ignore = true;
                    _warning_count += 1;
                    break;
                }
            }
        }
        if !ignore {
            let a = mesh.position(polygon[0]);
            let b = mesh.position(polygon[1]);
            let c = mesh.position(polygon[2]);
            let area = length3(cross(b - a, c - a)) * 0.5;
            if area <= AREA_EPSILON {
                ignore = true;
                _warning_count += 1;
            }
        }
        mesh.add_face(polygon, ignore, UINT32_MAX);
        let _ = face_i;
    }

    // AddMesh task: createColocals (ST scheduler runs immediately at join).
    mesh.create_colocals();

    let mut param_atlas = ParamAtlas::default();
    param_atlas.add_mesh(&mesh);
    param_atlas.compute_charts(chart);

    let mut pack_atlas = PackAtlas::default();
    pack_atlas.add_charts(&mut param_atlas);
    if pack.texels_per_unit < 0.0 {
        let mut p = pack.clone();
        p.texels_per_unit = 0.0;
        pack_atlas.pack_charts(&p);
    } else {
        pack_atlas.pack_charts(pack);
    }

    let width = pack_atlas.get_width();
    let height = pack_atlas.get_height();
    let atlas_count = pack_atlas.get_num_atlases();
    let chart_count = pack_atlas.get_chart_count();
    let texels_per_unit = pack_atlas.get_texels_per_unit();

    // Build output mesh (xatlas.cpp:9641).
    let invalid = param_atlas.invalid_mesh_geometry(0);
    let mut vertex_count_out = invalid.vertices().len() as u32;
    let mut index_count_out = invalid.faces().len() as u32 * 3;
    for cg in 0..param_atlas.chart_group_count(0) {
        let group = param_atlas.chart_group_at(0, cg);
        for c in 0..group.chart_count() {
            let ch = group.chart_at(c);
            vertex_count_out += ch.original_vertex_count();
            index_count_out += ch.unified_mesh().face_count() * 3;
        }
    }
    let mut vertices = vec![
        Vertex {
            xref: 0,
            atlas_index: -1,
            chart_index: -1,
            uv: [0.0, 0.0],
        };
        vertex_count_out as usize
    ];
    let mut indices_flat = vec![0u32; index_count_out as usize];

    // Invalid geometry first.
    {
        let verts = invalid.vertices();
        let faces = invalid.faces();
        let inds = invalid.indices();
        for v in 0..verts.len() {
            vertices[v] = Vertex {
                atlas_index: -1,
                chart_index: -1,
                uv: [0.0, 0.0],
                xref: verts[v],
            };
        }
        for f in 0..faces.len() {
            let index_offset = faces[f] * 3;
            for j in 0..3 {
                indices_flat[(index_offset + j) as usize] = inds[f * 3 + j as usize];
            }
        }
    }
    let mut first_vertex = invalid.vertices().len() as u32;
    let mut chart_index = 0u32;
    for cg in 0..param_atlas.chart_group_count(0) {
        let group = param_atlas.chart_group_at(0, cg);
        for c in 0..group.chart_count() {
            let ch = group.chart_at(c);
            let face_count = ch.unified_mesh().face_count();
            let pack_chart = pack_atlas.get_chart(chart_index);
            for v in 0..ch.original_vertex_count() {
                let atlas_index = pack_chart.atlas_index;
                let uv = pack_chart.vertices[ch.original_vertex_to_unified_vertex(v) as usize];
                vertices[(first_vertex + v) as usize] = Vertex {
                    atlas_index,
                    chart_index: chart_index as i32,
                    uv: [uv.x.max(0.0), uv.y.max(0.0)],
                    xref: ch.map_chart_vertex_to_source_vertex(v),
                };
            }
            for f in 0..face_count {
                let index_offset = ch.map_face_to_source_face(f) * 3;
                for j in 0..3 {
                    indices_flat[(index_offset + j) as usize] =
                        first_vertex + ch.original_vertices()[(f * 3 + j) as usize];
                }
            }
            chart_index += 1;
            first_vertex += ch.original_vertex_count();
        }
    }

    let mut indices = Vec::with_capacity(indices_flat.len() / 3);
    for t in indices_flat.chunks_exact(3) {
        indices.push([t[0], t[1], t[2]]);
    }

    Ok(Parametrize {
        width,
        height,
        atlas_count,
        chart_count,
        texels_per_unit,
        vertices,
        indices,
    })
}


