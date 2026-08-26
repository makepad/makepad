//! DINOv3 ViT-H/16+ image conditioner (`DinoV3ViT` in the released model.py,
//! transformers `modeling_dinov3_vit` semantics).
//!
//! 32 pre-LN layers, 1280 wide, 20 heads x 64, LayerScale residuals, a
//! SwiGLU MLP (`down(silu(gate(x)) * up(x))`, 5120 inner) and axial 2D
//! rotate-half rope with theta 100 over the patch tokens only — the cls token
//! and the 4 register tokens ride identity phase tables. Unlike TRELLIS's
//! extractor this model's OWN final norm IS applied, and the pipeline then
//! puts a weightless layer norm on top (`encode_image`'s
//! `F.layer_norm(feat.float(), feat.shape[-1:])`).
//!
//! The two LayerScale lambdas are folded into the `o_proj` / `down_proj` rows
//! at load, which is exact up to one f32 rounding and removes two full-width
//! elementwise passes per block.

use crate::splat::{
    SplatWeights, DINO_DEPTH, DINO_HEADS, DINO_HEAD_DIM, DINO_HIDDEN, DINO_MLP, DINO_NORM_EPS,
    DINO_PATCH, DINO_PATCH_DIM, DINO_PREFIX_TOKENS, DINO_REGISTERS, DINO_ROPE_FREQS,
    DINO_ROPE_THETA, DINO_ROT_HALF, IMAGENET_MEAN, IMAGENET_STD,
};
use crate::splat_ops::{
    add, attention, layer_norm, linear, mul, rope_half, silu, Device, Lin, Ten,
};
use crate::{emit_progress, DiffusionError, ProgressHook, Result};

struct DinoLayer {
    norm1_w: Vec<f32>,
    norm1_b: Vec<f32>,
    q: Lin,
    k: Lin,
    v: Lin,
    /// `o_proj` with `layer_scale1.lambda1` folded into its rows.
    o: Lin,
    norm2_w: Vec<f32>,
    norm2_b: Vec<f32>,
    gate: Lin,
    up: Lin,
    /// `down_proj` with `layer_scale2.lambda1` folded into its rows.
    down: Lin,
}

pub struct SplatDino {
    device: Device,
    patch: Lin,
    /// `(5, 1280)` = `[cls | register x4]`.
    prefix: Ten,
    layers: Vec<DinoLayer>,
    norm_w: Vec<f32>,
    norm_b: Vec<f32>,
}

fn host(weights: &SplatWeights, name: &str, expected: &[usize]) -> Result<Vec<f32>> {
    weights.f32_shaped(name, expected)
}

impl SplatDino {
    /// Every tensor name this module reads, in load order. Used by the
    /// state-dict contract test.
    pub fn expected_tensors() -> Vec<(String, Vec<usize>)> {
        let c = DINO_HIDDEN;
        let mut out = vec![
            (
                "embeddings.patch_embeddings.weight".to_string(),
                vec![c, 3, DINO_PATCH, DINO_PATCH],
            ),
            ("embeddings.patch_embeddings.bias".to_string(), vec![c]),
            ("embeddings.cls_token".to_string(), vec![1, 1, c]),
            (
                "embeddings.register_tokens".to_string(),
                vec![1, DINO_REGISTERS, c],
            ),
            ("norm.weight".to_string(), vec![c]),
            ("norm.bias".to_string(), vec![c]),
        ];
        for i in 0..DINO_DEPTH {
            let p = format!("layer.{i}");
            out.extend([
                (format!("{p}.norm1.weight"), vec![c]),
                (format!("{p}.norm1.bias"), vec![c]),
                (format!("{p}.attention.q_proj.weight"), vec![c, c]),
                (format!("{p}.attention.q_proj.bias"), vec![c]),
                (format!("{p}.attention.k_proj.weight"), vec![c, c]),
                (format!("{p}.attention.v_proj.weight"), vec![c, c]),
                (format!("{p}.attention.v_proj.bias"), vec![c]),
                (format!("{p}.attention.o_proj.weight"), vec![c, c]),
                (format!("{p}.attention.o_proj.bias"), vec![c]),
                (format!("{p}.layer_scale1.lambda1"), vec![c]),
                (format!("{p}.norm2.weight"), vec![c]),
                (format!("{p}.norm2.bias"), vec![c]),
                (format!("{p}.mlp.gate_proj.weight"), vec![DINO_MLP, c]),
                (format!("{p}.mlp.gate_proj.bias"), vec![DINO_MLP]),
                (format!("{p}.mlp.up_proj.weight"), vec![DINO_MLP, c]),
                (format!("{p}.mlp.up_proj.bias"), vec![DINO_MLP]),
                (format!("{p}.mlp.down_proj.weight"), vec![c, DINO_MLP]),
                (format!("{p}.mlp.down_proj.bias"), vec![c]),
                (format!("{p}.layer_scale2.lambda1"), vec![c]),
            ]);
        }
        out
    }

