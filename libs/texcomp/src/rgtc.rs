//! BC4 / BC5 (RGTC) unsigned: one or two 8-byte channel blocks per 4x4 tile,
//! written from the Khronos Data Format Specification 1.3, chapter 19. Used
//! for single-channel data and two-channel (XY) normal maps, transcoded from
//! UASTC by decoding the block and re-encoding its channels.

use crate::{ktx2, uastc};

/// The 8 palette values of a BC4 block (Table 108), rounded to 8 bits.
pub fn palette(r0: u8, r1: u8) -> [u8; 8] {
    let (a, b) = (r0 as u32, r1 as u32);
    if r0 > r1 {
        let m = |i: u32| ((a * (7 - i) + b * i + 3) / 7) as u8;
        [r0, r1, m(1), m(2), m(3), m(4), m(5), m(6)]
    } else {
        let m = |i: u32| ((a * (5 - i) + b * i + 2) / 5) as u8;
        [r0, r1, m(1), m(2), m(3), m(4), 0, 255]
    }
}

pub fn decode_bc4(block: &[u8; 8]) -> [u8; 16] {
    let p = palette(block[0], block[1]);
    let bits = block[2..8].iter().rev().fold(0u64, |v, b| v << 8 | *b as u64);
    std::array::from_fn(|i| p[((bits >> (3 * i)) & 7) as usize])
}

/// Encode 16 channel values (raster order). Tries the 8-value mode on the
/// block's range and the 6-value mode (which also has exact 0 and 255), then
/// refits the better one's endpoints to its chosen indices (least squares)
/// and tries the neighbouring endpoint pairs, keeping the lowest error.
pub fn encode_bc4(values: &[u8; 16]) -> [u8; 8] {
    let (lo, hi) = values.iter().fold((255u8, 0u8), |(l, h), &v| (l.min(v), h.max(v)));
    if lo == hi { return [lo, lo, 0, 0, 0, 0, 0, 0]; }
    let fit = |r0: u8, r1: u8| {
        let p = palette(r0, r1);
        let (mut bits, mut err) = (0u64, 0u32);
        for (i, &v) in values.iter().enumerate() {
            let (k, e) = p.iter().enumerate().map(|(k, &q)| (k, (q as i32 - v as i32).unsigned_abs())).min_by_key(|x| x.1).unwrap();
            bits |= (k as u64) << (3 * i);
            err += e * e;
        }
        (r0, r1, bits, err)
    };
    let mut best = fit(hi, lo);
    // Six-value mode spans the values away from the extremes.
    let inner = values.iter().filter(|&&v| v != 0 && v != 255);
    let (ilo, ihi) = inner.fold((255u8, 0u8), |(l, h), &v| (l.min(v), h.max(v)));
    if ilo < ihi { let six = fit(ilo, ihi); if six.3 < best.3 { best = six; } }
    for _ in 0..2 {
        if best.3 == 0 { break; }
        let (r0, r1, bits, _) = best;
        // Each index's weight on r1 (palette entries 0 and 1 are the ends).
        let steps = if r0 > r1 { 7.0 } else { 5.0 };
        let (mut a, mut b, mut c, mut x, mut y, mut n) = (0f32, 0f32, 0f32, 0f32, 0f32, 0);
        for (i, &v) in values.iter().enumerate() {
            let k = (bits >> (3 * i)) & 7;
            let t = match k { 0 => 0.0, 1 => 1.0, k if r0 <= r1 && k >= 6 => continue, k => (k - 1) as f32 / steps };
            let v = v as f32;
            a += (1.0 - t) * (1.0 - t); b += (1.0 - t) * t; c += t * t; x += (1.0 - t) * v; y += t * v; n += 1;
        }
        let det = a * c - b * b;
        if n == 0 || det.abs() < 1e-6 { break; }
        let e0 = ((c * x - b * y) / det).round().clamp(0.0, 255.0) as i32;
        let e1 = ((a * y - b * x) / det).round().clamp(0.0, 255.0) as i32;
        let before = best.3;
        for d0 in -1..=1 { for d1 in -1..=1 {
            let (p0, p1) = ((e0 + d0).clamp(0, 255) as u8, (e1 + d1).clamp(0, 255) as u8);
            // Keep the refitted mode: 8 values need r0 > r1, 6 need r0 <= r1.
            let (p0, p1) = if (r0 > r1) == (p0 > p1) { (p0, p1) } else { (p1, p0) };
            if (r0 > r1) != (p0 > p1) { continue; }
            let candidate = fit(p0, p1);
            if candidate.3 < best.3 { best = candidate; }
        } }
        if best.3 >= before { break; }
    }
    let (r0, r1, bits, _) = best;
    let mut b = [r0, r1, 0, 0, 0, 0, 0, 0];
    for k in 0..6 { b[2 + k] = (bits >> (8 * k)) as u8; }
    b
}

