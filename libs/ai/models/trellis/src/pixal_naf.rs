//! Sparse evaluation of Neighborhood Attention Filtering (NAF).
//!
//! Pixal3D only consumes bilinear samples of the upsampled feature image.
//! Evaluate attention at those four pixels per voxel, then interpolate.
//! This avoids constructing the 1024 x 1024 x 1024 texture feature image
//! (4 GiB at f32), or an unfolded neighborhood tensor. Q/K are image-guide
//! features; V remains on the original DINO patch grid.

use crate::backend::{
    gpu_concat_rows, gpu_conv2d_planar_cached, gpu_gather_cols, gpu_group_norm_planar, gpu_silu,
    gpu_upload, GpuTensor,
};
use crate::pixal3d::bilinear_taps;
use crate::trellis::TrellisWeights;
use crate::trellis_image::T2Image;
use crate::{DiffusionError, Result};
use makepad_ai_common::gpu::{
    gpu_pixal_naf_sample, gpu_pixal_pool, gpu_pixal_rope, gpu_planar_tokens_transpose,
};

struct NafConv {
    name: String,
    weight: Vec<f32>,
    bias: Vec<f32>,
    kernel: usize,
}
struct NafNorm {
    name: String,
    weight: Vec<f32>,
    bias: Vec<f32>,
}
struct NafBranch {
    conv: Vec<NafConv>,
    norm: Vec<NafNorm>,
}

/// The checkpoint's two convolution stacks, shared across all three NAF stages.
pub struct PixalNaf {
    branches: [NafBranch; 2],
    periods: Vec<f32>,
}

impl PixalNaf {
    pub fn prepare(weights: &TrellisWeights) -> Result<Self> {
        let load = |branch: &str, kernel: usize| -> Result<NafBranch> {
            let prefix = format!("naf.image_encoder.{branch}");
            let mut conv = Vec::new();
            let mut norm = Vec::new();
            for suffix in ["0", "1.conv1", "1.conv2", "2.conv1", "2.conv2"] {
                let name = format!("{prefix}.{suffix}");
                let w = weights.tensor_f32(&format!("{name}.weight"))?;
                let b = weights.tensor_f32(&format!("{name}.bias"))?;
                let input = if suffix == "0" { 3 } else { 128 };
                if w.len() != 128 * input * kernel * kernel || b.len() != 128 {
                    return Err(DiffusionError::model("invalid Pixal3D NAF convolution"));
                }
                conv.push(NafConv {
                    name,
                    weight: w,
                    bias: b,
                    kernel,
                });
            }
            for suffix in ["1.norm1", "1.norm2", "2.norm1", "2.norm2"] {
                let name = format!("{prefix}.{suffix}");
                let weight = weights.tensor_f32(&format!("{name}.weight"))?;
                let bias = weights.tensor_f32(&format!("{name}.bias"))?;
                if weight.len() != 128 || bias.len() != 128 {
                    return Err(DiffusionError::model("invalid Pixal3D NAF normalization"));
                }
                norm.push(NafNorm { name, weight, bias });
            }
            Ok(NafBranch { conv, norm })
        };
        let periods = weights.tensor_f32("naf.image_encoder.rope.periods")?;
        if periods.len() != 16 || periods.iter().any(|x| !x.is_finite() || *x <= 0.0) {
            return Err(DiffusionError::model("invalid Pixal3D NAF RoPE periods"));
        }
        Ok(Self {
            branches: [load("encoder", 1)?, load("sem_encoder", 3)?],
            periods,
        })
    }

