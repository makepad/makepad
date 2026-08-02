use makepad_svg::path::{LineCap, LineJoin, VectorPath};
use makepad_svg::tessellate::{compute_clip_radii, Tessellator, VVertex};

pub const VECTOR_FLOATS_PER_VERTEX: usize = 19;
pub const VECTOR_ZBIAS_STEP: f32 = 0.000001;
/// Selects DrawVector's signed-coordinate analytic fill fringe. Ordinary
/// fills use `1e6`; a distinct sentinel lets the same vertex format carry a
/// deliberately wide raster carrier while its visible coverage remains one
/// device pixel.
pub const VECTOR_ANALYTIC_FRINGE_STROKE_MULT: f32 = 2e6;

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
    deck_m: f32,
    deck_override: Option<&[f32]>,
) {
    if verts.is_empty() || indices.is_empty() || verts.len() != anchors.len() {
        return;
    }

    // Bridge decks taper to ground over the segment ends so approaches
    // read as ramps (stroke_dist is the along-line distance).
    let total_dist = verts.iter().map(|v| v.stroke_dist).fold(0.0f32, f32::max);
    let ramp = (total_dist * 0.35).min(96.0).max(1e-3);

    let base = (acc_verts.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    let start = acc_verts.len();
    let floats = verts.len() * VECTOR_FLOATS_PER_VERTEX;
    // One resize + slot writes into the zeroed tail: the per-vertex
    // extend_from_slice of a stack array still re-checked capacity and
    // copied through a temporary 19 floats at a time — measurable on the
    // face/fringe path that now routes every morphable surface here.
    acc_verts.resize(start + floats, 0.0);
    let shape_id = params.shape_id + EXPAND_STROKE_SHAPE_OFFSET;
    let decked = deck_m > 0.0 || deck_override.is_some();
    for (vi, ((v, anchor), record)) in verts
        .iter()
        .zip(anchors)
        .zip(acc_verts[start..].chunks_exact_mut(VECTOR_FLOATS_PER_VERTEX))
        .enumerate()
    {
        let deck_v = if let Some(decks) = deck_override {
            decks.get(vi).copied().unwrap_or(0.0)
        } else if deck_m > 0.0 {
            // Smoothstep the ramp: linear tapers read as hard facets.
            let t = (v.stroke_dist.min(total_dist - v.stroke_dist) / ramp).clamp(0.0, 1.0);
            deck_m * t * t * (3.0 - 2.0 * t)
        } else {
            params.params[4]
        };
        // A lifted deck is semantically ABOVE whatever it crosses: bump its
        // tilt micro-depth with the lift, or high-rank strokes underneath
        // (rail over secondary) still depth-win near the crossing.
        let param5 = if decked {
            params.params[5] + 0.30 * (deck_v / 2.0).min(1.0)
        } else {
            params.params[5]
        };
        record[0] = anchor[0];
        record[1] = anchor[1];
        record[2] = v.u;
        record[3] = v.v;
        record[4] = params.color[0];
        record[5] = params.color[1];
        record[6] = params.color[2];
        record[7] = params.color[3];
        record[8] = params.stroke_mult;
        record[9] = v.stroke_dist;
        record[10] = shape_id;
        record[11] = params.params[0];
        record[12] = v.x - anchor[0];
        record[13] = v.y - anchor[1];
        record[14] = expand_class;
        record[15] = deck_v;
        record[16] = param5;
        record[17] = v.clip_radius;
        record[18] = params.zbias;
    }

    acc_indices.extend(indices.iter().map(|&idx| base + idx));
}

pub fn append_tessellated_geometry(
    verts: &[VVertex],
    indices: &[u32],
    acc_verts: &mut Vec<f32>,
    acc_indices: &mut Vec<u32>,
    params: VectorRenderParams,
) {
    append_tessellated_geometry_decked(verts, indices, acc_verts, acc_indices, params, None)
}

/// Fill variant with a per-vertex deck override (meters, parallel to
/// `verts`): road-polygon fills riding a bridge corridor replace the
/// constant params[4] deck and get the same depth bump as decked strokes so
/// the lifted deck wins over grounded geometry underneath.
pub fn append_tessellated_geometry_decked(
    verts: &[VVertex],
    indices: &[u32],
    acc_verts: &mut Vec<f32>,
    acc_indices: &mut Vec<u32>,
    params: VectorRenderParams,
    deck_override: Option<&[f32]>,
) {
    if verts.is_empty() || indices.is_empty() {
        return;
    }

    let base = (acc_verts.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    acc_verts.reserve(verts.len() * VECTOR_FLOATS_PER_VERTEX);
    for (vi, v) in verts.iter().enumerate() {
        let deck_v = match deck_override {
            Some(decks) => decks.get(vi).copied().unwrap_or(0.0),
            None => params.params[4],
        };
        let param5 = if deck_v > 0.0 {
            params.params[5] + 0.30 * (deck_v / 2.0).min(1.0)
        } else {
            params.params[5]
        };
        acc_verts.extend_from_slice(&[
            v.x,
            v.y,
            v.u,
            v.v,
            params.color[0],
            params.color[1],
            params.color[2],
            params.color[3],
            params.stroke_mult,
            v.stroke_dist,
            params.shape_id,
            params.params[0],
            params.params[1],
            params.params[2],
            params.params[3],
            deck_v,
            param5,
            v.clip_radius,
            params.zbias,
        ]);
    }

    acc_indices.extend(indices.iter().map(|&idx| base + idx));
}
