//! The `flux-fill` backend: mask inpaint/outpaint through
//! `makepad-ai-flux`'s `FluxFillPipeline` (FLUX.1-Fill-dev). Compiled only
//! with the `flux` cargo feature, same as `flux_backend.rs`/`flux2_backend.rs`
//! (they share the `makepad-ai-flux` dependency).
//!
//! Dual-input contract (strict, same posture as `paint_backend.rs`): the
//! request must carry both named inputs `"image"` and `"mask"` (both
//! `image/png`) via `inputs`. A missing input is refused with an explicit
//! error — never inferred, never silently substituted from `input_b64`.
//! `mask` white/1 = repaint; outpaint = the client pre-pads the canvas and
//! masks the new area, same request shape as inpaint.
//!
//! Unlike the flux1-dev/schnell combined-checkpoint tier, `flux1-fill-dev`
//! is a 4-file SPLIT bundle (`unet/`, `vae/`, `text_encoders/` — see
//! `registry.json`'s note on why no combined-checkpoint repack exists for
//! Fill), resolved here by registry file `role` instead of by a single
//! `checkpoints/` name.
//!
//! Residency follows `flux_backend.rs`'s pattern: the loaded
//! `FluxFillPipeline` lives on its own keep-alive worker thread (not
//! `Send` — Metal runtime types on macOS) that survives across jobs. Model
//! weights warm-reuse exactly like flux1-dev/schnell
//! (`FluxFillPipeline::serves_plan` keys on resolved file paths + image
//! size); the image/mask conditioning is recomputed every job — a request's
//! image and mask are essentially never reused, so there is no warm-skip
//! for them the way there is for an unchanged prompt.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, NamedInput,
    ProgressSink,
};
use crate::error::AssetAiError;
use crate::registry::ModelSpec;
use crate::trellis_backend::decode_png_rgba8;
use makepad_ai_flux::comfy::{FluxGenerationConfig, FluxPrompts};
use makepad_ai_flux::flux::{FluxPromptToImagePlan, FluxResolvedBundle};
use makepad_ai_flux::flux_fill_pipeline::encode_fill_png_rgb;
use makepad_ai_common::DiffusionError;
use std::path::PathBuf;

pub const INPUT_IMAGE: &str = "image";
pub const INPUT_MASK: &str = "mask";
pub const IMAGE_CONTENT_TYPE: &str = "image/png";

pub const ROLE_DIFFUSION_MODEL: &str = "diffusion_model";
pub const ROLE_VAE: &str = "vae";
pub const ROLE_CLIP_L: &str = "clip_l";
pub const ROLE_T5XXL: &str = "t5xxl";

/// Default request parameters (BFL's recommended Fill settings — far higher
/// than dev's 20 steps / 3.5 guidance).
pub const DEFAULT_STEPS: u32 = 50;
pub const DEFAULT_GUIDANCE: f32 = 30.0;

/// True when this machine can actually execute flux1-fill-dev: a CUDA
/// device for the raw-F8 resident dense path, same fail-closed posture as
/// `flux_backend::flux_fp8_provisioned` (no CPU/Metal service fallback).
pub fn inpaint_fp8_provisioned() -> bool {
    static PROBE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PROBE.get_or_init(makepad_ai_common::backend::gpu_device_available)
}

pub struct InpaintBackend {
    model_id: String,
    ready: Option<ReadyFiles>,
    worker: Option<(u64, fill_worker::FluxFillWorker)>,
}

struct ReadyFiles {
    diffusion_model_path: PathBuf,
    vae_path: PathBuf,
    clip_l_path: PathBuf,
    t5xxl_path: PathBuf,
}

impl InpaintBackend {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            ready: None,
            worker: None,
        }
    }
}

fn diffusion_err(err: DiffusionError) -> AssetAiError {
    match err {
        DiffusionError::Cancelled => AssetAiError::Cancelled,
        err => AssetAiError::Backend(format!("flux-fill: {err}")),
    }
}

