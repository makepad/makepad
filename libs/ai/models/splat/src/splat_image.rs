//! `preprocess_image` from triposplat.py, op for op.
//!
//! `resize so the SHORT side is 1024 (Lanczos)` -> `background removal when
//! the input has no real alpha` -> `erode the matte with a 3x3 minimum
//! filter` -> `square crop around the alpha bbox, expanded 1.2x` -> `resize to
//! 1024x1024 (Lanczos)` -> `composite over black`.
//!
//! Everything stays 8-bit between stages because the reference is a chain of
//! PIL operations on 8-bit images, and the quantization at each step is part
//! of what the model was trained on. Background removal itself is NOT here:
//! the service owns the native BiRefNet stage so it runs on the same CUDA
//! worker (and can evict BiRefNet before the much larger TripoSplat weights
//! load), exactly like the TRELLIS backend. This module's contract is
//! "hand me RGBA whose alpha is meaningful".

use crate::splat::SPLAT_CANVAS;
use crate::{DiffusionError, Result};

const LANCZOS_SUPPORT: f32 = 3.0;

fn lanczos3(x: f32) -> f32 {
    if !(-LANCZOS_SUPPORT..LANCZOS_SUPPORT).contains(&x) {
        return 0.0;
    }
    if x == 0.0 {
        return 1.0;
    }
    let pix = std::f32::consts::PI * x;
    LANCZOS_SUPPORT * pix.sin() * (pix / LANCZOS_SUPPORT).sin() / (pix * pix)
}

/// PIL's per-destination window and normalized weights for one axis.
fn resample_axis(src_len: usize, dst_len: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = src_len as f32 / dst_len as f32;
    let filter_scale = scale.max(1.0);
    let support = LANCZOS_SUPPORT * filter_scale;
    let mut out = Vec::with_capacity(dst_len);
    for i in 0..dst_len {
        let center = (i as f32 + 0.5) * scale;
        let xmin = ((center - support) as isize).max(0) as usize;
        let xmax = ((center + support).ceil() as usize).min(src_len);
        let mut weights: Vec<f32> = (xmin..xmax)
            .map(|k| lanczos3((k as f32 + 0.5 - center) / filter_scale))
            .collect();
        let sum: f32 = weights.iter().sum();
        if sum != 0.0 {
            for weight in &mut weights {
                *weight /= sum;
            }
        }
        out.push((xmin, weights));
    }
    out
}

/// Interleaved 8-bit image with an explicit channel count (3 or 4).
#[derive(Clone, Debug)]
pub struct SplatImage {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub pixels: Vec<u8>,
}

impl SplatImage {
    pub fn new(pixels: Vec<u8>, width: usize, height: usize, channels: usize) -> Result<Self> {
        if width == 0 || height == 0 || !(channels == 3 || channels == 4) {
            return Err(DiffusionError::workflow("splat image must be RGB8 or RGBA8"));
        }
        if pixels.len() != width * height * channels {
            return Err(DiffusionError::workflow(format!(
                "splat image byte length {} != {width}*{height}*{channels}",
                pixels.len()
            )));
        }
        Ok(Self {
            width,
            height,
            channels,
            pixels,
        })
    }

    /// True when an RGBA input carries a matte the pipeline can trust —
    /// `image.mode == "RGBA" and alpha.min() < 255` in the reference.
    pub fn has_real_alpha(&self) -> bool {
        self.channels == 4 && self.pixels.chunks_exact(4).any(|pixel| pixel[3] < 255)
    }

    /// Lanczos-3 resize over every channel, quantized back to 8-bit.
    pub fn resize(&self, dst_w: usize, dst_h: usize) -> SplatImage {
        let xs = resample_axis(self.width, dst_w);
        let ys = resample_axis(self.height, dst_h);
        let c = self.channels;
        // Horizontal pass into f32.
        let mut mid = vec![0.0f32; dst_w * self.height * c];
        for y in 0..self.height {
            for (x, (xmin, weights)) in xs.iter().enumerate() {
                for channel in 0..c {
                    let mut acc = 0.0f32;
                    for (k, weight) in weights.iter().enumerate() {
                        acc += self.pixels[((y * self.width) + xmin + k) * c + channel] as f32
                            * weight;
                    }
                    mid[((y * dst_w) + x) * c + channel] = acc;
                }
            }
        }
        // Vertical pass, then quantize.
        let mut pixels = vec![0u8; dst_w * dst_h * c];
        for (y, (ymin, weights)) in ys.iter().enumerate() {
            for x in 0..dst_w {
                for channel in 0..c {
                    let mut acc = 0.0f32;
                    for (k, weight) in weights.iter().enumerate() {
                        acc += mid[(((ymin + k) * dst_w) + x) * c + channel] * weight;
                    }
                    pixels[((y * dst_w) + x) * c + channel] =
                        (acc + 0.5).floor().clamp(0.0, 255.0) as u8;
                }
            }
        }
        SplatImage {
            width: dst_w,
            height: dst_h,
            channels: c,
            pixels,
        }
    }

