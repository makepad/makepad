use crate::backend::{new_runtime, runtime_available};
use crate::comfy::FluxPrompts;
use crate::flux::{
    concat_packed_latent_channels, pack_flux_latents_nchw, unpack_flux_latents_nchw,
    FluxLatentShape, FluxPromptToImagePlan,
};
use crate::flux_schedule::{
    euler_step, FluxSchedule, FLUX_VAE_SCALING_FACTOR, FLUX_VAE_SHIFT_FACTOR,
};
use crate::flux_text::{
    FluxCompiledTextEncoders, FluxConditioning, FluxLoadedTextEncoders, FluxTokenizedPrompts,
};
use crate::flux_transformer::{CompiledFluxTransformer, LoadedFluxTransformerWeights};
use crate::flux_vae::{CompiledFluxVae, FluxVaeDecodeRun, LoadedFluxVaeWeights};
use crate::{BoxedProgressHook, DiffusionError, Result};
use makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_core::options::EncoderOptions;
use makepad_zune_png::PngEncoder;
use std::time::Instant;

#[derive(Clone, Debug, Default)]
pub struct FluxPipelineLoadTiming {
    pub runtime_init_ms: f64,
    pub text_tokenize_ms: f64,
    pub text_load_ms: f64,
    pub text_compile_ms: f64,
    pub text_execute_ms: f64,
    pub transformer_load_ms: f64,
    pub transformer_compile_ms: f64,
    pub transformer_graph_build_ms: f64,
    pub transformer_graph_prepare_ms: f64,
    pub transformer_session_create_ms: f64,
    pub vae_load_ms: f64,
    pub vae_compile_ms: f64,
    pub total_ms: f64,
}

#[derive(Clone, Debug, Default)]
pub struct FluxPipelineRunTiming {
    pub noise_ms: f64,
    pub pack_ms: f64,
    pub denoise_ms: f64,
    pub unpack_ms: f64,
    pub latent_rescale_ms: f64,
    pub vae_layout_ms: f64,
    pub vae_execute_ms: f64,
    pub total_ms: f64,
}

#[derive(Clone, Debug)]
pub struct FluxPipelineGenerateRun {
    pub image: FluxVaeDecodeRun,
    pub timing: FluxPipelineRunTiming,
}

/// Progress + cancellation hooks for [`FluxPipeline::load_with_hooks`] /
/// [`FluxPipeline::generate_with_hooks`].
///
/// `progress` receives a phase label and a 0..=1 fraction WITHIN the hooked
/// call (load or generate) — service backends remap that onto their overall
/// job fraction and forward the label as-is. Every multi-second phase moves
/// while it runs: weight streams tick cumulative bytes ("load t5 3.2/9.5GB",
/// "load unet 8.2/23.8GB", ~256MB cadence), the t5 encode ticks per block
/// ("text-encode t5 block 7/24"), and the lazy/device denoise + vae decode
/// tick per block within each step ("denoise 1/4 block 12/57",
/// "vae-decode block 5/19") — on the cold first step that is the device
/// weight stream-in made visible.
///
/// `cancel` is polled at every phase/step boundary; returning true unwinds
/// the run with [`DiffusionError::Cancelled`]. A single in-flight
/// forward/kernel is the granularity floor. The process-global GPU pool
/// self-recycles, so an unwound run leaves the device clean for the next
/// job (resident weight caches stay — that's the warm cache).
pub struct FluxRunHooks<'a> {
    pub progress: &'a mut dyn FnMut(&str, f64),
    pub cancel: &'a dyn Fn() -> bool,
}

/// Crate-visible (not just this file) so `flux_fill_pipeline.rs` can drive
/// [`FluxRunHooks`] with the exact same banding conventions instead of
/// duplicating them.
pub(crate) fn hook_emit(hooks: &mut Option<&mut FluxRunHooks>, name: &str, fraction: f64) {
    if let Some(hooks) = hooks.as_deref_mut() {
        (hooks.progress)(name, fraction);
    }
}

pub(crate) fn hook_check(hooks: &Option<&mut FluxRunHooks>) -> Result<()> {
    if let Some(hooks) = hooks.as_deref() {
        if (hooks.cancel)() {
            return Err(DiffusionError::Cancelled);
        }
    }
    Ok(())
}

/// Builds a sub-phase hook remapping a phase-local fraction 0..=1 onto the
/// `[base, base + span]` band of the hooked call. Labels pass through as-is;
/// every emission doubles as a cancel poll (host-side phases only — the
/// in-flight GPU step uses [`sub_hook_emit_only`]).
pub(crate) fn sub_hook<'a>(
    hooks: &'a mut Option<&mut FluxRunHooks<'_>>,
    base: f64,
    span: f64,
) -> Option<BoxedProgressHook<'a>> {
    hooks.as_deref_mut().map(move |hooks| {
        Box::new(move |label: &str, fraction: f64| -> Result<()> {
            if (hooks.cancel)() {
                return Err(DiffusionError::Cancelled);
            }
            (hooks.progress)(label, base + fraction.clamp(0.0, 1.0) * span);
            Ok(())
        }) as BoxedProgressHook<'a>
    })
}

