//! DINOv3 ViT-L/16 image conditioner for TRELLIS 2 (transformers
//! modeling_dinov3_vit.py semantics): 24 pre-LN layers, 16 heads x 64, plain
//! erf-GELU MLP, LayerScale residuals, axial 2D rotate-half rope (theta 100)
//! over the patch tokens only (cls + 4 register prefix rows ride identity
//! tables). The TRELLIS feature extractor SKIPS the model's final norm and
//! applies a weightless layer norm instead.
//!
//! Weights are f32 on disk and stay f32 device-resident (gpu_linear_f32 path,
//! ~1.2GB): correctness-first — the conditioner is 1.25s of a 37s reference
//! run. LayerScale lambdas are folded into o_proj / down_proj rows at load
//! (exact modulo one f32 rounding).

use crate::backend::{
    gpu_add, gpu_attention_packed_cross, gpu_concat_rows, gpu_download, gpu_gelu_erf,
    gpu_layer_norm_mul_add, gpu_linear_f32_resident, gpu_rope_half, gpu_upload, GpuTensor,
};
use crate::trellis::TrellisWeights;
use crate::{emit_progress, DiffusionError, ProgressHook, Result};

pub const DINO_HIDDEN: usize = 1024;
pub const DINO_DEPTH: usize = 24;
pub const DINO_HEADS: usize = 16;
pub const DINO_HEAD_DIM: usize = 64;
pub const DINO_MLP: usize = 4096;
pub const DINO_PATCH: usize = 16;
pub const DINO_PREFIX_TOKENS: usize = 5; // cls + 4 register
pub const DINO_ROPE_THETA: f32 = 100.0;
pub const DINO_ROT_HALF: usize = DINO_HEAD_DIM / 2; // 32
pub const DINO_NORM_EPS: f32 = 1e-5;
pub const DINO_PATCH_DIM: usize = 3 * DINO_PATCH * DINO_PATCH; // 768

const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

struct DinoLayer {
    norm1_w: Vec<f32>,
    norm1_b: Vec<f32>,
    q_w: GpuTensor,
    q_b: GpuTensor,
    k_w: GpuTensor,
    v_w: GpuTensor,
    v_b: GpuTensor,
    o_w: GpuTensor, // layer_scale1 folded
    o_b: GpuTensor,
    norm2_w: Vec<f32>,
    norm2_b: Vec<f32>,
    up_w: GpuTensor,
    up_b: GpuTensor,
    down_w: GpuTensor, // layer_scale2 folded
    down_b: GpuTensor,
}

pub struct T2Dino {
    patch_w: GpuTensor, // (1024, 768) reshaped conv2d weight
    patch_b: GpuTensor,
    prefix: GpuTensor, // (5, 1024) [cls | register x4]
    layers: Vec<DinoLayer>,
}

fn host_f32(weights: &TrellisWeights, name: &str, len: usize) -> Result<Vec<f32>> {
    let values = weights.tensor_f32(name)?;
    if values.len() != len {
        return Err(DiffusionError::model(format!(
            "dinov3 tensor '{name}' expected {len} values, got {}",
            values.len()
        )));
    }
    Ok(values)
}

fn upload(values: &[f32], rows: usize, cols: usize) -> Result<GpuTensor> {
    gpu_upload(values, rows, cols).map_err(DiffusionError::model)
}

impl T2Dino {
    pub fn prepare(weights: &TrellisWeights) -> Result<Self> {
        Self::prepare_with_progress(weights, None)
    }

