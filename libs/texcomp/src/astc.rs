//! ASTC 4x4 LDR blocks, written from the Khronos Data Format Specification
//! 1.3, chapter 23: the subset UASTC transcodes to (a 4x4 weight grid, 1-3
//! partitions sharing one direct colour endpoint mode, optional dual plane,
//! void-extent) — packing and a decoder of that subset for validation.

use crate::uastc;

/// One ASTC 4x4 block in plain form.
#[derive(Clone, Debug, PartialEq)]
pub struct AstcBlock {
    /// 1..=3 partitions; the pattern comes from `seed` (10 bits).
    pub partitions: u8,
    pub seed: u16,
    /// Colour endpoint mode shared by all partitions: 4 (LA), 8 (RGB) or 12 (RGBA), direct.
    pub cem: u8,
    /// Dual plane: the component on the second plane (Color Component Selector).
    pub ccs: Option<u8>,
    /// Weight levels: 2, 4, 8, 16 or 32 (plain bits).
    pub weight_levels: u8,
    /// Endpoint BISE range index (same numbering as UASTC).
    pub endpoint_range: u8,
    /// Endpoint integers, ASTC order (per partition v0..vN).
    pub endpoints: Vec<u8>,
    /// Weights in raster order; dual plane interleaves planes per texel.
    pub weights: [u8; 32],
    /// Solid colour (void-extent) instead of all of the above.
    pub solid: Option<[u8; 4]>,
}

fn values_per_partition(cem: u8) -> usize { ((cem >> 2) as usize + 1) * 2 }

/// Bits of an ISE sequence of `count` values in `range`.
pub fn ise_bits(range: u8, count: u32) -> u32 {
    let (bits, trits, quints) = uastc::range_bits(range);
    (count * 8 * trits).div_ceil(5) + (count * 7 * quints).div_ceil(3) + count * bits
}

/// The endpoint range ASTC implies for a configuration: the largest whose
/// encoding fits the bits left (spec 23.22).
pub fn implied_endpoint_range(partitions: u8, dual: bool, cem: u8, weight_bits: u32) -> Option<u8> {
    let mut config = if partitions > 1 { 29 } else { 17 };
    if dual { config += 2; }
    let remaining = 128i32 - config - weight_bits as i32;
    let values = values_per_partition(cem) as u32 * partitions as u32;
    (0..=20u8).rev().find(|&r| ise_bits(r, values) as i32 <= remaining)
}

// ── ISE ─────────────────────────────────────────────────────────────────

fn decode_trits(t: u32) -> [u32; 5] {
    let bit = |v: u32, i: u32| (v >> i) & 1;
    let (c, t4, t3);
    if (t >> 2) & 7 == 7 {
        c = ((t >> 5) & 7) << 2 | (t & 3);
        t4 = 2; t3 = 2;
    } else {
        c = t & 0x1f;
        if (t >> 5) & 3 == 3 { t4 = 2; t3 = bit(t, 7); } else { t4 = bit(t, 7); t3 = (t >> 5) & 3; }
    }
    let (t2, t1, t0);
    if c & 3 == 3 { t2 = 2; t1 = bit(c, 4); t0 = bit(c, 3) << 1 | (bit(c, 2) & !bit(c, 3) & 1); }
    else if (c >> 2) & 3 == 3 { t2 = 2; t1 = 2; t0 = c & 3; }
    else { t2 = bit(c, 4); t1 = (c >> 2) & 3; t0 = bit(c, 1) << 1 | (bit(c, 0) & !bit(c, 1) & 1); }
    [t0, t1, t2, t3, t4]
}
fn decode_quints(q: u32) -> [u32; 3] {
    let bit = |v: u32, i: u32| (v >> i) & 1;
    if (q >> 1) & 3 == 3 && (q >> 5) & 3 == 0 {
        let q2 = bit(q, 0) << 2 | (bit(q, 4) & !bit(q, 0) & 1) << 1 | (bit(q, 3) & !bit(q, 0) & 1);
        return [4, 4, q2];
    }
    let (q2, c);
    if (q >> 1) & 3 == 3 { q2 = 4; c = ((q >> 3) & 3) << 3 | (!(q >> 5) & 3) << 1 | (q & 1); }
    else { q2 = (q >> 5) & 3; c = q & 0x1f; }
    let (q1, q0) = if c & 7 == 5 { (4, (c >> 3) & 3) } else { ((c >> 3) & 3, c & 7) };
    [q0, q1, q2]
}
/// Encoding tables: packed code per trit/quint tuple (first decode hit).
fn trit_codes() -> &'static [u8; 243] {
    static T: std::sync::OnceLock<[u8; 243]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = [255u8; 243];
        for code in 0..256u32 {
            let v = decode_trits(code);
            let i = (v[0] + v[1] * 3 + v[2] * 9 + v[3] * 27 + v[4] * 81) as usize;
            if t[i] == 255 { t[i] = code as u8; }
        }
        t
    })
}
fn quint_codes() -> &'static [u8; 125] {
    static T: std::sync::OnceLock<[u8; 125]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = [255u8; 125];
        for code in 0..128u32 {
            let v = decode_quints(code);
            let i = (v[0] + v[1] * 5 + v[2] * 25) as usize;
            if t[i] == 255 { t[i] = code as u8; }
        }
        t
    })
}

