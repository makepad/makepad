//! The `h3` backend: video domain — MiniMax H3 text-to-video(+audio) through
//! the in-repo port in libs/diffusion (`h3_pipeline` on the makepad-ggml CUDA
//! stack), muxed to mp4 by the platform hardware video encoder
//! (`makepad_video`: Media Foundation/NVENC on Windows, VideoToolbox on
//! macOS). No python, no ffmpeg, no full UI platform crate.
//!
//! Layering (kokoro pattern, one level deeper):
//! - Request handling, parameter defaults, the 32 kHz -> 48 kHz audio
//!   resample and the mux-input assembly compile and test EVERYWHERE — the
//!   generator and the mp4 muxer are pluggable, so CI exercises the whole
//!   video job path with stubs.
//! - The real generator (`h3_gen`, feature `video`) tokenizes the prompt with
//!   the in-repo Qwen tokenizer port, runs TE -> DiT denoise -> video VAE ->
//!   audio VAE, and reports denoise progress per step.
//! - The real muxer (`h3_mux`, feature `video`) drives `VideoFileEncoder`
//!   (HEVC default / H264 on request, AAC audio track) and returns the mp4
//!   bytes.
//!
//! Request: `{model: "minimax-h3", prompt, width, height, frames, steps,
//! seed, codec}` -> one `video/mp4` artifact (24 fps, stereo AAC when the
//! pipeline yields audio).
//!
//! Numerics note: the validated H3 config is the f32 spine — H3 activations
//! overflow f16 (reference's own f16 dumps are inf), so `ensure_loaded` pins
//! FLUX_GEMM_F16ACC=0 unless the environment overrides it explicitly.
//!
//! Warm residency (flux pattern, measured 2026-08-13 on the 96GB box): H3
//! streams weights per tensor (no host arenas) and its DiT/VAE device
//! weight caches persist across jobs on the worker thread already — the
//! one real per-job waste was the text encoder, whose ~66.7GB namespace
//! re-streams from disk per prompt and is evicted after (~42s of a 46s
//! repeat job). `H3Gen` therefore keeps an [`H3CondCache`]: a repeat
//! prompt at the same canvas reuses the encoded conditioning (host-side,
//! Send — no worker thread needed) and skips the whole TE phase; a prompt
//! change re-encodes only the conditioning. Any failed or cancelled H3 job
//! retires its CUDA namespaces before another job is admitted; a successful
//! bf16 job stays warm until a changed presentation or explicit unload.

use crate::backend::{CancelToken, ArtifactData, BackendCtx, ContentBackend, GenerateParams, ProgressSink};
use crate::error::AssetAiError;
use crate::registry::ModelSpec;
use crate::resample::{interleave_stereo_i16, resample_channel};
use std::path::PathBuf;

/// H3's native clock.
pub const H3_FPS: u32 = 24;
/// What platform AAC encoders accept (Windows MF: 44.1/48 kHz only).
pub const MUX_AUDIO_RATE: u32 = 48_000;

pub const DEFAULT_WIDTH: u32 = 640;
pub const DEFAULT_HEIGHT: u32 = 352;
pub const DEFAULT_FRAMES: u32 = 124;
pub const DEFAULT_STEPS: u32 = 50;

/// Every CUDA weight-cache namespace owned by H3. The source-specific
/// quantized namespaces remain below these prefixes (`h3dit::gg::...`,
/// `h3dit::nv::...`), so one teardown covers bf16 and both quant tiers.
#[cfg(any(feature = "video", test))]
const H3_GPU_NAMESPACE_PREFIXES: [&str; 3] = ["h3te::", "h3dit::", "h3vae::"];

/// Truthful device-residency bookkeeping kept separate from tokenizer and
/// conditioning caches. Those small host-side helpers may survive a staged
/// run, but they must never let the admission gate skip its VRAM check.
#[derive(Default)]
#[cfg(any(feature = "video", test))]
struct H3RuntimeLifecycle {
    touched: bool,
}

#[cfg(any(feature = "video", test))]
impl H3RuntimeLifecycle {
    fn mark_touched(&mut self) {
        self.touched = true;
    }

    fn is_resident(&self) -> bool {
        self.touched
    }

    /// Clear the resident bit only after every namespace and scratch pool was
    /// released successfully. A teardown error stays truthfully resident so
    /// the server can surface it and retry rather than admitting another
    /// heavyweight model over leaked allocations.
    fn release_with(
        &mut self,
        release: impl FnOnce(&[&str]) -> Result<usize, AssetAiError>,
    ) -> Result<usize, AssetAiError> {
        if !self.touched {
            return Ok(0);
        }
        let released = release(&H3_GPU_NAMESPACE_PREFIXES)?;
        self.touched = false;
        Ok(released)
    }
}

// ---------------------------------------------------------------------------
// Quantized-tier manifests (data-driven from registry file ROLES)
// ---------------------------------------------------------------------------

/// Semantic file roles a quantized H3 tier manifest must carry. Their
/// presence — not the model id — selects the weight pipeline, so new tiers
/// are registry data, not code.
pub const ROLE_DIT_GGUF: &str = "dit-gguf";
pub const ROLE_TE_GGUF: &str = "te-gguf";
pub const ROLE_DIT_NVFP4: &str = "dit-nvfp4";
pub const ROLE_TE_NVFP4: &str = "te-nvfp4";
pub const ROLE_VIDEO_VAE: &str = "video-vae";
pub const ROLE_AUDIO_VAE: &str = "audio-vae";
pub const ROLE_AUDIO_VAE_CONFIG: &str = "audio-vae-config";
pub const ROLE_TOKENIZER_JSON: &str = "tokenizer-json";
/// Auxiliary role carried by every H3 tier: the Practical-RIFE v4.26
/// flownet used by the optional interpolation post-stage. It is a file of
/// the video models, never a model of its own — the domain must keep
/// exactly one selectable generator per tier.
pub const ROLE_INTERPOLATE: &str = "interpolate";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum H3TierKind {
    /// The canonical bf16 diffusers tree (96GB class) — legacy manifests
    /// anchored on `model_index.json`.
    Bf16Tree,
    /// GGUF Q4_K family (24GB class): pruned DiT + Q4_K_M TE, staged.
    GgufQ4,
    /// ComfyUI/ModelOpt NVFP4 (Blackwell 32GB class), staged.
    Nvfp4,
}

/// Everything `ensure_loaded` derives from a registry entry before any file
/// or GPU work: which weight pipeline runs and its hard canvas ceiling.
#[derive(Clone, Copy, Debug)]
pub struct H3TierPlan {
    pub kind: H3TierKind,
    /// Sequential component residency (load/unload per phase).
    pub staged: bool,
    /// Hard `width*height*frames` ceiling measured for the tier's VRAM
    /// class; requests above it are refused (fail closed, no WDDM thrash).
    pub max_pixel_frames: Option<u64>,
}

/// Measured tier ceilings (h3-tiers ladder, 2026-08-12): pruned-Q4 tops out
/// at 960x544x124 on 24GB; the full ladder passes up to 1344x768x124 within
/// 32GB.
const GGUF_Q4_MAX_PIXEL_FRAMES: u64 = 960 * 544 * 124;
const NVFP4_MAX_PIXEL_FRAMES: u64 = 1344 * 768 * 124;

/// Derives the tier plan from a registry entry's file roles. Fails closed on
/// contradictory or incomplete quantized manifests.
pub fn tier_plan_for_spec(spec: &ModelSpec) -> Result<H3TierPlan, AssetAiError> {
    let has = |role: &str| spec.file_by_role(role).is_some();
    let quant_kind = match (has(ROLE_DIT_GGUF), has(ROLE_DIT_NVFP4)) {
        (false, false) => {
            return Ok(H3TierPlan {
                kind: H3TierKind::Bf16Tree,
                staged: false,
                max_pixel_frames: None,
            })
        }
        (true, true) => {
            return Err(AssetAiError::Registry(format!(
                "model {}: both {ROLE_DIT_GGUF} and {ROLE_DIT_NVFP4} present — a tier manifest carries exactly one DiT",
                spec.id
            )))
        }
        (true, false) => H3TierKind::GgufQ4,
        (false, true) => H3TierKind::Nvfp4,
    };
    let te_role = match quant_kind {
        H3TierKind::GgufQ4 => ROLE_TE_GGUF,
        _ => ROLE_TE_NVFP4,
    };
    for role in [
        te_role,
        ROLE_VIDEO_VAE,
        ROLE_AUDIO_VAE,
        ROLE_AUDIO_VAE_CONFIG,
        ROLE_TOKENIZER_JSON,
    ] {
        if !has(role) {
            return Err(AssetAiError::Registry(format!(
                "model {}: quantized tier manifest is missing the {role:?} file role",
                spec.id
            )));
        }
    }
    Ok(H3TierPlan {
        kind: quant_kind,
        staged: true,
        max_pixel_frames: Some(match quant_kind {
            H3TierKind::GgufQ4 => GGUF_Q4_MAX_PIXEL_FRAMES,
            _ => NVFP4_MAX_PIXEL_FRAMES,
        }),
    })
}

/// Fail-closed GPU gate: a model carrying hard requirements refuses to load
/// when the GPU is below them — or UNKNOWN (no nvidia-smi / unparsable):
/// falling through to a thrashing or crashing run is the failure mode this
/// exists to prevent.
pub fn check_gpu_requirements(
    model_id: &str,
    min_vram_gb: Option<f64>,
    min_compute_cap: Option<f64>,
    gpu: &crate::gpu::GpuInfo,
) -> Result<(), AssetAiError> {
    crate::backend::check_gpu_requirements(model_id, min_vram_gb, min_compute_cap, gpu)
        .map_err(AssetAiError::Unavailable)
}

/// The tier canvas gate applied per job (see [`H3TierPlan::max_pixel_frames`]).
pub fn check_canvas_within_tier(
    model_id: &str,
    limit: Option<u64>,
    width: u32,
    height: u32,
    frames: u32,
) -> Result<(), AssetAiError> {
    let Some(limit) = limit else { return Ok(()) };
    let requested = width as u64 * height as u64 * frames as u64;
    if requested > limit {
        return Err(AssetAiError::Params(format!(
            "{model_id}: {width}x{height}x{frames} frames exceeds this tier's measured VRAM ceiling \
             ({requested} > {limit} pixel-frames) — lower the canvas/frames or use a larger-VRAM H3 tier"
        )));
    }
    Ok(())
}

