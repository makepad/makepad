//! The `flux2` backend: FLUX.2 through libs/diffusion. CUDA-only; fail
//! closed on Metal/CPU. Two models share it:
//! - `flux2-klein-4b`: instruction + reference image edit (input_b64 PNG).
//! - `flux2-dev`: 32B text-to-image, the official fp8 32GB-card recipe
//!   (euler, 20 steps, guidance 4.0, 1024px default).

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, LiveFrameIn,
    LiveFrameOut, ProgressSink, RgbImage,
};
use crate::error::AssetAiError;
use makepad_ai_common::backend::gpu_device_available;
use makepad_ai_flux::flux2_pipeline::{
    flux2_dev_paths_from_root, flux2_klein_paths_from_root, Flux2DevPipeline, Flux2EditRequest,
    Flux2GenerateRequest, Flux2KleinPipeline, FLUX2_DEV_DEFAULT_GUIDANCE,
    FLUX2_DEV_DEFAULT_SIZE, FLUX2_DEV_DEFAULT_STEPS,
};
use makepad_ai_flux::flux2_vae::flux2_image_from_rgb_u8;
use makepad_zune_core::options::DecoderOptions;
use makepad_zune_png::PngDecoder;
use std::io::BufReader;
use std::path::PathBuf;

pub fn flux2_cuda_provisioned() -> bool {
    gpu_device_available()
}

enum Flux2Pipeline {
    Klein(Flux2KleinPipeline),
    Dev(Box<Flux2DevPipeline>),
}

pub struct Flux2Backend {
    model_id: String,
    cache_dir: Option<PathBuf>,
    pipeline: Option<Flux2Pipeline>,
}

impl Flux2Backend {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            cache_dir: None,
            pipeline: None,
        }
    }

    fn is_dev(&self) -> bool {
        self.model_id == "flux2-dev"
    }
}

