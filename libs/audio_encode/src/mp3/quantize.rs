//! Quantization and Huffman coding of one granule of one channel: the
//! standard's two nested loops (Annex C.1.5.4). The inner loop finds the
//! smallest global gain whose coded size fits the bit budget; the outer loop
//! amplifies the scalefactors of the bands whose quantization noise exceeds
//! what the psychoacoustic model allows, and keeps the best result seen.
//!
//! The Huffman code tables are derived from the decoder's code trees
//! (Table 3-B.7), so encoder and decoder cannot disagree about a codeword.

use makepad_audio_decode::mp3::tables::{HuffNode, HUFF_TABLES, HUFF_TREES, LEAF};

use super::analysis::{block, LINES};
use super::bands::Bands;
use super::bits::BitWriter;

/// Largest quantized magnitude: 15 plus a 13-bit escape.
pub const MAX_IX: u32 = 15 + 8191;
/// part2_3_length is a 12-bit field.
pub const MAX_GRANULE_BITS: u32 = 4095;
/// Rounding offset of the standard's quantizer, `nint(x - 0.0946)`.
const ROUND: f32 = 0.4054;
const OUTER_ITERATIONS: usize = 24;

fn tree_xlen(tree: usize) -> u32 {
    match tree {
        0 => 2,
        1 | 2 => 3,
        3 | 4 => 4,
        5..=7 => 6,
        8..=10 => 8,
        _ => 16,
    }
}

/// The pair tables worth trying for a region whose largest value is `max`.
fn candidates(max: u32) -> &'static [u8] {
    match max {
        0 => &[0],
        1 => &[1],
        2 => &[2, 3],
        3 => &[5, 6],
        4 | 5 => &[7, 8, 9],
        6 | 7 => &[10, 11, 12],
        8..=15 => &[13, 15],
        16 => &[16, 24],
        17 | 18 => &[17, 24],
        19..=22 => &[18, 24],
        23..=30 => &[19, 24],
        31..=46 => &[20, 25],
        47..=78 => &[20, 26],
        79..=142 => &[21, 27],
        143..=270 => &[21, 28],
        271..=526 => &[22, 29],
        527..=1038 => &[22, 30],
        1039..=2062 => &[23, 30],
        _ => &[23, 31],
    }
}

pub struct Huffman {
    code: Box<[[u32; 256]; 17]>,
    len: Box<[[u8; 256]; 17]>,
    pow43: Vec<f32>,
}

/// The coded shape of a granule's spectrum.
#[derive(Clone, Copy, Debug, Default)]
pub struct Coding {
    pub big_values: u32,
    pub count1_end: usize,
    pub table_select: [u8; 3],
    pub region0_count: u8,
    pub region1_count: u8,
    pub count1_select: bool,
    /// Huffman bits including sign and escape bits.
    pub bits: u32,
}

/// One coded granule of one channel.
#[derive(Clone)]
pub struct Granule {
    pub block_type: u8,
    pub global_gain: u32,
    pub scalefac_compress: u32,
    pub part2_3_length: u32,
    pub coding: Coding,
    pub sf: [u8; 39],
    pub ix: [i32; LINES],
}

impl Granule {
    pub fn silent(block_type: u8) -> Self {
        Granule {
            block_type,
            global_gain: 0,
            scalefac_compress: 0,
            part2_3_length: 0,
            coding: Coding::default(),
            sf: [0; 39],
            ix: [0; LINES],
        }
    }
}

fn walk(tree: &[HuffNode], node: usize, code: u32, len: u8, codes: &mut [u32; 256], lens: &mut [u8; 256]) {
    for bit in 0..2u32 {
        let child = tree[node][bit as usize];
        let c = (code << 1) | bit;
        if child & LEAF != 0 {
            let v = (child & !LEAF) as usize & 0xff;
            codes[v] = c;
            lens[v] = len + 1;
        } else {
            walk(tree, child as usize, c, len + 1, codes, lens);
        }
    }
}

impl Default for Huffman {
    fn default() -> Self {
        Self::new()
    }
}

impl Huffman {
    pub fn new() -> Self {
        let mut code = Box::new([[0u32; 256]; 17]);
        let mut len = Box::new([[0u8; 256]; 17]);
        for (t, tree) in HUFF_TREES.iter().enumerate() {
            walk(tree, 0, 0, 0, &mut code[t], &mut len[t]);
        }
        let pow43 = (0..=MAX_IX).map(|i| (i as f64).powf(4.0 / 3.0) as f32).collect();
        Huffman { code, len, pow43 }
    }

