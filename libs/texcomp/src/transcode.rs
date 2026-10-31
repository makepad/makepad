//! Transcoding UASTC blocks to the block formats GPUs sample, following the
//! UASTC LDR 4x4 specification's conversion rules so the result matches the
//! reference transcoder's choices (endpoint scaling, optimal p-bits, weight
//! remapping, pattern mapping).
//!
//! UASTC → BC7 is a per-block remap (no texel unpacking): each UASTC mode has
//! a fixed BC7 mode, the partition patterns are common to both formats, and
//! only endpoint precision changes.

use crate::astc::{self, AstcBlock};
use crate::bc7::{self, Bc7};
use crate::uastc::{self, Unpacked};
use crate::uastc_tables as ut;

// ── UASTC → BC7 ─────────────────────────────────────────────────────────

/// BC7 mode each UASTC mode transcodes to (mode 8 chooses 5 or 6).
const UASTC_TO_BC7_MODE: [u8; 19] = [6, 3, 1, 2, 3, 6, 5, 2, 6, 7, 6, 5, 6, 5, 6, 6, 7, 5, 6];

/// Optimal BC7 endpoints for a solid colour component: for mode 6 (index 5,
/// per p-bit) and mode 5 (colour index 1). (lo, hi, error) per 8-bit value.
struct SolidTables { mode6: [[(u8, u8, u16); 2]; 256], mode5: [(u8, u8); 256] }
fn solid_tables() -> &'static SolidTables {
    static T: std::sync::OnceLock<SolidTables> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = SolidTables { mode6: [[(0, 0, u16::MAX); 2]; 256], mode5: [(0, 0); 256] };
        let w6 = bc7::weight(4, 5);
        let w5 = bc7::weight(2, 1);
        let mut best5 = [u16::MAX; 256];
        for lo in 0..128u8 { for hi in 0..128u8 {
            for p in 0..2u8 {
                let v = bc7::interpolate(bc7::expand(lo << 1 | p, 8), bc7::expand(hi << 1 | p, 8), w6) as usize;
                // Record the exact hit for this value, keeping the first (lowest) pair.
                let slot = &mut t.mode6[v][p as usize];
                if slot.2 != 0 { *slot = (lo, hi, 0); }
            }
            let v = bc7::interpolate(bc7::expand(lo, 7), bc7::expand(hi, 7), w5) as usize;
            if best5[v] != 0 { best5[v] = 0; t.mode5[v] = (lo, hi); }
        } }
        // Values no pair hits exactly: take the nearest reachable value.
        for p in 0..2 {
            for v in 0..256usize {
                if t.mode6[v][p].2 == 0 { continue; }
                let near = (0..256usize).filter(|&u| t.mode6[u][p].2 == 0).min_by_key(|&u| (u as i32 - v as i32).unsigned_abs()).unwrap();
                let d = (near as i32 - v as i32).unsigned_abs() as u16;
                t.mode6[v][p] = (t.mode6[near][p].0, t.mode6[near][p].1, d * d);
            }
        }
        for v in 0..256usize {
            if best5[v] == 0 { continue; }
            let near = (0..256usize).filter(|&u| best5[u] == 0).min_by_key(|&u| (u as i32 - v as i32).unsigned_abs()).unwrap();
            t.mode5[v] = t.mode5[near];
        }
        t
    })
}

/// The specification's `determine_unique_pbits`: per-endpoint p-bits.
fn unique_pbits(total_comps: usize, comp_bits: u32, xl: [f32; 4], xh: [f32; 4]) -> ([u8; 4], [u8; 4], [u8; 2]) {
    let total_bits = comp_bits + 1;
    let iscalep = (1i32 << total_bits) - 1;
    let scalep = iscalep as f32;
    let (mut best0, mut best1) = (1e9f32, 1e9f32);
    let (mut lo_out, mut hi_out, mut pbits) = ([0u8; 4], [0u8; 4], [0u8; 2]);
    // An endpoint with alpha exactly 1 keeps p-bit 1, so opaque stays 255
    // (with p-bit 0 the top alpha value is 254).
    let opaque = [total_comps == 4 && xl[3] >= 1.0, total_comps == 4 && xh[3] >= 1.0];
    for p in 0..2i32 {
        let mut lo = [0u8; 4];
        let mut hi = [0u8; 4];
        for c in 0..4 {
            lo[c] = ((((xl[c] * scalep - p as f32) / 2.0 + 0.5) as i32) * 2 + p).clamp(p, iscalep - 1 + p) as u8;
            hi[c] = ((((xh[c] * scalep - p as f32) / 2.0 + 0.5) as i32) * 2 + p).clamp(p, iscalep - 1 + p) as u8;
        }
        let (mut e0, mut e1) = (0f32, 0f32);
        for i in 0..total_comps {
            let sl = bc7::expand(lo[i], total_bits) as f32;
            let sh = bc7::expand(hi[i], total_bits) as f32;
            e0 += (sl - xl[i] * 255.0).powi(2);
            e1 += (sh - xh[i] * 255.0).powi(2);
        }
        if e0 < best0 && (p == 1 || !opaque[0]) { best0 = e0; pbits[0] = p as u8; for c in 0..4 { lo_out[c] = lo[c] >> 1; } }
        if e1 < best1 && (p == 1 || !opaque[1]) { best1 = e1; pbits[1] = p as u8; for c in 0..4 { hi_out[c] = hi[c] >> 1; } }
    }
    (lo_out, hi_out, pbits)
}

