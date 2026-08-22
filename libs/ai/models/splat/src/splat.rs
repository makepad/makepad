//! TripoSplat (VAST-AI-Research/TripoSplat, MIT) shared foundation: pinned
//! checkpoint manifest, architecture constants transcribed from the released
//! `model.py` / `triposplat.py`, the safetensors reader, the rectified-flow
//! Euler/CFG sampler math, and the quasi-random helpers the model's positional
//! anchors and per-point offsets are built from.
//!
//! Pipeline shape (triposplat.py `TripoSplatPipeline.run`):
//!
//! 1. preprocess: min-side Lanczos resize to 1024, BiRefNet cutout (skipped
//!    when the input already carries a real alpha), 3x3 alpha erode, square
//!    crop around the alpha bbox expanded 1.2x, Lanczos to 1024x1024,
//!    composite on black.
//! 2. condition: DINOv3 ViT-H/16+ tokens (cls + 4 registers + 4096 patches,
//!    1280 wide) followed by a weightless layer norm -> `feature1`; FLUX.2
//!    VAE encode of the same image in [-1, 1] (stochastic, seeded) packed to
//!    128 channels and prefixed with 5 zero rows -> `feature2`. Both are
//!    4101 tokens so the DiT can add their two projections.
//! 3. denoise: `LatentSeqMMFlowModel`, 8192 latent tokens x 16 channels plus
//!    one 5-channel camera token, Euler flow matching with the shift-3
//!    schedule and diffusers-convention CFG.
//! 4. decode: `OctreeGaussianDecoder` — an 8-level octree probability
//!    transformer draws `num_gaussians / 32` anchor points by systematic
//!    resampling, then a 16-block cross-attention decoder emits 32 gaussians
//!    per anchor.
//! 5. write: standard 3DGS binary-little-endian PLY (pre-activation opacity
//!    logit, log scales, wxyz quaternion) after the reference's default
//!    `[[1,0,0],[0,0,-1],[0,1,0]]` Y-up transform.

use crate::{DiffusionError, Result};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Pinned checkpoint manifest (HF VAST-AI/TripoSplat, MIT code + weights).
// The ComfyUI native node consumes exactly this repacked file set.
// ---------------------------------------------------------------------------

pub const TRIPOSPLAT_REPO: &str = "VAST-AI/TripoSplat";
pub const TRIPOSPLAT_REVISION: &str = "56a96e603204ec410c4da60c13ea4fa09a2169a9";

pub const TRIPOSPLAT_DIT_PATH: &str = "diffusion_models/triposplat_fp16.safetensors";
pub const TRIPOSPLAT_DIT_SIZE: u64 = 741_106_994;
pub const TRIPOSPLAT_DIT_SHA256: &str =
    "c870b97ac1d6bc9177608a5ec625e19ef9f3c5019aa68f64b0fb7803abcd6d20";

pub const TRIPOSPLAT_DECODER_PATH: &str = "vae/triposplat_vae_decoder_fp16.safetensors";
pub const TRIPOSPLAT_DECODER_SIZE: u64 = 576_148_442;
pub const TRIPOSPLAT_DECODER_SHA256: &str =
    "ed0d0c3d43b599e326845d0ec70f3cf77be9a55e2d97627ac3b34d2830763cc8";

pub const TRIPOSPLAT_DINO_PATH: &str = "clip_vision/dino_v3_vit_h.safetensors";
pub const TRIPOSPLAT_DINO_SIZE: u64 = 1_681_247_696;
pub const TRIPOSPLAT_DINO_SHA256: &str =
    "a29ef35101a16966972a0d50732a6f3a608ff7cfffb2afa9bbe9007cb842cc53";

pub const TRIPOSPLAT_VAE_PATH: &str = "vae/flux2-vae.safetensors";
pub const TRIPOSPLAT_VAE_SIZE: u64 = 336_213_556;
pub const TRIPOSPLAT_VAE_SHA256: &str =
    "d64f3a68e1cc4f9f4e29b6e0da38a0204fe9a49f2d4053f0ec1fa1ca02f9c4b5";

