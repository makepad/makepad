use makepad_diffusion::comfy::FluxWorkflow;
use makepad_diffusion::flux::{
    canonicalize_flux_diffusion_tensor_name, pack_flux_latents_nchw, unpack_flux_latents_nchw,
    ComfyModelRoots, FluxLatentShape, FluxPromptToImagePlan, FluxTransformerConfig,
};
use makepad_diffusion::flux_schedule::euler_step;
use makepad_diffusion::flux_text::{
    FluxCompiledTextEncoders, FluxConditioning, FluxLoadedTextEncoders, FluxTokenizedPrompts,
};
use makepad_diffusion::flux_transformer::{
    CompiledFluxTransformer, FluxTransformerStageOutput, LoadedFluxTransformerWeights,
};
use makepad_ggml::{bf16_to_f32, f16_to_f32};
use makepad_mlx::{MlxDType, MlxSafetensorsHeader, MlxTensorEntry};
use std::{env, fs, path::Path};

#[derive(Debug)]
struct RefFluxStep {
    sigma: f32,
    input_nchw: Vec<f32>,
    output_nchw: Vec<f32>,
}

fn usage() -> ! {
    eprintln!(
        "usage: flux-transformer-smoke <workflow.json> <comfy-root-or-model-root> [width height]"
    );
    std::process::exit(1);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workflow_path = env::args().nth(1).unwrap_or_else(|| usage());
    let root = env::args().nth(2).unwrap_or_else(|| usage());
    let width = env::args()
        .nth(3)
        .map(|value| value.parse::<u32>())
        .transpose()?;
    let height = env::args()
        .nth(4)
        .map(|value| value.parse::<u32>())
        .transpose()?;

    let workflow = FluxWorkflow::from_file(&workflow_path)?;
    let roots = ComfyModelRoots::new(root);
    let plan = FluxPromptToImagePlan::from_workflow(&workflow, &roots)?;
    let reference_stage_dir = env::var("FLUX_REF_STEP_DIR").ok();
    let conditioning = if let Some(conditioning) =
        load_reference_step_conditioning(reference_stage_dir.as_deref())?
    {
        conditioning
    } else if let Some(conditioning) = load_conditioning_override()? {
        conditioning
    } else {
        let prompts = FluxTokenizedPrompts::from_prompts(&plan.prompts)?;
        let mut text_weights = FluxLoadedTextEncoders::load_from_plan(&plan)?;
        let text = FluxCompiledTextEncoders::compile(&mut text_weights, &prompts)?;
        text.execute(&text_weights, &prompts)?
    };

    let width = width.unwrap_or(256);
    let height = height.unwrap_or(width);
    let latent_shape = FluxLatentShape::from_image_size(width, height)?;
    let reference_step = load_reference_step(latent_shape)?;
    let (packed, timestep, reference_output_nchw) = if let Some(reference_step) = reference_step {
        (
            pack_flux_latents_nchw(
                &reference_step.input_nchw,
                1,
                latent_shape.latent_height,
                latent_shape.latent_width,
            )?,
            reference_step.sigma,
            Some(reference_step.output_nchw),
        )
    } else {
        let latents = seeded_latents(
            latent_shape.latent_width,
            latent_shape.latent_height,
            plan.generation.seed,
        );
        (
            pack_flux_latents_nchw(
                &latents,
                1,
                latent_shape.latent_height,
                latent_shape.latent_width,
            )?,
            1.0,
            None,
        )
    };

    let transformer_path = plan.bundle.diffusion_model_path.as_path();
    let mut transformer = LoadedFluxTransformerWeights::load(transformer_path)?;
    let compiled = CompiledFluxTransformer::compile(&mut transformer, &conditioning, latent_shape)?;
    let dump_stages = env::var_os("FLUX_DEBUG_TRANSFORMER_STAGES").is_some();
    let run = if dump_stages {
        let debug = compiled.execute_with_debug(
            &transformer,
            &conditioning,
            &packed,
            timestep,
            plan.generation.guidance,
        )?;
        compare_direct_debug_stages(
            transformer_path,
            transformer.config,
            latent_shape,
            &conditioning,
            &packed,
            timestep,
            plan.generation.guidance,
            &debug.stages,
        )?;
        print_stage_summaries(&debug.stages, reference_stage_dir.as_deref())?;
        debug.run
    } else {
        compiled.execute(
            &transformer,
            &conditioning,
            &packed,
            timestep,
            plan.generation.guidance,
        )?
    };

    println!("workflow: {}", workflow.path.display());
    println!("transformer: {}", transformer_path.display());
    println!(
        "size: {}x{} latent={}x{} packed_tokens={}",
        width,
        height,
        latent_shape.latent_width,
        latent_shape.latent_height,
        latent_shape.image_token_count
    );
    println!(
        "transformer config: hidden={} heads={} double_blocks={} single_blocks={} guidance_embed={}",
        transformer.config.hidden_size,
        transformer.config.num_heads,
        transformer.config.depth,
        transformer.config.depth_single_blocks,
        transformer.config.guidance_embed
    );
    println!("transformer backend: {}", compiled.backend_name());
    println!(
        "conditioning: clip_pooled={} t5_hidden={}x{} guidance={} timestep={}",
        conditioning.clip_pooled.len(),
        conditioning.t5_hidden_size,
        conditioning.t5_token_count,
        plan.generation.guidance,
        timestep
    );
    println!(
        "prediction: channels={} tokens={} values={}",
        run.channel_count,
        run.image_token_count,
        run.prediction.len()
    );
    let preview_len = run.prediction.len().min(8);
    let max_abs = run
        .prediction
        .iter()
        .fold(0.0f32, |acc, value| acc.max(value.abs()));
    println!(
        "prediction[0..{}]: {:?}",
        preview_len,
        &run.prediction[..preview_len]
    );
    println!("prediction max_abs: {}", max_abs);
    if env::var_os("FLUX_CHECK_REUSE").is_some() {
        let rerun = compiled.execute(
            &transformer,
            &conditioning,
            &packed,
            timestep,
            plan.generation.guidance,
        )?;
        let (max_abs_diff, mean_abs_diff) = diff_stats(&run.prediction, &rerun.prediction)?;
        println!(
            "reuse.same_input diff: max_abs={} mean_abs={}",
            max_abs_diff, mean_abs_diff
        );
    }
    if let Some(reference_stage_dir) = reference_stage_dir.as_deref() {
        compare_reference_source(
            reference_stage_dir,
            "flux_input_packed_latents",
            Some("input.packed_latents"),
            &packed,
        )?;
        compare_reference_source(
            reference_stage_dir,
            "flux_input_context",
            Some("input.encoder_source"),
            &conditioning.t5_hidden_states,
        )?;
        compare_reference_source(
            reference_stage_dir,
            "flux_input_pooled",
            Some("input.pooled_source"),
            &conditioning.clip_pooled,
        )?;
        compare_reference_source(
            reference_stage_dir,
            "flux_input_timestep",
            Some("input.timestep_source"),
            &[timestep],
        )?;
        compare_reference_source(
            reference_stage_dir,
            "flux_input_guidance",
            Some("input.guidance_source"),
            &[plan.generation.guidance],
        )?;
        if let Some(reference_packed_output) =
            load_reference_stage(reference_stage_dir, "final.output")?
        {
            let (max_abs_diff, mean_abs_diff) =
                diff_stats(&run.prediction, &reference_packed_output)?;
            let preview_len = run
                .prediction
                .len()
                .min(reference_packed_output.len())
                .min(8);
            println!(
                "reference.final.output[0..{}]: {:?}",
                preview_len,
                &reference_packed_output[..preview_len]
            );
            println!(
                "prediction_packed[0..{}]: {:?}",
                preview_len,
                &run.prediction[..preview_len]
            );
            println!(
                "reference.final.output diff: max_abs={} mean_abs={}",
                max_abs_diff, mean_abs_diff
            );
        }
    }
    if let Some(reference_output_nchw) = reference_output_nchw.as_ref() {
        let prediction_nchw = unpack_flux_latents_nchw(
            &run.prediction,
            1,
            latent_shape.latent_height,
            latent_shape.latent_width,
        )?;
        let (max_abs_diff, mean_abs_diff) = diff_stats(&prediction_nchw, reference_output_nchw)?;
        let ref_preview_len = reference_output_nchw.len().min(8);
        println!(
            "reference.output[0..{}]: {:?}",
            ref_preview_len,
            &reference_output_nchw[..ref_preview_len]
        );
        println!(
            "prediction_nchw[0..{}]: {:?}",
            ref_preview_len,
            &prediction_nchw[..ref_preview_len]
        );
        println!(
            "reference diff: max_abs={} mean_abs={}",
            max_abs_diff, mean_abs_diff
        );
    }
    if let Some(reference_stage_dir) = reference_stage_dir.as_deref() {
        if let Some(reference_post_step_nchw) = load_reference_post_step(latent_shape)? {
            let mut stepped_packed = packed.clone();
            euler_step(&mut stepped_packed, &run.prediction, timestep, 0.0)?;
            let stepped_nchw = unpack_flux_latents_nchw(
                &stepped_packed,
                1,
                latent_shape.latent_height,
                latent_shape.latent_width,
            )?;
            let (max_abs_diff, mean_abs_diff) =
                diff_stats(&stepped_nchw, &reference_post_step_nchw)?;
            let preview_len = stepped_nchw
                .len()
                .min(reference_post_step_nchw.len())
                .min(8);
            println!(
                "reference.post_step[0..{}]: {:?}",
                preview_len,
                &reference_post_step_nchw[..preview_len]
            );
            println!(
                "prediction.post_step[0..{}]: {:?}",
                preview_len,
                &stepped_nchw[..preview_len]
            );
            println!(
                "reference.post_step diff: max_abs={} mean_abs={}",
                max_abs_diff, mean_abs_diff
            );
        } else {
            let _ = reference_stage_dir;
        }
    }

    Ok(())
}

