//! Pure-CPU image preprocessing for the `control` domain (FLUX.1-Depth-dev /
//! FLUX.1-Canny-dev): PNG decode helpers, depth-mm -> normalized grayscale,
//! and a from-scratch Canny edge detector. No GPU, no external process — the
//! whole file is unit-testable on any machine, which is why it lives outside
//! the `flux` cargo feature (`control_backend.rs`, which drives the actual
//! GPU pipeline, is feature-gated; this preprocessing is not).
//!
//! Depth normalization convention: da3-metric-large's 16-bit PNG carries
//! metric depth in millimeters (0 = invalid/no-return). FLUX.1-Depth-dev was
//! trained on MiDaS/DPT-style *inverse* depth maps, the convention the whole
//! depth-ControlNet ecosystem (SD1.5/SDXL depth ControlNets included) shares:
//! near = bright, far = dark. `normalize_depth_mm` reproduces that — it
//! min/max-normalizes the valid range and inverts it, mapping invalid (0mm)
//! pixels to black (far). This directionality is a documented judgment call,
//! not something verified against the real checkpoint (no GPU in this
//! environment) — see the crate-level control_backend.rs doc note.

use crate::error::AssetAiError;
use makepad_zune_core::options::DecoderOptions;
use makepad_zune_core::result::DecodingResult;
use makepad_zune_png::PngDecoder;
use std::io::BufReader;

// ---------------------------------------------------------------------------
// PNG decode
// ---------------------------------------------------------------------------

