use makepad_diffusion::comfy::FluxWorkflow;
use makepad_diffusion::flux::{tokenize_flux_t5xxl_prompt, ComfyModelRoots, FluxPromptToImagePlan};
use makepad_diffusion::t5_encoder::{
    CompiledT5xxlMetal, LazyT5xxlMetal, LoadedT5xxlWeights, T5xxlExecutionMode,
};
use std::env;

fn usage() -> ! {
    eprintln!("usage: flux-t5-smoke <workflow.json> <comfy-root-or-model-root>");
    std::process::exit(1);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workflow_path = env::args().nth(1).unwrap_or_else(|| usage());
    let root = env::args().nth(2).unwrap_or_else(|| usage());

    let workflow = FluxWorkflow::from_file(&workflow_path)?;
    let roots = ComfyModelRoots::new(root);
    let plan = FluxPromptToImagePlan::from_workflow(&workflow, &roots)?;
    let t5xxl_path = plan
        .bundle
        .t5xxl_path
        .as_ref()
        .ok_or("workflow bundle does not include t5xxl")?;
    let tokenized = tokenize_flux_t5xxl_prompt(&plan.prompts.t5xxl)?;

    let mut weights = LoadedT5xxlWeights::load(t5xxl_path)?;
    let mode = T5xxlExecutionMode::from_env();
    let run = match mode {
        T5xxlExecutionMode::Lazy => {
            let lazy = LazyT5xxlMetal::compile(&mut weights, &tokenized)?;
            lazy.execute(&weights, &tokenized.token_ids)?
        }
        T5xxlExecutionMode::Compiled => {
            let compiled = CompiledT5xxlMetal::compile(&mut weights, &tokenized)?;
            compiled.execute(&weights, &tokenized.token_ids)?
        }
    };

    println!("workflow: {}", workflow.path.display());
    println!("t5xxl model: {}", t5xxl_path.display());
    println!("t5xxl backend: {}", mode.as_str());
    println!("prompt.t5xxl: {}", plan.prompts.t5xxl);
    println!(
        "t5xxl config: vocab={} d_model={} layers={} heads={} head_dim={} ffn={} buckets={}",
        weights.config.vocab_size,
        weights.config.model_dim,
        weights.config.layer_count,
        weights.config.attention_head_count,
        weights.config.head_dim(),
        weights.config.feedforward_dim,
        weights.config.relative_attention_bucket_count
    );
    println!(
        "t5xxl output: hidden={}x{} eos_index={} valid_tokens={}",
        run.hidden_size,
        run.token_count,
        run.eos_index,
        tokenized.attention_mask.iter().filter(|&&value| value != 0).count()
    );
    let hidden_preview_len = run.hidden_size.min(8);
    let eos_start = run.eos_index * run.hidden_size;
    let eos_hidden = &run.hidden_states[eos_start..eos_start + hidden_preview_len];
    let hidden_max_abs = run
        .hidden_states
        .iter()
        .fold(0.0f32, |acc, value| acc.max(value.abs()));
    println!(
        "t5xxl hidden[0..{}]: {:?}",
        hidden_preview_len,
        &run.hidden_states[..hidden_preview_len]
    );
    println!("t5xxl eos_hidden[0..{}]: {:?}", hidden_preview_len, eos_hidden);
    println!("t5xxl max_abs: {}", hidden_max_abs);

    Ok(())
}
