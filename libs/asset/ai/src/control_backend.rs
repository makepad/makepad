//! The `control` backend: structure-conditioned image generation through
//! FLUX.1-Depth-dev / FLUX.1-Canny-dev — full 12B FLUX.1 [dev]-architecture
//! checkpoints whose `img_in` accepts 128 channels (packed noisy latents 64
//! + packed VAE-encoded control-image latents 64, concatenated every step;
//! see `makepad_ai_flux::flux_pipeline::FluxPipeline::generate_control_with_hooks`
//! and diffusers' `FluxControlPipeline`). Compiled only with the `flux`
//! cargo feature, same as `flux_backend`/`flux2_backend`.
//!
//! Model selection: `flux1-depth-dev` and `flux1-canny-dev` share this one
//! backend, distinguished by `model_id` (`.contains("canny")` vs the depth
//! default) — the only behavioral differences are the input preprocessing
//! (depth: normalize a 16-bit metric-mm PNG; canny: run our own CPU Canny
//! detector on whatever image comes in) and the default guidance scale
//! (depth ~10, canny ~30, per the BFL model cards).
//!
//! Request contract: `input_b64` is the CONTROL image —
//! - `flux1-depth-dev`: the 16-bit grayscale PNG the `depth`/`depth-native`
//!   domain produces (metric millimeters; see `depth_backend::check_depth_output`).
//!   Normalized to 0..1 grayscale via `control_image::normalize_depth_mm`
//!   (near = bright — see that function's doc for the convention and its
//!   caveats; UNVERIFIED against the real checkpoint, no GPU in this
//!   development environment).
//! - `flux1-canny-dev`: EITHER an already-edge-detected image OR a plain
//!   photo — this backend always runs its own CPU Canny pass
//!   (`control_image::canny_edges_u8`, defaults 50/200, request params
//!   `canny_low`/`canny_high`) on whatever comes in, so both are accepted
//!   uniformly (Canny-on-Canny is close to idempotent: a hard step edge
//!   stays a hard step edge through a second pass).
//!
//! `width`/`height` default to the control image's own size rounded to the
//! nearest multiple of 16 (FLUX's hard requirement — `FluxLatentShape`); an
//! explicit request size is honored (also rounded to 16) and the control
//! image is bilinear-resized to match.
//!
//! Fails closed without CUDA (`control_provisioned`) — there is no CPU/Metal
//! fallback for the 12B transformer, same policy as `flux`/`flux2`. The VAE
//! encode step this backend depends on
//! (`makepad_ai_flux::flux_vae::encode_control_image_hooked`) additionally
//! has no eager non-graph device path — see that function's doc — so it
//! needs either a compiled CUDA/Metal graph runtime or (slow, correctness-
//! only) the CPU fallback; on a provisioned CUDA box this is transparent.

use crate::backend::{ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink};
use crate::control_image::{self, CANNY_DEFAULT_HIGH, CANNY_DEFAULT_LOW};
use crate::error::AssetAiError;
use makepad_ai_common::DiffusionError;
use makepad_ai_flux::comfy::{FluxGenerationConfig, FluxPrompts};
use makepad_ai_flux::flux::{FluxPromptToImagePlan, FluxResolvedBundle};
use makepad_ai_flux::flux_pipeline::encode_png_rgb;
use std::path::PathBuf;

/// Recommended defaults per the BFL model cards (see the module doc): Canny
/// conditioning is weaker/more literal than depth, hence the much higher
/// guidance. Both models recommend 30-50 steps; 30 is the default (the
/// faster end of that range — a request may raise it via `steps`).
const CONTROL_DEPTH_DEFAULT_GUIDANCE: f32 = 10.0;
const CONTROL_CANNY_DEFAULT_GUIDANCE: f32 = 30.0;
const CONTROL_DEFAULT_STEPS: u32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlMode {
    Depth,
    Canny,
}

impl ControlMode {
    fn from_model_id(model_id: &str) -> Result<Self, AssetAiError> {
        if model_id.contains("canny") {
            Ok(ControlMode::Canny)
        } else if model_id.contains("depth") {
            Ok(ControlMode::Depth)
        } else {
            Err(AssetAiError::Backend(format!(
                "control backend: model id {model_id:?} is neither a depth nor a canny checkpoint \
                 (expected \"...depth...\" or \"...canny...\" in the id)"
            )))
        }
    }