/// Encode an RGBA8 image's R and G to BC5 (two BC4 blocks per 4x4 tile,
/// raster order; edge tiles clamp) across `threads` workers (0 = all
/// cores). Build-time work for two-channel data such as tangent-space
/// normal maps (X, Y; Z is rebuilt where sampled).
pub fn encode_bc5_image(rgba: &[u8], width: u32, height: u32, threads: usize) -> Vec<u8> {
    let (w, h) = (width.max(1) as usize, height.max(1) as usize);
    let (bx, by) = (w.div_ceil(4), h.div_ceil(4));
    let mut out = vec![0u8; bx * by * 16];
    let threads = if threads == 0 { std::thread::available_parallelism().map_or(1, |n| n.get()) } else { threads }.clamp(1, by);
    let rows_per = by.div_ceil(threads);
    std::thread::scope(|scope| {
        for (chunk, rows) in out.chunks_mut(rows_per * bx * 16).enumerate() {
            scope.spawn(move || {
                for (j, block) in rows.chunks_exact_mut(16).enumerate() {
                    let index = chunk * rows_per * bx + j;
                    let (x0, y0) = ((index % bx) * 4, (index / bx) * 4);
                    for c in 0..2 {
                        block[c * 8..c * 8 + 8].copy_from_slice(&encode_bc4(&std::array::from_fn(|i| {
                            let (x, y) = ((x0 + i % 4).min(w - 1), (y0 + i / 4).min(h - 1));
                            rgba[(y * w + x) * 4 + c]
                        })));
                    }
                }
            });
        }
    });
    out
}

/// Bytes of one BC4 (`channels == 1`) or BC5 (`2`) level.
pub fn encoded_len(width: u32, height: u32, channels: usize) -> usize {
    (width.max(1) as usize).div_ceil(4) * (height.max(1) as usize).div_ceil(4) * 8 * channels
}

/// A BC5 mip chain in KTX2 (`VK_FORMAT_BC5_UNORM_BLOCK`), levels largest
/// first, with an optional builder-assigned content id (the same key as
/// Basis textures, [`crate::basis::CONTENT_ID_KEY`]).
pub fn bc5_to_ktx2(levels: &[(u32, u32, &[u8])], content_id: Option<u64>) -> Result<Vec<u8>, String> {
    let &(width, height, _) = levels.first().ok_or("no mip levels")?;
    let mut w = ktx2::Writer::new(ktx2::Header {
        vk_format: ktx2::VK_FORMAT_BC5_UNORM_BLOCK, type_size: 1, width, height,
        depth: 0, layers: 0, faces: 1, levels: levels.len() as u32, supercompression: ktx2::SUPERCOMPRESSION_NONE,
    });
    for &(lw, lh, data) in levels {
        if data.len() != encoded_len(lw, lh, 2) { return Err(format!("level {lw}x{lh} has {} BC5 bytes", data.len())); }
        w.level(data.to_vec(), data.len() as u64);
    }
    w.dfd(bc5_dfd());
    w.key_value("KTXwriter", b"makepad-texcomp\0");
    if let Some(id) = content_id { w.key_value(crate::basis::CONTENT_ID_KEY, &id.to_le_bytes()); }
    w.write()
}

