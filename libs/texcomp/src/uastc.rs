//! UASTC LDR 4x4: the 19-mode, 128-bit universal block format of Basis
//! Universal, written from the public (CC0) UASTC LDR 4x4 specification.
//!
//! A UASTC block is a subset of 4x4 LDR ASTC with a simpler bit layout,
//! chosen so a block converts to ASTC losslessly and to BC7, ETC and friends
//! with a small, fixed amount of per-block work. This module holds the block
//! layout, a full decoder (all 19 modes, to 32-bit RGBA) and the encoder.
//!
//! Block layout (spec "List of UASTC Modes"): bits are written from bit 0 of
//! byte 0 upward, LSB first. Every mode starts with a Huffman-coded mode
//! index, then hint bits for other transcoders (BC1, ETC1, ETC2 alpha), then
//! the partition pattern (PAT) and component selector (COMPSEL) when the
//! mode has them, then the endpoint trit/quint groups (ETQ), the endpoint
//! bits (EBITS) and finally the weights. Weights are plain bits; each
//! subset's anchor texel stores one bit less (its MSB is zero).
//!
//! Endpoints are interpolated exactly like ASTC (16-bit expansion, 6-bit
//! weights, rounding), with the "srgb" option off, as the reference encoder
//! does.

use crate::uastc_tables as t;

/// UASTC mode index of the solid-colour (void-extent) block.
pub const MODE_SOLID: usize = 8;

/// `{ code, length }` of each mode index's Huffman code (mode 19 reserved).
const MODE_HUFF: [(u8, u8); 20] = [
    (0x1, 4), (0x35, 6), (0x1D, 5), (0x3, 5), (0x13, 5), (0xB, 5), (0x1B, 5), (0x7, 5),
    (0x17, 5), (0xF, 5), (0x2, 3), (0x0, 2), (0x6, 3), (0x1F, 5), (0xD, 5), (0x5, 7),
    (0x15, 6), (0x25, 6), (0x9, 4), (0x45, 7),
];
/// Mode index by the lowest 7 bits of the first byte.
const HUFF_MODES: [u8; 128] = [
    11, 0, 10, 3, 11, 15, 12, 7, 11, 18, 10, 5, 11, 14, 12, 9, 11, 0, 10, 4, 11, 16, 12, 8, 11, 18, 10, 6, 11, 2, 12, 13,
    11, 0, 10, 3, 11, 17, 12, 7, 11, 18, 10, 5, 11, 14, 12, 9, 11, 0, 10, 4, 11, 1, 12, 8, 11, 18, 10, 6, 11, 2, 12, 13,
    11, 0, 10, 3, 11, 19, 12, 7, 11, 18, 10, 5, 11, 14, 12, 9, 11, 0, 10, 4, 11, 16, 12, 8, 11, 18, 10, 6, 11, 2, 12, 13,
    11, 0, 10, 3, 11, 17, 12, 7, 11, 18, 10, 5, 11, 14, 12, 9, 11, 0, 10, 4, 11, 1, 12, 8, 11, 18, 10, 6, 11, 2, 12, 13,
];

// Mode description tables (spec "UASTC Mode Description Tables").
pub(crate) const WEIGHT_BITS: [u8; 19] = [4, 2, 3, 2, 2, 3, 2, 2, 0, 2, 4, 2, 3, 1, 2, 4, 2, 2, 5];
pub(crate) const ENDPOINT_RANGE: [u8; 19] = [19, 20, 8, 7, 12, 20, 18, 12, 0, 8, 13, 13, 19, 20, 20, 20, 20, 20, 11];
pub(crate) const SUBSETS: [u8; 19] = [1, 1, 2, 3, 2, 1, 1, 2, 0, 2, 1, 1, 1, 1, 1, 1, 2, 1, 1];
pub(crate) const PLANES: [u8; 19] = [1, 1, 1, 1, 1, 1, 2, 1, 0, 1, 1, 2, 1, 2, 1, 1, 1, 2, 1];
pub(crate) const COMPS: [u8; 19] = [3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 2, 2, 2, 3];
const HAS_ETC1_BIAS: [bool; 19] = [true, true, true, true, true, true, true, true, false, true, false, false, false, true, true, true, true, true, true];
const HAS_BC1_HINT1: [bool; 19] = [true, true, true, true, true, true, true, true, false, true, false, false, false, true, true, true, true, true, true];

/// (bits, trits, quints) of a BISE range (spec "UASTC/ASTC BISE Ranges Table").
pub(crate) fn range_bits(range: u8) -> (u32, u32, u32) {
    match range {
        0 => (1, 0, 0), 1 => (0, 1, 0), 2 => (2, 0, 0), 3 => (0, 0, 1), 4 => (1, 1, 0), 5 => (3, 0, 0),
        6 => (1, 0, 1), 7 => (2, 1, 0), 8 => (4, 0, 0), 9 => (2, 0, 1), 10 => (3, 1, 0), 11 => (5, 0, 0),
        12 => (3, 0, 1), 13 => (4, 1, 0), 14 => (6, 0, 0), 15 => (4, 0, 1), 16 => (5, 1, 0), 17 => (7, 0, 0),
        18 => (5, 0, 1), 19 => (6, 1, 0), _ => (8, 0, 0),
    }
}
/// Number of quantisation levels of a BISE range.
pub(crate) fn range_levels(range: u8) -> u32 {
    let (b, t, q) = range_bits(range);
    (1 << b) * if t == 1 { 3 } else if q == 1 { 5 } else { 1 }
}