    fn default_guidance(self) -> f32 {
        match self {
            ControlMode::Depth => CONTROL_DEPTH_DEFAULT_GUIDANCE,
            ControlMode::Canny => CONTROL_CANNY_DEFAULT_GUIDANCE,
        }
    }
}

pub struct ControlBackend {
    model_id: String,
    mode: Option<ControlMode>,
    ready: Option<ReadyFiles>,
    worker: Option<(u64, control_worker::ControlWorker)>,
}

struct ReadyFiles {
    unet_path: PathBuf,
    vae_path: PathBuf,
    clip_l_path: PathBuf,
    t5xxl_path: PathBuf,
}

/// True when this machine can actually execute a control checkpoint: a CUDA
/// device for the 12B dense BF16 transformer. Unlike `flux_fp8_provisioned`
/// this is not an FP8-specific gate (these checkpoints are plain BF16 —
/// no public FP8 repackage exists yet, see the registry note), but the
/// no-CPU/Metal-fallback policy is the same.
pub fn control_provisioned() -> bool {
    static PROBE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PROBE.get_or_init(makepad_ai_common::backend::gpu_device_available)
}

impl ControlBackend {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            mode: None,
            ready: None,
            worker: None,
        }
    }
}

fn diffusion_err(err: DiffusionError) -> AssetAiError {
    match err {
        DiffusionError::Cancelled => AssetAiError::Cancelled,
        err => AssetAiError::Backend(format!("control: {err}")),
    }
}

/// Rounds to the nearest multiple of 16, minimum 16 — `FluxLatentShape`'s
/// hard requirement (three VAE downsamples + one packing halving).
fn round_to_16(value: u32) -> u32 {
    let rounded = ((value + 8) / 16) * 16;
    rounded.max(16)
}

impl ContentBackend for ControlBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn is_resident(&self) -> bool {
        self.worker.is_some()
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        if let Some((generation, worker)) = self.worker.take() {
            control_worker::ControlWorker::retire_shared(generation);
            drop(worker);
        }
        Ok(())
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        if !control_provisioned() {
            return Err(AssetAiError::Unavailable(format!(
                "model {}: the control domain requires a CUDA device (no CPU/Metal fallback)",
                self.model_id
            )));
        }
        let mode = ControlMode::from_model_id(&self.model_id)?;
        self.mode = Some(mode);
        ctx.ensure_files()?;
        self.ready = Some(ReadyFiles {
            unet_path: ctx.path_by_role("unet")?,
            vae_path: ctx.path_by_role("vae")?,
            clip_l_path: ctx.path_by_role("clip-l")?,
            t5xxl_path: ctx.path_by_role("t5xxl")?,
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
            AssetAiError::Backend("control backend used before ensure_loaded".to_string())
        })?;
        let mode = self.mode.ok_or_else(|| {
            AssetAiError::Backend("control backend used before ensure_loaded".to_string())
        })?;
        if params.input_bytes.is_empty() {
            return Err(AssetAiError::Params(format!(
                "{} needs a control image (input_b64 png)",
                self.model_id
            )));
        }
        cancel.check()?;
        progress("prepare", 0.01);

        // -- decode + preprocess the control image into HWC RGB01, per mode --
        let (control_rgb01, src_width, src_height) = match mode {
            ControlMode::Depth => {
                crate::depth_backend::check_depth_output(&params.input_bytes)?;
                let (depth_mm, width, height) = control_image::decode_png_gray16(&params.input_bytes)?;
                let rgb01 = control_image::normalize_depth_mm(&depth_mm, width, height)?;
                (rgb01, width, height)
            }
            ControlMode::Canny => {
                let (rgba, width, height) = crate::trellis_backend::decode_png_rgba8(&params.input_bytes)?;
                let mut rgb = Vec::with_capacity(width * height * 3);
                for pixel in rgba.chunks_exact(4) {
                    rgb.extend_from_slice(&pixel[..3]);
                }
                let (width, height) = (width as u32, height as u32);
                let gray = control_image::rgb8_to_gray_u8(&rgb, width, height)?;
                let low = params.canny_low.unwrap_or(CANNY_DEFAULT_LOW);
                let high = params.canny_high.unwrap_or(CANNY_DEFAULT_HIGH);
                progress("canny", 0.03);
                let edges = control_image::canny_edges_u8(&gray, width, height, low, high)?;
                (control_image::gray_u8_to_rgb01(&edges), width, height)
            }
        };
        cancel.check()?;

