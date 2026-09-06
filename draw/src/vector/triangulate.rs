use makepad_svg::path::{LineCap, LineJoin, VectorPath};
use makepad_svg::tessellate::{compute_clip_radii, Tessellator, VVertex};
use crate::geometry::geometry_gen::{
    FaceVertexTyped, FillVertexTyped, RoadVertexTyped, RoofVertexTyped,
};
use crate::makepad_platform::{F16x2, I16x2, U16x2, UNorm8x4};

pub const VECTOR_FLOATS_PER_VERTEX: usize = 19;
/// Packed GPU layout: see `pack_vector_record` / VectorVertexPacked.
pub const VECTOR_PACKED_FLOATS_PER_VERTEX: usize = 12;
pub const FILL_TYPED_VERTEX_BYTES: usize = std::mem::size_of::<FillVertexTyped>();
pub const ROOF_TYPED_VERTEX_BYTES: usize = std::mem::size_of::<RoofVertexTyped>();
const FILL_PARAM5_STEP: f32 = 0.00001;
pub const ROAD_TYPED_VERTEX_BYTES: usize = std::mem::size_of::<RoadVertexTyped>();
pub const FACE_TYPED_VERTEX_BYTES: usize = std::mem::size_of::<FaceVertexTyped>();

/// Signed fixed-point units per tile unit for typed map anchors. The road
/// clip domain `[-3, 259]` occupies `[-192, 16576]`; nearest rounding is at
/// most 1/128 tile unit, or 0.125 px for a z14 tile viewed at z18 (16x).
/// The shader uses this same value through a `script_mod!` Rust splice.
pub const MAP_VERTEX_POSITION_SCALE: f32 = 64.0;

// Road params.x is an exactly representable f16 integer. Its low six bits
// retain the requested class + 8*material encoding; the upper bits carry the
// three dash masks and the fill/stroke/fringe dispatch needed by DrawMapRoad.
pub const ROAD_PARAM_DASH_SCALE: f32 = 64.0;
pub const ROAD_PARAM_KIND_SCALE: f32 = 256.0;
pub const ROAD_KIND_STROKE: f32 = 0.0;
pub const ROAD_KIND_FILL: f32 = 1.0;
pub const ROAD_KIND_FRINGE: f32 = 2.0;
/// params.x bit: the record is a GPU-expandable stroke/face (shape >= 100),
/// so the shader applies the baked offset with its width-class correction.
pub const ROAD_PARAM_EXPANDED_FLAG: f32 = 1024.0;