/// [`sub_hook`] without the cancel poll and with a label prefix — used inside
/// a denoise/decode step, where a single in-flight forward stays the cancel
/// granularity floor.
pub(crate) fn sub_hook_emit_only<'a>(
    hooks: &'a mut Option<&mut FluxRunHooks<'_>>,
    prefix: String,
    base: f64,
    span: f64,
) -> Option<BoxedProgressHook<'a>> {
    hooks.as_deref_mut().map(move |hooks| {
        Box::new(move |label: &str, fraction: f64| -> Result<()> {
            (hooks.progress)(
                &format!("{prefix}{label}"),
                base + fraction.clamp(0.0, 1.0) * span,
            );
            Ok(())
        }) as BoxedProgressHook<'a>
    })
}

pub struct FluxPipeline {
    plan: FluxPromptToImagePlan,
    latent_shape: FluxLatentShape,
    conditioning: FluxConditioning,
    clip_backend_name: String,
    t5_backend_name: String,
    transformer_backend_name: String,
    /// Loaded text-encoder weights stay resident for the pipeline's
    /// lifetime: a changed prompt re-tokenizes and re-encodes against these
    /// (device caches warm, zero weight bytes moved) instead of re-reading
    /// gigabytes from disk. For the combined-FP8 checkpoints this is the
    /// whole-stack-resident contract; the host cost is the raw component
    /// payload (~5GB), not an f32 expansion.
    text_weights: FluxLoadedTextEncoders,
    transformer_weights: LoadedFluxTransformerWeights,
    transformer: CompiledFluxTransformer,
    vae_backend_name: String,
    vae_weights: LoadedFluxVaeWeights,
    vae: CompiledFluxVae,
}

pub type FluxPipelineMetal = FluxPipeline;

impl FluxPipeline {
    pub fn load(
        plan: FluxPromptToImagePlan,
        image_width: Option<u32>,
        image_height: Option<u32>,
    ) -> Result<(Self, FluxPipelineLoadTiming)> {
        Self::load_with_hooks(plan, image_width, image_height, None)
    }