    pub fn prepare(
        device: Device,
        weights: &SplatWeights,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let c = DINO_HIDDEN;
        // conv2d(3, 1280, 16, stride 16) is a linear over flattened patches;
        // the (Co, Ci, kh, kw) memory order is exactly the patch row order.
        let patch_w = host(
            weights,
            "embeddings.patch_embeddings.weight",
            &[c, 3, DINO_PATCH, DINO_PATCH],
        )?;
        let patch_b = host(weights, "embeddings.patch_embeddings.bias", &[c])?;
        let cls = host(weights, "embeddings.cls_token", &[1, 1, c])?;
        let registers = host(weights, "embeddings.register_tokens", &[1, DINO_REGISTERS, c])?;
        let mut prefix = cls;
        prefix.extend_from_slice(&registers);

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
            let scale1 = host(weights, &format!("{p}.layer_scale1.lambda1"), &[c])?;
            let scale2 = host(weights, &format!("{p}.layer_scale2.lambda1"), &[c])?;
            let mut o_w = host(weights, &format!("{p}.attention.o_proj.weight"), &[c, c])?;
            let mut o_b = host(weights, &format!("{p}.attention.o_proj.bias"), &[c])?;
            fold_row_scale(&mut o_w, &mut o_b, &scale1, c);
            let mut down_w = host(weights, &format!("{p}.mlp.down_proj.weight"), &[c, DINO_MLP])?;
            let mut down_b = host(weights, &format!("{p}.mlp.down_proj.bias"), &[c])?;
            fold_row_scale(&mut down_w, &mut down_b, &scale2, DINO_MLP);

            layers.push(DinoLayer {
                norm1_w: host(weights, &format!("{p}.norm1.weight"), &[c])?,
                norm1_b: host(weights, &format!("{p}.norm1.bias"), &[c])?,
                q: Lin::new(
                    device,
                    &host(weights, &format!("{p}.attention.q_proj.weight"), &[c, c])?,
                    c,
                    c,
                    Some(&host(weights, &format!("{p}.attention.q_proj.bias"), &[c])?),
                )?,
                // `qkv_bias=(True, False, True)`: the key projection has no
                // bias in DINOv3.
                k: Lin::new(
                    device,
                    &host(weights, &format!("{p}.attention.k_proj.weight"), &[c, c])?,
                    c,
                    c,
                    None,
                )?,
                v: Lin::new(
                    device,
                    &host(weights, &format!("{p}.attention.v_proj.weight"), &[c, c])?,
                    c,
                    c,
                    Some(&host(weights, &format!("{p}.attention.v_proj.bias"), &[c])?),
                )?,
                o: Lin::new(device, &o_w, c, c, Some(&o_b))?,
                norm2_w: host(weights, &format!("{p}.norm2.weight"), &[c])?,
                norm2_b: host(weights, &format!("{p}.norm2.bias"), &[c])?,
                gate: Lin::new(
                    device,
                    &host(weights, &format!("{p}.mlp.gate_proj.weight"), &[DINO_MLP, c])?,
                    DINO_MLP,
                    c,
                    Some(&host(weights, &format!("{p}.mlp.gate_proj.bias"), &[DINO_MLP])?),
                )?,
                up: Lin::new(
                    device,
                    &host(weights, &format!("{p}.mlp.up_proj.weight"), &[DINO_MLP, c])?,
                    DINO_MLP,
                    c,
                    Some(&host(weights, &format!("{p}.mlp.up_proj.bias"), &[DINO_MLP])?),
                )?,
                down: Lin::new(device, &down_w, c, DINO_MLP, Some(&down_b))?,
            });
        }
        Ok(Self {
            device,
            patch: Lin::new(device, &patch_w, c, DINO_PATCH_DIM, Some(&patch_b))?,
            prefix: Ten::upload(device, &prefix, DINO_PREFIX_TOKENS, c)?,
            layers,
            norm_w: host(weights, "norm.weight", &[c])?,
            norm_b: host(weights, "norm.bias", &[c])?,
        })
    }

    /// `DinoV3ViT.forward` on an ALREADY normalized planar `(3, size, size)`
    /// image, then the pipeline's weightless layer norm. Returns
    /// `(prefix + patches, 1280)` host f32 — `feature1`.
    pub fn forward_normalized(&self, pixels: &[f32], size: usize) -> Result<Vec<f32>> {
        if size % DINO_PATCH != 0 || pixels.len() != 3 * size * size {
            return Err(DiffusionError::workflow("dinov3 input shape mismatch"));
        }
        let side = size / DINO_PATCH;
        let patches = side * side;
        let rows = DINO_PREFIX_TOKENS + patches;
        let scale = 1.0 / (DINO_HEAD_DIM as f32).sqrt();

        let patch_rows = patchify(pixels, size, side);
        let patch_in = Ten::upload(self.device, &patch_rows, patches, DINO_PATCH_DIM)?;
        let embedded = linear(&patch_in, &self.patch)?;
        drop(patch_in);
        let mut hidden = crate::splat_ops::concat_rows(&self.prefix, &embedded)?;
        drop(embedded);

        let (cos, sin) = rope_tables(side);
        let cos = Ten::upload(self.device, &cos, rows, DINO_ROT_HALF)?;
        let sin = Ten::upload(self.device, &sin, rows, DINO_ROT_HALF)?;

        for layer in &self.layers {
            let normed = layer_norm(
                &hidden,
                Some(&layer.norm1_w),
                Some(&layer.norm1_b),
                DINO_NORM_EPS,
            )?;
            let q = linear(&normed, &layer.q)?;
            let k = linear(&normed, &layer.k)?;
            let v = linear(&normed, &layer.v)?;
            drop(normed);
            let q = rope_half(&q, DINO_HEADS, DINO_ROT_HALF, &cos, &sin, DINO_PREFIX_TOKENS)?;
            let k = rope_half(&k, DINO_HEADS, DINO_ROT_HALF, &cos, &sin, DINO_PREFIX_TOKENS)?;
            let attn = attention(&q, &k, &v, DINO_HEADS, scale)?;
            drop((q, k, v));
            let attn = linear(&attn, &layer.o)?;
            hidden = add(&hidden, &attn)?;
            drop(attn);

            let normed = layer_norm(
                &hidden,
                Some(&layer.norm2_w),
                Some(&layer.norm2_b),
                DINO_NORM_EPS,
            )?;
            let gate = silu(&linear(&normed, &layer.gate)?)?;
            let up = linear(&normed, &layer.up)?;
            drop(normed);
            let ff = linear(&mul(&gate, &up)?, &layer.down)?;
            drop((gate, up));
            hidden = add(&hidden, &ff)?;
            drop(ff);
        }

        // The model's own final norm ...
        let hidden = layer_norm(&hidden, Some(&self.norm_w), Some(&self.norm_b), DINO_NORM_EPS)?;
        // ... then encode_image's weightless one, in f32.
        let out = layer_norm(&hidden, None, None, DINO_NORM_EPS)?;
        out.to_host()
    }

    /// [`Self::forward_normalized`] with the ImageNet normalization applied
    /// first (planar RGB in `[0, 1]`).
    pub fn forward_rgb(&self, rgb: &[f32], size: usize) -> Result<Vec<f32>> {
        self.forward_normalized(&imagenet_normalize(rgb, size)?, size)
    }
}