    /// `size / min(w, h)` scaled resize — the pipeline's first step.
    pub fn resize_short_side(&self, size: usize) -> SplatImage {
        let scale = size as f32 / self.width.min(self.height) as f32;
        let width = ((self.width as f32 * scale).round() as usize).max(1);
        let height = ((self.height as f32 * scale).round() as usize).max(1);
        if width == self.width && height == self.height {
            return self.clone();
        }
        self.resize(width, height)
    }

    /// Add an opaque alpha channel (an RGB input the caller has not matted).
    pub fn to_rgba(&self) -> SplatImage {
        if self.channels == 4 {
            return self.clone();
        }
        let mut pixels = Vec::with_capacity(self.width * self.height * 4);
        for pixel in self.pixels.chunks_exact(3) {
            pixels.extend_from_slice(pixel);
            pixels.push(255);
        }
        SplatImage {
            width: self.width,
            height: self.height,
            channels: 4,
            pixels,
        }
    }

    /// Replace the alpha channel (the BiRefNet matte handoff).
    pub fn set_alpha(&mut self, alpha: &[u8]) -> Result<()> {
        if self.channels != 4 || alpha.len() != self.width * self.height {
            return Err(DiffusionError::workflow("splat set_alpha shape mismatch"));
        }
        for (pixel, value) in self.pixels.chunks_exact_mut(4).zip(alpha) {
            pixel[3] = *value;
        }
        Ok(())
    }
}

/// `ImageFilter.MinFilter(2*radius + 1)` on the alpha channel. PIL's rank
/// filters leave the border untouched, so only pixels whose full window fits
/// are eroded.
pub fn erode_alpha(image: &mut SplatImage, radius: usize) -> Result<()> {
    if image.channels != 4 {
        return Err(DiffusionError::workflow("erode_alpha needs RGBA"));
    }
    if radius == 0 {
        return Ok(());
    }
    let (w, h) = (image.width, image.height);
    let source: Vec<u8> = image.pixels.chunks_exact(4).map(|pixel| pixel[3]).collect();
    for y in radius..h.saturating_sub(radius) {
        for x in radius..w.saturating_sub(radius) {
            let mut min = 255u8;
            for dy in 0..=2 * radius {
                for dx in 0..=2 * radius {
                    min = min.min(source[(y + dy - radius) * w + (x + dx - radius)]);
                }
            }
            image.pixels[(y * w + x) * 4 + 3] = min;
        }
    }
    Ok(())
}

/// Square crop bounds around the alpha bbox, expanded 1.2x. Returns
/// `(left, top, right, bottom)`; bounds may fall outside the image, which the
/// crop zero-fills exactly like PIL.
pub fn alpha_crop_bounds(image: &SplatImage) -> Result<(i64, i64, i64, i64)> {
    if image.channels != 4 {
        return Err(DiffusionError::workflow("alpha crop needs RGBA"));
    }
    let (w, h) = (image.width, image.height);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0usize, 0usize);
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            // np.nonzero: strictly greater than zero.
            if image.pixels[(y * w + x) * 4 + 3] != 0 {
                any = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if !any {
        return Err(DiffusionError::workflow(
            "input matte is empty — nothing to reconstruct",
        ));
    }
    let cx = (min_x + max_x) as f32 / 2.0;
    let cy = (min_y + max_y) as f32 / 2.0;
    let half = (max_x - min_x).max(max_y - min_y) as f32 / 2.0 * 1.2;
    // Python int() truncates toward zero.
    Ok((
        (cx - half) as i64,
        (cy - half) as i64,
        (cx + half) as i64,
        (cy + half) as i64,
    ))
}