/// Dequantised [0,255] value of BISE-coded endpoint integer `v` in `range`.
pub(crate) fn dequant_endpoint(range: u8, v: u32) -> u8 {
    let table: &[u8] = match range {
        4 => &t::ENDPOINT_DEQUANT_RANGE4, 6 => &t::ENDPOINT_DEQUANT_RANGE6, 7 => &t::ENDPOINT_DEQUANT_RANGE7,
        9 => &t::ENDPOINT_DEQUANT_RANGE9, 10 => &t::ENDPOINT_DEQUANT_RANGE10, 12 => &t::ENDPOINT_DEQUANT_RANGE12,
        13 => &t::ENDPOINT_DEQUANT_RANGE13, 15 => &t::ENDPOINT_DEQUANT_RANGE15, 16 => &t::ENDPOINT_DEQUANT_RANGE16,
        18 => &t::ENDPOINT_DEQUANT_RANGE18, 19 => &t::ENDPOINT_DEQUANT_RANGE19,
        _ => {
            // Binary ranges: replicate the value's bits down to 8 bits.
            let (bits, _, _) = range_bits(range);
            let mut out = 0u32;
            let mut have = 0;
            while have < 8 {
                out = (out << bits) | v;
                have += bits;
            }
            return (out >> (have - 8)) as u8;
        }
    };
    table[v as usize]
}

/// The 6-bit ASTC weight of weight index `w` at `bits` bits per weight.
pub(crate) fn weight_value(bits: u8, w: u32) -> u32 {
    const W1: [u8; 2] = [0, 64];
    const W2: [u8; 4] = [0, 21, 43, 64];
    const W3: [u8; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
    const W4: [u8; 16] = [0, 4, 8, 12, 17, 21, 25, 29, 35, 39, 43, 47, 52, 56, 60, 64];
    const W5: [u8; 32] = [0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64];
    (match bits { 1 => W1[w as usize], 2 => W2[w as usize], 3 => W3[w as usize], 4 => W4[w as usize], _ => W5[w as usize] }) as u32
}

/// ASTC endpoint interpolation (srgb off), exactly as the specification.
#[inline]
pub(crate) fn interpolate(l: u8, h: u8, w: u32) -> u8 {
    let l = (l as u32) << 8 | l as u32;
    let h = (h as u32) << 8 | h as u32;
    (((l * (64 - w) + h * w + 32) >> 6) >> 8) as u8
}

/// Subset of texel `i` for a partitioned mode's pattern.
pub(crate) fn subset_of(mode: usize, pattern: usize, i: usize) -> usize {
    match mode {
        3 => t::PATTERNS3[pattern][i] as usize,
        7 => t::PATTERNS2_BC73[pattern][i] as usize,
        _ if SUBSETS[mode] == 2 => t::PATTERNS2[pattern][i] as usize,
        _ => 0,
    }
}
/// Anchor texel of subset `s` (its weight's MSB is implied zero).
pub(crate) fn anchor_of(mode: usize, pattern: usize, s: usize) -> usize {
    match (mode, SUBSETS[mode]) {
        (_, 1) => 0,
        (3, _) => t::PATTERN3_ANCHORS[pattern][s] as usize,
        (7, _) => t::PATTERN2_BC73_ANCHORS[pattern][s] as usize,
        _ => t::PATTERN2_ANCHORS[pattern][s] as usize,
    }
}
/// Number of partition patterns a mode can choose from.
pub(crate) fn pattern_count(mode: usize) -> usize {
    match mode { 3 => 11, 7 => 19, _ if SUBSETS[mode] == 2 => 30, _ => 1 }
}

/// Bit offsets of one mode's fields.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout {
    pub bc1_hint0: u32,
    pub bc1_hint1: Option<u32>,
    pub etc1: u32,
    pub etc1_bias: Option<u32>,
    pub etc2_alpha: Option<u32>,
    pub pattern: Option<(u32, u32)>,
    pub compsel: Option<u32>,
    pub etq: u32,
    pub ebits: u32,
    pub weights: u32,
}

