use makepad_mlx::chat::{GemmaChatDecodeMode, GemmaChatSession};
use std::env;
use std::path::PathBuf;

const DEFAULT_MAX_NEW_TOKENS: usize = 32;

fn usage() -> &'static str {
    "Usage: gemma_chat_once <model_dir> <prompt> [--greedy] [--max-new-tokens N] [--no-cuda]"
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let model_path = PathBuf::from(args.next().ok_or_else(|| usage().to_string())?);
    let prompt = args.next().ok_or_else(|| usage().to_string())?;
    let mut decode_mode = GemmaChatDecodeMode::Sampled;
    let mut max_new_tokens = DEFAULT_MAX_NEW_TOKENS;
    let mut no_cuda = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--greedy" => decode_mode = GemmaChatDecodeMode::Greedy,
            "--max-new-tokens" => {
                let value = args.next().ok_or("--max-new-tokens requires a value")?;
                max_new_tokens = value.parse::<usize>()?;
            }
            "--no-cuda" => no_cuda = true,
            value if value.starts_with("--") => {
                return Err(format!("unknown option: {value}\n{}", usage()).into());
            }
            value => {
                return Err(format!("unexpected positional argument: {value}\n{}", usage()).into());
            }
        }
    }

    if no_cuda {
        unsafe {
            env::set_var("CUDA_VISIBLE_DEVICES", "");
        }
    }

    let mut session = GemmaChatSession::load_with_mode(
        &model_path,
        Some(max_new_tokens),
        decode_mode,
    )?;
    let output = session.send_user_message(prompt)?;
    println!("backend={}", session.backend_label());
    println!("prompt_token_count={}", output.prompt_token_ids.len());
    println!("generated_token_ids={:?}", output.generated_token_ids);
    println!("generated_text={:?}", output.generated_text);
    println!("stop_reason={:?}", output.stop_reason);
    Ok(())
}