struct BitWriter<'a> { block: &'a mut [u8; 16], pos: u32 }
impl BitWriter<'_> {
    fn put(&mut self, n: u32, v: u32) {
        for i in 0..n { let p = self.pos + i; if (v >> i) & 1 == 1 { self.block[(p >> 3) as usize] |= 1 << (p & 7); } }
        self.pos += n;
    }
}
fn get(block: &[u8; 16], pos: u32, n: u32) -> u32 {
    let mut v = 0;
    for i in 0..n { let p = pos + i; if p < 128 { v |= (((block[(p >> 3) as usize] >> (p & 7)) & 1) as u32) << i; } }
    v
}

/// Append an ISE sequence of `values` in `range` at `w.pos`.
fn ise_encode(w: &mut BitWriter, range: u8, values: &[u8]) {
    let (n, trits, quints) = uastc::range_bits(range);
    let m = |v: u8| v as u32 & ((1 << n) - 1);
    if trits == 1 {
        for chunk in values.chunks(5) {
            let mut t = [0u32; 5];
            let mut ms = [0u32; 5];
            for (i, &v) in chunk.iter().enumerate() { t[i] = v as u32 >> n; ms[i] = m(v); }
            let code = trit_codes()[(t[0] + t[1] * 3 + t[2] * 9 + t[3] * 27 + t[4] * 81) as usize] as u32;
            // m0 T1:0 m1 T3:2 m2 T4 m3 T6:5 m4 T7, truncated for a short tail.
            let fields: [(u32, u32); 10] = [(n, ms[0]), (2, code & 3), (n, ms[1]), (2, (code >> 2) & 3), (n, ms[2]), (1, (code >> 4) & 1), (n, ms[3]), (2, (code >> 5) & 3), (n, ms[4]), (1, (code >> 7) & 1)];
            let total = (chunk.len() as u32 * 8).div_ceil(5) + chunk.len() as u32 * n;
            let mut written = 0;
            for (len, v) in fields { let len = len.min(total - written); w.put(len, v); written += len; if written == total { break; } }
        }
    } else if quints == 1 {
        for chunk in values.chunks(3) {
            let mut q = [0u32; 3];
            let mut ms = [0u32; 3];
            for (i, &v) in chunk.iter().enumerate() { q[i] = v as u32 >> n; ms[i] = m(v); }
            let code = quint_codes()[(q[0] + q[1] * 5 + q[2] * 25) as usize] as u32;
            // m0 Q2:0 m1 Q4:3 m2 Q6:5
            let fields: [(u32, u32); 6] = [(n, ms[0]), (3, code & 7), (n, ms[1]), (2, (code >> 3) & 3), (n, ms[2]), (2, (code >> 5) & 3)];
            let total = (chunk.len() as u32 * 7).div_ceil(3) + chunk.len() as u32 * n;
            let mut written = 0;
            for (len, v) in fields { let len = len.min(total - written); w.put(len, v); written += len; if written == total { break; } }
        }
    } else {
        for &v in values { w.put(n, v as u32); }
    }
}

