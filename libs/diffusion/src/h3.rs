//! MiniMax H3 shared foundation: sharded safetensors weights, the rectified
//! flow schedule, the packed t2va sequence layout, rope tables and the
//! timestep plan. Every formula here mirrors the diffusers reference
//! (transformer_minimax_h3.py / before_denoise.py / scheduling_minimax_h3.py)
//! exactly — grid math in f64, schedule math in f32, rope angles in f32.

use crate::{DiffusionError, Result};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const H3_HIDDEN_SIZE: usize = 5376;
pub const H3_HEAD_COUNT: usize = 56;
pub const H3_HEAD_DIM: usize = 128;
pub const H3_DEPTH: usize = 50;
pub const H3_REFINER_DEPTH: usize = 2;
pub const H3_FFN_DIM: usize = 14336;
pub const H3_IN_CHANNELS: usize = 24;
pub const H3_AUDIO_IN_CHANNELS: usize = 32;
pub const H3_PATCH_H: usize = 2;
pub const H3_PATCH_W: usize = 2;
pub const H3_VIDEO_PATCH_DIM: usize = H3_IN_CHANNELS * H3_PATCH_H * H3_PATCH_W; // 96
pub const H3_TEXT_DIM: usize = 5120;
pub const H3_FREQ_DIM: usize = 256;
pub const H3_TIME_EMBED_HIDDEN: usize = 5376;
pub const H3_TIME_EMBED_DIM: usize = 2688;
pub const H3_ROPE_FREQ_DIM: usize = 16;
pub const H3_ROPE_THETA: f32 = 10000.0;
pub const H3_ROT_HALF: usize = 3 * H3_ROPE_FREQ_DIM; // 48 of head_dim 128
pub const H3_NORM_EPS: f32 = 1e-5;
pub const H3_MODALITY_NUM: usize = 3;
pub const H3_VIDEO_TAG: u8 = 0;
pub const H3_TEXT_TAG: u8 = 1;
pub const H3_AUDIO_TAG: u8 = 2;
pub const H3_VIDEO_SHIFT: f32 = 12.0;
pub const H3_AUDIO_SHIFT: f32 = 3.0;
pub const H3_FPS: usize = 24;
pub const H3_AUDIO_LATENTS_PER_SECOND: usize = 40;
pub const H3_AUDIO_CHANNELS: usize = 2;
pub const H3_VAE_SPATIAL_RATIO: usize = 16;
/// The `t` a visual conditioning anchor is held at, just short of clean
/// (`keyframe_noise_aug` in the reference — trained slightly noised, so
/// conditioning at exactly `t = 1.0` is off-distribution).
pub const H3_KEYFRAME_NOISE_AUG: f32 = 0.999;
pub const H3_VAE_FRAMES_PER_CHUNK: usize = 17;
pub const H3_VAE_LATENTS_PER_CHUNK: usize = 5;
pub const H3_CANVAS_MULTIPLE: usize = 32;

// Rotary-time constants (before_denoise.py).
const ROPE_FRAME_RESCALE: f64 = 5.0 / 3.0;
const ROPE_FRAMES_PER_LATENT: [f64; 5] = [1.0, 4.0, 4.0, 4.0, 4.0];
const ROPE_SPATIAL_SCALE: f64 = 32.0;

// ---------------------------------------------------------------------------
// Sharded safetensors weights (streamed: bytes are read per tensor on demand,
// never a whole-model host copy — the H3 DiT is 66GB against a 61GB-RAM box).
// ---------------------------------------------------------------------------

pub struct H3ShardedWeights {
    pub dir: PathBuf,
    source: H3WeightSource,
}

enum H3WeightSource {
    /// Safetensors shard dir or a single repack file. The map key is the
    /// CANONICAL tensor name; the value carries the shard index and the
    /// file-local spelling (the fp16 video-VAE repack stores its encoder
    /// under LDM names — see `h3_quant::video_vae_repack_canonical_name`).
    Shards {
        shards: Vec<MlxSafetensorsHeader>,
        map: HashMap<String, (usize, String)>,
    },
    Gguf(crate::h3_quant::H3GgufWeights),
    Nvfp4(crate::h3_quant::H3Nvfp4Weights),
}