#[inline]
fn f16_bits(value: f32) -> u32 {
    makepad_math::f32_to_f16_bits(value) as u32
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

/// The compact fill shader has one code lane for the exact shape/material
/// pairs emitted by the map's ground-fill pass. Pattern codes imply their
/// material because the inherited pixel path ignores material for shapes
/// 30..32. Everything else must stay on `VectorVertexPacked`.
#[inline]
pub fn map_fill_variant_code(record: &[f32]) -> Option<f32> {
    if record.len() < VECTOR_FLOATS_PER_VERTEX
        || record[8] <= 1e5
        || record[8] >= 1.5e6
        || record[11] != 0.0
        || record[12] != 0.0
        || record[13] != 0.0
        || record[15] != 0.0
    {
        return None;
    }
    let (shape, material) = (record[10], record[14]);
    if shape == 0.0 && (material == 0.0 || material == 3.0 || material == 5.0) {
        Some(material)
    } else if (shape == 30.0 || shape == 31.0 || shape == 32.0)
        && (material == 0.0 || material == 3.0 || material == 5.0)
    {
        Some(shape)
    } else {
        None
    }
}

#[inline]
fn pack_position(x: f32, y: f32) -> I16x2 {
    let x = x * MAP_VERTEX_POSITION_SCALE;
    let y = y * MAP_VERTEX_POSITION_SCALE;
    debug_assert!((i16::MIN as f32..=i16::MAX as f32).contains(&x));
    debug_assert!((i16::MIN as f32..=i16::MAX as f32).contains(&y));
    I16x2::from_f32(x, y)
}

#[inline]
pub fn unpack_typed_position(pos: I16x2) -> (f32, f32) {
    (
        pos.x as f32 / MAP_VERTEX_POSITION_SCALE,
        pos.y as f32 / MAP_VERTEX_POSITION_SCALE,
    )
}

#[inline]
fn pack_fill_depths(zbias: f32, param5: f32) -> U16x2 {
    let z = (zbias.max(0.0) / VECTOR_ZBIAS_STEP)
        .round()
        .min(u16::MAX as f32) as u16;
    let p = (param5.max(0.0) / FILL_PARAM5_STEP)
        .round()
        .min(u16::MAX as f32) as u16;
    U16x2::from_u16(z, p)
}

/// Decode the fixed-point depth pair carried in `FillVertexTyped::zbias`.
#[inline]
pub fn unpack_fill_depths(value: U16x2) -> (f32, f32) {
    (
        value.x as f32 * VECTOR_ZBIAS_STEP,
        value.y as f32 * FILL_PARAM5_STEP,
    )
}

/// One logical 19-float ground-fill record -> one 16-byte typed vertex.
/// Returns `None` for strokes, lifted/decked geometry, gradients and any
/// shape/material pair not handled by the compact fill shader.
#[inline]
pub fn pack_fill_record(record: &[f32]) -> Option<FillVertexTyped> {
    let code = map_fill_variant_code(record)?;
    Some(FillVertexTyped {
        pos: pack_position(record[0], record[1]),
        color: UNorm8x4::from_f32(record[4], record[5], record[6], record[7]),
        params: F16x2::from_f32(code, record[2]),
        zbias: pack_fill_depths(record[18], record[16]),
    })
}

/// Pack a buffer already classified as typed map fills.
pub fn pack_fill_vertices(vertices: &[f32]) -> Vec<u8> {
    let count = vertices.len() / VECTOR_FLOATS_PER_VERTEX;
    let mut out = Vec::with_capacity(count * FILL_TYPED_VERTEX_BYTES);
    for record in vertices.chunks_exact(VECTOR_FLOATS_PER_VERTEX) {
        append_fill_vertex(
            &mut out,
            pack_fill_record(record).expect("non-fill record in typed map fill stream"),
        );
    }
    out
}

/// Largest consecutive non-negative integer exactly representable by f16.
/// Road z-bias is stored in these integer `VECTOR_ZBIAS_STEP` ticks.
pub const ROAD_ZBIAS_MAX_EXACT_TICKS: f32 = 2048.0;

/// Pack one logical 19-float map-road record into the 28-byte typed layout.
/// Accepted records are GPU-expandable strokes (shape >= 100) and shape-0
/// union faces. params.x is class + 8*material plus compact dash (the
/// `get_stroke_mask` ids 10/11/12 as 1/2/3) and kind (stroke/fill/fringe)
/// tags. params.y is along-stroke distance for strokes, route-emissive
/// strength for material 7, and coverage otherwise; the tessellator's
/// complete u/v pair has its own slot.
pub fn pack_road_record(record: &[f32]) -> RoadVertexTyped {
    let expanded = record[10] >= EXPAND_STROKE_SHAPE_OFFSET - 0.5;
    debug_assert!(expanded || record[10].abs() < 0.5);
    let fringe = record[8] > 1.5e6;
    let shape_id = if expanded {
        record[10] - EXPAND_STROKE_SHAPE_OFFSET
    } else {
        0.0
    };
    let dash = if (shape_id - 10.0).abs() < 0.5 {
        1.0
    } else if (shape_id - 11.0).abs() < 0.5 {
        2.0
    } else if (shape_id - 12.0).abs() < 0.5 {
        3.0
    } else {
        0.0
    };
    let kind = if fringe {
        ROAD_KIND_FRINGE
    } else if record[8] > 1e5 {
        ROAD_KIND_FILL
    } else {
        ROAD_KIND_STROKE
    };
    let (class, material) = if expanded {
        (record[14].round().clamp(0.0, 7.0), 0.0)
    } else {
        (0.0, record[14].round().clamp(0.0, 7.0))
    };
    let meta = class
        + 8.0 * material
        + ROAD_PARAM_DASH_SCALE * dash
        + ROAD_PARAM_KIND_SCALE * kind
        + if expanded { ROAD_PARAM_EXPANDED_FLAG } else { 0.0 };
    let aux = if kind == ROAD_KIND_STROKE {
        record[9]
    } else if material > 6.5 {
        record[12]
    } else if fringe {
        (record[2] + 1.0).clamp(0.0, 1.0)
    } else {
        record[2].clamp(0.0, 1.0)
    };
    let (off_x, off_y) = if expanded {
        (record[12], record[13])
    } else {
        (0.0, 0.0)
    };
    let zbias_ticks = (record[18] / VECTOR_ZBIAS_STEP).round();
    debug_assert!(
        (0.0..=ROAD_ZBIAS_MAX_EXACT_TICKS).contains(&zbias_ticks),
        "road zbias tick {zbias_ticks} exceeds the exact f16 range"
    );
    RoadVertexTyped {
        pos: pack_position(record[0], record[1]),
        off: F16x2::from_f32(off_x, off_y),
        color: UNorm8x4::from_f32(record[4], record[5], record[6], record[7]),
        params: F16x2::from_f32(meta, aux),
        deck: record[15],
        depth: F16x2::from_f32(record[16], zbias_ticks),
        uv: F16x2::from_f32(record[2], record[3]),
    }
}

pub fn pack_road_vertices(vertices: &[f32]) -> Vec<u8> {
    let count = vertices.len() / VECTOR_FLOATS_PER_VERTEX;
    let mut out = Vec::with_capacity(count * ROAD_TYPED_VERTEX_BYTES);
    for record in vertices.chunks_exact(VECTOR_FLOATS_PER_VERTEX) {
        append_road_vertex(&mut out, pack_road_record(record));
    }
    out
}

/// The road-shader inputs a `FaceVertexTyped` record carries implicitly.
/// The face shader substitutes exactly these values, so a road record
/// holding them bit-for-bit draws identically from the 16-byte layout.
pub const FACE_IMPLICIT_OFF: F16x2 = F16x2 { x: 0, y: 0 };
pub const FACE_IMPLICIT_DECK: f32 = 0.0;
pub const FACE_IMPLICIT_UV: (f32, f32) = (0.5, 1.0);

/// Project a packed road record onto the face layout. `None` when the
/// record is a GPU-expandable stroke or when any dropped field differs from
/// its implicit value (a lifted face, a deck fascia wall, an AA fringe):
/// those stay on the road layout. The kept fields are copied bit-exact.
#[inline]
pub fn face_record_from_road(road: RoadVertexTyped) -> Option<FaceVertexTyped> {
    let (meta, _) = road.params.to_f32();
    if meta >= ROAD_PARAM_EXPANDED_FLAG
        || road.off != FACE_IMPLICIT_OFF
        || road.deck.to_bits() != FACE_IMPLICIT_DECK.to_bits()
        || road.uv != F16x2::from_f32(FACE_IMPLICIT_UV.0, FACE_IMPLICIT_UV.1)
    {
        return None;
    }
    Some(FaceVertexTyped {
        pos: road.pos,
        color: road.color,
        params: road.params,
        depth: road.depth,
    })
}

/// The 28-byte road record a face record stands for: the kept fields plus
/// the implicit constants. `face_record_from_road` inverts it exactly.
#[inline]
pub fn road_record_from_face(face: FaceVertexTyped) -> RoadVertexTyped {
    RoadVertexTyped {
        pos: face.pos,
        off: FACE_IMPLICIT_OFF,
        color: face.color,
        params: face.params,
        deck: FACE_IMPLICIT_DECK,
        depth: face.depth,
        uv: F16x2::from_f32(FACE_IMPLICIT_UV.0, FACE_IMPLICIT_UV.1),
    }
}

/// One logical 19-float road-pass record -> one 16-byte face vertex, when
/// its road packing projects losslessly (see `face_record_from_road`).
#[inline]
pub fn pack_face_record(record: &[f32]) -> Option<FaceVertexTyped> {
    face_record_from_road(pack_road_record(record))
}

/// Whether a road-pass record can move to the face stream without changing
/// what the road shader would have computed for it.
#[inline]
pub fn is_compact_face_record(record: &[f32]) -> bool {
    record.len() >= VECTOR_FLOATS_PER_VERTEX && pack_face_record(record).is_some()
}

/// Pack a buffer already classified as compact road-union faces.
pub fn pack_face_vertices(vertices: &[f32]) -> Vec<u8> {
    let count = vertices.len() / VECTOR_FLOATS_PER_VERTEX;
    let mut out = Vec::with_capacity(count * FACE_TYPED_VERTEX_BYTES);
    for record in vertices.chunks_exact(VECTOR_FLOATS_PER_VERTEX) {
        append_face_vertex(
            &mut out,
            pack_face_record(record).expect("non-face record in typed map face stream"),
        );
    }
    out
}

/// Whether a logical vector record can use the compact roof shader without
/// dropping any channel that its vertex/fragment paths observe. Parapet AO
/// records carry a distinct tilted depth and therefore stay on the generic
/// layout alongside any future gradient or patterned roofs.
#[inline]
pub fn is_compact_roof_record(record: &[f32]) -> bool {
    if record.len() < VECTOR_FLOATS_PER_VERTEX {
        return false;
    }
    let surface_depth = if record[15] > 0.0 {
        0.5 + 0.30 * (record[15] / 2.0).min(1.0)
    } else {
        0.5
    };
    record[2] == 0.5
        && record[3] == 1.0
        && record[8] > 1e5
        && record[8] < 1.5e6
        && record[9] == 0.0
        && record[10] == 0.0
        && record[11] == 0.0
        && record[12] == 0.0
        && record[13] == 0.0
        && record[14] == crate::scene_sun::MAT_ROOF
        && record[15].is_finite()
        && record[15] >= 0.0
        && record[16] == surface_depth
        && record[18].is_finite()
        && record[18] >= 0.0
        && record[18] / VECTOR_ZBIAS_STEP <= 65504.0
}

/// One logical lifted shape-0 roof record -> one 16-byte typed vertex.
#[inline]
pub fn pack_roof_record(record: &[f32]) -> Option<RoofVertexTyped> {
    if !is_compact_roof_record(record) {
        return None;
    }
    Some(RoofVertexTyped {
        pos: pack_position(record[0], record[1]),
        color: UNorm8x4::from_f32(record[4], record[5], record[6], record[7]),
        height: record[15],
        params: F16x2::from_f32(record[14], (record[18] / VECTOR_ZBIAS_STEP).round()),
    })
}

/// Pack a buffer already classified as compact map roofs.
pub fn pack_roof_vertices(vertices: &[f32]) -> Vec<u8> {
    let count = vertices.len() / VECTOR_FLOATS_PER_VERTEX;
    let mut out = Vec::with_capacity(count * ROOF_TYPED_VERTEX_BYTES);
    for record in vertices.chunks_exact(VECTOR_FLOATS_PER_VERTEX) {
        append_roof_vertex(
            &mut out,
            pack_roof_record(record).expect("non-roof record in typed map roof stream"),
        );
    }
    out
}

#[inline]
fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_ne_bytes());
}