fn print_stage_summaries(
    stages: &[FluxTransformerStageOutput],
    reference_stage_dir: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    for stage in stages {
        let (min, max, mean_abs) = summarize(&stage.values);
        let preview_len = stage.values.len().min(4);
        println!(
            "stage {} shape={:?} min={:.6} max={:.6} mean_abs={:.6} first={:?}",
            stage.name,
            stage.extents,
            min,
            max,
            mean_abs,
            &stage.values[..preview_len]
        );
        if let Some(reference_stage_dir) = reference_stage_dir {
            if let Some(reference_values) = load_reference_stage(reference_stage_dir, &stage.name)?
            {
                let (max_abs_diff, mean_abs_diff) = diff_stats(&stage.values, &reference_values)?;
                let ref_preview_len = reference_values.len().min(4);
                println!(
                    "stage {} reference first={:?} diff.max_abs={:.6} diff.mean_abs={:.6}",
                    stage.name,
                    &reference_values[..ref_preview_len],
                    max_abs_diff,
                    mean_abs_diff
                );
            }
        }
    }
    Ok(())
}

fn summarize(values: &[f32]) -> (f32, f32, f32) {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut sum_abs = 0.0f32;
    for &value in values {
        min = min.min(value);
        max = max.max(value);
        sum_abs += value.abs();
    }
    let mean_abs = if values.is_empty() {
        0.0
    } else {
        sum_abs / values.len() as f32
    };
    (min, max, mean_abs)
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

fn load_conditioning_override() -> Result<Option<FluxConditioning>, Box<dyn std::error::Error>> {
    let dir = match env::var("FLUX_COND_DIR") {
        Ok(dir) => dir,
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(err) => return Err(Box::new(err)),
    };
    let dir_path = Path::new(&dir);
    let clip_bytes = fs::read(dir_path.join("flux_clip_pooled.bin"))?;
    let t5_bytes = fs::read(dir_path.join("flux_t5_hidden.bin"))?;
    let meta_text = fs::read_to_string(dir_path.join("flux_t5_meta.txt")).unwrap_or_default();

    let clip_pooled = f32_bytes_to_vec(&clip_bytes)?;
    let t5_hidden_states = f32_bytes_to_vec(&t5_bytes)?;
    let hidden_size = parse_meta_usize(&meta_text, "hidden_size").unwrap_or(4096);
    let token_count = parse_meta_usize(&meta_text, "token_count")
        .unwrap_or_else(|| t5_hidden_states.len() / hidden_size.max(1));

    if hidden_size == 0 || token_count == 0 {
        return Err("conditioning override metadata resolved to zero-sized tensors".into());
    }
    if t5_hidden_states.len() != hidden_size * token_count {
        return Err(format!(
            "conditioning override t5 hidden length {} does not match {}x{}",
            t5_hidden_states.len(),
            hidden_size,
            token_count
        )
        .into());
    }

    Ok(Some(FluxConditioning {
        clip_hidden_size: clip_pooled.len(),
        clip_pooled,
        t5_hidden_states,
        t5_token_count: token_count,
        t5_hidden_size: hidden_size,
        t5_attention_mask: vec![1; token_count],
        t5_eos_index: token_count.saturating_sub(1),
    }))
}

fn load_reference_step_conditioning(
    dir: Option<&str>,
) -> Result<Option<FluxConditioning>, Box<dyn std::error::Error>> {
    let Some(dir) = dir else {
        return Ok(None);
    };
    let Some(t5_hidden_states) = load_exact_reference_source(dir, "flux_input_context")? else {
        return Ok(None);
    };
    let clip_pooled = load_exact_reference_source(dir, "flux_input_pooled")?
        .ok_or("reference step conditioning is missing flux_input_pooled.bin")?;
    let meta_text =
        fs::read_to_string(Path::new(dir).join("flux_step_meta.txt")).unwrap_or_default();
    let hidden_size = parse_meta_usize(&meta_text, "context_hidden_size").unwrap_or(4096);
    let token_count = parse_meta_usize(&meta_text, "context_token_count")
        .unwrap_or_else(|| t5_hidden_states.len() / hidden_size.max(1));

    if hidden_size == 0 || token_count == 0 {
        return Err("reference step conditioning metadata resolved to zero-sized tensors".into());
    }
    if t5_hidden_states.len() != hidden_size * token_count {
        return Err(format!(
            "reference step t5 hidden length {} does not match {}x{}",
            t5_hidden_states.len(),
            hidden_size,
            token_count
        )
        .into());
    }

    Ok(Some(FluxConditioning {
        clip_hidden_size: clip_pooled.len(),
        clip_pooled,
        t5_hidden_states,
        t5_token_count: token_count,
        t5_hidden_size: hidden_size,
        t5_attention_mask: vec![1; token_count],
        t5_eos_index: token_count.saturating_sub(1),
    }))
}

fn load_reference_step(
    latent_shape: FluxLatentShape,
) -> Result<Option<RefFluxStep>, Box<dyn std::error::Error>> {
    let dir = match env::var("FLUX_REF_STEP_DIR") {
        Ok(dir) => dir,
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(err) => return Err(Box::new(err)),
    };
    let dir_path = Path::new(&dir);
    let meta_text = fs::read_to_string(dir_path.join("flux_step_meta.txt"))?;
    let input_width =
        parse_meta_usize(&meta_text, "input_width").unwrap_or(latent_shape.latent_width as usize);
    let input_height =
        parse_meta_usize(&meta_text, "input_height").unwrap_or(latent_shape.latent_height as usize);
    let input_channels = parse_meta_usize(&meta_text, "input_channels").unwrap_or(16);
    let input_batch = parse_meta_usize(&meta_text, "input_batch").unwrap_or(1);
    let output_width =
        parse_meta_usize(&meta_text, "output_width").unwrap_or(latent_shape.latent_width as usize);
    let output_height = parse_meta_usize(&meta_text, "output_height")
        .unwrap_or(latent_shape.latent_height as usize);
    let output_channels = parse_meta_usize(&meta_text, "output_channels").unwrap_or(16);
    let output_batch = parse_meta_usize(&meta_text, "output_batch").unwrap_or(1);
    let sigma = parse_meta_f32(&meta_text, "sigma").unwrap_or(1.0);

    let expected_width = latent_shape.latent_width as usize;
    let expected_height = latent_shape.latent_height as usize;
    if input_width != expected_width || input_height != expected_height {
        return Err(format!(
            "reference input shape {}x{} does not match latent shape {}x{}",
            input_width, input_height, expected_width, expected_height
        )
        .into());
    }
    if output_width != expected_width || output_height != expected_height {
        return Err(format!(
            "reference output shape {}x{} does not match latent shape {}x{}",
            output_width, output_height, expected_width, expected_height
        )
        .into());
    }
    if input_channels != 16 || output_channels != 16 || input_batch != 1 || output_batch != 1 {
        return Err(format!(
            "reference step expected 1x16 latent tensors, got input batch={} channels={} output batch={} channels={}",
            input_batch, input_channels, output_batch, output_channels
        )
        .into());
    }

    let input_nchw = f32_bytes_to_vec(&fs::read(dir_path.join("flux_noised_input.bin"))?)?;
    let output_nchw = f32_bytes_to_vec(&fs::read(dir_path.join("flux_cond_out.bin"))?)?;
    let expected_values = expected_width
        .checked_mul(expected_height)
        .and_then(|value| value.checked_mul(16))
        .ok_or("reference step shape overflow")?;
    if input_nchw.len() != expected_values || output_nchw.len() != expected_values {
        return Err(format!(
            "reference step tensors expected {} values, got input {} output {}",
            expected_values,
            input_nchw.len(),
            output_nchw.len()
        )
        .into());
    }

    Ok(Some(RefFluxStep {
        sigma,
        input_nchw,
        output_nchw,
    }))
}

fn load_reference_post_step(
    latent_shape: FluxLatentShape,
) -> Result<Option<Vec<f32>>, Box<dyn std::error::Error>> {
    let dir = match env::var("FLUX_REF_STEP_DIR") {
        Ok(dir) => dir,
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(err) => return Err(Box::new(err)),
    };
    let path = Path::new(&dir).join("flux_post_step.bin");
    if !path.is_file() {
        return Ok(None);
    }
    let values = f32_bytes_to_vec(&fs::read(path)?)?;
    let expected = 16usize
        .checked_mul(latent_shape.latent_width as usize)
        .and_then(|value| value.checked_mul(latent_shape.latent_height as usize))
        .ok_or("reference post-step shape overflow")?;
    if values.len() != expected {
        return Err(format!(
            "reference post-step expected {} values, got {}",
            expected,
            values.len()
        )
        .into());
    }
    Ok(Some(values))
}

fn load_reference_stage(
    dir: &str,
    stage_name: &str,
) -> Result<Option<Vec<f32>>, Box<dyn std::error::Error>> {
    if let Some(exact_name) = exact_reference_stage_name(stage_name) {
        if let Some(reference_values) = load_exact_reference_source(dir, exact_name)? {
            return Ok(Some(reference_values));
        }
    }
    let path = Path::new(dir).join(format!("{stage_name}.bin"));
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(f32_bytes_to_vec(&fs::read(path)?)?))
}

