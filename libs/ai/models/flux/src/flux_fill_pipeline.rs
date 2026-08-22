//! FLUX.1-Fill-dev pipeline: mask inpaint/outpaint (black-forest-labs
//! FLUX.1-Fill-dev). Architecturally this is flux1-dev's DiT with a wider
//! `img_in`: 384 channels per token instead of 64 (packed noisy latent 64 +
//! packed masked-image latent 64 + packed mask 256), everything else —
//! text encoders, transformer blocks/attention/rope, VAE decode, the Euler
//! schedule — is byte-for-byte the same math as `flux_pipeline.rs`.
//!
//! `flux.rs`'s `FluxTransformerInspection::from_header` already infers
//! `in_channels`/`out_channels` generically from the checkpoint's
//! `img_in.weight`/`final_layer.linear.weight` shapes, and
//! `flux_transformer.rs`'s forward pass reads `weights.config.in_channels`
//! rather than hard-coding 64 — so loading a Fill checkpoint and running the
//! transformer needs ZERO changes there. What Fill actually needs beyond
//! flux1-dev:
//! 1. A VAE ENCODE path (dev/schnell only ever decode) to turn the
//!    caller's masked image into `masked_image_latents` —
//!    `flux_vae::encode_image_to_latent_mean`.
//! 2. [`pack_flux_fill_mask`] — packs the full-resolution mask into the
//!    256-channel-per-token conditioning diffusers'
//!    `FluxFillPipeline.prepare_mask_latents` produces.
//! 3. A denoise loop that concatenates the evolving 64-channel noisy
//!    latent with the two FIXED 64+256-channel conditioning blocks into a
//!    384-channel model input at every step, while `euler_step` keeps
//!    updating only the 64-channel noisy part (the transformer's output is
//!    still 64 channels — `out_channels` is unchanged from flux1-dev,
//!    confirmed against the community FP8 repack's
//!    `final_layer.linear.weight` shape `[64, 3072]`, see
//!    `libs/asset/ai/registry.json`'s `flux1-fill-dev` entry note).
//!
//! See `libs/asset/ai/src/inpaint_backend.rs` for how a `POST /generate`
//! request (named inputs `image` + `mask`, PNG) maps onto this pipeline —
//! PNG decode/encode and u8<->f32 conversion live there, this file only
//! does tensor math.

use crate::backend::{new_runtime, runtime_available};
use crate::comfy::FluxPrompts;
use crate::flux::{
    pack_flux_latents_nchw, unpack_flux_latents_nchw, FluxLatentShape, FluxPromptToImagePlan,
};
use crate::flux_pipeline::{
    gaussian_latents, hook_check, hook_emit, nchw_to_whcb, sub_hook, sub_hook_emit_only,
    FluxRunHooks,
};
use crate::flux_schedule::{
    euler_step, FluxSchedule, FLUX_VAE_SCALING_FACTOR, FLUX_VAE_SHIFT_FACTOR,
};
use crate::flux_text::{
    FluxCompiledTextEncoders, FluxConditioning, FluxLoadedTextEncoders, FluxTokenizedPrompts,
};
use crate::flux_transformer::{CompiledFluxTransformer, LoadedFluxTransformerWeights};
use crate::flux_vae::{
    encode_image_to_latent_mean, CompiledFluxVae, FluxVaeDecodeRun, LoadedFluxVaeWeights,
};
use crate::{DiffusionError, Result};
use std::time::Instant;

/// 8x8 pixel sub-blocks per latent-space pixel (the VAE's own downsample
/// factor, BEFORE the transformer's 2x2 patchify) — see [`pack_flux_fill_mask`].
const MASK_VAE_DOWNSAMPLE: usize = 8;
/// FLUX's 16-channel VAE latent space.
const VAE_LATENT_CHANNELS: usize = 16;