    #[inline]
    fn pair_len(&self, tree: usize, linbits: u32, x: u32, y: u32) -> u32 {
        if linbits > 0 {
            let (xx, yy) = (x.min(15), y.min(15));
            let mut n = self.len[tree][(xx * 16 + yy) as usize] as u32;
            if x >= 15 {
                n += linbits;
            }
            if y >= 15 {
                n += linbits;
            }
            n
        } else {
            self.len[tree][(x * 16 + y) as usize] as u32
        }
    }

    /// Bits (without signs) and the table for lines `a..b` coded as pairs.
    fn region(&self, q: &[u16; LINES], a: usize, b: usize) -> (u8, u32) {
        if a >= b {
            return (0, 0);
        }
        let max = q[a..b].iter().copied().max().unwrap_or(0) as u32;
        let mut best = (0u8, u32::MAX);
        for &t in candidates(max) {
            let Some((tree, linbits)) = HUFF_TABLES[t as usize] else {
                return (0, 0);
            };
            let (tree, linbits) = (tree as usize, linbits as u32);
            let mut bits = 0u32;
            let mut i = a;
            while i < b {
                bits += self.pair_len(tree, linbits, q[i] as u32, q[i + 1] as u32);
                i += 2;
            }
            if bits < best.1 {
                best = (t, bits);
            }
        }
        best
    }

    /// The coding of quantized magnitudes `q`: count1 and big-value regions,
    /// tables and the region split. `optimal` searches every long-block split;
    /// otherwise a fixed split by the big-value extent is used (never better
    /// than the search, so a size measured that way is an upper bound).
    pub fn coding(&self, q: &[u16; LINES], block_type: u8, sfb_long: &[u16; 23], optimal: bool) -> Coding {
        let mut end = LINES;
        while end >= 2 && q[end - 1] == 0 && q[end - 2] == 0 {
            end -= 2;
        }
        let mut big = end;
        while big >= 4 && q[big - 1] <= 1 && q[big - 2] <= 1 && q[big - 3] <= 1 && q[big - 4] <= 1 {
            big -= 4;
        }
        let mut c = Coding { big_values: (big / 2) as u32, count1_end: end, ..Coding::default() };
        // count1 quadruples: table A (tree 15) or the fixed-length table B
        let (mut a_bits, mut b_bits, mut signs) = (0u32, 0u32, 0u32);
        let mut i = big;
        while i < end {
            let mask = ((q[i] as usize) << 3) | ((q[i + 1] as usize) << 2) | ((q[i + 2] as usize) << 1) | q[i + 3] as usize;
            a_bits += self.len[15][mask] as u32;
            b_bits += self.len[16][mask] as u32;
            signs += mask.count_ones();
            i += 4;
        }
        c.count1_select = b_bits < a_bits;
        let mut bits = a_bits.min(b_bits) + signs;
        bits += q[..big].iter().filter(|v| **v != 0).count() as u32;

        if block_type != block::NORMAL {
            // Switched windows imply the split: region 0 is the first 36 lines.
            let r1 = 36.min(big);
            let (t0, b0) = self.region(q, 0, r1);
            let (t1, b1) = self.region(q, r1, big);
            c.table_select = [t0, t1, 0];
            c.bits = bits + b0 + b1;
            return c;
        }
        if !optimal {
            let n = (0..23).find(|&n| sfb_long[n] as usize >= big).unwrap_or(22);
            let (r0, r1) = SPLIT[n];
            let (t, b) = self.long_split(q, sfb_long, big, r0, r1);
            c.table_select = t;
            c.region0_count = r0;
            c.region1_count = r1;
            c.bits = bits + b;
            return c;
        }
        let (r0, r1, t, b) = self.best_split(q, sfb_long, big);
        c.table_select = t;
        c.region0_count = r0;
        c.region1_count = r1;
        c.bits = bits + b;
        c
    }

