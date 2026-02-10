use makepad_svg::path::{LineCap, LineJoin, VectorPath};
use makepad_svg::tessellate::{compute_clip_radii, Tessellator, VVertex};

pub const VECTOR_FLOATS_PER_VERTEX: usize = 19;
pub const VECTOR_ZBIAS_STEP: f32 = 0.000001;

#[derive(Clone, Copy, Debug)]
pub struct VectorRenderParams {
    pub color: [f32; 4],
    pub stroke_mult: f32,
    pub shape_id: f32,
    pub params: [f32; 6],
    pub zbias: f32,
}

pub fn tessellate_path_fill(
    path: &mut VectorPath,
    tess: &mut Tessellator,
    tess_verts: &mut Vec<VVertex>,
    tess_indices: &mut Vec<u32>,
    line_join: LineJoin,
    miter_limit: f32,
    aa: f32,
    gpu_expand_fill: bool,
) {
    tess.flatten(path, 0.25);
    tess.fill(
        aa,
        line_join,
        miter_limit,
        gpu_expand_fill,
        tess_verts,
        tess_indices,
    );
    compute_clip_radii(tess_verts, tess_indices);
    path.clear();
}

pub fn tessellate_path_stroke(
    path: &mut VectorPath,
    tess: &mut Tessellator,
    tess_verts: &mut Vec<VVertex>,
    tess_indices: &mut Vec<u32>,
    stroke_width: f32,
    line_cap: LineCap,
    line_join: LineJoin,
    miter_limit: f32,
    aa: f32,
) -> f32 {
    tess.flatten(path, 0.25);
    tess.stroke(
        stroke_width,
        line_cap,
        line_join,
        miter_limit,
        aa,
        tess_verts,
        tess_indices,
    );
    compute_clip_radii(tess_verts, tess_indices);
    path.clear();
    if aa > 0.0 {
        (stroke_width * 0.5 + aa * 0.5) / aa
    } else {
        1e6
    }
}

pub fn append_tessellated_geometry(
    verts: &[VVertex],
    indices: &[u32],
    acc_verts: &mut Vec<f32>,
    acc_indices: &mut Vec<u32>,
    params: VectorRenderParams,
) {
    if verts.is_empty() || indices.is_empty() {
        return;
    }

    let base = (acc_verts.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    for v in verts {
        acc_verts.push(v.x);
        acc_verts.push(v.y);
        acc_verts.push(v.u);
        acc_verts.push(v.v);
        acc_verts.push(params.color[0]);
        acc_verts.push(params.color[1]);
        acc_verts.push(params.color[2]);
        acc_verts.push(params.color[3]);
        acc_verts.push(params.stroke_mult);
        acc_verts.push(v.stroke_dist);
        acc_verts.push(params.shape_id);
        acc_verts.push(params.params[0]);
        acc_verts.push(params.params[1]);
        acc_verts.push(params.params[2]);
        acc_verts.push(params.params[3]);
        acc_verts.push(params.params[4]);
        acc_verts.push(params.params[5]);
        acc_verts.push(v.clip_radius);
        acc_verts.push(params.zbias);
    }

    for &idx in indices {
        acc_indices.push(base + idx);
    }
}