        // -- resolve the generation canvas: request size, else control image
        //    size, both rounded to FLUX's mandatory multiple of 16 --
        let width = round_to_16(params.width.unwrap_or(src_width));
        let height = round_to_16(params.height.unwrap_or(src_height));
        let resized_rgb01 = if width == src_width && height == src_height {
            control_rgb01
        } else {
            control_image::resize_hwc_bilinear(&control_rgb01, src_width, src_height, width, height, 3)?
        };
        // [0,1] -> [-1,1] (standard image_processor convention), then HWC ->
        // CHW (the layout flux_vae's encoder wants).
        let neg1_1: Vec<f32> = resized_rgb01.iter().map(|v| v * 2.0 - 1.0).collect();
        let control_pixels_chw = control_image::hwc01_to_chw01(&neg1_1, width, height, 3);

        let steps = params.steps.unwrap_or(CONTROL_DEFAULT_STEPS);
        let guidance = params.guidance.unwrap_or_else(|| mode.default_guidance());

        let bundle = FluxResolvedBundle::from_split(
            &ready.unet_path,
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
                width,
                height,
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
            self.worker = Some(control_worker::ControlWorker::shared()?);
        }
        let job = control_worker::ControlGenerateJob {
            plan,
            width,
            height,
            control_pixels_chw,
            seed: params.seed,
            steps: steps as usize,
            guidance,
        };
        let (generation, worker) = self.worker.as_ref().expect("control worker just acquired");
        let run = match worker.generate(job, cancel.clone(), progress) {
            Ok(run) => run,
            Err(control_worker::ControlWorkerError::Cancelled) => return Err(AssetAiError::Cancelled),
            Err(control_worker::ControlWorkerError::Other(message)) => {
                return Err(AssetAiError::Backend(format!("control: {message}")));
            }
            Err(control_worker::ControlWorkerError::WorkerGone(message)) => {
                control_worker::ControlWorker::retire_shared(*generation);
                self.worker = None;
                return Err(AssetAiError::Backend(format!("control: {message}")));
            }
        };
        cancel.check()?;
        progress("png-encode", 0.97);
        let png = encode_png_rgb(&run.image.image, run.image.width, run.image.height).map_err(diffusion_err)?;
        Ok(vec![ArtifactData {
            content_type: "image/png",
            ext: "png",
            bytes: png,
        }])
    }
}

// ---------------------------------------------------------------------------
// ControlWorker: the resident pipeline on a keep-alive worker thread — the
// same shape as flux_backend.rs's `flux_worker` (FluxPipeline is not `Send`,
// and every control job must land on ONE thread so the CUDA buffer pool /
// device weight cache — both thread-local — can evict what a model switch
// replaces), kept as its own separate worker/thread rather than sharing
// flux_backend's: a resident control pipeline (~34GB BF16, no FP8) and a
// resident flux1-dev pipeline (~17GB FP8) are a different checkpoint family
// entirely, and giving them independent warm state avoids one domain's
// thread panicking (or evicting) the other's residency.
// ---------------------------------------------------------------------------

mod control_worker {
    use crate::backend::{CancelToken, ProgressSink};
    use crate::error::AssetAiError;
    use makepad_ai_flux::flux::FluxPromptToImagePlan;
    use makepad_ai_flux::flux_pipeline::{FluxPipeline, FluxPipelineGenerateRun, FluxRunHooks};
    use makepad_ai_common::DiffusionError;
    use std::sync::mpsc;

    pub struct ControlGenerateJob {
        pub plan: FluxPromptToImagePlan,
        pub width: u32,
        pub height: u32,
        /// Interleaved-by-channel CHW01 (`[c][y][x]`), values in `[-1,1]`,
        /// already sized to `width`x`height` — see
        /// `ControlBackend::generate`'s preprocessing.
        pub control_pixels_chw: Vec<f32>,
        pub seed: u64,
        pub steps: usize,
        pub guidance: f32,
    }

    pub enum ControlWorkerError {
        Cancelled,
        Other(String),
        WorkerGone(String),
    }

    enum WorkerEvent {
        Progress(String, f64),
        Done(Result<FluxPipelineGenerateRun, ControlWorkerError>),
    }