/// The specification's `determine_shared_pbits`: one p-bit per subset.
fn shared_pbits(total_comps: usize, comp_bits: u32, xl: [f32; 4], xh: [f32; 4]) -> ([u8; 4], [u8; 4], u8) {
    let total_bits = comp_bits + 1;
    let iscalep = (1i32 << total_bits) - 1;
    let scalep = iscalep as f32;
    let mut best = 1e9f32;
    let (mut lo_out, mut hi_out, mut pbit) = ([0u8; 4], [0u8; 4], 0u8);
    for p in 0..2i32 {
        let mut lo = [0u8; 4];
        let mut hi = [0u8; 4];
        for c in 0..4 {
            lo[c] = ((((xl[c] * scalep - p as f32) / 2.0 + 0.5) as i32) * 2 + p).clamp(p, iscalep - 1 + p) as u8;
            hi[c] = ((((xh[c] * scalep - p as f32) / 2.0 + 0.5) as i32) * 2 + p).clamp(p, iscalep - 1 + p) as u8;
        }
        let mut e = 0f32;
        for i in 0..total_comps {
            e += (bc7::expand(lo[i], total_bits) as f32 / 255.0 - xl[i]).powi(2) + (bc7::expand(hi[i], total_bits) as f32 / 255.0 - xh[i]).powi(2);
        }
        if e < best { best = e; pbit = p as u8; for c in 0..4 { lo_out[c] = lo[c] >> 1; hi_out[c] = hi[c] >> 1; } }
    }
    (lo_out, hi_out, pbit)
}

/// UASTC weight index → BC7 index for the modes whose index widths differ.
fn convert_weight(uastc_mode: usize, w: u8) -> u8 {
    const W1_2: [u8; 2] = [0, 3];
    const W2_4: [u8; 4] = [0, 5, 10, 15];
    const W3_4: [u8; 8] = [0, 2, 4, 6, 9, 11, 13, 15];
    const W5_4: [u8; 32] = [0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 6, 7, 8, 9, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 15];
    match uastc_mode { 13 => W1_2[w as usize], 14 => W2_4[w as usize], 5 | 12 => W3_4[w as usize], 18 => W5_4[w as usize], _ => w }
}

/// Mode 7's mapping of a BC7 3-subset index to the UASTC 2-subset index.
fn convert_subset_3_to_2(p: usize, k: u8) -> usize {
    let mut p = match k >> 1 { 0 => (p > 1) as usize, 1 => (p != 0) as usize, _ => (p == 1) as usize };
    if k & 1 == 1 { p = 1 - p; }
    p
}