/// Fold a per-output-row scale into a `(n, k)` weight and its bias.
fn fold_row_scale(weight: &mut [f32], bias: &mut [f32], scale: &[f32], k: usize) {
    for (row, lambda) in scale.iter().enumerate() {
        for value in &mut weight[row * k..(row + 1) * k] {
            *value *= lambda;
        }
        bias[row] *= lambda;
    }
}

/// `transforms.Normalize(ImageNet)` on planar `(3, size, size)` pixels.
pub fn imagenet_normalize(rgb: &[f32], size: usize) -> Result<Vec<f32>> {
    if rgb.len() != 3 * size * size {
        return Err(DiffusionError::workflow("dinov3 rgb shape mismatch"));
    }
    let plane = size * size;
    let mut out = vec![0.0f32; rgb.len()];
    for c in 0..3 {
        let mean = IMAGENET_MEAN[c];
        let std = IMAGENET_STD[c];
        for i in 0..plane {
            out[c * plane + i] = (rgb[c * plane + i] - mean) / std;
        }
    }
    Ok(out)
}

/// Planar pixels -> one row per patch, laid out `[c][py][px]` to match the
/// conv2d weight's `(Co, Ci, kh, kw)` flattening.
fn patchify(pixels: &[f32], size: usize, side: usize) -> Vec<f32> {
    let mut rows = vec![0.0f32; side * side * DINO_PATCH_DIM];
    let plane = size * size;
    for gy in 0..side {
        for gx in 0..side {
            let base = (gy * side + gx) * DINO_PATCH_DIM;
            for c in 0..3 {
                for py in 0..DINO_PATCH {
                    let src = c * plane + (gy * DINO_PATCH + py) * size + gx * DINO_PATCH;
                    let dst = base + c * DINO_PATCH * DINO_PATCH + py * DINO_PATCH;
                    rows[dst..dst + DINO_PATCH].copy_from_slice(&pixels[src..src + DINO_PATCH]);
                }
            }
        }
    }
    rows
}