/// One generation request handed to the generator.
#[derive(Clone, Debug)]
pub struct VideoJob {
    pub prompt: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub steps: u32,
    pub seed: u64,
    /// i2v (fl2va): decoded first-frame keyframe as `(rgb8, width, height)`.
    /// The video VISIBLY starts from this image — the pipeline VAE-encodes it
    /// into never-denoised leading rows and feeds it to the vision tower.
    pub input_rgb: Option<(Vec<u8>, u32, u32)>,
    /// true (default) = decode the jointly-denoised audio latents and mux an
    /// AAC track. false = skip the audio VAE decode and mux a silent mp4.
    /// The DiT still denoises the audio rows either way — there is no
    /// upstream mode that drops them from the packed t2va sequence.
    pub audio: bool,
}

/// Raw decoded output of a video generator.
pub struct VideoClip {
    pub width: usize,
    pub height: usize,
    pub num_frames: usize,
    /// `num_frames * height * width * 3` tightly packed RGB.
    pub frames_rgb8: Vec<u8>,
    /// Stereo planar f32 `[L..., R...]` at `audio_rate`; `None` = silent clip.
    pub audio_planar: Option<Vec<f32>>,
    pub audio_rate: u32,
}

/// Everything the muxer needs; built by `generate` from the clip.
pub struct MuxInput {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub h264: bool,
    pub frames_rgb8: Vec<u8>,
    /// Interleaved stereo 16-bit PCM at `MUX_AUDIO_RATE`; `None` = no track.
    pub audio_i16: Option<Vec<i16>>,
}

/// Pluggable generation: the real path runs the H3 pipeline; tests plug in a
/// closure. The `CancelToken` must be honored between steps/phases — the
/// real path checks it between denoise steps and VAE batch groups.
pub type GenFn = Box<
    dyn FnMut(&VideoJob, ProgressSink, &CancelToken) -> Result<VideoClip, AssetAiError> + Send,
>;
/// Pluggable mp4 mux: the real path drives the platform `VideoFileEncoder`.
pub type MuxFn = Box<dyn FnMut(&MuxInput) -> Result<Vec<u8>, AssetAiError> + Send>;

enum Gen {
    Stub(GenFn),
    #[cfg(feature = "video")]
    H3(h3_gen::H3Gen),
}

enum Mux {
    Stub(MuxFn),
    #[cfg(feature = "video")]
    Platform,
}

pub struct H3Backend {
    model_id: String,
    gen: Gen,
    mux: Mux,
    /// Cache dir captured in `ensure_loaded`; the muxer writes its temp file
    /// here (`VideoFileEncoder` writes a file, the artifact wants bytes).
    cache_dir: Option<PathBuf>,
    /// Tier canvas ceiling captured in `ensure_loaded` (None until then, and
    /// for the bf16 tier).
    tier_limit_pixel_frames: Option<u64>,
    /// RIFE flownet path resolved from the tier's [`ROLE_INTERPOLATE`] file
    /// in `ensure_loaded`; None when the manifest carries no such role (an
    /// `interpolate` request then fails loudly instead of silently
    /// returning 24 fps).
    interpolate_weights: Option<PathBuf>,
}

impl H3Backend {
    /// Test/CI constructor: generation and muxing are the given closures.
    pub fn with_stubs(model_id: &str, gen: GenFn, mux: MuxFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
            mux: Mux::Stub(mux),
            cache_dir: None,
            tier_limit_pixel_frames: None,
            interpolate_weights: None,
        }
    }

    /// Real constructor used by `create_backend`.
    #[cfg(feature = "video")]
    pub fn new_h3(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::H3(h3_gen::H3Gen::new()),
            mux: Mux::Platform,
            cache_dir: None,
            tier_limit_pixel_frames: None,
            interpolate_weights: None,
        }
    }
}

/// Rounds a requested canvas dimension to the H3 grid: multiples of 16
/// (video VAE patch; also even, as 4:2:0 encoders require).
pub fn snap_dim(value: u32) -> u32 {
    let snapped = ((value + 8) / 16) * 16;
    snapped.max(16)
}

/// fl2va canvas dimensions must be multiples of 32 (Qwen3-VL vision patch 16
/// x spatial merge 2 — the keyframe feeds the vision tower without resizing).
pub fn snap_dim32(value: u32) -> u32 {
    let snapped = ((value + 16) / 32) * 32;
    snapped.max(32)
}

/// Derives the i2v canvas from the keyframe's aspect ratio (the reference
/// behaviour when no canvas is requested), scaled to the service's default
/// speed class: short edge 352, capped at the 640x352 pixel budget, snapped
/// to the 32-multiple grid fl2va needs.
pub fn derive_canvas_for_keyframe(keyframe_w: u32, keyframe_h: u32) -> (u32, u32) {
    const SHORT_EDGE: f64 = 352.0;
    const MAX_PIXELS: f64 = 640.0 * 352.0;
    let (kw, kh) = (keyframe_w.max(1) as f64, keyframe_h.max(1) as f64);
    let mut scale = SHORT_EDGE / kw.min(kh);
    if kw * scale * kh * scale > MAX_PIXELS {
        scale *= (MAX_PIXELS / (kw * scale * kh * scale)).sqrt();
    }
    (snap_dim32((kw * scale) as u32), snap_dim32((kh * scale) as u32))
}

/// Parses a requested codec name; empty = default (H265).
pub fn wants_h264(codec: &str) -> Result<bool, AssetAiError> {
    match codec.trim().to_ascii_lowercase().as_str() {
        "" | "h265" | "hevc" => Ok(false),
        "h264" | "avc" => Ok(true),
        other => Err(AssetAiError::Backend(format!(
            "unknown codec {other:?} (expected \"h265\" or \"h264\")"
        ))),
    }
}

/// Maps an H3 pipeline progress line to a `(stage, fraction)` for the job
/// state. Kept as a fallback mapper (and for log-driven tooling); the real
/// generation path now consumes the pipeline's STRUCTURED phase callback —
/// see [`stage_from_phase`] — so the job shows per-step/per-clip motion.
pub fn stage_from_line(line: &str) -> Option<(&'static str, f64)> {
    if line.starts_with("layout:") {
        return Some(("layout", 0.01));
    }
    if line.starts_with("te:") {
        return Some(("text-encode", 0.08));
    }
    if let Some(rest) = line.strip_prefix("step ") {
        let frac = rest.split(':').next().and_then(|counts| {
            let mut parts = counts.split('/');
            let k: f64 = parts.next()?.trim().parse().ok()?;
            let n: f64 = parts.next()?.trim().parse().ok()?;
            if n > 0.0 {
                Some(k / n)
            } else {
                None
            }
        })?;
        return Some(("denoise", 0.10 + 0.72 * frac.clamp(0.0, 1.0)));
    }
    if line.starts_with("vae:") {
        return Some(("video-decode", 0.86));
    }
    if line.starts_with("audio:") {
        return Some(("audio-decode", 0.90));
    }
    None
}

/// Maps the pipeline's structured `(phase, done, total)` callback to the job
/// progress convention: stage string carries the step count ("denoise 23/50"),
/// the fraction is the OVERALL job fraction. Budget: load+encode 0.02..0.15,
/// denoise 0.15..0.80 (per step: start of step k sits at (k-1)/N, completion
/// at k/N), vae 0.80..0.91 (per batch group), audio 0.92; the backend's own
/// resample/mux stages continue at 0.93/0.95.
pub fn stage_from_phase(phase: &str, done: usize, total: usize) -> Option<(String, f64)> {
    const DENOISE_BASE: f64 = 0.15;
    const DENOISE_SPAN: f64 = 0.65;
    const VAE_BASE: f64 = 0.81;
    const VAE_SPAN: f64 = 0.10;
    match phase {
        "te-load" => Some(("load text-encoder".to_string(), 0.02)),
        // The TE re-streams its weights every prompt (namespace evicted) —
        // 31-36s that must visibly move: per-layer counts from the pipeline.
        "text-encode" if total > 0 => Some((
            format!("text-encode {done}/{total}"),
            0.03 + 0.09 * done as f64 / total as f64,
        )),
        "text-encode" => Some(("text-encode".to_string(), 0.06)),
        // Warm conditioning reuse (H3CondCache hit): the whole TE phase is
        // skipped — visible so the ~40s jump reads as a feature, not a hang.
        "text-cached" => Some(("text-encode (cached)".to_string(), 0.06)),
        "keyframe-encode" => Some(("keyframe-encode".to_string(), 0.10)),
        "keyframe-cached" => Some(("keyframe-encode (cached)".to_string(), 0.10)),
        "dit-load" => Some(("load transformer".to_string(), 0.12)),
        "denoise" if total > 0 => {
            // Step `done` is STARTING; step 1 also streams the DiT weights.
            let stream = if done == 1 { " (streaming weights)" } else { "" };
            Some((
                format!("denoise {done}/{total}{stream}"),
                DENOISE_BASE + DENOISE_SPAN * (done - 1) as f64 / total as f64,
            ))
        }
        "denoise-done" if total > 0 => Some((
            format!("denoise {done}/{total}"),
            DENOISE_BASE + DENOISE_SPAN * done as f64 / total as f64,
        )),
        "vae-load" => Some(("load video-vae".to_string(), 0.80)),
        "vae" if total > 0 => Some((
            format!("vae {done}/{total}"),
            VAE_BASE + VAE_SPAN * (done - 1) as f64 / total as f64,
        )),
        "vae-done" => Some(("vae done".to_string(), VAE_BASE + VAE_SPAN)),
        "audio-decode" if total > 0 => Some((
            format!("audio-decode {done}/{total}"),
            0.90 + 0.06 * done as f64 / total as f64,
        )),
        "audio-decode" => Some(("audio-decode".to_string(), 0.92)),
        _ => None,
    }
}