/// Decodes a single-channel 16-bit grayscale PNG into raw samples plus
/// dimensions. This is the da3-metric-large depth contract
/// (`depth_backend::check_depth_output`): color type 0 (grayscale), 16-bit.
pub fn decode_png_gray16(bytes: &[u8]) -> Result<(Vec<u16>, u32, u32), AssetAiError> {
    let cursor = std::io::Cursor::new(bytes.to_vec());
    let reader = BufReader::new(cursor);
    let options = DecoderOptions::default();
    let mut decoder = PngDecoder::new_with_options(reader, options);
    decoder
        .decode_headers()
        .map_err(|err| AssetAiError::Params(format!("png decode: {err:?}")))?;
    let info = decoder
        .info()
        .cloned()
        .ok_or_else(|| AssetAiError::Params("png decode: no info".into()))?;
    let colorspace = decoder
        .colorspace()
        .ok_or_else(|| AssetAiError::Params("png decode: no colorspace".into()))?;
    if colorspace.num_components() != 1 {
        return Err(AssetAiError::Params(format!(
            "expected single-channel grayscale png, got {} channels",
            colorspace.num_components()
        )));
    }
    let (width, height) = (info.width as u32, info.height as u32);
    match decoder
        .decode()
        .map_err(|err| AssetAiError::Params(format!("png decode: {err:?}")))?
    {
        DecodingResult::U16(samples) => {
            if samples.len() != width as usize * height as usize {
                return Err(AssetAiError::Params(format!(
                    "gray16 png sample count {} != {}x{}",
                    samples.len(),
                    width,
                    height
                )));
            }
            Ok((samples, width, height))
        }
        DecodingResult::U8(samples) => Err(AssetAiError::Params(format!(
            "expected 16-bit grayscale png, decoded as 8-bit ({} samples)",
            samples.len()
        ))),
        _ => Err(AssetAiError::Params(
            "png decode: unsupported sample format".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Depth -> normalized grayscale RGB01
// ---------------------------------------------------------------------------

/// Normalizes a metric depth-in-millimeters map (0 = invalid) to interleaved
/// HWC RGB01 (each pixel's R/G/B replicated from the same normalized
/// grayscale value) — the "0..1 grayscale RGB" the control image encoder
/// expects. See the module doc for the near/far convention.
///
/// Invalid (0mm) pixels map to 0.0 (far/black) rather than being included in
/// the min/max range, so a handful of no-return pixels never compress the
/// normalized range of the real scene.
pub fn normalize_depth_mm(depth_mm: &[u16], width: u32, height: u32) -> Result<Vec<f32>, AssetAiError> {
    let expected = width as usize * height as usize;
    if depth_mm.len() != expected {
        return Err(AssetAiError::Params(format!(
            "normalize_depth_mm expected {} samples, got {}",
            expected,
            depth_mm.len()
        )));
    }
    let mut min = u16::MAX;
    let mut max = 0u16;
    let mut any_valid = false;
    for &value in depth_mm {
        if value == 0 {
            continue;
        }
        any_valid = true;
        min = min.min(value);
        max = max.max(value);
    }
    let mut out = vec![0.0f32; expected * 3];
    if !any_valid || max <= min {
        // No valid depth (or a perfectly flat scene): everything is black —
        // a degenerate-but-safe control image rather than an error.
        return Ok(out);
    }
    let span = (max - min) as f32;
    for (i, &value) in depth_mm.iter().enumerate() {
        let gray = if value == 0 {
            0.0
        } else {
            // Near = bright: invert the min/max-normalized value.
            1.0 - ((value - min) as f32 / span)
        };
        out[i * 3] = gray;
        out[i * 3 + 1] = gray;
        out[i * 3 + 2] = gray;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// RGB helpers
// ---------------------------------------------------------------------------

/// Standard luma (ITU-R BT.601) from interleaved u8 RGB (any extra channels,
/// e.g. alpha, must already be stripped — 3 bytes/pixel expected).
pub fn rgb8_to_gray_u8(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, AssetAiError> {
    let expected = width as usize * height as usize * 3;
    if rgb.len() != expected {
        return Err(AssetAiError::Params(format!(
            "rgb8_to_gray_u8 expected {expected} bytes, got {}",
            rgb.len()
        )));
    }
    Ok(rgb
        .chunks_exact(3)
        .map(|px| {
            let r = px[0] as f32;
            let g = px[1] as f32;
            let b = px[2] as f32;
            (0.299 * r + 0.587 * g + 0.114 * b).round().clamp(0.0, 255.0) as u8
        })
        .collect())
}

/// Interleaved HWC u8 grayscale (single channel) replicated to HWC RGB01.
pub fn gray_u8_to_rgb01(gray: &[u8]) -> Vec<f32> {
    let mut out = Vec::with_capacity(gray.len() * 3);
    for &value in gray {
        let v = value as f32 / 255.0;
        out.push(v);
        out.push(v);
        out.push(v);
    }
    out
}

/// Interleaved HWC RGB01 (`width*height*3` floats) -> channel-planar CHW01
/// (`[c][y][x]`, `c` outermost) — the layout `flux_vae`'s encoder wants.
pub fn hwc01_to_chw01(hwc: &[f32], width: u32, height: u32, channels: usize) -> Vec<f32> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0.0f32; w * h * channels];
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * channels;
            for c in 0..channels {
                out[c * w * h + y * w + x] = hwc[src + c];
            }
        }
    }
    out
}

/// Bilinear resize of interleaved HWC f32 data (any channel count). Used
/// only when a request overrides the default "control image size rounded to
/// 16" canvas — the common case needs no resize at all.
pub fn resize_hwc_bilinear(
    src: &[f32],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    channels: usize,
) -> Result<Vec<f32>, AssetAiError> {
    let (sw, sh) = (src_w as usize, src_h as usize);
    if src.len() != sw * sh * channels {
        return Err(AssetAiError::Params(format!(
            "resize_hwc_bilinear expected {} values, got {}",
            sw * sh * channels,
            src.len()
        )));
    }
    if dst_w == 0 || dst_h == 0 {
        return Err(AssetAiError::Params(
            "resize_hwc_bilinear: destination size must be non-zero".into(),
        ));
    }
    let (dw, dh) = (dst_w as usize, dst_h as usize);
    if sw == dw && sh == dh {
        return Ok(src.to_vec());
    }
    let mut out = vec![0.0f32; dw * dh * channels];
    let scale_x = sw as f32 / dw as f32;
    let scale_y = sh as f32 / dh as f32;
    let sample = |x: usize, y: usize, c: usize| -> f32 {
        src[(y.min(sh - 1) * sw + x.min(sw - 1)) * channels + c]
    };
    for y in 0..dh {
        // Half-pixel-center mapping (matches standard image resamplers).
        let fy = ((y as f32 + 0.5) * scale_y - 0.5).max(0.0);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = fy - y0 as f32;
        for x in 0..dw {
            let fx = ((x as f32 + 0.5) * scale_x - 0.5).max(0.0);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let tx = fx - x0 as f32;
            for c in 0..channels {
                let top = sample(x0, y0, c) * (1.0 - tx) + sample(x1, y0, c) * tx;
                let bottom = sample(x0, y1, c) * (1.0 - tx) + sample(x1, y1, c) * tx;
                out[(y * dw + x) * channels + c] = top * (1.0 - ty) + bottom * ty;
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Canny edge detector (Gaussian blur -> Sobel -> non-max suppression ->
// double-threshold hysteresis). Textbook implementation, CPU-only.
// ---------------------------------------------------------------------------

pub const CANNY_DEFAULT_LOW: f32 = 50.0;
pub const CANNY_DEFAULT_HIGH: f32 = 200.0;

/// Runs Canny edge detection on an interleaved u8 grayscale image (one byte
/// per pixel), returning a same-size white-edges-on-black u8 map (255 =
/// edge, 0 = not). `low`/`high` are gradient-magnitude thresholds in the
/// same units as the Sobel magnitude (roughly 0..~1443 for 8-bit input);
/// the OpenCV-style defaults are 50/200.
pub fn canny_edges_u8(gray: &[u8], width: u32, height: u32, low: f32, high: f32) -> Result<Vec<u8>, AssetAiError> {
    let expected = width as usize * height as usize;
    if gray.len() != expected {
        return Err(AssetAiError::Params(format!(
            "canny_edges_u8 expected {expected} bytes, got {}",
            gray.len()
        )));
    }
    if width < 3 || height < 3 {
        // Too small for a 3x3/5x5 neighborhood to mean anything: no edges.
        return Ok(vec![0u8; expected]);
    }
    let low = low.max(0.0);
    let high = high.max(low);

    let blurred = gaussian_blur_5x5(gray, width, height);
    let (magnitude, angle) = sobel_gradients(&blurred, width, height);
    let suppressed = non_max_suppress(&magnitude, &angle, width, height);
    Ok(hysteresis_threshold(&suppressed, width, height, low, high))
}

/// Standard 5x5 Gaussian kernel (sigma ~= 1.4, sum 159) with replicated
/// (clamped) border handling.
fn gaussian_blur_5x5(gray: &[u8], width: u32, height: u32) -> Vec<f32> {
    const KERNEL: [i32; 25] = [
        2, 4, 5, 4, 2, //
        4, 9, 12, 9, 4, //
        5, 12, 15, 12, 5, //
        4, 9, 12, 9, 4, //
        2, 4, 5, 4, 2,
    ];
    const KSUM: f32 = 159.0;
    let (w, h) = (width as i64, height as i64);
    let at = |x: i64, y: i64| -> f32 {
        let cx = x.clamp(0, w - 1) as usize;
        let cy = y.clamp(0, h - 1) as usize;
        gray[cy * w as usize + cx] as f32
    };
    let mut out = vec![0.0f32; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f32;
            let mut k = 0usize;
            for dy in -2i64..=2 {
                for dx in -2i64..=2 {
                    acc += at(x + dx, y + dy) * KERNEL[k] as f32;
                    k += 1;
                }
            }
            out[(y * w + x) as usize] = acc / KSUM;
        }
    }
    out
}

/// Sobel gradient magnitude + angle (degrees, wrapped to [0, 180)).
fn sobel_gradients(blurred: &[f32], width: u32, height: u32) -> (Vec<f32>, Vec<f32>) {
    let (w, h) = (width as i64, height as i64);
    let at = |x: i64, y: i64| -> f32 {
        let cx = x.clamp(0, w - 1) as usize;
        let cy = y.clamp(0, h - 1) as usize;
        blurred[cy * w as usize + cx]
    };
    let mut magnitude = vec![0.0f32; (w * h) as usize];
    let mut angle = vec![0.0f32; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let gx = -at(x - 1, y - 1) + at(x + 1, y - 1) - 2.0 * at(x - 1, y) + 2.0 * at(x + 1, y)
                - at(x - 1, y + 1)
                + at(x + 1, y + 1);
            let gy = at(x - 1, y - 1) + 2.0 * at(x, y - 1) + at(x + 1, y - 1)
                - at(x - 1, y + 1)
                - 2.0 * at(x, y + 1)
                - at(x + 1, y + 1);
            let idx = (y * w + x) as usize;
            magnitude[idx] = (gx * gx + gy * gy).sqrt();
            let mut deg = gy.atan2(gx).to_degrees();
            if deg < 0.0 {
                deg += 180.0;
            }
            if deg >= 180.0 {
                deg -= 180.0;
            }
            angle[idx] = deg;
        }
    }
    (magnitude, angle)
}

/// Thins the gradient magnitude to single-pixel-wide ridges: a pixel
/// survives only if its magnitude is >= both neighbors along the (quantized
/// to 0/45/90/135 degree) gradient direction.
fn non_max_suppress(magnitude: &[f32], angle: &[f32], width: u32, height: u32) -> Vec<f32> {
    let (w, h) = (width as usize, height as usize);
    let mag_at = |x: i64, y: i64| -> f32 {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            0.0
        } else {
            magnitude[y as usize * w + x as usize]
        }
    };
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let m = magnitude[idx];
            if m <= 0.0 {
                continue;
            }
            let deg = angle[idx];
            // Quantize to one of 4 directions.
            let (dx, dy) = if !(22.5..157.5).contains(&deg) {
                (1i64, 0i64) // 0 deg (horizontal gradient -> vertical edge)
            } else if deg < 67.5 {
                (1, 1) // 45 deg
            } else if deg < 112.5 {
                (0, 1) // 90 deg
            } else {
                (1, -1) // 135 deg
            };
            let (xi, yi) = (x as i64, y as i64);
            let neighbor_a = mag_at(xi + dx, yi + dy);
            let neighbor_b = mag_at(xi - dx, yi - dy);
            if m >= neighbor_a && m >= neighbor_b {
                out[idx] = m;
            }
        }
    }
    out
}

/// Double-threshold classification + hysteresis: pixels >= `high` are
/// strong edges; pixels in `[low, high)` are weak and only survive if
/// 8-connected to a strong edge (transitively, flood-filled).
fn hysteresis_threshold(suppressed: &[f32], width: u32, height: u32, low: f32, high: f32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; w * h];
    let mut stack = Vec::new();
    for (idx, &value) in suppressed.iter().enumerate() {
        if value >= high {
            out[idx] = 255;
            stack.push(idx);
        }
    }
    while let Some(idx) = stack.pop() {
        let x = (idx % w) as i64;
        let y = (idx / w) as i64;
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = x + dx;
                let ny = y + dy;
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let nidx = ny as usize * w + nx as usize;
                if out[nidx] == 0 && suppressed[nidx] >= low {
                    out[nidx] = 255;
                    stack.push(nidx);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_image(size: u32, square_lo: u32, square_hi: u32) -> Vec<u8> {
        let mut gray = vec![0u8; (size * size) as usize];
        for y in square_lo..square_hi {
            for x in square_lo..square_hi {
                gray[(y * size + x) as usize] = 255;
            }
        }
        gray
    }

    #[test]
    fn canny_finds_the_square_boundary_and_nothing_else() {
        let size = 32u32;
        let (lo, hi) = (8u32, 24u32);
        let gray = square_image(size, lo, hi);
        let edges = canny_edges_u8(&gray, size, size, CANNY_DEFAULT_LOW, CANNY_DEFAULT_HIGH).unwrap();
        assert_eq!(edges.len(), (size * size) as usize);

        // There must be SOME edge pixels (the square boundary is a hard
        // 0->255 step, well above the default 50/200 thresholds).
        let edge_count = edges.iter().filter(|&&v| v == 255).count();
        assert!(edge_count > 0, "expected at least one edge pixel");

        // Edge pixels must cluster near the square's boundary (within 2px),
        // not scattered across the flat interior/exterior.
        for (idx, &value) in edges.iter().enumerate() {
            if value != 255 {
                continue;
            }
            let x = (idx as u32) % size;
            let y = (idx as u32) / size;
            let near_boundary = [x, y]
                .iter()
                .zip([lo, lo])
                .any(|(&v, edge)| (v as i64 - edge as i64).unsigned_abs() <= 2)
                || [x, y]
                    .iter()
                    .zip([hi, hi])
                    .any(|(&v, edge)| (v as i64 - edge as i64).unsigned_abs() <= 2);
            assert!(
                near_boundary,
                "edge pixel at ({x},{y}) is not near the square boundary [{lo},{hi})"
            );
        }

        // The flat far corners (well inside the background) must be clean.
        assert_eq!(edges[0], 0, "top-left background corner must not be an edge");
        assert_eq!(
            edges[(size * size - 1) as usize],
            0,
            "bottom-right background corner must not be an edge"
        );
    }

    #[test]
    fn canny_flat_image_has_no_edges() {
        let size = 16u32;
        let gray = vec![128u8; (size * size) as usize];
        let edges = canny_edges_u8(&gray, size, size, CANNY_DEFAULT_LOW, CANNY_DEFAULT_HIGH).unwrap();
        assert!(edges.iter().all(|&v| v == 0));
    }

    #[test]
    fn canny_rejects_mismatched_length() {
        let err = canny_edges_u8(&[0u8; 4], 4, 4, CANNY_DEFAULT_LOW, CANNY_DEFAULT_HIGH).unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)));
    }

    #[test]
    fn depth_normalization_inverts_and_handles_invalid() {
        // 2x2: near (100mm), far (5000mm), invalid (0), and a duplicate near.
        let depth = vec![100u16, 5000, 0, 100];
        let rgb = normalize_depth_mm(&depth, 2, 2).unwrap();
        assert_eq!(rgb.len(), 2 * 2 * 3);
        // Near (min) -> bright (1.0); far (max) -> dark (0.0); invalid -> 0.0.
        assert!((rgb[0] - 1.0).abs() < 1e-6, "near pixel must be bright");
        assert!((rgb[3] - 0.0).abs() < 1e-6, "far pixel must be dark");
        assert!((rgb[6] - 0.0).abs() < 1e-6, "invalid pixel must be dark");
        assert!((rgb[9] - 1.0).abs() < 1e-6, "duplicate near pixel must be bright");
        // R == G == B everywhere.
        for px in rgb.chunks_exact(3) {
            assert_eq!(px[0], px[1]);
            assert_eq!(px[1], px[2]);
        }
    }

    #[test]
    fn depth_normalization_all_invalid_is_black_not_error() {
        let depth = vec![0u16; 4];
        let rgb = normalize_depth_mm(&depth, 2, 2).unwrap();
        assert!(rgb.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn depth_normalization_rejects_length_mismatch() {
        let err = normalize_depth_mm(&[1, 2, 3], 2, 2).unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)));
    }

    #[test]
    fn hwc_to_chw_round_trips_positions() {
        // 2x1 image, 3 channels: pixel0=(1,2,3) pixel1=(4,5,6).
        let hwc = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let chw = hwc01_to_chw01(&hwc, 2, 1, 3);
        // Channel-planar: R plane [1,4], G plane [2,5], B plane [3,6].
        assert_eq!(chw, vec![1.0, 4.0, 2.0, 5.0, 3.0, 6.0]);
    }

    #[test]
    fn gray_to_rgb01_replicates_channels() {
        let rgb = gray_u8_to_rgb01(&[0, 128, 255]);
        assert_eq!(rgb.len(), 9);
        assert!((rgb[0]).abs() < 1e-6);
        assert!((rgb[3] - 128.0 / 255.0).abs() < 1e-6);
        assert!((rgb[6] - 1.0).abs() < 1e-6);
        for px in rgb.chunks_exact(3) {
            assert_eq!(px[0], px[1]);
            assert_eq!(px[1], px[2]);
        }
    }

    #[test]
    fn resize_identity_is_a_no_op() {
        let src = vec![1.0, 2.0, 3.0, 4.0];
        let out = resize_hwc_bilinear(&src, 2, 2, 2, 2, 1).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn resize_upscale_preserves_corner_values_approximately() {
        // 2x2 checkerboard-ish gradient, upscale to 4x4: corners should stay
        // close to the source corner values (half-pixel-center resampling
        // can shift them slightly inward, so check with tolerance).
        let src = vec![0.0, 1.0, 0.0, 1.0]; // row0: 0,1 ; row1: 0,1
        let out = resize_hwc_bilinear(&src, 2, 2, 4, 4, 1).unwrap();
        assert_eq!(out.len(), 16);
        assert!(out[0] < 0.3, "top-left should stay near 0.0, got {}", out[0]);
        assert!(out[3] > 0.7, "top-right should stay near 1.0, got {}", out[3]);
    }

    #[test]
    fn resize_rejects_zero_destination() {
        let err = resize_hwc_bilinear(&[0.0; 4], 2, 2, 0, 2, 1).unwrap_err();
        assert!(matches!(err, AssetAiError::Params(_)));
    }

    #[test]
    fn rgb_to_gray_matches_luma_weights() {
        let rgb = vec![255u8, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
        let gray = rgb8_to_gray_u8(&rgb, 2, 2).unwrap();
        assert_eq!(gray.len(), 4);
        assert_eq!(gray[0], 76); // pure red -> 0.299*255 rounded
        assert_eq!(gray[1], 150); // pure green -> 0.587*255 rounded
        assert_eq!(gray[2], 29); // pure blue -> 0.114*255 rounded
        assert_eq!(gray[3], 255); // white -> 255
    }
}