    /// [`Self::load`] with progress/cancel hooks — every multi-second phase
    /// (the t5 text encode above all) gets its own label and a cancel
    /// boundary before it. Fractions span the whole load 0..=1.
    pub fn load_with_hooks(
        plan: FluxPromptToImagePlan,
        image_width: Option<u32>,
        image_height: Option<u32>,
        mut hooks: Option<&mut FluxRunHooks>,
    ) -> Result<(Self, FluxPipelineLoadTiming)> {
        let total_start = Instant::now();
        let width = image_width.unwrap_or(plan.generation.width);
        let height = image_height.unwrap_or(width);
        let mut latent_shape = FluxLatentShape::from_image_size(width, height)?;

        hook_emit(&mut hooks, "tokenize", 0.0);
        let tokenize_start = Instant::now();
        let prompts = FluxTokenizedPrompts::from_prompts(&plan.prompts)?;
        let text_tokenize_ms = elapsed_ms(tokenize_start);

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "load clip_l", 0.02);
        let text_load_start = Instant::now();
        let mut text_weights = {
            // The t5 weight stream (~9.5GB) reports "load t5 3.2/9.5GB"
            // every ~256MB across this band.
            let mut sub = sub_hook(&mut hooks, 0.02, 0.08);
            FluxLoadedTextEncoders::load_split(&plan.bundle, crate::hook_ref(&mut sub))?
        };
        let text_load_ms = elapsed_ms(text_load_start);

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "compile text encoders", 0.10);
        let text_compile_start = Instant::now();
        let text = FluxCompiledTextEncoders::compile(&mut text_weights, &prompts)?;
        let text_compile_ms = elapsed_ms(text_compile_start);
        let clip_backend_name = text.clip_backend_name().to_string();
        let t5_backend_name = text.t5_backend_name().to_string();

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "text-encode clip_l", 0.13);
        let text_execute_start = Instant::now();
        let conditioning = {
            // The long one: t5xxl streams + runs here (seconds to tens of
            // seconds cold) — the lazy path emits "text-encode t5 block
            // 7/24" per block across this band, each one a cancel boundary.
            let mut sub = sub_hook(&mut hooks, 0.14, 0.58);
            text.execute_split(&text_weights, &prompts, crate::hook_ref(&mut sub))?
        };
        let text_execute_ms = elapsed_ms(text_execute_start);

        hook_check(&hooks)?;
        let runtime_start = Instant::now();
        let runtime = if runtime_available() {
            Some(new_runtime()?)
        } else {
            None
        };
        let runtime_init_ms = if runtime.is_some() {
            elapsed_ms(runtime_start)
        } else {
            0.0
        };

        hook_emit(&mut hooks, "load unet", 0.72);
        let transformer_load_start = Instant::now();
        let mut transformer_weights = {
            // Diffusion weight stream ("load unet 8.2/11.1GB" every ~256MB);
            // combined checkpoints scope the component out of the one file.
            // Any requested LoRA adapters are merged into the arena here,
            // before compile and before any device upload.
            let mut sub = sub_hook(&mut hooks, 0.72, 0.06);
            LoadedFluxTransformerWeights::load_component_with_loras(
                &plan.bundle.diffusion_model_path,
                plan.bundle.component_prefixes().diffusion,
                &plan.loras,
                crate::hook_ref(&mut sub),
            )?
        };
        let transformer_load_ms = elapsed_ms(transformer_load_start);
        // Generalizes the packed-latent width to whatever this checkpoint's
        // `img_in` actually expects (already auto-inferred from the header
        // by `FluxTransformerInspection::from_header` — see flux.rs): 64 for
        // every plain dev/schnell checkpoint (a no-op override, same value),
        // 128 for the FLUX.1-Depth-dev / FLUX.1-Canny-dev control variants
        // (noisy + control packed latents concatenated on the channel axis
        // every step — see `generate_control_with_hooks` below). Nothing
        // else keyed on `FluxLatentShape` reads this field (see flux.rs's
        // `FluxLatentShape::transformer_channels` doc) so this is safe for
        // every existing caller.
        latent_shape.transformer_channels = transformer_weights.config.in_channels;

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "compile transformer", 0.78);
        let transformer_compile_start = Instant::now();
        let (transformer, transformer_compile) = {
            // Compiled mode labels its graph/prepare/session sub-stages;
            // lazy mode is instant and emits nothing.
            let mut sub = sub_hook(&mut hooks, 0.78, 0.12);
            CompiledFluxTransformer::compile_hooked(
                runtime.clone(),
                &mut transformer_weights,
                &conditioning,
                latent_shape,
                crate::hook_ref(&mut sub),
            )?
        };
        let transformer_compile_ms = elapsed_ms(transformer_compile_start);
        let transformer_backend_name = transformer.backend_name().to_string();

        let vae_path = plan
            .bundle
            .vae_path
            .as_ref()
            .ok_or_else(|| DiffusionError::workflow("workflow bundle does not include vae"))?;
        hook_check(&hooks)?;
        hook_emit(&mut hooks, "load vae", 0.90);
        let vae_load_start = Instant::now();
        let mut vae_weights =
            LoadedFluxVaeWeights::load_component(vae_path, plan.bundle.component_prefixes().vae)?;
        let vae_load_ms = elapsed_ms(vae_load_start);

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "compile vae", 0.94);
        let vae_compile_start = Instant::now();
        let vae = match runtime {
            Some(runtime) => {
                CompiledFluxVae::compile_with_runtime(runtime, &mut vae_weights, latent_shape)?
            }
            None => CompiledFluxVae::compile(&mut vae_weights, latent_shape)?,
        };
        let vae_compile_ms = elapsed_ms(vae_compile_start);
        let vae_backend_name = vae.backend_name().to_string();

        let pipeline = Self {
            plan,
            latent_shape,
            conditioning,
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
        // Persistent-residency guard: the checkpoint's device weight caches
        // must never be silently dropped by allocation-failure recovery —
        // scratch/pool trimming may proceed, but losing resident weights
        // would re-stream gigabytes behind the caller's back. Explicit
        // eviction (model switch/unload) still works and clears this.
        crate::backend::gpu_weight_cache_protect_prefixes(pipeline.residency_namespaces())
            .map_err(DiffusionError::model)?;
        Ok((
            pipeline,
            FluxPipelineLoadTiming {
                runtime_init_ms,
                text_tokenize_ms,
                text_load_ms,
                text_compile_ms,
                text_execute_ms,
                transformer_load_ms,
                transformer_compile_ms,
                transformer_graph_build_ms: transformer_compile.graph_build_ms,
                transformer_graph_prepare_ms: transformer_compile.graph_prepare_ms,
                transformer_session_create_ms: transformer_compile.session_create_ms,
                vae_load_ms,
                vae_compile_ms,
                total_ms: elapsed_ms(total_start),
            },
        ))
    }

    /// True when this resident pipeline can serve a new job without touching
    /// its weights: the same resolved model files at the same image size
    /// (paths are the identity — same as the process-global device weight
    /// cache, which keys on the transformer path). Prompts may differ — swap
    /// them with [`Self::ensure_prompts_with_hooks`]; seed/steps/guidance
    /// are plain [`Self::generate_with_hooks`] arguments.
    pub fn serves_plan(
        &self,
        plan: &FluxPromptToImagePlan,
        image_width: Option<u32>,
        image_height: Option<u32>,
    ) -> bool {
        plan_reusable(&self.plan, self.latent_shape, plan, image_width, image_height)
    }

    /// Path of the resident diffusion transformer weights — the identity the
    /// process-global device weight cache keys on (and the part of
    /// [`Self::serves_plan`] that distinguishes flux1-schnell from
    /// flux1-dev; vae/text-encoder files are typically shared).
    pub fn diffusion_model_path(&self) -> &std::path::Path {
        &self.plan.bundle.diffusion_model_path
    }

    /// Identity of the LoRA adaptation merged into the resident transformer
    /// weights (`""` = pristine). The other half of the device weight-cache
    /// key: a caller replacing this pipeline must evict when EITHER the
    /// checkpoint path or this fingerprint changes.
    pub fn lora_fingerprint(&self) -> &str {
        &self.transformer_weights.lora_fingerprint
    }

    /// Device weight-cache namespaces of every resident component. All four
    /// key on their source file path, so a combined checkpoint shares one
    /// evictable checkpoint root.
    fn residency_namespaces(&self) -> Vec<String> {
        vec![
            crate::flux_transformer::flux_cache_namespace(&self.transformer_weights),
            crate::flux_vae::flux_vae_cache_namespace(&self.vae_weights),
            crate::t5_encoder::t5_cache_namespace(&self.text_weights.t5xxl),
            crate::clip_l::clip_cache_namespace(&self.text_weights.clip_l),
        ]
    }

    /// Frees EVERY device weight-cache entry of this pipeline's checkpoint —
    /// transformer, vae, t5 and clip namespaces (for a combined checkpoint
    /// they all key on the one file). Dropping the pipeline releases its
    /// HOST weight arenas but deliberately leaves the device caches warm;
    /// when a resident pipeline is being REPLACED by a different model on
    /// the same thread, call this first — the caches never evict on their
    /// own, and a 24GB card cannot hold two combined FP8 stacks. Also
    /// clears the OOM-protection registration so the outgoing checkpoint
    /// stops pinning recovery behavior. Returns the number of buffers freed.
    pub fn evict_device_caches(&self) -> usize {
        let _ = crate::backend::gpu_weight_cache_protect_prefixes(Vec::new());
        crate::flux_transformer::evict_device_weight_cache(&self.transformer_weights)
            + crate::flux_vae::evict_device_weight_cache(&self.vae_weights)
            + crate::t5_encoder::evict_device_weight_cache(&self.text_weights.t5xxl)
            + crate::clip_l::evict_device_weight_cache(&self.text_weights.clip_l)
    }

    /// Backward-compatible alias of [`Self::evict_device_caches`] from when
    /// only the transformer namespace needed eviction on a model switch
    /// (split bundles shared vae/text-encoder files between models; combined
    /// checkpoints share nothing).
    pub fn evict_transformer_device_cache(&self) -> usize {
        self.evict_device_caches()
    }

    /// Re-encodes the text conditioning for `prompts` on a resident
    /// pipeline; unchanged prompts return immediately (the no-load warm
    /// path). The transformer/vae weights and compiled state are reused
    /// untouched — the conditioning SHAPE is prompt-invariant (t5 pads to
    /// its fixed 256 tokens, clip_l truncates to one 77-token window, and
    /// every execute re-uploads the values) — and the loaded text-encoder
    /// weights are RESIDENT struct fields, so a prompt change re-tokenizes
    /// and re-encodes with zero disk reads and zero weight uploads (device
    /// caches warm). On error (including cancel) the pipeline keeps its
    /// previous prompts and conditioning: state only changes after a fully
    /// successful encode.
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
            // Per-block "text-encode t5 block 7/24" across this band.
            let mut sub = sub_hook(&mut hooks, 0.12, 0.86);
            text.execute_split(&self.text_weights, &tokenized, crate::hook_ref(&mut sub))?
        };

        self.conditioning = conditioning;
        self.plan.prompts = prompts.clone();
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
    ) -> Result<FluxPipelineGenerateRun> {
        self.generate_with_hooks(seed, steps, guidance, None)
    }

    /// [`Self::generate`] with per-denoise-step progress ("denoise k/N") and
    /// cancel checks between steps and around the VAE decode. Fractions span
    /// the whole generate 0..=1 (denoise 0..0.9, vae 0.9..1).
    pub fn generate_with_hooks(
        &self,
        seed: u64,
        steps: usize,
        guidance: f32,
        mut hooks: Option<&mut FluxRunHooks>,
    ) -> Result<FluxPipelineGenerateRun> {
        let total_start = Instant::now();
        let steps = steps.max(1);
        let schedule = FluxSchedule::for_flux1(steps, self.plan.transformer.guidance_embed)?;

        let noise_start = Instant::now();
        let mut latents = gaussian_latents(
            self.latent_shape.latent_width,
            self.latent_shape.latent_height,
            seed,
        );
        let noise_ms = elapsed_ms(noise_start);

        let pack_start = Instant::now();
        let mut packed = pack_flux_latents_nchw(
            &latents,
            1,
            self.latent_shape.latent_height,
            self.latent_shape.latent_width,
        )?;
        let pack_ms = elapsed_ms(pack_start);

        let denoise_start = Instant::now();
        for step_index in 0..steps {
            hook_check(&hooks)?;
            // Step 1 also streams any not-yet-resident unet weights into the
            // device cache on CUDA — label it so the cold-start hang reads
            // as work, not a stall.
            let label = if step_index == 0 {
                format!("denoise 1/{steps} (streaming weights)")
            } else {
                format!("denoise {}/{}", step_index + 1, steps)
            };
            let step_base = 0.9 * step_index as f64 / steps as f64;
            hook_emit(&mut hooks, &label, step_base);
            let sigma = schedule.sigmas[step_index];
            let sigma_next = schedule.sigmas[step_index + 1];
            // Within the step the lazy/device executor ticks "denoise k/N
            // block 12/57" — on the cold first step that's the weight
            // stream-in moving.
            let mut sub = sub_hook_emit_only(
                &mut hooks,
                format!("denoise {}/{} ", step_index + 1, steps),
                step_base,
                0.9 / steps as f64,
            );
            let run = self.transformer.execute_hooked(
                &self.transformer_weights,
                &self.conditioning,
                &packed,
                sigma,
                guidance,
                crate::hook_ref(&mut sub),
            )?;
            euler_step(&mut packed, &run.prediction, sigma, sigma_next)?;
        }
        let denoise_ms = elapsed_ms(denoise_start);
        hook_check(&hooks)?;
        hook_emit(&mut hooks, "vae-decode", 0.9);

        let unpack_start = Instant::now();
        latents = unpack_flux_latents_nchw(
            &packed,
            1,
            self.latent_shape.latent_height,
            self.latent_shape.latent_width,
        )?;
        let unpack_ms = elapsed_ms(unpack_start);

        let latent_rescale_start = Instant::now();
        for value in &mut latents {
            *value = (*value / FLUX_VAE_SCALING_FACTOR) + FLUX_VAE_SHIFT_FACTOR;
        }
        let latent_rescale_ms = elapsed_ms(latent_rescale_start);

        let vae_layout_start = Instant::now();
        let latents = nchw_to_whcb(
            &latents,
            1,
            16,
            self.latent_shape.latent_height as usize,
            self.latent_shape.latent_width as usize,
        )?;
        let vae_layout_ms = elapsed_ms(vae_layout_start);

        hook_check(&hooks)?;
        let vae_execute_start = Instant::now();
        let image = {
            // The lazy/device decode ticks "vae-decode block 5/19".
            let mut sub = sub_hook_emit_only(&mut hooks, "vae-decode ".to_string(), 0.9, 0.1);
            self.vae
                .execute_hooked(&self.vae_weights, &latents, crate::hook_ref(&mut sub))?
        };
        let vae_execute_ms = elapsed_ms(vae_execute_start);
        hook_check(&hooks)?;

        Ok(FluxPipelineGenerateRun {
            image,
            timing: FluxPipelineRunTiming {
                noise_ms,
                pack_ms,
                denoise_ms,
                unpack_ms,
                latent_rescale_ms,
                vae_layout_ms,
                vae_execute_ms,
                total_ms: elapsed_ms(total_start),
            },
        })
    }

    // -----------------------------------------------------------------
    // Structure-conditioned generation (FLUX.1-Depth-dev / FLUX.1-Canny-dev
    // and any other checkpoint whose `img_in` accepts noise-latent +
    // control-latent concatenated on the channel axis, matching diffusers'
    // `FluxControlPipeline`). Additions only — `load`/`generate_with_hooks`
    // above are unchanged except the one-line `transformer_channels`
    // generalization in `load_with_hooks`.
    // -----------------------------------------------------------------

    /// Encodes a control image into packed control latents ready for
    /// [`Self::generate_control_with_hooks`]: `pixel_chw_neg1_to_1` is
    /// interleaved-by-channel (`[c][y][x]`) RGB already scaled to `[-1,1]`
    /// and already sized to this pipeline's own
    /// `latent_shape().image_width`/`image_height` (the control pipeline's
    /// generation canvas — the caller resizes the source image to that size
    /// before calling this, there is no resize here). Runs the VAE encoder
    /// (`flux_vae::encode_control_image_hooked` — CUDA/Metal graph path when
    /// a runtime is available, pure-CPU fallback otherwise), converts the
    /// raw posterior mean into the trained-latent domain (the inverse of
    /// `generate_with_hooks`'s `latent / scale + shift` decode step), and
    /// packs it 16ch -> 64 features/token the same way the noise latents are
    /// packed.
    pub fn encode_control_image_with_hooks(
        &self,
        pixel_chw_neg1_to_1: &[f32],
        mut hooks: Option<&mut FluxRunHooks>,
    ) -> Result<Vec<f32>> {
        let vae_path = self
            .plan
            .bundle
            .vae_path
            .as_ref()
            .ok_or_else(|| DiffusionError::workflow("workflow bundle does not include vae"))?;
        let vae_prefix = self.plan.bundle.component_prefixes().vae;

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "vae-encode", 0.0);
        let mean = {
            let mut sub = sub_hook_emit_only(&mut hooks, String::new(), 0.0, 0.9);
            crate::flux_vae::encode_control_image_hooked(
                vae_path,
                vae_prefix,
                pixel_chw_neg1_to_1,
                self.latent_shape.image_width as usize,
                self.latent_shape.image_height as usize,
                crate::hook_ref(&mut sub),
            )?
        };

        hook_check(&hooks)?;
        hook_emit(&mut hooks, "vae-encode pack", 0.95);
        let mut latents = mean;
        for value in &mut latents {
            *value = (*value - FLUX_VAE_SHIFT_FACTOR) * FLUX_VAE_SCALING_FACTOR;
        }
        let packed = pack_flux_latents_nchw(
            &latents,
            1,
            self.latent_shape.latent_height,
            self.latent_shape.latent_width,
        )?;
        hook_emit(&mut hooks, "vae-encode", 1.0);
        Ok(packed)
    }

    /// [`Self::generate_with_hooks`] for a structure-conditioned checkpoint:
    /// identical noise/schedule/euler loop, except the transformer sees
    /// `[noise_packed(64) | control_packed(64)]` (128 features/token — this
    /// pipeline's `in_channels`, generalized at load time, see
    /// `load_with_hooks`) at every step, freshly re-concatenated each time
    /// since only the noise half changes. The transformer's PREDICTION stays
    /// 64 features/token (only the input embedding widened, never the
    /// output — `out_channels` is unchanged from the plain dev checkpoint),
    /// so the euler step, unpack, rescale and VAE decode below are the same
    /// steps `generate_with_hooks` runs. `control_latents_packed` comes from
    /// [`Self::encode_control_image_with_hooks`] and must be exactly
    /// `latent_shape().image_token_count * 64` values — asserted, never
    /// silently truncated or padded.
    pub fn generate_control_with_hooks(
        &self,
        control_latents_packed: &[f32],
        seed: u64,
        steps: usize,
        guidance: f32,
        mut hooks: Option<&mut FluxRunHooks>,
    ) -> Result<FluxPipelineGenerateRun> {
        let total_start = Instant::now();
        let steps = steps.max(1);
        let tokens = self.latent_shape.image_token_count as usize;
        let expected_control = tokens
            .checked_mul(64)
            .ok_or_else(|| DiffusionError::model("flux control latent token count overflow"))?;
        if control_latents_packed.len() != expected_control {
            return Err(DiffusionError::workflow(format!(
                "flux control generate expected {} packed control-latent values ({} tokens x 64 channels), got {}",
                expected_control,
                tokens,
                control_latents_packed.len()
            )));
        }
        let schedule = FluxSchedule::for_flux1(steps, self.plan.transformer.guidance_embed)?;

        let noise_start = Instant::now();
        let mut latents = gaussian_latents(
            self.latent_shape.latent_width,
            self.latent_shape.latent_height,
            seed,
        );
        let noise_ms = elapsed_ms(noise_start);

        let pack_start = Instant::now();
        let mut packed = pack_flux_latents_nchw(
            &latents,
            1,
            self.latent_shape.latent_height,
            self.latent_shape.latent_width,
        )?;
        let pack_ms = elapsed_ms(pack_start);

        let denoise_start = Instant::now();
        for step_index in 0..steps {
            hook_check(&hooks)?;
            let label = if step_index == 0 {
                format!("denoise 1/{steps} (streaming weights)")
            } else {
                format!("denoise {}/{}", step_index + 1, steps)
            };
            let step_base = 0.9 * step_index as f64 / steps as f64;
            hook_emit(&mut hooks, &label, step_base);
            let sigma = schedule.sigmas[step_index];
            let sigma_next = schedule.sigmas[step_index + 1];
            // Re-concatenated every step: only `packed` (the noise half)
            // changes between steps, `control_latents_packed` is static
            // conditioning computed once by encode_control_image_with_hooks.
            let combined =
                concat_packed_latent_channels(&packed, control_latents_packed, tokens, 64, 64)?;
            let mut sub = sub_hook_emit_only(
                &mut hooks,
                format!("denoise {}/{} ", step_index + 1, steps),
                step_base,
                0.9 / steps as f64,
            );
            let run = self.transformer.execute_hooked(
                &self.transformer_weights,
                &self.conditioning,
                &combined,
                sigma,
                guidance,
                crate::hook_ref(&mut sub),
            )?;
            euler_step(&mut packed, &run.prediction, sigma, sigma_next)?;
        }
        let denoise_ms = elapsed_ms(denoise_start);
        hook_check(&hooks)?;
        hook_emit(&mut hooks, "vae-decode", 0.9);

        let unpack_start = Instant::now();
        latents = unpack_flux_latents_nchw(
            &packed,
            1,
            self.latent_shape.latent_height,
            self.latent_shape.latent_width,
        )?;
        let unpack_ms = elapsed_ms(unpack_start);

        let latent_rescale_start = Instant::now();
        for value in &mut latents {
            *value = (*value / FLUX_VAE_SCALING_FACTOR) + FLUX_VAE_SHIFT_FACTOR;
        }
        let latent_rescale_ms = elapsed_ms(latent_rescale_start);

        let vae_layout_start = Instant::now();
        let latents = nchw_to_whcb(
            &latents,
            1,
            16,
            self.latent_shape.latent_height as usize,
            self.latent_shape.latent_width as usize,
        )?;
        let vae_layout_ms = elapsed_ms(vae_layout_start);

        hook_check(&hooks)?;
        let vae_execute_start = Instant::now();
        let image = {
            let mut sub = sub_hook_emit_only(&mut hooks, "vae-decode ".to_string(), 0.9, 0.1);
            self.vae
                .execute_hooked(&self.vae_weights, &latents, crate::hook_ref(&mut sub))?
        };
        let vae_execute_ms = elapsed_ms(vae_execute_start);
        hook_check(&hooks)?;

        Ok(FluxPipelineGenerateRun {
            image,
            timing: FluxPipelineRunTiming {
                noise_ms,
                pack_ms,
                denoise_ms,
                unpack_ms,
                latent_rescale_ms,
                vae_layout_ms,
                vae_execute_ms,
                total_ms: elapsed_ms(total_start),
            },
        })
    }
}