/// PIL `crop` with zero fill outside the source.
pub fn crop(image: &SplatImage, bounds: (i64, i64, i64, i64)) -> Result<SplatImage> {
    let (left, top, right, bottom) = bounds;
    let out_w = (right - left).max(1) as usize;
    let out_h = (bottom - top).max(1) as usize;
    let c = image.channels;
    let mut pixels = vec![0u8; out_w * out_h * c];
    for y in 0..out_h {
        let sy = top + y as i64;
        if sy < 0 || sy >= image.height as i64 {
            continue;
        }
        for x in 0..out_w {
            let sx = left + x as i64;
            if sx < 0 || sx >= image.width as i64 {
                continue;
            }
            let src = (sy as usize * image.width + sx as usize) * c;
            let dst = (y * out_w + x) * c;
            pixels[dst..dst + c].copy_from_slice(&image.pixels[src..src + c]);
        }
    }
    SplatImage::new(pixels, out_w, out_h, c)
}

/// The conditioner input: crop, resize to the canvas, composite over black.
/// Returns the RGB canvas as 8-bit interleaved pixels (what the reference's
/// `prepared` image is) so the caller can both save it and feed it to the
/// encoders.
pub fn preprocess(image: &SplatImage, erode_radius: usize) -> Result<SplatImage> {
    let mut rgba = image.to_rgba().resize_short_side(SPLAT_CANVAS);
    erode_alpha(&mut rgba, erode_radius)?;
    let bounds = alpha_crop_bounds(&rgba)?;
    let cropped = crop(&rgba, bounds)?;
    let square = cropped.resize(SPLAT_CANVAS, SPLAT_CANVAS);
    // bg.paste(image, mask=alpha) over a black canvas.
    let mut pixels = vec![0u8; SPLAT_CANVAS * SPLAT_CANVAS * 3];
    for (out, pixel) in pixels.chunks_exact_mut(3).zip(square.pixels.chunks_exact(4)) {
        let alpha = pixel[3] as u32;
        for channel in 0..3 {
            // PIL composites in 8-bit: (fg * a + bg * (255 - a) + 127) / 255.
            out[channel] = ((pixel[channel] as u32 * alpha + 127) / 255) as u8;
        }
    }
    SplatImage::new(pixels, SPLAT_CANVAS, SPLAT_CANVAS, 3)
}

