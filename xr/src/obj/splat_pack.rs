//! GPU-side splat record: 16 bytes per splat, decoded in the vertex stage.
//!
//! Layout (four little-endian u32 words; each word is one BGRAu8 texel of
//! the splat data texture, so a splat is 4 adjacent texels and the shader
//! rebuilds the words from the sampled unorm8 channels — no float-bitcast
//! of packed data, which would be at the mercy of denormal flushing):
//!
//! ```text
//! word0  b8 | g8<<8 | r8<<16 | a8<<24            rgba as a BGRAu8 texel (sampled .xyzw = rgba)
//! word1  px14 | py14<<14 | pz_lo4<<28            chunk-relative positions, 14 bit
//! word2  pz_hi10 | sx8<<10 | sy8<<18 | sz_lo6<<26   log-encoded axis lengths, 8 bit
//! word3  sz_hi2 | q0_9<<2 | q1_9<<11 | q2_9<<20 | qi2<<29   smallest-three quaternion, 9 bit
//! ```
//!
//! Positions are 14-bit fixed point inside the bounds of the splat's CHUNK
//! (up to 256 consecutive records; records are laid out along a Morton
//! curve and a chunk never spans two level-`CHUNK_SPLIT_LEVEL` Morton cells,
//! so its extent is at most scene/8 and usually far smaller), which keeps
//! the quantization step at sub-pixel resolution for scene-sized models
//! (see `tests`). Chunk bounds live in a small RGBA32F side texture (two
//! texels per chunk: min xyz, extent xyz). Chunks closed early are padded
//! with invisible records (`radius_bound < 0`), which the sorter culls.

use std::cmp::Ordering;

pub const CHUNK_SPLATS: usize = 256;
/// Splat records per texture row; the data texture is 4x this wide.
pub const RECORDS_PER_ROW: usize = 2048;
/// Chunks per row of the chunk-bounds texture (two texels each).
pub const CHUNKS_PER_ROW: usize = 1024;
/// A chunk never crosses a Morton cell of this octree level (3 = 8x8x8
/// cells over the scene): bounds the chunk extent to scene/8 per axis.
pub const CHUNK_SPLIT_LEVEL: u32 = 3;
const POS_MAX: f32 = 16383.0;
const QUAT_MAX: f32 = 511.0;

/// One splat as the renderer wants it: local-space center, the three axis
/// lengths (after normalize/min_radius/radius_scale), unit quaternion
/// (xyzw) and rgba in [0,1].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplatRecord {
    pub center: [f32; 3],
    pub scales: [f32; 3],
    pub rotation: [f32; 4],
    pub color: [f32; 4],
}

/// Per-scene log-scale range the 8-bit axis lengths are spread over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleRange {
    pub ln_min: f32,
    pub ln_range: f32,
}

impl ScaleRange {
    pub fn from_records(records: &[SplatRecord]) -> Self {
        let mut ln_min = f32::INFINITY;
        let mut ln_max = f32::NEG_INFINITY;
        for record in records {
            for &s in &record.scales {
                let ln = s.max(1e-9).ln();
                ln_min = ln_min.min(ln);
                ln_max = ln_max.max(ln);
            }
        }
        if !ln_min.is_finite() || !ln_max.is_finite() {
            return Self { ln_min: -7.0, ln_range: 1.0 };
        }
        Self {
            ln_min,
            ln_range: (ln_max - ln_min).max(1e-6),
        }
    }

    #[inline]
    pub fn encode(&self, scale: f32) -> u32 {
        let t = (scale.max(1e-9).ln() - self.ln_min) / self.ln_range;
        (t.clamp(0.0, 1.0) * 255.0 + 0.5) as u32
    }

    #[inline]
    pub fn decode(&self, code: u32) -> f32 {
        (self.ln_min + code as f32 / 255.0 * self.ln_range).exp()
    }
}

/// Bounds of one 256-record chunk: positions are stored relative to `min`
/// in units of `extent / 32767`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChunkBounds {
    pub min: [f32; 3],
    pub extent: [f32; 3],
}