pub fn encode_png_rgb(image_whcb: &[f32], width: usize, height: usize) -> Result<Vec<u8>> {
    let expected = width
        .checked_mul(height)
        .and_then(|value| value.checked_mul(3))
        .ok_or_else(|| DiffusionError::model("png encode size overflow"))?;
    if image_whcb.len() != expected {
        return Err(DiffusionError::model(format!(
            "png encode expected {} float values, got {}",
            expected,
            image_whcb.len()
        )));
    }
    let mut pixels = Vec::with_capacity(width * height * 4);
    let plane = width * height;
    for y in 0..height {
        for x in 0..width {
            let pixel = y * width + x;
            let r = to_u8(image_whcb[pixel]);
            let g = to_u8(image_whcb[plane + pixel]);
            let b = to_u8(image_whcb[plane * 2 + pixel]);
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
    }
    let options = EncoderOptions::default()
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);
    let mut encoder = PngEncoder::new(&pixels, options);
    let mut out = Vec::new();
    encoder
        .encode(&mut out)
        .map_err(|err| DiffusionError::model(format!("png encode failed: {err:?}")))?;
    Ok(out)
}

/// The pure warm-reuse check behind [`FluxPipeline::serves_plan`]: a
/// resident pipeline (loaded for `current` at `current_shape`) can serve
/// `next` at the requested image size iff the latent shape, every resolved
/// model file path AND the LoRA set match. Prompt differences are fine (see
/// [`FluxPipeline::ensure_prompts_with_hooks`]); generation settings (seed,
/// steps, guidance) are per-generate arguments and never key the pipeline.
///
/// LoRAs are merged into the resident weight bytes, so a different adapter
/// set or strength cannot be served warm — it rebuilds. Strength 0 and "no
/// LoRAs" share one identity (both are the pristine model).
fn plan_reusable(
    current: &FluxPromptToImagePlan,
    current_shape: FluxLatentShape,
    next: &FluxPromptToImagePlan,
    image_width: Option<u32>,
    image_height: Option<u32>,
) -> bool {
    let width = image_width.unwrap_or(next.generation.width);
    let height = image_height.unwrap_or(width);
    let Ok(latent_shape) = FluxLatentShape::from_image_size(width, height) else {
        return false;
    };
    latent_shape == current_shape
        && next.bundle.diffusion_model_path == current.bundle.diffusion_model_path
        && next.bundle.vae_path == current.bundle.vae_path
        && next.bundle.clip_l_path == current.bundle.clip_l_path
        && next.bundle.t5xxl_path == current.bundle.t5xxl_path
        && next.loras.fingerprint() == current.loras.fingerprint()
}