fn exact_reference_stage_name(stage_name: &str) -> Option<&'static str> {
    match stage_name {
        "input.hidden" => Some("flux_input_hidden"),
        "input.encoder_hidden" => Some("flux_input_encoder_hidden"),
        "input.temb" => Some("flux_input_temb"),
        _ => None,
    }
}

fn compare_reference_source(
    dir: &str,
    exact_name: &str,
    fallback_stage_name: Option<&str>,
    actual: &[f32],
) -> Result<(), Box<dyn std::error::Error>> {
    let reference_values =
        if let Some(reference_values) = load_exact_reference_source(dir, exact_name)? {
            reference_values
        } else if let Some(stage_name) = fallback_stage_name {
            let Some(reference_values) = load_reference_stage(dir, stage_name)? else {
                return Ok(());
            };
            reference_values
        } else {
            return Ok(());
        };
    let (max_abs_diff, mean_abs_diff) = diff_stats(actual, &reference_values)?;
    let preview_len = actual.len().min(reference_values.len()).min(4);
    println!(
        "source {} actual={:?} reference={:?} diff.max_abs={:.6} diff.mean_abs={:.6}",
        exact_name,
        &actual[..preview_len],
        &reference_values[..preview_len],
        max_abs_diff,
        mean_abs_diff
    );
    Ok(())
}

fn load_exact_reference_source(
    dir: &str,
    base_name: &str,
) -> Result<Option<Vec<f32>>, Box<dyn std::error::Error>> {
    let path = Path::new(dir).join(format!("{base_name}.bin"));
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(f32_bytes_to_vec(&fs::read(path)?)?))
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

fn parse_meta_f32(text: &str, key: &str) -> Option<f32> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        if name.trim() == key {
            value.trim().parse::<f32>().ok()
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
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect())
}