fn named_input<'p>(
    params: &'p GenerateParams,
    name: &str,
) -> Result<&'p NamedInput, AssetAiError> {
    let input = params.inputs.iter().find(|i| i.name == name).ok_or_else(|| {
        let got: Vec<&str> = params.inputs.iter().map(|i| i.name.as_str()).collect();
        AssetAiError::Params(format!(
            "flux-fill requires named input {name:?} ({IMAGE_CONTENT_TYPE}); request carried \
             [{}]. Both \"image\" and \"mask\" are mandatory — no fallback, no inference.",
            got.join(", ")
        ))
    })?;
    if input.content_type != IMAGE_CONTENT_TYPE {
        return Err(AssetAiError::Params(format!(
            "named input {name:?} content_type must be {IMAGE_CONTENT_TYPE:?}, got {:?}",
            input.content_type
        )));
    }
    Ok(input)
}

fn role_path(
    spec: &ModelSpec,
    cache_dir: &std::path::Path,
    role: &str,
) -> Result<PathBuf, AssetAiError> {
    let file = spec.file_by_role(role).ok_or_else(|| {
        AssetAiError::Backend(format!(
            "model {}: registry has no file with role {role:?}",
            spec.id
        ))
    })?;
    Ok(file.dest_path(cache_dir))
}

/// Rounds up to the next multiple of 16 (FLUX's packed-token grid needs
/// `image_size % 16 == 0`); a size already aligned is unchanged.
fn round_up_to_16(value: u32) -> u32 {
    value.max(1).div_ceil(16) * 16
}

