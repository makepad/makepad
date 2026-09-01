//! End-to-end MiniMax H3 t2va generation on the device stack: TE encode ->
//! evict -> DiT rectified-flow denoise (video + audio rows step with their
//! own schedules) -> unpatchify -> video VAE decode -> u8 RGB frames, plus
//! audio VAE decode (CPU f32) -> stereo 32 kHz samples.
//!
//! Noise: pass reference noise rows (from an oracle dump — forward-0 inputs
//! ARE the seed's pure noise since t=0 is the noise end) for torch-seed
//! composition parity, or let the built-in xorshift/Box-Muller generator
//! draw (deterministic per seed, NOT torch-parity — quality class only).

use crate::error::{DiffusionError, Result};
use crate::h3::{
    h3_align_num_frames, h3_audio_latent_num_frames, h3_build_packed_layout,
    h3_build_row_timesteps_cond, h3_euler_step, h3_patchify_video_latents, h3_rope_tables,
    h3_scale_noise, h3_schedule, h3_unpatchify_video_latents, h3_video_latent_num_frames,
    H3KeyframeAnchor, H3ShardedWeights, H3_AUDIO_IN_CHANNELS, H3_AUDIO_SHIFT,
    H3_KEYFRAME_NOISE_AUG, H3_TEXT_TAG, H3_VIDEO_PATCH_DIM, H3_VIDEO_SHIFT, H3_VIDEO_TAG,
};
use crate::h3_audio_vae::H3AudioVae;
use crate::h3_text::{
    h3_text_encode_progress, h3_text_encoder_evict, H3TextEncoderPrepared, H3VisionImage,
    H3VisionSpan,
};
use crate::h3_transformer::{h3_dit_forward, H3DitPrepared};
use crate::h3_vae::{
    h3_vae_decode_ctrl, h3_vae_denormalize_latents, h3_vae_frames_to_u8, H3VaeCtrl,
    H3VaeDecoderPrepared, H3_VAE_PATCH,
};
use std::path::{Path, PathBuf};

/// Qwen3-VL special token ids (tokenizer_config.json of the H3 checkpoint).
pub const H3_TOKEN_VISION_START: u32 = 151652;
pub const H3_TOKEN_VISION_END: u32 = 151653;
pub const H3_TOKEN_IMAGE_PAD: u32 = 151655;
/// Seed the keyframe posterior is sampled under, independently of the
/// request seed (reference `keyframe_encode_seed` — same keyframe, same
/// anchor; our RNG is not torch-parity, but the fixed-seed property holds).
pub const H3_KEYFRAME_ENCODE_SEED: u64 = 42;

/// Mid-run control for service callers: structured phase progress plus
/// cooperative cancellation, polled at every natural boundary (between
/// denoise steps, between VAE batch groups, between pipeline phases). A
/// single DiT forward is the granularity floor — no kernel interruption.
///
/// Phases emitted through `on_phase(name, done, total)` (total == 0 means an
/// uncounted phase-start marker):
/// - "te-load" 0/0        (text-encoder weight load starting)
/// - "text-encode" 0/0, then k/50 per TE decoder layer — every layer
///                        re-streams its weights from disk (the namespace is
///                        evicted per prompt), so the 30s+ phase moves
/// - "text-cached" 0/0    (warm conditioning reused — the whole TE phase is
///                         skipped, see [`H3CondCache`])
/// - "keyframe-encode" 0/0 (fl2va keyframe VAE encode starting)
/// - "keyframe-cached" 0/0 (fl2va keyframe latents reused from the cache)
/// - "dit-load" 0/0       (DiT weight load/prepare starting; the bulk of the
///                         stream-in happens lazily inside denoise step 1)
/// - "denoise" k/N        at the START of step k (1-based)
/// - "denoise-done" k/N   after step k's euler update
/// - "vae-load" 0/0
/// - "vae" g/G            at the start of each batched ViT decode group
/// - "vae-done" 0/0
/// - "audio-decode" 0/0, then k/(2*stages) per decoder stage per channel
#[derive(Default)]
pub struct H3RunControl<'a> {
    pub on_phase: Option<&'a mut dyn FnMut(&str, usize, usize)>,
    /// Return true to abort: the run unwinds with
    /// [`DiffusionError::Cancelled`] at the next boundary.
    pub cancel: Option<&'a (dyn Fn() -> bool + 'a)>,
    /// Warm text-conditioning cache, owned by the service caller and kept
    /// across jobs. `None` (the default) = every run encodes from scratch,
    /// exactly the old behavior. See [`H3CondCache`].
    pub cond_cache: Option<&'a mut H3CondCache>,
}