pub(crate) fn layout(mode: usize) -> Layout {
    let mut pos = MODE_HUFF[mode].1 as u32;
    let bc1_hint0 = pos;
    pos += 1;
    let bc1_hint1 = HAS_BC1_HINT1[mode].then(|| { pos += 1; pos - 1 });
    let etc1 = pos;
    pos += 8;
    let etc1_bias = HAS_ETC1_BIAS[mode].then(|| { pos += 5; pos - 5 });
    let etc2_alpha = (9..=17).contains(&mode).then(|| { pos += 8; pos - 8 });
    let pattern = (SUBSETS[mode] > 1).then(|| { let bits = if mode == 3 { 4 } else { 5 }; pos += bits; (pos - bits, bits) });
    // LA dual plane (mode 17) always selects alpha: no field.
    let compsel = (PLANES[mode] == 2 && mode != 17).then(|| { pos += 2; pos - 2 });
    let etq = pos;
    pos += etq_bits(mode);
    let ebits = pos;
    let (b, _, _) = range_bits(ENDPOINT_RANGE[mode]);
    pos += b * endpoint_count(mode) as u32;
    Layout { bc1_hint0, bc1_hint1, etc1, etc1_bias, etc2_alpha, pattern, compsel, etq, ebits, weights: pos }
}

pub(crate) fn endpoint_count(mode: usize) -> usize { COMPS[mode] as usize * 2 * SUBSETS[mode] as usize }

/// Bits of the trit/quint group codes (complete groups, then a short tail).
fn etq_bits(mode: usize) -> u32 {
    let (_, trits, quints) = range_bits(ENDPOINT_RANGE[mode]);
    let n = endpoint_count(mode) as u32;
    if trits == 1 { (n / 5) * 8 + [0, 2, 4, 5, 7][(n % 5) as usize] }
    else if quints == 1 { (n / 3) * 7 + [0, 3, 5][(n % 3) as usize] }
    else { 0 }
}

pub fn weight_bits_total(mode: usize) -> u32 {
    let per = WEIGHT_BITS[mode] as u32;
    let planes = PLANES[mode] as u32;
    per * 16 * planes - SUBSETS[mode] as u32 * planes
}

fn get_bits(block: &[u8; 16], pos: u32, len: u32) -> u32 {
    let mut v = 0u32;
    for i in 0..len {
        let p = pos + i;
        v |= (((block[(p >> 3) as usize] >> (p & 7)) & 1) as u32) << i;
    }
    v
}
fn put_bits(block: &mut [u8; 16], pos: u32, len: u32, v: u32) {
    for i in 0..len {
        let p = pos + i;
        if (v >> i) & 1 == 1 { block[(p >> 3) as usize] |= 1 << (p & 7); }
    }
}

/// The mode index of a block, or `None` for the reserved code.
pub fn block_mode(block: &[u8; 16]) -> Option<usize> {
    let m = HUFF_MODES[(block[0] & 127) as usize] as usize;
    (m < 19).then_some(m)
}

/// A block's fields in plain form.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Unpacked {
    pub mode: usize,
    pub pattern: usize,
    pub compsel: usize,
    /// BISE-coded endpoint integers, in ASTC order (per subset: L0 H0 L1 H1 ...).
    pub endpoints: [u8; 18],
    /// Weight indices; dual plane modes interleave plane 0 and plane 1 per texel.
    pub weights: [u8; 32],
    pub solid: [u8; 4],
    /// Hint fields (kept so a transcoder can use them): BC1 hints, ETC1
    /// flip/diff/table indices packed as in the block, ETC1 bias, ETC2 alpha.
    pub bc1_hints: u8,
    pub etc1_hints: u8,
    pub etc1_bias: u8,
    pub etc2_alpha: u8,
}

/// Parse a block. `None` for the reserved mode.
pub fn unpack(block: &[u8; 16]) -> Option<Unpacked> {
    let mode = block_mode(block)?;
    let mut u = Unpacked { mode, ..Default::default() };
    if mode == MODE_SOLID {
        for c in 0..4 { u.solid[c] = get_bits(block, 5 + 8 * c as u32, 8) as u8; }
        return Some(u);
    }
    let l = layout(mode);
    u.bc1_hints = (get_bits(block, l.bc1_hint0, 1) | l.bc1_hint1.map_or(0, |p| get_bits(block, p, 1) << 1)) as u8;
    u.etc1_hints = get_bits(block, l.etc1, 8) as u8;
    u.etc1_bias = l.etc1_bias.map_or(13, |p| get_bits(block, p, 5)) as u8;
    u.etc2_alpha = l.etc2_alpha.map_or(0, |p| get_bits(block, p, 8)) as u8;
    if let Some((p, bits)) = l.pattern { u.pattern = get_bits(block, p, bits) as usize; if u.pattern >= pattern_count(mode) { return None; } }
    u.compsel = match l.compsel { Some(p) => get_bits(block, p, 2) as usize, None => if mode == 17 { 3 } else { 0 } };
    // Endpoints: trit/quint groups first, then each value's bits.
    let range = ENDPOINT_RANGE[mode];
    let (bits, trits, quints) = range_bits(range);
    let n = endpoint_count(mode);
    let mut tq = [0u32; 18];
    if trits == 1 || quints == 1 {
        let (group, base, full_bits, tail_bits): (usize, u32, u32, &[u32]) =
            if trits == 1 { (5, 3, 8, &[0, 2, 4, 5, 7]) } else { (3, 5, 7, &[0, 3, 5]) };
        let mut pos = l.etq;
        let mut i = 0;
        while i < n {
            let count = group.min(n - i);
            let len = if count == group { full_bits } else { tail_bits[count] };
            let mut code = get_bits(block, pos, len);
            pos += len;
            for j in 0..count { tq[i + j] = code % base; code /= base; }
            i += count;
        }
    }
    for i in 0..n {
        let b = get_bits(block, l.ebits + i as u32 * bits, bits);
        u.endpoints[i] = ((tq[i] << bits) | b) as u8;
    }
    // Weights: plain bits, anchors one bit short.
    let wb = WEIGHT_BITS[mode] as u32;
    let planes = PLANES[mode] as usize;
    let mut pos = l.weights;
    for i in 0..16 {
        let anchor = (0..SUBSETS[mode] as usize).any(|s| anchor_of(mode, u.pattern, s) == i);
        for p in 0..planes {
            let len = if anchor { wb - 1 } else { wb };
            u.weights[i * planes + p] = get_bits(block, pos, len) as u8;
            pos += len;
        }
    }
    Some(u)
}

