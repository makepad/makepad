use makepad_mlx::multimodal::{load_gemma_image, GemmaVisionProfile, GemmaVisionRuntime};
use makepad_mlx::MlxIndexedSafetensors;
use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn usage() -> &'static str {
    "Usage: gemma_image_bench [model.safetensors|model_dir] [--warmup N] [--iters N] <image_path>"
}

fn avg_duration(total: Duration, count: usize) -> Duration {
    if count == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(total.as_secs_f64() / count as f64)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut model_path = default_model_path();
    let mut warmup_iters = 1usize;
    let mut measured_iters = 3usize;
    let mut image_path = None::<PathBuf>;

    while let Some(arg) = args.next() {
        match arg.as_str() {
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
                if image_path.is_none()
                    && (value.ends_with(".safetensors") || PathBuf::from(value).is_dir())
                {
                    model_path = PathBuf::from(value);
                } else if image_path.is_none() {
                    image_path = Some(PathBuf::from(value));
                } else {
                    return Err(usage().into());
                }
            }
        }
    }

    let image_path = image_path.ok_or_else(|| usage().to_string())?;
    if measured_iters == 0 {
        return Err("--iters must be > 0".into());
    }

    let load_started = Instant::now();
    let weights = MlxIndexedSafetensors::load(&model_path)?;
    let load_elapsed = load_started.elapsed();
    let mut vision = GemmaVisionRuntime::load(&weights);

    let mut last_image = None;
    let mut last_embedding_count = 0usize;
    let mut last_embedding_width = 0usize;
    let mut last_profile = GemmaVisionProfile::default();

    for _ in 0..warmup_iters {
        let image = load_gemma_image(&weights.snapshot, Path::new(&image_path))?;
        let (embeddings, profile) = vision.encode_image_to_text_embeddings_profiled(&image)?;
        last_embedding_count = embeddings.len();
        last_embedding_width = embeddings.first().map(|row| row.len()).unwrap_or(0);
        last_profile = profile;
        last_image = Some(image);
    }

    let mut preprocess_total = Duration::ZERO;
    let mut vision_total = Duration::ZERO;
    let mut total_total = Duration::ZERO;
    for _ in 0..measured_iters {
        let total_started = Instant::now();
        let preprocess_started = Instant::now();
        let image = load_gemma_image(&weights.snapshot, Path::new(&image_path))?;
        let preprocess_elapsed = preprocess_started.elapsed();
        let vision_started = Instant::now();
        let (embeddings, profile) = vision.encode_image_to_text_embeddings_profiled(&image)?;
        let vision_elapsed = vision_started.elapsed();
        let total_elapsed = total_started.elapsed();

        preprocess_total += preprocess_elapsed;
        vision_total += vision_elapsed;
        total_total += total_elapsed;

        last_embedding_count = embeddings.len();
        last_embedding_width = embeddings.first().map(|row| row.len()).unwrap_or(0);
        last_profile = profile;
        last_image = Some(image);
    }

    let last_image = last_image.ok_or("image benchmark produced no image state")?;
    println!("model={}", model_path.display());
    println!("image={}", image_path.display());
    println!("warmup_iters={}", warmup_iters);
    println!("measured_iters={}", measured_iters);
    println!("load_s={:.6}", load_elapsed.as_secs_f64());
    println!("processed_width={}", last_image.width);
    println!("processed_height={}", last_image.height);
    println!("patch_grid_width={}", last_image.patch_grid_width);
    println!("patch_grid_height={}", last_image.patch_grid_height);
    println!("soft_token_count={}", last_image.soft_token_count);
    println!("embedding_rows={}", last_embedding_count);
    println!("embedding_width={}", last_embedding_width);
    println!(
        "preprocess_s_avg={:.6}",
        avg_duration(preprocess_total, measured_iters).as_secs_f64()
    );
    println!(
        "vision_encode_s_avg={:.6}",
        avg_duration(vision_total, measured_iters).as_secs_f64()
    );
    println!(
        "total_image_pipeline_s_avg={:.6}",
        avg_duration(total_total, measured_iters).as_secs_f64()
    );
    println!(
        "profile_patch_embed_s={:.6}",
        last_profile.patch_embed.as_secs_f64()
    );
    println!(
        "profile_layers_total_s={:.6}",
        last_profile.layers_total.as_secs_f64()
    );
    println!("profile_pool_s={:.6}", last_profile.pool.as_secs_f64());
    println!(
        "profile_standardize_s={:.6}",
        last_profile.standardize.as_secs_f64()
    );
    println!(
        "profile_project_s={:.6}",
        last_profile.project.as_secs_f64()
    );
    println!(
        "profile_layer_input_norm_s={:.6}",
        last_profile.layer_input_norm.as_secs_f64()
    );
    println!(
        "profile_layer_qkv_proj_s={:.6}",
        last_profile.layer_qkv_proj.as_secs_f64()
    );
    println!(
        "profile_layer_qk_norm_v_norm_s={:.6}",
        last_profile.layer_qk_norm_v_norm.as_secs_f64()
    );
    println!(
        "profile_layer_rope_s={:.6}",
        last_profile.layer_rope.as_secs_f64()
    );
    println!(
        "profile_layer_attention_s={:.6}",
        last_profile.layer_attention.as_secs_f64()
    );
    println!(
        "profile_layer_o_proj_post_norm_residual_s={:.6}",
        last_profile.layer_o_proj_post_norm_residual.as_secs_f64()
    );
    println!(
        "profile_layer_pre_ffn_norm_s={:.6}",
        last_profile.layer_pre_ffn_norm.as_secs_f64()
    );
    println!(
        "profile_layer_gate_up_proj_s={:.6}",
        last_profile.layer_gate_up_proj.as_secs_f64()
    );
    println!(
        "profile_layer_geglu_s={:.6}",
        last_profile.layer_geglu.as_secs_f64()
    );
    println!(
        "profile_layer_down_proj_post_norm_residual_s={:.6}",
        last_profile
            .layer_down_proj_post_norm_residual
            .as_secs_f64()
    );
    println!(
        "profile_layer_fused_mlp_s={:.6}",
        last_profile.layer_fused_mlp.as_secs_f64()
    );
    println!(
        "profile_flash_attn_successes={}",
        last_profile.flash_attn_successes
    );
    println!(
        "profile_flash_attn_fallbacks={}",
        last_profile.flash_attn_fallbacks
    );

    Ok(())
}
