use makepad_mlx::text_runtime::{
    benchmark_text_generation_with_backend_config, GemmaPromptFormat, GemmaTextBackendConfig,
    GemmaTextBackendMode, GemmaTextGenerationOptions, GemmaTextKvCompressionMode,
};
use std::env;
use std::path::PathBuf;

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn usage() -> &'static str {
    "Usage: gemma_text_bench [model.safetensors] [--raw-bos] [--greedy] [--max-new-tokens N] [--warmup N] [--iters N] [--reference-text-backend] [--force-exact-text-backend] [--rotor-k-cache] [--rotor-k-cache-planar3] <prompt>"
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut model_path = default_model_path();
    let mut prompt_format = GemmaPromptFormat::AutoChat;
    let mut greedy = false;
    let mut max_new_tokens = 64usize;
    let mut warmup_iters = 1usize;
    let mut measured_iters = 3usize;
    let mut backend_config = GemmaTextBackendConfig::default();
    let mut prompt_parts = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--raw-bos" => {
                prompt_format = GemmaPromptFormat::RawBos;
            }
            "--greedy" => {
                greedy = true;
            }
            "--max-new-tokens" => {
                let value = args.next().ok_or("--max-new-tokens requires a value")?;
                max_new_tokens = value.parse::<usize>()?;
            }
            "--warmup" => {
                let value = args.next().ok_or("--warmup requires a value")?;
                warmup_iters = value.parse::<usize>()?;
            }
            "--iters" => {
                let value = args.next().ok_or("--iters requires a value")?;
                measured_iters = value.parse::<usize>()?;
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
    let output = benchmark_text_generation_with_backend_config(
        model_path,
        prompt,
        GemmaTextGenerationOptions {
            max_new_tokens,
            prompt_format,
        },
        greedy,
        warmup_iters,
        measured_iters,
        backend_config,
    )?;

    println!("prompt_ids={:?}", output.prompt_token_ids);
    println!("prompt_token_count={}", output.prompt_token_ids.len());
    println!("last_generated_ids={:?}", output.last_generated_token_ids);
    println!("last_generated_text={:?}", output.last_generated_text);
    println!("warmup_iters={}", output.warmup_iters);
    println!("measured_iters={}", output.measured_iters);
    println!("max_new_tokens={}", output.max_new_tokens);
    println!("load_s={:.6}", output.load_duration.as_secs_f64());
    println!("elapsed_s={:.6}", output.elapsed.as_secs_f64());
    println!("total_generated_tokens={}", output.total_generated_tokens);
    println!(
        "ttft_s={:.6}",
        output.time_to_first_token_elapsed.as_secs_f64()
    );
    println!(
        "ttft_ms_avg={:.3}",
        output.time_to_first_token_elapsed.as_secs_f64() * 1000.0 / output.measured_iters as f64
    );
    println!(
        "prompt_prefill_tok_s={:.3}",
        output.prompt_prefill_tokens_per_second
    );
    println!(
        "steady_elapsed_s={:.6}",
        output.steady_state_elapsed.as_secs_f64()
    );
    println!(
        "steady_generated_tokens={}",
        output.steady_state_generated_tokens
    );
    println!(
        "steady_decode_tok_s={:.3}",
        output.steady_state_decode_tokens_per_second
    );
    println!("backend={}", output.backend_kind.label());
    if let Some(counters) = output.backend_counters.as_metal() {
        println!("backend_command_batches={}", counters.command_batches_begun);
        println!(
            "backend_batch_commits={}",
            counters.command_batches_committed
        );
        println!(
            "backend_command_buffer_commits={}",
            counters.command_buffer_commits
        );
        println!("backend_compute_dispatches={}", counters.compute_dispatches);
        println!("backend_buffer_barriers={}", counters.buffer_barriers);
        println!("backend_encoder_starts={}", counters.compute_encoder_starts);
        println!("backend_encoder_ends={}", counters.compute_encoder_ends);
        println!("backend_blit_copies={}", counters.blit_copy_calls);
        println!("backend_fence_waits={}", counters.fence_waits);
        println!("backend_fence_updates={}", counters.fence_updates);
        println!("backend_wait_idle_calls={}", counters.wait_idle_calls);
        println!(
            "backend_completion_wait_calls={}",
            counters.completion_wait_calls
        );
        println!("backend_readbacks={}", counters.readback_calls);
        println!(
            "backend_gpu_elapsed_s={:.6}",
            counters.gpu_elapsed_ns as f64 / 1e9
        );
        println!(
            "backend_cpu_gap_s={:.6}",
            (output.elapsed.as_secs_f64() - counters.gpu_elapsed_ns as f64 / 1e9).max(0.0)
        );
    }
    println!("decode_tok_s={:.3}", output.decode_tokens_per_second);
    println!("total_tok_s={:.3}", output.total_tokens_per_second);

    Ok(())
}
