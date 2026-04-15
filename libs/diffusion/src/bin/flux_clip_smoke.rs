use makepad_diffusion::clip_l::{CompiledClipL, LoadedClipLWeights};
use makepad_diffusion::comfy::FluxWorkflow;
use makepad_diffusion::flux::{
    tokenize_flux_clip_l_prompt, ComfyModelRoots, FluxPromptToImagePlan,
};
use std::env;

fn usage() -> ! {
    eprintln!("usage: flux-clip-smoke <workflow.json> <comfy-root-or-model-root>");
    std::process::exit(1);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workflow_path = env::args().nth(1).unwrap_or_else(|| usage());
    let root = env::args().nth(2).unwrap_or_else(|| usage());

    let workflow = FluxWorkflow::from_file(&workflow_path)?;
    let roots = ComfyModelRoots::new(root);
    let plan = FluxPromptToImagePlan::from_workflow(&workflow, &roots)?;
    let clip_l_path = plan
        .bundle
        .clip_l_path
        .as_ref()
        .ok_or("workflow bundle does not include clip_l")?;
    let tokenized = tokenize_flux_clip_l_prompt(&plan.prompts.clip_l)?;
    if tokenized.chunks.len() != 1 {
        return Err(format!(
            "flux-clip-smoke currently supports one clip_l chunk, got {}",
            tokenized.chunks.len()
        )
        .into());
    }

    let mut weights = LoadedClipLWeights::load(clip_l_path)?;
    let compiled = CompiledClipL::compile(&mut weights, &tokenized.chunks[0])?;
    let run = compiled.execute(&weights, &tokenized.chunks[0].token_ids)?;

    println!("workflow: {}", workflow.path.display());
    println!("clip_l model: {}", clip_l_path.display());
    println!("clip_l backend: {}", compiled.backend_name());
    println!("prompt.clip_l: {}", plan.prompts.clip_l);
    println!(
        "clip_l config: vocab={} hidden={} layers={} heads={} positions={}",
        weights.config.vocab_size,
        weights.config.hidden_size,
        weights.config.layer_count,
        weights.config.attention_head_count,
        weights.config.max_position_embeddings
    );
    println!(
        "clip_l output: hidden={}x{} pooled={} eos_index={}",
        run.hidden_size,
        run.token_count,
        run.pooled.len(),
        run.eos_index
    );
    let hidden_preview_len = run.hidden_size.min(8);
    let eos_start = run.eos_index * run.hidden_size;
    let eos_hidden = &run.hidden_states[eos_start..eos_start + hidden_preview_len];
    let pooled_preview = &run.pooled[..run.pooled.len().min(8)];
    let pooled_max_abs = run
        .pooled
        .iter()
        .fold(0.0f32, |acc, value| acc.max(value.abs()));
    let hidden_max_abs = run
        .hidden_states
        .iter()
        .fold(0.0f32, |acc, value| acc.max(value.abs()));
    let eos_pooled_max_diff = eos_hidden
        .iter()
        .zip(pooled_preview.iter())
        .fold(0.0f32, |acc, (hidden, pooled)| {
            acc.max((hidden - pooled).abs())
        });
    println!(
        "clip_l hidden[0..{}]: {:?}",
        hidden_preview_len,
        &run.hidden_states[..hidden_preview_len]
    );
    println!(
        "clip_l eos_hidden[0..{}]: {:?}",
        hidden_preview_len, eos_hidden
    );
    println!(
        "clip_l pooled[0..8]: {:?}",
        &run.pooled[..run.pooled.len().min(8)]
    );
    println!(
        "clip_l max_abs: hidden={} pooled={} eos_pooled_preview_max_diff={}",
        hidden_max_abs, pooled_max_abs, eos_pooled_max_diff
    );

    Ok(())
}