    /// Encode an image once for guide maps at several resolutions. The caller
    /// owns this optional cache and can release it before sparse decoding.
    pub fn encode(&self, image: &T2Image) -> Result<NafImageFeatures> {
        if image.channels != 3
            || image.width < 2
            || image.height < 2
            || image.width > 1024
            || image.height > 1024
        {
            return Err(DiffusionError::workflow("invalid Pixal3D NAF image"));
        }
        let width = image.width;
        let height = image.height;
        let input = gpu_upload(&image.data, 3, width * height).map_err(DiffusionError::model)?;
        // Reflect padding on device via one shared index table. Run the
        // existing fast same-convolution, then crop its extra border.
        let mut reflect = Vec::with_capacity((width + 2) * (height + 2));
        for y in 0..height + 2 {
            for x in 0..width + 2 {
                let ix = if x == 0 {
                    1
                } else if x == width + 1 {
                    width - 2
                } else {
                    x - 1
                };
                let iy = if y == 0 {
                    1
                } else if y == height + 1 {
                    height - 2
                } else {
                    y - 1
                };
                reflect.push((iy * width + ix) as u32);
            }
        }
        let crop: Vec<u32> = (0..height)
            .flat_map(|y| (0..width).map(move |x| ((y + 1) * (width + 2) + x + 1) as u32))
            .collect();
        let convolution = |x: &GpuTensor, conv: &NafConv| -> Result<GpuTensor> {
            if conv.kernel == 1 {
                gpu_conv2d_planar_cached(
                    x,
                    width,
                    height,
                    "p3naf",
                    &conv.name,
                    &conv.weight,
                    &conv.bias,
                    128,
                    1,
                    1,
                    0,
                    0,
                )
                .map_err(DiffusionError::model)
            } else {
                let padded = gpu_gather_cols(x, &reflect).map_err(DiffusionError::model)?;
                let output = gpu_conv2d_planar_cached(
                    &padded,
                    width + 2,
                    height + 2,
                    "p3naf",
                    &conv.name,
                    &conv.weight,
                    &conv.bias,
                    128,
                    3,
                    3,
                    1,
                    1,
                )
                .map_err(DiffusionError::model)?;
                gpu_gather_cols(&output, &crop).map_err(DiffusionError::model)
            }
        };
        let mut branches = Vec::new();
        for branch in &self.branches {
            let mut x = convolution(&input, &branch.conv[0])?;
            for (norm, conv) in branch.norm.iter().zip(&branch.conv[1..]) {
                x = gpu_group_norm_planar(
                    &x,
                    width,
                    height,
                    8,
                    "p3naf",
                    &norm.name,
                    &norm.weight,
                    &norm.bias,
                    1e-5,
                )
                .map_err(DiffusionError::model)?;
                x = gpu_silu(&x).map_err(DiffusionError::model)?;
                x = convolution(&x, conv)?;
            }
            branches.push(x);
        }
        let planar = gpu_concat_rows(&branches[0], &branches[1]).map_err(DiffusionError::model)?;
        Ok(NafImageFeatures {
            planar,
            width,
            height,
        })
    }

    /// Produces only the 256-channel guide maps. The 1024-channel values
    /// stay at DINO resolution and are never upsampled into a dense image.
    pub fn guide(&self, image: &T2Image, target: usize, low: usize) -> Result<NafGuide> {
        self.guide_from_features(&self.encode(image)?, target, low)
    }

    pub fn guide_from_features(
        &self,
        features: &NafImageFeatures,
        target: usize,
        low: usize,
    ) -> Result<NafGuide> {
        if target == 0 || low == 0 || target % low != 0 || target > 1024 {
            return Err(DiffusionError::workflow("invalid Pixal3D NAF target"));
        }
        let pooled = gpu_pixal_pool(
            &features.planar,
            features.width,
            features.height,
            target,
            target,
        )
        .map_err(DiffusionError::model)?;
        let periods = gpu_upload(&self.periods, 1, 16).map_err(DiffusionError::model)?;
        let q = gpu_pixal_rope(&pooled, &periods, target, target).map_err(DiffusionError::model)?;
        drop((periods, pooled));
        let planar = gpu_planar_tokens_transpose(&q).map_err(DiffusionError::model)?;
        let k = gpu_pixal_pool(&planar, target, target, low, low).map_err(DiffusionError::model)?;
        let k = gpu_planar_tokens_transpose(&k).map_err(DiffusionError::model)?;
        Ok(NafGuide { q, k, target, low })
    }
}

pub struct NafImageFeatures {
    planar: GpuTensor,
    width: usize,
    height: usize,
}

pub struct NafGuide {
    q: GpuTensor,
    k: GpuTensor,
    target: usize,
    low: usize,
}

/// Per-input conditioning. Only five global tokens enter cross attention;
/// local DINO patches are projected onto the current stage's sparse grid.
pub struct PixalConditioning {
    pub global: GpuTensor,
    pub negative: GpuTensor,
    patches: Vec<f32>,
    values: GpuTensor,
    image: T2Image,
    side: usize,
}