impl ContentBackend for H3Backend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        self.cache_dir = Some(ctx.cache_dir.to_path_buf());
        // Defense in depth before CUDA work. The authoritative service gate
        // runs before artifact preparation/download (including pull-only),
        // while this protects direct backend callers too.
        check_gpu_requirements(
            &ctx.spec.id,
            ctx.spec.min_vram_gb,
            ctx.spec.min_compute_cap,
            &crate::gpu::query_gpu(),
        )?;
        let plan = tier_plan_for_spec(ctx.spec)?;
        self.tier_limit_pixel_frames = plan.max_pixel_frames;
        self.interpolate_weights = ctx
            .spec
            .file_by_role(ROLE_INTERPOLATE)
            .map(|file| file.dest_path(ctx.cache_dir));
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "video")]
            Gen::H3(gen) => gen.ensure_loaded(ctx, &plan),
        }
    }

    fn is_resident(&self) -> bool {
        match &self.gen {
            Gen::Stub(_) => false,
            #[cfg(feature = "video")]
            Gen::H3(gen) => gen.is_resident(),
        }
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        match &mut self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "video")]
            Gen::H3(gen) => gen.unload(),
        }
    }

    fn resident_is_healthy_after_error(&self, _error: &AssetAiError) -> bool {
        // H3 can fail or be cancelled between streamed tensors. Even when a
        // phase normally leaves a warm cache, an interrupted phase is not a
        // proven reusable boundary. Retire it deterministically; H3Gen also
        // performs eager same-thread cleanup for pipeline errors.
        false
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        if params.prompt.trim().is_empty() {
            return Err(AssetAiError::Backend(
                "video generation needs a non-empty `prompt`".to_string(),
            ));
        }
        // i2v (fl2va): an input image is DECODED HERE so a bad relay fails
        // the job loudly — the pipeline must never silently ignore an image
        // and degrade to t2v.
        let input_rgb = match params.input_bytes.is_empty() {
            true => None,
            false => {
                if !params.input_content_type.starts_with("image/png")
                    && !params.input_content_type.is_empty()
                {
                    return Err(AssetAiError::Params(format!(
                        "minimax-h3: input_b64 content type {:?} not supported (image/png only)",
                        params.input_content_type
                    )));
                }
                let (rgba, w, h) = crate::trellis_backend::decode_png_rgba8(&params.input_bytes)?;
                let mut rgb = vec![0u8; w * h * 3];
                for (dst, src) in rgb.chunks_exact_mut(3).zip(rgba.chunks_exact(4)) {
                    dst.copy_from_slice(&src[..3]);
                }
                Some((rgb, w as u32, h as u32))
            }
        };
        let h264 = wants_h264(&params.codec)?;
        // Canvas: t2v snaps the request (or the default) to 16; i2v snaps to
        // the 32-grid the vision tower needs and, when no canvas was asked
        // for, derives it from the keyframe's aspect so the first frame is
        // not distorted.
        let (width, height) = match &input_rgb {
            None => (
                snap_dim(params.width.unwrap_or(DEFAULT_WIDTH)),
                snap_dim(params.height.unwrap_or(DEFAULT_HEIGHT)),
            ),
            Some((_, kf_w, kf_h)) => match (params.width, params.height) {
                (Some(w), Some(h)) => (snap_dim32(w), snap_dim32(h)),
                _ => derive_canvas_for_keyframe(*kf_w, *kf_h),
            },
        };
        let job = VideoJob {
            prompt: params.prompt.trim().to_string(),
            width,
            height,
            frames: params.frames.unwrap_or(DEFAULT_FRAMES).max(5),
            steps: params.steps.unwrap_or(DEFAULT_STEPS).clamp(2, 100),
            seed: params.seed,
            input_rgb,
            audio: params.audio.unwrap_or(true),
        };
        check_canvas_within_tier(
            &self.model_id,
            self.tier_limit_pixel_frames,
            job.width,
            job.height,
            job.frames,
        )?;

        let interpolate = match params.interpolate {
            None | Some(1) => 1,
            Some(factor @ (2 | 4)) => factor,
            Some(other) => {
                return Err(AssetAiError::Params(format!(
                    "minimax-h3: interpolate must be 1 (off), 2 or 4, got {other}"
                )))
            }
        };

        cancel.check()?;
        progress("starting", 0.0);
        let mut clip = match &mut self.gen {
            Gen::Stub(gen) => gen(&job, progress, cancel)?,
            #[cfg(feature = "video")]
            Gen::H3(gen) => gen.generate(&job, progress, cancel)?,
        };
        // A cancel raised during the generator's final phase (or by a stub)
        // unwinds before the mux writes anything.
        cancel.check()?;
        let expected = clip.num_frames * clip.height * clip.width * 3;
        if clip.frames_rgb8.len() != expected || clip.num_frames == 0 {
            return Err(AssetAiError::Backend(format!(
                "generator returned {} frame bytes, expected {} ({} frames {}x{})",
                clip.frames_rgb8.len(),
                expected,
                clip.num_frames,
                clip.width,
                clip.height
            )));
        }

        // Optional RIFE post-stage: same wall-clock duration, `interpolate`
        // times as many frames, so the mux fps scales with it. The audio
        // track is untouched — it was never resampled against the frame
        // cadence.
        let fps = if interpolate > 1 {
            interpolate_clip(
                &mut clip,
                interpolate,
                self.interpolate_weights.as_deref(),
                progress,
                cancel,
            )?;
            H3_FPS * interpolate
        } else {
            H3_FPS
        };

        // Audio: split planar stereo, resample to the AAC-encoder rate,
        // interleave + quantize.
        progress("audio-resample", 0.93);
        let audio_i16 = match &clip.audio_planar {
            Some(planar) => {
                if planar.len() % 2 != 0 {
                    return Err(AssetAiError::Backend(format!(
                        "generator returned odd planar stereo length {}",
                        planar.len()
                    )));
                }
                let half = planar.len() / 2;
                let left = resample_channel(&planar[..half], clip.audio_rate, MUX_AUDIO_RATE);
                let right = resample_channel(&planar[half..], clip.audio_rate, MUX_AUDIO_RATE);
                Some(interleave_stereo_i16(&left, &right))
            }
            None => None,
        };

        cancel.check()?;
        progress("mux", 0.95);
        let input = MuxInput {
            width: clip.width as u32,
            height: clip.height as u32,
            fps,
            h264,
            frames_rgb8: clip.frames_rgb8,
            audio_i16,
        };
        let bytes = match &mut self.mux {
            Mux::Stub(mux) => mux(&input)?,
            #[cfg(feature = "video")]
            Mux::Platform => {
                let tmp_dir = self
                    .cache_dir
                    .clone()
                    .unwrap_or_else(std::env::temp_dir)
                    .join("tmp");
                h3_mux::mux_mp4(&input, &tmp_dir)?
            }
        };
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: "video/mp4",
            ext: "mp4",
            bytes,
        }])
    }
}

// ---------------------------------------------------------------------------
// RIFE frame-interpolation post-stage (feature `interpolate`)
// ---------------------------------------------------------------------------

/// Progress band the interpolation stage owns, between the generator's last
/// phase and `audio-resample`.
// Read by `expand_frames` below, whose only production caller lives behind
// `#[cfg(feature = "interpolate")]`; kept ungated itself (see that fn's
// doc) so the cadence law stays unit-tested without the feature.
#[cfg_attr(not(feature = "interpolate"), allow(dead_code))]
const INTERPOLATE_BASE: f64 = 0.91;
#[cfg_attr(not(feature = "interpolate"), allow(dead_code))]
const INTERPOLATE_SPAN: f64 = 0.02;

/// Expands `clip` in place from `n` frames at [`H3_FPS`] to `n * factor`
/// frames at `H3_FPS * factor`: identical duration, denser cadence.
///
/// Every consecutive pair contributes its leading frame plus one
/// intermediate per entry of `timesteps`. The final source frame is then
/// held for `factor` slots, which is what makes the count come out at
/// exactly `n * factor` — a naive expansion lands one frame short of the
/// original duration and would drift the video against the untouched audio
/// track.
///
/// `middle` is the interpolator; it is a parameter so this cadence law is
/// unit-tested without weights, a GPU, or the `interpolate` feature.
/// Cancellation is checked per frame pair.
#[cfg_attr(not(feature = "interpolate"), allow(dead_code))]
fn expand_frames(
    clip: &mut VideoClip,
    factor: u32,
    timesteps: &[f32],
    mut middle: impl FnMut(&[u8], &[u8], f32) -> Result<Vec<u8>, AssetAiError>,
    progress: ProgressSink,
    cancel: &CancelToken,
) -> Result<(), AssetAiError> {
    if factor < 2 || timesteps.len() + 1 != factor as usize {
        return Err(AssetAiError::Backend(format!(
            "interpolate: {} timesteps do not expand a clip by {factor}x",
            timesteps.len()
        )));
    }
    let frame_bytes = clip.width * clip.height * 3;
    let pairs = clip.num_frames.saturating_sub(1);
    let mut frames = Vec::with_capacity(clip.num_frames * factor as usize * frame_bytes);
    for index in 0..pairs {
        cancel.check()?;
        let first = &clip.frames_rgb8[index * frame_bytes..(index + 1) * frame_bytes];
        let second = &clip.frames_rgb8[(index + 1) * frame_bytes..(index + 2) * frame_bytes];
        frames.extend_from_slice(first);
        for &timestep in timesteps {
            let generated = middle(first, second, timestep)?;
            if generated.len() != frame_bytes {
                return Err(AssetAiError::Backend(format!(
                    "interpolate returned {} bytes, expected {frame_bytes}",
                    generated.len()
                )));
            }
            frames.extend_from_slice(&generated);
        }
        progress(
            &format!("interpolate {}/{}", index + 1, pairs),
            INTERPOLATE_BASE + INTERPOLATE_SPAN * (index + 1) as f64 / pairs as f64,
        );
    }
    let last = &clip.frames_rgb8[(clip.num_frames - 1) * frame_bytes..];
    for _ in 0..factor {
        frames.extend_from_slice(last);
    }
    clip.frames_rgb8 = frames;
    clip.num_frames *= factor as usize;
    debug_assert_eq!(clip.frames_rgb8.len(), clip.num_frames * frame_bytes);
    Ok(())
}