#[inline]
fn push_i16(out: &mut Vec<u8>, value: i16) {
    out.extend_from_slice(&value.to_ne_bytes());
}

#[inline]
fn push_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_ne_bytes());
}

fn append_fill_vertex(out: &mut Vec<u8>, vertex: FillVertexTyped) {
    push_i16(out, vertex.pos.x);
    push_i16(out, vertex.pos.y);
    out.extend_from_slice(&vertex.color.0);
    push_u16(out, vertex.params.x);
    push_u16(out, vertex.params.y);
    push_u16(out, vertex.zbias.x);
    push_u16(out, vertex.zbias.y);
}

fn append_road_vertex(out: &mut Vec<u8>, vertex: RoadVertexTyped) {
    push_i16(out, vertex.pos.x);
    push_i16(out, vertex.pos.y);
    push_u16(out, vertex.off.x);
    push_u16(out, vertex.off.y);
    out.extend_from_slice(&vertex.color.0);
    push_u16(out, vertex.params.x);
    push_u16(out, vertex.params.y);
    push_f32(out, vertex.deck);
    push_u16(out, vertex.depth.x);
    push_u16(out, vertex.depth.y);
    push_u16(out, vertex.uv.x);
    push_u16(out, vertex.uv.y);
}

fn append_face_vertex(out: &mut Vec<u8>, vertex: FaceVertexTyped) {
    push_i16(out, vertex.pos.x);
    push_i16(out, vertex.pos.y);
    out.extend_from_slice(&vertex.color.0);
    push_u16(out, vertex.params.x);
    push_u16(out, vertex.params.y);
    push_u16(out, vertex.depth.x);
    push_u16(out, vertex.depth.y);
}

fn append_roof_vertex(out: &mut Vec<u8>, vertex: RoofVertexTyped) {
    push_i16(out, vertex.pos.x);
    push_i16(out, vertex.pos.y);
    out.extend_from_slice(&vertex.color.0);
    push_f32(out, vertex.height);
    push_u16(out, vertex.params.x);
    push_u16(out, vertex.params.y);
}

#[inline]
fn read_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_ne_bytes([bytes[at], bytes[at + 1]])
}

#[inline]
fn read_i16(bytes: &[u8], at: usize) -> i16 {
    i16::from_ne_bytes([bytes[at], bytes[at + 1]])
}

#[inline]
fn read_f32(bytes: &[u8], at: usize) -> f32 {
    f32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap())
}

pub fn decode_fill_vertex(bytes: &[u8]) -> FillVertexTyped {
    assert!(bytes.len() >= FILL_TYPED_VERTEX_BYTES);
    FillVertexTyped {
        pos: I16x2::from_i16(read_i16(bytes, 0), read_i16(bytes, 2)),
        color: UNorm8x4(bytes[4..8].try_into().unwrap()),
        params: F16x2 { x: read_u16(bytes, 8), y: read_u16(bytes, 10) },
        zbias: U16x2::from_u16(read_u16(bytes, 12), read_u16(bytes, 14)),
    }
}

pub fn decode_road_vertex(bytes: &[u8]) -> RoadVertexTyped {
    assert!(bytes.len() >= ROAD_TYPED_VERTEX_BYTES);
    RoadVertexTyped {
        pos: I16x2::from_i16(read_i16(bytes, 0), read_i16(bytes, 2)),
        off: F16x2 { x: read_u16(bytes, 4), y: read_u16(bytes, 6) },
        color: UNorm8x4(bytes[8..12].try_into().unwrap()),
        params: F16x2 { x: read_u16(bytes, 12), y: read_u16(bytes, 14) },
        deck: read_f32(bytes, 16),
        depth: F16x2 { x: read_u16(bytes, 20), y: read_u16(bytes, 22) },
        uv: F16x2 { x: read_u16(bytes, 24), y: read_u16(bytes, 26) },
    }
}

pub fn decode_face_vertex(bytes: &[u8]) -> FaceVertexTyped {
    assert!(bytes.len() >= FACE_TYPED_VERTEX_BYTES);
    FaceVertexTyped {
        pos: I16x2::from_i16(read_i16(bytes, 0), read_i16(bytes, 2)),
        color: UNorm8x4(bytes[4..8].try_into().unwrap()),
        params: F16x2 { x: read_u16(bytes, 8), y: read_u16(bytes, 10) },
        depth: F16x2 { x: read_u16(bytes, 12), y: read_u16(bytes, 14) },
    }
}

pub fn decode_roof_vertex(bytes: &[u8]) -> RoofVertexTyped {
    assert!(bytes.len() >= ROOF_TYPED_VERTEX_BYTES);
    RoofVertexTyped {
        pos: I16x2::from_i16(read_i16(bytes, 0), read_i16(bytes, 2)),
        color: UNorm8x4(bytes[4..8].try_into().unwrap()),
        height: read_f32(bytes, 8),
        params: F16x2 { x: read_u16(bytes, 12), y: read_u16(bytes, 14) },
    }
}

fn fill_vertex_bytes(vertex: FillVertexTyped) -> [u8; FILL_TYPED_VERTEX_BYTES] {
    let mut bytes = Vec::with_capacity(FILL_TYPED_VERTEX_BYTES);
    append_fill_vertex(&mut bytes, vertex);
    bytes.try_into().unwrap()
}

fn road_vertex_bytes(vertex: RoadVertexTyped) -> [u8; ROAD_TYPED_VERTEX_BYTES] {
    let mut bytes = Vec::with_capacity(ROAD_TYPED_VERTEX_BYTES);
    append_road_vertex(&mut bytes, vertex);
    bytes.try_into().unwrap()
}

fn face_vertex_bytes(vertex: FaceVertexTyped) -> [u8; FACE_TYPED_VERTEX_BYTES] {
    let mut bytes = Vec::with_capacity(FACE_TYPED_VERTEX_BYTES);
    append_face_vertex(&mut bytes, vertex);
    bytes.try_into().unwrap()
}

/// IEEE 754 binary16 decode — inverse of `f16_bits` above.
#[inline]
fn f16_bits_to_f32(h: u32) -> f32 {
    makepad_math::f16_bits_to_f32(h as u16)
}