/// Pack `u` into a block. Hint fields are written as given.
pub fn pack(u: &Unpacked) -> [u8; 16] {
    let mut block = [0u8; 16];
    let mode = u.mode;
    let (code, len) = MODE_HUFF[mode];
    put_bits(&mut block, 0, len as u32, code as u32);
    if mode == MODE_SOLID {
        for c in 0..4 { put_bits(&mut block, 5 + 8 * c as u32, 8, u.solid[c] as u32); }
        return block;
    }
    let l = layout(mode);
    put_bits(&mut block, l.bc1_hint0, 1, (u.bc1_hints & 1) as u32);
    if let Some(p) = l.bc1_hint1 { put_bits(&mut block, p, 1, ((u.bc1_hints >> 1) & 1) as u32); }
    put_bits(&mut block, l.etc1, 8, u.etc1_hints as u32);
    if let Some(p) = l.etc1_bias { put_bits(&mut block, p, 5, u.etc1_bias as u32); }
    if let Some(p) = l.etc2_alpha { put_bits(&mut block, p, 8, u.etc2_alpha as u32); }
    if let Some((p, bits)) = l.pattern { put_bits(&mut block, p, bits, u.pattern as u32); }
    if let Some(p) = l.compsel { put_bits(&mut block, p, 2, u.compsel as u32); }
    let (bits, trits, quints) = range_bits(ENDPOINT_RANGE[mode]);
    let n = endpoint_count(mode);
    if trits == 1 || quints == 1 {
        let (group, base, full_bits, tail_bits): (usize, u32, u32, &[u32]) =
            if trits == 1 { (5, 3, 8, &[0, 2, 4, 5, 7]) } else { (3, 5, 7, &[0, 3, 5]) };
        let mut pos = l.etq;
        let mut i = 0;
        while i < n {
            let count = group.min(n - i);
            let len = if count == group { full_bits } else { tail_bits[count] };
            let mut code = 0u32;
            for j in (0..count).rev() { code = code * base + ((u.endpoints[i + j] as u32) >> bits); }
            put_bits(&mut block, pos, len, code);
            pos += len;
            i += count;
        }
    }
    for i in 0..n {
        put_bits(&mut block, l.ebits + i as u32 * bits, bits, (u.endpoints[i] as u32) & ((1 << bits) - 1));
    }
    let wb = WEIGHT_BITS[mode] as u32;
    let planes = PLANES[mode] as usize;
    let mut pos = l.weights;
    for i in 0..16 {
        let anchor = (0..SUBSETS[mode] as usize).any(|s| anchor_of(mode, u.pattern, s) == i);
        for p in 0..planes {
            let len = if anchor { wb - 1 } else { wb };
            put_bits(&mut block, pos, len, u.weights[i * planes + p] as u32);
            pos += len;
        }
    }
    block
}

/// Decode an unpacked block to 16 RGBA texels (raster order).
pub fn decode_unpacked(u: &Unpacked) -> [[u8; 4]; 16] {
    if u.mode == MODE_SOLID { return [u.solid; 16]; }
    let mode = u.mode;
    let comps = COMPS[mode] as usize;
    let range = ENDPOINT_RANGE[mode];
    let wb = WEIGHT_BITS[mode];
    let planes = PLANES[mode] as usize;
    let mut out = [[0u8; 4]; 16];
    for i in 0..16 {
        let s = subset_of(mode, u.pattern, i);
        let e = &u.endpoints[s * comps * 2..];
        let w0 = weight_value(wb, u.weights[i * planes] as u32);
        let w1 = if planes == 2 { weight_value(wb, u.weights[i * planes + 1] as u32) } else { w0 };
        // Endpoint component c (in the mode's component space) interpolated.
        let comp = |c: usize, w: u32| interpolate(dequant_endpoint(range, e[c * 2] as u32), dequant_endpoint(range, e[c * 2 + 1] as u32), w);
        let pick = |ch: usize| if planes == 2 && ch == u.compsel { w1 } else { w0 };
        out[i] = match comps {
            2 => { let l = comp(0, pick(0)); [l, l, l, comp(1, pick(3))] }
            3 => [comp(0, pick(0)), comp(1, pick(1)), comp(2, pick(2)), 255],
            _ => [comp(0, pick(0)), comp(1, pick(1)), comp(2, pick(2)), comp(3, pick(3))],
        };
    }
    out
}

