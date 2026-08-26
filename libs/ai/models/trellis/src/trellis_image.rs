//! TRELLIS image preprocessing (pipeline.preprocess_image +
//! DinoV3FeatureExtractor input building): alpha-aware crop to the object
//! bbox, alpha-premultiply, PIL-style Lanczos-3 resize to the conditioner
//! resolution, /255. PIL semantics op for op: bbox over alpha > 0.8*255,
//! square crop around the bbox center (int-truncated bounds, zero-padded
//! outside), separable Lanczos with per-destination normalized f32 weights.

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

/// One separable pass: dst_len samples over src_len with PIL window math.
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
            for w in &mut weights {
                *w /= sum;
            }
        }
        out.push((xmin, weights));
    }
    out
}

/// Planar f32 image (channels, height, width).
pub struct T2Image {
    pub channels: usize,
    pub width: usize,
    pub height: usize,
    /// c-major planes, row-major within a plane, values 0..=1.
    pub data: Vec<f32>,
}

impl T2Image {
    pub fn from_rgba8(rgba: &[u8], width: usize, height: usize) -> Result<Self> {
        if rgba.len() != width * height * 4 {
            return Err(DiffusionError::workflow("rgba8 size mismatch"));
        }
        let plane = width * height;
        let mut data = vec![0.0f32; 4 * plane];
        for i in 0..plane {
            for c in 0..4 {
                data[c * plane + i] = rgba[i * 4 + c] as f32 / 255.0;
            }
        }
        Ok(Self {
            channels: 4,
            width,
            height,
            data,
        })
    }

    /// Lanczos-3 resize (all channels).
    pub fn resize(&self, dst_w: usize, dst_h: usize) -> T2Image {
        let xs = resample_axis(self.width, dst_w);
        let ys = resample_axis(self.height, dst_h);
        let src_plane = self.width * self.height;
        // Horizontal pass.
        let mut mid = vec![0.0f32; self.channels * dst_w * self.height];
        for c in 0..self.channels {
            for y in 0..self.height {
                let row = &self.data[c * src_plane + y * self.width..];
                for (x, (xmin, weights)) in xs.iter().enumerate() {
                    let mut acc = 0.0f32;
                    for (k, w) in weights.iter().enumerate() {
                        acc += row[xmin + k] * w;
                    }
                    mid[(c * self.height + y) * dst_w + x] = acc;
                }
            }
        }
        // Vertical pass.
        let mut data = vec![0.0f32; self.channels * dst_w * dst_h];
        let dst_plane = dst_w * dst_h;
        for c in 0..self.channels {
            for (y, (ymin, weights)) in ys.iter().enumerate() {
                for x in 0..dst_w {
                    let mut acc = 0.0f32;
                    for (k, w) in weights.iter().enumerate() {
                        acc += mid[(c * self.height + (ymin + k)) * dst_w + x] * w;
                    }
                    data[c * dst_plane + y * dst_w + x] = acc;
                }
            }
        }
        T2Image {
            channels: self.channels,
            width: dst_w,
            height: dst_h,
            data,
        }
    }
}

