//! FLUX.2-klein-4B image-edit pipeline.
//!
//! Instruction + one or more reference images → packed latents on T-axis
//! offsets 10, 20, … → 4-step distilled Euler denoise → VAE decode.
//! Fail-closed without CUDA.

use crate::backend::gpu_device_available;
use crate::flux2::{
    flux2_concat_ref_tokens, flux2_image_ids, flux2_ref_time_offset, flux2_schedule, flux2_text_ids,
    Flux2PackedLatents, Flux2PosId, Flux2WeightFile, Mistral3TextConfig, Qwen3TextConfig,
    FLUX2_SYSTEM_MESSAGE,
};
use crate::flux2_dev_text::{flux2_dev_text_encode, Flux2DevTextPrepared};
use crate::flux2_klein_text::{
    flux2_klein_text_encode, flux2_klein_text_release, flux2_klein_tokenize, flux2_klein_tokenizer_load,
    Flux2KleinTextPrepared,
};
use crate::flux2_tokenizer::{Flux2Tokenizer, FLUX2_MAX_SEQUENCE_LENGTH};
use crate::flux2_transformer::{
    flux2_dit_clear_pool, flux2_euler_step, flux2_transformer_forward, Flux2TransformerWeights,
};
use crate::flux2_vae::{
    flux2_image_to_rgb_u8, flux2_vae_decode, flux2_vae_encode, Flux2VaeImage,
    Flux2VaeWeights,
};
use crate::flux_pipeline::encode_png_rgb;
use crate::{DiffusionError, Result};
use std::path::{Path, PathBuf};

pub const FLUX2_KLEIN_DEFAULT_STEPS: usize = 4;
pub const FLUX2_KLEIN_DEFAULT_SIZE: u32 = 512;

pub struct Flux2KleinPaths {
    pub transformer: PathBuf,
    pub text_encoder: PathBuf,
    pub tokenizer: PathBuf,
    pub vae: PathBuf,
}

pub struct Flux2KleinPipeline {
    pub paths: Flux2KleinPaths,
    pub transformer: Flux2TransformerWeights,
    pub text_encoder: Flux2WeightFile,
    pub text_prepared: Flux2KleinTextPrepared,
    pub tokenizer: makepad_ai_h3::h3_tokenizer::H3Tokenizer,
    pub vae: Flux2VaeWeights,
}

/// img2img init for the FLUX.2 samplers (Klein edit, dev generate/edit): the
/// image to start from (same size as the output, multiple of 16) and the
/// denoise `strength` in `[0, 1]`. ComfyUI semantics for a flow model with
/// CONST noise scaling: the init is VAE-encoded to packed latents `z0`, the
/// sampler starts at sigma index `k = floor((1 - strength) * steps)` of the
/// full schedule with `x = sigma_k * noise + (1 - sigma_k) * z0`, and runs
/// the remaining `steps - k` steps. `strength = 1` is the plain t2i/edit run
/// (`k = 0`, x = noise), `strength = 0` returns the init re-encoded
/// (`k = steps`, no denoise). Fewer steps run at lower strength — the live
/// feed loop relies on that.
#[derive(Clone, Debug)]
pub struct Flux2Img2Img {
    pub image: Flux2VaeImage,
    pub strength: f32,
}

/// Resolve the img2img start: `(start_step, sample)`. `noise` is the full
/// sigma-1 noise (`[gen_tokens, 128]` token-major); `sigmas` the full
/// `steps + 1` schedule.
fn flux2_img2img_start(
    vae: &Flux2VaeWeights,
    init: &Flux2Img2Img,
    noise: Vec<f32>,
    sigmas: &[f32],
    steps: usize,
    packed_w: usize,
    packed_h: usize,
) -> Result<(usize, Vec<f32>)> {
    if !(0.0..=1.0).contains(&init.strength) || !init.strength.is_finite() {
        return Err(DiffusionError::workflow(format!(
            "flux2 img2img strength must be in [0, 1], got {}",
            init.strength
        )));
    }
    if init.image.width != packed_w * 16 || init.image.height != packed_h * 16 {
        return Err(DiffusionError::workflow(format!(
            "flux2 img2img init is {}x{}, output is {}x{} — the init must match the output size",
            init.image.width,
            init.image.height,
            packed_w * 16,
            packed_h * 16
        )));
    }
    let start = (((1.0 - init.strength) * steps as f32).floor() as usize).min(steps);
    if start == 0 {
        return Ok((0, noise));
    }
    let packed = flux2_vae_encode(vae, &init.image)?;
    if packed.width != packed_w || packed.height != packed_h || packed.channels != 128 {
        return Err(DiffusionError::workflow(format!(
            "flux2 img2img init encoded to {}x{}x{}, expected {packed_w}x{packed_h}x128",
            packed.width, packed.height, packed.channels
        )));
    }
    let z0 = packed.to_tokens();
    if z0.len() != noise.len() {
        return Err(DiffusionError::workflow(format!(
            "flux2 img2img init has {} latent values, noise has {}",
            z0.len(),
            noise.len()
        )));
    }
    let sigma = sigmas[start];
    let sample: Vec<f32> = noise
        .iter()
        .zip(z0.iter())
        .map(|(n, z)| sigma * n + (1.0 - sigma) * z)
        .collect();
    Ok((start, sample))
}