fn seeded_latents(latent_width: u32, latent_height: u32, seed: u64) -> Vec<f32> {
    let count = 16usize * latent_width as usize * latent_height as usize;
    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let value = state.wrapping_mul(0x2545_f491_4f6c_dd1d);
        let unit = ((value >> 40) as u32) as f32 / ((1u32 << 24) as f32);
        out.push(unit * 2.0 - 1.0);
    }
    out
}

fn compare_direct_debug_stages(
    transformer_path: &Path,
    config: FluxTransformerConfig,
    latent_shape: FluxLatentShape,
    conditioning: &FluxConditioning,
    packed_latents: &[f32],
    timestep: f32,
    guidance: f32,
    stages: &[FluxTransformerStageOutput],
) -> Result<(), Box<dyn std::error::Error>> {
    let header = MlxSafetensorsHeader::load(transformer_path)?;
    let input_hidden =
        cpu_linear_projection(&header, "img_in.weight", "img_in.bias", packed_latents)?;
    compare_direct_stage(
        "direct.input.hidden",
        input_hidden.clone(),
        find_stage(stages, "input.hidden")?,
    )?;
    let input_encoder_hidden = cpu_linear_projection(
        &header,
        "txt_in.weight",
        "txt_in.bias",
        &conditioning.t5_hidden_states,
    )?;
    compare_direct_stage(
        "direct.input.encoder_hidden",
        input_encoder_hidden.clone(),
        find_stage(stages, "input.encoder_hidden")?,
    )?;
    let mut temb = cpu_timestep_projection(&header, "time_in", timestep)?;
    let pooled = cpu_silu_mlp(&header, "vector_in", &conditioning.clip_pooled)?;
    add_inplace(&mut temb, &pooled)?;
    if config.guidance_embed {
        let guidance = cpu_timestep_projection(&header, "guidance_in", guidance)?;
        add_inplace(&mut temb, &guidance)?;
    }
    compare_direct_stage(
        "direct.input.temb",
        temb.clone(),
        find_stage(stages, "input.temb")?,
    )?;
    compare_direct_double_block0(
        &header,
        config,
        latent_shape,
        &input_hidden,
        &input_encoder_hidden,
        &temb,
        stages,
    )?;
    compare_direct_single_block0(&header, config, latent_shape, &temb, stages)?;
    Ok(())
}