/// TripoSplat ships its OWN BiRefNet repack. It is the same architecture as
/// the service's `birefnet-hr` entry and the same byte length, but NOT the
/// same weights (different sha256), so it is pinned separately instead of
/// aliasing the existing cache blob.
pub const TRIPOSPLAT_RMBG_PATH: &str = "background_removal/birefnet.safetensors";
pub const TRIPOSPLAT_RMBG_SIZE: u64 = 444_473_596;
pub const TRIPOSPLAT_RMBG_SHA256: &str =
    "9ab37426bf4de0567af6b5d21b16151357149139362e6e8992021b8ce356a154";

/// Device weight-cache namespaces (one per checkpoint) so the whole model
/// can be evicted by prefix on unload.
pub const SPLAT_DINO_NAMESPACE: &str = "tsplat-dino";
pub const SPLAT_FLOW_NAMESPACE: &str = "tsplat-flow";
pub const SPLAT_DEC_NAMESPACE: &str = "tsplat-dec";
pub const SPLAT_NAMESPACES: [&str; 3] = [
    SPLAT_DINO_NAMESPACE,
    SPLAT_FLOW_NAMESPACE,
    SPLAT_DEC_NAMESPACE,
];

// ---------------------------------------------------------------------------
// Architecture constants (model.py / triposplat.py).
// ---------------------------------------------------------------------------

/// `_CANVAS_SIZE`: the square the conditioners see.
pub const SPLAT_CANVAS: usize = 1024;

// DINOv3 ViT-H/16+ (`DinoV3ViT` defaults).
pub const DINO_HIDDEN: usize = 1280;
pub const DINO_HEADS: usize = 20;
pub const DINO_HEAD_DIM: usize = DINO_HIDDEN / DINO_HEADS; // 64
pub const DINO_DEPTH: usize = 32;
pub const DINO_MLP: usize = 5120;
pub const DINO_PATCH: usize = 16;
pub const DINO_REGISTERS: usize = 4;
pub const DINO_PREFIX_TOKENS: usize = 1 + DINO_REGISTERS; // cls + registers
pub const DINO_ROPE_THETA: f32 = 100.0;
/// `rotate_half` splits the head in two: the cos/sin table is half-wide and
/// tiled, exactly like `DinoV3RotaryEmbedding2D.forward`'s `.tile(2)`.
pub const DINO_ROT_HALF: usize = DINO_HEAD_DIM / 2; // 32
/// `inv_freq = 1 / base**arange(0, 1, 4/dim)` — 16 entries per axis.
pub const DINO_ROPE_FREQS: usize = DINO_ROT_HALF / 2; // 16
pub const DINO_NORM_EPS: f32 = 1e-5;
pub const DINO_PATCH_DIM: usize = 3 * DINO_PATCH * DINO_PATCH; // 768
pub const DINO_TOKENS: usize =
    DINO_PREFIX_TOKENS + (SPLAT_CANVAS / DINO_PATCH) * (SPLAT_CANVAS / DINO_PATCH); // 4101

/// ImageNet normalization (`transforms.Normalize` in `encode_image`).
pub const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
pub const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

// FLUX.2 VAE side of the condition.
/// `Flux2VAEEncoder.encode` packs 32 latent channels 2x2 -> 128.
pub const VAE_PACKED_CHANNELS: usize = 128;
/// The reference's `nn.BatchNorm1d(128, eps=1e-5)` — note this is NOT the
/// FLUX.2 pipeline's own 1e-4; TripoSplat constructs its own BatchNorm with
/// the torch default eps and loads the released running stats into it.
pub const VAE_BN_EPS: f32 = 1e-5;
/// `zero_reg`: 5 zero rows so feature2's length matches feature1's.
pub const VAE_ZERO_PREFIX: usize = DINO_PREFIX_TOKENS;