/// Loads the pinned RIFE flownet and runs [`expand_frames`] with it. A bad
/// manifest, a missing flownet, or a box without CUDA all fail loudly rather
/// than silently returning the 24 fps clip.
#[cfg(feature = "interpolate")]
fn interpolate_clip(
    clip: &mut VideoClip,
    factor: u32,
    weights_path: Option<&std::path::Path>,
    progress: ProgressSink,
    cancel: &CancelToken,
) -> Result<(), AssetAiError> {
    use makepad_ai_common::DiffusionError;
    use makepad_ai_rife::{interpolation_timesteps, Rife, RifeFramePair, RifeWeights};

    fn rife_err(err: DiffusionError) -> AssetAiError {
        match err {
            DiffusionError::Cancelled => AssetAiError::Cancelled,
            other => AssetAiError::Backend(format!("interpolate: {other}")),
        }
    }

    let path = weights_path.ok_or_else(|| {
        AssetAiError::Backend(format!(
            "interpolate={factor} requested but this model manifest carries no \
             {ROLE_INTERPOLATE:?} file role"
        ))
    })?;
    if !path.is_file() {
        return Err(AssetAiError::Backend(format!(
            "interpolate={factor}: RIFE flownet missing at {} — pull the model first",
            path.display()
        )));
    }
    cancel.check()?;
    progress("interpolate load", INTERPOLATE_BASE);
    let weights = RifeWeights::load(path).map_err(rife_err)?;
    let is_cancelled = || cancel.is_cancelled();
    let rife = Rife::prepare_controlled(&weights, Default::default(), Some(&is_cancelled), None)
        .map_err(rife_err)?;
    let timesteps = interpolation_timesteps(factor);
    let (width, height) = (clip.width, clip.height);
    expand_frames(
        clip,
        factor,
        &timesteps,
        |first, second, timestep| {
            let pair = RifeFramePair::new(first, second, width, height).map_err(rife_err)?;
            rife.interpolate_rgb8_controlled(pair, timestep, Some(&is_cancelled))
                .map_err(rife_err)
        },
        progress,
        cancel,
    )
}

/// Without the feature the request is refused rather than silently ignored.
#[cfg(not(feature = "interpolate"))]
fn interpolate_clip(
    _clip: &mut VideoClip,
    factor: u32,
    _weights_path: Option<&std::path::Path>,
    _progress: ProgressSink,
    _cancel: &CancelToken,
) -> Result<(), AssetAiError> {
    Err(AssetAiError::Unavailable(format!(
        "interpolate={factor}: this build has no frame-interpolation support \
         (feature \"interpolate\" is off)"
    )))
}

// ---------------------------------------------------------------------------
// Real generation through libs/diffusion (feature video)
// ---------------------------------------------------------------------------

#[cfg(feature = "video")]
mod h3_gen {
    use super::{stage_from_phase, VideoClip, VideoJob};
    use crate::backend::{BackendCtx, CancelToken, ProgressSink};
    use crate::error::AssetAiError;
    use makepad_ai_h3::h3_pipeline::{
        h3_generate_with_control, H3CondCache, H3ComponentFile, H3GenerateParams, H3KeyframeInput,
        H3ModelSet, H3RunControl, H3WeightFormat,
    };
    use makepad_ai_h3::h3_tokenizer::H3Tokenizer;
    use makepad_ai_common::DiffusionError;
    use std::path::PathBuf;

    pub struct H3Gen {
        models_dir: Option<PathBuf>,
        /// Quantized-tier component sources resolved from the registry roles
        /// (None for the bf16 tree).
        model_set: Option<H3ModelSet>,
        /// Sequential component residency for the 24/32GB tiers.
        staged_residency: bool,
        tokenizer: Option<H3Tokenizer>,
        /// Warm conditioning across jobs (host-side, Send — the heavy DiT/VAE
        /// device weight caches already persist on the worker thread). A
        /// repeat prompt at the same canvas skips the whole TE phase: the TE
        /// namespace (~66.7GB) otherwise re-streams from disk per job and is
        /// evicted after — measured ~42s of a 46s repeat job on the 96GB box.
        /// Entries mutate only after a successful encode. Pipeline errors
        /// retire device residency immediately; the service-level unload
        /// then clears this host cache before the next job.
        cond_cache: H3CondCache,
        /// Device residency only. Tokenizer/conditioning state is deliberately
        /// excluded: staged 24/32 GB tiers keep those small host caches while
        /// still requiring a fresh VRAM admission check on every job.
        runtime: super::H3RuntimeLifecycle,
    }

    impl H3Gen {
        pub fn new() -> Self {
            Self {
                models_dir: None,
                model_set: None,
                staged_residency: false,
                tokenizer: None,
                cond_cache: H3CondCache::default(),
                runtime: super::H3RuntimeLifecycle::default(),
            }
        }

        pub fn ensure_loaded(
            &mut self,
            ctx: &mut BackendCtx,
            plan: &super::H3TierPlan,
        ) -> Result<(), AssetAiError> {
            // The validated H3 numerics config is the f32 spine: f16-accumulate
            // gemms saturate H3's >1e4 activation outliers into NaN. Pin the
            // knob once unless the operator explicitly set it. (Holds for the
            // quantized tiers too: their weights dequantize into bf16 and run
            // the same f32-accumulate gemms.)
            if std::env::var_os("FLUX_GEMM_F16ACC").is_none() {
                std::env::set_var("FLUX_GEMM_F16ACC", "0");
            }
            // Downloads any missing registry files (bf16: 61 files ~134 GiB,
            // usually pre-seeded/junctioned; quant tiers: the pinned GGUF or
            // NVFP4 set, ~30-35 GiB).
            ctx.ensure_files()?;
            let (models_dir, model_set) = match plan.kind {
                super::H3TierKind::Bf16Tree => {
                    // The registry lays the diffusers tree out under the dir
                    // that holds model_index.json.
                    let index = ctx
                        .spec
                        .files
                        .iter()
                        .find(|file| {
                            file.cache_as.ends_with("/model_index.json")
                                || file.cache_as == "model_index.json"
                        })
                        .ok_or_else(|| {
                            AssetAiError::Backend(format!(
                                "model {}: registry lists no model_index.json to anchor the model dir",
                                ctx.spec.id
                            ))
                        })?;
                    let models_dir = index
                        .dest_path(ctx.cache_dir)
                        .parent()
                        .map(|p| p.to_path_buf())
                        .ok_or_else(|| {
                            AssetAiError::Backend("model_index.json has no parent dir".to_string())
                        })?;
                    (models_dir, None)
                }
                super::H3TierKind::GgufQ4 | super::H3TierKind::Nvfp4 => {
                    let role_path = |role: &str| -> Result<PathBuf, AssetAiError> {
                        let file = ctx.spec.file_by_role(role).ok_or_else(|| {
                            AssetAiError::Backend(format!(
                                "model {}: tier manifest lost its {role:?} role",
                                ctx.spec.id
                            ))
                        })?;
                        Ok(file.dest_path(ctx.cache_dir))
                    };
                    // The tokenizer anchors the tier's models_dir: generate()
                    // loads `<models_dir>/tokenizer/` exactly like the bf16
                    // tree, so the manifest lays tokenizer.json under
                    // `<tier root>/tokenizer/`.
                    let tokenizer_json = role_path(super::ROLE_TOKENIZER_JSON)?;
                    let models_dir = tokenizer_json
                        .parent()
                        .and_then(|tokenizer_dir| tokenizer_dir.parent())
                        .map(|p| p.to_path_buf())
                        .ok_or_else(|| {
                            AssetAiError::Backend(format!(
                                "model {}: tokenizer role path {:?} has no tier root",
                                ctx.spec.id, tokenizer_json
                            ))
                        })?;
                    let format = match plan.kind {
                        super::H3TierKind::GgufQ4 => H3WeightFormat::Gguf,
                        _ => H3WeightFormat::Nvfp4,
                    };
                    let (dit_role, te_role) = match plan.kind {
                        super::H3TierKind::GgufQ4 => {
                            (super::ROLE_DIT_GGUF, super::ROLE_TE_GGUF)
                        }
                        _ => (super::ROLE_DIT_NVFP4, super::ROLE_TE_NVFP4),
                    };
                    let audio_config = role_path(super::ROLE_AUDIO_VAE_CONFIG)?;
                    let audio_vae_dir = audio_config
                        .parent()
                        .map(|p| p.to_path_buf())
                        .ok_or_else(|| {
                            AssetAiError::Backend(format!(
                                "model {}: audio-vae-config path {:?} has no dir",
                                ctx.spec.id, audio_config
                            ))
                        })?;
                    let set = H3ModelSet {
                        dit: Some(H3ComponentFile {
                            path: role_path(dit_role)?,
                            format,
                        }),
                        text_encoder: Some(H3ComponentFile {
                            path: role_path(te_role)?,
                            format,
                        }),
                        video_vae_path: Some(role_path(super::ROLE_VIDEO_VAE)?),
                        audio_vae_dir: Some(audio_vae_dir),
                    };
                    (models_dir, Some(set))
                }
            };
            if self.models_dir.as_ref() != Some(&models_dir) {
                self.unload()?;
                self.models_dir = Some(models_dir);
            }
            self.model_set = model_set;
            self.staged_residency = plan.staged;
            Ok(())
        }

        pub fn is_resident(&self) -> bool {
            self.runtime.is_resident()
        }

        fn release_runtime(&mut self) -> Result<usize, AssetAiError> {
            self.runtime.release_with(|prefixes| {
                makepad_ai_common::backend::release_gpu_runtime_namespaces(prefixes)
                    .map_err(|error| AssetAiError::Backend(format!("h3 unload: {error}")))
            })
        }

        pub fn unload(&mut self) -> Result<(), AssetAiError> {
            self.release_runtime()?;
            // The tokenizer and conditioning cache are the only persistent
            // CPU state. Clear them after CUDA teardown so a failure remains
            // truthfully resident and can be retried.
            self.tokenizer = None;
            self.cond_cache = H3CondCache::default();
            Ok(())
        }