fn compare_direct_double_block0(
    header: &MlxSafetensorsHeader,
    config: FluxTransformerConfig,
    latent_shape: FluxLatentShape,
    hidden: &[f32],
    encoder_hidden: &[f32],
    temb: &[f32],
    stages: &[FluxTransformerStageOutput],
) -> Result<(), Box<dyn std::error::Error>> {
    let hidden_size = config.hidden_size as usize;
    let head_count = config.num_heads as usize;
    let head_dim = config.head_dim() as usize;

    let img_mod = cpu_linear_projection(
        header,
        "double_blocks.0.img_mod.lin.weight",
        "double_blocks.0.img_mod.lin.bias",
        &temb.iter().copied().map(silu_scalar).collect::<Vec<_>>(),
    )?;
    let txt_mod = cpu_linear_projection(
        header,
        "double_blocks.0.txt_mod.lin.weight",
        "double_blocks.0.txt_mod.lin.bias",
        &temb.iter().copied().map(silu_scalar).collect::<Vec<_>>(),
    )?;
    let img_shift_msa = cpu_chunk(&img_mod, hidden_size, 6, 0)?;
    let img_scale_msa = cpu_chunk(&img_mod, hidden_size, 6, 1)?;
    let img_gate_msa = cpu_chunk(&img_mod, hidden_size, 6, 2)?;
    let img_shift_mlp = cpu_chunk(&img_mod, hidden_size, 6, 3)?;
    let img_scale_mlp = cpu_chunk(&img_mod, hidden_size, 6, 4)?;
    let txt_shift_msa = cpu_chunk(&txt_mod, hidden_size, 6, 0)?;
    let txt_scale_msa = cpu_chunk(&txt_mod, hidden_size, 6, 1)?;
    let txt_gate_msa = cpu_chunk(&txt_mod, hidden_size, 6, 2)?;
    let txt_shift_mlp = cpu_chunk(&txt_mod, hidden_size, 6, 3)?;
    let txt_scale_mlp = cpu_chunk(&txt_mod, hidden_size, 6, 4)?;

    let norm_hidden =
        cpu_modulated_layer_norm(hidden, hidden_size, &img_scale_msa, &img_shift_msa)?;
    let norm_encoder_hidden =
        cpu_modulated_layer_norm(encoder_hidden, hidden_size, &txt_scale_msa, &txt_shift_msa)?;
    compare_direct_stage(
        "direct.double_blocks.0.norm_hidden",
        norm_hidden.clone(),
        find_stage(stages, "double_blocks.0.norm_hidden")?,
    )?;
    compare_direct_stage(
        "direct.double_blocks.0.norm_encoder_hidden",
        norm_encoder_hidden.clone(),
        find_stage(stages, "double_blocks.0.norm_encoder_hidden")?,
    )?;

    let img_qkv = cpu_linear_projection(
        header,
        "double_blocks.0.img_attn.qkv.weight",
        "double_blocks.0.img_attn.qkv.bias",
        &norm_hidden,
    )?;
    let txt_qkv = cpu_linear_projection(
        header,
        "double_blocks.0.txt_attn.qkv.weight",
        "double_blocks.0.txt_attn.qkv.bias",
        &norm_encoder_hidden,
    )?;
    let img_q = cpu_chunk(&img_qkv, hidden_size, 3, 0)?;
    let img_k = cpu_chunk(&img_qkv, hidden_size, 3, 1)?;
    let img_v = cpu_chunk(&img_qkv, hidden_size, 3, 2)?;
    let txt_q = cpu_chunk(&txt_qkv, hidden_size, 3, 0)?;
    let txt_k = cpu_chunk(&txt_qkv, hidden_size, 3, 1)?;
    let txt_v = cpu_chunk(&txt_qkv, hidden_size, 3, 2)?;

    let img_q_norm = cpu_head_rms_norm(
        &img_q,
        head_dim,
        head_count,
        &load_rank1_tensor(header, "double_blocks.0.img_attn.norm.query_norm.scale")?,
    )?;
    let img_k_norm = cpu_head_rms_norm(
        &img_k,
        head_dim,
        head_count,
        &load_rank1_tensor(header, "double_blocks.0.img_attn.norm.key_norm.scale")?,
    )?;
    let txt_q_norm = cpu_head_rms_norm(
        &txt_q,
        head_dim,
        head_count,
        &load_rank1_tensor(header, "double_blocks.0.txt_attn.norm.query_norm.scale")?,
    )?;
    let txt_k_norm = cpu_head_rms_norm(
        &txt_k,
        head_dim,
        head_count,
        &load_rank1_tensor(header, "double_blocks.0.txt_attn.norm.key_norm.scale")?,
    )?;

    compare_direct_stage(
        "direct.double_blocks.0.img_q_norm",
        img_q_norm.clone(),
        find_stage(stages, "double_blocks.0.img_q_norm")?,
    )?;
    compare_direct_stage(
        "direct.double_blocks.0.img_k_norm",
        img_k_norm.clone(),
        find_stage(stages, "double_blocks.0.img_k_norm")?,
    )?;
    compare_direct_stage(
        "direct.double_blocks.0.img_v",
        img_v.clone(),
        find_stage(stages, "double_blocks.0.img_v")?,
    )?;
    compare_direct_stage(
        "direct.double_blocks.0.txt_q_norm",
        txt_q_norm.clone(),
        find_stage(stages, "double_blocks.0.txt_q_norm")?,
    )?;
    compare_direct_stage(
        "direct.double_blocks.0.txt_k_norm",
        txt_k_norm.clone(),
        find_stage(stages, "double_blocks.0.txt_k_norm")?,
    )?;
    compare_direct_stage(
        "direct.double_blocks.0.txt_v",
        txt_v.clone(),
        find_stage(stages, "double_blocks.0.txt_v")?,
    )?;

    let q = concat_token_rows(&txt_q_norm, &img_q_norm, hidden_size)?;
    let k = concat_token_rows(&txt_k_norm, &img_k_norm, hidden_size)?;
    let v = concat_token_rows(&txt_v, &img_v, hidden_size)?;
    let positions = flux_position_ids(
        encoder_hidden.len() / hidden_size,
        latent_shape.packed_width as usize,
        latent_shape.packed_height as usize,
    );
    let mut q_rope = q.clone();
    let mut k_rope = k.clone();
    cpu_apply_flux_mrope(
        &mut q_rope,
        &positions,
        head_dim,
        head_count,
        config.axes_dim,
        config.theta as f32,
    )?;
    cpu_apply_flux_mrope(
        &mut k_rope,
        &positions,
        head_dim,
        head_count,
        config.axes_dim,
        config.theta as f32,
    )?;
    compare_direct_stage(
        "direct.double_blocks.0.q_rope",
        q_rope.clone(),
        find_stage(stages, "double_blocks.0.q_rope")?,
    )?;
    compare_direct_stage(
        "direct.double_blocks.0.k_rope",
        k_rope.clone(),
        find_stage(stages, "double_blocks.0.k_rope")?,
    )?;
    let attn = cpu_attention(&q_rope, &k_rope, &v, head_dim, head_count)?;
    let text_token_count = encoder_hidden.len() / hidden_size;
    compare_direct_stage(
        "direct.double_blocks.0.hidden_attn_input",
        attn[text_token_count * hidden_size..].to_vec(),
        find_stage(stages, "double_blocks.0.hidden_attn_input")?,
    )?;
    compare_direct_stage(
        "direct.double_blocks.0.encoder_attn_input",
        attn[..text_token_count * hidden_size].to_vec(),
        find_stage(stages, "double_blocks.0.encoder_attn_input")?,
    )?;

    let hidden_attn_input = find_stage(stages, "double_blocks.0.hidden_attn_input")?;
    let encoder_attn_input = find_stage(stages, "double_blocks.0.encoder_attn_input")?;
    let hidden_attn_proj = cpu_linear_projection(
        header,
        "double_blocks.0.img_attn.proj.weight",
        "double_blocks.0.img_attn.proj.bias",
        hidden_attn_input,
    )?;
    let encoder_attn_proj = cpu_linear_projection(
        header,
        "double_blocks.0.txt_attn.proj.weight",
        "double_blocks.0.txt_attn.proj.bias",
        encoder_attn_input,
    )?;
    let hidden_post_attn = cpu_gated_residual(hidden, &hidden_attn_proj, &img_gate_msa)?;
    let encoder_hidden_post_attn =
        cpu_gated_residual(encoder_hidden, &encoder_attn_proj, &txt_gate_msa)?;
    compare_direct_stage(
        "direct.double_blocks.0.hidden_post_attn",
        hidden_post_attn.clone(),
        find_stage(stages, "double_blocks.0.hidden_post_attn")?,
    )?;
    compare_direct_stage(
        "direct.double_blocks.0.encoder_hidden_post_attn",
        encoder_hidden_post_attn.clone(),
        find_stage(stages, "double_blocks.0.encoder_hidden_post_attn")?,
    )?;

    compare_direct_stage(
        "direct.double_blocks.0.hidden_ff_input",
        cpu_modulated_layer_norm(
            &hidden_post_attn,
            hidden_size,
            &img_scale_mlp,
            &img_shift_mlp,
        )?,
        find_stage(stages, "double_blocks.0.hidden_ff_input")?,
    )?;
    compare_direct_stage(
        "direct.double_blocks.0.encoder_ff_input",
        cpu_modulated_layer_norm(
            &encoder_hidden_post_attn,
            hidden_size,
            &txt_scale_mlp,
            &txt_shift_mlp,
        )?,
        find_stage(stages, "double_blocks.0.encoder_ff_input")?,
    )?;
    Ok(())
}