impl ContentBackend for Flux2Backend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn is_resident(&self) -> bool {
        self.pipeline.is_some()
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        self.pipeline = None;
        Ok(())
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        if !gpu_device_available() {
            return Err(AssetAiError::Unavailable(format!(
                "{} requires CUDA; refusing CPU/Metal fallback",
                self.model_id
            )));
        }
        ctx.ensure_files()
            .map_err(|err| AssetAiError::Backend(format!("{} download: {err}", self.model_id)))?;
        let root = ctx.cache_dir.join(&self.model_id);
        if self.pipeline.is_some() && self.cache_dir.as_ref() == Some(&root) {
            return Ok(());
        }
        let pipeline = if self.is_dev() {
            let paths = flux2_dev_paths_from_root(&root)
                .map_err(|err| AssetAiError::Backend(format!("flux2-dev paths: {err}")))?;
            Flux2Pipeline::Dev(Box::new(Flux2DevPipeline::load(paths).map_err(|err| {
                AssetAiError::Backend(format!("flux2-dev load: {err}"))
            })?))
        } else {
            let paths = flux2_klein_paths_from_root(&root).map_err(|err| {
                AssetAiError::Backend(format!("flux2-klein-4b paths: {err}"))
            })?;
            Flux2Pipeline::Klein(Flux2KleinPipeline::load(paths).map_err(|err| {
                AssetAiError::Backend(format!("flux2-klein-4b load: {err}"))
            })?)
        };
        self.cache_dir = Some(root);
        self.pipeline = Some(pipeline);
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        if !gpu_device_available() {
            return Err(AssetAiError::Unavailable(format!(
                "{} requires CUDA; refusing CPU/Metal fallback",
                self.model_id
            )));
        }
        cancel.check()?;
        progress("load", 0.05);
        let model_id = self.model_id.clone();
        let pipe = self
            .pipeline
            .as_mut()
            .ok_or_else(|| AssetAiError::Backend(format!("{model_id} not loaded")))?;
        match pipe {
            Flux2Pipeline::Dev(pipe) => {
                let out_w = params
                    .width
                    .unwrap_or(FLUX2_DEV_DEFAULT_SIZE)
                    .max(16)
                    / 16
                    * 16;
                let out_h = params
                    .height
                    .unwrap_or(FLUX2_DEV_DEFAULT_SIZE)
                    .max(16)
                    / 16
                    * 16;
                cancel.check()?;
                progress("generate", 0.1);
                let request = Flux2GenerateRequest {
                    prompt: params.prompt.clone(),
                    width: out_w,
                    height: out_h,
                    steps: params.steps.unwrap_or(FLUX2_DEV_DEFAULT_STEPS as u32) as usize,
                    guidance: params.guidance.unwrap_or(FLUX2_DEV_DEFAULT_GUIDANCE),
                    seed: params.seed,
                    noise: None,
                    teacher_embeds: None,
                    teacher_steps: None,
                };
                let steps_total = request.steps;
                let result = {
                    let mut on_stage = |stage: &str, done: usize, total: usize| {
                        let fraction = match stage {
                            "text-encode" => 0.1 + 0.15 * done as f64 / total.max(1) as f64,
                            "denoise" => 0.25 + 0.65 * done as f64 / steps_total.max(1) as f64,
                            _ => 0.92,
                        };
                        progress(stage, fraction);
                    };
                    pipe.generate_with_hooks(&request, Some(&mut on_stage))
                }
                .map_err(|err| AssetAiError::Backend(format!("flux2-dev generate: {err}")))?;
                cancel.check()?;
                progress("png", 0.97);
                Ok(vec![ArtifactData {
                    content_type: "image/png",
                    ext: "png",
                    bytes: result.png,
                }])
            }
            Flux2Pipeline::Klein(pipe) => {
                if params.input_bytes.is_empty() {
                    return Err(AssetAiError::Params(
                        "flux2-klein-4b edit requires input_b64 (reference PNG)".into(),
                    ));
                }
                let (rgb, width, height) = decode_png_rgb(&params.input_bytes)?;
                let reference = flux2_image_from_rgb_u8(&rgb, width, height)
                    .map_err(|err| AssetAiError::Backend(format!("flux2 ref: {err}")))?;
                let out_w = params.width.unwrap_or(width as u32).max(16) / 16 * 16;
                let out_h = params.height.unwrap_or(height as u32).max(16) / 16 * 16;
                cancel.check()?;
                progress("edit", 0.15);
                let request = Flux2EditRequest {
                    prompt: params.prompt.clone(),
                    width: out_w,
                    height: out_h,
                    steps: params.steps.unwrap_or(4) as usize,
                    seed: params.seed,
                    references: vec![reference],
                    noise: None,
                    teacher_ref_tokens: None,
                    teacher_embeds: None,
                };
                let result = pipe
                    .edit(&request)
                    .map_err(|err| AssetAiError::Backend(format!("flux2 edit: {err}")))?;
                cancel.check()?;
                progress("png", 0.95);
                Ok(vec![ArtifactData {
                    content_type: "image/png",
                    ext: "png",
                    bytes: result.png,
                }])
            }
        }
    }

    fn live_supported(&self) -> bool {
        // Declares the CAPABILITY (this backend implements live_step) for
        // whichever model this instance is: only flux2-klein-4b's edit
        // pipeline maps onto a live step; flux2-dev (32B t2i) has none. Real
        // runnability on THIS machine is `model_availability`'s job (CUDA
        // device required) — `POST /realtime` already 503s on a non-CUDA
        // build/box before this is ever consulted.
        !self.is_dev()
    }

    /// Maps one live-session step onto the Klein edit pipeline: `init`
    /// (the feed-mode latest input frame, or the feedback-warped previous
    /// output — `crate::realtime::run_live` resolves which) is the edit
    /// conditioning image, `config.references` ride along as additional
    /// Klein references, `config.prompt`/`steps`/`seed` map directly.
    /// flux2-dev has no live mode (see `live_supported`) — `POST /realtime`
    /// already refuses it at admission time, so reaching `Flux2Pipeline::
    /// Dev` here would mean that check was bypassed; fail closed rather
    /// than silently running the wrong pipeline.
    fn live_step(&mut self, frame: LiveFrameIn<'_>, cancel: &CancelToken) -> Result<LiveFrameOut, AssetAiError> {
        if !gpu_device_available() {
            return Err(AssetAiError::Unavailable(
                "flux2 requires CUDA; refusing CPU/Metal fallback".into(),
            ));
        }
        cancel.check()?;
        let start = std::time::Instant::now();
        let pipe = self
            .pipeline
            .as_mut()
            .ok_or_else(|| AssetAiError::Backend(format!("{} not loaded", self.model_id)))?;
        let pipe = match pipe {
            Flux2Pipeline::Dev(_) => {
                return Err(AssetAiError::Unavailable(
                    "flux2-dev has no live mode (only flux2-klein-4b's edit pipeline does)".into(),
                ))
            }
            Flux2Pipeline::Klein(pipe) => pipe,
        };
        let config = frame.config;
        let init = frame.init.ok_or_else(|| {
            AssetAiError::Params(
                "flux2-klein-4b live mode requires an init image (feed: latest input frame; \
                 feedback: previous output) — none available yet"
                    .into(),
            )
        })?;
        let mut references = vec![flux2_image_from_rgb_u8(&init.data, init.width as usize, init.height as usize)
            .map_err(|err| AssetAiError::Backend(format!("flux2 live init ref: {err}")))?];
        for extra in &config.references {
            references.push(
                flux2_image_from_rgb_u8(&extra.data, extra.width as usize, extra.height as usize)
                    .map_err(|err| AssetAiError::Backend(format!("flux2 live extra ref: {err}")))?,
            );
        }
        let out_w = config.width.max(16) / 16 * 16;
        let out_h = config.height.max(16) / 16 * 16;
        cancel.check()?;
        // TODO(realtime): Flux2EditRequest has no denoise-strength / noise-
        // scale knob exposed today (see flux2_pipeline.rs) — `config.strength`
        // is accepted on the live wire protocol but NOT applied here; every
        // live_step call is a full Klein edit at `config.steps`. Wire it
        // through `Flux2EditRequest.noise` (or a new pipeline field) if/when
        // flux2_pipeline grows one — do not invent pipeline internals here.
        let request = Flux2EditRequest {
            prompt: config.prompt.clone(),
            width: out_w,
            height: out_h,
            steps: config.steps.max(1) as usize,
            seed: config.seed,
            references,
            noise: None,
            teacher_ref_tokens: None,
            teacher_embeds: None,
        };
        let result = pipe
            .edit(&request)
            .map_err(|err| AssetAiError::Backend(format!("flux2 live edit: {err}")))?;
        cancel.check()?;
        let (rgb, width, height) = crate::testpattern::decode_png_rgb8(&result.png)?;
        Ok(LiveFrameOut {
            image: RgbImage { width, height, data: rgb },
            model_ms: start.elapsed().as_secs_f64() * 1000.0,
        })
    }
}