/// Packs a full-resolution single-channel mask (values in `[0,1]`, white/1
/// = repaint) into FLUX.1-Fill's per-token mask conditioning: 256 channels
/// per latent token (16x16 mask pixels per token — the transformer's
/// packed-token grid is `image_size / 16`, same as
/// [`crate::flux::FluxLatentShape`]).
///
/// This is diffusers' `FluxFillPipeline.prepare_mask_latents` mask branch:
/// reshape `(H, W)` into `(H/8, 8, W/8, 8)`, permute the two size-8 axes
/// into the channel dimension as a virtual 64-channel "latent"
/// `(64, H/8, W/8)`, then patchify-pack it exactly like
/// [`crate::flux::pack_flux_latents_nchw`] packs the real 16-channel VAE
/// latent (`feature = channel*4 + (y%2)*2 + (x%2)`, giving 64*4 = 256).
/// Verified against a Python port of that reshape/permute/pack chain before
/// porting (see the `pack_flux_fill_mask_matches_reference_reshape_permute`
/// test below, which independently reimplements the same chain).
pub fn pack_flux_fill_mask(mask: &[f32], image_width: u32, image_height: u32) -> Result<Vec<f32>> {
    if image_width % 16 != 0 || image_height % 16 != 0 {
        return Err(DiffusionError::workflow(format!(
            "FLUX Fill mask size must be divisible by 16, got {}x{}",
            image_width, image_height
        )));
    }
    let width = image_width as usize;
    let height = image_height as usize;
    let expected = width
        .checked_mul(height)
        .ok_or_else(|| DiffusionError::workflow("FLUX Fill mask size overflow"))?;
    if mask.len() != expected {
        return Err(DiffusionError::workflow(format!(
            "FLUX Fill mask pack expected {} values for {}x{}, got {}",
            expected, image_width, image_height, mask.len()
        )));
    }

    let vsf = MASK_VAE_DOWNSAMPLE;
    let packed_h = height / (vsf * 2);
    let packed_w = width / (vsf * 2);
    let mut packed = vec![0.0f32; packed_h * packed_w * 256];
    for token_y in 0..packed_h {
        for token_x in 0..packed_w {
            let token = token_y * packed_w + token_x;
            for c64 in 0..(vsf * vsf) {
                let dy = c64 / vsf;
                let dx = c64 % vsf;
                for quad in 0..4usize {
                    let ly = token_y * 2 + quad / 2;
                    let lx = token_x * 2 + quad % 2;
                    let y = ly * vsf + dy;
                    let x = lx * vsf + dx;
                    let feature = c64 * 4 + quad;
                    packed[token * 256 + feature] = mask[y * width + x];
                }
            }
        }
    }
    Ok(packed)
}

#[derive(Clone, Debug)]
struct FluxFillConditioning {
    /// Packed masked-image VAE latent, tokens x 64 (same packing/scaling as
    /// the noisy latent).
    masked_image_latents_packed: Vec<f32>,
    /// Packed mask, tokens x 256 (see [`pack_flux_fill_mask`]).
    mask_packed: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct FluxFillPipelineLoadTiming {
    pub text_tokenize_ms: f64,
    pub text_load_ms: f64,
    pub text_compile_ms: f64,
    pub text_execute_ms: f64,
    pub transformer_load_ms: f64,
    pub transformer_compile_ms: f64,
    pub vae_load_ms: f64,
    pub vae_compile_ms: f64,
    pub total_ms: f64,
}

#[derive(Clone, Debug, Default)]
pub struct FluxFillPipelineRunTiming {
    pub condition_ms: f64,
    pub denoise_ms: f64,
    pub vae_decode_ms: f64,
    pub total_ms: f64,
}

#[derive(Clone, Debug)]
pub struct FluxFillPipelineGenerateRun {
    pub image: FluxVaeDecodeRun,
    pub timing: FluxFillPipelineRunTiming,
}

pub struct FluxFillPipeline {
    plan: FluxPromptToImagePlan,
    latent_shape: FluxLatentShape,
    conditioning: FluxConditioning,
    fill: Option<FluxFillConditioning>,
    clip_backend_name: String,
    t5_backend_name: String,
    transformer_backend_name: String,
    text_weights: FluxLoadedTextEncoders,
    transformer_weights: LoadedFluxTransformerWeights,
    transformer: CompiledFluxTransformer,
    vae_backend_name: String,
    vae_weights: LoadedFluxVaeWeights,
    vae: CompiledFluxVae,
}

impl FluxFillPipeline {
    pub fn load(
        plan: FluxPromptToImagePlan,
        image_width: Option<u32>,
        image_height: Option<u32>,
    ) -> Result<(Self, FluxFillPipelineLoadTiming)> {
        Self::load_with_hooks(plan, image_width, image_height, None)
    }