    /// [`Self::prepare`] ticking "load dino block k/24" per transformer
    /// block — unlike the DiTs (which stream lazily inside their first
    /// forwards), the DINO weights are read from disk AND uploaded here.
    pub fn prepare_with_progress(
        weights: &TrellisWeights,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let c = DINO_HIDDEN;
        let patch_w = host_f32(weights, "embeddings.patch_embeddings.weight", c * DINO_PATCH_DIM)?;
        let patch_b = host_f32(weights, "embeddings.patch_embeddings.bias", c)?;
        let cls = host_f32(weights, "embeddings.cls_token", c)?;
        let register = host_f32(weights, "embeddings.register_tokens", 4 * c)?;
        let mut prefix = cls;
        prefix.extend_from_slice(&register);

        let mut layers = Vec::with_capacity(DINO_DEPTH);
        for i in 0..DINO_DEPTH {
            if progress.is_some() {
                emit_progress(
                    &mut progress,
                    &format!("load dino block {}/{DINO_DEPTH}", i + 1),
                    i as f64 / DINO_DEPTH as f64,
                )?;
            }
            let p = format!("layer.{i}");
            let scale1 = host_f32(weights, &format!("{p}.layer_scale1.lambda1"), c)?;
            let scale2 = host_f32(weights, &format!("{p}.layer_scale2.lambda1"), c)?;
            let mut o_w = host_f32(weights, &format!("{p}.attention.o_proj.weight"), c * c)?;
            let mut o_b = host_f32(weights, &format!("{p}.attention.o_proj.bias"), c)?;
            for (row, lambda) in scale1.iter().enumerate() {
                for value in &mut o_w[row * c..(row + 1) * c] {
                    *value *= lambda;
                }
                o_b[row] *= lambda;
            }
            let mut down_w = host_f32(weights, &format!("{p}.mlp.down_proj.weight"), c * DINO_MLP)?;
            let mut down_b = host_f32(weights, &format!("{p}.mlp.down_proj.bias"), c)?;
            for (row, lambda) in scale2.iter().enumerate() {
                for value in &mut down_w[row * DINO_MLP..(row + 1) * DINO_MLP] {
                    *value *= lambda;
                }
                down_b[row] *= lambda;
            }
            layers.push(DinoLayer {
                norm1_w: host_f32(weights, &format!("{p}.norm1.weight"), c)?,
                norm1_b: host_f32(weights, &format!("{p}.norm1.bias"), c)?,
                q_w: upload(
                    &host_f32(weights, &format!("{p}.attention.q_proj.weight"), c * c)?,
                    c,
                    c,
                )?,
                q_b: upload(&host_f32(weights, &format!("{p}.attention.q_proj.bias"), c)?, 1, c)?,
                k_w: upload(
                    &host_f32(weights, &format!("{p}.attention.k_proj.weight"), c * c)?,
                    c,
                    c,
                )?,
                v_w: upload(
                    &host_f32(weights, &format!("{p}.attention.v_proj.weight"), c * c)?,
                    c,
                    c,
                )?,
                v_b: upload(&host_f32(weights, &format!("{p}.attention.v_proj.bias"), c)?, 1, c)?,
                o_w: upload(&o_w, c, c)?,
                o_b: upload(&o_b, 1, c)?,
                norm2_w: host_f32(weights, &format!("{p}.norm2.weight"), c)?,
                norm2_b: host_f32(weights, &format!("{p}.norm2.bias"), c)?,
                up_w: upload(
                    &host_f32(weights, &format!("{p}.mlp.up_proj.weight"), DINO_MLP * c)?,
                    DINO_MLP,
                    c,
                )?,
                up_b: upload(&host_f32(weights, &format!("{p}.mlp.up_proj.bias"), DINO_MLP)?, 1, DINO_MLP)?,
                down_w: upload(&down_w, c, DINO_MLP)?,
                down_b: upload(&down_b, 1, c)?,
            });
        }
        Ok(Self {
            patch_w: upload(&patch_w, c, DINO_PATCH_DIM)?,
            patch_b: upload(&patch_b, 1, c)?,
            prefix: upload(&prefix, DINO_PREFIX_TOKENS, c)?,
            layers,
        })
    }

