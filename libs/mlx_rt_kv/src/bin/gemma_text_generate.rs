use makepad_mlx_rt_kv::text_runtime::{
    generate_text, GemmaPromptFormat, GemmaTextGenerationOptions,
};
use std::env;
use std::path::PathBuf;

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn usage() -> &'static str {
    "Usage: gemma_text_generate [model.safetensors] [--raw-bos] [--max-new-tokens N] <prompt>"
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut model_path = default_model_path();
    let mut prompt_format = GemmaPromptFormat::Gemma4UserTurn;
    let mut max_new_tokens = 8usize;
    let mut prompt_parts = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--raw-bos" => {
                prompt_format = GemmaPromptFormat::RawBos;
            }
            "--max-new-tokens" => {
                let value = args.next().ok_or("--max-new-tokens requires a value")?;
                max_new_tokens = value.parse::<usize>()?;
            }
            value if value.starts_with("--") => {
                return Err(format!("unknown option: {value}\n{}", usage()).into());
            }
            value => {
                if prompt_parts.is_empty()
                    && (value.ends_with(".safetensors") || PathBuf::from(value).is_dir())
                {
                    model_path = PathBuf::from(value);
                } else {
                    prompt_parts.push(value.to_string());
                    prompt_parts.extend(args);
                    break;
                }
            }
        }
    }

    if prompt_parts.is_empty() {
        return Err(usage().into());
    }

    let prompt = prompt_parts.join(" ");
    let output = generate_text(
        model_path,
        prompt,
        GemmaTextGenerationOptions {
            max_new_tokens,
            prompt_format,
        },
    )?;

    println!("prompt_ids={:?}", output.prompt_token_ids);
    println!("generated_ids={:?}", output.generated_token_ids);
    println!("generated_text={:?}", output.generated_text);
    println!("stop_reason={:?}", output.stop_reason);

    Ok(())
}
