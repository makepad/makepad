//! ETC2 RGBA8 (EAC alpha + ETC2 colour), written from the Khronos Data
//! Format Specification 1.3, chapters 21-22. The GL / mobile fallback
//! target: UASTC blocks are decoded and re-encoded here. The colour encoder
//! uses ETC2's ETC1-compatible individual and differential modes (a valid
//! subset of ETC2); the decoder covers the same subset plus alpha.

use crate::uastc;

/// ETC1/ETC2 intensity modifier sets (Tables 126/129): (a, b) per codeword.
const INTENSITY: [[i32; 2]; 8] = [[2, 8], [5, 17], [9, 29], [13, 42], [18, 60], [24, 80], [33, 106], [47, 183]];
/// EAC alpha modifier sets (Table 133).
const ALPHA: [[i32; 8]; 16] = [
    [-3, -6, -9, -15, 2, 5, 8, 14], [-3, -7, -10, -13, 2, 6, 9, 12], [-2, -5, -8, -13, 1, 4, 7, 12], [-2, -4, -6, -13, 1, 3, 5, 12],
    [-3, -6, -8, -12, 2, 5, 7, 11], [-3, -7, -9, -11, 2, 6, 8, 10], [-4, -7, -8, -11, 3, 6, 7, 10], [-3, -5, -8, -11, 2, 4, 7, 10],
    [-2, -6, -8, -10, 1, 5, 7, 9], [-2, -5, -8, -10, 1, 4, 7, 9], [-2, -4, -8, -10, 1, 3, 7, 9], [-2, -5, -7, -10, 1, 4, 6, 9],
    [-3, -4, -7, -10, 2, 3, 6, 9], [-1, -2, -3, -10, 0, 1, 2, 9], [-4, -6, -8, -9, 3, 5, 7, 8], [-3, -5, -7, -9, 2, 4, 6, 8],
];

/// Modifier for a 2-bit pixel index (MSB<<1 | LSB), Table 127.
fn modifier(table: usize, index: u32) -> i32 {
    let [a, b] = INTENSITY[table];
    match index { 0 => a, 1 => b, 2 => -a, _ => -b }
}
/// Texel (x, y) is pixel `x*4+y` (column-major a..p).
fn pixel(x: usize, y: usize) -> usize { x * 4 + y }
fn in_sub(flip: bool, sub: usize, x: usize, y: usize) -> bool { if flip { (y >= 2) as usize == sub } else { (x >= 2) as usize == sub } }

// ── decode ──────────────────────────────────────────────────────────────

/// Decode an ETC2 colour block of the individual/differential modes.
/// Returns `None` for T, H or planar blocks (never produced here).
pub fn decode_color(block: &[u8; 8]) -> Option<[[u8; 3]; 16]> {
    let v = u64::from_be_bytes(*block);
    let diff = (v >> 33) & 1 == 1;
    let flip = (v >> 32) & 1 == 1;
    let tables = [((v >> 37) & 7) as usize, ((v >> 34) & 7) as usize];
    let mut base = [[0i32; 3]; 2];
    for c in 0..3 {
        let shift = 59 - 8 * c as u32;
        if diff {
            let b5 = ((v >> shift) & 31) as i32;
            let d = (((v >> (shift - 3)) & 7) as i32) << 29 >> 29;
            let second = b5 + d;
            if !(0..=31).contains(&second) { return None; }
            base[0][c] = (b5 << 3) | (b5 >> 2);
            base[1][c] = (second << 3) | (second >> 2);
        } else {
            let hi = ((v >> (shift + 1)) & 15) as i32;
            let lo = ((v >> (shift - 3)) & 15) as i32;
            base[0][c] = hi * 17;
            base[1][c] = lo * 17;
        }
    }
    let mut out = [[0u8; 3]; 16];
    for x in 0..4 { for y in 0..4 {
        let p = pixel(x, y);
        let index = ((v >> (16 + p)) & 1) << 1 | ((v >> p) & 1);
        let sub = in_sub(flip, 1, x, y) as usize;
        let m = modifier(tables[sub], index as u32);
        out[y * 4 + x] = std::array::from_fn(|c| (base[sub][c] + m).clamp(0, 255) as u8);
    } }
    Some(out)
}

pub fn decode_alpha(block: &[u8; 8]) -> [u8; 16] {
    let v = u64::from_be_bytes(*block);
    let base = (v >> 56) as i32;
    let mult = ((v >> 52) & 15) as i32;
    let table = ((v >> 48) & 15) as usize;
    let mut out = [0u8; 16];
    for x in 0..4 { for y in 0..4 {
        let p = pixel(x, y);
        let index = ((v >> (45 - 3 * p)) & 7) as usize;
        out[y * 4 + x] = (base + ALPHA[table][index] * mult).clamp(0, 255) as u8;
    } }
    out
}

