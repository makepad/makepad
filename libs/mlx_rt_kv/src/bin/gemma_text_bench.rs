use makepad_mlx_rt_kv::text_runtime::{
    benchmark_text_generation, GemmaPromptFormat, GemmaTextGenerationOptions,
};
use std::env;
use std::path::PathBuf;

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn usage() -> &'static str {
    "Usage: gemma_text_bench [model.safetensors] [--raw-bos] [--max-new-tokens N] [--warmup N] [--iters N] <prompt>"
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut model_path = default_model_path();
    let mut prompt_format = GemmaPromptFormat::Gemma4UserTurn;
    let mut max_new_tokens = 64usize;
    let mut warmup_iters = 1usize;
    let mut measured_iters = 3usize;
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
            "--warmup" => {
                let value = args.next().ok_or("--warmup requires a value")?;
                warmup_iters = value.parse::<usize>()?;
            }
            "--iters" => {
                let value = args.next().ok_or("--iters requires a value")?;
                measured_iters = value.parse::<usize>()?;
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
    let output = benchmark_text_generation(
        model_path,
        prompt,
        GemmaTextGenerationOptions {
            max_new_tokens,
            prompt_format,
        },
        warmup_iters,
        measured_iters,
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
    println!(
        "metal_command_batches={}",
        output.metal_counters.command_batches_begun
    );
    println!(
        "metal_batch_commits={}",
        output.metal_counters.command_batches_committed
    );
    println!(
        "metal_command_buffer_commits={}",
        output.metal_counters.command_buffer_commits
    );
    println!(
        "metal_compute_dispatches={}",
        output.metal_counters.compute_dispatches
    );
    println!(
        "metal_buffer_barriers={}",
        output.metal_counters.buffer_barriers
    );
    println!(
        "metal_encoder_starts={}",
        output.metal_counters.compute_encoder_starts
    );
    println!(
        "metal_encoder_ends={}",
        output.metal_counters.compute_encoder_ends
    );
    println!(
        "metal_blit_copies={}",
        output.metal_counters.blit_copy_calls
    );
    println!("metal_fence_waits={}", output.metal_counters.fence_waits);
    println!(
        "metal_fence_updates={}",
        output.metal_counters.fence_updates
    );
    println!(
        "metal_wait_idle_calls={}",
        output.metal_counters.wait_idle_calls
    );
    println!(
        "metal_completion_wait_calls={}",
        output.metal_counters.completion_wait_calls
    );
    println!("metal_readbacks={}", output.metal_counters.readback_calls);
    println!(
        "metal_gpu_elapsed_s={:.6}",
        output.metal_counters.gpu_elapsed_ns as f64 / 1e9
    );
    println!(
        "metal_cpu_gap_s={:.6}",
        (output.elapsed.as_secs_f64() - output.metal_counters.gpu_elapsed_ns as f64 / 1e9).max(0.0)
    );
    println!("decode_tok_s={:.3}", output.decode_tokens_per_second);
    println!("total_tok_s={:.3}", output.total_tokens_per_second);

    Ok(())
}