#[inline]
pub fn unpack_pair_f16(v: f32) -> (f32, f32) {
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

fn midpoint_fill_typed_record(
    a: &[u8],
    b: &[u8],
) -> [u8; FILL_TYPED_VERTEX_BYTES] {
    let m = |x: f32, y: f32| (x + y) * 0.5;
    let a = decode_fill_vertex(a);
    let b = decode_fill_vertex(b);
    let (ax, ay) = unpack_typed_position(a.pos);
    let (bx, by) = unpack_typed_position(b.pos);
    let ac = a.color.to_f32();
    let bc = b.color.to_f32();
    let ap = a.params.to_f32();
    let bp = b.params.to_f32();
    fill_vertex_bytes(FillVertexTyped {
        pos: pack_position(m(ax, bx), m(ay, by)),
        color: UNorm8x4::from_f32(
            m(ac.0, bc.0),
            m(ac.1, bc.1),
            m(ac.2, bc.2),
            m(ac.3, bc.3),
        ),
        params: F16x2::from_f32(m(ap.0, bp.0), m(ap.1, bp.1)),
        zbias: U16x2::from_f32(
            m(a.zbias.x as f32, b.zbias.x as f32),
            m(a.zbias.y as f32, b.zbias.y as f32),
        ),
    })
}

fn midpoint_road_record(a: &[u8], b: &[u8]) -> [u8; ROAD_TYPED_VERTEX_BYTES] {
    let m = |x: f32, y: f32| (x + y) * 0.5;
    let pair = |x: F16x2, y: F16x2| {
        let x = x.to_f32();
        let y = y.to_f32();
        F16x2::from_f32(m(x.0, y.0), m(x.1, y.1))
    };
    let color = |x: UNorm8x4, y: UNorm8x4| {
        let x = x.to_f32();
        let y = y.to_f32();
        UNorm8x4::from_f32(m(x.0, y.0), m(x.1, y.1), m(x.2, y.2), m(x.3, y.3))
    };
    let a = decode_road_vertex(a);
    let b = decode_road_vertex(b);
    let (ax, ay) = unpack_typed_position(a.pos);
    let (bx, by) = unpack_typed_position(b.pos);
    road_vertex_bytes(RoadVertexTyped {
        pos: pack_position(m(ax, bx), m(ay, by)),
        off: pair(a.off, b.off),
        color: color(a.color, b.color),
        params: pair(a.params, b.params),
        deck: m(a.deck, b.deck),
        depth: pair(a.depth, b.depth),
        uv: pair(a.uv, b.uv),
    })
}

/// The road midpoint, projected: a refined face record is exactly what the
/// same refinement of its 28-byte form would have been. The implicit fields
/// midpoint to themselves, so the projection cannot fail.
fn midpoint_face_record(a: &[u8], b: &[u8]) -> [u8; FACE_TYPED_VERTEX_BYTES] {
    let a = road_vertex_bytes(road_record_from_face(decode_face_vertex(a)));
    let b = road_vertex_bytes(road_record_from_face(decode_face_vertex(b)));
    let midpoint = decode_road_vertex(&midpoint_road_record(&a, &b));
    face_vertex_bytes(
        face_record_from_road(midpoint).expect("face midpoint keeps its implicit fields"),
    )
}

/// Crack-free midpoint refinement of an already-PACKED tile mesh: every
/// edge longer than `max_edge` (tile-local units) splits until the fixpoint
/// — shared midpoints via the edge map so neighboring triangles agree, the
/// same canonical-rotation scheme as `subdivide_face_mesh`. Used by the
/// space-warp mode, whose curved fold any long flat chord would slice
/// through; the triangulator itself is untouched — this runs on its output.
pub fn subdivide_packed_mesh(indices: &mut Vec<u32>, vertices: &mut Vec<f32>, max_edge: f32) {
    let mut budget = SubdivisionBudget::unlimited();
    subdivide_packed_mesh_with::<VECTOR_PACKED_FLOATS_PER_VERTEX>(
        indices,
        vertices,
        max_edge,
        &mut budget,
        midpoint_packed_record,
    );
}

/// A shared, consumable subdivision allowance. Passes are charged using a
/// conservative worst case before any pass-sized allocation is attempted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubdivisionBudget {
    remaining_bytes: usize,
    remaining_work: usize,
    unlimited: bool,
}

impl SubdivisionBudget {
    pub fn new(max_bytes: usize, max_work: usize) -> Self {
        Self {
            remaining_bytes: max_bytes,
            remaining_work: max_work,
            unlimited: false,
        }
    }

    pub fn unlimited() -> Self {
        Self {
            remaining_bytes: usize::MAX,
            remaining_work: usize::MAX,
            unlimited: true,
        }
    }

    pub fn remaining_bytes(&self) -> usize {
        self.remaining_bytes
    }

    pub fn remaining_work(&self) -> usize {
        self.remaining_work
    }

    fn can_charge(&self, bytes: usize, work: usize) -> bool {
        self.unlimited || (bytes <= self.remaining_bytes && work <= self.remaining_work)
    }

    fn charge(&mut self, bytes: usize, work: usize) {
        if !self.unlimited {
            self.remaining_bytes -= bytes;
            self.remaining_work -= work;
        }
    }
}

const SUBDIVISION_HASH_BYTES_PER_EDGE: usize = 32;

struct SubdivisionPassEstimate {
    max_indices: usize,
    max_midpoints: usize,
    bytes: usize,
    work: usize,
}

/// Count-only worst case: four output triangles and three distinct midpoint
/// records per input triangle. `bytes` models both sides of a realloc plus
/// old/new index storage and deliberately generous midpoint-map overhead.
fn subdivision_pass_estimate(
    index_count: usize,
    vertex_count: usize,
    vertex_stride_bytes: usize,
) -> Option<SubdivisionPassEstimate> {
    let triangles = index_count / 3;
    let max_indices = triangles.checked_mul(12)?;
    let max_midpoints = triangles.checked_mul(3)?;
    let max_vertices = vertex_count.checked_add(max_midpoints)?;
    if max_vertices > u32::MAX as usize {
        return None;
    }
    let old_indices = index_count.checked_mul(std::mem::size_of::<u32>())?;
    let new_indices = max_indices.checked_mul(std::mem::size_of::<u32>())?;
    let old_vertices = vertex_count.checked_mul(vertex_stride_bytes)?;
    let new_vertices = max_vertices.checked_mul(vertex_stride_bytes)?;
    let midpoint_hash = max_midpoints.checked_mul(SUBDIVISION_HASH_BYTES_PER_EDGE)?;
    let bytes = old_indices
        .checked_add(new_indices)?
        .checked_add(old_vertices)?
        .checked_add(new_vertices)?
        .checked_add(midpoint_hash)?;
    let work = index_count
        .checked_add(max_indices)?
        .checked_add(max_midpoints)?;
    Some(SubdivisionPassEstimate {
        max_indices,
        max_midpoints,
        bytes,
        work,
    })
}

pub fn subdivide_packed_mesh_budgeted(
    indices: &mut Vec<u32>,
    vertices: &mut Vec<f32>,
    max_edge: f32,
    budget: &mut SubdivisionBudget,
) {
    subdivide_packed_mesh_with::<VECTOR_PACKED_FLOATS_PER_VERTEX>(
        indices,
        vertices,
        max_edge,
        budget,
        midpoint_packed_record,
    );
}

