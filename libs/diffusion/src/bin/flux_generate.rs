use makepad_diffusion::backend::{new_runtime, runtime_available};
use makepad_diffusion::comfy::{FluxGenerationConfig, FluxPrompts};
use makepad_diffusion::flux::{
    pack_flux_latents_nchw, unpack_flux_latents_nchw, FluxLatentShape, FluxPromptToImagePlan,
    FluxResolvedBundle,
};
use makepad_diffusion::flux_schedule::{
    euler_step, FluxSchedule, FLUX_VAE_SCALING_FACTOR, FLUX_VAE_SHIFT_FACTOR,
};
use makepad_diffusion::flux_text::{
    FluxCompiledTextEncoders, FluxConditioning, FluxLoadedTextEncoders, FluxTokenizedPrompts,
};
use makepad_diffusion::flux_transformer::{
    CompiledFluxTransformer, FluxTransformerCompileTiming, LoadedFluxTransformerWeights,
};
use makepad_diffusion::flux_vae::{CompiledFluxVae, LoadedFluxVaeWeights};
use makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_core::options::EncoderOptions;
use makepad_zune_png::PngEncoder;
use std::env;
use std::fs;
use std::path::Path;
use std::time::Instant;

fn usage() -> ! {
    eprintln!(
        "usage: flux-generate --unet <dit.gguf|safetensors> --vae <ae.safetensors> --output <out.png>\n\
         \n\
         optional:\n\
           --clip <clip_l>   --t5 <t5xxl>\n\
           --width 256       --height 256\n\
           --steps 4         --seed 1\n\
           --prompt TEXT     --guidance 1\n\
         \n\
         --cond-dir overrides T5/CLIP with precomputed bins."
    );
    std::process::exit(1);
}

struct GenerateArgs {
    unet: String,
    vae: String,
    output: String,
    clip: Option<String>,
    t5: Option<String>,
    width: u32,
    height: u32,
    steps: u32,
    seed: u64,
    prompt: String,
    guidance: f32,
    skip_denoise: bool,
    cond_dir: Option<String>,
    compare_cond_dir: Option<String>,
    recompile_transformer_each_step: bool,
    fixed_t: Option<String>,
}

