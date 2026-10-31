//! UASTC encoder quality floors: encode a synthetic image, decode it with
//! the UASTC decoder and compare against the source (PSNR per channel group,
//! SSIM on luma).

use makepad_texcomp::uastc::{self, Quality};

/// A 128x128 test image with the content that stresses block codecs:
/// smooth gradients, hard edges, fine noise, thin lines and soft alpha.
pub fn test_image(alpha: bool) -> (Vec<u8>, u32, u32) {
    let (w, h) = (128u32, 128u32);
    let mut seed = 0x9e37_79b9u32;
    let mut noise = move || { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; (seed & 0xff) as i32 };
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h { for x in 0..w {
        let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
        let mut c = [fx * 255.0, fy * 255.0, (1.0 - fx) * 200.0 + 30.0, 255.0];
        // Quadrants: gradient, checker edges, noisy stone, thin lines.
        if x >= 64 && y < 64 { let on = ((x / 8) + (y / 8)) % 2 == 0; c = if on { [230.0, 40.0, 40.0, 255.0] } else { [20.0, 60.0, 200.0, 255.0] }; }
        if x < 64 && y >= 64 { let n = noise() as f32 * 0.25; c = [120.0 + n, 110.0 + n, 95.0 + n * 0.8, 255.0]; }
        if x >= 64 && y >= 64 { c = if x % 7 == 0 || y % 9 == 0 { [250.0, 250.0, 240.0, 255.0] } else { [40.0, 45.0, 50.0, 255.0] }; }
        if alpha {
            let d = ((fx - 0.5).powi(2) + (fy - 0.5).powi(2)).sqrt();
            c[3] = ((1.0 - d * 2.0).clamp(0.0, 1.0) * 255.0).round();
        }
        let i = ((y * w + x) * 4) as usize;
        for k in 0..4 { out[i + k] = c[k].round().clamp(0.0, 255.0) as u8; }
    } }
    (out, w, h)
}

pub fn psnr(a: &[u8], b: &[u8], channels: &[usize]) -> f64 {
    let mut mse = 0f64;
    let mut n = 0f64;
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for &c in channels { let d = pa[c] as f64 - pb[c] as f64; mse += d * d; n += 1.0; }
    }
    mse /= n;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
}

/// Mean SSIM of luma over 8x8 windows (stride 4).
pub fn ssim(a: &[u8], b: &[u8], w: usize, h: usize) -> f64 {
    let luma = |p: &[u8], i: usize| 0.299 * p[i * 4] as f64 + 0.587 * p[i * 4 + 1] as f64 + 0.114 * p[i * 4 + 2] as f64;
    let (c1, c2) = ((0.01f64 * 255.0).powi(2), (0.03f64 * 255.0).powi(2));
    let (mut total, mut count) = (0f64, 0f64);
    for y0 in (0..h.saturating_sub(7)).step_by(4) { for x0 in (0..w.saturating_sub(7)).step_by(4) {
        let (mut ma, mut mb, mut va, mut vb, mut cov) = (0f64, 0f64, 0f64, 0f64, 0f64);
        for y in 0..8 { for x in 0..8 { let i = (y0 + y) * w + x0 + x; ma += luma(a, i); mb += luma(b, i); } }
        ma /= 64.0; mb /= 64.0;
        for y in 0..8 { for x in 0..8 {
            let i = (y0 + y) * w + x0 + x;
            let (da, db) = (luma(a, i) - ma, luma(b, i) - mb);
            va += da * da; vb += db * db; cov += da * db;
        } }
        va /= 63.0; vb /= 63.0; cov /= 63.0;
        total += ((2.0 * ma * mb + c1) * (2.0 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2));
        count += 1.0;
    } }
    total / count
}

