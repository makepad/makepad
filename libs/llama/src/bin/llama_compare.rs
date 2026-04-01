use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use makepad_ggml::backend::metal::{
    prepare_graph, BufferStorageMode, MetalGraphSession, MetalGraphTensorWrite, MetalRuntime,
};
use makepad_ggml::{
    f32_to_f16, Context, TensorId, TensorType, GGML_ROPE_TYPE_IMROPE, GGML_ROPE_TYPE_MROPE,
};
use makepad_llama::{
    build_delta_net_recurrent_decode_graph, compile_attention_block_metal,
    compile_attention_decode_metal, compile_delta_net_recurrent_decode_metal,
    compile_hybrid_decode_metal, execute_attention_block_graph_metal_cached,
    execute_attention_decode_graph_metal_cached,
    execute_delta_net_recurrent_decode_graph_metal_cached,
    execute_hybrid_decode_graph_metal_cached, prepare_attention_block_graph,
    prepare_attention_decode_graph, qwen35moe_attention_block_layout,
    qwen35moe_attention_decode_spec, qwen35moe_delta_net_recurrent_decode_spec,
    qwen35moe_first_attention_block_spec, qwen35moe_first_recurrent_block_spec,
    qwen35moe_hybrid_decode_spec, qwen35moe_recurrent_block_layout, AttentionRopeSpec,
    HybridCacheLayout, HybridCacheShape, HybridCacheTypes, LlamaModel, LlamaVocab,
    LogitsProbeInput,
};

const DEFAULT_PROMPT: &str = "The capital of France is";
const DEFAULT_TOP_K: usize = 10;
const DEFAULT_UPSTREAM_DEBUG: &str =
    "local/llama.cpp/build-arm64-apple-clang-release/bin/llama-debug";

struct Args {
    model_path: PathBuf,
    prompt: String,
    upstream_debug_path: PathBuf,
    top_k: usize,
}

struct UpstreamReference {
    token_ids: Vec<i32>,
    logits: Vec<f32>,
    output_dir: PathBuf,
}