    /// Rope tables over [prefix identity rows | patch rows]: patch centers
    /// normalized to [-1, 1], angles = 2*pi * coord * theta^(-j/16), layout
    /// per row [y-freqs | x-freqs] (32 = rot_half; both rotated halves share
    /// the table).
    fn rope_tables(&self, patches_side: usize) -> (Vec<f32>, Vec<f32>) {
        let rows = DINO_PREFIX_TOKENS + patches_side * patches_side;
        let mut inv_freq = [0.0f32; 16];
        for (j, value) in inv_freq.iter_mut().enumerate() {
            *value = 1.0 / DINO_ROPE_THETA.powf(j as f32 * 4.0 / DINO_HEAD_DIM as f32);
        }
        let mut cos = vec![1.0f32; rows * DINO_ROT_HALF];
        let mut sin = vec![0.0f32; rows * DINO_ROT_HALF];
        for py in 0..patches_side {
            for px in 0..patches_side {
                let row = DINO_PREFIX_TOKENS + py * patches_side + px;
                let coord_y = 2.0 * ((py as f32 + 0.5) / patches_side as f32) - 1.0;
                let coord_x = 2.0 * ((px as f32 + 0.5) / patches_side as f32) - 1.0;
                for (j, freq) in inv_freq.iter().enumerate() {
                    let ay = 2.0 * std::f32::consts::PI * coord_y * freq;
                    let ax = 2.0 * std::f32::consts::PI * coord_x * freq;
                    cos[row * DINO_ROT_HALF + j] = ay.cos();
                    sin[row * DINO_ROT_HALF + j] = ay.sin();
                    cos[row * DINO_ROT_HALF + 16 + j] = ax.cos();
                    sin[row * DINO_ROT_HALF + 16 + j] = ax.sin();
                }
            }
        }
        (cos, sin)
    }

