//! DINOv3 ViT-H+/16 backbone for SAM 3D Body.
//!
//! Matrix weights stay as packed bf16 cache entries and use
//! `gpu_linear_nt_cached_bf16_f32acc`; activations and reductions are f32.
//! LayerScale is folded into the output projection rows and biases at load.

use crate::backend::{
    gpu_add, gpu_attention_packed_cross, gpu_attention_packed_flash2_d64, gpu_concat_rows_many,
    gpu_download,
    gpu_layer_norm_mul_add, gpu_linear_nt_cached_bf16_f32acc, gpu_mul, gpu_rope_half,
    gpu_silu, gpu_slice_rows, gpu_upload, GpuLinearPart, GpuTensor,
};
use crate::weights::BodyWeights;
use crate::{
    emit_progress, DiffusionError, ProgressHook, Result, DINO_DEPTH, DINO_DIM, DINO_FFN,
    DINO_HEADS, DINO_HEAD_DIM, DINO_NORM_EPS, DINO_PREFIX_TOKENS, DINO_ROPE_BASE, IMAGE_SIZE,
    NUM_PATCHES, PATCH, PATCHES_SIDE, ROPE_HALF,
};
use makepad_ai_common::quant::GGML_TYPE_BF16;
use makepad_ai_loader::MlxDType;

const PATCH_DIM: usize = 3 * PATCH * PATCH;
const CACHE_NAMESPACE: &str = "body-dinov3-hplus-bf16";

struct Bf16Linear {
    bytes: Vec<u8>,
    key: String,
    out: usize,
    bias: Vec<f32>,
}

impl Bf16Linear {
    fn load(
        weights: &BodyWeights,
        name: &str,
        out: usize,
        inn: usize,
        bias: bool,
    ) -> Result<Self> {
        let bytes = bf16_bytes_shaped(weights, &format!("{name}.weight"), &[out, inn])?;
        let bias = if bias {
            weights.f32_shaped(&format!("{name}.bias"), &[out])?
        } else {
            Vec::new()
        };
        Ok(Self {
            bytes,
            key: name.to_string(),
            out,
            bias,
        })
    }

    fn load_folded(
        weights: &BodyWeights,
        name: &str,
        scale_name: &str,
        out: usize,
        inn: usize,
    ) -> Result<Self> {
        let mut matrix = weights.f32_shaped(&format!("{name}.weight"), &[out, inn])?;
        let mut bias = weights.f32_shaped(&format!("{name}.bias"), &[out])?;
        let scale = weights.f32_shaped(scale_name, &[out])?;
        for row in 0..out {
            for value in &mut matrix[row * inn..(row + 1) * inn] {
                *value *= scale[row];
            }
            bias[row] *= scale[row];
        }
        Ok(Self {
            bytes: f32_to_bf16_bytes(&matrix),
            key: format!("{name}.layerscale_folded"),
            out,
            bias,
        })
    }

    fn load_patch(weights: &BodyWeights) -> Result<Self> {
        let name = "backbone.embeddings.patch_embeddings";
        Ok(Self {
            bytes: bf16_bytes_shaped(
                weights,
                &format!("{name}.weight"),
                &[DINO_DIM, 3, PATCH, PATCH],
            )?,
            key: name.to_string(),
            out: DINO_DIM,
            bias: weights.f32_shaped(&format!("{name}.bias"), &[DINO_DIM])?,
        })
    }

    fn forward(&self, input: &GpuTensor) -> Result<GpuTensor> {
        gpu_linear_nt_cached_bf16_f32acc(
            input,
            CACHE_NAMESPACE,
            &[GpuLinearPart {
                bt_ggml_type: GGML_TYPE_BF16,
                n: self.out,
                cache_key: &self.key,
                bytes: &self.bytes,
            }],
            &self.bias,
        )
        .map_err(DiffusionError::model)
    }
}

struct DinoLayer {
    norm1_w: Vec<f32>,
    norm1_b: Vec<f32>,
    q: Bf16Linear,
    k: Bf16Linear,
    v: Bf16Linear,
    out: Bf16Linear,
    norm2_w: Vec<f32>,
    norm2_b: Vec<f32>,
    gate: Bf16Linear,
    up: Bf16Linear,
    down: Bf16Linear,
}

pub struct BodyDino {
    patch: Bf16Linear,
    prefix: GpuTensor,
    layers: Vec<DinoLayer>,
    final_norm_w: Vec<f32>,
    final_norm_b: Vec<f32>,
}