/// Read `count` ISE values in `range` from bit `pos` (bits past 127 read as 0).
fn ise_decode(block: &[u8; 16], pos: u32, range: u8, count: usize) -> Vec<u8> {
    let (n, trits, quints) = uastc::range_bits(range);
    let mut out = Vec::with_capacity(count);
    let mut p = pos;
    let mut take = |len: u32| { let v = get(block, p, len); p += len; v };
    if trits == 1 {
        while out.len() < count {
            let (m0, t10, m1, t32, m2, t4, m3, t65, m4, t7) = (take(n), take(2), take(n), take(2), take(n), take(1), take(n), take(2), take(n), take(1));
            let t = decode_trits(t10 | t32 << 2 | t4 << 4 | t65 << 5 | t7 << 7);
            for (i, m) in [m0, m1, m2, m3, m4].into_iter().enumerate() { if out.len() < count { out.push((t[i] << n | m) as u8); } }
        }
    } else if quints == 1 {
        while out.len() < count {
            let (m0, q20, m1, q43, m2, q65) = (take(n), take(3), take(n), take(2), take(n), take(2));
            let q = decode_quints(q20 | q43 << 3 | q65 << 5);
            for (i, m) in [m0, m1, m2].into_iter().enumerate() { if out.len() < count { out.push((q[i] << n | m) as u8); } }
        }
    } else {
        for _ in 0..count { out.push(take(n) as u8); }
    }
    out
}

// ── partition pattern (spec 23.21) ──────────────────────────────────────

fn hash52(mut p: u32) -> u32 {
    p ^= p >> 15; p = p.wrapping_sub(p << 17); p = p.wrapping_add(p << 7); p = p.wrapping_add(p << 4);
    p ^= p >> 5; p = p.wrapping_add(p << 16); p ^= p >> 7; p ^= p >> 3;
    p ^= p << 6; p ^= p >> 17;
    p
}

/// Partition of texel (x, y) in a 4x4 block (small_block).
pub fn select_partition(seed: u32, x: u32, y: u32, count: u32) -> u32 {
    if count <= 1 { return 0; }
    let (x, y, z) = (x << 1, y << 1, 0u32);
    let seed = seed + (count - 1) * 1024;
    let rnum = hash52(seed);
    let mut s = [rnum & 0xF, (rnum >> 4) & 0xF, (rnum >> 8) & 0xF, (rnum >> 12) & 0xF, (rnum >> 16) & 0xF, (rnum >> 20) & 0xF,
        (rnum >> 24) & 0xF, (rnum >> 28) & 0xF, (rnum >> 18) & 0xF, (rnum >> 22) & 0xF, (rnum >> 26) & 0xF, ((rnum >> 30) | (rnum << 2)) & 0xF];
    for v in &mut s { *v *= *v; }
    let (sh1, sh2) = if seed & 1 == 1 { (if seed & 2 != 0 { 4 } else { 5 }, if count == 3 { 6 } else { 5 }) }
        else { (if count == 3 { 6 } else { 5 }, if seed & 2 != 0 { 4 } else { 5 }) };
    let sh3 = if seed & 0x10 != 0 { sh1 } else { sh2 };
    for (i, v) in s.iter_mut().enumerate() { *v >>= match i { 0 | 2 | 4 | 6 => sh1, 1 | 3 | 5 | 7 => sh2, _ => sh3 }; }
    let a = (s[0] * x + s[1] * y + s[10] * z + (rnum >> 14)) & 0x3F;
    let b = (s[2] * x + s[3] * y + s[11] * z + (rnum >> 10)) & 0x3F;
    let mut c = (s[4] * x + s[5] * y + s[8] * z + (rnum >> 6)) & 0x3F;
    if count < 3 { c = 0; }
    if a >= b && a >= c { 0 } else if b >= c { 1 } else { 2 }
}

// ── block pack / decode ─────────────────────────────────────────────────

fn weight_encoding(levels: u8) -> (u32, u32) {
    // (P, rho) for plain-bit weight ranges (Table 146).
    match levels { 2 => (0, 2), 4 => (0, 4), 8 => (0, 7), 16 => (1, 4), _ => (1, 7) }
}
fn weight_bits(levels: u8) -> u32 { levels.trailing_zeros() }