    /// Forward on an already-normalized (3, size, size) input (the model's
    /// exact pixel tensor: /255, ImageNet mean/std). Returns the TRELLIS
    /// conditioning: layer stack WITHOUT the model norm, then a weightless
    /// layer norm — (tokens, 1024) host f32.
    pub fn forward_normalized(&self, pixels: &[f32], size: usize) -> Result<Vec<f32>> {
        if size % DINO_PATCH != 0 || pixels.len() != 3 * size * size {
            return Err(DiffusionError::workflow("dinov3 input shape mismatch"));
        }
        let side = size / DINO_PATCH;
        let num_patches = side * side;
        // Host patchify: patch vector layout [c][py][px] matches the conv2d
        // weight (Co, Ci, kh, kw) flattening.
        let mut patch_rows = vec![0.0f32; num_patches * DINO_PATCH_DIM];
        let plane = size * size;
        for gy in 0..side {
            for gx in 0..side {
                let row = gy * side + gx;
                let base = row * DINO_PATCH_DIM;
                for c in 0..3 {
                    for py in 0..DINO_PATCH {
                        let src = c * plane + (gy * DINO_PATCH + py) * size + gx * DINO_PATCH;
                        let dst = base + c * DINO_PATCH * DINO_PATCH + py * DINO_PATCH;
                        patch_rows[dst..dst + DINO_PATCH]
                            .copy_from_slice(&pixels[src..src + DINO_PATCH]);
                    }
                }
            }
        }
        let patches_in = upload(&patch_rows, num_patches, DINO_PATCH_DIM)?;
        let patches = gpu_linear_f32_resident(&patches_in, &self.patch_w, Some(&self.patch_b))
            .map_err(DiffusionError::model)?;
        drop(patches_in);
        let mut hidden =
            gpu_concat_rows(&self.prefix, &patches).map_err(DiffusionError::model)?;
        drop(patches);

        let (cos, sin) = self.rope_tables(side);
        let rows = DINO_PREFIX_TOKENS + num_patches;
        let cos = upload(&cos, rows, DINO_ROT_HALF)?;
        let sin = upload(&sin, rows, DINO_ROT_HALF)?;
        let scale = 1.0 / (DINO_HEAD_DIM as f32).sqrt();

        for layer in &self.layers {
            let normed =
                gpu_layer_norm_mul_add(&hidden, &layer.norm1_w, &layer.norm1_b, DINO_NORM_EPS)
                    .map_err(DiffusionError::model)?;
            let q = gpu_linear_f32_resident(&normed, &layer.q_w, Some(&layer.q_b))
                .map_err(DiffusionError::model)?;
            let k = gpu_linear_f32_resident(&normed, &layer.k_w, None)
                .map_err(DiffusionError::model)?;
            let v = gpu_linear_f32_resident(&normed, &layer.v_w, Some(&layer.v_b))
                .map_err(DiffusionError::model)?;
            drop(normed);
            let q = gpu_rope_half(&q, DINO_HEADS, DINO_ROT_HALF, &cos, &sin)
                .map_err(DiffusionError::model)?;
            let k = gpu_rope_half(&k, DINO_HEADS, DINO_ROT_HALF, &cos, &sin)
                .map_err(DiffusionError::model)?;
            // Chunked composite attention (the cross entry point with
            // q == kv): hd64 can't take the fused kernel, and the plain
            // composite materializes the FULL 16 x 4101^2 score tensor —
            // 1.08GB per layer x 24 layers of pool-thrash at the stage
            // boundary (measured: warm cond 0.33s -> 1.48s once the pool
            // holds the flow stages' sizes). The chunked path reuses one
            // bounded scores buffer.
            let attn = gpu_attention_packed_cross(&q, &k, &v, DINO_HEADS, scale)
                .map_err(DiffusionError::model)?;
            drop((q, k, v));
            let attn = gpu_linear_f32_resident(&attn, &layer.o_w, Some(&layer.o_b))
                .map_err(DiffusionError::model)?;
            hidden = gpu_add(&hidden, &attn).map_err(DiffusionError::model)?;
            drop(attn);

            let normed =
                gpu_layer_norm_mul_add(&hidden, &layer.norm2_w, &layer.norm2_b, DINO_NORM_EPS)
                    .map_err(DiffusionError::model)?;
            let ff = gpu_linear_f32_resident(&normed, &layer.up_w, Some(&layer.up_b))
                .map_err(DiffusionError::model)?;
            drop(normed);
            let ff = gpu_gelu_erf(&ff).map_err(DiffusionError::model)?;
            let ff = gpu_linear_f32_resident(&ff, &layer.down_w, Some(&layer.down_b))
                .map_err(DiffusionError::model)?;
            hidden = gpu_add(&hidden, &ff).map_err(DiffusionError::model)?;
            drop(ff);
        }

        // TRELLIS extract_features: weightless layer norm (not model.norm).
        let ones = vec![1.0f32; DINO_HIDDEN];
        let zeros = vec![0.0f32; DINO_HIDDEN];
        let out = gpu_layer_norm_mul_add(&hidden, &ones, &zeros, DINO_NORM_EPS)
            .map_err(DiffusionError::model)?;
        gpu_download(&out).map_err(DiffusionError::model)
    }

    /// Forward on raw RGB [0, 1] pixels (3, size, size): applies the ImageNet
    /// normalization first.
    pub fn forward_rgb(&self, rgb: &[f32], size: usize) -> Result<Vec<f32>> {
        if rgb.len() != 3 * size * size {
            return Err(DiffusionError::workflow("dinov3 rgb shape mismatch"));
        }
        let plane = size * size;
        let mut normalized = vec![0.0f32; rgb.len()];
        for c in 0..3 {
            let mean = IMAGENET_MEAN[c];
            let std = IMAGENET_STD[c];
            for i in 0..plane {
                normalized[c * plane + i] = (rgb[c * plane + i] - mean) / std;
            }
        }
        self.forward_normalized(&normalized, size)
    }
}