/// Transcode one unpacked UASTC block to a BC7 block.
pub fn uastc_to_bc7_unpacked(u: &Unpacked) -> Bc7 {
    if u.mode == uastc::MODE_SOLID { return solid_bc7(u.solid); }
    let mode = u.mode;
    let comps = uastc::COMPS[mode] as usize;
    let range = uastc::ENDPOINT_RANGE[mode];
    let planes = uastc::PLANES[mode] as usize;
    // Dequantised RGBA endpoints per UASTC subset.
    let mut ends = [[[0u8; 4]; 2]; 3];
    for s in 0..uastc::SUBSETS[mode] as usize {
        for e in 0..2 {
            let v = |k: usize| uastc::dequant_endpoint(range, u.endpoints[(s * comps + k) * 2 + e] as u32);
            ends[s][e] = match comps { 2 => { let l = v(0); [l, l, l, v(1)] } 3 => [v(0), v(1), v(2), 255], _ => [v(0), v(1), v(2), v(3)] };
        }
    }
    let bm = UASTC_TO_BC7_MODE[mode];
    let m = bm as usize;
    let mut b = Bc7 { mode: bm, ..Default::default() };
    // BC7 subset k takes UASTC subset `map[k]`; the texel pattern follows.
    let mut map = [0usize, 1, 2];
    match mode {
        1 => { b.partition = 0; map = [0, 0, 0]; }
        2 | 4 | 9 | 16 => {
            let (bc7_pattern, _, invert) = ut::PARTITIONS2[u.pattern];
            b.partition = bc7_pattern;
            map = if invert { [1, 0, 0] } else { [0, 1, 0] };
        }
        3 => {
            let (bc7_pattern, _, perm) = ut::PARTITIONS3[u.pattern];
            b.partition = bc7_pattern;
            // perm maps ASTC subset → BC7 subset; invert it.
            for a in 0..3 { map[ut::ASTC_BC7_SUBSET_PERM[perm as usize][a] as usize] = a; }
        }
        7 => {
            let (bc7_pattern, _, k) = ut::PARTITIONS2_BC73[u.pattern];
            b.partition = bc7_pattern;
            for p in 0..3 { map[p] = convert_subset_3_to_2(p, k); }
        }
        _ => {}
    }
    // Endpoints.
    let norm = |c: [u8; 4]| c.map(|v| v as f32 / 255.0);
    let scale = |v: u8, bits: u32| ((v as u32 * ((1 << bits) - 1) + 127) / 255) as u8;
    // BC7 modes 1 and 3 have no alpha. Elsewhere alpha counts in the p-bit
    // choice even for opaque UASTC, so opaque stays 255 (not 254).
    let total_comps = if m == 1 || m == 3 { 3 } else { 4 };
    for k in 0..bc7::NS[m] as usize {
        let src = ends[map[k]];
        match m {
            6 | 3 | 7 | 0 => {
                let (lo, hi, pb) = unique_pbits(total_comps, bc7::CB[m] as u32, norm(src[0]), norm(src[1]));
                b.endpoints[k] = [lo, hi];
                b.pbits[k] = pb;
            }
            1 => {
                let (lo, hi, pb) = shared_pbits(3, bc7::CB[m] as u32, norm(src[0]), norm(src[1]));
                b.endpoints[k] = [lo, hi];
                b.pbits[k] = [pb, pb];
            }
            2 => { for e in 0..2 { for c in 0..3 { b.endpoints[k][e][c] = scale(src[e][c], 5); } } }
            _ => {
                // BC7 mode 5: dual plane; the CCS channel moves to alpha.
                let mut src = src;
                let rotation = if u.compsel < 3 && planes == 2 { u.compsel as u8 + 1 } else { 0 };
                if rotation > 0 { for e in 0..2 { src[e].swap(rotation as usize - 1, 3); } }
                b.rotation = rotation;
                for e in 0..2 { for c in 0..3 { b.endpoints[k][e][c] = scale(src[e][c], 7); } b.endpoints[k][e][3] = src[e][3]; }
            }
        }
    }
    // Weights.
    for i in 0..16 {
        if m == 5 {
            b.indices[i] = convert_weight(mode, u.weights[i * planes]);
            b.indices2[i] = if planes == 2 { convert_weight(mode, u.weights[i * planes + 1]) } else { b.indices[i] };
        } else {
            b.indices[i] = convert_weight(mode, u.weights[i * planes]);
        }
    }
    fix_bc7_anchors(&mut b);
    b
}

/// A solid colour as BC7 mode 6 when it reproduces it exactly, else mode 5.
fn solid_bc7(c: [u8; 4]) -> Bc7 {
    let t = solid_tables();
    let err = |p: usize| (0..4).map(|k| t.mode6[c[k] as usize][p].2 as u32).sum::<u32>();
    let (e0, e1) = (err(0), err(1));
    let mut b = Bc7::default();
    if e0 > 0 && e1 > 0 {
        b.mode = 5;
        for k in 0..3 { let (lo, hi) = t.mode5[c[k] as usize]; b.endpoints[0][0][k] = lo; b.endpoints[0][1][k] = hi; }
        b.endpoints[0][0][3] = c[3];
        b.endpoints[0][1][3] = c[3];
        b.indices = [1; 16];
        b.indices[0] = 1;
        b.indices2 = [0; 16];
    } else {
        let p = if e1 < e0 { 1 } else { 0 };
        b.mode = 6;
        for k in 0..4 { let (lo, hi, _) = t.mode6[c[k] as usize][p]; b.endpoints[0][0][k] = lo; b.endpoints[0][1][k] = hi; }
        b.pbits[0] = [p as u8, p as u8];
        b.indices = [5; 16];
    }
    fix_bc7_anchors(&mut b);
    b
}