fn main() {
    match run() {
        Ok(()) => {}
        Err(err) => {
            eprintln!("llama-compare failed: {err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args(std::env::args_os())?;
    let model = LlamaModel::load(&args.model_path)?;
    model.validate_layout()?;
    let vocab = LlamaVocab::from_model(&model).ok();
    let upstream = run_upstream_debug(&args)?;

    let input_token_labels = format_token_list(&upstream.token_ids, vocab.as_ref());
    println!("model: {}", args.model_path.display());
    println!("prompt: {}", args.prompt);
    println!("input.token_count: {}", upstream.token_ids.len());
    println!("input.tokens: {:?}", upstream.token_ids);
    println!("input.token_pieces: {:?}", input_token_labels);
    println!("upstream.output_dir: {}", upstream.output_dir.display());

    let rust_logits = run_rust_hybrid_decode(&model, &upstream.token_ids)?;
    let rust_batched_logits = match run_rust_hybrid_decode_batched(&model, &upstream.token_ids) {
        Ok(logits) => Some(logits),
        Err(err) => {
            println!("compare.batched.error: {}", err);
            None
        }
    };
    if rust_logits.len() != upstream.logits.len() {
        return Err(format!(
            "logit length mismatch: rust={} upstream={}",
            rust_logits.len(),
            upstream.logits.len()
        )
        .into());
    }
    let rust_top = top_k_logits(&rust_logits, args.top_k);
    let upstream_top = top_k_logits(&upstream.logits, args.top_k);
    let stats = compare_logits(&rust_logits, &upstream.logits);
    let rust_top_ids = rust_top.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let upstream_top_ids = upstream_top.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let top_overlap = rust_top_ids
        .iter()
        .filter(|id| upstream_top_ids.contains(id))
        .count();

    println!(
        "compare.same_top1: {}",
        rust_top.first().map(|(id, _)| *id) == upstream_top.first().map(|(id, _)| *id)
    );
    println!(
        "compare.same_top{}_ids: {}",
        args.top_k,
        rust_top_ids == upstream_top_ids
    );
    println!(
        "compare.top{}_overlap: {}/{}",
        args.top_k, top_overlap, args.top_k
    );
    println!("compare.max_abs_diff: {:.9}", stats.max_abs_diff);
    println!("compare.mean_abs_diff: {:.9}", stats.mean_abs_diff);
    println!("compare.rms_diff: {:.9}", stats.rms_diff);
    println!("compare.cosine_similarity: {:.9}", stats.cosine_similarity);
    if let Some(rust_batched_logits) = &rust_batched_logits {
        if rust_batched_logits.len() != upstream.logits.len() {
            return Err(format!(
                "batched logit length mismatch: rust={} upstream={}",
                rust_batched_logits.len(),
                upstream.logits.len()
            )
            .into());
        }
        let rust_batched_top = top_k_logits(rust_batched_logits, args.top_k);
        let batched_stats = compare_logits(rust_batched_logits, &upstream.logits);
        let rust_batched_top_ids = rust_batched_top
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        let batched_top_overlap = rust_batched_top_ids
            .iter()
            .filter(|id| upstream_top_ids.contains(id))
            .count();
        println!(
            "compare.batched.same_top1: {}",
            rust_batched_top.first().map(|(id, _)| *id) == upstream_top.first().map(|(id, _)| *id)
        );
        println!(
            "compare.batched.same_top{}_ids: {}",
            args.top_k,
            rust_batched_top_ids == upstream_top_ids
        );
        println!(
            "compare.batched.top{}_overlap: {}/{}",
            args.top_k, batched_top_overlap, args.top_k
        );
        println!(
            "compare.batched.max_abs_diff: {:.9}",
            batched_stats.max_abs_diff
        );
        println!(
            "compare.batched.mean_abs_diff: {:.9}",
            batched_stats.mean_abs_diff
        );
        println!("compare.batched.rms_diff: {:.9}", batched_stats.rms_diff);
        println!(
            "compare.batched.cosine_similarity: {:.9}",
            batched_stats.cosine_similarity
        );
        println!(
            "rust_batched.next.top{}: {:?}",
            args.top_k,
            describe_top_k(&rust_batched_top, vocab.as_ref())
        );
    }

    println!(
        "upstream.next.top{}: {:?}",
        args.top_k,
        describe_top_k(&upstream_top, vocab.as_ref())
    );
    println!(
        "rust.next.top{}: {:?}",
        args.top_k,
        describe_top_k(&rust_top, vocab.as_ref())
    );
    if upstream.token_ids.len() > 1 {
        let attention_check_f16 =
            attention_cache_self_check(&model, &upstream.token_ids[..2], TensorType::F16)?;
        let attention_check_f32 =
            attention_cache_self_check(&model, &upstream.token_ids[..2], TensorType::F32)?;
        let attention_decode_batch_check =
            attention_decode_batch_self_check(&model, &upstream.token_ids[..2])?;
        let recurrent_check = recurrent_cache_self_check(&model, &upstream.token_ids[..2])?;
        let recurrent_step_cpu_check = recurrent_step_cpu_check(&model, &upstream.token_ids[..2])?;
        println!(
            "attention_cache.f16.layer{}._same_top1: {}",
            attention_check_f16.layer_index, attention_check_f16.same_top1
        );
        println!(
            "attention_cache.f16.layer{}._hidden_max_abs_diff: {:.9}",
            attention_check_f16.layer_index, attention_check_f16.hidden_stats.max_abs_diff
        );
        println!(
            "attention_cache.f16.layer{}._hidden_mean_abs_diff: {:.9}",
            attention_check_f16.layer_index, attention_check_f16.hidden_stats.mean_abs_diff
        );
        println!(
            "attention_cache.f16.layer{}._hidden_rms_diff: {:.9}",
            attention_check_f16.layer_index, attention_check_f16.hidden_stats.rms_diff
        );
        println!(
            "attention_cache.f16.layer{}._hidden_cosine_similarity: {:.9}",
            attention_check_f16.layer_index, attention_check_f16.hidden_stats.cosine_similarity
        );
        println!(
            "attention_cache.f32.layer{}._same_top1: {}",
            attention_check_f32.layer_index, attention_check_f32.same_top1
        );
        println!(
            "attention_cache.f32.layer{}._hidden_max_abs_diff: {:.9}",
            attention_check_f32.layer_index, attention_check_f32.hidden_stats.max_abs_diff
        );
        println!(
            "attention_cache.f32.layer{}._hidden_mean_abs_diff: {:.9}",
            attention_check_f32.layer_index, attention_check_f32.hidden_stats.mean_abs_diff
        );
        println!(
            "attention_cache.f32.layer{}._hidden_rms_diff: {:.9}",
            attention_check_f32.layer_index, attention_check_f32.hidden_stats.rms_diff
        );
        println!(
            "attention_cache.f32.layer{}._hidden_cosine_similarity: {:.9}",
            attention_check_f32.layer_index, attention_check_f32.hidden_stats.cosine_similarity
        );
        println!(
            "attention_decode_batch.layer{}._hidden_max_abs_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check.hidden_stats.max_abs_diff
        );
        println!(
            "attention_decode_batch.layer{}._hidden_mean_abs_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check.hidden_stats.mean_abs_diff
        );
        println!(
            "attention_decode_batch.layer{}._hidden_rms_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check.hidden_stats.rms_diff
        );
        println!(
            "attention_decode_batch.layer{}._hidden_cosine_similarity: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check.hidden_stats.cosine_similarity
        );
        println!(
            "attention_decode_batch.layer{}._k_cache_max_abs_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check.k_cache_stats.max_abs_diff
        );
        println!(
            "attention_decode_batch.layer{}._v_cache_max_abs_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check.v_cache_stats.max_abs_diff
        );
        println!(
            "recurrent_cache.layer{}._same_top1: {}",
            recurrent_check.layer_index, recurrent_check.same_top1
        );
        println!(
            "recurrent_cache.layer{}._hidden_max_abs_diff: {:.9}",
            recurrent_check.layer_index, recurrent_check.hidden_stats.max_abs_diff
        );
        println!(
            "recurrent_cache.layer{}._hidden_mean_abs_diff: {:.9}",
            recurrent_check.layer_index, recurrent_check.hidden_stats.mean_abs_diff
        );
        println!(
            "recurrent_cache.layer{}._hidden_rms_diff: {:.9}",
            recurrent_check.layer_index, recurrent_check.hidden_stats.rms_diff
        );
        println!(
            "recurrent_cache.layer{}._hidden_cosine_similarity: {:.9}",
            recurrent_check.layer_index, recurrent_check.hidden_stats.cosine_similarity
        );
        println!(
            "recurrent_cache.layer{}._r_cache_max_abs_diff: {:.9}",
            recurrent_check.layer_index, recurrent_check.r_cache_stats.max_abs_diff
        );
        println!(
            "recurrent_cache.layer{}._s_cache_max_abs_diff: {:.9}",
            recurrent_check.layer_index, recurrent_check.s_cache_stats.max_abs_diff
        );
        println!(
            "recurrent_step_cpu.layer{}._conv_output_max_abs_diff: {:.9}",
            recurrent_step_cpu_check.layer_index,
            recurrent_step_cpu_check.conv_output_cpu_stats.max_abs_diff
        );
        println!(
            "recurrent_step_cpu.layer{}._q_conv_max_abs_diff: {:.9}",
            recurrent_step_cpu_check.layer_index,
            recurrent_step_cpu_check.q_conv_cpu_stats.max_abs_diff
        );
        println!(
            "recurrent_step_cpu.layer{}._k_conv_max_abs_diff: {:.9}",
            recurrent_step_cpu_check.layer_index,
            recurrent_step_cpu_check.k_conv_cpu_stats.max_abs_diff
        );
        println!(
            "recurrent_step_cpu.layer{}._output_view_max_abs_diff: {:.9}",
            recurrent_step_cpu_check.layer_index,
            recurrent_step_cpu_check.output_view_cpu_stats.max_abs_diff
        );
        let recurrent_tensor_check = recurrent_tensor_check(&model, &upstream.token_ids[..2])?;
        println!(
            "recurrent_tensor.layer{}._z_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index, recurrent_tensor_check.z_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._beta_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index, recurrent_tensor_check.beta_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._gate_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index, recurrent_tensor_check.gate_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._conv_output_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index,
            recurrent_tensor_check.conv_output_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._output_view_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index,
            recurrent_tensor_check.output_view_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._output_norm_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index,
            recurrent_tensor_check.output_norm_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._z_silu_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index, recurrent_tensor_check.z_silu_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._gated_output_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index,
            recurrent_tensor_check.gated_output_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._final_output_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index,
            recurrent_tensor_check.final_output_stats.max_abs_diff
        );
        let attention_tensor_check =
            attention_cache_tensor_check(&model, &upstream.token_ids[..2])?;
        println!(
            "attention_tensor.layer{}._q_proj_max_abs_diff: {:.9}",
            attention_tensor_check.layer_index, attention_tensor_check.q_proj_stats.max_abs_diff
        );
        println!(
            "attention_tensor.layer{}._q_pre_max_abs_diff: {:.9}",
            attention_tensor_check.layer_index, attention_tensor_check.q_pre_stats.max_abs_diff
        );
        println!(
            "attention_tensor.layer{}._q_norm_max_abs_diff: {:.9}",
            attention_tensor_check.layer_index, attention_tensor_check.q_norm_stats.max_abs_diff
        );
        println!(
            "attention_tensor.layer{}._k_norm_max_abs_diff: {:.9}",
            attention_tensor_check.layer_index, attention_tensor_check.k_norm_stats.max_abs_diff
        );
        println!(
            "attention_tensor.layer{}._q_max_abs_diff: {:.9}",
            attention_tensor_check.layer_index, attention_tensor_check.q_stats.max_abs_diff
        );
        println!(
            "attention_tensor.layer{}._k_store_max_abs_diff: {:.9}",
            attention_tensor_check.layer_index, attention_tensor_check.k_store_stats.max_abs_diff
        );
        println!(
            "attention_tensor.layer{}._v_store_max_abs_diff: {:.9}",
            attention_tensor_check.layer_index, attention_tensor_check.v_store_stats.max_abs_diff
        );
        println!(
            "attention_tensor.layer{}._k_cache_max_abs_diff: {:.9}",
            attention_tensor_check.layer_index, attention_tensor_check.k_cache_stats.max_abs_diff
        );
        println!(
            "attention_tensor.layer{}._v_cache_max_abs_diff: {:.9}",
            attention_tensor_check.layer_index, attention_tensor_check.v_cache_stats.max_abs_diff
        );
        println!(
            "attention_tensor.layer{}._attn_max_abs_diff: {:.9}",
            attention_tensor_check.layer_index, attention_tensor_check.attn_stats.max_abs_diff
        );
        println!(
            "attention_tensor.layer{}._full_attn_cpu_max_abs_diff: {:.9}",
            attention_tensor_check.layer_index,
            attention_tensor_check.full_attn_cpu_stats.max_abs_diff
        );
        println!(
            "attention_tensor.layer{}._decode_attn_cpu_max_abs_diff: {:.9}",
            attention_tensor_check.layer_index,
            attention_tensor_check.decode_attn_cpu_stats.max_abs_diff
        );
    }

    if let (Some((rust_top1, rust_logit)), Some((upstream_top1, upstream_logit))) =
        (rust_top.first(), upstream_top.first())
    {
        println!("compare.top1.rust_id: {}", rust_top1);
        println!("compare.top1.upstream_id: {}", upstream_top1);
        println!("compare.top1.rust_logit: {:.9}", rust_logit);
        println!("compare.top1.upstream_logit: {:.9}", upstream_logit);
        if let Some(vocab) = &vocab {
            println!(
                "compare.top1.rust_piece: {}",
                vocab
                    .escaped_piece(*rust_top1)
                    .unwrap_or_else(|| "<unknown>".to_owned())
            );
            println!(
                "compare.top1.upstream_piece: {}",
                vocab
                    .escaped_piece(*upstream_top1)
                    .unwrap_or_else(|| "<unknown>".to_owned())
            );
        }
    }

    Ok(())
}

fn parse_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Args, Box<dyn std::error::Error>> {
    let mut args = args.into_iter();
    let _exe = args.next();

    let model_path = args
        .next()
        .ok_or("usage: llama-compare <model.gguf> [prompt words ...]")?;
    let prompt_parts = args.collect::<Vec<_>>();
    let prompt = if prompt_parts.is_empty() {
        DEFAULT_PROMPT.to_owned()
    } else {
        prompt_parts
            .iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    };

    Ok(Args {
        model_path: PathBuf::from(model_path),
        prompt,
        upstream_debug_path: PathBuf::from(DEFAULT_UPSTREAM_DEBUG),
        top_k: DEFAULT_TOP_K,
    })
}

fn run_upstream_debug(args: &Args) -> Result<UpstreamReference, Box<dyn std::error::Error>> {
    let output_dir = std::env::temp_dir().join(format!(
        "makepad-llama-compare-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
    ));
    fs::create_dir_all(&output_dir)?;

    let output = Command::new(&args.upstream_debug_path)
        .arg("-m")
        .arg(&args.model_path)
        .arg("-p")
        .arg(&args.prompt)
        .arg("--save-logits")
        .arg("--logits-output-dir")
        .arg(&output_dir)
        .arg("-ngl")
        .arg("999")
        .arg("-fa")
        .arg("1")
        .arg("-ctk")
        .arg("f16")
        .arg("-ctv")
        .arg("f16")
        .output()?;
    ensure_success("llama-debug", &output)?;

    let model_name = args
        .model_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or("failed to derive model stem for upstream output files")?;
    let base = output_dir.join(format!("llamacpp-{model_name}"));
    let logits = read_f32_file(&base.with_extension("bin"))?;
    let token_ids =
        read_i32_file(&base.with_file_name(format!("llamacpp-{model_name}-tokens.bin")))?;

    Ok(UpstreamReference {
        token_ids,
        logits,
        output_dir,
    })
}

fn run_rust_hybrid_decode(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("upstream prompt produced no tokens".into());
    }

    let plan = model.execution_plan()?;
    let cache_shape = HybridCacheShape {
        n_ctx_seq: u32::try_from(token_ids.len())?,
        n_seq_max: 1,
    };
    let cache_types = HybridCacheTypes {
        attention_k_type: TensorType::F16,
        attention_v_type: TensorType::F16,
        recurrent_r_type: TensorType::F32,
        recurrent_s_type: TensorType::F32,
    };
    let extra_bytes = plan
        .hybrid_cache
        .as_ref()
        .map(|template| HybridCacheLayout::new(template.materialize(cache_shape, cache_types)))
        .transpose()?
        .map_or(0, |layout| layout.total_bytes);
    let extra_bytes = extra_bytes.saturating_add(256 << 20);
    let mut loaded = plan
        .full_weights
        .allocate_and_load_with_extra(&model.gguf, extra_bytes)?;
    let spec = qwen35moe_hybrid_decode_spec(
        model,
        cache_shape.n_ctx_seq,
        cache_shape.n_seq_max,
        cache_types.attention_k_type,
        cache_types.attention_v_type,
        cache_types.recurrent_r_type,
        cache_types.recurrent_s_type,
    )?;
    let compiled = compile_hybrid_decode_metal(&mut loaded, &spec, 1)?;

    let mut final_logits = None;
    for (position, token_id) in token_ids.iter().copied().enumerate() {
        let position_i32 = i32::try_from(position)?;
        let run = execute_hybrid_decode_graph_metal_cached(
            &compiled,
            &mut loaded,
            LogitsProbeInput::TokenIds(std::slice::from_ref(&token_id)),
            &[position_i32],
            position + 1,
        )?;
        final_logits = Some(run.logits);
    }

    final_logits.ok_or_else(|| "hybrid decode did not produce logits".into())
}

fn run_rust_hybrid_decode_batched(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("upstream prompt produced no tokens".into());
    }

    let plan = model.execution_plan()?;
    let cache_shape = HybridCacheShape {
        n_ctx_seq: u32::try_from(token_ids.len())?,
        n_seq_max: 1,
    };
    let cache_types = HybridCacheTypes {
        attention_k_type: TensorType::F16,
        attention_v_type: TensorType::F16,
        recurrent_r_type: TensorType::F32,
        recurrent_s_type: TensorType::F32,
    };
    let extra_bytes = plan
        .hybrid_cache
        .as_ref()
        .map(|template| HybridCacheLayout::new(template.materialize(cache_shape, cache_types)))
        .transpose()?
        .map_or(0, |layout| layout.total_bytes);
    let extra_bytes = extra_bytes.saturating_add(256 << 20);
    let mut loaded = plan
        .full_weights
        .allocate_and_load_with_extra(&model.gguf, extra_bytes)?;
    let spec = qwen35moe_hybrid_decode_spec(
        model,
        cache_shape.n_ctx_seq,
        cache_shape.n_seq_max,
        cache_types.attention_k_type,
        cache_types.attention_v_type,
        cache_types.recurrent_r_type,
        cache_types.recurrent_s_type,
    )?;
    let compiled = compile_hybrid_decode_metal(&mut loaded, &spec, token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let run = execute_hybrid_decode_graph_metal_cached(
        &compiled,
        &mut loaded,
        LogitsProbeInput::TokenIds(token_ids),
        &positions,
        token_ids.len(),
    )?;
    if run.vocab_size == 0 || run.logits.len() % run.vocab_size != 0 {
        return Err(format!(
            "batched hybrid decode produced malformed logits: len={} vocab_size={}",
            run.logits.len(),
            run.vocab_size
        )
        .into());
    }
    let last = run
        .logits
        .len()
        .checked_sub(run.vocab_size)
        .ok_or("batched hybrid decode logits were unexpectedly empty")?;
    Ok(run.logits[last..].to_vec())
}

fn ensure_success(name: &str, output: &Output) -> Result<(), Box<dyn std::error::Error>> {
    if output.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "{name} exited with {}.\nstdout:\n{}\nstderr:\n{}",
        output.status, stdout, stderr
    )
    .into())
}