#[test]
fn uastc_meets_quality_floors_per_preset() {
    for alpha in [false, true] {
        let (src, w, h) = test_image(alpha);
        // Measured 2026-09-28: RGB 47.9-53.1 dB, alpha 40.0-41.2 dB, SSIM 0.997-0.9996.
        for (quality, rgb_floor, a_floor, ssim_floor) in [(Quality::Fast, 46.0, 38.5, 0.995), (Quality::Default, 46.5, 39.5, 0.995), (Quality::Slow, 47.0, 39.5, 0.995)] {
            let start = std::time::Instant::now();
            let blocks = uastc::encode_image(&src, w, h, quality, 0).unwrap();
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(blocks.len(), uastc::encoded_len(w, h));
            let back = uastc::decode_image(&blocks, w, h).unwrap();
            let (rgb, a, s) = (psnr(&src, &back, &[0, 1, 2]), psnr(&src, &back, &[3]), ssim(&src, &back, w as usize, h as usize));
            eprintln!("UASTC {quality:?} alpha={alpha}: RGB {rgb:.2} dB, A {a:.2} dB, SSIM {s:.4}, {ms:.1} ms for {w}x{h}");
            assert!(rgb >= rgb_floor, "{quality:?} alpha={alpha}: RGB PSNR {rgb:.2} < {rgb_floor}");
            assert!(a >= a_floor, "{quality:?} alpha={alpha}: alpha PSNR {a:.2} < {a_floor}");
            assert!(s >= ssim_floor, "{quality:?} alpha={alpha}: SSIM {s:.4} < {ssim_floor}");
        }
    }
}

#[test]
fn solid_and_edge_sizes_round_trip() {
    for (w, h) in [(1u32, 1u32), (3, 5), (5, 3), (4, 4), (9, 7)] {
        let src: Vec<u8> = (0..w * h).flat_map(|i| [(i * 37 % 256) as u8, 90, 200, 255]).collect();
        let blocks = uastc::encode_image(&src, w, h, Quality::Default, 1).unwrap();
        let back = uastc::decode_image(&blocks, w, h).unwrap();
        assert_eq!(back.len(), src.len());
        assert!(psnr(&src, &back, &[0, 1, 2]) > 30.0, "{w}x{h}");
    }
    let solid = [17u8, 34, 51, 68].repeat(16);
    let blocks = uastc::encode_image(&solid, 4, 4, Quality::Fast, 1).unwrap();
    assert_eq!(uastc::block_mode(blocks[..16].try_into().unwrap()), Some(uastc::MODE_SOLID));
    assert_eq!(uastc::decode_image(&blocks, 4, 4).unwrap(), solid);
}

#[test]
fn uastc_to_bc7_stays_close_to_uastc() {
    use makepad_texcomp::{bc7, transcode};
    for alpha in [false, true] {
        let (src, w, h) = test_image(alpha);
        let blocks = uastc::encode_image(&src, w, h, Quality::Default, 0).unwrap();
        let direct = uastc::decode_image(&blocks, w, h).unwrap();
        let start = std::time::Instant::now();
        let bc7_blocks = transcode::uastc_to_bc7_image(&blocks);
        let us = start.elapsed().as_secs_f64() * 1e6;
        let via = bc7::decode_image(&bc7_blocks, w, h).unwrap();
        let (u_rgb, b_rgb) = (psnr(&src, &direct, &[0, 1, 2]), psnr(&src, &via, &[0, 1, 2]));
        let (u_a, b_a) = (psnr(&src, &direct, &[3]), psnr(&src, &via, &[3]));
        let s = ssim(&src, &via, w as usize, h as usize);
        eprintln!("UASTC->BC7 alpha={alpha}: RGB {b_rgb:.2} dB (UASTC {u_rgb:.2}), A {b_a:.2} dB (UASTC {u_a:.2}), SSIM {s:.4}, {us:.0} us for {} blocks", blocks.len() / 16);
        // Spec: BC7 lands within ~0.75-1.5 dB of the UASTC result on typical
        // content. RGB dual-plane blocks (UASTC mode 6 → BC7 mode 5) re-round
        // colour endpoints to 7 bits (about 52.8 dB on their own), so an image
        // coded mostly in that mode loses more; it still has to stay high.
        assert!(b_rgb >= u_rgb - 2.0 || b_rgb >= 48.0, "RGB {b_rgb:.2} vs UASTC {u_rgb:.2}");
        assert!(b_a >= u_a - 2.0 || b_a >= 45.0, "alpha {b_a:.2} vs UASTC {u_a:.2}");
        assert!(s >= 0.99);
        if !alpha { assert!(via.chunks_exact(4).all(|p| p[3] == 255), "opaque stays opaque"); }
    }
}

