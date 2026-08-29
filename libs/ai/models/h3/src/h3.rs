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

/// Where one canonical tensor lives in the shard set.
#[derive(Clone, Debug)]
struct ShardEntry {
    shard: usize,
    /// The file-local spelling, which may differ from the canonical name.
    file_name: String,
    /// Set when our rows are not simply the file tensor's rows in order —
    /// the video-VAE repack fuses `attn.to_qkv` and swaps `ff.w1`'s halves.
    rows: Option<crate::h3_quant::H3RowMap>,
    /// True when this name was SYNTHESIZED from a differently-spelled file
    /// tensor rather than read off the file verbatim.
    aliased: bool,
}

enum H3WeightSource {
    /// Safetensors shard dir or a single repack file. The map key is the
    /// CANONICAL tensor name; the entry carries the shard index, the
    /// file-local spelling and any row selection (the fp16 video-VAE repack
    /// spells both its encoder and its decoder differently and fuses the
    /// decoder's QKV — see `h3_quant::video_vae_repack_aliases`).
    Shards {
        shards: Vec<MlxSafetensorsHeader>,
        map: HashMap<String, ShardEntry>,
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
        let mut map: HashMap<String, ShardEntry> = HashMap::new();
        for (index, path) in files.iter().enumerate() {
            let header = MlxSafetensorsHeader::load(path)
                .map_err(|err| DiffusionError::model(format!("{}: {err}", path.display())))?;
            for name in header.tensors.keys() {
                // A canonical slot claimed by BOTH a verbatim name and an
                // aliased one means two file tensors describe the same
                // module in two spellings and one would be silently dropped.
                // Fail closed either way round — `header.tensors` is a
                // HashMap, so which of the two lands first is arbitrary and
                // an order-dependent check would be a coin flip. Two
                // verbatim names cannot collide (map keys are unique per
                // file), so plain multi-shard loading is unaffected.
                let aliases = crate::h3_quant::video_vae_repack_aliases(name);
                let claims = if aliases.is_empty() {
                    vec![(name.clone(), None, false)]
                } else {
                    aliases
                        .into_iter()
                        .map(|alias| (alias.canonical, alias.rows, true))
                        .collect()
                };
                for (canonical, rows, aliased) in claims {
                    if let Some(previous) = map.get(&canonical) {
                        if previous.aliased || aliased {
                            return Err(DiffusionError::model(format!(
                                "h3 weights {}: '{name}' claims '{canonical}', already supplied \
                                 by '{}'",
                                path.display(),
                                previous.file_name
                            )));
                        }
                    }
                    map.insert(
                        canonical,
                        ShardEntry {
                            shard: index,
                            file_name: name.clone(),
                            rows,
                            aliased,
                        },
                    );
                }
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

    fn shard_entry(&self, name: &str) -> Result<(&MlxSafetensorsHeader, &ShardEntry)> {
        match &self.source {
            H3WeightSource::Shards { shards, map } => {
                let entry = map.get(name).ok_or_else(|| {
                    DiffusionError::model(format!(
                        "h3 tensor '{name}' not found in {}",
                        self.dir.display()
                    ))
                })?;
                Ok((&shards[entry.shard], entry))
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
                let (shard, mapping) = self.shard_entry(name)?;
                let entry = shard.tensor(&mapping.file_name).ok_or_else(|| {
                    DiffusionError::model(format!("h3 tensor '{name}' missing entry"))
                })?;
                let parts = mapping.rows.map(|rows| rows.parts()).unwrap_or(1);
                Ok(entry.data_len_bytes() / parts)
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
        let (shard, mapping) = self.shard_entry(name)?;
        let entry = shard
            .tensor(&mapping.file_name)
            .ok_or_else(|| DiffusionError::model(format!("h3 tensor '{name}' missing entry")))?;
        let mut shape = entry.shape.clone();
        if let Some(rows) = mapping.rows {
            let leading = shape.first_mut().ok_or_else(|| {
                DiffusionError::model(format!(
                    "h3 tensor '{name}': packed source '{}' is rank 0",
                    mapping.file_name
                ))
            })?;
            *leading = rows.validate(*leading).map_err(|why| {
                DiffusionError::model(format!(
                    "h3 tensor '{name}': packed source '{}' {why}",
                    mapping.file_name
                ))
            })?;
        }
        Ok((entry.dtype, shape))
    }

    /// Raw on-disk bytes of one tensor. For quantized sources this is the
    /// device-uploadable linear payload (block stream / packed pairs blob),
    /// row-sliced out of fused tensors where needed — exactly what
    /// `gpu_weight_cache_ensure(_quant)` expects for `linear_ggml_type`.
    pub fn tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        match &self.source {
            H3WeightSource::Shards { .. } => {
                let (shard, mapping) = self.shard_entry(name)?;
                let Some(row_map) = mapping.rows else {
                    return shard
                        .read_tensor_bytes(&mapping.file_name)
                        .map_err(|err| DiffusionError::model(format!("h3 tensor '{name}': {err}")));
                };
                // Gather our rows run by run, in canonical order. Each run
                // is one contiguous read, so this touches only our bytes.
                let (_dtype, shape) = self.tensor_dtype_shape(name)?;
                let rows = shape[0];
                let run = row_map.run_len(rows).max(1);
                let mut out = Vec::new();
                let mut first = 0u64;
                while first < rows {
                    let take = run.min(rows - first);
                    let chunk = shard
                        .read_row_run_bytes(
                            &mapping.file_name,
                            row_map.file_row(first, rows),
                            take,
                        )
                        .map_err(|err| {
                            DiffusionError::model(format!("h3 tensor '{name}' rows at {first}: {err}"))
                        })?;
                    out.extend_from_slice(&chunk);
                    first += take;
                }
                Ok(out)
            }
            H3WeightSource::Gguf(gguf) => gguf.linear_payload(name),
            H3WeightSource::Nvfp4(weights) => weights.linear_payload(name),
        }
    }

    /// One row of a rank-2 tensor (e.g. a single embedding row) as f32.
    pub fn tensor_row_f32(&self, name: &str, row: u64) -> Result<Vec<f32>> {
        match &self.source {
            H3WeightSource::Shards { .. } => {
                let (shard, mapping) = self.shard_entry(name)?;
                let entry = shard.tensor(&mapping.file_name).ok_or_else(|| {
                    DiffusionError::model(format!("h3 tensor '{name}' missing entry"))
                })?;
                let dtype = entry.dtype;
                let file_row = match mapping.rows {
                    Some(row_map) => {
                        let rows = self.tensor_dtype_shape(name)?.1[0];
                        if row >= rows {
                            return Err(DiffusionError::model(format!(
                                "h3 tensor '{name}' row {row} out of range ({rows} rows)"
                            )));
                        }
                        row_map.file_row(row, rows)
                    }
                    None => row,
                };
                let bytes = shard
                    .read_rank2_row_bytes(&mapping.file_name, file_row)
                    .map_err(|err| {
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
                crate::f16_word_to_f32(word)
            })
            .collect()),
        other => Err(DiffusionError::model(format!(
            "h3 tensor '{name}': unsupported dtype {other:?} for f32 decode"
        ))),
    }
}

pub use makepad_ai_common::f16_word_to_f32;

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
        &[],
    )
}

/// Which end of the clip one keyframe conditioning block anchors.
///
/// Reference: `before_encoder.py::MiniMaxH3FL2VASetupStep.__call__`
/// (diffusers `modular_pipelines/minimax_h3`, lines 118-123) resolves the
/// packed order as `("first", image)` then `("last", last_image)`, keeping
/// only the ones that were supplied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum H3KeyframeAnchor {
    /// Pinned at the first latent frame's rotary time (`float(n_text)`).
    First,
    /// Pinned at the last latent frame's anchor time (see
    /// [`h3_last_anchor_time`]).
    Last,
}

/// numpy's pairwise summation over f64, reproduced exactly
/// (`numpy/_core/src/umath/loops_utils.h::@TYPE@_pairwise_sum`): under 8
/// elements it sums sequentially, up to 128 it accumulates into 8 partials
/// and folds them as `((r0+r1)+(r2+r3))+((r4+r5)+(r6+r7))`, and above that
/// it splits at a multiple of 8 and recurses.
///
/// The "last" keyframe anchor is the one H3 call site that goes through
/// `ndarray.sum()` rather than a sequential accumulation, and the two orders
/// differ in the last ulp from 16 latent frames onwards — so this is not a
/// cosmetic detail, it is the reference's arithmetic.
fn numpy_pairwise_sum(values: &[f64]) -> f64 {
    let n = values.len();
    if n < 8 {
        let mut acc = 0.0f64;
        for value in values {
            acc += *value;
        }
        return acc;
    }
    if n <= 128 {
        let mut r = [
            values[0], values[1], values[2], values[3], values[4], values[5], values[6],
            values[7],
        ];
        let mut i = 8;
        while i < n - (n % 8) {
            for (k, slot) in r.iter_mut().enumerate() {
                *slot += values[i + k];
            }
            i += 8;
        }
        let mut acc = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            acc += values[i];
            i += 1;
        }
        return acc;
    }
    let mut half = n / 2;
    half -= half % 8;
    numpy_pairwise_sum(&values[..half]) + numpy_pairwise_sum(&values[half..])
}

/// Rotary time of a "last"-anchored keyframe conditioning block.
///
/// Reference: `before_denoise.py::build_packed_sequence` lines 328-336 —
/// `spans = ones(n) * 5/3` scaled per position by the `(1,4,4,4,4)` pattern,
/// then `float(n_text) + float(spans.sum()) - 5/3`. Note this is NOT the
/// last generated frame's own rotary time (which stops one span short of the
/// total): it is the clip's full rotary span, less one base unit.
pub fn h3_last_anchor_time(num_latent_frames: usize, num_text_tokens: usize) -> f64 {
    let spans: Vec<f64> = (0..num_latent_frames)
        .map(|index| {
            ROPE_FRAME_RESCALE * ROPE_FRAMES_PER_LATENT[index % ROPE_FRAMES_PER_LATENT.len()]
        })
        .collect();
    num_text_tokens as f64 + numpy_pairwise_sum(&spans) - ROPE_FRAME_RESCALE
}

/// Build the `[text | keyframe conditions | target audio | target video]`
/// layout shared by t2va (`keyframe_anchors.is_empty()`) and fl2va. Every
/// keyframe conditioning block takes the video frame's spatial grid; its
/// rotary time is the media clock's origin `float(num_text_tokens)` for a
/// [`H3KeyframeAnchor::First`] block and [`h3_last_anchor_time`] for a
/// [`H3KeyframeAnchor::Last`] one. Blocks are laid out in the packed order
/// the slice gives (upstream: first, then last).
pub fn h3_build_packed_layout(
    text_token_tags: &[u8],
    num_latent_frames: usize,
    latent_height: usize,
    latent_width: usize,
    num_audio_latents: usize,
    keyframe_anchors: &[H3KeyframeAnchor],
) -> Result<H3PackedLayout> {
    if latent_height % H3_PATCH_H != 0 || latent_width % H3_PATCH_W != 0 {
        return Err(DiffusionError::workflow(format!(
            "h3 latent canvas {latent_width}x{latent_height} not divisible by patch"
        )));
    }
    let rows_per_frame = (latent_height / H3_PATCH_H) * (latent_width / H3_PATCH_W);
    let num_text_tokens = text_token_tags.len();
    let num_condition_rows = keyframe_anchors.len() * rows_per_frame;
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

    // Keyframe conditioning rows: the anchor picks the rotary time ("first"
    // = the media clock origin, "last" = the clip's rotary span less one
    // base unit); spatial coordinates are the video frame grid; tagged as
    // video rows.
    let mut row = condition_start;
    for anchor in keyframe_anchors {
        let anchor_time = match anchor {
            H3KeyframeAnchor::First => num_text_tokens as f64,
            H3KeyframeAnchor::Last => h3_last_anchor_time(num_latent_frames, num_text_tokens),
        };
        for h in &height_grid {
            for w in &width_grid {
                position_ids[row * 3] = anchor_time;
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
        let layout = h3_build_packed_layout(&tags, 37, 22, 40, 207, &[H3KeyframeAnchor::First]).unwrap();
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

    /// `h3_last_anchor_time` against numpy: `float(n_text) + spans.sum() -
    /// 5/3` with `spans = ones(n) * 5/3` scaled by `(1,4,4,4,4)`, evaluated
    /// under numpy's pairwise summation. The cases cover all three branches
    /// of that summation: under 8 elements, the 8-partial block, and the
    /// recursive split above 128.
    #[test]
    fn last_anchor_time_matches_numpy_pairwise_sum() {
        let cases = [
            (2usize, 3usize, 9.666666666666668f64),
            (7, 4, 39.00000000000001),
            (16, 10, 95.0),
            (37, 90, 294.99999999999994),
            (129, 7, 735.3333333333334),
            (200, 5, 1136.6666666666665),
        ];
        for (frames, text, expected) in cases {
            let ours = h3_last_anchor_time(frames, text);
            assert_eq!(
                ours.to_bits(),
                expected.to_bits(),
                "frames={frames} text={text}: {ours:?} != {expected:?}"
            );
        }
    }

    /// The "last" anchor is NOT the last generated frame's own rotary time:
    /// the frame grid stops one span short of the clip's total span, while
    /// the anchor is the total less one BASE unit. They coincide only when
    /// the last latent frame's span is the base unit — and even then the two
    /// summation orders differ in the last ulp from 16 frames on, which is
    /// why this port reproduces numpy's order rather than reusing the grid.
    #[test]
    fn last_anchor_is_not_the_last_frame_time() {
        let tags = vec![H3_TEXT_TAG; 90];
        let layout =
            h3_build_packed_layout(&tags, 37, 22, 40, 207, &[H3KeyframeAnchor::Last]).unwrap();
        let last_video_row = layout.sequence_length - 1;
        let last_frame_time = layout.position_ids[last_video_row * 3];
        let anchor_time = layout.position_ids[layout.condition_start * 3];
        assert_eq!(anchor_time, 294.99999999999994);
        assert!(
            (anchor_time - last_frame_time).abs() > 1.0,
            "anchor {anchor_time} vs last frame {last_frame_time}"
        );
    }

    /// PARITY GATE. A single `First` anchor must reproduce the pre-endframe
    /// builder bit-for-bit: every conditioning row is the first video
    /// frame's row verbatim, and every other row is the t2va layout's row
    /// verbatim. If this fails, first-only jobs have moved.
    #[test]
    fn first_only_layout_is_bit_identical_to_t2va_plus_frame_zero() {
        for (frames, lh, lw, audio, text) in [
            (37usize, 22usize, 40usize, 207usize, 90usize),
            (7, 4, 4, 10, 4),
            (12, 8, 12, 41, 137),
        ] {
            let tags = vec![H3_TEXT_TAG; text];
            let fl2va =
                h3_build_packed_layout(&tags, frames, lh, lw, audio, &[H3KeyframeAnchor::First])
                    .unwrap();
            let t2va = h3_build_t2va_layout(&tags, frames, lh, lw, audio).unwrap();
            assert_eq!(fl2va.num_condition_rows, fl2va.rows_per_frame);
            // Text rows.
            for row in 0..text {
                assert_eq!(
                    fl2va.position_ids[row * 3..row * 3 + 3],
                    t2va.position_ids[row * 3..row * 3 + 3]
                );
                assert_eq!(fl2va.token_tags[row], t2va.token_tags[row]);
            }
            // Condition rows == video frame 0's rows, bit for bit.
            for i in 0..fl2va.rows_per_frame {
                let c = (fl2va.condition_start + i) * 3;
                let v = (fl2va.video_start + i) * 3;
                assert_eq!(
                    fl2va.position_ids[c].to_bits(),
                    fl2va.position_ids[v].to_bits()
                );
                assert_eq!(fl2va.position_ids[c], text as f64);
                assert_eq!(fl2va.position_ids[c + 1], fl2va.position_ids[v + 1]);
                assert_eq!(fl2va.position_ids[c + 2], fl2va.position_ids[v + 2]);
                assert_eq!(fl2va.token_tags[fl2va.condition_start + i], H3_VIDEO_TAG);
            }
            // Audio + video rows: the same values, shifted by the block.
            let shift = fl2va.num_condition_rows;
            for row in t2va.audio_start..t2va.sequence_length {
                let moved = row + shift;
                assert_eq!(
                    fl2va.position_ids[moved * 3..moved * 3 + 3],
                    t2va.position_ids[row * 3..row * 3 + 3],
                    "row {row}"
                );
                assert_eq!(fl2va.token_tags[moved], t2va.token_tags[row]);
            }
        }
    }

    /// Last-only: one conditioning block, pinned at the last anchor, on the
    /// video frame's spatial grid.
    #[test]
    fn last_only_layout_anchors_at_the_clip_end() {
        let tags = vec![H3_TEXT_TAG; 90];
        let layout =
            h3_build_packed_layout(&tags, 37, 22, 40, 207, &[H3KeyframeAnchor::Last]).unwrap();
        assert_eq!(layout.num_condition_rows, 220);
        let anchor = h3_last_anchor_time(37, 90);
        for i in 0..220 {
            let c = (layout.condition_start + i) * 3;
            let v = (layout.video_start + i) * 3;
            assert_eq!(layout.position_ids[c], anchor);
            assert_eq!(layout.position_ids[c + 1], layout.position_ids[v + 1]);
            assert_eq!(layout.position_ids[c + 2], layout.position_ids[v + 2]);
            assert_eq!(layout.token_tags[layout.condition_start + i], H3_VIDEO_TAG);
        }
    }

    /// First+last: two blocks in packed order (first, then last), the
    /// audio/video rows pushed out by both. The two blocks share the spatial
    /// grid and differ only in rotary time — which is what makes them read
    /// as the two ENDS of the same clip.
    #[test]
    fn first_last_layout_packs_two_blocks_in_order() {
        let tags = vec![H3_TEXT_TAG; 90];
        let anchors = [H3KeyframeAnchor::First, H3KeyframeAnchor::Last];
        let layout = h3_build_packed_layout(&tags, 37, 22, 40, 207, &anchors).unwrap();
        assert_eq!(layout.num_condition_rows, 2 * 220);
        assert_eq!(layout.audio_start, 90 + 440);
        assert_eq!(layout.video_start, 90 + 440 + 414);
        assert_eq!(layout.sequence_length, 90 + 440 + 414 + 37 * 220);
        let last_anchor = h3_last_anchor_time(37, 90);
        for i in 0..220 {
            let first = (layout.condition_start + i) * 3;
            let last = (layout.condition_start + 220 + i) * 3;
            let video = (layout.video_start + i) * 3;
            assert_eq!(layout.position_ids[first], 90.0);
            assert_eq!(layout.position_ids[last], last_anchor);
            // Same spatial grid for both blocks and the video frame.
            assert_eq!(layout.position_ids[first + 1], layout.position_ids[video + 1]);
            assert_eq!(layout.position_ids[last + 1], layout.position_ids[video + 1]);
            assert_eq!(layout.position_ids[first + 2], layout.position_ids[video + 2]);
            assert_eq!(layout.position_ids[last + 2], layout.position_ids[video + 2]);
            assert_eq!(layout.token_tags[layout.condition_start + i], H3_VIDEO_TAG);
            assert_eq!(
                layout.token_tags[layout.condition_start + 220 + i],
                H3_VIDEO_TAG
            );
        }
        // Both blocks are pinned at the same conditioning timestep: the
        // denoise plan sees one extra timestep value, not two.
        let plan = h3_build_row_timesteps_cond(&layout, 0.2, 0.5, 0.999);
        assert_eq!(plan.values, vec![0.2, 0.5, 0.999]);
        assert_eq!(plan.indices[layout.condition_start], 2);
        assert_eq!(plan.indices[layout.condition_start + 220], 2);
        assert_eq!(plan.indices[layout.video_start], 0);
    }

    /// An identical image at both ends is still TWO condition blocks — the
    /// rows repeat, only the rotary time differs. (The clip is near-looping,
    /// not pixel-closed: the conditioning rows never reach the VAE decode.)
    #[test]
    fn identical_start_and_end_still_packs_two_blocks() {
        let tags = vec![H3_TEXT_TAG; 12];
        let anchors = [H3KeyframeAnchor::First, H3KeyframeAnchor::Last];
        let layout = h3_build_packed_layout(&tags, 7, 4, 4, 10, &anchors).unwrap();
        let rows = layout.rows_per_frame;
        assert_eq!(layout.num_condition_rows, 2 * rows);
        for i in 0..rows {
            let first = (layout.condition_start + i) * 3;
            let last = (layout.condition_start + rows + i) * 3;
            assert_ne!(layout.position_ids[first], layout.position_ids[last]);
            assert_eq!(layout.position_ids[first + 1], layout.position_ids[last + 1]);
            assert_eq!(layout.position_ids[first + 2], layout.position_ids[last + 2]);
        }
    }

    #[test]
    fn fl2va_row_timesteps_pin_condition() {
        let tags = vec![H3_TEXT_TAG; 4];
        let layout = h3_build_packed_layout(&tags, 7, 4, 4, 10, &[H3KeyframeAnchor::First]).unwrap();
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

    // -----------------------------------------------------------------
    // Video-VAE naming conventions.
    //
    // The full 96GB tier ships a diffusers-converted VAE; the Q4/NVFP4
    // tiers pin `minimax_h3_video_vae_fp16.safetensors`, which is the
    // ORIGINAL MiniMax/LDM checkpoint under different names with a FUSED
    // decoder QKV. `H3VaeDecoderPrepared`/`H3VaeEncoderPrepared` read one
    // canonical (diffusers) name set, so both files must present the same
    // tensors through `H3ShardedWeights`.
    // -----------------------------------------------------------------

    /// Minimal f32 safetensors writer: u64 LE header length, JSON header,
    /// payload. `tensors` is (name, shape, values) in write order.
    fn write_safetensors(path: &Path, tensors: &[(&str, Vec<u64>, Vec<f32>)]) {
        let mut payload: Vec<u8> = Vec::new();
        let mut entries: Vec<String> = Vec::new();
        for (name, shape, values) in tensors {
            let start = payload.len();
            for value in values {
                payload.extend_from_slice(&value.to_le_bytes());
            }
            let dims: Vec<String> = shape.iter().map(|dim| dim.to_string()).collect();
            entries.push(format!(
                "\"{name}\":{{\"dtype\":\"F32\",\"shape\":[{}],\"data_offsets\":[{start},{}]}}",
                dims.join(","),
                payload.len()
            ));
        }
        let header = format!("{{{}}}", entries.join(","));
        let mut out = (header.len() as u64).to_le_bytes().to_vec();
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(&payload);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, out).unwrap();
    }

    const TEST_HEADS: usize = 2;
    const TEST_COLS: usize = 8;
    const TEST_DIM: usize = TEST_HEADS * H3_VAE_HEAD_DIM_FOR_TEST;
    const H3_VAE_HEAD_DIM_FOR_TEST: usize = crate::h3_vae::H3_VAE_HEAD_DIM;

    /// Distinct per (part, row, col) so a misplaced row is unmissable.
    fn qkv_value(part: usize, row: usize, col: usize) -> f32 {
        (part * 1_000_000 + row * 100 + col) as f32
    }

    fn part_rows(part: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(TEST_DIM * TEST_COLS);
        for row in 0..TEST_DIM {
            for col in 0..TEST_COLS {
                out.push(qkv_value(part, row, col));
            }
        }
        out
    }

    /// The repack's fused layout: HEAD-MAJOR, canonical row `head * 64 +
    /// chan` sitting at fused row `head * 192 + part * 64 + chan`.
    fn fused_rows() -> Vec<f32> {
        let head_dim = H3_VAE_HEAD_DIM_FOR_TEST;
        let mut out = vec![0.0f32; 3 * TEST_DIM * TEST_COLS];
        for head in 0..TEST_HEADS {
            for part in 0..3 {
                for chan in 0..head_dim {
                    let canonical = head * head_dim + chan;
                    let fused = head * 3 * head_dim + part * head_dim + chan;
                    for col in 0..TEST_COLS {
                        out[fused * TEST_COLS + col] = qkv_value(part, canonical, col);
                    }
                }
            }
        }
        out
    }

    fn fused_bias() -> Vec<f32> {
        let head_dim = H3_VAE_HEAD_DIM_FOR_TEST;
        let mut out = vec![0.0f32; 3 * TEST_DIM];
        for head in 0..TEST_HEADS {
            for part in 0..3 {
                for chan in 0..head_dim {
                    let canonical = head * head_dim + chan;
                    out[head * 3 * head_dim + part * head_dim + chan] =
                        qkv_value(part, canonical, 0);
                }
            }
        }
        out
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("h3-vae-names-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn video_vae_repack_and_diffusers_names_load_identically() {
        let dir = scratch_dir("both");
        let block = "decoder.transformer_blocks.0";
        let dim = TEST_DIM as u64;
        let cols = TEST_COLS as u64;
        let bias = |part: usize| {
            (0..TEST_DIM).map(|row| qkv_value(part, row, 0)).collect::<Vec<f32>>()
        };
        let small = vec![1.0f32; TEST_COLS];
        // The SwiGLU projection: two halves with distinct contents, so the
        // repack's [gate | value] vs diffusers' [value | gate] ordering is
        // load-bearing here rather than invisible.
        const FF_HALF: usize = 4;
        let ff_half = |tag: usize| {
            (0..FF_HALF)
                .flat_map(|row| (0..TEST_COLS).map(move |col| (tag + row * 100 + col) as f32))
                .collect::<Vec<f32>>()
        };
        let (ff_value, ff_gate) = (ff_half(700_000), ff_half(900_000));
        let mut ff_diffusers = ff_value.clone(); // [value | gate]
        ff_diffusers.extend_from_slice(&ff_gate);
        let mut ff_repack = ff_gate.clone(); // [gate | value]
        ff_repack.extend_from_slice(&ff_value);
        let ff_rows = 2 * FF_HALF as u64;

        // The diffusers convention (the working 96GB tier).
        let diffusers = dir.join("diffusers.safetensors");
        write_safetensors(
            &diffusers,
            &[
                (&format!("{block}.attn.to_q.weight"), vec![dim, cols], part_rows(0)),
                (&format!("{block}.attn.to_k.weight"), vec![dim, cols], part_rows(1)),
                (&format!("{block}.attn.to_v.weight"), vec![dim, cols], part_rows(2)),
                (&format!("{block}.attn.to_q.bias"), vec![dim], bias(0)),
                (&format!("{block}.attn.to_k.bias"), vec![dim], bias(1)),
                (&format!("{block}.attn.to_v.bias"), vec![dim], bias(2)),
                (&format!("{block}.attn.to_out.0.weight"), vec![cols], small.clone()),
                (
                    &format!("{block}.ff.net.0.proj.weight"),
                    vec![ff_rows, cols],
                    ff_diffusers.clone(),
                ),
                (&format!("{block}.ff.net.2.weight"), vec![cols], small.clone()),
                (&format!("{block}.norm1.weight"), vec![cols], small.clone()),
                ("decoder.proj_in.weight", vec![cols], small.clone()),
                ("decoder.register_tokens", vec![cols], small.clone()),
                (
                    "encoder.down_blocks.1.resnets.0.conv_shortcut.weight",
                    vec![cols],
                    small.clone(),
                ),
                (
                    "encoder.down_blocks.2.downsamplers.0.conv.bias",
                    vec![cols],
                    small.clone(),
                ),
                ("quant_conv.weight", vec![cols], small.clone()),
            ],
        );

        // The unsloth repack convention (the Q4/NVFP4 tiers).
        let repack = dir.join("repack.safetensors");
        write_safetensors(
            &repack,
            &[
                (
                    &format!("{block}.attn.to_qkv.weight"),
                    vec![3 * dim, cols],
                    fused_rows(),
                ),
                (&format!("{block}.attn.to_qkv.bias"), vec![3 * dim], fused_bias()),
                (&format!("{block}.attn.to_out.weight"), vec![cols], small.clone()),
                (&format!("{block}.ff.w1.weight"), vec![ff_rows, cols], ff_repack.clone()),
                (&format!("{block}.ff.w2.weight"), vec![cols], small.clone()),
                (&format!("{block}.norm1.weight"), vec![cols], small.clone()),
                ("decoder.x_embedder.weight", vec![cols], small.clone()),
                ("decoder.register_tokens", vec![cols], small.clone()),
                ("encoder.down.1.block.0.nin_shortcut.weight", vec![cols], small.clone()),
                ("encoder.down.2.downsample.conv.bias", vec![cols], small.clone()),
                ("quant_conv.weight", vec![cols], small.clone()),
            ],
        );

        let full = H3ShardedWeights::load(&diffusers).unwrap();
        let q4 = H3ShardedWeights::load(&repack).unwrap();

        // Same canonical inventory, tensor for tensor.
        let mut full_names = full.tensor_names();
        let mut q4_names = q4.tensor_names();
        full_names.sort();
        q4_names.sort();
        assert_eq!(full_names, q4_names, "the two conventions expose the same names");

        // ...and byte-identical contents through it, INCLUDING the three
        // that only exist fused in the repack.
        for name in &full_names {
            assert_eq!(
                full.tensor_dtype_shape(name).unwrap(),
                q4.tensor_dtype_shape(name).unwrap(),
                "shape of {name}"
            );
            assert_eq!(
                full.tensor_f32(name).unwrap(),
                q4.tensor_f32(name).unwrap(),
                "values of {name}"
            );
        }

        // The fused split is HEAD-MAJOR, not three contiguous blocks. Under
        // a contiguous cut, to_k row 0 would be fused row `dim` (128) and
        // to_v row 0 fused row `2 * dim` (256); the true layout puts them at
        // 64 and 128. Assert the true placement AND that the contiguous
        // reading is a different tensor, so a regression to the plain
        // 3-way cut cannot pass.
        let head_dim = H3_VAE_HEAD_DIM_FOR_TEST;
        let fused = fused_rows();
        let row_of = |row: usize| fused[row * TEST_COLS..(row + 1) * TEST_COLS].to_vec();
        let to_k = q4.tensor_f32(&format!("{block}.attn.to_k.weight")).unwrap();
        let to_v = q4.tensor_f32(&format!("{block}.attn.to_v.weight")).unwrap();
        assert_eq!(to_k[..TEST_COLS].to_vec(), row_of(head_dim));
        assert_eq!(to_v[..TEST_COLS].to_vec(), row_of(2 * head_dim));
        assert_ne!(to_k[..TEST_COLS].to_vec(), row_of(TEST_DIM));
        assert_ne!(to_v[..TEST_COLS].to_vec(), row_of(2 * TEST_DIM));
        // Head 1 too — one head would pass either way.
        assert_eq!(
            to_k[head_dim * TEST_COLS..(head_dim + 1) * TEST_COLS].to_vec(),
            row_of(3 * head_dim + head_dim)
        );

        // The SwiGLU halves come back EXCHANGED, so both files present the
        // diffusers [value | gate] order that gpu_swiglu_value_gate wants.
        // Reading the repack's rows straight through would silently feed the
        // decoder gate-as-value — a bug no shape or name check can see.
        let ff = q4.tensor_f32(&format!("{block}.ff.net.0.proj.weight")).unwrap();
        assert_eq!(ff, ff_diffusers, "w1's halves must be swapped into place");
        assert_ne!(ff, ff_repack, "the file's own order must NOT reach the decoder");
        assert_eq!(&ff[..TEST_COLS], &ff_value[..TEST_COLS]);
        assert_eq!(
            &ff[FF_HALF * TEST_COLS..(FF_HALF + 1) * TEST_COLS],
            &ff_gate[..TEST_COLS]
        );
        assert_eq!(
            q4.tensor_dtype_shape(&format!("{block}.ff.net.0.proj.weight")).unwrap().1,
            vec![ff_rows, cols],
            "a half swap keeps the tensor's full size"
        );

        // A part's shape/disk size is the fused tensor divided by 3.
        let (dtype, shape) = q4.tensor_dtype_shape(&format!("{block}.attn.to_q.weight")).unwrap();
        assert_eq!(dtype, MlxDType::F32);
        assert_eq!(shape, vec![dim, cols]);
        assert_eq!(
            q4.tensor_disk_bytes(&format!("{block}.attn.to_q.weight")).unwrap(),
            dim * cols * 4
        );
        // Rank-1 fused bias splits on the same rule.
        assert_eq!(
            q4.tensor_dtype_shape(&format!("{block}.attn.to_q.bias")).unwrap().1,
            vec![dim]
        );
        // Single-row reads follow the selection too.
        assert_eq!(
            q4.tensor_row_f32(&format!("{block}.attn.to_v.weight"), 1).unwrap(),
            full.tensor_row_f32(&format!("{block}.attn.to_v.weight"), 1).unwrap()
        );

        // Names shared by both conventions are never rewritten.
        assert!(q4.has_tensor("quant_conv.weight"));
        assert!(q4.has_tensor("decoder.register_tokens"));
        assert!(q4.has_tensor(&format!("{block}.norm1.weight")));
        // ...and the repack's own spellings are gone, replaced by canonical.
        assert!(!q4.has_tensor("decoder.x_embedder.weight"));
        assert!(!q4.has_tensor(&format!("{block}.attn.to_qkv.weight")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn colliding_video_vae_aliases_fail_closed() {
        let dir = scratch_dir("collide");
        let block = "decoder.transformer_blocks.0";
        let small = vec![1.0f32; 4];
        let path = dir.join("mixed.safetensors");
        // Both spellings of to_out in one file: one would silently win.
        write_safetensors(
            &path,
            &[
                (&format!("{block}.attn.to_out.weight"), vec![4], small.clone()),
                (&format!("{block}.attn.to_out.0.weight"), vec![4], small.clone()),
            ],
        );
        let err = match H3ShardedWeights::load(&path) {
            Ok(_) => panic!("both spellings of to_out in one file must fail closed"),
            Err(err) => err.to_string(),
        };
        assert!(
            err.contains("already supplied"),
            "expected an alias collision error, got {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
