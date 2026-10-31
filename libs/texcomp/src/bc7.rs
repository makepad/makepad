//! BC7 (BPTC) block format: pack, unpack and decode all eight modes, written
//! from the Khronos Data Format Specification 1.3, section 20.1. The BC7
//! blocks this crate produces come from transcoding UASTC (see
//! [`crate::transcode`]); there is no separate BC7 encoder.

use crate::bc7_tables as t;

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

struct Bits<'a> { data: &'a [u8; 16], pos: u32 }
impl Bits<'_> {
    fn get(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for i in 0..n { let p = self.pos + i; v |= (((self.data[(p >> 3) as usize] >> (p & 7)) & 1) as u32) << i; }
        self.pos += n;
        v
    }
}
struct Writer { data: [u8; 16], pos: u32 }
impl Writer {
    fn put(&mut self, n: u32, v: u32) {
        for i in 0..n { let p = self.pos + i; if (v >> i) & 1 == 1 { self.data[(p >> 3) as usize] |= 1 << (p & 7); } }
        self.pos += n;
    }
}

pub fn unpack(block: &[u8; 16]) -> Option<Bc7> {
    let mode = (0..8).find(|m| (block[0] >> m) & 1 == 1)? as u8;
    let m = mode as usize;
    let mut r = Bits { data: block, pos: m as u32 + 1 };
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
    let mut w = Writer { data: [0; 16], pos: 0 };
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
    w.data
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