fn bf16_bytes_shaped(
    weights: &BodyWeights,
    name: &str,
    expected: &[usize],
) -> Result<Vec<u8>> {
    weights.expect_shape(name, expected)?;
    if weights.dtype(name)? != MlxDType::BF16 {
        return Err(DiffusionError::model(format!(
            "body DINO tensor {name} is not bf16"
        )));
    }
    let bytes = weights.bytes(name)?;
    let values = expected.iter().try_fold(1usize, |product, &dimension| {
        product.checked_mul(dimension).ok_or_else(|| {
            DiffusionError::model(format!("body DINO tensor {name} shape overflows usize"))
        })
    })?;
    if bytes.len() != values * 2 {
        return Err(DiffusionError::model(format!(
            "body DINO tensor {name} has {} bytes, expected {}",
            bytes.len(),
            values * 2
        )));
    }
    Ok(bytes)
}

fn f32_to_bf16_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 2);
    for &value in values {
        let bits = value.to_bits();
        let rounded = ((bits.wrapping_add(0x7fff + ((bits >> 16) & 1))) >> 16) as u16;
        bytes.extend_from_slice(&rounded.to_le_bytes());
    }
    bytes
}

/// Head-dim-64 attention: the FA2 flash kernel (f16 operands, f32 softmax
/// and accumulation — the reference's own precision class) where the
/// backend has it, else the composite f32 path.
pub(crate) fn attention_d64(
    q: &GpuTensor,
    k: &GpuTensor,
    v: &GpuTensor,
    heads: usize,
) -> Result<GpuTensor> {
    match gpu_attention_packed_flash2_d64(q, k, v, heads, 0.125) {
        Ok(out) => Ok(out),
        Err(_) => gpu_attention_packed_cross(q, k, v, heads, 0.125).map_err(DiffusionError::model),
    }
}

impl BodyDino {
    pub fn prepare(weights: &BodyWeights) -> Result<Self> {
        Self::prepare_with_progress(weights, None)
    }

    pub fn prepare_with_progress(
        weights: &BodyWeights,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let cls = weights.f32_shaped("backbone.embeddings.cls_token", &[1, 1, DINO_DIM])?;
        let registers = weights.f32_shaped(
            "backbone.embeddings.register_tokens",
            &[1, DINO_PREFIX_TOKENS - 1, DINO_DIM],
        )?;
        let mut prefix = cls;
        prefix.extend_from_slice(&registers);

        let patch = Bf16Linear::load_patch(weights)?;
        let mut layers = Vec::with_capacity(DINO_DEPTH);
        for i in 0..DINO_DEPTH {
            if progress.is_some() {
                emit_progress(
                    &mut progress,
                    &format!("load body dino block {}/{DINO_DEPTH}", i + 1),
                    i as f64 / DINO_DEPTH as f64,
                )?;
            }
            let p = format!("backbone.layer.{i}");
            layers.push(DinoLayer {
                norm1_w: weights.f32_shaped(&format!("{p}.norm1.weight"), &[DINO_DIM])?,
                norm1_b: weights.f32_shaped(&format!("{p}.norm1.bias"), &[DINO_DIM])?,
                q: Bf16Linear::load(
                    weights,
                    &format!("{p}.attention.q_proj"),
                    DINO_DIM,
                    DINO_DIM,
                    true,
                )?,
                k: Bf16Linear::load(
                    weights,
                    &format!("{p}.attention.k_proj"),
                    DINO_DIM,
                    DINO_DIM,
                    false,
                )?,
                v: Bf16Linear::load(
                    weights,
                    &format!("{p}.attention.v_proj"),
                    DINO_DIM,
                    DINO_DIM,
                    true,
                )?,
                out: Bf16Linear::load_folded(
                    weights,
                    &format!("{p}.attention.o_proj"),
                    &format!("{p}.layer_scale1.lambda1"),
                    DINO_DIM,
                    DINO_DIM,
                )?,
                norm2_w: weights.f32_shaped(&format!("{p}.norm2.weight"), &[DINO_DIM])?,
                norm2_b: weights.f32_shaped(&format!("{p}.norm2.bias"), &[DINO_DIM])?,
                gate: Bf16Linear::load(
                    weights,
                    &format!("{p}.mlp.gate_proj"),
                    DINO_FFN,
                    DINO_DIM,
                    true,
                )?,
                up: Bf16Linear::load(
                    weights,
                    &format!("{p}.mlp.up_proj"),
                    DINO_FFN,
                    DINO_DIM,
                    true,
                )?,
                down: Bf16Linear::load_folded(
                    weights,
                    &format!("{p}.mlp.down_proj"),
                    &format!("{p}.layer_scale2.lambda1"),
                    DINO_DIM,
                    DINO_FFN,
                )?,
            });
        }

        Ok(Self {
            patch,
            prefix: gpu_upload(&prefix, DINO_PREFIX_TOKENS, DINO_DIM)
                .map_err(DiffusionError::model)?,
            layers,
            final_norm_w: weights.f32_shaped("backbone.norm.weight", &[DINO_DIM])?,
            final_norm_b: weights.f32_shaped("backbone.norm.bias", &[DINO_DIM])?,
        })
    }