/// Swap endpoints (and invert indices) so each BC7 anchor index has MSB 0.
fn fix_bc7_anchors(b: &mut Bc7) {
    let m = b.mode as usize;
    let ns = bc7::NS[m];
    // Modes 4 and 5 keep colour and alpha index sets apart: each swap only
    // touches the channels its index set drives.
    for s in 0..ns as usize {
        let a = bc7::anchor(ns, b.partition, s);
        let bits = bc7::IB[m];
        if b.indices[a] >> (bits - 1) & 1 == 1 {
            let top = (1u8 << bits) - 1;
            let chans: &[usize] = if bc7::IB2[m] > 0 { if b.index_select == 1 { &[3] } else { &[0, 1, 2] } } else { &[0, 1, 2, 3] };
            for &c in chans { let (x, y) = (b.endpoints[s][0][c], b.endpoints[s][1][c]); b.endpoints[s][0][c] = y; b.endpoints[s][1][c] = x; }
            if bc7::IB2[m] == 0 { b.pbits[s].swap(0, 1); }
            for i in (0..16).filter(|&i| bc7::subset(ns, b.partition, i) == s) { b.indices[i] = top - b.indices[i]; }
        }
    }
    if bc7::IB2[m] > 0 {
        let bits = bc7::IB2[m];
        if b.indices2[0] >> (bits - 1) & 1 == 1 {
            let top = (1u8 << bits) - 1;
            let chans: &[usize] = if b.index_select == 1 { &[0, 1, 2] } else { &[3] };
            for &c in chans { let (x, y) = (b.endpoints[0][0][c], b.endpoints[0][1][c]); b.endpoints[0][0][c] = y; b.endpoints[0][1][c] = x; }
            for i in 0..16 { b.indices2[i] = top - b.indices2[i]; }
        }
    }
}

/// Transcode one UASTC block to BC7.
pub fn uastc_to_bc7(block: &[u8; 16]) -> [u8; 16] {
    match uastc::unpack(block) {
        Some(u) => bc7::pack(&uastc_to_bc7_unpacked(&u)),
        None => bc7::pack(&solid_bc7([255, 0, 255, 255])),
    }
}

/// Transcode a whole UASTC level to BC7 (same block grid).
pub fn uastc_to_bc7_image(data: &[u8]) -> Vec<u8> { let mut out = Vec::new(); uastc_to_bc7_into(data, &mut out); out }
/// [`uastc_to_bc7_image`] appended to `out` (an upload buffer), no staging copy.
pub fn uastc_to_bc7_into(data: &[u8], out: &mut Vec<u8>) {
    out.reserve(data.len());
    for src in data.chunks_exact(16) { out.extend_from_slice(&uastc_to_bc7(src.try_into().unwrap())); }
}

// ── UASTC → ASTC 4x4 ────────────────────────────────────────────────────

/// Transcode one unpacked UASTC block to ASTC. Lossless: UASTC is an ASTC
/// subset. Only ASTC's blue contraction (taken when the second RGB endpoint
/// sums lower than the first) needs undoing, by swapping that subset's
/// endpoints and inverting its weights.
pub fn uastc_to_astc_unpacked(u: &Unpacked) -> AstcBlock {
    let mode = u.mode;
    if mode == uastc::MODE_SOLID {
        return AstcBlock { partitions: 1, seed: 0, cem: 0, ccs: None, weight_levels: 2, endpoint_range: 0, endpoints: Vec::new(), weights: [0; 32], solid: Some(u.solid) };
    }
    let comps = uastc::COMPS[mode] as usize;
    let planes = uastc::PLANES[mode] as usize;
    let subsets = uastc::SUBSETS[mode] as usize;
    let range = uastc::ENDPOINT_RANGE[mode];
    let seed = match mode {
        3 => ut::PARTITIONS3[u.pattern].1,
        7 => ut::PARTITIONS2_BC73[u.pattern].1,
        _ if subsets == 2 => ut::PARTITIONS2[u.pattern].1,
        _ => 0,
    };
    let mut endpoints = u.endpoints[..uastc::endpoint_count(mode)].to_vec();
    let mut weights = u.weights;
    let top = (1u8 << uastc::WEIGHT_BITS[mode]) - 1;
    if comps >= 3 {
        for s in 0..subsets {
            let e = &mut endpoints[s * comps * 2..(s + 1) * comps * 2];
            let d = |v: u8| uastc::dequant_endpoint(range, v as u32) as u32;
            let (s0, s1) = (d(e[0]) + d(e[2]) + d(e[4]), d(e[1]) + d(e[3]) + d(e[5]));
            if s1 < s0 {
                for k in 0..comps { e.swap(k * 2, k * 2 + 1); }
                for i in (0..16).filter(|&i| uastc::subset_of(mode, u.pattern, i) == s) {
                    for p in 0..planes { weights[i * planes + p] = top - weights[i * planes + p]; }
                }
            }
        }
    }
    AstcBlock {
        partitions: subsets as u8, seed, cem: match comps { 2 => 4, 3 => 8, _ => 12 },
        ccs: (planes == 2).then_some(u.compsel as u8), weight_levels: 1 << uastc::WEIGHT_BITS[mode],
        endpoint_range: range, endpoints, weights, solid: None,
    }
}