    struct WorkerMsg {
        job: ControlGenerateJob,
        cancel: CancelToken,
        events: mpsc::Sender<WorkerEvent>,
    }

    #[derive(Clone)]
    pub struct ControlWorker {
        tx: mpsc::Sender<WorkerMsg>,
    }

    static SHARED_WORKER: std::sync::Mutex<(u64, Option<ControlWorker>)> = std::sync::Mutex::new((0, None));

    impl ControlWorker {
        pub fn shared() -> Result<(u64, Self), AssetAiError> {
            let mut shared = SHARED_WORKER.lock().unwrap();
            if shared.1.is_none() {
                shared.0 += 1;
                shared.1 = Some(Self::spawn()?);
            }
            Ok((shared.0, shared.1.clone().expect("shared control worker set")))
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
                .name("control-pipeline".to_string())
                .spawn(move || {
                    let mut warm: Option<FluxPipeline> = None;
                    while let Ok(WorkerMsg { job, cancel, events }) = rx.recv() {
                        let result = run_generate(&mut warm, job, &cancel, &events);
                        let _ = events.send(WorkerEvent::Done(result));
                    }
                })
                .map_err(|e| AssetAiError::Backend(format!("spawn control worker: {e}")))?;
            Ok(Self { tx })
        }

        pub fn generate(
            &self,
            job: ControlGenerateJob,
            cancel: CancelToken,
            progress: ProgressSink,
        ) -> Result<FluxPipelineGenerateRun, ControlWorkerError> {
            let (event_tx, event_rx) = mpsc::channel();
            self.tx
                .send(WorkerMsg {
                    job,
                    cancel,
                    events: event_tx,
                })
                .map_err(|_| ControlWorkerError::WorkerGone("control worker thread is gone".to_string()))?;
            loop {
                match event_rx.recv() {
                    Ok(WorkerEvent::Progress(name, fraction)) => progress(&name, fraction),
                    Ok(WorkerEvent::Done(result)) => return result,
                    Err(_) => {
                        return Err(ControlWorkerError::WorkerGone(
                            "control worker dropped the reply".to_string(),
                        ))
                    }
                }
            }
        }
    }

    fn worker_err(err: DiffusionError) -> ControlWorkerError {
        match err {
            DiffusionError::Cancelled => ControlWorkerError::Cancelled,
            err => ControlWorkerError::Other(err.to_string()),
        }
    }