/// Decode an ETC2 RGBA8 block (alpha block, then colour block).
pub fn decode_rgba(block: &[u8; 16]) -> Option<[[u8; 4]; 16]> {
    let a = decode_alpha(block[..8].try_into().unwrap());
    let c = decode_color(block[8..].try_into().unwrap())?;
    Some(std::array::from_fn(|i| [c[i][0], c[i][1], c[i][2], a[i]]))
}

// ── encode ──────────────────────────────────────────────────────────────

/// Best table and indices for one sub-block around `base` (8-bit colour).
fn fit_sub(texels: &[[u8; 4]; 16], flip: bool, sub: usize, base: [i32; 3]) -> (u32, usize, u32) {
    let mut best = (u32::MAX, 0, 0u32);
    for table in 0..8 {
        let (mut err, mut bits) = (0u32, 0u32);
        for x in 0..4 { for y in 0..4 {
            if !in_sub(flip, sub, x, y) { continue; }
            let t = texels[y * 4 + x];
            let (e, i) = (0..4u32).map(|i| {
                let m = modifier(table, i);
                let e: u32 = (0..3).map(|c| { let d = (base[c] + m).clamp(0, 255) - t[c] as i32; (d * d) as u32 }).sum();
                (e, i)
            }).min().unwrap();
            err += e;
            let p = pixel(x, y);
            bits |= (i >> 1) << (16 + p) | (i & 1) << p;
        } }
        if err < best.0 { best = (err, table, bits); }
    }
    best
}

fn average(texels: &[[u8; 4]; 16], flip: bool, sub: usize) -> [f32; 3] {
    let mut s = [0f32; 3];
    for x in 0..4 { for y in 0..4 { if in_sub(flip, sub, x, y) { for c in 0..3 { s[c] += texels[y * 4 + x][c] as f32 / 8.0; } } } }
    s
}

/// Encode the colour of a block (individual/differential ETC2 modes).
pub fn encode_color(texels: &[[u8; 4]; 16]) -> [u8; 8] {
    let mut best: (u32, u64) = (u32::MAX, 0);
    for flip in [false, true] {
        let avg = [average(texels, flip, 0), average(texels, flip, 1)];
        // Individual: 4-bit bases, each refined by +-1 per channel.
        let q4 = |v: f32| ((v / 17.0).round() as i32).clamp(0, 15);
        let mut sub_best = [(u32::MAX, [0i32; 3], 0usize, 0u32); 2];
        for s in 0..2 {
            let c0 = [q4(avg[s][0]), q4(avg[s][1]), q4(avg[s][2])];
            // Refining the base by +-1 per channel (27 candidates) gained
            // 1.1 dB RGB for 26x the time (35.9 vs 34.8 dB on the quality
            // test); a load-time transcode keeps the rounded average.
            for dr in 0..=0 { for dg in 0..=0 { for db in 0..=0 {
                let q = [(c0[0] + dr).clamp(0, 15), (c0[1] + dg).clamp(0, 15), (c0[2] + db).clamp(0, 15)];
                let (e, t, bits) = fit_sub(texels, flip, s, q.map(|v| v * 17));
                if e < sub_best[s].0 { sub_best[s] = (e, q, t, bits); }
            } } }
        }
        let err = sub_best[0].0 + sub_best[1].0;
        if err < best.0 {
            let mut v = 0u64;
            for c in 0..3 {
                let shift = 60 - 8 * c as u32;
                v |= (sub_best[0].1[c] as u64) << shift | (sub_best[1].1[c] as u64) << (shift - 4);
            }
            v |= (sub_best[0].2 as u64) << 37 | (sub_best[1].2 as u64) << 34 | (flip as u64) << 32 | (sub_best[0].3 | sub_best[1].3) as u64;
            best = (err, v);
        }
        // Differential: 5-bit first base, second within [-4, 3] of it.
        let q5 = |v: f32| ((v / 255.0 * 31.0).round() as i32).clamp(0, 31);
        let e5 = |v: i32| (v << 3) | (v >> 2);
        let a = [q5(avg[0][0]), q5(avg[0][1]), q5(avg[0][2])];
        let b = [q5(avg[1][0]), q5(avg[1][1]), q5(avg[1][2])];
        let b: [i32; 3] = std::array::from_fn(|c| b[c].clamp(a[c] - 4, a[c] + 3).clamp(0, 31));
        let (ea, ta, ba) = fit_sub(texels, flip, 0, a.map(e5));
        let (eb, tb, bb) = fit_sub(texels, flip, 1, b.map(e5));
        if ea + eb < best.0 {
            let mut v = 0u64;
            for c in 0..3 {
                let shift = 59 - 8 * c as u32;
                v |= (a[c] as u64) << shift | (((b[c] - a[c]) & 7) as u64) << (shift - 3);
            }
            v |= (ta as u64) << 37 | (tb as u64) << 34 | 1 << 33 | (flip as u64) << 32 | (ba | bb) as u64;
            best = (ea + eb, v);
        }
    }
    best.1.to_be_bytes()
}