#[test]
fn uastc_to_astc_is_lossless() {
    use makepad_texcomp::{astc, transcode};
    for alpha in [false, true] {
        for quality in [Quality::Fast, Quality::Slow] {
            let (src, w, h) = test_image(alpha);
            let blocks = uastc::encode_image(&src, w, h, quality, 0).unwrap();
            let start = std::time::Instant::now();
            let astc_blocks = transcode::uastc_to_astc_image(&blocks);
            let us = start.elapsed().as_secs_f64() * 1e6;
            let mut modes = [0usize; 19];
            for (u, a) in blocks.chunks_exact(16).zip(astc_blocks.chunks_exact(16)) {
                let (u, a): (&[u8; 16], &[u8; 16]) = (u.try_into().unwrap(), a.try_into().unwrap());
                modes[uastc::block_mode(u).unwrap()] += 1;
                assert_eq!(astc::decode_block(a).expect("decodable ASTC"), uastc::decode_block(u), "mode {:?}", uastc::block_mode(u));
            }
            eprintln!("UASTC->ASTC {quality:?} alpha={alpha}: lossless over {} blocks in {us:.0} us; modes used {modes:?}", blocks.len() / 16);
        }
    }
}

#[test]
fn uastc_to_bc4_bc5_keep_channel_quality() {
    use makepad_texcomp::rgtc;
    let (src, w, h) = test_image(true);
    let blocks = uastc::encode_image(&src, w, h, Quality::Default, 0).unwrap();
    // BC5 from RG (a normal map's XY), BC4 from alpha (single channel).
    let bc5 = rgtc::decode_image(&rgtc::uastc_to_bc5_image(&blocks, [0, 1]), w, h, 2);
    let bc4 = rgtc::decode_image(&rgtc::uastc_to_bc4_image(&blocks, 3), w, h, 1);
    let chan = |data: &[u8], stride: usize, c: usize, src_c: usize| {
        let mse = (0..(w * h) as usize).map(|i| { let d = data[i * stride + c] as f64 - src[i * 4 + src_c] as f64; d * d }).sum::<f64>() / (w * h) as f64;
        10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
    };
    let (r, g, a) = (chan(&bc5, 2, 0, 0), chan(&bc5, 2, 1, 1), chan(&bc4, 1, 0, 3));
    eprintln!("UASTC->BC5 R {r:.2} dB, G {g:.2} dB; UASTC->BC4 A {a:.2} dB");
    assert!(r >= 40.0 && g >= 40.0 && a >= 38.0);
}

#[test]
fn uastc_to_etc2_rgba_meets_its_floor() {
    use makepad_texcomp::etc;
    for alpha in [false, true] {
        let (src, w, h) = test_image(alpha);
        let blocks = uastc::encode_image(&src, w, h, Quality::Default, 0).unwrap();
        let start = std::time::Instant::now();
        let etc2 = etc::uastc_to_etc2_rgba_image(&blocks);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let back = etc::decode_rgba_image(&etc2, w, h);
        let (rgb, a, s) = (psnr(&src, &back, &[0, 1, 2]), psnr(&src, &back, &[3]), ssim(&src, &back, w as usize, h as usize));
        eprintln!("UASTC->ETC2 alpha={alpha}: RGB {rgb:.2} dB, A {a:.2} dB, SSIM {s:.4}, {ms:.1} ms for {} blocks", blocks.len() / 16);
        // ETC is the lowest-quality target (4 bpp colour): a lower floor.
        assert!(rgb >= 32.0 && a >= 36.0 && s >= 0.97);
    }
}
