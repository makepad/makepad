//! BC7 (BPTC) block format: pack, unpack, decode all eight modes and encode
//! RGBA8 / BGRA8 images directly to BC7 blocks, written from the Khronos Data
//! Format Specification 1.3, section 20.1. UASTC transcoding lives alongside
//! in [`crate::transcode`].

use crate::bc7_tables as t;
use crate::transcode;

/// Per mode: subsets, partition bits, rotation bits, index-selection bits,
/// colour bits, alpha bits, per-endpoint p-bit, shared p-bit, index bits,
/// secondary index bits (Table 109).
pub(crate) const NS: [u8; 8] = [3, 2, 3, 2, 1, 1, 1, 2];
const PB: [u8; 8] = [4, 6, 6, 6, 0, 0, 0, 6];
const RB: [u8; 8] = [0, 0, 0, 0, 2, 2, 0, 0];
const ISB: [u8; 8] = [0, 0, 0, 0, 1, 0, 0, 0];
pub(crate) const CB: [u8; 8] = [4, 6, 5, 7, 5, 7, 7, 5];
pub(crate) const AB: [u8; 8] = [0, 0, 0, 0, 6, 8, 7, 5];
pub(crate) const EPB: [u8; 8] = [1, 0, 0, 1, 0, 0, 1, 1];
pub(crate) const SPB: [u8; 8] = [0, 1, 0, 0, 0, 0, 0, 0];
pub(crate) const IB: [u8; 8] = [3, 3, 2, 2, 2, 2, 4, 2];
pub(crate) const IB2: [u8; 8] = [0, 0, 0, 0, 3, 2, 0, 0];

/// A BC7 block's fields. Endpoints are the stored (unexpanded) values;
/// p-bits are per endpoint (a shared p-bit is stored in both).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bc7 {
    pub mode: u8,
    pub partition: u8,
    pub rotation: u8,
    pub index_select: u8,
    /// `[subset][endpoint][rgba]`.
    pub endpoints: [[[u8; 4]; 2]; 3],
    /// `[subset][endpoint]`.
    pub pbits: [[u8; 2]; 3],
    pub indices: [u8; 16],
    pub indices2: [u8; 16],
}

/// Subset of texel `i` in `partition` for a mode with `ns` subsets.
pub fn subset(ns: u8, partition: u8, i: usize) -> usize {
    match ns { 2 => t::P2[partition as usize][i] as usize, 3 => t::P3[partition as usize][i] as usize, _ => 0 }
}
/// Anchor texel of `subset` (index MSB implied zero).
pub fn anchor(ns: u8, partition: u8, subset: usize) -> usize {
    match (ns, subset) {
        (_, 0) => 0,
        (2, _) => t::ANCHOR2[partition as usize] as usize,
        (3, 1) => t::ANCHOR3_2[partition as usize] as usize,
        _ => t::ANCHOR3_3[partition as usize] as usize,
    }
}
fn is_anchor(ns: u8, partition: u8, i: usize) -> bool { (0..ns as usize).any(|s| anchor(ns, partition, s) == i) }