        pub fn generate(
            &mut self,
            job: &VideoJob,
            progress: ProgressSink,
            cancel: &CancelToken,
        ) -> Result<VideoClip, AssetAiError> {
            let models_dir = self
                .models_dir
                .clone()
                .ok_or_else(|| AssetAiError::Backend("h3 used before ensure_loaded".into()))?;
            if self.tokenizer.is_none() {
                self.tokenizer = Some(
                    H3Tokenizer::load(&models_dir.join("tokenizer"))
                        .map_err(|e| AssetAiError::Backend(format!("h3 tokenizer: {e}")))?,
                );
            }
            progress("tokenize", 0.005);
            let token_ids = self.tokenizer.as_ref().unwrap().encode(&job.prompt);
            if token_ids.is_empty() {
                return Err(AssetAiError::Backend(
                    "prompt tokenized to zero tokens".to_string(),
                ));
            }

            // fl2va keyframe: the decoded input image + the tokenized
            // "<Picture 1>: " label of its vision block.
            let keyframe = job.input_rgb.as_ref().map(|(rgb, w, h)| H3KeyframeInput {
                rgb: rgb.clone(),
                width: *w as usize,
                height: *h as usize,
                picture_label_ids: self
                    .tokenizer
                    .as_ref()
                    .unwrap()
                    .encode("<Picture 1>: "),
            });

            let params = H3GenerateParams {
                width: job.width as usize,
                height: job.height as usize,
                num_frames: job.frames as usize,
                num_inference_steps: job.steps as usize,
                token_ids,
                seed: job.seed,
                keyframe,
                video_noise_rows: None,
                audio_noise_rows: None,
                condition_rows_override: None,
                act16: false,
                decode_audio: job.audio,
                model_set: self.model_set.clone(),
                staged_residency: self.staged_residency,
            };
            // Structured phase channel -> the job's progress convention
            // ("denoise 23/50" + overall fraction); the human timing lines go
            // to the service log. Cancellation: the pipeline polls the hook
            // between denoise steps / VAE batch groups / phases.
            let mut on_phase = |phase: &str, done: usize, total: usize| {
                if let Some((stage, frac)) = stage_from_phase(phase, done, total) {
                    progress(&stage, frac);
                }
            };
            let cancelled = || cancel.is_cancelled();
            self.runtime.mark_touched();
            let result = {
                let mut ctrl = H3RunControl {
                    on_phase: Some(&mut on_phase),
                    cancel: Some(&cancelled),
                    cond_cache: Some(&mut self.cond_cache),
                };
                h3_generate_with_control(
                    &models_dir,
                    &params,
                    |line| println!("h3: {line}"),
                    &mut ctrl,
                )
            };
            let output = match result {
                Ok(output) => {
                    if self.staged_residency {
                        // The quant pipeline already retires each component
                        // at its phase boundary. Trim the remaining scratch
                        // pool and report the backend cold so the next job is
                        // admitted against fresh VRAM instead of tokenizer
                        // state masquerading as device residency.
                        self.release_runtime()?;
                    }
                    output
                }
                Err(error) => {
                    let was_cancelled = matches!(error, DiffusionError::Cancelled);
                    if !was_cancelled {
                        // Any real failure: drop warm conditioning rather
                        // than trust state from a broken run; the next job
                        // re-encodes.
                        self.cond_cache = H3CondCache::default();
                    }
                    // Pipeline failures can occur after only part of a
                    // namespace streamed. Clean it on this SAME worker thread
                    // immediately; the service-level unload is a second,
                    // idempotent safety net.
                    if let Err(cleanup) = self.release_runtime() {
                        return Err(AssetAiError::Backend(format!(
                            "h3 generate failed ({error}); CUDA cleanup failed: {cleanup}"
                        )));
                    }
                    if was_cancelled {
                        return Err(AssetAiError::Cancelled);
                    }
                    return Err(AssetAiError::Backend(format!("h3 generate: {error}")));
                }
            };

            Ok(VideoClip {
                width: output.width,
                height: output.height,
                num_frames: output.num_frames,
                frames_rgb8: output.frames_rgb8,
                audio_planar: output.audio_planar,
                audio_rate: output.audio_sample_rate,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Real mp4 mux through the platform hardware encoder (feature video)
// ---------------------------------------------------------------------------

#[cfg(feature = "video")]
mod h3_mux {
    use super::{MuxInput, MUX_AUDIO_RATE};
    use crate::error::AssetAiError;
    use makepad_video::{
        PcmAudioTrackOptions, VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions,
    };
    use std::path::Path;

    pub fn mux_mp4(input: &MuxInput, tmp_dir: &Path) -> Result<Vec<u8>, AssetAiError> {
        std::fs::create_dir_all(tmp_dir)
            .map_err(|e| AssetAiError::Io(format!("mux tmp dir {}: {e}", tmp_dir.display())))?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = tmp_dir.join(format!("h3-mux-{nanos}.mp4"));
        let path_text = path.to_string_lossy().into_owned();

        let options = VideoFileEncoderOptions {
            codec: if input.h264 {
                VideoFileCodec::H264
            } else {
                VideoFileCodec::H265
            },
            width: input.width,
            height: input.height,
            fps_num: input.fps,
            fps_den: 1,
            video_bitrate_bps: 8_000_000,
            audio: input.audio_i16.as_ref().map(|_| PcmAudioTrackOptions {
                sample_rate: MUX_AUDIO_RATE,
                channels: 2,
                aac_bitrate_bps: 128_000,
            }),
            ..Default::default()
        };
        let mux_err = |context: &str, e: makepad_video::VideoFileError| {
            AssetAiError::Backend(format!("mp4 mux {context}: {e}"))
        };
        let encode = || -> Result<Vec<u8>, AssetAiError> {
            let mut encoder =
                VideoFileEncoder::new(&path_text, options).map_err(|e| mux_err("open", e))?;
            if let Some(info) = encoder.video_transform() {
                println!(
                    "h3 mux: video transform {:?} (hardware: {})",
                    info.name, info.is_hardware
                );
            }
            let frame_bytes = input.width as usize * input.height as usize * 3;
            for frame in input.frames_rgb8.chunks_exact(frame_bytes) {
                encoder
                    .push_frame_rgb8(frame, None)
                    .map_err(|e| mux_err("frame", e))?;
            }
            if let Some(audio) = &input.audio_i16 {
                encoder
                    .push_audio_i16(audio)
                    .map_err(|e| mux_err("audio", e))?;
            }
            encoder.finish().map_err(|e| mux_err("finish", e))?;
            std::fs::read(&path)
                .map_err(|e| AssetAiError::Io(format!("mux read {}: {e}", path.display())))
        };
        // The temp file never outlives the call — success or failure.
        let result = encode();
        let _ = std::fs::remove_file(&path);
        result
    }
}

// ---------------------------------------------------------------------------
// Tests (stubbed generation + mux — this is what CI exercises)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GenerateRequestJson;

    fn video_params(request: GenerateRequestJson) -> GenerateParams {
        GenerateParams::from_request(&request).unwrap()
    }

    /// Deterministic tiny clip: 3 frames of 32x16 gradient + a 32 kHz stereo
    /// ramp so the resample/interleave path has real work.
    fn stub_clip(job: &VideoJob) -> VideoClip {
        let (w, h, f) = (job.width as usize, job.height as usize, 3usize);
        let mut frames = Vec::with_capacity(f * h * w * 3);
        for frame in 0..f {
            for y in 0..h {
                for x in 0..w {
                    frames.push((x * 8 + frame) as u8);
                    frames.push((y * 8) as u8);
                    frames.push(job.seed as u8);
                }
            }
        }
        // 3 frames @24fps = 0.125s => 4000 samples at 32k per channel.
        let samples = 4000usize;
        let mut planar = Vec::with_capacity(samples * 2);
        for i in 0..samples {
            planar.push((i as f32 / samples as f32) - 0.5);
        }
        for i in 0..samples {
            planar.push(0.5 - (i as f32 / samples as f32));
        }
        VideoClip {
            width: w,
            height: h,
            num_frames: f,
            frames_rgb8: frames,
            audio_planar: Some(planar),
            audio_rate: 32_000,
        }
    }

    #[test]
    fn stub_video_job_to_mp4_artifact() {
        let mut backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|job: &VideoJob, progress: ProgressSink, _c: &CancelToken| {
                // Defaults applied + snapping (33x17 requested -> 32x16).
                assert_eq!(job.prompt, "a red car");
                assert_eq!(job.width, 32);
                assert_eq!(job.height, 64);
                assert_eq!(job.frames, 124);
                assert_eq!(job.steps, 50);
                assert_eq!(job.seed, 7);
                progress("denoise", 0.5);
                Ok(stub_clip(job))
            }),
            Box::new(|input: &MuxInput| {
                assert_eq!(input.width, 32);
                assert_eq!(input.height, 64);
                assert_eq!(input.fps, H3_FPS);
                assert!(!input.h264);
                assert_eq!(input.frames_rgb8.len(), 3 * 32 * 64 * 3);
                // 4000 samples 32k -> 6000 at 48k, interleaved stereo.
                let audio = input.audio_i16.as_ref().unwrap();
                assert_eq!(audio.len(), 6000 * 2);
                Ok(b"MP4STUB".to_vec())
            }),
        );
        let params = video_params(GenerateRequestJson {
            model: "minimax-h3".to_string(),
            prompt: Some("a red car".to_string()),
            width: Some(33),
            height: Some(65),
            seed: Some(7),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend.generate(&params, &mut sink, &CancelToken::new()).unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "video/mp4");
        assert_eq!(artifacts[0].ext, "mp4");
        assert_eq!(artifacts[0].bytes, b"MP4STUB");
    }

    // -- interpolation post-stage -----------------------------------------

    /// The cadence law: `n` frames become exactly `n * factor`, the source
    /// frames stay in place at every `factor`-th slot, the intermediates
    /// arrive in timestep order, and the tail is held so the clip keeps its
    /// original duration against the untouched audio track.
    #[test]
    fn expand_frames_keeps_the_duration_and_the_source_frames() {
        let (w, h) = (2usize, 1usize);
        let frame_bytes = w * h * 3;
        let source: Vec<u8> = (0..3u8)
            .flat_map(|f| std::iter::repeat(f * 10).take(frame_bytes))
            .collect();
        let mut clip = VideoClip {
            width: w,
            height: h,
            num_frames: 3,
            frames_rgb8: source.clone(),
            audio_planar: None,
            audio_rate: 32_000,
        };
        let mut sink = |_: &str, _: f64| {};
        expand_frames(
            &mut clip,
            2,
            &[0.5],
            |first, second, timestep| {
                assert_eq!(timestep, 0.5);
                Ok(first
                    .iter()
                    .zip(second)
                    .map(|(a, b)| ((u16::from(*a) + u16::from(*b)) / 2) as u8)
                    .collect())
            },
            &mut sink,
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(clip.num_frames, 6);
        assert_eq!(clip.frames_rgb8.len(), 6 * frame_bytes);
        let frame = |index: usize| clip.frames_rgb8[index * frame_bytes];
        // 0, mid(0,10), 10, mid(10,20), 20, 20 (held tail).
        assert_eq!(
            (0..6).map(frame).collect::<Vec<_>>(),
            vec![0, 5, 10, 15, 20, 20]
        );
    }

    #[test]
    fn expand_frames_emits_every_timestep_for_4x() {
        let frame_bytes = 3;
        let mut clip = VideoClip {
            width: 1,
            height: 1,
            num_frames: 2,
            frames_rgb8: vec![0, 0, 0, 40, 40, 40],
            audio_planar: None,
            audio_rate: 32_000,
        };
        let mut seen = Vec::new();
        let mut sink = |_: &str, _: f64| {};
        expand_frames(
            &mut clip,
            4,
            &[0.25, 0.5, 0.75],
            |_, second, timestep| {
                seen.push(timestep);
                Ok(vec![(f32::from(second[0]) * timestep) as u8; frame_bytes])
            },
            &mut sink,
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(seen, vec![0.25, 0.5, 0.75]);
        assert_eq!(clip.num_frames, 8);
        assert_eq!(
            (0..8).map(|i| clip.frames_rgb8[i * frame_bytes]).collect::<Vec<_>>(),
            vec![0, 10, 20, 30, 40, 40, 40, 40]
        );
    }

    #[test]
    fn expand_frames_refuses_a_timestep_list_that_does_not_match_the_factor() {
        let mut clip = VideoClip {
            width: 1,
            height: 1,
            num_frames: 2,
            frames_rgb8: vec![0; 6],
            audio_planar: None,
            audio_rate: 32_000,
        };
        let mut sink = |_: &str, _: f64| {};
        let result = expand_frames(
            &mut clip,
            4,
            &[0.5],
            |_, _, _| Ok(vec![0; 3]),
            &mut sink,
            &CancelToken::new(),
        );
        assert!(matches!(result, Err(AssetAiError::Backend(_))));
    }

    /// A factor the interpolator cannot honor is refused at parameter
    /// parsing, before any generation happens.
    #[test]
    fn interpolate_factor_is_validated() {
        for (value, ok) in [(None, true), (Some(1), true), (Some(2), true), (Some(4), true)] {
            let request = GenerateRequestJson {
                model: "minimax-h3".to_string(),
                prompt: Some("p".to_string()),
                interpolate: value,
                ..GenerateRequestJson::default()
            };
            assert_eq!(GenerateParams::from_request(&request).is_ok(), ok);
        }
        for bad in [0u32, 3, 5, 8] {
            let request = GenerateRequestJson {
                model: "minimax-h3".to_string(),
                prompt: Some("p".to_string()),
                interpolate: Some(bad),
                ..GenerateRequestJson::default()
            };
            match GenerateParams::from_request(&request) {
                Err(AssetAiError::Params(message)) => {
                    assert!(message.contains("interpolate"), "{message}")
                }
                other => panic!("interpolate={bad} should be a Params error, got {other:?}"),
            }
        }
        // `1` and absent both mean "off" — the backend must see None.
        let off = GenerateRequestJson {
            model: "minimax-h3".to_string(),
            prompt: Some("p".to_string()),
            interpolate: Some(1),
            ..GenerateRequestJson::default()
        };
        assert_eq!(GenerateParams::from_request(&off).unwrap().interpolate, None);
    }

    /// Off by default: the mux still gets H3's native 24 fps.
    #[test]
    fn no_interpolation_keeps_the_native_frame_rate() {
        let mut backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|job: &VideoJob, _p: ProgressSink, _c: &CancelToken| Ok(stub_clip(job))),
            Box::new(|input: &MuxInput| {
                assert_eq!(input.fps, H3_FPS);
                assert_eq!(input.frames_rgb8.len(), 3 * 32 * 16 * 3);
                Ok(vec![1])
            }),
        );
        let params = video_params(GenerateRequestJson {
            model: "minimax-h3".to_string(),
            prompt: Some("p".to_string()),
            width: Some(32),
            height: Some(16),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        backend.generate(&params, &mut sink, &CancelToken::new()).unwrap();
    }

    /// An interpolated job on a backend whose manifest carries no
    /// [`ROLE_INTERPOLATE`] file fails loudly instead of quietly muxing
    /// 24 fps — the whole point of the request would be lost otherwise.
    #[test]
    fn interpolation_without_the_flownet_role_fails_loudly() {
        let mut backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|job: &VideoJob, _p: ProgressSink, _c: &CancelToken| Ok(stub_clip(job))),
            Box::new(|_: &MuxInput| panic!("the mux must never be reached")),
        );
        let params = video_params(GenerateRequestJson {
            model: "minimax-h3".to_string(),
            prompt: Some("p".to_string()),
            width: Some(32),
            height: Some(16),
            interpolate: Some(2),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        match backend.generate(&params, &mut sink, &CancelToken::new()) {
            Err(AssetAiError::Backend(message)) => {
                assert!(message.contains("interpolate"), "{message}")
            }
            Err(AssetAiError::Unavailable(message)) => {
                // Build without the `interpolate` feature.
                assert!(message.contains("interpolate"), "{message}")
            }
            Err(other) => panic!("expected a loud interpolation failure, got {other:?}"),
            Ok(_) => panic!("expected a loud interpolation failure, got artifacts"),
        }
    }

    /// `audio` defaults to true (the job wants the jointly-denoised audio
    /// decoded); an explicit `audio: false` request reaches the generator as
    /// `job.audio == false` so the real H3 pipeline can skip the audio VAE
    /// decode + AAC mux (see `H3GenerateParams::decode_audio` in h3_pipeline).
    #[test]
    fn audio_request_field_reaches_the_job() {
        let mut backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|job: &VideoJob, _p: ProgressSink, _c: &CancelToken| {
                assert!(job.audio, "audio omitted from the request must default to true");
                Ok(stub_clip(job))
            }),
            Box::new(|_: &MuxInput| Ok(vec![1])),
        );
        let params = video_params(GenerateRequestJson {
            model: "minimax-h3".to_string(),
            prompt: Some("p".to_string()),
            width: Some(32),
            height: Some(16),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        backend.generate(&params, &mut sink, &CancelToken::new()).unwrap();

        let mut backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|job: &VideoJob, _p: ProgressSink, _c: &CancelToken| {
                assert!(!job.audio, "audio: Some(false) must reach the job as false");
                // The real pipeline (decode_audio: false) never populates
                // audio_planar; the stub mirrors that.
                let mut clip = stub_clip(job);
                clip.audio_planar = None;
                Ok(clip)
            }),
            Box::new(|input: &MuxInput| {
                assert!(input.audio_i16.is_none(), "video-only request must mux no audio track");
                Ok(vec![1])
            }),
        );
        let params = video_params(GenerateRequestJson {
            model: "minimax-h3".to_string(),
            prompt: Some("p".to_string()),
            width: Some(32),
            height: Some(16),
            audio: Some(false),
            ..GenerateRequestJson::default()
        });
        backend.generate(&params, &mut sink, &CancelToken::new()).unwrap();
    }

    #[test]
    fn silent_clip_has_no_audio_track() {
        let mut backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|job: &VideoJob, _p: ProgressSink, _c: &CancelToken| {
                let mut clip = stub_clip(job);
                clip.audio_planar = None;
                Ok(clip)
            }),
            Box::new(|input: &MuxInput| {
                assert!(input.audio_i16.is_none());
                Ok(vec![1])
            }),
        );
        let params = video_params(GenerateRequestJson {
            model: "minimax-h3".to_string(),
            prompt: Some("p".to_string()),
            width: Some(32),
            height: Some(16),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        assert_eq!(backend.generate(&params, &mut sink, &CancelToken::new()).unwrap().len(), 1);
    }

    #[test]
    fn codec_flag_and_validation() {
        assert!(!wants_h264("").unwrap());
        assert!(!wants_h264("h265").unwrap());
        assert!(!wants_h264("HEVC").unwrap());
        assert!(wants_h264("h264").unwrap());
        assert!(wants_h264("vp9").is_err());

        let mut backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|job: &VideoJob, _p: ProgressSink, _c: &CancelToken| Ok(stub_clip(job))),
            Box::new(|input: &MuxInput| {
                assert!(input.h264);
                Ok(vec![1])
            }),
        );
        let params = video_params(GenerateRequestJson {
            model: "minimax-h3".to_string(),
            prompt: Some("p".to_string()),
            width: Some(32),
            height: Some(16),
            codec: Some("h264".to_string()),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        backend.generate(&params, &mut sink, &CancelToken::new()).unwrap();

        // Empty prompt rejected before generation.
        let params = video_params(GenerateRequestJson {
            model: "minimax-h3".to_string(),
            prompt: Some("   ".to_string()),
            ..GenerateRequestJson::default()
        });
        let mut backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|_: &VideoJob, _p: ProgressSink, _c: &CancelToken| unreachable!()),
            Box::new(|_: &MuxInput| unreachable!()),
        );
        assert!(backend.generate(&params, &mut sink, &CancelToken::new()).is_err());
    }