impl H3RunControl<'_> {
    fn phase(&mut self, name: &str, done: usize, total: usize) {
        if let Some(on_phase) = self.on_phase.as_deref_mut() {
            on_phase(name, done, total);
        }
    }

    fn check(&self) -> Result<()> {
        if self.cancel.map_or(false, |cancelled| cancelled()) {
            Err(DiffusionError::Cancelled)
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Warm conditioning cache (the H3 half of the flux warm-residency pattern)
// ---------------------------------------------------------------------------

/// Warm text-conditioning cache for repeat jobs. H3 holds no whole-model
/// host arenas (weights stream per tensor), and the DiT/VAE device weight
/// caches already persist across jobs on the worker thread — the one
/// heavyweight per-job rebuild is the TEXT ENCODER: its full namespace
/// (~66.7GB) re-streams from disk for every encode and is evicted right
/// after (TE + DiT can never be co-resident). Measured on the 96GB box:
/// ~42s of a 46s repeat job was the TE phase for embeds that are
/// byte-identical to the previous job's.
///
/// This cache keeps the ENCODED conditioning (host `Vec<f32>`, a few MB):
/// - text embeds, keyed by models dir + the full token presentation +
///   canvas size + the keyframe canvas pixels (fl2va embeds see the image);
/// - fl2va keyframe condition LATENTS (pre-noise, seed-independent — the
///   posterior sample uses the fixed [`H3_KEYFRAME_ENCODE_SEED`]), keyed by
///   models dir + canvas pixels + size, so a prompt change with the same
///   keyframe skips the VAE encode too. The per-job `cond_noise` (drawn
///   from the REQUEST seed) is applied after lookup, preserving fixed-seed
///   byte parity.
///
/// A hit skips TE load + encode + evict entirely and preserves the warm
/// DiT/VAE. On a miss the pipeline retires those compute namespaces before
/// loading the TE, then re-streams them for denoise/decode. Entries only
/// mutate after a fully successful encode; the whole struct is host-side and
/// `Send`, so the backend owns it directly across jobs.
#[derive(Default)]
pub struct H3CondCache {
    embeds: Option<(H3CondKey, Vec<f32>)>,
    /// Most-recent-first, one entry per distinct keyframe canvas. A
    /// first+last request encodes two anchors, and swapping only the prompt
    /// must still hit both.
    keyframe_latents: Vec<(H3KeyframeKey, Vec<f32>)>,
}

/// How many keyframe latent blocks the cache keeps (2 keyframes x a
/// previous request's pair).
const H3_KEYFRAME_CACHE_SLOTS: usize = 4;

/// Identity of one text-conditioning result. Exact comparison, no hashing:
/// the token ids are short and the canvas comparison is a cheap memcmp.
#[derive(PartialEq)]
struct H3CondKey {
    models_dir: PathBuf,
    token_ids: Vec<u32>,
    width: usize,
    height: usize,
    /// fl2va: the prepared keyframe canvases the vision tower saw, in packed
    /// order (empty = t2v).
    canvases: Vec<Vec<u8>>,
}

/// Identity of one keyframe VAE-encode result (prompt-independent).
#[derive(PartialEq)]
struct H3KeyframeKey {
    models_dir: PathBuf,
    width: usize,
    height: usize,
    canvas: Vec<u8>,
}

impl H3CondCache {
    fn embeds_for(&self, key: &H3CondKey) -> Option<&[f32]> {
        self.embeds
            .as_ref()
            .filter(|(cached, _)| cached == key)
            .map(|(_, embeds)| embeds.as_slice())
    }

    fn store_embeds(&mut self, key: H3CondKey, embeds: Vec<f32>) {
        self.embeds = Some((key, embeds));
    }

    fn latents_for(&self, key: &H3KeyframeKey) -> Option<&[f32]> {
        self.keyframe_latents
            .iter()
            .find(|(cached, _)| cached == key)
            .map(|(_, latents)| latents.as_slice())
    }

    fn store_latents(&mut self, key: H3KeyframeKey, latents: Vec<f32>) {
        self.keyframe_latents.retain(|(cached, _)| *cached != key);
        self.keyframe_latents.insert(0, (key, latents));
        self.keyframe_latents.truncate(H3_KEYFRAME_CACHE_SLOTS);
    }
}

/// One fl2va keyframe: a decoded RGB image (any size — the first one is
/// LANCZOS-stretched onto the canvas, the followers are cover-cropped onto
/// it) plus the tokenized `"<Picture i>: "` label that precedes its vision
/// block in the TE presentation, and which end of the clip it anchors.
pub struct H3KeyframeInput {
    /// Tightly packed RGB, `height * width * 3`.
    pub rgb: Vec<u8>,
    pub width: usize,
    pub height: usize,
    /// Which end of the generated clip this image conditions.
    pub anchor: H3KeyframeAnchor,
    /// Tokenizer ids of `"<Picture i>: "` (no special tokens), `i` being the
    /// 1-based position in the packed keyframe order.
    pub picture_label_ids: Vec<u32>,
}

/// Weight container format of an explicitly sourced H3 component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum H3WeightFormat {
    /// GGUF (unsloth/leejet Q4_K family, full or AdaLN-curve pruned).
    Gguf,
    /// ComfyUI/TensorRT-ModelOpt NVFP4 safetensors (Blackwell tier).
    Nvfp4,
    /// A bf16 diffusers shard dir (or single safetensors file) in the
    /// canonical tensor spelling — a DiT swapped under the canonical tree,
    /// such as the FastH3 distilled transformer. Loads through the same
    /// streamed shard reader as the tree itself.
    Bf16Shards,
}

#[derive(Clone, Debug)]
pub struct H3ComponentFile {
    pub path: PathBuf,
    pub format: H3WeightFormat,
    /// Device weight-cache namespace for a [`H3WeightFormat::Bf16Shards`]
    /// DiT (`h3dit::<tag>`). Required for that format: the canonical tree
    /// caches under plain `h3dit`, and a second bf16 DiT on the same keys
    /// would be served the other model's tensors. Ignored by the quantized
    /// formats, which carry their own namespaces.
    pub dit_namespace: Option<String>,
}

/// Explicit per-component weight sources for the quantized tiers. `None`
/// fields fall back to the canonical bf16 diffusers tree under `models_dir`
/// (`text_encoder/`, `transformer/`, `vae/`, `audio_vae/`).
#[derive(Clone, Debug, Default)]
pub struct H3ModelSet {
    pub dit: Option<H3ComponentFile>,
    pub text_encoder: Option<H3ComponentFile>,
    /// Single-file fp16 video-VAE repack (or an alternate shard dir).
    pub video_vae_path: Option<PathBuf>,
    /// Dir holding the audio VAE `config.json` + weights.
    pub audio_vae_dir: Option<PathBuf>,
}

pub struct H3GenerateParams {
    pub width: usize,
    pub height: usize,
    /// Requested frame count; snapped up to the VAE's 17n+5 alignment.
    pub num_frames: usize,
    pub num_inference_steps: usize,
    /// Raw tokenizer ids of the PROMPT (no chat template, no BOS). For fl2va
    /// the pipeline prepends the keyframe's label + vision block itself.
    pub token_ids: Vec<u32>,
    pub seed: u64,
    /// Keyframes in PACKED ORDER: any non-empty list switches the run to the
    /// fl2va workflow — vision-conditioned TE + VAE-encoded never-denoised
    /// leading video rows, one condition block per keyframe. Upstream packs
    /// `image` (anchor `First`) before `last_image` (anchor `Last`); the
    /// first entry is also the geometry anchor the canvas is derived from
    /// and the only one that is stretched rather than cover-cropped.
    pub keyframes: Vec<H3KeyframeInput>,
    /// Optional reference initial noise in packed row layout
    /// (num_video_rows x 96). Overrides the seeded generator.
    pub video_noise_rows: Option<Vec<f32>>,
    /// Optional reference audio noise rows (num_audio_rows x 32).
    pub audio_noise_rows: Option<Vec<f32>>,
    /// Optional reference NOISED condition rows (num_condition_rows x 96,
    /// fl2va): bypasses VAE-encode + noising for oracle-parity runs.
    pub condition_rows_override: Option<Vec<f32>>,
    pub act16: bool,
    /// Decode the denoised audio latents through the audio VAE (CPU f32).
    /// Off = silent clip (`audio_planar: None`).
    pub decode_audio: bool,
    /// Per-component weight-source overrides (quantized tiers). `None` =
    /// the canonical bf16 diffusers tree under `models_dir`.
    pub model_set: Option<H3ModelSet>,
    /// Release each component's device weight namespace as soon as its
    /// phase completes (24/32 GB tiers: TE, DiT and VAE never co-reside).
    /// The bf16/96GB tier keeps DiT+VAE warm across jobs instead.
    pub staged_residency: bool,
}

#[derive(Default)]
pub struct H3GenerateTimings {
    pub te_load_s: f64,
    pub te_encode_s: f64,
    /// fl2va: keyframe VAE spatial encode + condition-row prep.
    pub keyframe_encode_s: f64,
    pub dit_load_s: f64,
    /// Wall seconds per DiT forward (index 0 includes the weight stream-in).
    pub forwards_s: Vec<f64>,
    pub vae_load_s: f64,
    pub vae_decode_s: f64,
    pub audio_decode_s: f64,
    pub total_s: f64,
}

impl H3GenerateTimings {
    /// Mean of the warm forwards (all but the first).
    pub fn warm_forward_s(&self) -> Option<f64> {
        if self.forwards_s.len() < 2 {
            return None;
        }
        let warm = &self.forwards_s[1..];
        Some(warm.iter().sum::<f64>() / warm.len() as f64)
    }
}

pub struct H3GenerateOutput {
    /// (frames, height, width, 3) u8 RGB.
    pub frames_rgb8: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub num_frames: usize,
    /// Stereo planar f32 `[L..., R...]` at `audio_sample_rate`, when
    /// `decode_audio` was requested.
    pub audio_planar: Option<Vec<f32>>,
    pub audio_sample_rate: u32,
    pub timings: H3GenerateTimings,
}

/// Deterministic standard-normal noise (xorshift64* + Box-Muller). NOT
/// torch-randn compatible — same quality class, different composition.
pub struct H3NoiseRng {
    state: u64,
    spare: Option<f32>,
}

impl H3NoiseRng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_mul(0x9e3779b97f4a7c15).max(1) | 1,
            spare: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545f4914f6cdd1d)
    }

    fn next_uniform(&mut self) -> f32 {
        // (0, 1]: never 0 so ln() stays finite.
        (((self.next_u64() >> 40) as f32) + 1.0) / (1u32 << 24) as f32
    }

    pub fn next_normal(&mut self) -> f32 {
        if let Some(value) = self.spare.take() {
            return value;
        }
        let u1 = self.next_uniform();
        let u2 = self.next_uniform();
        let radius = (-2.0 * u1.ln()).sqrt();
        let angle = 2.0 * std::f32::consts::PI * u2;
        self.spare = Some(radius * angle.sin());
        radius * angle.cos()
    }

    pub fn fill_normal(&mut self, len: usize) -> Vec<f32> {
        (0..len).map(|_| self.next_normal()).collect()
    }
}

