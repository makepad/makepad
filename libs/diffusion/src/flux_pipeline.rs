use crate::backend::{new_runtime, runtime_available};
use crate::flux::{
    pack_flux_latents_nchw, unpack_flux_latents_nchw, FluxLatentShape, FluxPromptToImagePlan,
};
use crate::flux_schedule::{
    euler_step, FluxSchedule, FLUX_VAE_SCALING_FACTOR, FLUX_VAE_SHIFT_FACTOR,
};
use crate::flux_text::{
    FluxCompiledTextEncoders, FluxConditioning, FluxLoadedTextEncoders, FluxTokenizedPrompts,
};
use crate::flux_transformer::{CompiledFluxTransformer, LoadedFluxTransformerWeights};
use crate::flux_vae::{CompiledFluxVae, FluxVaeDecodeRun, LoadedFluxVaeWeights};
use crate::{DiffusionError, Result};
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

pub struct FluxPipeline {
    plan: FluxPromptToImagePlan,
    latent_shape: FluxLatentShape,
    conditioning: FluxConditioning,
    clip_backend_name: String,
    t5_backend_name: String,
    transformer_backend_name: String,
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
        let total_start = Instant::now();
        let width = image_width.unwrap_or(plan.generation.width);
        let height = image_height.unwrap_or(width);
        let latent_shape = FluxLatentShape::from_image_size(width, height)?;

        let tokenize_start = Instant::now();
        let prompts = FluxTokenizedPrompts::from_prompts(&plan.prompts)?;
        let text_tokenize_ms = elapsed_ms(tokenize_start);

        let text_load_start = Instant::now();
        let mut text_weights = FluxLoadedTextEncoders::load_from_plan(&plan)?;
        let text_load_ms = elapsed_ms(text_load_start);

        let text_compile_start = Instant::now();
        let text = FluxCompiledTextEncoders::compile(&mut text_weights, &prompts)?;
        let text_compile_ms = elapsed_ms(text_compile_start);
        let clip_backend_name = text.clip_backend_name().to_string();
        let t5_backend_name = text.t5_backend_name().to_string();

        let text_execute_start = Instant::now();
        let conditioning = text.execute(&text_weights, &prompts)?;
        let text_execute_ms = elapsed_ms(text_execute_start);

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

        let transformer_load_start = Instant::now();
        let mut transformer_weights =
            LoadedFluxTransformerWeights::load(&plan.bundle.diffusion_model_path)?;
        let transformer_load_ms = elapsed_ms(transformer_load_start);

        let transformer_compile_start = Instant::now();
        let (transformer, transformer_compile) = match runtime.clone() {
            Some(runtime) => CompiledFluxTransformer::compile_with_runtime_profiled(
                runtime,
                &mut transformer_weights,
                &conditioning,
                latent_shape,
            )?,
            None => CompiledFluxTransformer::compile_profiled(
                &mut transformer_weights,
                &conditioning,
                latent_shape,
            )?,
        };
        let transformer_compile_ms = elapsed_ms(transformer_compile_start);
        let transformer_backend_name = transformer.backend_name().to_string();

        let vae_path = plan
            .bundle
            .vae_path
            .as_ref()
            .ok_or_else(|| DiffusionError::workflow("workflow bundle does not include vae"))?;
        let vae_load_start = Instant::now();
        let mut vae_weights = LoadedFluxVaeWeights::load(vae_path)?;
        let vae_load_ms = elapsed_ms(vae_load_start);

        let vae_compile_start = Instant::now();
        let vae = match runtime {
            Some(runtime) => {
                CompiledFluxVae::compile_with_runtime(runtime, &mut vae_weights, latent_shape)?
            }
            None => CompiledFluxVae::compile(&mut vae_weights, latent_shape)?,
        };
        let vae_compile_ms = elapsed_ms(vae_compile_start);
        let vae_backend_name = vae.backend_name().to_string();

        Ok((
            Self {
                plan,
                latent_shape,
                conditioning,
                clip_backend_name,
                t5_backend_name,
                transformer_backend_name,
                transformer_weights,
                transformer,
                vae_backend_name,
                vae_weights,
                vae,
            },
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
            let sigma = schedule.sigmas[step_index];
            let sigma_next = schedule.sigmas[step_index + 1];
            let run = self.transformer.execute(
                &self.transformer_weights,
                &self.conditioning,
                &packed,
                sigma,
                guidance,
            )?;
            euler_step(&mut packed, &run.prediction, sigma, sigma_next)?;
        }
        let denoise_ms = elapsed_ms(denoise_start);

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

        let vae_execute_start = Instant::now();
        let image = self.vae.execute(&self.vae_weights, &latents)?;
        let vae_execute_ms = elapsed_ms(vae_execute_start);

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

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn gaussian_latents(latent_width: u32, latent_height: u32, seed: u64) -> Vec<f32> {
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

fn nchw_to_whcb(
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