fn read_f32_file(path: &Path) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    if bytes.len() % std::mem::size_of::<f32>() != 0 {
        return Err(format!("file '{}' is not a multiple of f32 size", path.display()).into());
    }
    Ok(bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect())
}

fn read_i32_file(path: &Path) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    if bytes.len() % std::mem::size_of::<i32>() != 0 {
        return Err(format!("file '{}' is not a multiple of i32 size", path.display()).into());
    }
    Ok(bytes
        .chunks_exact(std::mem::size_of::<i32>())
        .map(|chunk| i32::from_le_bytes(chunk.try_into().unwrap()))
        .collect())
}

fn top_k_logits(logits: &[f32], k: usize) -> Vec<(i32, f32)> {
    let mut values = logits
        .iter()
        .copied()
        .enumerate()
        .map(|(index, value)| (i32::try_from(index).unwrap(), value))
        .collect::<Vec<_>>();
    values.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    values.truncate(k.min(values.len()));
    values
}

fn format_token_list(token_ids: &[i32], vocab: Option<&LlamaVocab>) -> Vec<String> {
    token_ids
        .iter()
        .map(|&token_id| match vocab {
            Some(vocab) => format!(
                "{}:{}",
                token_id,
                vocab
                    .escaped_piece(token_id)
                    .unwrap_or_else(|| "<unknown>".to_owned())
            ),
            None => token_id.to_string(),
        })
        .collect()
}

fn describe_top_k(top_k: &[(i32, f32)], vocab: Option<&LlamaVocab>) -> Vec<String> {
    top_k
        .iter()
        .map(|(token_id, logit)| match vocab {
            Some(vocab) => format!(
                "{}:{:.6}:{}",
                token_id,
                logit,
                vocab
                    .escaped_piece(*token_id)
                    .unwrap_or_else(|| "<unknown>".to_owned())
            ),
            None => format!("{}:{:.6}", token_id, logit),
        })
        .collect()
}

struct LogitComparison {
    max_abs_diff: f64,
    mean_abs_diff: f64,
    rms_diff: f64,
    cosine_similarity: f64,
}

struct AttentionCacheSelfCheck {
    layer_index: u32,
    same_top1: bool,
    hidden_stats: LogitComparison,
}

struct AttentionDecodeBatchSelfCheck {
    layer_index: u32,
    hidden_stats: LogitComparison,
    k_cache_stats: LogitComparison,
    v_cache_stats: LogitComparison,
}

struct AttentionCacheTensorCheck {
    layer_index: u32,
    q_proj_stats: LogitComparison,
    q_pre_stats: LogitComparison,
    q_norm_stats: LogitComparison,
    k_norm_stats: LogitComparison,
    q_stats: LogitComparison,
    k_store_stats: LogitComparison,
    v_store_stats: LogitComparison,
    k_cache_stats: LogitComparison,
    v_cache_stats: LogitComparison,
    attn_stats: LogitComparison,
    full_attn_cpu_stats: LogitComparison,
    decode_attn_cpu_stats: LogitComparison,
}

struct RecurrentCacheSelfCheck {
    layer_index: u32,
    same_top1: bool,
    hidden_stats: LogitComparison,
    r_cache_stats: LogitComparison,
    s_cache_stats: LogitComparison,
}

struct RecurrentTensorCheck {
    layer_index: u32,
    z_stats: LogitComparison,
    beta_stats: LogitComparison,
    gate_stats: LogitComparison,
    conv_output_stats: LogitComparison,
    output_view_stats: LogitComparison,
    output_norm_stats: LogitComparison,
    z_silu_stats: LogitComparison,
    gated_output_stats: LogitComparison,
    final_output_stats: LogitComparison,
}

struct RecurrentStepCpuCheck {
    layer_index: u32,
    conv_output_cpu_stats: LogitComparison,
    q_conv_cpu_stats: LogitComparison,
    k_conv_cpu_stats: LogitComparison,
    output_view_cpu_stats: LogitComparison,
}

fn attention_cache_self_check(
    model: &LlamaModel,
    token_ids: &[i32],
    cache_type: TensorType,
) -> Result<AttentionCacheSelfCheck, Box<dyn std::error::Error>> {
    let (layer_index, block_spec) = qwen35moe_first_attention_block_spec(model)?;
    let layout = qwen35moe_attention_block_layout(model, layer_index)?;
    let decode_spec = qwen35moe_attention_decode_spec(
        model,
        layer_index,
        u32::try_from(token_ids.len())?,
        1,
        cache_type,
        cache_type,
    )?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut full_loaded = layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let compiled_full =
        compile_attention_block_metal(&mut full_loaded, &block_spec, token_ids.len())?;
    let full_run = execute_attention_block_graph_metal_cached(
        &compiled_full,
        &mut full_loaded,
        LogitsProbeInput::TokenIds(token_ids),
        &positions,
    )?;

    let mut decode_loaded = layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let compiled_decode = compile_attention_decode_metal(&mut decode_loaded, &decode_spec, 1)?;
    let mut decode_last_hidden = None;
    for (position, token_id) in token_ids.iter().copied().enumerate() {
        let run = execute_attention_decode_graph_metal_cached(
            &compiled_decode,
            &mut decode_loaded,
            LogitsProbeInput::TokenIds(std::slice::from_ref(&token_id)),
            &[i32::try_from(position)?],
            position + 1,
        )?;
        decode_last_hidden = Some(run.hidden);
    }
    let decode_last_hidden =
        decode_last_hidden.ok_or("attention decode self-check did not produce hidden output")?;
    let hidden_size = decode_last_hidden.len();
    let full_last_hidden = full_run
        .hidden
        .get(
            full_run
                .hidden
                .len()
                .checked_sub(hidden_size)
                .ok_or("attention block hidden output was shorter than expected")?..,
        )
        .ok_or("attention block hidden slice was out of range")?;
    let full_top1 = top_k_logits(full_last_hidden, 1)
        .first()
        .copied()
        .ok_or("attention block self-check produced no logits-like values")?;
    let decode_top1 = top_k_logits(&decode_last_hidden, 1)
        .first()
        .copied()
        .ok_or("attention decode self-check produced no hidden values")?;

    Ok(AttentionCacheSelfCheck {
        layer_index,
        same_top1: full_top1.0 == decode_top1.0,
        hidden_stats: compare_logits(full_last_hidden, &decode_last_hidden),
    })
}

