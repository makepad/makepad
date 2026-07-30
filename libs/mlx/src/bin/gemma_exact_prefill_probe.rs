use makepad_mlx::fnv1a64_u32_words;
use makepad_mlx::layer0_cached_case::{GemmaExactMetalBackendMode, GemmaExactMetalConfig};
use makepad_mlx::text_runtime::{probe_exact_prefill_with_backend_config, GemmaPromptFormat};
use std::env;
use std::error::Error;
use std::path::PathBuf;

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn usage() {
    eprintln!(
        "Usage: gemma_exact_prefill_probe [model.safetensors|model_dir] [--raw-bos] <prompt>"
    );
}

fn bf16_word_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut model_path = default_model_path();
    let mut prompt_format = GemmaPromptFormat::AutoChat;
    let mut prompt_parts = Vec::new();

    for arg in env::args().skip(1) {
        if arg == "-h" || arg == "--help" {
            usage();
            return Ok(());
        }
        if arg == "--raw-bos" {
            prompt_format = GemmaPromptFormat::RawBos;
            continue;
        }
        if prompt_parts.is_empty()
            && (arg.ends_with(".safetensors") || PathBuf::from(&arg).is_dir())
        {
            model_path = PathBuf::from(arg);
        } else {
            prompt_parts.push(arg);
        }
    }

    if prompt_parts.is_empty() {
        usage();
        return Err("missing prompt".into());
    }

    let prompt_text = prompt_parts.join(" ");
    let mut backend_config = GemmaExactMetalConfig::default();
    backend_config.backend_mode = GemmaExactMetalBackendMode::Force;
    let output = probe_exact_prefill_with_backend_config(
        model_path.clone(),
        prompt_text.clone(),
        prompt_format,
        backend_config,
    )?;
    let final_hidden_bits = output
        .final_hidden_bf16_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .map(f32::to_bits)
        .collect::<Vec<_>>();

    println!("model={}", model_path.display());
    println!("prompt={:?}", prompt_text);
    println!("formatted_prompt={:?}", output.formatted_prompt_text);
    println!("prompt_ids={:?}", output.prompt_token_ids);
    println!(
        "final_hidden_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&final_hidden_bits)
    );
    println!("top1_token_id={}", output.next_token.token_id);
    println!("top1_logit={}", output.next_token.logit);
    println!("top1_text={:?}", output.next_token_text);

    Ok(())
}