// LatentSeqMMFlowModel (`FLOW_MODEL_ARGS`).
pub const FLOW_Q_TOKENS: usize = 8192;
pub const FLOW_IN_CHANNELS: usize = 16;
pub const FLOW_OUT_CHANNELS: usize = 16;
pub const FLOW_CAM_CHANNELS: usize = 5;
pub const FLOW_MODEL_CHANNELS: usize = 1024;
pub const FLOW_COND_CHANNELS: usize = 1280;
pub const FLOW_COND2_CHANNELS: usize = 128;
pub const FLOW_REFINER_BLOCKS: usize = 2;
pub const FLOW_BLOCKS: usize = 24;
pub const FLOW_HEADS: usize = 16;
pub const FLOW_HEAD_DIM: usize = FLOW_MODEL_CHANNELS / FLOW_HEADS; // 64
pub const FLOW_MLP_RATIO: usize = 4;
pub const FLOW_FFN: usize = FLOW_MODEL_CHANNELS * FLOW_MLP_RATIO; // 4096
/// `LayerNorm32(channels, eps=1e-6)` inside the blocks.
pub const FLOW_NORM_EPS: f32 = 1e-6;
/// `F.layer_norm` default eps at the heads.
pub const FLOW_FINAL_NORM_EPS: f32 = 1e-5;
pub const FLOW_MOD_COLS: usize = 6 * FLOW_MODEL_CHANNELS; // 6144
pub const T_FREQ_DIM: usize = 256;
/// `RePo3DRotaryEmbedding(repo_hidden_ratio=0.125)`.
pub const REPO_HIDDEN: usize = FLOW_MODEL_CHANNELS / 8; // 128
pub const REPO_MAX_FREQ: f32 = 16.0;
/// `dim_0 = dim_1 = 2*(head_dim//6)`, `dim_2 = head_dim - dim_0 - dim_1`;
/// the per-axis frequency counts are half of each.
pub const REPO_FREQ_0: usize = 2 * (FLOW_HEAD_DIM / 6) / 2; // 10
pub const REPO_FREQ_1: usize = REPO_FREQ_0; // 10
pub const REPO_FREQ_2: usize = FLOW_HEAD_DIM / 2 - REPO_FREQ_0 - REPO_FREQ_1; // 12
pub const REPO_PAIRS: usize = FLOW_HEAD_DIM / 2; // 32
/// The Sobol engine that anchors the 8192 latent tokens in the unit cube.
pub const FLOW_SOBOL_SEED: u64 = 123;
/// `PcdAbsolutePositionEmbedder(model_channels)` (v1, `max_res=16`).
pub const POS_EMBED_MAX_RES: usize = 16;
/// `PcdAbsolutePositionEmbedderV2(channels, in_channels=3)` (`max_res=10`).
pub const POS_EMBED_V2_MAX_RES: usize = 10;

// OctreeGaussianDecoder (`OCTREE_DECODER_ARGS` / `GS_DECODER_ARGS`).
pub const OCT_MODEL_CHANNELS: usize = 1024;
pub const OCT_COND_CHANNELS: usize = 16;
pub const OCT_BLOCKS: usize = 4;
pub const OCT_HEADS: usize = 16;
pub const OCT_HEAD_DIM: usize = OCT_MODEL_CHANNELS / OCT_HEADS;
pub const OCT_FFN: usize = OCT_MODEL_CHANNELS * 4;
pub const OCT_OUT: usize = 8;
/// `OctreeGaussianDecoder._MAX_VOXEL_LEVEL`.
pub const OCT_LEVEL: usize = 8;

pub const GS_IN_CHANNELS: usize = 3;
pub const GS_MODEL_CHANNELS: usize = 1024;
pub const GS_COND_CHANNELS: usize = 16;
pub const GS_BLOCKS: usize = 16;
pub const GS_HEADS: usize = 16;
pub const GS_HEAD_DIM: usize = GS_MODEL_CHANNELS / GS_HEADS;
pub const GS_FFN: usize = GS_MODEL_CHANNELS * 4;
/// `representation_config`.
pub const GS_PER_POINT: usize = 32;
pub const GS_PERTURB_SIZE: f32 = 1.5;
pub const GS_OFFSET_SCALE: f32 = 0.05;
pub const GS_FILTER_KERNEL_3D: f32 = 0.0009;
pub const GS_SCALING_BIAS: f32 = 0.004;
pub const GS_OPACITY_BIAS: f32 = 0.1;
pub const GS_LR_ROTATION: f32 = 0.1;
/// `_calc_layout` field widths for one anchor point (32 gaussians each).
pub const GS_LAYOUT: [(&str, usize); 6] = [
    ("_xyz", GS_PER_POINT * 3),
    ("_features_dc", GS_PER_POINT * 3),
    ("_scaling", GS_PER_POINT * 3),
    ("_rotation", GS_PER_POINT * 4),
    ("_opacity", GS_PER_POINT),
    ("_offset_scale", GS_PER_POINT),
];
/// Total `out_proj` width: 96 + 96 + 96 + 128 + 32 + 32.
pub const GS_OUT_CHANNELS: usize = 480;