fn recurrent_cache_self_check(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<RecurrentCacheSelfCheck, Box<dyn std::error::Error>> {
    let (layer_index, _) = qwen35moe_first_recurrent_block_spec(model)?;
    let layout = qwen35moe_recurrent_block_layout(model, layer_index)?;
    let spec = qwen35moe_delta_net_recurrent_decode_spec(
        model,
        layer_index,
        1,
        TensorType::F32,
        TensorType::F32,
    )?;

    let mut full_loaded = layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let full_compiled =
        compile_delta_net_recurrent_decode_metal(&mut full_loaded, &spec, token_ids.len())?;
    let full_run = execute_delta_net_recurrent_decode_graph_metal_cached(
        &full_compiled,
        &mut full_loaded,
        LogitsProbeInput::TokenIds(token_ids),
    )?;
    let full_r_cache = read_tensor_f32s(&full_loaded.ctx, "recur_decode.r_cache")?;
    let full_s_cache = read_tensor_f32s(&full_loaded.ctx, "recur_decode.s_cache")?;

    let mut decode_loaded = layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let decode_compiled = compile_delta_net_recurrent_decode_metal(&mut decode_loaded, &spec, 1)?;
    let mut decode_last_hidden = None;
    for &token_id in token_ids {
        let run = execute_delta_net_recurrent_decode_graph_metal_cached(
            &decode_compiled,
            &mut decode_loaded,
            LogitsProbeInput::TokenIds(std::slice::from_ref(&token_id)),
        )?;
        decode_last_hidden = Some(run.hidden);
    }
    let decode_last_hidden =
        decode_last_hidden.ok_or("recurrent decode self-check did not produce hidden output")?;
    let decode_r_cache = read_tensor_f32s(&decode_loaded.ctx, "recur_decode.r_cache")?;
    let decode_s_cache = read_tensor_f32s(&decode_loaded.ctx, "recur_decode.s_cache")?;
    let full_last_hidden = last_token_slice(&full_run.hidden, full_run.hidden_size)?;

    let hidden_top1 = top_k_logits(full_last_hidden, 1)
        .first()
        .copied()
        .ok_or("recurrent full decode produced no hidden values")?;
    let decode_top1 = top_k_logits(&decode_last_hidden, 1)
        .first()
        .copied()
        .ok_or("recurrent step decode produced no hidden values")?;

    Ok(RecurrentCacheSelfCheck {
        layer_index,
        same_top1: hidden_top1.0 == decode_top1.0,
        hidden_stats: compare_logits(full_last_hidden, &decode_last_hidden),
        r_cache_stats: compare_logits(&full_r_cache, &decode_r_cache),
        s_cache_stats: compare_logits(&full_s_cache, &decode_s_cache),
    })
}

fn recurrent_tensor_check(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<RecurrentTensorCheck, Box<dyn std::error::Error>> {
    let (layer_index, _) = qwen35moe_first_recurrent_block_spec(model)?;
    let layout = qwen35moe_recurrent_block_layout(model, layer_index)?;
    let spec = qwen35moe_delta_net_recurrent_decode_spec(
        model,
        layer_index,
        1,
        TensorType::F32,
        TensorType::F32,
    )?;

    let mut full_loaded = layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let full_runtime = MetalRuntime::new()?;
    let full_features = full_runtime.features();
    let mut full_graph = build_delta_net_recurrent_decode_graph(
        &mut full_loaded.ctx,
        &full_loaded.tensor_ids,
        &spec,
        token_ids.len(),
    )?;
    let full_z_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.z",
        "recur_decode.z_ck",
    )?;
    let full_beta_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.beta",
        "recur_decode.beta_ck",
    )?;
    let full_gate_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.gate",
        "recur_decode.gate_ck",
    )?;
    let full_conv_output_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.conv_output",
        "recur_decode.conv_output_ck",
    )?;
    let full_output_view_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.output_view",
        "recur_decode.output_view_ck",
    )?;
    let full_output_norm_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.output_norm",
        "recur_decode.output_norm_ck",
    )?;
    let full_z_silu_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.z_silu",
        "recur_decode.z_silu_ck",
    )?;
    let full_gated_output_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.gated_output",
        "recur_decode.gated_output_ck",
    )?;
    let full_final_output_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.final_output",
        "recur_decode.final_output_ck",
    )?;
    for tensor_id in [
        full_z_id,
        full_beta_id,
        full_gate_id,
        full_conv_output_id,
        full_output_view_id,
        full_output_norm_id,
        full_z_silu_id,
        full_gated_output_id,
        full_final_output_id,
    ] {
        full_graph
            .graph
            .build_forward_expand(&full_loaded.ctx, tensor_id)?;
    }
    let full_prepared = prepare_graph(&full_loaded.ctx, &full_graph.graph, full_features)?;
    let full_session = MetalGraphSession::from_runtime(
        full_runtime,
        &full_loaded.ctx,
        &full_prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let full_token_bytes = i32s_to_bytes(token_ids);
    let full_outputs = [
        full_z_id,
        full_beta_id,
        full_gate_id,
        full_conv_output_id,
        full_output_view_id,
        full_output_norm_id,
        full_z_silu_id,
        full_gated_output_id,
        full_final_output_id,
    ];
    let full_execution = full_session.execute(
        &full_loaded.ctx,
        &[MetalGraphTensorWrite {
            tensor_id: full_graph.input_primary,
            bytes: &full_token_bytes,
        }],
        &full_outputs,
    )?;
    let full_z = output_last_token_slice(&full_loaded.ctx, &full_execution, full_z_id)?;
    let full_beta = output_last_token_slice(&full_loaded.ctx, &full_execution, full_beta_id)?;
    let full_gate = output_last_token_slice(&full_loaded.ctx, &full_execution, full_gate_id)?;
    let full_conv_output =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_conv_output_id)?;
    let full_output_view =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_output_view_id)?;
    let full_output_norm =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_output_norm_id)?;
    let full_z_silu = output_last_token_slice(&full_loaded.ctx, &full_execution, full_z_silu_id)?;
    let full_gated_output =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_gated_output_id)?;
    let full_final_output =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_final_output_id)?;

    let mut step_loaded = layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let step_runtime = MetalRuntime::new()?;
    let step_features = step_runtime.features();
    let mut step_graph = build_delta_net_recurrent_decode_graph(
        &mut step_loaded.ctx,
        &step_loaded.tensor_ids,
        &spec,
        1,
    )?;
    let step_z_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.z",
        "recur_decode.z_ck",
    )?;
    let step_beta_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.beta",
        "recur_decode.beta_ck",
    )?;
    let step_gate_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.gate",
        "recur_decode.gate_ck",
    )?;
    let step_conv_output_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.conv_output",
        "recur_decode.conv_output_ck",
    )?;
    let step_output_view_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.output_view",
        "recur_decode.output_view_ck",
    )?;
    let step_output_norm_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.output_norm",
        "recur_decode.output_norm_ck",
    )?;
    let step_z_silu_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.z_silu",
        "recur_decode.z_silu_ck",
    )?;
    let step_gated_output_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.gated_output",
        "recur_decode.gated_output_ck",
    )?;
    let step_final_output_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.final_output",
        "recur_decode.final_output_ck",
    )?;
    for tensor_id in [
        step_z_id,
        step_beta_id,
        step_gate_id,
        step_conv_output_id,
        step_output_view_id,
        step_output_norm_id,
        step_z_silu_id,
        step_gated_output_id,
        step_final_output_id,
    ] {
        step_graph
            .graph
            .build_forward_expand(&step_loaded.ctx, tensor_id)?;
    }
    let step_prepared = prepare_graph(&step_loaded.ctx, &step_graph.graph, step_features)?;
    let step_session = MetalGraphSession::from_runtime(
        step_runtime,
        &step_loaded.ctx,
        &step_prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let step_outputs = [
        step_z_id,
        step_beta_id,
        step_gate_id,
        step_conv_output_id,
        step_output_view_id,
        step_output_norm_id,
        step_z_silu_id,
        step_gated_output_id,
        step_final_output_id,
    ];
    let mut step_z = None;
    let mut step_beta = None;
    let mut step_gate = None;
    let mut step_conv_output = None;
    let mut step_output_view = None;
    let mut step_output_norm = None;
    let mut step_z_silu = None;
    let mut step_gated_output = None;
    let mut step_final_output = None;
    for &token_id in token_ids {
        let step_token_bytes = i32s_to_bytes(&[token_id]);
        let execution = step_session.execute(
            &step_loaded.ctx,
            &[MetalGraphTensorWrite {
                tensor_id: step_graph.input_primary,
                bytes: &step_token_bytes,
            }],
            &step_outputs,
        )?;
        step_z = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_z_id,
        )?);
        step_beta = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_beta_id,
        )?);
        step_gate = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_gate_id,
        )?);
        step_conv_output = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_conv_output_id,
        )?);
        step_output_view = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_output_view_id,
        )?);
        step_output_norm = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_output_norm_id,
        )?);
        step_z_silu = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_z_silu_id,
        )?);
        step_gated_output = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_gated_output_id,
        )?);
        step_final_output = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_final_output_id,
        )?);
    }

    Ok(RecurrentTensorCheck {
        layer_index,
        z_stats: compare_logits(
            &full_z,
            step_z.as_deref().ok_or("missing recurrent step z")?,
        ),
        beta_stats: compare_logits(
            &full_beta,
            step_beta.as_deref().ok_or("missing recurrent step beta")?,
        ),
        gate_stats: compare_logits(
            &full_gate,
            step_gate.as_deref().ok_or("missing recurrent step gate")?,
        ),
        conv_output_stats: compare_logits(
            &full_conv_output,
            step_conv_output
                .as_deref()
                .ok_or("missing recurrent step conv_output")?,
        ),
        output_view_stats: compare_logits(
            &full_output_view,
            step_output_view
                .as_deref()
                .ok_or("missing recurrent step output_view")?,
        ),
        output_norm_stats: compare_logits(
            &full_output_norm,
            step_output_norm
                .as_deref()
                .ok_or("missing recurrent step output_norm")?,
        ),
        z_silu_stats: compare_logits(
            &full_z_silu,
            step_z_silu
                .as_deref()
                .ok_or("missing recurrent step z_silu")?,
        ),
        gated_output_stats: compare_logits(
            &full_gated_output,
            step_gated_output
                .as_deref()
                .ok_or("missing recurrent step gated_output")?,
        ),
        final_output_stats: compare_logits(
            &full_final_output,
            step_final_output
                .as_deref()
                .ok_or("missing recurrent step final_output")?,
        ),
    })
}