impl H3ShardedWeights {
    /// Open a safetensors weight source: either every `*.safetensors` in a
    /// dir (headers only, no index.json needed) or one single `.safetensors`
    /// file (the fp16/fp32 repacks). Tensor names are canonicalized.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let mut files: Vec<PathBuf> = if dir.is_file() {
            vec![dir.clone()]
        } else {
            std::fs::read_dir(&dir)
                .map_err(|err| {
                    DiffusionError::model(format!("h3 weights dir {}: {err}", dir.display()))
                })?
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    path.extension()
                        .map(|ext| ext == "safetensors")
                        .unwrap_or(false)
                })
                .collect()
        };
        files.sort();
        if files.is_empty() {
            return Err(DiffusionError::model(format!(
                "h3 weights dir {} holds no safetensors",
                dir.display()
            )));
        }
        let mut shards = Vec::with_capacity(files.len());
        let mut map = HashMap::new();
        for (index, path) in files.iter().enumerate() {
            let header = MlxSafetensorsHeader::load(path)
                .map_err(|err| DiffusionError::model(format!("{}: {err}", path.display())))?;
            for name in header.tensors.keys() {
                let canonical = crate::h3_quant::video_vae_repack_canonical_name(name)
                    .unwrap_or_else(|| name.clone());
                map.insert(canonical, (index, name.clone()));
            }
            shards.push(header);
        }
        let dir = if dir.is_file() {
            dir.parent().map(|p| p.to_path_buf()).unwrap_or(dir)
        } else {
            dir
        };
        Ok(Self {
            dir,
            source: H3WeightSource::Shards { shards, map },
        })
    }

    /// GGUF-quantized DiT (unsloth/leejet MiniMax-H3 fl2va Q4_K family,
    /// full or AdaLN-curve pruned).
    pub fn load_gguf_dit(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let gguf = crate::h3_quant::H3GgufWeights::load(
            path,
            crate::h3_quant::H3QuantComponent::Dit,
        )?;
        Ok(Self {
            dir: path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            source: H3WeightSource::Gguf(gguf),
        })
    }

    /// GGUF-quantized Qwen3-VL text encoder (qwen3vl_32b Q4_K_M).
    pub fn load_gguf_te(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let gguf = crate::h3_quant::H3GgufWeights::load(
            path,
            crate::h3_quant::H3QuantComponent::TextEncoder,
        )?;
        Ok(Self {
            dir: path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            source: H3WeightSource::Gguf(gguf),
        })
    }

    /// ComfyUI/ModelOpt NVFP4 DiT checkpoint (Blackwell tier).
    pub fn load_nvfp4_dit(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let weights = crate::h3_quant::H3Nvfp4Weights::load(
            path,
            crate::h3_quant::H3QuantComponent::Dit,
        )?;
        Ok(Self {
            dir: path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            source: H3WeightSource::Nvfp4(weights),
        })
    }

    /// NVFP4-AWQ text encoder checkpoint (Blackwell tier).
    pub fn load_nvfp4_te(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let weights = crate::h3_quant::H3Nvfp4Weights::load(
            path,
            crate::h3_quant::H3QuantComponent::TextEncoder,
        )?;
        Ok(Self {
            dir: path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            source: H3WeightSource::Nvfp4(weights),
        })
    }

    fn shard_for(&self, name: &str) -> Result<(&MlxSafetensorsHeader, &str)> {
        match &self.source {
            H3WeightSource::Shards { shards, map } => {
                let (index, file_name) = map.get(name).ok_or_else(|| {
                    DiffusionError::model(format!(
                        "h3 tensor '{name}' not found in {}",
                        self.dir.display()
                    ))
                })?;
                Ok((&shards[*index], file_name.as_str()))
            }
            _ => Err(DiffusionError::model(format!(
                "h3 tensor '{name}': not a safetensors source"
            ))),
        }
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        match &self.source {
            H3WeightSource::Shards { map, .. } => map.contains_key(name),
            H3WeightSource::Gguf(gguf) => gguf.has_tensor(name),
            H3WeightSource::Nvfp4(weights) => weights.has_tensor(name),
        }
    }

    /// On-disk byte size of one tensor (safetensors data span).
    pub fn tensor_disk_bytes(&self, name: &str) -> Result<u64> {
        match &self.source {
            H3WeightSource::Shards { .. } => {
                let (shard, file_name) = self.shard_for(name)?;
                let entry = shard.tensor(file_name).ok_or_else(|| {
                    DiffusionError::model(format!("h3 tensor '{name}' missing entry"))
                })?;
                Ok(entry.data_len_bytes())
            }
            H3WeightSource::Gguf(gguf) => gguf.tensor_disk_bytes(name),
            H3WeightSource::Nvfp4(weights) => weights.tensor_disk_bytes(name),
        }
    }

    /// Total on-disk bytes of every tensor across the shards — the "total"
    /// side of load-progress labels ("load moss te 1.2/3.4GB").
    pub fn total_disk_bytes(&self) -> u64 {
        match &self.source {
            H3WeightSource::Shards { shards, .. } => shards
                .iter()
                .map(|shard| {
                    shard
                        .tensors
                        .values()
                        .map(|entry| entry.data_len_bytes())
                        .sum::<u64>()
                })
                .sum(),
            H3WeightSource::Gguf(gguf) => gguf.total_disk_bytes(),
            H3WeightSource::Nvfp4(weights) => weights.total_disk_bytes(),
        }
    }

    pub fn tensor_names(&self) -> Vec<String> {
        match &self.source {
            H3WeightSource::Shards { map, .. } => map.keys().cloned().collect(),
            H3WeightSource::Gguf(gguf) => gguf.tensor_names(),
            H3WeightSource::Nvfp4(weights) => weights.tensor_names(),
        }
    }

    pub fn tensor_dtype_shape(&self, name: &str) -> Result<(MlxDType, Vec<u64>)> {
        let (shard, file_name) = self.shard_for(name)?;
        let entry = shard
            .tensor(file_name)
            .ok_or_else(|| DiffusionError::model(format!("h3 tensor '{name}' missing entry")))?;
        Ok((entry.dtype, entry.shape.clone()))
    }

    /// Raw on-disk bytes of one tensor. For quantized sources this is the
    /// device-uploadable linear payload (block stream / packed pairs blob),
    /// row-sliced out of fused tensors where needed — exactly what
    /// `gpu_weight_cache_ensure(_quant)` expects for `linear_ggml_type`.
    pub fn tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        match &self.source {
            H3WeightSource::Shards { .. } => {
                let (shard, file_name) = self.shard_for(name)?;
                shard
                    .read_tensor_bytes(file_name)
                    .map_err(|err| DiffusionError::model(format!("h3 tensor '{name}': {err}")))
            }
            H3WeightSource::Gguf(gguf) => gguf.linear_payload(name),
            H3WeightSource::Nvfp4(weights) => weights.linear_payload(name),
        }
    }

    /// One row of a rank-2 tensor (e.g. a single embedding row) as f32.
    pub fn tensor_row_f32(&self, name: &str, row: u64) -> Result<Vec<f32>> {
        match &self.source {
            H3WeightSource::Shards { .. } => {
                let (shard, file_name) = self.shard_for(name)?;
                let entry = shard.tensor(file_name).ok_or_else(|| {
                    DiffusionError::model(format!("h3 tensor '{name}' missing entry"))
                })?;
                let dtype = entry.dtype;
                let bytes = shard.read_rank2_row_bytes(file_name, row).map_err(|err| {
                    DiffusionError::model(format!("h3 tensor '{name}' row {row}: {err}"))
                })?;
                bytes_to_f32(&bytes, dtype, name)
            }
            H3WeightSource::Gguf(gguf) => gguf.tensor_row_f32(name, row),
            H3WeightSource::Nvfp4(weights) => weights.tensor_row_f32(name, row),
        }
    }

    /// Whole tensor decoded to f32 (small host-side tensors: biases, norm
    /// weights, the f32 modules, the timestep MLP).
    pub fn tensor_f32(&self, name: &str) -> Result<Vec<f32>> {
        match &self.source {
            H3WeightSource::Shards { .. } => {
                let (dtype, _shape) = self.tensor_dtype_shape(name)?;
                let bytes = self.tensor_bytes(name)?;
                bytes_to_f32(&bytes, dtype, name)
            }
            H3WeightSource::Gguf(gguf) => gguf.tensor_f32(name),
            H3WeightSource::Nvfp4(weights) => weights.tensor_f32(name),
        }
    }

    /// The ggml type `tensor_bytes(name)` yields for a streamed 2-D linear
    /// weight: bf16 for safetensors sources, the per-tensor quantized (or
    /// raw) type for GGUF/NVFP4 sources.
    pub fn linear_ggml_type(&self, name: &str) -> Result<u32> {
        match &self.source {
            H3WeightSource::Shards { .. } => Ok(crate::h3_quant::GGML_TYPE_BF16),
            H3WeightSource::Gguf(gguf) => gguf
                .linear(name)
                .map(|linear| linear.ggml_type)
                .ok_or_else(|| {
                    DiffusionError::model(format!("h3 gguf linear '{name}' not mapped"))
                }),
            H3WeightSource::Nvfp4(weights) => weights
                .linear(name)
                .map(|linear| linear.ggml_type)
                .ok_or_else(|| {
                    DiffusionError::model(format!("h3 nvfp4 linear '{name}' not mapped"))
                }),
        }
    }

    /// The pruned checkpoints' AdaLN timestep curve (replaces the
    /// `time_embedder` MLP); `None` for the full/bf16 layouts.
    pub fn adaln_curve(&self) -> Option<&crate::h3_quant::H3AdalnCurve> {
        match &self.source {
            H3WeightSource::Shards { .. } => None,
            H3WeightSource::Gguf(gguf) => gguf.adaln_curve(),
            H3WeightSource::Nvfp4(weights) => weights.adaln_curve(),
        }
    }

    /// Device weight-cache namespace for the DiT when streamed from this
    /// source. Variants keep the `h3dit::`-prefixed key space (so the
    /// backend's unload prefixes catch every variant) while making
    /// cross-source cache-key collisions impossible.
    pub fn dit_namespace(&self) -> &'static str {
        match &self.source {
            H3WeightSource::Shards { .. } => "h3dit",
            H3WeightSource::Gguf(_) => "h3dit::gg",
            H3WeightSource::Nvfp4(_) => "h3dit::nv",
        }
    }

    /// Same for the text encoder ("h3te::"-prefixed).
    pub fn te_namespace(&self) -> &'static str {
        match &self.source {
            H3WeightSource::Shards { .. } => "h3te",
            H3WeightSource::Gguf(_) => "h3te::gg",
            H3WeightSource::Nvfp4(_) => "h3te::nv",
        }
    }
}