/// `TripoSplatPipeline._NUM_GAUSSIANS_MIN` / `_MAX`.
pub const GAUSSIANS_MIN: usize = 32_768;
pub const GAUSSIANS_MAX: usize = 262_144;
pub const GAUSSIANS_DEFAULT: usize = GAUSSIANS_MAX;

/// `run()` defaults.
pub const DEFAULT_STEPS: usize = 20;
pub const DEFAULT_GUIDANCE: f32 = 3.0;
pub const DEFAULT_SHIFT: f32 = 3.0;
pub const DEFAULT_SEED: u64 = 42;
pub const DEFAULT_ERODE_RADIUS: usize = 1;

/// Half-open byte offsets of one layout field inside a decoder feature row.
pub fn gs_layout_range(name: &str) -> Option<(usize, usize)> {
    let mut start = 0usize;
    for (key, size) in GS_LAYOUT {
        if key == name {
            return Some((start, start + size));
        }
        start += size;
    }
    None
}

/// `_validate_num_gaussians`: bounds-check then round to a multiple of the
/// decoder's gaussians-per-anchor. Unlike the reference (which asserts) this
/// clamps, because the service caps the request itself.
pub fn resolve_num_gaussians(requested: usize) -> usize {
    let clamped = requested.clamp(GAUSSIANS_MIN, GAUSSIANS_MAX);
    if clamped % GS_PER_POINT == 0 {
        return clamped;
    }
    let rounded =
        ((clamped as f64 / GS_PER_POINT as f64).round() as usize).max(1) * GS_PER_POINT;
    rounded.clamp(GAUSSIANS_MIN, GAUSSIANS_MAX)
}

pub type SplatCancel<'a> = &'a dyn Fn() -> bool;

pub fn check_cancel(cancel: Option<SplatCancel<'_>>) -> Result<()> {
    if cancel.is_some_and(|is_cancelled| is_cancelled()) {
        Err(DiffusionError::Cancelled)
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Single-file safetensors reader (same shape as the TRELLIS one: TripoSplat
// ships one flat file per component and never shards).
// ---------------------------------------------------------------------------

pub struct SplatWeights {
    pub path: PathBuf,
    header: MlxSafetensorsHeader,
}

impl SplatWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let header = MlxSafetensorsHeader::load(&path)
            .map_err(|err| DiffusionError::model(format!("{}: {err:?}", path.display())))?;
        Ok(Self { path, header })
    }

    pub fn file_len(&self) -> u64 {
        self.header.file_len
    }

    pub fn has(&self, name: &str) -> bool {
        self.header.tensors.contains_key(name)
    }

    pub fn tensor_names(&self) -> impl Iterator<Item = &String> {
        self.header.tensors.keys()
    }

    pub fn dtype_shape(&self, name: &str) -> Result<(MlxDType, Vec<usize>)> {
        let entry = self.header.tensor(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "triposplat tensor '{name}' not found in {}",
                self.path.display()
            ))
        })?;
        Ok((
            entry.dtype,
            entry.shape.iter().map(|value| *value as usize).collect(),
        ))
    }

    pub fn bytes(&self, name: &str) -> Result<Vec<u8>> {
        self.header
            .read_tensor_bytes(name)
            .map_err(|err| DiffusionError::model(format!("triposplat tensor '{name}': {err:?}")))
    }

    /// Whole tensor decoded to f32 (checkpoints are fp16 except the DINO
    /// norms, which some repacks keep f32).
    pub fn f32(&self, name: &str) -> Result<Vec<f32>> {
        let (dtype, _shape) = self.dtype_shape(name)?;
        let bytes = self.bytes(name)?;
        match dtype {
            MlxDType::F32 => Ok(bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()),
            MlxDType::F16 => Ok(bytes
                .chunks_exact(2)
                .map(|chunk| crate::f16_word_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])))
                .collect()),
            MlxDType::BF16 => Ok(bytes
                .chunks_exact(2)
                .map(|chunk| {
                    f32::from_bits(u32::from(u16::from_le_bytes([chunk[0], chunk[1]])) << 16)
                })
                .collect()),
            other => Err(DiffusionError::model(format!(
                "triposplat tensor '{name}': unsupported dtype {other:?}"
            ))),
        }
    }

    /// [`Self::f32`] with a shape assertion — the state-dict contract.
    pub fn f32_shaped(&self, name: &str, expected: &[usize]) -> Result<Vec<f32>> {
        let (_dtype, shape) = self.dtype_shape(name)?;
        if shape != expected {
            return Err(DiffusionError::model(format!(
                "triposplat tensor '{name}' shape {shape:?}, expected {expected:?}"
            )));
        }
        self.f32(name)
    }
}