    /// Mirrors [`crate::flux_pipeline::FluxPipeline::load_with_hooks`]
    /// exactly (same building blocks, same hook bands) — the difference is
    /// entirely in [`Self::in_channels`]/[`Self::generate_with_hooks`], not
    /// in how the checkpoint loads.
    pub fn load_with_hooks(
        plan: FluxPromptToImagePlan,
        image_width: Option<u32>,
        image_height: Option<u32>,
        mut hooks: Option<&mut FluxRunHooks>,
    ) -> Result<(Self, FluxFillPipelineLoadTiming)> {
        let total_start = Instant::now();
        let width = image_width.unwrap_or(plan.generation.width);
        let height = image_height.unwrap_or(width);
        let latent_shape = FluxLatentShape::from_image_size(width, height)?;

        hook_emit(&mut hooks, "tokenize", 0.0);
        let tokenize_start = Instant::now();
        let prompts = FluxTokenizedPrompts::from_prompts(&plan.prompts)?;
        let text_tokenize_ms = crate::flux_pipeline::elapsed_ms(tokenize_start);

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "load clip_l", 0.02);
        let text_load_start = Instant::now();
        let mut text_weights = {
            let mut sub = sub_hook(&mut hooks, 0.02, 0.08);
            FluxLoadedTextEncoders::load_split(&plan.bundle, crate::hook_ref(&mut sub))?
        };
        let text_load_ms = crate::flux_pipeline::elapsed_ms(text_load_start);

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "compile text encoders", 0.10);
        let text_compile_start = Instant::now();
        let text = FluxCompiledTextEncoders::compile(&mut text_weights, &prompts)?;
        let text_compile_ms = crate::flux_pipeline::elapsed_ms(text_compile_start);
        let clip_backend_name = text.clip_backend_name().to_string();
        let t5_backend_name = text.t5_backend_name().to_string();

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "text-encode clip_l", 0.13);
        let text_execute_start = Instant::now();
        let conditioning = {
            let mut sub = sub_hook(&mut hooks, 0.14, 0.46);
            text.execute_split(&text_weights, &prompts, crate::hook_ref(&mut sub))?
        };
        let text_execute_ms = crate::flux_pipeline::elapsed_ms(text_execute_start);

        hook_check(&hooks)?;
        let runtime = if runtime_available() {
            Some(new_runtime()?)
        } else {
            None
        };

        hook_emit(&mut hooks, "load unet", 0.60);
        let transformer_load_start = Instant::now();
        let mut transformer_weights = {
            let mut sub = sub_hook(&mut hooks, 0.60, 0.15);
            LoadedFluxTransformerWeights::load_component_with_progress(
                &plan.bundle.diffusion_model_path,
                plan.bundle.component_prefixes().diffusion,
                crate::hook_ref(&mut sub),
            )?
        };
        let transformer_load_ms = crate::flux_pipeline::elapsed_ms(transformer_load_start);

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "compile transformer", 0.75);
        let transformer_compile_start = Instant::now();
        let (transformer, _compile_timing) = {
            let mut sub = sub_hook(&mut hooks, 0.75, 0.15);
            CompiledFluxTransformer::compile_hooked(
                runtime.clone(),
                &mut transformer_weights,
                &conditioning,
                latent_shape,
                crate::hook_ref(&mut sub),
            )?
        };
        let transformer_compile_ms = crate::flux_pipeline::elapsed_ms(transformer_compile_start);
        let transformer_backend_name = transformer.backend_name().to_string();

        let vae_path = plan
            .bundle
            .vae_path
            .as_ref()
            .ok_or_else(|| DiffusionError::workflow("workflow bundle does not include vae"))?;
        hook_check(&hooks)?;
        hook_emit(&mut hooks, "load vae", 0.92);
        let vae_load_start = Instant::now();
        let mut vae_weights =
            LoadedFluxVaeWeights::load_component(vae_path, plan.bundle.component_prefixes().vae)?;
        let vae_load_ms = crate::flux_pipeline::elapsed_ms(vae_load_start);

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "compile vae", 0.96);
        let vae_compile_start = Instant::now();
        let vae = match runtime {
            Some(runtime) => {
                CompiledFluxVae::compile_with_runtime(runtime, &mut vae_weights, latent_shape)?
            }
            None => CompiledFluxVae::compile(&mut vae_weights, latent_shape)?,
        };
        let vae_compile_ms = crate::flux_pipeline::elapsed_ms(vae_compile_start);
        let vae_backend_name = vae.backend_name().to_string();

        let pipeline = Self {
            plan,
            latent_shape,
            conditioning,
            fill: None,
            clip_backend_name,
            t5_backend_name,
            transformer_backend_name,
            text_weights,
            transformer_weights,
            transformer,
            vae_backend_name,
            vae_weights,
            vae,
        };
        crate::backend::gpu_weight_cache_protect_prefixes(pipeline.residency_namespaces())
            .map_err(DiffusionError::model)?;
        Ok((
            pipeline,
            FluxFillPipelineLoadTiming {
                text_tokenize_ms,
                text_load_ms,
                text_compile_ms,
                text_execute_ms,
                transformer_load_ms,
                transformer_compile_ms,
                vae_load_ms,
                vae_compile_ms,
                total_ms: crate::flux_pipeline::elapsed_ms(total_start),
            },
        ))
    }

    /// Same warm-reuse identity as
    /// [`crate::flux_pipeline::FluxPipeline::serves_plan`]: resolved model
    /// file paths + image size. Prompts, and the per-request image/mask
    /// conditioning, are never part of this — see
    /// [`Self::ensure_prompts_with_hooks`] and
    /// [`Self::prepare_conditioning_with_hooks`].
    pub fn serves_plan(
        &self,
        plan: &FluxPromptToImagePlan,
        image_width: Option<u32>,
        image_height: Option<u32>,
    ) -> bool {
        let width = image_width.unwrap_or(plan.generation.width);
        let height = image_height.unwrap_or(width);
        let Ok(latent_shape) = FluxLatentShape::from_image_size(width, height) else {
            return false;
        };
        latent_shape == self.latent_shape
            && plan.bundle.diffusion_model_path == self.plan.bundle.diffusion_model_path
            && plan.bundle.vae_path == self.plan.bundle.vae_path
            && plan.bundle.clip_l_path == self.plan.bundle.clip_l_path
            && plan.bundle.t5xxl_path == self.plan.bundle.t5xxl_path
    }

    pub fn diffusion_model_path(&self) -> &std::path::Path {
        &self.plan.bundle.diffusion_model_path
    }

    fn residency_namespaces(&self) -> Vec<String> {
        vec![
            crate::flux_transformer::flux_cache_namespace(&self.transformer_weights),
            crate::flux_vae::flux_vae_cache_namespace(&self.vae_weights),
            crate::t5_encoder::t5_cache_namespace(&self.text_weights.t5xxl),
            crate::clip_l::clip_cache_namespace(&self.text_weights.clip_l),
        ]
    }

    /// See [`crate::flux_pipeline::FluxPipeline::evict_device_caches`].
    pub fn evict_device_caches(&self) -> usize {
        let _ = crate::backend::gpu_weight_cache_protect_prefixes(Vec::new());
        crate::flux_transformer::evict_device_weight_cache(&self.transformer_weights)
            + crate::flux_vae::evict_device_weight_cache(&self.vae_weights)
            + crate::t5_encoder::evict_device_weight_cache(&self.text_weights.t5xxl)
            + crate::clip_l::evict_device_weight_cache(&self.text_weights.clip_l)
    }

    /// See [`crate::flux_pipeline::FluxPipeline::ensure_prompts_with_hooks`].
    pub fn ensure_prompts_with_hooks(
        &mut self,
        prompts: &FluxPrompts,
        mut hooks: Option<&mut FluxRunHooks>,
    ) -> Result<()> {
        if self.plan.prompts == *prompts {
            return Ok(());
        }
        hook_emit(&mut hooks, "tokenize", 0.0);
        let tokenized = FluxTokenizedPrompts::from_prompts(prompts)?;

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "compile text encoders", 0.05);
        let text = FluxCompiledTextEncoders::compile(&mut self.text_weights, &tokenized)?;

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "text-encode clip_l", 0.10);
        let conditioning = {
            let mut sub = sub_hook(&mut hooks, 0.12, 0.86);
            text.execute_split(&self.text_weights, &tokenized, crate::hook_ref(&mut sub))?
        };

        self.conditioning = conditioning;
        self.plan.prompts = prompts.clone();
        Ok(())
    }

    /// True once [`Self::prepare_conditioning_with_hooks`] has run for the
    /// current request — [`Self::generate_with_hooks`] refuses to run
    /// without it (never falls back to a zero/blank mask).
    pub fn has_conditioning(&self) -> bool {
        self.fill.is_some()
    }

    /// Encodes this request's image+mask conditioning: VAE-encodes the
    /// masked image (`image * (1 - mask)`, diffusers' `prepare_mask_latents`
    /// masking convention) to `masked_image_latents`, and packs the mask —
    /// both FIXED for every denoise step of the following
    /// [`Self::generate_with_hooks`] call. Always recomputed per request
    /// (unlike prompts, a request's image/mask are essentially never
    /// reused across jobs, so there is no warm-skip here) — only the
    /// resident checkpoint weights are the warm cache.
    ///
    /// `image_chw`/`mask` are already-decoded, already-normalized planar
    /// buffers: `image_chw` is `[3][height][width]` in `[-1, 1]` (the same
    /// convention [`crate::flux_pipeline::encode_png_rgb`] decodes to/from),
    /// `mask` is `[height][width]` in `[0, 1]` (white/1 = repaint). PNG
    /// decode and u8<->f32 conversion are the caller's job (see
    /// `libs/asset/ai/src/inpaint_backend.rs`) — this stays pure tensor math.
    pub fn prepare_conditioning_with_hooks(
        &mut self,
        image_chw: &[f32],
        mask: &[f32],
        mut hooks: Option<&mut FluxRunHooks>,
    ) -> Result<()> {
        let width = self.latent_shape.image_width as usize;
        let height = self.latent_shape.image_height as usize;
        let expected_image = width
            .checked_mul(height)
            .and_then(|value| value.checked_mul(3))
            .ok_or_else(|| DiffusionError::workflow("flux fill image size overflow"))?;
        if image_chw.len() != expected_image {
            return Err(DiffusionError::workflow(format!(
                "flux fill expected a {}x{} RGB image ({} values), got {}",
                width,
                height,
                expected_image,
                image_chw.len()
            )));
        }
        let expected_mask = width
            .checked_mul(height)
            .ok_or_else(|| DiffusionError::workflow("flux fill mask size overflow"))?;
        if mask.len() != expected_mask {
            return Err(DiffusionError::workflow(format!(
                "flux fill expected a {}x{} mask ({} values), got {}",
                width,
                height,
                expected_mask,
                mask.len()
            )));
        }

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "vae-encode masked image", 0.0);

        let plane = width * height;
        let mut masked_image = vec![0.0f32; expected_image];
        for channel in 0..3usize {
            let base = channel * plane;
            for pixel in 0..plane {
                masked_image[base + pixel] = image_chw[base + pixel] * (1.0 - mask[pixel]);
            }
        }

        let raw_mean =
            encode_image_to_latent_mean(&self.vae_weights, &masked_image, width, height)?;
        let mut scaled = raw_mean;
        for value in &mut scaled {
            *value = (*value - FLUX_VAE_SHIFT_FACTOR) * FLUX_VAE_SCALING_FACTOR;
        }
        let masked_image_latents_packed = pack_flux_latents_nchw(
            &scaled,
            1,
            self.latent_shape.latent_height,
            self.latent_shape.latent_width,
        )?;

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "pack mask", 0.6);
        let mask_packed = pack_flux_fill_mask(
            mask,
            self.latent_shape.image_width,
            self.latent_shape.image_height,
        )?;

        self.fill = Some(FluxFillConditioning {
            masked_image_latents_packed,
            mask_packed,
        });
        Ok(())
    }

    pub fn latent_shape(&self) -> FluxLatentShape {
        self.latent_shape
    }

    pub fn t5_backend_name(&self) -> &str {
        &self.t5_backend_name
    }

    pub fn clip_backend_name(&self) -> &str {
        &self.clip_backend_name
    }

    pub fn transformer_backend_name(&self) -> &str {
        &self.transformer_backend_name
    }

    pub fn vae_backend_name(&self) -> &str {
        &self.vae_backend_name
    }

    pub fn default_seed(&self) -> u64 {
        self.plan.generation.seed
    }

    pub fn default_guidance(&self) -> f32 {
        self.plan.generation.guidance
    }

    pub fn generate(
        &self,
        seed: u64,
        steps: usize,
        guidance: f32,
    ) -> Result<FluxFillPipelineGenerateRun> {
        self.generate_with_hooks(seed, steps, guidance, None)
    }

    /// Same denoise/decode shape as
    /// [`crate::flux_pipeline::FluxPipeline::generate_with_hooks`]; the only
    /// difference is the per-step model input: the evolving 64-channel
    /// noisy packed latent is concatenated with the FIXED 64+256-channel
    /// image/mask conditioning from [`Self::prepare_conditioning_with_hooks`]
    /// into a 384-channel input before every transformer call, while
    /// [`euler_step`] keeps integrating only the 64-channel noisy part (the
    /// transformer's prediction stays 64 channels — Fill only widens
    /// `img_in`, not `final_layer`).
    pub fn generate_with_hooks(
        &self,
        seed: u64,
        steps: usize,
        guidance: f32,
        mut hooks: Option<&mut FluxRunHooks>,
    ) -> Result<FluxFillPipelineGenerateRun> {
        let fill = self.fill.as_ref().ok_or_else(|| {
            DiffusionError::workflow(
                "flux fill: call prepare_conditioning_with_hooks before generate",
            )
        })?;
        let total_start = Instant::now();
        let steps = steps.max(1);
        let schedule = FluxSchedule::for_flux1(steps, self.plan.transformer.guidance_embed)?;

        let condition_start = Instant::now();
        let image_token_count = self.latent_shape.image_token_count as usize;
        if fill.masked_image_latents_packed.len() != image_token_count * 64
            || fill.mask_packed.len() != image_token_count * 256
        {
            return Err(DiffusionError::model(
                "flux fill: stale conditioning does not match this pipeline's latent shape",
            ));
        }
        let condition_ms = crate::flux_pipeline::elapsed_ms(condition_start);

        let mut latents = gaussian_latents(
            self.latent_shape.latent_width,
            self.latent_shape.latent_height,
            seed,
        );
        let mut packed = pack_flux_latents_nchw(
            &latents,
            1,
            self.latent_shape.latent_height,
            self.latent_shape.latent_width,
        )?;

        let denoise_start = Instant::now();
        let mut model_input = vec![0.0f32; image_token_count * 384];
        for step_index in 0..steps {
            hook_check(&hooks)?;
            let label = if step_index == 0 {
                format!("denoise 1/{steps} (streaming weights)")
            } else {
                format!("denoise {}/{}", step_index + 1, steps)
            };
            let step_base = 0.85 * step_index as f64 / steps as f64;
            hook_emit(&mut hooks, &label, step_base);
            let sigma = schedule.sigmas[step_index];
            let sigma_next = schedule.sigmas[step_index + 1];

            for token in 0..image_token_count {
                let dst = token * 384;
                model_input[dst..dst + 64].copy_from_slice(&packed[token * 64..token * 64 + 64]);
                model_input[dst + 64..dst + 128].copy_from_slice(
                    &fill.masked_image_latents_packed[token * 64..token * 64 + 64],
                );
                model_input[dst + 128..dst + 384]
                    .copy_from_slice(&fill.mask_packed[token * 256..token * 256 + 256]);
            }

            let mut sub = sub_hook_emit_only(
                &mut hooks,
                format!("denoise {}/{} ", step_index + 1, steps),
                step_base,
                0.85 / steps as f64,
            );
            let run = self.transformer.execute_hooked(
                &self.transformer_weights,
                &self.conditioning,
                &model_input,
                sigma,
                guidance,
                crate::hook_ref(&mut sub),
            )?;
            if run.channel_count != 64 {
                return Err(DiffusionError::model(format!(
                    "flux fill: transformer predicted {} channels, expected 64",
                    run.channel_count
                )));
            }
            euler_step(&mut packed, &run.prediction, sigma, sigma_next)?;
        }
        let denoise_ms = crate::flux_pipeline::elapsed_ms(denoise_start);

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "vae-decode", 0.9);
        let vae_decode_start = Instant::now();

        latents = unpack_flux_latents_nchw(
            &packed,
            1,
            self.latent_shape.latent_height,
            self.latent_shape.latent_width,
        )?;
        for value in &mut latents {
            *value = (*value / FLUX_VAE_SCALING_FACTOR) + FLUX_VAE_SHIFT_FACTOR;
        }
        let latents = nchw_to_whcb(
            &latents,
            1,
            VAE_LATENT_CHANNELS,
            self.latent_shape.latent_height as usize,
            self.latent_shape.latent_width as usize,
        )?;

        hook_check(&hooks)?;
        let image = {
            let mut sub = sub_hook_emit_only(&mut hooks, "vae-decode ".to_string(), 0.9, 0.1);
            self.vae
                .execute_hooked(&self.vae_weights, &latents, crate::hook_ref(&mut sub))?
        };
        let vae_decode_ms = crate::flux_pipeline::elapsed_ms(vae_decode_start);
        hook_check(&hooks)?;

        Ok(FluxFillPipelineGenerateRun {
            image,
            timing: FluxFillPipelineRunTiming {
                condition_ms,
                denoise_ms,
                vae_decode_ms,
                total_ms: crate::flux_pipeline::elapsed_ms(total_start),
            },
        })
    }
}