/// Transcode one UASTC block to ASTC 4x4.
pub fn uastc_to_astc(block: &[u8; 16]) -> [u8; 16] {
    match uastc::unpack(block) {
        Some(u) => astc::pack(&uastc_to_astc_unpacked(&u)),
        None => astc::pack(&AstcBlock { partitions: 1, seed: 0, cem: 0, ccs: None, weight_levels: 2, endpoint_range: 0, endpoints: Vec::new(), weights: [0; 32], solid: Some([255, 0, 255, 255]) }),
    }
}

/// Transcode a whole UASTC level to ASTC 4x4 (same block grid).
pub fn uastc_to_astc_image(data: &[u8]) -> Vec<u8> { let mut out = Vec::new(); uastc_to_astc_into(data, &mut out); out }
/// [`uastc_to_astc_image`] appended to `out` (an upload buffer), no staging copy.
pub fn uastc_to_astc_into(data: &[u8], out: &mut Vec<u8>) {
    out.reserve(data.len());
    for src in data.chunks_exact(16) { out.extend_from_slice(&uastc_to_astc(src.try_into().unwrap())); }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Random valid UASTC blocks of every mode (anchor rules respected).
    fn random_blocks() -> Vec<Unpacked> {
        let mut seed = 0xC0FFEEu32;
        let mut rnd = move |n: u32| { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); (seed >> 8) % n.max(1) };
        let mut out = Vec::new();
        for mode in 0..19usize {
            for _ in 0..200 {
                let mut u = Unpacked { mode, etc1_bias: 13, ..Default::default() };
                if mode == uastc::MODE_SOLID { u.solid = [rnd(256) as u8, rnd(256) as u8, rnd(256) as u8, rnd(256) as u8]; out.push(u); continue; }
                u.pattern = rnd(uastc::pattern_count(mode) as u32) as usize;
                u.compsel = if uastc::PLANES[mode] == 2 { if mode == 17 { 3 } else { rnd(4) as usize } } else { 0 };
                for i in 0..uastc::endpoint_count(mode) { u.endpoints[i] = rnd(uastc::range_levels(uastc::ENDPOINT_RANGE[mode])) as u8; }
                let planes = uastc::PLANES[mode] as usize;
                for i in 0..16 { for p in 0..planes { u.weights[i * planes + p] = rnd(1 << uastc::WEIGHT_BITS[mode]) as u8; } }
                // Re-pack through the block format so anchors are valid.
                let block = uastc::pack(&u);
                let mut fixed = uastc::unpack(&block).unwrap();
                fixed.mode = mode;
                out.push(fixed);
            }
        }
        out
    }

    #[test]
    fn every_uastc_mode_transcodes_to_astc_losslessly() {
        for u in random_blocks() {
            let expect = uastc::decode_unpacked(&u);
            let got = astc::decode_block(&astc::pack(&uastc_to_astc_unpacked(&u))).expect("decodable");
            assert_eq!(got, expect, "mode {} pattern {} compsel {}", u.mode, u.pattern, u.compsel);
        }
    }

    #[test]
    fn every_uastc_mode_transcodes_to_bc7_closely() {
        for u in random_blocks() {
            let expect = uastc::decode_unpacked(&u);
            let got = bc7::decode_block(&bc7::pack(&uastc_to_bc7_unpacked(&u)));
            let worst = (0..16).flat_map(|i| (0..4).map(move |c| (i, c))).map(|(i, c)| (got[i][c] as i32 - expect[i][c] as i32).abs()).max().unwrap();
            // Endpoint precision drops to 5-8 bits; weights remap within a step.
            assert!(worst <= 24, "mode {} pattern {} compsel {}: worst channel error {worst}", u.mode, u.pattern, u.compsel);
        }
    }
}