fn recurrent_step_cpu_check(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<RecurrentStepCpuCheck, Box<dyn std::error::Error>> {
    if token_ids.len() < 2 {
        return Err("recurrent step cpu check requires at least two tokens".into());
    }

    let (layer_index, block_spec) = qwen35moe_first_recurrent_block_spec(model)?;
    let layout = qwen35moe_recurrent_block_layout(model, layer_index)?;
    let spec = qwen35moe_delta_net_recurrent_decode_spec(
        model,
        layer_index,
        1,
        TensorType::F32,
        TensorType::F32,
    )?;

    let mut loaded = layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let runtime = MetalRuntime::new()?;
    let features = runtime.features();
    let mut graph =
        build_delta_net_recurrent_decode_graph(&mut loaded.ctx, &loaded.tensor_ids, &spec, 1)?;
    let conv_input_id = add_hidden_token_checkpoint_by_name(
        &mut loaded.ctx,
        "recur_decode.conv_input",
        "recur_decode.conv_input_ck",
    )?;
    let beta_id = add_hidden_token_checkpoint_by_name(
        &mut loaded.ctx,
        "recur_decode.beta",
        "recur_decode.beta_ck",
    )?;
    let gate_id = add_hidden_token_checkpoint_by_name(
        &mut loaded.ctx,
        "recur_decode.gate",
        "recur_decode.gate_ck",
    )?;
    let q_conv_id = add_hidden_token_checkpoint_by_name(
        &mut loaded.ctx,
        "recur_decode.q_conv_predelta",
        "recur_decode.q_conv_ck",
    )?;
    let k_conv_id = add_hidden_token_checkpoint_by_name(
        &mut loaded.ctx,
        "recur_decode.k_conv_predelta",
        "recur_decode.k_conv_ck",
    )?;
    let conv_output_id = add_hidden_token_checkpoint_by_name(
        &mut loaded.ctx,
        "recur_decode.conv_output",
        "recur_decode.conv_output_ck",
    )?;
    let output_view_id = add_hidden_token_checkpoint_by_name(
        &mut loaded.ctx,
        "recur_decode.output_view",
        "recur_decode.output_view_ck",
    )?;
    for tensor_id in [
        conv_input_id,
        beta_id,
        gate_id,
        q_conv_id,
        k_conv_id,
        conv_output_id,
        output_view_id,
    ] {
        graph.graph.build_forward_expand(&loaded.ctx, tensor_id)?;
    }
    let prepared = prepare_graph(&loaded.ctx, &graph.graph, features)?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &loaded.ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;

    let first_token_bytes = i32s_to_bytes(&[token_ids[0]]);
    let first_execution = session.execute(
        &loaded.ctx,
        &[MetalGraphTensorWrite {
            tensor_id: graph.input_primary,
            bytes: &first_token_bytes,
        }],
        &[graph.s_cache],
    )?;
    let state_before = bytes_to_f32s(
        first_execution
            .outputs
            .get(&graph.s_cache)
            .ok_or("missing recurrent step s_cache output after first token")?,
    );

    let second_token_bytes = i32s_to_bytes(&[token_ids[1]]);
    let execution = session.execute(
        &loaded.ctx,
        &[MetalGraphTensorWrite {
            tensor_id: graph.input_primary,
            bytes: &second_token_bytes,
        }],
        &[
            conv_input_id,
            beta_id,
            gate_id,
            q_conv_id,
            k_conv_id,
            conv_output_id,
            output_view_id,
        ],
    )?;

    let conv_input = bytes_to_f32s(
        execution
            .outputs
            .get(&conv_input_id)
            .ok_or("missing recurrent step conv_input output")?,
    );
    let beta = bytes_to_f32s(
        execution
            .outputs
            .get(&beta_id)
            .ok_or("missing recurrent step beta output")?,
    );
    let gate = bytes_to_f32s(
        execution
            .outputs
            .get(&gate_id)
            .ok_or("missing recurrent step gate output")?,
    );
    let q_conv = bytes_to_f32s(
        execution
            .outputs
            .get(&q_conv_id)
            .ok_or("missing recurrent step q_conv output")?,
    );
    let k_conv = bytes_to_f32s(
        execution
            .outputs
            .get(&k_conv_id)
            .ok_or("missing recurrent step k_conv output")?,
    );
    let conv_output = bytes_to_f32s(
        execution
            .outputs
            .get(&conv_output_id)
            .ok_or("missing recurrent step conv_output output")?,
    );
    let output_view = bytes_to_f32s(
        execution
            .outputs
            .get(&output_view_id)
            .ok_or("missing recurrent step output_view output")?,
    );

    let conv_kernel_id = loaded
        .tensor_ids
        .get(&block_spec.conv_kernel_name)
        .copied()
        .ok_or("missing recurrent conv kernel tensor id")?;
    let conv_kernel_tensor = loaded
        .ctx
        .tensor(conv_kernel_id)
        .ok_or("invalid recurrent conv kernel tensor")?;
    let conv_kernel_size = usize::try_from(conv_kernel_tensor.ne[0])?;
    let conv_channels = usize::try_from(conv_kernel_tensor.ne[1])?;
    let conv_kernel = read_tensor_f32s(&loaded.ctx, &block_spec.conv_kernel_name)?;

    let conv_output_cpu =
        cpu_ssm_conv_single_token(&conv_input, &conv_kernel, conv_kernel_size, conv_channels)?;

    let key_head_dim = usize::try_from(block_spec.key_head_dim)?;
    let key_head_count = usize::try_from(block_spec.key_head_count)?;
    let value_head_dim = usize::try_from(block_spec.value_head_dim)?;
    let value_head_count = usize::try_from(block_spec.value_head_count)?;

    let q_width = key_head_dim
        .checked_mul(key_head_count)
        .ok_or("overflow computing recurrent q width")?;
    let k_width = q_width;
    let v_width = value_head_dim
        .checked_mul(value_head_count)
        .ok_or("overflow computing recurrent v width")?;
    let expected_qkv = q_width
        .checked_add(k_width)
        .and_then(|v| v.checked_add(v_width))
        .ok_or("overflow computing recurrent qkv width")?;
    if conv_output.len() != expected_qkv || conv_output_cpu.len() != expected_qkv {
        return Err(format!(
            "unexpected recurrent conv output lengths: metal={} cpu={} expected {}",
            conv_output.len(),
            conv_output_cpu.len(),
            expected_qkv
        )
        .into());
    }

    let q_cpu = cpu_l2_norm_heads(
        &conv_output[..q_width],
        key_head_dim,
        key_head_count,
        block_spec.rms_epsilon,
    )?;
    let k_cpu = cpu_l2_norm_heads(
        &conv_output[q_width..q_width + k_width],
        key_head_dim,
        key_head_count,
        block_spec.rms_epsilon,
    )?;
    let output_view_cpu = cpu_gated_delta_net_last_token(
        &q_cpu,
        &k_cpu,
        &conv_output[q_width + k_width..],
        &gate,
        &beta,
        &state_before,
        key_head_count,
        value_head_count,
        value_head_dim,
    )?;

    Ok(RecurrentStepCpuCheck {
        layer_index,
        conv_output_cpu_stats: compare_logits(&conv_output, &conv_output_cpu),
        q_conv_cpu_stats: compare_logits(&q_conv, &q_cpu),
        k_conv_cpu_stats: compare_logits(&k_conv, &k_cpu),
        output_view_cpu_stats: compare_logits(&output_view, &output_view_cpu),
    })
}

fn attention_decode_batch_self_check(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<AttentionDecodeBatchSelfCheck, Box<dyn std::error::Error>> {
    let (layer_index, _) = qwen35moe_first_attention_block_spec(model)?;
    let layout = qwen35moe_attention_block_layout(model, layer_index)?;
    let spec = qwen35moe_attention_decode_spec(
        model,
        layer_index,
        u32::try_from(token_ids.len())?,
        1,
        TensorType::F32,
        TensorType::F32,
    )?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut batched_loaded = layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let batched_compiled =
        compile_attention_decode_metal(&mut batched_loaded, &spec, token_ids.len())?;
    let batched_run = execute_attention_decode_graph_metal_cached(
        &batched_compiled,
        &mut batched_loaded,
        LogitsProbeInput::TokenIds(token_ids),
        &positions,
        token_ids.len(),
    )?;
    let batched_last_hidden = last_token_slice(&batched_run.hidden, batched_run.hidden_size)?;
    let batched_k_cache = read_tensor_f32s(&batched_loaded.ctx, "attn_decode.k_cache")?;
    let batched_v_cache = read_tensor_f32s(&batched_loaded.ctx, "attn_decode.v_cache")?;

    let mut step_loaded = layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let step_compiled = compile_attention_decode_metal(&mut step_loaded, &spec, 1)?;
    let mut step_last_hidden = None;
    for (position, token_id) in token_ids.iter().copied().enumerate() {
        let run = execute_attention_decode_graph_metal_cached(
            &step_compiled,
            &mut step_loaded,
            LogitsProbeInput::TokenIds(std::slice::from_ref(&token_id)),
            &[i32::try_from(position)?],
            position + 1,
        )?;
        step_last_hidden = Some(run.hidden);
    }
    let step_last_hidden = step_last_hidden
        .ok_or("attention decode batch self-check did not produce hidden output")?;
    let step_k_cache = read_tensor_f32s(&step_loaded.ctx, "attn_decode.k_cache")?;
    let step_v_cache = read_tensor_f32s(&step_loaded.ctx, "attn_decode.v_cache")?;

    Ok(AttentionDecodeBatchSelfCheck {
        layer_index,
        hidden_stats: compare_logits(batched_last_hidden, &step_last_hidden),
        k_cache_stats: compare_logits(&batched_k_cache, &step_k_cache),
        v_cache_stats: compare_logits(&batched_v_cache, &step_v_cache),
    })
}

fn add_contiguous_checkpoint(
    ctx: &mut Context,
    src: TensorId,
    name: &str,
) -> Result<TensorId, Box<dyn std::error::Error>> {
    let tensor = ctx
        .tensor(src)
        .ok_or_else(|| format!("invalid tensor id {src} for checkpoint {name}"))?;
    let cont = ctx.cont_2d(src, tensor.ne[0], tensor.ne[1])?;
    ctx.set_tensor_name(cont, name)?;
    Ok(cont)
}

fn add_hidden_token_checkpoint_by_name(
    ctx: &mut Context,
    src_name: &str,
    checkpoint_name: &str,
) -> Result<TensorId, Box<dyn std::error::Error>> {
    let src = ctx
        .get_tensor(src_name)
        .ok_or_else(|| format!("missing tensor '{src_name}'"))?;
    let tensor = ctx
        .tensor(src)
        .ok_or_else(|| format!("invalid tensor id {src} for checkpoint {checkpoint_name}"))?;
    let rank = tensor.desc.layout.rank();
    let cont = if rank <= 2 {
        ctx.cont_2d(src, tensor.ne[0], tensor.ne[1])?
    } else {
        ctx.cont_2d(
            src,
            tensor.ne[0] * tensor.ne[1],
            tensor.ne[2] * tensor.ne[3],
        )?
    };
    ctx.set_tensor_name(cont, checkpoint_name)?;
    Ok(cont)
}

fn encode_rope_positions(
    rope: &AttentionRopeSpec,
    positions: &[i32],
    n_tokens: usize,
) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
    let n_components =
        if rope.mode == GGML_ROPE_TYPE_IMROPE || (rope.mode & GGML_ROPE_TYPE_MROPE) != 0 {
            4
        } else {
            1
        };

    if n_components == 1 {
        if positions.len() != n_tokens {
            return Err(format!(
                "rope positions length mismatch: got {}, expected {}",
                positions.len(),
                n_tokens
            )
            .into());
        }
        return Ok(positions.to_vec());
    }

    let expanded_len = n_tokens
        .checked_mul(n_components)
        .ok_or("overflow computing expanded rope positions")?;
    if positions.len() == expanded_len {
        return Ok(positions.to_vec());
    }
    if positions.len() != n_tokens {
        return Err(format!(
            "mrope positions length mismatch: got {}, expected {} or {}",
            positions.len(),
            n_tokens,
            expanded_len
        )
        .into());
    }

    let mut expanded = vec![0_i32; expanded_len];
    expanded[..n_tokens].copy_from_slice(positions);
    expanded[n_tokens..2 * n_tokens].copy_from_slice(positions);
    expanded[2 * n_tokens..3 * n_tokens].copy_from_slice(positions);
    Ok(expanded)
}

fn causal_mask_f16_bytes(n_tokens: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(n_tokens * n_tokens * std::mem::size_of::<u16>());
    let zero = f32_to_f16(0.0);
    let neg_inf = f32_to_f16(f32::NEG_INFINITY);
    for query in 0..n_tokens {
        for key in 0..n_tokens {
            let value = if key > query { neg_inf } else { zero };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

fn causal_mask_f32_bytes(n_tokens: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(n_tokens * n_tokens * std::mem::size_of::<f32>());
    for query in 0..n_tokens {
        for key in 0..n_tokens {
            let value = if key > query { f32::NEG_INFINITY } else { 0.0 };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

fn causal_mask_bytes_for_tensor(
    ctx: &Context,
    tensor_id: TensorId,
    n_tokens: usize,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid attention mask tensor id {tensor_id}"))?;
    Ok(match tensor.desc.ty {
        TensorType::F16 => causal_mask_f16_bytes(n_tokens),
        TensorType::F32 => causal_mask_f32_bytes(n_tokens),
        other => {
            return Err(format!("unsupported attention mask tensor type {}", other.name()).into())
        }
    })
}

fn attention_cache_tensor_check(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<AttentionCacheTensorCheck, Box<dyn std::error::Error>> {
    let (layer_index, block_spec) = qwen35moe_first_attention_block_spec(model)?;
    let layout = qwen35moe_attention_block_layout(model, layer_index)?;
    let decode_spec = qwen35moe_attention_decode_spec(
        model,
        layer_index,
        u32::try_from(token_ids.len())?,
        1,
        TensorType::F32,
        TensorType::F32,
    )?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let full_rope_positions = block_spec
        .rope
        .as_ref()
        .map(|rope| encode_rope_positions(rope, &positions, token_ids.len()))
        .transpose()?;

    let mut full_loaded = layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let full_runtime = MetalRuntime::new()?;
    let full_features = full_runtime.features();
    let (mut full_block, _) = prepare_attention_block_graph(
        &mut full_loaded.ctx,
        &full_loaded.tensor_ids,
        &block_spec,
        token_ids.len(),
        full_features,
    )?;
    let full_token_bytes = i32s_to_bytes(token_ids);
    let full_pos_bytes = i32s_to_bytes(
        full_rope_positions
            .as_deref()
            .ok_or("missing full rope positions")?,
    );
    let full_q_store_id = full_loaded
        .ctx
        .get_tensor("attn.q_store")
        .ok_or("missing attn.q_store")?;
    let full_q_proj_id = full_loaded
        .ctx
        .get_tensor("attn.q_proj")
        .ok_or("missing attn.q_proj")?;
    let full_q_pre_store_id = full_loaded
        .ctx
        .get_tensor("attn.q_pre_store")
        .ok_or("missing attn.q_pre_store")?;
    let full_q_pre_cont_id =
        add_contiguous_checkpoint(&mut full_loaded.ctx, full_q_pre_store_id, "attn.q_pre_cont")?;
    let full_q_norm_store_id = full_loaded
        .ctx
        .get_tensor("attn.q_norm_store")
        .ok_or("missing attn.q_norm_store")?;
    let full_q_norm_cont_id = add_contiguous_checkpoint(
        &mut full_loaded.ctx,
        full_q_norm_store_id,
        "attn.q_norm_cont",
    )?;
    let full_k_store_id = full_loaded
        .ctx
        .get_tensor("attn.k_store")
        .ok_or("missing attn.k_store")?;
    let full_k_cont_id =
        add_contiguous_checkpoint(&mut full_loaded.ctx, full_k_store_id, "attn.k_cont")?;
    let full_k_norm_store_id = full_loaded
        .ctx
        .get_tensor("attn.k_norm_store")
        .ok_or("missing attn.k_norm_store")?;
    let full_k_norm_cont_id = add_contiguous_checkpoint(
        &mut full_loaded.ctx,
        full_k_norm_store_id,
        "attn.k_norm_cont",
    )?;
    let full_v_store_id = full_loaded
        .ctx
        .get_tensor("attn.v_store")
        .ok_or("missing attn.v_store")?;
    let full_v_cont_id =
        add_contiguous_checkpoint(&mut full_loaded.ctx, full_v_store_id, "attn.v_cont")?;
    let full_q_cont_id =
        add_contiguous_checkpoint(&mut full_loaded.ctx, full_q_store_id, "attn.q_cont")?;
    let full_attn_id = full_loaded
        .ctx
        .get_tensor("attn.attn_flat")
        .ok_or("missing attn.attn_flat")?;
    full_block
        .graph
        .build_forward_expand(&full_loaded.ctx, full_q_cont_id)?;
    full_block
        .graph
        .build_forward_expand(&full_loaded.ctx, full_q_proj_id)?;
    full_block
        .graph
        .build_forward_expand(&full_loaded.ctx, full_q_pre_cont_id)?;
    full_block
        .graph
        .build_forward_expand(&full_loaded.ctx, full_q_norm_cont_id)?;
    full_block
        .graph
        .build_forward_expand(&full_loaded.ctx, full_k_cont_id)?;
    full_block
        .graph
        .build_forward_expand(&full_loaded.ctx, full_k_norm_cont_id)?;
    full_block
        .graph
        .build_forward_expand(&full_loaded.ctx, full_v_cont_id)?;
    full_block
        .graph
        .build_forward_expand(&full_loaded.ctx, full_attn_id)?;
    let full_prepared = prepare_graph(&full_loaded.ctx, &full_block.graph, full_features)?;
    let full_session = MetalGraphSession::from_runtime(
        full_runtime,
        &full_loaded.ctx,
        &full_prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let full_outputs = [
        full_q_cont_id,
        full_q_proj_id,
        full_q_pre_cont_id,
        full_q_norm_cont_id,
        full_k_cont_id,
        full_k_norm_cont_id,
        full_v_cont_id,
        full_attn_id,
    ];
    let full_mask_bytes = full_block
        .input_mask
        .map(|input_mask| {
            causal_mask_bytes_for_tensor(&full_loaded.ctx, input_mask, token_ids.len())
        })
        .transpose()?;
    let mut full_writes = vec![MetalGraphTensorWrite {
        tensor_id: full_block.input_primary,
        bytes: &full_token_bytes,
    }];
    if let Some(input_positions) = full_block.input_positions {
        full_writes.push(MetalGraphTensorWrite {
            tensor_id: input_positions,
            bytes: &full_pos_bytes,
        });
    }
    if let Some(input_mask) = full_block.input_mask {
        full_writes.push(MetalGraphTensorWrite {
            tensor_id: input_mask,
            bytes: full_mask_bytes
                .as_deref()
                .ok_or("missing full attention mask")?,
        });
    }
    let full_execution = full_session.execute(&full_loaded.ctx, &full_writes, &full_outputs)?;
    let full_q_store = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_q_cont_id)
            .ok_or("missing full q_store output")?,
    );
    let full_q_proj = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_q_proj_id)
            .ok_or("missing full q_proj output")?,
    );
    let full_q_pre_store = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_q_pre_cont_id)
            .ok_or("missing full q_pre_store output")?,
    );
    let full_q_norm_store = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_q_norm_cont_id)
            .ok_or("missing full q_norm_store output")?,
    );
    let full_k_store = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_k_cont_id)
            .ok_or("missing full k_store output")?,
    );
    let full_k_norm_store = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_k_norm_cont_id)
            .ok_or("missing full k_norm_store output")?,
    );
    let full_v_store = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_v_cont_id)
            .ok_or("missing full v_store output")?,
    );
    let full_attn = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_attn_id)
            .ok_or("missing full attn output")?,
    );

    let q_row_width = full_q_store.len() / token_ids.len();
    let k_row_width = full_k_store.len() / token_ids.len();
    let v_row_width = full_v_store.len() / token_ids.len();
    let attn_row_width = full_attn.len() / token_ids.len();

    let mut decode_loaded = layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let decode_runtime = MetalRuntime::new()?;
    let decode_features = decode_runtime.features();
    let (mut decode_graph, _) = prepare_attention_decode_graph(
        &mut decode_loaded.ctx,
        &decode_loaded.tensor_ids,
        &decode_spec,
        1,
        decode_features,
    )?;
    let decode_rope_positions = decode_spec
        .block
        .rope
        .as_ref()
        .map(|rope| encode_rope_positions(rope, &[positions[1]], 1))
        .transpose()?;

    let k_cache_len = decode_loaded
        .ctx
        .tensor(decode_graph.k_cache)
        .ok_or("invalid decode k_cache tensor")?
        .nelements()
        .try_into()?;
    let v_cache_len = decode_loaded
        .ctx
        .tensor(decode_graph.v_cache)
        .ok_or("invalid decode v_cache tensor")?
        .nelements()
        .try_into()?;
    let mut k_cache = vec![0.0f32; k_cache_len];
    let mut v_cache = vec![0.0f32; v_cache_len];
    k_cache[..k_row_width].copy_from_slice(&full_k_store[..k_row_width]);
    v_cache[..v_row_width].copy_from_slice(&full_v_store[..v_row_width]);
    decode_loaded
        .ctx
        .write_tensor_data(decode_graph.k_cache, &f32s_to_bytes(&k_cache))?;
    decode_loaded
        .ctx
        .write_tensor_data(decode_graph.v_cache, &f32s_to_bytes(&v_cache))?;

    let decode_q_store_id = decode_loaded
        .ctx
        .get_tensor("attn_decode.q_store")
        .ok_or("missing attn_decode.q_store")?;
    let decode_q_cont_id = add_contiguous_checkpoint(
        &mut decode_loaded.ctx,
        decode_q_store_id,
        "attn_decode.q_cont",
    )?;
    let decode_q_proj_id = decode_loaded
        .ctx
        .get_tensor("attn_decode.q_proj")
        .ok_or("missing attn_decode.q_proj")?;
    let decode_q_pre_store_id = decode_loaded
        .ctx
        .get_tensor("attn_decode.q_pre_store")
        .ok_or("missing attn_decode.q_pre_store")?;
    let decode_q_pre_cont_id = add_contiguous_checkpoint(
        &mut decode_loaded.ctx,
        decode_q_pre_store_id,
        "attn_decode.q_pre_cont",
    )?;
    let decode_q_norm_store_id = decode_loaded
        .ctx
        .get_tensor("attn_decode.q_norm_store")
        .ok_or("missing attn_decode.q_norm_store")?;
    let decode_q_norm_cont_id = add_contiguous_checkpoint(
        &mut decode_loaded.ctx,
        decode_q_norm_store_id,
        "attn_decode.q_norm_cont",
    )?;
    let decode_k_store_id = decode_loaded
        .ctx
        .get_tensor("attn_decode.k_store")
        .ok_or("missing attn_decode.k_store")?;
    let decode_k_cont_id = add_contiguous_checkpoint(
        &mut decode_loaded.ctx,
        decode_k_store_id,
        "attn_decode.k_cont",
    )?;
    let decode_k_norm_store_id = decode_loaded
        .ctx
        .get_tensor("attn_decode.k_norm_store")
        .ok_or("missing attn_decode.k_norm_store")?;
    let decode_k_norm_cont_id = add_contiguous_checkpoint(
        &mut decode_loaded.ctx,
        decode_k_norm_store_id,
        "attn_decode.k_norm_cont",
    )?;
    let decode_v_store_id = decode_loaded
        .ctx
        .get_tensor("attn_decode.v_store")
        .ok_or("missing attn_decode.v_store")?;
    let decode_v_cont_id = add_contiguous_checkpoint(
        &mut decode_loaded.ctx,
        decode_v_store_id,
        "attn_decode.v_cont",
    )?;
    let decode_attn_id = decode_loaded
        .ctx
        .get_tensor("attn_decode.attn_flat")
        .ok_or("missing attn_decode.attn_flat")?;
    decode_graph
        .graph
        .build_forward_expand(&decode_loaded.ctx, decode_q_cont_id)?;
    decode_graph
        .graph
        .build_forward_expand(&decode_loaded.ctx, decode_q_proj_id)?;
    decode_graph
        .graph
        .build_forward_expand(&decode_loaded.ctx, decode_q_pre_cont_id)?;
    decode_graph
        .graph
        .build_forward_expand(&decode_loaded.ctx, decode_q_norm_cont_id)?;
    decode_graph
        .graph
        .build_forward_expand(&decode_loaded.ctx, decode_k_cont_id)?;
    decode_graph
        .graph
        .build_forward_expand(&decode_loaded.ctx, decode_k_norm_cont_id)?;
    decode_graph
        .graph
        .build_forward_expand(&decode_loaded.ctx, decode_v_cont_id)?;
    decode_graph
        .graph
        .build_forward_expand(&decode_loaded.ctx, decode_graph.k_cache)?;
    decode_graph
        .graph
        .build_forward_expand(&decode_loaded.ctx, decode_graph.v_cache)?;
    decode_graph
        .graph
        .build_forward_expand(&decode_loaded.ctx, decode_attn_id)?;
    let decode_prepared = prepare_graph(&decode_loaded.ctx, &decode_graph.graph, decode_features)?;

    let decode_session = MetalGraphSession::from_runtime(
        decode_runtime,
        &decode_loaded.ctx,
        &decode_prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let decode_token_bytes = i32s_to_bytes(&[token_ids[1]]);
    let decode_pos_bytes = i32s_to_bytes(&[positions[1]]);
    let decode_rope_pos_bytes = decode_rope_positions.as_deref().map(i32s_to_bytes);
    let decode_outputs = [
        decode_q_cont_id,
        decode_q_proj_id,
        decode_q_pre_cont_id,
        decode_q_norm_cont_id,
        decode_k_cont_id,
        decode_k_norm_cont_id,
        decode_v_cont_id,
        decode_graph.k_cache,
        decode_graph.v_cache,
        decode_attn_id,
    ];
    let decode_writes = [
        MetalGraphTensorWrite {
            tensor_id: decode_graph.input_primary,
            bytes: &decode_token_bytes,
        },
        MetalGraphTensorWrite {
            tensor_id: decode_graph.input_positions,
            bytes: &decode_pos_bytes,
        },
    ];
    let mut decode_writes = decode_writes.to_vec();
    if let Some(input_rope_positions) = decode_graph.input_rope_positions {
        decode_writes.push(MetalGraphTensorWrite {
            tensor_id: input_rope_positions,
            bytes: decode_rope_pos_bytes
                .as_deref()
                .ok_or("missing decode rope positions")?,
        });
    }
    let decode_execution =
        decode_session.execute(&decode_loaded.ctx, &decode_writes, &decode_outputs)?;
    let decode_q_store = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_q_cont_id)
            .ok_or("missing decode q_store output")?,
    );
    let decode_q_proj = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_q_proj_id)
            .ok_or("missing decode q_proj output")?,
    );
    let decode_q_pre_store = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_q_pre_cont_id)
            .ok_or("missing decode q_pre_store output")?,
    );
    let decode_q_norm_store = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_q_norm_cont_id)
            .ok_or("missing decode q_norm_store output")?,
    );
    let decode_k_store = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_k_cont_id)
            .ok_or("missing decode k_store output")?,
    );
    let decode_k_norm_store = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_k_norm_cont_id)
            .ok_or("missing decode k_norm_store output")?,
    );
    let decode_v_store = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_v_cont_id)
            .ok_or("missing decode v_store output")?,
    );
    let decode_k_cache = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_graph.k_cache)
            .ok_or("missing decode k_cache output")?,
    );
    let decode_v_cache = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_graph.v_cache)
            .ok_or("missing decode v_cache output")?,
    );
    let decode_attn = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_attn_id)
            .ok_or("missing decode attn output")?,
    );
    let attn_cpu = cpu_flash_attn_gqa_last_token(
        &full_q_store[q_row_width..],
        &full_k_store,
        &full_v_store,
        usize::try_from(block_spec.q_head_dim)?,
        usize::try_from(block_spec.q_head_count)?,
        usize::try_from(block_spec.kv_head_count)?,
        token_ids.len(),
    )?;

    Ok(AttentionCacheTensorCheck {
        layer_index,
        q_proj_stats: compare_logits(
            &decode_q_proj,
            &full_q_proj[full_q_proj.len() / token_ids.len()..],
        ),
        q_pre_stats: compare_logits(&decode_q_pre_store, &full_q_pre_store[q_row_width..]),
        q_norm_stats: compare_logits(&decode_q_norm_store, &full_q_norm_store[q_row_width..]),
        k_norm_stats: compare_logits(&decode_k_norm_store, &full_k_norm_store[k_row_width..]),
        q_stats: compare_logits(&decode_q_store, &full_q_store[q_row_width..]),
        k_store_stats: compare_logits(&decode_k_store, &full_k_store[k_row_width..]),
        v_store_stats: compare_logits(&decode_v_store, &full_v_store[v_row_width..]),
        k_cache_stats: compare_logits(&decode_k_cache[..full_k_store.len()], &full_k_store),
        v_cache_stats: compare_logits(&decode_v_cache[..full_v_store.len()], &full_v_store),
        attn_stats: compare_logits(&decode_attn, &full_attn[attn_row_width..]),
        full_attn_cpu_stats: compare_logits(&full_attn[attn_row_width..], &attn_cpu),
        decode_attn_cpu_stats: compare_logits(&decode_attn, &attn_cpu),
    })
}