impl PixalConditioning {
    pub fn new(tokens: &[f32], rgb: &[f32], resolution: usize) -> Result<Self> {
        let side = resolution / 16;
        if ![512, 1024].contains(&resolution)
            || tokens.len() != (5 + side * side) * 1024
            || rgb.len() != 3 * resolution * resolution
        {
            return Err(DiffusionError::workflow(
                "invalid Pixal3D DINO conditioning",
            ));
        }
        let patches = tokens[5 * 1024..].to_vec();
        Ok(Self {
            global: gpu_upload(&tokens[..5 * 1024], 5, 1024).map_err(DiffusionError::model)?,
            negative: gpu_upload(&vec![0.0; 5 * 1024], 5, 1024).map_err(DiffusionError::model)?,
            values: gpu_upload(&patches, side * side, 1024).map_err(DiffusionError::model)?,
            patches,
            side,
            image: T2Image {
                width: resolution,
                height: resolution,
                channels: 3,
                data: rgb.to_vec(),
            },
        })
    }

    /// The high-resolution map is evaluated only where the model samples it.
    /// Guide storage is released before the DiT and sparse VAE start.
    pub fn project(
        &self,
        camera: crate::pixal3d::PixalCamera,
        coords: &[[i32; 3]],
        grid: usize,
        naf: Option<(&PixalNaf, usize)>,
    ) -> Result<GpuTensor> {
        let uv = camera.project(coords, grid, self.image.width)?;
        let low = crate::pixal3d::sample_features(&self.patches, self.side, self.side, 1024, &uv)?;
        let low = gpu_upload(&low, coords.len(), 1024).map_err(DiffusionError::model)?;
        if let Some((naf, target)) = naf {
            let guide = naf.guide(&self.image, target, self.side)?;
            let high = guide.sample(&self.values, &uv)?;
            crate::backend::gpu_concat_cols(&[&low, &high]).map_err(DiffusionError::model)
        } else {
            Ok(low)
        }
    }
}