/// Pads an RGBA8 image up to `(target_w, target_h)` by replicating the
/// bottom/right edge — used when the caller's image isn't already a
/// multiple of 16 (or requests a larger outpaint canvas without pre-padding
/// it themselves). No-op (returns the input unchanged) when already the
/// target size.
fn pad_rgba_edge_replicate(
    rgba: &[u8],
    width: usize,
    height: usize,
    target_width: usize,
    target_height: usize,
) -> Vec<u8> {
    if width == target_width && height == target_height {
        return rgba.to_vec();
    }
    let mut out = vec![0u8; target_width * target_height * 4];
    for y in 0..target_height {
        let src_y = y.min(height.saturating_sub(1));
        for x in 0..target_width {
            let src_x = x.min(width.saturating_sub(1));
            let src = (src_y * width + src_x) * 4;
            let dst = (y * target_width + x) * 4;
            out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    out
}

/// Pads a mask (as decoded RGBA8; only the R channel is read downstream) up
/// to `(target_w, target_h)` with ZEROES (not repaint) — the padded strip
/// has no corresponding real image content, so it never gets a fabricated
/// "repaint me" hint unless the caller's own mask already reaches the edge.
fn pad_mask_zero(
    rgba: &[u8],
    width: usize,
    height: usize,
    target_width: usize,
    target_height: usize,
) -> Vec<u8> {
    if width == target_width && height == target_height {
        return rgba.to_vec();
    }
    let mut out = vec![0u8; target_width * target_height * 4];
    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) * 4;
            let dst = (y * target_width + x) * 4;
            out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    out
}

/// Decoded RGBA8 (alpha dropped) -> planar `[3][h][w]` in `[-1, 1]`, the
/// convention `FluxFillPipeline::prepare_conditioning_with_hooks` and
/// `encode_fill_png_rgb` both use.
fn rgba8_to_chw_neg1_1(rgba: &[u8], width: usize, height: usize) -> Vec<f32> {
    let plane = width * height;
    let mut out = vec![0.0f32; plane * 3];
    for pixel in 0..plane {
        for channel in 0..3usize {
            let byte = rgba[pixel * 4 + channel];
            out[channel * plane + pixel] = (byte as f32 / 255.0) * 2.0 - 1.0;
        }
    }
    out
}

/// Decoded RGBA8's R channel -> `[h][w]` in `[0, 1]` (white/1 = repaint).
/// Grayscale mask PNGs decode with R=G=B (see `decode_png_rgba8`), so the R
/// channel alone is the mask value regardless of how it was authored.
fn mask_rgba8_to_unit(rgba: &[u8], width: usize, height: usize) -> Vec<f32> {
    let plane = width * height;
    let mut out = vec![0.0f32; plane];
    for pixel in 0..plane {
        out[pixel] = rgba[pixel * 4] as f32 / 255.0;
    }
    out
}

impl ContentBackend for InpaintBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn is_resident(&self) -> bool {
        self.worker.is_some()
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        if let Some((generation, worker)) = self.worker.take() {
            fill_worker::FluxFillWorker::retire_shared(generation);
            drop(worker);
        }
        Ok(())
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        if !inpaint_fp8_provisioned() {
            return Err(AssetAiError::Unavailable(format!(
                "model {}: FLUX.1-Fill-dev requires a CUDA device (no CPU/Metal fallback)",
                self.model_id
            )));
        }
        ctx.ensure_files()?;
        self.ready = Some(ReadyFiles {
            diffusion_model_path: role_path(ctx.spec, ctx.cache_dir, ROLE_DIFFUSION_MODEL)?,
            vae_path: role_path(ctx.spec, ctx.cache_dir, ROLE_VAE)?,
            clip_l_path: role_path(ctx.spec, ctx.cache_dir, ROLE_CLIP_L)?,
            t5xxl_path: role_path(ctx.spec, ctx.cache_dir, ROLE_T5XXL)?,
        });
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        let ready = self.ready.as_ref().ok_or_else(|| {
            AssetAiError::Backend("flux-fill backend used before ensure_loaded".to_string())
        })?;
        progress("prepare", 0.01);
        cancel.check()?;

        let image_input = named_input(params, INPUT_IMAGE)?;
        let mask_input = named_input(params, INPUT_MASK)?;
        let (image_rgba, image_width, image_height) = decode_png_rgba8(&image_input.bytes)?;
        let (mask_rgba, mask_width, mask_height) = decode_png_rgba8(&mask_input.bytes)?;
        if mask_width != image_width || mask_height != image_height {
            return Err(AssetAiError::Params(format!(
                "flux-fill: mask is {mask_width}x{mask_height} but image is {image_width}x{image_height} — \
                 they must match exactly (no resize is performed)"
            )));
        }

        let target_width = round_up_to_16(params.width.unwrap_or(image_width as u32));
        let target_height = round_up_to_16(params.height.unwrap_or(image_height as u32));
        if (target_width as usize) < image_width || (target_height as usize) < image_height {
            return Err(AssetAiError::Params(format!(
                "flux-fill: requested canvas {target_width}x{target_height} is smaller than the \
                 input image {image_width}x{image_height} — this backend never crops"
            )));
        }

        let image_rgba = pad_rgba_edge_replicate(
            &image_rgba,
            image_width,
            image_height,
            target_width as usize,
            target_height as usize,
        );
        let mask_rgba = pad_mask_zero(
            &mask_rgba,
            mask_width,
            mask_height,
            target_width as usize,
            target_height as usize,
        );
        let image_chw = rgba8_to_chw_neg1_1(&image_rgba, target_width as usize, target_height as usize);
        let mask = mask_rgba8_to_unit(&mask_rgba, target_width as usize, target_height as usize);

        let steps = params.steps.unwrap_or(DEFAULT_STEPS);
        let guidance = params.guidance.unwrap_or(DEFAULT_GUIDANCE);

        let bundle = FluxResolvedBundle::from_split(
            &ready.diffusion_model_path,
            &ready.vae_path,
            Some(&ready.clip_l_path),
            Some(&ready.t5xxl_path),
        )
        .map_err(diffusion_err)?;
        let plan = FluxPromptToImagePlan::from_files(
            bundle,
            FluxPrompts {
                clip_l: params.prompt.clone(),
                t5xxl: params.prompt.clone(),
                negative: params.negative_prompt.clone(),
            },
            FluxGenerationConfig {
                width: target_width,
                height: target_height,
                batch_size: 1,
                seed: params.seed,
                steps,
                cfg: 1.0,
                denoise: 1.0,
                guidance,
                sampler_name: "euler".to_string(),
                scheduler: "simple".to_string(),
            },
        )
        .map_err(diffusion_err)?;

        if self.worker.is_none() {
            self.worker = Some(fill_worker::FluxFillWorker::shared()?);
        }
        let job = fill_worker::GenerateJob {
            plan,
            width: target_width,
            height: target_height,
            seed: params.seed,
            steps: steps as usize,
            guidance,
            image_chw,
            mask,
        };
        let (generation, worker) = self.worker.as_ref().expect("flux-fill worker just acquired");
        let run = match worker.generate(job, cancel.clone(), progress) {
            Ok(run) => run,
            Err(fill_worker::FluxFillWorkerError::Cancelled) => return Err(AssetAiError::Cancelled),
            Err(fill_worker::FluxFillWorkerError::Other(message)) => {
                return Err(AssetAiError::Backend(format!("flux-fill: {message}")));
            }
            Err(fill_worker::FluxFillWorkerError::WorkerGone(message)) => {
                fill_worker::FluxFillWorker::retire_shared(*generation);
                self.worker = None;
                return Err(AssetAiError::Backend(format!("flux-fill: {message}")));
            }
        };
        cancel.check()?;
        let png = encode_fill_png_rgb(&run.image.image, run.image.width, run.image.height)
            .map_err(diffusion_err)?;
        Ok(vec![ArtifactData {
            content_type: "image/png",
            ext: "png",
            bytes: png,
        }])
    }
}