fn compare_direct_single_block0(
    header: &MlxSafetensorsHeader,
    config: FluxTransformerConfig,
    latent_shape: FluxLatentShape,
    temb: &[f32],
    stages: &[FluxTransformerStageOutput],
) -> Result<(), Box<dyn std::error::Error>> {
    let hidden = find_stage(stages, "double_blocks.18.hidden")?;
    let encoder_hidden = find_stage(stages, "double_blocks.18.encoder_hidden")?;
    let hidden_size = config.hidden_size as usize;
    let head_count = config.num_heads as usize;
    let head_dim = config.head_dim() as usize;
    let text_token_count = encoder_hidden.len() / hidden_size;

    let joint_input = concat_token_rows(encoder_hidden, hidden, hidden_size)?;
    let mod_lin = cpu_linear_projection(
        header,
        "single_blocks.0.modulation.lin.weight",
        "single_blocks.0.modulation.lin.bias",
        &temb.iter().copied().map(silu_scalar).collect::<Vec<_>>(),
    )?;
    let shift = cpu_chunk(&mod_lin, hidden_size, 3, 0)?;
    let scale = cpu_chunk(&mod_lin, hidden_size, 3, 1)?;
    let gate = cpu_chunk(&mod_lin, hidden_size, 3, 2)?;

    let norm_joint = cpu_modulated_layer_norm(&joint_input, hidden_size, &scale, &shift)?;
    compare_direct_stage(
        "direct.single_blocks.0.norm_joint",
        norm_joint.clone(),
        find_stage(stages, "single_blocks.0.norm_joint")?,
    )?;

    let linear1 = cpu_linear_projection(
        header,
        "single_blocks.0.linear1.weight",
        "single_blocks.0.linear1.bias",
        &norm_joint,
    )?;
    let q = cpu_row_slice(&linear1, hidden_size * 7, 0, hidden_size)?;
    let k = cpu_row_slice(&linear1, hidden_size * 7, hidden_size, hidden_size)?;
    let v = cpu_row_slice(&linear1, hidden_size * 7, hidden_size * 2, hidden_size)?;
    let mlp = cpu_row_slice(&linear1, hidden_size * 7, hidden_size * 3, hidden_size * 4)?;

    let q_norm = cpu_head_rms_norm(
        &q,
        head_dim,
        head_count,
        &load_rank1_tensor(header, "single_blocks.0.norm.query_norm.scale")?,
    )?;
    let k_norm = cpu_head_rms_norm(
        &k,
        head_dim,
        head_count,
        &load_rank1_tensor(header, "single_blocks.0.norm.key_norm.scale")?,
    )?;
    compare_direct_stage(
        "direct.single_blocks.0.q_norm",
        q_norm.clone(),
        find_stage(stages, "single_blocks.0.q_norm")?,
    )?;
    compare_direct_stage(
        "direct.single_blocks.0.k_norm",
        k_norm.clone(),
        find_stage(stages, "single_blocks.0.k_norm")?,
    )?;
    compare_direct_stage(
        "direct.single_blocks.0.v",
        v.clone(),
        find_stage(stages, "single_blocks.0.v")?,
    )?;

    let positions = flux_position_ids(
        text_token_count,
        latent_shape.packed_width as usize,
        latent_shape.packed_height as usize,
    );
    let mut q_rope = q_norm.clone();
    let mut k_rope = k_norm.clone();
    cpu_apply_flux_mrope(
        &mut q_rope,
        &positions,
        head_dim,
        head_count,
        config.axes_dim,
        config.theta as f32,
    )?;
    cpu_apply_flux_mrope(
        &mut k_rope,
        &positions,
        head_dim,
        head_count,
        config.axes_dim,
        config.theta as f32,
    )?;
    let attn = cpu_attention(&q_rope, &k_rope, &v, head_dim, head_count)?;
    compare_direct_stage(
        "direct.single_blocks.0.attn",
        attn.clone(),
        find_stage(stages, "single_blocks.0.attn")?,
    )?;

    let mlp = mlp.into_iter().map(gelu_scalar).collect::<Vec<_>>();
    compare_direct_stage(
        "direct.single_blocks.0.mlp",
        mlp.clone(),
        find_stage(stages, "single_blocks.0.mlp")?,
    )?;

    let fused = concat_feature_rows(&attn, hidden_size, &mlp, hidden_size * 4)?;
    let proj = cpu_linear_projection(
        header,
        "single_blocks.0.linear2.weight",
        "single_blocks.0.linear2.bias",
        &fused,
    )?;
    compare_direct_stage(
        "direct.single_blocks.0.proj",
        proj.clone(),
        find_stage(stages, "single_blocks.0.proj")?,
    )?;

    let joint = cpu_gated_residual(&joint_input, &proj, &gate)?;
    compare_direct_stage(
        "direct.single_blocks.0.joint",
        joint,
        find_stage(stages, "single_blocks.0.joint")?,
    )?;
    Ok(())
}

fn find_stage<'a>(
    stages: &'a [FluxTransformerStageOutput],
    name: &str,
) -> Result<&'a [f32], Box<dyn std::error::Error>> {
    stages
        .iter()
        .find(|stage| stage.name == name)
        .map(|stage| stage.values.as_slice())
        .ok_or_else(|| format!("missing debug stage '{name}'").into())
}

fn compare_direct_stage(
    label: &str,
    expected: Vec<f32>,
    actual: &[f32],
) -> Result<(), Box<dyn std::error::Error>> {
    let (max_abs_diff, mean_abs_diff) = diff_stats(actual, &expected)?;
    let preview_len = expected.len().min(actual.len()).min(4);
    println!(
        "{} actual={:?} expected={:?} diff.max_abs={:.6} diff.mean_abs={:.6}",
        label,
        &actual[..preview_len],
        &expected[..preview_len],
        max_abs_diff,
        mean_abs_diff
    );
    Ok(())
}

fn cpu_timestep_projection(
    header: &MlxSafetensorsHeader,
    prefix: &str,
    timestep: f32,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let embed = cpu_timestep_embedding(timestep * 1000.0, 256, 10_000);
    cpu_silu_mlp(header, prefix, &embed)
}

fn cpu_silu_mlp(
    header: &MlxSafetensorsHeader,
    prefix: &str,
    input: &[f32],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let hidden = cpu_linear_projection(
        header,
        &format!("{prefix}.in_layer.weight"),
        &format!("{prefix}.in_layer.bias"),
        input,
    )?;
    let hidden = hidden.into_iter().map(silu_scalar).collect::<Vec<_>>();
    cpu_linear_projection(
        header,
        &format!("{prefix}.out_layer.weight"),
        &format!("{prefix}.out_layer.bias"),
        &hidden,
    )
}

fn cpu_linear_projection(
    header: &MlxSafetensorsHeader,
    weight_name: &str,
    bias_name: &str,
    input: &[f32],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let weight = load_rank2_tensor(header, weight_name)?;
    let bias = load_rank1_tensor(header, bias_name)?;
    if input.len() % weight.cols != 0 {
        return Err(format!(
            "linear '{}' expected input width {}, got {} values",
            weight_name,
            weight.cols,
            input.len()
        )
        .into());
    }
    if bias.len() != weight.rows {
        return Err(format!(
            "linear '{}' bias expected {} values, got {}",
            bias_name,
            weight.rows,
            bias.len()
        )
        .into());
    }

    let token_count = input.len() / weight.cols;
    let mut output = vec![0.0f32; weight.rows * token_count];
    for token in 0..token_count {
        let input_row = &input[token * weight.cols..(token + 1) * weight.cols];
        let output_row = &mut output[token * weight.rows..(token + 1) * weight.rows];
        output_row.copy_from_slice(&bias);
        for (row, row_values) in weight.values.chunks_exact(weight.cols).enumerate() {
            let mut acc = output_row[row];
            for (w, x) in row_values.iter().zip(input_row.iter()) {
                acc += w * x;
            }
            output_row[row] = acc;
        }
    }
    Ok(output)
}

fn load_rank1_tensor(
    header: &MlxSafetensorsHeader,
    base_name: &str,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let parts = load_canonical_parts(header, base_name)?;
    let mut values = Vec::new();
    for part in parts {
        if part.entry.shape.len() != 1 {
            return Err(format!(
                "tensor '{}' expected rank1 part, got {:?}",
                part.original_name, part.entry.shape
            )
            .into());
        }
        values.extend(read_tensor_f32(header, part.original_name, part.entry)?);
    }
    Ok(values)
}

fn load_rank2_tensor(
    header: &MlxSafetensorsHeader,
    base_name: &str,
) -> Result<CpuMatrix, Box<dyn std::error::Error>> {
    let parts = load_canonical_parts(header, base_name)?;
    let mut values = Vec::new();
    let mut cols = None;
    let mut rows = 0usize;
    for part in parts {
        if part.entry.shape.len() != 2 {
            return Err(format!(
                "tensor '{}' expected rank2 part, got {:?}",
                part.original_name, part.entry.shape
            )
            .into());
        }
        let part_rows = usize::try_from(part.entry.shape[0])?;
        let part_cols = usize::try_from(part.entry.shape[1])?;
        match cols {
            Some(cols) if cols != part_cols => {
                return Err(format!(
                    "tensor '{}' has inconsistent input width {} vs {}",
                    base_name, cols, part_cols
                )
                .into())
            }
            None => cols = Some(part_cols),
            _ => {}
        }
        rows += part_rows;
        values.extend(read_tensor_f32(header, part.original_name, part.entry)?);
    }
    Ok(CpuMatrix {
        rows,
        cols: cols.unwrap_or(0),
        values,
    })
}

