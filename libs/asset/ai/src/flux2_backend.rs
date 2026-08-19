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
    Flux2GenerateRequest, Flux2Img2Img, Flux2KleinPipeline, FLUX2_DEV_DEFAULT_GUIDANCE,
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

    /// Every `flux2-dev*` registry id (the fp8 32 GB recipe and any
    /// quantized 24 GB tier) is the dev pipeline — text-to-image AND the
    /// reference-image instruction edit — so a new tier never silently
    /// falls into the klein loader.
    fn is_dev(&self) -> bool {
        self.model_id.starts_with("flux2-dev")
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
        // img2img strength is not a FLUX.2 knob yet (the edit pipelines
        // condition on reference TOKENS and always generate fully); refuse
        // instead of silently ignoring a user setting.
        if let Some(strength) = params.strength {
            if (strength - 1.0).abs() > f32::EPSILON {
                return Err(AssetAiError::Params(format!(
                    "{} does not support img2img strength (got {strength}); instruction edits always regenerate from the reference tokens — omit `strength`",
                    self.model_id
                )));
            }
        }
        let extra_refs = flux2_extra_reference_inputs(&params.inputs)?;
        progress("load", 0.05);
        let model_id = self.model_id.clone();
        let pipe = self
            .pipeline
            .as_mut()
            .ok_or_else(|| AssetAiError::Backend(format!("{model_id} not loaded")))?;
        match pipe {
            // A reference image makes dev an instruction editor (same
            // multi-reference mechanism as klein, dev's conditioning +
            // guidance); without one it is the text-to-image model.
            Flux2Pipeline::Dev(pipe) if !params.input_bytes.is_empty() => {
                let (rgb, width, height) = decode_png_rgb(&params.input_bytes)?;
                // diffusers `Flux2Pipeline.__call__`: a reference over 1024x1024
                // px is downscaled to that area (aspect-preserving) before the
                // multiple-of-16 floor+crop; an unspecified output size is then
                // inherited from THIS processed reference, not the raw upload.
                let (rgb, width, height) = flux2_prepare_edit_reference(&rgb, width, height)?;
                let reference = flux2_image_from_rgb_u8(&rgb, width, height)
                    .map_err(|err| AssetAiError::Backend(format!("flux2 ref: {err}")))?;
                let out_w = params.width.unwrap_or(width as u32).max(16) / 16 * 16;
                let out_h = params.height.unwrap_or(height as u32).max(16) / 16 * 16;
                // Extra references (`inputs` reference_1..N) get the same
                // diffusers preprocessing as the primary one.
                let mut references = vec![reference];
                for (rgb, width, height) in &extra_refs {
                    let (rgb, width, height) =
                        flux2_prepare_edit_reference(rgb, *width, *height)?;
                    references.push(
                        flux2_image_from_rgb_u8(&rgb, width, height)
                            .map_err(|err| AssetAiError::Backend(format!("flux2 extra ref: {err}")))?,
                    );
                }
                cancel.check()?;
                progress("edit", 0.1);
                let request = Flux2EditRequest {
                    prompt: params.prompt.clone(),
                    width: out_w,
                    height: out_h,
                    steps: params.steps.unwrap_or(FLUX2_DEV_DEFAULT_STEPS as u32) as usize,
                    seed: params.seed,
                    references,
                    noise: None,
                    teacher_ref_tokens: None,
                    teacher_embeds: None,
                    init: None,
                };
                let guidance = params.guidance.unwrap_or(FLUX2_DEV_DEFAULT_GUIDANCE);
                let steps_total = request.steps;
                let result = {
                    let mut on_stage = |stage: &str, done: usize, total: usize| {
                        let fraction = match stage {
                            "text-encode" => 0.1 + 0.12 * done as f64 / total.max(1) as f64,
                            "encode-refs" => 0.22 + 0.03 * done as f64 / total.max(1) as f64,
                            "denoise" => 0.25 + 0.65 * done as f64 / steps_total.max(1) as f64,
                            _ => 0.92,
                        };
                        progress(stage, fraction);
                    };
                    pipe.edit_with_hooks(&request, guidance, Some(&mut on_stage))
                }
                .map_err(|err| AssetAiError::Backend(format!("flux2-dev edit: {err}")))?;
                cancel.check()?;
                progress("png", 0.97);
                Ok(vec![ArtifactData {
                    content_type: "image/png",
                    ext: "png",
                    bytes: result.png,
                }])
            }
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
                    init: None,
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
                let mut references = vec![reference];
                for (rgb, width, height) in &extra_refs {
                    references.push(
                        flux2_image_from_rgb_u8(rgb, *width, *height)
                            .map_err(|err| AssetAiError::Backend(format!("flux2 extra ref: {err}")))?,
                    );
                }
                cancel.check()?;
                progress("edit", 0.15);
                let request = Flux2EditRequest {
                    prompt: params.prompt.clone(),
                    width: out_w,
                    height: out_h,
                    steps: params.steps.unwrap_or(4) as usize,
                    seed: params.seed,
                    references,
                    noise: None,
                    teacher_ref_tokens: None,
                    teacher_embeds: None,
                    init: None,
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
        // img2img: the incoming frame is ALSO the sampler's init at
        // `config.strength` (ComfyUI CONST semantics — start at sigma index
        // floor((1-strength)*steps), run the remaining steps; strength 1.0 =
        // the full Klein edit from noise, lower = fewer steps, closer to the
        // frame). The init must match the output size; a feed frame of another
        // size is resampled by the session before it gets here, so mismatch is
        // a protocol error we surface rather than silently restyle from noise.
        let init = if config.strength < 1.0 {
            Some(Flux2Img2Img {
                image: flux2_image_from_rgb_u8(&init.data, init.width as usize, init.height as usize)
                    .map_err(|err| AssetAiError::Backend(format!("flux2 live init: {err}")))?,
                strength: config.strength.clamp(0.0, 1.0),
            })
        } else {
            None
        };
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
            init,
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

/// FLUX.2 [dev] edit-mode reference-image area cap (diffusers
/// `Flux2Pipeline.__call__`: `image_width * image_height > 1024 * 1024`
/// triggers `image_processor._resize_to_target_area(img, 1024*1024)`). BFL's
/// own reference repo (`black-forest-labs/flux2` `encode_image_refs`) caps a
/// single reference at 2024^2 and only drops to 1024^2 once more than one
/// reference is supplied; we follow the diffusers `Flux2Pipeline` number
/// since the rest of this port (schedule, TE padding) is already pinned
/// against it and our service only ever sends a single reference per call.
const FLUX2_DEV_EDIT_REF_AREA_LIMIT: usize = 1024 * 1024;

/// Mirrors the reference pipelines' edit-mode preprocessing: downscale a
/// reference over the area cap (aspect-preserving), then floor+center-crop
/// to a multiple of 16 (`vae_scale_factor * 2`) — diffusers does this via
/// `image_processor.preprocess(..., resize_mode="crop")`, BFL via
/// `cap_pixels` + `center_crop_to_multiple_of_x`. `flux2_vae_encode` and
/// `Flux2EditRequest` both require multiple-of-16 input; the pipeline layer
/// deliberately does not enforce this itself (see `Flux2EditRequest.
/// references` doc), so the backend owns it, same as klein already assumes.
///
/// Uses a plain box-filter downscale rather than PIL's LANCZOS — this runs
/// host-side ahead of the numerically-gated GPU path, so exact pixel parity
/// with the oracle is not the goal, only respecting the area cap (VRAM) and
/// the multiple-of-16 contract the VAE encoder requires.
fn flux2_prepare_edit_reference(
    rgb: &[u8],
    width: usize,
    height: usize,
) -> Result<(Vec<u8>, usize, usize), AssetAiError> {
    if width < 16 || height < 16 {
        return Err(AssetAiError::Params(format!(
            "flux2-dev edit reference {width}x{height} is smaller than the 16px minimum"
        )));
    }
    let (mut rgb, mut width, mut height) = (rgb.to_vec(), width, height);
    let area = width * height;
    if area > FLUX2_DEV_EDIT_REF_AREA_LIMIT {
        let scale = (FLUX2_DEV_EDIT_REF_AREA_LIMIT as f64 / area as f64).sqrt();
        let new_w = ((width as f64 * scale).round() as usize).max(1);
        let new_h = ((height as f64 * scale).round() as usize).max(1);
        rgb = resize_box_rgb(&rgb, width, height, new_w, new_h);
        width = new_w;
        height = new_h;
    }
    let cropped_w = (width / 16) * 16;
    let cropped_h = (height / 16) * 16;
    if cropped_w == 0 || cropped_h == 0 {
        return Err(AssetAiError::Params(format!(
            "flux2-dev edit reference {width}x{height} rounds to 0 at the 16px multiple"
        )));
    }
    if cropped_w != width || cropped_h != height {
        rgb = center_crop_rgb(&rgb, width, height, cropped_w, cropped_h);
    }
    Ok((rgb, cropped_w, cropped_h))
}

/// Aspect-agnostic box-filter downscale (average of the source rectangle
/// mapped to each destination pixel) — only ever called with `dw <= sw` and
/// `dh <= sh` (downscale-only, see `flux2_prepare_edit_reference`).
fn resize_box_rgb(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh * 3];
    for oy in 0..dh {
        let sy0 = oy * sh / dh;
        let sy1 = (((oy + 1) * sh / dh).max(sy0 + 1)).min(sh);
        for ox in 0..dw {
            let sx0 = ox * sw / dw;
            let sx1 = (((ox + 1) * sw / dw).max(sx0 + 1)).min(sw);
            let mut sum = [0u64; 3];
            let mut count = 0u64;
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let idx = (sy * sw + sx) * 3;
                    sum[0] += src[idx] as u64;
                    sum[1] += src[idx + 1] as u64;
                    sum[2] += src[idx + 2] as u64;
                    count += 1;
                }
            }
            let count = count.max(1);
            let didx = (oy * dw + ox) * 3;
            out[didx] = (sum[0] / count) as u8;
            out[didx + 1] = (sum[1] / count) as u8;
            out[didx + 2] = (sum[2] / count) as u8;
        }
    }
    out
}

