//! FLUX.2-klein-4B image-edit pipeline.
//!
//! Instruction + one or more reference images → packed latents on T-axis
//! offsets 10, 20, … → 4-step distilled Euler denoise → VAE decode.
//! Fail-closed without CUDA.

use crate::backend::gpu_device_available;
use crate::flux2::{
    flux2_concat_ref_tokens, flux2_image_ids, flux2_ref_time_offset, flux2_schedule, flux2_text_ids,
    Flux2PackedLatents, Flux2PosId, Flux2WeightFile, Qwen3TextConfig,
};
use crate::flux2_klein_text::{
    flux2_klein_text_encode, flux2_klein_text_release, flux2_klein_tokenize, flux2_klein_tokenizer_load,
    Flux2KleinTextPrepared,
};
use crate::flux2_transformer::{
    flux2_dit_clear_pool, flux2_euler_step, flux2_transformer_forward, Flux2TransformerWeights,
};
use crate::flux2_vae::{
    flux2_image_from_rgb_u8, flux2_image_to_rgb_u8, flux2_vae_decode, flux2_vae_encode, Flux2VaeImage,
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
    pub tokenizer: crate::h3_tokenizer::H3Tokenizer,
    pub vae: Flux2VaeWeights,
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

        let prof = std::env::var_os("MAKEPAD_GPU_PROF").is_some();
        if prof {
            // Discard text-encode/VAE-encode counters so the denoise report
            // covers only the loop below.
            let _ = makepad_ggml::backend::prof::report_and_reset("");
        }
        let started = std::time::Instant::now();
        let mut step_residuals = Vec::new();
        for step in 0..request.steps {
            let step_started = std::time::Instant::now();
            let (img_tokens, img_ids) = flux2_concat_ref_tokens(&sample, &gen_ids, &refs);
            let run = flux2_transformer_forward(
                &self.transformer,
                &img_tokens,
                &img_ids,
                &prompt_embeds,
                &txt_ids,
                sigmas[step],
                gen_tokens,
            )?;
            if step == request.steps / 2 || step == 0 {
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
                makepad_ggml::backend::prof::report_and_reset("flux2 prof denoise ")
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
                makepad_ggml::backend::prof::report_and_reset("flux2 prof vae ")
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

pub fn flux2_require_cuda() -> Result<()> {
    if gpu_device_available() {
        Ok(())
    } else {
        Err(DiffusionError::workflow(
            "flux2-klein-4b is CUDA-only; refusing CPU/Metal fallback",
        ))
    }
}