/// Fill-layout twin used after builder-thread packing. Space-warp refinement
/// therefore stays on the main thread without restoring 19-float records.
pub fn subdivide_fill_packed_mesh(
    indices: &mut Vec<u32>,
    vertices: &mut Vec<u8>,
    max_edge: f32,
) {
    let mut budget = SubdivisionBudget::unlimited();
    subdivide_typed_mesh_with::<FILL_TYPED_VERTEX_BYTES>(
        indices,
        vertices,
        max_edge,
        &mut budget,
        |record| unpack_typed_position(decode_fill_vertex(record).pos),
        midpoint_fill_typed_record,
    );
}

pub fn subdivide_fill_packed_mesh_budgeted(
    indices: &mut Vec<u32>,
    vertices: &mut Vec<u8>,
    max_edge: f32,
    budget: &mut SubdivisionBudget,
) {
    subdivide_typed_mesh_with::<FILL_TYPED_VERTEX_BYTES>(
        indices,
        vertices,
        max_edge,
        budget,
        |record| unpack_typed_position(decode_fill_vertex(record).pos),
        midpoint_fill_typed_record,
    );
}

fn subdivide_packed_mesh_with<const S: usize>(
    indices: &mut Vec<u32>,
    vertices: &mut Vec<f32>,
    max_edge: f32,
    budget: &mut SubdivisionBudget,
    midpoint_record: impl Fn(&[f32], &[f32]) -> [f32; S] + Copy,
) {
    use std::collections::HashMap;
    if S < 2
        || indices.is_empty()
        || indices.len() % 3 != 0
        || vertices.len() < S
        || vertices.len() % S != 0
        || !max_edge.is_finite()
        || max_edge <= 0.0
    {
        return;
    }
    let vertex_count = vertices.len() / S;
    if !indices.iter().all(|&index| (index as usize) < vertex_count) {
        return;
    }
    let max_edge_sq = max_edge * max_edge;
    for _pass in 0..12 {
        let Some(estimate) = subdivision_pass_estimate(
            indices.len(),
            vertices.len() / S,
            S.checked_mul(std::mem::size_of::<f32>()).unwrap_or(usize::MAX),
        ) else {
            break;
        };
        if !budget.can_charge(estimate.bytes, estimate.work) {
            break;
        }
        let mut out: Vec<u32> = if budget.unlimited {
            Vec::with_capacity(indices.len())
        } else {
            let mut out = Vec::new();
            if out.try_reserve_exact(estimate.max_indices).is_err() {
                break;
            }
            out
        };
        let mut midpoints: HashMap<(u32, u32), u32> = HashMap::new();
        if !budget.unlimited
            && (midpoints.try_reserve(estimate.max_midpoints).is_err()
                || vertices
                    .try_reserve_exact(estimate.max_midpoints.saturating_mul(S))
                    .is_err())
        {
            break;
        }
        budget.charge(estimate.bytes, estimate.work);
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
                let record = midpoint_record(&ra, &rb);
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

fn subdivide_typed_mesh_with<const S: usize>(
    indices: &mut Vec<u32>,
    vertices: &mut Vec<u8>,
    max_edge: f32,
    budget: &mut SubdivisionBudget,
    position: impl Fn(&[u8]) -> (f32, f32) + Copy,
    midpoint_record: impl Fn(&[u8], &[u8]) -> [u8; S] + Copy,
) {
    use std::collections::HashMap;
    if S < 1
        || indices.is_empty()
        || indices.len() % 3 != 0
        || vertices.len() < S
        || vertices.len() % S != 0
        || !max_edge.is_finite()
        || max_edge <= 0.0
    {
        return;
    }
    let vertex_count = vertices.len() / S;
    if !indices.iter().all(|&index| (index as usize) < vertex_count) {
        return;
    }
    let max_edge_sq = max_edge * max_edge;
    for _pass in 0..12 {
        let Some(estimate) =
            subdivision_pass_estimate(indices.len(), vertices.len() / S, S)
        else {
            break;
        };
        if !budget.can_charge(estimate.bytes, estimate.work) {
            break;
        }
        let mut out: Vec<u32> = if budget.unlimited {
            Vec::with_capacity(indices.len())
        } else {
            let mut out = Vec::new();
            if out.try_reserve_exact(estimate.max_indices).is_err() {
                break;
            }
            out
        };
        let mut midpoints: HashMap<(u32, u32), u32> = HashMap::new();
        if !budget.unlimited
            && (midpoints.try_reserve(estimate.max_midpoints).is_err()
                || vertices
                    .try_reserve_exact(estimate.max_midpoints.saturating_mul(S))
                    .is_err())
        {
            break;
        }
        budget.charge(estimate.bytes, estimate.work);
        let mut split_any = false;
        let need_split = |vertices: &[u8], i: u32, j: u32| -> bool {
            let (vi, vj) = (i as usize * S, j as usize * S);
            let a = position(&vertices[vi..vi + S]);
            let b = position(&vertices[vj..vj + S]);
            (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) > max_edge_sq
        };
        for triangle in indices.chunks_exact(3) {
            let (mut a, mut b, mut c) = (triangle[0], triangle[1], triangle[2]);
            let (mut sab, mut sbc, mut sca) = (
                need_split(vertices, a, b),
                need_split(vertices, b, c),
                need_split(vertices, c, a),
            );
            for _ in 0..2 {
                let rotate = match (sab, sbc, sca) {
                    (false, true, _) | (false, false, true) | (true, false, true) => true,
                    _ => false,
                };
                if !rotate {
                    break;
                }
                (a, b, c) = (b, c, a);
                (sab, sbc, sca) = (sbc, sca, sab);
            }
            let mut mid = |i: u32, j: u32, vertices: &mut Vec<u8>| -> u32 {
                let key = (i.min(j), i.max(j));
                if let Some(&midpoint) = midpoints.get(&key) {
                    return midpoint;
                }
                let (vi, vj) = (i as usize * S, j as usize * S);
                let record = midpoint_record(&vertices[vi..vi + S], &vertices[vj..vj + S]);
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

/// Space-warp refinement for the typed road layout. Subdivision happens
/// after packing; every record is decoded, interpolated and encoded again.
pub fn subdivide_road_mesh(indices: &mut Vec<u32>, vertices: &mut Vec<u8>, max_edge: f32) {
    let mut budget = SubdivisionBudget::unlimited();
    subdivide_typed_mesh_with::<ROAD_TYPED_VERTEX_BYTES>(
        indices,
        vertices,
        max_edge,
        &mut budget,
        |record| unpack_typed_position(decode_road_vertex(record).pos),
        midpoint_road_record,
    );
}

pub fn subdivide_road_mesh_budgeted(
    indices: &mut Vec<u32>,
    vertices: &mut Vec<u8>,
    max_edge: f32,
    budget: &mut SubdivisionBudget,
) {
    subdivide_typed_mesh_with::<ROAD_TYPED_VERTEX_BYTES>(
        indices,
        vertices,
        max_edge,
        budget,
        |record| unpack_typed_position(decode_road_vertex(record).pos),
        midpoint_road_record,
    );
}

/// Space-warp refinement for the typed road-union face layout; the same
/// split rule and midpoint arithmetic as `subdivide_road_mesh`.
pub fn subdivide_face_typed_mesh(indices: &mut Vec<u32>, vertices: &mut Vec<u8>, max_edge: f32) {
    let mut budget = SubdivisionBudget::unlimited();
    subdivide_typed_mesh_with::<FACE_TYPED_VERTEX_BYTES>(
        indices,
        vertices,
        max_edge,
        &mut budget,
        |record| unpack_typed_position(decode_face_vertex(record).pos),
        midpoint_face_record,
    );
}


pub fn subdivide_face_typed_mesh_budgeted(
    indices: &mut Vec<u32>,
    vertices: &mut Vec<u8>,
    max_edge: f32,
    budget: &mut SubdivisionBudget,
) {
    subdivide_typed_mesh_with::<FACE_TYPED_VERTEX_BYTES>(
        indices,
        vertices,
        max_edge,
        budget,
        |record| unpack_typed_position(decode_face_vertex(record).pos),
        midpoint_face_record,
    );
}

#[cfg(test)]
mod road_pack_tests {
    use super::*;

    #[test]
    fn typed_map_anchor_precision_covers_clip_and_z18_overzoom() {
        for value in [-3.0, -2.987, 0.0, 128.123, 256.25, 259.0] {
            let packed = pack_position(value, value);
            let (x, y) = unpack_typed_position(packed);
            assert!((x - value).abs() <= 1.0 / 128.0 + f32::EPSILON);
            assert!((y - value).abs() <= 1.0 / 128.0 + f32::EPSILON);
            assert!((x - value).abs() * 16.0 <= 0.125 + f32::EPSILON);
        }
        assert_eq!(pack_position(-3.0, 259.0), I16x2::from_i16(-192, 16_576));
    }

    #[test]
    fn road_pack_round_trips_logical_record() {
        let mut record = [0.0f32; VECTOR_FLOATS_PER_VERTEX];
        record[0] = 12.251;
        record[1] = -2.997;
        record[2] = 0.375;
        record[3] = 0.625;
        record[4..8].copy_from_slice(&[0.2, 0.4, 0.6, 0.8]);
        record[9] = 123.5;
        record[10] = 111.0;
        record[12] = -2.125;
        record[13] = 3.75;
        record[14] = 2.0;
        record[15] = 100.25;
        record[16] = 0.3456;
        record[18] = 137.0 * VECTOR_ZBIAS_STEP;

        let packed = pack_road_vertices(&record);
        assert_eq!(packed.len(), ROAD_TYPED_VERTEX_BYTES);
        let packed = decode_road_vertex(&packed);
        let pos = unpack_typed_position(packed.pos);
        assert!((pos.0 - record[0]).abs() <= 1.0 / 128.0 + f32::EPSILON);
        assert!((pos.1 - record[1]).abs() <= 1.0 / 128.0 + f32::EPSILON);
        assert_eq!(packed.deck, record[15]);
        let (ox, oy) = packed.off.to_f32();
        assert!((ox - record[12]).abs() < 0.002);
        assert!((oy - record[13]).abs() < 0.002);
        let rgba = packed.color.to_f32();
        for (actual, expected) in [rgba.0, rgba.1, rgba.2, rgba.3]
            .into_iter()
            .zip(record[4..8].iter().copied())
        {
            assert!((actual - expected).abs() <= 0.5 / 255.0 + f32::EPSILON);
        }
        let (meta, stroke_dist) = packed.params.to_f32();
        assert_eq!(meta, 2.0 + ROAD_PARAM_DASH_SCALE * 2.0 + ROAD_PARAM_EXPANDED_FLAG);
        assert_eq!(stroke_dist, record[9]);
        let (param5, zbias_ticks) = packed.depth.to_f32();
        assert!((param5 - record[16]).abs() < 0.0002);
        assert_eq!(zbias_ticks, 137.0);
        let (u, v) = packed.uv.to_f32();
        assert!((u - record[2]).abs() < 0.001);
        assert!((v - record[3]).abs() < 0.001);
    }

    #[test]
    fn road_zbias_tick_range_matches_exact_f16_integer_range() {
        let (_, max_ticks) = unpack_pair_f16(pack_pair_f16(0.0, ROAD_ZBIAS_MAX_EXACT_TICKS));
        let (_, first_inexact) =
            unpack_pair_f16(pack_pair_f16(0.0, ROAD_ZBIAS_MAX_EXACT_TICKS + 1.0));
        assert_eq!(max_ticks, ROAD_ZBIAS_MAX_EXACT_TICKS);
        assert_ne!(first_inexact, ROAD_ZBIAS_MAX_EXACT_TICKS + 1.0);
    }

    #[test]
    fn road_pack_converts_fringe_u_to_coverage() {
        let mut record = [0.0f32; VECTOR_FLOATS_PER_VERTEX];
        record[2] = -1.0;
        record[4..8].copy_from_slice(&[1.0, 0.5, 0.25, 1.0]);
        record[8] = VECTOR_ANALYTIC_FRINGE_STROKE_MULT;
        record[10] = 0.0;
        record[14] = 3.0;
        let packed = pack_road_record(&record);
        let (meta, coverage) = packed.params.to_f32();
        assert_eq!(meta, 8.0 * 3.0 + ROAD_PARAM_KIND_SCALE * ROAD_KIND_FRINGE);
        assert_eq!(coverage, 0.0);
        let (u, v) = packed.uv.to_f32();
        assert_eq!((u, v), (-1.0, 0.0));
        let (ox, oy) = packed.off.to_f32();
        assert_eq!((ox, oy), (0.0, 0.0));
    }

    #[test]
    fn road_pack_union_face_is_class_zero_fill() {
        let mut record = [0.0f32; VECTOR_FLOATS_PER_VERTEX];
        record[0] = 4.0;
        record[1] = 8.0;
        record[2] = 0.5;
        record[4..8].copy_from_slice(&[0.9, 0.8, 0.1, 1.0]);
        record[8] = 1e6;
        record[10] = 0.0;
        record[12] = 9.0;
        record[13] = -3.0;
        record[14] = 0.0;
        let packed = pack_road_record(&record);
        let (ox, oy) = packed.off.to_f32();
        assert_eq!((ox, oy), (0.0, 0.0));
        let (meta, coverage) = packed.params.to_f32();
        assert_eq!(meta, ROAD_PARAM_KIND_SCALE * ROAD_KIND_FILL);
        assert!((coverage - 0.5).abs() < 0.001);
        assert!((packed.color.to_f32().0 - 0.9).abs() <= 0.5 / 255.0 + f32::EPSILON);
    }

    /// A grounded union face as the Boolean emits it: fill sentinel, the
    /// tessellator's (0.5, 1) uv, shape 0, no deck.
    fn union_face_record(material: f32, emissive: f32) -> [f32; VECTOR_FLOATS_PER_VERTEX] {
        let mut record = [0.0f32; VECTOR_FLOATS_PER_VERTEX];
        record[0] = 131.015625;
        record[1] = -2.5;
        record[2] = 0.5;
        record[3] = 1.0;
        record[4..8].copy_from_slice(&[0.9, 0.8, 0.1, 1.0]);
        record[8] = 1e6;
        record[12] = emissive;
        record[14] = material;
        record[16] = 0.14196777;
        record[18] = 3195.0 * VECTOR_ZBIAS_STEP;
        record
    }

    #[test]
    fn face_pack_is_the_road_pack_minus_its_implicit_fields() {
        assert_eq!(FACE_TYPED_VERTEX_BYTES, 16);
        for (material, emissive) in [(0.0, 0.0), (7.0, 0.75)] {
            let record = union_face_record(material, emissive);
            let road = pack_road_record(&record);
            let face = pack_face_record(&record).expect("union face projects");
            assert_eq!(face.pos, road.pos);
            assert_eq!(face.color, road.color);
            assert_eq!(face.params, road.params);
            assert_eq!(face.depth, road.depth);
            assert_eq!(road_record_from_face(face), road);
            let bytes = pack_face_vertices(&record);
            assert_eq!(bytes.len(), FACE_TYPED_VERTEX_BYTES);
            assert_eq!(decode_face_vertex(&bytes), face);
            // Ticks beyond f16's exact integer range round exactly as the
            // road layout rounds them: the face stream never re-quantizes.
            assert_eq!(face.depth.to_f32().1, 3196.0);
            let (meta, aux) = face.params.to_f32();
            assert_eq!(meta, 8.0 * material + ROAD_PARAM_KIND_SCALE * ROAD_KIND_FILL);
            assert_eq!(aux, if material > 6.5 { emissive } else { 0.5 });
        }
    }

    #[test]
    fn face_pack_keeps_every_record_the_face_shader_cannot_substitute() {
        let mut lifted = union_face_record(0.0, 0.0);
        lifted[15] = 2.5;
        let mut fascia = union_face_record(0.0, 0.0);
        fascia[3] = 0.25;
        fascia[15] = 1.4;
        let mut fringe = union_face_record(0.0, 0.0);
        fringe[2] = -0.5;
        fringe[8] = VECTOR_ANALYTIC_FRINGE_STROKE_MULT;
        let mut stroke = union_face_record(0.0, 0.0);
        stroke[8] = 1.0;
        stroke[10] = 100.0;
        stroke[12] = 1.5;
        let mut morph_face = union_face_record(0.0, 0.0);
        morph_face[10] = 100.0;
        morph_face[14] = 4.0;
        for record in [lifted, fascia, fringe, stroke, morph_face] {
            assert!(!is_compact_face_record(&record));
            assert!(face_record_from_road(pack_road_record(&record)).is_none());
        }
        assert!(!is_compact_face_record(&[0.0; 3]));
        assert!(is_compact_face_record(&union_face_record(0.0, 0.0)));
    }

    #[test]
    fn face_midpoint_is_the_projected_road_midpoint() {
        let mut far = union_face_record(0.0, 0.0);
        far[0] = 200.0;
        far[1] = 40.0;
        far[4..8].copy_from_slice(&[0.1, 0.2, 0.3, 1.0]);
        far[16] = 0.3022461;
        far[18] = 12.0 * VECTOR_ZBIAS_STEP;
        let near = union_face_record(0.0, 0.0);
        let road_mid = midpoint_road_record(
            &pack_road_vertices(&near),
            &pack_road_vertices(&far),
        );
        let face_mid = midpoint_face_record(
            &pack_face_vertices(&near),
            &pack_face_vertices(&far),
        );
        assert_eq!(
            road_record_from_face(decode_face_vertex(&face_mid)),
            decode_road_vertex(&road_mid)
        );

        let mut indices = vec![0, 1, 2];
        let mut road = pack_road_vertices(&near);
        road.extend(pack_road_vertices(&far));
        road.extend(pack_road_vertices(&{
            let mut third = union_face_record(0.0, 0.0);
            third[0] = 200.0;
            third
        }));
        let mut face_indices = indices.clone();
        let mut face: Vec<u8> = road
            .chunks_exact(ROAD_TYPED_VERTEX_BYTES)
            .flat_map(|record| {
                face_vertex_bytes(face_record_from_road(decode_road_vertex(record)).unwrap())
            })
            .collect();
        subdivide_road_mesh(&mut indices, &mut road, 24.0);
        subdivide_face_typed_mesh(&mut face_indices, &mut face, 24.0);
        assert_eq!(face_indices, indices);
        let projected: Vec<u8> = face
            .chunks_exact(FACE_TYPED_VERTEX_BYTES)
            .flat_map(|record| road_vertex_bytes(road_record_from_face(decode_face_vertex(record))))
            .collect();
        assert_eq!(projected, road);
    }

    #[test]
    fn road_pack_expanded_fill_keeps_offset_and_fill_kind() {
        let mut record = [0.0f32; VECTOR_FLOATS_PER_VERTEX];
        record[8] = 1e6;
        record[10] = 100.0;
        record[12] = 1.5;
        record[13] = -0.75;
        record[14] = 4.0;
        let packed = pack_road_record(&record);
        let (ox, oy) = packed.off.to_f32();
        assert!((ox - 1.5).abs() < 0.002);
        assert!((oy + 0.75).abs() < 0.002);
        let (meta, _) = packed.params.to_f32();
        assert_eq!(meta, 4.0 + ROAD_PARAM_KIND_SCALE * ROAD_KIND_FILL + ROAD_PARAM_EXPANDED_FLAG);
    }

    #[test]
    fn typed_road_subdivision_decodes_interpolates_and_reencodes() {
        let mut logical = Vec::new();
        for (x, y, color) in [(0.0, 0.0, 0.0), (16.0, 0.0, 1.0), (0.0, 16.0, 0.5)] {
            let mut record = [0.0; VECTOR_FLOATS_PER_VERTEX];
            record[0] = x;
            record[1] = y;
            record[2] = 0.5;
            record[4..8].copy_from_slice(&[color, color, color, 1.0]);
            record[8] = 1e6;
            logical.extend_from_slice(&record);
        }
        let mut vertices = pack_road_vertices(&logical);
        let mut indices = vec![0, 1, 2];
        subdivide_road_mesh(&mut indices, &mut vertices, 8.0);
        assert!(vertices.len() > 3 * ROAD_TYPED_VERTEX_BYTES);
        assert!(indices.len() > 3);
        for vertex in vertices.chunks_exact(ROAD_TYPED_VERTEX_BYTES) {
            let pos = unpack_typed_position(decode_road_vertex(vertex).pos);
            assert_eq!(pos.0 * MAP_VERTEX_POSITION_SCALE, (pos.0 * MAP_VERTEX_POSITION_SCALE).round());
            assert_eq!(pos.1 * MAP_VERTEX_POSITION_SCALE, (pos.1 * MAP_VERTEX_POSITION_SCALE).round());
        }
    }

    #[test]
    fn typed_fill_subdivision_decodes_interpolates_and_reencodes() {
        let mut logical = Vec::new();
        for (x, y, color) in [(0.0, 0.0, 0.0), (16.0, 0.0, 1.0), (0.0, 16.0, 0.5)] {
            let mut record = [0.0; VECTOR_FLOATS_PER_VERTEX];
            record[0] = x;
            record[1] = y;
            record[2] = color;
            record[4..8].copy_from_slice(&[color, color, color, 1.0]);
            record[8] = 1e6;
            logical.extend_from_slice(&record);
        }
        let mut vertices = pack_fill_vertices(&logical);
        let mut indices = vec![0, 1, 2];
        subdivide_fill_packed_mesh(&mut indices, &mut vertices, 8.0);
        assert!(vertices.len() > 3 * FILL_TYPED_VERTEX_BYTES);
        assert!(indices.len() > 3);

        let midpoint = vertices
            .chunks_exact(FILL_TYPED_VERTEX_BYTES)
            .map(decode_fill_vertex)
            .find(|vertex| unpack_typed_position(vertex.pos) == (8.0, 0.0))
            .expect("long edge midpoint");
        assert!((midpoint.color.to_f32().0 - 0.5).abs() <= 0.5 / 255.0 + f32::EPSILON);
        assert!((midpoint.params.to_f32().1 - 0.5).abs() < 0.001);
    }

    fn packed_test_mesh(points: &[(f32, f32)]) -> Vec<f32> {
        points
            .iter()
            .flat_map(|&(x, y)| {
                let mut record = [0.0; VECTOR_PACKED_FLOATS_PER_VERTEX];
                record[0] = x;
                record[1] = y;
                record
            })
            .collect()
    }

    #[test]
    fn tiny_budget_refuses_huge_packed_refinement_without_mutation() {
        let mut indices = vec![0, 1, 2];
        let mut vertices = packed_test_mesh(&[(0.0, 0.0), (1.0e30, 0.0), (0.0, 1.0e30)]);
        let original_indices = indices.clone();
        let original_vertices = vertices.clone();
        let mut budget = SubdivisionBudget::new(1, 1);
        subdivide_packed_mesh_budgeted(
            &mut indices,
            &mut vertices,
            f32::MIN_POSITIVE,
            &mut budget,
        );
        assert_eq!(indices, original_indices);
        assert_eq!(vertices, original_vertices);
        assert!(indices
            .iter()
            .all(|&index| (index as usize) < vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX));
    }

    #[test]
    fn budgeted_normal_meshes_equal_unlimited_packed_and_typed_output() {
        let source_indices = vec![0, 1, 2];
        let source_packed = packed_test_mesh(&[(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)]);
        let (mut expected_indices, mut expected_packed) =
            (source_indices.clone(), source_packed.clone());
        subdivide_packed_mesh(&mut expected_indices, &mut expected_packed, 8.0);
        let (mut actual_indices, mut actual_packed) = (source_indices.clone(), source_packed);
        let mut budget = SubdivisionBudget::new(16 * 1024 * 1024, 16 * 1024 * 1024);
        subdivide_packed_mesh_budgeted(
            &mut actual_indices,
            &mut actual_packed,
            8.0,
            &mut budget,
        );
        assert_eq!((actual_indices, actual_packed), (expected_indices, expected_packed));

        let mut logical = Vec::new();
        for (x, y) in [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)] {
            let mut record = [0.0; VECTOR_FLOATS_PER_VERTEX];
            record[0] = x;
            record[1] = y;
            record[8] = 1e6;
            logical.extend_from_slice(&record);
        }
        let source_typed = pack_fill_vertices(&logical);
        let (mut expected_indices, mut expected_typed) =
            (source_indices.clone(), source_typed.clone());
        subdivide_fill_packed_mesh(&mut expected_indices, &mut expected_typed, 8.0);
        let (mut actual_indices, mut actual_typed) = (source_indices, source_typed);
        let mut budget = SubdivisionBudget::new(16 * 1024 * 1024, 16 * 1024 * 1024);
        subdivide_fill_packed_mesh_budgeted(
            &mut actual_indices,
            &mut actual_typed,
            8.0,
            &mut budget,
        );
        assert_eq!((actual_indices, actual_typed), (expected_indices, expected_typed));
    }

    #[test]
    fn a_budget_boundary_keeps_shared_edges_on_one_whole_pass() {
        let mut indices = vec![0, 1, 2, 0, 2, 3];
        let mut vertices =
            packed_test_mesh(&[(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)]);
        let estimate = subdivision_pass_estimate(
            indices.len(),
            vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX,
            VECTOR_PACKED_FLOATS_PER_VERTEX * std::mem::size_of::<f32>(),
        )
        .unwrap();
        let mut budget = SubdivisionBudget::new(estimate.bytes, estimate.work);
        subdivide_packed_mesh_budgeted(&mut indices, &mut vertices, 0.1, &mut budget);

        assert_eq!(indices.len(), 24);
        assert_eq!(vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX, 9);
        assert_eq!(
            vertices
                .chunks_exact(VECTOR_PACKED_FLOATS_PER_VERTEX)
                .filter(|record| record[0] == 1.0 && record[1] == 1.0)
                .count(),
            1
        );
        assert!(indices
            .iter()
            .all(|&index| (index as usize) < vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX));
        assert_eq!(budget.remaining_bytes(), 0);
        assert_eq!(budget.remaining_work(), 0);
    }

    #[test]
    fn invalid_thresholds_and_malformed_indices_are_noops() {
        let source_vertices = packed_test_mesh(&[(0.0, 0.0), (2.0, 0.0), (0.0, 2.0)]);
        for threshold in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -1.0] {
            let mut indices = vec![0, 1, 2];
            let mut vertices = source_vertices.clone();
            subdivide_packed_mesh(&mut indices, &mut vertices, threshold);
            assert_eq!(indices, [0, 1, 2]);
            assert_eq!(vertices, source_vertices);
        }

        let mut indices = vec![0, 1, 9];
        let mut vertices = source_vertices.clone();
        subdivide_packed_mesh(&mut indices, &mut vertices, 0.5);
        assert_eq!(indices, [0, 1, 9]);
        assert_eq!(vertices, source_vertices);

        let mut logical = vec![0.0; VECTOR_FLOATS_PER_VERTEX * 3];
        for record in logical.chunks_exact_mut(VECTOR_FLOATS_PER_VERTEX) {
            record[8] = 1e6;
        }
        let source_typed = pack_fill_vertices(&logical);
        let mut typed_indices = vec![0, 1, u32::MAX];
        let mut typed_vertices = source_typed.clone();
        subdivide_fill_packed_mesh(&mut typed_indices, &mut typed_vertices, 0.5);
        assert_eq!(typed_indices, [0, 1, u32::MAX]);
        assert_eq!(typed_vertices, source_typed);
    }

    #[test]
    fn typed_roof_byte_stream_keeps_exact_height() {
        let mut record = [0.0; VECTOR_FLOATS_PER_VERTEX];
        record[0] = 91.125;
        record[1] = 37.875;
        record[2] = 0.5;
        record[3] = 1.0;
        record[4..8].copy_from_slice(&[0.25, 0.5, 0.75, 1.0]);
        record[8] = 1e6;
        record[14] = crate::scene_sun::MAT_ROOF;
        record[15] = 123.456;
        record[16] = 0.8;
        record[18] = 77.0 * VECTOR_ZBIAS_STEP;

        let bytes = pack_roof_vertices(&record);
        assert_eq!(bytes.len(), ROOF_TYPED_VERTEX_BYTES);
        let roof = decode_roof_vertex(&bytes);
        assert_eq!(unpack_typed_position(roof.pos), (record[0], record[1]));
        assert_eq!(roof.height, record[15]);
        assert_eq!(roof.params.to_f32(), (crate::scene_sun::MAT_ROOF, 77.0));
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