/// Decode one block to 16 RGBA texels. A reserved-mode block decodes to
/// opaque magenta (the ASTC error colour), as a hardware decoder would.
pub fn decode_block(block: &[u8; 16]) -> [[u8; 4]; 16] {
    match unpack(block) { Some(u) => decode_unpacked(&u), None => [[255, 0, 255, 255]; 16] }
}

/// Bytes of a UASTC image: one 16-byte block per 4x4 tile.
pub fn encoded_len(width: u32, height: u32) -> usize { ((width as usize + 3) / 4) * ((height as usize + 3) / 4) * 16 }

/// Decode a whole UASTC image to RGBA8 (row-major, `width*height*4` bytes).
pub fn decode_image(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let (w, h) = (width as usize, height as usize);
    if data.len() < encoded_len(width, height) { return Err(format!("UASTC data too short for {width}x{height}")); }
    let mut out = vec![0u8; w * h * 4];
    let bx = (w + 3) / 4;
    for (index, chunk) in data.chunks_exact(16).take(encoded_len(width, height) / 16).enumerate() {
        let texels = decode_block(chunk.try_into().unwrap());
        let (x0, y0) = ((index % bx) * 4, (index / bx) * 4);
        for y in 0..4 { for x in 0..4 {
            let (px, py) = (x0 + x, y0 + y);
            if px < w && py < h { out[(py * w + px) * 4..][..4].copy_from_slice(&texels[y * 4 + x]); }
        } }
    }
    Ok(out)
}

// ── encoder ─────────────────────────────────────────────────────────────

/// Encoder effort. More effort tries more modes, partitions and refinement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// Single-subset modes plus one RGB dual-plane mode, one refinement pass.
    Fast,
    /// Adds 2-subset modes (best few patterns) and two refinement passes.
    Default,
    /// Adds 3-subset and dual-plane modes and wider pattern search.
    Slow,
}

/// Endpoint quantiser for one BISE range: nearest coded integer per value.
struct Quantizer { to_int: [u8; 256] }
impl Quantizer {
    fn new(range: u8) -> Self {
        let levels = range_levels(range);
        let mut to_int = [0u8; 256];
        for (v, slot) in to_int.iter_mut().enumerate() {
            let mut best = (u32::MAX, 0u32);
            for i in 0..levels {
                let d = (dequant_endpoint(range, i) as i32 - v as i32).unsigned_abs();
                if d < best.0 { best = (d, i); }
            }
            *slot = best.1 as u8;
        }
        Quantizer { to_int }
    }
}
fn quantizer(range: u8) -> &'static Quantizer {
    use std::sync::OnceLock;
    static Q: [OnceLock<Quantizer>; 21] = [const { OnceLock::new() }; 21];
    Q[range as usize].get_or_init(|| Quantizer::new(range))
}

/// Texel channels a mode encodes, as indices into RGBA.
fn mode_channels(mode: usize) -> &'static [usize] {
    match COMPS[mode] { 2 => &[0, 3], 3 => &[0, 1, 2], _ => &[0, 1, 2, 3] }
}

/// Squared error of `decoded` against `texels` over RGBA.
fn block_error(texels: &[[u8; 4]; 16], decoded: &[[u8; 4]; 16]) -> u32 {
    let mut e = 0u32;
    for i in 0..16 { for c in 0..4 { let d = texels[i][c] as i32 - decoded[i][c] as i32; e += (d * d) as u32; } }
    e
}

/// One subset's endpoints along the principal axis of `points` (each `dims`
/// wide), clamped to [0,255].
fn fit_line(points: &[[f32; 4]], dims: usize) -> ([f32; 4], [f32; 4]) {
    let n = points.len().max(1) as f32;
    let mut mean = [0f32; 4];
    for p in points { for c in 0..dims { mean[c] += p[c] / n; } }
    let mut cov = [[0f32; 4]; 4];
    for p in points { for a in 0..dims { for b in 0..dims { cov[a][b] += (p[a] - mean[a]) * (p[b] - mean[b]); } } }
    // Power iteration from the largest-variance axis.
    let mut axis = [0f32; 4];
    let widest = (0..dims).max_by(|&a, &b| cov[a][a].total_cmp(&cov[b][b])).unwrap_or(0);
    axis[widest] = 1.0;
    for _ in 0..6 {
        let mut next = [0f32; 4];
        for a in 0..dims { for b in 0..dims { next[a] += cov[a][b] * axis[b]; } }
        let len = next.iter().map(|v| v * v).sum::<f32>().sqrt();
        if len < 1e-6 { break; }
        for c in 0..dims { axis[c] = next[c] / len; }
    }
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for p in points {
        let d: f32 = (0..dims).map(|c| (p[c] - mean[c]) * axis[c]).sum();
        lo = lo.min(d);
        hi = hi.max(d);
    }
    if lo > hi { lo = 0.0; hi = 0.0; }
    let mut l = [0f32; 4];
    let mut h = [0f32; 4];
    for c in 0..dims {
        l[c] = (mean[c] + axis[c] * lo).clamp(0.0, 255.0);
        h[c] = (mean[c] + axis[c] * hi).clamp(0.0, 255.0);
    }
    (l, h)
}