/// Center-crop an RGB8 buffer from `sw x sh` down to `dw x dh` (`dw <= sw`,
/// `dh <= sh`).
fn center_crop_rgb(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    let x0 = (sw - dw) / 2;
    let y0 = (sh - dh) / 2;
    let mut out = vec![0u8; dw * dh * 3];
    for y in 0..dh {
        let srow = ((y0 + y) * sw + x0) * 3;
        let drow = y * dw * 3;
        out[drow..drow + dw * 3].copy_from_slice(&src[srow..srow + dw * 3]);
    }
    out
}

/// Extra edit references from the named inputs: every `reference_N` /
/// `reference` PNG, in wire order, decoded to RGB8. Any other named input
/// is refused (a FLUX.2 edit has no other input roles; a typo must not be
/// silently dropped). Capped so a runaway client can't submit a 40-image
/// context window.
pub const FLUX2_MAX_EXTRA_REFERENCES: usize = 7;

fn flux2_extra_reference_inputs(
    inputs: &[crate::backend::NamedInput],
) -> Result<Vec<(Vec<u8>, usize, usize)>, AssetAiError> {
    let mut out = Vec::new();
    for input in inputs {
        let is_ref = input.name == "reference" || input.name.starts_with("reference_");
        if !is_ref {
            return Err(AssetAiError::Params(format!(
                "flux2 edit: unknown named input {:?} (only reference_1..N PNGs are accepted next to input_b64)",
                input.name
            )));
        }
        if !input.content_type.to_ascii_lowercase().starts_with("image/png") {
            return Err(AssetAiError::Params(format!(
                "flux2 edit: named input {:?} must be image/png, got {:?}",
                input.name, input.content_type
            )));
        }
        if out.len() >= FLUX2_MAX_EXTRA_REFERENCES {
            return Err(AssetAiError::Params(format!(
                "flux2 edit: at most {FLUX2_MAX_EXTRA_REFERENCES} extra references"
            )));
        }
        out.push(decode_png_rgb(&input.bytes)?);
    }
    Ok(out)
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

#[cfg(test)]
mod flux2_edit_reference_tests {
    use super::*;

    fn solid(width: usize, height: usize, rgb: [u8; 3]) -> Vec<u8> {
        let mut out = vec![0u8; width * height * 3];
        for px in out.chunks_exact_mut(3) {
            px.copy_from_slice(&rgb);
        }
        out
    }

    #[test]
    fn already_valid_reference_is_untouched() {
        // 512x512 is under the area cap and already a multiple of 16: the
        // pipeline's assumed "already cropped to a multiple of 16" input
        // must round-trip byte-for-byte, not just same-size.
        let src = solid(512, 512, [10, 20, 30]);
        let (out, w, h) = flux2_prepare_edit_reference(&src, 512, 512).expect("prepare");
        assert_eq!((w, h), (512, 512));
        assert_eq!(out, src);
    }

    #[test]
    fn non_multiple_of_16_is_center_cropped_not_stretched() {
        // 500x500 is under the area cap, so this exercises ONLY the
        // multiple-of-16 floor+crop path (500 -> 496).
        let src = solid(500, 500, [1, 2, 3]);
        let (out, w, h) = flux2_prepare_edit_reference(&src, 500, 500).expect("prepare");
        assert_eq!((w, h), (496, 496));
        assert_eq!(out.len(), 496 * 496 * 3);
        assert!(out.chunks_exact(3).all(|px| px == [1, 2, 3]));
    }

    #[test]
    fn oversized_reference_is_capped_under_the_area_limit() {
        // 2000x1500 = 3,000,000px, well over the 1024*1024 (1,048,576) cap;
        // diffusers `Flux2Pipeline.__call__` downscales aspect-preserving
        // before flooring to a multiple of 16.
        let (sw, sh) = (2000usize, 1500usize);
        let src = solid(sw, sh, [200, 100, 50]);
        let (out, w, h) = flux2_prepare_edit_reference(&src, sw, sh).expect("prepare");
        assert!(w * h <= FLUX2_DEV_EDIT_REF_AREA_LIMIT, "{w}x{h} exceeds the area cap");
        assert_eq!(w % 16, 0);
        assert_eq!(h % 16, 0);
        // Aspect ratio preserved within the rounding the multiple-of-16
        // floor introduces.
        let src_ar = sw as f64 / sh as f64;
        let dst_ar = w as f64 / h as f64;
        assert!((src_ar - dst_ar).abs() < 0.02, "aspect drifted: {src_ar} vs {dst_ar}");
        assert_eq!(out.len(), w * h * 3);
        // A solid-color source must resample to the same solid color.
        assert!(out.chunks_exact(3).all(|px| px == [200, 100, 50]));
    }

    #[test]
    fn single_pixel_short_side_over_16_survives_floor() {
        // 16x2000: pixel count is small (32,000, under the cap) so only the
        // multiple-of-16 floor applies; the short side is already 16.
        let (sw, sh) = (16usize, 2000usize);
        let src = solid(sw, sh, [5, 6, 7]);
        let (out, w, h) = flux2_prepare_edit_reference(&src, sw, sh).expect("prepare");
        assert_eq!(w, 16);
        assert_eq!(h, 2000);
        assert_eq!(out.len(), w * h * 3);
    }

    #[test]
    fn reference_below_16px_is_rejected() {
        let src = solid(8, 8, [0, 0, 0]);
        assert!(flux2_prepare_edit_reference(&src, 8, 8).is_err());
    }

    #[test]
    fn box_resize_preserves_solid_color() {
        let src = solid(100, 100, [42, 84, 126]);
        let out = resize_box_rgb(&src, 100, 100, 37, 41);
        assert_eq!(out.len(), 37 * 41 * 3);
        assert!(out.chunks_exact(3).all(|px| px == [42, 84, 126]));
    }

    #[test]
    fn center_crop_keeps_the_middle_pixel() {
        // 4x4 with a distinct center-left pixel; crop to 2x2 must keep it.
        let mut src = solid(4, 4, [0, 0, 0]);
        let center_idx = (1 * 4 + 1) * 3;
        src[center_idx..center_idx + 3].copy_from_slice(&[9, 9, 9]);
        let out = center_crop_rgb(&src, 4, 4, 2, 2);
        assert_eq!(out.len(), 2 * 2 * 3);
        assert!(out.chunks_exact(3).any(|px| px == [9, 9, 9]));
    }

    fn named(name: &str, content_type: &str) -> crate::backend::NamedInput {
        crate::backend::NamedInput {
            name: name.to_string(),
            content_type: content_type.to_string(),
            bytes: vec![0u8; 4],
        }
    }

    #[test]
    fn extra_references_refuse_unknown_roles_and_non_png() {
        assert!(flux2_extra_reference_inputs(&[]).unwrap().is_empty());
        let err = flux2_extra_reference_inputs(&[named("mesh", "image/png")]).unwrap_err();
        assert!(format!("{err:?}").contains("unknown named input"), "{err:?}");
        let err =
            flux2_extra_reference_inputs(&[named("reference_1", "image/jpeg")]).unwrap_err();
        assert!(format!("{err:?}").contains("must be image/png"), "{err:?}");
        // A reference role with undecodable bytes fails at decode (not
        // silently skipped).
        assert!(flux2_extra_reference_inputs(&[named("reference_1", "image/png")]).is_err());
    }
}