fn bytes_to_f32(bytes: &[u8], dtype: MlxDType, name: &str) -> Result<Vec<f32>> {
    match dtype {
        MlxDType::F32 => Ok(bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()),
        MlxDType::BF16 => Ok(bytes
            .chunks_exact(2)
            .map(|chunk| {
                let word = u16::from_le_bytes([chunk[0], chunk[1]]);
                f32::from_bits((word as u32) << 16)
            })
            .collect()),
        MlxDType::F16 => Ok(bytes
            .chunks_exact(2)
            .map(|chunk| {
                let word = u16::from_le_bytes([chunk[0], chunk[1]]);
                crate::h3::f16_word_to_f32(word)
            })
            .collect()),
        other => Err(DiffusionError::model(format!(
            "h3 tensor '{name}': unsupported dtype {other:?} for f32 decode"
        ))),
    }
}

pub fn f16_word_to_f32(word: u16) -> f32 {
    let sign = ((word >> 15) & 1) as u32;
    let exp = ((word >> 10) & 0x1f) as u32;
    let frac = (word & 0x3ff) as u32;
    let bits = if exp == 0 {
        if frac == 0 {
            sign << 31
        } else {
            // subnormal
            let mut exp32 = 127 - 15 + 1;
            let mut frac32 = frac;
            while frac32 & 0x400 == 0 {
                frac32 <<= 1;
                exp32 -= 1;
            }
            frac32 &= 0x3ff;
            (sign << 31) | ((exp32 as u32) << 23) | (frac32 << 13)
        }
    } else if exp == 31 {
        (sign << 31) | (0xff << 23) | (frac << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (frac << 13)
    };
    f32::from_bits(bits)
}

// ---------------------------------------------------------------------------
// Frame / latent arithmetic (modular_pipeline.py helpers).
// ---------------------------------------------------------------------------

/// Snap a frame count up to the next `17n + 5` the video VAE can decode.
pub fn h3_align_num_frames(num_frames: usize) -> usize {
    let mut frames = num_frames.max(1);
    while frames % H3_VAE_FRAMES_PER_CHUNK != H3_VAE_LATENTS_PER_CHUNK {
        frames += 1;
    }
    frames
}

/// Latent frames for an aligned `17n + 5` frame count: `5n + 2`.
pub fn h3_video_latent_num_frames(num_frames: usize) -> Result<usize> {
    if num_frames % H3_VAE_FRAMES_PER_CHUNK != H3_VAE_LATENTS_PER_CHUNK {
        return Err(DiffusionError::workflow(format!(
            "h3 num_frames must be 17n+5, got {num_frames}"
        )));
    }
    Ok((num_frames - H3_VAE_LATENTS_PER_CHUNK) / H3_VAE_FRAMES_PER_CHUNK
        * H3_VAE_LATENTS_PER_CHUNK
        + 2)
}

/// Audio latents per channel covering `num_frames` at 24 fps / 40 latents/s.
pub fn h3_audio_latent_num_frames(num_frames: usize) -> usize {
    (num_frames as f64 / H3_FPS as f64 * H3_AUDIO_LATENTS_PER_SECOND as f64).round() as usize
}

// ---------------------------------------------------------------------------
// Rectified-flow schedule (scheduling_minimax_h3.py). All f32, matching the
// reference op-by-op: linspace grid, exponential shift, consecutive dedup.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct H3Schedule {
    /// Sigma grid including the terminal 0 — `timesteps.len() + 1` entries.
    pub sigmas: Vec<f32>,
    /// `1 - sigmas[:-1]` — one forward per entry; t = 1 is clean.
    pub timesteps: Vec<f32>,
}