    #[test]
    fn frame_size_mismatch_rejected() {
        let mut backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|job: &VideoJob, _p: ProgressSink, _c: &CancelToken| {
                let mut clip = stub_clip(job);
                clip.frames_rgb8.pop();
                Ok(clip)
            }),
            Box::new(|_: &MuxInput| unreachable!()),
        );
        let params = video_params(GenerateRequestJson {
            model: "minimax-h3".to_string(),
            prompt: Some("p".to_string()),
            width: Some(32),
            height: Some(16),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        assert!(backend.generate(&params, &mut sink, &CancelToken::new()).is_err());
    }

    #[test]
    fn runtime_success_then_unload_releases_exact_h3_namespaces() {
        let mut runtime = H3RuntimeLifecycle::default();
        assert!(!runtime.is_resident());

        // A successful bf16 pipeline leaves its compute namespaces warm.
        runtime.mark_touched();
        assert!(runtime.is_resident());

        let mut seen = Vec::<String>::new();
        let released = runtime
            .release_with(|prefixes| {
                seen.extend(prefixes.iter().map(|prefix| (*prefix).to_string()));
                Ok(37)
            })
            .unwrap();
        assert_eq!(released, 37);
        assert_eq!(seen, ["h3te::", "h3dit::", "h3vae::"]);
        assert!(!runtime.is_resident());

        // Explicit unload is idempotent and must not initialize or revisit a
        // cold CUDA runtime.
        runtime
            .release_with(|_| panic!("cold unload must not invoke CUDA release"))
            .unwrap();
    }

    #[test]
    fn failed_runtime_release_stays_truthfully_resident_until_retry() {
        let mut runtime = H3RuntimeLifecycle::default();
        runtime.mark_touched();
        let error = runtime
            .release_with(|_| Err(AssetAiError::Backend("synthetic CUDA teardown error".into())))
            .unwrap_err();
        assert!(error.to_string().contains("synthetic CUDA teardown error"));
        assert!(
            runtime.is_resident(),
            "a failed release must not advertise leaked allocations as cold"
        );

        runtime.release_with(|_| Ok(3)).unwrap();
        assert!(!runtime.is_resident());
    }

    #[test]
    fn every_h3_job_error_requests_retirement_including_cancel() {
        let backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|_, _, _| unreachable!()),
            Box::new(|_| unreachable!()),
        );
        assert!(!backend.resident_is_healthy_after_error(&AssetAiError::Cancelled));
        assert!(!backend.resident_is_healthy_after_error(&AssetAiError::Backend(
            "synthetic failure".into()
        )));
    }

    fn tier_spec(id: &str, roles: &[&str]) -> crate::registry::ModelSpec {
        use crate::registry::{Domain, FileSpec, ModelSpec};
        let files = roles
            .iter()
            .map(|role| FileSpec {
                role: Some(role.to_string()),
                repo: "org/repo".into(),
                path: format!("{role}.bin"),
                revision: None,
                cache_as: format!("video/tiers/{role}.bin"),
                size: None,
                sha256: None,
                local: false,
                converts_to: None,
                conversion: None,
            })
            .collect();
        ModelSpec {
            id: id.to_string(),
            domain: Domain::Video,
            backend: "h3".to_string(),
            available: true,
            gated: false,
            vram_gb: None,
            min_vram_gb: None,
            min_compute_cap: None,
            note: None,
            license: None,
            files,
        }
    }

    /// Tier selection is data-driven from file roles and fails closed on
    /// contradictory or incomplete quantized manifests.
    #[test]
    fn tier_plan_selection_and_fail_closed() {
        // No quant roles: the legacy bf16 tree, unstaged, no ceiling.
        let plan = tier_plan_for_spec(&tier_spec("bf16", &[])).unwrap();
        assert_eq!(plan.kind, H3TierKind::Bf16Tree);
        assert!(!plan.staged && plan.max_pixel_frames.is_none());

        // Complete GGUF manifest.
        let plan = tier_plan_for_spec(&tier_spec(
            "q4",
            &[
                ROLE_DIT_GGUF,
                ROLE_TE_GGUF,
                ROLE_VIDEO_VAE,
                ROLE_AUDIO_VAE,
                ROLE_AUDIO_VAE_CONFIG,
                ROLE_TOKENIZER_JSON,
            ],
        ))
        .unwrap();
        assert_eq!(plan.kind, H3TierKind::GgufQ4);
        assert!(plan.staged);
        assert_eq!(plan.max_pixel_frames, Some(960 * 544 * 124));

        // Complete NVFP4 manifest gets the bigger measured ceiling.
        let plan = tier_plan_for_spec(&tier_spec(
            "nv4",
            &[
                ROLE_DIT_NVFP4,
                ROLE_TE_NVFP4,
                ROLE_VIDEO_VAE,
                ROLE_AUDIO_VAE,
                ROLE_AUDIO_VAE_CONFIG,
                ROLE_TOKENIZER_JSON,
            ],
        ))
        .unwrap();
        assert_eq!(plan.kind, H3TierKind::Nvfp4);
        assert_eq!(plan.max_pixel_frames, Some(1344 * 768 * 124));

        // Incomplete manifests refuse instead of falling back to bf16.
        for missing in [
            ROLE_TE_GGUF,
            ROLE_VIDEO_VAE,
            ROLE_AUDIO_VAE,
            ROLE_AUDIO_VAE_CONFIG,
            ROLE_TOKENIZER_JSON,
        ] {
            let roles: Vec<&str> = [
                ROLE_DIT_GGUF,
                ROLE_TE_GGUF,
                ROLE_VIDEO_VAE,
                ROLE_AUDIO_VAE,
                ROLE_AUDIO_VAE_CONFIG,
                ROLE_TOKENIZER_JSON,
            ]
            .into_iter()
            .filter(|role| *role != missing)
            .collect();
            let err = tier_plan_for_spec(&tier_spec("broken", &roles)).unwrap_err();
            assert!(err.to_string().contains(missing), "{missing}: {err}");
        }
        // A GGUF-tier TE cannot satisfy an NVFP4 manifest (same-family rule).
        let err = tier_plan_for_spec(&tier_spec(
            "mixed-te",
            &[
                ROLE_DIT_NVFP4,
                ROLE_TE_GGUF,
                ROLE_VIDEO_VAE,
                ROLE_AUDIO_VAE,
                ROLE_AUDIO_VAE_CONFIG,
                ROLE_TOKENIZER_JSON,
            ],
        ))
        .unwrap_err();
        assert!(err.to_string().contains(ROLE_TE_NVFP4), "{err}");
        // Two DiTs in one manifest is a registry bug, not a choice.
        assert!(tier_plan_for_spec(&tier_spec("both", &[ROLE_DIT_GGUF, ROLE_DIT_NVFP4])).is_err());
    }

    /// The GPU gate refuses below-spec AND unknown GPUs (fail closed) and
    /// ignores models without hard requirements.
    #[test]
    fn gpu_gate_fails_closed() {
        use crate::gpu::GpuInfo;
        let gpu = |vram: Option<u64>, cap: Option<f64>| GpuInfo {
            name: Some("test".into()),
            vram_free_mb: None,
            vram_total_mb: vram,
            compute_cap: cap,
        };
        // No requirements: any GPU, even unknown, passes (legacy models).
        check_gpu_requirements("legacy", None, None, &GpuInfo::default()).unwrap();
        // 4090-class box: q4 tier passes, nvfp4 (cap) and bf16 (vram) refuse.
        let rtx4090 = gpu(Some(24_564), Some(8.9));
        check_gpu_requirements("q4", Some(22.0), Some(8.9), &rtx4090).unwrap();
        assert!(check_gpu_requirements("nv4", Some(30.0), Some(12.0), &rtx4090).is_err());
        assert!(check_gpu_requirements("bf16", Some(90.0), None, &rtx4090).is_err());
        // 5090-class box: everything up to nvfp4 passes, bf16 refuses.
        let rtx5090 = gpu(Some(32_607), Some(12.0));
        check_gpu_requirements("q4", Some(22.0), Some(8.9), &rtx5090).unwrap();
        check_gpu_requirements("nv4", Some(30.0), Some(12.0), &rtx5090).unwrap();
        assert!(check_gpu_requirements("bf16", Some(90.0), None, &rtx5090).is_err());
        // 96GB box passes bf16.
        check_gpu_requirements("bf16", Some(90.0), Some(8.9), &gpu(Some(97_887), Some(12.0)))
            .unwrap();
        // UNKNOWN GPU with any requirement: refuse — never thrash-and-see.
        assert!(check_gpu_requirements("q4", Some(22.0), None, &GpuInfo::default()).is_err());
        assert!(check_gpu_requirements("nv4", None, Some(12.0), &GpuInfo::default()).is_err());
        let vram_only = gpu(Some(32_607), None);
        assert!(check_gpu_requirements("nv4", Some(30.0), Some(12.0), &vram_only).is_err());
    }

    /// Canvas requests above a tier's measured ceiling are refused with a
    /// Params error (fail closed instead of WDDM thrash).
    #[test]
    fn canvas_ceiling_gate() {
        // No limit (bf16 tier): anything goes.
        check_canvas_within_tier("bf16", None, 1344, 768, 243).unwrap();
        let q4 = Some(960u64 * 544 * 124);
        check_canvas_within_tier("q4", q4, 960, 544, 124).unwrap();
        check_canvas_within_tier("q4", q4, 640, 352, 124).unwrap();
        match check_canvas_within_tier("q4", q4, 1344, 768, 124) {
            Err(AssetAiError::Params(message)) => {
                assert!(message.contains("1344x768"), "{message}");
            }
            other => panic!("expected Params error, got {other:?}"),
        }
        // Frame count participates in the budget, not just the canvas.
        assert!(check_canvas_within_tier("q4", q4, 960, 544, 243).is_err());
    }

    #[test]
    fn dim_snapping() {
        assert_eq!(snap_dim(640), 640);
        assert_eq!(snap_dim(650), 656);
        assert_eq!(snap_dim(1), 16);
        assert_eq!(snap_dim(360), 368);
        assert_eq!(snap_dim(352), 352);
    }

    /// Real platform encode smoke (feature video + H3_MUX_SMOKE=1): drives the
    /// actual `VideoFileEncoder` (AVFoundation on macOS, Media Foundation on
    /// Windows) with a tiny synthetic clip + tone and checks we get an mp4.
    #[cfg(feature = "video")]
    #[test]
    fn real_mux_smoke() {
        if std::env::var("H3_MUX_SMOKE").map(|v| v != "1").unwrap_or(true) {
            return;
        }
        let (w, h, f) = (64usize, 64usize, 24usize);
        let mut frames = Vec::with_capacity(f * h * w * 3);
        for frame in 0..f {
            for y in 0..h {
                for x in 0..w {
                    frames.push((x * 4 + frame * 8) as u8);
                    frames.push((y * 4) as u8);
                    frames.push(128);
                }
            }
        }
        // 1 second of 440 Hz tone at 48k stereo.
        let mut audio = Vec::with_capacity(48_000 * 2);
        for n in 0..48_000 {
            let s = ((2.0 * std::f64::consts::PI * 440.0 * n as f64 / 48_000.0).sin() * 0.3
                * 32767.0) as i16;
            audio.push(s);
            audio.push(s);
        }
        let input = MuxInput {
            width: w as u32,
            height: h as u32,
            fps: H3_FPS,
            h264: false,
            frames_rgb8: frames,
            audio_i16: Some(audio),
        };
        let tmp = std::env::temp_dir().join("h3_mux_smoke");
        let bytes = super::h3_mux::mux_mp4(&input, &tmp).expect("mux failed");
        assert!(bytes.len() > 1000, "mp4 too small: {}", bytes.len());
        // "ftyp" box within the first bytes = an mp4 container.
        assert_eq!(&bytes[4..8], b"ftyp", "not an mp4: {:?}", &bytes[..12]);
        println!("real_mux_smoke: {} byte mp4", bytes.len());
    }

    fn b64(bytes: &[u8]) -> String {
        String::from_utf8(makepad_base64::base64_encode(
            bytes,
            &makepad_base64::BASE64_STANDARD,
        ))
        .unwrap()
    }

    /// An input image reaches the generator as a DECODED keyframe — the i2v
    /// request must never silently degrade to t2v. With no canvas requested,
    /// it is derived from the keyframe's aspect (here square -> 352x352).
    #[test]
    fn input_image_flows_to_generator_as_keyframe() {
        let mut backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|job: &VideoJob, _p: ProgressSink, _c: &CancelToken| {
                let (rgb, w, h) = job.input_rgb.as_ref().expect("keyframe must flow");
                assert_eq!((*w, *h), (8, 4));
                assert_eq!(rgb.len(), 8 * 4 * 3);
                // Canvas derived from the keyframe: 32-multiples, wide.
                assert_eq!(job.width % 32, 0);
                assert_eq!(job.height % 32, 0);
                assert!(job.width > job.height);
                Ok(stub_clip(job))
            }),
            Box::new(|_: &MuxInput| Ok(vec![1])),
        );
        // 8x4 gradient png via the testpattern encoder.
        let mut rgba = Vec::new();
        for y in 0..4usize {
            for x in 0..8usize {
                rgba.extend_from_slice(&[(x * 16) as u8, (y * 16) as u8, 128, 255]);
            }
        }
        let png = crate::testpattern::encode_png_rgba(&rgba, 8, 4).unwrap();
        let params = video_params(GenerateRequestJson {
            model: "minimax-h3".to_string(),
            prompt: Some("a boat sails away".to_string()),
            input_b64: Some(b64(&png)),
            input_content_type: Some("image/png".to_string()),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
    }

    /// Undecodable image input is a loud Params error, not a silent t2v run.
    #[test]
    fn bad_input_image_is_a_params_error() {
        let mut backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|_: &VideoJob, _p: ProgressSink, _c: &CancelToken| {
                unreachable!("generator must not run for an undecodable image")
            }),
            Box::new(|_: &MuxInput| unreachable!()),
        );
        let mut sink = |_: &str, _: f64| {};
        for (bytes, content_type) in [
            (b"\x89PNG fake".to_vec(), Some("image/png".to_string())),
            (b"whatever".to_vec(), Some("image/jpeg".to_string())),
        ] {
            let params = video_params(GenerateRequestJson {
                model: "minimax-h3".to_string(),
                prompt: Some("a boat sails away".to_string()),
                input_b64: Some(b64(&bytes)),
                input_content_type: content_type,
                ..GenerateRequestJson::default()
            });
            match backend.generate(&params, &mut sink, &CancelToken::new()) {
                Err(AssetAiError::Params(_)) => {}
                Err(other) => panic!("expected Params error, got {other:?}"),
                Ok(_) => panic!("expected Params error, got artifacts"),
            }
        }
    }

    #[test]
    fn i2v_canvas_snap_and_derivation() {
        assert_eq!(snap_dim32(640), 640);
        assert_eq!(snap_dim32(352), 352);
        assert_eq!(snap_dim32(368), 384);
        assert_eq!(snap_dim32(1), 32);
        // Square keyframe -> square canvas at the short-edge budget.
        assert_eq!(derive_canvas_for_keyframe(512, 512), (352, 352));
        // 16:9-ish keyframe stays within the 640x352 pixel budget.
        let (w, h) = derive_canvas_for_keyframe(960, 544);
        assert!(w % 32 == 0 && h % 32 == 0, "{w}x{h}");
        assert!(w as f64 * h as f64 <= 640.0 * 352.0 * 1.10, "{w}x{h}");
        let aspect_in = 960.0 / 544.0;
        let aspect_out = w as f64 / h as f64;
        assert!((aspect_in - aspect_out).abs() / aspect_in < 0.12, "{w}x{h}");
    }

    #[test]
    fn pre_raised_cancel_short_circuits_before_generation() {
        let mut backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|_: &VideoJob, _p: ProgressSink, _c: &CancelToken| {
                unreachable!("generator must not run for a cancelled job")
            }),
            Box::new(|_: &MuxInput| unreachable!()),
        );
        let params = video_params(GenerateRequestJson {
            model: "minimax-h3".to_string(),
            prompt: Some("p".to_string()),
            ..GenerateRequestJson::default()
        });
        let token = CancelToken::new();
        token.cancel();
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params, &mut sink, &token),
            Err(AssetAiError::Cancelled)
        ));
    }

    #[test]
    fn cancel_raised_mid_generation_skips_mux() {
        let mut backend = H3Backend::with_stubs(
            "minimax-h3",
            Box::new(|job: &VideoJob, _p: ProgressSink, c: &CancelToken| {
                // Simulates POST /job/<id>/cancel landing while the generator
                // runs: the flag is up by the time the clip returns.
                c.cancel();
                Ok(stub_clip(job))
            }),
            Box::new(|_: &MuxInput| unreachable!("mux must not run after cancel")),
        );
        let params = video_params(GenerateRequestJson {
            model: "minimax-h3".to_string(),
            prompt: Some("p".to_string()),
            width: Some(32),
            height: Some(16),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let token = CancelToken::new();
        assert!(matches!(
            backend.generate(&params, &mut sink, &token),
            Err(AssetAiError::Cancelled)
        ));
        assert!(token.is_cancelled());
    }

    #[test]
    fn phase_mapping_counts_and_fractions() {
        let (stage, frac) = stage_from_phase("denoise", 1, 50).unwrap();
        assert_eq!(stage, "denoise 1/50 (streaming weights)");
        assert!((frac - 0.15).abs() < 1e-9);
        let (stage, frac) = stage_from_phase("denoise", 23, 50).unwrap();
        assert_eq!(stage, "denoise 23/50");
        assert!((frac - (0.15 + 0.65 * 22.0 / 50.0)).abs() < 1e-9);
        let (stage, frac) = stage_from_phase("denoise-done", 50, 50).unwrap();
        assert_eq!(stage, "denoise 50/50");
        assert!((frac - 0.80).abs() < 1e-9);
        let (stage, frac) = stage_from_phase("vae", 3, 7).unwrap();
        assert_eq!(stage, "vae 3/7");
        assert!((frac - (0.81 + 0.10 * 2.0 / 7.0)).abs() < 1e-9);
        assert!(stage_from_phase("something-else", 0, 0).is_none());
        // Warm-conditioning markers stay monotonic with the phases around
        // them (text-cached <= keyframe <= dit-load).
        let (stage, frac) = stage_from_phase("text-cached", 0, 0).unwrap();
        assert_eq!(stage, "text-encode (cached)");
        assert!(frac <= stage_from_phase("keyframe-cached", 0, 0).unwrap().1);
        assert!(
            stage_from_phase("keyframe-cached", 0, 0).unwrap().1
                <= stage_from_phase("dit-load", 0, 0).unwrap().1
        );
        // The whole run's fractions are monotonic.
        let sequence = [
            stage_from_phase("te-load", 0, 0).unwrap().1,
            stage_from_phase("text-encode", 0, 0).unwrap().1,
            stage_from_phase("dit-load", 0, 0).unwrap().1,
            stage_from_phase("denoise", 1, 50).unwrap().1,
            stage_from_phase("denoise-done", 50, 50).unwrap().1,
            stage_from_phase("vae-load", 0, 0).unwrap().1,
            stage_from_phase("vae", 1, 7).unwrap().1,
            stage_from_phase("vae-done", 0, 0).unwrap().1,
            stage_from_phase("audio-decode", 0, 0).unwrap().1,
        ];
        assert!(
            sequence.windows(2).all(|pair| pair[0] <= pair[1]),
            "{sequence:?}"
        );
    }

    #[test]
    fn progress_line_mapping() {
        assert_eq!(stage_from_line("layout: seq=8646 ..."), Some(("layout", 0.01)));
        assert_eq!(
            stage_from_line("te: encode 32.00s (92 tokens), 400 buffers evicted"),
            Some(("text-encode", 0.08))
        );
        let (stage, frac) = stage_from_line("step 25/49: 1.690s (video_t=0.5)").unwrap();
        assert_eq!(stage, "denoise");
        assert!((frac - (0.10 + 0.72 * 25.0 / 49.0)).abs() < 1e-9);
        assert_eq!(stage_from_line("vae: decode 3.10s -> 124 frames"), Some(("video-decode", 0.86)));
        assert_eq!(stage_from_line("audio: decode 1.2s"), Some(("audio-decode", 0.90)));
        assert_eq!(stage_from_line("noise: own RNG seed 7"), None);
    }
}