impl ChunkBounds {
    pub fn from_records(records: &[SplatRecord]) -> Self {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for record in records {
            for axis in 0..3 {
                min[axis] = min[axis].min(record.center[axis]);
                max[axis] = max[axis].max(record.center[axis]);
            }
        }
        let mut extent = [0.0f32; 3];
        for axis in 0..3 {
            if !min[axis].is_finite() {
                min[axis] = 0.0;
                max[axis] = 0.0;
            }
            // A flat chunk still needs a nonzero step for the decode.
            extent[axis] = (max[axis] - min[axis]).max(1e-7);
        }
        Self { min, extent }
    }

    #[inline]
    fn encode_axis(&self, axis: usize, value: f32) -> u32 {
        let t = (value - self.min[axis]) / self.extent[axis];
        (t.clamp(0.0, 1.0) * POS_MAX + 0.5) as u32
    }

    #[inline]
    pub fn decode_axis(&self, axis: usize, code: u32) -> f32 {
        self.min[axis] + code as f32 / POS_MAX * self.extent[axis]
    }
}

/// Smallest-three quaternion: drop the largest-magnitude component (made
/// positive by sign flip), store the other three as 9 bits over
/// [-1/sqrt2, 1/sqrt2].
#[inline]
pub fn encode_quaternion(q: [f32; 4]) -> (u32, u32, u32, u32) {
    let mut largest = 0usize;
    for i in 1..4 {
        if q[i].abs() > q[largest].abs() {
            largest = i;
        }
    }
    let sign = if q[largest] < 0.0 { -1.0 } else { 1.0 };
    let mut small = [0u32; 3];
    let mut k = 0;
    for i in 0..4 {
        if i == largest {
            continue;
        }
        let c = (q[i] * sign) * std::f32::consts::FRAC_1_SQRT_2; // [-0.5, 0.5]
        small[k] = ((c + 0.5).clamp(0.0, 1.0) * QUAT_MAX + 0.5) as u32;
        k += 1;
    }
    (small[0], small[1], small[2], largest as u32)
}

#[inline]
pub fn decode_quaternion(c0: u32, c1: u32, c2: u32, largest: u32) -> [f32; 4] {
    let dec = |c: u32| (c as f32 / QUAT_MAX - 0.5) * std::f32::consts::SQRT_2;
    let a = dec(c0);
    let b = dec(c1);
    let c = dec(c2);
    let big = (1.0 - a * a - b * b - c * c).max(0.0).sqrt();
    match largest {
        0 => [big, a, b, c],
        1 => [a, big, b, c],
        2 => [a, b, big, c],
        _ => [a, b, c, big],
    }
}

#[inline]
fn unorm8(v: f32) -> u32 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32
}

/// Encode one record into its four words.
#[inline]
pub fn pack_record(record: &SplatRecord, chunk: &ChunkBounds, scales: &ScaleRange) -> [u32; 4] {
    let px = chunk.encode_axis(0, record.center[0]);
    let py = chunk.encode_axis(1, record.center[1]);
    let pz = chunk.encode_axis(2, record.center[2]);
    let sx = scales.encode(record.scales[0]);
    let sy = scales.encode(record.scales[1]);
    let sz = scales.encode(record.scales[2]);
    let (q0, q1, q2, qi) = encode_quaternion(record.rotation);
    // BGRA byte order (makepad's BGRAu8_32 texel): sampled .xyzw = r,g,b,a.
    let word0 = unorm8(record.color[2])
        | unorm8(record.color[1]) << 8
        | unorm8(record.color[0]) << 16
        | unorm8(record.color[3]) << 24;
    let word1 = px | py << 14 | (pz & 0xf) << 28;
    let word2 = (pz >> 4) | sx << 10 | sy << 18 | (sz & 0x3f) << 26;
    let word3 = (sz >> 6) | q0 << 2 | q1 << 11 | q2 << 20 | qi << 29;
    [word0, word1, word2, word3]
}

