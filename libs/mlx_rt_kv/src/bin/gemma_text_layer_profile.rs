use makepad_mlx_rt_kv::layer0_cached_case::profile_decode_layers_after_prompt_token_ids;
use std::env;
use std::path::PathBuf;

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn usage() -> &'static str {
    "Usage: gemma_text_layer_profile [model.safetensors] --token-ids ID,ID,..."
}

fn parse_token_ids(text: &str) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    text.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| Ok(part.parse::<u32>()?))
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut model_path = default_model_path();
    let mut token_ids = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--token-ids" => {
                let value = args.next().ok_or("--token-ids requires a value")?;
                token_ids = Some(parse_token_ids(&value)?);
            }
            value if value.starts_with("--") => {
                return Err(format!("unknown option: {value}\n{}", usage()).into());
            }
            value => {
                if value.ends_with(".safetensors") || PathBuf::from(value).is_dir() {
                    model_path = PathBuf::from(value);
                } else {
                    return Err(format!("unexpected argument: {value}\n{}", usage()).into());
                }
            }
        }
    }

    let token_ids = token_ids.ok_or_else(|| usage().to_string())?;
    let profile = profile_decode_layers_after_prompt_token_ids(model_path, &token_ids)?;

    println!("profile_prompt_token_count={}", profile.prompt_token_count);
    println!(
        "profile_first_generated_token_id={}",
        profile.first_generated_token_id
    );
    println!("profiled_token_id={}", profile.profiled_token_id);
    println!("profiled_position={}", profile.profiled_position);
    println!(
        "embed_profile elapsed_ms={:.3} compute_dispatches={} buffer_barriers={} command_buffer_commits={} gpu_elapsed_ms={:.3}",
        profile.embed.elapsed.as_secs_f64() * 1000.0,
        profile.embed.counters.compute_dispatches,
        profile.embed.counters.buffer_barriers,
        profile.embed.counters.command_buffer_commits,
        profile.embed.counters.gpu_elapsed_ns as f64 / 1e6,
    );
    for layer in &profile.layers {
        println!(
            "layer_profile layer_idx={} layer_type={} elapsed_ms={:.3} compute_dispatches={} buffer_barriers={} command_buffer_commits={} gpu_elapsed_ms={:.3}",
            layer.layer_idx,
            layer.attention.as_str(),
            layer.elapsed.as_secs_f64() * 1000.0,
            layer.counters.compute_dispatches,
            layer.counters.buffer_barriers,
            layer.counters.command_buffer_commits,
            layer.counters.gpu_elapsed_ns as f64 / 1e6,
        );
    }
    println!(
        "head_profile elapsed_ms={:.3} compute_dispatches={} buffer_barriers={} command_buffer_commits={} gpu_elapsed_ms={:.3}",
        profile.head.elapsed.as_secs_f64() * 1000.0,
        profile.head.counters.compute_dispatches,
        profile.head.counters.buffer_barriers,
        profile.head.counters.command_buffer_commits,
        profile.head.counters.gpu_elapsed_ns as f64 / 1e6,
    );
    for stage in &profile.head_stages {
        println!(
            "head_stage_profile stage_name={} elapsed_ms={:.3} compute_dispatches={} buffer_barriers={} command_buffer_commits={} gpu_elapsed_ms={:.3}",
            stage.stage_name,
            stage.elapsed.as_secs_f64() * 1000.0,
            stage.counters.compute_dispatches,
            stage.counters.buffer_barriers,
            stage.counters.command_buffer_commits,
            stage.counters.gpu_elapsed_ns as f64 / 1e6,
        );
    }
    println!("profile_predicted_token_id={}", profile.predicted_token_id);
    Ok(())
}