    fn long_split(&self, q: &[u16; LINES], sfb: &[u16; 23], big: usize, r0: u8, r1: u8) -> ([u8; 3], u32) {
        let b1 = (sfb[(r0 as usize + 1).min(22)] as usize).min(big);
        let b2 = (sfb[(r0 as usize + r1 as usize + 2).min(22)] as usize).min(big);
        let (t0, x0) = self.region(q, 0, b1);
        let (t1, x1) = self.region(q, b1, b2);
        let (t2, x2) = self.region(q, b2, big);
        ([t0, t1, t2], x0 + x1 + x2)
    }

    /// Every (region0_count, region1_count), from per-band costs of every
    /// table: summing band costs is far cheaper than recounting regions.
    fn best_split(&self, q: &[u16; LINES], sfb: &[u16; 23], big: usize) -> (u8, u8, [u8; 3], u32) {
        // band k covers lines sfb[k]..sfb[k+1] clipped to `big`
        let nb = (0..22).find(|&k| sfb[k + 1] as usize >= big).map(|k| k + 1).unwrap_or(22);
        let mut cost = [[0u32; 15]; 22];
        let mut esc = [0u32; 22];
        let mut maxv = [0u32; 22];
        for k in 0..nb {
            let (a, b) = (sfb[k] as usize, (sfb[k + 1] as usize).min(big));
            let mut m = 0u32;
            let mut e = 0u32;
            for i in (a..b).step_by(2) {
                let (x, y) = (q[i] as u32, q[i + 1] as u32);
                m = m.max(x).max(y);
                e += (x >= 15) as u32 + (y >= 15) as u32;
            }
            maxv[k] = m;
            esc[k] = e;
            for tree in 0..15 {
                let xlen = tree_xlen(tree);
                if tree < 13 && m >= xlen {
                    cost[k][tree] = u32::MAX / 64;
                    continue;
                }
                let mut s = 0u32;
                for i in (a..b).step_by(2) {
                    let (x, y) = ((q[i] as u32).min(15), (q[i + 1] as u32).min(15));
                    s += self.len[tree][(x * 16 + y) as usize] as u32;
                }
                cost[k][tree] = s;
            }
        }
        let region = |k0: usize, k1: usize| -> (u8, u32) {
            if k0 >= k1 {
                return (0, 0);
            }
            let m = maxv[k0..k1].iter().copied().max().unwrap_or(0);
            let mut best = (0u8, u32::MAX);
            for &t in candidates(m) {
                let Some((tree, linbits)) = HUFF_TABLES[t as usize] else {
                    return (0, 0);
                };
                let mut s = 0u32;
                for k in k0..k1 {
                    s += cost[k][tree as usize] + linbits as u32 * esc[k];
                }
                if s < best.1 {
                    best = (t, s);
                }
            }
            best
        };
        let mut best = (0u8, 0u8, [0u8; 3], u32::MAX);
        for r0 in 0..16usize {
            let k1 = (r0 + 1).min(nb);
            let (t0, c0) = region(0, k1);
            for r1 in 0..8usize {
                if r0 + r1 + 2 > 22 {
                    break;
                }
                let k2 = (r0 + r1 + 2).min(nb);
                let (t1, c1) = region(k1, k2);
                let (t2, c2) = region(k2, nb);
                let total = c0 + c1 + c2;
                if total < best.3 {
                    best = (r0 as u8, r1 as u8, [t0, t1, t2], total);
                }
                if k2 >= nb {
                    break;
                }
            }
            if k1 >= nb {
                break;
            }
        }
        best
    }

    /// Quantize with the given gain and scalefactors; false on overflow.
    fn quantize(&self, xr34: &[f32; LINES], bands: &Bands, gain: u32, sf: &[u8; 39], q: &mut [u16; LINES]) -> bool {
        for b in 0..bands.count {
            let e = gain as i32 - 210 - 2 * sf[b] as i32;
            let f = (-0.1875 * e as f32).exp2();
            let (start, width) = (bands.start[b] as usize, bands.width[b] as usize);
            for i in start..start + width {
                let v = xr34[i] * f + ROUND;
                if v > MAX_IX as f32 {
                    return false;
                }
                q[i] = v as u16;
            }
        }
        true
    }

    /// Bits for the given quantization (heuristic split) plus scalefactors;
    /// `None` when it cannot be coded.
    fn size(&self, xr34: &[f32; LINES], bands: &Bands, gain: u32, sf: &[u8; 39], bt: u8, q: &mut [u16; LINES]) -> Option<u32> {
        let (_, part2) = scalefac_compress(bands, sf)?;
        if !self.quantize(xr34, bands, gain, sf, q) {
            return None;
        }
        Some(self.coding(q, bt, &bands.sfb_long, false).bits + part2)
    }