pub(crate) fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

/// Crate-visible so `flux_fill_pipeline.rs` seeds its noise identically to
/// flux1-dev/schnell (same seed -> same initial packed latents).
pub(crate) fn gaussian_latents(latent_width: u32, latent_height: u32, seed: u64) -> Vec<f32> {
    let count = 16usize * latent_width as usize * latent_height as usize;
    let mut rng = XorShift64::new(seed);
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let u1 = rng.next_unit().max(1.0e-7);
        let u2 = rng.next_unit();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f32::consts::PI * u2;
        out.push(r * theta.cos());
        if out.len() < count {
            out.push(r * theta.sin());
        }
    }
    out
}

pub(crate) fn nchw_to_whcb(
    input: &[f32],
    batch: usize,
    channels: usize,
    height: usize,
    width: usize,
) -> Result<Vec<f32>> {
    let expected = batch
        .checked_mul(channels)
        .and_then(|value| value.checked_mul(height))
        .and_then(|value| value.checked_mul(width))
        .ok_or_else(|| DiffusionError::model("nchw_to_whcb size overflow"))?;
    if input.len() != expected {
        return Err(DiffusionError::model(format!(
            "nchw_to_whcb expected {} values, got {}",
            expected,
            input.len()
        )));
    }
    Ok(input.to_vec())
}