fn cpu_flash_attn_gqa_last_token(
    q_last: &[f32],
    k_all: &[f32],
    v_all: &[f32],
    head_dim: usize,
    q_head_count: usize,
    kv_head_count: usize,
    n_tokens: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if q_head_count == 0 || kv_head_count == 0 || q_head_count % kv_head_count != 0 {
        return Err(format!(
            "invalid gqa shape: q_head_count={} kv_head_count={}",
            q_head_count, kv_head_count
        )
        .into());
    }
    let q_expected = head_dim
        .checked_mul(q_head_count)
        .ok_or("overflow computing q attention width")?;
    let kv_expected = head_dim
        .checked_mul(kv_head_count)
        .ok_or("overflow computing kv attention width")?;
    if q_last.len() != q_expected {
        return Err(format!(
            "invalid q_last length: got {}, expected {}",
            q_last.len(),
            q_expected
        )
        .into());
    }
    if k_all.len() != kv_expected * n_tokens || v_all.len() != kv_expected * n_tokens {
        return Err(format!(
            "invalid kv lengths: k={} v={} expected {}",
            k_all.len(),
            v_all.len(),
            kv_expected * n_tokens
        )
        .into());
    }

    let heads_per_kv = q_head_count / kv_head_count;
    let scale = 1.0f32 / (head_dim as f32).sqrt();
    let mut out = vec![0.0f32; q_expected];

    for q_head in 0..q_head_count {
        let kv_head = q_head / heads_per_kv;
        let q_row = &q_last[q_head * head_dim..(q_head + 1) * head_dim];
        let mut scores = vec![0.0f32; n_tokens];
        for token in 0..n_tokens {
            let k_offset = token * kv_expected + kv_head * head_dim;
            let k_row = &k_all[k_offset..k_offset + head_dim];
            let mut dot = 0.0f32;
            for i in 0..head_dim {
                dot += q_row[i] * k_row[i];
            }
            scores[token] = dot * scale;
        }

        let max_score = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut sum = 0.0f32;
        for score in &mut scores {
            *score = (*score - max_score).exp();
            sum += *score;
        }
        for score in &mut scores {
            *score /= sum.max(f32::MIN_POSITIVE);
        }

        let out_row = &mut out[q_head * head_dim..(q_head + 1) * head_dim];
        for token in 0..n_tokens {
            let v_offset = token * kv_expected + kv_head * head_dim;
            let v_row = &v_all[v_offset..v_offset + head_dim];
            let weight = scores[token];
            for i in 0..head_dim {
                out_row[i] += weight * v_row[i];
            }
        }
    }

    Ok(out)
}

