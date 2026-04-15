use makepad_diffusion::comfy::FluxWorkflow;
use makepad_diffusion::flux::{ComfyModelRoots, FluxPromptToImagePlan};
use makepad_diffusion::flux_text::{
    FluxCompiledTextEncoders, FluxConditioning, FluxLoadedTextEncoders, FluxTokenizedPrompts,
};
use std::{env, fs, path::Path};

#[derive(Debug)]
struct Summary {
    min: f32,
    max: f32,
    mean_abs: f32,
}

fn usage() -> ! {
    eprintln!("usage: flux-text-smoke <workflow.json> <comfy-root-or-model-root>");
    std::process::exit(1);
}

fn summarize(values: &[f32]) -> Summary {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut sum_abs = 0.0f64;
    for &value in values {
        min = min.min(value);
        max = max.max(value);
        sum_abs += value.abs() as f64;
    }
    Summary {
        min,
        max,
        mean_abs: if values.is_empty() {
            0.0
        } else {
            (sum_abs / values.len() as f64) as f32
        },
    }
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

fn f32_bytes_to_vec(bytes: &[u8]) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if bytes.len() % std::mem::size_of::<f32>() != 0 {
        return Err(format!("expected f32 byte length, got {} bytes", bytes.len()).into());
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
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

fn load_conditioning_override() -> Result<Option<FluxConditioning>, Box<dyn std::error::Error>> {
    let dir = match env::var("FLUX_COND_DIR") {
        Ok(dir) => dir,
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(err) => return Err(Box::new(err)),
    };
    let dir_path = Path::new(&dir);
    let clip_pooled = f32_bytes_to_vec(&fs::read(dir_path.join("flux_clip_pooled.bin"))?)?;
    let t5_hidden_states = f32_bytes_to_vec(&fs::read(dir_path.join("flux_t5_hidden.bin"))?)?;
    let meta_text = fs::read_to_string(dir_path.join("flux_t5_meta.txt")).unwrap_or_default();
    let hidden_size = parse_meta_usize(&meta_text, "hidden_size").unwrap_or(4096);
    let token_count = parse_meta_usize(&meta_text, "token_count")
        .unwrap_or_else(|| t5_hidden_states.len() / hidden_size.max(1));

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

fn f32s_to_le_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<f32>());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn maybe_dump_conditioning(
    conditioning: &FluxConditioning,
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = match env::var("FLUX_DUMP_COND_DIR") {
        Ok(dir) => dir,
        Err(env::VarError::NotPresent) => return Ok(()),
        Err(err) => return Err(Box::new(err)),
    };
    let dir_path = Path::new(&dir);
    fs::create_dir_all(dir_path)?;
    fs::write(
        dir_path.join("flux_clip_pooled.bin"),
        f32s_to_le_bytes(&conditioning.clip_pooled),
    )?;
    fs::write(
        dir_path.join("flux_t5_hidden.bin"),
        f32s_to_le_bytes(&conditioning.t5_hidden_states),
    )?;
    fs::write(
        dir_path.join("flux_t5_meta.txt"),
        format!(
            "hidden_size={}\ntoken_count={}\neos_index={}\n",
            conditioning.t5_hidden_size, conditioning.t5_token_count, conditioning.t5_eos_index
        ),
    )?;
    println!("conditioning_dump: {}", dir_path.display());
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workflow_path = env::args().nth(1).unwrap_or_else(|| usage());
    let root = env::args().nth(2).unwrap_or_else(|| usage());

    let workflow = FluxWorkflow::from_file(&workflow_path)?;
    let roots = ComfyModelRoots::new(root);
    let plan = FluxPromptToImagePlan::from_workflow(&workflow, &roots)?;
    let prompts = FluxTokenizedPrompts::from_prompts(&plan.prompts)?;
    let mut weights = FluxLoadedTextEncoders::load_from_plan(&plan)?;
    let compiled = FluxCompiledTextEncoders::compile(&mut weights, &prompts)?;
    let clip_backend = compiled.clip_backend_name();
    let t5_backend = compiled.t5_backend_name();
    let conditioning = compiled.execute(&weights, &prompts)?;
    maybe_dump_conditioning(&conditioning)?;
    let clip_summary = summarize(&conditioning.clip_pooled);
    let t5_summary = summarize(&conditioning.t5_hidden_states);
    let reference_conditioning = load_conditioning_override()?;

    println!("workflow: {}", workflow.path.display());
    println!("clip_l backend: {}", clip_backend);
    println!("t5xxl backend: {}", t5_backend);
    println!("prompt.clip_l: {}", plan.prompts.clip_l);
    println!("prompt.t5xxl: {}", plan.prompts.t5xxl);
    println!(
        "clip.token_ids[0..8]: {:?}",
        &prompts.clip_l.token_ids[..prompts.clip_l.token_ids.len().min(8)]
    );
    println!(
        "t5.token_ids[0..8]: {:?}",
        &prompts.t5xxl.token_ids[..prompts.t5xxl.token_ids.len().min(8)]
    );
    println!(
        "conditioning: clip_pooled={} t5_hidden={}x{} t5_valid_tokens={} t5_eos_index={}",
        conditioning.clip_pooled.len(),
        conditioning.t5_hidden_size,
        conditioning.t5_token_count,
        conditioning
            .t5_attention_mask
            .iter()
            .filter(|&&value| value != 0)
            .count(),
        conditioning.t5_eos_index
    );
    println!(
        "clip_pooled.summary: min={} max={} mean_abs={}",
        clip_summary.min, clip_summary.max, clip_summary.mean_abs
    );
    println!(
        "t5_hidden.summary: min={} max={} mean_abs={}",
        t5_summary.min, t5_summary.max, t5_summary.mean_abs
    );
    println!(
        "clip_pooled[0..8]: {:?}",
        &conditioning.clip_pooled[..conditioning.clip_pooled.len().min(8)]
    );
    println!(
        "t5_hidden[0..8]: {:?}",
        &conditioning.t5_hidden_states[..conditioning.t5_hidden_states.len().min(8)]
    );
    let eos_start = conditioning.t5_eos_index * conditioning.t5_hidden_size;
    let eos_end = eos_start + conditioning.t5_hidden_size.min(8);
    println!(
        "t5_eos_hidden[0..8]: {:?}",
        &conditioning.t5_hidden_states[eos_start..eos_end]
    );
    let pad_index = conditioning
        .t5_eos_index
        .saturating_add(1)
        .min(conditioning.t5_token_count.saturating_sub(1));
    let pad_start = pad_index * conditioning.t5_hidden_size;
    let pad_end = pad_start + conditioning.t5_hidden_size.min(8);
    println!(
        "t5_pad_hidden[token={}][0..8]: {:?}",
        pad_index,
        &conditioning.t5_hidden_states[pad_start..pad_end]
    );
    if let Some(reference_conditioning) = reference_conditioning.as_ref() {
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

    Ok(())
}
