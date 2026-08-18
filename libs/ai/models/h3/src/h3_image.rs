//! Keyframe image ops for the H3 fl2va (i2v) workflow: the PIL-exact LANCZOS
//! canvas resize.
//!
//! The reference preprocessing (before_encoder.py MiniMaxH3ResizeStep)
//! stretches the first keyframe onto the canvas with PIL
//! `Image.resize((W, H), Image.LANCZOS)`. This is a port of Pillow's
//! Resample.c 8-bit path, reproducing its FIXED-POINT arithmetic exactly
//! (not a float approximation):
//!
//! * separable resample, horizontal pass first into a (dst_w, src_h) u8
//!   intermediate, then the vertical pass — each pass re-quantizes to u8;
//! * Lanczos kernel a=3 (`sinc(x) * sinc(x/3)` on the half-open [-3, 3)),
//!   filter support scaled by `max(1, src/dst)` per axis, tap windows
//!   clipped to the image, per-pixel f64 coefficient normalization
//!   (window coefficients sum to 1);
//! * coefficients quantized to i32 with PRECISION_BITS = 32-8-2 = 22 and
//!   round-half-away-from-zero; accumulation in i32 seeded with the
//!   1 << 21 rounding term; clip8 to [0, 255].
//!
//! Because the fixed-point pipeline is reproduced bit-for-bit, the output
//! matches PIL byte-exactly (verified against Pillow 12.1.1 vectors in the
//! tests below), not merely within ±1 LSB.

/// Pillow's PRECISION_BITS for 8-bit channels (Resample.c: 32 - 8 - 2).
const PRECISION_BITS: u32 = 22;

/// Lanczos a=3 kernel on the half-open support [-3, 3) — Pillow's
/// `lanczos_filter` (note the asymmetric boundary: -3 included, +3 not).
fn lanczos_filter(x: f64) -> f64 {
    #[inline]
    fn sinc(x: f64) -> f64 {
        if x == 0.0 {
            1.0
        } else {
            let x = x * std::f64::consts::PI;
            x.sin() / x
        }
    }
    if (-3.0..3.0).contains(&x) {
        sinc(x) * sinc(x / 3.0)
    } else {
        0.0
    }
}

/// One axis' resample plan: per output pixel the source tap window and the
/// normalized fixed-point coefficients (Pillow `precompute_coeffs` +
/// `normalize_coeffs_8bpc`).
struct AxisCoeffs {
    ksize: usize,
    /// (first source index, tap count) per output pixel.
    bounds: Vec<(usize, usize)>,
    /// (out_size * ksize) i32 coefficients, `coeff * 2^22` rounded
    /// half-away-from-zero.
    k: Vec<i32>,
}

fn precompute_coeffs(in_size: usize, out_size: usize) -> AxisCoeffs {
    let scale = in_size as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = 3.0 * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let inv_scale = 1.0 / filterscale;
    let mut bounds = Vec::with_capacity(out_size);
    let mut k = vec![0i32; out_size * ksize];
    let mut window = vec![0f64; ksize];
    for xx in 0..out_size {
        let center = (xx as f64 + 0.5) * scale;
        // C `(int)` casts truncate toward zero; both bounds are clamped to
        // the image so the trunc/floor difference on negatives is moot.
        let xmin = ((center - support + 0.5) as i64).max(0) as usize;
        let xmax = (((center + support + 0.5) as i64).min(in_size as i64)) as usize;
        let count = xmax - xmin;
        let mut total = 0.0f64;
        for (x, slot) in window[..count].iter_mut().enumerate() {
            let w = lanczos_filter((x as f64 + xmin as f64 - center + 0.5) * inv_scale);
            *slot = w;
            total += w;
        }
        for x in 0..count {
            let coeff = if total != 0.0 { window[x] / total } else { window[x] };
            let scaled = coeff * (1u64 << PRECISION_BITS) as f64;
            // Round half away from zero, truncating cast like the C code.
            k[xx * ksize + x] =
                if coeff < 0.0 { (scaled - 0.5) as i32 } else { (scaled + 0.5) as i32 };
        }
        bounds.push((xmin, count));
    }
    AxisCoeffs { ksize, bounds, k }
}

/// Pillow `clip8`: the i32 accumulator (already seeded with the 1 << 21
/// rounding term) to a u8 channel.
#[inline]
fn clip8(acc: i32) -> u8 {
    if acc >= 1 << (PRECISION_BITS + 8) {
        255
    } else if acc <= 0 {
        0
    } else {
        (acc >> PRECISION_BITS) as u8
    }
}

fn resample_horizontal(src: &[u8], src_w: usize, src_h: usize, coeffs: &AxisCoeffs) -> Vec<u8> {
    let dst_w = coeffs.bounds.len();
    let mut out = vec![0u8; dst_w * src_h * 3];
    for y in 0..src_h {
        let src_row = &src[y * src_w * 3..(y + 1) * src_w * 3];
        for (xx, &(xmin, count)) in coeffs.bounds.iter().enumerate() {
            let k = &coeffs.k[xx * coeffs.ksize..xx * coeffs.ksize + count];
            let mut acc = [1i32 << (PRECISION_BITS - 1); 3];
            for (x, &coeff) in k.iter().enumerate() {
                let pixel = &src_row[(xmin + x) * 3..(xmin + x) * 3 + 3];
                for c in 0..3 {
                    acc[c] = acc[c].wrapping_add(pixel[c] as i32 * coeff);
                }
            }
            let dst = &mut out[(y * dst_w + xx) * 3..(y * dst_w + xx) * 3 + 3];
            for c in 0..3 {
                dst[c] = clip8(acc[c]);
            }
        }
    }
    out
}