/// preprocess_image for an RGBA input that HAS meaningful alpha (the rembg
/// path is not needed for alpha inputs): downscale to <= 1024, bbox over
/// alpha > 0.8*255, square crop centered on the bbox (int bounds, zero pad),
/// alpha-premultiply to RGB.
pub fn t2_preprocess_rgba(image: &T2Image) -> Result<T2Image> {
    if image.channels != 4 {
        return Err(DiffusionError::workflow("preprocess expects rgba"));
    }
    let max_size = image.width.max(image.height);
    let scale = (1024.0 / max_size as f64).min(1.0);
    let scaled;
    let img = if scale < 1.0 {
        scaled = image.resize(
            (image.width as f64 * scale) as usize,
            (image.height as f64 * scale) as usize,
        );
        &scaled
    } else {
        image
    };
    let plane = img.width * img.height;
    let alpha = &img.data[3 * plane..4 * plane];
    let threshold = 0.8; // alpha stored 0..1; reference: a > 0.8*255 on u8
    let mut min_x = usize::MAX;
    let mut min_y = usize::MAX;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    for y in 0..img.height {
        for x in 0..img.width {
            if alpha[y * img.width + x] > threshold {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if min_x == usize::MAX {
        return Err(DiffusionError::workflow("image alpha entirely empty"));
    }
    // Reference: center = midpoint, size = max extent, bbox =
    // (cx - size//2, cy - size//2, cx + size//2, cy + size//2), PIL crop
    // truncates the float bounds toward zero.
    let center_x = (min_x + max_x) as f64 / 2.0;
    let center_y = (min_y + max_y) as f64 / 2.0;
    let size = (max_x - min_x).max(max_y - min_y);
    let half = (size / 2) as f64;
    let left = (center_x - half) as isize;
    let top = (center_y - half) as isize;
    let right = (center_x + half) as isize;
    let bottom = (center_y + half) as isize;
    let out_w = (right - left) as usize;
    let out_h = (bottom - top) as usize;
    let mut data = vec![0.0f32; 3 * out_w * out_h];
    let out_plane = out_w * out_h;
    for y in 0..out_h {
        let sy = top + y as isize;
        if sy < 0 || sy >= img.height as isize {
            continue;
        }
        for x in 0..out_w {
            let sx = left + x as isize;
            if sx < 0 || sx >= img.width as isize {
                continue;
            }
            let src = sy as usize * img.width + sx as usize;
            let a = alpha[src];
            for c in 0..3 {
                // output = rgb * alpha, then u8 round-trip like the reference
                // (fromarray(u8) both ways).
                let v = img.data[c * plane + src] * a;
                data[c * out_plane + y * out_w + x] = (v * 255.0).floor().clamp(0.0, 255.0) / 255.0;
            }
        }
    }
    Ok(T2Image {
        channels: 3,
        width: out_w,
        height: out_h,
        data,
    })
}

/// Border so an alpha-cropped subject does not touch the conditioner frame.
///
/// A fixed 10 px is ~1% of a full-frame headshot after crop and disappears
/// in the 512/1024 DINO resize, which is how a bust becomes an O-Voxel
/// domain-wall sheet. Ten percent of the cropped edge (16 px floor) keeps
/// heads, busts, and other tight mattes inside the reconstruction volume.
pub fn t2_subject_border(min_edge: usize) -> usize {
    min_edge.div_ceil(10).max(16)
}

/// Add an opaque-black border after alpha crop/premultiplication.
///
/// This is the `ImageOps.expand(..., fill=(0, 0, 0, 255))` stage used by
/// the shipping ComfyUI TRELLIS.2 workflows. At this point the image is RGB,
/// so opaque black is represented by zeroes. Keeping the subject away from
/// the conditioner frame is important: a silhouette touching the frame can
/// be decoded as a surface touching the O-Voxel domain boundary.
pub fn t2_pad_black(image: &T2Image, padding: usize) -> Result<T2Image> {
    if image.channels != 3 {
        return Err(DiffusionError::workflow("padding expects rgb"));
    }
    if padding == 0 {
        return Ok(T2Image {
            channels: image.channels,
            width: image.width,
            height: image.height,
            data: image.data.clone(),
        });
    }
    let width = image
        .width
        .checked_add(padding.saturating_mul(2))
        .ok_or_else(|| DiffusionError::workflow("padded image width overflow"))?;
    let height = image
        .height
        .checked_add(padding.saturating_mul(2))
        .ok_or_else(|| DiffusionError::workflow("padded image height overflow"))?;
    let src_plane = image.width * image.height;
    let dst_plane = width * height;
    let mut data = vec![0.0f32; 3 * dst_plane];
    for c in 0..3 {
        for y in 0..image.height {
            let src = c * src_plane + y * image.width;
            let dst = c * dst_plane + (y + padding) * width + padding;
            data[dst..dst + image.width]
                .copy_from_slice(&image.data[src..src + image.width]);
        }
    }
    Ok(T2Image {
        channels: 3,
        width,
        height,
        data,
    })
}

/// The conditioner input: resize to (res, res) with Lanczos, values stay
/// 0..1 RGB (the ImageNet normalize happens in the DINOv3 forward).
pub fn t2_cond_input(preprocessed: &T2Image, res: usize) -> Result<Vec<f32>> {
    if preprocessed.channels != 3 {
        return Err(DiffusionError::workflow("cond input expects rgb"));
    }
    // PIL converts to u8 before resize (Image.fromarray) — quantize first.
    let mut quantized = preprocessed.data.clone();
    for v in &mut quantized {
        *v = (*v * 255.0).round().clamp(0.0, 255.0) / 255.0;
    }
    let img = T2Image {
        channels: 3,
        width: preprocessed.width,
        height: preprocessed.height,
        data: quantized,
    };
    let resized = img.resize(res, res);
    Ok(resized.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanczos_values() {
        assert!((lanczos3(0.0) - 1.0).abs() < 1e-7);
        assert!((lanczos3(0.5) - 0.607_927_1).abs() < 1e-5);
        assert!((lanczos3(1.5) + 0.135_094_9).abs() < 1e-5);
        assert_eq!(lanczos3(3.0), 0.0);
    }

    #[test]
    fn identity_resize() {
        let img = T2Image {
            channels: 1,
            width: 4,
            height: 4,
            data: (0..16).map(|i| i as f32 / 16.0).collect(),
        };
        let out = img.resize(4, 4);
        for (a, b) in out.data.iter().zip(img.data.iter()) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn preprocess_crops_to_alpha() {
        // 8x8, object alpha=1 in a 2x4 rect at x 2..6, y 3..5.
        let mut rgba = vec![0u8; 8 * 8 * 4];
        for y in 3..5 {
            for x in 2..6 {
                let i = (y * 8 + x) * 4;
                rgba[i] = 255;
                rgba[i + 3] = 255;
            }
        }
        let img = T2Image::from_rgba8(&rgba, 8, 8).unwrap();
        let out = t2_preprocess_rgba(&img).unwrap();
        // size = max(3, 1) = 3 -> half = 1 -> crop 2x2 (int truncation).
        assert_eq!(out.channels, 3);
        assert_eq!((out.width, out.height), (2, 2));
    }

    #[test]
    fn black_padding_matches_imageops_expand_layout() {
        let image = T2Image {
            channels: 3,
            width: 2,
            height: 1,
            data: vec![1.0, 0.5, 0.25, 0.75, 0.1, 0.2],
        };
        let out = t2_pad_black(&image, 1).unwrap();
        assert_eq!((out.channels, out.width, out.height), (3, 4, 3));
        for c in 0..3 {
            let plane = out.width * out.height;
            assert_eq!(out.data[c * plane + out.width + 1], image.data[c * 2]);
            assert_eq!(out.data[c * plane + out.width + 2], image.data[c * 2 + 1]);
            assert_eq!(out.data[c * plane], 0.0);
            assert_eq!(out.data[c * plane + out.width + 3], 0.0);
        }
    }

    #[test]
    fn subject_border_scales_with_a_tight_headshot_crop() {
        assert_eq!(t2_subject_border(0), 16);
        assert_eq!(t2_subject_border(80), 16);
        assert_eq!(t2_subject_border(512), 52);
        assert_eq!(t2_subject_border(900), 90);
        assert_eq!(t2_subject_border(1024), 103);
    }
}
