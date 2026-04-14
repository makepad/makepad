use makepad_diffusion::comfy::FluxWorkflow;
use makepad_diffusion::flux::{
    tokenize_flux_clip_l_prompt, tokenize_flux_t5xxl_prompt, FLUX_CLIP_L_MAX_LENGTH,
    FLUX_T5XXL_MAX_LENGTH,
};
use std::env;

fn usage() -> ! {
    eprintln!("usage: flux-tokenize <workflow.json>");
    std::process::exit(1);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workflow_path = env::args().nth(1).unwrap_or_else(|| usage());
    let workflow = FluxWorkflow::from_file(&workflow_path)?;
    let clip_l = tokenize_flux_clip_l_prompt(&workflow.prompts.clip_l)?;
    let t5xxl = tokenize_flux_t5xxl_prompt(&workflow.prompts.t5xxl)?;

    println!("workflow: {}", workflow.path.display());
    println!("prompt.clip_l: {}", workflow.prompts.clip_l);
    println!("prompt.t5xxl: {}", workflow.prompts.t5xxl);
    println!("clip_l max length: {}", FLUX_CLIP_L_MAX_LENGTH);
    println!("t5xxl max length: {}", FLUX_T5XXL_MAX_LENGTH);
    println!("clip_l raw tokens: {}", clip_l.raw_token_ids.len());
    println!("clip_l chunks: {}", clip_l.chunks.len());
    for (index, chunk) in clip_l.chunks.iter().enumerate() {
        println!(
            "clip_l chunk {}: len={} eos_index={} ids={:?}",
            index,
            chunk.token_ids.len(),
            chunk.eos_index,
            chunk.token_ids
        );
    }
    println!("t5xxl raw tokens: {}", t5xxl.raw_token_ids.len());
    println!(
        "t5xxl sequence: len={} eos_index={} ids={:?}",
        t5xxl.token_ids.len(),
        t5xxl.eos_index,
        t5xxl.token_ids
    );
    println!("t5xxl attention_mask: {:?}", t5xxl.attention_mask);

    Ok(())
}
