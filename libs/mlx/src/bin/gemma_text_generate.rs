use makepad_mlx::text_runtime::{
    generate_multimodal_text_with_backend_config, generate_text_with_backend_config,
    GemmaPromptFormat, GemmaTextBackendConfig, GemmaTextBackendMode, GemmaTextGenerationOptions,
    GemmaTextKvCompressionMode,
};
use std::env;
use std::path::PathBuf;

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn usage() -> &'static str {
    "Usage: gemma_text_generate [model.safetensors|model_dir] [--image PATH] [--raw-bos] [--max-new-tokens N] [--reference-text-backend] [--force-exact-text-backend] [--rotor-k-cache] [--rotor-k-cache-planar3] <prompt>"
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut model_path = default_model_path();
    let mut image_path = None;
    let mut prompt_format = GemmaPromptFormat::AutoChat;
    let mut max_new_tokens = 8usize;
    let mut backend_config = GemmaTextBackendConfig::default();
    let mut prompt_parts = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--image" => {
                image_path = Some(PathBuf::from(
                    args.next().ok_or("--image requires a value")?,
                ));
            }
            "--raw-bos" => {
                prompt_format = GemmaPromptFormat::RawBos;
            }
            "--max-new-tokens" => {
                let value = args.next().ok_or("--max-new-tokens requires a value")?;
                max_new_tokens = value.parse::<usize>()?;
            }
            "--reference-text-backend" => {
                backend_config.backend_mode = GemmaTextBackendMode::Disabled;
            }
            "--force-exact-text-backend" => {
                backend_config.backend_mode = GemmaTextBackendMode::Force;
            }
            "--rotor-k-cache" => {
                backend_config.kv_compression =
                    GemmaTextKvCompressionMode::RotorPlanar4FullAttentionK;
            }
            "--rotor-k-cache-planar3" => {
                backend_config.kv_compression =
                    GemmaTextKvCompressionMode::RotorPlanar3FullAttentionK;
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
    let options = GemmaTextGenerationOptions {
        max_new_tokens,
        prompt_format,
    };
    let output = if let Some(image_path) = image_path {
        generate_multimodal_text_with_backend_config(
            model_path,
            image_path,
            prompt,
            options,
            backend_config,
        )?
    } else {
        generate_text_with_backend_config(model_path, prompt, options, backend_config)?
    };

    println!("prompt_ids={:?}", output.prompt_token_ids);
    println!("generated_ids={:?}", output.generated_token_ids);
    println!("generated_text={:?}", output.generated_text);
    println!("stop_reason={:?}", output.stop_reason);

    Ok(())
}