/// A candidate block being fitted.
struct Candidate { u: Unpacked, err: u32 }

/// Fit `mode` (with `pattern` and, for dual plane, `compsel`) to `texels`.
fn fit_mode(texels: &[[u8; 4]; 16], mode: usize, pattern: usize, compsel: usize, passes: usize) -> Candidate {
    let chans = mode_channels(mode);
    let comps = chans.len();
    let subsets = SUBSETS[mode] as usize;
    let dual = PLANES[mode] == 2;
    let planes = PLANES[mode] as usize;
    let range = ENDPOINT_RANGE[mode];
    let q = quantizer(range);
    let wb = WEIGHT_BITS[mode];
    let levels = 1u32 << wb;
    // Which mode component the second plane drives.
    let plane1_comp = if dual { chans.iter().position(|&ch| ch == compsel) } else { None };
    let mut u = Unpacked { mode, pattern, compsel, etc1_bias: 13, ..Default::default() };
    // Initial endpoints per subset (and plane).
    let mut ends = [[[0f32; 4]; 2]; 3];
    for s in 0..subsets {
        let pts: Vec<[f32; 4]> = (0..16).filter(|&i| subset_of(mode, pattern, i) == s)
            .map(|i| { let mut p = [0f32; 4]; for (k, &ch) in chans.iter().enumerate() { p[k] = texels[i][ch] as f32; } p }).collect();
        match plane1_comp {
            None => { let (l, h) = fit_line(&pts, comps); ends[s] = [l, h]; }
            Some(k) => {
                // Plane 0: the other components along their own line; plane 1: 1D range.
                let others: Vec<[f32; 4]> = pts.iter().map(|p| { let mut o = *p; o[k] = 0.0; o }).collect();
                let (l, h) = fit_line(&others, comps);
                ends[s] = [l, h];
                let (lo, hi) = pts.iter().fold((255f32, 0f32), |(a, b), p| (a.min(p[k]), b.max(p[k])));
                ends[s][0][k] = lo;
                ends[s][1][k] = hi;
            }
        }
    }
    let mut best: Option<Candidate> = None;
    for pass in 0..=passes {
        // Quantise endpoints.
        for s in 0..subsets { for k in 0..comps {
            u.endpoints[(s * comps + k) * 2] = q.to_int[ends[s][0][k].round().clamp(0.0, 255.0) as usize];
            u.endpoints[(s * comps + k) * 2 + 1] = q.to_int[ends[s][1][k].round().clamp(0.0, 255.0) as usize];
        } }
        // Best weight per texel (and plane) against the quantised endpoints.
        for i in 0..16 {
            let s = subset_of(mode, pattern, i);
            let lo: Vec<u8> = (0..comps).map(|k| dequant_endpoint(range, u.endpoints[(s * comps + k) * 2] as u32)).collect();
            let hi: Vec<u8> = (0..comps).map(|k| dequant_endpoint(range, u.endpoints[(s * comps + k) * 2 + 1] as u32)).collect();
            for p in 0..planes {
                let in_plane = |k: usize| match plane1_comp { None => true, Some(pk) => (p == 1) == (k == pk) };
                let mut bw = (u32::MAX, 0u32);
                for w in 0..levels {
                    let wv = weight_value(wb, w);
                    let mut e = 0u32;
                    for k in 0..comps {
                        if !in_plane(k) { continue; }
                        let d = interpolate(lo[k], hi[k], wv) as i32 - texels[i][chans[k]] as i32;
                        e += (d * d) as u32;
                    }
                    if e < bw.0 { bw = (e, w); }
                }
                u.weights[i * planes + p] = bw.1 as u8;
            }
        }
        let err = block_error(texels, &decode_unpacked(&u));
        if best.as_ref().map_or(true, |b| err < b.err) { best = Some(Candidate { u: u.clone(), err }); }
        if pass == passes || err == 0 { break; }
        // Least-squares endpoint refit from the chosen weights.
        for s in 0..subsets { for k in 0..comps {
            let p = match plane1_comp { Some(pk) if pk == k => 1, _ => 0 };
            let (mut a, mut b, mut c, mut x, mut y) = (0f32, 0f32, 0f32, 0f32, 0f32);
            for i in (0..16).filter(|&i| subset_of(mode, pattern, i) == s) {
                let w = weight_value(wb, u.weights[i * planes + p] as u32) as f32 / 64.0;
                let v = texels[i][chans[k]] as f32;
                a += (1.0 - w) * (1.0 - w); b += (1.0 - w) * w; c += w * w;
                x += (1.0 - w) * v; y += w * v;
            }
            let det = a * c - b * b;
            if det.abs() > 1e-6 {
                ends[s][0][k] = ((c * x - b * y) / det).clamp(0.0, 255.0);
                ends[s][1][k] = ((a * y - b * x) / det).clamp(0.0, 255.0);
            }
        } }
    }
    let mut cand = best.unwrap();
    enforce_anchors(&mut cand.u);
    cand
}

