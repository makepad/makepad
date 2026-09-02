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

/// Pack a tessellated symbol mesh into the 4-slot `IconVertexPacked` layout:
/// (x, y) are screen-px offsets from the instance anchor.
pub fn pack_icon_vertices(verts: &[VVertex]) -> Vec<f32> {
    let mut out = Vec::with_capacity(verts.len() * 4);
    for v in verts {
        out.extend_from_slice(&[v.x, v.y, pack_pair_f16(v.u, v.v), v.stroke_dist]);
    }
    out
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
/// IEEE 754 binary16 decode — inverse of `f16_bits` above.
#[inline]
fn f16_bits_to_f32(h: u32) -> f32 {
    let sign = (h & 0x8000) << 16;
    let exp = (h >> 10) & 0x1f;
    let frac = h & 0x3ff;
    if exp == 0 {
        if frac == 0 {
            return f32::from_bits(sign);
        }
        let v = frac as f32 * (-24f32).exp2();
        return if sign != 0 { -v } else { v };
    }
    if exp == 0x1f {
        return f32::from_bits(sign | 0x7f80_0000 | (frac << 13));
    }
    f32::from_bits(sign | ((exp + 112) << 23) | (frac << 13))
}

#[inline]
fn unpack_pair_f16(v: f32) -> (f32, f32) {
    let bits = v.to_bits();
    (f16_bits_to_f32(bits & 0xffff), f16_bits_to_f32(bits >> 16))
}

#[inline]
fn unpack_unorm8x4(v: f32) -> [f32; 4] {
    let b = v.to_bits();
    [
        (b & 0xff) as f32 / 255.0,
        ((b >> 8) & 0xff) as f32 / 255.0,
        ((b >> 16) & 0xff) as f32 / 255.0,
        ((b >> 24) & 0xff) as f32 / 255.0,
    ]
}

/// Midpoint of two 12-slot PACKED records — every channel is unpacked,
/// averaged and repacked (clip_radius takes the max, mirroring
/// `subdivide_face_mesh`). Per-feature constants midpoint to themselves, so
/// splitting a triangle never changes what the shader sees at a pixel.
fn midpoint_packed_record(a: &[f32], b: &[f32]) -> [f32; VECTOR_PACKED_FLOATS_PER_VERTEX] {
    let m = |x: f32, y: f32| (x + y) * 0.5;
    let pair = |x: f32, y: f32| {
        let (x0, x1) = unpack_pair_f16(x);
        let (y0, y1) = unpack_pair_f16(y);
        pack_pair_f16(m(x0, y0), m(x1, y1))
    };
    let color = |x: f32, y: f32| {
        let xc = unpack_unorm8x4(x);
        let yc = unpack_unorm8x4(y);
        pack_unorm8x4(m(xc[0], yc[0]), m(xc[1], yc[1]), m(xc[2], yc[2]), m(xc[3], yc[3]))
    };
    // slot 8 = pair(param, clip_radius): midpoint the param, MAX the radius.
    let clip = {
        let (xp, xr) = unpack_pair_f16(a[8]);
        let (yp, yr) = unpack_pair_f16(b[8]);
        pack_pair_f16(m(xp, yp), xr.max(yr))
    };
    [
        m(a[0], b[0]),
        m(a[1], b[1]),
        pair(a[2], b[2]),
        color(a[3], b[3]),
        m(a[4], b[4]),
        m(a[5], b[5]),
        pair(a[6], b[6]),
        pair(a[7], b[7]),
        clip,
        m(a[9], b[9]),
        m(a[10], b[10]),
        m(a[11], b[11]),
    ]
}

/// Crack-free midpoint refinement of an already-PACKED tile mesh: every
/// edge longer than `max_edge` (tile-local units) splits until the fixpoint
/// — shared midpoints via the edge map so neighboring triangles agree, the
/// same canonical-rotation scheme as `subdivide_face_mesh`. Used by the
/// space-warp mode, whose curved fold any long flat chord would slice
/// through; the triangulator itself is untouched — this runs on its output.
pub fn subdivide_packed_mesh(indices: &mut Vec<u32>, vertices: &mut Vec<f32>, max_edge: f32) {
    use std::collections::HashMap;
    const S: usize = VECTOR_PACKED_FLOATS_PER_VERTEX;
    if indices.is_empty() || vertices.len() < S || max_edge <= 0.0 {
        return;
    }
    let max_edge_sq = max_edge * max_edge;
    for _pass in 0..12 {
        let mut midpoints: HashMap<(u32, u32), u32> = HashMap::new();
        let mut out: Vec<u32> = Vec::with_capacity(indices.len());
        let mut split_any = false;
        let need_split = |vertices: &[f32], i: u32, j: u32| -> bool {
            let (vi, vj) = (i as usize * S, j as usize * S);
            let d2 = (vertices[vi] - vertices[vj]).powi(2)
                + (vertices[vi + 1] - vertices[vj + 1]).powi(2);
            d2 > max_edge_sq
        };
        for t in 0..indices.len() / 3 {
            let (mut a, mut b, mut c) = (indices[t * 3], indices[t * 3 + 1], indices[t * 3 + 2]);
            let (mut sab, mut sbc, mut sca) = (
                need_split(vertices, a, b),
                need_split(vertices, b, c),
                need_split(vertices, c, a),
            );
            for _ in 0..2 {
                let rotate = match (sab, sbc, sca) {
                    (false, true, _) | (false, false, true) => true,
                    (true, false, true) => true,
                    _ => false,
                };
                if !rotate {
                    break;
                }
                let (na, nb, nc) = (b, c, a);
                let (nab, nbc, nca) = (sbc, sca, sab);
                a = na;
                b = nb;
                c = nc;
                sab = nab;
                sbc = nbc;
                sca = nca;
            }
            let mut mid = |i: u32, j: u32, vertices: &mut Vec<f32>| -> u32 {
                let key = (i.min(j), i.max(j));
                if let Some(&midpoint) = midpoints.get(&key) {
                    return midpoint;
                }
                let (vi, vj) = (i as usize * S, j as usize * S);
                let mut ra = [0f32; S];
                let mut rb = [0f32; S];
                ra.copy_from_slice(&vertices[vi..vi + S]);
                rb.copy_from_slice(&vertices[vj..vj + S]);
                let record = midpoint_packed_record(&ra, &rb);
                vertices.extend_from_slice(&record);
                let midpoint = (vertices.len() / S - 1) as u32;
                midpoints.insert(key, midpoint);
                midpoint
            };
            match (sab, sbc, sca) {
                (false, false, false) => out.extend_from_slice(&[a, b, c]),
                (true, false, false) => {
                    let m = mid(a, b, vertices);
                    out.extend_from_slice(&[a, m, c, m, b, c]);
                    split_any = true;
                }
                (true, true, false) => {
                    let m1 = mid(a, b, vertices);
                    let m2 = mid(b, c, vertices);
                    out.extend_from_slice(&[a, m1, c, m1, m2, c, m1, b, m2]);
                    split_any = true;
                }
                (true, true, true) => {
                    let m1 = mid(a, b, vertices);
                    let m2 = mid(b, c, vertices);
                    let m3 = mid(c, a, vertices);
                    out.extend_from_slice(&[a, m1, m3, m1, b, m2, m3, m2, c, m1, m2, m3]);
                    split_any = true;
                }
                _ => out.extend_from_slice(&[a, b, c]),
            }
        }
        *indices = out;
        if !split_any {
            break;
        }
    }
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