pub fn h3_schedule(num_inference_steps: usize, shift: f32) -> Result<H3Schedule> {
    if num_inference_steps < 2 {
        return Err(DiffusionError::workflow(
            "h3 schedule needs num_inference_steps >= 2",
        ));
    }
    let n = num_inference_steps;
    let mut sigmas = Vec::with_capacity(n);
    for i in 0..n {
        // torch.linspace(1, 0, n, dtype=f32): double step, rounded per value.
        let base = (1.0 - i as f64 / (n - 1) as f64) as f32;
        // shift * base / (1 + (shift - 1) * base), each op rounded at f32.
        let num = shift * base;
        let den = 1.0f32 + (shift - 1.0) * base;
        sigmas.push(num / den);
    }
    // unique_consecutive: the shift compresses near sigma = 1.
    sigmas.dedup();
    let timesteps = sigmas[..sigmas.len() - 1]
        .iter()
        .map(|sigma| 1.0 - sigma)
        .collect();
    Ok(H3Schedule { sigmas, timesteps })
}

/// One Euler step, in the reference's exact arithmetic: `x0 = x + (1-t) * v`
/// (data-ward velocity, sigma recovered FROM THE TIMESTEP, not the grid),
/// then `x_next = r*x + (1-r)*x0` with `r = sigma_next / sigma` from the
/// GRID. All f32.
pub fn h3_euler_step(
    sample: &mut [f32],
    velocity: &[f32],
    timestep: f32,
    sigma: f32,
    sigma_next: f32,
) -> Result<()> {
    if sample.len() != velocity.len() {
        return Err(DiffusionError::workflow(format!(
            "h3 euler step length mismatch: {} vs {}",
            sample.len(),
            velocity.len()
        )));
    }
    let sigma_from_timestep = 1.0 - timestep;
    let ratio = sigma_next / sigma;
    for (x, v) in sample.iter_mut().zip(velocity.iter()) {
        let denoised = *x + sigma_from_timestep * *v;
        *x = ratio * *x + (1.0 - ratio) * denoised;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Packed t2va layout (before_denoise.py build_packed_sequence). All position
// math in f64; the rope forward later casts to f32.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct H3PackedLayout {
    pub num_text_tokens: usize,
    pub num_latent_frames: usize,
    pub latent_height: usize,
    pub latent_width: usize,
    pub num_audio_latents: usize,
    pub rows_per_frame: usize,
    /// Leading keyframe-conditioning video rows (fl2va; 0 for t2va). They sit
    /// between the text rows and the audio rows, count as video rows for
    /// tags/rope/heads, are pinned near-clean in the timestep plan, and the
    /// denoise loop never writes them.
    pub num_condition_rows: usize,
    pub num_audio_rows: usize,
    /// GENERATED video rows (excludes the conditioning rows).
    pub num_video_rows: usize,
    pub sequence_length: usize,
    /// First conditioning row (== num_text_tokens).
    pub condition_start: usize,
    pub audio_start: usize,
    pub video_start: usize,
    /// (seq, 3) row-major (t, h, w) rotary coordinates, f64.
    pub position_ids: Vec<f64>,
    /// Modality tag per row: 0 video, 1 text, 2 audio.
    pub token_tags: Vec<u8>,
}

fn spatial_position_grid(dim: usize, patch: usize, sqrt_area: f64) -> Vec<f64> {
    // np.linspace(left, left + ratio, dim / patch, endpoint=False) * 32:
    // start + arange(num) * (stop - start) / num, f64 throughout.
    let ratio = dim as f64 / sqrt_area;
    let left = (1.0 - ratio) / 2.0;
    let num = dim / patch;
    let step = ratio / num as f64;
    (0..num)
        .map(|i| (i as f64 * step + left) * ROPE_SPATIAL_SCALE)
        .collect()
}

fn temporal_position_grid(num_latent_frames: usize, origin: f64) -> Vec<f64> {
    // origin + [0, cumsum(5/3 * pattern)[..n-1]]
    let mut out = Vec::with_capacity(num_latent_frames);
    let mut acc = 0.0f64;
    for index in 0..num_latent_frames {
        out.push(origin + acc);
        acc += ROPE_FRAME_RESCALE * ROPE_FRAMES_PER_LATENT[index % ROPE_FRAMES_PER_LATENT.len()];
    }
    out
}

/// Build the `[text | target audio | target video]` t2va layout.
pub fn h3_build_t2va_layout(
    text_token_tags: &[u8],
    num_latent_frames: usize,
    latent_height: usize,
    latent_width: usize,
    num_audio_latents: usize,
) -> Result<H3PackedLayout> {
    h3_build_packed_layout(
        text_token_tags,
        num_latent_frames,
        latent_height,
        latent_width,
        num_audio_latents,
        0,
    )
}

/// Build the `[text | keyframe conditions | target audio | target video]`
/// layout shared by t2va (`num_first_keyframes == 0`) and fl2va. Every
/// keyframe conditioning block is "first"-anchored: its rotary time is the
/// media clock's origin `float(num_text_tokens)` and its spatial grid is the
/// video frame grid. (The reference also supports a "last" anchor for
/// `last_image`; out of scope here.)
pub fn h3_build_packed_layout(
    text_token_tags: &[u8],
    num_latent_frames: usize,
    latent_height: usize,
    latent_width: usize,
    num_audio_latents: usize,
    num_first_keyframes: usize,
) -> Result<H3PackedLayout> {
    if latent_height % H3_PATCH_H != 0 || latent_width % H3_PATCH_W != 0 {
        return Err(DiffusionError::workflow(format!(
            "h3 latent canvas {latent_width}x{latent_height} not divisible by patch"
        )));
    }
    let rows_per_frame = (latent_height / H3_PATCH_H) * (latent_width / H3_PATCH_W);
    let num_text_tokens = text_token_tags.len();
    let num_condition_rows = num_first_keyframes * rows_per_frame;
    let num_audio_rows = num_audio_latents * H3_AUDIO_CHANNELS;
    let num_video_rows = num_latent_frames * rows_per_frame;
    let sequence_length =
        num_text_tokens + num_condition_rows + num_audio_rows + num_video_rows;
    let condition_start = num_text_tokens;
    let audio_start = condition_start + num_condition_rows;
    let video_start = audio_start + num_audio_rows;

    let mut position_ids = vec![0.0f64; sequence_length * 3];
    let mut token_tags = vec![0u8; sequence_length];

    // Text rows: t = row index; h = w = 0.
    for row in 0..num_text_tokens {
        position_ids[row * 3] = row as f64;
        token_tags[row] = text_token_tags[row];
    }

    let sqrt_area = ((latent_height * latent_width) as f64).sqrt();
    let height_grid = spatial_position_grid(latent_height, H3_PATCH_H, sqrt_area);
    let width_grid = spatial_position_grid(latent_width, H3_PATCH_W, sqrt_area);

    // Keyframe conditioning rows: "first" anchor -> rotary time is the media
    // clock origin; spatial coordinates are the video frame grid; tagged as
    // video rows.
    let mut row = condition_start;
    for _keyframe in 0..num_first_keyframes {
        for h in &height_grid {
            for w in &width_grid {
                position_ids[row * 3] = num_text_tokens as f64;
                position_ids[row * 3 + 1] = *h;
                position_ids[row * 3 + 2] = *w;
                token_tags[row] = H3_VIDEO_TAG;
                row += 1;
            }
        }
    }
    debug_assert_eq!(row, audio_start);

    // Audio rows, channel-major: t = n_text + i per channel, h = 0, w pinned
    // to the two extremes of the width grid (left channel first).
    for channel in 0..H3_AUDIO_CHANNELS {
        let w_pin = if channel == 0 {
            width_grid[0]
        } else {
            width_grid[width_grid.len() - 1]
        };
        for i in 0..num_audio_latents {
            let row = audio_start + channel * num_audio_latents + i;
            position_ids[row * 3] = num_text_tokens as f64 + i as f64;
            position_ids[row * 3 + 2] = w_pin;
            token_tags[row] = H3_AUDIO_TAG;
        }
    }

    // Video rows: frame-major, then h-major, then w-major.
    let frame_time = temporal_position_grid(num_latent_frames, num_text_tokens as f64);
    let mut row = video_start;
    for frame in 0..num_latent_frames {
        let t = frame_time[frame];
        for h in &height_grid {
            for w in &width_grid {
                position_ids[row * 3] = t;
                position_ids[row * 3 + 1] = *h;
                position_ids[row * 3 + 2] = *w;
                token_tags[row] = H3_VIDEO_TAG;
                row += 1;
            }
        }
    }
    debug_assert_eq!(row, sequence_length);

    Ok(H3PackedLayout {
        num_text_tokens,
        num_latent_frames,
        latent_height,
        latent_width,
        num_audio_latents,
        rows_per_frame,
        num_condition_rows,
        num_audio_rows,
        num_video_rows,
        sequence_length,
        condition_start,
        audio_start,
        video_start,
        position_ids,
        token_tags,
    })
}

/// Rectified-flow forward process in MiniMax-H3's `t` convention:
/// `x_t = t*x_0 + (1-t)*noise` (f32; `t = 1` returns the sample unchanged).
/// The fl2va keyframe anchors are noised with `t = 0.999`
/// ([`H3_KEYFRAME_NOISE_AUG`]).
pub fn h3_scale_noise(sample: &[f32], timestep: f32, noise: &[f32]) -> Result<Vec<f32>> {
    if sample.len() != noise.len() {
        return Err(DiffusionError::workflow(format!(
            "h3 scale_noise length mismatch: {} vs {}",
            sample.len(),
            noise.len()
        )));
    }
    Ok(sample
        .iter()
        .zip(noise.iter())
        .map(|(x0, n)| timestep * *x0 + (1.0 - timestep) * *n)
        .collect())
}

// ---------------------------------------------------------------------------
// Rope tables: (seq, 48) cos/sin in f32 — position_ids cast f64 -> f32, then
// angle = pos_axis * inv_freq[j], inv_freq = theta^(-j/16). The reference
// duplicates the 48-frequency block across both rotated halves, so the table
// holds one half and the kernel reuses it.
// ---------------------------------------------------------------------------

pub struct H3RopeTables {
    pub rows: usize,
    /// (rows, 48) row-major.
    pub cos: Vec<f32>,
    pub sin: Vec<f32>,
}

pub fn h3_rope_tables(position_ids: &[f64]) -> H3RopeTables {
    let rows = position_ids.len() / 3;
    let mut inv_freq = [0.0f32; H3_ROPE_FREQ_DIM];
    for (j, value) in inv_freq.iter_mut().enumerate() {
        // 1 / theta^(2j / (2 * freq_dim)), computed like torch: f32 pow.
        *value = 1.0 / H3_ROPE_THETA.powf(2.0 * j as f32 / (2.0 * H3_ROPE_FREQ_DIM as f32));
    }
    let mut cos = vec![0.0f32; rows * H3_ROT_HALF];
    let mut sin = vec![0.0f32; rows * H3_ROT_HALF];
    for row in 0..rows {
        for axis in 0..3 {
            let pos = position_ids[row * 3 + axis] as f32;
            for j in 0..H3_ROPE_FREQ_DIM {
                let angle = pos * inv_freq[j];
                let col = axis * H3_ROPE_FREQ_DIM + j;
                cos[row * H3_ROT_HALF + col] = angle.cos();
                sin[row * H3_ROT_HALF + col] = angle.sin();
            }
        }
    }
    H3RopeTables { rows, cos, sin }
}

// ---------------------------------------------------------------------------
// Row timestep plan (before_denoise.py build_row_timesteps): distinct
// timesteps sorted ascending + a per-row index, then the AdaLN index
// idx = t_idx * 3 + tag.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct H3RowTimesteps {
    /// Distinct timesteps, ascending (1 or 2 for t2va, up to 3 with anchors).
    pub values: Vec<f32>,
    /// Per-row index into `values`.
    pub indices: Vec<u32>,
    /// Per-row AdaLN table row: `indices * 3 + token_tags`.
    pub adaln_indices: Vec<u32>,
}