fn parse_args() -> Result<GenerateArgs, Box<dyn std::error::Error>> {
    let mut unet = None;
    let mut vae = None;
    let mut output = None;
    let mut clip = None;
    let mut t5 = None;
    let mut width = 256u32;
    let mut height = 256u32;
    let mut steps = 4u32;
    let mut seed = 1u64;
    let mut prompt = "a red cube".to_string();
    let mut guidance = 1.0f32;
    let mut skip_denoise = false;
    let mut cond_dir = None;
    let mut compare_cond_dir = None;
    let mut recompile_transformer_each_step = false;
    let mut fixed_t = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let require = |flag: &str, got: Option<String>| {
            got.ok_or_else(|| format!("missing value for {flag}"))
        };
        match arg.as_str() {
            "-h" | "--help" => usage(),
            "--unet" => unet = Some(require(&arg, args.next())?),
            "--vae" => vae = Some(require(&arg, args.next())?),
            "--output" | "-o" => output = Some(require(&arg, args.next())?),
            "--clip" => clip = Some(require(&arg, args.next())?),
            "--t5" => t5 = Some(require(&arg, args.next())?),
            "--width" => width = require(&arg, args.next())?.parse()?,
            "--height" => height = require(&arg, args.next())?.parse()?,
            "--steps" => steps = require(&arg, args.next())?.parse()?,
            "--seed" => seed = require(&arg, args.next())?.parse()?,
            "--prompt" => prompt = require(&arg, args.next())?,
            "--guidance" => guidance = require(&arg, args.next())?.parse()?,
            "--skip-denoise" => skip_denoise = true,
            "--cond-dir" => cond_dir = Some(require(&arg, args.next())?),
            "--compare-cond-dir" => compare_cond_dir = Some(require(&arg, args.next())?),
            "--recompile-transformer-each-step" => recompile_transformer_each_step = true,
            "--fixed-t" => fixed_t = Some(require(&arg, args.next())?),
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    Ok(GenerateArgs {
        unet: unet.ok_or("--unet is required")?,
        vae: vae.ok_or("--vae is required")?,
        output: output.ok_or("--output is required")?,
        clip,
        t5,
        width,
        height,
        steps,
        seed,
        prompt,
        guidance,
        skip_denoise,
        cond_dir,
        compare_cond_dir,
        recompile_transformer_each_step,
        fixed_t,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let total_start = Instant::now();
    let args = parse_args()?;
    let output_path = args.output.clone();

    let plan_start = Instant::now();
    let bundle = FluxResolvedBundle::from_split(
        &args.unet,
        &args.vae,
        args.clip.as_deref(),
        args.t5.as_deref(),
    )?;
    let plan = FluxPromptToImagePlan::from_files(
        bundle,
        FluxPrompts {
            clip_l: args.prompt.clone(),
            t5xxl: args.prompt.clone(),
            negative: String::new(),
        },
        FluxGenerationConfig {
            width: args.width,
            height: args.height,
            batch_size: 1,
            seed: args.seed,
            steps: args.steps,
            cfg: 1.0,
            denoise: 1.0,
            guidance: args.guidance,
            sampler_name: "euler".to_string(),
            scheduler: "simple".to_string(),
        },
    )?;
    let plan_ms = elapsed_ms(plan_start);
    let runtime_start = Instant::now();
    let shared_runtime = if runtime_available() {
        Some(new_runtime().map_err(|err| format!("backend runtime init failed: {err}"))?)
    } else {
        None
    };
    let runtime_init_ms = if shared_runtime.is_some() {
        elapsed_ms(runtime_start)
    } else {
        0.0
    };
    let skip_denoise = args.skip_denoise;
    let mut conditioning_source = if skip_denoise { "skipped" } else { "native" }.to_string();
    let mut t5_backend = "skipped".to_string();
    let mut conditioning_override_ms = 0.0;
    let mut text_tokenize_ms = 0.0;
    let mut text_load_ms = 0.0;
    let mut text_compile_ms = 0.0;
    let mut text_execute_ms = 0.0;
    let conditioning = if skip_denoise {
        None
    } else {
        let override_start = Instant::now();
        if let Some(conditioning) = load_conditioning_override(args.cond_dir.as_deref())? {
            conditioning_override_ms = elapsed_ms(override_start);
            conditioning_source = "override".to_string();
            t5_backend = "override".to_string();
            Some(conditioning)
        } else {
            Some({
                let tokenize_start = Instant::now();
                let prompts = FluxTokenizedPrompts::from_prompts(&plan.prompts)?;
                text_tokenize_ms = elapsed_ms(tokenize_start);
                let text_load_start = Instant::now();
                let mut text_weights = FluxLoadedTextEncoders::load_from_plan(&plan)?;
                text_load_ms = elapsed_ms(text_load_start);
                let text_compile_start = Instant::now();
                let text = match shared_runtime.clone() {
                    Some(runtime) => FluxCompiledTextEncoders::compile_with_runtime(
                        runtime,
                        &mut text_weights,
                        &prompts,
                    )?,
                    None => FluxCompiledTextEncoders::compile(&mut text_weights, &prompts)?,
                };
                text_compile_ms = elapsed_ms(text_compile_start);
                t5_backend = text.t5_backend_name().to_string();
                let text_execute_start = Instant::now();
                let conditioning = text.execute(&text_weights, &prompts)?;
                text_execute_ms = elapsed_ms(text_execute_start);
                conditioning
            })
        }
    };
    if let Some(conditioning) = conditioning.as_ref() {
        if let Some(compare_dir) = args.compare_cond_dir.as_deref() {
            let reference_conditioning = load_conditioning_from_dir(Path::new(compare_dir))?;
            let (clip_max_abs, clip_mean_abs) = diff_stats(
                &conditioning.clip_pooled,
                &reference_conditioning.clip_pooled,
            )?;
            let (t5_max_abs, t5_mean_abs) = diff_stats(
                &conditioning.t5_hidden_states,
                &reference_conditioning.t5_hidden_states,
            )?;
            println!(
                "reference.clip diff: max_abs={} mean_abs={}",
                clip_max_abs, clip_mean_abs
            );
            println!(
                "reference.t5 diff: max_abs={} mean_abs={}",
                t5_max_abs, t5_mean_abs
            );
        }
    }

    let width = plan.generation.width;
    let height = plan.generation.height;
    let steps = plan.generation.steps.max(1) as usize;
    let latent_shape = FluxLatentShape::from_image_size(width, height)?;
    let schedule = FluxSchedule::for_flux1(steps, plan.transformer.guidance_embed)?;
    let recompile_transformer_each_step = args.recompile_transformer_each_step;

    let noise_start = Instant::now();
    let mut latents = gaussian_latents(
        latent_shape.latent_width,
        latent_shape.latent_height,
        plan.generation.seed,
    );
    let noise_ms = elapsed_ms(noise_start);
    let pack_start = Instant::now();
    let mut packed = pack_flux_latents_nchw(
        &latents,
        1,
        latent_shape.latent_height,
        latent_shape.latent_width,
    )?;
    let pack_ms = elapsed_ms(pack_start);
    let mut transformer_load_ms = 0.0;
    let mut transformer_compile_ms = 0.0;
    let mut transformer_graph_build_ms = 0.0;
    let mut transformer_graph_prepare_ms = 0.0;
    let mut transformer_session_create_ms = 0.0;
    let mut transformer_backend = "skipped".to_string();
    let mut denoise_ms = 0.0;
    if let Some(conditioning) = conditioning.as_ref() {
        let transformer_load_start = Instant::now();
        let mut transformer =
            LoadedFluxTransformerWeights::load(&plan.bundle.diffusion_model_path)?;
        transformer_load_ms = elapsed_ms(transformer_load_start);
        if recompile_transformer_each_step {
            let denoise_start = Instant::now();
            for step_index in 0..steps {
                let sigma = schedule.sigmas[step_index];
                let sigma_next = schedule.sigmas[step_index + 1];
                let (transformer_compiled, compile_timing) = match shared_runtime.clone() {
                    Some(runtime) => CompiledFluxTransformer::compile_with_runtime_profiled(
                        runtime,
                        &mut transformer,
                        conditioning,
                        latent_shape,
                    )?,
                    None => CompiledFluxTransformer::compile_profiled(
                        &mut transformer,
                        conditioning,
                        latent_shape,
                    )?,
                };
                transformer_backend = transformer_compiled.backend_name().to_string();
                accumulate_transformer_compile_timing(
                    &compile_timing,
                    &mut transformer_compile_ms,
                    &mut transformer_graph_build_ms,
                    &mut transformer_graph_prepare_ms,
                    &mut transformer_session_create_ms,
                );
                let run = transformer_compiled.execute(
                    &transformer,
                    conditioning,
                    &packed,
                    sigma,
                    plan.generation.guidance,
                )?;
                euler_step(&mut packed, &run.prediction, sigma, sigma_next)?;
            }
            denoise_ms = elapsed_ms(denoise_start);
        } else {
            let (transformer_compiled, compile_timing) = match shared_runtime.clone() {
                Some(runtime) => CompiledFluxTransformer::compile_with_runtime_profiled(
                    runtime,
                    &mut transformer,
                    conditioning,
                    latent_shape,
                )?,
                None => CompiledFluxTransformer::compile_profiled(
                    &mut transformer,
                    conditioning,
                    latent_shape,
                )?,
            };
            transformer_backend = transformer_compiled.backend_name().to_string();
            accumulate_transformer_compile_timing(
                &compile_timing,
                &mut transformer_compile_ms,
                &mut transformer_graph_build_ms,
                &mut transformer_graph_prepare_ms,
                &mut transformer_session_create_ms,
            );
            let denoise_start = Instant::now();
            let fixed_t = args.fixed_t.as_deref().and_then(|value| {
                if value == "1" || value.eq_ignore_ascii_case("on") {
                    Some(schedule.sigmas[0])
                } else {
                    value.parse::<f32>().ok()
                }
            });
            for step_index in 0..steps {
                let sigma = schedule.sigmas[step_index];
                let sigma_next = schedule.sigmas[step_index + 1];
                let model_t = fixed_t.unwrap_or(sigma);
                let run = transformer_compiled.execute(
                    &transformer,
                    conditioning,
                    &packed,
                    model_t,
                    plan.generation.guidance,
                )?;
                euler_step(&mut packed, &run.prediction, sigma, sigma_next)?;
            }
            denoise_ms = elapsed_ms(denoise_start);
        }
    }

    let unpack_start = Instant::now();
    latents = unpack_flux_latents_nchw(
        &packed,
        1,
        latent_shape.latent_height,
        latent_shape.latent_width,
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
        latent_shape.latent_height as usize,
        latent_shape.latent_width as usize,
    )?;
    let vae_layout_ms = elapsed_ms(vae_layout_start);

    let vae_path = plan
        .bundle
        .vae_path
        .as_ref()
        .ok_or("vae path is required")?;
    let vae_load_start = Instant::now();
    let mut vae = LoadedFluxVaeWeights::load(vae_path)?;
    let vae_load_ms = elapsed_ms(vae_load_start);
    let vae_compile_start = Instant::now();
    let vae_compiled = match shared_runtime {
        Some(runtime) => CompiledFluxVae::compile_with_runtime(runtime, &mut vae, latent_shape)?,
        None => CompiledFluxVae::compile(&mut vae, latent_shape)?,
    };
    let vae_compile_ms = elapsed_ms(vae_compile_start);
    let vae_backend = vae_compiled.backend_name().to_string();
    let vae_execute_start = Instant::now();
    let image = vae_compiled.execute(&vae, &latents)?;
    let vae_execute_ms = elapsed_ms(vae_execute_start);
    let png_encode_start = Instant::now();
    let png = encode_png_rgb(&image.image, image.width, image.height)?;
    let png_encode_ms = elapsed_ms(png_encode_start);
    let png_write_start = Instant::now();
    fs::write(Path::new(&output_path), png)?;
    let png_write_ms = elapsed_ms(png_write_start);

    println!(
        "unet: {}",
        plan.bundle.diffusion_model_path.display()
    );
    println!(
        "vae: {}",
        plan.bundle
            .vae_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "missing".to_string())
    );
    println!("output: {}", output_path);
    println!("conditioning.source: {}", conditioning_source);
    println!("t5xxl backend: {}", t5_backend);
    println!("transformer backend: {}", transformer_backend);
    println!("vae backend: {}", vae_backend);
    println!(
        "size: {}x{} steps={} seed={} guidance={}",
        width, height, steps, plan.generation.seed, plan.generation.guidance
    );
    println!("prompt: {}", plan.prompts.t5xxl);
    println!("timing.plan_ms={:.3}", plan_ms);
    println!("timing.runtime_init_ms={:.3}", runtime_init_ms);
    println!(
        "timing.conditioning_override_ms={:.3}",
        conditioning_override_ms
    );
    println!("timing.text_tokenize_ms={:.3}", text_tokenize_ms);
    println!("timing.text_load_ms={:.3}", text_load_ms);
    println!("timing.text_compile_ms={:.3}", text_compile_ms);
    println!("timing.text_execute_ms={:.3}", text_execute_ms);
    println!("timing.noise_ms={:.3}", noise_ms);
    println!("timing.pack_ms={:.3}", pack_ms);
    println!("timing.transformer_load_ms={:.3}", transformer_load_ms);
    println!(
        "timing.transformer_compile_ms={:.3}",
        transformer_compile_ms
    );
    println!(
        "timing.transformer_graph_build_ms={:.3}",
        transformer_graph_build_ms
    );
    println!(
        "timing.transformer_graph_prepare_ms={:.3}",
        transformer_graph_prepare_ms
    );
    println!(
        "timing.transformer_session_create_ms={:.3}",
        transformer_session_create_ms
    );
    println!("timing.denoise_ms={:.3}", denoise_ms);
    println!(
        "timing.denoise_step_avg_ms={:.3}",
        if steps == 0 {
            0.0
        } else {
            denoise_ms / steps as f64
        }
    );
    println!("timing.unpack_ms={:.3}", unpack_ms);
    println!("timing.latent_rescale_ms={:.3}", latent_rescale_ms);
    println!("timing.vae_layout_ms={:.3}", vae_layout_ms);
    println!("timing.vae_load_ms={:.3}", vae_load_ms);
    println!("timing.vae_compile_ms={:.3}", vae_compile_ms);
    println!("timing.vae_execute_ms={:.3}", vae_execute_ms);
    println!("timing.png_encode_ms={:.3}", png_encode_ms);
    println!("timing.png_write_ms={:.3}", png_write_ms);
    println!("timing.total_ms={:.3}", elapsed_ms(total_start));

    Ok(())
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn accumulate_transformer_compile_timing(
    timing: &FluxTransformerCompileTiming,
    total_ms: &mut f64,
    graph_build_ms: &mut f64,
    graph_prepare_ms: &mut f64,
    session_create_ms: &mut f64,
) {
    *graph_build_ms += timing.graph_build_ms;
    *graph_prepare_ms += timing.graph_prepare_ms;
    *session_create_ms += timing.session_create_ms;
    *total_ms += timing.graph_build_ms + timing.graph_prepare_ms + timing.session_create_ms;
}

fn load_conditioning_override(dir: Option<&str>) -> Result<Option<FluxConditioning>, Box<dyn std::error::Error>> {
    let Some(dir) = dir else { return Ok(None); };
    Ok(Some(load_conditioning_from_dir(Path::new(dir))?))
}

fn parse_meta_usize(text: &str, key: &str) -> Option<usize> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        if name.trim() == key {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    })
}

fn f32_bytes_to_vec(bytes: &[u8]) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if bytes.len() % std::mem::size_of::<f32>() != 0 {
        return Err(format!("expected f32 byte length, got {} bytes", bytes.len()).into());
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn load_conditioning_from_dir(
    dir_path: &Path,
) -> Result<FluxConditioning, Box<dyn std::error::Error>> {
    let clip_bytes = fs::read(dir_path.join("flux_clip_pooled.bin"))?;
    let t5_bytes = fs::read(dir_path.join("flux_t5_hidden.bin"))?;
    let meta_text = fs::read_to_string(dir_path.join("flux_t5_meta.txt")).unwrap_or_default();

    let clip_pooled = f32_bytes_to_vec(&clip_bytes)?;
    let t5_hidden_states = f32_bytes_to_vec(&t5_bytes)?;
    let hidden_size = parse_meta_usize(&meta_text, "hidden_size").unwrap_or(4096);
    let token_count = parse_meta_usize(&meta_text, "token_count")
        .unwrap_or_else(|| t5_hidden_states.len() / hidden_size.max(1));

    if hidden_size == 0 || token_count == 0 {
        return Err("conditioning metadata resolved to zero-sized tensors".into());
    }
    if t5_hidden_states.len() != hidden_size * token_count {
        return Err(format!(
            "conditioning t5 hidden length {} does not match {}x{}",
            t5_hidden_states.len(),
            hidden_size,
            token_count
        )
        .into());
    }

    Ok(FluxConditioning {
        clip_hidden_size: clip_pooled.len(),
        clip_pooled,
        t5_hidden_states,
        t5_token_count: token_count,
        t5_hidden_size: hidden_size,
        t5_attention_mask: vec![1; token_count],
        t5_eos_index: token_count.saturating_sub(1),
    })
}

fn diff_stats(lhs: &[f32], rhs: &[f32]) -> Result<(f32, f32), Box<dyn std::error::Error>> {
    if lhs.len() != rhs.len() {
        return Err(format!("diff length mismatch: {} vs {}", lhs.len(), rhs.len()).into());
    }
    let mut max_abs = 0.0f32;
    let mut sum_abs = 0.0f64;
    for (&left, &right) in lhs.iter().zip(rhs.iter()) {
        let diff = (left - right).abs();
        max_abs = max_abs.max(diff);
        sum_abs += diff as f64;
    }
    let mean_abs = if lhs.is_empty() {
        0.0
    } else {
        (sum_abs / lhs.len() as f64) as f32
    };
    Ok((max_abs, mean_abs))
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
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let expected = batch * channels * height * width;
    if input.len() != expected {
        return Err(format!(
            "nchw_to_whcb expected {} values, got {}",
            expected,
            input.len()
        )
        .into());
    }
    let _ = (batch, channels, height, width);
    // NCHW-contiguous and ggml's [W,H,C,B]-contiguous layouts are identical here.
    Ok(input.to_vec())
}

fn encode_png_rgb(
    image_whcb: &[f32],
    width: usize,
    height: usize,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let expected = width * height * 3;
    if image_whcb.len() != expected {
        return Err(format!(
            "png encode expected {} float values, got {}",
            expected,
            image_whcb.len()
        )
        .into());
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
        .map_err(|err| format!("png encode failed: {err:?}"))?;
    Ok(out)
}

fn to_u8(value: f32) -> u8 {
    let scaled = (value * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0;
    scaled.round() as u8
}

struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9e37_79b9_7f4a_7c15,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state = self.state.wrapping_mul(0x2545_f491_4f6c_dd1d);
        self.state
    }

    fn next_unit(&mut self) -> f32 {
        ((self.next_u64() >> 40) as u32) as f32 / ((1u32 << 24) as f32)
    }
}