/// The exact inverse of `pack_record` (the shader does the same math in
/// f32); used by the tests to bound the quantization error.
pub fn unpack_record(words: [u32; 4], chunk: &ChunkBounds, scales: &ScaleRange) -> SplatRecord {
    let [w0, w1, w2, w3] = words;
    let color = [
        ((w0 >> 16) & 0xff) as f32 / 255.0,
        ((w0 >> 8) & 0xff) as f32 / 255.0,
        (w0 & 0xff) as f32 / 255.0,
        ((w0 >> 24) & 0xff) as f32 / 255.0,
    ];
    let px = w1 & 0x3fff;
    let py = (w1 >> 14) & 0x3fff;
    let pz = ((w1 >> 28) & 0xf) | ((w2 & 0x3ff) << 4);
    let sx = (w2 >> 10) & 0xff;
    let sy = (w2 >> 18) & 0xff;
    let sz = ((w2 >> 26) & 0x3f) | ((w3 & 0x3) << 6);
    let q0 = (w3 >> 2) & 0x1ff;
    let q1 = (w3 >> 11) & 0x1ff;
    let q2 = (w3 >> 20) & 0x1ff;
    let qi = (w3 >> 29) & 0x3;
    SplatRecord {
        center: [
            chunk.decode_axis(0, px),
            chunk.decode_axis(1, py),
            chunk.decode_axis(2, pz),
        ],
        scales: [scales.decode(sx), scales.decode(sy), scales.decode(sz)],
        rotation: decode_quaternion(q0, q1, q2, qi),
        color,
    }
}

/// 30-bit Morton code of a position inside `bounds_min..bounds_max`.
#[inline]
pub fn morton_code(p: [f32; 3], bounds_min: [f32; 3], inv_extent: [f32; 3]) -> u32 {
    let mut code = 0u32;
    for axis in 0..3 {
        let t = ((p[axis] - bounds_min[axis]) * inv_extent[axis]).clamp(0.0, 1.0);
        let mut v = (t * 1023.0) as u32;
        // Spread the 10 bits of v to every third bit.
        v = (v | (v << 16)) & 0x030000ff;
        v = (v | (v << 8)) & 0x0300f00f;
        v = (v | (v << 4)) & 0x030c30c3;
        v = (v | (v << 2)) & 0x09249249;
        code |= v << axis;
    }
    code
}

/// Permutation that orders `records` along a Morton curve (spatially coherent
/// runs make the chunk bounds tight), plus each ordered record's Morton
/// code. Stable for equal codes.
pub fn morton_sorted(records: &[SplatRecord]) -> (Vec<u32>, Vec<u32>) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for record in records {
        for axis in 0..3 {
            min[axis] = min[axis].min(record.center[axis]);
            max[axis] = max[axis].max(record.center[axis]);
        }
    }
    let mut inv_extent = [0.0f32; 3];
    for axis in 0..3 {
        if !min[axis].is_finite() {
            min[axis] = 0.0;
            max[axis] = 0.0;
        }
        inv_extent[axis] = 1.0 / (max[axis] - min[axis]).max(1e-7);
    }
    let mut keyed: Vec<(u32, u32)> = records
        .iter()
        .enumerate()
        .map(|(i, record)| (morton_code(record.center, min, inv_extent), i as u32))
        .collect();
    keyed.sort_unstable_by(|a, b| match a.0.cmp(&b.0) {
        Ordering::Equal => a.1.cmp(&b.1),
        other => other,
    });
    let order = keyed.iter().map(|(_, i)| *i).collect();
    let codes = keyed.iter().map(|(code, _)| *code).collect();
    (order, codes)
}

/// Chunk assignment of Morton-ordered records: `(first, len)` runs of at
/// most `CHUNK_SPLATS` that never cross a level-`CHUNK_SPLIT_LEVEL` cell.
pub fn chunk_runs(codes: &[u32]) -> Vec<(usize, usize)> {
    let shift = 30 - 3 * CHUNK_SPLIT_LEVEL;
    let mut runs = Vec::with_capacity(codes.len() / CHUNK_SPLATS + 1);
    let mut start = 0usize;
    while start < codes.len() {
        let cell = codes[start] >> shift;
        let mut end = start + 1;
        while end < codes.len() && end - start < CHUNK_SPLATS && (codes[end] >> shift) == cell {
            end += 1;
        }
        runs.push((start, end - start));
        start = end;
    }
    runs
}

