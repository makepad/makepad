use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::Instant;

use makepad_llama::{LlamaSession, LlamaSessionConfig, LlamaStopReason};

const DEFAULT_MAX_NEW_TOKENS: usize = 64;
const DEFAULT_TOKENIZE_BIN: &str =
    "local/llama.cpp/build-arm64-apple-clang-release/bin/llama-tokenize";
const DEFAULT_UPSTREAM_COMPLETION_BIN: &str =
    "local/llama.cpp/build-arm64-apple-clang-release/bin/llama-completion";

struct Args {
    model_path: PathBuf,
    prompt: String,
    max_new_tokens: usize,
    tokenize_bin: PathBuf,
    upstream_completion_bin: PathBuf,
    no_bos: bool,
    dump_token_ids: bool,
    no_stream: bool,
    verify_upstream: bool,
}

fn main() {
    match run() {
        Ok(()) => {}
        Err(err) => {
            eprintln!("llama-generate failed: {err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args(std::env::args_os())?;
    let prompt_token_ids = tokenize_prompt(&args)?;
    if prompt_token_ids.is_empty() {
        return Err("tokenizer produced no prompt tokens".into());
    }

    if args.dump_token_ids {
        eprintln!("prompt.token_ids: {:?}", prompt_token_ids);
    }

    let max_context = prompt_token_ids
        .len()
        .checked_add(args.max_new_tokens)
        .ok_or("overflow computing total generation context")?;
    let mut session = LlamaSession::load(
        &args.model_path,
        LlamaSessionConfig {
            max_context: Some(u32::try_from(max_context)?),
            ..LlamaSessionConfig::default()
        },
    )?;

    if !args.no_stream {
        print!("{}", args.prompt);
        std::io::stdout().flush()?;
    }

    let total_start = Instant::now();
    let prefill_start = Instant::now();
    session.append_tokens(&prompt_token_ids)?;
    let prefill_elapsed = prefill_start.elapsed();

    let generation_start = Instant::now();
    let generation = session.continue_greedy(args.max_new_tokens)?;
    let generation_elapsed = generation_start.elapsed();
    let total_elapsed = total_start.elapsed();

    if !args.no_stream {
        print!("{}", generation.text);
        println!();
    }

    if args.dump_token_ids {
        eprintln!("generated.token_ids: {:?}", generation.token_ids);
    }

    if args.verify_upstream {
        let upstream_output = run_upstream_completion(&args)?;
        verify_exact_output(&generation.text, &upstream_output)?;
        eprintln!("verify.upstream.exact_text_match: true");
        eprintln!("verify.upstream.generated_bytes: {}", upstream_output.len());
    }

    eprintln!("stop.reason: {}", stop_reason_name(generation.stop_reason));
    eprintln!("prompt.tokens: {}", prompt_token_ids.len());
    eprintln!("generated.tokens: {}", generation.token_ids.len());
    eprintln!("prefill.seconds: {:.3}", prefill_elapsed.as_secs_f64());
    eprintln!(
        "prefill.tok_s: {:.3}",
        tok_per_second(prompt_token_ids.len(), prefill_elapsed.as_secs_f64())
    );
    eprintln!(
        "generation.seconds: {:.3}",
        generation_elapsed.as_secs_f64()
    );
    eprintln!(
        "generation.tok_s: {:.3}",
        tok_per_second(generation.token_ids.len(), generation_elapsed.as_secs_f64())
    );
    eprintln!("total.seconds: {:.3}", total_elapsed.as_secs_f64());
    eprintln!(
        "total.tok_s: {:.3}",
        tok_per_second(
            prompt_token_ids.len() + generation.token_ids.len(),
            total_elapsed.as_secs_f64()
        )
    );
    Ok(())
}

fn parse_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Args, Box<dyn std::error::Error>> {
    let mut args = args.into_iter();
    let _exe = args.next();

    let mut model_path = None;
    let mut prompt = None;
    let mut prompt_parts = Vec::new();
    let mut max_new_tokens = DEFAULT_MAX_NEW_TOKENS;
    let mut tokenize_bin = PathBuf::from(DEFAULT_TOKENIZE_BIN);
    let mut upstream_completion_bin = PathBuf::from(DEFAULT_UPSTREAM_COMPLETION_BIN);
    let mut no_bos = false;
    let mut dump_token_ids = false;
    let mut no_stream = false;
    let mut verify_upstream = false;

    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "--max-new-tokens" => {
                let value = args.next().ok_or("--max-new-tokens requires a value")?;
                max_new_tokens = value.to_string_lossy().parse()?;
            }
            "--prompt" => {
                let value = args.next().ok_or("--prompt requires a value")?;
                prompt = Some(value.to_string_lossy().into_owned());
            }
            "--tokenize-bin" => {
                let value = args.next().ok_or("--tokenize-bin requires a value")?;
                tokenize_bin = PathBuf::from(value);
            }
            "--upstream-completion-bin" => {
                let value = args
                    .next()
                    .ok_or("--upstream-completion-bin requires a value")?;
                upstream_completion_bin = PathBuf::from(value);
            }
            "--no-bos" => {
                no_bos = true;
            }
            "--dump-token-ids" => {
                dump_token_ids = true;
            }
            "--no-stream" => {
                no_stream = true;
            }
            "--verify-upstream" => {
                verify_upstream = true;
            }
            _ if model_path.is_none() => {
                model_path = Some(PathBuf::from(arg));
            }
            _ => {
                prompt_parts.push(arg.to_string_lossy().into_owned());
            }
        }
    }

    let model_path = model_path.ok_or_else(|| {
        print_usage();
        "usage: llama-generate <model.gguf> [--max-new-tokens N] [--prompt TEXT | prompt words ...]"
    })?;
    let prompt = prompt.unwrap_or_else(|| prompt_parts.join(" "));
    if prompt.is_empty() {
        return Err("missing prompt text".into());
    }

    Ok(Args {
        model_path,
        prompt,
        max_new_tokens,
        tokenize_bin,
        upstream_completion_bin,
        no_bos,
        dump_token_ids,
        no_stream,
        verify_upstream,
    })
}

fn print_usage() {
    eprintln!(
        "usage: llama-generate <model.gguf> [--max-new-tokens N] [--tokenize-bin PATH] [--upstream-completion-bin PATH] [--no-bos] [--dump-token-ids] [--no-stream] [--verify-upstream] [--prompt TEXT | prompt words ...]"
    );
}

fn tokenize_prompt(args: &Args) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
    let mut command = Command::new(&args.tokenize_bin);
    command
        .arg("-m")
        .arg(&args.model_path)
        .arg("--ids")
        .arg("--log-disable")
        .arg("-p")
        .arg(&args.prompt);
    if args.no_bos {
        command.arg("--no-bos");
    }

    let output = command.output()?;
    ensure_success("llama-tokenize", &output)?;
    parse_token_id_list(&String::from_utf8(output.stdout)?)
}