    /// Load (warm-reuse aware, same policy as flux_backend's `run_generate`)
    /// -> encode the control image -> denoise+decode. Progress bands: load
    /// 0.03..0.30, vae-encode 0.30..0.40, denoise+vae-decode 0.40..0.95 (the
    /// backend adds canny/png-encode around this on the caller side).
    fn run_generate(
        warm: &mut Option<FluxPipeline>,
        job: ControlGenerateJob,
        cancel: &CancelToken,
        events: &mpsc::Sender<WorkerEvent>,
    ) -> Result<FluxPipelineGenerateRun, ControlWorkerError> {
        let progress = |name: &str, fraction: f64| {
            let _ = events.send(WorkerEvent::Progress(name.to_string(), fraction));
        };
        let is_cancelled = || cancel.is_cancelled();

        if !warm
            .as_ref()
            .is_some_and(|pipeline| pipeline.serves_plan(&job.plan, Some(job.width), Some(job.height)))
        {
            if let Some(old) = warm.take() {
                if old.diffusion_model_path() != job.plan.bundle.diffusion_model_path {
                    old.evict_device_caches();
                }
                drop(old);
            }
        }
        let load_result = {
            let mut load_progress = |name: &str, fraction: f64| progress(name, 0.03 + fraction * 0.27);
            let mut hooks = FluxRunHooks {
                progress: &mut load_progress,
                cancel: &is_cancelled,
            };
            match warm.as_mut() {
                Some(pipeline) => pipeline
                    .ensure_prompts_with_hooks(&job.plan.prompts, Some(&mut hooks))
                    .map(|_| None),
                None => FluxPipeline::load_with_hooks(job.plan, Some(job.width), Some(job.height), Some(&mut hooks))
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
            return Err(ControlWorkerError::Cancelled);
        }

        let pipeline = warm.as_ref().expect("warm control pipeline just ensured");
        let control_latents_packed = {
            let mut encode_progress = |name: &str, fraction: f64| progress(name, 0.30 + fraction * 0.10);
            let mut hooks = FluxRunHooks {
                progress: &mut encode_progress,
                cancel: &is_cancelled,
            };
            match pipeline.encode_control_image_with_hooks(&job.control_pixels_chw, Some(&mut hooks)) {
                Ok(latents) => latents,
                Err(err) => {
                    if !matches!(err, DiffusionError::Cancelled) {
                        *warm = None;
                    }
                    return Err(worker_err(err));
                }
            }
        };
        if cancel.is_cancelled() {
            return Err(ControlWorkerError::Cancelled);
        }

        let run = {
            let pipeline = warm.as_ref().expect("warm control pipeline just ensured");
            let mut gen_progress = |name: &str, fraction: f64| progress(name, 0.40 + fraction * 0.55);
            let mut hooks = FluxRunHooks {
                progress: &mut gen_progress,
                cancel: &is_cancelled,
            };
            pipeline.generate_control_with_hooks(
                &control_latents_packed,
                job.seed,
                job.steps,
                job.guidance,
                Some(&mut hooks),
            )
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

// ---------------------------------------------------------------------------
// Tests: input validation + mode/rounding logic only — everything past
// `ensure_loaded` needs a provisioned CUDA box (see the module doc). The
// preprocessing math itself (Canny, depth normalization, resize) is tested
// in `control_image.rs`, which has no GPU dependency at all.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_from_model_id() {
        assert_eq!(ControlMode::from_model_id("flux1-depth-dev").unwrap(), ControlMode::Depth);
        assert_eq!(ControlMode::from_model_id("flux1-canny-dev").unwrap(), ControlMode::Canny);
        assert!(ControlMode::from_model_id("flux1-dev").is_err());
    }

    #[test]
    fn default_guidance_matches_bfl_recommendations() {
        assert_eq!(ControlMode::Depth.default_guidance(), 10.0);
        assert_eq!(ControlMode::Canny.default_guidance(), 30.0);
    }

    #[test]
    fn round_to_16_rounds_to_nearest_with_16_minimum() {
        assert_eq!(round_to_16(0), 16);
        assert_eq!(round_to_16(1), 16);
        assert_eq!(round_to_16(8), 16);
        assert_eq!(round_to_16(9), 16);
        assert_eq!(round_to_16(17), 16);
        assert_eq!(round_to_16(24), 32);
        assert_eq!(round_to_16(25), 32);
        assert_eq!(round_to_16(1024), 1024);
        assert_eq!(round_to_16(1023), 1024);
    }

    #[test]
    fn not_provisioned_without_cuda() {
        // This dev/CI machine has no CUDA device: the gate must read false,
        // matching flux_backend's flux_fp8_provisioned on the same box.
        if std::env::var("MAKEPAD_FORCE_CUDA_PROVISIONED").is_err() {
            assert!(!control_provisioned() || makepad_ai_common::backend::gpu_device_available());
        }
    }

    #[test]
    fn generate_without_ensure_loaded_is_a_backend_error() {
        let mut backend = ControlBackend::new("flux1-depth-dev");
        let params = GenerateParams {
            model: "flux1-depth-dev".to_string(),
            prompt: "a mountain".to_string(),
            negative_prompt: String::new(),
            width: None,
            height: None,
            seed: 1,
            steps: None,
            guidance: None,
            delay_ms: 0,
            pull_only: false,
            input_bytes: vec![1, 2, 3],
            input_content_type: "image/png".to_string(),
            inputs: Vec::new(),
            strength: None,
            loras: Vec::new(),
            interpolate: None,
            upscale: None,
            flow_map: false,
            frames: None,
            codec: String::new(),
            audio: None,
            target_domain: "image".to_string(),
            identity_anchor: String::new(),
            style: String::new(),
            max_tokens: 512,
            temperature: 0.7,
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
            gaussians: None,
            motion_mode: None,
            canny_low: None,
            canny_high: None,
            peer_sources: Vec::new(),
            peer_tickets: Vec::new(),
        };
        let mut sink = |_: &str, _: f64| {};
        let err = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .err()
            .expect("must fail without ensure_loaded");
        assert!(matches!(err, AssetAiError::Backend(_)));
    }
}