    /// Code one granule of one channel within `budget` bits (scalefactors
    /// included). `xr` is in bitstream order for `bands`.
    pub fn encode(&self, xr: &[f32; LINES], bands: &Bands, block_type: u8, allowed: &[f32; 39], budget: u32) -> Granule {
        let budget = budget.min(MAX_GRANULE_BITS);
        let mut xr34 = [0.0f32; LINES];
        let mut any = false;
        for (o, x) in xr34.iter_mut().zip(xr.iter()) {
            let a = x.abs();
            *o = (a * a.sqrt()).sqrt();
            any |= a > 1e-12;
        }
        if !any {
            return Granule::silent(block_type);
        }
        let mut q = [0u16; LINES];
        let mut sf = [0u8; 39];
        // inner loop: the smallest gain that fits
        let fits = |gain: u32, sf: &[u8; 39], q: &mut [u16; LINES]| self.size(&xr34, bands, gain, sf, block_type, q).is_some_and(|b| b <= budget);
        let (mut lo, mut hi) = (0u32, 255u32);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if fits(mid, &sf, &mut q) {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        let mut gain = lo;
        if !fits(gain, &sf, &mut q) {
            // Not even the coarsest gain fits (a budget near zero): send silence.
            return Granule::silent(block_type);
        }
        let mut best_sf = sf;
        let mut best_gain = gain;
        let mut best_score = f32::MAX;
        let mut best_q = q;
        for _ in 0..OUTER_ITERATIONS {
            // distortion per band against the allowed noise
            let mut score = 0.0f32;
            let mut over = [false; 39];
            let mut any_over = false;
            for b in 0..bands.count {
                let e = gain as i32 - 210 - 2 * sf[b] as i32;
                let scale = (0.25 * e as f32).exp2();
                let (start, width) = (bands.start[b] as usize, bands.width[b] as usize);
                let mut noise = 0.0f32;
                for i in start..start + width {
                    let d = xr[i].abs() - self.pow43[q[i] as usize] * scale;
                    noise += d * d;
                }
                if noise > allowed[b] {
                    score += (noise / allowed[b]).log10();
                    over[b] = true;
                    any_over = true;
                }
            }
            if score < best_score {
                best_score = score;
                best_sf = sf;
                best_gain = gain;
                best_q = q;
            }
            if !any_over {
                break;
            }
            // amplify the distorted bands that carry a scalefactor
            let mut amplified = 0;
            let mut transmitted = 0;
            for b in 0..bands.count {
                if !bands.has_sf[b] {
                    continue;
                }
                transmitted += 1;
                if over[b] {
                    sf[b] += 1;
                    amplified += 1;
                }
            }
            if amplified == 0 || amplified == transmitted || scalefac_compress(bands, &sf).is_none() {
                break;
            }
            // the amplification costs bits: raise the gain until it fits again
            let mut ok = false;
            while gain <= 255 {
                if fits(gain, &sf, &mut q) {
                    ok = true;
                    break;
                }
                gain += 1;
            }
            if !ok {
                break;
            }
        }
        let (sfc, part2) = scalefac_compress(bands, &best_sf).unwrap_or((0, 0));
        let coding = self.coding(&best_q, block_type, &bands.sfb_long, true);
        let mut ix = [0i32; LINES];
        for i in 0..LINES {
            ix[i] = if xr[i] < 0.0 { -(best_q[i] as i32) } else { best_q[i] as i32 };
        }
        Granule {
            block_type,
            global_gain: best_gain,
            scalefac_compress: sfc,
            part2_3_length: part2 + coding.bits,
            coding,
            sf: best_sf,
            ix,
        }
    }

    /// The granule's main data: scalefactors, big values, count1.
    pub fn write(&self, g: &Granule, bands: &Bands, w: &mut BitWriter) {
        let start = w.len_bits();
        if g.part2_3_length == 0 {
            return;
        }
        let (slen1, slen2) = SLEN[g.scalefac_compress as usize];
        let split = if bands.short { 18 } else { 11 };
        for b in 0..bands.count {
            if !bands.has_sf[b] {
                continue;
            }
            let n = if b < split { slen1 } else { slen2 };
            w.put(g.sf[b] as u32, n as u32);
        }
        let c = &g.coding;
        let big = c.big_values as usize * 2;
        let (r1, r2) = if g.block_type != block::NORMAL {
            (36.min(big), big)
        } else {
            let s = &bands.sfb_long;
            (
                (s[(c.region0_count as usize + 1).min(22)] as usize).min(big),
                (s[(c.region0_count as usize + c.region1_count as usize + 2).min(22)] as usize).min(big),
            )
        };
        let mut i = 0;
        while i < big {
            let region = if i < r1 { 0 } else if i < r2 { 1 } else { 2 };
            let t = c.table_select[region] as usize;
            if let Some((tree, linbits)) = HUFF_TABLES[t] {
                let (tree, linbits) = (tree as usize, linbits as u32);
                let (x, y) = (g.ix[i], g.ix[i + 1]);
                let (ax, ay) = (x.unsigned_abs(), y.unsigned_abs());
                let (cx, cy) = if linbits > 0 { (ax.min(15), ay.min(15)) } else { (ax, ay) };
                let v = (cx * 16 + cy) as usize;
                w.put(self.code[tree][v], self.len[tree][v] as u32);
                if linbits > 0 && ax >= 15 {
                    w.put(ax - 15, linbits);
                }
                if ax != 0 {
                    w.put((x < 0) as u32, 1);
                }
                if linbits > 0 && ay >= 15 {
                    w.put(ay - 15, linbits);
                }
                if ay != 0 {
                    w.put((y < 0) as u32, 1);
                }
            }
            i += 2;
        }
        let tree = if c.count1_select { 16 } else { 15 };
        while i < c.count1_end {
            let v = &g.ix[i..i + 4];
            let mask = v.iter().fold(0usize, |m, x| (m << 1) | (*x != 0) as usize);
            w.put(self.code[tree][mask], self.len[tree][mask] as u32);
            for x in v {
                if *x != 0 {
                    w.put((*x < 0) as u32, 1);
                }
            }
            i += 4;
        }
        debug_assert_eq!(w.len_bits() - start, g.part2_3_length as usize);
    }
}

/// `scalefac_compress` -> (slen1, slen2), Table 3-B.4.
const SLEN: [(u8, u8); 16] = [
    (0, 0), (0, 1), (0, 2), (0, 3), (3, 0), (1, 1), (1, 2), (1, 3), (2, 1), (2, 2), (2, 3), (3, 1), (3, 2), (3, 3), (4, 2), (4, 3),
];

/// Fixed long-block region split by the index of the first band boundary at
/// or above the big-value extent: (region0_count, region1_count).
const SPLIT: [(u8, u8); 23] = [
    (0, 0), (0, 0), (0, 0), (0, 0), (0, 0), (0, 1), (1, 1), (1, 1), (1, 2), (2, 2), (2, 3), (2, 3),
    (3, 4), (3, 4), (3, 4), (4, 5), (4, 5), (4, 6), (5, 6), (5, 6), (5, 7), (6, 7), (6, 7),
];

fn bits_for(v: u8) -> u8 {
    (8 - v.leading_zeros()) as u8
}

/// The cheapest `scalefac_compress` for these scalefactors and the part2 bits
/// it costs; `None` when they exceed what any entry can carry.
fn scalefac_compress(bands: &Bands, sf: &[u8; 39]) -> Option<(u32, u32)> {
    let split = if bands.short { 18 } else { 11 };
    let (mut m1, mut m2) = (0u8, 0u8);
    for b in 0..bands.count {
        if !bands.has_sf[b] {
            continue;
        }
        if b < split {
            m1 = m1.max(sf[b]);
        } else {
            m2 = m2.max(sf[b]);
        }
    }
    let (need1, need2) = (bits_for(m1), bits_for(m2));
    let (n1, n2) = if bands.short { (18, 18) } else { (11, 10) };
    let mut best: Option<(u32, u32)> = None;
    for (i, &(s1, s2)) in SLEN.iter().enumerate() {
        if s1 >= need1 && s2 >= need2 {
            let bits = n1 * s1 as u32 + n2 * s2 as u32;
            if best.is_none_or(|(_, b)| bits < b) {
                best = Some((i as u32, bits));
            }
        }
    }
    best
}