/// Swap endpoints (and invert weights) so every anchor weight's MSB is 0.
/// Exact: the weight tables and interpolation are symmetric.
fn enforce_anchors(u: &mut Unpacked) {
    let mode = u.mode;
    let comps = COMPS[mode] as usize;
    let planes = PLANES[mode] as usize;
    let wb = WEIGHT_BITS[mode];
    let top = (1u8 << wb) - 1;
    let half = 1u8 << (wb - 1);
    let chans = mode_channels(mode);
    let plane1_comp = if planes == 2 { chans.iter().position(|&ch| ch == u.compsel) } else { None };
    for s in 0..SUBSETS[mode] as usize {
        let a = anchor_of(mode, u.pattern, s);
        for p in 0..planes {
            if u.weights[a * planes + p] & half == 0 { continue; }
            for k in 0..comps {
                let on_plane = match plane1_comp { None => true, Some(pk) => (p == 1) == (k == pk) };
                if on_plane { u.endpoints.swap((s * comps + k) * 2, (s * comps + k) * 2 + 1); }
            }
            for i in (0..16).filter(|&i| subset_of(mode, u.pattern, i) == s) { u.weights[i * planes + p] = top - u.weights[i * planes + p]; }
        }
    }
}

/// Encode one 4x4 block of RGBA texels.
pub fn encode_block(texels: &[[u8; 4]; 16], quality: Quality) -> [u8; 16] {
    if texels.iter().all(|t| *t == texels[0]) {
        return pack(&Unpacked { mode: MODE_SOLID, solid: texels[0], ..Default::default() });
    }
    let opaque = texels.iter().all(|t| t[3] == 255);
    let grey = texels.iter().all(|t| t[0] == t[1] && t[1] == t[2]);
    let passes = match quality { Quality::Fast => 1, Quality::Default => 2, Quality::Slow => 3 };
    let mut singles: Vec<(usize, usize)> = Vec::new();
    let mut partitioned: Vec<usize> = Vec::new();
    if opaque {
        singles.extend([(0, 0), (5, 0), (18, 0), (1, 0)]);
        if quality != Quality::Fast { partitioned.extend([2, 4]); }
        if quality == Quality::Slow { partitioned.extend([3, 7]); singles.extend([(6, 0), (6, 1), (6, 2)]); }
    } else {
        if grey { singles.extend([(15, 0), (17, 3)]); if quality != Quality::Fast { partitioned.push(16); } }
        singles.extend([(10, 0), (12, 0), (14, 0)]);
        if quality != Quality::Fast { singles.extend([(13, 3), (11, 3)]); partitioned.push(9); }
        if quality == Quality::Slow { singles.extend([(13, 0), (13, 1), (13, 2), (11, 0), (11, 1), (11, 2)]); }
    }
    let mut best: Option<Candidate> = None;
    let mut consider = |c: Candidate| if best.as_ref().map_or(true, |b| c.err < b.err) { best = Some(c) };
    for &(mode, compsel) in &singles { consider(fit_mode(texels, mode, 0, compsel, passes)); }
    // One RGB dual-plane candidate (a channel that varies apart from the
    // others, e.g. crossed gradients), its plane chosen by a cheap fit. Slow
    // tries all three planes above.
    if opaque && quality != Quality::Slow {
        let compsel = (0..3).min_by_key(|&c| fit_mode(texels, 6, 0, c, 0).err).unwrap_or(0);
        consider(fit_mode(texels, 6, 0, compsel, passes));
    }
    for &mode in &partitioned {
        // Rank patterns by a cheap fit, then refine the best few.
        let mut ranked: Vec<(u32, usize)> = (0..pattern_count(mode)).map(|p| (fit_mode(texels, mode, p, 0, 0).err, p)).collect();
        ranked.sort_unstable();
        let keep = match quality { Quality::Slow => 4, _ => 2 };
        for &(_, p) in ranked.iter().take(keep) { consider(fit_mode(texels, mode, p, 0, passes)); }
    }
    pack(&best.unwrap().u)
}