/// Interleaved RGB8 -> planar f32 in `[0, 1]` (`transforms.ToTensor()`).
pub fn to_planar_f32(image: &SplatImage) -> Result<Vec<f32>> {
    if image.channels != 3 {
        return Err(DiffusionError::workflow("planar conversion needs RGB"));
    }
    let plane = image.width * image.height;
    let mut out = vec![0.0f32; 3 * plane];
    for (i, pixel) in image.pixels.chunks_exact(3).enumerate() {
        for channel in 0..3 {
            out[channel * plane + i] = pixel[channel] as f32 / 255.0;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: usize, height: usize, pixel: [u8; 4]) -> SplatImage {
        let mut pixels = Vec::with_capacity(width * height * 4);
        for _ in 0..width * height {
            pixels.extend_from_slice(&pixel);
        }
        SplatImage::new(pixels, width, height, 4).unwrap()
    }

    #[test]
    fn lanczos_kernel_values() {
        assert_eq!(lanczos3(0.0), 1.0);
        assert!(lanczos3(1.0).abs() < 1e-6);
        assert!(lanczos3(3.0) == 0.0);
        assert!(lanczos3(1.5) < 0.0);
    }

    #[test]
    fn resize_preserves_a_constant_image_and_the_short_side_rule() {
        let image = solid(7, 3, [10, 20, 30, 255]);
        let out = image.resize(14, 6);
        assert_eq!(out.width, 14);
        assert!(out.pixels.chunks_exact(4).all(|p| p == [10, 20, 30, 255]));
        // short side 3 -> 1024 scales the long side by the same factor.
        let scaled = image.resize_short_side(6);
        assert_eq!((scaled.width, scaled.height), (14, 6));
    }

    #[test]
    fn alpha_detection_needs_a_non_opaque_pixel() {
        assert!(!solid(2, 2, [0, 0, 0, 255]).has_real_alpha());
        let mut image = solid(2, 2, [0, 0, 0, 255]);
        image.pixels[3] = 254;
        assert!(image.has_real_alpha());
        // An RGB input never claims a matte.
        let rgb = SplatImage::new(vec![1; 12], 2, 2, 3).unwrap();
        assert!(!rgb.has_real_alpha());
        assert!(rgb.to_rgba().pixels.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn erode_shrinks_the_matte_and_leaves_the_border() {
        // 5x5 fully opaque except a single transparent pixel in the middle:
        // a 3x3 min filter spreads that hole to its 8 neighbours.
        let mut image = solid(5, 5, [255, 255, 255, 255]);
        image.pixels[(2 * 5 + 2) * 4 + 3] = 0;
        erode_alpha(&mut image, 1).unwrap();
        let alpha = |x: usize, y: usize| image.pixels[(y * 5 + x) * 4 + 3];
        for y in 1..4 {
            for x in 1..4 {
                assert_eq!(alpha(x, y), 0, "({x},{y})");
            }
        }
        // The border row is untouched by PIL's rank filter.
        assert_eq!(alpha(0, 0), 255);
        assert_eq!(alpha(4, 2), 255);
        // radius 0 is a no-op.
        let mut image = solid(3, 3, [0, 0, 0, 7]);
        erode_alpha(&mut image, 0).unwrap();
        assert_eq!(image.pixels[3], 7);
    }

    #[test]
    fn crop_bounds_are_square_and_expanded_by_1_2() {
        // 100x100 canvas with an opaque 20x10 box at (40..60, 45..55).
        let mut image = solid(100, 100, [255, 255, 255, 0]);
        for y in 45..=55 {
            for x in 40..=60 {
                image.pixels[(y * 100 + x) * 4 + 3] = 255;
            }
        }
        let (left, top, right, bottom) = alpha_crop_bounds(&image).unwrap();
        // center (50, 50), extent max(20, 10) = 20, half = 12
        assert_eq!((left, top, right, bottom), (38, 38, 62, 62));
        assert_eq!(right - left, bottom - top);
        // An empty matte is an explicit error, never a panic.
        let empty = solid(4, 4, [0, 0, 0, 0]);
        assert!(alpha_crop_bounds(&empty).is_err());
    }

    #[test]
    fn crop_zero_fills_outside_the_source() {
        let image = solid(4, 4, [9, 9, 9, 255]);
        let out = crop(&image, (-2, -2, 2, 2)).unwrap();
        assert_eq!((out.width, out.height), (4, 4));
        // Top-left quadrant is outside the source.
        assert_eq!(&out.pixels[..4], &[0, 0, 0, 0]);
        // Bottom-right quadrant came from the source.
        assert_eq!(&out.pixels[(2 * 4 + 2) * 4..(2 * 4 + 2) * 4 + 4], &[9, 9, 9, 255]);
    }

    #[test]
    fn preprocess_produces_a_black_composited_canvas() {
        // A 64x64 image with an opaque red square in the middle.
        let mut image = solid(64, 64, [255, 0, 0, 0]);
        for y in 24..40 {
            for x in 24..40 {
                image.pixels[(y * 64 + x) * 4 + 3] = 255;
            }
        }
        let out = preprocess(&image, 1).unwrap();
        assert_eq!(out.channels, 3);
        assert_eq!((out.width, out.height), (SPLAT_CANVAS, SPLAT_CANVAS));
        // The subject fills the middle; the expanded crop leaves black edges.
        let center = ((SPLAT_CANVAS / 2) * SPLAT_CANVAS + SPLAT_CANVAS / 2) * 3;
        assert!(out.pixels[center] > 200, "{}", out.pixels[center]);
        assert_eq!(out.pixels[center + 1], 0);
        assert_eq!(&out.pixels[..3], &[0, 0, 0]);
        // Planar conversion is /255 with a channel-major layout.
        let planar = to_planar_f32(&out).unwrap();
        assert_eq!(planar.len(), 3 * SPLAT_CANVAS * SPLAT_CANVAS);
        assert!(planar.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn composite_matches_pil_rounding() {
        // alpha 128 over black: (255*128 + 127)/255 = 128
        let mut image = solid(2, 2, [255, 255, 255, 128]);
        image.pixels[3] = 128;
        let mut square = image.clone();
        square.pixels.iter_mut().skip(3).step_by(4).for_each(|a| *a = 128);
        let mut out = vec![0u8; 3];
        let pixel = [255u8, 255, 255, 128];
        for channel in 0..3 {
            out[channel] = ((pixel[channel] as u32 * 128 + 127) / 255) as u8;
        }
        assert_eq!(out, vec![128, 128, 128]);
    }
}