/// Pack a block.
pub fn pack(b: &AstcBlock) -> [u8; 16] {
    let mut block = [0u8; 16];
    if let Some(c) = b.solid {
        let mut w = BitWriter { block: &mut block, pos: 0 };
        w.put(12, 0xDFC); // void-extent, LDR
        // Extent coordinates all ones: no extent information.
        for i in 12..64u32 { block[(i >> 3) as usize] |= 1 << (i & 7); }
        for (k, v) in c.iter().enumerate() {
            let v16 = *v as u16 * 257;
            block[8 + k * 2] = v16 as u8;
            block[9 + k * 2] = (v16 >> 8) as u8;
        }
        return block;
    }
    let dual = b.ccs.is_some();
    let (p, rho) = weight_encoding(b.weight_levels);
    // Block mode, 4x4 grid: DP P W(0) H(2) rho0 0 0 rho2 rho1.
    let mode = (dual as u32) << 10 | p << 9 | 0 << 7 | 2 << 5 | (rho & 1) << 4 | (rho >> 2 & 1) << 1 | (rho >> 1 & 1);
    let mut w = BitWriter { block: &mut block, pos: 0 };
    w.put(11, mode);
    w.put(2, b.partitions as u32 - 1);
    if b.partitions == 1 { w.put(4, b.cem as u32); } else { w.put(10, b.seed as u32); w.put(6, (b.cem as u32) << 2); }
    ise_encode(&mut w, b.endpoint_range, &b.endpoints);
    // Weights: plain bits, bit-reversed from the top of the block down.
    let wb = weight_bits(b.weight_levels);
    let count = if dual { 32 } else { 16 };
    let mut pos = 0u32;
    for i in 0..count {
        let v = b.weights[i] as u32;
        for k in 0..wb { if (v >> k) & 1 == 1 { let bit = 127 - (pos + k); block[(bit >> 3) as usize] |= 1 << (bit & 7); } }
        pos += wb;
    }
    if let Some(ccs) = b.ccs {
        let at = 128 - count as u32 * wb - 2;
        for k in 0..2 { if (ccs >> k) & 1 == 1 { let bit = at + k; block[(bit >> 3) as usize] |= 1 << (bit & 7); } }
    }
    block
}