// ---------------------------------------------------------------------------
// FlowEulerCfgSampler schedule (triposplat.py).
//
// `t_seq = shift * linspace(1, 0, steps+1) / (1 + (shift-1) * linspace(...))`
// in numpy f64; the model is called with `1000 * t` as f32 and the Euler
// update is `x -= v * (t - t_prev)`.
// ---------------------------------------------------------------------------

pub fn splat_t_sequence(steps: usize, shift: f64) -> Vec<f64> {
    let mut out = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        // numpy linspace(1, 0, steps+1)[i]
        let t = if steps == 0 {
            1.0
        } else {
            1.0 - i as f64 / steps as f64
        };
        out.push(shift * t / (1.0 + (shift - 1.0) * t));
    }
    out
}

/// `_cfg_prediction`: `pred = s*cond - (s-1)*uncond`, and `s <= 1` means the
/// unconditional pass never runs at all (diffusers convention).
pub fn splat_cfg_enabled(guidance_scale: f32) -> bool {
    guidance_scale > 1.0
}

pub fn splat_cfg_combine(pred_cond: &mut [f32], pred_uncond: &[f32], guidance_scale: f32) {
    for (cond, uncond) in pred_cond.iter_mut().zip(pred_uncond) {
        *cond = guidance_scale * *cond - (guidance_scale - 1.0) * *uncond;
    }
}