fn cpu_ssm_conv_single_token(
    conv_input: &[f32],
    conv_kernel: &[f32],
    kernel_size: usize,
    channels: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let expected = kernel_size
        .checked_mul(channels)
        .ok_or("overflow computing ssm conv tensor size")?;
    if conv_input.len() != expected || conv_kernel.len() != expected {
        return Err(format!(
            "invalid ssm conv lengths: input={} kernel={} expected={}",
            conv_input.len(),
            conv_kernel.len(),
            expected
        )
        .into());
    }

    let mut out = vec![0.0f32; channels];
    for channel in 0..channels {
        let row_offset = channel
            .checked_mul(kernel_size)
            .ok_or("overflow computing ssm conv row offset")?;
        let mut sum = 0.0f32;
        for tap in 0..kernel_size {
            sum += conv_input[row_offset + tap] * conv_kernel[row_offset + tap];
        }
        out[channel] = sum / (1.0 + (-sum).exp());
    }
    Ok(out)
}

fn cpu_l2_norm_heads(
    values: &[f32],
    head_dim: usize,
    head_count: usize,
    epsilon: f32,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let expected = head_dim
        .checked_mul(head_count)
        .ok_or("overflow computing l2_norm length")?;
    if values.len() != expected {
        return Err(format!(
            "invalid l2_norm input length: got {}, expected {}",
            values.len(),
            expected
        )
        .into());
    }

    let mut out = vec![0.0f32; values.len()];
    for head in 0..head_count {
        let start = head
            .checked_mul(head_dim)
            .ok_or("overflow computing l2_norm head offset")?;
        let end = start + head_dim;
        let row = &values[start..end];
        let norm = row.iter().map(|v| v * v).sum::<f32>() + epsilon;
        let scale = norm.sqrt().max(f32::MIN_POSITIVE);
        for (dst, src) in out[start..end].iter_mut().zip(row.iter().copied()) {
            *dst = src / scale;
        }
    }
    Ok(out)
}