fn resample_vertical(src: &[u8], width: usize, coeffs: &AxisCoeffs) -> Vec<u8> {
    let dst_h = coeffs.bounds.len();
    let mut out = vec![0u8; width * dst_h * 3];
    for (yy, &(ymin, count)) in coeffs.bounds.iter().enumerate() {
        let k = &coeffs.k[yy * coeffs.ksize..yy * coeffs.ksize + count];
        for x in 0..width {
            let mut acc = [1i32 << (PRECISION_BITS - 1); 3];
            for (y, &coeff) in k.iter().enumerate() {
                let pixel = &src[((ymin + y) * width + x) * 3..((ymin + y) * width + x) * 3 + 3];
                for c in 0..3 {
                    acc[c] = acc[c].wrapping_add(pixel[c] as i32 * coeff);
                }
            }
            let dst = &mut out[(yy * width + x) * 3..(yy * width + x) * 3 + 3];
            for c in 0..3 {
                dst[c] = clip8(acc[c]);
            }
        }
    }
    out
}

/// PIL `Image.resize((dst_w, dst_h), Image.LANCZOS)` for interleaved RGB u8
/// (row-major (h, w, 3)) — byte-exact port of Pillow's Resample.c. Like PIL,
/// an axis whose size does not change skips its pass entirely.
pub fn resize_rgb_lanczos3(
    src: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<u8> {
    assert!(src_w > 0 && src_h > 0 && dst_w > 0 && dst_h > 0, "resize: empty image");
    assert_eq!(src.len(), src_w * src_h * 3, "resize: src is not (h, w, 3) u8");
    let need_horizontal = dst_w != src_w;
    let need_vertical = dst_h != src_h;
    if !need_horizontal && !need_vertical {
        return src.to_vec();
    }
    let mut image = Vec::new();
    let mut current: &[u8] = src;
    if need_horizontal {
        image = resample_horizontal(current, src_w, src_h, &precompute_coeffs(src_w, dst_w));
        current = &image;
    }
    if need_vertical {
        image = resample_vertical(current, dst_w, &precompute_coeffs(src_h, dst_h));
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The deterministic test pattern shared with the PIL vector generator
    /// (scratchpad gen_lanczos_vectors.py, Pillow 12.1.1).
    fn test_pattern(w: usize, h: usize) -> Vec<u8> {
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    data.push(((x * 31 + y * 17 + c * 97 + (x * y % 7) * 13) % 256) as u8);
                }
            }
        }
        data
    }

    fn fnv1a(data: &[u8]) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for &byte in data {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    #[test]
    fn identity_is_exact_passthrough() {
        let src = test_pattern(9, 7);
        assert_eq!(resize_rgb_lanczos3(&src, 9, 7, 9, 7), src);
    }

    #[test]
    fn coeffs_downscale_by_2_symmetry() {
        // Exact 2x downscale: every output pixel's window is symmetric
        // around its center, so the fixed-point taps mirror pairwise, and
        // each window sums to 2^22 (+/- the half-away rounding of each tap).
        let coeffs = precompute_coeffs(8, 4);
        assert_eq!(coeffs.bounds.len(), 4);
        // Interior pixel (index 2): center 5.0, full 12-tap window.
        let (xmin, count) = coeffs.bounds[2];
        assert_eq!((xmin, count), (0, 8)); // clipped to the image
        let k = &coeffs.k[2 * coeffs.ksize..2 * coeffs.ksize + count];
        // Window centered at source 5.0 over taps 0..8: tap distances
        // -4.5..3.5 in steps of 1 (scaled by 1/2) — symmetric pairs around
        // the center are (1,8)=(-4.5,+3.5)? No: pairs equidistant from 5.0
        // are taps (2,7), (3,6), (4,5) at distances +/-2.5, +/-1.5, +/-0.5.
        assert_eq!(k[4], k[5]);
        assert_eq!(k[3], k[6]);
        assert_eq!(k[2], k[7]);
        let sum: i64 = k.iter().map(|&v| v as i64).sum();
        assert!((sum - (1 << PRECISION_BITS)).abs() <= count as i64);
    }

    #[test]
    fn matches_pil_7x5_to_4x3() {
        let src = test_pattern(7, 5);
        let expected: [u8; 36] = [
            19, 114, 223, 78, 196, 63, 155, 183, 75, 152, 113, 152, 56, 163, 171, 150, 163, 87,
            222, 27, 137, 121, 95, 201, 94, 205, 85, 180, 130, 103, 210, 53, 169, 70, 128, 218,
        ];
        assert_eq!(resize_rgb_lanczos3(&src, 7, 5, 4, 3), expected);
    }

    #[test]
    fn matches_pil_8x8_to_4x4() {
        let src = test_pattern(8, 8);
        let expected: [u8; 48] = [
            27, 127, 202, 107, 194, 56, 162, 140, 116, 179, 79, 170, 73, 192, 136, 184, 114, 97,
            205, 40, 169, 84, 118, 216, 126, 152, 67, 199, 76, 135, 86, 112, 213, 23, 140, 226,
            149, 154, 85, 220, 51, 153, 68, 112, 223, 50, 156, 139,
        ];
        assert_eq!(resize_rgb_lanczos3(&src, 8, 8, 4, 4), expected);
    }

    #[test]
    fn matches_pil_4x4_to_8x8_upscale() {
        let src = test_pattern(4, 4);
        let expected: [u8; 192] = [
            0, 93, 181, 2, 99, 208, 19, 115, 231, 37, 135, 168, 49, 147, 39, 68, 162, 0, 84, 173,
            9, 91, 177, 38, 2, 96, 181, 7, 104, 218, 25, 122, 253, 45, 140, 198, 59, 153, 55, 79,
            178, 2, 96, 204, 19, 103, 217, 52, 9, 105, 187, 17, 114, 231, 38, 137, 255, 62, 156,
            226, 80, 168, 79, 103, 200, 17, 123, 239, 46, 132, 255, 81, 17, 115, 211, 28, 124,
            223, 54, 147, 218, 82, 188, 172, 103, 222, 80, 134, 232, 52, 163, 221, 91, 176, 209,
            122, 24, 123, 244, 37, 130, 195, 66, 152, 112, 97, 218, 49, 119, 255, 55, 156, 232,
            94, 192, 135, 129, 208, 71, 148, 34, 129, 255, 48, 147, 187, 81, 177, 64, 120, 218, 0,
            148, 215, 61, 175, 146, 125, 196, 49, 137, 202, 0, 139, 43, 132, 255, 57, 165, 200,
            93, 208, 85, 141, 199, 30, 176, 116, 92, 188, 42, 136, 183, 11, 124, 176, 2, 114, 46,
            132, 253, 62, 175, 210, 98, 225, 109, 151, 184, 62, 189, 54, 112, 191, 0, 136, 172, 0,
            112, 157, 23, 95,
        ];
        assert_eq!(resize_rgb_lanczos3(&src, 4, 4, 8, 8), expected);
    }

    #[test]
    fn matches_pil_vertical_only_6x3_to_6x5() {
        let src = test_pattern(6, 3);
        let expected: [u8; 90] = [
            0, 96, 193, 29, 126, 214, 59, 156, 0, 89, 177, 27, 116, 240, 54, 146, 255, 84, 4, 101,
            198, 38, 135, 255, 72, 169, 10, 106, 229, 44, 149, 162, 87, 183, 196, 121, 17, 114,
            211, 61, 158, 255, 105, 202, 43, 149, 246, 87, 193, 34, 131, 237, 78, 175, 30, 127,
            224, 84, 181, 106, 138, 235, 76, 192, 117, 130, 185, 0, 123, 239, 54, 177, 35, 132,
            229, 93, 190, 4, 151, 248, 89, 209, 23, 147, 166, 16, 104, 224, 74, 162,
        ];
        assert_eq!(resize_rgb_lanczos3(&src, 6, 3, 6, 5), expected);
    }

    #[test]
    fn matches_pil_horizontal_only_5x4_to_9x4() {
        let src = test_pattern(5, 4);
        let expected: [u8; 108] = [
            0, 94, 182, 6, 103, 219, 27, 124, 235, 47, 144, 127, 62, 159, 0, 77, 174, 0, 97, 194,
            40, 118, 215, 58, 127, 224, 65, 13, 110, 198, 26, 123, 239, 55, 152, 255, 83, 175,
            163, 105, 202, 43, 127, 255, 37, 155, 226, 98, 184, 94, 125, 197, 7, 135, 29, 126,
            254, 46, 140, 171, 83, 175, 47, 118, 242, 18, 148, 245, 86, 190, 144, 133, 204, 30,
            142, 184, 6, 122, 165, 15, 103, 45, 133, 255, 65, 180, 189, 109, 221, 74, 166, 149,
            66, 191, 32, 129, 175, 0, 118, 175, 20, 113, 219, 63, 157, 249, 90, 187,
        ];
        assert_eq!(resize_rgb_lanczos3(&src, 5, 4, 9, 4), expected);
    }

    #[test]
    fn matches_pil_canvas_geometry_960x544_to_640x352() {
        // The real fl2va canvas geometry, checked as an fnv1a-64 digest of
        // the full output against Pillow 12.1.1.
        let src = test_pattern(960, 544);
        assert_eq!(fnv1a(&src), 0x4dc8d6ebc4b58bff, "test pattern generator drifted");
        let out = resize_rgb_lanczos3(&src, 960, 544, 640, 352);
        assert_eq!(out.len(), 640 * 352 * 3);
        assert_eq!(fnv1a(&out), 0xfe85a2c64a531430);
    }
}