/// `DinoV3RotaryEmbedding2D`: patch centers normalized to `[-1, 1]`,
/// `angle = 2*pi * coord * theta^(-4j/head_dim)`, laid out per row as
/// `[h-freqs (16) | w-freqs (16)]` — the reference's `.tile(2)` makes the
/// upper half of the head reuse the same table, which is exactly the
/// rotate-half convention. Prefix rows are identity (`cos = 1`, `sin = 0`).
pub fn rope_tables(side: usize) -> (Vec<f32>, Vec<f32>) {
    let rows = DINO_PREFIX_TOKENS + side * side;
    let mut inv_freq = [0.0f32; DINO_ROPE_FREQS];
    for (j, value) in inv_freq.iter_mut().enumerate() {
        *value = 1.0 / DINO_ROPE_THETA.powf(j as f32 * 4.0 / DINO_HEAD_DIM as f32);
    }
    let mut cos = vec![1.0f32; rows * DINO_ROT_HALF];
    let mut sin = vec![0.0f32; rows * DINO_ROT_HALF];
    for py in 0..side {
        for px in 0..side {
            let row = DINO_PREFIX_TOKENS + py * side + px;
            let coord_h = 2.0 * ((py as f32 + 0.5) / side as f32) - 1.0;
            let coord_w = 2.0 * ((px as f32 + 0.5) / side as f32) - 1.0;
            for (j, freq) in inv_freq.iter().enumerate() {
                let ah = 2.0 * std::f32::consts::PI * coord_h * freq;
                let aw = 2.0 * std::f32::consts::PI * coord_w * freq;
                cos[row * DINO_ROT_HALF + j] = ah.cos();
                sin[row * DINO_ROT_HALF + j] = ah.sin();
                cos[row * DINO_ROT_HALF + DINO_ROPE_FREQS + j] = aw.cos();
                sin[row * DINO_ROT_HALF + DINO_ROPE_FREQS + j] = aw.sin();
            }
        }
    }
    (cos, sin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rope_tables_are_identity_on_the_prefix_and_centered_on_the_grid() {
        let (cos, sin) = rope_tables(2);
        assert_eq!(cos.len(), (DINO_PREFIX_TOKENS + 4) * DINO_ROT_HALF);
        for row in 0..DINO_PREFIX_TOKENS {
            let base = row * DINO_ROT_HALF;
            assert!(cos[base..base + DINO_ROT_HALF].iter().all(|v| *v == 1.0));
            assert!(sin[base..base + DINO_ROT_HALF].iter().all(|v| *v == 0.0));
        }
        // 2x2 grid: centers at 0.25/0.75 -> coords -0.5/+0.5, so patch (0,0)
        // has h = w = -0.5 and freq[0] = 1 -> angle = -pi.
        let base = DINO_PREFIX_TOKENS * DINO_ROT_HALF;
        assert!((cos[base] + 1.0).abs() < 1e-6);
        assert!(sin[base].abs() < 1e-6);
        assert!((cos[base + DINO_ROPE_FREQS] + 1.0).abs() < 1e-6);
        // Patch (0,1) shares h with (0,0) but flips w.
        let base1 = (DINO_PREFIX_TOKENS + 1) * DINO_ROT_HALF;
        assert_eq!(cos[base1], cos[base]);
        assert!((cos[base1 + DINO_ROPE_FREQS] + 1.0).abs() < 1e-6);
        assert!((sin[base1 + DINO_ROPE_FREQS] - std::f32::consts::PI.sin()).abs() < 1e-6);
        // The frequency ladder decays as theta^(-4j/64).
        let want = 1.0 / DINO_ROPE_THETA.powf(15.0 * 4.0 / 64.0);
        let angle = 2.0 * std::f32::consts::PI * -0.5 * want;
        assert!((cos[base + DINO_ROPE_FREQS - 1] - angle.cos()).abs() < 1e-6);
    }

    #[test]
    fn patchify_walks_channel_then_row_inside_a_patch() {
        // 2x2 image, patch size would be 16 in the model; exercise the layout
        // rule directly with a 1-patch, size-16 image of ramp values.
        let size = 16;
        let mut pixels = vec![0.0f32; 3 * size * size];
        for c in 0..3 {
            for i in 0..size * size {
                pixels[c * size * size + i] = (c * 1000 + i) as f32;
            }
        }
        let rows = patchify(&pixels, size, 1);
        assert_eq!(rows.len(), DINO_PATCH_DIM);
        assert_eq!(rows[0], 0.0);
        assert_eq!(rows[15], 15.0);
        assert_eq!(rows[16], 16.0);
        assert_eq!(rows[256], 1000.0);
        assert_eq!(rows[512], 2000.0);
    }

    #[test]
    fn imagenet_normalization_matches_the_reference_constants() {
        let out = imagenet_normalize(&[0.485, 0.456, 0.406], 1).unwrap();
        assert!(out.iter().all(|v| v.abs() < 1e-6));
        let out = imagenet_normalize(&[0.485 + 0.229, 0.456, 0.406], 1).unwrap();
        assert!((out[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn layer_scale_folding_scales_rows_of_the_weight_and_bias() {
        let mut w = vec![1.0f32, 2.0, 3.0, 4.0];
        let mut b = vec![1.0f32, 1.0];
        fold_row_scale(&mut w, &mut b, &[2.0, 0.5], 2);
        assert_eq!(w, vec![2.0, 4.0, 1.5, 2.0]);
        assert_eq!(b, vec![2.0, 0.5]);
    }

    #[test]
    fn state_dict_contract_covers_every_block() {
        let expected = SplatDino::expected_tensors();
        // 6 top-level + 19 per block.
        assert_eq!(expected.len(), 6 + 19 * DINO_DEPTH);
        assert!(expected
            .iter()
            .any(|(name, shape)| name == "layer.31.mlp.down_proj.weight"
                && shape == &[DINO_HIDDEN, DINO_MLP]));
        // k_proj has no bias entry — DINOv3's qkv_bias is (True, False, True).
        assert!(!expected
            .iter()
            .any(|(name, _)| name == "layer.0.attention.k_proj.bias"));
    }
}