/// Decode a block of the subset this module produces to RGBA8 texels.
/// `None` for anything outside that subset.
pub fn decode_block(block: &[u8; 16]) -> Option<[[u8; 4]; 16]> {
    let head = get(block, 0, 12);
    if head & 0x1FF == 0x1FC {
        let c = |k: usize| (u16::from_le_bytes([block[8 + k * 2], block[9 + k * 2]]) >> 8) as u8;
        return Some([[c(0), c(1), c(2), c(3)]; 16]);
    }
    let mode = get(block, 0, 11);
    if mode & 3 == 0 || (mode >> 2) & 3 != 0 || (mode >> 7) & 3 != 0 || (mode >> 5) & 3 != 2 { return None; }
    let dual = (mode >> 10) & 1 == 1;
    let p = (mode >> 9) & 1;
    let rho = (mode >> 4) & 1 | (mode & 1) << 1 | ((mode >> 1) & 1) << 2;
    let levels = match (p, rho) { (0, 2) => 2, (0, 4) => 4, (0, 7) => 8, (1, 4) => 16, (1, 7) => 32, _ => return None };
    let partitions = get(block, 11, 2) + 1;
    let (seed, cem, start) = if partitions == 1 { (0, get(block, 13, 4), 17) } else {
        let c = get(block, 23, 6);
        if c & 3 != 0 { return None; }
        (get(block, 13, 10), c >> 2, 29)
    };
    if !matches!(cem, 4 | 8 | 12) || partitions > 3 { return None; }
    let wb = weight_bits(levels);
    let count = if dual { 32 } else { 16 };
    let range = implied_endpoint_range(partitions as u8, dual, cem as u8, count as u32 * wb)?;
    let per = values_per_partition(cem as u8);
    let values = ise_decode(block, start, range, per * partitions as usize);
    let mut weights = [0u32; 32];
    for i in 0..count {
        let mut v = 0;
        for k in 0..wb { let bit = 127 - (i as u32 * wb + k); v |= (((block[(bit >> 3) as usize] >> (bit & 7)) & 1) as u32) << k; }
        weights[i] = uastc::weight_value(wb as u8, v);
    }
    let ccs = if dual { get(block, 128 - count as u32 * wb - 2, 2) as usize } else { 4 };
    let mut out = [[0u8; 4]; 16];
    for i in 0..16 {
        let part = select_partition(seed, (i % 4) as u32, (i / 4) as u32, partitions) as usize;
        let v: Vec<u8> = values[part * per..(part + 1) * per].iter().map(|&x| uastc::dequant_endpoint(range, x as u32)).collect();
        let (e0, e1): ([u8; 4], [u8; 4]) = match cem {
            4 => ([v[0], v[0], v[0], v[2]], [v[1], v[1], v[1], v[3]]),
            _ => {
                let a = |k: usize| if cem == 12 { v[k] } else { 255 };
                let (s0, s1) = (v[0] as u32 + v[2] as u32 + v[4] as u32, v[1] as u32 + v[3] as u32 + v[5] as u32);
                if s1 >= s0 { ([v[0], v[2], v[4], a(6)], [v[1], v[3], v[5], a(7)]) }
                else {
                    let bc = |r: u8, g: u8, b: u8, al: u8| [((r as u32 + b as u32) >> 1) as u8, ((g as u32 + b as u32) >> 1) as u8, b, al];
                    (bc(v[1], v[3], v[5], a(7)), bc(v[0], v[2], v[4], a(6)))
                }
            }
        };
        for c in 0..4 {
            let w = if dual && c == ccs { weights[i * 2 + 1] } else if dual { weights[i * 2] } else { weights[i] };
            out[i][c] = uastc::interpolate(e0[c], e1[c], w);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uastc_tables as ut;

    #[test]
    fn partition_hash_reproduces_the_uastc_pattern_tables() {
        for (i, (_, seed, _)) in ut::PARTITIONS2.iter().enumerate() {
            let p: Vec<u8> = (0..16).map(|t| select_partition(*seed as u32, t % 4, t / 4, 2) as u8).collect();
            assert_eq!(p, ut::PATTERNS2[i], "2-subset seed {seed}");
        }
        for (i, (_, seed, _)) in ut::PARTITIONS3.iter().enumerate() {
            let p: Vec<u8> = (0..16).map(|t| select_partition(*seed as u32, t % 4, t / 4, 3) as u8).collect();
            assert_eq!(p, ut::PATTERNS3[i], "3-subset seed {seed}");
        }
        for (i, (_, seed, _)) in ut::PARTITIONS2_BC73.iter().enumerate() {
            let p: Vec<u8> = (0..16).map(|t| select_partition(*seed as u32, t % 4, t / 4, 2) as u8).collect();
            assert_eq!(p, ut::PATTERNS2_BC73[i], "mode 7 seed {seed}");
        }
    }

    #[test]
    fn astc_implies_the_uastc_endpoint_range_for_every_mode() {
        for mode in (0..19).filter(|&m| m != uastc::MODE_SOLID) {
            let cem = match uastc::COMPS[mode] { 2 => 4, 3 => 8, _ => 12 };
            let planes = uastc::PLANES[mode] as u32;
            let wb = uastc::WEIGHT_BITS[mode] as u32 * 16 * planes;
            assert_eq!(implied_endpoint_range(uastc::SUBSETS[mode], planes == 2, cem, wb), Some(uastc::ENDPOINT_RANGE[mode]), "mode {mode}");
        }
    }

    #[test]
    fn ise_round_trips_every_range() {
        let mut seed = 99u32;
        for range in 0..=20u8 {
            let levels = uastc::range_levels(range);
            for count in (1..=18usize).filter(|&c| ise_bits(range, c as u32) <= 125) {
                let values: Vec<u8> = (0..count).map(|_| { seed = seed.wrapping_mul(747796405).wrapping_add(2891336453); ((seed >> 9) % levels) as u8 }).collect();
                let mut block = [0u8; 16];
                let mut w = BitWriter { block: &mut block, pos: 3 };
                ise_encode(&mut w, range, &values);
                assert_eq!(w.pos - 3, ise_bits(range, count as u32), "range {range} count {count} size");
                assert_eq!(ise_decode(&block, 3, range, count), values, "range {range} count {count}");
            }
        }
    }
}