// ---------------------------------------------------------------------------
// FluxFillWorker: the resident pipeline on a keep-alive worker thread.
// Mirrors flux_backend.rs's `flux_worker` module exactly, parametrized on
// `FluxFillPipeline` instead of `FluxPipeline` — see that module's doc
// comments for the full rationale (Metal non-`Send` types, thread-local CUDA
// buffer pool/device weight cache). The one behavioral difference: every job
// re-runs `prepare_conditioning_with_hooks` (image/mask are per-request, so
// there is no warm-skip the way an unchanged prompt gets one).
// ---------------------------------------------------------------------------

mod fill_worker {
    use crate::backend::{CancelToken, ProgressSink};
    use crate::error::AssetAiError;
    use makepad_ai_flux::flux::FluxPromptToImagePlan;
    use makepad_ai_flux::flux_fill_pipeline::{FluxFillPipeline, FluxFillPipelineGenerateRun};
    use makepad_ai_flux::flux_pipeline::FluxRunHooks;
    use makepad_ai_common::DiffusionError;
    use std::sync::mpsc;

    pub struct GenerateJob {
        pub plan: FluxPromptToImagePlan,
        pub width: u32,
        pub height: u32,
        pub seed: u64,
        pub steps: usize,
        pub guidance: f32,
        /// Planar `[3][height][width]` RGB in `[-1, 1]`.
        pub image_chw: Vec<f32>,
        /// `[height][width]` in `[0, 1]`, white/1 = repaint.
        pub mask: Vec<f32>,
    }

    pub enum FluxFillWorkerError {
        Cancelled,
        Other(String),
        WorkerGone(String),
    }

    enum WorkerEvent {
        Progress(String, f64),
        Done(Result<FluxFillPipelineGenerateRun, FluxFillWorkerError>),
    }

    struct WorkerMsg {
        job: GenerateJob,
        cancel: CancelToken,
        events: mpsc::Sender<WorkerEvent>,
    }

    #[derive(Clone)]
    pub struct FluxFillWorker {
        tx: mpsc::Sender<WorkerMsg>,
    }

    static SHARED_WORKER: std::sync::Mutex<(u64, Option<FluxFillWorker>)> =
        std::sync::Mutex::new((0, None));

    impl FluxFillWorker {
        pub fn shared() -> Result<(u64, Self), AssetAiError> {
            let mut shared = SHARED_WORKER.lock().unwrap();
            if shared.1.is_none() {
                shared.0 += 1;
                shared.1 = Some(Self::spawn()?);
            }
            Ok((shared.0, shared.1.clone().expect("shared flux-fill worker set")))
        }

        pub fn retire_shared(generation: u64) {
            let mut shared = SHARED_WORKER.lock().unwrap();
            if shared.0 == generation {
                shared.1 = None;
            }
        }

        fn spawn() -> Result<Self, AssetAiError> {
            let (tx, rx) = mpsc::channel::<WorkerMsg>();
            std::thread::Builder::new()
                .name("flux-fill-pipeline".to_string())
                .spawn(move || {
                    let mut warm: Option<FluxFillPipeline> = None;
                    while let Ok(WorkerMsg { job, cancel, events }) = rx.recv() {
                        let result = run_generate(&mut warm, job, &cancel, &events);
                        let _ = events.send(WorkerEvent::Done(result));
                    }
                })
                .map_err(|e| AssetAiError::Backend(format!("spawn flux-fill worker: {e}")))?;
            Ok(Self { tx })
        }