#[derive(Clone, Debug)]
pub struct Flux2EditRequest {
    pub prompt: String,
    pub width: u32,
    pub height: u32,
    pub steps: usize,
    pub seed: u64,
    /// RGB8 reference images, already cropped to a multiple of 16.
    pub references: Vec<Flux2VaeImage>,
    /// Optional teacher noise (token-major `[gen_tokens, 128]`) from an
    /// oracle dump. When absent a host SplitMix64 fill is used (NOT
    /// torch-philox; pin against a dumped `noise` for numeric parity).
    pub noise: Option<Vec<f32>>,
    /// Optional oracle ref tokens (token-major `[ref_tokens, 128]`) to
    /// isolate the DiT from VAE encode error.
    pub teacher_ref_tokens: Option<Vec<f32>>,
    /// Optional oracle prompt embeds (token-major `[512, 7680]`).
    pub teacher_embeds: Option<Vec<f32>>,
    /// Optional img2img init (see [`Flux2Img2Img`]): start the sampler from
    /// this image at the given strength instead of from pure noise. The
    /// `references` still condition the edit as before.
    pub init: Option<Flux2Img2Img>,
}

#[derive(Clone, Debug)]
pub struct Flux2EditResult {
    pub image: Flux2VaeImage,
    pub png: Vec<u8>,
    pub packed_latents: Flux2PackedLatents,
    pub ref_packed: Flux2PackedLatents,
    pub input_ids: Vec<u32>,
    pub real_len: usize,
    pub prompt_embeds: Vec<f32>,
    pub step_residuals: Vec<Vec<f32>>,
    pub sigmas: Vec<f32>,
    pub warm_ms: f64,
    /// Per-stage wall times for the same call `warm_ms` covers. Every stage
    /// ends in a device sync (each finishes with a gpu_download or a host
    /// step), so these are honest per-stage walls, not queue-depth artifacts.
    pub te_ms: f64,
    pub encode_ms: f64,
    pub decode_ms: f64,
    pub png_ms: f64,
    pub total_ms: f64,
}

impl Flux2KleinPipeline {
    pub fn load(paths: Flux2KleinPaths) -> Result<Self> {
        if !gpu_device_available() {
            return Err(DiffusionError::workflow(
                "flux2-klein-4b requires CUDA (MAKEPAD_GGML_REQUIRE_CUDA=1)",
            ));
        }
        let tokenizer = flux2_klein_tokenizer_load(&paths.tokenizer)?;
        let text_encoder = Flux2WeightFile::load(&paths.text_encoder)?;
        let text_prepared =
            Flux2KleinTextPrepared::prepare(&text_encoder, Qwen3TextConfig::flux2_klein_4b())?;
        let vae = Flux2VaeWeights::load(&paths.vae)?;
        let transformer = Flux2TransformerWeights::load(&paths.transformer)?;
        if transformer.config.guidance_embed {
            return Err(DiffusionError::model(
                "flux2-klein-4b loader received a guidance_embed=true transformer",
            ));
        }
        Ok(Self {
            paths,
            transformer,
            text_encoder,
            text_prepared,
            tokenizer,
            vae,
        })
    }