fn cpu_gated_delta_net_last_token(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    gate: &[f32],
    beta: &[f32],
    state: &[f32],
    key_head_count: usize,
    value_head_count: usize,
    value_head_dim: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if key_head_count == 0 || value_head_count == 0 || value_head_count % key_head_count != 0 {
        return Err(format!(
            "invalid recurrent head shape: key_head_count={} value_head_count={}",
            key_head_count, value_head_count
        )
        .into());
    }

    let key_width = value_head_dim
        .checked_mul(key_head_count)
        .ok_or("overflow computing recurrent key width")?;
    let value_width = value_head_dim
        .checked_mul(value_head_count)
        .ok_or("overflow computing recurrent value width")?;
    let state_per_head = value_head_dim
        .checked_mul(value_head_dim)
        .ok_or("overflow computing recurrent state size")?;
    let expected_state = state_per_head
        .checked_mul(value_head_count)
        .ok_or("overflow computing recurrent state width")?;
    if q.len() != key_width || k.len() != key_width || v.len() != value_width {
        return Err(format!(
            "invalid recurrent qkv lengths: q={} k={} v={} expected q/k={} v={}",
            q.len(),
            k.len(),
            v.len(),
            key_width,
            value_width
        )
        .into());
    }
    if state.len() != expected_state {
        return Err(format!(
            "invalid recurrent state length: got {}, expected {}",
            state.len(),
            expected_state
        )
        .into());
    }
    if beta.len() != value_head_count {
        return Err(format!(
            "invalid recurrent beta length: got {}, expected {}",
            beta.len(),
            value_head_count
        )
        .into());
    }
    if gate.len() != value_head_count && gate.len() != value_width {
        return Err(format!(
            "invalid recurrent gate length: got {}, expected {} or {}",
            gate.len(),
            value_head_count,
            value_width
        )
        .into());
    }

    let scale = 1.0f32 / (value_head_dim as f32).sqrt();
    let mut out = vec![0.0f32; value_width];

    for v_head in 0..value_head_count {
        let q_head = v_head % key_head_count;
        let q_row = &q[q_head * value_head_dim..(q_head + 1) * value_head_dim];
        let k_row = &k[q_head * value_head_dim..(q_head + 1) * value_head_dim];
        let v_row = &v[v_head * value_head_dim..(v_head + 1) * value_head_dim];
        let beta_value = beta[v_head];
        let gate_row = if gate.len() == value_head_count {
            None
        } else {
            Some(&gate[v_head * value_head_dim..(v_head + 1) * value_head_dim])
        };
        let gate_scalar = if gate.len() == value_head_count {
            Some(gate[v_head])
        } else {
            None
        };
        let state_offset = v_head
            .checked_mul(state_per_head)
            .ok_or("overflow computing recurrent state head offset")?;
        let state_head = &state[state_offset..state_offset + state_per_head];
        let out_row = &mut out[v_head * value_head_dim..(v_head + 1) * value_head_dim];

        for row in 0..value_head_dim {
            let row_offset = row
                .checked_mul(value_head_dim)
                .ok_or("overflow computing recurrent state row offset")?;
            let state_row = &state_head[row_offset..row_offset + value_head_dim];
            let mut decayed = vec![0.0f32; value_head_dim];
            for col in 0..value_head_dim {
                let decay = if let Some(gate_scalar) = gate_scalar {
                    gate_scalar.exp()
                } else {
                    gate_row.ok_or("missing recurrent gate row")?[col].exp()
                };
                decayed[col] = state_row[col] * decay;
            }

            let dot_k = decayed
                .iter()
                .zip(k_row.iter())
                .map(|(a, b)| a * b)
                .sum::<f32>();
            let delta = (v_row[row] - dot_k) * beta_value;
            let mut updated = vec![0.0f32; value_head_dim];
            for col in 0..value_head_dim {
                updated[col] = decayed[col] + k_row[col] * delta;
            }
            out_row[row] = updated
                .iter()
                .zip(q_row.iter())
                .map(|(a, b)| a * b)
                .sum::<f32>()
                * scale;
        }
    }

    Ok(out)
}

fn compare_logits(rust: &[f32], upstream: &[f32]) -> LogitComparison {
    let mut max_abs_diff = 0.0_f64;
    let mut abs_sum = 0.0_f64;
    let mut sq_sum = 0.0_f64;
    let mut dot = 0.0_f64;
    let mut rust_norm = 0.0_f64;
    let mut upstream_norm = 0.0_f64;

    for (&rust, &upstream) in rust.iter().zip(upstream.iter()) {
        let rust = rust as f64;
        let upstream = upstream as f64;
        let diff = (rust - upstream).abs();
        max_abs_diff = max_abs_diff.max(diff);
        abs_sum += diff;
        sq_sum += diff * diff;
        dot += rust * upstream;
        rust_norm += rust * rust;
        upstream_norm += upstream * upstream;
    }

    let count = rust.len().max(1) as f64;
    let cosine_similarity = if rust_norm == 0.0 || upstream_norm == 0.0 {
        0.0
    } else {
        dot / (rust_norm.sqrt() * upstream_norm.sqrt())
    };

    LogitComparison {
        max_abs_diff,
        mean_abs_diff: abs_sum / count,
        rms_diff: (sq_sum / count).sqrt(),
        cosine_similarity,
    }
}

fn last_token_slice(
    values: &[f32],
    hidden_size: usize,
) -> Result<&[f32], Box<dyn std::error::Error>> {
    if hidden_size == 0 {
        return Err("hidden_size was zero".into());
    }
    let start = values
        .len()
        .checked_sub(hidden_size)
        .ok_or("value buffer shorter than hidden size")?;
    Ok(&values[start..])
}

fn output_last_token_slice(
    ctx: &Context,
    execution: &makepad_ggml::backend::metal::MetalGraphExecution,
    tensor_id: TensorId,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let values = bytes_to_f32s(
        execution
            .outputs
            .get(&tensor_id)
            .ok_or_else(|| format!("missing execution output for tensor {tensor_id}"))?,
    );
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid tensor id {tensor_id}"))?;
    Ok(last_token_slice(
        &values,
        usize::try_from(tensor.ne[0]).map_err(|_| "tensor width does not fit in usize")?,
    )?
    .to_vec())
}

fn bytes_to_f32s(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect()
}

fn read_tensor_f32s(ctx: &Context, name: &str) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let tensor_id = ctx
        .get_tensor(name)
        .ok_or_else(|| format!("missing tensor '{name}'"))?;
    Ok(bytes_to_f32s(ctx.tensor_data(tensor_id)?))
}

fn f32s_to_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn i32s_to_bytes(values: &[i32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}