        pub fn generate(
            &self,
            job: GenerateJob,
            cancel: CancelToken,
            progress: ProgressSink,
        ) -> Result<FluxFillPipelineGenerateRun, FluxFillWorkerError> {
            let (event_tx, event_rx) = mpsc::channel();
            self.tx
                .send(WorkerMsg {
                    job,
                    cancel,
                    events: event_tx,
                })
                .map_err(|_| {
                    FluxFillWorkerError::WorkerGone("flux-fill worker thread is gone".to_string())
                })?;
            loop {
                match event_rx.recv() {
                    Ok(WorkerEvent::Progress(name, fraction)) => progress(&name, fraction),
                    Ok(WorkerEvent::Done(result)) => return result,
                    Err(_) => {
                        return Err(FluxFillWorkerError::WorkerGone(
                            "flux-fill worker dropped the reply".to_string(),
                        ))
                    }
                }
            }
        }
    }

    fn worker_err(err: DiffusionError) -> FluxFillWorkerError {
        match err {
            DiffusionError::Cancelled => FluxFillWorkerError::Cancelled,
            err => FluxFillWorkerError::Other(err.to_string()),
        }
    }

    /// Phase bands: load/prompt 0.02..0.15, image+mask conditioning
    /// 0.15..0.30 (the VAE encode — new work dev/schnell never do),
    /// denoise+vae-decode 0.30..0.95.
    fn run_generate(
        warm: &mut Option<FluxFillPipeline>,
        job: GenerateJob,
        cancel: &CancelToken,
        events: &mpsc::Sender<WorkerEvent>,
    ) -> Result<FluxFillPipelineGenerateRun, FluxFillWorkerError> {
        let progress = |name: &str, fraction: f64| {
            let _ = events.send(WorkerEvent::Progress(name.to_string(), fraction));
        };
        let is_cancelled = || cancel.is_cancelled();

        if !warm.as_ref().is_some_and(|pipeline| {
            pipeline.serves_plan(&job.plan, Some(job.width), Some(job.height))
        }) {
            if let Some(old) = warm.take() {
                if old.diffusion_model_path() != job.plan.bundle.diffusion_model_path {
                    old.evict_device_caches();
                }
                drop(old);
            }
        }
        let load_result = {
            let mut load_progress = |name: &str, fraction: f64| progress(name, 0.02 + fraction * 0.13);
            let mut hooks = FluxRunHooks {
                progress: &mut load_progress,
                cancel: &is_cancelled,
            };
            match warm.as_mut() {
                Some(pipeline) => pipeline
                    .ensure_prompts_with_hooks(&job.plan.prompts, Some(&mut hooks))
                    .map(|_| None),
                None => FluxFillPipeline::load_with_hooks(
                    job.plan,
                    Some(job.width),
                    Some(job.height),
                    Some(&mut hooks),
                )
                .map(|(pipeline, _load_timing)| Some(pipeline)),
            }
        };
        match load_result {
            Ok(Some(pipeline)) => *warm = Some(pipeline),
            Ok(None) => {}
            Err(err) => {
                if !matches!(err, DiffusionError::Cancelled) {
                    *warm = None;
                }
                return Err(worker_err(err));
            }
        }
        if cancel.is_cancelled() {
            return Err(FluxFillWorkerError::Cancelled);
        }

        // Always re-encode: unlike prompts, a request's image/mask are
        // essentially never reused, so there is no warm-skip path here.
        {
            let pipeline = warm.as_mut().expect("warm flux-fill pipeline just ensured");
            let mut condition_progress = |name: &str, fraction: f64| progress(name, 0.15 + fraction * 0.15);
            let mut hooks = FluxRunHooks {
                progress: &mut condition_progress,
                cancel: &is_cancelled,
            };
            if let Err(err) =
                pipeline.prepare_conditioning_with_hooks(&job.image_chw, &job.mask, Some(&mut hooks))
            {
                if !matches!(err, DiffusionError::Cancelled) {
                    *warm = None;
                }
                return Err(worker_err(err));
            }
        }
        if cancel.is_cancelled() {
            return Err(FluxFillWorkerError::Cancelled);
        }

        let run = {
            let pipeline = warm.as_ref().expect("warm flux-fill pipeline just conditioned");
            let mut gen_progress = |name: &str, fraction: f64| progress(name, 0.30 + fraction * 0.65);
            let mut hooks = FluxRunHooks {
                progress: &mut gen_progress,
                cancel: &is_cancelled,
            };
            pipeline.generate_with_hooks(job.seed, job.steps, job.guidance, Some(&mut hooks))
        };
        match run {
            Ok(run) => Ok(run),
            Err(err) => {
                if !matches!(err, DiffusionError::Cancelled) {
                    *warm = None;
                }
                Err(worker_err(err))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_up_to_16_snaps_up() {
        assert_eq!(round_up_to_16(1024), 1024);
        assert_eq!(round_up_to_16(1000), 1008);
        assert_eq!(round_up_to_16(1), 16);
        assert_eq!(round_up_to_16(17), 32);
    }

    #[test]
    fn pad_rgba_edge_replicate_is_noop_when_already_target_size() {
        let rgba = vec![7u8; 4 * 4 * 4];
        let out = pad_rgba_edge_replicate(&rgba, 4, 4, 4, 4);
        assert_eq!(out, rgba);
    }

    #[test]
    fn pad_rgba_edge_replicate_replicates_border() {
        // 2x2 image, pad up to 3x3: the new row/col repeats the last one.
        #[rustfmt::skip]
        let rgba: Vec<u8> = vec![
            10, 10, 10, 255,   20, 20, 20, 255,
            30, 30, 30, 255,   40, 40, 40, 255,
        ];
        let out = pad_rgba_edge_replicate(&rgba, 2, 2, 3, 3);
        assert_eq!(out.len(), 3 * 3 * 4);
        // Bottom-right corner replicates pixel (1,1) = 40.
        let br = &out[(2 * 3 + 2) * 4..(2 * 3 + 2) * 4 + 4];
        assert_eq!(br, [40, 40, 40, 255]);
        // Top-right new column replicates pixel (1,0) = 20.
        let tr = &out[(0 * 3 + 2) * 4..(0 * 3 + 2) * 4 + 4];
        assert_eq!(tr, [20, 20, 20, 255]);
    }

    #[test]
    fn pad_mask_zero_leaves_padding_at_zero() {
        let rgba = vec![255u8; 2 * 2 * 4];
        let out = pad_mask_zero(&rgba, 2, 2, 4, 4);
        assert_eq!(out.len(), 4 * 4 * 4);
        // Original 2x2 corner preserved.
        assert_eq!(&out[0..4], &[255, 255, 255, 255]);
        // Padding is zero, not replicated.
        assert_eq!(&out[(3 * 4 + 3) * 4..(3 * 4 + 3) * 4 + 4], &[0, 0, 0, 0]);
    }

    #[test]
    fn rgba8_to_chw_neg1_1_maps_range() {
        let rgba = vec![0u8, 128, 255, 255]; // one pixel: R=0,G=128,B=255,A=255
        let out = rgba8_to_chw_neg1_1(&rgba, 1, 1);
        assert_eq!(out.len(), 3);
        assert!((out[0] - (-1.0)).abs() < 1e-6);
        assert!((out[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mask_rgba8_to_unit_reads_r_channel() {
        let rgba = vec![255u8, 0, 0, 0, 0, 0, 0, 255]; // two pixels: white, black
        let out = mask_rgba8_to_unit(&rgba, 2, 1);
        assert_eq!(out, vec![1.0, 0.0]);
    }

    #[test]
    fn named_input_errors_list_missing_input_names() {
        let params = GenerateParams {
            model: "flux1-fill-dev".to_string(),
            prompt: String::new(),
            negative_prompt: String::new(),
            width: None,
            height: None,
            seed: 0,
            steps: None,
            guidance: None,
            delay_ms: 0,
            pull_only: false,
            input_bytes: Vec::new(),
            input_content_type: "image/png".to_string(),
            inputs: Vec::new(),
            strength: None,
            loras: Vec::new(),
            interpolate: None,
            canny_low: None,
            canny_high: None,
            frames: None,
            codec: String::new(),
            audio: None,
            target_domain: "image".to_string(),
            identity_anchor: String::new(),
            style: String::new(),
            max_tokens: 0,
            temperature: 0.0,
            variants: 1,
            text: String::new(),
            voice: String::new(),
            speed: 1.0,
            emotion: None,
            seconds: None,
            lyrics: String::new(),
            remesh_resolution: None,
            texture: None,
            decimation_target: None,
            texture_size: None,
            motion_mode: None,
            peer_sources: Vec::new(),
            peer_tickets: Vec::new(),
        };
        let err = match named_input(&params, INPUT_IMAGE) {
            Err(err) => err,
            Ok(_) => panic!("expected a missing-input error"),
        };
        let message = format!("{err}");
        assert!(message.contains("image"), "{message}");
    }
}