/// fl2va keyframe -> canvas: PIL-exact LANCZOS stretch (byte-identical to
/// the reference's `Image.resize(LANCZOS)`).
fn fl2va_resize_canvas(
    rgb: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Result<Vec<u8>> {
    Ok(crate::h3_image::resize_rgb_lanczos3(
        rgb, src_w, src_h, dst_w, dst_h,
    ))
}

/// fl2va follower keyframe -> canvas: COVER-CROP, not a stretch.
///
/// Reference: `before_encoder.py::MiniMaxH3FL2VASetupStep.__call__`
/// (diffusers `modular_pipelines/minimax_h3`, lines 135-148). Only
/// `keyframes[0]` — the geometry anchor the canvas was derived from — is
/// stretched; every later keyframe is scaled to cover, LANCZOS-resized and
/// centre-cropped, with the released model's own arithmetic:
/// `scale = max(W/src_w, H/src_h)`, `size = (max(W, round(src_w*scale)),
/// max(H, round(src_h*scale)))`, `left = (size_w - W) // 2`. `round` is
/// Python's half-to-even, hence `round_ties_even` here; the reference file
/// flags that diffusers' own `resize_mode="crop"` (floor division, a
/// different centring) disagrees by a pixel on about half of the aspect
/// ratios, which would move the conditioning latents.
fn fl2va_cover_crop_canvas(
    rgb: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Result<Vec<u8>> {
    let scale = (dst_w as f64 / src_w as f64).max(dst_h as f64 / src_h as f64);
    let resized_w = dst_w.max((src_w as f64 * scale).round_ties_even() as usize);
    let resized_h = dst_h.max((src_h as f64 * scale).round_ties_even() as usize);
    let left = resized_w.saturating_sub(dst_w) / 2;
    let top = resized_h.saturating_sub(dst_h) / 2;
    let resized = if resized_w == src_w && resized_h == src_h {
        rgb.to_vec()
    } else {
        crate::h3_image::resize_rgb_lanczos3(rgb, src_w, src_h, resized_w, resized_h)
    };
    let mut out = vec![0u8; dst_w * dst_h * 3];
    for y in 0..dst_h {
        let src = ((top + y) * resized_w + left) * 3;
        out[y * dst_w * 3..(y + 1) * dst_w * 3]
            .copy_from_slice(&resized[src..src + dst_w * 3]);
    }
    Ok(out)
}

/// Put one keyframe onto the canvas: `index == 0` is the geometry anchor and
/// is stretched, the followers are cover-cropped
/// (`before_encoder.py` lines 135-148).
fn fl2va_prepare_canvas(
    keyframe: &H3KeyframeInput,
    index: usize,
    width: usize,
    height: usize,
) -> Result<Vec<u8>> {
    if keyframe.rgb.len() != keyframe.width * keyframe.height * 3 {
        return Err(DiffusionError::workflow(format!(
            "h3 keyframe rgb: expected {} bytes, got {}",
            keyframe.width * keyframe.height * 3,
            keyframe.rgb.len()
        )));
    }
    if keyframe.width == 0 || keyframe.height == 0 {
        return Err(DiffusionError::workflow("h3 keyframe: empty image"));
    }
    if keyframe.width == width && keyframe.height == height {
        return Ok(keyframe.rgb.clone());
    }
    if index == 0 {
        fl2va_resize_canvas(&keyframe.rgb, keyframe.width, keyframe.height, width, height)
    } else {
        fl2va_cover_crop_canvas(&keyframe.rgb, keyframe.width, keyframe.height, width, height)
    }
}

/// fl2va TE: vision-conditioned encode (Qwen3-VL vision tower + interleaved
/// mrope + deepstack) over every keyframe's vision block, in packed order.
#[allow(clippy::too_many_arguments)]
fn fl2va_text_encode(
    weights: &H3ShardedWeights,
    prepared: &crate::h3_text::H3TextEncoderPrepared,
    token_ids: &[u32],
    vision_pad_starts: &[usize],
    num_vision_tokens: usize,
    canvases: &[Vec<u8>],
    width: usize,
    height: usize,
    on_layer: Option<&mut dyn FnMut(usize, usize)>,
) -> Result<Vec<f32>> {
    let mut pixels = Vec::with_capacity(canvases.len());
    for canvas in canvases {
        let (pixel_values, gh, gw) =
            crate::h3_text::h3_vision_preprocess(canvas, width, height)?;
        if gh * gw / 4 != num_vision_tokens {
            return Err(DiffusionError::workflow(format!(
                "h3 fl2va: vision grid {gh}x{gw} yields {} tokens, presentation reserved {num_vision_tokens}",
                gh * gw / 4
            )));
        }
        pixels.push((pixel_values, gh, gw));
    }
    let images: Vec<H3VisionImage> = vision_pad_starts
        .iter()
        .zip(pixels.iter())
        .map(|(start, (pixel_values, gh, gw))| H3VisionImage {
            span: H3VisionSpan {
                start_row: *start,
                len: num_vision_tokens,
                gh: *gh,
                gw: *gw,
            },
            pixel_values: pixel_values.as_slice(),
        })
        .collect();
    crate::h3_text::h3_text_encode_fl2va(weights, prepared, token_ids, &images, on_layer)
}

/// fl2va keyframe VAE encode: canvas pixels -> tiled spatial encoder ->
/// posterior sample (fixed seed [`H3_KEYFRAME_ENCODE_SEED`], own RNG — the
/// same keyframe always encodes to the same anchor) -> f16 round ->
/// normalized condition latents (24, lh, lw).
/// Resolve the DiT weight source: quantized tier file or the bf16 tree.
fn load_dit_weights(
    models_dir: &Path,
    model_set: &Option<H3ModelSet>,
) -> Result<H3ShardedWeights> {
    match model_set.as_ref().and_then(|set| set.dit.as_ref()) {
        Some(file) => match file.format {
            H3WeightFormat::Gguf => H3ShardedWeights::load_gguf_dit(&file.path),
            H3WeightFormat::Nvfp4 => H3ShardedWeights::load_nvfp4_dit(&file.path),
            H3WeightFormat::Bf16Shards => {
                let namespace = file.dit_namespace.as_deref().ok_or_else(|| {
                    DiffusionError::workflow(format!(
                        "h3 bf16 shard dit {} needs its own dit_namespace",
                        file.path.display()
                    ))
                })?;
                H3ShardedWeights::load(&file.path)?.with_dit_namespace(namespace)
            }
        },
        None => H3ShardedWeights::load(models_dir.join("transformer")),
    }
}

/// Resolve the text-encoder weight source.
fn load_te_weights(
    models_dir: &Path,
    model_set: &Option<H3ModelSet>,
) -> Result<H3ShardedWeights> {
    match model_set.as_ref().and_then(|set| set.text_encoder.as_ref()) {
        Some(file) => match file.format {
            H3WeightFormat::Gguf => H3ShardedWeights::load_gguf_te(&file.path),
            H3WeightFormat::Nvfp4 => H3ShardedWeights::load_nvfp4_te(&file.path),
            H3WeightFormat::Bf16Shards => H3ShardedWeights::load(&file.path),
        },
        None => H3ShardedWeights::load(models_dir.join("text_encoder")),
    }
}

/// Video VAE location: the fp16 single-file repack or the bf16 shard dir.
fn video_vae_source(models_dir: &Path, model_set: &Option<H3ModelSet>) -> PathBuf {
    model_set
        .as_ref()
        .and_then(|set| set.video_vae_path.clone())
        .unwrap_or_else(|| models_dir.join("vae"))
}

fn audio_vae_source(models_dir: &Path, model_set: &Option<H3ModelSet>) -> PathBuf {
    model_set
        .as_ref()
        .and_then(|set| set.audio_vae_dir.clone())
        .unwrap_or_else(|| models_dir.join("audio_vae"))
}

/// Encode a batch of keyframe canvases under ONE VAE weight load. Each
/// canvas gets its own posterior generator freshly seeded with
/// [`H3_KEYFRAME_ENCODE_SEED`], exactly as upstream calls
/// `encode_vae_condition(..., components.keyframe_encode_seed)` once per
/// keyframe (`encoders.py::MiniMaxH3KeyframeVaeEncoderStep`), so a canvas
/// always encodes to the same anchor whatever position it holds.
fn fl2va_condition_latents_batch(
    vae_path: &Path,
    canvases: &[&[u8]],
    width: usize,
    height: usize,
    latent_h: usize,
    latent_w: usize,
) -> Result<Vec<Vec<f32>>> {
    let vae_weights = H3ShardedWeights::load(vae_path)?;
    let prepared = crate::h3_vae::H3VaeEncoderPrepared::prepare(&vae_weights)?;
    let mut out = Vec::with_capacity(canvases.len());
    for canvas in canvases {
        let moments =
            crate::h3_vae::h3_vae_encode_keyframe_moments(&prepared, canvas, width, height)?;
        let mut eps_rng = H3NoiseRng::new(H3_KEYFRAME_ENCODE_SEED);
        let eps = eps_rng.fill_normal(24 * latent_h * latent_w);
        out.push(crate::h3_vae::h3_vae_condition_latents(
            &moments, latent_h, latent_w, &eps,
        )?);
    }
    Ok(out)
}

pub fn h3_generate(
    models_dir: &Path,
    params: &H3GenerateParams,
    progress: impl FnMut(&str),
) -> Result<H3GenerateOutput> {
    h3_generate_with_control(models_dir, params, progress, &mut H3RunControl::default())
}

/// [`h3_generate`] plus an [`H3RunControl`]: structured per-phase/per-step
/// progress and cooperative cancellation for service callers. The `progress`
/// line channel is unchanged (human-readable timing lines).
pub fn h3_generate_with_control(
    models_dir: &Path,
    params: &H3GenerateParams,
    mut progress: impl FnMut(&str),
    ctrl: &mut H3RunControl,
) -> Result<H3GenerateOutput> {
    let total_start = std::time::Instant::now();
    let mut timings = H3GenerateTimings::default();

    // --- geometry ----------------------------------------------------------
    if params.width % H3_VAE_PATCH != 0 || params.height % H3_VAE_PATCH != 0 {
        return Err(DiffusionError::workflow(format!(
            "h3 canvas must be multiples of {H3_VAE_PATCH}, got {}x{}",
            params.width, params.height
        )));
    }
    if !params.keyframes.is_empty() && (params.width % 32 != 0 || params.height % 32 != 0) {
        return Err(DiffusionError::workflow(format!(
            "h3 fl2va canvas must be multiples of 32 (vision patch 16 x merge 2), got {}x{}",
            params.width, params.height
        )));
    }
    let num_frames = h3_align_num_frames(params.num_frames);
    let latent_w = params.width / H3_VAE_PATCH;
    let latent_h = params.height / H3_VAE_PATCH;
    let num_latent_frames = h3_video_latent_num_frames(num_frames)?;
    let num_audio_latents = h3_audio_latent_num_frames(num_frames);

    // --- fl2va keyframes: canvas prep + TE presentation assembly ----------
    // Presentation, per keyframe in packed order: a `"<Picture i>: "` label
    // (text) + a vision block <|vision_start|> + <|image_pad|>*N +
    // <|vision_end|> (all tagged VIDEO), then the prompt (text).
    // N = (h/16)*(w/16)/4 merged vision tokens. Mirrors
    // `encoders.py::MiniMaxH3FL2VATextEncoderStep.__call__` lines 278-295.
    let mut keyframe_canvases: Vec<Vec<u8>> = Vec::with_capacity(params.keyframes.len());
    for (index, keyframe) in params.keyframes.iter().enumerate() {
        keyframe_canvases.push(fl2va_prepare_canvas(
            keyframe,
            index,
            params.width,
            params.height,
        )?);
    }
    let vision_grid_h = params.height / 16;
    let vision_grid_w = params.width / 16;
    let num_vision_tokens = vision_grid_h * vision_grid_w / 4;
    let (full_token_ids, text_tags, vision_pad_starts) = if params.keyframes.is_empty() {
        (
            params.token_ids.clone(),
            vec![H3_TEXT_TAG; params.token_ids.len()],
            Vec::new(),
        )
    } else {
        let mut ids: Vec<u32> = Vec::new();
        let mut tags: Vec<u8> = Vec::new();
        let mut pad_starts = Vec::with_capacity(params.keyframes.len());
        for keyframe in &params.keyframes {
            ids.extend_from_slice(&keyframe.picture_label_ids);
            tags.extend(std::iter::repeat(H3_TEXT_TAG).take(keyframe.picture_label_ids.len()));
            ids.push(H3_TOKEN_VISION_START);
            pad_starts.push(ids.len());
            ids.extend(std::iter::repeat(H3_TOKEN_IMAGE_PAD).take(num_vision_tokens));
            ids.push(H3_TOKEN_VISION_END);
            tags.extend(std::iter::repeat(H3_VIDEO_TAG).take(num_vision_tokens + 2));
        }
        ids.extend_from_slice(&params.token_ids);
        tags.extend(std::iter::repeat(H3_TEXT_TAG).take(params.token_ids.len()));
        (ids, tags, pad_starts)
    };

    let keyframe_anchors: Vec<H3KeyframeAnchor> =
        params.keyframes.iter().map(|kf| kf.anchor).collect();
    let layout = h3_build_packed_layout(
        &text_tags,
        num_latent_frames,
        latent_h,
        latent_w,
        num_audio_latents,
        &keyframe_anchors,
    )?;
    let rope = h3_rope_tables(&layout.position_ids);
    progress(&format!(
        "layout: seq={} text={} cond_rows={} audio_rows={} video_rows={} ({}x{}x{} latent {}x{}x{})",
        layout.sequence_length,
        layout.num_text_tokens,
        layout.num_condition_rows,
        layout.num_audio_rows,
        layout.num_video_rows,
        params.width,
        params.height,
        num_frames,
        latent_w,
        latent_h,
        num_latent_frames,
    ));

    // --- schedules ----------------------------------------------------------
    let video_sched = h3_schedule(params.num_inference_steps, H3_VIDEO_SHIFT)?;
    let audio_sched = h3_schedule(params.num_inference_steps, H3_AUDIO_SHIFT)?;
    if video_sched.timesteps.len() != audio_sched.timesteps.len() {
        return Err(DiffusionError::workflow(format!(
            "h3 schedule length mismatch after dedup: video {} audio {} — \
             pick a step count where both grids keep the same length",
            video_sched.timesteps.len(),
            audio_sched.timesteps.len()
        )));
    }
    let num_forwards = video_sched.timesteps.len();
    progress(&format!("schedule: {num_forwards} forwards"));

    // --- initial noise ------------------------------------------------------
    // Draw order mirrors the reference's single-generator discipline: the
    // conditioning noise FIRST (one draw per keyframe, latent-tensor shape),
    // then the video noise, then the audio noise.
    // One draw per keyframe, in packed order, mirroring
    // `before_denoise.py::MiniMaxH3PackConditionRowsStep.__call__`
    // ("One draw per condition, in packed order", lines 946-953). With one
    // keyframe this is byte-for-byte the single draw it always was.
    let mut rng = H3NoiseRng::new(params.seed);
    let cond_noise: Vec<Vec<f32>> = if params.condition_rows_override.is_none() {
        params
            .keyframes
            .iter()
            .map(|_| rng.fill_normal(24 * latent_h * latent_w))
            .collect()
    } else {
        Vec::new()
    };
    let mut video_rows = match &params.video_noise_rows {
        Some(rows) => {
            if rows.len() != layout.num_video_rows * H3_VIDEO_PATCH_DIM {
                return Err(DiffusionError::workflow(format!(
                    "h3 video noise rows: expected {} values, got {}",
                    layout.num_video_rows * H3_VIDEO_PATCH_DIM,
                    rows.len()
                )));
            }
            rows.clone()
        }
        None => rng.fill_normal(layout.num_video_rows * H3_VIDEO_PATCH_DIM),
    };
    let mut audio_rows = match &params.audio_noise_rows {
        Some(rows) => {
            if rows.len() != layout.num_audio_rows * H3_AUDIO_IN_CHANNELS {
                return Err(DiffusionError::workflow(format!(
                    "h3 audio noise rows: expected {} values, got {}",
                    layout.num_audio_rows * H3_AUDIO_IN_CHANNELS,
                    rows.len()
                )));
            }
            rows.clone()
        }
        None => rng.fill_normal(layout.num_audio_rows * H3_AUDIO_IN_CHANNELS),
    };

    // --- text encoder (resident alone, then evicted) -------------------------
    // Warm conditioning: an identical presentation (same models dir, token
    // ids, canvas size and keyframe pixels) reuses the cached embeds and
    // skips the whole TE phase — otherwise every encode re-streams the full
    // TE namespace from disk just to reproduce byte-identical embeds.
    ctrl.check()?;
    let cond_key = H3CondKey {
        models_dir: models_dir.to_path_buf(),
        token_ids: full_token_ids.clone(),
        width: params.width,
        height: params.height,
        canvases: keyframe_canvases.clone(),
    };
    let cached_embeds = ctrl
        .cond_cache
        .as_deref()
        .and_then(|cache| cache.embeds_for(&cond_key))
        .map(<[f32]>::to_vec);
    let text_embeds = match cached_embeds {
        Some(embeds) => {
            ctrl.phase("text-cached", 0, 0);
            progress(&format!(
                "te: conditioning cached ({} tokens)",
                full_token_ids.len()
            ));
            embeds
        }
        None => {
            // A previous bf16 job deliberately leaves its DiT/VAE warm. A
            // different prompt cannot load H3's ~67 GB text encoder beside
            // that ~75 GB resident set, even on the 96 GB node. Only a real
            // conditioning-cache miss takes this path, so same-prompt repeat
            // jobs keep the fast warm DiT/VAE path; changed prompts retire the
            // old compute namespaces before streaming the TE.
            let released = crate::backend::release_gpu_runtime_namespaces(&[
                "h3dit::",
                "h3vae::",
            ])?;
            if released > 0 {
                progress(&format!(
                    "te: conditioning changed, released {released} warm DiT/VAE buffers"
                ));
            }
            ctrl.phase("te-load", 0, 0);
            let start = std::time::Instant::now();
            let te_weights = load_te_weights(models_dir, &params.model_set)?;
            let te_prepared = H3TextEncoderPrepared::prepare(&te_weights)?;
            timings.te_load_s = start.elapsed().as_secs_f64();
            ctrl.check()?;
            ctrl.phase("text-encode", 0, 0);
            let start = std::time::Instant::now();
            let text_embeds = {
                // Per-layer counts: the encode re-streams every TE layer's
                // weights (the namespace is evicted after each prompt), so
                // the 30s+ phase ticks "text-encode" 7/50 instead of
                // sitting still.
                let mut on_layer =
                    |done: usize, total: usize| ctrl.phase("text-encode", done, total);
                if keyframe_canvases.is_empty() {
                    h3_text_encode_progress(
                        &te_weights,
                        &te_prepared,
                        &full_token_ids,
                        Some(&mut on_layer),
                    )?
                } else {
                    fl2va_text_encode(
                        &te_weights,
                        &te_prepared,
                        &full_token_ids,
                        &vision_pad_starts,
                        num_vision_tokens,
                        &keyframe_canvases,
                        params.width,
                        params.height,
                        Some(&mut on_layer),
                    )?
                }
            };
            timings.te_encode_s = start.elapsed().as_secs_f64();
            drop(te_prepared);
            drop(te_weights);
            let freed = h3_text_encoder_evict()?;
            progress(&format!(
                "te: encode {:.2}s ({} tokens{}), {freed} buffers evicted",
                timings.te_encode_s,
                full_token_ids.len(),
                if keyframe_canvases.is_empty() {
                    String::new()
                } else {
                    format!(
                        ", {} x {num_vision_tokens} vision",
                        keyframe_canvases.len()
                    )
                }
            ));
            // Cache state only mutates after a fully successful encode.
            if let Some(cache) = ctrl.cond_cache.as_deref_mut() {
                cache.store_embeds(cond_key, text_embeds.clone());
            }
            text_embeds
        }
    };

    // --- fl2va keyframe conditioning rows (never denoised) -------------------
    // One condition block per keyframe, concatenated in packed order: VAE
    // encode -> noise to t=0.999 -> patchify
    // (`before_denoise.py::MiniMaxH3PackConditionRowsStep`, lines 946-957).
    let condition_rows: Vec<f32> = match &params.condition_rows_override {
        Some(rows) => {
            if rows.len() != layout.num_condition_rows * H3_VIDEO_PATCH_DIM {
                return Err(DiffusionError::workflow(format!(
                    "h3 condition rows override: expected {} values, got {}",
                    layout.num_condition_rows * H3_VIDEO_PATCH_DIM,
                    rows.len()
                )));
            }
            rows.clone()
        }
        None if keyframe_canvases.is_empty() => Vec::new(),
        None => {
            ctrl.check()?;
            // The pre-noise latents are keyframe-deterministic (fixed
            // posterior seed) and prompt/seed-independent — cache them; the
            // per-job cond noise (request seed) is applied after lookup.
            let keyframe_keys: Vec<H3KeyframeKey> = keyframe_canvases
                .iter()
                .map(|canvas| H3KeyframeKey {
                    models_dir: models_dir.to_path_buf(),
                    width: params.width,
                    height: params.height,
                    canvas: canvas.clone(),
                })
                .collect();
            let cached: Vec<Option<Vec<f32>>> = keyframe_keys
                .iter()
                .map(|key| {
                    ctrl.cond_cache
                        .as_deref()
                        .and_then(|cache| cache.latents_for(key))
                        .map(<[f32]>::to_vec)
                })
                .collect();
            let start = std::time::Instant::now();
            let missing: Vec<&[u8]> = keyframe_canvases
                .iter()
                .zip(cached.iter())
                .filter(|(_, hit)| hit.is_none())
                .map(|(canvas, _)| canvas.as_slice())
                .collect();
            let mut encoded = if missing.is_empty() {
                ctrl.phase("keyframe-cached", 0, 0);
                progress("keyframe: condition latents cached");
                Vec::new().into_iter()
            } else {
                ctrl.phase("keyframe-encode", 0, 0);
                fl2va_condition_latents_batch(
                    &video_vae_source(models_dir, &params.model_set),
                    &missing,
                    params.width,
                    params.height,
                    latent_h,
                    latent_w,
                )?
                .into_iter()
            };
            let mut latents_per_keyframe = Vec::with_capacity(keyframe_canvases.len());
            for (index, hit) in cached.into_iter().enumerate() {
                let latents = match hit {
                    Some(latents) => latents,
                    None => encoded
                        .next()
                        .expect("one encode per cache miss, in packed order"),
                };
                if let Some(cache) = ctrl.cond_cache.as_deref_mut() {
                    cache.store_latents(
                        H3KeyframeKey {
                            models_dir: models_dir.to_path_buf(),
                            width: params.width,
                            height: params.height,
                            canvas: keyframe_canvases[index].clone(),
                        },
                        latents.clone(),
                    );
                }
                latents_per_keyframe.push(latents);
            }
            let mut rows: Vec<f32> = Vec::with_capacity(
                layout.num_condition_rows * H3_VIDEO_PATCH_DIM,
            );
            for (latents, noise) in latents_per_keyframe.iter().zip(cond_noise.iter()) {
                let noised = h3_scale_noise(latents, H3_KEYFRAME_NOISE_AUG, noise)?;
                rows.extend(h3_patchify_video_latents(&noised, 1, latent_h, latent_w)?);
            }
            timings.keyframe_encode_s = start.elapsed().as_secs_f64();
            if params.staged_residency {
                // The VAE encoder must not co-reside with the DiT on the
                // 24 GB tier; the decoder re-streams after denoise anyway.
                let freed = crate::backend::gpu_weight_cache_evict_prefix_if_loaded("h3vae::")
                    .map_err(DiffusionError::model)?;
                if freed > 0 {
                    progress(&format!("staged: released {freed} VAE encoder buffers"));
                }
            }
            progress(&format!(
                "keyframe: vae encode + condition rows {:.2}s ({} anchors, {} rows @ t={})",
                timings.keyframe_encode_s,
                keyframe_canvases.len(),
                layout.num_condition_rows,
                H3_KEYFRAME_NOISE_AUG,
            ));
            rows
        }
    };
    if condition_rows.len() != layout.num_condition_rows * H3_VIDEO_PATCH_DIM {
        return Err(DiffusionError::workflow(
            "h3 fl2va: condition row count does not match the layout",
        ));
    }

    // --- DiT denoise loop -----------------------------------------------------
    ctrl.check()?;
    ctrl.phase("dit-load", 0, 0);
    let start = std::time::Instant::now();
    let dit_weights = load_dit_weights(models_dir, &params.model_set)?;
    let dit_prepared = H3DitPrepared::prepare(&dit_weights)?;
    timings.dit_load_s = start.elapsed().as_secs_f64();

    // The DiT consumes (and emits) video rows conditioning-first; the euler
    // step only ever writes the generated tail — the anchors ride through
    // every step unchanged.
    let cond_values = layout.num_condition_rows * H3_VIDEO_PATCH_DIM;
    let mut all_video_rows = condition_rows;
    all_video_rows.append(&mut video_rows);
    for step in 0..num_forwards {
        ctrl.check()?;
        ctrl.phase("denoise", step + 1, num_forwards);
        let video_t = video_sched.timesteps[step];
        let audio_t = audio_sched.timesteps[step];
        let plan = h3_build_row_timesteps_cond(
            &layout,
            video_t,
            audio_t,
            video_t.max(H3_KEYFRAME_NOISE_AUG),
        );
        let start = std::time::Instant::now();
        let out = h3_dit_forward(
            &dit_weights,
            &dit_prepared,
            &layout,
            &plan,
            &rope,
            &all_video_rows,
            &audio_rows,
            &text_embeds,
            params.act16,
            &[],
        )?;
        let elapsed = start.elapsed().as_secs_f64();
        timings.forwards_s.push(elapsed);
        h3_euler_step(
            &mut all_video_rows[cond_values..],
            &out.video_velocity[cond_values..],
            video_t,
            video_sched.sigmas[step],
            video_sched.sigmas[step + 1],
        )?;
        h3_euler_step(
            &mut audio_rows,
            &out.audio_velocity,
            audio_t,
            audio_sched.sigmas[step],
            audio_sched.sigmas[step + 1],
        )?;
        ctrl.phase("denoise-done", step + 1, num_forwards);
        progress(&format!(
            "step {}/{num_forwards}: {elapsed:.3}s (video_t={video_t:.4} audio_t={audio_t:.4})",
            step + 1
        ));
    }
    drop(dit_weights);
    if params.staged_residency {
        // 24/32 GB tiers: free the DiT's device weights before the VAE
        // decode claims its compute working set. The pool is left in place —
        // only the weight namespace goes.
        ctrl.phase("dit-unload", 0, 0);
        let freed = crate::backend::gpu_weight_cache_evict_prefix_if_loaded("h3dit::")
            .map_err(DiffusionError::model)?;
        progress(&format!("staged: released {freed} DiT weight buffers"));
    }

    // --- video VAE decode ------------------------------------------------------
    ctrl.check()?;
    ctrl.phase("vae-load", 0, 0);
    // The conditioning rows are dropped before decode — only the generated
    // rows become pixels (the keyframe re-appears as the model's own frame 0).
    let mut latents = h3_unpatchify_video_latents(
        &all_video_rows[cond_values..],
        num_latent_frames,
        latent_h,
        latent_w,
    )?;
    h3_vae_denormalize_latents(&mut latents, num_latent_frames, latent_h, latent_w);
    let start = std::time::Instant::now();
    let vae_weights = H3ShardedWeights::load(video_vae_source(models_dir, &params.model_set))?;
    let vae_prepared = H3VaeDecoderPrepared::prepare(&vae_weights)?;
    timings.vae_load_s = start.elapsed().as_secs_f64();
    let start = std::time::Instant::now();
    let decoded = {
        let mut on_group = |group: usize, total: usize| {
            if let Some(on_phase) = ctrl.on_phase.as_deref_mut() {
                on_phase("vae", group, total);
            }
        };
        let mut vae_ctrl = H3VaeCtrl {
            on_group: &mut on_group,
            cancel: ctrl.cancel,
        };
        h3_vae_decode_ctrl(
            &vae_weights,
            &vae_prepared,
            &latents,
            num_latent_frames,
            latent_h,
            latent_w,
            Some(&mut vae_ctrl),
        )?
    };
    timings.vae_decode_s = start.elapsed().as_secs_f64();
    let frames_rgb8 = h3_vae_frames_to_u8(&decoded.raw);
    ctrl.phase("vae-done", 0, 0);
    progress(&format!(
        "vae: decode {:.2}s -> {} frames {}x{}",
        timings.vae_decode_s, decoded.raw.f, decoded.raw.w, decoded.raw.h
    ));
    if params.staged_residency {
        ctrl.phase("vae-unload", 0, 0);
        let freed = crate::backend::gpu_weight_cache_evict_prefix_if_loaded("h3vae::")
            .map_err(DiffusionError::model)?;
        progress(&format!("staged: released {freed} VAE weight buffers"));
    }

    // --- audio VAE decode (CPU f32) ------------------------------------------
    let mut audio_planar = None;
    let mut audio_sample_rate = 0;
    if params.decode_audio {
        ctrl.check()?;
        ctrl.phase("audio-decode", 0, 0);
        let start = std::time::Instant::now();
        let audio_vae = H3AudioVae::load(&audio_vae_source(models_dir, &params.model_set))?;
        let stereo = {
            let mut on_step = |done: usize, total: usize| ctrl.phase("audio-decode", done, total);
            audio_vae.decode_rows_progress(&audio_rows, num_audio_latents, Some(&mut on_step))?
        };
        timings.audio_decode_s = start.elapsed().as_secs_f64();
        audio_sample_rate = audio_vae.sampling_rate();
        progress(&format!(
            "audio: decode {:.2}s -> {} samples/ch @ {audio_sample_rate}Hz",
            timings.audio_decode_s,
            stereo.len() / 2,
        ));
        audio_planar = Some(stereo);
    }

    timings.total_s = total_start.elapsed().as_secs_f64();
    Ok(H3GenerateOutput {
        frames_rgb8,
        width: params.width,
        height: params.height,
        num_frames,
        audio_planar,
        audio_sample_rate,
        timings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(prompt_ids: &[u32], w: usize, h: usize, canvas: Option<Vec<u8>>) -> H3CondKey {
        H3CondKey {
            models_dir: PathBuf::from("/models/h3"),
            token_ids: prompt_ids.to_vec(),
            width: w,
            height: h,
            canvases: canvas.into_iter().collect(),
        }
    }

    fn keyframe(rgb: Vec<u8>, w: usize, h: usize, anchor: H3KeyframeAnchor) -> H3KeyframeInput {
        H3KeyframeInput {
            rgb,
            width: w,
            height: h,
            anchor,
            picture_label_ids: Vec::new(),
        }
    }

    /// A flat WxH RGB image, distinguishable per call by `tint`.
    fn flat(w: usize, h: usize, tint: u8) -> Vec<u8> {
        (0..w * h * 3).map(|i| tint.wrapping_add(i as u8)).collect()
    }

    /// The cache keys on the exact conditioning identity: same presentation
    /// reuses, any prompt/size/canvas/dir difference misses. Seed and step
    /// count are per-generate arguments and never key the cache.
    #[test]
    fn cond_cache_keys_on_presentation_not_seed() {
        let mut cache = H3CondCache::default();
        let embeds = vec![1.0f32, 2.0, 3.0];
        cache.store_embeds(key(&[5, 6, 7], 640, 352, None), embeds.clone());

        // Identical presentation: hit.
        assert_eq!(
            cache.embeds_for(&key(&[5, 6, 7], 640, 352, None)),
            Some(embeds.as_slice())
        );
        // Different prompt tokens: miss.
        assert!(cache.embeds_for(&key(&[5, 6, 8], 640, 352, None)).is_none());
        // Different canvas size: miss (vision grid + layout change).
        assert!(cache.embeds_for(&key(&[5, 6, 7], 864, 480, None)).is_none());
        // Keyframe appears: miss (fl2va embeds see the pixels).
        assert!(cache
            .embeds_for(&key(&[5, 6, 7], 640, 352, Some(vec![0, 1, 2])))
            .is_none());
        // Different models dir: miss.
        let mut other_dir = key(&[5, 6, 7], 640, 352, None);
        other_dir.models_dir = PathBuf::from("/models/other");
        assert!(cache.embeds_for(&other_dir).is_none());

        // Store replaces the single entry (one warm prompt at a time).
        let embeds2 = vec![9.0f32];
        cache.store_embeds(key(&[5, 6, 8], 640, 352, None), embeds2.clone());
        assert!(cache.embeds_for(&key(&[5, 6, 7], 640, 352, None)).is_none());
        assert_eq!(
            cache.embeds_for(&key(&[5, 6, 8], 640, 352, None)),
            Some(embeds2.as_slice())
        );
    }

    /// Keyframe latents key on the canvas pixels, NOT the prompt: a prompt
    /// change with the same keyframe reuses the VAE encode.
    #[test]
    fn keyframe_latents_key_on_canvas_not_prompt() {
        let mut cache = H3CondCache::default();
        let canvas = vec![10u8, 20, 30];
        let latents = vec![0.5f32; 8];
        let kf = |canvas: Vec<u8>, w: usize| H3KeyframeKey {
            models_dir: PathBuf::from("/models/h3"),
            width: w,
            height: 352,
            canvas,
        };
        cache.store_latents(kf(canvas.clone(), 640), latents.clone());
        assert_eq!(
            cache.latents_for(&kf(canvas.clone(), 640)),
            Some(latents.as_slice())
        );
        // Different pixels or size: miss.
        assert!(cache.latents_for(&kf(vec![1, 2, 3], 640)).is_none());
        assert!(cache.latents_for(&kf(canvas, 672)).is_none());
    }

    /// A first+last request encodes two anchors, so the latent cache must
    /// hold both — a single slot would thrash every job.
    #[test]
    fn keyframe_latents_cache_holds_both_anchors() {
        let mut cache = H3CondCache::default();
        let kf = |canvas: Vec<u8>| H3KeyframeKey {
            models_dir: PathBuf::from("/models/h3"),
            width: 640,
            height: 352,
            canvas,
        };
        cache.store_latents(kf(vec![1, 2, 3]), vec![1.0f32]);
        cache.store_latents(kf(vec![4, 5, 6]), vec![2.0f32]);
        assert_eq!(cache.latents_for(&kf(vec![1, 2, 3])), Some(&[1.0f32][..]));
        assert_eq!(cache.latents_for(&kf(vec![4, 5, 6])), Some(&[2.0f32][..]));
        // Re-storing an entry keeps one copy, most recent first.
        cache.store_latents(kf(vec![1, 2, 3]), vec![3.0f32]);
        assert_eq!(cache.keyframe_latents.len(), 2);
        assert_eq!(cache.latents_for(&kf(vec![1, 2, 3])), Some(&[3.0f32][..]));
        // Past the slot count the oldest is dropped.
        for tint in 0..H3_KEYFRAME_CACHE_SLOTS as u8 {
            cache.store_latents(kf(vec![100 + tint]), vec![tint as f32]);
        }
        assert_eq!(cache.keyframe_latents.len(), H3_KEYFRAME_CACHE_SLOTS);
        assert!(cache.latents_for(&kf(vec![4, 5, 6])).is_none());
    }

    /// PARITY GATE. With one keyframe the presentation is byte-for-byte what
    /// the single-image pipeline built: label, `<|vision_start|>`, N pads,
    /// `<|vision_end|>`, prompt — with the vision block tagged VIDEO and the
    /// pad span starting right after `<|vision_start|>`.
    #[test]
    fn single_keyframe_presentation_is_unchanged() {
        let prompt = [7u32, 8, 9];
        let label = [40u32, 41];
        let num_vision_tokens = 6usize;
        let (ids, tags, starts) =
            presentation(&label_only(&label, 1), &prompt, num_vision_tokens);
        let mut expected_ids = label.to_vec();
        expected_ids.push(H3_TOKEN_VISION_START);
        expected_ids.extend(std::iter::repeat(H3_TOKEN_IMAGE_PAD).take(num_vision_tokens));
        expected_ids.push(H3_TOKEN_VISION_END);
        expected_ids.extend_from_slice(&prompt);
        assert_eq!(ids, expected_ids);
        assert_eq!(starts, vec![label.len() + 1]);
        let mut expected_tags = vec![H3_TEXT_TAG; label.len()];
        expected_tags.extend(std::iter::repeat(H3_VIDEO_TAG).take(num_vision_tokens + 2));
        expected_tags.extend(std::iter::repeat(H3_TEXT_TAG).take(prompt.len()));
        assert_eq!(tags, expected_tags);
    }

    /// Two keyframes = two labelled vision blocks in packed order, then the
    /// prompt once. Mirrors `encoders.py::MiniMaxH3FL2VATextEncoderStep`.
    #[test]
    fn two_keyframes_build_two_labelled_vision_blocks() {
        let prompt = [7u32, 8];
        let labels = [vec![40u32, 41], vec![50u32, 51, 52]];
        let num_vision_tokens = 4usize;
        let keyframes: Vec<H3KeyframeInput> = labels
            .iter()
            .zip([H3KeyframeAnchor::First, H3KeyframeAnchor::Last])
            .map(|(label, anchor)| H3KeyframeInput {
                rgb: Vec::new(),
                width: 0,
                height: 0,
                anchor,
                picture_label_ids: label.clone(),
            })
            .collect();
        let (ids, tags, starts) = presentation(&keyframes, &prompt, num_vision_tokens);
        assert_eq!(starts, vec![3, 3 + 4 + 1 + 3 + 1]);
        assert_eq!(ids.len(), tags.len());
        assert_eq!(ids[starts[0]], H3_TOKEN_IMAGE_PAD);
        assert_eq!(ids[starts[1]], H3_TOKEN_IMAGE_PAD);
        assert_eq!(ids[starts[0] - 1], H3_TOKEN_VISION_START);
        assert_eq!(ids[starts[1] - 1], H3_TOKEN_VISION_START);
        assert_eq!(&ids[ids.len() - prompt.len()..], &prompt);
        // The labels are text, the whole vision block (delimiters included)
        // is video.
        for start in &starts {
            for row in start - 1..start + num_vision_tokens + 1 {
                assert_eq!(tags[row], H3_VIDEO_TAG, "row {row}");
            }
            assert_eq!(tags[start - 2], H3_TEXT_TAG);
        }
    }

    /// The geometry anchor is stretched; the follower is cover-cropped. A
    /// 2:1 source onto a square canvas keeps the middle half, not the whole
    /// frame squeezed.
    #[test]
    fn follower_keyframe_is_cover_cropped_not_stretched() {
        let (src_w, src_h) = (64usize, 32usize);
        let rgb = flat(src_w, src_h, 3);
        let first = keyframe(rgb.clone(), src_w, src_h, H3KeyframeAnchor::First);
        let last = keyframe(rgb, src_w, src_h, H3KeyframeAnchor::Last);
        let stretched = fl2va_prepare_canvas(&first, 0, 32, 32).unwrap();
        let cropped = fl2va_prepare_canvas(&last, 1, 32, 32).unwrap();
        assert_eq!(stretched.len(), 32 * 32 * 3);
        assert_eq!(cropped.len(), 32 * 32 * 3);
        assert_ne!(stretched, cropped);
        // Cover: scale = max(32/64, 32/32) = 1.0 -> resize to 64x32, crop 32
        // wide from x=16. So the crop is the source's middle columns, byte
        // for byte (no resample at scale 1).
        let source = flat(src_w, src_h, 3);
        for y in 0..32usize {
            let src = (y * src_w + 16) * 3;
            assert_eq!(
                &cropped[y * 32 * 3..(y + 1) * 32 * 3],
                &source[src..src + 32 * 3],
                "row {y}"
            );
        }
    }

    /// A canvas-sized keyframe is passed through untouched whatever its
    /// position — no resample, no crop.
    #[test]
    fn exact_canvas_keyframe_is_passed_through() {
        let rgb = flat(32, 32, 9);
        let kf = keyframe(rgb.clone(), 32, 32, H3KeyframeAnchor::Last);
        assert_eq!(fl2va_prepare_canvas(&kf, 0, 32, 32).unwrap(), rgb);
        assert_eq!(fl2va_prepare_canvas(&kf, 1, 32, 32).unwrap(), rgb);
    }

    /// The same image at both ends prepares to the same canvas bytes, so the
    /// two condition blocks carry identical latents and differ only in their
    /// rotary anchor.
    #[test]
    fn identical_start_and_end_prepare_to_the_same_canvas() {
        let rgb = flat(48, 48, 21);
        let first = keyframe(rgb.clone(), 48, 48, H3KeyframeAnchor::First);
        let last = keyframe(rgb, 48, 48, H3KeyframeAnchor::Last);
        let a = fl2va_prepare_canvas(&first, 0, 32, 32).unwrap();
        let b = fl2va_prepare_canvas(&last, 1, 32, 32).unwrap();
        assert_eq!(a, b);
    }

    /// A keyframe whose byte length disagrees with its declared size is a
    /// loud error, not a silent crop.
    #[test]
    fn malformed_keyframe_rgb_is_refused() {
        let kf = keyframe(vec![0u8; 10], 8, 8, H3KeyframeAnchor::First);
        assert!(fl2va_prepare_canvas(&kf, 0, 32, 32).is_err());
    }

    // -- helpers mirroring the pipeline's presentation assembly ------------

    fn label_only(label: &[u32], _count: usize) -> Vec<H3KeyframeInput> {
        vec![H3KeyframeInput {
            rgb: Vec::new(),
            width: 0,
            height: 0,
            anchor: H3KeyframeAnchor::First,
            picture_label_ids: label.to_vec(),
        }]
    }

    /// The exact presentation loop of `h3_generate_with_control`, lifted so
    /// the token layout can be asserted without a checkpoint on disk.
    fn presentation(
        keyframes: &[H3KeyframeInput],
        prompt: &[u32],
        num_vision_tokens: usize,
    ) -> (Vec<u32>, Vec<u8>, Vec<usize>) {
        let mut ids: Vec<u32> = Vec::new();
        let mut tags: Vec<u8> = Vec::new();
        let mut pad_starts = Vec::with_capacity(keyframes.len());
        for keyframe in keyframes {
            ids.extend_from_slice(&keyframe.picture_label_ids);
            tags.extend(std::iter::repeat(H3_TEXT_TAG).take(keyframe.picture_label_ids.len()));
            ids.push(H3_TOKEN_VISION_START);
            pad_starts.push(ids.len());
            ids.extend(std::iter::repeat(H3_TOKEN_IMAGE_PAD).take(num_vision_tokens));
            ids.push(H3_TOKEN_VISION_END);
            tags.extend(std::iter::repeat(H3_VIDEO_TAG).take(num_vision_tokens + 2));
        }
        ids.extend_from_slice(prompt);
        tags.extend(std::iter::repeat(H3_TEXT_TAG).take(prompt.len()));
        (ids, tags, pad_starts)
    }
}