fn run_upstream_completion(args: &Args) -> Result<String, Box<dyn std::error::Error>> {
    let max_context = args
        .max_new_tokens
        .checked_add(tokenize_prompt(args)?.len())
        .ok_or("overflow computing upstream completion context")?;
    let mut command = Command::new(&args.upstream_completion_bin);
    command
        .arg("-m")
        .arg(&args.model_path)
        .arg("-p")
        .arg(&args.prompt)
        .arg("-n")
        .arg(args.max_new_tokens.to_string())
        .arg("-c")
        .arg(max_context.to_string())
        .arg("-no-cnv")
        .arg("--simple-io")
        .arg("--no-display-prompt")
        .arg("--no-warmup")
        .arg("--log-disable")
        .arg("--seed")
        .arg("0")
        .arg("--temp")
        .arg("0")
        .arg("--top-k")
        .arg("1")
        .arg("--top-p")
        .arg("1")
        .arg("--repeat-penalty")
        .arg("1")
        .arg("--presence-penalty")
        .arg("0")
        .arg("--frequency-penalty")
        .arg("0")
        .arg("--dry-multiplier")
        .arg("0")
        .arg("-fa")
        .arg("on")
        .arg("-ctk")
        .arg("f16")
        .arg("-ctv")
        .arg("f16");
    if args.no_bos {
        command
            .arg("--override-kv")
            .arg("tokenizer.ggml.add_bos_token=bool:false");
    }

    let output = command.output()?;
    ensure_success("llama-completion", &output)?;
    Ok(String::from_utf8(output.stdout)?)
}

fn ensure_success(name: &str, output: &Output) -> Result<(), Box<dyn std::error::Error>> {
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{name} exited with {}.\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .into())
}

fn parse_token_id_list(stdout: &str) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
    let line = stdout
        .lines()
        .rev()
        .find(|line| {
            let line = line.trim();
            line.starts_with('[') && line.ends_with(']')
        })
        .ok_or("tokenizer output did not contain an id list")?;
    let line = line.trim();
    let inner = line
        .strip_prefix('[')
        .and_then(|line| line.strip_suffix(']'))
        .ok_or("tokenizer id list was malformed")?
        .trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    inner
        .split(',')
        .map(|part| part.trim().parse::<i32>().map_err(Into::into))
        .collect()
}

fn verify_exact_output(
    rust_output: &str,
    upstream_output: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if rust_output == upstream_output {
        return Ok(());
    }

    let diff = first_diff_byte(rust_output.as_bytes(), upstream_output.as_bytes());
    let diff_message = match diff {
        Some((index, rust_byte, upstream_byte)) => format!(
            "first difference at byte {}: rust={:?} upstream={:?}",
            index, rust_byte, upstream_byte
        ),
        None => format!(
            "output length mismatch: rust={} upstream={}",
            rust_output.len(),
            upstream_output.len()
        ),
    };
    Err(format!(
        "exact upstream verification failed: {diff_message}\nrust.preview: {:?}\nupstream.preview: {:?}",
        preview_text(rust_output, 160),
        preview_text(upstream_output, 160)
    )
    .into())
}

fn first_diff_byte(lhs: &[u8], rhs: &[u8]) -> Option<(usize, Option<u8>, Option<u8>)> {
    let common_len = lhs.len().min(rhs.len());
    for index in 0..common_len {
        if lhs[index] != rhs[index] {
            return Some((index, Some(lhs[index]), Some(rhs[index])));
        }
    }
    if lhs.len() == rhs.len() {
        None
    } else {
        Some((
            common_len,
            lhs.get(common_len).copied(),
            rhs.get(common_len).copied(),
        ))
    }
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let mut preview = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

fn stop_reason_name(reason: LlamaStopReason) -> &'static str {
    match reason {
        LlamaStopReason::MaxNewTokens => "max_new_tokens",
        LlamaStopReason::EndOfSequence => "eos_token",
        LlamaStopReason::PaddingToken => "padding_token",
    }
}

fn tok_per_second(token_count: usize, seconds: f64) -> f64 {
    if seconds > 0.0 {
        token_count as f64 / seconds
    } else {
        0.0
    }
}
