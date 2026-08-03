use makepad_svg::path::{LineCap, LineJoin, VectorPath};
use makepad_svg::tessellate::{compute_clip_radii, Tessellator, VVertex};

pub const VECTOR_FLOATS_PER_VERTEX: usize = 19;
/// Packed GPU layout: see `pack_vector_record` / VectorVertexPacked.
pub const VECTOR_PACKED_FLOATS_PER_VERTEX: usize = 12;

#[inline]
fn f16_bits(value: f32) -> u32 {
    // IEEE 754 binary16 encode (round-to-nearest-even, clamps to inf).
    let bits = value.to_bits();
    let sign = (bits >> 16) & 0x8000;
    let exp = ((bits >> 23) & 0xff) as i32;
    let frac = bits & 0x007f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if frac != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let frac = frac | 0x0080_0000;
        let shift = (14 - e) as u32;
        let half = frac >> shift;
        let rem = frac & ((1 << shift) - 1);
        let round = (rem > (1 << (shift - 1)))
            || (rem == (1 << (shift - 1)) && (half & 1) != 0);
        return sign | (half + round as u32);
    }
    let half = ((e as u32) << 10) | (frac >> 13);
    let rem = frac & 0x1fff;
    let round = (rem > 0x1000) || (rem == 0x1000 && (half & 1) != 0);
    sign | (half + round as u32)
}

/// Two floats into one f32 slot as an f16 pair; unpacked in-shader with
/// `unpack2f16`. Public so other packed vertex layouts reuse this rounding
/// rather than growing a second, subtly different implementation.
#[inline]
pub fn pack_pair_f16(a: f32, b: f32) -> f32 {
    f32::from_bits(f16_bits(a) | (f16_bits(b) << 16))
}

/// Four 0..1 channels into one f32 slot as unorm8x4; unpacked in-shader
/// with `unpack4u8`.
#[inline]
pub fn pack_unorm8x4(r: f32, g: f32, b: f32, a: f32) -> f32 {
    let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    f32::from_bits(q(r) | (q(g) << 8) | (q(b) << 16) | (q(a) << 24))
}

/// One 19-float logical record -> the 12-slot packed layout.
#[inline]
pub fn pack_vector_record(record: &[f32]) -> [f32; VECTOR_PACKED_FLOATS_PER_VERTEX] {
    [
        record[0],
        record[1],
        pack_pair_f16(record[2], record[3]),
        pack_unorm8x4(record[4], record[5], record[6], record[7]),
        record[8],
        // stroke_dist stays f32: multi-km merged roads exceed f16 range
        // (inf -> NaN varyings) and dash phase needs the precision.
        record[9],
        pack_pair_f16(record[11], record[10]),
        pack_pair_f16(record[12], record[13]),
        // clip_radius clamped into f16 range: huge radii mean "never
        // clipped" either way.
        pack_pair_f16(record[14], record[17].min(60000.0)),
        record[15],
        record[16],
        record[18],
    ]
}

/// Pack a whole 19-stride vertex buffer for GPU upload.
pub fn pack_vector_vertices(vertices: &[f32]) -> Vec<f32> {
    let count = vertices.len() / VECTOR_FLOATS_PER_VERTEX;
    let mut out = Vec::with_capacity(count * VECTOR_PACKED_FLOATS_PER_VERTEX);
    for record in vertices.chunks_exact(VECTOR_FLOATS_PER_VERTEX) {
        out.extend_from_slice(&pack_vector_record(record));
    }
    out
}
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