pub fn h3_build_row_timesteps(
    layout: &H3PackedLayout,
    video_timestep: f32,
    audio_timestep: f32,
) -> H3RowTimesteps {
    h3_build_row_timesteps_cond(layout, video_timestep, audio_timestep, video_timestep)
}

/// Row timestep plan with the fl2va conditioning rows pinned at their own
/// timestep. The reference pins them at `max(video_t, 0.999)` per step —
/// that `max` stays at the call site.
pub fn h3_build_row_timesteps_cond(
    layout: &H3PackedLayout,
    video_timestep: f32,
    audio_timestep: f32,
    condition_video_timestep: f32,
) -> H3RowTimesteps {
    let mut row_timesteps = vec![video_timestep; layout.sequence_length];
    for row in layout.condition_start..layout.audio_start {
        row_timesteps[row] = condition_video_timestep;
    }
    for row in layout.audio_start..layout.video_start {
        row_timesteps[row] = audio_timestep;
    }
    // torch.unique(sorted=True, return_inverse=True)
    let mut values: Vec<f32> = Vec::new();
    for value in &row_timesteps {
        if !values.iter().any(|existing| existing == value) {
            values.push(*value);
        }
    }
    values.sort_by(|a, b| a.partial_cmp(b).expect("finite timesteps"));
    let indices: Vec<u32> = row_timesteps
        .iter()
        .map(|value| {
            values
                .iter()
                .position(|existing| existing == value)
                .expect("timestep present") as u32
        })
        .collect();
    let adaln_indices = indices
        .iter()
        .zip(layout.token_tags.iter())
        .map(|(t_idx, tag)| t_idx * H3_MODALITY_NUM as u32 + *tag as u32)
        .collect();
    H3RowTimesteps {
        values,
        indices,
        adaln_indices,
    }
}

