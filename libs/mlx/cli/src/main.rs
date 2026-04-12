use makepad_mlx::chat::{GemmaChatDecodeMode, GemmaChatRole, GemmaChatSession};
use makepad_mlx::text_runtime::{
    GemmaExactMetalBackendMode, GemmaExactMetalConfig, GemmaExactMetalKvCompressionMode,
};
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const DEFAULT_CLI_MAX_NEW_TOKENS: usize = 512;
const STREAM_FLUSH_CHARS: usize = 256;
const STREAM_FLUSH_INTERVAL: Duration = Duration::from_millis(150);

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn usage() -> &'static str {
    "Usage: mlx-cli [model.safetensors|model_dir] [--image PATH] [--max-new-tokens N] [--greedy] [--reference-text-backend] [--force-exact-text-backend] [--rotor-k-cache] [--rotor-k-cache-planar3]"
}

fn format_max_new_tokens(max_new_tokens: Option<usize>) -> String {
    max_new_tokens
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unbounded".to_owned())
}

fn format_stop_reason(stop_reason: makepad_mlx::text_runtime::GemmaStopReason) -> String {
    match stop_reason {
        makepad_mlx::text_runtime::GemmaStopReason::MaxNewTokens => "max_new_tokens".to_owned(),
        makepad_mlx::text_runtime::GemmaStopReason::EosToken(token_id) => {
            format!("eos({token_id})")
        }
    }
}

fn print_block(prefix: &str, text: &str) {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        println!("{prefix}");
        return;
    }
    let mut lines = trimmed.lines();
    if let Some(first) = lines.next() {
        println!("{prefix}{first}");
    }
    for line in lines {
        println!("  {line}");
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut model_path = default_model_path();
    let mut initial_image_path = None;
    let mut max_new_tokens = None;
    let mut decode_mode = GemmaChatDecodeMode::Sampled;
    let mut backend_config = GemmaExactMetalConfig::default();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--image" => {
                initial_image_path = Some(PathBuf::from(
                    args.next().ok_or("--image requires a value")?,
                ));
            }
            "--max-new-tokens" => {
                let value = args.next().ok_or("--max-new-tokens requires a value")?;
                max_new_tokens = Some(value.parse::<usize>()?);
            }
            "--greedy" => {
                decode_mode = GemmaChatDecodeMode::Greedy;
            }
            "--reference-text-backend" => {
                backend_config.backend_mode = GemmaExactMetalBackendMode::Disabled;
            }
            "--force-exact-text-backend" => {
                backend_config.backend_mode = GemmaExactMetalBackendMode::Force;
            }
            "--rotor-k-cache" => {
                backend_config.kv_compression =
                    GemmaExactMetalKvCompressionMode::RotorPlanar4FullAttentionK;
            }
            "--rotor-k-cache-planar3" => {
                backend_config.kv_compression =
                    GemmaExactMetalKvCompressionMode::RotorPlanar3FullAttentionK;
            }
            value if value.starts_with("--") => {
                return Err(format!("unknown option: {value}\n{}", usage()).into());
            }
            value => {
                model_path = PathBuf::from(value);
            }
        }
    }

    if max_new_tokens.is_none() {
        max_new_tokens = Some(DEFAULT_CLI_MAX_NEW_TOKENS);
    }

    eprintln!("loading model={}...", model_path.display());
    let mut session = GemmaChatSession::load_with_mode_and_backend_config(
        &model_path,
        max_new_tokens,
        decode_mode,
        backend_config.clone(),
    )?;
    if let Some(image_path) = initial_image_path {
        session.set_image(image_path);
    }
    println!("model={}", model_path.display());
    println!(
        "max_new_tokens={}",
        format_max_new_tokens(session.max_new_tokens())
    );
    println!(
        "decode_mode={}",
        match session.decode_mode() {
            GemmaChatDecodeMode::Sampled => "sampled",
            GemmaChatDecodeMode::Greedy => "greedy",
        }
    );
    println!(
        "text_backend={}",
        match backend_config.backend_mode {
            GemmaExactMetalBackendMode::Auto => "auto",
            GemmaExactMetalBackendMode::Force => "force_exact",
            GemmaExactMetalBackendMode::Disabled => "reference",
        }
    );
    println!(
        "kv_compression={}",
        match backend_config.kv_compression {
            GemmaExactMetalKvCompressionMode::Disabled => "disabled",
            GemmaExactMetalKvCompressionMode::RotorPlanar3FullAttentionK =>
                "rotor_planar3_full_attention_k",
            GemmaExactMetalKvCompressionMode::RotorPlanar4FullAttentionK =>
                "rotor_planar4_full_attention_k",
        }
    );
    println!("backend={}", session.backend_label());
    if let Some(image_path) = session.current_image_path() {
        println!("image={}", image_path.display());
    }
    println!("commands: /image PATH /clear-image /reset /history /exit");
    println!("ready");

    let stdin = io::stdin();
    loop {
        print!("you> ");
        io::stdout().flush()?;

        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            break;
        }
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        match input {
            "/exit" | "/quit" => break,
            "/reset" => {
                session.reset();
                println!("history cleared");
                continue;
            }
            "/clear-image" => {
                session.clear_image();
                println!("image cleared");
                println!("backend={}", session.backend_label());
                continue;
            }
            "/history" => {
                for message in session.messages() {
                    let prefix = match message.role {
                        GemmaChatRole::User => "user> ",
                        GemmaChatRole::Assistant => "assistant> ",
                    };
                    print_block(prefix, message.content.as_ref());
                }
                continue;
            }
            _ => {}
        }
        if let Some(rest) = input.strip_prefix("/image ") {
            let image_path = PathBuf::from(rest.trim());
            session.set_image(image_path.clone());
            println!("image={}", image_path.display());
            println!("backend={}", session.backend_label());
            continue;
        }

        print!("assistant> ");
        io::stdout().flush()?;
        let mut buffered_output = String::new();
        let mut last_flush = Instant::now();
        let output = session.send_user_message_streaming(input, |delta| {
            buffered_output.push_str(delta);
            if buffered_output.len() >= STREAM_FLUSH_CHARS
                || last_flush.elapsed() >= STREAM_FLUSH_INTERVAL
            {
                print!("{buffered_output}");
                buffered_output.clear();
                io::stdout().flush()?;
                last_flush = Instant::now();
            }
            Ok(())
        })?;
        if !buffered_output.is_empty() {
            print!("{buffered_output}");
        }
        println!();
        println!(
            "[generated_tokens={} stop={} prompt_prefill_tok_s={:.3} steady_decode_tok_s={:.3} overall_tok_s={:.3}]",
            output.generated_token_ids.len(),
            format_stop_reason(output.stop_reason),
            output.metrics.prompt_prefill_tokens_per_second,
            output.metrics.steady_state_decode_tokens_per_second,
            output.metrics.decode_tokens_per_second,
        );
    }

    Ok(())
}