/// Everything the GPU needs for a scene, ready to upload.
pub struct PackedScene {
    /// 4 words per record, rows of `4 * RECORDS_PER_ROW` texels, row count
    /// `rows`; the tail of the last row is zero.
    pub words: Vec<u32>,
    pub rows: usize,
    /// 8 floats per chunk (min xyz, pad, extent xyz, pad), rows of
    /// `2 * CHUNKS_PER_ROW` texels.
    pub chunk_texels: Vec<f32>,
    pub chunk_rows: usize,
    pub scale_range: ScaleRange,
    /// Record slots (real records + chunk padding) = chunks * CHUNK_SPLATS.
    pub count: usize,
    /// Real records.
    pub records: usize,
    /// Sort-side mirror, one entry per slot: centers (local space), the
    /// largest axis length per splat (for culling bounds; < 0 = padding) and
    /// the product of the two largest axes (projected-area estimate).
    pub centers: Vec<[f32; 3]>,
    pub radius_bound: Vec<f32>,
    pub axis_product: Vec<f32>,
}

impl PackedScene {
    /// Pack `records` (in Morton order, with their codes): assign chunks,
    /// pad closed-early chunks with invisible records, encode.
    pub fn build(records: &[SplatRecord], codes: &[u32]) -> PackedScene {
        debug_assert_eq!(records.len(), codes.len());
        let scale_range = ScaleRange::from_records(records);
        let runs = chunk_runs(codes);
        let chunk_count = runs.len().max(1);
        let slot_count = chunk_count * CHUNK_SPLATS;
        let rows = slot_count.div_ceil(RECORDS_PER_ROW).max(1);
        let mut words = vec![0u32; rows * RECORDS_PER_ROW * 4];
        let chunk_rows = chunk_count.div_ceil(CHUNKS_PER_ROW).max(1);
        let mut chunk_texels = vec![0.0f32; chunk_rows * CHUNKS_PER_ROW * 8];
        // Padding slots: center at the chunk origin, negative radius bound.
        let mut centers = vec![[0.0f32; 3]; slot_count];
        let mut radius_bound = vec![-1.0f32; slot_count];
        let mut axis_product = vec![0.0f32; slot_count];

        for (chunk_index, &(first, len)) in runs.iter().enumerate() {
            let chunk_records = &records[first..first + len];
            let bounds = ChunkBounds::from_records(chunk_records);
            let base = chunk_index * 8;
            chunk_texels[base..base + 3].copy_from_slice(&bounds.min);
            chunk_texels[base + 4..base + 7].copy_from_slice(&bounds.extent);
            for (k, record) in chunk_records.iter().enumerate() {
                let slot = chunk_index * CHUNK_SPLATS + k;
                words[slot * 4..slot * 4 + 4]
                    .copy_from_slice(&pack_record(record, &bounds, &scale_range));
                centers[slot] = record.center;
                let mut axes = [
                    record.scales[0].abs(),
                    record.scales[1].abs(),
                    record.scales[2].abs(),
                ];
                axes.sort_by(|a, b| b.total_cmp(a));
                radius_bound[slot] = axes[0];
                axis_product[slot] = axes[0] * axes[1];
            }
            for k in len..CHUNK_SPLATS {
                centers[chunk_index * CHUNK_SPLATS + k] = bounds.min;
            }
        }

        PackedScene {
            words,
            rows,
            chunk_texels,
            chunk_rows,
            scale_range,
            count: slot_count,
            records: records.len(),
            centers,
            radius_bound,
            axis_product,
        }
    }

    pub fn record_bytes(&self) -> u64 {
        (self.words.len() * 4) as u64
    }

    pub fn chunk_bytes(&self) -> u64 {
        (self.chunk_texels.len() * 4) as u64
    }
}

#[cfg(test)]
include!("../tests/obj/splat_pack.rs");