/// Basic Data Format Descriptor for BC5 unsigned (Khronos Data Format 1.3:
/// colour model KHR_DF_MODEL_BC5 = 132, a 4x4 block of 16 bytes, samples
/// red (bits 0-63) and green (bits 64-127), linear).
fn bc5_dfd() -> Vec<u8> {
    let block_size: u32 = 24 + 2 * 16;
    let mut d = Vec::with_capacity(4 + block_size as usize);
    d.extend_from_slice(&(4 + block_size).to_le_bytes()); // dfdTotalSize
    d.extend_from_slice(&0u32.to_le_bytes()); // vendorId 0 (Khronos) | descriptorType 0 (basic)
    d.extend_from_slice(&(2u32 | (block_size << 16)).to_le_bytes()); // versionNumber 2 | descriptorBlockSize
    d.extend_from_slice(&[132, 1 /* BT709 */, 1 /* linear */, 0 /* flags */]);
    d.extend_from_slice(&[3, 3, 0, 0]); // texelBlockDimension: 4x4
    d.extend_from_slice(&[16, 0, 0, 0, 0, 0, 0, 0]); // bytesPlane0 = 16
    for (offset, channel) in [(0u32, 0u32), (64, 1)] {
        d.extend_from_slice(&(offset | 63 << 16 | channel << 24).to_le_bytes()); // bitOffset, bitLength-1, channel
        d.extend_from_slice(&[0, 0, 0, 0]); // samplePosition
        d.extend_from_slice(&0u32.to_le_bytes()); // sampleLower
        d.extend_from_slice(&u32::MAX.to_le_bytes()); // sampleUpper
    }
    d
}

/// A BC5 KTX2 read in place (levels borrow the file bytes).
pub struct Bc5View<'a> { pub levels: Vec<(u32, u32, &'a [u8])>, pub content_id: Option<u64> }

impl<'a> Bc5View<'a> {
    /// `Err` for anything but an uncompressed-scheme BC5 KTX2 whose levels
    /// have exactly the bytes their sizes need.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        let r = ktx2::Reader::new(bytes)?;
        let h = r.header().clone();
        if h.vk_format != ktx2::VK_FORMAT_BC5_UNORM_BLOCK { return Err("not a BC5 texture".into()); }
        if h.supercompression != ktx2::SUPERCOMPRESSION_NONE { return Err(format!("supercompression scheme {} is not supported", h.supercompression)); }
        let mut levels = Vec::new();
        for i in 0..r.levels().len() {
            let (width, height) = ((h.width >> i).max(1), (h.height.max(1) >> i).max(1));
            let data = r.level_data(i)?;
            if data.len() != encoded_len(width, height, 2) { return Err(format!("level {i}: {} bytes for {width}x{height}", data.len())); }
            levels.push((width, height, data));
        }
        if levels.is_empty() { return Err("BC5 texture without levels".into()); }
        let content_id = r.key_values().into_iter().find(|(k, _)| k == crate::basis::CONTENT_ID_KEY).and_then(|(_, v)| Some(u64::from_le_bytes(v.get(..8)?.try_into().ok()?)));
        Ok(Bc5View { levels, content_id })
    }
}

/// Transcode a UASTC level to BC4 from one channel (0..=3) of the decode.
pub fn uastc_to_bc4_image(data: &[u8], channel: usize) -> Vec<u8> {
    data.chunks_exact(16).flat_map(|b| {
        let texels = uastc::decode_block(b.try_into().unwrap());
        encode_bc4(&std::array::from_fn(|i| texels[i][channel]))
    }).collect()
}