/// Encode an RGBA8 image (row-major, `width*height*4` bytes) to UASTC,
/// across `threads` workers (0 = all cores). Edge tiles clamp.
pub fn encode_image(rgba: &[u8], width: u32, height: u32, quality: Quality, threads: usize) -> Result<Vec<u8>, String> {
    let (w, h) = (width as usize, height as usize);
    if rgba.len() < w * h * 4 { return Err(format!("RGBA data too short for {width}x{height}")); }
    if w == 0 || h == 0 { return Ok(Vec::new()); }
    let (bx, by) = ((w + 3) / 4, (h + 3) / 4);
    let mut out = vec![0u8; bx * by * 16];
    let threads = if threads == 0 { std::thread::available_parallelism().map_or(1, |n| n.get()) } else { threads }.clamp(1, by);
    let rows_per = (by + threads - 1) / threads;
    std::thread::scope(|scope| {
        for (chunk, rows) in out.chunks_mut(rows_per * bx * 16).enumerate() {
            scope.spawn(move || {
                for (j, block) in rows.chunks_exact_mut(16).enumerate() {
                    let index = chunk * rows_per * bx + j;
                    let (x0, y0) = ((index % bx) * 4, (index / bx) * 4);
                    let mut texels = [[0u8; 4]; 16];
                    for y in 0..4 { for x in 0..4 {
                        let (px, py) = ((x0 + x).min(w - 1), (y0 + y).min(h - 1));
                        texels[y * 4 + x].copy_from_slice(&rgba[(py * w + px) * 4..][..4]);
                    } }
                    block.copy_from_slice(&encode_block(&texels, quality));
                }
            });
        }
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_the_specification_offsets() {
        // WEIGHTS bit offset and total bits per mode, from "List of UASTC Modes".
        let spec: [(usize, u32, u32); 18] = [(0, 65, 128), (1, 69, 100), (2, 73, 119), (3, 89, 118), (4, 89, 119), (5, 68, 115), (6, 66, 128), (7, 89, 119),
            (9, 97, 127), (10, 65, 128), (11, 66, 128), (12, 81, 128), (13, 94, 124), (14, 92, 123), (15, 62, 125), (16, 98, 128), (17, 61, 123), (18, 49, 128)];
        for (mode, weights, total) in spec {
            let l = layout(mode);
            assert_eq!(l.weights, weights, "mode {mode} weights offset");
            assert_eq!(l.weights + weight_bits_total(mode), total, "mode {mode} total bits");
        }
        // A few inner fields.
        assert_eq!(layout(0).etq, 19);
        assert_eq!(layout(0).ebits, 29);
        assert_eq!(layout(3).pattern, Some((20, 4)));
        assert_eq!(layout(3).ebits, 53);
        assert_eq!(layout(6).compsel, Some(20));
        assert_eq!(layout(11).compsel, Some(19));
        assert_eq!(layout(13).compsel, Some(28));
        assert_eq!(layout(16).pattern, Some((29, 5)));
    }

    #[test]
    fn mode_codes_decode_through_the_acceleration_table() {
        for mode in 0..19 {
            let mut block = [0u8; 16];
            put_bits(&mut block, 0, MODE_HUFF[mode].1 as u32, MODE_HUFF[mode].0 as u32);
            assert_eq!(block_mode(&block), Some(mode));
        }
    }

    #[test]
    fn dequantisation_is_monotonic_in_value_and_spans_the_range() {
        for range in [7u8, 8, 11, 12, 13, 18, 19, 20] {
            let levels = range_levels(range);
            let mut values: Vec<u8> = (0..levels).map(|i| dequant_endpoint(range, i)).collect();
            values.sort_unstable();
            values.dedup();
            assert_eq!(values.len() as u32, levels, "range {range} values distinct");
            assert_eq!((values[0], *values.last().unwrap()), (0, 255));
        }
    }

    #[test]
    fn pack_unpack_round_trips_every_mode() {
        let mut seed = 0x1234_5678u32;
        let mut rnd = |n: u32| { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); (seed >> 8) % n.max(1) };
        for mode in (0..19).filter(|&m| m != MODE_SOLID) {
            for _ in 0..50 {
                let mut u = Unpacked { mode, pattern: rnd(pattern_count(mode) as u32) as usize, etc1_bias: 13, ..Default::default() };
                u.compsel = if PLANES[mode] == 2 { if mode == 17 { 3 } else { rnd(4) as usize } } else { 0 };
                for i in 0..endpoint_count(mode) { u.endpoints[i] = rnd(range_levels(ENDPOINT_RANGE[mode])) as u8; }
                let planes = PLANES[mode] as usize;
                for i in 0..16 { for p in 0..planes { u.weights[i * planes + p] = rnd(1 << WEIGHT_BITS[mode]) as u8; } }
                enforce_anchors(&mut u);
                let back = unpack(&pack(&u)).unwrap();
                assert_eq!(back.endpoints, u.endpoints, "mode {mode}");
                assert_eq!(back.weights, u.weights, "mode {mode}");
                assert_eq!((back.pattern, back.compsel), (u.pattern, u.compsel));
            }
        }
    }
}