fn to_u8(value: f32) -> u8 {
    let value = ((value + 1.0) * 0.5 * 255.0).clamp(0.0, 255.0);
    value.round() as u8
}

#[derive(Clone, Debug)]
struct XorShift64 {
    state: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_hooks_remap_fractions_and_poll_cancel() {
        let mut seen: Vec<(String, f64)> = Vec::new();
        let cancelled;
        {
            let mut progress = |label: &str, fraction: f64| {
                seen.push((label.to_string(), fraction));
            };
            let cancel = || false;
            let mut run_hooks = FluxRunHooks {
                progress: &mut progress,
                cancel: &cancel,
            };
            let mut hooks = Some(&mut run_hooks);

            // Band remap: phase-local 0.5 lands mid-band, out-of-range clamps.
            let mut sub = sub_hook(&mut hooks, 0.2, 0.4);
            let hook = crate::hook_ref(&mut sub).expect("hooks present");
            hook("load t5 4.8/9.5GB", 0.5).unwrap();
            hook("load t5 9.5/9.5GB", 1.5).unwrap();
            drop(sub);

            // Emit-only prefixing.
            let mut sub =
                sub_hook_emit_only(&mut hooks, "denoise 1/4 ".to_string(), 0.0, 0.9);
            let hook = crate::hook_ref(&mut sub).expect("hooks present");
            hook("block 2/57", 1.0 / 57.0).unwrap();
        }
        {
            let mut progress = |_: &str, _: f64| panic!("must not emit after cancel");
            let cancel = || true;
            let mut run_hooks = FluxRunHooks {
                progress: &mut progress,
                cancel: &cancel,
            };
            let mut hooks = Some(&mut run_hooks);
            let mut sub = sub_hook(&mut hooks, 0.0, 1.0);
            let hook = crate::hook_ref(&mut sub).expect("hooks present");
            cancelled = matches!(hook("load t5", 0.0), Err(DiffusionError::Cancelled));
        }

        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].0, "load t5 4.8/9.5GB");
        assert!((seen[0].1 - 0.4).abs() < 1e-9);
        assert!((seen[1].1 - 0.6).abs() < 1e-9, "fraction must clamp to band end");
        assert_eq!(seen[2].0, "denoise 1/4 block 2/57");
        assert!((seen[2].1 - 0.9 / 57.0).abs() < 1e-9);
        assert!(cancelled, "sub_hook must surface cancel as Err(Cancelled)");
    }

    #[test]
    fn byte_progress_formats_gb_and_skips_absent_hook() {
        let mut seen: Vec<(String, f64)> = Vec::new();
        let mut hook = |label: &str, fraction: f64| -> Result<()> {
            seen.push((label.to_string(), fraction));
            Ok(())
        };
        let mut progress: Option<crate::ProgressHook> = Some(&mut hook);
        const GB: usize = 1024 * 1024 * 1024;
        crate::emit_byte_progress(&mut progress, "load unet", 8 * GB + GB / 5, 24 * GB)
            .unwrap();
        assert_eq!(seen[0].0, "load unet 8.2/24.0GB");
        assert!((seen[0].1 - (8.2 / 24.0)).abs() < 1e-3);

        // Absent hook: no formatting, no error.
        crate::emit_byte_progress(&mut None, "load unet", 0, 0).unwrap();
    }

    #[test]
    fn plan_reuse_keys_on_files_and_size_not_prompts() {
        use crate::comfy::{FluxGenerationConfig, FluxWorkflowKind};
        use crate::flux::{FluxResolvedBundle, FluxTransformerConfig};
        use crate::flux_lora::{FluxLoraRef, FluxLoraStack};
        use std::path::PathBuf;

        let lora_stack = |entries: &[(&str, f32)]| {
            FluxLoraStack::new(
                entries
                    .iter()
                    .map(|(name, strength)| FluxLoraRef {
                        name: name.to_string(),
                        path: PathBuf::from(format!("loras/{name}.safetensors")),
                        strength: *strength,
                    })
                    .collect(),
            )
        };
        let plan_for = |unet: &str, prompt: &str, width: u32, height: u32| {
            let latent_shape = FluxLatentShape::from_image_size(width, height).unwrap();
            FluxPromptToImagePlan {
                workflow_path: PathBuf::from("<test>"),
                kind: FluxWorkflowKind::SplitModel,
                bundle: FluxResolvedBundle {
                    kind: FluxWorkflowKind::SplitModel,
                    diffusion_model_path: PathBuf::from(unet),
                    vae_path: Some(PathBuf::from("vae/ae.safetensors")),
                    clip_l_path: Some(PathBuf::from("text_encoders/clip_l.safetensors")),
                    t5xxl_path: Some(PathBuf::from("text_encoders/t5xxl_fp16.safetensors")),
                },
                prompts: FluxPrompts {
                    clip_l: prompt.to_string(),
                    t5xxl: prompt.to_string(),
                    negative: String::new(),
                },
                generation: FluxGenerationConfig {
                    width,
                    height,
                    batch_size: 1,
                    seed: 7,
                    steps: 4,
                    cfg: 1.0,
                    denoise: 1.0,
                    guidance: 3.5,
                    sampler_name: "euler".to_string(),
                    scheduler: "simple".to_string(),
                },
                latent_shape,
                transformer: FluxTransformerConfig::flux1_dev(),
                loras: FluxLoraStack::default(),
            }
        };

        let current = plan_for("unet/flux1-schnell.safetensors", "a red fox", 512, 512);
        let shape = current.latent_shape;

        // Same files + size: reusable — prompts and generation settings
        // (seed differs per job) never key the pipeline.
        let same = plan_for("unet/flux1-schnell.safetensors", "a blue boat", 512, 512);
        assert!(plan_reusable(&current, shape, &same, Some(512), Some(512)));

        // Different model file: rebuild.
        let other_model = plan_for("unet/flux1-dev.safetensors", "a red fox", 512, 512);
        assert!(!plan_reusable(&current, shape, &other_model, Some(512), Some(512)));

        // Different image size: rebuild (compiled graphs are shape-bound).
        let other_size = plan_for("unet/flux1-schnell.safetensors", "a red fox", 1024, 1024);
        assert!(!plan_reusable(&current, shape, &other_size, Some(1024), Some(1024)));

        // Explicit size args override the plan's generation size.
        assert!(plan_reusable(&current, shape, &other_size, Some(512), Some(512)));

        // Absent size args fall back to the plan's generation config.
        assert!(plan_reusable(&current, shape, &same, None, None));
        assert!(!plan_reusable(&current, shape, &other_size, None, None));

        // LoRAs are merged into the resident weights, so they key the
        // pipeline: adding, removing or restrengthening one rebuilds.
        let mut with_lora = plan_for("unet/flux1-schnell.safetensors", "a red fox", 512, 512);
        with_lora.loras = lora_stack(&[("style", 0.8)]);
        assert!(!plan_reusable(&current, shape, &with_lora, Some(512), Some(512)));
        assert!(!plan_reusable(&with_lora, shape, &current, Some(512), Some(512)));

        let mut same_lora = plan_for("unet/flux1-schnell.safetensors", "a blue boat", 512, 512);
        same_lora.loras = lora_stack(&[("style", 0.8)]);
        assert!(plan_reusable(&with_lora, shape, &same_lora, Some(512), Some(512)));

        let mut other_strength = plan_for("unet/flux1-schnell.safetensors", "a red fox", 512, 512);
        other_strength.loras = lora_stack(&[("style", 0.4)]);
        assert!(!plan_reusable(&with_lora, shape, &other_strength, Some(512), Some(512)));

        // Strength 0 == pristine: reuses the un-adapted pipeline.
        let mut zero_strength = plan_for("unet/flux1-schnell.safetensors", "a red fox", 512, 512);
        zero_strength.loras = lora_stack(&[("style", 0.0)]);
        assert!(plan_reusable(&current, shape, &zero_strength, Some(512), Some(512)));
    }
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        let state = if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        };
        Self { state }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_unit(&mut self) -> f32 {
        let bits = (self.next_u64() >> 40) as u32;
        bits as f32 / (1u32 << 24) as f32
    }
}