    pub fn edit(&mut self, request: &Flux2EditRequest) -> Result<Flux2EditResult> {
        if request.width % 16 != 0 || request.height % 16 != 0 {
            return Err(DiffusionError::workflow(format!(
                "flux2 edit size must be a multiple of 16, got {}x{}",
                request.width, request.height
            )));
        }
        if request.references.is_empty() && request.teacher_ref_tokens.is_none() {
            return Err(DiffusionError::workflow(
                "flux2 edit requires at least one reference image",
            ));
        }
        let total_started = std::time::Instant::now();
        let tokenized = flux2_klein_tokenize(&self.tokenizer, &request.prompt)?;
        let prompt_embeds = if let Some(embeds) = &request.teacher_embeds {
            embeds.clone()
        } else {
            let embeds =
                flux2_klein_text_encode(&self.text_encoder, &self.text_prepared, &tokenized)?;
            // TE weights stay device-resident by default (re-uploading ~8 GB
            // per call dominated warm TE). MAKEPAD_FLUX2_TE_RELEASE=1 restores
            // the evict-after-encode behavior for VRAM-constrained flows.
            if std::env::var("MAKEPAD_FLUX2_TE_RELEASE").as_deref() == Ok("1") {
                let _ = flux2_klein_text_release();
            }
            embeds
        };
        let te_ms = total_started.elapsed().as_secs_f64() * 1000.0;

        let encode_started = std::time::Instant::now();
        let mut ref_packed = Vec::new();
        if let Some(tokens) = &request.teacher_ref_tokens {
            let packed_w = (request.width / 16) as usize;
            let packed_h = (request.height / 16) as usize;
            let packed = Flux2PackedLatents::from_tokens(tokens, packed_w, packed_h, 128)?;
            let ids = flux2_image_ids(packed.width, packed.height, flux2_ref_time_offset(0));
            ref_packed.push((tokens.clone(), ids, packed));
        } else {
            for (index, image) in request.references.iter().enumerate() {
                let packed = flux2_vae_encode(&self.vae, image)?;
                let tokens = packed.to_tokens();
                let ids = flux2_image_ids(packed.width, packed.height, flux2_ref_time_offset(index));
                ref_packed.push((tokens, ids, packed));
            }
        }
        let encode_ms = encode_started.elapsed().as_secs_f64() * 1000.0;

        let packed_w = (request.width / 16) as usize;
        let packed_h = (request.height / 16) as usize;
        let gen_tokens = packed_w * packed_h;
        let channels = 128usize;
        let mut sample = match &request.noise {
            Some(noise) => {
                if noise.len() != gen_tokens * channels {
                    return Err(DiffusionError::workflow(format!(
                        "flux2 noise expected {} values, got {}",
                        gen_tokens * channels,
                        noise.len()
                    )));
                }
                noise.clone()
            }
            None => splitmix_noise(request.seed, gen_tokens * channels),
        };
        let gen_ids = flux2_image_ids(packed_w, packed_h, 0);
        let refs: Vec<(Vec<f32>, Vec<Flux2PosId>)> = ref_packed
            .iter()
            .map(|(tokens, ids, _)| (tokens.clone(), ids.clone()))
            .collect();
        let txt_ids = flux2_text_ids(tokenized.token_ids.len());
        let sigmas = flux2_schedule(request.steps, gen_tokens)?;
        let start_step = match &request.init {
            Some(init) => {
                let (start, mixed) =
                    flux2_img2img_start(&self.vae, init, sample, &sigmas, request.steps, packed_w, packed_h)?;
                sample = mixed;
                start
            }
            None => 0,
        };

        let prof = std::env::var_os("MAKEPAD_GPU_PROF").is_some();
        if prof {
            // Discard text-encode/VAE-encode counters so the denoise report
            // covers only the loop below.
            let _ = makepad_ai_common::backend::prof::report_and_reset("");
        }
        let started = std::time::Instant::now();
        let mut step_residuals = Vec::new();
        for step in start_step..request.steps {
            let step_started = std::time::Instant::now();
            let (img_tokens, img_ids) = flux2_concat_ref_tokens(&sample, &gen_ids, &refs);
            let run = flux2_transformer_forward(
                &self.transformer,
                &img_tokens,
                &img_ids,
                &prompt_embeds,
                &txt_ids,
                sigmas[step],
                None,
                gen_tokens,
            )?;
            if step == request.steps / 2 || step == start_step {
                step_residuals.push(run.prediction.clone());
            }
            flux2_euler_step(&mut sample, &run.prediction, sigmas[step], sigmas[step + 1])?;
            if prof {
                eprintln!(
                    "flux2 prof step{step} ms={:.1}",
                    step_started.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
        let warm_ms = started.elapsed().as_secs_f64() * 1000.0;
        if prof {
            eprint!(
                "{}",
                makepad_ai_common::backend::prof::report_and_reset("flux2 prof denoise ")
            );
        }
        flux2_dit_clear_pool();

        let packed = Flux2PackedLatents::from_tokens(&sample, packed_w, packed_h, channels)?;
        let decode_started = std::time::Instant::now();
        let image = flux2_vae_decode(&self.vae, &packed)?;
        let decode_ms = decode_started.elapsed().as_secs_f64() * 1000.0;
        if prof {
            eprintln!("flux2 prof vae_decode ms={decode_ms:.1}");
            eprint!(
                "{}",
                makepad_ai_common::backend::prof::report_and_reset("flux2 prof vae ")
            );
        }
        let png_started = std::time::Instant::now();
        let rgb = flux2_image_to_rgb_u8(&image);
        let png = encode_png_rgb(
            &planar_rgb_to_whcb(&rgb, image.width, image.height),
            image.width,
            image.height,
        )?;
        let png_ms = png_started.elapsed().as_secs_f64() * 1000.0;
        Ok(Flux2EditResult {
            image,
            png,
            packed_latents: packed,
            ref_packed: ref_packed[0].2.clone(),
            input_ids: tokenized.token_ids,
            real_len: tokenized.real_len,
            prompt_embeds,
            step_residuals,
            sigmas,
            warm_ms,
            te_ms,
            encode_ms,
            decode_ms,
            png_ms,
            total_ms: total_started.elapsed().as_secs_f64() * 1000.0,
        })
    }
}

fn planar_rgb_to_whcb(rgb: &[u8], width: usize, height: usize) -> Vec<f32> {
    // encode_png_rgb's to_u8 maps [-1,1] -> [0,255] (flux1 hands it raw
    // decoder output). Feed it [-1,1] so the u8 roundtrip is exact; the
    // earlier [0,1] feed applied the (x+1)/2 remap TWICE and washed out
    // every saved PNG (the validator's u8 gates never read the file, so
    // they were unaffected).
    let plane = width * height;
    let mut out = vec![0.0f32; plane * 3];
    for i in 0..plane {
        out[i] = rgb[i * 3] as f32 * (2.0 / 255.0) - 1.0;
        out[plane + i] = rgb[i * 3 + 1] as f32 * (2.0 / 255.0) - 1.0;
        out[2 * plane + i] = rgb[i * 3 + 2] as f32 * (2.0 / 255.0) - 1.0;
    }
    out
}

fn splitmix_noise(seed: u64, n: usize) -> Vec<f32> {
    let mut state = seed.wrapping_add(0x9E3779B97F4A7C15);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        state = state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
        let u1 = ((z >> 11) as f64) / ((1u64 << 53) as f64);
        state = state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z2 = state;
        z2 = (z2 ^ (z2 >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z2 = (z2 ^ (z2 >> 27)).wrapping_mul(0x94D049BB133111EB);
        z2 ^= z2 >> 31;
        let u2 = ((z2 >> 11) as f64) / ((1u64 << 53) as f64);
        let r = (-2.0 * u1.max(1e-12).ln()).sqrt();
        let theta = 2.0 * std::f64::consts::PI * u2;
        out.push((r * theta.cos()) as f32);
        if out.len() < n {
            out.push((r * theta.sin()) as f32);
        }
    }
    out
}

/// Resolve the Klein-4B weight layout used by the validator and the
/// asset-ai backend (`transformer/`, `text_encoder/`, `tokenizer/`, `vae/`
/// or a single `flux-2-klein-4b.safetensors` next to those dirs).
pub fn flux2_klein_paths_from_root(root: impl AsRef<Path>) -> Result<Flux2KleinPaths> {
    let root = root.as_ref();
    // Prefer the oracle's diffusers `transformer/` over the BFL single file.
    // Names are mapped in the DiT loader; the two files are identical except
    // `norm_out.linear` which is the AdaLN [scale,shift] swap of BFL.
    let transformer = [
        root.join("transformer/diffusion_pytorch_model.safetensors"),
        root.join("transformer"),
        root.join("flux-2-klein-4b.safetensors"),
    ]
    .into_iter()
    .find(|p| p.exists())
    .ok_or_else(|| {
        DiffusionError::workflow(format!(
            "flux2-klein-4b transformer not found under {}",
            root.display()
        ))
    })?;
    let text_encoder = root.join("text_encoder");
    let tokenizer = root.join("tokenizer");
    let vae = [
        root.join("vae/diffusion_pytorch_model.safetensors"),
        root.join("vae"),
        root.join("flux2-vae.safetensors"),
        root.join("ae.safetensors"),
    ]
    .into_iter()
    .find(|p| p.exists())
    .ok_or_else(|| {
        DiffusionError::workflow(format!("flux2-klein-4b vae not found under {}", root.display()))
    })?;
    if !text_encoder.is_dir() {
        return Err(DiffusionError::workflow(format!(
            "flux2-klein-4b text_encoder dir missing: {}",
            text_encoder.display()
        )));
    }
    if !tokenizer.is_dir() {
        return Err(DiffusionError::workflow(format!(
            "flux2-klein-4b tokenizer dir missing: {}",
            tokenizer.display()
        )));
    }
    Ok(Flux2KleinPaths {
        transformer,
        text_encoder,
        tokenizer,
        vae,
    })
}

// --- FLUX.2-dev text-to-image -----------------------------------------------

pub const FLUX2_DEV_DEFAULT_STEPS: usize = 20;
pub const FLUX2_DEV_DEFAULT_SIZE: u32 = 1024;
pub const FLUX2_DEV_DEFAULT_GUIDANCE: f32 = 4.0;

pub struct Flux2DevPaths {
    /// `flux2_dev_fp8mixed.safetensors` (Comfy fp8mixed single file).
    pub transformer: PathBuf,
    /// `mistral_3_small_flux2_fp8.safetensors` (Comfy pruned fp8 TE).
    pub text_encoder: PathBuf,
    /// HF `tokenizer/` dir (tokenizer.json, Tekken).
    pub tokenizer: PathBuf,
    /// `flux2-vae.safetensors`.
    pub vae: PathBuf,
}

pub struct Flux2DevPipeline {
    pub paths: Flux2DevPaths,
    pub transformer: Flux2TransformerWeights,
    pub text_encoder: Flux2WeightFile,
    pub text_prepared: Flux2DevTextPrepared,
    pub tokenizer: Flux2Tokenizer,
    pub vae: Flux2VaeWeights,
    /// Conditioning cache: prompt -> (padded 512x15360 embeds, token ids).
    /// Mirrors the reference server's node cache — a warm generate with an
    /// unchanged prompt never re-runs the TE.
    cached_prompt: Option<(String, Vec<f32>, Vec<u32>)>,
}

#[derive(Clone, Debug)]
pub struct Flux2GenerateRequest {
    pub prompt: String,
    pub width: u32,
    pub height: u32,
    pub steps: usize,
    pub guidance: f32,
    pub seed: u64,
    /// Optional teacher noise (token-major `[gen_tokens, 128]`) from an
    /// oracle dump (the oracle's step-0 x IS the noise at sigma 1.0).
    pub noise: Option<Vec<f32>>,
    /// Optional oracle conditioning, ALREADY zero-left-padded `[512, 15360]`.
    pub teacher_embeds: Option<Vec<f32>>,
    /// Optional teacher forcing: the oracle's latent `x` at EVERY step
    /// (token-major `[gen_tokens, 128]`, `len() == steps`). When set, step
    /// `i` runs the transformer on the oracle's `x_i` instead of the
    /// natively integrated sample, and `step_predictions` carries all
    /// steps — the per-step parity metric that is not polluted by the
    /// trajectory's chaotic amplification of fp8/bf16 ulps.
    pub teacher_steps: Option<Vec<Vec<f32>>>,
    /// Optional img2img init (see [`Flux2Img2Img`]).
    pub init: Option<Flux2Img2Img>,
}

#[derive(Clone, Debug)]
pub struct Flux2GenerateResult {
    pub image: Flux2VaeImage,
    pub png: Vec<u8>,
    pub packed_latents: Flux2PackedLatents,
    pub input_ids: Vec<u32>,
    /// The DiT-side conditioning (512 rows, zero-left-padded).
    pub prompt_embeds: Vec<f32>,
    /// Predictions captured at step 0 and the final step (oracle gates).
    pub step_predictions: Vec<(usize, Vec<f32>)>,
    pub sigmas: Vec<f32>,
    pub te_ms: f64,
    pub denoise_ms: f64,
    pub decode_ms: f64,
    pub png_ms: f64,
    pub total_ms: f64,
}

/// Zero-LEFT-pad `(seq, width)` conditioning rows to `(target, width)` —
/// comfy `Flux2.extra_conds`: pad rows FIRST, real conditioning last.
pub fn flux2_dev_pad_conditioning(
    conditioning: &[f32],
    seq: usize,
    width: usize,
    target: usize,
) -> Result<Vec<f32>> {
    if conditioning.len() != seq * width {
        return Err(DiffusionError::workflow(format!(
            "flux2 dev conditioning expected {} values, got {}",
            seq * width,
            conditioning.len()
        )));
    }
    if seq > target {
        return Err(DiffusionError::workflow(format!(
            "flux2 dev conditioning {seq} rows exceeds the {target} window"
        )));
    }
    let mut out = vec![0.0f32; target * width];
    out[(target - seq) * width..].copy_from_slice(conditioning);
    Ok(out)
}

impl Flux2DevPipeline {
    pub fn load(paths: Flux2DevPaths) -> Result<Self> {
        if !gpu_device_available() {
            return Err(DiffusionError::workflow(
                "flux2-dev requires CUDA (MAKEPAD_GGML_REQUIRE_CUDA=1)",
            ));
        }
        let tokenizer = Flux2Tokenizer::load(&paths.tokenizer)?;
        let text_encoder = Flux2WeightFile::load(&paths.text_encoder)?;
        let text_prepared =
            Flux2DevTextPrepared::prepare(&text_encoder, Mistral3TextConfig::flux2_dev())?;
        let vae = Flux2VaeWeights::load(&paths.vae)?;
        let transformer = Flux2TransformerWeights::load(&paths.transformer)?;
        if !transformer.config.guidance_embed {
            return Err(DiffusionError::model(
                "flux2-dev loader received a guidance_embed=false transformer",
            ));
        }
        Ok(Self {
            paths,
            transformer,
            text_encoder,
            text_prepared,
            tokenizer,
            vae,
            cached_prompt: None,
        })
    }

    pub fn generate(&mut self, request: &Flux2GenerateRequest) -> Result<Flux2GenerateResult> {
        self.generate_with_hooks(request, None)
    }

    pub fn generate_with_hooks(
        &mut self,
        request: &Flux2GenerateRequest,
        mut on_stage: Option<&mut dyn FnMut(&str, usize, usize)>,
    ) -> Result<Flux2GenerateResult> {
        if request.width % 16 != 0 || request.height % 16 != 0 {
            return Err(DiffusionError::workflow(format!(
                "flux2 dev size must be a multiple of 16, got {}x{}",
                request.width, request.height
            )));
        }
        if request.steps == 0 {
            return Err(DiffusionError::workflow("flux2 dev needs at least 1 step"));
        }
        let total_started = std::time::Instant::now();

        let width = FLUX2_MAX_SEQUENCE_LENGTH;
        let cond_width = Mistral3TextConfig::flux2_dev().conditioning_dim() as usize;
        let (prompt_embeds, input_ids) = if let Some(embeds) = &request.teacher_embeds {
            if embeds.len() != width * cond_width {
                return Err(DiffusionError::workflow(format!(
                    "flux2 dev teacher embeds expected {} values, got {}",
                    width * cond_width,
                    embeds.len()
                )));
            }
            (embeds.clone(), Vec::new())
        } else if let Some((prompt, embeds, ids)) = self
            .cached_prompt
            .as_ref()
            .filter(|(prompt, _, _)| *prompt == request.prompt)
        {
            let _ = prompt;
            (embeds.clone(), ids.clone())
        } else {
            let ids = self
                .tokenizer
                .encode_t2i_unpadded(FLUX2_SYSTEM_MESSAGE, &request.prompt);
            let mut te_hook = on_stage
                .as_deref_mut()
                .map(|hook| move |done: usize, total: usize| hook("text-encode", done, total));
            let (conditioning, _) = flux2_dev_text_encode(
                &self.text_encoder,
                &self.text_prepared,
                &ids,
                te_hook
                    .as_mut()
                    .map(|hook| hook as &mut dyn FnMut(usize, usize)),
                false,
            )?;
            drop(te_hook);
            let padded = flux2_dev_pad_conditioning(&conditioning, ids.len(), cond_width, width)?;
            self.cached_prompt = Some((request.prompt.clone(), padded.clone(), ids.clone()));
            (padded, ids)
        };
        let te_ms = total_started.elapsed().as_secs_f64() * 1000.0;

        let packed_w = (request.width / 16) as usize;
        let packed_h = (request.height / 16) as usize;
        let gen_tokens = packed_w * packed_h;
        let channels = 128usize;
        let mut sample = match &request.noise {
            Some(noise) => {
                if noise.len() != gen_tokens * channels {
                    return Err(DiffusionError::workflow(format!(
                        "flux2 dev noise expected {} values, got {}",
                        gen_tokens * channels,
                        noise.len()
                    )));
                }
                noise.clone()
            }
            None => splitmix_noise(request.seed, gen_tokens * channels),
        };
        let gen_ids = flux2_image_ids(packed_w, packed_h, 0);
        let txt_ids = flux2_text_ids(width);
        let sigmas = flux2_schedule(request.steps, gen_tokens)?;
        let start_step = match &request.init {
            Some(_) if request.teacher_steps.is_some() => {
                return Err(DiffusionError::workflow(
                    "flux2 dev: img2img init and teacher_steps are mutually exclusive",
                ))
            }
            Some(init) => {
                let (start, mixed) =
                    flux2_img2img_start(&self.vae, init, sample, &sigmas, request.steps, packed_w, packed_h)?;
                sample = mixed;
                start
            }
            None => 0,
        };

        let prof = std::env::var_os("MAKEPAD_GPU_PROF").is_some();
        if prof {
            let _ = makepad_ai_common::backend::prof::report_and_reset("");
        }
        let denoise_started = std::time::Instant::now();
        let mut step_predictions = Vec::new();
        if let Some(teacher) = &request.teacher_steps {
            if teacher.len() != request.steps {
                return Err(DiffusionError::workflow(format!(
                    "flux2 dev teacher_steps has {} entries for {} steps",
                    teacher.len(),
                    request.steps
                )));
            }
        }
        for step in start_step..request.steps {
            let step_started = std::time::Instant::now();
            if let Some(hook) = on_stage.as_deref_mut() {
                hook("denoise", step + 1, request.steps);
            }
            if let Some(teacher) = &request.teacher_steps {
                if teacher[step].len() != sample.len() {
                    return Err(DiffusionError::workflow(format!(
                        "flux2 dev teacher_steps[{step}] has {} values, expected {}",
                        teacher[step].len(),
                        sample.len()
                    )));
                }
                sample.copy_from_slice(&teacher[step]);
            }
            let run = flux2_transformer_forward(
                &self.transformer,
                &sample,
                &gen_ids,
                &prompt_embeds,
                &txt_ids,
                sigmas[step],
                Some(request.guidance),
                gen_tokens,
            )?;
            if step == start_step || step + 1 == request.steps || request.teacher_steps.is_some() {
                step_predictions.push((step, run.prediction.clone()));
            }
            flux2_euler_step(&mut sample, &run.prediction, sigmas[step], sigmas[step + 1])?;
            if prof {
                eprintln!(
                    "flux2dev prof step{step} ms={:.1}",
                    step_started.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
        let denoise_ms = denoise_started.elapsed().as_secs_f64() * 1000.0;
        if prof {
            eprint!(
                "{}",
                makepad_ai_common::backend::prof::report_and_reset("flux2dev prof denoise ")
            );
            flux2_dev_prof_mem("after denoise");
        }
        flux2_dit_clear_pool();
        // Decode transients (~2-3GB at 1024px) plus the resident DiT sit at
        // the 32GB WDDM cliff; the ring slots are the cheapest headroom —
        // freed here, re-primed on the next forward.
        let _ = crate::backend::gpu_stream_ring_release_slots();
        if prof {
            flux2_dev_prof_mem("before decode (pool cleared, ring slots released)");
        }

        let packed = Flux2PackedLatents::from_tokens(&sample, packed_w, packed_h, channels)?;
        let decode_started = std::time::Instant::now();
        if let Some(hook) = on_stage.as_deref_mut() {
            hook("decode", 0, 1);
        }
        let image = flux2_vae_decode(&self.vae, &packed)?;
        let decode_ms = decode_started.elapsed().as_secs_f64() * 1000.0;
        if prof {
            eprintln!("flux2dev prof vae_decode ms={decode_ms:.1}");
            flux2_dev_prof_mem("after decode");
            eprint!(
                "{}",
                makepad_ai_common::backend::prof::report_and_reset("flux2dev prof vae ")
            );
        }
        let png_started = std::time::Instant::now();
        let rgb = flux2_image_to_rgb_u8(&image);
        let png = encode_png_rgb(
            &planar_rgb_to_whcb(&rgb, image.width, image.height),
            image.width,
            image.height,
        )?;
        let png_ms = png_started.elapsed().as_secs_f64() * 1000.0;
        Ok(Flux2GenerateResult {
            image,
            png,
            packed_latents: packed,
            input_ids,
            prompt_embeds,
            step_predictions,
            sigmas,
            te_ms,
            denoise_ms,
            decode_ms,
            png_ms,
            total_ms: total_started.elapsed().as_secs_f64() * 1000.0,
        })
    }
}

impl Flux2DevPipeline {
    /// Instruction edit: `request.prompt` + reference images → edited image.
    /// Same mechanism as Klein (`flux2_concat_ref_tokens`: reference latents
    /// packed on T-axis offsets 10, 20, …, the DiT predicts only the first
    /// `gen_tokens`), with dev's Mistral conditioning and guidance embed.
    /// `guidance` = the distilled guidance value (dev default 4.0).
    pub fn edit_with_hooks(
        &mut self,
        request: &Flux2EditRequest,
        guidance: f32,
        mut on_stage: Option<&mut dyn FnMut(&str, usize, usize)>,
    ) -> Result<Flux2EditResult> {
        if request.width % 16 != 0 || request.height % 16 != 0 {
            return Err(DiffusionError::workflow(format!(
                "flux2 dev edit size must be a multiple of 16, got {}x{}",
                request.width, request.height
            )));
        }
        if request.steps == 0 {
            return Err(DiffusionError::workflow("flux2 dev edit needs at least 1 step"));
        }
        if request.references.is_empty() && request.teacher_ref_tokens.is_none() {
            return Err(DiffusionError::workflow(
                "flux2 dev edit requires at least one reference image",
            ));
        }
        let total_started = std::time::Instant::now();

        // Conditioning: identical to generate (system message + instruction,
        // zero-left-padded to the 512 window), served from the prompt cache.
        let width = FLUX2_MAX_SEQUENCE_LENGTH;
        let cond_width = Mistral3TextConfig::flux2_dev().conditioning_dim() as usize;
        let (prompt_embeds, input_ids) = if let Some(embeds) = &request.teacher_embeds {
            if embeds.len() != width * cond_width {
                return Err(DiffusionError::workflow(format!(
                    "flux2 dev teacher embeds expected {} values, got {}",
                    width * cond_width,
                    embeds.len()
                )));
            }
            (embeds.clone(), Vec::new())
        } else if let Some((_, embeds, ids)) = self
            .cached_prompt
            .as_ref()
            .filter(|(prompt, _, _)| *prompt == request.prompt)
        {
            (embeds.clone(), ids.clone())
        } else {
            let ids = self
                .tokenizer
                .encode_t2i_unpadded(FLUX2_SYSTEM_MESSAGE, &request.prompt);
            let mut te_hook = on_stage
                .as_deref_mut()
                .map(|hook| move |done: usize, total: usize| hook("text-encode", done, total));
            let (conditioning, _) = flux2_dev_text_encode(
                &self.text_encoder,
                &self.text_prepared,
                &ids,
                te_hook
                    .as_mut()
                    .map(|hook| hook as &mut dyn FnMut(usize, usize)),
                false,
            )?;
            drop(te_hook);
            let padded = flux2_dev_pad_conditioning(&conditioning, ids.len(), cond_width, width)?;
            self.cached_prompt = Some((request.prompt.clone(), padded.clone(), ids.clone()));
            (padded, ids)
        };
        let te_ms = total_started.elapsed().as_secs_f64() * 1000.0;

        // Reference images → packed latents on successive T offsets.
        let encode_started = std::time::Instant::now();
        if let Some(hook) = on_stage.as_deref_mut() {
            hook("encode-refs", 0, request.references.len().max(1));
        }
        let mut ref_packed = Vec::new();
        if let Some(tokens) = &request.teacher_ref_tokens {
            let packed_w = (request.width / 16) as usize;
            let packed_h = (request.height / 16) as usize;
            let packed = Flux2PackedLatents::from_tokens(tokens, packed_w, packed_h, 128)?;
            let ids = flux2_image_ids(packed.width, packed.height, flux2_ref_time_offset(0));
            ref_packed.push((tokens.clone(), ids, packed));
        } else {
            for (index, image) in request.references.iter().enumerate() {
                let packed = flux2_vae_encode(&self.vae, image)?;
                let tokens = packed.to_tokens();
                let ids = flux2_image_ids(packed.width, packed.height, flux2_ref_time_offset(index));
                ref_packed.push((tokens, ids, packed));
                if let Some(hook) = on_stage.as_deref_mut() {
                    hook("encode-refs", index + 1, request.references.len());
                }
            }
        }
        let encode_ms = encode_started.elapsed().as_secs_f64() * 1000.0;

        let packed_w = (request.width / 16) as usize;
        let packed_h = (request.height / 16) as usize;
        let gen_tokens = packed_w * packed_h;
        let channels = 128usize;
        let mut sample = match &request.noise {
            Some(noise) => {
                if noise.len() != gen_tokens * channels {
                    return Err(DiffusionError::workflow(format!(
                        "flux2 dev edit noise expected {} values, got {}",
                        gen_tokens * channels,
                        noise.len()
                    )));
                }
                noise.clone()
            }
            None => splitmix_noise(request.seed, gen_tokens * channels),
        };
        let gen_ids = flux2_image_ids(packed_w, packed_h, 0);
        let refs: Vec<(Vec<f32>, Vec<Flux2PosId>)> = ref_packed
            .iter()
            .map(|(tokens, ids, _)| (tokens.clone(), ids.clone()))
            .collect();
        let txt_ids = flux2_text_ids(width);
        let sigmas = flux2_schedule(request.steps, gen_tokens)?;
        let start_step = match &request.init {
            Some(init) => {
                let (start, mixed) =
                    flux2_img2img_start(&self.vae, init, sample, &sigmas, request.steps, packed_w, packed_h)?;
                sample = mixed;
                start
            }
            None => 0,
        };

        let denoise_started = std::time::Instant::now();
        let mut step_residuals = Vec::new();
        for step in start_step..request.steps {
            if let Some(hook) = on_stage.as_deref_mut() {
                hook("denoise", step + 1, request.steps);
            }
            let (img_tokens, img_ids) = flux2_concat_ref_tokens(&sample, &gen_ids, &refs);
            let run = flux2_transformer_forward(
                &self.transformer,
                &img_tokens,
                &img_ids,
                &prompt_embeds,
                &txt_ids,
                sigmas[step],
                Some(guidance),
                gen_tokens,
            )?;
            if step == start_step || step + 1 == request.steps {
                step_residuals.push(run.prediction.clone());
            }
            flux2_euler_step(&mut sample, &run.prediction, sigmas[step], sigmas[step + 1])?;
        }
        let warm_ms = denoise_started.elapsed().as_secs_f64() * 1000.0;
        flux2_dit_clear_pool();
        let _ = crate::backend::gpu_stream_ring_release_slots();

        let packed = Flux2PackedLatents::from_tokens(&sample, packed_w, packed_h, channels)?;
        let decode_started = std::time::Instant::now();
        if let Some(hook) = on_stage.as_deref_mut() {
            hook("decode", 0, 1);
        }
        let image = flux2_vae_decode(&self.vae, &packed)?;
        let decode_ms = decode_started.elapsed().as_secs_f64() * 1000.0;
        let png_started = std::time::Instant::now();
        let rgb = flux2_image_to_rgb_u8(&image);
        let png = encode_png_rgb(
            &planar_rgb_to_whcb(&rgb, image.width, image.height),
            image.width,
            image.height,
        )?;
        let png_ms = png_started.elapsed().as_secs_f64() * 1000.0;
        Ok(Flux2EditResult {
            image,
            png,
            packed_latents: packed,
            ref_packed: ref_packed[0].2.clone(),
            input_ids,
            real_len: 0,
            prompt_embeds,
            step_residuals,
            sigmas,
            warm_ms,
            te_ms,
            encode_ms,
            decode_ms,
            png_ms,
            total_ms: total_started.elapsed().as_secs_f64() * 1000.0,
        })
    }
}

/// `MAKEPAD_GPU_PROF` line: live device memory + weight-cache/pool counters
/// (reset on each call) — the 32GB-card decode phase lives at the WDDM
/// residency cliff, so "where the bytes are" is the first question.
fn flux2_dev_prof_mem(label: &str) {
    let stats = crate::backend::gpu_perf_stats(true);
    eprintln!(
        "flux2dev prof mem {label}: free={:.0}MB total={:.0}MB weight_stream={} ({:.0}MB) \
         weight_evict_events={} pool_fresh_alloc={} ({:.0}MB) pool_oom_clears={} \
         pool_overcap_free={:.0}MB",
        stats.mem_free_bytes as f64 / (1024.0 * 1024.0),
        stats.mem_total_bytes as f64 / (1024.0 * 1024.0),
        stats.weight_stream_count,
        stats.weight_stream_bytes as f64 / (1024.0 * 1024.0),
        stats.weight_evict_events,
        stats.pool_fresh_alloc_count,
        stats.pool_fresh_alloc_bytes as f64 / (1024.0 * 1024.0),
        stats.pool_oom_clears,
        stats.pool_overcap_free_bytes as f64 / (1024.0 * 1024.0),
    );
}

/// Resolve the dev weight layout under one root: the Comfy fp8mixed DiT
/// (`flux2_dev_fp8mixed.safetensors`) or, for a quantized tier, the single
/// `*.gguf` DiT in the root (city96 `flux2-dev-Q4_K_M.gguf` & co.), plus
/// the TE / VAE / tokenizer. Those three are identical across every dev tier
/// (the tiers differ only in the DiT), so a tier root that lacks them falls
/// back to the sibling canonical `flux2-dev/` dir — the registry lists them
/// for each tier with the same `cache_as`, so the 18 GB TE is downloaded
/// once and shared.
pub fn flux2_dev_paths_from_root(root: impl AsRef<Path>) -> Result<Flux2DevPaths> {
    let root = root.as_ref();
    let canonical = root
        .parent()
        .map(|parent| parent.join("flux2-dev"))
        .filter(|dir| dir != root && dir.is_dir());
    let shared = |name: &str| -> PathBuf {
        let own = root.join(name);
        if own.exists() {
            return own;
        }
        match &canonical {
            Some(dir) if dir.join(name).exists() => dir.join(name),
            _ => own,
        }
    };
    let fp8 = root.join("flux2_dev_fp8mixed.safetensors");
    let transformer = if fp8.is_file() {
        fp8
    } else {
        let mut ggufs: Vec<PathBuf> = std::fs::read_dir(root)
            .map_err(|err| DiffusionError::io(root, err.to_string()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
            })
            .collect();
        ggufs.sort();
        match ggufs.len() {
            0 => {
                return Err(DiffusionError::workflow(format!(
                    "flux2-dev transformer missing: neither {} nor a *.gguf under {}",
                    fp8.display(),
                    root.display()
                )))
            }
            1 => ggufs.remove(0),
            n => {
                return Err(DiffusionError::workflow(format!(
                    "flux2-dev root {} holds {n} *.gguf files; one DiT per tier root",
                    root.display()
                )))
            }
        }
    };
    let text_encoder = shared("mistral_3_small_flux2_fp8.safetensors");
    let tokenizer = shared("tokenizer");
    let vae = ["flux2-vae.safetensors", "vae/diffusion_pytorch_model.safetensors"]
        .into_iter()
        .map(shared)
        .find(|p| p.exists())
        .ok_or_else(|| {
            DiffusionError::workflow(format!("flux2-dev vae not found under {}", root.display()))
        })?;
    if !text_encoder.is_file() {
        return Err(DiffusionError::workflow(format!(
            "flux2-dev text_encoder missing: {}",
            text_encoder.display()
        )));
    }
    if !tokenizer.is_dir() {
        return Err(DiffusionError::workflow(format!(
            "flux2-dev tokenizer dir missing: {}",
            tokenizer.display()
        )));
    }
    Ok(Flux2DevPaths {
        transformer,
        text_encoder,
        tokenizer,
        vae,
    })
}

pub fn flux2_require_cuda() -> Result<()> {
    if gpu_device_available() {
        Ok(())
    } else {
        Err(DiffusionError::workflow(
            "flux2-klein-4b is CUDA-only; refusing CPU/Metal fallback",
        ))
    }
}