    fn rope_tables(&self) -> (Vec<f32>, Vec<f32>) {
        let rows = DINO_PREFIX_TOKENS + NUM_PATCHES;
        let mut inv_freq = [0.0f32; 16];
        for (j, value) in inv_freq.iter_mut().enumerate() {
            *value = 1.0 / DINO_ROPE_BASE.powf(j as f32 * 4.0 / DINO_HEAD_DIM as f32);
        }
        let mut cos = vec![1.0f32; rows * ROPE_HALF];
        let mut sin = vec![0.0f32; rows * ROPE_HALF];
        for gy in 0..PATCHES_SIDE {
            for gx in 0..PATCHES_SIDE {
                let row = DINO_PREFIX_TOKENS + gy * PATCHES_SIDE + gx;
                let y = 2.0 * ((gy as f32 + 0.5) / PATCHES_SIDE as f32) - 1.0;
                let x = 2.0 * ((gx as f32 + 0.5) / PATCHES_SIDE as f32) - 1.0;
                for (j, frequency) in inv_freq.iter().enumerate() {
                    let ay = 2.0 * std::f32::consts::PI * y * frequency;
                    let ax = 2.0 * std::f32::consts::PI * x * frequency;
                    cos[row * ROPE_HALF + j] = ay.cos();
                    sin[row * ROPE_HALF + j] = ay.sin();
                    cos[row * ROPE_HALF + 16 + j] = ax.cos();
                    sin[row * ROPE_HALF + 16 + j] = ax.sin();
                }
            }
        }
        (cos, sin)
    }