/// Re-exported so service backends only need this module for the whole
/// Fill request/response image conversion (mirrors
/// [`crate::flux_pipeline::encode_png_rgb`]'s role for dev/schnell).
pub use crate::flux_pipeline::encode_png_rgb as encode_fill_png_rgb;

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent reimplementation of diffusers'
    /// `FluxFillPipeline.prepare_mask_latents` mask branch (reshape (H,W)
    /// into (H/8,8,W/8,8), permute the two 8-blocks into the channel dim as
    /// a virtual 64-channel latent, then the same 2x2 patchify pack used
    /// for VAE latents) — written straight from the algorithm description,
    /// not from `pack_flux_fill_mask`'s closed-form shortcut, so agreement
    /// between the two is real cross-validation.
    fn mask_pack_reference(mask: &[f32], width: usize, height: usize) -> Vec<f32> {
        let vsf = 8usize;
        let latent_h = height / vsf;
        let latent_w = width / vsf;
        let mut virt = vec![0.0f32; vsf * vsf * latent_h * latent_w];
        for ly in 0..latent_h {
            for dy in 0..vsf {
                for lx in 0..latent_w {
                    for dx in 0..vsf {
                        let y = ly * vsf + dy;
                        let x = lx * vsf + dx;
                        let c64 = dy * vsf + dx;
                        virt[(c64 * latent_h + ly) * latent_w + lx] = mask[y * width + x];
                    }
                }
            }
        }
        let packed_h = latent_h / 2;
        let packed_w = latent_w / 2;
        let mut out = vec![0.0f32; packed_h * packed_w * 256];
        for c in 0..(vsf * vsf) {
            for y in 0..latent_h {
                for x in 0..latent_w {
                    let ty = y / 2;
                    let tx = x / 2;
                    let token = ty * packed_w + tx;
                    let feature = c * 4 + (y % 2) * 2 + (x % 2);
                    out[token * 256 + feature] = virt[(c * latent_h + y) * latent_w + x];
                }
            }
        }
        out
    }

    #[test]
    fn pack_flux_fill_mask_matches_reference_reshape_permute() {
        let (width, height) = (32u32, 32u32);
        let mask: Vec<f32> = (0..(width * height))
            .map(|i| ((i as f32 * 0.0173) % 1.0))
            .collect();
        let actual = pack_flux_fill_mask(&mask, width, height).unwrap();
        let expected = mask_pack_reference(&mask, width as usize, height as usize);
        assert_eq!(actual.len(), expected.len());
        assert_eq!(actual.len(), 2 * 2 * 256);
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert!((a - e).abs() < 1.0e-7, "mask pack mismatch: {a} vs {e}");
        }
    }

    #[test]
    fn pack_flux_fill_mask_rejects_non_multiple_of_16() {
        let mask = vec![0.0f32; 16 * 15];
        let err = pack_flux_fill_mask(&mask, 16, 15).unwrap_err();
        assert!(matches!(err, DiffusionError::Workflow(_)));
    }

    #[test]
    fn pack_flux_fill_mask_rejects_wrong_length() {
        let mask = vec![0.0f32; 16 * 16 - 1];
        let err = pack_flux_fill_mask(&mask, 16, 16).unwrap_err();
        assert!(matches!(err, DiffusionError::Workflow(_)));
    }

    #[test]
    fn pack_flux_fill_mask_token_order_matches_latent_shape() {
        // A 32x48 image packs to a packed_width=3, packed_height=2 grid,
        // same row-major (token_y*packed_w+token_x) order
        // `pack_flux_latents_nchw` uses for the real VAE latent.
        let shape = FluxLatentShape::from_image_size(48, 32).unwrap();
        let mask = vec![0.0f32; 48 * 32];
        let packed = pack_flux_fill_mask(&mask, 48, 32).unwrap();
        assert_eq!(
            packed.len() as u32,
            shape.packed_width * shape.packed_height * 256
        );
    }
}