/// Transcode a UASTC level to BC5 from two channels (e.g. 0 and 1 for XY).
pub fn uastc_to_bc5_image(data: &[u8], channels: [usize; 2]) -> Vec<u8> {
    data.chunks_exact(16).flat_map(|b| {
        let texels = uastc::decode_block(b.try_into().unwrap());
        let mut out = [0u8; 16];
        out[..8].copy_from_slice(&encode_bc4(&std::array::from_fn(|i| texels[i][channels[0]])));
        out[8..].copy_from_slice(&encode_bc4(&std::array::from_fn(|i| texels[i][channels[1]])));
        out
    }).collect()
}

/// Decode a BC4 (`channels == 1`) or BC5 (`2`) image to 8-bit channels.
pub fn decode_image(data: &[u8], width: u32, height: u32, channels: usize) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let bx = w.div_ceil(4);
    let mut out = vec![0u8; w * h * channels];
    for (index, block) in data.chunks_exact(8 * channels).enumerate() {
        let (x0, y0) = ((index % bx) * 4, (index / bx) * 4);
        for c in 0..channels {
            let v = decode_bc4(block[c * 8..c * 8 + 8].try_into().unwrap());
            for y in 0..4 { for x in 0..4 {
                let (px, py) = (x0 + x, y0 + y);
                if px < w && py < h { out[(py * w + px) * channels + c] = v[y * 4 + x]; }
            } }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_follow_the_specification_table() {
        let p = palette(200, 60);
        assert_eq!(p, [200, 60, 180, 160, 140, 120, 100, 80]);
        let p = palette(60, 200);
        assert_eq!(p, [60, 200, 88, 116, 144, 172, 0, 255]);
    }

    #[test]
    fn bc5_ktx2_round_trips_levels_and_content_id() {
        let rgba: Vec<u8> = (0..8 * 8).flat_map(|i| [(i * 4) as u8, (255 - i * 3) as u8, 0, 255]).collect();
        let base = encode_bc5_image(&rgba, 8, 8, 1);
        let small = encode_bc5_image(&rgba[..4 * 4 * 4], 4, 4, 2);
        let bytes = bc5_to_ktx2(&[(8, 8, &base), (4, 4, &small)], Some(0x1234)).unwrap();
        let view = Bc5View::parse(&bytes).unwrap();
        assert_eq!(view.content_id, Some(0x1234));
        assert_eq!(view.levels, vec![(8, 8, &base[..]), (4, 4, &small[..])]);
        let back = decode_image(view.levels[0].2, 8, 8, 2);
        assert!(back.chunks_exact(2).zip(rgba.chunks_exact(4)).all(|(b, s)| b[0].abs_diff(s[0]) <= 8 && b[1].abs_diff(s[1]) <= 8), "8 levels over a block's range");
        assert!(Bc5View::parse(&bytes[..bytes.len() - 1]).is_err(), "truncated");
        let dfd = ktx2::Reader::new(&bytes).unwrap().dfd().unwrap().to_vec();
        assert_eq!(u32::from_le_bytes(dfd[0..4].try_into().unwrap()) as usize, dfd.len());
    }

    #[test]
    fn encode_decode_is_near_lossless_on_smooth_ramps_and_exact_at_extremes() {
        let ramp: [u8; 16] = std::array::from_fn(|i| 40 + i as u8 * 9);
        let back = decode_bc4(&encode_bc4(&ramp));
        // 8 levels over a 135-wide ramp: half a step is under 10.
        assert!(ramp.iter().zip(back).all(|(a, b)| (*a as i32 - b as i32).abs() <= 10));
        let mixed: [u8; 16] = std::array::from_fn(|i| match i % 4 { 0 => 0, 1 => 255, _ => 100 + i as u8 });
        let back = decode_bc4(&encode_bc4(&mixed));
        assert!(mixed.iter().zip(back).all(|(a, b)| (*a as i32 - b as i32).abs() <= 6), "{mixed:?} {back:?}");
        let solid = [77u8; 16];
        assert_eq!(decode_bc4(&encode_bc4(&solid)), solid);
    }
}