/// One Euler step: `x -= v * (t - t_prev)`.
pub fn splat_euler_step(sample: &mut [f32], velocity: &[f32], t: f64, t_prev: f64) -> Result<()> {
    if sample.len() != velocity.len() {
        return Err(DiffusionError::workflow("splat euler length mismatch"));
    }
    let dt = (t - t_prev) as f32;
    for (x, v) in sample.iter_mut().zip(velocity) {
        *x -= dt * *v;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Embedders shared by the flow model and the decoders.
// ---------------------------------------------------------------------------

/// `TimestepEmbedder.timestep_embedding(t, 256, max_period=10000)`:
/// `[cos(t*freqs) | sin(t*freqs)]` with `freqs = exp(-ln(P) * i/half)`.
pub fn timestep_embedding(t: f32, dim: usize, max_period: f32) -> Vec<f32> {
    let half = dim / 2;
    let mut out = vec![0.0f32; dim];
    for i in 0..half {
        let freq = (-(max_period.ln()) * i as f32 / half as f32).exp();
        let arg = t * freq;
        out[i] = arg.cos();
        out[half + i] = arg.sin();
    }
    out
}

/// `LevelEmbedder.level_embedding(t, 256, max_period=1024)` — same shape as
/// the timestep embedding but with an extra `* 2 * pi` on the argument.
pub fn level_embedding(t: f32, dim: usize, max_period: f32) -> Vec<f32> {
    let half = dim / 2;
    let mut out = vec![0.0f32; dim];
    for i in 0..half {
        let freq = (-(max_period.ln()) * i as f32 / half as f32).exp();
        let arg = t * freq * 2.0 * std::f32::consts::PI;
        out[i] = arg.cos();
        out[half + i] = arg.sin();
    }
    out
}

/// `PcdAbsolutePositionEmbedder` (v1): `freq_dim = channels/3/2` frequencies,
/// the first `max_res` being `2^k` and the rest a linear ramp in the exponent,
/// then `[sin(2*pi*x*f) | cos(2*pi*x*f)]` per input channel, zero padded to
/// `channels`. Note the flatten order: sin block and cos block are
/// concatenated over the LAST axis before the reshape, so one point's row is
/// `[sin(x*f0..)|cos(x*f0..)| sin(y*..)|cos(y*..)| sin(z*..)|cos(z*..)]`.
pub fn pcd_position_embed_v1_freqs(channels: usize, in_channels: usize, max_res: usize) -> Vec<f32> {
    let freq_dim = channels / in_channels / 2;
    let res_dim = freq_dim.saturating_sub(max_res);
    let mut exps = Vec::with_capacity(freq_dim);
    for k in 0..max_res.min(freq_dim) {
        exps.push(k as f32);
    }
    for k in 0..res_dim {
        exps.push(k as f32 / res_dim.max(1) as f32 * max_res as f32);
    }
    exps.truncate(freq_dim);
    exps.iter().map(|e| 2.0f32.powf(*e)).collect()
}

/// `PcdAbsolutePositionEmbedderV2`: `freq_dim` frequencies spaced
/// `2^linspace(0, max_res, freq_dim)`.
pub fn pcd_position_embed_v2_freqs(channels: usize, in_channels: usize, max_res: usize) -> Vec<f32> {
    let freq_dim = channels / in_channels / 2;
    (0..freq_dim)
        .map(|i| {
            let t = if freq_dim <= 1 {
                0.0
            } else {
                i as f32 / (freq_dim - 1) as f32
            };
            2.0f32.powf(t * max_res as f32)
        })
        .collect()
}

/// Embed one `(n, in_channels)` point list. `scale` is the extra factor on the
/// angle: `2*pi` for v1, `pi` for v2. Rows are zero padded to `channels`.
pub fn pcd_position_embed(
    points: &[f32],
    in_channels: usize,
    freqs: &[f32],
    scale: f32,
    channels: usize,
) -> Vec<f32> {
    let n = points.len() / in_channels;
    let mut out = vec![0.0f32; n * channels];
    let width = in_channels * freqs.len() * 2;
    for row in 0..n {
        let dst = row * channels;
        for c in 0..in_channels {
            let value = points[row * in_channels + c];
            let block = dst + c * freqs.len() * 2;
            for (j, freq) in freqs.iter().enumerate() {
                let angle = value * freq * scale;
                out[block + j] = angle.sin();
                out[block + freqs.len() + j] = angle.cos();
            }
        }
        debug_assert!(width <= channels);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_explicit() {
        assert_eq!(TRIPOSPLAT_REVISION.len(), 40);
        for sha in [
            TRIPOSPLAT_DIT_SHA256,
            TRIPOSPLAT_DECODER_SHA256,
            TRIPOSPLAT_DINO_SHA256,
            TRIPOSPLAT_VAE_SHA256,
            TRIPOSPLAT_RMBG_SHA256,
        ] {
            assert_eq!(sha.len(), 64);
        }
        // TripoSplat's BiRefNet repack is byte-length identical to the
        // service's birefnet-hr entry but is a DIFFERENT checkpoint.
        assert_ne!(
            TRIPOSPLAT_RMBG_SHA256,
            "a5a4de698739ea5e0e8bbab28e1b293dde95092b87a442d566cbc585c53cef55"
        );
    }

    #[test]
    fn architecture_constants_match_the_reference_config() {
        assert_eq!(DINO_TOKENS, 4101);
        assert_eq!(DINO_HEAD_DIM, 64);
        assert_eq!(REPO_FREQ_0, 10);
        assert_eq!(REPO_FREQ_1, 10);
        assert_eq!(REPO_FREQ_2, 12);
        assert_eq!(REPO_FREQ_0 + REPO_FREQ_1 + REPO_FREQ_2, REPO_PAIRS);
        assert_eq!(FLOW_MOD_COLS, 6144);
        assert_eq!(GS_OUT_CHANNELS, GS_LAYOUT.iter().map(|(_, n)| n).sum::<usize>());
        assert_eq!(gs_layout_range("_xyz"), Some((0, 96)));
        assert_eq!(gs_layout_range("_features_dc"), Some((96, 192)));
        assert_eq!(gs_layout_range("_rotation"), Some((288, 416)));
        assert_eq!(gs_layout_range("_opacity"), Some((416, 448)));
        assert_eq!(gs_layout_range("_offset_scale"), Some((448, 480)));
    }

    #[test]
    fn num_gaussians_rounds_to_the_decoder_stride() {
        assert_eq!(resolve_num_gaussians(262_144), 262_144);
        assert_eq!(resolve_num_gaussians(32_768), 32_768);
        // 100_000 is already a multiple of 32 and passes through.
        assert_eq!(resolve_num_gaussians(100_000), 100_000);
        // 100_001 rounds to the nearest multiple (down, here).
        assert_eq!(resolve_num_gaussians(100_001), 100_000);
        assert_eq!(resolve_num_gaussians(100_017), 100_032);
        assert_eq!(resolve_num_gaussians(1), GAUSSIANS_MIN);
        assert_eq!(resolve_num_gaussians(10_000_000), GAUSSIANS_MAX);
        assert_eq!(resolve_num_gaussians(50_001) % 32, 0);
    }

    #[test]
    fn t_sequence_matches_the_reference_schedule() {
        let seq = splat_t_sequence(20, 3.0);
        assert_eq!(seq.len(), 21);
        assert!((seq[0] - 1.0).abs() < 1e-15);
        assert_eq!(seq[20], 0.0);
        // t = 0.5 -> 3*0.5 / (1 + 2*0.5) = 0.75
        assert!((seq[10] - 0.75).abs() < 1e-12);
        // shift 1 is the identity schedule.
        let flat = splat_t_sequence(4, 1.0);
        assert!((flat[1] - 0.75).abs() < 1e-12);
    }

    #[test]
    fn cfg_follows_the_diffusers_convention() {
        assert!(!splat_cfg_enabled(1.0));
        assert!(!splat_cfg_enabled(0.5));
        assert!(splat_cfg_enabled(3.0));
        let mut cond = vec![2.0f32, 4.0];
        splat_cfg_combine(&mut cond, &[1.0, 1.0], 3.0);
        // 3*2 - 2*1 = 4 ; 3*4 - 2*1 = 10
        assert_eq!(cond, vec![4.0, 10.0]);
    }

    #[test]
    fn euler_step_math() {
        let mut x = vec![1.0f32, 2.0];
        splat_euler_step(&mut x, &[1.0, 4.0], 0.5, 0.25).unwrap();
        assert_eq!(x, vec![0.75, 1.0]);
    }

    #[test]
    fn timestep_embedding_endpoints() {
        let emb = timestep_embedding(0.0, 8, 10000.0);
        // t = 0 -> cos block all ones, sin block all zeros.
        assert_eq!(&emb[..4], &[1.0, 1.0, 1.0, 1.0]);
        assert_eq!(&emb[4..], &[0.0, 0.0, 0.0, 0.0]);
        // freq[0] = 1, so cos(t), sin(t) lead each block.
        let emb = timestep_embedding(1.0, 4, 10000.0);
        assert!((emb[0] - 1.0f32.cos()).abs() < 1e-6);
        assert!((emb[2] - 1.0f32.sin()).abs() < 1e-6);
    }

    #[test]
    fn level_embedding_carries_the_two_pi() {
        let emb = level_embedding(1.0, 4, 1024.0);
        let two_pi = 2.0 * std::f32::consts::PI;
        assert!((emb[0] - two_pi.cos()).abs() < 1e-5);
        assert!((emb[2] - two_pi.sin()).abs() < 1e-5);
    }

    #[test]
    fn position_embedder_frequency_ladders() {
        // v1 with channels=1024, in=3 -> freq_dim = 170; first 16 are 2^k.
        let v1 = pcd_position_embed_v1_freqs(1024, 3, 16);
        assert_eq!(v1.len(), 170);
        assert_eq!(v1[0], 1.0);
        assert_eq!(v1[4], 16.0);
        assert_eq!(v1[15], 32768.0);
        // v2 spans 2^0 .. 2^max_res inclusive.
        let v2 = pcd_position_embed_v2_freqs(1024, 3, 10);
        assert_eq!(v2.len(), 170);
        assert_eq!(v2[0], 1.0);
        assert!((v2[169] - 1024.0).abs() < 1e-3);
    }

    #[test]
    fn position_embed_layout_is_sin_then_cos_per_axis() {
        let freqs = vec![1.0f32, 2.0];
        // in_channels = 2, freqs = 2 -> width 8, pad to 10.
        let out = pcd_position_embed(&[0.25, 0.5], 2, &freqs, 1.0, 10);
        assert_eq!(out.len(), 10);
        assert!((out[0] - 0.25f32.sin()).abs() < 1e-6);
        assert!((out[1] - 0.5f32.sin()).abs() < 1e-6);
        assert!((out[2] - 0.25f32.cos()).abs() < 1e-6);
        assert!((out[3] - 0.5f32.cos()).abs() < 1e-6);
        assert!((out[4] - 0.5f32.sin()).abs() < 1e-6);
        assert!((out[6] - 0.5f32.cos()).abs() < 1e-6);
        assert_eq!(&out[8..], &[0.0, 0.0]);
    }
}