fn load_canonical_parts<'a>(
    header: &'a MlxSafetensorsHeader,
    base_name: &str,
) -> Result<Vec<CanonicalTensorPart<'a>>, Box<dyn std::error::Error>> {
    let mut parts = header
        .tensors
        .iter()
        .filter_map(|(name, entry)| {
            let canonical = canonicalize_flux_diffusion_tensor_name(name);
            if canonical == base_name {
                Some(CanonicalTensorPart {
                    original_name: name.as_str(),
                    entry,
                    suffix_index: 0,
                })
            } else if let Some(suffix) = canonical.strip_prefix(&format!("{base_name}.")) {
                suffix
                    .parse::<usize>()
                    .ok()
                    .map(|suffix_index| CanonicalTensorPart {
                        original_name: name.as_str(),
                        entry,
                        suffix_index,
                    })
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    parts.sort_by_key(|part| part.suffix_index);
    if parts.is_empty() {
        return Err(format!("missing canonical tensor '{base_name}'").into());
    }
    Ok(parts)
}

fn read_tensor_f32(
    header: &MlxSafetensorsHeader,
    name: &str,
    entry: &MlxTensorEntry,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let bytes = header.read_tensor_bytes(name)?;
    match entry.dtype {
        MlxDType::F32 => Ok(bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect()),
        MlxDType::F16 => Ok(bytes
            .chunks_exact(2)
            .map(|chunk| f16_to_f32(u16::from_le_bytes(chunk.try_into().unwrap())))
            .collect()),
        MlxDType::BF16 => Ok(bytes
            .chunks_exact(2)
            .map(|chunk| bf16_to_f32(u16::from_le_bytes(chunk.try_into().unwrap())))
            .collect()),
        _ => Err(format!("unsupported tensor dtype {:?} for '{}'", entry.dtype, name).into()),
    }
}

fn cpu_timestep_embedding(timestep: f32, dim: usize, max_period: i32) -> Vec<f32> {
    let half = dim / 2;
    let mut embed = vec![0.0f32; dim];
    for j in 0..half {
        let freq = (-((max_period as f32).ln()) * j as f32 / half as f32).exp();
        let arg = timestep * freq;
        embed[j] = arg.cos();
        embed[j + half] = arg.sin();
    }
    embed
}

fn silu_scalar(x: f32) -> f32 {
    x / (1.0 + (-x).exp())
}

fn gelu_scalar(x: f32) -> f32 {
    let inner = (2.0f32 / std::f32::consts::PI).sqrt() * (x + 0.044_715 * x * x * x);
    0.5 * x * (1.0 + inner.tanh())
}

fn add_inplace(dst: &mut [f32], src: &[f32]) -> Result<(), Box<dyn std::error::Error>> {
    if dst.len() != src.len() {
        return Err(format!(
            "cannot add tensors of different lengths: {} vs {}",
            dst.len(),
            src.len()
        )
        .into());
    }
    for (dst_value, src_value) in dst.iter_mut().zip(src.iter()) {
        *dst_value += src_value;
    }
    Ok(())
}

fn cpu_chunk(
    values: &[f32],
    chunk_width: usize,
    chunk_count: usize,
    chunk_index: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let total_width = chunk_width
        .checked_mul(chunk_count)
        .ok_or("chunk width overflow")?;
    if values.len() % total_width != 0 {
        return Err(format!(
            "cannot chunk tensor of length {} into {} chunks of width {}",
            values.len(),
            chunk_count,
            chunk_width
        )
        .into());
    }
    if chunk_index >= chunk_count {
        return Err(format!(
            "chunk index {} out of range for {} chunks",
            chunk_index, chunk_count
        )
        .into());
    }
    let token_count = values.len() / total_width;
    let mut out = Vec::with_capacity(token_count * chunk_width);
    for token in 0..token_count {
        let base = token * total_width + chunk_index * chunk_width;
        out.extend_from_slice(&values[base..base + chunk_width]);
    }
    Ok(out)
}

fn cpu_row_slice(
    values: &[f32],
    row_width: usize,
    start: usize,
    len: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if values.len() % row_width != 0 {
        return Err(format!(
            "cpu row slice input len {} is not aligned to row width {}",
            values.len(),
            row_width
        )
        .into());
    }
    if start + len > row_width {
        return Err(format!(
            "cpu row slice [{}..{}) exceeds row width {}",
            start,
            start + len,
            row_width
        )
        .into());
    }
    let row_count = values.len() / row_width;
    let mut out = Vec::with_capacity(row_count * len);
    for row in 0..row_count {
        let row_values = &values[row * row_width..(row + 1) * row_width];
        out.extend_from_slice(&row_values[start..start + len]);
    }
    Ok(out)
}

fn cpu_modulated_layer_norm(
    input: &[f32],
    hidden_size: usize,
    scale: &[f32],
    shift: &[f32],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if input.len() % hidden_size != 0 {
        return Err(format!(
            "layer norm input length {} is not divisible by hidden size {}",
            input.len(),
            hidden_size
        )
        .into());
    }
    if scale.len() != hidden_size || shift.len() != hidden_size {
        return Err(format!(
            "layer norm modulation length mismatch: scale={} shift={} hidden={}",
            scale.len(),
            shift.len(),
            hidden_size
        )
        .into());
    }
    let token_count = input.len() / hidden_size;
    let mut output = vec![0.0f32; input.len()];
    for token in 0..token_count {
        let row = &input[token * hidden_size..(token + 1) * hidden_size];
        let mean = row.iter().sum::<f32>() / hidden_size as f32;
        let variance = row
            .iter()
            .map(|value| {
                let centered = *value - mean;
                centered * centered
            })
            .sum::<f32>()
            / hidden_size as f32;
        let inv_std = 1.0f32 / (variance + 1.0e-6).sqrt();
        let output_row = &mut output[token * hidden_size..(token + 1) * hidden_size];
        for index in 0..hidden_size {
            let norm = (row[index] - mean) * inv_std;
            output_row[index] = norm * (1.0 + scale[index]) + shift[index];
        }
    }
    Ok(output)
}

fn cpu_head_rms_norm(
    input: &[f32],
    head_dim: usize,
    head_count: usize,
    scale: &[f32],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let hidden_size = head_dim
        .checked_mul(head_count)
        .ok_or("head size overflow")?;
    if input.len() % hidden_size != 0 {
        return Err(format!(
            "head rms input length {} is not divisible by hidden size {}",
            input.len(),
            hidden_size
        )
        .into());
    }
    if scale.len() != head_dim {
        return Err(format!(
            "head rms scale length {} does not match head dim {}",
            scale.len(),
            head_dim
        )
        .into());
    }
    let token_count = input.len() / hidden_size;
    let mut output = vec![0.0f32; input.len()];
    for token in 0..token_count {
        for head in 0..head_count {
            let start = token * hidden_size + head * head_dim;
            let row = &input[start..start + head_dim];
            let mean_square = row.iter().map(|value| value * value).sum::<f32>() / head_dim as f32;
            let inv_rms = 1.0f32 / (mean_square + 1.0e-6).sqrt();
            let output_row = &mut output[start..start + head_dim];
            for index in 0..head_dim {
                output_row[index] = row[index] * inv_rms * scale[index];
            }
        }
    }
    Ok(output)
}

fn cpu_gated_residual(
    residual: &[f32],
    update: &[f32],
    gate: &[f32],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if residual.len() != update.len() {
        return Err(format!(
            "gated residual length mismatch: residual={} update={}",
            residual.len(),
            update.len()
        )
        .into());
    }
    if update.len() % gate.len() != 0 {
        return Err(format!(
            "gated residual gate length {} is not broadcast-compatible with {}",
            gate.len(),
            update.len()
        )
        .into());
    }
    let token_count = update.len() / gate.len();
    let mut output = residual.to_vec();
    for token in 0..token_count {
        let gate_row = gate;
        let update_row = &update[token * gate.len()..(token + 1) * gate.len()];
        let output_row = &mut output[token * gate.len()..(token + 1) * gate.len()];
        for index in 0..gate.len() {
            output_row[index] += update_row[index] * gate_row[index];
        }
    }
    Ok(output)
}

fn concat_token_rows(
    lhs: &[f32],
    rhs: &[f32],
    row_width: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if lhs.len() % row_width != 0 || rhs.len() % row_width != 0 {
        return Err("concat token rows received non-row-aligned inputs".into());
    }
    let mut out = Vec::with_capacity(lhs.len() + rhs.len());
    out.extend_from_slice(lhs);
    out.extend_from_slice(rhs);
    Ok(out)
}

fn concat_feature_rows(
    lhs: &[f32],
    lhs_width: usize,
    rhs: &[f32],
    rhs_width: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if lhs.len() % lhs_width != 0 || rhs.len() % rhs_width != 0 {
        return Err("concat feature rows received non-row-aligned inputs".into());
    }
    let row_count = lhs.len() / lhs_width;
    if rhs.len() / rhs_width != row_count {
        return Err(format!(
            "concat feature rows count mismatch: {} vs {}",
            row_count,
            rhs.len() / rhs_width
        )
        .into());
    }
    let mut out = Vec::with_capacity(lhs.len() + rhs.len());
    for row in 0..row_count {
        let lhs_row = &lhs[row * lhs_width..(row + 1) * lhs_width];
        let rhs_row = &rhs[row * rhs_width..(row + 1) * rhs_width];
        out.extend_from_slice(lhs_row);
        out.extend_from_slice(rhs_row);
    }
    Ok(out)
}

fn flux_position_ids(
    text_token_count: usize,
    packed_width: usize,
    packed_height: usize,
) -> Vec<[i32; 3]> {
    let mut ids = vec![[0i32; 3]; text_token_count + packed_width * packed_height];
    let mut index = text_token_count;
    for row in 0..packed_height {
        for col in 0..packed_width {
            ids[index] = [0, row as i32, col as i32];
            index += 1;
        }
    }
    ids
}

fn cpu_apply_flux_mrope(
    values: &mut [f32],
    positions: &[[i32; 3]],
    head_dim: usize,
    head_count: usize,
    axes_dim: [u32; 3],
    theta: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    let hidden_size = head_dim
        .checked_mul(head_count)
        .ok_or("mrope hidden size overflow")?;
    if values.len() != positions.len() * hidden_size {
        return Err(format!(
            "mrope values length {} does not match {} tokens x {} hidden",
            values.len(),
            positions.len(),
            hidden_size
        )
        .into());
    }

    let mut section_start = 0usize;
    for (axis_index, axis_dim) in axes_dim.into_iter().enumerate() {
        let section_dim = axis_dim as usize;
        let section_pairs = section_dim / 2;
        for token in 0..positions.len() {
            let pos = positions[token][axis_index] as f32;
            for head in 0..head_count {
                let base = token * hidden_size + head * head_dim + section_start;
                for pair in 0..section_pairs {
                    let exponent = (2.0 * pair as f32) / section_dim as f32;
                    let omega = 1.0f32 / theta.powf(exponent);
                    let angle = pos * omega;
                    let cos = angle.cos();
                    let sin = angle.sin();
                    let even = base + pair * 2;
                    let odd = even + 1;
                    let x0 = values[even];
                    let x1 = values[odd];
                    values[even] = x0 * cos - x1 * sin;
                    values[odd] = x0 * sin + x1 * cos;
                }
            }
        }
        section_start += section_dim;
    }
    Ok(())
}

fn cpu_attention(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    head_dim: usize,
    head_count: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let hidden_size = head_dim
        .checked_mul(head_count)
        .ok_or("attention hidden size overflow")?;
    if q.len() != k.len() || q.len() != v.len() || q.len() % hidden_size != 0 {
        return Err("attention input shape mismatch".into());
    }
    let token_count = q.len() / hidden_size;
    let scale = 1.0f32 / (head_dim as f32).sqrt();
    let mut output = vec![0.0f32; q.len()];
    let mut scores = vec![0.0f32; token_count];
    let mut weights = vec![0.0f32; token_count];

    for head in 0..head_count {
        for q_token in 0..token_count {
            let q_base = q_token * hidden_size + head * head_dim;
            let q_row = &q[q_base..q_base + head_dim];
            let mut max_score = f32::NEG_INFINITY;
            for k_token in 0..token_count {
                let k_base = k_token * hidden_size + head * head_dim;
                let k_row = &k[k_base..k_base + head_dim];
                let mut dot = 0.0f32;
                for dim in 0..head_dim {
                    dot += q_row[dim] * k_row[dim];
                }
                let score = dot * scale;
                scores[k_token] = score;
                max_score = max_score.max(score);
            }

            let mut sum = 0.0f32;
            for token in 0..token_count {
                let weight = (scores[token] - max_score).exp();
                weights[token] = weight;
                sum += weight;
            }

            let out_base = q_token * hidden_size + head * head_dim;
            let out_row = &mut output[out_base..out_base + head_dim];
            out_row.fill(0.0);
            for k_token in 0..token_count {
                let v_base = k_token * hidden_size + head * head_dim;
                let v_row = &v[v_base..v_base + head_dim];
                let normalized = weights[k_token] / sum;
                for dim in 0..head_dim {
                    out_row[dim] += normalized * v_row[dim];
                }
            }
        }
    }

    Ok(output)
}

struct CpuMatrix {
    rows: usize,
    cols: usize,
    values: Vec<f32>,
}

struct CanonicalTensorPart<'a> {
    original_name: &'a str,
    entry: &'a MlxTensorEntry,
    suffix_index: usize,
}