impl NafGuide {
    pub fn sample(&self, values: &GpuTensor, uv: &[[f32; 2]]) -> Result<GpuTensor> {
        if uv.is_empty() || uv.iter().flatten().any(|v| !v.is_finite()) {
            return Err(DiffusionError::workflow(
                "invalid sparse NAF sample coordinates",
            ));
        }
        let packed: Vec<f32> = uv.iter().flatten().copied().collect();
        let points = gpu_upload(&packed, uv.len(), 2).map_err(DiffusionError::model)?;
        gpu_pixal_naf_sample(
            &self.q,
            &self.k,
            values,
            &points,
            self.target,
            self.target,
            self.low,
            self.low,
            4,
            9,
        )
        .map_err(DiffusionError::model)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NafShape {
    pub width: usize,
    pub height: usize,
    pub low_width: usize,
    pub low_height: usize,
    pub heads: usize,
    pub query_channels: usize,
    pub value_channels: usize,
    pub kernel: usize,
}

impl NafShape {
    pub fn validate(self, q_len: usize, k_len: usize, v_len: usize) -> Result<()> {
        let size = |w: usize, h: usize, c: usize| w.checked_mul(h)?.checked_mul(c);
        if self.low_width == 0
            || self.low_height == 0
            || self.heads == 0
            || self.width < self.low_width
            || self.height < self.low_height
            || self.width % self.low_width != 0
            || self.height % self.low_height != 0
            || self.query_channels == 0
            || self.value_channels == 0
            || self.query_channels % self.heads != 0
            || self.value_channels % self.heads != 0
            || self.kernel == 0
            || self.kernel > 15
            || self.kernel % 2 == 0
            || size(self.width, self.height, self.query_channels) != Some(q_len)
            || size(self.low_width, self.low_height, self.query_channels) != Some(k_len)
            || size(self.low_width, self.low_height, self.value_channels) != Some(v_len)
        {
            return Err(DiffusionError::workflow("invalid sparse NAF shapes"));
        }
        Ok(())
    }
}

/// Scalar oracle, also useful for testing the fused GPU implementation.
/// Q/K/V are token-major with contiguous heads. The reference workflow uses
/// zero-padded neighborhoods: out-of-image keys have score zero and value
/// zero, and still participate in the softmax denominator.
pub fn naf_sample_reference(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    uv: &[[f32; 2]],
    shape: NafShape,
) -> Result<Vec<f32>> {
    shape.validate(q.len(), k.len(), v.len())?;
    let len = uv
        .len()
        .checked_mul(shape.value_channels)
        .ok_or_else(|| DiffusionError::workflow("sparse NAF output size overflow"))?;
    let mut out = vec![0.0; len];
    let qd = shape.query_channels / shape.heads;
    let vd = shape.value_channels / shape.heads;
    let scale = 1.0 / (qd as f32).sqrt();
    let radius = (shape.kernel / 2) as isize;
    let dx = shape.width / shape.low_width;
    let dy = shape.height / shape.low_height;
    let mut scores = vec![0.0f32; shape.kernel * shape.kernel];
    let mut indices = vec![None; scores.len()];
    for (point, row) in uv.iter().zip(out.chunks_exact_mut(shape.value_channels)) {
        for (pixel, weight) in bilinear_taps(*point, shape.width, shape.height)? {
            if weight == 0.0 {
                continue;
            }
            let x = pixel % shape.width;
            let y = pixel / shape.width;
            for head in 0..shape.heads {
                let query = &q[pixel * shape.query_channels + head * qd..][..qd];
                for ky in 0..shape.kernel {
                    for kx in 0..shape.kernel {
                        let i = ky * shape.kernel + kx;
                        let nx = x as isize + (kx as isize - radius) * dx as isize;
                        let ny = y as isize + (ky as isize - radius) * dy as isize;
                        let valid = nx >= 0
                            && ny >= 0
                            && nx < shape.width as isize
                            && ny < shape.height as isize;
                        indices[i] =
                            valid.then(|| ny as usize / dy * shape.low_width + nx as usize / dx);
                        scores[i] = match indices[i] {
                            Some(p) => {
                                query
                                    .iter()
                                    .zip(&k[p * shape.query_channels + head * qd..][..qd])
                                    .map(|(a, b)| a * b)
                                    .sum::<f32>()
                                    * scale
                            }
                            None => 0.0,
                        };
                    }
                }
                let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let mut sum = 0.0;
                for s in &mut scores {
                    *s = (*s - max).exp();
                    sum += *s;
                }
                for (score, index) in scores.iter().zip(&indices) {
                    if let Some(p) = index {
                        let factor = weight * score / sum;
                        let values = &v[p * shape.value_channels + head * vd..][..vd];
                        for (dst, src) in row[head * vd..][..vd].iter_mut().zip(values) {
                            *dst += factor * src;
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_padding_participates_in_softmax() {
        let shape = NafShape {
            width: 4,
            height: 4,
            low_width: 2,
            low_height: 2,
            heads: 1,
            query_channels: 1,
            value_channels: 1,
            kernel: 3,
        };
        let out = naf_sample_reference(
            &[0.0; 16],
            &[0.0; 4],
            &[9.0; 4],
            &[[0.125, 0.125], [0.5, 0.5], [0.875, 0.875]],
            shape,
        )
        .unwrap();
        // Every dilated 3x3 window contains four valid LR cells and five zeros.
        for value in out {
            assert!((value - 4.0).abs() < 1e-6);
        }
    }

    #[test]
    fn heads_and_nearest_exact_addressing() {
        let shape = NafShape {
            width: 4,
            height: 2,
            low_width: 2,
            low_height: 1,
            heads: 2,
            query_channels: 2,
            value_channels: 4,
            kernel: 1,
        };
        let out = naf_sample_reference(
            &[0.0; 16],
            &[0.0; 4],
            &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
            &[[0.5, 0.5]],
            shape,
        )
        .unwrap();
        assert_eq!(out, [3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn malformed_shapes_are_errors() {
        let shape = NafShape {
            width: 4,
            height: 4,
            low_width: 0,
            low_height: 2,
            heads: 1,
            query_channels: 1,
            value_channels: 1,
            kernel: 3,
        };
        assert!(naf_sample_reference(&[], &[], &[], &[], shape).is_err());
    }
}