    pub fn forward_normalized(&self, pixels: &[f32]) -> Result<GpuTensor> {
        if pixels.len() != 3 * IMAGE_SIZE * IMAGE_SIZE {
            return Err(DiffusionError::workflow(format!(
                "body DINO input has {} values, expected {}",
                pixels.len(),
                3 * IMAGE_SIZE * IMAGE_SIZE
            )));
        }

        // Patch vectors use [channel][patch_y][patch_x], matching flattened
        // conv2d weights [out, channel, patch_y, patch_x].
        let mut patch_rows = vec![0.0f32; NUM_PATCHES * PATCH_DIM];
        let plane = IMAGE_SIZE * IMAGE_SIZE;
        for gy in 0..PATCHES_SIDE {
            for gx in 0..PATCHES_SIDE {
                let row = gy * PATCHES_SIDE + gx;
                let base = row * PATCH_DIM;
                for c in 0..3 {
                    for py in 0..PATCH {
                        let src = c * plane + (gy * PATCH + py) * IMAGE_SIZE + gx * PATCH;
                        let dst = base + c * PATCH * PATCH + py * PATCH;
                        patch_rows[dst..dst + PATCH].copy_from_slice(&pixels[src..src + PATCH]);
                    }
                }
            }
        }
        let patch_rows = gpu_upload(&patch_rows, NUM_PATCHES, PATCH_DIM)
            .map_err(DiffusionError::model)?;
        let patches = self.patch.forward(&patch_rows)?;
        let mut hidden = gpu_concat_rows_many(&[&self.prefix, &patches])
            .map_err(DiffusionError::model)?;

        let rows = DINO_PREFIX_TOKENS + NUM_PATCHES;
        let (cos, sin) = self.rope_tables();
        let cos = gpu_upload(&cos, rows, ROPE_HALF).map_err(DiffusionError::model)?;
        let sin = gpu_upload(&sin, rows, ROPE_HALF).map_err(DiffusionError::model)?;

        for layer in &self.layers {
            let normed = gpu_layer_norm_mul_add(
                &hidden,
                &layer.norm1_w,
                &layer.norm1_b,
                DINO_NORM_EPS,
            )
            .map_err(DiffusionError::model)?;
            let q = layer.q.forward(&normed)?;
            let k = layer.k.forward(&normed)?;
            let v = layer.v.forward(&normed)?;
            let q = gpu_rope_half(&q, DINO_HEADS, ROPE_HALF, &cos, &sin)
                .map_err(DiffusionError::model)?;
            let k = gpu_rope_half(&k, DINO_HEADS, ROPE_HALF, &cos, &sin)
                .map_err(DiffusionError::model)?;
            let attention = attention_d64(&q, &k, &v, DINO_HEADS)?;
            let attention = layer.out.forward(&attention)?;
            hidden = gpu_add(&hidden, &attention).map_err(DiffusionError::model)?;

            let normed = gpu_layer_norm_mul_add(
                &hidden,
                &layer.norm2_w,
                &layer.norm2_b,
                DINO_NORM_EPS,
            )
            .map_err(DiffusionError::model)?;
            let gate = layer.gate.forward(&normed)?;
            let up = layer.up.forward(&normed)?;
            let gate = gpu_silu(&gate).map_err(DiffusionError::model)?;
            let ff = gpu_mul(&gate, &up).map_err(DiffusionError::model)?;
            let ff = layer.down.forward(&ff)?;
            hidden = gpu_add(&hidden, &ff).map_err(DiffusionError::model)?;
        }

        let normalized = gpu_layer_norm_mul_add(
            &hidden,
            &self.final_norm_w,
            &self.final_norm_b,
            DINO_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        gpu_slice_rows(&normalized, DINO_PREFIX_TOKENS, NUM_PATCHES)
            .map_err(DiffusionError::model)
    }

    pub fn forward_normalized_host(&self, pixels: &[f32]) -> Result<Vec<f32>> {
        let output = self.forward_normalized(pixels)?;
        gpu_download(&output).map_err(DiffusionError::model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::gpu_device_available;

    fn planar_to_tokens(values: &[f32]) -> Vec<f32> {
        let mut output = vec![0.0f32; NUM_PATCHES * DINO_DIM];
        for c in 0..DINO_DIM {
            for token in 0..NUM_PATCHES {
                output[token * DINO_DIM + c] = values[c * NUM_PATCHES + token];
            }
        }
        output
    }

    #[test]
    fn gpu_fixture_backbone() {
        let Some((_, input)) = crate::fixture::load("backbone_in") else {
            eprintln!("body oracle fixtures absent; skipping backbone GPU parity");
            return;
        };
        let Some((expected_shape, expected)) = crate::fixture::load("backbone_out") else {
            eprintln!("body backbone output fixture absent; skipping backbone GPU parity");
            return;
        };
        let Some(weights_path) = crate::fixture::weights_path() else {
            eprintln!("body weights path absent; skipping backbone GPU parity");
            return;
        };
        if !gpu_device_available() || !crate::fixture::gpu_required_ops_available() {
            eprintln!("body GPU unavailable; skipping backbone GPU parity");
            return;
        }
        let weights = BodyWeights::load(weights_path).expect("load body weights");
        let dino = BodyDino::prepare(&weights).expect("prepare body DINO");
        let actual = dino
            .forward_normalized_host(&input)
            .expect("body DINO forward");
        let expected = if expected_shape.ends_with(&[DINO_DIM, PATCHES_SIDE, PATCHES_SIDE]) {
            planar_to_tokens(&expected)
        } else {
            expected
        };
        assert_eq!(actual.len(), expected.len());
        let mut max_abs = 0.0f32;
        let mut abs_error_sum = 0.0f64;
        let mut reference_abs_sum = 0.0f64;
        for (a, b) in actual.iter().zip(&expected) {
            let error = (a - b).abs();
            max_abs = max_abs.max(error);
            abs_error_sum += error as f64;
            reference_abs_sum += b.abs() as f64;
        }
        let mean_relative = abs_error_sum / reference_abs_sum.max(f64::EPSILON);
        eprintln!(
            "body backbone max abs error: {max_abs:.6}; relative mean error: {mean_relative:.6}"
        );
        assert!(mean_relative < 3e-2);
    }
}