// ---------------------------------------------------------------------------
// Timestep sinusoid (diffusers get_timestep_embedding: half = 128,
// exponent = -ln(10000) * j / 128, order [cos | sin], f32).
// ---------------------------------------------------------------------------

pub fn h3_timestep_embedding(timesteps: &[f32]) -> Vec<f32> {
    let half = H3_FREQ_DIM / 2;
    let ln_max_period = (10000.0f64).ln() as f32;
    let mut out = vec![0.0f32; timesteps.len() * H3_FREQ_DIM];
    for (row, t) in timesteps.iter().enumerate() {
        for j in 0..half {
            let exponent = -ln_max_period * j as f32 / half as f32;
            let angle = *t * exponent.exp();
            out[row * H3_FREQ_DIM + j] = angle.cos();
            out[row * H3_FREQ_DIM + half + j] = angle.sin();
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Video latent (un)packing: patchify (1, 24, nf, lh, lw) -> rows of 96 with
// column order c*4 + py*2 + px, frame-major then h-major then w-major rows.
// ---------------------------------------------------------------------------

pub fn h3_patchify_video_latents(
    latents: &[f32],
    num_latent_frames: usize,
    latent_height: usize,
    latent_width: usize,
) -> Result<Vec<f32>> {
    let expected = H3_IN_CHANNELS * num_latent_frames * latent_height * latent_width;
    if latents.len() != expected {
        return Err(DiffusionError::workflow(format!(
            "h3 patchify expected {expected} values, got {}",
            latents.len()
        )));
    }
    let rows_h = latent_height / H3_PATCH_H;
    let rows_w = latent_width / H3_PATCH_W;
    let num_rows = num_latent_frames * rows_h * rows_w;
    let mut out = vec![0.0f32; num_rows * H3_VIDEO_PATCH_DIM];
    let plane = latent_height * latent_width;
    let frame_stride = plane; // per channel
    for frame in 0..num_latent_frames {
        for hy in 0..rows_h {
            for wx in 0..rows_w {
                let row = (frame * rows_h + hy) * rows_w + wx;
                let base = row * H3_VIDEO_PATCH_DIM;
                for c in 0..H3_IN_CHANNELS {
                    for py in 0..H3_PATCH_H {
                        for px in 0..H3_PATCH_W {
                            let src = c * (num_latent_frames * frame_stride)
                                + frame * frame_stride
                                + (hy * H3_PATCH_H + py) * latent_width
                                + wx * H3_PATCH_W
                                + px;
                            out[base + c * (H3_PATCH_H * H3_PATCH_W) + py * H3_PATCH_W + px] =
                                latents[src];
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

pub fn h3_unpatchify_video_latents(
    rows: &[f32],
    num_latent_frames: usize,
    latent_height: usize,
    latent_width: usize,
) -> Result<Vec<f32>> {
    let rows_h = latent_height / H3_PATCH_H;
    let rows_w = latent_width / H3_PATCH_W;
    let num_rows = num_latent_frames * rows_h * rows_w;
    if rows.len() != num_rows * H3_VIDEO_PATCH_DIM {
        return Err(DiffusionError::workflow(format!(
            "h3 unpatchify expected {} values, got {}",
            num_rows * H3_VIDEO_PATCH_DIM,
            rows.len()
        )));
    }
    let plane = latent_height * latent_width;
    let mut out = vec![0.0f32; H3_IN_CHANNELS * num_latent_frames * plane];
    for frame in 0..num_latent_frames {
        for hy in 0..rows_h {
            for wx in 0..rows_w {
                let row = (frame * rows_h + hy) * rows_w + wx;
                let base = row * H3_VIDEO_PATCH_DIM;
                for c in 0..H3_IN_CHANNELS {
                    for py in 0..H3_PATCH_H {
                        for px in 0..H3_PATCH_W {
                            let dst = c * (num_latent_frames * plane)
                                + frame * plane
                                + (hy * H3_PATCH_H + py) * latent_width
                                + wx * H3_PATCH_W
                                + px;
                            out[dst] = rows
                                [base + c * (H3_PATCH_H * H3_PATCH_W) + py * H3_PATCH_W + px];
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
    fn schedule_matches_reference_algebra() {
        // 4 steps, video shift 12: base = [1, 2/3, 1/3, 0].
        let sched = h3_schedule(4, 12.0).unwrap();
        assert_eq!(sched.sigmas.len(), 4);
        assert!((sched.sigmas[0] - 1.0).abs() < 1e-7);
        assert!((sched.sigmas[1] - 0.96).abs() < 1e-6); // 8/(25/3)
        assert!((sched.sigmas[2] - 6.0 / 7.0).abs() < 1e-6);
        assert_eq!(sched.sigmas[3], 0.0);
        assert_eq!(sched.timesteps.len(), 3);
        assert_eq!(sched.timesteps[0], 0.0);
        // audio shift 3: sigma[1] = 2/(1+4/3) = 6/7, sigma[2] = 0.6.
        let audio = h3_schedule(4, 3.0).unwrap();
        assert!((audio.sigmas[1] - 6.0 / 7.0).abs() < 1e-6);
        assert!((audio.sigmas[2] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn latent_frame_math() {
        assert_eq!(h3_align_num_frames(120), 124);
        assert_eq!(h3_align_num_frames(124), 124);
        assert_eq!(h3_video_latent_num_frames(124).unwrap(), 37);
        assert_eq!(h3_video_latent_num_frames(22).unwrap(), 7);
        assert_eq!(h3_audio_latent_num_frames(124), 207);
    }

    #[test]
    fn t2va_layout_shape() {
        // 640x352 canvas -> latent 40x22 -> rows/frame 20*11 = 220.
        let tags = vec![H3_TEXT_TAG; 90];
        let layout = h3_build_t2va_layout(&tags, 37, 22, 40, 207).unwrap();
        assert_eq!(layout.rows_per_frame, 220);
        assert_eq!(layout.sequence_length, 90 + 414 + 37 * 220);
        assert_eq!(layout.audio_start, 90);
        assert_eq!(layout.video_start, 90 + 414);
        // Text time axis.
        assert_eq!(layout.position_ids[3], 1.0);
        // First audio row: t = 90, w = width_grid[0].
        let a0 = layout.audio_start * 3;
        assert_eq!(layout.position_ids[a0], 90.0);
        // Video frame time spacing: frame 1 starts 5/3 after frame 0, frame 2
        // is 5/3 * 4 later.
        let v0 = layout.video_start * 3;
        let v1 = (layout.video_start + 220) * 3;
        let v2 = (layout.video_start + 440) * 3;
        assert!((layout.position_ids[v1] - layout.position_ids[v0] - 5.0 / 3.0).abs() < 1e-12);
        assert!(
            (layout.position_ids[v2] - layout.position_ids[v1] - 20.0 / 3.0).abs() < 1e-12
        );
        // Aspect-normalized grids: width is the long axis on 40x22.
        let w_first = layout.position_ids[v0 + 2];
        let h_first = layout.position_ids[v0 + 1];
        assert!(w_first < h_first, "wide canvas centres the height grid");
    }

    #[test]
    fn row_timesteps_dedup_and_order() {
        let tags = vec![H3_TEXT_TAG; 4];
        let layout = h3_build_t2va_layout(&tags, 7, 4, 4, 10).unwrap();
        // Equal video/audio timesteps collapse to one value (step 0).
        let plan = h3_build_row_timesteps(&layout, 0.0, 0.0);
        assert_eq!(plan.values.len(), 1);
        assert!(plan.indices.iter().all(|index| *index == 0));
        // Distinct: ascending order, video rows point at the smaller value
        // when video_t < audio_t.
        let plan = h3_build_row_timesteps(&layout, 0.04, 0.14);
        assert_eq!(plan.values, vec![0.04, 0.14]);
        assert_eq!(plan.indices[0], 0); // text inherits video
        assert_eq!(plan.indices[layout.audio_start], 1);
        assert_eq!(plan.indices[layout.video_start], 0);
        // AdaLN index = t_idx * 3 + tag.
        assert_eq!(
            plan.adaln_indices[layout.video_start],
            0 * 3 + H3_VIDEO_TAG as u32
        );
        assert_eq!(
            plan.adaln_indices[layout.audio_start],
            1 * 3 + H3_AUDIO_TAG as u32
        );
        assert_eq!(plan.adaln_indices[0], 0 * 3 + H3_TEXT_TAG as u32);
    }

    #[test]
    fn fl2va_layout_condition_rows() {
        // 640x352 canvas, one first-anchored keyframe: [text | 220 cond | audio | video].
        let tags = vec![H3_TEXT_TAG; 90];
        let layout = h3_build_packed_layout(&tags, 37, 22, 40, 207, 1).unwrap();
        assert_eq!(layout.num_condition_rows, 220);
        assert_eq!(layout.condition_start, 90);
        assert_eq!(layout.audio_start, 90 + 220);
        assert_eq!(layout.video_start, 90 + 220 + 414);
        assert_eq!(layout.sequence_length, 90 + 220 + 414 + 37 * 220);
        // Cond rows: t pinned at the media origin, spatial grid == first video frame's.
        let c0 = layout.condition_start * 3;
        let v0 = layout.video_start * 3;
        assert_eq!(layout.position_ids[c0], 90.0);
        assert_eq!(layout.position_ids[c0], layout.position_ids[v0]);
        for i in 0..220 {
            let c = (layout.condition_start + i) * 3;
            let v = (layout.video_start + i) * 3;
            assert_eq!(layout.position_ids[c + 1], layout.position_ids[v + 1]);
            assert_eq!(layout.position_ids[c + 2], layout.position_ids[v + 2]);
            assert_eq!(layout.token_tags[layout.condition_start + i], H3_VIDEO_TAG);
        }
        // t2va path unchanged: zero cond rows, condition_start == audio_start.
        let t2va = h3_build_t2va_layout(&tags, 37, 22, 40, 207).unwrap();
        assert_eq!(t2va.num_condition_rows, 0);
        assert_eq!(t2va.condition_start, t2va.audio_start);
    }

    #[test]
    fn fl2va_row_timesteps_pin_condition() {
        let tags = vec![H3_TEXT_TAG; 4];
        let layout = h3_build_packed_layout(&tags, 7, 4, 4, 10, 1).unwrap();
        // Step 0: video_t = audio_t = 0, cond pinned at 0.999 -> two values.
        let plan = h3_build_row_timesteps_cond(&layout, 0.0, 0.0, 0.999);
        assert_eq!(plan.values, vec![0.0, 0.999]);
        assert_eq!(plan.indices[0], 0); // text inherits video
        assert_eq!(plan.indices[layout.condition_start], 1);
        assert_eq!(plan.indices[layout.audio_start], 0);
        assert_eq!(plan.indices[layout.video_start], 0);
        // AdaLN: cond rows are VIDEO-tagged at the pinned timestep index.
        assert_eq!(
            plan.adaln_indices[layout.condition_start],
            1 * 3 + H3_VIDEO_TAG as u32
        );
        // Mid-schedule: three distinct values, ascending.
        let plan = h3_build_row_timesteps_cond(&layout, 0.2, 0.5, 0.999);
        assert_eq!(plan.values, vec![0.2, 0.5, 0.999]);
        assert_eq!(plan.indices[layout.condition_start], 2);
        assert_eq!(plan.indices[layout.audio_start], 1);
        // Late schedule where video_t > aug: caller passes max(t, 0.999).
        let plan = h3_build_row_timesteps_cond(&layout, 0.9995, 0.99, 0.9995);
        assert_eq!(plan.values, vec![0.99, 0.9995]);
        assert_eq!(plan.indices[layout.condition_start], 1);
        assert_eq!(plan.indices[layout.video_start], 1);
    }

    #[test]
    fn scale_noise_arithmetic() {
        let x0 = vec![1.0f32, -2.0, 0.5];
        let noise = vec![0.0f32, 1.0, -1.0];
        let out = h3_scale_noise(&x0, 0.999, &noise).unwrap();
        assert!((out[0] - 0.999).abs() < 1e-6);
        assert!((out[1] - (0.999 * -2.0 + 0.001)).abs() < 1e-6);
        let clean = h3_scale_noise(&x0, 1.0, &noise).unwrap();
        assert_eq!(clean, x0);
    }

    #[test]
    fn patchify_roundtrip() {
        let (nf, lh, lw) = (2usize, 4usize, 4usize);
        let count = H3_IN_CHANNELS * nf * lh * lw;
        let latents: Vec<f32> = (0..count).map(|i| i as f32).collect();
        let rows = h3_patchify_video_latents(&latents, nf, lh, lw).unwrap();
        let back = h3_unpatchify_video_latents(&rows, nf, lh, lw).unwrap();
        assert_eq!(latents, back);
        // Row 0, cols 0..4 = channel 0's 2x2 patch at (0,0) of frame 0.
        assert_eq!(rows[0], latents[0]);
        assert_eq!(rows[1], latents[1]);
        assert_eq!(rows[2], latents[lw as usize] as f32);
    }

    #[test]
    fn rope_table_shape() {
        let tags = vec![H3_TEXT_TAG; 2];
        let layout = h3_build_t2va_layout(&tags, 7, 4, 4, 5).unwrap();
        let tables = h3_rope_tables(&layout.position_ids);
        assert_eq!(tables.rows, layout.sequence_length);
        assert_eq!(tables.cos.len(), layout.sequence_length * H3_ROT_HALF);
        // Row 0 (text, t=0): all angles zero -> cos 1, sin 0.
        assert!(tables.cos[..H3_ROT_HALF].iter().all(|v| *v == 1.0));
        assert!(tables.sin[..H3_ROT_HALF].iter().all(|v| *v == 0.0));
        // Row 1 (text, t=1): first t-frequency angle is 1.0 rad.
        assert!((tables.cos[H3_ROT_HALF] - 1.0f32.cos()).abs() < 1e-6);
    }

    #[test]
    fn timestep_embedding_shape() {
        let emb = h3_timestep_embedding(&[0.0, 0.5]);
        assert_eq!(emb.len(), 2 * H3_FREQ_DIM);
        // t = 0: cos half all 1, sin half all 0.
        assert!(emb[..128].iter().all(|v| *v == 1.0));
        assert!(emb[128..256].iter().all(|v| *v == 0.0));
    }
}
