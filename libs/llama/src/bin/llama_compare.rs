use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use makepad_ggml::{
    backend::metal::{
        prepare_graph, try_rms_norm_mul_f32, BufferStorageMode, MetalBuffer, MetalGraphSession,
        MetalGraphTensorWrite, MetalRuntime,
    },
    bf16_to_f32, f16_to_f32, f32_to_f16, get_rows_ggml_bytes_cpu, ggml_row_size_for_type,
    BufferUsage, Context, Graph, InitParams, Prec, Tensor, TensorId, TensorLayout, TensorType,
    GGML_ROPE_TYPE_IMROPE, GGML_ROPE_TYPE_MROPE,
};
use makepad_llama::{
    allocate_hybrid_shared_cache_tensors, build_delta_net_recurrent_decode_graph,
    build_hybrid_decode_graph_with_outputs, build_moe_ffn_graph, compile_attention_block_metal,
    compile_attention_decode_metal_with_key_count, compile_delta_net_recurrent_decode_metal,
    compile_hybrid_decode_metal_with_shared_runtime_and_state_and_outputs_and_attention_key_count,
    create_metal_context_buffer_with_runtime, execute_attention_block_graph_metal_cached,
    execute_attention_decode_graph_metal_cached,
    execute_delta_net_recurrent_decode_graph_metal_cached, gemma4_attention_block_layout,
    gemma4_attention_block_spec, gemma4_attention_decode_spec,
    gemma4_embedding_logits_probe_spec, gemma4_first_attention_block_spec,
    gemma4_first_full_attention_block_spec, execute_logits_probe_metal,
    prepare_attention_block_graph,
    prepare_attention_decode_graph_with_key_count, qwen35_attention_block_layout,
    qwen35_attention_block_spec, qwen35_attention_decode_spec,
    qwen35_delta_net_recurrent_decode_spec, qwen35_first_attention_block_spec,
    qwen35_first_recurrent_block_spec, qwen35_recurrent_block_layout, qwen35_recurrent_block_spec,
    qwen35moe_attention_block_layout, qwen35moe_attention_block_spec,
    qwen35moe_attention_decode_spec, qwen35moe_delta_net_recurrent_decode_spec,
    qwen35moe_first_attention_block_spec, qwen35moe_first_recurrent_block_spec,
    qwen35moe_moe_ffn_layout, qwen35moe_moe_ffn_spec, qwen35moe_recurrent_block_layout,
    AttentionBlockSpec, AttentionDecodeSpec, AttentionRopeSpec, GgufWeightLayout,
    HybridDecodeBatchLayout, HybridDecodeGraph, HybridDecodeSpec, HybridLayerSpec,
    HybridSharedCacheTensorIds, LlamaArchitecture, LlamaError, LlamaModel, LlamaSession,
    LlamaSessionConfig, LlamaVocab, LoadedGgufWeights, LogitsProbeInput, ProbeInputKind,
    Qwen35MoeLayerKind,
};

const DEFAULT_PROMPT: &str = "The capital of France is";
const DEFAULT_TOP_K: usize = 10;
const DEFAULT_UPSTREAM_DEBUG: &str =
    "/Users/admin/llama.cpp/build-arm64-apple-clang-release/bin/llama-debug";
const COMPARE_EXTRA_CONTEXT_BYTES: usize = 512 << 20;
const COMPARE_SHARED_HYBRID_EXTRA_CONTEXT_BYTES: usize = 1536 << 20;
const TENSOR_PREVIEW_EDGE_COUNT: usize = 3;

struct Args {
    model_path: PathBuf,
    prompt: String,
    upstream_debug_path: PathBuf,
    top_k: usize,
}

struct UpstreamReference {
    token_ids: Vec<i32>,
    logits: Vec<f32>,
    step_logits: Vec<Vec<f32>>,
    output_dir: PathBuf,
}

enum AttentionDecodeSequenceInput<'a> {
    TokenIds(&'a [i32]),
    EmbeddingsF32 { data: &'a [f32], hidden_size: usize },
}

struct AttentionDecodeSequenceRun {
    result_output: Vec<f32>,
    last_hidden: Vec<f32>,
    k_cache_bytes: Vec<u8>,
    v_cache_bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpstreamDebugMode {
    Batched,
    Stepwise,
}

impl UpstreamDebugMode {
    fn output_dir_suffix(self) -> &'static str {
        match self {
            Self::Batched => "batched",
            Self::Stepwise => "step",
        }
    }

    fn is_stepwise(self) -> bool {
        matches!(self, Self::Stepwise)
    }
}

struct SharedHybridDebugEnv {
    weights: LoadedGgufWeights,
    spec: HybridDecodeSpec,
    shared_runtime: MetalRuntime,
    shared_main_buffer: MetalBuffer,
    shared_cache: HybridSharedCacheTensorIds,
}

struct HybridCheckpointTensor {
    label: String,
    tensor_id: TensorId,
}

struct HybridCheckpointSession {
    weights: LoadedGgufWeights,
    spec: HybridDecodeSpec,
    decode: HybridDecodeGraph,
    session: MetalGraphSession,
    #[allow(dead_code)]
    shared_cache: HybridSharedCacheTensorIds,
    checkpoints: Vec<HybridCheckpointTensor>,
}

#[derive(Default)]
struct HybridCacheSnapshot {
    attention_k: BTreeMap<u32, Vec<f32>>,
    attention_v: BTreeMap<u32, Vec<f32>>,
    recurrent_r: BTreeMap<u32, Vec<f32>>,
    recurrent_s: BTreeMap<u32, Vec<f32>>,
}

#[derive(Clone, Copy, Debug, Default)]
struct LayerDiffSummary {
    layer_index: Option<u32>,
    max_abs_diff: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct HybridCacheDiffSummary {
    attention_k: LayerDiffSummary,
    attention_v: LayerDiffSummary,
    recurrent_r: LayerDiffSummary,
    recurrent_s: LayerDiffSummary,
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
    let vocab = LlamaVocab::from_model(&model)?;
    let rust_prompt_token_ids = vocab.tokenize(&args.prompt, false, true)?;
    let rust_step_logits = run_rust_hybrid_decode_stepwise(&model, &rust_prompt_token_ids)?;
    let rust_logits = run_rust_hybrid_decode(&model, &rust_prompt_token_ids)?;
    let rust_batched_logits = match run_rust_hybrid_decode_batched(&model, &rust_prompt_token_ids)
    {
        Ok(logits) => Some(logits),
        Err(err) => {
            println!("compare.batched.error: {}", err);
            None
        }
    };
    let upstream_batched = run_upstream_debug(&args)?;
    let upstream = run_upstream_step_debug(&args)?;
    if upstream_batched.token_ids != upstream.token_ids {
        return Err("upstream batched and upstream step tokenization differ".into());
    }
    if rust_prompt_token_ids != upstream.token_ids {
        return Err(format!(
            "native and upstream tokenization differ: rust={:?} upstream={:?}",
            rust_prompt_token_ids, upstream.token_ids
        )
        .into());
    }
    let upstream_step_final_logits = upstream
        .step_logits
        .last()
        .ok_or("upstream step output produced no step logits")?;

    let input_token_labels = format_token_list(&upstream.token_ids, Some(&vocab));
    println!("model: {}", args.model_path.display());
    println!("prompt: {}", args.prompt);
    println!("input.token_count: {}", upstream.token_ids.len());
    println!("input.tokens: {:?}", upstream.token_ids);
    println!("input.token_pieces: {:?}", input_token_labels);
    println!(
        "upstream.step.output_dir: {}",
        upstream.output_dir.display()
    );
    println!(
        "upstream.batched.output_dir: {}",
        upstream_batched.output_dir.display()
    );

    if rust_logits.len() != upstream_step_final_logits.len() {
        return Err(format!(
            "logit length mismatch: rust={} upstream={}",
            rust_logits.len(),
            upstream_step_final_logits.len()
        )
        .into());
    }
    let rust_top = top_k_logits(&rust_logits, args.top_k);
    let upstream_top = top_k_logits(upstream_step_final_logits, args.top_k);
    let continue_token_id = upstream_top.first().map(|(id, _)| *id);
    let stats = compare_logits(&rust_logits, upstream_step_final_logits);
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
    if rust_step_logits.len() != upstream.step_logits.len() {
        return Err(format!(
            "step count mismatch: rust={} upstream={}",
            rust_step_logits.len(),
            upstream.step_logits.len()
        )
        .into());
    }
    let mut first_step_diff_index = None;
    for (step_index, (rust_step, upstream_step)) in rust_step_logits
        .iter()
        .zip(upstream.step_logits.iter())
        .enumerate()
    {
        if rust_step.len() != upstream_step.len() {
            return Err(format!(
                "step {step_index} logit length mismatch: rust={} upstream={}",
                rust_step.len(),
                upstream_step.len()
            )
            .into());
        }
        let step_stats = compare_logits(rust_step, upstream_step);
        let rust_step_top1 = top_k_logits(rust_step, 1).first().map(|(id, _)| *id);
        let upstream_step_top1 = top_k_logits(upstream_step, 1).first().map(|(id, _)| *id);
        if first_step_diff_index.is_none()
            && (rust_step_top1 != upstream_step_top1 || step_stats.max_abs_diff > 0.0)
        {
            first_step_diff_index = Some(step_index);
        }
        println!(
            "compare.step{step_index}.token: {}",
            input_token_labels
                .get(step_index)
                .cloned()
                .unwrap_or_else(|| upstream.token_ids[step_index].to_string())
        );
        println!(
            "compare.step{step_index}.same_top1: {}",
            rust_step_top1 == upstream_step_top1
        );
        println!(
            "compare.step{step_index}.max_abs_diff: {:.9}",
            step_stats.max_abs_diff
        );
        println!(
            "compare.step{step_index}.mean_abs_diff: {:.9}",
            step_stats.mean_abs_diff
        );
        println!(
            "compare.step{step_index}.rms_diff: {:.9}",
            step_stats.rms_diff
        );
        println!(
            "compare.step{step_index}.cosine_similarity: {:.9}",
            step_stats.cosine_similarity
        );
    }
    println!("compare.step.count: {}", rust_step_logits.len());
    println!(
        "compare.step.first_diff_index: {}",
        first_step_diff_index
            .map(|index| index.to_string())
            .unwrap_or_else(|| "none".to_owned())
    );
    if matches!(model.architecture, LlamaArchitecture::Qwen35) && upstream.token_ids.len() > 1 {
        let step0_shared_cache_diff = compare_hybrid_first_step_shared_cache_checkpoints(
            &model,
            upstream.token_ids[0],
            u32::try_from(upstream.token_ids.len())?,
        )?;
        println!(
            "step0.shared_cache.checkpoint.input_embed_max_abs_diff: {:.9}",
            step0_shared_cache_diff.input_embed.max_abs_diff
        );
        for layer in &step0_shared_cache_diff.layers {
            if let Some(attn_post_norm) = &layer.attn_post_norm {
                println!(
                    "step0.shared_cache.layer{}._attn_post_norm_max_abs_diff: {:.9}",
                    layer.layer_index, attn_post_norm.max_abs_diff
                );
            }
            println!(
                "step0.shared_cache.layer{}._attn_residual_max_abs_diff: {:.9}",
                layer.layer_index, layer.attn_residual.max_abs_diff
            );
            if let Some(ffn_input_norm) = &layer.ffn_input_norm {
                println!(
                    "step0.shared_cache.layer{}._ffn_input_norm_max_abs_diff: {:.9}",
                    layer.layer_index, ffn_input_norm.max_abs_diff
                );
            }
            println!(
                "step0.shared_cache.layer{}._ffn_out_max_abs_diff: {:.9}",
                layer.layer_index, layer.ffn_out.max_abs_diff
            );
            if let Some(ffn_post_norm) = &layer.ffn_post_norm {
                println!(
                    "step0.shared_cache.layer{}._ffn_post_norm_max_abs_diff: {:.9}",
                    layer.layer_index, ffn_post_norm.max_abs_diff
                );
            }
            println!(
                "step0.shared_cache.layer{}._post_ffn_max_abs_diff: {:.9}",
                layer.layer_index, layer.post_ffn.max_abs_diff
            );
        }
        println!(
            "step0.shared_cache.checkpoint.result_norm_max_abs_diff: {:.9}",
            step0_shared_cache_diff.result_norm.max_abs_diff
        );
        println!(
            "step0.shared_cache.checkpoint.result_logits_max_abs_diff: {:.9}",
            step0_shared_cache_diff.result_logits.max_abs_diff
        );
        let (first_attention_layer, _) = qwen35_first_attention_block_spec(&model)?;
        let step0_first_attn_standalone = attention_from_hidden_first_step_capacity_check(
            &model,
            upstream.token_ids[0],
            first_attention_layer.saturating_sub(1),
            first_attention_layer,
            u32::try_from(upstream.token_ids.len())?,
        )?;
        println!(
            "step0.first_attn_standalone.layer{}._hidden_max_abs_diff: {:.9}",
            first_attention_layer, step0_first_attn_standalone.hidden_stats.max_abs_diff
        );
        println!(
            "step0.first_attn_standalone.layer{}._k_cache_max_abs_diff: {:.9}",
            first_attention_layer, step0_first_attn_standalone.k_cache_stats.max_abs_diff
        );
        println!(
            "step0.first_attn_standalone.layer{}._v_cache_max_abs_diff: {:.9}",
            first_attention_layer, step0_first_attn_standalone.v_cache_stats.max_abs_diff
        );
    }
    let upstream_step_saved_final_stats =
        compare_logits(&upstream.logits, upstream_step_final_logits);
    println!(
        "upstream.step.saved_final_vs_last_step.max_abs_diff: {:.9}",
        upstream_step_saved_final_stats.max_abs_diff
    );
    println!(
        "upstream.step.saved_final_vs_last_step.mean_abs_diff: {:.9}",
        upstream_step_saved_final_stats.mean_abs_diff
    );
    println!(
        "upstream.step.saved_final_vs_last_step.rms_diff: {:.9}",
        upstream_step_saved_final_stats.rms_diff
    );
    println!(
        "upstream.step.saved_final_vs_last_step.cosine_similarity: {:.9}",
        upstream_step_saved_final_stats.cosine_similarity
    );
    let upstream_mode_stats = compare_logits(upstream_step_final_logits, &upstream_batched.logits);
    println!(
        "upstream.mode.step_vs_batched.max_abs_diff: {:.9}",
        upstream_mode_stats.max_abs_diff
    );
    println!(
        "upstream.mode.step_vs_batched.mean_abs_diff: {:.9}",
        upstream_mode_stats.mean_abs_diff
    );
    println!(
        "upstream.mode.step_vs_batched.rms_diff: {:.9}",
        upstream_mode_stats.rms_diff
    );
    println!(
        "upstream.mode.step_vs_batched.cosine_similarity: {:.9}",
        upstream_mode_stats.cosine_similarity
    );
    if let Some(rust_batched_logits) = &rust_batched_logits {
        if rust_batched_logits.len() != upstream_batched.logits.len() {
            return Err(format!(
                "batched logit length mismatch: rust={} upstream={}",
                rust_batched_logits.len(),
                upstream_batched.logits.len()
            )
            .into());
        }
        let rust_batched_top = top_k_logits(rust_batched_logits, args.top_k);
        let upstream_batched_top = top_k_logits(&upstream_batched.logits, args.top_k);
        let upstream_batched_top_ids = upstream_batched_top
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        let batched_stats = compare_logits(rust_batched_logits, &upstream_batched.logits);
        let rust_batched_top_ids = rust_batched_top
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        let batched_top_overlap = rust_batched_top_ids
            .iter()
            .filter(|id| upstream_batched_top_ids.contains(id))
            .count();
        println!(
            "compare.batched.same_top1: {}",
            rust_batched_top.first().map(|(id, _)| *id)
                == upstream_batched_top.first().map(|(id, _)| *id)
        );
        println!(
            "compare.batched.same_top{}_ids: {}",
            args.top_k,
            rust_batched_top_ids == upstream_batched_top_ids
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
            describe_top_k(&rust_batched_top, Some(&vocab))
        );

        if let Some(continue_token) = continue_token_id {
            let mut continued_token_ids = upstream.token_ids.clone();
            continued_token_ids.push(continue_token);
            let rust_continued_logits = run_rust_session_prefill_then_continue(
                &model,
                &upstream.token_ids,
                continue_token,
                upstream.token_ids.len(),
            )?;
            let rust_full_continued_logits =
                run_rust_hybrid_decode_batched(&model, &continued_token_ids)?;
            if rust_continued_logits.len() != rust_full_continued_logits.len() {
                return Err(format!(
                    "continued logit length mismatch: continued={} full={}",
                    rust_continued_logits.len(),
                    rust_full_continued_logits.len()
                )
                .into());
            }
            let continued_top = top_k_logits(&rust_continued_logits, args.top_k);
            let full_continued_top = top_k_logits(&rust_full_continued_logits, args.top_k);
            let continued_stats =
                compare_logits(&rust_continued_logits, &rust_full_continued_logits);
            println!("continue.token_id: {}", continue_token);
            println!(
                "continue.same_top1_vs_batched_full: {}",
                continued_top.first().map(|(id, _)| *id)
                    == full_continued_top.first().map(|(id, _)| *id)
            );
            println!(
                "continue.max_abs_diff_vs_batched_full: {:.9}",
                continued_stats.max_abs_diff
            );
            println!(
                "continue.mean_abs_diff_vs_batched_full: {:.9}",
                continued_stats.mean_abs_diff
            );
            println!(
                "continue.rms_diff_vs_batched_full: {:.9}",
                continued_stats.rms_diff
            );
            println!(
                "continue.cosine_similarity_vs_batched_full: {:.9}",
                continued_stats.cosine_similarity
            );
            println!(
                "continue.step.top{}: {:?}",
                args.top_k,
                describe_top_k(&continued_top, Some(&vocab))
            );
            println!(
                "continue.batched_full.top{}: {:?}",
                args.top_k,
                describe_top_k(&full_continued_top, Some(&vocab))
            );

            let final_cache_diff = compare_shared_hybrid_split_vs_full_cache_state(
                &model,
                &upstream.token_ids,
                continue_token,
                TensorType::F16,
                TensorType::F16,
            )?;
            println!(
                "continue.final_cache.attn_k_max_abs_diff: {:.9}{}",
                final_cache_diff.attention_k.max_abs_diff,
                format_layer_suffix(final_cache_diff.attention_k.layer_index)
            );
            println!(
                "continue.final_cache.attn_v_max_abs_diff: {:.9}{}",
                final_cache_diff.attention_v.max_abs_diff,
                format_layer_suffix(final_cache_diff.attention_v.layer_index)
            );
            println!(
                "continue.final_cache.recurrent_r_max_abs_diff: {:.9}{}",
                final_cache_diff.recurrent_r.max_abs_diff,
                format_layer_suffix(final_cache_diff.recurrent_r.layer_index)
            );
            println!(
                "continue.final_cache.recurrent_s_max_abs_diff: {:.9}{}",
                final_cache_diff.recurrent_s.max_abs_diff,
                format_layer_suffix(final_cache_diff.recurrent_s.layer_index)
            );
            if matches!(
                model.architecture,
                LlamaArchitecture::Qwen35 | LlamaArchitecture::Gemma4
            ) {
                let continued_stats_f32 = compare_shared_hybrid_split_vs_full_logits(
                    &model,
                    &upstream.token_ids,
                    continue_token,
                    TensorType::F32,
                    TensorType::F32,
                )?;
                println!(
                    "continue.f32.max_abs_diff_vs_batched_full: {:.9}",
                    continued_stats_f32.max_abs_diff
                );
                println!(
                    "continue.f32.mean_abs_diff_vs_batched_full: {:.9}",
                    continued_stats_f32.mean_abs_diff
                );
                println!(
                    "continue.f32.rms_diff_vs_batched_full: {:.9}",
                    continued_stats_f32.rms_diff
                );
                println!(
                    "continue.f32.cosine_similarity_vs_batched_full: {:.9}",
                    continued_stats_f32.cosine_similarity
                );
                let final_cache_diff_f32 = compare_shared_hybrid_split_vs_full_cache_state(
                    &model,
                    &upstream.token_ids,
                    continue_token,
                    TensorType::F32,
                    TensorType::F32,
                )?;
                println!(
                    "continue.f32_cache.attn_k_max_abs_diff: {:.9}{}",
                    final_cache_diff_f32.attention_k.max_abs_diff,
                    format_layer_suffix(final_cache_diff_f32.attention_k.layer_index)
                );
                println!(
                    "continue.f32_cache.attn_v_max_abs_diff: {:.9}{}",
                    final_cache_diff_f32.attention_v.max_abs_diff,
                    format_layer_suffix(final_cache_diff_f32.attention_v.layer_index)
                );
                println!(
                    "continue.f32_cache.recurrent_r_max_abs_diff: {:.9}{}",
                    final_cache_diff_f32.recurrent_r.max_abs_diff,
                    format_layer_suffix(final_cache_diff_f32.recurrent_r.layer_index)
                );
                println!(
                    "continue.f32_cache.recurrent_s_max_abs_diff: {:.9}{}",
                    final_cache_diff_f32.recurrent_s.max_abs_diff,
                    format_layer_suffix(final_cache_diff_f32.recurrent_s.layer_index)
                );
            }
            let continue_checkpoint_diff =
                compare_hybrid_continue_checkpoints(&model, &upstream.token_ids, continue_token)?;
            println!(
                "continue.checkpoint.input_embed_max_abs_diff: {:.9}",
                continue_checkpoint_diff.input_embed.max_abs_diff
            );
            println!(
                "continue.checkpoint.result_norm_max_abs_diff: {:.9}",
                continue_checkpoint_diff.result_norm.max_abs_diff
            );
            println!(
                "continue.checkpoint.result_logits_max_abs_diff: {:.9}",
                continue_checkpoint_diff.result_logits.max_abs_diff
            );
            let first_continue_layer_diff = continue_checkpoint_diff
                .layers
                .iter()
                .find(|layer| hybrid_split_layer_max_abs_diff(layer) > 0.0);
            println!(
                "continue.checkpoint.first_layer_diff: {}",
                first_continue_layer_diff
                    .map(|layer| layer.layer_index.to_string())
                    .unwrap_or_else(|| "none".to_owned())
            );
            if let Some(layer) = first_continue_layer_diff {
                println!(
                    "continue.checkpoint.layer{}._attn_residual_max_abs_diff: {:.9}",
                    layer.layer_index, layer.attn_residual.max_abs_diff
                );
                if let Some(attn_post_norm) = &layer.attn_post_norm {
                    println!(
                        "continue.checkpoint.layer{}._attn_post_norm_max_abs_diff: {:.9}",
                        layer.layer_index, attn_post_norm.max_abs_diff
                    );
                }
                if let Some(ffn_input_norm) = &layer.ffn_input_norm {
                    println!(
                        "continue.checkpoint.layer{}._ffn_input_norm_max_abs_diff: {:.9}",
                        layer.layer_index, ffn_input_norm.max_abs_diff
                    );
                }
                println!(
                    "continue.checkpoint.layer{}._ffn_out_max_abs_diff: {:.9}",
                    layer.layer_index, layer.ffn_out.max_abs_diff
                );
                if let Some(ffn_post_norm) = &layer.ffn_post_norm {
                    println!(
                        "continue.checkpoint.layer{}._ffn_post_norm_max_abs_diff: {:.9}",
                        layer.layer_index, ffn_post_norm.max_abs_diff
                    );
                }
                println!(
                    "continue.checkpoint.layer{}._post_ffn_max_abs_diff: {:.9}",
                    layer.layer_index, layer.post_ffn.max_abs_diff
                );
            }
        }
    }
    println!(
        "upstream.step.next.top{}: {:?}",
        args.top_k,
        describe_top_k(&upstream_top, Some(&vocab))
    );
    println!(
        "upstream.batched.next.top{}: {:?}",
        args.top_k,
        describe_top_k(
            &top_k_logits(&upstream_batched.logits, args.top_k),
            Some(&vocab)
        )
    );
    println!(
        "rust.next.top{}: {:?}",
        args.top_k,
        describe_top_k(&rust_top, Some(&vocab))
    );
    if upstream.token_ids.len() > 1 {
        let attention_check_f16 =
            attention_cache_self_check(&model, &upstream.token_ids[..2], TensorType::F16)?;
        let attention_check_f32 =
            attention_cache_self_check(&model, &upstream.token_ids[..2], TensorType::F32)?;
        let attention_decode_batch_check =
            attention_decode_batch_self_check(&model, &upstream.token_ids[..2])?;
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
            "attention_decode_batch.layer{}._result_output_max_abs_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check
                .result_output_stats
                .max_abs_diff
        );
        println!(
            "attention_decode_batch.layer{}._result_output_token0_max_abs_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check
                .first_token_result_output_stats
                .max_abs_diff
        );
        println!(
            "attention_decode_batch.layer{}._result_output_token1_max_abs_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check
                .last_token_result_output_stats
                .max_abs_diff
        );
        println!(
            "attention_decode_batch.layer{}._k_cache_max_abs_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check.k_cache_stats.max_abs_diff
        );
        println!(
            "attention_decode_batch.layer{}._step0_k_cache_row_max_abs_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check
                .step0_k_cache_row_stats
                .max_abs_diff
        );
        println!(
            "attention_decode_batch.layer{}._step0_k_cache_tail_zero_max_abs_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check
                .step0_k_cache_tail_zero_stats
                .max_abs_diff
        );
        println!(
            "attention_decode_batch.layer{}._v_cache_max_abs_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check.v_cache_stats.max_abs_diff
        );
        println!(
            "attention_decode_batch.layer{}._step0_v_cache_row_max_abs_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check
                .step0_v_cache_row_stats
                .max_abs_diff
        );
        println!(
            "attention_decode_batch.layer{}._step0_v_cache_tail_zero_max_abs_diff: {:.9}",
            attention_decode_batch_check.layer_index,
            attention_decode_batch_check
                .step0_v_cache_tail_zero_stats
                .max_abs_diff
        );
        if matches!(model.architecture, LlamaArchitecture::Gemma4) {
            let (full_layer_index, _) = gemma4_first_full_attention_block_spec(&model)?;
            let full_attention_check = attention_cache_self_check_for_layer(
                &model,
                &upstream.token_ids[..2],
                full_layer_index,
                TensorType::F32,
            )?;
            let full_attention_decode_batch_check = attention_decode_batch_self_check_for_layer(
                &model,
                &upstream.token_ids[..2],
                full_layer_index,
            )?;
            println!(
                "attention_cache_full.f32.layer{}._same_top1: {}",
                full_attention_check.layer_index, full_attention_check.same_top1
            );
            println!(
                "attention_cache_full.f32.layer{}._hidden_max_abs_diff: {:.9}",
                full_attention_check.layer_index, full_attention_check.hidden_stats.max_abs_diff
            );
            println!(
                "attention_cache_full.f32.layer{}._hidden_mean_abs_diff: {:.9}",
                full_attention_check.layer_index, full_attention_check.hidden_stats.mean_abs_diff
            );
            println!(
                "attention_cache_full.f32.layer{}._hidden_rms_diff: {:.9}",
                full_attention_check.layer_index, full_attention_check.hidden_stats.rms_diff
            );
            println!(
                "attention_cache_full.f32.layer{}._hidden_cosine_similarity: {:.9}",
                full_attention_check.layer_index,
                full_attention_check.hidden_stats.cosine_similarity
            );
            println!(
                "attention_decode_batch_full.layer{}._hidden_max_abs_diff: {:.9}",
                full_attention_decode_batch_check.layer_index,
                full_attention_decode_batch_check.hidden_stats.max_abs_diff
            );
            println!(
                "attention_decode_batch_full.layer{}._result_output_max_abs_diff: {:.9}",
                full_attention_decode_batch_check.layer_index,
                full_attention_decode_batch_check
                    .result_output_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_batch_full.layer{}._k_cache_max_abs_diff: {:.9}",
                full_attention_decode_batch_check.layer_index,
                full_attention_decode_batch_check.k_cache_stats.max_abs_diff
            );
            println!(
                "attention_decode_batch_full.layer{}._v_cache_max_abs_diff: {:.9}",
                full_attention_decode_batch_check.layer_index,
                full_attention_decode_batch_check.v_cache_stats.max_abs_diff
            );
            let gemma_upstream_input =
                gemma_upstream_input_check(&args, &model, &upstream.token_ids)?;
            println!(
                "gemma_upstream.input_embed_max_abs_diff: {:.9}",
                gemma_upstream_input.input_embed_stats.max_abs_diff
            );
            println!(
                "gemma_upstream.input_embed_standalone_vs_rust_max_abs_diff: {:.9}",
                gemma_upstream_input
                    .input_embed_standalone_vs_rust_stats
                    .max_abs_diff
            );
            println!(
                "gemma_upstream.input_embed_standalone_vs_upstream_max_abs_diff: {:.9}",
                gemma_upstream_input
                    .input_embed_standalone_vs_upstream_stats
                    .max_abs_diff
            );
            println!(
                "gemma_upstream.attn_norm_layer0_max_abs_diff: {:.9}",
                gemma_upstream_input.attn_input_norm_stats.max_abs_diff
            );
            println!(
                "gemma_upstream.attn_norm_layer0_standalone_vs_rust_max_abs_diff: {:.9}",
                gemma_upstream_input
                    .attn_input_norm_standalone_vs_rust_stats
                    .max_abs_diff
            );
            println!(
                "gemma_upstream.attn_norm_layer0_standalone_vs_upstream_max_abs_diff: {:.9}",
                gemma_upstream_input
                    .attn_input_norm_standalone_vs_upstream_stats
                    .max_abs_diff
            );
            let gemma_upstream_stack =
                gemma_upstream_layer_preview_stack_check(&args, &model, &upstream.token_ids)?;
            println!(
                "gemma_upstream.layer_stack.first_diff_layer: {}",
                gemma_upstream_stack
                    .first_diff_layer
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_owned())
            );
            if let Some(stats) = gemma_upstream_stack.first_diff_stats {
                println!(
                    "gemma_upstream.layer_stack.first_diff_max_abs_diff: {:.9}",
                    stats.max_abs_diff
                );
                println!(
                    "gemma_upstream.layer_stack.first_diff_mean_abs_diff: {:.9}",
                    stats.mean_abs_diff
                );
                println!(
                    "gemma_upstream.layer_stack.first_diff_cosine_similarity: {:.9}",
                    stats.cosine_similarity
                );
            }
            println!(
                "gemma_upstream.layer_stack.max_diff_layer: {}",
                gemma_upstream_stack
                    .max_diff_layer
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_owned())
            );
            println!(
                "gemma_upstream.layer_stack.max_diff_max_abs_diff: {:.9}",
                gemma_upstream_stack.max_diff_stats.max_abs_diff
            );
            println!(
                "gemma_upstream.layer_stack.max_diff_mean_abs_diff: {:.9}",
                gemma_upstream_stack.max_diff_stats.mean_abs_diff
            );
            println!(
                "gemma_upstream.layer_stack.max_diff_cosine_similarity: {:.9}",
                gemma_upstream_stack.max_diff_stats.cosine_similarity
            );
            let gemma_upstream_tail =
                gemma_upstream_tail_check(&args, &model, &upstream.token_ids)?;
            println!(
                "gemma_upstream.result_norm_max_abs_diff: {:.9}",
                gemma_upstream_tail.result_norm_stats.max_abs_diff
            );
            println!(
                "gemma_upstream.result_output_max_abs_diff: {:.9}",
                gemma_upstream_tail.result_output_stats.max_abs_diff
            );
            let gemma_final_probe = gemma_final_probe_check(
                &model,
                &upstream.token_ids,
                upstream_step_final_logits,
            )?;
            println!(
                "gemma_probe.result_output_vs_hybrid_max_abs_diff: {:.9}",
                gemma_final_probe.probe_vs_hybrid_stats.max_abs_diff
            );
            println!(
                "gemma_probe.result_output_vs_upstream_max_abs_diff: {:.9}",
                gemma_final_probe.probe_vs_upstream_stats.max_abs_diff
            );
            if model.require_gemma4()?.embedding_length_per_layer_input != 0 {
                let gemma_shared_per_layer =
                    gemma_upstream_shared_per_layer_input_check(&args, &model, &upstream.token_ids)?;
                println!(
                    "gemma_upstream.shared_per_layer.selected_max_abs_diff: {:.9}",
                    gemma_shared_per_layer.selected_stats.max_abs_diff
                );
                println!(
                    "gemma_upstream.shared_per_layer.proj_max_abs_diff: {:.9}",
                    gemma_shared_per_layer.proj_stats.max_abs_diff
                );
                println!(
                    "gemma_upstream.shared_per_layer.input_max_abs_diff: {:.9}",
                    gemma_shared_per_layer.input_stats.max_abs_diff
                );
                let direct_get_rows =
                    gemma_per_layer_token_get_rows_direct_check(&model, &upstream.token_ids)?;
                println!(
                    "gemma_manual.shared_per_layer.direct_get_rows_max_abs_diff: {:.9}",
                    direct_get_rows.max_abs_diff
                );
            }
            if let Some(layer_index) = gemma_upstream_stack.first_diff_layer {
                let layer_check = gemma_upstream_layer_preview_check(
                    &args,
                    &model,
                    &upstream.token_ids,
                    layer_index,
                )?;
                if let Some(attn_post_norm) = layer_check.attn_post_norm_stats {
                    println!(
                        "gemma_upstream.layer{}._attn_post_norm_max_abs_diff: {:.9}",
                        layer_check.layer_index, attn_post_norm.max_abs_diff
                    );
                }
                println!(
                    "gemma_upstream.layer{}._attn_out_max_abs_diff: {:.9}",
                    layer_check.layer_index, layer_check.attn_residual_stats.max_abs_diff
                );
                if let Some(ffn_input_norm) = layer_check.ffn_input_norm_stats {
                    println!(
                        "gemma_upstream.layer{}._ffn_norm_max_abs_diff: {:.9}",
                        layer_check.layer_index, ffn_input_norm.max_abs_diff
                    );
                }
                println!(
                    "gemma_upstream.layer{}._ffn_out_max_abs_diff: {:.9}",
                    layer_check.layer_index, layer_check.ffn_out_stats.max_abs_diff
                );
                if let Some(ffn_post_norm) = layer_check.ffn_post_norm_stats {
                    println!(
                        "gemma_upstream.layer{}._ffn_post_norm_max_abs_diff: {:.9}",
                        layer_check.layer_index, ffn_post_norm.max_abs_diff
                    );
                }
                if let Some(pe_in) = layer_check.pe_in_stats {
                    println!(
                        "gemma_upstream.layer{}._pe_in_max_abs_diff: {:.9}",
                        layer_check.layer_index, pe_in.max_abs_diff
                    );
                }
                if let Some(per_layer_embd_out) = layer_check.per_layer_embd_out_stats {
                    println!(
                        "gemma_upstream.layer{}._per_layer_embd_out_max_abs_diff: {:.9}",
                        layer_check.layer_index, per_layer_embd_out.max_abs_diff
                    );
                }
                println!(
                    "gemma_upstream.layer{}._layer_out_max_abs_diff: {:.9}",
                    layer_check.layer_index, layer_check.post_ffn_stats.max_abs_diff
                );
                let standalone_layer_out = gemma_upstream_layer_output_standalone_check(
                    &args,
                    &model,
                    &upstream.token_ids,
                    layer_check.layer_index,
                )?;
                let standalone_layer_check = gemma_upstream_layer_preview_standalone_check(
                    &args,
                    &model,
                    &upstream.token_ids,
                    layer_check.layer_index,
                )?;
                if let Some(attn_post_norm) = standalone_layer_check.attn_post_norm_stats {
                    println!(
                        "gemma_upstream.layer{}._attn_post_norm_standalone_max_abs_diff: {:.9}",
                        layer_check.layer_index, attn_post_norm.max_abs_diff
                    );
                }
                let raw_attn_post_norm = gemma_upstream_attn_post_norm_from_raw_attention_check(
                    &args,
                    &model,
                    &upstream.token_ids,
                    layer_check.layer_index,
                )?;
                println!(
                    "gemma_upstream.layer{}._attn_post_norm_from_raw_attn_max_abs_diff: {:.9}",
                    layer_check.layer_index, raw_attn_post_norm.max_abs_diff
                );
                if layer_check.layer_index == 0 && layer_check.post_ffn_stats.max_abs_diff > 1e-3 {
                    let manual_shared =
                        gemma_shared_per_layer_manual_check(&model, &upstream.token_ids)?;
                    println!(
                        "gemma_manual.shared_per_layer.selected_max_abs_diff: {:.9}",
                        manual_shared.selected_stats.max_abs_diff
                    );
                    println!(
                        "gemma_manual.shared_per_layer.proj_max_abs_diff: {:.9}",
                        manual_shared.proj_stats.max_abs_diff
                    );
                    println!(
                        "gemma_manual.shared_per_layer.input_max_abs_diff: {:.9}",
                        manual_shared.input_stats.max_abs_diff
                    );
                    let manual_graph_f16 = gemma_layer_manual_graph_check(
                        &model,
                        &upstream.token_ids,
                        layer_check.layer_index,
                        TensorType::F16,
                    )?;
                    println!(
                        "gemma_manual.layer{}._f16_attn_post_norm_max_abs_diff: {:.9}",
                        manual_graph_f16.layer_index,
                        manual_graph_f16.attn_post_norm_stats.max_abs_diff
                    );
                    println!(
                        "gemma_manual.layer{}._f16_attn_residual_max_abs_diff: {:.9}",
                        manual_graph_f16.layer_index, manual_graph_f16.attn_residual_stats.max_abs_diff
                    );
                    println!(
                        "gemma_manual.layer{}._f16_ffn_norm_max_abs_diff: {:.9}",
                        manual_graph_f16.layer_index,
                        manual_graph_f16.ffn_input_norm_stats.max_abs_diff
                    );
                    println!(
                        "gemma_manual.layer{}._f16_ffn_out_max_abs_diff: {:.9}",
                        manual_graph_f16.layer_index, manual_graph_f16.ffn_out_stats.max_abs_diff
                    );
                    println!(
                        "gemma_manual.layer{}._f16_ffn_post_norm_max_abs_diff: {:.9}",
                        manual_graph_f16.layer_index,
                        manual_graph_f16.ffn_post_norm_stats.max_abs_diff
                    );
                    println!(
                        "gemma_manual.layer{}._f16_pe_in_max_abs_diff: {:.9}",
                        manual_graph_f16.layer_index, manual_graph_f16.pe_in_stats.max_abs_diff
                    );
                    if let Some(per_layer_embd_out) = &manual_graph_f16.per_layer_embd_out_stats {
                        println!(
                            "gemma_manual.layer{}._f16_per_layer_embd_out_max_abs_diff: {:.9}",
                            manual_graph_f16.layer_index, per_layer_embd_out.max_abs_diff
                        );
                    }
                    println!(
                        "gemma_manual.layer{}._f16_post_ffn_max_abs_diff: {:.9}",
                        manual_graph_f16.layer_index, manual_graph_f16.post_ffn_stats.max_abs_diff
                    );
                    let manual_graph_f32 = gemma_layer_manual_graph_check(
                        &model,
                        &upstream.token_ids,
                        layer_check.layer_index,
                        TensorType::F32,
                    )?;
                    println!(
                        "gemma_manual.layer{}._f32_attn_post_norm_max_abs_diff: {:.9}",
                        manual_graph_f32.layer_index,
                        manual_graph_f32.attn_post_norm_stats.max_abs_diff
                    );
                    println!(
                        "gemma_manual.layer{}._f32_attn_residual_max_abs_diff: {:.9}",
                        manual_graph_f32.layer_index, manual_graph_f32.attn_residual_stats.max_abs_diff
                    );
                    println!(
                        "gemma_manual.layer{}._f32_ffn_norm_max_abs_diff: {:.9}",
                        manual_graph_f32.layer_index,
                        manual_graph_f32.ffn_input_norm_stats.max_abs_diff
                    );
                    println!(
                        "gemma_manual.layer{}._f32_ffn_out_max_abs_diff: {:.9}",
                        manual_graph_f32.layer_index, manual_graph_f32.ffn_out_stats.max_abs_diff
                    );
                    println!(
                        "gemma_manual.layer{}._f32_ffn_post_norm_max_abs_diff: {:.9}",
                        manual_graph_f32.layer_index,
                        manual_graph_f32.ffn_post_norm_stats.max_abs_diff
                    );
                    println!(
                        "gemma_manual.layer{}._f32_pe_in_max_abs_diff: {:.9}",
                        manual_graph_f32.layer_index, manual_graph_f32.pe_in_stats.max_abs_diff
                    );
                    if let Some(per_layer_embd_out) = &manual_graph_f32.per_layer_embd_out_stats {
                        println!(
                            "gemma_manual.layer{}._f32_per_layer_embd_out_max_abs_diff: {:.9}",
                            manual_graph_f32.layer_index, per_layer_embd_out.max_abs_diff
                        );
                    }
                    println!(
                        "gemma_manual.layer{}._f32_post_ffn_max_abs_diff: {:.9}",
                        manual_graph_f32.layer_index, manual_graph_f32.post_ffn_stats.max_abs_diff
                    );
                }
                println!(
                    "gemma_upstream.layer{}._attn_out_standalone_max_abs_diff: {:.9}",
                    layer_check.layer_index, standalone_layer_check.attn_residual_stats.max_abs_diff
                );
                println!(
                    "gemma_upstream.layer{}._layer_out_standalone_max_abs_diff: {:.9}",
                    layer_check.layer_index, standalone_layer_out.max_abs_diff
                );
            }
            if gemma_upstream_stack.max_diff_layer != gemma_upstream_stack.first_diff_layer {
                if let Some(layer_index) = gemma_upstream_stack.max_diff_layer {
                    let gemma_tensors = model.gemma4_tensors()?;
                    let gemma_layer = gemma_tensors
                        .layers
                        .iter()
                        .find(|layer| layer.index == layer_index)
                        .ok_or_else(|| format!("missing gemma4 layer {}", layer_index))?;
                    let decode_spec = gemma4_attention_decode_spec(
                        &model,
                        layer_index,
                        u32::try_from(upstream.token_ids.len())?,
                        1,
                        TensorType::F16,
                        TensorType::F16,
                    )?;
                    println!(
                        "gemma_upstream.layer{}._is_swa: {}",
                        layer_index, gemma_layer.is_swa
                    );
                    println!(
                        "gemma_upstream.layer{}._write_kv: {}",
                        layer_index, decode_spec.write_kv
                    );
                    println!(
                        "gemma_upstream.layer{}._cache_layer_index: {}",
                        layer_index, decode_spec.cache_layer_index
                    );
                    if !decode_spec.write_kv {
                        let source_layer_index = decode_spec.cache_layer_index;
                        println!(
                            "gemma_upstream.layer{}._cache_source_layer: {}",
                            layer_index, source_layer_index
                        );
                        let source_self_check = attention_cache_self_check_for_layer(
                            &model,
                            &upstream.token_ids,
                            source_layer_index,
                            TensorType::F32,
                        )?;
                        println!(
                            "gemma_upstream.layer{}._cache_source_selfcheck_same_top1: {}",
                            source_layer_index, source_self_check.same_top1
                        );
                        println!(
                            "gemma_upstream.layer{}._cache_source_selfcheck_max_abs_diff: {:.9}",
                            source_layer_index, source_self_check.hidden_stats.max_abs_diff
                        );
                        let source_cache_check =
                            gemma_hybrid_source_cache_check(&model, &upstream.token_ids, source_layer_index)?;
                        println!(
                            "gemma_upstream.layer{}._cache_source_k_cache_max_abs_diff: {:.9}",
                            source_cache_check.layer_index, source_cache_check.k_cache_stats.max_abs_diff
                        );
                        println!(
                            "gemma_upstream.layer{}._cache_source_v_cache_max_abs_diff: {:.9}",
                            source_cache_check.layer_index, source_cache_check.v_cache_stats.max_abs_diff
                        );
                        let source_layer_check = gemma_upstream_layer_preview_check(
                            &args,
                            &model,
                            &upstream.token_ids,
                            source_layer_index,
                        )?;
                        if let Some(attn_post_norm) = source_layer_check.attn_post_norm_stats {
                            println!(
                                "gemma_upstream.layer{}._cache_source_attn_post_norm_max_abs_diff: {:.9}",
                                source_layer_index, attn_post_norm.max_abs_diff
                            );
                        }
                        println!(
                            "gemma_upstream.layer{}._cache_source_attn_out_max_abs_diff: {:.9}",
                            source_layer_index, source_layer_check.attn_residual_stats.max_abs_diff
                        );
                        println!(
                            "gemma_upstream.layer{}._cache_source_layer_out_max_abs_diff: {:.9}",
                            source_layer_index, source_layer_check.post_ffn_stats.max_abs_diff
                        );
                        for late_layer_index in source_layer_index..=layer_index {
                            if let Some(layer_stats) =
                                gemma_upstream_stack.layer_post_ffn_stats.get(&late_layer_index)
                            {
                                println!(
                                    "gemma_upstream.layer{}._stack_layer_out_max_abs_diff: {:.9}",
                                    late_layer_index, layer_stats.max_abs_diff
                                );
                            }
                        }
                        let first_large_shared_diff_layer = (source_layer_index + 1..=layer_index)
                            .find(|late_layer_index| {
                                gemma_upstream_stack
                                    .layer_post_ffn_stats
                                    .get(late_layer_index)
                                    .map(|layer_stats| layer_stats.max_abs_diff > 0.01)
                                    .unwrap_or(false)
                            });
                        if let Some(first_large_shared_diff_layer) = first_large_shared_diff_layer {
                            println!(
                                "gemma_upstream.first_large_shared_diff_layer: {}",
                                first_large_shared_diff_layer
                            );
                            let first_large_layer = gemma_tensors
                                .layers
                                .iter()
                                .find(|layer| layer.index == first_large_shared_diff_layer)
                                .ok_or_else(|| {
                                    format!(
                                        "missing gemma4 first large shared diff layer {}",
                                        first_large_shared_diff_layer
                                    )
                                })?;
                            let first_large_decode_spec = gemma4_attention_decode_spec(
                                &model,
                                first_large_shared_diff_layer,
                                u32::try_from(upstream.token_ids.len())?,
                                1,
                                TensorType::F16,
                                TensorType::F16,
                            )?;
                            println!(
                                "gemma_upstream.layer{}._is_swa: {}",
                                first_large_shared_diff_layer, first_large_layer.is_swa
                            );
                            println!(
                                "gemma_upstream.layer{}._write_kv: {}",
                                first_large_shared_diff_layer, first_large_decode_spec.write_kv
                            );
                            println!(
                                "gemma_upstream.layer{}._cache_layer_index: {}",
                                first_large_shared_diff_layer,
                                first_large_decode_spec.cache_layer_index
                            );
                            let first_large_layer_check = gemma_upstream_layer_preview_check(
                                &args,
                                &model,
                                &upstream.token_ids,
                                first_large_shared_diff_layer,
                            )?;
                            if let Some(attn_post_norm) =
                                first_large_layer_check.attn_post_norm_stats
                            {
                                println!(
                                    "gemma_upstream.layer{}._attn_post_norm_max_abs_diff: {:.9}",
                                    first_large_shared_diff_layer, attn_post_norm.max_abs_diff
                                );
                            }
                            println!(
                                "gemma_upstream.layer{}._attn_out_max_abs_diff: {:.9}",
                                first_large_shared_diff_layer,
                                first_large_layer_check.attn_residual_stats.max_abs_diff
                            );
                            println!(
                                "gemma_upstream.layer{}._layer_out_max_abs_diff: {:.9}",
                                first_large_shared_diff_layer,
                                first_large_layer_check.post_ffn_stats.max_abs_diff
                            );
                        }

                        let first_shared_layer_index = source_layer_index + 1;
                        if gemma_tensors
                            .layers
                            .iter()
                            .any(|layer| layer.index == first_shared_layer_index)
                        {
                            let first_shared_layer = gemma_tensors
                                .layers
                                .iter()
                                .find(|layer| layer.index == first_shared_layer_index)
                                .ok_or_else(|| {
                                    format!(
                                        "missing gemma4 first shared layer {}",
                                        first_shared_layer_index
                                    )
                                })?;
                            let first_shared_decode_spec = gemma4_attention_decode_spec(
                                &model,
                                first_shared_layer_index,
                                u32::try_from(upstream.token_ids.len())?,
                                1,
                                TensorType::F16,
                                TensorType::F16,
                            )?;
                            println!(
                                "gemma_upstream.layer{}._is_swa: {}",
                                first_shared_layer_index, first_shared_layer.is_swa
                            );
                            println!(
                                "gemma_upstream.layer{}._write_kv: {}",
                                first_shared_layer_index, first_shared_decode_spec.write_kv
                            );
                            println!(
                                "gemma_upstream.layer{}._cache_layer_index: {}",
                                first_shared_layer_index,
                                first_shared_decode_spec.cache_layer_index
                            );
                            let first_shared_layer_check = gemma_upstream_layer_preview_check(
                                &args,
                                &model,
                                &upstream.token_ids,
                                first_shared_layer_index,
                            )?;
                            if let Some(attn_post_norm) =
                                first_shared_layer_check.attn_post_norm_stats
                            {
                                println!(
                                    "gemma_upstream.layer{}._attn_post_norm_max_abs_diff: {:.9}",
                                    first_shared_layer_index, attn_post_norm.max_abs_diff
                                );
                            }
                            println!(
                                "gemma_upstream.layer{}._attn_out_max_abs_diff: {:.9}",
                                first_shared_layer_index,
                                first_shared_layer_check.attn_residual_stats.max_abs_diff
                            );
                            println!(
                                "gemma_upstream.layer{}._layer_out_max_abs_diff: {:.9}",
                                first_shared_layer_index,
                                first_shared_layer_check.post_ffn_stats.max_abs_diff
                            );
                        }
                    }
                    let layer_check =
                        gemma_upstream_layer_preview_check(&args, &model, &upstream.token_ids, layer_index)?;
                    if let Some(attn_post_norm) = layer_check.attn_post_norm_stats {
                        println!(
                            "gemma_upstream.layer{}._attn_post_norm_max_abs_diff: {:.9}",
                            layer_check.layer_index, attn_post_norm.max_abs_diff
                        );
                    }
                    println!(
                        "gemma_upstream.layer{}._attn_out_max_abs_diff: {:.9}",
                        layer_check.layer_index, layer_check.attn_residual_stats.max_abs_diff
                    );
                    if let Some(ffn_input_norm) = layer_check.ffn_input_norm_stats {
                        println!(
                            "gemma_upstream.layer{}._ffn_norm_max_abs_diff: {:.9}",
                            layer_check.layer_index, ffn_input_norm.max_abs_diff
                        );
                    }
                    println!(
                        "gemma_upstream.layer{}._ffn_out_max_abs_diff: {:.9}",
                        layer_check.layer_index, layer_check.ffn_out_stats.max_abs_diff
                    );
                    if let Some(ffn_post_norm) = layer_check.ffn_post_norm_stats {
                        println!(
                            "gemma_upstream.layer{}._ffn_post_norm_max_abs_diff: {:.9}",
                            layer_check.layer_index, ffn_post_norm.max_abs_diff
                        );
                    }
                    if let Some(pe_in) = layer_check.pe_in_stats {
                        println!(
                            "gemma_upstream.layer{}._pe_in_max_abs_diff: {:.9}",
                            layer_check.layer_index, pe_in.max_abs_diff
                        );
                    }
                    if let Some(per_layer_embd_out) = layer_check.per_layer_embd_out_stats {
                        println!(
                            "gemma_upstream.layer{}._per_layer_embd_out_max_abs_diff: {:.9}",
                            layer_check.layer_index, per_layer_embd_out.max_abs_diff
                        );
                    }
                    println!(
                        "gemma_upstream.layer{}._layer_out_max_abs_diff: {:.9}",
                        layer_check.layer_index, layer_check.post_ffn_stats.max_abs_diff
                    );
                }
            }
        }
        if architecture_has_recurrent_layers(&model.architecture) {
            let recurrent_check = recurrent_cache_self_check(&model, &upstream.token_ids[..2])?;
            let recurrent_step_cpu_base =
                recurrent_step_cpu_check(&model, &upstream.token_ids[..2], None)?;
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
                recurrent_step_cpu_base.layer_index,
                recurrent_step_cpu_base.conv_output_cpu_stats.max_abs_diff
            );
            println!(
                "recurrent_step_cpu.layer{}._q_conv_max_abs_diff: {:.9}",
                recurrent_step_cpu_base.layer_index,
                recurrent_step_cpu_base.q_conv_cpu_stats.max_abs_diff
            );
            println!(
                "recurrent_step_cpu.layer{}._k_conv_max_abs_diff: {:.9}",
                recurrent_step_cpu_base.layer_index,
                recurrent_step_cpu_base.k_conv_cpu_stats.max_abs_diff
            );
            println!(
                "recurrent_step_cpu.layer{}._output_view_max_abs_diff: {:.9}",
                recurrent_step_cpu_base.layer_index,
                recurrent_step_cpu_base.output_view_cpu_stats.max_abs_diff
            );
        }
        if matches!(model.architecture, LlamaArchitecture::Qwen35) {
            let recurrent_step_cpu_layer8 =
                recurrent_step_cpu_check(&model, &upstream.token_ids[..2], Some(8))?;
            println!(
                "recurrent_step_cpu.layer{}._conv_output_max_abs_diff: {:.9}",
                recurrent_step_cpu_layer8.layer_index,
                recurrent_step_cpu_layer8.conv_output_cpu_stats.max_abs_diff
            );
            println!(
                "recurrent_step_cpu.layer{}._output_view_max_abs_diff: {:.9}",
                recurrent_step_cpu_layer8.layer_index,
                recurrent_step_cpu_layer8.output_view_cpu_stats.max_abs_diff
            );
            let recurrent_hidden_layer8 =
                recurrent_from_hidden_batch_self_check(&model, &upstream.token_ids[..2], 7, 8)?;
            println!(
                "recurrent_hidden.layer{}._input_layer{}._hidden_max_abs_diff: {:.9}",
                recurrent_hidden_layer8.layer_index,
                recurrent_hidden_layer8.source_layer_index,
                recurrent_hidden_layer8.hidden_stats.max_abs_diff
            );
            println!(
                "recurrent_hidden.layer{}._input_layer{}._r_cache_max_abs_diff: {:.9}",
                recurrent_hidden_layer8.layer_index,
                recurrent_hidden_layer8.source_layer_index,
                recurrent_hidden_layer8.r_cache_stats.max_abs_diff
            );
            println!(
                "recurrent_hidden.layer{}._input_layer{}._s_cache_max_abs_diff: {:.9}",
                recurrent_hidden_layer8.layer_index,
                recurrent_hidden_layer8.source_layer_index,
                recurrent_hidden_layer8.s_cache_stats.max_abs_diff
            );
            let attention_hidden_layer7 =
                attention_from_hidden_batch_self_check(&model, &upstream.token_ids[..2], 6, 7)?;
            println!(
                "attention_hidden.layer{}._input_layer{}._hidden_max_abs_diff: {:.9}",
                attention_hidden_layer7.layer_index,
                attention_hidden_layer7.source_layer_index,
                attention_hidden_layer7.hidden_stats.max_abs_diff
            );
            println!(
                "attention_hidden.layer{}._input_layer{}._result_output_max_abs_diff: {:.9}",
                attention_hidden_layer7.layer_index,
                attention_hidden_layer7.source_layer_index,
                attention_hidden_layer7.result_output_stats.max_abs_diff
            );
            println!(
                "attention_hidden.layer{}._input_layer{}._result_output_token0_max_abs_diff: {:.9}",
                attention_hidden_layer7.layer_index,
                attention_hidden_layer7.source_layer_index,
                attention_hidden_layer7
                    .first_token_result_output_stats
                    .max_abs_diff
            );
            println!(
                "attention_hidden.layer{}._input_layer{}._result_output_token1_max_abs_diff: {:.9}",
                attention_hidden_layer7.layer_index,
                attention_hidden_layer7.source_layer_index,
                attention_hidden_layer7
                    .last_token_result_output_stats
                    .max_abs_diff
            );
            println!(
                "attention_hidden.layer{}._input_layer{}._k_cache_max_abs_diff: {:.9}",
                attention_hidden_layer7.layer_index,
                attention_hidden_layer7.source_layer_index,
                attention_hidden_layer7.k_cache_stats.max_abs_diff
            );
            println!(
                "attention_hidden.layer{}._input_layer{}._step0_k_cache_row_max_abs_diff: {:.9}",
                attention_hidden_layer7.layer_index,
                attention_hidden_layer7.source_layer_index,
                attention_hidden_layer7.step0_k_cache_row_stats.max_abs_diff
            );
            println!(
                "attention_hidden.layer{}._input_layer{}._step0_k_cache_tail_zero_max_abs_diff: {:.9}",
                attention_hidden_layer7.layer_index,
                attention_hidden_layer7.source_layer_index,
                attention_hidden_layer7
                    .step0_k_cache_tail_zero_stats
                    .max_abs_diff
            );
            println!(
                "attention_hidden.layer{}._input_layer{}._v_cache_max_abs_diff: {:.9}",
                attention_hidden_layer7.layer_index,
                attention_hidden_layer7.source_layer_index,
                attention_hidden_layer7.v_cache_stats.max_abs_diff
            );
            println!(
                "attention_hidden.layer{}._input_layer{}._step0_v_cache_row_max_abs_diff: {:.9}",
                attention_hidden_layer7.layer_index,
                attention_hidden_layer7.source_layer_index,
                attention_hidden_layer7.step0_v_cache_row_stats.max_abs_diff
            );
            println!(
            "attention_hidden.layer{}._input_layer{}._step0_v_cache_tail_zero_max_abs_diff: {:.9}",
            attention_hidden_layer7.layer_index,
            attention_hidden_layer7.source_layer_index,
            attention_hidden_layer7
                .step0_v_cache_tail_zero_stats
                .max_abs_diff
        );
            let attention_tensor_check =
                attention_cache_tensor_check(&model, &upstream.token_ids[..2])?;
            println!(
                "attention_tensor.layer{}._q_proj_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index,
                attention_tensor_check.q_proj_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._q_pre_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index, attention_tensor_check.q_pre_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._q_norm_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index,
                attention_tensor_check.q_norm_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._k_norm_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index,
                attention_tensor_check.k_norm_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._q_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index, attention_tensor_check.q_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._k_store_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index,
                attention_tensor_check.k_store_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._v_store_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index,
                attention_tensor_check.v_store_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._k_cache_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index,
                attention_tensor_check.k_cache_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._v_cache_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index,
                attention_tensor_check.v_cache_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._attn_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index, attention_tensor_check.attn_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._isolated_attn_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index,
                attention_tensor_check.isolated_attn_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._output_proj_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index,
                attention_tensor_check.output_proj_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._result_output_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index,
                attention_tensor_check.result_output_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._full_attn_cpu_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index,
                attention_tensor_check.full_attn_cpu_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._isolated_attn_cpu_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index,
                attention_tensor_check.isolated_attn_cpu_stats.max_abs_diff
            );
            println!(
                "attention_tensor.layer{}._decode_attn_cpu_max_abs_diff: {:.9}",
                attention_tensor_check.layer_index,
                attention_tensor_check.decode_attn_cpu_stats.max_abs_diff
            );
            let attention_decode_batched_tensor_check =
                attention_decode_batched_tensor_check(&model, &upstream.token_ids[..2])?;
            println!(
                "attention_decode_batched_tensor.layer{}._q_proj_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check
                    .q_proj_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_batched_tensor.layer{}._q_pre_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check
                    .q_pre_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_batched_tensor.layer{}._q_norm_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check
                    .q_norm_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_batched_tensor.layer{}._k_norm_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check
                    .k_norm_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_batched_tensor.layer{}._q_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check.q_stats.max_abs_diff
            );
            println!(
                "attention_decode_batched_tensor.layer{}._k_store_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check
                    .k_store_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_batched_tensor.layer{}._v_store_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check
                    .v_store_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_batched_tensor.layer{}._k_cache_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check
                    .k_cache_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_batched_tensor.layer{}._v_cache_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check
                    .v_cache_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_batched_tensor.layer{}._k_cache_view_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check
                    .k_cache_view_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_batched_tensor.layer{}._v_cache_view_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check
                    .v_cache_view_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_batched_tensor.layer{}._attn_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check
                    .attn_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_batched_tensor.layer{}._output_proj_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check
                    .output_proj_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_batched_tensor.layer{}._result_output_max_abs_diff: {:.9}",
                attention_decode_batched_tensor_check.layer_index,
                attention_decode_batched_tensor_check
                    .result_output_stats
                    .max_abs_diff
            );
            let attention_decode_stepwise_tensor_check =
                attention_decode_stepwise_tensor_check(&model, &upstream.token_ids[..2])?;
            println!(
                "attention_decode_stepwise_tensor.layer{}._q_proj_max_abs_diff: {:.9}",
                attention_decode_stepwise_tensor_check.layer_index,
                attention_decode_stepwise_tensor_check
                    .q_proj_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_stepwise_tensor.layer{}._q_pre_max_abs_diff: {:.9}",
                attention_decode_stepwise_tensor_check.layer_index,
                attention_decode_stepwise_tensor_check
                    .q_pre_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_stepwise_tensor.layer{}._q_norm_max_abs_diff: {:.9}",
                attention_decode_stepwise_tensor_check.layer_index,
                attention_decode_stepwise_tensor_check
                    .q_norm_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_stepwise_tensor.layer{}._k_norm_max_abs_diff: {:.9}",
                attention_decode_stepwise_tensor_check.layer_index,
                attention_decode_stepwise_tensor_check
                    .k_norm_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_stepwise_tensor.layer{}._q_max_abs_diff: {:.9}",
                attention_decode_stepwise_tensor_check.layer_index,
                attention_decode_stepwise_tensor_check.q_stats.max_abs_diff
            );
            println!(
                "attention_decode_stepwise_tensor.layer{}._k_store_max_abs_diff: {:.9}",
                attention_decode_stepwise_tensor_check.layer_index,
                attention_decode_stepwise_tensor_check
                    .k_store_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_stepwise_tensor.layer{}._v_store_max_abs_diff: {:.9}",
                attention_decode_stepwise_tensor_check.layer_index,
                attention_decode_stepwise_tensor_check
                    .v_store_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_stepwise_tensor.layer{}._k_cache_max_abs_diff: {:.9}",
                attention_decode_stepwise_tensor_check.layer_index,
                attention_decode_stepwise_tensor_check
                    .k_cache_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_stepwise_tensor.layer{}._v_cache_max_abs_diff: {:.9}",
                attention_decode_stepwise_tensor_check.layer_index,
                attention_decode_stepwise_tensor_check
                    .v_cache_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_stepwise_tensor.layer{}._attn_max_abs_diff: {:.9}",
                attention_decode_stepwise_tensor_check.layer_index,
                attention_decode_stepwise_tensor_check
                    .attn_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_stepwise_tensor.layer{}._output_proj_max_abs_diff: {:.9}",
                attention_decode_stepwise_tensor_check.layer_index,
                attention_decode_stepwise_tensor_check
                    .output_proj_stats
                    .max_abs_diff
            );
            println!(
                "attention_decode_stepwise_tensor.layer{}._result_output_max_abs_diff: {:.9}",
                attention_decode_stepwise_tensor_check.layer_index,
                attention_decode_stepwise_tensor_check
                    .result_output_stats
                    .max_abs_diff
            );
        }
        if model.architecture != LlamaArchitecture::Qwen35Moe {
            return Ok(());
        }
        let recurrent_tensor_check = recurrent_tensor_check(&model, &upstream.token_ids[..2])?;
        println!(
            "recurrent_tensor.layer{}._input_embed_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index,
            recurrent_tensor_check.input_embed_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._input_norm_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index,
            recurrent_tensor_check.input_norm_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._qkv_mixed_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index, recurrent_tensor_check.qkv_mixed_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._qkv_mixed_full_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index,
            recurrent_tensor_check.qkv_mixed_full_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._qkv_mixed_transposed_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index,
            recurrent_tensor_check
                .qkv_mixed_transposed_stats
                .max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._conv_states_reshaped_zero_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index,
            recurrent_tensor_check
                .conv_states_reshaped_zero_stats
                .max_abs_diff
        );
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
            "recurrent_tensor.layer{}._conv_input_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index,
            recurrent_tensor_check.conv_input_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._q_conv_predelta_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index,
            recurrent_tensor_check.q_conv_predelta_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._k_conv_predelta_max_abs_diff: {:.9}",
            recurrent_tensor_check.layer_index,
            recurrent_tensor_check.k_conv_predelta_stats.max_abs_diff
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
        match recurrent_upstream_preview_check(&args, &model, &upstream.token_ids[..2]) {
            Ok(recurrent_upstream_check) => {
                println!(
                    "recurrent_upstream.layer{}._input_norm_max_abs_diff: {:.9}",
                    recurrent_upstream_check.layer_index,
                    recurrent_upstream_check.input_norm_stats.max_abs_diff
                );
                println!(
                    "recurrent_upstream.layer{}._input_norm_cpu_max_abs_diff: {:.9}",
                    recurrent_upstream_check.layer_index,
                    recurrent_upstream_check.input_norm_cpu_stats.max_abs_diff
                );
                println!(
                    "recurrent_upstream.layer{}._final_output_max_abs_diff: {:.9}",
                    recurrent_upstream_check.layer_index,
                    recurrent_upstream_check.final_output_stats.max_abs_diff
                );
                println!(
                    "recurrent_upstream.layer{}._linear_attn_out_max_abs_diff: {:.9}",
                    recurrent_upstream_check.layer_index,
                    recurrent_upstream_check.linear_attn_out_stats.max_abs_diff
                );
                println!(
                    "recurrent_upstream.layer{}._attn_residual_max_abs_diff: {:.9}",
                    recurrent_upstream_check.layer_index,
                    recurrent_upstream_check.attn_residual_stats.max_abs_diff
                );
            }
            Err(err) => {
                println!("recurrent_upstream.error: {}", err);
            }
        }
        match moe_preview_check(&args, &model, &upstream.token_ids[..2]) {
            Ok(moe_preview_check) => {
                println!(
                    "moe_preview.layer{}._router_weight_type: {}",
                    moe_preview_check.layer_index, moe_preview_check.router_weight_type
                );
                println!(
                    "moe_preview.layer{}._router_weight_dims: {:?}",
                    moe_preview_check.layer_index, moe_preview_check.router_weight_dims
                );
                println!(
                    "moe_preview.layer{}._router_weight_offset: {:?}",
                    moe_preview_check.layer_index, moe_preview_check.router_weight_offset
                );
                println!(
                    "moe_preview.layer{}._router_weight_strides: {:?}",
                    moe_preview_check.layer_index, moe_preview_check.router_weight_strides
                );
                println!(
                    "moe_preview.layer{}._router_weight_is_transposed: {}",
                    moe_preview_check.layer_index, moe_preview_check.router_weight_is_transposed
                );
                println!(
                    "moe_preview.layer{}._router_weight_is_permuted: {}",
                    moe_preview_check.layer_index, moe_preview_check.router_weight_is_permuted
                );
                println!(
                    "moe_preview.layer{}._router_weight_is_contiguous: {}",
                    moe_preview_check.layer_index, moe_preview_check.router_weight_is_contiguous
                );
                println!(
                    "moe_preview.layer{}._router_weight_is_view: {}",
                    moe_preview_check.layer_index, moe_preview_check.router_weight_is_view
                );
                println!(
                    "moe_preview.layer{}._attn_residual_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index,
                    moe_preview_check.attn_residual_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._attn_residual_sum_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.attn_residual_sum_diff
                );
                println!(
                    "moe_preview.layer{}._input_norm_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.input_norm_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._input_norm_sum_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.input_norm_sum_diff
                );
                println!(
                    "moe_preview.layer{}._input_norm_cpu_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index,
                    moe_preview_check.input_norm_cpu_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._router_logits_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index,
                    moe_preview_check.router_logits_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._router_logits_sum_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.router_logits_sum_diff
                );
                println!(
                    "moe_preview.layer{}._router_logits_cpu_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index,
                    moe_preview_check.router_logits_cpu_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._router_logits_tensor_cpu_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index,
                    moe_preview_check
                        .router_logits_tensor_cpu_stats
                        .max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._router_logits_isolated_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index,
                    moe_preview_check.router_logits_isolated_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._router_logits_isolated_cpu_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index,
                    moe_preview_check
                        .router_logits_isolated_cpu_stats
                        .max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._router_logits_cloned_loaded_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index,
                    moe_preview_check
                        .router_logits_cloned_loaded_stats
                        .max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._router_logits_cloned_cpu_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index,
                    moe_preview_check
                        .router_logits_cloned_cpu_stats
                        .max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._router_probs_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index,
                    moe_preview_check.router_probs_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._router_probs_sum_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.router_probs_sum_diff
                );
                println!(
                    "moe_preview.layer{}._selected_experts_match_cpu: {}",
                    moe_preview_check.layer_index, moe_preview_check.selected_experts_match_cpu
                );
                println!(
                    "moe_preview.layer{}._selected_experts_diff_count: {}",
                    moe_preview_check.layer_index, moe_preview_check.selected_experts_diff_count
                );
                println!(
                    "moe_preview.layer{}._selected_experts_match_upstream: {}",
                    moe_preview_check.layer_index,
                    moe_preview_check.selected_experts_match_upstream
                );
                println!(
                    "moe_preview.layer{}._selected_experts_upstream_diff_count: {}",
                    moe_preview_check.layer_index,
                    moe_preview_check.selected_experts_upstream_diff_count
                );
                println!(
                    "moe_preview.layer{}._selected_experts_upstream_set_diff_count: {}",
                    moe_preview_check.layer_index,
                    moe_preview_check.selected_experts_upstream_set_diff_count
                );
                println!(
                    "moe_preview.layer{}._min_topk_margin: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.min_topk_margin
                );
                println!(
                    "moe_preview.layer{}._weights_norm_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index,
                    moe_preview_check.weights_norm_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._weights_norm_sum_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.weights_norm_sum_diff
                );
                println!(
                    "moe_preview.layer{}._up_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.up_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._up_sum_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.up_sum_diff
                );
                println!(
                    "moe_preview.layer{}._down_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.down_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._down_sum_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.down_sum_diff
                );
                println!(
                    "moe_preview.layer{}._weighted_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.weighted_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._weighted_sum_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.weighted_sum_diff
                );
                println!(
                    "moe_preview.layer{}._moe_out_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.moe_out_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._moe_out_sum_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.moe_out_sum_diff
                );
                println!(
                    "moe_preview.layer{}._shared_gated_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index,
                    moe_preview_check.shared_gated_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._shared_gated_sum_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.shared_gated_sum_diff
                );
                println!(
                    "moe_preview.layer{}._ffn_out_max_abs_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.ffn_out_stats.max_abs_diff
                );
                println!(
                    "moe_preview.layer{}._ffn_out_sum_diff: {:.9}",
                    moe_preview_check.layer_index, moe_preview_check.ffn_out_sum_diff
                );
            }
            Err(err) => {
                println!("moe_preview.error: {}", err);
            }
        }
    }

    if let (Some((rust_top1, rust_logit)), Some((upstream_top1, upstream_logit))) =
        (rust_top.first(), upstream_top.first())
    {
        println!("compare.top1.rust_id: {}", rust_top1);
        println!("compare.top1.upstream_id: {}", upstream_top1);
        println!("compare.top1.rust_logit: {:.9}", rust_logit);
        println!("compare.top1.upstream_logit: {:.9}", upstream_logit);
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
    run_upstream_debug_with_mode(args, UpstreamDebugMode::Batched)
}

fn run_upstream_step_debug(args: &Args) -> Result<UpstreamReference, Box<dyn std::error::Error>> {
    run_upstream_debug_with_mode(args, UpstreamDebugMode::Stepwise)
}

fn run_upstream_debug_with_mode(
    args: &Args,
    mode: UpstreamDebugMode,
) -> Result<UpstreamReference, Box<dyn std::error::Error>> {
    let output_dir = std::env::temp_dir().join(format!(
        "makepad-llama-compare-{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        mode.output_dir_suffix()
    ));
    fs::create_dir_all(&output_dir)?;

    let gpu_output = run_upstream_debug_command(args, &output_dir, "999", mode)?;
    if !gpu_output.status.success() {
        fs::remove_dir_all(&output_dir)?;
        fs::create_dir_all(&output_dir)?;
        let cpu_output = run_upstream_debug_command(args, &output_dir, "0", mode)?;
        ensure_success("llama-debug", &cpu_output)?;
    }

    let model_name = upstream_model_name(args)?;
    let logits_path = output_dir.join(format!("llamacpp-{model_name}.bin"));
    let token_ids_path = output_dir.join(format!("llamacpp-{model_name}-tokens.bin"));
    let logits = read_f32_file(&logits_path)?;
    let token_ids = read_i32_file(&token_ids_path)?;
    let step_logits = if mode.is_stepwise() {
        read_upstream_step_logits(&output_dir, model_name)?
    } else {
        Vec::new()
    };

    Ok(UpstreamReference {
        token_ids,
        logits,
        step_logits,
        output_dir,
    })
}

fn run_upstream_debug_command(
    args: &Args,
    output_dir: &Path,
    n_gpu_layers: &str,
    mode: UpstreamDebugMode,
) -> Result<Output, Box<dyn std::error::Error>> {
    let mut command = Command::new(&args.upstream_debug_path);
    command
        .arg("-m")
        .arg(&args.model_path)
        .arg("-p")
        .arg(&args.prompt)
        .arg("--tensor-filter")
        .arg("result_output")
        .arg("--save-logits")
        .arg("--logits-output-dir")
        .arg(output_dir)
        .arg("-ngl")
        .arg(n_gpu_layers)
        .arg("-fa")
        .arg("1")
        .arg("-ctk")
        .arg("f16")
        .arg("-ctv")
        .arg("f16");
    if n_gpu_layers == "0" {
        command.arg("-dev").arg("none");
    }
    if mode.is_stepwise() {
        command
            .env("MAKEPAD_LLAMA_DEBUG_STEP_PROMPT", "1")
            .env("MAKEPAD_LLAMA_DEBUG_SAVE_STEP_LOGITS", "1");
    }
    Ok(command.output()?)
}

fn run_rust_hybrid_decode(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    run_rust_session_hybrid_decode(model, token_ids, 1)
}

fn run_rust_session_hybrid_decode(
    model: &LlamaModel,
    token_ids: &[i32],
    prefill_batch_size: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("upstream prompt produced no tokens".into());
    }

    let mut session = LlamaSession::from_model(
        model,
        LlamaSessionConfig {
            max_context: Some(u32::try_from(token_ids.len())?),
            prefill_batch_size,
            ..LlamaSessionConfig::default()
        },
    )?;
    session.append_tokens(token_ids)?;
    session
        .last_logits()
        .map(|logits| logits.to_vec())
        .ok_or_else(|| "session did not produce logits".into())
}

fn run_rust_hybrid_decode_stepwise(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("upstream prompt produced no tokens".into());
    }

    let mut session = LlamaSession::from_model(
        model,
        LlamaSessionConfig {
            max_context: Some(u32::try_from(token_ids.len())?),
            prefill_batch_size: 1,
            ..LlamaSessionConfig::default()
        },
    )?;
    let mut step_logits = Vec::with_capacity(token_ids.len());
    for &token_id in token_ids {
        session.append_token(token_id)?;
        let logits = session
            .last_logits()
            .ok_or_else(|| "stepwise session did not produce logits".to_owned())?;
        step_logits.push(logits.to_vec());
    }
    Ok(step_logits)
}

fn run_rust_hybrid_decode_batched(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    run_rust_session_hybrid_decode(model, token_ids, token_ids.len())
}

fn upstream_model_name(args: &Args) -> Result<&str, Box<dyn std::error::Error>> {
    args.model_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| "failed to derive model stem for upstream output files".into())
}

fn read_upstream_step_logits(
    output_dir: &Path,
    model_name: &str,
) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
    let prefix = format!("llamacpp-{model_name}-step-");
    let mut step_paths = Vec::new();
    for entry in fs::read_dir(output_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let Some(index_text) = file_name
            .strip_prefix(&prefix)
            .and_then(|name| name.strip_suffix(".bin"))
        else {
            continue;
        };
        let step_index = index_text.parse::<usize>()?;
        step_paths.push((step_index, entry.path()));
    }
    step_paths.sort_by_key(|(step_index, _)| *step_index);
    if step_paths.is_empty() {
        return Err(format!(
            "no upstream step logits found in '{}'",
            output_dir.display()
        )
        .into());
    }
    for (expected_index, (step_index, _)) in step_paths.iter().enumerate() {
        if *step_index != expected_index {
            return Err(format!(
                "missing upstream step logits {} in '{}'",
                expected_index,
                output_dir.display()
            )
            .into());
        }
    }
    step_paths
        .into_iter()
        .map(|(_, path)| read_f32_file(&path))
        .collect()
}

fn run_rust_session_prefill_then_continue(
    model: &LlamaModel,
    prompt_token_ids: &[i32],
    continue_token_id: i32,
    prefill_batch_size: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if prompt_token_ids.is_empty() {
        return Err("cannot continue from an empty prompt".into());
    }

    let max_context = prompt_token_ids
        .len()
        .checked_add(1)
        .ok_or("overflow computing continued session context")?;
    let mut session = LlamaSession::from_model(
        model,
        LlamaSessionConfig {
            max_context: Some(u32::try_from(max_context)?),
            prefill_batch_size,
            ..LlamaSessionConfig::default()
        },
    )?;
    session.append_tokens(prompt_token_ids)?;
    session.append_token(continue_token_id)?;
    session
        .last_logits()
        .map(|logits| logits.to_vec())
        .ok_or_else(|| "continued session did not produce logits".into())
}

fn build_shared_hybrid_debug_env(
    model: &LlamaModel,
    max_context: u32,
    attention_k_type: TensorType,
    attention_v_type: TensorType,
) -> Result<SharedHybridDebugEnv, Box<dyn std::error::Error>> {
    let plan = model.execution_plan()?;
    let mut weights = plan
        .full_weights
        .allocate_and_load_with_extra(&model.gguf, COMPARE_SHARED_HYBRID_EXTRA_CONTEXT_BYTES)?;
    let spec = model.hybrid_decode_spec(
        max_context,
        1,
        attention_k_type,
        attention_v_type,
        TensorType::F32,
        TensorType::F32,
    )?;
    let shared_runtime = MetalRuntime::new()?;
    let shared_cache =
        allocate_hybrid_shared_cache_tensors(&mut weights.ctx, &weights.tensor_ids, &spec)?;
    let shared_main_buffer =
        create_metal_context_buffer_with_runtime(&shared_runtime, &weights.ctx)?;
    Ok(SharedHybridDebugEnv {
        weights,
        spec,
        shared_runtime,
        shared_main_buffer,
        shared_cache,
    })
}

fn run_shared_hybrid_graph(
    env: &mut SharedHybridDebugEnv,
    token_ids: &[i32],
    positions: &[i32],
    cache_tokens: usize,
    output_ids: &[i32],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let mut compiled =
        compile_hybrid_decode_metal_with_shared_runtime_and_state_and_outputs_and_attention_key_count(
        &mut env.weights,
        &env.spec,
        &env.shared_runtime,
        &env.shared_cache,
        &env.shared_main_buffer,
        token_ids.len(),
        output_ids.len(),
        cache_tokens,
    )?;
    let mut layout = HybridDecodeBatchLayout::from_contiguous_positions_and_outputs(
        positions,
        cache_tokens,
        output_ids,
    )?;
    if compiled.decode().input_recurrent_state_rows.is_none() {
        layout.recurrent_state_rows.clear();
    }
    let run =
        compiled.execute_logits_only_with_layout(LogitsProbeInput::TokenIds(token_ids), &layout)?;
    Ok(run.logits)
}

fn read_shared_tensor_values_f32(
    runtime: &MetalRuntime,
    main_buffer: &MetalBuffer,
    ctx: &Context,
    tensor_id: TensorId,
    len_bytes: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid shared tensor id {}", tensor_id))?;
    let offset = tensor
        .data_offset
        .ok_or_else(|| format!("shared tensor {} is missing data_offset", tensor_id))?;
    let bytes = runtime.read_buffer_range(main_buffer, offset, len_bytes)?;
    match tensor.desc.ty {
        TensorType::F32 => Ok(bytes
            .chunks_exact(std::mem::size_of::<f32>())
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect()),
        TensorType::F16 => Ok(bytes
            .chunks_exact(std::mem::size_of::<u16>())
            .map(|chunk| f16_to_f32(u16::from_le_bytes(chunk.try_into().unwrap())))
            .collect()),
        other => Err(format!(
            "shared tensor '{}' uses unsupported compare type {}",
            tensor.name().unwrap_or("<unnamed>"),
            other.name()
        )
        .into()),
    }
}

fn read_attention_cache_prefix_values_f32(
    runtime: &MetalRuntime,
    main_buffer: &MetalBuffer,
    ctx: &Context,
    tensor_id: TensorId,
    cache_tokens: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid attention cache tensor id {}", tensor_id))?;
    if tensor.ne[1] <= 0 {
        return Err(format!("attention cache tensor {} has invalid ne[1]", tensor_id).into());
    }
    let row_bytes = usize::try_from(tensor.nb[1])?;
    let len_bytes = row_bytes
        .checked_mul(cache_tokens)
        .ok_or("overflow computing attention cache prefix byte length")?;
    read_shared_tensor_values_f32(runtime, main_buffer, ctx, tensor_id, len_bytes)
}

fn read_full_tensor_values_f32(
    runtime: &MetalRuntime,
    main_buffer: &MetalBuffer,
    ctx: &Context,
    tensor_id: TensorId,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid tensor id {}", tensor_id))?;
    read_shared_tensor_values_f32(runtime, main_buffer, ctx, tensor_id, tensor.nbytes())
}

fn update_layer_diff_summary(
    summary: &mut LayerDiffSummary,
    layer_index: u32,
    stats: LogitComparison,
) {
    if stats.max_abs_diff > summary.max_abs_diff {
        summary.max_abs_diff = stats.max_abs_diff;
        summary.layer_index = Some(layer_index);
    }
}

fn capture_shared_hybrid_cache_snapshot(
    env: &SharedHybridDebugEnv,
    cache_tokens: usize,
) -> Result<HybridCacheSnapshot, Box<dyn std::error::Error>> {
    let mut snapshot = HybridCacheSnapshot::default();

    for (&layer_index, ids) in &env.shared_cache.attention {
        snapshot.attention_k.insert(
            layer_index,
            read_attention_cache_prefix_values_f32(
                &env.shared_runtime,
                &env.shared_main_buffer,
                &env.weights.ctx,
                ids.k_cache,
                cache_tokens,
            )?,
        );
        snapshot.attention_v.insert(
            layer_index,
            read_attention_cache_prefix_values_f32(
                &env.shared_runtime,
                &env.shared_main_buffer,
                &env.weights.ctx,
                ids.v_cache,
                cache_tokens,
            )?,
        );
    }

    for (&layer_index, ids) in &env.shared_cache.recurrent {
        snapshot.recurrent_r.insert(
            layer_index,
            read_full_tensor_values_f32(
                &env.shared_runtime,
                &env.shared_main_buffer,
                &env.weights.ctx,
                ids.r_cache,
            )?,
        );
        snapshot.recurrent_s.insert(
            layer_index,
            read_full_tensor_values_f32(
                &env.shared_runtime,
                &env.shared_main_buffer,
                &env.weights.ctx,
                ids.s_cache,
            )?,
        );
    }

    Ok(snapshot)
}

fn compare_shared_hybrid_split_vs_full_cache_state(
    model: &LlamaModel,
    prompt_token_ids: &[i32],
    continue_token_id: i32,
    attention_k_type: TensorType,
    attention_v_type: TensorType,
) -> Result<HybridCacheDiffSummary, Box<dyn std::error::Error>> {
    if prompt_token_ids.is_empty() {
        return Err("cannot compare split/full cache state for an empty prompt".into());
    }
    let total_tokens = prompt_token_ids
        .len()
        .checked_add(1)
        .ok_or("overflow computing split/full token count")?;
    let max_context = u32::try_from(total_tokens)?;

    let split_snapshot = {
        let mut split_env =
            build_shared_hybrid_debug_env(model, max_context, attention_k_type, attention_v_type)?;
        let split_prompt_positions = (0..prompt_token_ids.len())
            .map(i32::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        let split_prompt_output_id = [i32::try_from(prompt_token_ids.len() - 1)?];
        let _ = run_shared_hybrid_graph(
            &mut split_env,
            prompt_token_ids,
            &split_prompt_positions,
            prompt_token_ids.len(),
            &split_prompt_output_id,
        )?;
        let split_step_position = [i32::try_from(prompt_token_ids.len())?];
        let split_step_output_id = [0_i32];
        let _ = run_shared_hybrid_graph(
            &mut split_env,
            std::slice::from_ref(&continue_token_id),
            &split_step_position,
            total_tokens,
            &split_step_output_id,
        )?;
        capture_shared_hybrid_cache_snapshot(&split_env, total_tokens)?
    };

    let mut full_env =
        build_shared_hybrid_debug_env(model, max_context, attention_k_type, attention_v_type)?;
    let mut full_token_ids = prompt_token_ids.to_vec();
    full_token_ids.push(continue_token_id);
    let full_positions = (0..full_token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let full_output_id = [i32::try_from(full_token_ids.len() - 1)?];
    let _ = run_shared_hybrid_graph(
        &mut full_env,
        &full_token_ids,
        &full_positions,
        full_token_ids.len(),
        &full_output_id,
    )?;

    let mut summary = HybridCacheDiffSummary::default();

    for (&layer_index, full_ids) in &full_env.shared_cache.attention {
        let full_k = read_attention_cache_prefix_values_f32(
            &full_env.shared_runtime,
            &full_env.shared_main_buffer,
            &full_env.weights.ctx,
            full_ids.k_cache,
            total_tokens,
        )?;
        let split_k = split_snapshot
            .attention_k
            .get(&layer_index)
            .ok_or_else(|| format!("split attention-k snapshot missing layer {}", layer_index))?;
        update_layer_diff_summary(
            &mut summary.attention_k,
            layer_index,
            compare_logits(&split_k, &full_k),
        );

        let full_v = read_attention_cache_prefix_values_f32(
            &full_env.shared_runtime,
            &full_env.shared_main_buffer,
            &full_env.weights.ctx,
            full_ids.v_cache,
            total_tokens,
        )?;
        let split_v = split_snapshot
            .attention_v
            .get(&layer_index)
            .ok_or_else(|| format!("split attention-v snapshot missing layer {}", layer_index))?;
        update_layer_diff_summary(
            &mut summary.attention_v,
            layer_index,
            compare_logits(&split_v, &full_v),
        );
    }

    for (&layer_index, full_ids) in &full_env.shared_cache.recurrent {
        let full_r = read_full_tensor_values_f32(
            &full_env.shared_runtime,
            &full_env.shared_main_buffer,
            &full_env.weights.ctx,
            full_ids.r_cache,
        )?;
        let split_r = split_snapshot
            .recurrent_r
            .get(&layer_index)
            .ok_or_else(|| format!("split recurrent-r snapshot missing layer {}", layer_index))?;
        update_layer_diff_summary(
            &mut summary.recurrent_r,
            layer_index,
            compare_logits(&split_r, &full_r),
        );

        let full_s = read_full_tensor_values_f32(
            &full_env.shared_runtime,
            &full_env.shared_main_buffer,
            &full_env.weights.ctx,
            full_ids.s_cache,
        )?;
        let split_s = split_snapshot
            .recurrent_s
            .get(&layer_index)
            .ok_or_else(|| format!("split recurrent-s snapshot missing layer {}", layer_index))?;
        update_layer_diff_summary(
            &mut summary.recurrent_s,
            layer_index,
            compare_logits(&split_s, &full_s),
        );
    }

    Ok(summary)
}

fn compare_shared_hybrid_split_vs_full_logits(
    model: &LlamaModel,
    prompt_token_ids: &[i32],
    continue_token_id: i32,
    attention_k_type: TensorType,
    attention_v_type: TensorType,
) -> Result<LogitComparison, Box<dyn std::error::Error>> {
    if prompt_token_ids.is_empty() {
        return Err("cannot compare split/full logits for an empty prompt".into());
    }
    let total_tokens = prompt_token_ids
        .len()
        .checked_add(1)
        .ok_or("overflow computing split/full token count")?;
    let max_context = u32::try_from(total_tokens)?;

    let split_logits = {
        let mut split_env =
            build_shared_hybrid_debug_env(model, max_context, attention_k_type, attention_v_type)?;
        let split_prompt_positions = (0..prompt_token_ids.len())
            .map(i32::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        let split_prompt_output_id = [i32::try_from(prompt_token_ids.len() - 1)?];
        let _ = run_shared_hybrid_graph(
            &mut split_env,
            prompt_token_ids,
            &split_prompt_positions,
            prompt_token_ids.len(),
            &split_prompt_output_id,
        )?;
        let split_step_position = [i32::try_from(prompt_token_ids.len())?];
        let split_step_output_id = [0_i32];
        run_shared_hybrid_graph(
            &mut split_env,
            std::slice::from_ref(&continue_token_id),
            &split_step_position,
            total_tokens,
            &split_step_output_id,
        )?
    };

    let full_logits = {
        let mut full_env =
            build_shared_hybrid_debug_env(model, max_context, attention_k_type, attention_v_type)?;
        let mut full_token_ids = prompt_token_ids.to_vec();
        full_token_ids.push(continue_token_id);
        let full_positions = (0..full_token_ids.len())
            .map(i32::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        let full_output_id = [i32::try_from(full_token_ids.len() - 1)?];
        run_shared_hybrid_graph(
            &mut full_env,
            &full_token_ids,
            &full_positions,
            full_token_ids.len(),
            &full_output_id,
        )?
    };

    Ok(compare_logits(&split_logits, &full_logits))
}

#[allow(dead_code)]
fn build_hybrid_checkpoint_session(
    model: &LlamaModel,
    max_context: u32,
    n_tokens: usize,
) -> Result<HybridCheckpointSession, Box<dyn std::error::Error>> {
    build_hybrid_checkpoint_session_with_shared_cache(model, max_context, n_tokens, true)
}

fn build_hybrid_checkpoint_session_with_shared_cache(
    model: &LlamaModel,
    max_context: u32,
    n_tokens: usize,
    use_shared_cache: bool,
) -> Result<HybridCheckpointSession, Box<dyn std::error::Error>> {
    build_hybrid_checkpoint_session_with_shared_cache_and_seed(
        model,
        max_context,
        n_tokens,
        use_shared_cache,
        None,
    )
}

fn build_hybrid_checkpoint_session_with_seeded_cache(
    model: &LlamaModel,
    max_context: u32,
    n_tokens: usize,
    initial_cache: &HybridCacheSnapshot,
) -> Result<HybridCheckpointSession, Box<dyn std::error::Error>> {
    build_hybrid_checkpoint_session_with_shared_cache_and_seed(
        model,
        max_context,
        n_tokens,
        true,
        Some(initial_cache),
    )
}

fn build_hybrid_checkpoint_session_with_shared_cache_and_seed(
    model: &LlamaModel,
    max_context: u32,
    n_tokens: usize,
    use_shared_cache: bool,
    initial_cache: Option<&HybridCacheSnapshot>,
) -> Result<HybridCheckpointSession, Box<dyn std::error::Error>> {
    let plan = model.execution_plan()?;
    let mut weights = plan
        .full_weights
        .allocate_and_load_with_extra(&model.gguf, COMPARE_SHARED_HYBRID_EXTRA_CONTEXT_BYTES)?;
    let spec = model.hybrid_decode_spec(
        max_context,
        1,
        TensorType::F16,
        TensorType::F16,
        TensorType::F32,
        TensorType::F32,
    )?;
    let shared_cache = if use_shared_cache {
        allocate_hybrid_shared_cache_tensors(&mut weights.ctx, &weights.tensor_ids, &spec)?
    } else {
        HybridSharedCacheTensorIds::default()
    };
    let mut decode = build_hybrid_decode_graph_with_outputs(
        &mut weights.ctx,
        &weights.tensor_ids,
        &spec,
        use_shared_cache.then_some(&shared_cache),
        n_tokens,
        1,
    )?;
    if let Some(initial_cache) = initial_cache {
        write_hybrid_cache_snapshot(&mut weights.ctx, &shared_cache, initial_cache)?;
    }

    let mut checkpoints = Vec::new();
    checkpoints.push(HybridCheckpointTensor {
        label: "model.input_embed".to_owned(),
        tensor_id: add_hidden_token_checkpoint_by_name(
            &mut weights.ctx,
            "hybrid_decode.input_embed",
            "hybrid_decode.input_embed.ck",
        )?,
    });

    for layer in &spec.layers {
        let layer_index = match layer {
            HybridLayerSpec::Attention { layer_index, .. }
            | HybridLayerSpec::Recurrent { layer_index, .. } => *layer_index,
        };
        let attn_residual_name = format!("hybrid_decode.layer{layer_index}.attn_residual");
        checkpoints.push(HybridCheckpointTensor {
            label: format!("layer{layer_index}.attn_residual"),
            tensor_id: add_hidden_token_checkpoint_by_name(
                &mut weights.ctx,
                &attn_residual_name,
                &format!("{attn_residual_name}.ck"),
            )?,
        });

        let attn_post_norm_name = format!("hybrid_decode.layer{layer_index}.attn_post_norm");
        if let Some(tensor_id) = maybe_add_hidden_token_checkpoint_by_name(
            &mut weights.ctx,
            &attn_post_norm_name,
            &format!("{attn_post_norm_name}.ck"),
        )? {
            checkpoints.push(HybridCheckpointTensor {
                label: format!("layer{layer_index}.attn_post_norm"),
                tensor_id,
            });
        }

        let ffn_input_norm_name = format!("hybrid_decode.layer{layer_index}.ffn.input_norm");
        if let Some(tensor_id) = maybe_add_hidden_token_checkpoint_by_name(
            &mut weights.ctx,
            &ffn_input_norm_name,
            &format!("{ffn_input_norm_name}.ck"),
        )? {
            checkpoints.push(HybridCheckpointTensor {
                label: format!("layer{layer_index}.ffn_input_norm"),
                tensor_id,
            });
        }

        let ffn_out_name = format!("hybrid_decode.layer{layer_index}.ffn.result_output");
        checkpoints.push(HybridCheckpointTensor {
            label: format!("layer{layer_index}.ffn_out"),
            tensor_id: add_hidden_token_checkpoint_by_name(
                &mut weights.ctx,
                &ffn_out_name,
                &format!("{ffn_out_name}.ck"),
            )?,
        });

        let ffn_post_norm_name = format!("hybrid_decode.layer{layer_index}.ffn_post_norm");
        if let Some(tensor_id) = maybe_add_hidden_token_checkpoint_by_name(
            &mut weights.ctx,
            &ffn_post_norm_name,
            &format!("{ffn_post_norm_name}.ck"),
        )? {
            checkpoints.push(HybridCheckpointTensor {
                label: format!("layer{layer_index}.ffn_post_norm"),
                tensor_id,
            });
        }

        let post_ffn_name = format!("hybrid_decode.layer{layer_index}.post_ffn");
        checkpoints.push(HybridCheckpointTensor {
            label: format!("layer{layer_index}.post_ffn"),
            tensor_id: add_hidden_token_checkpoint_by_any_name(
                &mut weights.ctx,
                &[&post_ffn_name, "hybrid_decode.result_hidden"],
                &format!("{post_ffn_name}.ck"),
            )?,
        });
    }

    checkpoints.push(HybridCheckpointTensor {
        label: "model.result_norm".to_owned(),
        tensor_id: add_hidden_token_checkpoint_by_name(
            &mut weights.ctx,
            "hybrid_decode.result_norm",
            "hybrid_decode.result_norm.ck",
        )?,
    });
    checkpoints.push(HybridCheckpointTensor {
        label: "model.result_logits".to_owned(),
        tensor_id: add_hidden_token_checkpoint_by_name(
            &mut weights.ctx,
            "hybrid_decode.result_logits",
            "hybrid_decode.result_logits.ck",
        )?,
    });

    for checkpoint in &checkpoints {
        decode
            .graph
            .build_forward_expand(&weights.ctx, checkpoint.tensor_id)?;
    }

    let runtime = MetalRuntime::new()?;
    let prepared = prepare_graph(&weights.ctx, &decode.graph, runtime.features())?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &weights.ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;

    Ok(HybridCheckpointSession {
        weights,
        spec,
        decode,
        session,
        shared_cache,
        checkpoints,
    })
}

fn build_hybrid_preview_session(
    model: &LlamaModel,
    max_context: u32,
    n_tokens: usize,
    checkpoint_specs: &[HybridPreviewCheckpointSpec],
) -> Result<HybridCheckpointSession, Box<dyn std::error::Error>> {
    let plan = model.execution_plan()?;
    let mut weights = plan
        .full_weights
        .allocate_and_load_with_extra(&model.gguf, COMPARE_SHARED_HYBRID_EXTRA_CONTEXT_BYTES)?;
    let spec = model.hybrid_decode_spec(
        max_context,
        1,
        TensorType::F16,
        TensorType::F16,
        TensorType::F32,
        TensorType::F32,
    )?;
    build_hybrid_preview_session_with_spec(weights, spec, n_tokens, checkpoint_specs)
}

fn build_hybrid_preview_session_with_spec(
    mut weights: makepad_llama::LoadedGgufWeights,
    spec: makepad_llama::HybridDecodeSpec,
    n_tokens: usize,
    checkpoint_specs: &[HybridPreviewCheckpointSpec],
) -> Result<HybridCheckpointSession, Box<dyn std::error::Error>> {
    let shared_cache =
        allocate_hybrid_shared_cache_tensors(&mut weights.ctx, &weights.tensor_ids, &spec)?;
    let mut decode = build_hybrid_decode_graph_with_outputs(
        &mut weights.ctx,
        &weights.tensor_ids,
        &spec,
        Some(&shared_cache),
        n_tokens,
        1,
    )?;

    let mut checkpoints = Vec::new();
    for checkpoint_spec in checkpoint_specs {
        if !checkpoint_spec
            .source_names
            .iter()
            .any(|name| weights.ctx.get_tensor(name).is_some())
        {
            continue;
        }
        let source_names = checkpoint_spec
            .source_names
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let checkpoint_name = format!("{}.ck", checkpoint_spec.label);
        let tensor_id = add_contiguous_checkpoint_by_any_name(
            &mut weights.ctx,
            &source_names,
            &checkpoint_name,
        )?;
        checkpoints.push(HybridCheckpointTensor {
            label: checkpoint_spec.label.clone(),
            tensor_id,
        });
    }

    for checkpoint in &checkpoints {
        decode
            .graph
            .build_forward_expand(&weights.ctx, checkpoint.tensor_id)?;
    }

    let runtime = MetalRuntime::new()?;
    let prepared = prepare_graph(&weights.ctx, &decode.graph, runtime.features())?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &weights.ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;

    Ok(HybridCheckpointSession {
        weights,
        spec,
        decode,
        session,
        shared_cache,
        checkpoints,
    })
}

fn build_hybrid_hidden_token_capture_session(
    model: &LlamaModel,
    max_context: u32,
    n_tokens: usize,
    attention_k_type: TensorType,
    attention_v_type: TensorType,
    checkpoint_specs: &[HybridPreviewCheckpointSpec],
) -> Result<HybridCheckpointSession, Box<dyn std::error::Error>> {
    let plan = model.execution_plan()?;
    let weights = plan
        .full_weights
        .allocate_and_load_with_extra(&model.gguf, COMPARE_SHARED_HYBRID_EXTRA_CONTEXT_BYTES)?;
    let spec = model.hybrid_decode_spec(
        max_context,
        1,
        attention_k_type,
        attention_v_type,
        TensorType::F32,
        TensorType::F32,
    )?;
    build_hybrid_hidden_token_capture_session_with_spec(weights, spec, n_tokens, checkpoint_specs)
}

fn build_hybrid_token_dim2_capture_session(
    model: &LlamaModel,
    max_context: u32,
    n_tokens: usize,
    attention_k_type: TensorType,
    attention_v_type: TensorType,
    checkpoint_specs: &[HybridPreviewCheckpointSpec],
) -> Result<HybridCheckpointSession, Box<dyn std::error::Error>> {
    let plan = model.execution_plan()?;
    let weights = plan
        .full_weights
        .allocate_and_load_with_extra(&model.gguf, COMPARE_SHARED_HYBRID_EXTRA_CONTEXT_BYTES)?;
    let spec = model.hybrid_decode_spec(
        max_context,
        1,
        attention_k_type,
        attention_v_type,
        TensorType::F32,
        TensorType::F32,
    )?;
    build_hybrid_token_dim2_capture_session_with_spec(weights, spec, n_tokens, checkpoint_specs)
}

fn build_hybrid_hidden_token_capture_session_with_spec(
    mut weights: makepad_llama::LoadedGgufWeights,
    spec: makepad_llama::HybridDecodeSpec,
    n_tokens: usize,
    checkpoint_specs: &[HybridPreviewCheckpointSpec],
) -> Result<HybridCheckpointSession, Box<dyn std::error::Error>> {
    let shared_cache =
        allocate_hybrid_shared_cache_tensors(&mut weights.ctx, &weights.tensor_ids, &spec)?;
    let mut decode = build_hybrid_decode_graph_with_outputs(
        &mut weights.ctx,
        &weights.tensor_ids,
        &spec,
        Some(&shared_cache),
        n_tokens,
        1,
    )?;

    let mut checkpoints = Vec::new();
    for checkpoint_spec in checkpoint_specs {
        if !checkpoint_spec
            .source_names
            .iter()
            .any(|name| weights.ctx.get_tensor(name).is_some())
        {
            continue;
        }
        let source_names = checkpoint_spec
            .source_names
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let checkpoint_name = format!("{}.ck", checkpoint_spec.label);
        let tensor_id = add_hidden_token_checkpoint_by_any_name(
            &mut weights.ctx,
            &source_names,
            &checkpoint_name,
        )?;
        checkpoints.push(HybridCheckpointTensor {
            label: checkpoint_spec.label.clone(),
            tensor_id,
        });
    }

    for checkpoint in &checkpoints {
        decode
            .graph
            .build_forward_expand(&weights.ctx, checkpoint.tensor_id)?;
    }

    let runtime = MetalRuntime::new()?;
    let prepared = prepare_graph(&weights.ctx, &decode.graph, runtime.features())?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &weights.ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;

    Ok(HybridCheckpointSession {
        weights,
        spec,
        decode,
        session,
        shared_cache,
        checkpoints,
    })
}

fn build_hybrid_token_dim2_capture_session_with_spec(
    mut weights: makepad_llama::LoadedGgufWeights,
    spec: makepad_llama::HybridDecodeSpec,
    n_tokens: usize,
    checkpoint_specs: &[HybridPreviewCheckpointSpec],
) -> Result<HybridCheckpointSession, Box<dyn std::error::Error>> {
    let shared_cache =
        allocate_hybrid_shared_cache_tensors(&mut weights.ctx, &weights.tensor_ids, &spec)?;
    let mut decode = build_hybrid_decode_graph_with_outputs(
        &mut weights.ctx,
        &weights.tensor_ids,
        &spec,
        Some(&shared_cache),
        n_tokens,
        1,
    )?;

    let mut checkpoints = Vec::new();
    for checkpoint_spec in checkpoint_specs {
        if !checkpoint_spec
            .source_names
            .iter()
            .any(|name| weights.ctx.get_tensor(name).is_some())
        {
            continue;
        }
        let source_names = checkpoint_spec
            .source_names
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let checkpoint_name = format!("{}.ck", checkpoint_spec.label);
        let tensor_id = add_token_dim2_checkpoint_by_any_name(
            &mut weights.ctx,
            &source_names,
            &checkpoint_name,
        )?;
        checkpoints.push(HybridCheckpointTensor {
            label: checkpoint_spec.label.clone(),
            tensor_id,
        });
    }

    for checkpoint in &checkpoints {
        decode
            .graph
            .build_forward_expand(&weights.ctx, checkpoint.tensor_id)?;
    }

    let runtime = MetalRuntime::new()?;
    let prepared = prepare_graph(&weights.ctx, &decode.graph, runtime.features())?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &weights.ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;

    Ok(HybridCheckpointSession {
        weights,
        spec,
        decode,
        session,
        shared_cache,
        checkpoints,
    })
}

fn build_truncated_hybrid_preview_session(
    model: &LlamaModel,
    max_context: u32,
    n_tokens: usize,
    max_layer_index: u32,
    checkpoint_specs: &[HybridPreviewCheckpointSpec],
) -> Result<HybridCheckpointSession, Box<dyn std::error::Error>> {
    let plan = model.execution_plan()?;
    let weights = plan
        .full_weights
        .allocate_and_load_with_extra(&model.gguf, COMPARE_SHARED_HYBRID_EXTRA_CONTEXT_BYTES)?;
    let mut spec = model.hybrid_decode_spec(
        max_context,
        1,
        TensorType::F16,
        TensorType::F16,
        TensorType::F32,
        TensorType::F32,
    )?;
    spec.layers.retain(|layer| match layer {
        HybridLayerSpec::Attention { layer_index, .. }
        | HybridLayerSpec::Recurrent { layer_index, .. } => *layer_index <= max_layer_index,
    });
    build_hybrid_preview_session_with_spec(weights, spec, n_tokens, checkpoint_specs)
}

fn build_single_hybrid_capture_session(
    model: &LlamaModel,
    max_context: u32,
    n_tokens: usize,
    source_names: &[&str],
    label: &str,
    capture_last_hidden_token: bool,
) -> Result<HybridCheckpointSession, Box<dyn std::error::Error>> {
    let plan = model.execution_plan()?;
    let mut weights = plan
        .full_weights
        .allocate_and_load_with_extra(&model.gguf, COMPARE_SHARED_HYBRID_EXTRA_CONTEXT_BYTES)?;
    let spec = model.hybrid_decode_spec(
        max_context,
        1,
        TensorType::F16,
        TensorType::F16,
        TensorType::F32,
        TensorType::F32,
    )?;
    let shared_cache =
        allocate_hybrid_shared_cache_tensors(&mut weights.ctx, &weights.tensor_ids, &spec)?;
    let mut decode = build_hybrid_decode_graph_with_outputs(
        &mut weights.ctx,
        &weights.tensor_ids,
        &spec,
        Some(&shared_cache),
        n_tokens,
        1,
    )?;

    let checkpoint_name = format!("{label}.ck");
    let tensor_id = if capture_last_hidden_token {
        add_hidden_token_checkpoint_by_any_name(&mut weights.ctx, source_names, &checkpoint_name)?
    } else {
        add_contiguous_checkpoint_by_any_name(&mut weights.ctx, source_names, &checkpoint_name)?
    };
    decode.graph.build_forward_expand(&weights.ctx, tensor_id)?;

    let runtime = MetalRuntime::new()?;
    let prepared = prepare_graph(&weights.ctx, &decode.graph, runtime.features())?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &weights.ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;

    Ok(HybridCheckpointSession {
        weights,
        spec,
        decode,
        session,
        shared_cache,
        checkpoints: vec![HybridCheckpointTensor {
            label: label.to_owned(),
            tensor_id,
        }],
    })
}

#[allow(dead_code)]
fn compare_hybrid_first_step_capacity_tensor(
    model: &LlamaModel,
    token_id: i32,
    max_context: u32,
    source_names: &[&str],
    label: &str,
    capture_last_hidden_token: bool,
) -> Result<LogitComparison, Box<dyn std::error::Error>> {
    if max_context <= 1 {
        return Err("first-step tensor compare requires max_context > 1".into());
    }

    let token_ids = [token_id];
    let positions = [0_i32];
    let output_ids = [0_i32];

    let mut reference_session = build_single_hybrid_capture_session(
        model,
        1,
        1,
        source_names,
        label,
        capture_last_hidden_token,
    )?;
    let reference_outputs = execute_hybrid_checkpoint_session(
        &mut reference_session,
        &token_ids,
        &positions,
        1,
        &output_ids,
    )?;

    let mut wide_session = build_single_hybrid_capture_session(
        model,
        max_context,
        1,
        source_names,
        label,
        capture_last_hidden_token,
    )?;
    let wide_outputs = execute_hybrid_checkpoint_session(
        &mut wide_session,
        &token_ids,
        &positions,
        1,
        &output_ids,
    )?;

    compare_named_checkpoint(&wide_outputs, &reference_outputs, label)
}

fn execute_hybrid_checkpoint_session(
    session: &mut HybridCheckpointSession,
    token_ids: &[i32],
    positions: &[i32],
    cache_tokens: usize,
    output_ids: &[i32],
) -> Result<BTreeMap<String, Vec<f32>>, Box<dyn std::error::Error>> {
    let mut layout = HybridDecodeBatchLayout::from_contiguous_positions_and_outputs(
        positions,
        cache_tokens,
        output_ids,
    )?;
    if session.decode.input_recurrent_state_rows.is_none() {
        layout.recurrent_state_rows.clear();
    }

    for cache_view in &session.decode.attention_cache_views {
        if compare_should_use_flash_attention(cache_view.k_head_dim as usize, positions.len()) {
            compare_configure_attention_cache_view(
                &mut session.weights.ctx,
                cache_view.k_cache_view,
                cache_view.k_head_dim,
                cache_tokens,
                cache_view.kv_head_count,
                cache_view.max_sequences,
            )?;
            compare_configure_attention_cache_view(
                &mut session.weights.ctx,
                cache_view.v_cache_view,
                cache_view.v_head_dim,
                cache_tokens,
                cache_view.kv_head_count,
                cache_view.max_sequences,
            )?;
            if let Some(input_mask) = cache_view.input_mask {
                compare_configure_attention_mask_view(
                    &mut session.weights.ctx,
                    input_mask,
                    cache_tokens,
                    positions.len(),
                )?;
            }
        }
    }

    let input_primary = i32s_to_bytes(token_ids);
    let output_id_bytes = i32s_to_bytes(&layout.output_ids);
    let attention_write_index_bytes = layout
        .attention_write_indices
        .is_empty()
        .then(Vec::new)
        .unwrap_or_else(|| i32s_to_bytes(&layout.attention_write_indices));
    let recurrent_state_row_bytes = layout
        .recurrent_state_rows
        .is_empty()
        .then(Vec::new)
        .unwrap_or_else(|| i32s_to_bytes(&layout.recurrent_state_rows));
    let rope_positions = if session.decode.input_rope_positions.is_some() {
        let rope = session
            .spec
            .layers
            .iter()
            .find_map(|layer| match layer {
                HybridLayerSpec::Attention { decode, .. } => decode.block.rope.as_ref(),
                HybridLayerSpec::Recurrent { .. } => None,
            })
            .ok_or("hybrid checkpoint session is missing rope spec")?;
        Some(encode_rope_positions(rope, positions, positions.len())?)
    } else {
        None
    };
    let rope_position_bytes = rope_positions
        .as_ref()
        .map(|positions| i32s_to_bytes(positions));

    let mut attention_mask_bytes = Vec::new();
    for cache_view in &session.decode.attention_cache_views {
        if let Some(input_mask) = cache_view.input_mask {
            let key_count = compare_attention_mask_write_key_count(
                &session.weights.ctx,
                input_mask,
                cache_view.k_head_dim,
                cache_tokens,
                positions.len(),
            )?;
            attention_mask_bytes.push(position_attention_mask_bytes_for_tensor(
                &session.weights.ctx,
                input_mask,
                key_count,
                positions,
            )?);
        }
    }

    let mut writes = vec![
        MetalGraphTensorWrite {
            tensor_id: session.decode.input_primary,
            bytes: &input_primary,
        },
        MetalGraphTensorWrite {
            tensor_id: session.decode.input_output_ids,
            bytes: &output_id_bytes,
        },
    ];
    if let Some(input_per_layer_primary) = session.decode.input_per_layer_primary {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_per_layer_primary,
            bytes: &input_primary,
        });
    }
    if let Some(input_attention_write_indices) = session.decode.input_attention_write_indices {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_attention_write_indices,
            bytes: &attention_write_index_bytes,
        });
    }
    if let Some(input_rope_positions) = session.decode.input_rope_positions {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_rope_positions,
            bytes: rope_position_bytes
                .as_deref()
                .ok_or("rope position bytes were not prepared")?,
        });
    }
    if let Some(input_recurrent_state_rows) = session.decode.input_recurrent_state_rows {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_recurrent_state_rows,
            bytes: &recurrent_state_row_bytes,
        });
    }

    let mut attention_mask_index = 0usize;
    for cache_view in &session.decode.attention_cache_views {
        if let Some(input_mask) = cache_view.input_mask {
            let bytes = attention_mask_bytes
                .get(attention_mask_index)
                .ok_or("attention mask bytes were not prepared")?;
            attention_mask_index += 1;
            writes.push(MetalGraphTensorWrite {
                tensor_id: input_mask,
                bytes,
            });
        }
    }

    let output_tensors = session
        .checkpoints
        .iter()
        .map(|checkpoint| checkpoint.tensor_id)
        .collect::<Vec<_>>();
    let execution = session
        .session
        .execute(&session.weights.ctx, &writes, &output_tensors)?;

    let mut outputs = BTreeMap::new();
    for checkpoint in &session.checkpoints {
        let bytes = execution
            .outputs
            .get(&checkpoint.tensor_id)
            .ok_or_else(|| format!("missing output for checkpoint '{}'", checkpoint.label))?;
        outputs.insert(
            checkpoint.label.clone(),
            output_tensor_values_f32(&session.weights.ctx, checkpoint.tensor_id, bytes)?,
        );
    }
    Ok(outputs)
}

fn execute_hybrid_checkpoint_session_previews(
    session: &mut HybridCheckpointSession,
    token_ids: &[i32],
    positions: &[i32],
    cache_tokens: usize,
    output_ids: &[i32],
) -> Result<BTreeMap<String, TensorPreview>, Box<dyn std::error::Error>> {
    let mut layout = HybridDecodeBatchLayout::from_contiguous_positions_and_outputs(
        positions,
        cache_tokens,
        output_ids,
    )?;
    if session.decode.input_recurrent_state_rows.is_none() {
        layout.recurrent_state_rows.clear();
    }

    for cache_view in &session.decode.attention_cache_views {
        if compare_should_use_flash_attention(cache_view.k_head_dim as usize, positions.len()) {
            compare_configure_attention_cache_view(
                &mut session.weights.ctx,
                cache_view.k_cache_view,
                cache_view.k_head_dim,
                cache_tokens,
                cache_view.kv_head_count,
                cache_view.max_sequences,
            )?;
            compare_configure_attention_cache_view(
                &mut session.weights.ctx,
                cache_view.v_cache_view,
                cache_view.v_head_dim,
                cache_tokens,
                cache_view.kv_head_count,
                cache_view.max_sequences,
            )?;
            if let Some(input_mask) = cache_view.input_mask {
                compare_configure_attention_mask_view(
                    &mut session.weights.ctx,
                    input_mask,
                    cache_tokens,
                    positions.len(),
                )?;
            }
        }
    }

    let input_primary = i32s_to_bytes(token_ids);
    let output_id_bytes = i32s_to_bytes(&layout.output_ids);
    let attention_write_index_bytes = layout
        .attention_write_indices
        .is_empty()
        .then(Vec::new)
        .unwrap_or_else(|| i32s_to_bytes(&layout.attention_write_indices));
    let recurrent_state_row_bytes = layout
        .recurrent_state_rows
        .is_empty()
        .then(Vec::new)
        .unwrap_or_else(|| i32s_to_bytes(&layout.recurrent_state_rows));
    let rope_positions = if session.decode.input_rope_positions.is_some() {
        let rope = session
            .spec
            .layers
            .iter()
            .find_map(|layer| match layer {
                HybridLayerSpec::Attention { decode, .. } => decode.block.rope.as_ref(),
                HybridLayerSpec::Recurrent { .. } => None,
            })
            .ok_or("hybrid checkpoint session is missing rope spec")?;
        Some(encode_rope_positions(rope, positions, positions.len())?)
    } else {
        None
    };
    let rope_position_bytes = rope_positions
        .as_ref()
        .map(|positions| i32s_to_bytes(positions));

    let mut attention_mask_bytes = Vec::new();
    for cache_view in &session.decode.attention_cache_views {
        if let Some(input_mask) = cache_view.input_mask {
            let key_count = compare_attention_mask_write_key_count(
                &session.weights.ctx,
                input_mask,
                cache_view.k_head_dim,
                cache_tokens,
                positions.len(),
            )?;
            attention_mask_bytes.push(position_attention_mask_bytes_for_tensor(
                &session.weights.ctx,
                input_mask,
                key_count,
                positions,
            )?);
        }
    }

    let mut writes = vec![
        MetalGraphTensorWrite {
            tensor_id: session.decode.input_primary,
            bytes: &input_primary,
        },
        MetalGraphTensorWrite {
            tensor_id: session.decode.input_output_ids,
            bytes: &output_id_bytes,
        },
    ];
    if let Some(input_per_layer_primary) = session.decode.input_per_layer_primary {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_per_layer_primary,
            bytes: &input_primary,
        });
    }
    if let Some(input_attention_write_indices) = session.decode.input_attention_write_indices {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_attention_write_indices,
            bytes: &attention_write_index_bytes,
        });
    }
    if let Some(input_rope_positions) = session.decode.input_rope_positions {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_rope_positions,
            bytes: rope_position_bytes
                .as_deref()
                .ok_or("rope position bytes were not prepared")?,
        });
    }
    if let Some(input_recurrent_state_rows) = session.decode.input_recurrent_state_rows {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_recurrent_state_rows,
            bytes: &recurrent_state_row_bytes,
        });
    }

    let mut attention_mask_index = 0usize;
    for cache_view in &session.decode.attention_cache_views {
        if let Some(input_mask) = cache_view.input_mask {
            let bytes = attention_mask_bytes
                .get(attention_mask_index)
                .ok_or("attention mask bytes were not prepared")?;
            attention_mask_index += 1;
            writes.push(MetalGraphTensorWrite {
                tensor_id: input_mask,
                bytes,
            });
        }
    }

    let output_tensors = session
        .checkpoints
        .iter()
        .map(|checkpoint| checkpoint.tensor_id)
        .collect::<Vec<_>>();
    let execution = session
        .session
        .execute(&session.weights.ctx, &writes, &output_tensors)?;

    let mut outputs = BTreeMap::new();
    for checkpoint in &session.checkpoints {
        let bytes = execution
            .outputs
            .get(&checkpoint.tensor_id)
            .ok_or_else(|| format!("missing output for checkpoint '{}'", checkpoint.label))?;
        let tensor = session
            .weights
            .ctx
            .tensor(checkpoint.tensor_id)
            .ok_or_else(|| format!("invalid checkpoint tensor '{}'", checkpoint.label))?;
        outputs.insert(
            checkpoint.label.clone(),
            tensor_preview_from_tensor_bytes(tensor, bytes)?,
        );
    }
    Ok(outputs)
}

fn compare_preview_label_to_upstream_names(
    rust_previews: &BTreeMap<String, TensorPreview>,
    upstream_previews: &BTreeMap<String, TensorPreview>,
    rust_label: &str,
    upstream_names: &[&str],
) -> Result<LogitComparison, Box<dyn std::error::Error>> {
    let rust_preview = rust_previews
        .get(rust_label)
        .ok_or_else(|| format!("missing rust preview '{rust_label}'"))?;
    let upstream_preview = upstream_preview_by_any_name(upstream_previews, upstream_names)?;
    Ok(compare_logits(
        &rust_preview.values,
        &upstream_preview.values,
    ))
}

fn compare_optional_preview_label_to_upstream_names(
    rust_previews: &BTreeMap<String, TensorPreview>,
    upstream_previews: &BTreeMap<String, TensorPreview>,
    rust_label: &str,
    upstream_names: &[&str],
) -> Result<Option<LogitComparison>, Box<dyn std::error::Error>> {
    let Some(rust_preview) = rust_previews.get(rust_label) else {
        return Ok(None);
    };
    let upstream_preview = upstream_preview_by_any_name(upstream_previews, upstream_names)?;
    Ok(Some(compare_logits(
        &rust_preview.values,
        &upstream_preview.values,
    )))
}

fn gemma_upstream_layer_preview_stack_check(
    args: &Args,
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<HybridUpstreamLayerPreviewStackCheck, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma upstream preview stack check requires a non-empty prompt".into());
    }

    let tensors = model.gemma4_tensors()?;
    let max_context = u32::try_from(token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let output_ids = [i32::try_from(token_ids.len() - 1)?];

    let checkpoint_specs = tensors
        .layers
        .iter()
        .map(|layer| HybridPreviewCheckpointSpec {
            label: format!("layer{}.post_ffn", layer.index),
            source_names: vec![format!("hybrid_decode.layer{}.post_ffn", layer.index)],
        })
        .collect::<Vec<_>>();
    let mut rust_session =
        build_hybrid_preview_session(model, max_context, token_ids.len(), &checkpoint_specs)?;
    let rust_previews = execute_hybrid_checkpoint_session_previews(
        &mut rust_session,
        token_ids,
        &positions,
        token_ids.len(),
        &output_ids,
    )?;

    let mut upstream_filter_names = Vec::with_capacity(tensors.layers.len() * 2);
    for layer in &tensors.layers {
        upstream_filter_names.push(format!("out_scaled-{}", layer.index));
        upstream_filter_names.push(format!("l_out-{}", layer.index));
    }
    let upstream_filter_refs = upstream_filter_names
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let upstream_previews = run_upstream_debug_tensor_previews(args, &upstream_filter_refs)?;

    let mut first_diff_layer = None;
    let mut first_diff_stats = None;
    let mut max_diff_layer = None;
    let mut max_diff_stats = LogitComparison {
        max_abs_diff: 0.0,
        mean_abs_diff: 0.0,
        rms_diff: 0.0,
        cosine_similarity: 1.0,
    };
    let mut layer_post_ffn_stats = BTreeMap::new();

    let mut compared_any_layer = false;
    for layer in &tensors.layers {
        let rust_label = format!("layer{}.post_ffn", layer.index);
        if !rust_previews.contains_key(&rust_label) {
            continue;
        }
        let upstream_out_scaled = format!("out_scaled-{}", layer.index);
        let upstream_l_out = format!("l_out-{}", layer.index);
        let stats = compare_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            &rust_label,
            &[upstream_out_scaled.as_str(), upstream_l_out.as_str()],
        )?;
        layer_post_ffn_stats.insert(layer.index, stats.clone());
        compared_any_layer = true;
        if first_diff_layer.is_none() && stats.max_abs_diff > 0.0 {
            first_diff_layer = Some(layer.index);
            first_diff_stats = Some(LogitComparison {
                max_abs_diff: stats.max_abs_diff,
                mean_abs_diff: stats.mean_abs_diff,
                rms_diff: stats.rms_diff,
                cosine_similarity: stats.cosine_similarity,
            });
        }
        if stats.max_abs_diff >= max_diff_stats.max_abs_diff {
            max_diff_layer = Some(layer.index);
            max_diff_stats = LogitComparison {
                max_abs_diff: stats.max_abs_diff,
                mean_abs_diff: stats.mean_abs_diff,
                rms_diff: stats.rms_diff,
                cosine_similarity: stats.cosine_similarity,
            };
        }
    }

    if !compared_any_layer {
        return Err(
            "gemma upstream preview stack check found no comparable layer checkpoints".into(),
        );
    }

    Ok(HybridUpstreamLayerPreviewStackCheck {
        first_diff_layer,
        first_diff_stats,
        max_diff_layer,
        max_diff_stats,
        layer_post_ffn_stats,
    })
}

fn gemma_upstream_shared_per_layer_input_check(
    args: &Args,
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<GemmaUpstreamSharedPerLayerInputCheck, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma upstream shared per-layer input check requires a non-empty prompt".into());
    }

    let max_context = u32::try_from(token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let output_ids = [i32::try_from(token_ids.len() - 1)?];
    let checkpoint_specs = vec![
        HybridPreviewCheckpointSpec {
            label: "shared_per_layer.selected".to_owned(),
            source_names: vec!["hybrid_decode.per_layer_input.selected_reshaped".to_owned()],
        },
        HybridPreviewCheckpointSpec {
            label: "shared_per_layer.proj".to_owned(),
            source_names: vec!["hybrid_decode.per_layer_input.model_proj_norm".to_owned()],
        },
        HybridPreviewCheckpointSpec {
            label: "shared_per_layer.input".to_owned(),
            source_names: vec!["hybrid_decode.per_layer_input.combined_scaled".to_owned()],
        },
    ];
    let mut rust_session =
        build_hybrid_preview_session(model, max_context, token_ids.len(), &checkpoint_specs)?;
    let rust_previews = execute_hybrid_checkpoint_session_previews(
        &mut rust_session,
        token_ids,
        &positions,
        token_ids.len(),
        &output_ids,
    )?;
    let upstream_previews = run_upstream_debug_tensor_previews(
        args,
        &["inp_per_layer_selected", "per_layer_proj", "inp_per_layer"],
    )?;

    Ok(GemmaUpstreamSharedPerLayerInputCheck {
        selected_stats: compare_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            "shared_per_layer.selected",
            &["inp_per_layer_selected"],
        )?,
        proj_stats: compare_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            "shared_per_layer.proj",
            &["per_layer_proj"],
        )?,
        input_stats: compare_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            "shared_per_layer.input",
            &["inp_per_layer"],
        )?,
    })
}

fn gemma_upstream_input_check(
    args: &Args,
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<GemmaUpstreamInputCheck, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma upstream input check requires a non-empty prompt".into());
    }

    let max_context = u32::try_from(token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let output_ids = [i32::try_from(token_ids.len() - 1)?];
    let checkpoint_specs = vec![
        HybridPreviewCheckpointSpec {
            label: "input_embed".to_owned(),
            source_names: vec!["hybrid_decode.input_embed".to_owned()],
        },
        HybridPreviewCheckpointSpec {
            label: "layer0.attn_input_norm".to_owned(),
            source_names: vec!["hybrid_decode.layer0.attn.input_norm".to_owned()],
        },
    ];
    let mut rust_session =
        build_hybrid_preview_session(model, max_context, token_ids.len(), &checkpoint_specs)?;
    let rust_previews = execute_hybrid_checkpoint_session_previews(
        &mut rust_session,
        token_ids,
        &positions,
        token_ids.len(),
        &output_ids,
    )?;

    let upstream_previews =
        run_upstream_debug_tensor_previews(args, &["inp_scaled", "attn_norm-0"])?;
    let standalone = execute_gemma_standalone_input_reference(model, token_ids)?;
    let rust_input_embed = rust_previews
        .get("input_embed")
        .ok_or("missing rust gemma input_embed preview")?;
    let rust_attn_input_norm = rust_previews
        .get("layer0.attn_input_norm")
        .ok_or("missing rust gemma attn input norm preview")?;
    let upstream_input_embed = upstream_previews
        .get("inp_scaled")
        .ok_or("missing upstream gemma inp_scaled preview")?;
    let upstream_attn_input_norm = upstream_previews
        .get("attn_norm-0")
        .ok_or("missing upstream gemma attn_norm-0 preview")?;

    Ok(GemmaUpstreamInputCheck {
        input_embed_stats: compare_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            "input_embed",
            &["inp_scaled"],
        )?,
        input_embed_standalone_vs_rust_stats: compare_logits(
            &standalone.input_embed.values,
            &rust_input_embed.values,
        ),
        input_embed_standalone_vs_upstream_stats: compare_logits(
            &standalone.input_embed.values,
            &upstream_input_embed.values,
        ),
        attn_input_norm_stats: compare_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            "layer0.attn_input_norm",
            &["attn_norm-0"],
        )?,
        attn_input_norm_standalone_vs_rust_stats: compare_logits(
            &standalone.attn_input_norm.values,
            &rust_attn_input_norm.values,
        ),
        attn_input_norm_standalone_vs_upstream_stats: compare_logits(
            &standalone.attn_input_norm.values,
            &upstream_attn_input_norm.values,
        ),
    })
}

fn execute_gemma_standalone_input_reference(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<GemmaStandaloneInputReference, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma standalone input reference requires at least one token".into());
    }

    let cfg = model.require_gemma4()?;
    let tensors = model.gemma4_tensors()?;
    let first_layer = tensors
        .layers
        .first()
        .ok_or("gemma standalone input reference requires at least one layer")?;
    let plan = model.execution_plan()?;
    let mut loaded = plan
        .full_weights
        .allocate_and_load_with_extra(&model.gguf, COMPARE_SHARED_HYBRID_EXTRA_CONTEXT_BYTES)?;
    let input_tokens = loaded.ctx.new_tensor_1d(
        TensorType::I32,
        i64::try_from(token_ids.len())?,
        BufferUsage::Activations,
    )?;
    loaded
        .ctx
        .tensor_mut(input_tokens)
        .ok_or("invalid gemma standalone input token tensor")?
        .set_input();

    let token_embd_id = *loaded
        .tensor_ids
        .get(&tensors.globals.token_embd.name)
        .ok_or("missing gemma token embedding tensor id")?;
    let mut input_embed = loaded
        .ctx
        .get_rows(token_embd_id, input_tokens, BufferUsage::Activations)?;
    input_embed = loaded.ctx.scale(
        input_embed,
        (cfg.embedding_length as f32).sqrt(),
        BufferUsage::Activations,
    )?;
    loaded
        .ctx
        .set_tensor_name(input_embed, "gemma_standalone.input_embed")?;
    let input_embed_out = loaded.ctx.cont(input_embed)?;
    loaded
        .ctx
        .set_tensor_name(input_embed_out, "gemma_standalone.input_embed.out")?;
    loaded
        .ctx
        .tensor_mut(input_embed_out)
        .ok_or("invalid gemma standalone input embed output tensor")?
        .set_output();

    let attn_norm_weight_id = *loaded
        .tensor_ids
        .get(&first_layer.attn_norm.name)
        .ok_or("missing gemma layer0 attn_norm tensor id")?;
    let attn_norm = loaded.ctx.rms_norm_eps(
        input_embed,
        cfg.attention_layer_norm_rms_epsilon,
        BufferUsage::Activations,
    )?;
    let attn_input_norm = loaded.ctx.binary_like_a(
        makepad_ggml::Op::Mul,
        attn_norm,
        attn_norm_weight_id,
        BufferUsage::Activations,
    )?;
    loaded
        .ctx
        .set_tensor_name(attn_input_norm, "gemma_standalone.attn_norm")?;
    let attn_input_norm_out = loaded.ctx.cont(attn_input_norm)?;
    loaded
        .ctx
        .set_tensor_name(attn_input_norm_out, "gemma_standalone.attn_norm.out")?;
    loaded
        .ctx
        .tensor_mut(attn_input_norm_out)
        .ok_or("invalid gemma standalone attn norm output tensor")?
        .set_output();

    let mut graph = Graph::new();
    graph.build_forward_expand(&loaded.ctx, input_embed_out)?;
    graph.build_forward_expand(&loaded.ctx, attn_input_norm_out)?;

    let runtime = MetalRuntime::new()?;
    let prepared = prepare_graph(&loaded.ctx, &graph, runtime.features())?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &loaded.ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let token_bytes = i32s_to_bytes(token_ids);
    let writes = [MetalGraphTensorWrite {
        tensor_id: input_tokens,
        bytes: &token_bytes,
    }];
    let execution = session.execute(&loaded.ctx, &writes, &[input_embed_out, attn_input_norm_out])?;

    Ok(GemmaStandaloneInputReference {
        input_embed: tensor_preview_from_tensor_bytes(
            loaded
                .ctx
                .tensor(input_embed_out)
                .ok_or("invalid gemma standalone input_embed output tensor")?,
            execution
                .outputs
                .get(&input_embed_out)
                .ok_or("missing gemma standalone input_embed output")?,
        )?,
        attn_input_norm: tensor_preview_from_tensor_bytes(
            loaded
                .ctx
                .tensor(attn_input_norm_out)
                .ok_or("invalid gemma standalone attn_norm output tensor")?,
            execution
                .outputs
                .get(&attn_input_norm_out)
                .ok_or("missing gemma standalone attn_norm output")?,
        )?,
    })
}

fn gemma_per_layer_token_get_rows_direct_check(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<LogitComparison, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma direct per-layer get_rows check requires a non-empty prompt".into());
    }

    let tensor = model
        .gemma4_tensors()?
        .globals
        .per_layer_token_embd
        .ok_or("gemma4 model is missing per_layer_token_embd.weight")?;
    let layout = GgufWeightLayout::from_tensors(vec![tensor.clone()])?;
    let mut loaded = layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let cpu = cpu_get_rows_loaded(&mut loaded, &tensor.name, token_ids)?;
    let metal = metal_get_rows_loaded(&mut loaded, &tensor.name, token_ids)?;
    Ok(compare_logits(&metal, &cpu))
}

fn gemma_upstream_layer_preview_check(
    args: &Args,
    model: &LlamaModel,
    token_ids: &[i32],
    layer_index: u32,
) -> Result<GemmaUpstreamLayerPreviewCheck, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma upstream preview check requires a non-empty prompt".into());
    }

    let max_context = u32::try_from(token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let output_ids = [i32::try_from(token_ids.len() - 1)?];
    let prefix = format!("hybrid_decode.layer{layer_index}");
    let checkpoint_specs = vec![
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.attn_post_norm"),
            source_names: vec![format!("{prefix}.attn_post_norm")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.attn_residual"),
            source_names: vec![format!("{prefix}.attn_residual")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.ffn_input_norm"),
            source_names: vec![format!("{prefix}.ffn.input_norm")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.ffn_out"),
            source_names: vec![format!("{prefix}.ffn.result_output")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.ffn_post_norm"),
            source_names: vec![format!("{prefix}.ffn_post_norm")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.pe_in"),
            source_names: vec![format!("{prefix}.pe_in")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.per_layer_embd_out"),
            source_names: vec![format!("{prefix}.per_layer_input.post_norm")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.post_ffn"),
            source_names: vec![format!("{prefix}.post_ffn")],
        },
    ];
    let mut rust_session =
        build_hybrid_preview_session(model, max_context, token_ids.len(), &checkpoint_specs)?;
    let rust_previews = execute_hybrid_checkpoint_session_previews(
        &mut rust_session,
        token_ids,
        &positions,
        token_ids.len(),
        &output_ids,
    )?;

    let upstream_filter_names = vec![
        format!("attn_post_norm-{layer_index}"),
        format!("attn_out-{layer_index}"),
        format!("ffn_norm-{layer_index}"),
        format!("ffn_out-{layer_index}"),
        format!("ffn_post_norm-{layer_index}"),
        format!("pe_in-{layer_index}"),
        format!("per_layer_embd_out-{layer_index}"),
        format!("out_scaled-{layer_index}"),
        format!("l_out-{layer_index}"),
    ];
    let upstream_filter_refs = upstream_filter_names
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let upstream_previews = run_upstream_debug_tensor_previews(args, &upstream_filter_refs)?;

    let upstream_attn_post_norm = format!("attn_post_norm-{layer_index}");
    let upstream_attn_out = format!("attn_out-{layer_index}");
    let upstream_ffn_norm = format!("ffn_norm-{layer_index}");
    let upstream_ffn_out = format!("ffn_out-{layer_index}");
    let upstream_ffn_post_norm = format!("ffn_post_norm-{layer_index}");
    let upstream_pe_in = format!("pe_in-{layer_index}");
    let upstream_per_layer_embd_out = format!("per_layer_embd_out-{layer_index}");
    let upstream_out_scaled = format!("out_scaled-{layer_index}");
    let upstream_l_out = format!("l_out-{layer_index}");

    Ok(GemmaUpstreamLayerPreviewCheck {
        layer_index,
        attn_post_norm_stats: compare_optional_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            &format!("layer{layer_index}.attn_post_norm"),
            &[upstream_attn_post_norm.as_str()],
        )?,
        attn_residual_stats: compare_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            &format!("layer{layer_index}.attn_residual"),
            &[upstream_attn_out.as_str()],
        )?,
        ffn_input_norm_stats: compare_optional_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            &format!("layer{layer_index}.ffn_input_norm"),
            &[upstream_ffn_norm.as_str()],
        )?,
        ffn_out_stats: compare_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            &format!("layer{layer_index}.ffn_out"),
            &[upstream_ffn_out.as_str()],
        )?,
        ffn_post_norm_stats: compare_optional_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            &format!("layer{layer_index}.ffn_post_norm"),
            &[upstream_ffn_post_norm.as_str()],
        )?,
        pe_in_stats: compare_optional_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            &format!("layer{layer_index}.pe_in"),
            &[upstream_pe_in.as_str()],
        )?,
        per_layer_embd_out_stats: compare_optional_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            &format!("layer{layer_index}.per_layer_embd_out"),
            &[upstream_per_layer_embd_out.as_str()],
        )?,
        post_ffn_stats: compare_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            &format!("layer{layer_index}.post_ffn"),
            &[upstream_out_scaled.as_str(), upstream_l_out.as_str()],
        )?,
    })
}

fn gemma_upstream_layer_output_standalone_check(
    args: &Args,
    model: &LlamaModel,
    token_ids: &[i32],
    layer_index: u32,
) -> Result<LogitComparison, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma upstream standalone layer output check requires a non-empty prompt".into());
    }

    let max_context = u32::try_from(token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let output_ids = [i32::try_from(token_ids.len() - 1)?];
    let checkpoint_specs = vec![HybridPreviewCheckpointSpec {
        label: format!("layer{layer_index}.post_ffn"),
        source_names: vec![
            format!("hybrid_decode.layer{layer_index}.post_ffn"),
            "hybrid_decode.result_hidden".to_owned(),
        ],
    }];
    let mut rust_session = build_truncated_hybrid_preview_session(
        model,
        max_context,
        token_ids.len(),
        layer_index,
        &checkpoint_specs,
    )?;
    let rust_previews = execute_hybrid_checkpoint_session_previews(
        &mut rust_session,
        token_ids,
        &positions,
        token_ids.len(),
        &output_ids,
    )?;

    let upstream_filter_names = vec![
        format!("out_scaled-{layer_index}"),
        format!("l_out-{layer_index}"),
    ];
    let upstream_filter_refs = upstream_filter_names
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let upstream_previews = run_upstream_debug_tensor_previews(args, &upstream_filter_refs)?;
    let upstream_out_scaled = format!("out_scaled-{layer_index}");
    let upstream_l_out = format!("l_out-{layer_index}");
    compare_preview_label_to_upstream_names(
        &rust_previews,
        &upstream_previews,
        &format!("layer{layer_index}.post_ffn"),
        &[upstream_out_scaled.as_str(), upstream_l_out.as_str()],
    )
}

fn gemma_upstream_layer_preview_standalone_check(
    args: &Args,
    model: &LlamaModel,
    token_ids: &[i32],
    layer_index: u32,
) -> Result<GemmaUpstreamLayerPreviewCheck, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma upstream standalone layer preview check requires a non-empty prompt".into());
    }

    let max_context = u32::try_from(token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let output_ids = [i32::try_from(token_ids.len() - 1)?];
    let prefix = format!("hybrid_decode.layer{layer_index}");
    let checkpoint_specs = vec![
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.attn_post_norm"),
            source_names: vec![format!("{prefix}.attn_post_norm")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.attn_residual"),
            source_names: vec![format!("{prefix}.attn_residual")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.post_ffn"),
            source_names: vec![
                format!("{prefix}.post_ffn"),
                "hybrid_decode.result_hidden".to_owned(),
            ],
        },
    ];
    let mut rust_session = build_truncated_hybrid_preview_session(
        model,
        max_context,
        token_ids.len(),
        layer_index,
        &checkpoint_specs,
    )?;
    let rust_previews = execute_hybrid_checkpoint_session_previews(
        &mut rust_session,
        token_ids,
        &positions,
        token_ids.len(),
        &output_ids,
    )?;

    let upstream_filter_names = vec![
        format!("attn_post_norm-{layer_index}"),
        format!("attn_out-{layer_index}"),
        format!("out_scaled-{layer_index}"),
        format!("l_out-{layer_index}"),
    ];
    let upstream_filter_refs = upstream_filter_names
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let upstream_previews = run_upstream_debug_tensor_previews(args, &upstream_filter_refs)?;

    let upstream_attn_post_norm = format!("attn_post_norm-{layer_index}");
    let upstream_attn_out = format!("attn_out-{layer_index}");
    let upstream_out_scaled = format!("out_scaled-{layer_index}");
    let upstream_l_out = format!("l_out-{layer_index}");

    Ok(GemmaUpstreamLayerPreviewCheck {
        layer_index,
        attn_post_norm_stats: compare_optional_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            &format!("layer{layer_index}.attn_post_norm"),
            &[upstream_attn_post_norm.as_str()],
        )?,
        attn_residual_stats: compare_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            &format!("layer{layer_index}.attn_residual"),
            &[upstream_attn_out.as_str()],
        )?,
        ffn_input_norm_stats: None,
        ffn_out_stats: LogitComparison {
            max_abs_diff: 0.0,
            mean_abs_diff: 0.0,
            rms_diff: 0.0,
            cosine_similarity: 1.0,
        },
        ffn_post_norm_stats: None,
        pe_in_stats: None,
        per_layer_embd_out_stats: None,
        post_ffn_stats: compare_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            &format!("layer{layer_index}.post_ffn"),
            &[upstream_out_scaled.as_str(), upstream_l_out.as_str()],
        )?,
    })
}

fn gemma_upstream_attn_post_norm_from_raw_attention_check(
    args: &Args,
    model: &LlamaModel,
    token_ids: &[i32],
    layer_index: u32,
) -> Result<LogitComparison, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err(
            "gemma upstream raw-attention post norm check requires a non-empty prompt".into(),
        );
    }

    let cfg = model.require_gemma4()?;
    let tensors = model.gemma4_tensors()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.index == layer_index)
        .ok_or_else(|| format!("missing gemma4 layer {}", layer_index))?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let layout = gemma4_attention_block_layout(model, layer_index)?;
    let mut spec = gemma4_attention_decode_spec(
        model,
        layer_index,
        u32::try_from(token_ids.len())?,
        1,
        TensorType::F32,
        TensorType::F32,
    )?;
    spec.block.residual = false;
    let run = run_attention_decode_sequence_exact(
        model,
        &layout,
        &spec,
        &positions,
        AttentionDecodeSequenceInput::TokenIds(token_ids),
    )?;

    let post_norm_layout = GgufWeightLayout::from_tensors(vec![layer.post_attention_norm.clone()])?;
    let post_norm_weights = post_norm_layout.allocate_and_load(&model.gguf)?;
    let post_norm_id = *post_norm_weights
        .tensor_ids
        .get(&layer.post_attention_norm.name)
        .ok_or("missing gemma post_attention_norm tensor id")?;
    let post_norm_tensor = post_norm_weights
        .ctx
        .tensor(post_norm_id)
        .ok_or("invalid gemma post_attention_norm tensor")?;
    let post_norm_bytes = post_norm_weights
        .ctx
        .tensor_data(post_norm_id)
        .map_err(|err| format!("failed to read gemma post_attention_norm bytes: {err}"))?;
    let post_norm_weight = tensor_values_from_tensor_bytes_f32(post_norm_tensor, post_norm_bytes)?;
    let hidden_size = usize::try_from(cfg.embedding_length)?;
    let post_norm = try_rms_norm_mul_f32(
        &run.result_output,
        &[token_ids.len(), hidden_size],
        &post_norm_weight,
        &[hidden_size],
        cfg.attention_layer_norm_rms_epsilon,
    )
    .ok_or("Metal rms_norm_mul helper failed for gemma raw attention output")?;
    let rust_preview = tensor_preview_from_values(&post_norm, hidden_size, token_ids.len())?;

    let upstream_name = format!("attn_post_norm-{layer_index}");
    let upstream_previews = run_upstream_debug_tensor_previews(args, &[upstream_name.as_str()])?;
    let upstream_preview = upstream_previews
        .get(&upstream_name)
        .ok_or_else(|| format!("missing upstream preview '{}'", upstream_name))?;
    Ok(compare_logits(&rust_preview.values, &upstream_preview.values))
}

fn gemma_hybrid_source_cache_check(
    model: &LlamaModel,
    token_ids: &[i32],
    layer_index: u32,
) -> Result<GemmaHybridSourceCacheCheck, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma hybrid source cache check requires a non-empty prompt".into());
    }

    let max_context = u32::try_from(token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let output_ids = [i32::try_from(token_ids.len() - 1)?];
    let plan = model.execution_plan()?;
    let weights = plan
        .full_weights
        .allocate_and_load_with_extra(&model.gguf, COMPARE_SHARED_HYBRID_EXTRA_CONTEXT_BYTES)?;
    let spec = model.hybrid_decode_spec(
        max_context,
        1,
        TensorType::F32,
        TensorType::F32,
        TensorType::F32,
        TensorType::F32,
    )?;
    let mut session = build_hybrid_preview_session_with_spec(weights, spec, token_ids.len(), &[])?;
    let _ = execute_hybrid_checkpoint_session(
        &mut session,
        token_ids,
        &positions,
        token_ids.len(),
        &output_ids,
    )?;
    let snapshot = capture_checkpoint_session_cache_snapshot(&session, token_ids.len())?;
    let hybrid_k = snapshot
        .attention_k
        .get(&layer_index)
        .ok_or_else(|| format!("missing hybrid attention-k snapshot for layer {}", layer_index))?;
    let hybrid_v = snapshot
        .attention_v
        .get(&layer_index)
        .ok_or_else(|| format!("missing hybrid attention-v snapshot for layer {}", layer_index))?;

    let (_, _, layout, decode_spec) =
        attention_check_setup_for_layer(model, layer_index, max_context, TensorType::F32)?;
    let decode_run = run_attention_decode_sequence_exact(
        model,
        &layout,
        &decode_spec,
        &positions,
        AttentionDecodeSequenceInput::TokenIds(token_ids),
    )?;
    let expected_k = bytes_to_f32s(&decode_run.k_cache_bytes);
    let expected_v = bytes_to_f32s(&decode_run.v_cache_bytes);
    if expected_k.len() < hybrid_k.len() || expected_v.len() < hybrid_v.len() {
        return Err(format!(
            "standalone source cache output for layer {} was shorter than hybrid snapshot",
            layer_index
        )
        .into());
    }

    Ok(GemmaHybridSourceCacheCheck {
        layer_index,
        k_cache_stats: compare_logits(hybrid_k, &expected_k[..hybrid_k.len()]),
        v_cache_stats: compare_logits(hybrid_v, &expected_v[..hybrid_v.len()]),
    })
}

fn gemma_upstream_tail_check(
    args: &Args,
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<GemmaUpstreamTailCheck, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma upstream tail check requires a non-empty prompt".into());
    }

    let max_context = u32::try_from(token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let output_ids = [i32::try_from(token_ids.len() - 1)?];
    let checkpoint_specs = vec![
        HybridPreviewCheckpointSpec {
            label: "model.result_norm".to_owned(),
            source_names: vec!["hybrid_decode.result_norm".to_owned()],
        },
        HybridPreviewCheckpointSpec {
            label: "model.result_logits".to_owned(),
            source_names: vec!["hybrid_decode.result_logits".to_owned()],
        },
    ];
    let mut rust_session =
        build_hybrid_preview_session(model, max_context, token_ids.len(), &checkpoint_specs)?;
    let rust_previews = execute_hybrid_checkpoint_session_previews(
        &mut rust_session,
        token_ids,
        &positions,
        token_ids.len(),
        &output_ids,
    )?;
    let upstream_previews =
        run_upstream_debug_tensor_previews(args, &["result_norm", "result_output"])?;

    Ok(GemmaUpstreamTailCheck {
        result_norm_stats: compare_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            "model.result_norm",
            &["result_norm"],
        )?,
        result_output_stats: compare_preview_label_to_upstream_names(
            &rust_previews,
            &upstream_previews,
            "model.result_logits",
            &["result_output"],
        )?,
    })
}

fn gemma_final_probe_check(
    model: &LlamaModel,
    token_ids: &[i32],
    upstream_logits: &[f32],
) -> Result<GemmaFinalProbeCheck, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma final probe check requires a non-empty prompt".into());
    }

    let max_context = u32::try_from(token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let output_ids = [i32::try_from(token_ids.len() - 1)?];
    let checkpoint_specs = vec![
        HybridPreviewCheckpointSpec {
            label: "model.result_hidden".to_owned(),
            source_names: vec!["hybrid_decode.result_hidden".to_owned()],
        },
        HybridPreviewCheckpointSpec {
            label: "model.result_logits".to_owned(),
            source_names: vec!["hybrid_decode.result_logits".to_owned()],
        },
    ];
    let mut session = build_hybrid_hidden_token_capture_session(
        model,
        max_context,
        token_ids.len(),
        TensorType::F16,
        TensorType::F16,
        &checkpoint_specs,
    )?;
    let outputs = execute_hybrid_checkpoint_session(
        &mut session,
        token_ids,
        &positions,
        token_ids.len(),
        &output_ids,
    )?;
    let result_hidden = outputs
        .get("model.result_hidden")
        .ok_or("missing model.result_hidden checkpoint")?;
    let hybrid_logits = outputs
        .get("model.result_logits")
        .ok_or("missing model.result_logits checkpoint")?;
    let probe_spec = gemma4_embedding_logits_probe_spec(model)?;
    let probe_run = execute_logits_probe_metal(
        &session.weights,
        &probe_spec,
        LogitsProbeInput::EmbeddingsF32 {
            data: result_hidden,
            n_tokens: 1,
        },
        &[0],
    )?;

    Ok(GemmaFinalProbeCheck {
        probe_vs_hybrid_stats: compare_logits(&probe_run.logits, hybrid_logits),
        probe_vs_upstream_stats: compare_logits(&probe_run.logits, upstream_logits),
    })
}

fn gemma_layer_manual_graph_check(
    model: &LlamaModel,
    token_ids: &[i32],
    layer_index: u32,
    attention_cache_type: TensorType,
) -> Result<GemmaLayerManualGraphCheck, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma manual layer graph check requires a non-empty prompt".into());
    }

    let max_context = u32::try_from(token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let output_ids = [i32::try_from(token_ids.len() - 1)?];
    let prefix = format!("hybrid_decode.layer{layer_index}");
    let checkpoint_specs = vec![
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.attn_post_norm"),
            source_names: vec![format!("{prefix}.attn_post_norm")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.attn_residual"),
            source_names: vec![format!("{prefix}.attn_residual")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.ffn_input_norm"),
            source_names: vec![format!("{prefix}.ffn.input_norm")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.ffn_out"),
            source_names: vec![format!("{prefix}.ffn.result_output")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.ffn_post_norm"),
            source_names: vec![format!("{prefix}.ffn_post_norm")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.pe_in"),
            source_names: vec![format!("{prefix}.pe_in")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.per_layer_embd_out"),
            source_names: vec![format!("{prefix}.per_layer_input.post_norm")],
        },
        HybridPreviewCheckpointSpec {
            label: format!("layer{layer_index}.post_ffn"),
            source_names: vec![format!("{prefix}.post_ffn")],
        },
    ];
    let mut session = build_hybrid_hidden_token_capture_session(
        model,
        max_context,
        token_ids.len(),
        attention_cache_type,
        attention_cache_type,
        &checkpoint_specs,
    )?;
    let graph_outputs = execute_hybrid_checkpoint_session(
        &mut session,
        token_ids,
        &positions,
        token_ids.len(),
        &output_ids,
    )?;
    let manual_outputs = gemma_layer_manual_reference(model, token_ids, layer_index)?;

    Ok(GemmaLayerManualGraphCheck {
        layer_index,
        attn_post_norm_stats: compare_named_checkpoint(
            &graph_outputs,
            &manual_outputs,
            &format!("layer{layer_index}.attn_post_norm"),
        )?,
        attn_residual_stats: compare_named_checkpoint(
            &graph_outputs,
            &manual_outputs,
            &format!("layer{layer_index}.attn_residual"),
        )?,
        ffn_input_norm_stats: compare_named_checkpoint(
            &graph_outputs,
            &manual_outputs,
            &format!("layer{layer_index}.ffn_input_norm"),
        )?,
        ffn_out_stats: compare_named_checkpoint(
            &graph_outputs,
            &manual_outputs,
            &format!("layer{layer_index}.ffn_out"),
        )?,
        ffn_post_norm_stats: compare_named_checkpoint(
            &graph_outputs,
            &manual_outputs,
            &format!("layer{layer_index}.ffn_post_norm"),
        )?,
        pe_in_stats: compare_named_checkpoint(
            &graph_outputs,
            &manual_outputs,
            &format!("layer{layer_index}.pe_in"),
        )?,
        per_layer_embd_out_stats: compare_optional_named_checkpoint(
            &graph_outputs,
            &manual_outputs,
            &format!("layer{layer_index}.per_layer_embd_out"),
        )?,
        post_ffn_stats: compare_named_checkpoint(
            &graph_outputs,
            &manual_outputs,
            &format!("layer{layer_index}.post_ffn"),
        )?,
    })
}

fn gemma_shared_per_layer_manual_check(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<GemmaSharedPerLayerManualCheck, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma shared per-layer manual check requires a non-empty prompt".into());
    }

    let max_context = u32::try_from(token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let output_ids = [i32::try_from(token_ids.len() - 1)?];
    let checkpoint_specs = vec![
        HybridPreviewCheckpointSpec {
            label: "shared_per_layer.selected".to_owned(),
            source_names: vec!["hybrid_decode.per_layer_input.selected_reshaped".to_owned()],
        },
        HybridPreviewCheckpointSpec {
            label: "shared_per_layer.proj".to_owned(),
            source_names: vec!["hybrid_decode.per_layer_input.model_proj_norm".to_owned()],
        },
        HybridPreviewCheckpointSpec {
            label: "shared_per_layer.input".to_owned(),
            source_names: vec!["hybrid_decode.per_layer_input.combined_scaled".to_owned()],
        },
    ];
    let mut session = build_hybrid_token_dim2_capture_session(
        model,
        max_context,
        token_ids.len(),
        TensorType::F32,
        TensorType::F32,
        &checkpoint_specs,
    )?;
    let graph_outputs = execute_hybrid_checkpoint_session(
        &mut session,
        token_ids,
        &positions,
        token_ids.len(),
        &output_ids,
    )?;
    let manual_outputs = gemma_layer_manual_reference(model, token_ids, 0)?;

    Ok(GemmaSharedPerLayerManualCheck {
        selected_stats: compare_named_checkpoint(
            &graph_outputs,
            &manual_outputs,
            "shared_per_layer.selected",
        )?,
        proj_stats: compare_named_checkpoint(
            &graph_outputs,
            &manual_outputs,
            "shared_per_layer.proj",
        )?,
        input_stats: compare_named_checkpoint(
            &graph_outputs,
            &manual_outputs,
            "shared_per_layer.input",
        )?,
    })
}

fn gemma_layer_manual_reference(
    model: &LlamaModel,
    token_ids: &[i32],
    layer_index: u32,
) -> Result<BTreeMap<String, Vec<f32>>, Box<dyn std::error::Error>> {
    if token_ids.is_empty() {
        return Err("gemma manual layer reference requires a non-empty prompt".into());
    }

    let cfg = model.require_gemma4()?;
    let tensors = model.gemma4_tensors()?;
    let layer = tensors
        .layers
        .iter()
        .find(|layer| layer.index == layer_index)
        .ok_or_else(|| format!("missing gemma4 layer {}", layer_index))?;
    let hidden_size = usize::try_from(cfg.embedding_length)?;
    let n_tokens = token_ids.len();

    let mut manual_tensors = vec![
        tensors.globals.token_embd.clone(),
        layer.post_attention_norm.clone(),
        layer.ffn_norm.clone(),
        layer.ffn_gate.clone(),
        layer.ffn_up.clone(),
        layer.ffn_down.clone(),
        layer.post_ffw_norm.clone(),
    ];
    if let Some(tensor) = tensors.globals.per_layer_token_embd.as_ref() {
        manual_tensors.push(tensor.clone());
    }
    if let Some(tensor) = tensors.globals.per_layer_model_proj.as_ref() {
        manual_tensors.push(tensor.clone());
    }
    if let Some(tensor) = tensors.globals.per_layer_proj_norm.as_ref() {
        manual_tensors.push(tensor.clone());
    }
    if let Some(tensor) = layer.per_layer_inp_gate.as_ref() {
        manual_tensors.push(tensor.clone());
    }
    if let Some(tensor) = layer.per_layer_proj.as_ref() {
        manual_tensors.push(tensor.clone());
    }
    if let Some(tensor) = layer.per_layer_post_norm.as_ref() {
        manual_tensors.push(tensor.clone());
    }
    if let Some(tensor) = layer.layer_output_scale.as_ref() {
        manual_tensors.push(tensor.clone());
    }

    let manual_layout = GgufWeightLayout::from_tensors(manual_tensors)?;
    let mut loaded =
        manual_layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;

    let mut outputs = BTreeMap::new();

    let mut input_embed =
        cpu_get_rows_loaded(&mut loaded, &tensors.globals.token_embd.name, token_ids)?;
    cpu_scale_inplace(
        &mut input_embed,
        (cfg.embedding_length as f32).sqrt(),
    );

    let positions = (0..n_tokens)
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let layout = gemma4_attention_block_layout(model, layer_index)?;
    let mut spec = gemma4_attention_decode_spec(
        model,
        layer_index,
        u32::try_from(n_tokens)?,
        1,
        TensorType::F32,
        TensorType::F32,
    )?;
    spec.block.residual = false;
    let attn_run = run_attention_decode_sequence_exact(
        model,
        &layout,
        &spec,
        &positions,
        AttentionDecodeSequenceInput::TokenIds(token_ids),
    )?;

    let attn_post_norm_weight =
        read_loaded_tensor_values_f32(&loaded, &layer.post_attention_norm.name)?;
    let attn_post_norm = cpu_rms_norm_mul_rows(
        &attn_run.result_output,
        hidden_size,
        n_tokens,
        &attn_post_norm_weight,
        cfg.attention_layer_norm_rms_epsilon,
    )?;
    outputs.insert(
        format!("layer{layer_index}.attn_post_norm"),
        last_token_slice(&attn_post_norm, hidden_size)?.to_vec(),
    );

    let attn_residual = cpu_add_rows(&attn_post_norm, &input_embed)?;
    outputs.insert(
        format!("layer{layer_index}.attn_residual"),
        last_token_slice(&attn_residual, hidden_size)?.to_vec(),
    );

    let ffn_norm_weight = read_loaded_tensor_values_f32(&loaded, &layer.ffn_norm.name)?;
    let ffn_input_norm = cpu_rms_norm_mul_rows(
        &attn_residual,
        hidden_size,
        n_tokens,
        &ffn_norm_weight,
        cfg.attention_layer_norm_rms_epsilon,
    )?;
    outputs.insert(
        format!("layer{layer_index}.ffn_input_norm"),
        last_token_slice(&ffn_input_norm, hidden_size)?.to_vec(),
    );

    let ffn_gate =
        cpu_mul_mat_loaded(&mut loaded, &layer.ffn_gate.name, &ffn_input_norm, n_tokens)?;
    let ffn_up = cpu_mul_mat_loaded(&mut loaded, &layer.ffn_up.name, &ffn_input_norm, n_tokens)?;
    let ffn_hidden = cpu_geglu_rows(&ffn_gate, &ffn_up)?;
    let ffn_out = cpu_mul_mat_loaded(&mut loaded, &layer.ffn_down.name, &ffn_hidden, n_tokens)?;
    outputs.insert(
        format!("layer{layer_index}.ffn_out"),
        last_token_slice(&ffn_out, hidden_size)?.to_vec(),
    );

    let ffn_post_norm_weight = read_loaded_tensor_values_f32(&loaded, &layer.post_ffw_norm.name)?;
    let ffn_post_norm = cpu_rms_norm_mul_rows(
        &ffn_out,
        hidden_size,
        n_tokens,
        &ffn_post_norm_weight,
        cfg.attention_layer_norm_rms_epsilon,
    )?;
    outputs.insert(
        format!("layer{layer_index}.ffn_post_norm"),
        last_token_slice(&ffn_post_norm, hidden_size)?.to_vec(),
    );

    let pe_in = cpu_add_rows(&ffn_post_norm, &attn_residual)?;
    outputs.insert(
        format!("layer{layer_index}.pe_in"),
        last_token_slice(&pe_in, hidden_size)?.to_vec(),
    );

    let mut post_ffn = pe_in.clone();
    if cfg.embedding_length_per_layer_input != 0 {
        let hidden_per_layer = usize::try_from(cfg.embedding_length_per_layer_input)?;
        let layer_count = usize::try_from(cfg.block_count)?;
        let per_layer_token_name = tensors
            .globals
            .per_layer_token_embd
            .as_ref()
            .ok_or("missing gemma per_layer_token_embd")?
            .name
            .clone();
        let per_layer_model_proj_name = tensors
            .globals
            .per_layer_model_proj
            .as_ref()
            .ok_or("missing gemma per_layer_model_proj")?
            .name
            .clone();
        let per_layer_proj_norm_name = tensors
            .globals
            .per_layer_proj_norm
            .as_ref()
            .ok_or("missing gemma per_layer_proj_norm")?
            .name
            .clone();
        let per_layer_inp_gate_name = layer
            .per_layer_inp_gate
            .as_ref()
            .ok_or("missing gemma per_layer_inp_gate")?
            .name
            .clone();
        let per_layer_proj_name = layer
            .per_layer_proj
            .as_ref()
            .ok_or("missing gemma per_layer_proj")?
            .name
            .clone();
        let per_layer_post_norm_name = layer
            .per_layer_post_norm
            .as_ref()
            .ok_or("missing gemma per_layer_post_norm")?
            .name
            .clone();

        let mut per_layer_selected =
            cpu_get_rows_loaded(&mut loaded, &per_layer_token_name, token_ids)?;
        cpu_scale_inplace(
            &mut per_layer_selected,
            (cfg.embedding_length_per_layer_input as f32).sqrt(),
        );
        let shared_width = hidden_per_layer
            .checked_mul(layer_count)
            .ok_or("overflow computing gemma shared per-layer width")?;
        outputs.insert(
            "shared_per_layer.selected".to_owned(),
            last_token_slice(&per_layer_selected, shared_width)?.to_vec(),
        );
        let mut per_layer_model_proj = cpu_mul_mat_loaded(
            &mut loaded,
            &per_layer_model_proj_name,
            &input_embed,
            n_tokens,
        )?;
        cpu_scale_inplace(
            &mut per_layer_model_proj,
            1.0 / (cfg.embedding_length as f32).sqrt(),
        );
        let per_layer_proj_norm_weight =
            read_loaded_tensor_values_f32(&loaded, &per_layer_proj_norm_name)?;
        let per_layer_model_proj = cpu_rms_norm_mul_rows(
            &per_layer_model_proj,
            hidden_per_layer,
            n_tokens
                .checked_mul(layer_count)
                .ok_or("overflow computing gemma per-layer norm row count")?,
            &per_layer_proj_norm_weight,
            cfg.attention_layer_norm_rms_epsilon,
        )?;
        outputs.insert(
            "shared_per_layer.proj".to_owned(),
            last_token_slice(&per_layer_model_proj, shared_width)?.to_vec(),
        );
        let mut shared_per_layer_inputs =
            cpu_add_rows(&per_layer_model_proj, &per_layer_selected)?;
        cpu_scale_inplace(&mut shared_per_layer_inputs, 1.0 / 2.0f32.sqrt());
        outputs.insert(
            "shared_per_layer.input".to_owned(),
            last_token_slice(&shared_per_layer_inputs, shared_width)?.to_vec(),
        );
        let layer_inputs = cpu_extract_interleaved_layer_rows(
            &shared_per_layer_inputs,
            hidden_per_layer,
            layer_count,
            usize::try_from(layer_index)?,
            n_tokens,
        )?;

        let per_layer_gate =
            cpu_mul_mat_loaded(&mut loaded, &per_layer_inp_gate_name, &pe_in, n_tokens)?;
        let per_layer_gate = cpu_gelu_rows(&per_layer_gate)?;
        let per_layer_gated = cpu_mul_rows_broadcast(&per_layer_gate, hidden_per_layer, &layer_inputs)?;
        let per_layer_proj = cpu_mul_mat_loaded(
            &mut loaded,
            &per_layer_proj_name,
            &per_layer_gated,
            n_tokens,
        )?;
        let per_layer_post_norm_weight =
            read_loaded_tensor_values_f32(&loaded, &per_layer_post_norm_name)?;
        let per_layer_embd_out = cpu_rms_norm_mul_rows(
            &per_layer_proj,
            hidden_size,
            n_tokens,
            &per_layer_post_norm_weight,
            cfg.attention_layer_norm_rms_epsilon,
        )?;
        outputs.insert(
            format!("layer{layer_index}.per_layer_embd_out"),
            last_token_slice(&per_layer_embd_out, hidden_size)?.to_vec(),
        );
        post_ffn = cpu_add_rows(&pe_in, &per_layer_embd_out)?;
    }

    if let Some(scale_name) = layer.layer_output_scale.as_ref().map(|tensor| tensor.name.as_str()) {
        let scale = read_loaded_tensor_values_f32(&loaded, scale_name)?;
        post_ffn = cpu_mul_rows_broadcast(&post_ffn, hidden_size, &scale)?;
    }

    outputs.insert(
        format!("layer{layer_index}.post_ffn"),
        last_token_slice(&post_ffn, hidden_size)?.to_vec(),
    );

    Ok(outputs)
}

#[allow(dead_code)]
fn capture_checkpoint_session_cache_snapshot(
    session: &HybridCheckpointSession,
    cache_tokens: usize,
) -> Result<HybridCacheSnapshot, Box<dyn std::error::Error>> {
    let runtime = session.session.runtime();
    let main_buffer = &session.session.compiled().main_buffer;
    let mut snapshot = HybridCacheSnapshot::default();

    for (&layer_index, ids) in &session.shared_cache.attention {
        snapshot.attention_k.insert(
            layer_index,
            read_attention_cache_prefix_values_f32(
                runtime,
                main_buffer,
                &session.weights.ctx,
                ids.k_cache,
                cache_tokens,
            )?,
        );
        snapshot.attention_v.insert(
            layer_index,
            read_attention_cache_prefix_values_f32(
                runtime,
                main_buffer,
                &session.weights.ctx,
                ids.v_cache,
                cache_tokens,
            )?,
        );
    }

    for (&layer_index, ids) in &session.shared_cache.recurrent {
        snapshot.recurrent_r.insert(
            layer_index,
            read_full_tensor_values_f32(runtime, main_buffer, &session.weights.ctx, ids.r_cache)?,
        );
        snapshot.recurrent_s.insert(
            layer_index,
            read_full_tensor_values_f32(runtime, main_buffer, &session.weights.ctx, ids.s_cache)?,
        );
    }

    Ok(snapshot)
}

fn write_hybrid_cache_snapshot(
    ctx: &mut Context,
    shared_cache: &HybridSharedCacheTensorIds,
    snapshot: &HybridCacheSnapshot,
) -> Result<(), Box<dyn std::error::Error>> {
    for (&layer_index, ids) in &shared_cache.attention {
        let k_values = snapshot
            .attention_k
            .get(&layer_index)
            .ok_or_else(|| format!("attention-k snapshot missing layer {}", layer_index))?;
        write_tensor_f32_prefix_snapshot(ctx, ids.k_cache, k_values)?;
        let v_values = snapshot
            .attention_v
            .get(&layer_index)
            .ok_or_else(|| format!("attention-v snapshot missing layer {}", layer_index))?;
        write_tensor_f32_prefix_snapshot(ctx, ids.v_cache, v_values)?;
    }

    for (&layer_index, ids) in &shared_cache.recurrent {
        let r_values = snapshot
            .recurrent_r
            .get(&layer_index)
            .ok_or_else(|| format!("recurrent-r snapshot missing layer {}", layer_index))?;
        write_tensor_f32_prefix_snapshot(ctx, ids.r_cache, r_values)?;
        let s_values = snapshot
            .recurrent_s
            .get(&layer_index)
            .ok_or_else(|| format!("recurrent-s snapshot missing layer {}", layer_index))?;
        write_tensor_f32_prefix_snapshot(ctx, ids.s_cache, s_values)?;
    }

    Ok(())
}

fn write_tensor_f32_prefix_snapshot(
    ctx: &mut Context,
    tensor_id: TensorId,
    values: &[f32],
) -> Result<(), Box<dyn std::error::Error>> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid snapshot tensor id {}", tensor_id))?
        .clone();
    let mut bytes = vec![0u8; tensor.nbytes()];
    let prefix = match tensor.desc.ty {
        TensorType::F32 => f32s_to_bytes(values),
        TensorType::F16 => values
            .iter()
            .flat_map(|value| f32_to_f16(*value).to_le_bytes())
            .collect(),
        other => {
            return Err(format!(
                "snapshot write does not support tensor '{}' type {}",
                tensor.name().unwrap_or("<unnamed>"),
                other.name()
            )
            .into())
        }
    };
    if prefix.len() > bytes.len() {
        return Err(format!(
            "snapshot write for tensor '{}' exceeds storage: {} > {}",
            tensor.name().unwrap_or("<unnamed>"),
            prefix.len(),
            bytes.len()
        )
        .into());
    }
    bytes[..prefix.len()].copy_from_slice(&prefix);
    ctx.write_tensor_data(tensor_id, &bytes)?;
    Ok(())
}

#[allow(dead_code)]
fn compare_hybrid_prompt_split_checkpoints(
    model: &LlamaModel,
    prompt_token_ids: &[i32],
) -> Result<HybridSplitCheckpointDiff, Box<dyn std::error::Error>> {
    if prompt_token_ids.len() < 2 {
        return Err("hybrid prompt split checkpoint compare requires at least two tokens".into());
    }
    let total_tokens = 2usize;
    let max_context = u32::try_from(total_tokens)?;
    let prompt_first = [prompt_token_ids[0]];
    let prompt_second = [prompt_token_ids[1]];

    let mut full_session = build_hybrid_checkpoint_session(model, max_context, total_tokens)?;
    let full_outputs = execute_hybrid_checkpoint_session(
        &mut full_session,
        &prompt_token_ids[..2],
        &[0, 1],
        total_tokens,
        &[1],
    )?;

    let mut split_session = build_hybrid_checkpoint_session(model, max_context, 1)?;
    let _ = execute_hybrid_checkpoint_session(&mut split_session, &prompt_first, &[0], 1, &[0])?;
    let split_outputs =
        execute_hybrid_checkpoint_session(&mut split_session, &prompt_second, &[1], 2, &[0])?;

    let mut layers = Vec::with_capacity(full_session.spec.layers.len());
    for layer in &full_session.spec.layers {
        let layer_index = match layer {
            HybridLayerSpec::Attention { layer_index, .. }
            | HybridLayerSpec::Recurrent { layer_index, .. } => *layer_index,
        };
        layers.push(HybridSplitLayerCheckpointDiff {
            layer_index,
            attn_post_norm: compare_optional_named_checkpoint(
                &split_outputs,
                &full_outputs,
                &format!("layer{layer_index}.attn_post_norm"),
            )?,
            attn_residual: compare_named_checkpoint(
                &split_outputs,
                &full_outputs,
                &format!("layer{layer_index}.attn_residual"),
            )?,
            ffn_input_norm: compare_optional_named_checkpoint(
                &split_outputs,
                &full_outputs,
                &format!("layer{layer_index}.ffn_input_norm"),
            )?,
            ffn_out: compare_named_checkpoint(
                &split_outputs,
                &full_outputs,
                &format!("layer{layer_index}.ffn_out"),
            )?,
            ffn_post_norm: compare_optional_named_checkpoint(
                &split_outputs,
                &full_outputs,
                &format!("layer{layer_index}.ffn_post_norm"),
            )?,
            post_ffn: compare_named_checkpoint(
                &split_outputs,
                &full_outputs,
                &format!("layer{layer_index}.post_ffn"),
            )?,
        });
    }

    Ok(HybridSplitCheckpointDiff {
        input_embed: compare_named_checkpoint(&split_outputs, &full_outputs, "model.input_embed")?,
        result_norm: compare_named_checkpoint(&split_outputs, &full_outputs, "model.result_norm")?,
        result_logits: compare_named_checkpoint(
            &split_outputs,
            &full_outputs,
            "model.result_logits",
        )?,
        layers,
    })
}

#[allow(dead_code)]
fn compare_hybrid_first_step_capacity_checkpoints(
    model: &LlamaModel,
    token_id: i32,
    max_context: u32,
) -> Result<HybridCheckpointAndCacheDiff, Box<dyn std::error::Error>> {
    if max_context <= 1 {
        return Err("first-step capacity compare requires max_context > 1".into());
    }

    let token_ids = [token_id];
    let positions = [0_i32];
    let output_ids = [0_i32];

    let mut reference_session = build_hybrid_checkpoint_session(model, 1, 1)?;
    let reference_outputs = execute_hybrid_checkpoint_session(
        &mut reference_session,
        &token_ids,
        &positions,
        1,
        &output_ids,
    )?;
    let reference_cache = capture_checkpoint_session_cache_snapshot(&reference_session, 1)?;

    let mut wide_session = build_hybrid_checkpoint_session(model, max_context, 1)?;
    let wide_outputs = execute_hybrid_checkpoint_session(
        &mut wide_session,
        &token_ids,
        &positions,
        1,
        &output_ids,
    )?;
    let wide_cache = capture_checkpoint_session_cache_snapshot(&wide_session, 1)?;

    let mut layers = Vec::with_capacity(reference_session.spec.layers.len());
    for layer in &reference_session.spec.layers {
        let layer_index = match layer {
            HybridLayerSpec::Attention { layer_index, .. }
            | HybridLayerSpec::Recurrent { layer_index, .. } => *layer_index,
        };
        layers.push(HybridSplitLayerCheckpointDiff {
            layer_index,
            attn_post_norm: compare_optional_named_checkpoint(
                &wide_outputs,
                &reference_outputs,
                &format!("layer{layer_index}.attn_post_norm"),
            )?,
            attn_residual: compare_named_checkpoint(
                &wide_outputs,
                &reference_outputs,
                &format!("layer{layer_index}.attn_residual"),
            )?,
            ffn_input_norm: compare_optional_named_checkpoint(
                &wide_outputs,
                &reference_outputs,
                &format!("layer{layer_index}.ffn_input_norm"),
            )?,
            ffn_out: compare_named_checkpoint(
                &wide_outputs,
                &reference_outputs,
                &format!("layer{layer_index}.ffn_out"),
            )?,
            ffn_post_norm: compare_optional_named_checkpoint(
                &wide_outputs,
                &reference_outputs,
                &format!("layer{layer_index}.ffn_post_norm"),
            )?,
            post_ffn: compare_named_checkpoint(
                &wide_outputs,
                &reference_outputs,
                &format!("layer{layer_index}.post_ffn"),
            )?,
        });
    }

    let mut cache = HybridCacheDiffSummary::default();
    for (&layer_index, reference_values) in &reference_cache.attention_k {
        let wide_values = wide_cache
            .attention_k
            .get(&layer_index)
            .ok_or_else(|| format!("wide attention-k snapshot missing layer {}", layer_index))?;
        update_layer_diff_summary(
            &mut cache.attention_k,
            layer_index,
            compare_logits(wide_values, reference_values),
        );
    }
    for (&layer_index, reference_values) in &reference_cache.attention_v {
        let wide_values = wide_cache
            .attention_v
            .get(&layer_index)
            .ok_or_else(|| format!("wide attention-v snapshot missing layer {}", layer_index))?;
        update_layer_diff_summary(
            &mut cache.attention_v,
            layer_index,
            compare_logits(wide_values, reference_values),
        );
    }
    for (&layer_index, reference_values) in &reference_cache.recurrent_r {
        let wide_values = wide_cache
            .recurrent_r
            .get(&layer_index)
            .ok_or_else(|| format!("wide recurrent-r snapshot missing layer {}", layer_index))?;
        update_layer_diff_summary(
            &mut cache.recurrent_r,
            layer_index,
            compare_logits(wide_values, reference_values),
        );
    }
    for (&layer_index, reference_values) in &reference_cache.recurrent_s {
        let wide_values = wide_cache
            .recurrent_s
            .get(&layer_index)
            .ok_or_else(|| format!("wide recurrent-s snapshot missing layer {}", layer_index))?;
        update_layer_diff_summary(
            &mut cache.recurrent_s,
            layer_index,
            compare_logits(wide_values, reference_values),
        );
    }

    Ok(HybridCheckpointAndCacheDiff {
        checkpoints: HybridSplitCheckpointDiff {
            input_embed: compare_named_checkpoint(
                &wide_outputs,
                &reference_outputs,
                "model.input_embed",
            )?,
            result_norm: compare_named_checkpoint(
                &wide_outputs,
                &reference_outputs,
                "model.result_norm",
            )?,
            result_logits: compare_named_checkpoint(
                &wide_outputs,
                &reference_outputs,
                "model.result_logits",
            )?,
            layers,
        },
        cache,
    })
}

fn compare_hybrid_first_step_shared_cache_checkpoints(
    model: &LlamaModel,
    token_id: i32,
    max_context: u32,
) -> Result<HybridSplitCheckpointDiff, Box<dyn std::error::Error>> {
    let token_ids = [token_id];
    let positions = [0_i32];
    let output_ids = [0_i32];

    let mut internal_session =
        build_hybrid_checkpoint_session_with_shared_cache(model, max_context, 1, false)?;
    let internal_outputs = execute_hybrid_checkpoint_session(
        &mut internal_session,
        &token_ids,
        &positions,
        1,
        &output_ids,
    )?;

    let mut shared_session =
        build_hybrid_checkpoint_session_with_shared_cache(model, max_context, 1, true)?;
    let shared_outputs = execute_hybrid_checkpoint_session(
        &mut shared_session,
        &token_ids,
        &positions,
        1,
        &output_ids,
    )?;

    let mut layers = Vec::with_capacity(shared_session.spec.layers.len());
    for layer in &shared_session.spec.layers {
        let layer_index = match layer {
            HybridLayerSpec::Attention { layer_index, .. }
            | HybridLayerSpec::Recurrent { layer_index, .. } => *layer_index,
        };
        layers.push(HybridSplitLayerCheckpointDiff {
            layer_index,
            attn_post_norm: compare_optional_named_checkpoint(
                &shared_outputs,
                &internal_outputs,
                &format!("layer{layer_index}.attn_post_norm"),
            )?,
            attn_residual: compare_named_checkpoint(
                &shared_outputs,
                &internal_outputs,
                &format!("layer{layer_index}.attn_residual"),
            )?,
            ffn_input_norm: compare_optional_named_checkpoint(
                &shared_outputs,
                &internal_outputs,
                &format!("layer{layer_index}.ffn_input_norm"),
            )?,
            ffn_out: compare_named_checkpoint(
                &shared_outputs,
                &internal_outputs,
                &format!("layer{layer_index}.ffn_out"),
            )?,
            ffn_post_norm: compare_optional_named_checkpoint(
                &shared_outputs,
                &internal_outputs,
                &format!("layer{layer_index}.ffn_post_norm"),
            )?,
            post_ffn: compare_named_checkpoint(
                &shared_outputs,
                &internal_outputs,
                &format!("layer{layer_index}.post_ffn"),
            )?,
        });
    }

    Ok(HybridSplitCheckpointDiff {
        input_embed: compare_named_checkpoint(
            &shared_outputs,
            &internal_outputs,
            "model.input_embed",
        )?,
        result_norm: compare_named_checkpoint(
            &shared_outputs,
            &internal_outputs,
            "model.result_norm",
        )?,
        result_logits: compare_named_checkpoint(
            &shared_outputs,
            &internal_outputs,
            "model.result_logits",
        )?,
        layers,
    })
}

fn compare_hybrid_continue_checkpoints(
    model: &LlamaModel,
    prompt_token_ids: &[i32],
    continue_token_id: i32,
) -> Result<HybridSplitCheckpointDiff, Box<dyn std::error::Error>> {
    if prompt_token_ids.is_empty() {
        return Err("hybrid continue checkpoint compare requires a non-empty prompt".into());
    }

    let total_tokens = prompt_token_ids
        .len()
        .checked_add(1)
        .ok_or("overflow computing hybrid continue token count")?;
    let max_context = u32::try_from(total_tokens)?;

    let mut full_token_ids = prompt_token_ids.to_vec();
    full_token_ids.push(continue_token_id);
    let full_positions = (0..full_token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let full_output_ids = [i32::try_from(full_token_ids.len() - 1)?];

    let mut full_session = build_hybrid_checkpoint_session(model, max_context, total_tokens)?;
    let full_outputs = execute_hybrid_checkpoint_session(
        &mut full_session,
        &full_token_ids,
        &full_positions,
        total_tokens,
        &full_output_ids,
    )?;

    let prompt_positions = (0..prompt_token_ids.len())
        .map(i32::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    let prompt_output_ids = [i32::try_from(prompt_token_ids.len() - 1)?];
    let mut prompt_session =
        build_hybrid_checkpoint_session(model, max_context, prompt_token_ids.len())?;
    let _ = execute_hybrid_checkpoint_session(
        &mut prompt_session,
        prompt_token_ids,
        &prompt_positions,
        prompt_token_ids.len(),
        &prompt_output_ids,
    )?;
    let prompt_cache =
        capture_checkpoint_session_cache_snapshot(&prompt_session, prompt_token_ids.len())?;

    let continue_positions = [i32::try_from(prompt_token_ids.len())?];
    let continue_output_ids = [0_i32];
    let mut split_session =
        build_hybrid_checkpoint_session_with_seeded_cache(model, max_context, 1, &prompt_cache)?;
    let split_outputs = execute_hybrid_checkpoint_session(
        &mut split_session,
        std::slice::from_ref(&continue_token_id),
        &continue_positions,
        total_tokens,
        &continue_output_ids,
    )?;

    let mut layers = Vec::with_capacity(full_session.spec.layers.len());
    for layer in &full_session.spec.layers {
        let layer_index = match layer {
            HybridLayerSpec::Attention { layer_index, .. }
            | HybridLayerSpec::Recurrent { layer_index, .. } => *layer_index,
        };
        layers.push(HybridSplitLayerCheckpointDiff {
            layer_index,
            attn_post_norm: compare_optional_named_checkpoint(
                &split_outputs,
                &full_outputs,
                &format!("layer{layer_index}.attn_post_norm"),
            )?,
            attn_residual: compare_named_checkpoint(
                &split_outputs,
                &full_outputs,
                &format!("layer{layer_index}.attn_residual"),
            )?,
            ffn_input_norm: compare_optional_named_checkpoint(
                &split_outputs,
                &full_outputs,
                &format!("layer{layer_index}.ffn_input_norm"),
            )?,
            ffn_out: compare_named_checkpoint(
                &split_outputs,
                &full_outputs,
                &format!("layer{layer_index}.ffn_out"),
            )?,
            ffn_post_norm: compare_optional_named_checkpoint(
                &split_outputs,
                &full_outputs,
                &format!("layer{layer_index}.ffn_post_norm"),
            )?,
            post_ffn: compare_named_checkpoint(
                &split_outputs,
                &full_outputs,
                &format!("layer{layer_index}.post_ffn"),
            )?,
        });
    }

    Ok(HybridSplitCheckpointDiff {
        input_embed: compare_named_checkpoint(&split_outputs, &full_outputs, "model.input_embed")?,
        result_norm: compare_named_checkpoint(&split_outputs, &full_outputs, "model.result_norm")?,
        result_logits: compare_named_checkpoint(
            &split_outputs,
            &full_outputs,
            "model.result_logits",
        )?,
        layers,
    })
}

fn compare_named_checkpoint(
    lhs: &BTreeMap<String, Vec<f32>>,
    rhs: &BTreeMap<String, Vec<f32>>,
    label: &str,
) -> Result<LogitComparison, Box<dyn std::error::Error>> {
    let lhs = lhs
        .get(label)
        .ok_or_else(|| format!("missing lhs checkpoint '{label}'"))?;
    let rhs = rhs
        .get(label)
        .ok_or_else(|| format!("missing rhs checkpoint '{label}'"))?;
    Ok(compare_logits(lhs, rhs))
}

fn compare_optional_named_checkpoint(
    lhs: &BTreeMap<String, Vec<f32>>,
    rhs: &BTreeMap<String, Vec<f32>>,
    label: &str,
) -> Result<Option<LogitComparison>, Box<dyn std::error::Error>> {
    match (lhs.get(label), rhs.get(label)) {
        (Some(lhs), Some(rhs)) => Ok(Some(compare_logits(lhs, rhs))),
        (None, None) => Ok(None),
        _ => Err(format!("checkpoint '{label}' is present on only one side").into()),
    }
}

fn output_tensor_values_f32(
    ctx: &Context,
    tensor_id: TensorId,
    bytes: &[u8],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid tensor id {tensor_id}"))?;
    match tensor.desc.ty {
        TensorType::F32 => Ok(bytes_to_f32s(bytes)),
        TensorType::F16 => Ok(bytes
            .chunks_exact(std::mem::size_of::<u16>())
            .map(|chunk| f16_to_f32(u16::from_le_bytes(chunk.try_into().unwrap())))
            .collect()),
        other => Err(format!(
            "unsupported checkpoint tensor type {} for '{}'",
            other.name(),
            tensor.name().unwrap_or("<unnamed>")
        )
        .into()),
    }
}

fn compare_configure_attention_cache_view(
    ctx: &mut Context,
    tensor_id: TensorId,
    ne0: i64,
    ne1: usize,
    ne2: i64,
    ne3: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid attention cache view tensor {tensor_id}"))?;
    let layout = TensorLayout::from_parts(4, &[ne0, ne2, i64::try_from(ne1)?, ne3], &tensor.nb)?;
    ctx.set_tensor_layout(tensor_id, layout)?;
    Ok(())
}

fn compare_configure_attention_mask_view(
    ctx: &mut Context,
    tensor_id: TensorId,
    key_count: usize,
    query_count: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid attention mask tensor {tensor_id}"))?;
    let layout = TensorLayout::for_ggml(
        tensor.desc.ty,
        &[
            i64::try_from(key_count)?,
            i64::try_from(query_count)?,
            tensor.ne[2],
            tensor.ne[3],
        ],
    )?;
    ctx.set_tensor_layout(tensor_id, layout)?;
    Ok(())
}

fn compare_attention_mask_write_key_count(
    ctx: &Context,
    tensor_id: TensorId,
    head_dim: i64,
    cache_tokens: usize,
    n_tokens: usize,
) -> Result<usize, Box<dyn std::error::Error>> {
    if compare_should_use_flash_attention(usize::try_from(head_dim)?, n_tokens) {
        return Ok(cache_tokens);
    }
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid attention mask tensor {tensor_id}"))?;
    usize::try_from(tensor.ne[0]).map_err(|_| "attention mask width does not fit in usize".into())
}

fn format_layer_suffix(layer_index: Option<u32>) -> String {
    layer_index
        .map(|layer_index| format!(" layer={layer_index}"))
        .unwrap_or_default()
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

#[derive(Clone)]
struct LogitComparison {
    max_abs_diff: f64,
    mean_abs_diff: f64,
    rms_diff: f64,
    cosine_similarity: f64,
}

struct HybridSplitLayerCheckpointDiff {
    layer_index: u32,
    attn_post_norm: Option<LogitComparison>,
    attn_residual: LogitComparison,
    ffn_input_norm: Option<LogitComparison>,
    ffn_out: LogitComparison,
    ffn_post_norm: Option<LogitComparison>,
    post_ffn: LogitComparison,
}

struct HybridSplitCheckpointDiff {
    input_embed: LogitComparison,
    result_norm: LogitComparison,
    result_logits: LogitComparison,
    layers: Vec<HybridSplitLayerCheckpointDiff>,
}

#[allow(dead_code)]
struct HybridCheckpointAndCacheDiff {
    checkpoints: HybridSplitCheckpointDiff,
    cache: HybridCacheDiffSummary,
}

fn hybrid_split_layer_max_abs_diff(layer: &HybridSplitLayerCheckpointDiff) -> f64 {
    let mut max_abs_diff = layer.attn_residual.max_abs_diff;
    if let Some(attn_post_norm) = &layer.attn_post_norm {
        max_abs_diff = max_abs_diff.max(attn_post_norm.max_abs_diff);
    }
    if let Some(ffn_input_norm) = &layer.ffn_input_norm {
        max_abs_diff = max_abs_diff.max(ffn_input_norm.max_abs_diff);
    }
    max_abs_diff = max_abs_diff.max(layer.ffn_out.max_abs_diff);
    if let Some(ffn_post_norm) = &layer.ffn_post_norm {
        max_abs_diff = max_abs_diff.max(ffn_post_norm.max_abs_diff);
    }
    max_abs_diff.max(layer.post_ffn.max_abs_diff)
}

struct HybridUpstreamLayerPreviewStackCheck {
    first_diff_layer: Option<u32>,
    first_diff_stats: Option<LogitComparison>,
    max_diff_layer: Option<u32>,
    max_diff_stats: LogitComparison,
    layer_post_ffn_stats: BTreeMap<u32, LogitComparison>,
}

struct GemmaUpstreamInputCheck {
    input_embed_stats: LogitComparison,
    input_embed_standalone_vs_rust_stats: LogitComparison,
    input_embed_standalone_vs_upstream_stats: LogitComparison,
    attn_input_norm_stats: LogitComparison,
    attn_input_norm_standalone_vs_rust_stats: LogitComparison,
    attn_input_norm_standalone_vs_upstream_stats: LogitComparison,
}

struct GemmaStandaloneInputReference {
    input_embed: TensorPreview,
    attn_input_norm: TensorPreview,
}

struct GemmaUpstreamLayerPreviewCheck {
    layer_index: u32,
    attn_post_norm_stats: Option<LogitComparison>,
    attn_residual_stats: LogitComparison,
    ffn_input_norm_stats: Option<LogitComparison>,
    ffn_out_stats: LogitComparison,
    ffn_post_norm_stats: Option<LogitComparison>,
    pe_in_stats: Option<LogitComparison>,
    per_layer_embd_out_stats: Option<LogitComparison>,
    post_ffn_stats: LogitComparison,
}

struct GemmaHybridSourceCacheCheck {
    layer_index: u32,
    k_cache_stats: LogitComparison,
    v_cache_stats: LogitComparison,
}

struct GemmaUpstreamTailCheck {
    result_norm_stats: LogitComparison,
    result_output_stats: LogitComparison,
}

struct GemmaFinalProbeCheck {
    probe_vs_hybrid_stats: LogitComparison,
    probe_vs_upstream_stats: LogitComparison,
}

struct GemmaLayerManualGraphCheck {
    layer_index: u32,
    attn_post_norm_stats: LogitComparison,
    attn_residual_stats: LogitComparison,
    ffn_input_norm_stats: LogitComparison,
    ffn_out_stats: LogitComparison,
    ffn_post_norm_stats: LogitComparison,
    pe_in_stats: LogitComparison,
    per_layer_embd_out_stats: Option<LogitComparison>,
    post_ffn_stats: LogitComparison,
}

struct GemmaUpstreamSharedPerLayerInputCheck {
    selected_stats: LogitComparison,
    proj_stats: LogitComparison,
    input_stats: LogitComparison,
}

struct GemmaSharedPerLayerManualCheck {
    selected_stats: LogitComparison,
    proj_stats: LogitComparison,
    input_stats: LogitComparison,
}

#[derive(Clone, Debug)]
struct HybridPreviewCheckpointSpec {
    label: String,
    source_names: Vec<String>,
}

struct TensorPreview {
    sum: f64,
    values: Vec<f32>,
}

struct MoePreviewCheck {
    layer_index: u32,
    router_weight_type: String,
    router_weight_dims: [i64; 4],
    router_weight_offset: Option<usize>,
    router_weight_strides: [usize; 4],
    router_weight_is_transposed: bool,
    router_weight_is_permuted: bool,
    router_weight_is_contiguous: bool,
    router_weight_is_view: bool,
    attn_residual_stats: LogitComparison,
    attn_residual_sum_diff: f64,
    input_norm_stats: LogitComparison,
    input_norm_cpu_stats: LogitComparison,
    input_norm_sum_diff: f64,
    router_logits_stats: LogitComparison,
    router_logits_isolated_stats: LogitComparison,
    router_logits_cpu_stats: LogitComparison,
    router_logits_tensor_cpu_stats: LogitComparison,
    router_logits_isolated_cpu_stats: LogitComparison,
    router_logits_cloned_loaded_stats: LogitComparison,
    router_logits_cloned_cpu_stats: LogitComparison,
    router_logits_sum_diff: f64,
    router_probs_stats: LogitComparison,
    router_probs_sum_diff: f64,
    selected_experts_match_cpu: bool,
    selected_experts_diff_count: usize,
    selected_experts_match_upstream: bool,
    selected_experts_upstream_diff_count: usize,
    selected_experts_upstream_set_diff_count: usize,
    min_topk_margin: f32,
    weights_norm_stats: LogitComparison,
    weights_norm_sum_diff: f64,
    up_stats: LogitComparison,
    up_sum_diff: f64,
    down_stats: LogitComparison,
    down_sum_diff: f64,
    weighted_stats: LogitComparison,
    weighted_sum_diff: f64,
    moe_out_stats: LogitComparison,
    moe_out_sum_diff: f64,
    shared_gated_stats: LogitComparison,
    shared_gated_sum_diff: f64,
    ffn_out_stats: LogitComparison,
    ffn_out_sum_diff: f64,
}

struct LayerResidualRun {
    layer_index: u32,
    hidden: Vec<f32>,
    hidden_size: usize,
    n_tokens: usize,
}

struct AttentionCacheSelfCheck {
    layer_index: u32,
    same_top1: bool,
    hidden_stats: LogitComparison,
}

struct AttentionDecodeBatchSelfCheck {
    layer_index: u32,
    hidden_stats: LogitComparison,
    result_output_stats: LogitComparison,
    first_token_result_output_stats: LogitComparison,
    last_token_result_output_stats: LogitComparison,
    step0_k_cache_row_stats: LogitComparison,
    step0_v_cache_row_stats: LogitComparison,
    step0_k_cache_tail_zero_stats: LogitComparison,
    step0_v_cache_tail_zero_stats: LogitComparison,
    k_cache_stats: LogitComparison,
    v_cache_stats: LogitComparison,
}

struct RecurrentFromHiddenBatchSelfCheck {
    source_layer_index: u32,
    layer_index: u32,
    hidden_stats: LogitComparison,
    r_cache_stats: LogitComparison,
    s_cache_stats: LogitComparison,
}

struct AttentionFromHiddenBatchSelfCheck {
    source_layer_index: u32,
    layer_index: u32,
    hidden_stats: LogitComparison,
    result_output_stats: LogitComparison,
    first_token_result_output_stats: LogitComparison,
    last_token_result_output_stats: LogitComparison,
    step0_k_cache_row_stats: LogitComparison,
    step0_v_cache_row_stats: LogitComparison,
    step0_k_cache_tail_zero_stats: LogitComparison,
    step0_v_cache_tail_zero_stats: LogitComparison,
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
    isolated_attn_stats: LogitComparison,
    output_proj_stats: LogitComparison,
    result_output_stats: LogitComparison,
    full_attn_cpu_stats: LogitComparison,
    isolated_attn_cpu_stats: LogitComparison,
    decode_attn_cpu_stats: LogitComparison,
}

struct AttentionDecodeBatchedTensorCheck {
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
    k_cache_view_stats: LogitComparison,
    v_cache_view_stats: LogitComparison,
    attn_stats: LogitComparison,
    output_proj_stats: LogitComparison,
    result_output_stats: LogitComparison,
}

struct AttentionDecodeStepwiseTensorCheck {
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
    output_proj_stats: LogitComparison,
    result_output_stats: LogitComparison,
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
    input_embed_stats: LogitComparison,
    input_norm_stats: LogitComparison,
    qkv_mixed_stats: LogitComparison,
    qkv_mixed_full_stats: LogitComparison,
    qkv_mixed_transposed_stats: LogitComparison,
    conv_states_reshaped_zero_stats: LogitComparison,
    z_stats: LogitComparison,
    beta_stats: LogitComparison,
    gate_stats: LogitComparison,
    conv_input_stats: LogitComparison,
    q_conv_predelta_stats: LogitComparison,
    k_conv_predelta_stats: LogitComparison,
    conv_output_stats: LogitComparison,
    output_view_stats: LogitComparison,
    output_norm_stats: LogitComparison,
    z_silu_stats: LogitComparison,
    gated_output_stats: LogitComparison,
    final_output_stats: LogitComparison,
}

struct RecurrentUpstreamPreviewCheck {
    layer_index: u32,
    input_norm_stats: LogitComparison,
    input_norm_cpu_stats: LogitComparison,
    final_output_stats: LogitComparison,
    linear_attn_out_stats: LogitComparison,
    attn_residual_stats: LogitComparison,
}

struct RecurrentStepCpuCheck {
    layer_index: u32,
    conv_output_cpu_stats: LogitComparison,
    q_conv_cpu_stats: LogitComparison,
    k_conv_cpu_stats: LogitComparison,
    output_view_cpu_stats: LogitComparison,
}

fn moe_preview_check(
    args: &Args,
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<MoePreviewCheck, Box<dyn std::error::Error>> {
    let upstream_previews = run_upstream_debug_tensor_previews(
        args,
        &[
            "attn_residual-0",
            "attn_post_norm-0",
            "ffn_moe_logits-0",
            "ffn_moe_probs-0",
            "ffn_moe_topk-0",
            "ffn_moe_weights_norm-0",
            "ffn_moe_up-0",
            "ffn_moe_down_scaled-0",
            "ffn_moe_down-0",
            "ffn_moe_weighted-0",
            "ffn_moe_out-0",
            "ffn_shexp_gated-0",
            "ffn_out-0",
        ],
    )?;
    let residual = first_layer_residual_hidden(model, token_ids)?;
    let layer_index = residual.layer_index;
    let moe_spec = qwen35moe_moe_ffn_spec(model, layer_index)?;
    let moe_layout = qwen35moe_moe_ffn_layout(model, layer_index)?;
    let mut loaded =
        moe_layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let expected_hidden_values = residual
        .hidden_size
        .checked_mul(residual.n_tokens)
        .ok_or("overflow computing residual hidden length")?;
    if residual.hidden.len() != expected_hidden_values {
        return Err(format!(
            "residual hidden length mismatch: got {}, expected {}",
            residual.hidden.len(),
            expected_hidden_values
        )
        .into());
    }
    let rust_attn_residual =
        tensor_preview_from_values(&residual.hidden, residual.hidden_size, residual.n_tokens)?;
    let runtime = MetalRuntime::new()?;
    let features = runtime.features();
    let mut graph = build_moe_ffn_graph(
        &mut loaded.ctx,
        &loaded.tensor_ids,
        &moe_spec,
        token_ids.len(),
    )?;
    let rust_input_norm_id = tensor_id_by_name(&loaded.ctx, "moe_ffn.input_norm")?;
    let rust_router_logits_id = tensor_id_by_name(&loaded.ctx, "moe_ffn.router_logits")?;
    let rust_router_probs_id = tensor_id_by_name(&loaded.ctx, "moe_ffn.router_probs")?;
    let rust_weights_norm_id = tensor_id_by_name(&loaded.ctx, "moe_ffn.selected_weights_norm")?;
    let rust_selected_experts_id = graph.selected_experts;
    let rust_up_id = tensor_id_by_any_name(&loaded.ctx, &["moe_ffn.up"])?;
    let rust_down_id =
        tensor_id_by_any_name(&loaded.ctx, &["moe_ffn.down_scaled", "moe_ffn.down"])?;
    let rust_weighted_id = tensor_id_by_name(&loaded.ctx, "moe_ffn.weighted")?;
    let rust_moe_out_id = tensor_id_by_name(&loaded.ctx, "moe_ffn.moe_out")?;
    let rust_shared_gated_id = tensor_id_by_name(&loaded.ctx, "moe_ffn.shared_gated")?;
    let rust_ffn_out_id = tensor_id_by_name(&loaded.ctx, "moe_ffn.result_output")?;
    for tensor_id in [
        rust_input_norm_id,
        rust_router_logits_id,
        rust_router_probs_id,
        rust_weights_norm_id,
        rust_selected_experts_id,
        rust_up_id,
        rust_down_id,
        rust_weighted_id,
        rust_moe_out_id,
        rust_shared_gated_id,
        rust_ffn_out_id,
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
    let hidden_bytes = f32s_to_bytes(&residual.hidden);
    let execution = session.execute(
        &loaded.ctx,
        &[MetalGraphTensorWrite {
            tensor_id: graph.input_primary,
            bytes: &hidden_bytes,
        }],
        &[
            rust_input_norm_id,
            rust_router_logits_id,
            rust_router_probs_id,
            rust_weights_norm_id,
            rust_selected_experts_id,
            rust_up_id,
            rust_down_id,
            rust_weighted_id,
            rust_moe_out_id,
            rust_shared_gated_id,
            rust_ffn_out_id,
        ],
    )?;
    let rust_input_norm =
        tensor_preview_from_execution(&loaded.ctx, &execution, rust_input_norm_id)?;
    let rust_input_norm_full =
        tensor_values_from_execution_f32(&loaded.ctx, &execution, rust_input_norm_id)?;
    let rust_router_logits =
        tensor_preview_from_execution(&loaded.ctx, &execution, rust_router_logits_id)?;
    let rust_router_logits_full =
        tensor_values_from_execution_f32(&loaded.ctx, &execution, rust_router_logits_id)?;
    let rust_router_probs =
        tensor_preview_from_execution(&loaded.ctx, &execution, rust_router_probs_id)?;
    let rust_weights_norm =
        tensor_preview_from_execution(&loaded.ctx, &execution, rust_weights_norm_id)?;
    let rust_up = tensor_preview_from_execution(&loaded.ctx, &execution, rust_up_id)?;
    let rust_down = tensor_preview_from_execution(&loaded.ctx, &execution, rust_down_id)?;
    let rust_weighted = tensor_preview_from_execution(&loaded.ctx, &execution, rust_weighted_id)?;
    let rust_moe_out = tensor_preview_from_execution(&loaded.ctx, &execution, rust_moe_out_id)?;
    let rust_shared_gated =
        tensor_preview_from_execution(&loaded.ctx, &execution, rust_shared_gated_id)?;
    let rust_ffn_out = tensor_preview_from_execution(&loaded.ctx, &execution, rust_ffn_out_id)?;
    let rust_router_probs_values =
        tensor_values_from_execution_f32(&loaded.ctx, &execution, rust_router_probs_id)?;
    let rust_selected_experts =
        tensor_values_from_execution_i32(&loaded.ctx, &execution, rust_selected_experts_id)?;
    let input_norm_cpu = if let Some(norm) = &moe_spec.input_norm {
        let norm_weight_id = *loaded
            .tensor_ids
            .get(&norm.weight_name)
            .ok_or("missing moe input_norm weight tensor id")?;
        let norm_weight = bytes_to_f32s(loaded.ctx.tensor_data(norm_weight_id)?);
        cpu_rms_norm_mul_rows(
            &residual.hidden,
            residual.hidden_size,
            residual.n_tokens,
            &norm_weight,
            norm.epsilon,
        )?
    } else {
        residual.hidden.clone()
    };
    let router_weight_id = *loaded
        .tensor_ids
        .get(&moe_spec.router_proj_name)
        .ok_or("missing moe router weight tensor id")?;
    let router_weight = loaded
        .ctx
        .tensor(router_weight_id)
        .ok_or("invalid moe router weight tensor")?
        .clone();
    let router_weight_bytes = loaded.ctx.tensor_data(router_weight_id)?.to_vec();
    let router_cols = usize::try_from(router_weight.ne[0])?;
    let router_rows = usize::try_from(router_weight.ne[1])?;
    let router_row_ids = (0..router_rows)
        .map(i32::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let router_weight_rows = get_rows_ggml_bytes_cpu(
        loaded.ctx.tensor_data(router_weight_id)?,
        router_weight.desc.ty.ggml_type(),
        router_cols,
        router_rows,
        &router_row_ids,
    )
    .ok_or("unsupported moe router weight get_rows")?;
    let router_logits_cpu = cpu_mul_mat_rows(
        &router_weight_rows,
        router_cols,
        router_rows,
        &rust_input_norm_full,
        residual.n_tokens,
    )?;
    let router_logits_tensor_cpu = cpu_mul_mat_tensor_rows(
        &router_weight,
        &router_weight_bytes,
        &rust_input_norm_full,
        residual.n_tokens,
    )?;
    let router_logits_cloned = execute_f32_mul_mat_in_fresh_context(
        &router_weight_bytes,
        router_cols,
        router_rows,
        &rust_input_norm_full,
        residual.n_tokens,
    )?;
    let isolated_router_input = loaded.ctx.new_named_tensor(
        "moe_router_only.inp_embd",
        TensorType::F32,
        2,
        &[router_cols as i64, residual.n_tokens as i64],
        BufferUsage::Activations,
    )?;
    let isolated_router_logits_id = loaded
        .ctx
        .mul_mat(
            router_weight_id,
            isolated_router_input,
            BufferUsage::Activations,
        )
        .map_err(LlamaError::format)?;
    loaded
        .ctx
        .set_tensor_name(isolated_router_logits_id, "moe_router_only.logits")
        .map_err(LlamaError::format)?;
    let mut isolated_graph = Graph::new();
    isolated_graph.build_forward_expand(&loaded.ctx, isolated_router_logits_id)?;
    let isolated_prepared = prepare_graph(&loaded.ctx, &isolated_graph, features)?;
    let isolated_session = MetalGraphSession::from_runtime(
        MetalRuntime::new()?,
        &loaded.ctx,
        &isolated_prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let isolated_execution = isolated_session.execute(
        &loaded.ctx,
        &[MetalGraphTensorWrite {
            tensor_id: isolated_router_input,
            bytes: &f32s_to_bytes(&rust_input_norm_full),
        }],
        &[isolated_router_logits_id],
    )?;
    let isolated_router_logits = tensor_values_from_execution_f32(
        &loaded.ctx,
        &isolated_execution,
        isolated_router_logits_id,
    )?;
    let cpu_selected_experts = cpu_top_k_rows_i32(
        &rust_router_probs_values,
        usize::try_from(moe_spec.expert_count).map_err(|_| "expert_count does not fit in usize")?,
        usize::try_from(moe_spec.expert_used_count)
            .map_err(|_| "expert_used_count does not fit in usize")?,
    );
    let selected_experts_diff_count = rust_selected_experts
        .iter()
        .zip(cpu_selected_experts.iter())
        .filter(|(lhs, rhs)| lhs != rhs)
        .count()
        + rust_selected_experts
            .len()
            .abs_diff(cpu_selected_experts.len());
    let upstream_selected_experts = preview_values_to_i32s(
        &upstream_previews
            .get("ffn_moe_topk-0")
            .ok_or("missing upstream ffn_moe_topk-0 preview")?
            .values,
    )?;
    let selected_experts_upstream_diff_count = rust_selected_experts
        .iter()
        .zip(upstream_selected_experts.iter())
        .filter(|(lhs, rhs)| lhs != rhs)
        .count()
        + rust_selected_experts
            .len()
            .abs_diff(upstream_selected_experts.len());
    let expert_count =
        usize::try_from(moe_spec.expert_count).map_err(|_| "expert_count does not fit in usize")?;
    let expert_used_count = usize::try_from(moe_spec.expert_used_count)
        .map_err(|_| "expert_used_count does not fit in usize")?;
    let selected_experts_upstream_set_diff_count = topk_set_diff_count(
        &rust_selected_experts,
        &upstream_selected_experts,
        expert_used_count,
    );
    let min_topk_margin =
        min_topk_margin(&rust_router_probs_values, expert_count, expert_used_count);
    let upstream_attn_residual = upstream_previews
        .get("attn_residual-0")
        .ok_or("missing upstream attn_residual-0 preview")?;
    let upstream_input_norm = upstream_previews
        .get("attn_post_norm-0")
        .ok_or("missing upstream attn_post_norm-0 preview")?;
    let upstream_router_logits = upstream_previews
        .get("ffn_moe_logits-0")
        .ok_or("missing upstream ffn_moe_logits-0 preview")?;
    let upstream_router_probs = upstream_previews
        .get("ffn_moe_probs-0")
        .ok_or("missing upstream ffn_moe_probs-0 preview")?;
    let upstream_weights_norm = upstream_previews
        .get("ffn_moe_weights_norm-0")
        .ok_or("missing upstream ffn_moe_weights_norm-0 preview")?;
    let upstream_up = upstream_previews
        .get("ffn_moe_up-0")
        .ok_or("missing upstream ffn_moe_up-0 preview")?;
    let upstream_down = upstream_preview_by_any_name(
        &upstream_previews,
        &["ffn_moe_down_scaled-0", "ffn_moe_down-0"],
    )?;
    let upstream_weighted = upstream_previews
        .get("ffn_moe_weighted-0")
        .ok_or("missing upstream ffn_moe_weighted-0 preview")?;
    let upstream_moe_out = upstream_previews
        .get("ffn_moe_out-0")
        .ok_or("missing upstream ffn_moe_out-0 preview")?;
    let upstream_shared_gated = upstream_previews
        .get("ffn_shexp_gated-0")
        .ok_or("missing upstream ffn_shexp_gated-0 preview")?;
    let upstream_ffn_out = upstream_previews
        .get("ffn_out-0")
        .ok_or("missing upstream ffn_out-0 preview")?;

    Ok(MoePreviewCheck {
        layer_index,
        router_weight_type: router_weight.desc.ty.name().to_string(),
        router_weight_dims: router_weight.ne,
        router_weight_offset: router_weight.data_offset,
        router_weight_strides: router_weight.nb,
        router_weight_is_transposed: router_weight.is_transposed(),
        router_weight_is_permuted: router_weight.is_permuted(),
        router_weight_is_contiguous: router_weight.is_contiguous(),
        router_weight_is_view: router_weight.is_view(),
        attn_residual_stats: compare_logits(
            &rust_attn_residual.values,
            &upstream_attn_residual.values,
        ),
        attn_residual_sum_diff: (rust_attn_residual.sum - upstream_attn_residual.sum).abs(),
        input_norm_stats: compare_logits(&rust_input_norm.values, &upstream_input_norm.values),
        input_norm_cpu_stats: compare_logits(&rust_input_norm_full, &input_norm_cpu),
        input_norm_sum_diff: (rust_input_norm.sum - upstream_input_norm.sum).abs(),
        router_logits_stats: compare_logits(
            &rust_router_logits.values,
            &upstream_router_logits.values,
        ),
        router_logits_isolated_stats: compare_logits(
            &rust_router_logits_full,
            &isolated_router_logits,
        ),
        router_logits_cpu_stats: compare_logits(&rust_router_logits_full, &router_logits_cpu),
        router_logits_tensor_cpu_stats: compare_logits(
            &rust_router_logits_full,
            &router_logits_tensor_cpu,
        ),
        router_logits_isolated_cpu_stats: compare_logits(
            &isolated_router_logits,
            &router_logits_cpu,
        ),
        router_logits_cloned_loaded_stats: compare_logits(
            &router_logits_cloned,
            &rust_router_logits_full,
        ),
        router_logits_cloned_cpu_stats: compare_logits(&router_logits_cloned, &router_logits_cpu),
        router_logits_sum_diff: (rust_router_logits.sum - upstream_router_logits.sum).abs(),
        router_probs_stats: compare_logits(
            &rust_router_probs.values,
            &upstream_router_probs.values,
        ),
        router_probs_sum_diff: (rust_router_probs.sum - upstream_router_probs.sum).abs(),
        selected_experts_match_cpu: selected_experts_diff_count == 0,
        selected_experts_diff_count,
        selected_experts_match_upstream: selected_experts_upstream_diff_count == 0,
        selected_experts_upstream_diff_count,
        selected_experts_upstream_set_diff_count,
        min_topk_margin,
        weights_norm_stats: compare_logits(
            &rust_weights_norm.values,
            &upstream_weights_norm.values,
        ),
        weights_norm_sum_diff: (rust_weights_norm.sum - upstream_weights_norm.sum).abs(),
        up_stats: compare_logits(&rust_up.values, &upstream_up.values),
        up_sum_diff: (rust_up.sum - upstream_up.sum).abs(),
        down_stats: compare_logits(&rust_down.values, &upstream_down.values),
        down_sum_diff: (rust_down.sum - upstream_down.sum).abs(),
        weighted_stats: compare_logits(&rust_weighted.values, &upstream_weighted.values),
        weighted_sum_diff: (rust_weighted.sum - upstream_weighted.sum).abs(),
        moe_out_stats: compare_logits(&rust_moe_out.values, &upstream_moe_out.values),
        moe_out_sum_diff: (rust_moe_out.sum - upstream_moe_out.sum).abs(),
        shared_gated_stats: compare_logits(
            &rust_shared_gated.values,
            &upstream_shared_gated.values,
        ),
        shared_gated_sum_diff: (rust_shared_gated.sum - upstream_shared_gated.sum).abs(),
        ffn_out_stats: compare_logits(&rust_ffn_out.values, &upstream_ffn_out.values),
        ffn_out_sum_diff: (rust_ffn_out.sum - upstream_ffn_out.sum).abs(),
    })
}

fn first_layer_residual_hidden(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<LayerResidualRun, Box<dyn std::error::Error>> {
    let first_layer = model
        .qwen35moe_tensors()?
        .layers
        .into_iter()
        .next()
        .ok_or("qwen35moe model has no layers")?;

    match first_layer.kind {
        Qwen35MoeLayerKind::Attention => {
            let layout = qwen35moe_attention_block_layout(model, first_layer.index)?;
            let block_spec = qwen35moe_attention_block_spec(model, first_layer.index)?;
            let positions = (0..token_ids.len())
                .map(i32::try_from)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut loaded =
                layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
            let compiled =
                compile_attention_block_metal(&mut loaded, &block_spec, token_ids.len())?;
            let run = execute_attention_block_graph_metal_cached(
                &compiled,
                &loaded,
                LogitsProbeInput::TokenIds(token_ids),
                &positions,
            )?;
            Ok(LayerResidualRun {
                layer_index: first_layer.index,
                hidden: run.hidden,
                hidden_size: run.hidden_size,
                n_tokens: run.n_tokens,
            })
        }
        Qwen35MoeLayerKind::Recurrent => {
            let layout = qwen35moe_recurrent_block_layout(model, first_layer.index)?;
            let spec = qwen35moe_delta_net_recurrent_decode_spec(
                model,
                first_layer.index,
                1,
                TensorType::F32,
                TensorType::F32,
            )?;
            let mut loaded =
                layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
            let compiled =
                compile_delta_net_recurrent_decode_metal(&mut loaded, &spec, token_ids.len())?;
            let run = execute_delta_net_recurrent_decode_graph_metal_cached(
                &compiled,
                &mut loaded,
                LogitsProbeInput::TokenIds(token_ids),
            )?;
            Ok(LayerResidualRun {
                layer_index: first_layer.index,
                hidden: run.hidden,
                hidden_size: run.hidden_size,
                n_tokens: run.n_tokens,
            })
        }
    }
}

fn run_upstream_debug_tensor_previews(
    args: &Args,
    tensor_filters: &[&str],
) -> Result<BTreeMap<String, TensorPreview>, Box<dyn std::error::Error>> {
    let mut command = Command::new(&args.upstream_debug_path);
    command
        .arg("-m")
        .arg(&args.model_path)
        .arg("-p")
        .arg(&args.prompt)
        .arg("-ngl")
        .arg("999")
        .arg("-fa")
        .arg("1")
        .arg("-ctk")
        .arg("f16")
        .arg("-ctv")
        .arg("f16")
        .arg("--verbose");
    for tensor_filter in tensor_filters {
        command.arg("--tensor-filter").arg(tensor_filter);
    }
    let output = command.output()?;
    ensure_success("llama-debug preview", &output)?;
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    parse_debug_tensor_previews(&combined, tensor_filters)
}

fn parse_debug_tensor_previews(
    text: &str,
    expected: &[&str],
) -> Result<BTreeMap<String, TensorPreview>, Box<dyn std::error::Error>> {
    let mut previews = BTreeMap::new();
    let expected = expected.iter().copied().collect::<Vec<_>>();
    let mut current_name: Option<String> = None;
    let mut current_matches = false;
    let mut current_values = Vec::new();
    let mut current_sum: Option<f64> = None;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("common_debug_cb_eval:") {
            finish_debug_tensor_preview(
                &mut previews,
                &mut current_name,
                &mut current_matches,
                &mut current_values,
                &mut current_sum,
            )?;
            let Some((name, _)) = rest.trim_start().split_once(" = ") else {
                continue;
            };
            current_name = Some(name.to_owned());
            current_matches = expected.iter().any(|expected_name| *expected_name == name);
            continue;
        }

        if !current_matches {
            continue;
        }

        let trimmed = line.trim();
        if let Some(sum_text) = trimmed.strip_prefix("sum = ") {
            let parsed = sum_text.parse::<f64>()?;
            current_sum.get_or_insert(parsed);
            continue;
        }
        if let Some(mut row) = parse_preview_row(trimmed) {
            current_values.append(&mut row);
        }
    }
    finish_debug_tensor_preview(
        &mut previews,
        &mut current_name,
        &mut current_matches,
        &mut current_values,
        &mut current_sum,
    )?;
    let _ = expected;
    Ok(previews)
}

fn finish_debug_tensor_preview(
    previews: &mut BTreeMap<String, TensorPreview>,
    current_name: &mut Option<String>,
    current_matches: &mut bool,
    current_values: &mut Vec<f32>,
    current_sum: &mut Option<f64>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(name) = current_name.take() {
        if *current_matches {
            previews.insert(
                name.clone(),
                TensorPreview {
                    sum: current_sum.ok_or_else(|| format!("missing preview sum for {name}"))?,
                    values: std::mem::take(current_values),
                },
            );
        }
    }
    *current_matches = false;
    *current_sum = None;
    current_values.clear();
    Ok(())
}

fn parse_preview_row(line: &str) -> Option<Vec<f32>> {
    if !line.contains('[') || !line.chars().any(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let content = line
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(',')
        .trim_end_matches(']')
        .trim();
    let values = content
        .split(',')
        .filter_map(|part| {
            let trimmed = part.trim();
            if trimmed.is_empty() || trimmed == "..." {
                None
            } else {
                trimmed.parse::<f32>().ok()
            }
        })
        .collect::<Vec<_>>();
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

fn tensor_preview_from_execution(
    ctx: &Context,
    execution: &makepad_ggml::backend::metal::MetalGraphExecution,
    tensor_id: TensorId,
) -> Result<TensorPreview, Box<dyn std::error::Error>> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid tensor id {tensor_id}"))?;
    let bytes = execution
        .outputs
        .get(&tensor_id)
        .ok_or_else(|| format!("missing execution output for tensor {tensor_id}"))?;
    tensor_preview_from_tensor_bytes(tensor, bytes)
}

fn tensor_values_from_execution_f32(
    ctx: &Context,
    execution: &makepad_ggml::backend::metal::MetalGraphExecution,
    tensor_id: TensorId,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid tensor id {tensor_id}"))?;
    let bytes = execution
        .outputs
        .get(&tensor_id)
        .ok_or_else(|| format!("missing execution output for tensor {tensor_id}"))?;
    tensor_values_from_tensor_bytes_f32(tensor, bytes)
}

fn tensor_values_from_execution_i32(
    ctx: &Context,
    execution: &makepad_ggml::backend::metal::MetalGraphExecution,
    tensor_id: TensorId,
) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid tensor id {tensor_id}"))?;
    let bytes = execution
        .outputs
        .get(&tensor_id)
        .ok_or_else(|| format!("missing execution output for tensor {tensor_id}"))?;
    tensor_values_from_tensor_bytes_i32(tensor, bytes)
}

fn tensor_values_from_tensor_bytes_f32(
    tensor: &Tensor,
    bytes: &[u8],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if bytes.len() != tensor.nbytes() {
        return Err(format!(
            "tensor '{}' value byte length mismatch: got {}, expected {}",
            tensor.name().unwrap_or("<unnamed>"),
            bytes.len(),
            tensor.nbytes()
        )
        .into());
    }

    let ne = tensor_preview_dims(tensor)?;
    let mut values = Vec::with_capacity(
        ne[0]
            .checked_mul(ne[1])
            .and_then(|value| value.checked_mul(ne[2]))
            .and_then(|value| value.checked_mul(ne[3]))
            .ok_or("tensor value count overflow")?,
    );
    for i3 in 0..ne[3] {
        for i2 in 0..ne[2] {
            for i1 in 0..ne[1] {
                for i0 in 0..ne[0] {
                    values.push(tensor_scalar_to_f32(tensor, bytes, [i0, i1, i2, i3])?);
                }
            }
        }
    }
    Ok(values)
}

fn tensor_values_from_tensor_bytes_i32(
    tensor: &Tensor,
    bytes: &[u8],
) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
    if bytes.len() != tensor.nbytes() {
        return Err(format!(
            "tensor '{}' value byte length mismatch: got {}, expected {}",
            tensor.name().unwrap_or("<unnamed>"),
            bytes.len(),
            tensor.nbytes()
        )
        .into());
    }

    let ne = tensor_preview_dims(tensor)?;
    let mut values = Vec::with_capacity(
        ne[0]
            .checked_mul(ne[1])
            .and_then(|value| value.checked_mul(ne[2]))
            .and_then(|value| value.checked_mul(ne[3]))
            .ok_or("tensor value count overflow")?,
    );
    for i3 in 0..ne[3] {
        for i2 in 0..ne[2] {
            for i1 in 0..ne[1] {
                for i0 in 0..ne[0] {
                    values.push(tensor_scalar_to_i32(tensor, bytes, [i0, i1, i2, i3])?);
                }
            }
        }
    }
    Ok(values)
}

fn tensor_preview_from_values(
    values: &[f32],
    width: usize,
    rows: usize,
) -> Result<TensorPreview, Box<dyn std::error::Error>> {
    let expected = width
        .checked_mul(rows)
        .ok_or("overflow computing tensor preview size")?;
    if values.len() != expected {
        return Err(format!(
            "tensor preview length mismatch: got {}, expected {}",
            values.len(),
            expected
        )
        .into());
    }
    let mut preview = Vec::new();
    for row in values.chunks_exact(width) {
        if width <= 6 {
            preview.extend_from_slice(row);
        } else {
            preview.extend_from_slice(&row[..3]);
            preview.extend_from_slice(&row[width - 3..]);
        }
    }
    Ok(TensorPreview {
        sum: values.iter().map(|&value| f64::from(value)).sum(),
        values: preview,
    })
}

fn tensor_preview_from_tensor_bytes(
    tensor: &Tensor,
    bytes: &[u8],
) -> Result<TensorPreview, Box<dyn std::error::Error>> {
    if bytes.len() != tensor.nbytes() {
        return Err(format!(
            "tensor '{}' preview byte length mismatch: got {}, expected {}",
            tensor.name().unwrap_or("<unnamed>"),
            bytes.len(),
            tensor.nbytes()
        )
        .into());
    }

    let ne = tensor_preview_dims(tensor)?;
    let mut sum = 0.0f64;
    for i3 in 0..ne[3] {
        for i2 in 0..ne[2] {
            for i1 in 0..ne[1] {
                for i0 in 0..ne[0] {
                    sum += f64::from(tensor_scalar_to_f32(tensor, bytes, [i0, i1, i2, i3])?);
                }
            }
        }
    }

    let mut preview = Vec::new();
    let i0_indices = preview_indices(ne[0], TENSOR_PREVIEW_EDGE_COUNT);
    let i1_indices = preview_indices(ne[1], TENSOR_PREVIEW_EDGE_COUNT);
    let i2_indices = preview_indices(ne[2], TENSOR_PREVIEW_EDGE_COUNT);
    for i3 in 0..ne[3] {
        for &i2 in &i2_indices {
            for &i1 in &i1_indices {
                for &i0 in &i0_indices {
                    preview.push(tensor_scalar_to_f32(tensor, bytes, [i0, i1, i2, i3])?);
                }
            }
        }
    }

    Ok(TensorPreview {
        sum,
        values: preview,
    })
}

fn tensor_preview_dims(tensor: &Tensor) -> Result<[usize; 4], Box<dyn std::error::Error>> {
    Ok([
        usize::try_from(tensor.ne[0]).map_err(|_| "tensor ne[0] does not fit in usize")?,
        usize::try_from(tensor.ne[1]).map_err(|_| "tensor ne[1] does not fit in usize")?,
        usize::try_from(tensor.ne[2]).map_err(|_| "tensor ne[2] does not fit in usize")?,
        usize::try_from(tensor.ne[3]).map_err(|_| "tensor ne[3] does not fit in usize")?,
    ])
}

fn preview_indices(len: usize, edge_count: usize) -> Vec<usize> {
    if len <= edge_count.saturating_mul(2) {
        return (0..len).collect();
    }
    let mut indices = (0..edge_count).collect::<Vec<_>>();
    indices.extend(len - edge_count..len);
    indices
}

fn tensor_scalar_to_f32(
    tensor: &Tensor,
    bytes: &[u8],
    index: [usize; 4],
) -> Result<f32, Box<dyn std::error::Error>> {
    let offset = tensor_element_offset(tensor, index)?;
    match tensor.desc.ty {
        TensorType::F32 => Ok(f32::from_le_bytes(
            bytes
                .get(offset..offset + std::mem::size_of::<f32>())
                .ok_or_else(|| tensor_bounds_error(tensor, offset, std::mem::size_of::<f32>()))?
                .try_into()
                .unwrap(),
        )),
        TensorType::F16 => Ok(f16_to_f32(u16::from_le_bytes(
            bytes
                .get(offset..offset + std::mem::size_of::<u16>())
                .ok_or_else(|| tensor_bounds_error(tensor, offset, std::mem::size_of::<u16>()))?
                .try_into()
                .unwrap(),
        ))),
        TensorType::BF16 => Ok(bf16_to_f32(u16::from_le_bytes(
            bytes
                .get(offset..offset + std::mem::size_of::<u16>())
                .ok_or_else(|| tensor_bounds_error(tensor, offset, std::mem::size_of::<u16>()))?
                .try_into()
                .unwrap(),
        ))),
        TensorType::I64 => Ok(i64::from_le_bytes(
            bytes
                .get(offset..offset + std::mem::size_of::<i64>())
                .ok_or_else(|| tensor_bounds_error(tensor, offset, std::mem::size_of::<i64>()))?
                .try_into()
                .unwrap(),
        ) as f32),
        TensorType::I32 => Ok(i32::from_le_bytes(
            bytes
                .get(offset..offset + std::mem::size_of::<i32>())
                .ok_or_else(|| tensor_bounds_error(tensor, offset, std::mem::size_of::<i32>()))?
                .try_into()
                .unwrap(),
        ) as f32),
        TensorType::I16 => Ok(i16::from_le_bytes(
            bytes
                .get(offset..offset + std::mem::size_of::<i16>())
                .ok_or_else(|| tensor_bounds_error(tensor, offset, std::mem::size_of::<i16>()))?
                .try_into()
                .unwrap(),
        ) as f32),
        TensorType::I8 => Ok(i8::from_le_bytes(
            bytes
                .get(offset..offset + std::mem::size_of::<i8>())
                .ok_or_else(|| tensor_bounds_error(tensor, offset, std::mem::size_of::<i8>()))?
                .try_into()
                .unwrap(),
        ) as f32),
        other => Err(format!(
            "tensor '{}' preview does not support type {}",
            tensor.name().unwrap_or("<unnamed>"),
            other.name()
        )
        .into()),
    }
}

fn tensor_scalar_to_i32(
    tensor: &Tensor,
    bytes: &[u8],
    index: [usize; 4],
) -> Result<i32, Box<dyn std::error::Error>> {
    let offset = tensor_element_offset(tensor, index)?;
    match tensor.desc.ty {
        TensorType::I32 => Ok(i32::from_le_bytes(
            bytes
                .get(offset..offset + std::mem::size_of::<i32>())
                .ok_or_else(|| tensor_bounds_error(tensor, offset, std::mem::size_of::<i32>()))?
                .try_into()
                .unwrap(),
        )),
        other => Err(format!(
            "tensor '{}' i32 extraction does not support type {}",
            tensor.name().unwrap_or("<unnamed>"),
            other.name()
        )
        .into()),
    }
}

fn tensor_element_offset(
    tensor: &Tensor,
    index: [usize; 4],
) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(index[3]
        .checked_mul(tensor.nb[3])
        .and_then(|offset| {
            index[2]
                .checked_mul(tensor.nb[2])
                .and_then(|value| offset.checked_add(value))
        })
        .and_then(|offset| {
            index[1]
                .checked_mul(tensor.nb[1])
                .and_then(|value| offset.checked_add(value))
        })
        .and_then(|offset| {
            index[0]
                .checked_mul(tensor.nb[0])
                .and_then(|value| offset.checked_add(value))
        })
        .ok_or_else(|| {
            format!(
                "tensor '{}' offset overflow",
                tensor.name().unwrap_or("<unnamed>")
            )
        })?)
}

fn tensor_bounds_error(tensor: &Tensor, offset: usize, len: usize) -> String {
    format!(
        "tensor '{}' preview read out of bounds at {}..{} of {} bytes",
        tensor.name().unwrap_or("<unnamed>"),
        offset,
        offset.saturating_add(len),
        tensor.nbytes()
    )
}

fn upstream_preview_by_any_name<'a>(
    previews: &'a BTreeMap<String, TensorPreview>,
    names: &[&str],
) -> Result<&'a TensorPreview, Box<dyn std::error::Error>> {
    for name in names {
        if let Some(preview) = previews.get(*name) {
            return Ok(preview);
        }
    }
    Err(format!(
        "missing upstream tensor preview, tried any of {}",
        names.join(", ")
    )
    .into())
}

fn architecture_has_recurrent_layers(architecture: &LlamaArchitecture) -> bool {
    matches!(
        architecture,
        LlamaArchitecture::Qwen35 | LlamaArchitecture::Qwen35Moe
    )
}

fn first_attention_check_setup(
    model: &LlamaModel,
    token_count: usize,
    cache_type: TensorType,
) -> Result<
    (
        u32,
        AttentionBlockSpec,
        GgufWeightLayout,
        AttentionDecodeSpec,
    ),
    Box<dyn std::error::Error>,
> {
    let token_count = u32::try_from(token_count)?;
    match &model.architecture {
        LlamaArchitecture::Qwen35 => {
            let (layer_index, _) = qwen35_first_attention_block_spec(model)?;
            attention_check_setup_for_layer(model, layer_index, token_count, cache_type)
        }
        LlamaArchitecture::Qwen35Moe => {
            let (layer_index, _) = qwen35moe_first_attention_block_spec(model)?;
            attention_check_setup_for_layer(model, layer_index, token_count, cache_type)
        }
        LlamaArchitecture::Gemma4 => {
            let (layer_index, _) = gemma4_first_attention_block_spec(model)?;
            attention_check_setup_for_layer(model, layer_index, token_count, cache_type)
        }
        LlamaArchitecture::Unknown(name) => {
            Err(format!("unsupported architecture for attention self-check: {name}").into())
        }
    }
}

fn attention_check_setup_for_layer(
    model: &LlamaModel,
    layer_index: u32,
    token_count: u32,
    cache_type: TensorType,
) -> Result<
    (
        u32,
        AttentionBlockSpec,
        GgufWeightLayout,
        AttentionDecodeSpec,
    ),
    Box<dyn std::error::Error>,
> {
    let token_count = u32::try_from(token_count)?;
    match &model.architecture {
        LlamaArchitecture::Qwen35 => {
            let block_spec = qwen35_attention_block_spec(model, layer_index)?;
            let layout = qwen35_attention_block_layout(model, layer_index)?;
            let decode_spec = qwen35_attention_decode_spec(
                model,
                layer_index,
                token_count,
                1,
                cache_type,
                cache_type,
            )?;
            Ok((layer_index, block_spec, layout, decode_spec))
        }
        LlamaArchitecture::Qwen35Moe => {
            let block_spec = qwen35moe_attention_block_spec(model, layer_index)?;
            let layout = qwen35moe_attention_block_layout(model, layer_index)?;
            let decode_spec = qwen35moe_attention_decode_spec(
                model,
                layer_index,
                token_count,
                1,
                cache_type,
                cache_type,
            )?;
            Ok((layer_index, block_spec, layout, decode_spec))
        }
        LlamaArchitecture::Gemma4 => {
            let block_spec = gemma4_attention_block_spec(model, layer_index)?;
            let layout = gemma4_attention_block_layout(model, layer_index)?;
            let decode_spec = gemma4_attention_decode_spec(
                model,
                layer_index,
                token_count,
                1,
                cache_type,
                cache_type,
            )?;
            Ok((layer_index, block_spec, layout, decode_spec))
        }
        LlamaArchitecture::Unknown(name) => {
            Err(format!("unsupported architecture for attention self-check: {name}").into())
        }
    }
}

fn attention_cache_self_check(
    model: &LlamaModel,
    token_ids: &[i32],
    cache_type: TensorType,
) -> Result<AttentionCacheSelfCheck, Box<dyn std::error::Error>> {
    let setup = first_attention_check_setup(model, token_ids.len(), cache_type)?;
    attention_cache_self_check_with_setup(model, token_ids, setup)
}

fn attention_cache_self_check_for_layer(
    model: &LlamaModel,
    token_ids: &[i32],
    layer_index: u32,
    cache_type: TensorType,
) -> Result<AttentionCacheSelfCheck, Box<dyn std::error::Error>> {
    let setup = attention_check_setup_for_layer(
        model,
        layer_index,
        u32::try_from(token_ids.len())?,
        cache_type,
    )?;
    attention_cache_self_check_with_setup(model, token_ids, setup)
}

fn attention_cache_self_check_with_setup(
    model: &LlamaModel,
    token_ids: &[i32],
    (layer_index, block_spec, layout, decode_spec): (
        u32,
        AttentionBlockSpec,
        GgufWeightLayout,
        AttentionDecodeSpec,
    ),
) -> Result<AttentionCacheSelfCheck, Box<dyn std::error::Error>> {
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut full_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let compiled_full =
        compile_attention_block_metal(&mut full_loaded, &block_spec, token_ids.len())?;
    let full_run = execute_attention_block_graph_metal_cached(
        &compiled_full,
        &mut full_loaded,
        LogitsProbeInput::TokenIds(token_ids),
        &positions,
    )?;

    let decode_run = run_attention_decode_sequence_exact(
        model,
        &layout,
        &decode_spec,
        &positions,
        AttentionDecodeSequenceInput::TokenIds(token_ids),
    )?;
    let decode_last_hidden = decode_run.last_hidden;
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
    let (layer_index, layout, spec) = match &model.architecture {
        LlamaArchitecture::Qwen35 => {
            let (layer_index, _) = qwen35_first_recurrent_block_spec(model)?;
            let layout = qwen35_recurrent_block_layout(model, layer_index)?;
            let spec = qwen35_delta_net_recurrent_decode_spec(
                model,
                layer_index,
                1,
                TensorType::F32,
                TensorType::F32,
            )?;
            (layer_index, layout, spec)
        }
        LlamaArchitecture::Qwen35Moe => {
            let (layer_index, _) = qwen35moe_first_recurrent_block_spec(model)?;
            let layout = qwen35moe_recurrent_block_layout(model, layer_index)?;
            let spec = qwen35moe_delta_net_recurrent_decode_spec(
                model,
                layer_index,
                1,
                TensorType::F32,
                TensorType::F32,
            )?;
            (layer_index, layout, spec)
        }
        LlamaArchitecture::Gemma4 => {
            return Err(
                "recurrent self-check is not implemented for gemma4 because gemma4 has no recurrent layers"
                    .into(),
            );
        }
        LlamaArchitecture::Unknown(name) => {
            return Err(
                format!("unsupported architecture for recurrent self-check: {name}").into(),
            );
        }
    };

    let mut full_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let full_compiled =
        compile_delta_net_recurrent_decode_metal(&mut full_loaded, &spec, token_ids.len())?;
    let full_run = execute_delta_net_recurrent_decode_graph_metal_cached(
        &full_compiled,
        &mut full_loaded,
        LogitsProbeInput::TokenIds(token_ids),
    )?;
    let full_r_cache = read_tensor_f32s(&full_loaded.ctx, "recur_decode.r_cache")?;
    let full_s_cache = read_tensor_f32s(&full_loaded.ctx, "recur_decode.s_cache")?;

    let mut decode_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
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

    let mut full_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let full_runtime = MetalRuntime::new()?;
    let full_features = full_runtime.features();
    let mut full_graph = build_delta_net_recurrent_decode_graph(
        &mut full_loaded.ctx,
        &full_loaded.tensor_ids,
        &spec,
        token_ids.len(),
    )?;
    let full_input_embed_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.input_embed",
        "recur_decode.input_embed_ck",
    )?;
    let full_input_norm_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.input_norm",
        "recur_decode.input_norm_ck",
    )?;
    let full_qkv_mixed_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.qkv_mixed",
        "recur_decode.qkv_mixed_ck",
    )?;
    let full_qkv_mixed_flat_id = add_flattened_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.qkv_mixed",
        "recur_decode.qkv_mixed_full_ck",
    )?;
    let full_qkv_mixed_transposed_flat_id = add_flattened_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.qkv_mixed_transposed",
        "recur_decode.qkv_mixed_transposed_full_ck",
    )?;
    let full_conv_states_reshaped_flat_id = add_flattened_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.conv_states_reshaped",
        "recur_decode.conv_states_reshaped_full_ck",
    )?;
    let full_z_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.z",
        "recur_decode.z_ck",
    )?;
    let full_beta_id = add_token_dim2_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.beta",
        "recur_decode.beta_ck",
    )?;
    let full_gate_id = add_token_dim2_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.gate",
        "recur_decode.gate_ck",
    )?;
    let conv_kernel_rows = full_loaded
        .ctx
        .tensor(
            *full_loaded
                .tensor_ids
                .get(&spec.block.conv_kernel_name)
                .ok_or("missing recurrent conv kernel tensor id")?,
        )
        .ok_or("invalid recurrent conv kernel tensor")?
        .ne[0];
    let full_conv_input_id = add_last_dim0_rows_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.conv_input",
        "recur_decode.conv_input_ck",
        conv_kernel_rows,
    )?;
    let full_conv_output_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.conv_output",
        "recur_decode.conv_output_ck",
    )?;
    let full_q_conv_predelta_id = add_token_dim2_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.q_conv_predelta",
        "recur_decode.q_conv_predelta_ck",
    )?;
    let full_k_conv_predelta_id = add_token_dim2_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.k_conv_predelta",
        "recur_decode.k_conv_predelta_ck",
    )?;
    let full_output_view_id = add_token_dim2_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.output_view",
        "recur_decode.output_view_ck",
    )?;
    let full_output_norm_id = add_token_dim2_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.output_norm",
        "recur_decode.output_norm_ck",
    )?;
    let full_z_silu_id = add_token_dim2_checkpoint_by_name(
        &mut full_loaded.ctx,
        "recur_decode.z_silu",
        "recur_decode.z_silu_ck",
    )?;
    let full_gated_output_id = add_token_dim2_checkpoint_by_name(
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
        full_input_embed_id,
        full_input_norm_id,
        full_qkv_mixed_id,
        full_qkv_mixed_flat_id,
        full_qkv_mixed_transposed_flat_id,
        full_conv_states_reshaped_flat_id,
        full_z_id,
        full_beta_id,
        full_gate_id,
        full_conv_input_id,
        full_conv_output_id,
        full_q_conv_predelta_id,
        full_k_conv_predelta_id,
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
        full_input_embed_id,
        full_input_norm_id,
        full_qkv_mixed_id,
        full_qkv_mixed_flat_id,
        full_qkv_mixed_transposed_flat_id,
        full_conv_states_reshaped_flat_id,
        full_z_id,
        full_beta_id,
        full_gate_id,
        full_conv_input_id,
        full_conv_output_id,
        full_q_conv_predelta_id,
        full_k_conv_predelta_id,
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
    let full_input_embed =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_input_embed_id)?;
    let full_input_norm =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_input_norm_id)?;
    let full_qkv_mixed =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_qkv_mixed_id)?;
    let full_qkv_mixed_flat = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_qkv_mixed_flat_id)
            .ok_or("missing recurrent full qkv_mixed output")?,
    );
    let full_qkv_mixed_transposed_flat = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_qkv_mixed_transposed_flat_id)
            .ok_or("missing recurrent full qkv_mixed_transposed output")?,
    );
    let full_conv_states_reshaped_flat = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_conv_states_reshaped_flat_id)
            .ok_or("missing recurrent full conv_states_reshaped output")?,
    );
    let full_z = output_last_token_slice(&full_loaded.ctx, &full_execution, full_z_id)?;
    let full_beta = output_last_token_slice(&full_loaded.ctx, &full_execution, full_beta_id)?;
    let full_gate = output_last_token_slice(&full_loaded.ctx, &full_execution, full_gate_id)?;
    let full_conv_input =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_conv_input_id)?;
    let full_conv_output =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_conv_output_id)?;
    let full_q_conv_predelta =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_q_conv_predelta_id)?;
    let full_k_conv_predelta =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_k_conv_predelta_id)?;
    let full_output_view =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_output_view_id)?;
    let full_output_norm =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_output_norm_id)?;
    let full_z_silu = output_last_token_slice(&full_loaded.ctx, &full_execution, full_z_silu_id)?;
    let full_gated_output =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_gated_output_id)?;
    let full_final_output =
        output_last_token_slice(&full_loaded.ctx, &full_execution, full_final_output_id)?;

    let mut step_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let step_runtime = MetalRuntime::new()?;
    let step_features = step_runtime.features();
    let mut step_graph = build_delta_net_recurrent_decode_graph(
        &mut step_loaded.ctx,
        &step_loaded.tensor_ids,
        &spec,
        1,
    )?;
    let step_input_embed_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.input_embed",
        "recur_decode.input_embed_ck",
    )?;
    let step_input_norm_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.input_norm",
        "recur_decode.input_norm_ck",
    )?;
    let step_qkv_mixed_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.qkv_mixed",
        "recur_decode.qkv_mixed_ck",
    )?;
    let step_z_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.z",
        "recur_decode.z_ck",
    )?;
    let step_beta_id = add_token_dim2_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.beta",
        "recur_decode.beta_ck",
    )?;
    let step_gate_id = add_token_dim2_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.gate",
        "recur_decode.gate_ck",
    )?;
    let step_conv_input_id = add_last_dim0_rows_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.conv_input",
        "recur_decode.conv_input_ck",
        conv_kernel_rows,
    )?;
    let step_conv_output_id = add_hidden_token_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.conv_output",
        "recur_decode.conv_output_ck",
    )?;
    let step_q_conv_predelta_id = add_token_dim2_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.q_conv_predelta",
        "recur_decode.q_conv_predelta_ck",
    )?;
    let step_k_conv_predelta_id = add_token_dim2_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.k_conv_predelta",
        "recur_decode.k_conv_predelta_ck",
    )?;
    let step_output_view_id = add_token_dim2_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.output_view",
        "recur_decode.output_view_ck",
    )?;
    let step_output_norm_id = add_token_dim2_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.output_norm",
        "recur_decode.output_norm_ck",
    )?;
    let step_z_silu_id = add_token_dim2_checkpoint_by_name(
        &mut step_loaded.ctx,
        "recur_decode.z_silu",
        "recur_decode.z_silu_ck",
    )?;
    let step_gated_output_id = add_token_dim2_checkpoint_by_name(
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
        step_input_embed_id,
        step_input_norm_id,
        step_qkv_mixed_id,
        step_z_id,
        step_beta_id,
        step_gate_id,
        step_conv_input_id,
        step_conv_output_id,
        step_q_conv_predelta_id,
        step_k_conv_predelta_id,
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
        step_input_embed_id,
        step_input_norm_id,
        step_qkv_mixed_id,
        step_z_id,
        step_beta_id,
        step_gate_id,
        step_conv_input_id,
        step_conv_output_id,
        step_q_conv_predelta_id,
        step_k_conv_predelta_id,
        step_output_view_id,
        step_output_norm_id,
        step_z_silu_id,
        step_gated_output_id,
        step_final_output_id,
    ];
    let mut step_input_embed = None;
    let mut step_input_norm = None;
    let mut step_qkv_mixed = None;
    let mut step_qkv_mixed_history = Vec::with_capacity(token_ids.len());
    let mut step_z = None;
    let mut step_beta = None;
    let mut step_gate = None;
    let mut step_conv_input = None;
    let mut step_conv_output = None;
    let mut step_q_conv_predelta = None;
    let mut step_k_conv_predelta = None;
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
        step_input_embed = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_input_embed_id,
        )?);
        step_input_norm = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_input_norm_id,
        )?);
        step_qkv_mixed = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_qkv_mixed_id,
        )?);
        step_qkv_mixed_history.push(
            step_qkv_mixed
                .as_deref()
                .ok_or("missing recurrent step qkv_mixed slice")?
                .to_vec(),
        );
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
        step_conv_input = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_conv_input_id,
        )?);
        step_conv_output = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_conv_output_id,
        )?);
        step_q_conv_predelta = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_q_conv_predelta_id,
        )?);
        step_k_conv_predelta = Some(output_last_token_slice(
            &step_loaded.ctx,
            &execution,
            step_k_conv_predelta_id,
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

    let qkv_token_width = step_qkv_mixed_history
        .first()
        .map(Vec::len)
        .ok_or("missing recurrent step qkv_mixed history")?;
    let mut step_qkv_mixed_flat = Vec::with_capacity(
        qkv_token_width
            .checked_mul(step_qkv_mixed_history.len())
            .ok_or("overflow building recurrent qkv_mixed history")?,
    );
    for token_slice in &step_qkv_mixed_history {
        if token_slice.len() != qkv_token_width {
            return Err("recurrent qkv_mixed history width mismatch".into());
        }
        step_qkv_mixed_flat.extend_from_slice(token_slice);
    }
    let mut step_qkv_mixed_transposed_flat = Vec::with_capacity(step_qkv_mixed_flat.len());
    for hidden_index in 0..qkv_token_width {
        for token_slice in &step_qkv_mixed_history {
            step_qkv_mixed_transposed_flat.push(token_slice[hidden_index]);
        }
    }

    if let ProbeInputKind::TokenIds {
        token_embedding_name,
        ..
    } = &spec.block.input
    {
        let token_embd_id = *full_loaded
            .tensor_ids
            .get(token_embedding_name)
            .ok_or("missing recurrent token embedding tensor id")?;
        let token_embd = full_loaded
            .ctx
            .tensor(token_embd_id)
            .ok_or("invalid recurrent token embedding tensor")?
            .clone();
        println!(
            "recurrent_tensor.layer{}._input_embed_tensor_type: {}",
            layer_index,
            token_embd.desc.ty.name()
        );
        let hidden_size = usize::try_from(token_embd.ne[0])?;
        let vocab_size = usize::try_from(token_embd.ne[1])?;
        let input_embed_cpu = get_rows_ggml_bytes_cpu(
            full_loaded.ctx.tensor_data(token_embd_id)?,
            token_embd.desc.ty.ggml_type(),
            hidden_size,
            vocab_size,
            &[*token_ids.last().ok_or("missing recurrent last token")?],
        )
        .ok_or("unsupported CPU token embedding get_rows")?;
        let full_input_embed_cpu_stats = compare_logits(&full_input_embed, &input_embed_cpu);
        let step_input_embed_cpu_stats = compare_logits(
            step_input_embed
                .as_deref()
                .ok_or("missing recurrent step input_embed")?,
            &input_embed_cpu,
        );
        println!(
            "recurrent_tensor.layer{}._input_embed_full_cpu_max_abs_diff: {:.9}",
            layer_index, full_input_embed_cpu_stats.max_abs_diff
        );
        println!(
            "recurrent_tensor.layer{}._input_embed_step_cpu_max_abs_diff: {:.9}",
            layer_index, step_input_embed_cpu_stats.max_abs_diff
        );
    }

    Ok(RecurrentTensorCheck {
        layer_index,
        input_embed_stats: compare_logits(
            &full_input_embed,
            step_input_embed
                .as_deref()
                .ok_or("missing recurrent step input_embed")?,
        ),
        input_norm_stats: compare_logits(
            &full_input_norm,
            step_input_norm
                .as_deref()
                .ok_or("missing recurrent step input_norm")?,
        ),
        qkv_mixed_stats: compare_logits(
            &full_qkv_mixed,
            step_qkv_mixed
                .as_deref()
                .ok_or("missing recurrent step qkv_mixed")?,
        ),
        qkv_mixed_full_stats: compare_logits(&full_qkv_mixed_flat, &step_qkv_mixed_flat),
        qkv_mixed_transposed_stats: compare_logits(
            &full_qkv_mixed_transposed_flat,
            &step_qkv_mixed_transposed_flat,
        ),
        conv_states_reshaped_zero_stats: compare_logits(
            &full_conv_states_reshaped_flat,
            &vec![0.0; full_conv_states_reshaped_flat.len()],
        ),
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
        conv_input_stats: compare_logits(
            &full_conv_input,
            step_conv_input
                .as_deref()
                .ok_or("missing recurrent step conv_input")?,
        ),
        q_conv_predelta_stats: compare_logits(
            &full_q_conv_predelta,
            step_q_conv_predelta
                .as_deref()
                .ok_or("missing recurrent step q_conv_predelta")?,
        ),
        k_conv_predelta_stats: compare_logits(
            &full_k_conv_predelta,
            step_k_conv_predelta
                .as_deref()
                .ok_or("missing recurrent step k_conv_predelta")?,
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

fn recurrent_upstream_preview_check(
    args: &Args,
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<RecurrentUpstreamPreviewCheck, Box<dyn std::error::Error>> {
    let (layer_index, _) = qwen35moe_first_recurrent_block_spec(model)?;
    let upstream_filters = vec![
        format!("attn_norm-{layer_index}"),
        format!("final_output-{layer_index}"),
        format!("linear_attn_out-{layer_index}"),
        format!("attn_residual-{layer_index}"),
    ];
    let upstream_filter_refs = upstream_filters
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let upstream = run_upstream_debug_tensor_previews(args, &upstream_filter_refs)?;

    let layout = qwen35moe_recurrent_block_layout(model, layer_index)?;
    let spec = qwen35moe_delta_net_recurrent_decode_spec(
        model,
        layer_index,
        1,
        TensorType::F32,
        TensorType::F32,
    )?;
    let mut loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let runtime = MetalRuntime::new()?;
    let features = runtime.features();
    let mut graph = build_delta_net_recurrent_decode_graph(
        &mut loaded.ctx,
        &loaded.tensor_ids,
        &spec,
        token_ids.len(),
    )?;
    let input_norm_id = tensor_id_by_name(&loaded.ctx, "recur_decode.input_norm")?;
    let final_output_id = tensor_id_by_name(&loaded.ctx, "recur_decode.final_output")?;
    let linear_attn_out_id = tensor_id_by_name(&loaded.ctx, "recur_decode.linear_attn_out")?;
    for tensor_id in [input_norm_id, final_output_id, linear_attn_out_id] {
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
    let token_bytes = i32s_to_bytes(token_ids);
    let state_row_bytes = i32s_to_bytes(&[0]);
    let execution = session.execute(
        &loaded.ctx,
        &[
            MetalGraphTensorWrite {
                tensor_id: graph.input_primary,
                bytes: &token_bytes,
            },
            MetalGraphTensorWrite {
                tensor_id: graph.input_state_rows,
                bytes: &state_row_bytes,
            },
        ],
        &[
            input_norm_id,
            final_output_id,
            linear_attn_out_id,
            graph.result_output,
        ],
    )?;
    let rust_input_norm = tensor_preview_from_execution(&loaded.ctx, &execution, input_norm_id)?;
    let rust_input_norm_full =
        tensor_values_from_execution_f32(&loaded.ctx, &execution, input_norm_id)?;
    let rust_final_output =
        tensor_preview_from_execution(&loaded.ctx, &execution, final_output_id)?;
    let rust_linear_attn_out =
        tensor_preview_from_execution(&loaded.ctx, &execution, linear_attn_out_id)?;
    let rust_attn_residual =
        tensor_preview_from_execution(&loaded.ctx, &execution, graph.result_output)?;

    let input_norm_cpu = match &spec.block.input {
        ProbeInputKind::TokenIds {
            token_embedding_name,
            ..
        } => {
            let token_embd_id = *loaded
                .tensor_ids
                .get(token_embedding_name)
                .ok_or("missing recurrent token embedding tensor id")?;
            let token_embd = loaded
                .ctx
                .tensor(token_embd_id)
                .ok_or("invalid recurrent token embedding tensor")?
                .clone();
            let hidden_size = usize::try_from(token_embd.ne[0])?;
            let vocab_size = usize::try_from(token_embd.ne[1])?;
            let input_embed = get_rows_ggml_bytes_cpu(
                loaded.ctx.tensor_data(token_embd_id)?,
                token_embd.desc.ty.ggml_type(),
                hidden_size,
                vocab_size,
                token_ids,
            )
            .ok_or("unsupported recurrent CPU token embedding get_rows")?;
            let norm_weight_id = *loaded
                .tensor_ids
                .get(&spec.block.input_norm_name)
                .ok_or("missing recurrent input norm weight tensor id")?;
            let norm_weight = bytes_to_f32s(loaded.ctx.tensor_data(norm_weight_id)?);
            cpu_rms_norm_mul_rows(
                &input_embed,
                hidden_size,
                token_ids.len(),
                &norm_weight,
                spec.block.rms_epsilon,
            )?
        }
        ProbeInputKind::Embeddings { .. } => {
            return Err("recurrent upstream preview check expects token-id input".into());
        }
    };

    let upstream_input_norm = upstream
        .get(&upstream_filters[0])
        .ok_or("missing upstream recurrent attn_norm preview")?;
    let upstream_final_output = upstream
        .get(&upstream_filters[1])
        .ok_or("missing upstream recurrent final_output preview")?;
    let upstream_linear_attn_out = upstream
        .get(&upstream_filters[2])
        .ok_or("missing upstream recurrent linear_attn_out preview")?;
    let upstream_attn_residual = upstream
        .get(&upstream_filters[3])
        .ok_or("missing upstream recurrent attn_residual preview")?;

    Ok(RecurrentUpstreamPreviewCheck {
        layer_index,
        input_norm_stats: compare_logits(&rust_input_norm.values, &upstream_input_norm.values),
        input_norm_cpu_stats: compare_logits(&rust_input_norm_full, &input_norm_cpu),
        final_output_stats: compare_logits(
            &rust_final_output.values,
            &upstream_final_output.values,
        ),
        linear_attn_out_stats: compare_logits(
            &rust_linear_attn_out.values,
            &upstream_linear_attn_out.values,
        ),
        attn_residual_stats: compare_logits(
            &rust_attn_residual.values,
            &upstream_attn_residual.values,
        ),
    })
}

fn recurrent_step_cpu_check(
    model: &LlamaModel,
    token_ids: &[i32],
    requested_layer_index: Option<u32>,
) -> Result<RecurrentStepCpuCheck, Box<dyn std::error::Error>> {
    if token_ids.len() < 2 {
        return Err("recurrent step cpu check requires at least two tokens".into());
    }

    let (layer_index, block_spec, layout, spec) = match &model.architecture {
        LlamaArchitecture::Qwen35 => {
            let (layer_index, block_spec) = if let Some(layer_index) = requested_layer_index {
                (
                    layer_index,
                    qwen35_recurrent_block_spec(model, layer_index)?,
                )
            } else {
                qwen35_first_recurrent_block_spec(model)?
            };
            let layout = qwen35_recurrent_block_layout(model, layer_index)?;
            let spec = qwen35_delta_net_recurrent_decode_spec(
                model,
                layer_index,
                1,
                TensorType::F32,
                TensorType::F32,
            )?;
            (layer_index, block_spec, layout, spec)
        }
        LlamaArchitecture::Qwen35Moe => {
            if requested_layer_index.is_some() {
                return Err(
                    "explicit recurrent layer checks are not implemented for qwen35moe".into(),
                );
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
            (layer_index, block_spec, layout, spec)
        }
        LlamaArchitecture::Gemma4 => {
            return Err(
                "recurrent step cpu check is not implemented for gemma4 because gemma4 has no recurrent layers"
                    .into(),
            );
        }
        LlamaArchitecture::Unknown(name) => {
            return Err(
                format!("unsupported architecture for recurrent step cpu check: {name}").into(),
            );
        }
    };

    let mut loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let runtime = MetalRuntime::new()?;
    let features = runtime.features();
    let mut graph =
        build_delta_net_recurrent_decode_graph(&mut loaded.ctx, &loaded.tensor_ids, &spec, 1)?;
    let conv_input_id = add_flattened_checkpoint_by_name(
        &mut loaded.ctx,
        "recur_decode.conv_input",
        "recur_decode.conv_input_ck",
    )?;
    let beta_id = add_token_dim2_checkpoint_by_name(
        &mut loaded.ctx,
        "recur_decode.beta",
        "recur_decode.beta_ck",
    )?;
    let gate_id = add_token_dim2_checkpoint_by_name(
        &mut loaded.ctx,
        "recur_decode.gate",
        "recur_decode.gate_ck",
    )?;
    let q_conv_id = add_token_dim2_checkpoint_by_name(
        &mut loaded.ctx,
        "recur_decode.q_conv_predelta",
        "recur_decode.q_conv_ck",
    )?;
    let k_conv_id = add_token_dim2_checkpoint_by_name(
        &mut loaded.ctx,
        "recur_decode.k_conv_predelta",
        "recur_decode.k_conv_ck",
    )?;
    let conv_output_id = add_hidden_token_checkpoint_by_name(
        &mut loaded.ctx,
        "recur_decode.conv_output",
        "recur_decode.conv_output_ck",
    )?;
    let output_view_id = add_token_dim2_checkpoint_by_name(
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

fn run_attention_decode_with_result_checkpoint(
    model: &LlamaModel,
    layout: &GgufWeightLayout,
    spec: &AttentionDecodeSpec,
    positions: &[i32],
    key_count: usize,
    input: AttentionDecodeSequenceInput<'_>,
    previous_k_cache: Option<&[u8]>,
    previous_v_cache: Option<&[u8]>,
) -> Result<AttentionDecodeSequenceRun, Box<dyn std::error::Error>> {
    if positions.is_empty() {
        return Err("attention decode checkpoint run requires at least one token".into());
    }
    match &input {
        AttentionDecodeSequenceInput::TokenIds(token_ids) => {
            if token_ids.len() != positions.len() {
                return Err(format!(
                    "attention decode token/position length mismatch: {} vs {}",
                    token_ids.len(),
                    positions.len()
                )
                .into());
            }
        }
        AttentionDecodeSequenceInput::EmbeddingsF32 { data, hidden_size } => {
            let expected = hidden_size
                .checked_mul(positions.len())
                .ok_or("overflow computing attention decode embedding length")?;
            if data.len() != expected {
                return Err(format!(
                    "attention decode embedding length mismatch: got {}, expected {}",
                    data.len(),
                    expected
                )
                .into());
            }
        }
    }

    let input_primary = match input {
        AttentionDecodeSequenceInput::TokenIds(token_ids) => i32s_to_bytes(token_ids),
        AttentionDecodeSequenceInput::EmbeddingsF32 { data, hidden_size } => {
            let expected = hidden_size
                .checked_mul(positions.len())
                .ok_or("overflow computing attention decode embedding input length")?;
            if data.len() != expected {
                return Err(format!(
                    "attention decode embedding input length mismatch: got {}, expected {}",
                    data.len(),
                    expected
                )
                .into());
            }
            f32s_to_bytes(data)
        }
    };

    let mut loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let runtime = MetalRuntime::new()?;
    let features = runtime.features();
    let (mut decode_graph, _) = prepare_attention_decode_graph_with_key_count(
        &mut loaded.ctx,
        &loaded.tensor_ids,
        spec,
        positions.len(),
        key_count,
        features,
    )?;
    let zero_k_cache = vec![0u8; loaded.ctx.tensor_data(decode_graph.k_cache)?.len()];
    let zero_v_cache = vec![0u8; loaded.ctx.tensor_data(decode_graph.v_cache)?.len()];
    loaded
        .ctx
        .write_tensor_data(decode_graph.k_cache, &zero_k_cache)?;
    loaded
        .ctx
        .write_tensor_data(decode_graph.v_cache, &zero_v_cache)?;
    if let Some(bytes) = previous_k_cache {
        loaded.ctx.write_tensor_data(decode_graph.k_cache, bytes)?;
    }
    if let Some(bytes) = previous_v_cache {
        loaded.ctx.write_tensor_data(decode_graph.v_cache, bytes)?;
    }
    let result_output_id = add_contiguous_checkpoint_by_any_name(
        &mut loaded.ctx,
        &["attn_decode.output_residual", "attn_decode.output_proj"],
        "attn_decode.result_output_ck",
    )?;
    let k_cache_view_id = add_contiguous_checkpoint(
        &mut loaded.ctx,
        decode_graph.k_cache_view,
        "attn_decode.k_cache_view_ck",
    )?;
    let v_cache_view_id = add_contiguous_checkpoint(
        &mut loaded.ctx,
        decode_graph.v_cache_view,
        "attn_decode.v_cache_view_ck",
    )?;
    let last_hidden_id = add_hidden_token_checkpoint_by_any_name(
        &mut loaded.ctx,
        &["attn_decode.output_residual", "attn_decode.output_proj"],
        "attn_decode.last_hidden_ck",
    )?;
    decode_graph
        .graph
        .build_forward_expand(&loaded.ctx, result_output_id)?;
    decode_graph
        .graph
        .build_forward_expand(&loaded.ctx, last_hidden_id)?;
    decode_graph
        .graph
        .build_forward_expand(&loaded.ctx, k_cache_view_id)?;
    decode_graph
        .graph
        .build_forward_expand(&loaded.ctx, v_cache_view_id)?;
    let prepared = prepare_graph(&loaded.ctx, &decode_graph.graph, features)?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &loaded.ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;

    let rope_positions = spec
        .block
        .rope
        .as_ref()
        .map(|rope| encode_rope_positions(rope, positions, positions.len()))
        .transpose()?;
    let rope_pos_bytes = rope_positions.as_deref().map(i32s_to_bytes);
    let positions_bytes = i32s_to_bytes(positions);
    let mut writes = vec![
        MetalGraphTensorWrite {
            tensor_id: decode_graph.input_primary,
            bytes: &input_primary,
        },
        MetalGraphTensorWrite {
            tensor_id: decode_graph.input_write_indices,
            bytes: &positions_bytes,
        },
    ];
    if let Some(input_rope_positions) = decode_graph.input_rope_positions {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_rope_positions,
            bytes: rope_pos_bytes
                .as_deref()
                .ok_or("attention decode rope positions were not prepared")?,
        });
    }
    let mask_bytes = decode_graph
        .input_mask
        .map(|input_mask| {
            position_attention_mask_bytes_for_tensor(
                &loaded.ctx,
                input_mask,
                compare_attention_mask_write_key_count(
                    &loaded.ctx,
                    input_mask,
                    i64::from(spec.block.q_head_dim),
                    key_count,
                    positions.len(),
                )?,
                positions,
            )
        })
        .transpose()?;
    if let Some(input_mask) = decode_graph.input_mask {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_mask,
            bytes: mask_bytes
                .as_deref()
                .ok_or("missing attention decode causal mask bytes")?,
        });
    }

    let execution = session.execute(
        &loaded.ctx,
        &writes,
        &[
            result_output_id,
            last_hidden_id,
            k_cache_view_id,
            v_cache_view_id,
        ],
    )?;
    let result_output = bytes_to_f32s(
        execution
            .outputs
            .get(&result_output_id)
            .ok_or("missing attention decode result_output checkpoint output")?,
    );
    if result_output.is_empty() {
        return Err("attention decode result_output checkpoint was empty".into());
    }
    let mut k_cache_bytes = vec![0u8; loaded.ctx.tensor_data(decode_graph.k_cache)?.len()];
    let exported_k_cache_view = execution
        .outputs
        .get(&k_cache_view_id)
        .ok_or("missing attention decode k_cache_view checkpoint output")?;
    if exported_k_cache_view.len() > k_cache_bytes.len() {
        return Err(format!(
            "attention decode exported k_cache_view size {} exceeds cache buffer size {}",
            exported_k_cache_view.len(),
            k_cache_bytes.len()
        )
        .into());
    }
    k_cache_bytes[..exported_k_cache_view.len()].copy_from_slice(exported_k_cache_view);
    let mut v_cache_bytes = vec![0u8; loaded.ctx.tensor_data(decode_graph.v_cache)?.len()];
    let exported_v_cache_view = execution
        .outputs
        .get(&v_cache_view_id)
        .ok_or("missing attention decode v_cache_view checkpoint output")?;
    if exported_v_cache_view.len() > v_cache_bytes.len() {
        return Err(format!(
            "attention decode exported v_cache_view size {} exceeds cache buffer size {}",
            exported_v_cache_view.len(),
            v_cache_bytes.len()
        )
        .into());
    }
    v_cache_bytes[..exported_v_cache_view.len()].copy_from_slice(exported_v_cache_view);
    Ok(AttentionDecodeSequenceRun {
        result_output,
        last_hidden: bytes_to_f32s(
            execution
                .outputs
                .get(&last_hidden_id)
                .ok_or("missing attention decode last_hidden checkpoint output")?,
        ),
        k_cache_bytes,
        v_cache_bytes,
    })
}

fn run_attention_decode_sequence_exact(
    model: &LlamaModel,
    layout: &GgufWeightLayout,
    spec: &AttentionDecodeSpec,
    positions: &[i32],
    input: AttentionDecodeSequenceInput<'_>,
) -> Result<AttentionDecodeSequenceRun, Box<dyn std::error::Error>> {
    if positions.is_empty() {
        return Err("attention decode step sequence requires at least one token".into());
    }

    match &input {
        AttentionDecodeSequenceInput::TokenIds(token_ids) => {
            if token_ids.len() != positions.len() {
                return Err(format!(
                    "attention decode token/position length mismatch: {} vs {}",
                    token_ids.len(),
                    positions.len()
                )
                .into());
            }
        }
        AttentionDecodeSequenceInput::EmbeddingsF32 { data, hidden_size } => {
            let expected = hidden_size
                .checked_mul(positions.len())
                .ok_or("overflow computing attention decode embedding length")?;
            if data.len() != expected {
                return Err(format!(
                    "attention decode embedding length mismatch: got {}, expected {}",
                    data.len(),
                    expected
                )
                .into());
            }
        }
    }

    let stable_key_count = usize::try_from(spec.cache.max_context)?;
    if positions.iter().copied().any(|position| {
        position < 0
            || usize::try_from(position)
                .ok()
                .map(|position| position >= stable_key_count)
                .unwrap_or(true)
    }) {
        return Err(format!(
            "attention decode step sequence positions {:?} exceed stable key_count {}",
            positions, stable_key_count
        )
        .into());
    }

    let mut previous_k_cache = None;
    let mut previous_v_cache = None;
    let mut result_output = Vec::new();
    let mut last_hidden = None;

    for token_index in 0..positions.len() {
        let run = match &input {
            AttentionDecodeSequenceInput::TokenIds(token_ids) => {
                run_attention_decode_with_result_checkpoint(
                    model,
                    layout,
                    spec,
                    &positions[token_index..token_index + 1],
                    stable_key_count,
                    AttentionDecodeSequenceInput::TokenIds(std::slice::from_ref(
                        &token_ids[token_index],
                    )),
                    previous_k_cache.as_deref(),
                    previous_v_cache.as_deref(),
                )?
            }
            AttentionDecodeSequenceInput::EmbeddingsF32 { data, hidden_size } => {
                let start = token_index
                    .checked_mul(*hidden_size)
                    .ok_or("overflow computing attention embedding token start")?;
                let end = start
                    .checked_add(*hidden_size)
                    .ok_or("overflow computing attention embedding token end")?;
                run_attention_decode_with_result_checkpoint(
                    model,
                    layout,
                    spec,
                    &positions[token_index..token_index + 1],
                    stable_key_count,
                    AttentionDecodeSequenceInput::EmbeddingsF32 {
                        data: &data[start..end],
                        hidden_size: *hidden_size,
                    },
                    previous_k_cache.as_deref(),
                    previous_v_cache.as_deref(),
                )?
            }
        };

        result_output.extend_from_slice(&run.result_output);
        last_hidden = Some(run.last_hidden);
        previous_k_cache = Some(run.k_cache_bytes);
        previous_v_cache = Some(run.v_cache_bytes);
    }

    Ok(AttentionDecodeSequenceRun {
        result_output,
        last_hidden: last_hidden.ok_or("attention decode step sequence produced no output")?,
        k_cache_bytes: previous_k_cache
            .ok_or("attention decode step sequence produced no k cache state")?,
        v_cache_bytes: previous_v_cache
            .ok_or("attention decode step sequence produced no v cache state")?,
    })
}

fn attention_decode_batch_self_check(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<AttentionDecodeBatchSelfCheck, Box<dyn std::error::Error>> {
    let setup = first_attention_check_setup(model, token_ids.len(), TensorType::F32)?;
    attention_decode_batch_self_check_with_setup(model, token_ids, setup)
}

fn attention_decode_batch_self_check_for_layer(
    model: &LlamaModel,
    token_ids: &[i32],
    layer_index: u32,
) -> Result<AttentionDecodeBatchSelfCheck, Box<dyn std::error::Error>> {
    let setup = attention_check_setup_for_layer(
        model,
        layer_index,
        u32::try_from(token_ids.len())?,
        TensorType::F32,
    )?;
    attention_decode_batch_self_check_with_setup(model, token_ids, setup)
}

fn attention_decode_batch_self_check_with_setup(
    model: &LlamaModel,
    token_ids: &[i32],
    (layer_index, _, layout, spec): (
        u32,
        AttentionBlockSpec,
        GgufWeightLayout,
        AttentionDecodeSpec,
    ),
) -> Result<AttentionDecodeBatchSelfCheck, Box<dyn std::error::Error>> {
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let step0_run = run_attention_decode_with_result_checkpoint(
        model,
        &layout,
        &spec,
        &positions[..1],
        1,
        AttentionDecodeSequenceInput::TokenIds(&token_ids[..1]),
        None,
        None,
    )?;

    let batched_run = run_attention_decode_with_result_checkpoint(
        model,
        &layout,
        &spec,
        &positions,
        token_ids.len(),
        AttentionDecodeSequenceInput::TokenIds(token_ids),
        None,
        None,
    )?;
    let batched_last_hidden = batched_run.last_hidden.as_slice();
    let batched_k_cache = bytes_to_f32s(&batched_run.k_cache_bytes);
    let batched_v_cache = bytes_to_f32s(&batched_run.v_cache_bytes);
    let step0_k_cache = bytes_to_f32s(&step0_run.k_cache_bytes);
    let step0_v_cache = bytes_to_f32s(&step0_run.v_cache_bytes);
    let k_row_width = usize::try_from(spec.block.k_head_dim)?
        .checked_mul(usize::try_from(spec.block.kv_head_count)?)
        .ok_or("overflow computing attention decode k row width")?;
    let v_row_width = usize::try_from(spec.block.v_head_dim)?
        .checked_mul(usize::try_from(spec.block.kv_head_count)?)
        .ok_or("overflow computing attention decode v row width")?;

    let step_run = run_attention_decode_sequence_exact(
        model,
        &layout,
        &spec,
        &positions,
        AttentionDecodeSequenceInput::TokenIds(token_ids),
    )?;
    let batched_result_output = batched_run.result_output.as_slice();
    let step_result_output = step_run.result_output.as_slice();
    if batched_result_output.len() != step_result_output.len() {
        return Err(format!(
            "attention decode batch result_output length mismatch: {} vs {}",
            batched_result_output.len(),
            step_result_output.len()
        )
        .into());
    }
    let hidden_size = step_result_output
        .len()
        .checked_div(token_ids.len())
        .ok_or("attention decode batch hidden width division failed")?;
    if hidden_size == 0 || hidden_size * token_ids.len() != step_result_output.len() {
        return Err(format!(
            "unexpected attention decode batch result_output length {} for {} tokens",
            step_result_output.len(),
            token_ids.len()
        )
        .into());
    }
    let step_last_hidden = step_run.last_hidden;
    let step_k_cache = bytes_to_f32s(&step_run.k_cache_bytes);
    let step_v_cache = bytes_to_f32s(&step_run.v_cache_bytes);

    Ok(AttentionDecodeBatchSelfCheck {
        layer_index,
        hidden_stats: compare_logits(batched_last_hidden, &step_last_hidden),
        result_output_stats: compare_logits(batched_result_output, step_result_output),
        first_token_result_output_stats: compare_logits(
            token_slice(batched_result_output, hidden_size, 0)?,
            token_slice(step_result_output, hidden_size, 0)?,
        ),
        last_token_result_output_stats: compare_logits(
            token_slice(batched_result_output, hidden_size, token_ids.len() - 1)?,
            token_slice(step_result_output, hidden_size, token_ids.len() - 1)?,
        ),
        step0_k_cache_row_stats: compare_logits(
            step0_k_cache
                .get(..k_row_width)
                .ok_or("step0 k_cache row slice was out of range")?,
            batched_k_cache
                .get(..k_row_width)
                .ok_or("batched k_cache row slice was out of range")?,
        ),
        step0_v_cache_row_stats: compare_logits(
            step0_v_cache
                .get(..v_row_width)
                .ok_or("step0 v_cache row slice was out of range")?,
            batched_v_cache
                .get(..v_row_width)
                .ok_or("batched v_cache row slice was out of range")?,
        ),
        step0_k_cache_tail_zero_stats: compare_logits(
            step0_k_cache
                .get(k_row_width..)
                .ok_or("step0 k_cache tail slice was out of range")?,
            &vec![0.0; step0_k_cache.len().saturating_sub(k_row_width)],
        ),
        step0_v_cache_tail_zero_stats: compare_logits(
            step0_v_cache
                .get(v_row_width..)
                .ok_or("step0 v_cache tail slice was out of range")?,
            &vec![0.0; step0_v_cache.len().saturating_sub(v_row_width)],
        ),
        k_cache_stats: compare_logits(&batched_k_cache, &step_k_cache),
        v_cache_stats: compare_logits(&batched_v_cache, &step_v_cache),
    })
}

fn recurrent_from_hidden_batch_self_check(
    model: &LlamaModel,
    token_ids: &[i32],
    source_layer_index: u32,
    layer_index: u32,
) -> Result<RecurrentFromHiddenBatchSelfCheck, Box<dyn std::error::Error>> {
    if token_ids.len() < 2 {
        return Err("recurrent hidden self-check requires at least two tokens".into());
    }
    if model.architecture != LlamaArchitecture::Qwen35 {
        return Err("recurrent hidden self-check is only implemented for qwen35".into());
    }

    let hidden_size = usize::try_from(model.require_qwen35()?.embedding_length)?;
    let max_context = u32::try_from(token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let output_ids = [i32::try_from(token_ids.len() - 1)?];
    let source_name = format!("hybrid_decode.layer{source_layer_index}.post_ffn");
    let label = format!("layer{source_layer_index}.post_ffn.full");
    let mut capture_session = build_single_hybrid_capture_session(
        model,
        max_context,
        token_ids.len(),
        &[source_name.as_str()],
        &label,
        false,
    )?;
    let captured = execute_hybrid_checkpoint_session(
        &mut capture_session,
        token_ids,
        &positions,
        token_ids.len(),
        &output_ids,
    )?;
    let hidden_input = captured
        .get(&label)
        .ok_or_else(|| format!("missing captured tensor '{label}'"))?;
    let expected_hidden_len = hidden_size
        .checked_mul(token_ids.len())
        .ok_or("overflow computing recurrent hidden input length")?;
    if hidden_input.len() != expected_hidden_len {
        return Err(format!(
            "captured hidden input length mismatch: got {}, expected {}",
            hidden_input.len(),
            expected_hidden_len
        )
        .into());
    }

    let mut spec = qwen35_delta_net_recurrent_decode_spec(
        model,
        layer_index,
        1,
        TensorType::F32,
        TensorType::F32,
    )?;
    spec.block.input = ProbeInputKind::Embeddings {
        hidden_size: u32::try_from(hidden_size)?,
        input_type: TensorType::F32,
    };
    let layout = qwen35_recurrent_block_layout(model, layer_index)?;

    let mut full_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let full_compiled =
        compile_delta_net_recurrent_decode_metal(&mut full_loaded, &spec, token_ids.len())?;
    let full_run = execute_delta_net_recurrent_decode_graph_metal_cached(
        &full_compiled,
        &mut full_loaded,
        LogitsProbeInput::EmbeddingsF32 {
            data: hidden_input,
            n_tokens: token_ids.len(),
        },
    )?;
    let full_last_hidden = last_token_slice(&full_run.hidden, full_run.hidden_size)?.to_vec();
    let full_r_cache = read_tensor_f32s(&full_loaded.ctx, "recur_decode.r_cache")?;
    let full_s_cache = read_tensor_f32s(&full_loaded.ctx, "recur_decode.s_cache")?;

    let mut step_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let step_compiled = compile_delta_net_recurrent_decode_metal(&mut step_loaded, &spec, 1)?;
    let mut step_last_hidden = None;
    for token_index in 0..token_ids.len() {
        let start = token_index
            .checked_mul(hidden_size)
            .ok_or("overflow computing recurrent hidden token start")?;
        let end = start
            .checked_add(hidden_size)
            .ok_or("overflow computing recurrent hidden token end")?;
        let run = execute_delta_net_recurrent_decode_graph_metal_cached(
            &step_compiled,
            &mut step_loaded,
            LogitsProbeInput::EmbeddingsF32 {
                data: &hidden_input[start..end],
                n_tokens: 1,
            },
        )?;
        step_last_hidden = Some(run.hidden);
    }
    let step_last_hidden =
        step_last_hidden.ok_or("recurrent hidden self-check did not produce output")?;
    let step_r_cache = read_tensor_f32s(&step_loaded.ctx, "recur_decode.r_cache")?;
    let step_s_cache = read_tensor_f32s(&step_loaded.ctx, "recur_decode.s_cache")?;

    Ok(RecurrentFromHiddenBatchSelfCheck {
        source_layer_index,
        layer_index,
        hidden_stats: compare_logits(&full_last_hidden, &step_last_hidden),
        r_cache_stats: compare_logits(&full_r_cache, &step_r_cache),
        s_cache_stats: compare_logits(&full_s_cache, &step_s_cache),
    })
}

fn attention_from_hidden_batch_self_check(
    model: &LlamaModel,
    token_ids: &[i32],
    source_layer_index: u32,
    layer_index: u32,
) -> Result<AttentionFromHiddenBatchSelfCheck, Box<dyn std::error::Error>> {
    if token_ids.len() < 2 {
        return Err("attention hidden self-check requires at least two tokens".into());
    }
    if model.architecture != LlamaArchitecture::Qwen35 {
        return Err("attention hidden self-check is only implemented for qwen35".into());
    }

    let hidden_size = usize::try_from(model.require_qwen35()?.embedding_length)?;
    let max_context = u32::try_from(token_ids.len())?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let output_ids = [i32::try_from(token_ids.len() - 1)?];
    let source_name = format!("hybrid_decode.layer{source_layer_index}.post_ffn");
    let label = format!("layer{source_layer_index}.post_ffn.full");
    let mut capture_session = build_single_hybrid_capture_session(
        model,
        max_context,
        token_ids.len(),
        &[source_name.as_str()],
        &label,
        false,
    )?;
    let captured = execute_hybrid_checkpoint_session(
        &mut capture_session,
        token_ids,
        &positions,
        token_ids.len(),
        &output_ids,
    )?;
    let hidden_input = captured
        .get(&label)
        .ok_or_else(|| format!("missing captured tensor '{label}'"))?;
    let expected_hidden_len = hidden_size
        .checked_mul(token_ids.len())
        .ok_or("overflow computing attention hidden input length")?;
    if hidden_input.len() != expected_hidden_len {
        return Err(format!(
            "captured hidden input length mismatch: got {}, expected {}",
            hidden_input.len(),
            expected_hidden_len
        )
        .into());
    }

    let mut spec = qwen35_attention_decode_spec(
        model,
        layer_index,
        max_context,
        1,
        TensorType::F32,
        TensorType::F32,
    )?;
    spec.block.input = ProbeInputKind::Embeddings {
        hidden_size: u32::try_from(hidden_size)?,
        input_type: TensorType::F32,
    };
    let layout = qwen35_attention_block_layout(model, layer_index)?;
    let step0_run = run_attention_decode_with_result_checkpoint(
        model,
        &layout,
        &spec,
        &positions[..1],
        1,
        AttentionDecodeSequenceInput::EmbeddingsF32 {
            data: &hidden_input[..hidden_size],
            hidden_size,
        },
        None,
        None,
    )?;

    let full_run = run_attention_decode_with_result_checkpoint(
        model,
        &layout,
        &spec,
        &positions,
        token_ids.len(),
        AttentionDecodeSequenceInput::EmbeddingsF32 {
            data: hidden_input,
            hidden_size,
        },
        None,
        None,
    )?;
    let full_last_hidden = full_run.last_hidden;
    let full_k_cache = bytes_to_f32s(&full_run.k_cache_bytes);
    let full_v_cache = bytes_to_f32s(&full_run.v_cache_bytes);
    let step0_k_cache = bytes_to_f32s(&step0_run.k_cache_bytes);
    let step0_v_cache = bytes_to_f32s(&step0_run.v_cache_bytes);
    let k_row_width = usize::try_from(spec.block.k_head_dim)?
        .checked_mul(usize::try_from(spec.block.kv_head_count)?)
        .ok_or("overflow computing attention hidden k row width")?;
    let v_row_width = usize::try_from(spec.block.v_head_dim)?
        .checked_mul(usize::try_from(spec.block.kv_head_count)?)
        .ok_or("overflow computing attention hidden v row width")?;

    let step_run = run_attention_decode_sequence_exact(
        model,
        &layout,
        &spec,
        &positions,
        AttentionDecodeSequenceInput::EmbeddingsF32 {
            data: hidden_input,
            hidden_size,
        },
    )?;
    let full_result_output = full_run.result_output.as_slice();
    let step_result_output = step_run.result_output.as_slice();
    if full_result_output.len() != step_result_output.len() {
        return Err(format!(
            "attention hidden batch result_output length mismatch: {} vs {}",
            full_result_output.len(),
            step_result_output.len()
        )
        .into());
    }
    let row_width = step_result_output
        .len()
        .checked_div(token_ids.len())
        .ok_or("attention hidden batch hidden width division failed")?;
    if row_width == 0 || row_width * token_ids.len() != step_result_output.len() {
        return Err(format!(
            "unexpected attention hidden batch result_output length {} for {} tokens",
            step_result_output.len(),
            token_ids.len()
        )
        .into());
    }
    let step_last_hidden = step_run.last_hidden;
    let step_k_cache = bytes_to_f32s(&step_run.k_cache_bytes);
    let step_v_cache = bytes_to_f32s(&step_run.v_cache_bytes);

    Ok(AttentionFromHiddenBatchSelfCheck {
        source_layer_index,
        layer_index,
        hidden_stats: compare_logits(&full_last_hidden, &step_last_hidden),
        result_output_stats: compare_logits(full_result_output, step_result_output),
        first_token_result_output_stats: compare_logits(
            token_slice(full_result_output, row_width, 0)?,
            token_slice(step_result_output, row_width, 0)?,
        ),
        last_token_result_output_stats: compare_logits(
            token_slice(full_result_output, row_width, token_ids.len() - 1)?,
            token_slice(step_result_output, row_width, token_ids.len() - 1)?,
        ),
        step0_k_cache_row_stats: compare_logits(
            step0_k_cache
                .get(..k_row_width)
                .ok_or("attention hidden step0 k_cache row slice was out of range")?,
            full_k_cache
                .get(..k_row_width)
                .ok_or("attention hidden full k_cache row slice was out of range")?,
        ),
        step0_v_cache_row_stats: compare_logits(
            step0_v_cache
                .get(..v_row_width)
                .ok_or("attention hidden step0 v_cache row slice was out of range")?,
            full_v_cache
                .get(..v_row_width)
                .ok_or("attention hidden full v_cache row slice was out of range")?,
        ),
        step0_k_cache_tail_zero_stats: compare_logits(
            step0_k_cache
                .get(k_row_width..)
                .ok_or("attention hidden step0 k_cache tail slice was out of range")?,
            &vec![0.0; step0_k_cache.len().saturating_sub(k_row_width)],
        ),
        step0_v_cache_tail_zero_stats: compare_logits(
            step0_v_cache
                .get(v_row_width..)
                .ok_or("attention hidden step0 v_cache tail slice was out of range")?,
            &vec![0.0; step0_v_cache.len().saturating_sub(v_row_width)],
        ),
        k_cache_stats: compare_logits(&full_k_cache, &step_k_cache),
        v_cache_stats: compare_logits(&full_v_cache, &step_v_cache),
    })
}

fn attention_from_hidden_first_step_capacity_check(
    model: &LlamaModel,
    token_id: i32,
    source_layer_index: u32,
    layer_index: u32,
    max_context: u32,
) -> Result<AttentionFromHiddenBatchSelfCheck, Box<dyn std::error::Error>> {
    if max_context <= 1 {
        return Err("attention first-step capacity check requires max_context > 1".into());
    }
    if model.architecture != LlamaArchitecture::Qwen35 {
        return Err("attention first-step capacity check is only implemented for qwen35".into());
    }

    let hidden_size = usize::try_from(model.require_qwen35()?.embedding_length)?;
    let source_name = format!("hybrid_decode.layer{source_layer_index}.post_ffn");
    let label = format!("layer{source_layer_index}.post_ffn.step0");
    let mut capture_session =
        build_single_hybrid_capture_session(model, 1, 1, &[source_name.as_str()], &label, false)?;
    let captured =
        execute_hybrid_checkpoint_session(&mut capture_session, &[token_id], &[0], 1, &[0])?;
    let hidden_input = captured
        .get(&label)
        .ok_or_else(|| format!("missing captured tensor '{label}'"))?;
    if hidden_input.len() != hidden_size {
        return Err(format!(
            "captured hidden input length mismatch: got {}, expected {}",
            hidden_input.len(),
            hidden_size
        )
        .into());
    }

    let mut spec_small =
        qwen35_attention_decode_spec(model, layer_index, 1, 1, TensorType::F32, TensorType::F32)?;
    spec_small.block.input = ProbeInputKind::Embeddings {
        hidden_size: u32::try_from(hidden_size)?,
        input_type: TensorType::F32,
    };
    let mut spec_wide = qwen35_attention_decode_spec(
        model,
        layer_index,
        max_context,
        1,
        TensorType::F32,
        TensorType::F32,
    )?;
    spec_wide.block.input = ProbeInputKind::Embeddings {
        hidden_size: u32::try_from(hidden_size)?,
        input_type: TensorType::F32,
    };
    let layout = qwen35_attention_block_layout(model, layer_index)?;

    let mut small_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let small_compiled =
        compile_attention_decode_metal_with_key_count(&mut small_loaded, &spec_small, 1, 1)?;
    let small_run = execute_attention_decode_graph_metal_cached(
        &small_compiled,
        &mut small_loaded,
        LogitsProbeInput::EmbeddingsF32 {
            data: hidden_input,
            n_tokens: 1,
        },
        &[0],
        1,
    )?;
    let small_k_cache = read_tensor_f32s(&small_loaded.ctx, "attn_decode.k_cache")?;
    let small_v_cache = read_tensor_f32s(&small_loaded.ctx, "attn_decode.v_cache")?;

    let mut wide_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let wide_compiled =
        compile_attention_decode_metal_with_key_count(&mut wide_loaded, &spec_wide, 1, 1)?;
    let wide_run = execute_attention_decode_graph_metal_cached(
        &wide_compiled,
        &mut wide_loaded,
        LogitsProbeInput::EmbeddingsF32 {
            data: hidden_input,
            n_tokens: 1,
        },
        &[0],
        1,
    )?;
    let wide_k_cache = read_tensor_f32s(&wide_loaded.ctx, "attn_decode.k_cache")?;
    let wide_v_cache = read_tensor_f32s(&wide_loaded.ctx, "attn_decode.v_cache")?;

    Ok(AttentionFromHiddenBatchSelfCheck {
        source_layer_index,
        layer_index,
        hidden_stats: compare_logits(&wide_run.hidden, &small_run.hidden),
        result_output_stats: compare_logits(&wide_run.hidden, &small_run.hidden),
        first_token_result_output_stats: compare_logits(&wide_run.hidden, &small_run.hidden),
        last_token_result_output_stats: compare_logits(&wide_run.hidden, &small_run.hidden),
        step0_k_cache_row_stats: compare_logits(&wide_k_cache, &small_k_cache),
        step0_v_cache_row_stats: compare_logits(&wide_v_cache, &small_v_cache),
        step0_k_cache_tail_zero_stats: compare_logits(&wide_k_cache, &small_k_cache),
        step0_v_cache_tail_zero_stats: compare_logits(&wide_v_cache, &small_v_cache),
        k_cache_stats: compare_logits(&wide_k_cache, &small_k_cache),
        v_cache_stats: compare_logits(&wide_v_cache, &small_v_cache),
    })
}

fn add_contiguous_checkpoint(
    ctx: &mut Context,
    src: TensorId,
    name: &str,
) -> Result<TensorId, Box<dyn std::error::Error>> {
    let _ = ctx
        .tensor(src)
        .ok_or_else(|| format!("invalid tensor id {src} for checkpoint {name}"))?;
    let cont = ctx.cont(src)?;
    ctx.set_tensor_name(cont, name)?;
    Ok(cont)
}

#[allow(dead_code)]
fn add_contiguous_checkpoint_by_name(
    ctx: &mut Context,
    src_name: &str,
    checkpoint_name: &str,
) -> Result<TensorId, Box<dyn std::error::Error>> {
    let src = ctx
        .get_tensor(src_name)
        .ok_or_else(|| format!("missing tensor '{src_name}'"))?;
    add_contiguous_checkpoint(ctx, src, checkpoint_name)
}

fn add_contiguous_checkpoint_by_any_name(
    ctx: &mut Context,
    src_names: &[&str],
    checkpoint_name: &str,
) -> Result<TensorId, Box<dyn std::error::Error>> {
    for src_name in src_names {
        if let Some(src) = ctx.get_tensor(src_name) {
            return add_contiguous_checkpoint(ctx, src, checkpoint_name);
        }
    }
    Err(format!("missing tensor, tried any of {}", src_names.join(", ")).into())
}

fn tensor_id_by_name(ctx: &Context, name: &str) -> Result<TensorId, Box<dyn std::error::Error>> {
    ctx.get_tensor(name)
        .ok_or_else(|| format!("missing tensor '{name}'").into())
}

fn tensor_id_by_any_name(
    ctx: &Context,
    names: &[&str],
) -> Result<TensorId, Box<dyn std::error::Error>> {
    for name in names {
        if let Some(tensor_id) = ctx.get_tensor(name) {
            return Ok(tensor_id);
        }
    }
    Err(format!("missing tensor, tried any of {}", names.join(", ")).into())
}

fn add_hidden_token_checkpoint_by_name(
    ctx: &mut Context,
    src_name: &str,
    checkpoint_name: &str,
) -> Result<TensorId, Box<dyn std::error::Error>> {
    add_last_token_checkpoint_by_name(ctx, src_name, checkpoint_name, 1)
}

fn maybe_add_hidden_token_checkpoint_by_name(
    ctx: &mut Context,
    src_name: &str,
    checkpoint_name: &str,
) -> Result<Option<TensorId>, Box<dyn std::error::Error>> {
    if ctx.get_tensor(src_name).is_none() {
        return Ok(None);
    }
    Ok(Some(add_hidden_token_checkpoint_by_name(
        ctx,
        src_name,
        checkpoint_name,
    )?))
}

fn add_hidden_token_checkpoint_by_any_name(
    ctx: &mut Context,
    src_names: &[&str],
    checkpoint_name: &str,
) -> Result<TensorId, Box<dyn std::error::Error>> {
    for src_name in src_names {
        if ctx.get_tensor(src_name).is_some() {
            return add_hidden_token_checkpoint_by_name(ctx, src_name, checkpoint_name);
        }
    }
    Err(format!("missing tensor, tried any of {}", src_names.join(", ")).into())
}

fn add_token_dim2_checkpoint_by_any_name(
    ctx: &mut Context,
    src_names: &[&str],
    checkpoint_name: &str,
) -> Result<TensorId, Box<dyn std::error::Error>> {
    for src_name in src_names {
        if ctx.get_tensor(src_name).is_some() {
            return add_token_dim2_checkpoint_by_name(ctx, src_name, checkpoint_name);
        }
    }
    Err(format!("missing tensor, tried any of {}", src_names.join(", ")).into())
}

fn add_token_dim2_checkpoint_by_name(
    ctx: &mut Context,
    src_name: &str,
    checkpoint_name: &str,
) -> Result<TensorId, Box<dyn std::error::Error>> {
    add_last_token_checkpoint_by_name(ctx, src_name, checkpoint_name, 2)
}

fn add_last_token_checkpoint_by_name(
    ctx: &mut Context,
    src_name: &str,
    checkpoint_name: &str,
    token_dim: usize,
) -> Result<TensorId, Box<dyn std::error::Error>> {
    let src = ctx
        .get_tensor(src_name)
        .ok_or_else(|| format!("missing tensor '{src_name}'"))?;
    let tensor = ctx
        .tensor(src)
        .ok_or_else(|| format!("invalid tensor id {src} for checkpoint {checkpoint_name}"))?
        .clone();
    if token_dim >= 4 {
        return Err(format!("unsupported token dim {} for '{}'", token_dim, src_name).into());
    }
    if tensor.ne[token_dim] <= 0 {
        return Err(format!(
            "tensor '{}' has invalid token dim{} {}",
            src_name, token_dim, tensor.ne[token_dim]
        )
        .into());
    }
    let token_offset = tensor.nb[token_dim]
        .checked_mul(usize::try_from(tensor.ne[token_dim] - 1).map_err(|_| "token index overflow")?)
        .ok_or("token checkpoint offset overflow")?;
    let mut ne = tensor.ne;
    ne[token_dim] = 1;
    let last_token_view = ctx.view_4d(
        src,
        ne[0],
        ne[1],
        ne[2],
        ne[3],
        tensor.nb[1],
        tensor.nb[2],
        tensor.nb[3],
        token_offset,
    )?;
    let width = (0..4)
        .filter(|&dim| dim != token_dim)
        .try_fold(1_i64, |acc, dim| {
            acc.checked_mul(ne[dim]).ok_or("checkpoint width overflow")
        })?;
    let cont = ctx.cont_2d(last_token_view, width, 1)?;
    ctx.set_tensor_name(cont, checkpoint_name)?;
    Ok(cont)
}

fn add_flattened_checkpoint_by_name(
    ctx: &mut Context,
    src_name: &str,
    checkpoint_name: &str,
) -> Result<TensorId, Box<dyn std::error::Error>> {
    let src = ctx
        .get_tensor(src_name)
        .ok_or_else(|| format!("missing tensor '{src_name}'"))?;
    let tensor = ctx
        .tensor(src)
        .ok_or_else(|| format!("invalid tensor id {src} for checkpoint {checkpoint_name}"))?
        .clone();
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

fn add_last_dim0_rows_checkpoint_by_name(
    ctx: &mut Context,
    src_name: &str,
    checkpoint_name: &str,
    row_count: i64,
) -> Result<TensorId, Box<dyn std::error::Error>> {
    let src = ctx
        .get_tensor(src_name)
        .ok_or_else(|| format!("missing tensor '{src_name}'"))?;
    let tensor = ctx
        .tensor(src)
        .ok_or_else(|| format!("invalid tensor id {src} for checkpoint {checkpoint_name}"))?
        .clone();
    if tensor.ne[0] <= 0 {
        return Err(format!("tensor '{src_name}' has invalid dim0 {}", tensor.ne[0]).into());
    }
    if row_count <= 0 || row_count > tensor.ne[0] {
        return Err(format!(
            "tensor '{}' cannot capture {} dim0 rows from extent {}",
            src_name, row_count, tensor.ne[0]
        )
        .into());
    }
    let start_row = tensor.ne[0] - row_count;
    let offset = match tensor.desc.layout.rank() {
        2 => ggml_row_size_for_type(tensor.desc.ty, start_row)?,
        3 | 4 => ggml_row_size_for_type(tensor.desc.ty, start_row)?,
        rank => {
            return Err(format!(
                "last-dim0-row checkpoint requires rank 2-4, got {} for '{}'",
                rank, src_name
            )
            .into())
        }
    };
    let view = ctx.view_4d(
        src,
        row_count,
        tensor.ne[1],
        tensor.ne[2],
        tensor.ne[3],
        tensor.nb[1],
        tensor.nb[2],
        tensor.nb[3],
        offset,
    )?;
    let cont = ctx.cont_2d(
        view,
        row_count
            .checked_mul(tensor.ne[1])
            .ok_or("last-dim0-row checkpoint width overflow")?,
        tensor.ne[2] * tensor.ne[3],
    )?;
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
    for component in 0..n_components {
        let start = component * n_tokens;
        let end = start + n_tokens;
        expanded[start..end].copy_from_slice(positions);
    }
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

fn position_causal_mask_f16_bytes(
    key_count: usize,
    positions: &[i32],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::with_capacity(
        positions
            .len()
            .checked_mul(key_count)
            .and_then(|v| v.checked_mul(std::mem::size_of::<u16>()))
            .ok_or("overflow computing attention decode f16 mask bytes")?,
    );
    let zero = f32_to_f16(0.0);
    let neg_inf = f32_to_f16(f32::NEG_INFINITY);
    for &position in positions {
        let position = usize::try_from(position)?;
        if position >= key_count {
            return Err(format!(
                "attention position {} exceeds key_count {}",
                position, key_count
            )
            .into());
        }
        for key in 0..key_count {
            let value = if key > position { neg_inf } else { zero };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(bytes)
}

fn position_causal_mask_f32_bytes(
    key_count: usize,
    positions: &[i32],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::with_capacity(
        positions
            .len()
            .checked_mul(key_count)
            .and_then(|v| v.checked_mul(std::mem::size_of::<f32>()))
            .ok_or("overflow computing attention decode f32 mask bytes")?,
    );
    for &position in positions {
        let position = usize::try_from(position)?;
        if position >= key_count {
            return Err(format!(
                "attention position {} exceeds key_count {}",
                position, key_count
            )
            .into());
        }
        for key in 0..key_count {
            let value = if key > position {
                f32::NEG_INFINITY
            } else {
                0.0
            };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(bytes)
}

fn position_attention_mask_bytes_for_tensor(
    ctx: &Context,
    tensor_id: TensorId,
    key_count: usize,
    positions: &[i32],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid attention mask tensor {tensor_id}"))?;
    match tensor.desc.ty {
        TensorType::F16 => position_causal_mask_f16_bytes(key_count, positions),
        TensorType::F32 => position_causal_mask_f32_bytes(key_count, positions),
        other => Err(format!(
            "unsupported attention decode mask tensor type {}",
            other.name()
        )
        .into()),
    }
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

fn compare_should_use_flash_attention(head_dim: usize, n_tokens: usize) -> bool {
    matches!(
        head_dim,
        32 | 40 | 48 | 64 | 72 | 80 | 96 | 112 | 128 | 192 | 256 | 576
    ) && n_tokens < 20
}

fn compare_cast_tensor_to_type(
    ctx: &mut Context,
    src: TensorId,
    ty: TensorType,
) -> Result<TensorId, Box<dyn std::error::Error>> {
    let src_tensor = ctx.tensor(src).ok_or("invalid cast source tensor")?.clone();
    if src_tensor.desc.ty == ty {
        return Ok(src);
    }
    let dst = ctx.new_tensor(
        ty,
        src_tensor.desc.layout.rank(),
        src_tensor.desc.layout.extents(),
        BufferUsage::Activations,
    )?;
    Ok(ctx.cpy(src, dst, BufferUsage::Activations)?)
}

#[allow(clippy::too_many_arguments)]
fn run_isolated_attention_last_token(
    q_store: &[f32],
    k_store: &[f32],
    v_store: &[f32],
    q_head_dim: usize,
    k_head_dim: usize,
    v_head_dim: usize,
    q_head_count: usize,
    kv_head_count: usize,
    n_tokens: usize,
    causal: bool,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if !compare_should_use_flash_attention(q_head_dim, n_tokens) {
        return Err("isolated attention probe currently only supports the flash path".into());
    }

    let q_width = q_head_dim
        .checked_mul(q_head_count)
        .ok_or("overflow computing isolated q width")?;
    let k_width = k_head_dim
        .checked_mul(kv_head_count)
        .ok_or("overflow computing isolated k width")?;
    let v_width = v_head_dim
        .checked_mul(kv_head_count)
        .ok_or("overflow computing isolated v width")?;
    if q_store.len() != q_width * n_tokens {
        return Err(format!(
            "isolated q length mismatch: got {}, expected {}",
            q_store.len(),
            q_width * n_tokens
        )
        .into());
    }
    if k_store.len() != k_width * n_tokens {
        return Err(format!(
            "isolated k length mismatch: got {}, expected {}",
            k_store.len(),
            k_width * n_tokens
        )
        .into());
    }
    if v_store.len() != v_width * n_tokens {
        return Err(format!(
            "isolated v length mismatch: got {}, expected {}",
            v_store.len(),
            v_width * n_tokens
        )
        .into());
    }

    let mut ctx = Context::new(InitParams {
        mem_size: 8 << 20,
        mem_buffer: None,
        no_alloc: false,
    });
    let q_base = ctx.new_tensor_2d(
        TensorType::F32,
        i64::try_from(q_width)?,
        i64::try_from(n_tokens)?,
        BufferUsage::Activations,
    )?;
    let k_base = ctx.new_tensor_2d(
        TensorType::F32,
        i64::try_from(k_width)?,
        i64::try_from(n_tokens)?,
        BufferUsage::Activations,
    )?;
    let v_base = ctx.new_tensor_2d(
        TensorType::F32,
        i64::try_from(v_width)?,
        i64::try_from(n_tokens)?,
        BufferUsage::Activations,
    )?;
    ctx.write_tensor_data(q_base, &f32s_to_bytes(q_store))?;
    ctx.write_tensor_data(k_base, &f32s_to_bytes(k_store))?;
    ctx.write_tensor_data(v_base, &f32s_to_bytes(v_store))?;

    let q_states = ctx.reshape(
        q_base,
        &[
            i64::try_from(q_head_dim)?,
            i64::try_from(q_head_count)?,
            i64::try_from(n_tokens)?,
            1,
        ],
    )?;
    let k_states = ctx.reshape(
        k_base,
        &[
            i64::try_from(k_head_dim)?,
            i64::try_from(kv_head_count)?,
            i64::try_from(n_tokens)?,
            1,
        ],
    )?;
    let v_states = ctx.reshape(
        v_base,
        &[
            i64::try_from(v_head_dim)?,
            i64::try_from(kv_head_count)?,
            i64::try_from(n_tokens)?,
            1,
        ],
    )?;

    let input_mask = if causal {
        let mask = ctx.new_tensor_4d(
            TensorType::F16,
            i64::try_from(n_tokens)?,
            i64::try_from(n_tokens)?,
            1,
            1,
            BufferUsage::Activations,
        )?;
        ctx.write_tensor_data(mask, &causal_mask_f16_bytes(n_tokens))?;
        Some(mask)
    } else {
        None
    };

    let q_tensor = ctx
        .tensor(q_states)
        .ok_or("invalid isolated q tensor")?
        .clone();
    let q = ctx.view_4d(
        q_states,
        q_tensor.ne[0],
        q_tensor.ne[1],
        q_tensor.ne[2],
        q_tensor.ne[3],
        q_tensor.nb[1],
        q_tensor.nb[2],
        q_tensor.nb[3],
        0,
    )?;
    let q = ctx.permute(q, [0, 2, 1, 3])?;
    let mut k = ctx.permute(k_states, [0, 2, 1, 3])?;
    let mut v = ctx.permute(v_states, [0, 2, 1, 3])?;
    let v_trans = {
        let v_tensor = ctx.tensor(v_states).ok_or("invalid isolated v tensor")?;
        v_tensor.nb[1] > v_tensor.nb[2]
    };
    if v_trans {
        v = ctx.transpose(v)?;
    }
    k = compare_cast_tensor_to_type(&mut ctx, k, TensorType::F16)?;
    v = compare_cast_tensor_to_type(&mut ctx, v, TensorType::F16)?;

    let attn = ctx.flash_attn_ext(
        q,
        k,
        v,
        input_mask,
        1.0f32 / (q_head_dim as f32).sqrt(),
        0.0,
        0.0,
        BufferUsage::Activations,
    )?;
    ctx.flash_attn_ext_set_prec(attn, Prec::F32)?;
    let attn_tensor = ctx
        .tensor(attn)
        .ok_or("invalid isolated attn tensor")?
        .clone();
    let attn = ctx.reshape(
        attn,
        &[
            attn_tensor.ne[0] * attn_tensor.ne[1],
            attn_tensor.ne[2] * attn_tensor.ne[3],
        ],
    )?;

    let mut graph = Graph::new();
    graph.build_forward_expand(&ctx, attn)?;
    let runtime = MetalRuntime::new()?;
    let prepared = prepare_graph(&ctx, &graph, runtime.features())?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let execution = session.execute(&ctx, &[], &[attn])?;
    let output = bytes_to_f32s(
        execution
            .outputs
            .get(&attn)
            .ok_or("missing isolated attn output")?,
    );
    let row_width = output.len() / n_tokens;
    Ok(output[output.len() - row_width..].to_vec())
}

fn attention_cache_tensor_check(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<AttentionCacheTensorCheck, Box<dyn std::error::Error>> {
    let (layer_index, block_spec, layout, decode_spec) =
        first_attention_check_setup(model, token_ids.len(), TensorType::F32)?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let full_rope_positions = block_spec
        .rope
        .as_ref()
        .map(|rope| encode_rope_positions(rope, &positions, token_ids.len()))
        .transpose()?;

    let mut full_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
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
    let full_output_proj_id = add_hidden_token_checkpoint_by_name(
        &mut full_loaded.ctx,
        "attn.output_proj",
        "attn.output_proj_ck",
    )?;
    let full_result_output_id = add_hidden_token_checkpoint_by_any_name(
        &mut full_loaded.ctx,
        &["attn.output_residual", "attn.output_proj"],
        "attn.result_output_ck",
    )?;
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
    full_block
        .graph
        .build_forward_expand(&full_loaded.ctx, full_output_proj_id)?;
    full_block
        .graph
        .build_forward_expand(&full_loaded.ctx, full_result_output_id)?;
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
        full_output_proj_id,
        full_result_output_id,
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
    let full_output_proj = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_output_proj_id)
            .ok_or("missing full output_proj output")?,
    );
    let full_result_output = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_result_output_id)
            .ok_or("missing full result_output output")?,
    );

    let q_row_width = full_q_store.len() / token_ids.len();
    let k_row_width = full_k_store.len() / token_ids.len();
    let v_row_width = full_v_store.len() / token_ids.len();
    let attn_row_width = full_attn.len() / token_ids.len();

    let mut decode_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let decode_runtime = MetalRuntime::new()?;
    let decode_features = decode_runtime.features();
    let decode_key_count = usize::try_from(positions[1])?
        .checked_add(1)
        .ok_or("overflow computing decode key_count")?;
    let (mut decode_graph, _) = prepare_attention_decode_graph_with_key_count(
        &mut decode_loaded.ctx,
        &decode_loaded.tensor_ids,
        &decode_spec,
        1,
        decode_key_count,
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
    let decode_output_proj_id = add_hidden_token_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.output_proj",
        "attn_decode.output_proj_ck",
    )?;
    let decode_result_output_id = add_hidden_token_checkpoint_by_any_name(
        &mut decode_loaded.ctx,
        &["attn_decode.output_residual", "attn_decode.output_proj"],
        "attn_decode.result_output_ck",
    )?;
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
    decode_graph
        .graph
        .build_forward_expand(&decode_loaded.ctx, decode_output_proj_id)?;
    decode_graph
        .graph
        .build_forward_expand(&decode_loaded.ctx, decode_result_output_id)?;
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
    let decode_mask_bytes = decode_graph
        .input_mask
        .map(|input_mask| {
            position_attention_mask_bytes_for_tensor(
                &decode_loaded.ctx,
                input_mask,
                decode_key_count,
                &[positions[1]],
            )
        })
        .transpose()?;
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
        decode_output_proj_id,
        decode_result_output_id,
    ];
    let decode_writes = [
        MetalGraphTensorWrite {
            tensor_id: decode_graph.input_primary,
            bytes: &decode_token_bytes,
        },
        MetalGraphTensorWrite {
            tensor_id: decode_graph.input_write_indices,
            bytes: &decode_pos_bytes,
        },
    ];
    let mut decode_writes = decode_writes.to_vec();
    if let Some(input_mask) = decode_graph.input_mask {
        decode_writes.push(MetalGraphTensorWrite {
            tensor_id: input_mask,
            bytes: decode_mask_bytes
                .as_deref()
                .ok_or("missing decode attention mask")?,
        });
    }
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
    let decode_output_proj = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_output_proj_id)
            .ok_or("missing decode output_proj output")?,
    );
    let decode_result_output = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_result_output_id)
            .ok_or("missing decode result_output output")?,
    );
    let isolated_attn = run_isolated_attention_last_token(
        &full_q_store,
        &full_k_store,
        &full_v_store,
        usize::try_from(block_spec.q_head_dim)?,
        usize::try_from(block_spec.k_head_dim)?,
        usize::try_from(block_spec.v_head_dim)?,
        usize::try_from(block_spec.q_head_count)?,
        usize::try_from(block_spec.kv_head_count)?,
        token_ids.len(),
        block_spec.causal,
    )?;
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
        isolated_attn_stats: compare_logits(&isolated_attn, &full_attn[attn_row_width..]),
        output_proj_stats: compare_logits(&decode_output_proj, &full_output_proj),
        result_output_stats: compare_logits(&decode_result_output, &full_result_output),
        full_attn_cpu_stats: compare_logits(&full_attn[attn_row_width..], &attn_cpu),
        isolated_attn_cpu_stats: compare_logits(&isolated_attn, &attn_cpu),
        decode_attn_cpu_stats: compare_logits(&decode_attn, &attn_cpu),
    })
}

fn attention_decode_stepwise_tensor_check(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<AttentionDecodeStepwiseTensorCheck, Box<dyn std::error::Error>> {
    let (layer_index, block_spec, layout, decode_spec) =
        first_attention_check_setup(model, token_ids.len(), TensorType::F32)?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let rope_positions = block_spec
        .rope
        .as_ref()
        .map(|rope| encode_rope_positions(rope, &positions, token_ids.len()))
        .transpose()?;

    let mut full_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let full_runtime = MetalRuntime::new()?;
    let full_features = full_runtime.features();
    let (mut full_block, _) = prepare_attention_block_graph(
        &mut full_loaded.ctx,
        &full_loaded.tensor_ids,
        &block_spec,
        token_ids.len(),
        full_features,
    )?;
    let full_q_proj_id =
        add_contiguous_checkpoint_by_name(&mut full_loaded.ctx, "attn.q_proj", "attn.q_proj_ck")?;
    let full_q_pre_id = add_contiguous_checkpoint_by_name(
        &mut full_loaded.ctx,
        "attn.q_pre_store",
        "attn.q_pre_ck",
    )?;
    let full_q_norm_id = add_contiguous_checkpoint_by_name(
        &mut full_loaded.ctx,
        "attn.q_norm_store",
        "attn.q_norm_ck",
    )?;
    let full_k_norm_id = add_contiguous_checkpoint_by_name(
        &mut full_loaded.ctx,
        "attn.k_norm_store",
        "attn.k_norm_ck",
    )?;
    let full_q_id =
        add_contiguous_checkpoint_by_name(&mut full_loaded.ctx, "attn.q_store", "attn.q_ck")?;
    let full_k_id =
        add_contiguous_checkpoint_by_name(&mut full_loaded.ctx, "attn.k_store", "attn.k_ck")?;
    let full_v_id =
        add_contiguous_checkpoint_by_name(&mut full_loaded.ctx, "attn.v_store", "attn.v_ck")?;
    let full_attn_id =
        add_contiguous_checkpoint_by_name(&mut full_loaded.ctx, "attn.attn_flat", "attn.attn_ck")?;
    let full_output_proj_id = add_contiguous_checkpoint_by_name(
        &mut full_loaded.ctx,
        "attn.output_proj",
        "attn.output_proj_ck",
    )?;
    let full_result_output_id = add_contiguous_checkpoint_by_any_name(
        &mut full_loaded.ctx,
        &["attn.output_residual", "attn.output_proj"],
        "attn.result_output_ck",
    )?;
    for tensor_id in [
        full_q_proj_id,
        full_q_pre_id,
        full_q_norm_id,
        full_k_norm_id,
        full_q_id,
        full_k_id,
        full_v_id,
        full_attn_id,
        full_output_proj_id,
        full_result_output_id,
    ] {
        full_block
            .graph
            .build_forward_expand(&full_loaded.ctx, tensor_id)?;
    }
    let full_prepared = prepare_graph(&full_loaded.ctx, &full_block.graph, full_features)?;
    let full_session = MetalGraphSession::from_runtime(
        full_runtime,
        &full_loaded.ctx,
        &full_prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let full_token_bytes = i32s_to_bytes(token_ids);
    let full_pos_bytes = rope_positions.as_deref().map(i32s_to_bytes);
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
            bytes: full_pos_bytes
                .as_deref()
                .ok_or("missing full attention rope positions")?,
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
    let full_execution = full_session.execute(
        &full_loaded.ctx,
        &full_writes,
        &[
            full_q_proj_id,
            full_q_pre_id,
            full_q_norm_id,
            full_k_norm_id,
            full_q_id,
            full_k_id,
            full_v_id,
            full_attn_id,
            full_output_proj_id,
            full_result_output_id,
        ],
    )?;
    let full_q_proj = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_q_proj_id)
            .ok_or("missing full q_proj output")?,
    );
    let full_q_pre = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_q_pre_id)
            .ok_or("missing full q_pre output")?,
    );
    let full_q_norm = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_q_norm_id)
            .ok_or("missing full q_norm output")?,
    );
    let full_k_norm = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_k_norm_id)
            .ok_or("missing full k_norm output")?,
    );
    let full_q = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_q_id)
            .ok_or("missing full q output")?,
    );
    let full_k = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_k_id)
            .ok_or("missing full k output")?,
    );
    let full_v = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_v_id)
            .ok_or("missing full v output")?,
    );
    let full_attn = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_attn_id)
            .ok_or("missing full attn output")?,
    );
    let full_output_proj = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_output_proj_id)
            .ok_or("missing full output_proj output")?,
    );
    let full_result_output = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_result_output_id)
            .ok_or("missing full result_output output")?,
    );

    let q_proj_row_width = full_q_proj.len() / token_ids.len();
    let q_pre_row_width = full_q_pre.len() / token_ids.len();
    let q_norm_row_width = full_q_norm.len() / token_ids.len();
    let k_norm_row_width = full_k_norm.len() / token_ids.len();
    let q_row_width = full_q.len() / token_ids.len();
    let k_row_width = full_k.len() / token_ids.len();
    let v_row_width = full_v.len() / token_ids.len();
    let attn_row_width = full_attn.len() / token_ids.len();
    let output_proj_row_width = full_output_proj.len() / token_ids.len();
    let result_output_row_width = full_result_output.len() / token_ids.len();
    let stable_key_count = usize::try_from(decode_spec.cache.max_context)?;

    let step0_run = run_attention_decode_with_result_checkpoint(
        model,
        &layout,
        &decode_spec,
        &positions[..1],
        stable_key_count,
        AttentionDecodeSequenceInput::TokenIds(&token_ids[..1]),
        None,
        None,
    )?;

    let mut decode_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let decode_runtime = MetalRuntime::new()?;
    let decode_features = decode_runtime.features();
    let decode_key_count = stable_key_count;
    let (mut decode_graph, _) = prepare_attention_decode_graph_with_key_count(
        &mut decode_loaded.ctx,
        &decode_loaded.tensor_ids,
        &decode_spec,
        1,
        decode_key_count,
        decode_features,
    )?;
    decode_loaded
        .ctx
        .write_tensor_data(decode_graph.k_cache, &step0_run.k_cache_bytes)?;
    decode_loaded
        .ctx
        .write_tensor_data(decode_graph.v_cache, &step0_run.v_cache_bytes)?;

    let decode_rope_positions = decode_spec
        .block
        .rope
        .as_ref()
        .map(|rope| encode_rope_positions(rope, &[positions[1]], 1))
        .transpose()?;
    let decode_q_proj_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.q_proj",
        "attn_decode.stepwise.q_proj_ck",
    )?;
    let decode_q_pre_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.q_pre_store",
        "attn_decode.stepwise.q_pre_ck",
    )?;
    let decode_q_norm_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.q_norm_store",
        "attn_decode.stepwise.q_norm_ck",
    )?;
    let decode_k_norm_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.k_norm_store",
        "attn_decode.stepwise.k_norm_ck",
    )?;
    let decode_q_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.q_store",
        "attn_decode.stepwise.q_ck",
    )?;
    let decode_k_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.k_store",
        "attn_decode.stepwise.k_ck",
    )?;
    let decode_v_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.v_store",
        "attn_decode.stepwise.v_ck",
    )?;
    let decode_attn_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.attn_flat",
        "attn_decode.stepwise.attn_ck",
    )?;
    let decode_output_proj_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.output_proj",
        "attn_decode.stepwise.output_proj_ck",
    )?;
    let decode_result_output_id = add_contiguous_checkpoint_by_any_name(
        &mut decode_loaded.ctx,
        &["attn_decode.output_residual", "attn_decode.output_proj"],
        "attn_decode.stepwise.result_output_ck",
    )?;
    for tensor_id in [
        decode_q_proj_id,
        decode_q_pre_id,
        decode_q_norm_id,
        decode_k_norm_id,
        decode_q_id,
        decode_k_id,
        decode_v_id,
        decode_graph.k_cache,
        decode_graph.v_cache,
        decode_attn_id,
        decode_output_proj_id,
        decode_result_output_id,
    ] {
        decode_graph
            .graph
            .build_forward_expand(&decode_loaded.ctx, tensor_id)?;
    }
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
    let decode_rope_bytes = decode_rope_positions.as_deref().map(i32s_to_bytes);
    let decode_mask_bytes = decode_graph
        .input_mask
        .map(|input_mask| {
            position_attention_mask_bytes_for_tensor(
                &decode_loaded.ctx,
                input_mask,
                decode_key_count,
                &[positions[1]],
            )
        })
        .transpose()?;
    let mut decode_writes = vec![
        MetalGraphTensorWrite {
            tensor_id: decode_graph.input_primary,
            bytes: &decode_token_bytes,
        },
        MetalGraphTensorWrite {
            tensor_id: decode_graph.input_write_indices,
            bytes: &decode_pos_bytes,
        },
    ];
    if let Some(input_mask) = decode_graph.input_mask {
        decode_writes.push(MetalGraphTensorWrite {
            tensor_id: input_mask,
            bytes: decode_mask_bytes
                .as_deref()
                .ok_or("missing stepwise decode attention mask")?,
        });
    }
    if let Some(input_rope_positions) = decode_graph.input_rope_positions {
        decode_writes.push(MetalGraphTensorWrite {
            tensor_id: input_rope_positions,
            bytes: decode_rope_bytes
                .as_deref()
                .ok_or("missing stepwise decode rope positions")?,
        });
    }
    let decode_execution = decode_session.execute(
        &decode_loaded.ctx,
        &decode_writes,
        &[
            decode_q_proj_id,
            decode_q_pre_id,
            decode_q_norm_id,
            decode_k_norm_id,
            decode_q_id,
            decode_k_id,
            decode_v_id,
            decode_graph.k_cache,
            decode_graph.v_cache,
            decode_attn_id,
            decode_output_proj_id,
            decode_result_output_id,
        ],
    )?;
    let decode_q_proj = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_q_proj_id)
            .ok_or("missing stepwise decode q_proj output")?,
    );
    let decode_q_pre = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_q_pre_id)
            .ok_or("missing stepwise decode q_pre output")?,
    );
    let decode_q_norm = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_q_norm_id)
            .ok_or("missing stepwise decode q_norm output")?,
    );
    let decode_k_norm = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_k_norm_id)
            .ok_or("missing stepwise decode k_norm output")?,
    );
    let decode_q = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_q_id)
            .ok_or("missing stepwise decode q output")?,
    );
    let decode_k = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_k_id)
            .ok_or("missing stepwise decode k output")?,
    );
    let decode_v = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_v_id)
            .ok_or("missing stepwise decode v output")?,
    );
    let decode_k_cache = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_graph.k_cache)
            .ok_or("missing stepwise decode k_cache output")?,
    );
    let decode_v_cache = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_graph.v_cache)
            .ok_or("missing stepwise decode v_cache output")?,
    );
    let decode_attn = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_attn_id)
            .ok_or("missing stepwise decode attn output")?,
    );
    let decode_output_proj = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_output_proj_id)
            .ok_or("missing stepwise decode output_proj output")?,
    );
    let decode_result_output = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_result_output_id)
            .ok_or("missing stepwise decode result_output output")?,
    );

    Ok(AttentionDecodeStepwiseTensorCheck {
        layer_index,
        q_proj_stats: compare_logits(&decode_q_proj, &full_q_proj[q_proj_row_width..]),
        q_pre_stats: compare_logits(&decode_q_pre, &full_q_pre[q_pre_row_width..]),
        q_norm_stats: compare_logits(&decode_q_norm, &full_q_norm[q_norm_row_width..]),
        k_norm_stats: compare_logits(&decode_k_norm, &full_k_norm[k_norm_row_width..]),
        q_stats: compare_logits(&decode_q, &full_q[q_row_width..]),
        k_store_stats: compare_logits(&decode_k, &full_k[k_row_width..]),
        v_store_stats: compare_logits(&decode_v, &full_v[v_row_width..]),
        k_cache_stats: compare_logits(&decode_k_cache[..full_k.len()], &full_k),
        v_cache_stats: compare_logits(&decode_v_cache[..full_v.len()], &full_v),
        attn_stats: compare_logits(&decode_attn, &full_attn[attn_row_width..]),
        output_proj_stats: compare_logits(
            &decode_output_proj,
            &full_output_proj[output_proj_row_width..],
        ),
        result_output_stats: compare_logits(
            &decode_result_output,
            &full_result_output[result_output_row_width..],
        ),
    })
}

fn attention_decode_batched_tensor_check(
    model: &LlamaModel,
    token_ids: &[i32],
) -> Result<AttentionDecodeBatchedTensorCheck, Box<dyn std::error::Error>> {
    let (layer_index, block_spec, layout, decode_spec) =
        first_attention_check_setup(model, token_ids.len(), TensorType::F32)?;
    let positions = (0..token_ids.len())
        .map(i32::try_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let rope_positions = block_spec
        .rope
        .as_ref()
        .map(|rope| encode_rope_positions(rope, &positions, token_ids.len()))
        .transpose()?;

    let mut full_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let full_runtime = MetalRuntime::new()?;
    let full_features = full_runtime.features();
    let (mut full_block, _) = prepare_attention_block_graph(
        &mut full_loaded.ctx,
        &full_loaded.tensor_ids,
        &block_spec,
        token_ids.len(),
        full_features,
    )?;
    let full_q_proj_id =
        add_contiguous_checkpoint_by_name(&mut full_loaded.ctx, "attn.q_proj", "attn.q_proj_ck")?;
    let full_q_pre_id = add_contiguous_checkpoint_by_name(
        &mut full_loaded.ctx,
        "attn.q_pre_store",
        "attn.q_pre_ck",
    )?;
    let full_q_norm_id = add_contiguous_checkpoint_by_name(
        &mut full_loaded.ctx,
        "attn.q_norm_store",
        "attn.q_norm_ck",
    )?;
    let full_k_norm_id = add_contiguous_checkpoint_by_name(
        &mut full_loaded.ctx,
        "attn.k_norm_store",
        "attn.k_norm_ck",
    )?;
    let full_q_id =
        add_contiguous_checkpoint_by_name(&mut full_loaded.ctx, "attn.q_store", "attn.q_ck")?;
    let full_k_id =
        add_contiguous_checkpoint_by_name(&mut full_loaded.ctx, "attn.k_store", "attn.k_ck")?;
    let full_v_id =
        add_contiguous_checkpoint_by_name(&mut full_loaded.ctx, "attn.v_store", "attn.v_ck")?;
    let full_attn_id =
        add_contiguous_checkpoint_by_name(&mut full_loaded.ctx, "attn.attn_flat", "attn.attn_ck")?;
    let full_output_proj_id = add_contiguous_checkpoint_by_name(
        &mut full_loaded.ctx,
        "attn.output_proj",
        "attn.output_proj_ck",
    )?;
    let full_result_output_id = add_contiguous_checkpoint_by_any_name(
        &mut full_loaded.ctx,
        &["attn.output_residual", "attn.output_proj"],
        "attn.result_output_ck",
    )?;
    for tensor_id in [
        full_q_proj_id,
        full_q_pre_id,
        full_q_norm_id,
        full_k_norm_id,
        full_q_id,
        full_k_id,
        full_v_id,
        full_attn_id,
        full_output_proj_id,
        full_result_output_id,
    ] {
        full_block
            .graph
            .build_forward_expand(&full_loaded.ctx, tensor_id)?;
    }
    let full_prepared = prepare_graph(&full_loaded.ctx, &full_block.graph, full_features)?;
    let full_session = MetalGraphSession::from_runtime(
        full_runtime,
        &full_loaded.ctx,
        &full_prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let full_token_bytes = i32s_to_bytes(token_ids);
    let full_pos_bytes = rope_positions.as_deref().map(i32s_to_bytes);
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
            bytes: full_pos_bytes
                .as_deref()
                .ok_or("missing full attention rope positions")?,
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
    let full_execution = full_session.execute(
        &full_loaded.ctx,
        &full_writes,
        &[
            full_q_proj_id,
            full_q_pre_id,
            full_q_norm_id,
            full_k_norm_id,
            full_q_id,
            full_k_id,
            full_v_id,
            full_attn_id,
            full_output_proj_id,
            full_result_output_id,
        ],
    )?;
    let full_q_proj = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_q_proj_id)
            .ok_or("missing full q_proj output")?,
    );
    let full_q_pre = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_q_pre_id)
            .ok_or("missing full q_pre output")?,
    );
    let full_q_norm = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_q_norm_id)
            .ok_or("missing full q_norm output")?,
    );
    let full_k_norm = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_k_norm_id)
            .ok_or("missing full k_norm output")?,
    );
    let full_q = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_q_id)
            .ok_or("missing full q output")?,
    );
    let full_k = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_k_id)
            .ok_or("missing full k output")?,
    );
    let full_v = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_v_id)
            .ok_or("missing full v output")?,
    );
    let full_attn = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_attn_id)
            .ok_or("missing full attn output")?,
    );
    let full_output_proj = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_output_proj_id)
            .ok_or("missing full output_proj output")?,
    );
    let full_result_output = bytes_to_f32s(
        full_execution
            .outputs
            .get(&full_result_output_id)
            .ok_or("missing full result_output output")?,
    );

    let mut decode_loaded =
        layout.allocate_and_load_with_extra(&model.gguf, COMPARE_EXTRA_CONTEXT_BYTES)?;
    let decode_runtime = MetalRuntime::new()?;
    let decode_features = decode_runtime.features();
    let (mut decode_graph, _) = prepare_attention_decode_graph_with_key_count(
        &mut decode_loaded.ctx,
        &decode_loaded.tensor_ids,
        &decode_spec,
        token_ids.len(),
        token_ids.len(),
        decode_features,
    )?;
    let decode_q_proj_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.q_proj",
        "attn_decode.q_proj_ck",
    )?;
    let decode_q_pre_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.q_pre_store",
        "attn_decode.q_pre_ck",
    )?;
    let decode_q_norm_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.q_norm_store",
        "attn_decode.q_norm_ck",
    )?;
    let decode_k_norm_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.k_norm_store",
        "attn_decode.k_norm_ck",
    )?;
    let decode_q_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.q_store",
        "attn_decode.q_ck",
    )?;
    let decode_k_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.k_store",
        "attn_decode.k_ck",
    )?;
    let decode_v_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.v_store",
        "attn_decode.v_ck",
    )?;
    let decode_k_cache_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.k_cache",
        "attn_decode.k_cache_ck",
    )?;
    let decode_v_cache_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.v_cache",
        "attn_decode.v_cache_ck",
    )?;
    let decode_k_cache_view_id = add_flattened_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.k_cache_view",
        "attn_decode.k_cache_view_ck",
    )?;
    let decode_v_cache_view_id = add_flattened_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.v_cache_view",
        "attn_decode.v_cache_view_ck",
    )?;
    let decode_attn_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.attn_flat",
        "attn_decode.attn_ck",
    )?;
    let decode_output_proj_id = add_contiguous_checkpoint_by_name(
        &mut decode_loaded.ctx,
        "attn_decode.output_proj",
        "attn_decode.output_proj_ck",
    )?;
    let decode_result_output_id = add_contiguous_checkpoint_by_any_name(
        &mut decode_loaded.ctx,
        &["attn_decode.output_residual", "attn_decode.output_proj"],
        "attn_decode.result_output_ck",
    )?;
    for tensor_id in [
        decode_q_proj_id,
        decode_q_pre_id,
        decode_q_norm_id,
        decode_k_norm_id,
        decode_q_id,
        decode_k_id,
        decode_v_id,
        decode_k_cache_id,
        decode_v_cache_id,
        decode_k_cache_view_id,
        decode_v_cache_view_id,
        decode_attn_id,
        decode_output_proj_id,
        decode_result_output_id,
    ] {
        decode_graph
            .graph
            .build_forward_expand(&decode_loaded.ctx, tensor_id)?;
    }
    let decode_prepared = prepare_graph(&decode_loaded.ctx, &decode_graph.graph, decode_features)?;
    let decode_session = MetalGraphSession::from_runtime(
        decode_runtime,
        &decode_loaded.ctx,
        &decode_prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let zero_k_cache = vec![0u8; decode_loaded.ctx.tensor_data(decode_graph.k_cache)?.len()];
    let zero_v_cache = vec![0u8; decode_loaded.ctx.tensor_data(decode_graph.v_cache)?.len()];
    decode_loaded
        .ctx
        .write_tensor_data(decode_graph.k_cache, &zero_k_cache)?;
    decode_loaded
        .ctx
        .write_tensor_data(decode_graph.v_cache, &zero_v_cache)?;
    let decode_token_bytes = i32s_to_bytes(token_ids);
    let decode_write_index_bytes = i32s_to_bytes(&positions);
    let decode_rope_bytes = rope_positions.as_deref().map(i32s_to_bytes);
    let decode_mask_bytes = decode_graph
        .input_mask
        .map(|input_mask| {
            position_attention_mask_bytes_for_tensor(
                &decode_loaded.ctx,
                input_mask,
                compare_attention_mask_write_key_count(
                    &decode_loaded.ctx,
                    input_mask,
                    i64::from(block_spec.q_head_dim),
                    token_ids.len(),
                    token_ids.len(),
                )?,
                &positions,
            )
        })
        .transpose()?;
    let mut decode_writes = vec![
        MetalGraphTensorWrite {
            tensor_id: decode_graph.input_primary,
            bytes: &decode_token_bytes,
        },
        MetalGraphTensorWrite {
            tensor_id: decode_graph.input_write_indices,
            bytes: &decode_write_index_bytes,
        },
    ];
    if let Some(input_rope_positions) = decode_graph.input_rope_positions {
        decode_writes.push(MetalGraphTensorWrite {
            tensor_id: input_rope_positions,
            bytes: decode_rope_bytes
                .as_deref()
                .ok_or("missing decode rope positions")?,
        });
    }
    if let Some(input_mask) = decode_graph.input_mask {
        decode_writes.push(MetalGraphTensorWrite {
            tensor_id: input_mask,
            bytes: decode_mask_bytes
                .as_deref()
                .ok_or("missing decode attention mask")?,
        });
    }
    let decode_execution = decode_session.execute(
        &decode_loaded.ctx,
        &decode_writes,
        &[
            decode_q_proj_id,
            decode_q_pre_id,
            decode_q_norm_id,
            decode_k_norm_id,
            decode_q_id,
            decode_k_id,
            decode_v_id,
            decode_k_cache_id,
            decode_v_cache_id,
            decode_k_cache_view_id,
            decode_v_cache_view_id,
            decode_attn_id,
            decode_output_proj_id,
            decode_result_output_id,
        ],
    )?;
    let decode_q_proj = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_q_proj_id)
            .ok_or("missing decode q_proj output")?,
    );
    let decode_q_pre = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_q_pre_id)
            .ok_or("missing decode q_pre output")?,
    );
    let decode_q_norm = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_q_norm_id)
            .ok_or("missing decode q_norm output")?,
    );
    let decode_k_norm = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_k_norm_id)
            .ok_or("missing decode k_norm output")?,
    );
    let decode_q = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_q_id)
            .ok_or("missing decode q output")?,
    );
    let decode_k = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_k_id)
            .ok_or("missing decode k output")?,
    );
    let decode_v = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_v_id)
            .ok_or("missing decode v output")?,
    );
    let decode_k_cache = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_k_cache_id)
            .ok_or("missing decode k_cache output")?,
    );
    let decode_v_cache = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_v_cache_id)
            .ok_or("missing decode v_cache output")?,
    );
    let decode_k_cache_view = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_k_cache_view_id)
            .ok_or("missing decode k_cache_view output")?,
    );
    let decode_v_cache_view = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_v_cache_view_id)
            .ok_or("missing decode v_cache_view output")?,
    );
    let decode_attn = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_attn_id)
            .ok_or("missing decode attn output")?,
    );
    let decode_output_proj = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_output_proj_id)
            .ok_or("missing decode output_proj output")?,
    );
    let decode_result_output = bytes_to_f32s(
        decode_execution
            .outputs
            .get(&decode_result_output_id)
            .ok_or("missing decode result_output output")?,
    );

    Ok(AttentionDecodeBatchedTensorCheck {
        layer_index,
        q_proj_stats: compare_logits(&decode_q_proj, &full_q_proj),
        q_pre_stats: compare_logits(&decode_q_pre, &full_q_pre),
        q_norm_stats: compare_logits(&decode_q_norm, &full_q_norm),
        k_norm_stats: compare_logits(&decode_k_norm, &full_k_norm),
        q_stats: compare_logits(&decode_q, &full_q),
        k_store_stats: compare_logits(&decode_k, &full_k),
        v_store_stats: compare_logits(&decode_v, &full_v),
        k_cache_stats: compare_logits(&decode_k_cache, &full_k),
        v_cache_stats: compare_logits(&decode_v_cache, &full_v),
        k_cache_view_stats: compare_logits(&decode_k_cache_view, &full_k),
        v_cache_view_stats: compare_logits(&decode_v_cache_view, &full_v),
        attn_stats: compare_logits(&decode_attn, &full_attn),
        output_proj_stats: compare_logits(&decode_output_proj, &full_output_proj),
        result_output_stats: compare_logits(&decode_result_output, &full_result_output),
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

fn token_slice<'a>(
    values: &'a [f32],
    hidden_size: usize,
    token_index: usize,
) -> Result<&'a [f32], Box<dyn std::error::Error>> {
    if hidden_size == 0 {
        return Err("hidden_size was zero".into());
    }
    let start = token_index
        .checked_mul(hidden_size)
        .ok_or("token slice start overflow")?;
    let end = start
        .checked_add(hidden_size)
        .ok_or("token slice end overflow")?;
    values
        .get(start..end)
        .ok_or_else(|| "token slice was out of range".into())
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

#[allow(dead_code)]
fn bytes_to_i32s(bytes: &[u8]) -> Vec<i32> {
    bytes
        .chunks_exact(std::mem::size_of::<i32>())
        .map(|chunk| i32::from_le_bytes(chunk.try_into().unwrap()))
        .collect()
}

fn cpu_rms_norm_mul_rows(
    src: &[f32],
    hidden_size: usize,
    n_rows: usize,
    weight: &[f32],
    eps: f32,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if weight.len() != hidden_size {
        return Err(format!(
            "rms_norm weight length mismatch: got {}, expected {}",
            weight.len(),
            hidden_size
        )
        .into());
    }
    let expected = hidden_size
        .checked_mul(n_rows)
        .ok_or("overflow computing rms_norm input length")?;
    if src.len() != expected {
        return Err(format!(
            "rms_norm input length mismatch: got {}, expected {}",
            src.len(),
            expected
        )
        .into());
    }

    let mut out = vec![0.0f32; src.len()];
    for row_index in 0..n_rows {
        let row = &src[row_index * hidden_size..(row_index + 1) * hidden_size];
        let mean_sq = row.iter().map(|value| value * value).sum::<f32>() / hidden_size as f32;
        let scale = 1.0f32 / (mean_sq + eps).sqrt();
        for hidden_index in 0..hidden_size {
            out[row_index * hidden_size + hidden_index] =
                row[hidden_index] * scale * weight[hidden_index];
        }
    }
    Ok(out)
}

fn cpu_add_rows(lhs: &[f32], rhs: &[f32]) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if lhs.len() != rhs.len() {
        return Err(format!(
            "row add length mismatch: lhs={} rhs={}",
            lhs.len(),
            rhs.len()
        )
        .into());
    }
    Ok(lhs
        .iter()
        .zip(rhs.iter())
        .map(|(lhs, rhs)| lhs + rhs)
        .collect())
}

fn cpu_scale_inplace(values: &mut [f32], scale: f32) {
    for value in values {
        *value *= scale;
    }
}

fn cpu_gelu_tanh(value: f32) -> f32 {
    const GELU_COEF_A: f32 = 0.044_715;
    const SQRT_2_OVER_PI: f32 = 0.797_884_6;
    0.5 * value * (1.0 + (SQRT_2_OVER_PI * value * (1.0 + GELU_COEF_A * value * value)).tanh())
}

fn cpu_gelu_rows(values: &[f32]) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    Ok(values.iter().copied().map(cpu_gelu_tanh).collect())
}

fn cpu_geglu_rows(gate: &[f32], up: &[f32]) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if gate.len() != up.len() {
        return Err(format!(
            "geglu input length mismatch: gate={} up={}",
            gate.len(),
            up.len()
        )
        .into());
    }
    Ok(gate
        .iter()
        .zip(up.iter())
        .map(|(gate, up)| cpu_gelu_tanh(*gate) * up)
        .collect())
}

fn cpu_mul_rows_broadcast(
    lhs: &[f32],
    row_width: usize,
    rhs: &[f32],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if row_width == 0 {
        return Err("row broadcast width was zero".into());
    }
    if lhs.is_empty() {
        return Ok(Vec::new());
    }
    if lhs.len() % row_width != 0 {
        return Err(format!(
            "lhs length {} is not divisible by row width {}",
            lhs.len(),
            row_width
        )
        .into());
    }
    match rhs.len() {
        1 => Ok(lhs.iter().map(|value| value * rhs[0]).collect()),
        len if len == row_width => Ok(lhs
            .chunks_exact(row_width)
            .flat_map(|row| row.iter().zip(rhs.iter()).map(|(lhs, rhs)| lhs * rhs))
            .collect()),
        len if len == lhs.len() => Ok(lhs
            .iter()
            .zip(rhs.iter())
            .map(|(lhs, rhs)| lhs * rhs)
            .collect()),
        len => Err(format!(
            "unsupported broadcast length {} for lhs len {} and row width {}",
            len,
            lhs.len(),
            row_width
        )
        .into()),
    }
}

fn cpu_mul_mat_rows(
    weight_rows: &[f32],
    n_cols: usize,
    n_rows: usize,
    src: &[f32],
    n_tokens: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let expected_weights = n_cols
        .checked_mul(n_rows)
        .ok_or("overflow computing mul_mat weight length")?;
    if weight_rows.len() != expected_weights {
        return Err(format!(
            "cpu mul_mat weight length mismatch: got {}, expected {}",
            weight_rows.len(),
            expected_weights
        )
        .into());
    }
    let expected_src = n_cols
        .checked_mul(n_tokens)
        .ok_or("overflow computing mul_mat input length")?;
    if src.len() != expected_src {
        return Err(format!(
            "cpu mul_mat input length mismatch: got {}, expected {}",
            src.len(),
            expected_src
        )
        .into());
    }

    let mut out = vec![
        0.0f32;
        n_rows
            .checked_mul(n_tokens)
            .ok_or("overflow computing mul_mat output length")?
    ];
    for token_index in 0..n_tokens {
        let src_row = &src[token_index * n_cols..(token_index + 1) * n_cols];
        for row_index in 0..n_rows {
            let weight_row = &weight_rows[row_index * n_cols..(row_index + 1) * n_cols];
            let mut sum = 0.0f32;
            for col_index in 0..n_cols {
                sum += weight_row[col_index] * src_row[col_index];
            }
            out[token_index * n_rows + row_index] = sum;
        }
    }
    Ok(out)
}

fn cpu_get_rows_loaded(
    loaded: &mut LoadedGgufWeights,
    tensor_name: &str,
    row_ids: &[i32],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let tensor_id = *loaded
        .tensor_ids
        .get(tensor_name)
        .ok_or_else(|| format!("missing tensor id for '{}'", tensor_name))?;
    let tensor = loaded
        .ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid tensor '{}'", tensor_name))?;
    let width = usize::try_from(tensor.ne[0])?;
    let height = usize::try_from(tensor.ne[1])?;
    let bytes = loaded.ctx.tensor_data(tensor_id)?;
    if let Some(rows) = get_rows_ggml_bytes_cpu(
        bytes,
        tensor.desc.ty.ggml_type(),
        width,
        height,
        row_ids,
    ) {
        return Ok(rows);
    }
    metal_get_rows_loaded(loaded, tensor_name, row_ids)
}

fn cpu_mul_mat_loaded(
    loaded: &mut LoadedGgufWeights,
    tensor_name: &str,
    src: &[f32],
    n_tokens: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    metal_mul_mat_loaded(loaded, tensor_name, src, n_tokens)
}

fn read_loaded_tensor_values_f32(
    loaded: &LoadedGgufWeights,
    tensor_name: &str,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let tensor_id = *loaded
        .tensor_ids
        .get(tensor_name)
        .ok_or_else(|| format!("missing tensor id for '{}'", tensor_name))?;
    let tensor = loaded
        .ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid tensor '{}'", tensor_name))?;
    tensor_values_from_tensor_bytes_f32(tensor, loaded.ctx.tensor_data(tensor_id)?)
}

fn cpu_extract_interleaved_layer_rows(
    values: &[f32],
    hidden_size: usize,
    layer_count: usize,
    layer_index: usize,
    n_tokens: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if hidden_size == 0 || layer_count == 0 {
        return Err("invalid gemma per-layer dimensions".into());
    }
    if layer_index >= layer_count {
        return Err(format!(
            "layer index {} exceeds layer count {}",
            layer_index, layer_count
        )
        .into());
    }
    let row_width = hidden_size
        .checked_mul(layer_count)
        .ok_or("overflow computing interleaved layer row width")?;
    let expected = row_width
        .checked_mul(n_tokens)
        .ok_or("overflow computing interleaved layer length")?;
    if values.len() != expected {
        return Err(format!(
            "interleaved layer input length mismatch: got {}, expected {}",
            values.len(),
            expected
        )
        .into());
    }
    let mut out = Vec::with_capacity(
        hidden_size
            .checked_mul(n_tokens)
            .ok_or("overflow computing extracted layer length")?,
    );
    for token_index in 0..n_tokens {
        let token_start = token_index
            .checked_mul(row_width)
            .ok_or("overflow computing interleaved token offset")?;
        let layer_start = token_start
            .checked_add(
                layer_index
                    .checked_mul(hidden_size)
                    .ok_or("overflow computing interleaved layer offset")?,
            )
            .ok_or("overflow computing interleaved layer start")?;
        out.extend_from_slice(&values[layer_start..layer_start + hidden_size]);
    }
    Ok(out)
}

fn metal_get_rows_loaded(
    loaded: &mut LoadedGgufWeights,
    tensor_name: &str,
    row_ids: &[i32],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let tensor_id = *loaded
        .tensor_ids
        .get(tensor_name)
        .ok_or_else(|| format!("missing tensor id for '{}'", tensor_name))?;
    let row_tensor = loaded.ctx.new_tensor_1d(
        TensorType::I32,
        i64::try_from(row_ids.len())?,
        BufferUsage::Activations,
    )?;
    loaded
        .ctx
        .tensor_mut(row_tensor)
        .ok_or("invalid get_rows row-id tensor")?
        .set_input();
    let selected = loaded
        .ctx
        .get_rows(tensor_id, row_tensor, BufferUsage::Activations)?;
    let selected = loaded.ctx.cont(selected)?;

    let mut graph = Graph::new();
    graph.build_forward_expand(&loaded.ctx, selected)?;
    let runtime = MetalRuntime::new()?;
    let prepared = prepare_graph(&loaded.ctx, &graph, runtime.features())?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &loaded.ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let row_bytes = i32s_to_bytes(row_ids);
    let execution = session.execute(
        &loaded.ctx,
        &[MetalGraphTensorWrite {
            tensor_id: row_tensor,
            bytes: &row_bytes,
        }],
        &[selected],
    )?;
    tensor_values_from_execution_f32(&loaded.ctx, &execution, selected)
}

fn metal_mul_mat_loaded(
    loaded: &mut LoadedGgufWeights,
    tensor_name: &str,
    src: &[f32],
    n_tokens: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let tensor_id = *loaded
        .tensor_ids
        .get(tensor_name)
        .ok_or_else(|| format!("missing tensor id for '{}'", tensor_name))?;
    let tensor = loaded
        .ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid tensor '{}'", tensor_name))?;
    let n_cols = usize::try_from(tensor.ne[0])?;
    let expected = n_cols
        .checked_mul(n_tokens)
        .ok_or("overflow computing mul_mat metal input length")?;
    if src.len() != expected {
        return Err(format!(
            "mul_mat input length mismatch for '{}': got {}, expected {}",
            tensor_name,
            src.len(),
            expected
        )
        .into());
    }

    let input = loaded.ctx.new_tensor_2d(
        TensorType::F32,
        i64::try_from(n_cols)?,
        i64::try_from(n_tokens)?,
        BufferUsage::Activations,
    )?;
    loaded
        .ctx
        .tensor_mut(input)
        .ok_or("invalid mul_mat input tensor")?
        .set_input();
    let output = loaded
        .ctx
        .mul_mat(tensor_id, input, BufferUsage::Activations)?;
    let output = loaded.ctx.cont(output)?;

    let mut graph = Graph::new();
    graph.build_forward_expand(&loaded.ctx, output)?;
    let runtime = MetalRuntime::new()?;
    let prepared = prepare_graph(&loaded.ctx, &graph, runtime.features())?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &loaded.ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let input_bytes = f32s_to_bytes(src);
    let execution = session.execute(
        &loaded.ctx,
        &[MetalGraphTensorWrite {
            tensor_id: input,
            bytes: &input_bytes,
        }],
        &[output],
    )?;
    tensor_values_from_execution_f32(&loaded.ctx, &execution, output)
}

fn cpu_mul_mat_tensor_rows(
    weight_tensor: &Tensor,
    weight_bytes: &[u8],
    src: &[f32],
    n_tokens: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    if weight_tensor.ne[2] != 1 || weight_tensor.ne[3] != 1 {
        return Err(format!(
            "cpu tensor mul_mat expects 2d weights, got ne={:?}",
            weight_tensor.ne
        )
        .into());
    }

    let n_cols = usize::try_from(weight_tensor.ne[0])?;
    let n_rows = usize::try_from(weight_tensor.ne[1])?;
    let expected_src = n_cols
        .checked_mul(n_tokens)
        .ok_or("overflow computing tensor mul_mat input length")?;
    if src.len() != expected_src {
        return Err(format!(
            "cpu tensor mul_mat input length mismatch: got {}, expected {}",
            src.len(),
            expected_src
        )
        .into());
    }

    let mut out = vec![
        0.0f32;
        n_rows
            .checked_mul(n_tokens)
            .ok_or("overflow computing tensor mul_mat output length")?
    ];
    for token_index in 0..n_tokens {
        let src_row = &src[token_index * n_cols..(token_index + 1) * n_cols];
        for row_index in 0..n_rows {
            let mut sum = 0.0f32;
            for col_index in 0..n_cols {
                sum += tensor_scalar_to_f32(
                    weight_tensor,
                    weight_bytes,
                    [col_index, row_index, 0, 0],
                )? * src_row[col_index];
            }
            out[token_index * n_rows + row_index] = sum;
        }
    }
    Ok(out)
}

fn execute_f32_mul_mat_in_fresh_context(
    weight_bytes: &[u8],
    n_cols: usize,
    n_rows: usize,
    src: &[f32],
    n_tokens: usize,
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let runtime = MetalRuntime::new()?;
    let mut ctx = Context::new(InitParams {
        mem_size: 8 << 20,
        mem_buffer: None,
        no_alloc: false,
    });
    let weights = ctx.new_tensor_2d(
        TensorType::F32,
        i64::try_from(n_cols)?,
        i64::try_from(n_rows)?,
        BufferUsage::Weights,
    )?;
    let input = ctx.new_tensor_2d(
        TensorType::F32,
        i64::try_from(n_cols)?,
        i64::try_from(n_tokens)?,
        BufferUsage::Activations,
    )?;
    let output = ctx.mul_mat(weights, input, BufferUsage::Activations)?;
    ctx.write_tensor_data(weights, weight_bytes)?;
    ctx.write_tensor_data(input, &f32s_to_bytes(src))?;

    let mut graph = Graph::new();
    graph.build_forward_expand(&ctx, output)?;
    let prepared = prepare_graph(&ctx, &graph, runtime.features())?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let execution = session.execute(&ctx, &[], &[output])?;
    Ok(bytes_to_f32s(
        execution
            .outputs
            .get(&output)
            .ok_or("fresh mul_mat execution missing output tensor")?,
    ))
}

fn preview_values_to_i32s(values: &[f32]) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
    values
        .iter()
        .map(|value| {
            if !value.is_finite() {
                return Err(format!("non-finite preview value {value}").into());
            }
            let rounded = value.round();
            if (value - rounded).abs() > 1e-4 {
                return Err(format!("preview value {value} is not close to an integer").into());
            }
            if rounded < i32::MIN as f32 || rounded > i32::MAX as f32 {
                return Err(format!("preview value {value} does not fit in i32").into());
            }
            Ok(rounded as i32)
        })
        .collect()
}

fn read_tensor_f32s(ctx: &Context, name: &str) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let tensor_id = ctx
        .get_tensor(name)
        .ok_or_else(|| format!("missing tensor '{name}'"))?;
    Ok(bytes_to_f32s(ctx.tensor_data(tensor_id)?))
}

fn cpu_top_k_rows_i32(values: &[f32], row_len: usize, k: usize) -> Vec<i32> {
    let mut out = Vec::with_capacity((values.len() / row_len) * k);
    for row in 0..(values.len() / row_len) {
        let mut indices = (0..row_len).collect::<Vec<_>>();
        indices.sort_by(|&a, &b| values[row * row_len + b].total_cmp(&values[row * row_len + a]));
        out.extend(indices.into_iter().take(k).map(|idx| idx as i32));
    }
    out
}

fn topk_set_diff_count(lhs: &[i32], rhs: &[i32], k: usize) -> usize {
    if k == 0 {
        return lhs.len().abs_diff(rhs.len());
    }
    let rows = usize::min(lhs.len() / k, rhs.len() / k);
    let mut diff = 0usize;
    for row in 0..rows {
        let mut lhs_row = lhs[row * k..(row + 1) * k].to_vec();
        let mut rhs_row = rhs[row * k..(row + 1) * k].to_vec();
        lhs_row.sort_unstable();
        rhs_row.sort_unstable();
        diff += lhs_row
            .iter()
            .zip(rhs_row.iter())
            .filter(|(a, b)| a != b)
            .count();
    }
    diff + lhs.len().abs_diff(rhs.len())
}

fn min_topk_margin(values: &[f32], row_len: usize, k: usize) -> f32 {
    if row_len == 0 || k == 0 || k >= row_len {
        return 0.0;
    }
    let mut min_margin = f32::INFINITY;
    for row in 0..(values.len() / row_len) {
        let mut row_values = values[row * row_len..(row + 1) * row_len].to_vec();
        row_values.sort_by(|a, b| b.total_cmp(a));
        let margin = row_values[k - 1] - row_values[k];
        min_margin = min_margin.min(margin);
    }
    if min_margin.is_finite() {
        min_margin
    } else {
        0.0
    }
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
