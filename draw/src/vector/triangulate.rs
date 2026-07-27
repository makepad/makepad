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

#[allow(clippy::too_many_arguments)]
pub fn tessellate_path_fill(
    path: &mut VectorPath,
    tess: &mut Tessellator,
    tess_verts: &mut Vec<VVertex>,
    tess_indices: &mut Vec<u32>,
    line_join: LineJoin,
    miter_limit: f32,
    aa: f32,
    gpu_expand_fill: bool,
    tolerance: f32,
) {
    tess.flatten(path, tolerance);
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

#[allow(clippy::too_many_arguments)]
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
    tolerance: f32,
) -> f32 {
    tessellate_path_stroke_ends(
        path,
        tess,
        tess_verts,
        tess_indices,
        stroke_width,
        line_cap,
        line_cap,
        line_join,
        miter_limit,
        aa,
        tolerance,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn tessellate_path_stroke_ends(
    path: &mut VectorPath,
    tess: &mut Tessellator,
    tess_verts: &mut Vec<VVertex>,
    tess_indices: &mut Vec<u32>,
    stroke_width: f32,
    start_cap: LineCap,
    end_cap: LineCap,
    line_join: LineJoin,
    miter_limit: f32,
    aa: f32,
    tolerance: f32,
) -> f32 {
    tess.flatten(path, tolerance);
    tess.stroke_ends(
        stroke_width,
        start_cap,
        end_cap,
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

/// `tessellate_path_stroke_ends` variant that also returns the centerline
/// anchor of every emitted vertex, for GPU re-expandable strokes.
#[allow(clippy::too_many_arguments)]
pub fn tessellate_path_stroke_ends_anchored(
    path: &mut VectorPath,
    tess: &mut Tessellator,
    tess_verts: &mut Vec<VVertex>,
    tess_indices: &mut Vec<u32>,
    tess_anchors: &mut Vec<[f32; 2]>,
    stroke_width: f32,
    start_cap: LineCap,
    end_cap: LineCap,
    line_join: LineJoin,
    miter_limit: f32,
    aa: f32,
    tolerance: f32,
) -> f32 {
    tess.flatten(path, tolerance);
    tess.stroke_ends_anchored(
        stroke_width,
        start_cap,
        end_cap,
        line_join,
        miter_limit,
        aa,
        tess_verts,
        tess_indices,
        tess_anchors,
    );
    compute_clip_radii(tess_verts, tess_indices);
    path.clear();
    if aa > 0.0 {
        (stroke_width * 0.5 + aa * 0.5) / aa
    } else {
        1e6
    }
}

/// Shape-id offset marking GPU-expandable stroke vertices: the vertex
/// position is the centerline anchor, param1/param2 carry the baked offset
/// and param3 the width-growth class. A zoom-aware vertex shader re-expands
/// the stroke at the width the current view calls for; plain shaders can
/// subtract the offset back. Fragment-side the shape behaves as
/// `shape_id - EXPAND_STROKE_SHAPE_OFFSET`.
pub const EXPAND_STROKE_SHAPE_OFFSET: f32 = 100.0;

/// Append stroke geometry in GPU re-expandable form: anchors as positions,
/// per-vertex offsets in param1/param2, width-growth class in param3.
pub fn append_expanded_stroke_geometry(
    verts: &[VVertex],
    anchors: &[[f32; 2]],
    indices: &[u32],
    acc_verts: &mut Vec<f32>,
    acc_indices: &mut Vec<u32>,
    params: VectorRenderParams,
    expand_class: f32,
) {
    if verts.is_empty() || indices.is_empty() || verts.len() != anchors.len() {
        return;
    }

    let base = (acc_verts.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    for (v, anchor) in verts.iter().zip(anchors) {
        acc_verts.push(anchor[0]);
        acc_verts.push(anchor[1]);
        acc_verts.push(v.u);
        acc_verts.push(v.v);
        acc_verts.push(params.color[0]);
        acc_verts.push(params.color[1]);
        acc_verts.push(params.color[2]);
        acc_verts.push(params.color[3]);
        acc_verts.push(params.stroke_mult);
        acc_verts.push(v.stroke_dist);
        acc_verts.push(params.shape_id + EXPAND_STROKE_SHAPE_OFFSET);
        acc_verts.push(params.params[0]);
        acc_verts.push(v.x - anchor[0]);
        acc_verts.push(v.y - anchor[1]);
        acc_verts.push(expand_class);
        acc_verts.push(params.params[4]);
        acc_verts.push(params.params[5]);
        acc_verts.push(v.clip_radius);
        acc_verts.push(params.zbias);
    }

    for &idx in indices {
        acc_indices.push(base + idx);
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