fn decode_png_rgb(bytes: &[u8]) -> Result<(Vec<u8>, usize, usize), AssetAiError> {
    let cursor = std::io::Cursor::new(bytes.to_vec());
    let reader = BufReader::new(cursor);
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = PngDecoder::new_with_options(reader, options);
    decoder
        .decode_headers()
        .map_err(|err| AssetAiError::Params(format!("flux2 png: {err:?}")))?;
    let info = decoder
        .info()
        .cloned()
        .ok_or_else(|| AssetAiError::Params("flux2 png: no info".into()))?;
    let colorspace = decoder
        .colorspace()
        .ok_or_else(|| AssetAiError::Params("flux2 png: no colorspace".into()))?;
    let pixels = decoder
        .decode_raw()
        .map_err(|err| AssetAiError::Params(format!("flux2 png: {err:?}")))?;
    let components = colorspace.num_components();
    let (w, h) = (info.width as usize, info.height as usize);
    if components < 3 {
        return Err(AssetAiError::Params(format!(
            "flux2 png: need rgb, got {components} channels"
        )));
    }
    let mut rgb = vec![0u8; w * h * 3];
    for (i, chunk) in pixels.chunks_exact(components).enumerate() {
        rgb[i * 3..i * 3 + 3].copy_from_slice(&chunk[..3]);
    }
    Ok((rgb, w, h))
}