/// BPTC interpolation weights (Table 119).
pub fn weight(bits: u8, i: u8) -> u32 {
    const W2: [u8; 4] = [0, 21, 43, 64];
    const W3: [u8; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
    const W4: [u8; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];
    (match bits { 2 => W2[i as usize], 3 => W3[i as usize], _ => W4[i as usize] }) as u32
}
#[inline]
pub fn interpolate(e0: u8, e1: u8, w: u32) -> u8 { (((64 - w) * e0 as u32 + w * e1 as u32 + 32) >> 6) as u8 }

/// Expand an `n`-bit endpoint value to 8 bits by replicating its top bits.
#[inline]
pub fn expand(v: u8, n: u32) -> u8 { let v = v as u32; ((v << (8 - n)) | (v >> (2 * n - 8))) as u8 }

struct Bits { bits: u128, pos: u32 }
impl Bits {
    #[inline]
    fn new(data: &[u8; 16], pos: u32) -> Self { Self { bits: u128::from_le_bytes(*data), pos } }
    #[inline]
    fn get(&mut self, n: u32) -> u32 {
        if n == 0 { return 0; }
        let v = ((self.bits >> self.pos) as u32) & ((1u32 << n) - 1);
        self.pos += n;
        v
    }
}
struct Writer { bits: u128, pos: u32 }
impl Writer {
    #[inline]
    fn put(&mut self, n: u32, v: u32) {
        if n == 0 { return; }
        self.bits |= ((v as u128) & ((1u128 << n) - 1)) << self.pos;
        self.pos += n;
    }
}

pub fn unpack(block: &[u8; 16]) -> Option<Bc7> {
    let mode = (0..8).find(|m| (block[0] >> m) & 1 == 1)? as u8;
    let m = mode as usize;
    let mut r = Bits::new(block, m as u32 + 1);
    let mut b = Bc7 { mode, ..Default::default() };
    b.partition = r.get(PB[m] as u32) as u8;
    b.rotation = r.get(RB[m] as u32) as u8;
    b.index_select = r.get(ISB[m] as u32) as u8;
    let ns = NS[m] as usize;
    for c in 0..3 { for s in 0..ns { for e in 0..2 { b.endpoints[s][e][c] = r.get(CB[m] as u32) as u8; } } }
    if AB[m] > 0 { for s in 0..ns { for e in 0..2 { b.endpoints[s][e][3] = r.get(AB[m] as u32) as u8; } } }
    if EPB[m] == 1 { for s in 0..ns { for e in 0..2 { b.pbits[s][e] = r.get(1) as u8; } } }
    if SPB[m] == 1 { for s in 0..ns { let p = r.get(1) as u8; b.pbits[s] = [p, p]; } }
    for i in 0..16 { b.indices[i] = r.get(IB[m] as u32 - is_anchor(NS[m], b.partition, i) as u32) as u8; }
    if IB2[m] > 0 { for i in 0..16 { b.indices2[i] = r.get(IB2[m] as u32 - (i == 0) as u32) as u8; } }
    Some(b)
}

pub fn pack(b: &Bc7) -> [u8; 16] {
    let m = b.mode as usize;
    let mut w = Writer { bits: 0, pos: 0 };
    w.put(m as u32 + 1, 1 << m);
    w.put(PB[m] as u32, b.partition as u32);
    w.put(RB[m] as u32, b.rotation as u32);
    w.put(ISB[m] as u32, b.index_select as u32);
    let ns = NS[m] as usize;
    for c in 0..3 { for s in 0..ns { for e in 0..2 { w.put(CB[m] as u32, b.endpoints[s][e][c] as u32); } } }
    if AB[m] > 0 { for s in 0..ns { for e in 0..2 { w.put(AB[m] as u32, b.endpoints[s][e][3] as u32); } } }
    if EPB[m] == 1 { for s in 0..ns { for e in 0..2 { w.put(1, b.pbits[s][e] as u32); } } }
    if SPB[m] == 1 { for s in 0..ns { w.put(1, b.pbits[s][0] as u32); } }
    for i in 0..16 { w.put(IB[m] as u32 - is_anchor(NS[m], b.partition, i) as u32, b.indices[i] as u32); }
    if IB2[m] > 0 { for i in 0..16 { w.put(IB2[m] as u32 - (i == 0) as u32, b.indices2[i] as u32); } }
    debug_assert_eq!(w.pos, 128);
    w.bits.to_le_bytes()
}

/// The 8-bit RGBA endpoints of a block (`[subset][endpoint]`).
pub fn expanded_endpoints(b: &Bc7) -> [[[u8; 4]; 2]; 3] {
    let m = b.mode as usize;
    let mut out = [[[0u8; 4]; 2]; 3];
    let pbit = EPB[m] == 1 || SPB[m] == 1;
    for s in 0..NS[m] as usize { for e in 0..2 {
        for c in 0..3 {
            out[s][e][c] = if pbit { expand(b.endpoints[s][e][c] << 1 | b.pbits[s][e], CB[m] as u32 + 1) } else { expand(b.endpoints[s][e][c], CB[m] as u32) };
        }
        out[s][e][3] = match AB[m] {
            0 => 255,
            bits if pbit => expand(b.endpoints[s][e][3] << 1 | b.pbits[s][e], bits as u32 + 1),
            8 => b.endpoints[s][e][3],
            bits => expand(b.endpoints[s][e][3], bits as u32),
        };
    } }
    out
}

pub fn decode_unpacked(b: &Bc7) -> [[u8; 4]; 16] {
    let m = b.mode as usize;
    let e = expanded_endpoints(b);
    let mut out = [[0u8; 4]; 16];
    for i in 0..16 {
        let s = subset(NS[m], b.partition, i);
        let (ci, cb, ai, ab) = if IB2[m] == 0 { (b.indices[i], IB[m], b.indices[i], IB[m]) }
            else if b.index_select == 1 { (b.indices2[i], IB2[m], b.indices[i], IB[m]) }
            else { (b.indices[i], IB[m], b.indices2[i], IB2[m]) };
        let mut px = [0u8; 4];
        for c in 0..3 { px[c] = interpolate(e[s][0][c], e[s][1][c], weight(cb, ci)); }
        px[3] = interpolate(e[s][0][3], e[s][1][3], weight(ab, ai));
        match b.rotation { 1 => px.swap(0, 3), 2 => px.swap(1, 3), 3 => px.swap(2, 3), _ => {} }
        out[i] = px;
    }
    out
}

/// Decode one block (a reserved all-zero mode byte decodes to zeros).
pub fn decode_block(block: &[u8; 16]) -> [[u8; 4]; 16] {
    match unpack(block) { Some(b) => decode_unpacked(&b), None => [[0; 4]; 16] }
}

/// Decode a BC7 image to RGBA8.
pub fn decode_image(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let (w, h) = (width as usize, height as usize);
    let bx = (w + 3) / 4;
    let need = bx * ((h + 3) / 4) * 16;
    if data.len() < need { return Err(format!("BC7 data too short for {width}x{height}")); }
    let mut out = vec![0u8; w * h * 4];
    for (index, chunk) in data[..need].chunks_exact(16).enumerate() {
        let texels = decode_block(chunk.try_into().unwrap());
        let (x0, y0) = ((index % bx) * 4, (index / bx) * 4);
        for y in 0..4 { for x in 0..4 {
            let (px, py) = (x0 + x, y0 + y);
            if px < w && py < h { out[(py * w + px) * 4..][..4].copy_from_slice(&texels[y * 4 + x]); }
        } }
    }
    Ok(out)
}

// ── Direct RGBA8 / BGRA8 → BC7 encoder ──────────────────────────────────

const W2_U32: [u32; 4] = [0, 21, 43, 64];
const W3_U32: [u32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const W4_U32: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// Mode 6 all-zero block (`0x40` mode bit, zero endpoints, zero p-bits, zero
/// indices): decodes to `[[0, 0, 0, 0]; 16]` with zero error.
const ZERO_MODE6_BLOCK: [u8; 16] = [0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// Canonical 2-subset partitions covering vertical, horizontal, diagonal and
/// corner splits for opaque sharp-edge blocks (Mode 1).
const MODE1_PARTITIONS: [u8; 16] = [0, 1, 2, 3, 13, 14, 15, 17, 18, 26, 29, 32, 33, 35, 44, 52];

const fn build_nearest_w(weights: &[u32]) -> [u8; 65] {
    let mut table = [0u8; 65];
    let mut w = 0u32;
    while w <= 64 {
        let mut best_i = 0usize;
        let mut best_d = u32::MAX;
        let mut i = 0usize;
        while i < weights.len() {
            let d = w.abs_diff(weights[i]);
            if d < best_d {
                best_d = d;
                best_i = i;
            }
            i += 1;
        }
        table[w as usize] = best_i as u8;
        w += 1;
    }
    table
}

const NEAREST_W3: [u8; 65] = build_nearest_w(&W3_U32);
const NEAREST_W4: [u8; 65] = build_nearest_w(&W4_U32);

/// Bytes of one BC7 level of `width × height` texels (`16` bytes per `4×4` tile).
#[inline]
pub const fn encoded_len(width: u32, height: u32) -> usize {
    if width == 0 || height == 0 {
        return 0;
    }
    ((width as usize + 3) / 4) * ((height as usize + 3) / 4) * 16
}

/// Principal-axis line segment through `pts` (`dims` = 3 or 4) on the stack.
#[inline]
fn fit_line_stack(pts: &[[f32; 4]], dims: usize) -> ([f32; 4], [f32; 4]) {
    if pts.is_empty() {
        return ([0.0; 4], [0.0; 4]);
    }
    let inv_n = 1.0 / pts.len() as f32;
    let mut mean = [0.0f32; 4];
    for p in pts {
        for c in 0..dims {
            mean[c] += p[c];
        }
    }
    for c in 0..dims {
        mean[c] *= inv_n;
    }
    let mut cov = [[0.0f32; 4]; 4];
    for p in pts {
        let mut d = [0.0f32; 4];
        for c in 0..dims {
            d[c] = p[c] - mean[c];
        }
        for r in 0..dims {
            for c in r..dims {
                cov[r][c] += d[r] * d[c];
            }
        }
    }
    let mut max_c = 0usize;
    for r in 0..dims {
        for c in 0..r {
            cov[r][c] = cov[c][r];
        }
        if cov[r][r] > cov[max_c][max_c] {
            max_c = r;
        }
    }
    if cov[max_c][max_c] < 1e-4 {
        return (mean, mean);
    }
    let mut axis = [0.0f32; 4];
    axis[max_c] = 1.0;
    for _ in 0..4 {
        let mut next = [0.0f32; 4];
        let mut len2 = 0.0f32;
        for r in 0..dims {
            let mut s = 0.0f32;
            for c in 0..dims {
                s += cov[r][c] * axis[c];
            }
            next[r] = s;
            len2 += s * s;
        }
        if len2 <= 1e-12 {
            break;
        }
        let inv = 1.0 / len2.sqrt();
        for r in 0..dims {
            axis[r] = next[r] * inv;
        }
    }
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for p in pts {
        let mut proj = 0.0f32;
        for c in 0..dims {
            proj += (p[c] - mean[c]) * axis[c];
        }
        if proj < lo {
            lo = proj;
        }
        if proj > hi {
            hi = proj;
        }
    }
    let mut xl = [0.0f32; 4];
    let mut xh = [0.0f32; 4];
    for c in 0..dims {
        xl[c] = (mean[c] + axis[c] * lo).clamp(0.0, 255.0);
        xh[c] = (mean[c] + axis[c] * hi).clamp(0.0, 255.0);
    }
    (xl, xh)
}

#[inline]
fn pack_mode6(mut lo7: [u8; 4], mut hi7: [u8; 4], mut pbits: [u8; 2], indices: &[u8; 16]) -> [u8; 16] {
    let flip = indices[0] >= 8;
    if flip {
        std::mem::swap(&mut lo7, &mut hi7);
        pbits.swap(0, 1);
    }
    let mut bits: u128 = 0x40;
    let mut pos: u32 = 7;
    for c in 0..4 {
        bits |= ((lo7[c] & 0x7f) as u128) << pos;
        pos += 7;
        bits |= ((hi7[c] & 0x7f) as u128) << pos;
        pos += 7;
    }
    bits |= ((pbits[0] & 1) as u128) << pos;
    bits |= ((pbits[1] & 1) as u128) << (pos + 1);
    pos += 2;
    let idx0 = if flip { 15 - indices[0] } else { indices[0] };
    bits |= ((idx0 & 0x7) as u128) << pos;
    pos += 3;
    for &raw in &indices[1..] {
        let idx = if flip { 15 - raw } else { raw };
        bits |= ((idx & 0xf) as u128) << pos;
        pos += 4;
    }
    debug_assert_eq!(pos, 128);
    bits.to_le_bytes()
}

/// Fit BC7 Mode 6 (1 subset, 7-bit RGBA + per-endpoint p-bit = 8-bit RGBA
/// endpoints, 4-bit = 16 weights) with one least-squares refinement pass.
#[inline]
fn fit_mode6(texels: &[[u8; 4]; 16], opaque: bool) -> ([u8; 16], u32) {
    let mut pts = [[0.0f32; 4]; 16];
    for i in 0..16 {
        pts[i] = [texels[i][0] as f32, texels[i][1] as f32, texels[i][2] as f32, texels[i][3] as f32];
    }
    let dims = if opaque { 3 } else { 4 };
    let (mut xl, mut xh) = fit_line_stack(&pts, dims);
    if opaque {
        xl[3] = 255.0;
        xh[3] = 255.0;
    }

    let mut best_lo7 = [0u8; 4];
    let mut best_hi7 = [0u8; 4];
    let mut best_pbits = [0u8; 2];
    let mut best_indices = [0u8; 16];
    let mut best_err = u32::MAX;

    for pass in 0..2 {
        let norm = |v: [f32; 4]| [v[0] * (1.0 / 255.0), v[1] * (1.0 / 255.0), v[2] * (1.0 / 255.0), v[3] * (1.0 / 255.0)];
        let (lo7, hi7, pbits) = transcode::unique_pbits(4, 7, norm(xl), norm(xh));
        let e0 = [(lo7[0] << 1) | pbits[0], (lo7[1] << 1) | pbits[0], (lo7[2] << 1) | pbits[0], (lo7[3] << 1) | pbits[0]];
        let e1 = [(hi7[0] << 1) | pbits[1], (hi7[1] << 1) | pbits[1], (hi7[2] << 1) | pbits[1], (hi7[3] << 1) | pbits[1]];
        let d = [
            e1[0] as i32 - e0[0] as i32,
            e1[1] as i32 - e0[1] as i32,
            e1[2] as i32 - e0[2] as i32,
            e1[3] as i32 - e0[3] as i32,
        ];
        let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2] + d[3] * d[3];

        let mut indices = [0u8; 16];
        let mut total_err = 0u32;
        if len2 == 0 {
            for px in texels {
                let dr = e0[0] as i32 - px[0] as i32;
                let dg = e0[1] as i32 - px[1] as i32;
                let db = e0[2] as i32 - px[2] as i32;
                let da = e0[3] as i32 - px[3] as i32;
                total_err += (dr * dr + dg * dg + db * db + da * da) as u32;
            }
        } else {
            let inv_len2 = 64.0 / len2 as f32;
            for (px, dst_idx) in texels.iter().zip(indices.iter_mut()) {
                let dot = (px[0] as i32 - e0[0] as i32) * d[0]
                    + (px[1] as i32 - e0[1] as i32) * d[1]
                    + (px[2] as i32 - e0[2] as i32) * d[2]
                    + (px[3] as i32 - e0[3] as i32) * d[3];
                let center = if dot <= 0 {
                    0usize
                } else if dot >= len2 {
                    15usize
                } else {
                    let w_est = (dot as f32 * inv_len2 + 0.5) as usize;
                    if w_est >= 64 { 15 } else { NEAREST_W4[w_est & 63] as usize }
                };
                let k_lo = center.saturating_sub(1);
                let k_hi = (center + 1).min(15);
                let mut best_k = center as u8;
                let mut best_e = u32::MAX;
                for k in k_lo..=k_hi {
                    let w = W4_U32[k & 15];
                    let dr = interpolate(e0[0], e1[0], w) as i32 - px[0] as i32;
                    let dg = interpolate(e0[1], e1[1], w) as i32 - px[1] as i32;
                    let db = interpolate(e0[2], e1[2], w) as i32 - px[2] as i32;
                    let da = interpolate(e0[3], e1[3], w) as i32 - px[3] as i32;
                    let e = (dr * dr + dg * dg + db * db + da * da) as u32;
                    if e < best_e {
                        best_e = e;
                        best_k = k as u8;
                    }
                }
                *dst_idx = best_k;
                total_err += best_e;
            }
        }

        if total_err < best_err {
            best_err = total_err;
            best_lo7 = lo7;
            best_hi7 = hi7;
            best_pbits = pbits;
            best_indices = indices;
        }
        if pass == 1 || total_err <= 16 {
            break;
        }

        let (mut sw11, mut sw12, mut sw22) = (0u32, 0u32, 0u32);
        let mut sx = [0u32; 4];
        let mut sy = [0u32; 4];
        for (px, &idx) in texels.iter().zip(indices.iter()) {
            let w = W4_U32[(idx & 15) as usize];
            let iw = 64 - w;
            sw11 += iw * iw;
            sw12 += iw * w;
            sw22 += w * w;
            for c in 0..4 {
                let v = px[c] as u32;
                sx[c] += iw * v;
                sy[c] += w * v;
            }
        }
        let det = sw11 as i64 * sw22 as i64 - sw12 as i64 * sw12 as i64;
        if det <= 0 {
            break;
        }
        let inv_det = 64.0 / det as f32;
        for c in 0..4 {
            let num0 = sw22 as i64 * sx[c] as i64 - sw12 as i64 * sy[c] as i64;
            let num1 = sw11 as i64 * sy[c] as i64 - sw12 as i64 * sx[c] as i64;
            xl[c] = (num0 as f32 * inv_det).clamp(0.0, 255.0);
            xh[c] = (num1 as f32 * inv_det).clamp(0.0, 255.0);
        }
        if opaque {
            xl[3] = 255.0;
            xh[3] = 255.0;
        }
    }

    (pack_mode6(best_lo7, best_hi7, best_pbits, &best_indices), best_err)
}

/// Fit dual-plane translucent BC7 Mode 5 (`7-bit RGB + 8-bit A, 2+2 weights`)
/// or Mode 4 (`5-bit RGB + 6-bit A, 2-bit RGB + 3-bit A weights`) so straight-alpha
/// anti-aliased silhouettes decouple RGB from Alpha without dark fringes.
fn fit_dual_alpha(texels: &[[u8; 4]; 16], mode: u8) -> ([u8; 16], u32) {
    let mut pts = [[0.0f32; 4]; 16];
    let (mut min_a, mut max_a) = (255u8, 0u8);
    for i in 0..16 {
        pts[i] = [texels[i][0] as f32, texels[i][1] as f32, texels[i][2] as f32, 0.0];
        min_a = min_a.min(texels[i][3]);
        max_a = max_a.max(texels[i][3]);
    }
    let (mut xl, mut xh) = fit_line_stack(&pts, 3);
    let (mut al, mut ah) = (min_a as f32, max_a as f32);

    let (cb, ab, aw): (u32, u32, &[u32]) = if mode == 5 { (7, 8, &W2_U32) } else { (5, 6, &W3_U32) };
    let c_max = ((1u32 << cb) - 1) as f32;
    let a_max = ((1u32 << ab) - 1) as f32;

    let mut best_b = Bc7 { mode, ..Default::default() };
    let mut best_err = u32::MAX;

    for pass in 0..2 {
        let mut lo = [0u8; 4];
        let mut hi = [0u8; 4];
        let mut e0 = [0u8; 4];
        let mut e1 = [0u8; 4];
        for c in 0..3 {
            lo[c] = ((xl[c] * (c_max / 255.0)).round() as i32).clamp(0, c_max as i32) as u8;
            hi[c] = ((xh[c] * (c_max / 255.0)).round() as i32).clamp(0, c_max as i32) as u8;
            e0[c] = expand(lo[c], cb);
            e1[c] = expand(hi[c], cb);
        }
        lo[3] = ((al * (a_max / 255.0)).round() as i32).clamp(0, a_max as i32) as u8;
        hi[3] = ((ah * (a_max / 255.0)).round() as i32).clamp(0, a_max as i32) as u8;
        e0[3] = if ab == 8 { lo[3] } else { expand(lo[3], ab) };
        e1[3] = if ab == 8 { hi[3] } else { expand(hi[3], ab) };

        let mut indices = [0u8; 16];
        let mut indices2 = [0u8; 16];
        let mut total_err = 0u32;

        for i in 0..16 {
            let px = texels[i];
            let mut best_ck = 0u8;
            let mut best_ce = u32::MAX;
            for (k, &w) in W2_U32.iter().enumerate() {
                let dr = interpolate(e0[0], e1[0], w) as i32 - px[0] as i32;
                let dg = interpolate(e0[1], e1[1], w) as i32 - px[1] as i32;
                let db = interpolate(e0[2], e1[2], w) as i32 - px[2] as i32;
                let e = (dr * dr + dg * dg + db * db) as u32;
                if e < best_ce {
                    best_ce = e;
                    best_ck = k as u8;
                }
            }
            let mut best_ak = 0u8;
            let mut best_ae = u32::MAX;
            for (k, &w) in aw.iter().enumerate() {
                let da = interpolate(e0[3], e1[3], w) as i32 - px[3] as i32;
                let e = (da * da) as u32;
                if e < best_ae {
                    best_ae = e;
                    best_ak = k as u8;
                }
            }
            indices[i] = best_ck;
            indices2[i] = best_ak;
            total_err += best_ce + best_ae;
        }

        if total_err < best_err {
            best_err = total_err;
            best_b = Bc7 {
                mode,
                endpoints: [[lo, hi], [[0; 4]; 2], [[0; 4]; 2]],
                indices,
                indices2,
                ..Default::default()
            };
        }
        if pass == 1 || total_err <= 16 {
            break;
        }

        // Refit RGB from `indices` (W2) and Alpha from `indices2` (`aw`).
        let (mut c11, mut c12, mut c22) = (0u32, 0u32, 0u32);
        let mut cx0 = [0u32; 3];
        let mut cy0 = [0u32; 3];
        let (mut a11, mut a12, mut a22, mut ax0, mut ay0) = (0u32, 0u32, 0u32, 0u32, 0u32);
        for i in 0..16 {
            let cw = W2_U32[indices[i] as usize];
            let icw = 64 - cw;
            c11 += icw * icw;
            c12 += icw * cw;
            c22 += cw * cw;
            for c in 0..3 {
                let v = texels[i][c] as u32;
                cx0[c] += icw * v;
                cy0[c] += cw * v;
            }
            let aw_i = aw[indices2[i] as usize];
            let iaw = 64 - aw_i;
            a11 += iaw * iaw;
            a12 += iaw * aw_i;
            a22 += aw_i * aw_i;
            let va = texels[i][3] as u32;
            ax0 += iaw * va;
            ay0 += aw_i * va;
        }
        let cdet = c11 as i64 * c22 as i64 - c12 as i64 * c12 as i64;
        if cdet > 0 {
            let inv = 64.0 / cdet as f32;
            for c in 0..3 {
                xl[c] = ((c22 as i64 * cx0[c] as i64 - c12 as i64 * cy0[c] as i64) as f32 * inv).clamp(0.0, 255.0);
                xh[c] = ((c11 as i64 * cy0[c] as i64 - c12 as i64 * cx0[c] as i64) as f32 * inv).clamp(0.0, 255.0);
            }
        }
        let adet = a11 as i64 * a22 as i64 - a12 as i64 * a12 as i64;
        if adet > 0 {
            let inv = 64.0 / adet as f32;
            al = ((a22 as i64 * ax0 as i64 - a12 as i64 * ay0 as i64) as f32 * inv).clamp(0.0, 255.0);
            ah = ((a11 as i64 * ay0 as i64 - a12 as i64 * ax0 as i64) as f32 * inv).clamp(0.0, 255.0);
        }
    }

    transcode::fix_bc7_anchors(&mut best_b);
    (pack(&best_b), best_err)
}

/// Fit opaque 2-subset BC7 Mode 1 (`6-bit RGB + shared p-bit, 3-bit weights`)
/// for sharp two-colour boundaries inside an opaque 4x4 block.
fn fit_mode1_opaque(texels: &[[u8; 4]; 16]) -> ([u8; 16], u32) {
    // Fast partition ranking by per-subset bounding-box diagonal projection error.
    let mut best_part = MODE1_PARTITIONS[0];
    let mut best_est = u32::MAX;
    for &part in &MODE1_PARTITIONS {
        let map = &t::P2[part as usize];
        let mut mn = [[255i32; 3]; 2];
        let mut mx = [[0i32; 3]; 2];
        for i in 0..16 {
            let s = map[i] as usize;
            for c in 0..3 {
                let v = texels[i][c] as i32;
                if v < mn[s][c] { mn[s][c] = v; }
                if v > mx[s][c] { mx[s][c] = v; }
            }
        }
        let d = [
            [mx[0][0] - mn[0][0], mx[0][1] - mn[0][1], mx[0][2] - mn[0][2]],
            [mx[1][0] - mn[1][0], mx[1][1] - mn[1][1], mx[1][2] - mn[1][2]],
        ];
        let len2 = [
            d[0][0] * d[0][0] + d[0][1] * d[0][1] + d[0][2] * d[0][2],
            d[1][0] * d[1][0] + d[1][1] * d[1][1] + d[1][2] * d[1][2],
        ];
        let inv_l2 = [
            if len2[0] > 0 { 1.0 / len2[0] as f32 } else { 0.0 },
            if len2[1] > 0 { 1.0 / len2[1] as f32 } else { 0.0 },
        ];
        let mut est = 0u32;
        for i in 0..16 {
            let s = (map[i] & 1) as usize;
            let l2 = len2[s];
            if l2 == 0 {
                continue;
            }
            let vx = texels[i][0] as i32 - mn[s][0];
            let vy = texels[i][1] as i32 - mn[s][1];
            let vz = texels[i][2] as i32 - mn[s][2];
            let dot = (vx * d[s][0] + vy * d[s][1] + vz * d[s][2]).clamp(0, l2);
            let t = dot as f32 * inv_l2[s];
            let rx = vx as f32 - d[s][0] as f32 * t;
            let ry = vy as f32 - d[s][1] as f32 * t;
            let rz = vz as f32 - d[s][2] as f32 * t;
            est += (rx * rx + ry * ry + rz * rz) as u32;
        }
        if est < best_est {
            best_est = est;
            best_part = part;
            if est == 0 {
                break;
            }
        }
    }

    let map = &t::P2[best_part as usize];
    let mut ends = [[[0.0f32; 4]; 2]; 2];
    for s in 0..2 {
        let mut buf = [[0.0f32; 4]; 16];
        let mut n = 0usize;
        for i in 0..16 {
            if map[i] as usize == s {
                buf[n] = [texels[i][0] as f32, texels[i][1] as f32, texels[i][2] as f32, 255.0];
                n += 1;
            }
        }
        let (xl, xh) = fit_line_stack(&buf[..n], 3);
        ends[s] = [xl, xh];
    }

    let mut best_b = Bc7 { mode: 1, partition: best_part, ..Default::default() };
    let mut best_err = u32::MAX;
    let norm = |v: [f32; 4]| [v[0] * (1.0 / 255.0), v[1] * (1.0 / 255.0), v[2] * (1.0 / 255.0), 1.0];

    for pass in 0..2 {
        let mut lo6 = [[0u8; 4]; 2];
        let mut hi6 = [[0u8; 4]; 2];
        let mut pbit = [0u8; 2];
        let mut e0 = [[0u8; 3]; 2];
        let mut e1 = [[0u8; 3]; 2];
        let mut d = [[0i32; 3]; 2];
        let mut len2 = [0i32; 2];
        for s in 0..2 {
            let (l, h, p) = transcode::shared_pbits(3, 6, norm(ends[s][0]), norm(ends[s][1]));
            lo6[s] = l;
            hi6[s] = h;
            pbit[s] = p;
            for c in 0..3 {
                e0[s][c] = expand((l[c] << 1) | p, 7);
                e1[s][c] = expand((h[c] << 1) | p, 7);
                d[s][c] = e1[s][c] as i32 - e0[s][c] as i32;
            }
            len2[s] = d[s][0] * d[s][0] + d[s][1] * d[s][1] + d[s][2] * d[s][2];
        }

        let inv_l2 = [
            if len2[0] > 0 { 64.0 / len2[0] as f32 } else { 0.0 },
            if len2[1] > 0 { 64.0 / len2[1] as f32 } else { 0.0 },
        ];
        let mut indices = [0u8; 16];
        let mut total_err = 0u32;
        for i in 0..16 {
            let s = (map[i] & 1) as usize;
            let px = texels[i];
            let l2 = len2[s];
            if l2 == 0 {
                let dr = e0[s][0] as i32 - px[0] as i32;
                let dg = e0[s][1] as i32 - px[1] as i32;
                let db = e0[s][2] as i32 - px[2] as i32;
                total_err += (dr * dr + dg * dg + db * db) as u32;
                continue;
            }
            let dot = (px[0] as i32 - e0[s][0] as i32) * d[s][0]
                + (px[1] as i32 - e0[s][1] as i32) * d[s][1]
                + (px[2] as i32 - e0[s][2] as i32) * d[s][2];
            let center = if dot <= 0 {
                0usize
            } else if dot >= l2 {
                7usize
            } else {
                let w_est = (dot as f32 * inv_l2[s] + 0.5) as usize;
                if w_est >= 64 { 7 } else { NEAREST_W3[w_est & 63] as usize }
            };
            let k_lo = center.saturating_sub(1);
            let k_hi = (center + 1).min(7);
            let mut best_k = center as u8;
            let mut best_e = u32::MAX;
            for k in k_lo..=k_hi {
                let w = W3_U32[k & 7];
                let dr = interpolate(e0[s][0], e1[s][0], w) as i32 - px[0] as i32;
                let dg = interpolate(e0[s][1], e1[s][1], w) as i32 - px[1] as i32;
                let db = interpolate(e0[s][2], e1[s][2], w) as i32 - px[2] as i32;
                let e = (dr * dr + dg * dg + db * db) as u32;
                if e < best_e {
                    best_e = e;
                    best_k = k as u8;
                }
            }
            indices[i] = best_k;
            total_err += best_e;
        }

        if total_err < best_err {
            best_err = total_err;
            best_b = Bc7 {
                mode: 1,
                partition: best_part,
                endpoints: [[lo6[0], hi6[0]], [lo6[1], hi6[1]], [[0; 4]; 2]],
                pbits: [[pbit[0], pbit[0]], [pbit[1], pbit[1]], [0; 2]],
                indices,
                ..Default::default()
            };
        }
        if pass == 1 || total_err <= 16 {
            break;
        }

        for s in 0..2 {
            let (mut sw11, mut sw12, mut sw22) = (0u32, 0u32, 0u32);
            let mut sx = [0u32; 3];
            let mut sy = [0u32; 3];
            for i in 0..16 {
                if map[i] as usize != s {
                    continue;
                }
                let w = W3_U32[indices[i] as usize];
                let iw = 64 - w;
                sw11 += iw * iw;
                sw12 += iw * w;
                sw22 += w * w;
                for c in 0..3 {
                    let v = texels[i][c] as u32;
                    sx[c] += iw * v;
                    sy[c] += w * v;
                }
            }
            let det = sw11 as i64 * sw22 as i64 - sw12 as i64 * sw12 as i64;
            if det > 0 {
                let inv = 64.0 / det as f32;
                for c in 0..3 {
                    ends[s][0][c] = ((sw22 as i64 * sx[c] as i64 - sw12 as i64 * sy[c] as i64) as f32 * inv).clamp(0.0, 255.0);
                    ends[s][1][c] = ((sw11 as i64 * sy[c] as i64 - sw12 as i64 * sx[c] as i64) as f32 * inv).clamp(0.0, 255.0);
                }
            }
        }
    }

    transcode::fix_bc7_anchors(&mut best_b);
    (pack(&best_b), best_err)
}

/// Encode one 4×4 RGBA8 tile into a 16-byte BC7 block.
///
/// Solid tiles reproduce every 8-bit RGBA colour exactly (`0` error); smooth
/// RGBA tiles use Mode 6 (`8-bit` endpoints, `16` weights); translucent
/// silhouettes evaluate dual-plane Modes 5 and 4 so straight-alpha RGB edges
/// never darken; opaque sharp-edge tiles evaluate 2-subset Mode 1.
pub fn encode_block(texels: &[[u8; 4]; 16]) -> [u8; 16] {
    let first = texels[0];
    let mut solid = true;
    let mut opaque = first[3] == 255;
    for px in &texels[1..] {
        if *px != first {
            solid = false;
        }
        if px[3] != 255 {
            opaque = false;
        }
    }
    if solid {
        if first == [0, 0, 0, 0] {
            return ZERO_MODE6_BLOCK;
        }
        return pack(&transcode::solid_bc7(first));
    }

    let (mut best_block, mut best_err) = fit_mode6(texels, opaque);
    if !opaque && best_err > 64 {
        let (b5, e5) = fit_dual_alpha(texels, 5);
        if e5 < best_err {
            best_block = b5;
            best_err = e5;
        }
        let (b4, e4) = fit_dual_alpha(texels, 4);
        if e4 < best_err {
            best_block = b4;
        }
    } else if opaque && best_err > 256 {
        let (b1, e1) = fit_mode1_opaque(texels);
        if e1 < best_err {
            best_block = b1;
        }
    }
    best_block
}

/// Encode a row-major RGBA8 level (`width * height * 4` bytes) into `dst`
/// (`encoded_len(width, height)` bytes) across `threads` workers (`0` = all
/// cores). Edge tiles clamp to the nearest in-bounds texel.
pub fn encode_image_slice(rgba: &[u8], width: u32, height: u32, threads: usize, dst: &mut [u8]) -> Result<(), String> {
    let (w, h) = (width as usize, height as usize);
    let need_src = w.saturating_mul(h).saturating_mul(4);
    if rgba.len() < need_src {
        return Err(format!("RGBA data too short: need {need_src} bytes for {width}x{height}, got {}", rgba.len()));
    }
    let need_dst = encoded_len(width, height);
    if dst.len() < need_dst {
        return Err(format!("BC7 output slice too short: need {need_dst} bytes for {width}x{height}, got {}", dst.len()));
    }
    if w == 0 || h == 0 {
        return Ok(());
    }
    let (bx, by) = ((w + 3) / 4, (h + 3) / 4);
    let out = &mut dst[..need_dst];
    let workers = if threads == 0 {
        std::thread::available_parallelism().map_or(1, |n| n.get()).min((bx * by / 512).max(1))
    } else {
        threads
    }
    .clamp(1, by);
    let encode_rows = |y_block_start: usize, rows: &mut [u8]| {
        for (j, block) in rows.chunks_exact_mut(16).enumerate() {
            let index = y_block_start * bx + j;
            let (x0, y0) = ((index % bx) * 4, (index / bx) * 4);
            let mut texels = [[0u8; 4]; 16];
            if x0 + 4 <= w && y0 + 4 <= h {
                for y in 0..4 {
                    let row = &rgba[((y0 + y) * w + x0) * 4..][..16];
                    for x in 0..4 {
                        texels[y * 4 + x] = [row[x * 4], row[x * 4 + 1], row[x * 4 + 2], row[x * 4 + 3]];
                    }
                }
            } else {
                for y in 0..4 {
                    let py = (y0 + y).min(h - 1);
                    for x in 0..4 {
                        let px = (x0 + x).min(w - 1);
                        let src = (py * w + px) * 4;
                        texels[y * 4 + x] = [rgba[src], rgba[src + 1], rgba[src + 2], rgba[src + 3]];
                    }
                }
            }
            block.copy_from_slice(&encode_block(&texels));
        }
    };
    if workers <= 1 {
        encode_rows(0, out);
    } else {
        let rows_per = by.div_ceil(workers);
        std::thread::scope(|scope| {
            for (chunk, rows) in out.chunks_mut(rows_per * bx * 16).enumerate() {
                let y_block_start = chunk * rows_per;
                scope.spawn(move || encode_rows(y_block_start, rows));
            }
        });
    }
    Ok(())
}

/// Append a BC7-encoded RGBA8 level directly onto `out` without a staging copy.
pub fn encode_image_into(rgba: &[u8], width: u32, height: u32, threads: usize, out: &mut Vec<u8>) -> Result<(), String> {
    let need = encoded_len(width, height);
    let start = out.len();
    out.resize(start + need, 0);
    encode_image_slice(rgba, width, height, threads, &mut out[start..start + need])
}

/// Encode a row-major RGBA8 image (`width * height * 4` bytes) to BC7 across
/// `threads` workers (`0` = all cores).
pub fn encode_image(rgba: &[u8], width: u32, height: u32, threads: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0u8; encoded_len(width, height)];
    encode_image_slice(rgba, width, height, threads, &mut out)?;
    Ok(out)
}

/// Encode a row-major packed `0xAARRGGBB` (`VecBGRAu8_32` / `VecMipBGRAu8_32`)
/// level (`width * height` `u32` texels) directly into `dst` across `threads`
/// workers (`0` = all cores).
pub fn encode_bgra_u32_slice(bgra: &[u32], width: u32, height: u32, threads: usize, dst: &mut [u8]) -> Result<(), String> {
    let (w, h) = (width as usize, height as usize);
    let need_src = w.saturating_mul(h);
    if bgra.len() < need_src {
        return Err(format!("BGRA u32 data too short: need {need_src} texels for {width}x{height}, got {}", bgra.len()));
    }
    let need_dst = encoded_len(width, height);
    if dst.len() < need_dst {
        return Err(format!("BC7 output slice too short: need {need_dst} bytes for {width}x{height}, got {}", dst.len()));
    }
    if w == 0 || h == 0 {
        return Ok(());
    }
    let (bx, by) = ((w + 3) / 4, (h + 3) / 4);
    let out = &mut dst[..need_dst];
    let workers = if threads == 0 {
        std::thread::available_parallelism().map_or(1, |n| n.get()).min((bx * by / 512).max(1))
    } else {
        threads
    }
    .clamp(1, by);
    let unpack_bgra = |px: u32| [(px >> 16) as u8, (px >> 8) as u8, px as u8, (px >> 24) as u8];
    let encode_rows = |y_block_start: usize, rows: &mut [u8]| {
        for (j, block) in rows.chunks_exact_mut(16).enumerate() {
            let index = y_block_start * bx + j;
            let (x0, y0) = ((index % bx) * 4, (index / bx) * 4);
            let mut texels = [[0u8; 4]; 16];
            if x0 + 4 <= w && y0 + 4 <= h {
                for y in 0..4 {
                    let row = &bgra[(y0 + y) * w + x0..][..4];
                    for x in 0..4 {
                        texels[y * 4 + x] = unpack_bgra(row[x]);
                    }
                }
            } else {
                for y in 0..4 {
                    let py = (y0 + y).min(h - 1);
                    for x in 0..4 {
                        let px = (x0 + x).min(w - 1);
                        texels[y * 4 + x] = unpack_bgra(bgra[py * w + px]);
                    }
                }
            }
            block.copy_from_slice(&encode_block(&texels));
        }
    };
    if workers <= 1 {
        encode_rows(0, out);
    } else {
        let rows_per = by.div_ceil(workers);
        std::thread::scope(|scope| {
            for (chunk, rows) in out.chunks_mut(rows_per * bx * 16).enumerate() {
                let y_block_start = chunk * rows_per;
                scope.spawn(move || encode_rows(y_block_start, rows));
            }
        });
    }
    Ok(())
}

/// Append a BC7-encoded `0xAARRGGBB` (`u32`) level onto `out`.
pub fn encode_bgra_u32_into(bgra: &[u32], width: u32, height: u32, threads: usize, out: &mut Vec<u8>) -> Result<(), String> {
    let need = encoded_len(width, height);
    let start = out.len();
    out.resize(start + need, 0);
    encode_bgra_u32_slice(bgra, width, height, threads, &mut out[start..start + need])
}

/// Encode a row-major `0xAARRGGBB` (`u32`) image to BC7 across `threads` workers.
pub fn encode_bgra_u32_image(bgra: &[u32], width: u32, height: u32, threads: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0u8; encoded_len(width, height)];
    encode_bgra_u32_slice(bgra, width, height, threads, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_round_trips_every_mode() {
        let mut seed = 7u32;
        let mut rnd = |n: u32| { seed = seed.wrapping_mul(1103515245).wrapping_add(12345); (seed >> 8) % n };
        for mode in 0..8u8 {
            let m = mode as usize;
            for _ in 0..40 {
                let mut b = Bc7 { mode, ..Default::default() };
                b.partition = rnd(1 << PB[m]) as u8;
                b.rotation = if RB[m] > 0 { rnd(4) as u8 } else { 0 };
                b.index_select = if ISB[m] > 0 { rnd(2) as u8 } else { 0 };
                for s in 0..NS[m] as usize { for e in 0..2 {
                    for c in 0..3 { b.endpoints[s][e][c] = rnd(1 << CB[m]) as u8; }
                    if AB[m] > 0 { b.endpoints[s][e][3] = rnd(1 << AB[m]) as u8; }
                    if EPB[m] == 1 { b.pbits[s][e] = rnd(2) as u8; }
                } if SPB[m] == 1 { let p = rnd(2) as u8; b.pbits[s] = [p, p]; } }
                for i in 0..16 {
                    let bits = IB[m] as u32 - is_anchor(NS[m], b.partition, i) as u32;
                    b.indices[i] = rnd(1 << bits) as u8;
                    if IB2[m] > 0 { b.indices2[i] = rnd(1 << (IB2[m] as u32 - (i == 0) as u32)) as u8; }
                }
                assert_eq!(unpack(&pack(&b)).unwrap(), b, "mode {mode}");
            }
        }
    }

    #[test]
    fn mode6_solid_decodes_exactly() {
        let b = Bc7 { mode: 6, endpoints: [[[100, 50, 25, 127]; 2], [[0; 4]; 2], [[0; 4]; 2]], pbits: [[1, 1], [0; 2], [0; 2]], ..Default::default() };
        for px in decode_block(&pack(&b)) { assert_eq!(px, [201, 101, 51, 255]); }
    }
}