/// Encode the alpha of a block (EAC): search table, multiplier and base.
pub fn encode_alpha(alpha: &[u8; 16]) -> [u8; 8] {
    let (lo, hi) = alpha.iter().fold((255i32, 0i32), |(l, h), &a| (l.min(a as i32), h.max(a as i32)));
    if lo == hi { return ((lo as u64) << 56 | 13 << 48).to_be_bytes(); } // multiplier 0: every texel is the base
    let mut best = (u32::MAX, 0u64);
    for table in 0..16 {
        let (mn, mx) = (ALPHA[table].iter().min().copied().unwrap(), ALPHA[table].iter().max().copied().unwrap());
        let m0 = (((hi - lo) as f32) / (mx - mn) as f32).round().clamp(1.0, 15.0) as i32;
        for mult in (m0 - 1).max(1)..=(m0 + 1).min(15) {
            let centre = lo - mn * mult;
            for base in (centre - 2).max(0)..=(centre + 2).min(255) {
                let (mut err, mut bits) = (0u32, 0u64);
                for x in 0..4 { for y in 0..4 {
                    let a = alpha[y * 4 + x] as i32;
                    let (e, i) = (0..8).map(|i| (((base + ALPHA[table][i] * mult).clamp(0, 255) - a).unsigned_abs().pow(2), i)).min().unwrap();
                    err += e;
                    bits |= (i as u64) << (45 - 3 * pixel(x, y));
                } }
                if err < best.0 { best = (err, (base as u64) << 56 | (mult as u64) << 52 | (table as u64) << 48 | bits); }
            }
        }
    }
    best.1.to_be_bytes()
}

pub fn encode_rgba(texels: &[[u8; 4]; 16]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&encode_alpha(&std::array::from_fn(|i| texels[i][3])));
    out[8..].copy_from_slice(&encode_color(texels));
    out
}

/// Transcode a UASTC level to ETC2 RGBA8 (same block grid, 16 bytes/block).
pub fn uastc_to_etc2_rgba_image(data: &[u8]) -> Vec<u8> {
    data.chunks_exact(16).flat_map(|b| encode_rgba(&uastc::decode_block(b.try_into().unwrap()))).collect()
}

/// Decode an ETC2 RGBA8 image to RGBA8.
pub fn decode_rgba_image(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let bx = w.div_ceil(4);
    let mut out = vec![0u8; w * h * 4];
    for (index, block) in data.chunks_exact(16).enumerate() {
        let texels = decode_rgba(block.try_into().unwrap()).unwrap_or([[255, 0, 255, 255]; 16]);
        let (x0, y0) = ((index % bx) * 4, (index / bx) * 4);
        for y in 0..4 { for x in 0..4 {
            let (px, py) = (x0 + x, y0 + y);
            if px < w && py < h { out[(py * w + px) * 4..][..4].copy_from_slice(&texels[y * 4 + x]); }
        } }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_blocks_decode_close_and_alpha_exactly() {
        for c in [[0u8, 0, 0, 255], [255, 255, 255, 0], [200, 100, 50, 128], [17, 34, 51, 68]] {
            let back = decode_rgba(&encode_rgba(&[c; 16])).unwrap();
            for t in back {
                assert_eq!(t[3], c[3], "alpha {c:?}");
                for k in 0..3 { assert!((t[k] as i32 - c[k] as i32).abs() <= 6, "{c:?} -> {t:?}"); }
            }
        }
    }

    #[test]
    fn differential_mode_never_overflows_into_t_h_planar() {
        let mut seed = 5u32;
        for _ in 0..500 {
            let texels: [[u8; 4]; 16] = std::array::from_fn(|_| { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); let v = seed.to_le_bytes(); [v[0], v[1], v[2], 255] });
            assert!(decode_color(&encode_color(&texels)).is_some());
        }
    }
}
