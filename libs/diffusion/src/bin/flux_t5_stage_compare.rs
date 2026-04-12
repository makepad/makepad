use makepad_diffusion::comfy::FluxWorkflow;
use makepad_diffusion::flux::{tokenize_flux_t5xxl_prompt, ComfyModelRoots, FluxPromptToImagePlan};
use makepad_diffusion::t5_encoder::LoadedT5xxlWeights;
use makepad_ggml::{bf16_to_f32, f16_to_f32, TensorType};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const HEAD_INDEX: usize = 0;
const HEAD_DIM_SLICE: usize = 8;
const HEAD_DIM_FULL: usize = 64;
const PAD_TOKEN_INDEX: usize = 2;

fn usage() -> ! {
    eprintln!("usage: flux-t5-stage-compare <workflow.json> <model-root> <ggml-debug-dir> [layer]");
    std::process::exit(1);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workflow_path = env::args().nth(1).unwrap_or_else(|| usage());
    let root = env::args().nth(2).unwrap_or_else(|| usage());
    let ggml_debug_dir = env::args().nth(3).map(PathBuf::from).unwrap_or_else(|| usage());
    let layer = env::args()
        .nth(4)
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(0);

    let workflow = FluxWorkflow::from_file(&workflow_path)?;
    let roots = ComfyModelRoots::new(root);
    let plan = FluxPromptToImagePlan::from_workflow(&workflow, &roots)?;
    let t5xxl_path = plan
        .bundle
        .t5xxl_path
        .as_ref()
        .ok_or("workflow bundle does not include t5xxl")?;
    let tokenized = tokenize_flux_t5xxl_prompt(&plan.prompts.t5xxl)?;
    let weights = LoadedT5xxlWeights::load(t5xxl_path)?;

    let token_count = tokenized.token_ids.len();
    let hidden_size = usize::try_from(weights.config.model_dim)?;
    let head_count = usize::try_from(weights.config.attention_head_count)?;
    let head_dim = usize::try_from(weights.config.head_dim())?;
    let compare_head_dims = HEAD_DIM_SLICE.min(head_dim);
    let compare_score_keys = 8usize.min(token_count);
    let block_prefix = format!("t5_block_{layer:02}");
    let attn_prefix = format!("encoder.block.{layer}.layer.0");
    let ff_prefix = format!("encoder.block.{layer}.layer.1");

    let block_input = if layer == 0 {
        embedding_all_tokens(&weights, &tokenized.token_ids)?
    } else {
        read_f32_file(&ggml_debug_dir.join(format!("t5_block_{:02}.bin", layer - 1)))?
    };
    let norm1_weight = vector_tensor(&weights, &format!("{attn_prefix}.layer_norm.weight"))?;
    let norm1 = rms_norm_all_tokens(
        &block_input,
        &norm1_weight,
        hidden_size,
        token_count,
        weights.config.layer_norm_epsilon(),
    );

    let q_head = linear_selected_outputs(
        &weights,
        &format!("{attn_prefix}.SelfAttention.q.weight"),
        &norm1,
        hidden_size,
        token_count,
        &(0..HEAD_DIM_FULL.min(head_dim)).collect::<Vec<_>>(),
        None,
    )?;
    let k_head = linear_selected_outputs(
        &weights,
        &format!("{attn_prefix}.SelfAttention.k.weight"),
        &norm1,
        hidden_size,
        token_count,
        &(0..HEAD_DIM_FULL.min(head_dim)).collect::<Vec<_>>(),
        None,
    )?;
    let v_head = linear_selected_outputs(
        &weights,
        &format!("{attn_prefix}.SelfAttention.v.weight"),
        &norm1,
        hidden_size,
        token_count,
        &(0..compare_head_dims).collect::<Vec<_>>(),
        None,
    )?;
    let scores = attention_scores_for_query(
        &q_head,
        &k_head,
        weights.relative_attention_bias(),
        token_count,
        HEAD_DIM_FULL.min(head_dim),
        head_count,
        weights.config.relative_attention_bucket_count,
        weights.config.relative_attention_max_distance,
        0,
    )?;
    let probs = softmax(&scores);
    let attn = attention_output_for_query(&probs, &v_head, compare_head_dims, token_count, 0);

    if layer == 0 {
        compare_stage_2d(
            &ggml_debug_dir.join("t5_embed.bin"),
            "t5_embed",
            hidden_size,
            token_count,
            &block_input,
            0,
            compare_head_dims,
        )?;
        compare_stage_2d(
            &ggml_debug_dir.join("t5_embed.bin"),
            "t5_embed[token=2]",
            hidden_size,
            token_count,
            &block_input,
            PAD_TOKEN_INDEX,
            compare_head_dims,
        )?;
    }
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_norm1.bin")),
        &format!("{block_prefix}_norm1"),
        hidden_size,
        token_count,
        &norm1,
        0,
        compare_head_dims,
    )?;
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_norm1.bin")),
        &format!("{block_prefix}_norm1[token=2]"),
        hidden_size,
        token_count,
        &norm1,
        PAD_TOKEN_INDEX,
        compare_head_dims,
    )?;
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_q_linear.bin")),
        &format!("{block_prefix}_q_linear"),
        hidden_size,
        token_count,
        &q_head,
        0,
        compare_head_dims,
    )?;
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_q_linear.bin")),
        &format!("{block_prefix}_q_linear[token=2]"),
        hidden_size,
        token_count,
        &q_head,
        PAD_TOKEN_INDEX,
        compare_head_dims,
    )?;
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_k_linear.bin")),
        &format!("{block_prefix}_k_linear"),
        hidden_size,
        token_count,
        &k_head,
        0,
        compare_head_dims,
    )?;
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_k_linear.bin")),
        &format!("{block_prefix}_k_linear[token=2]"),
        hidden_size,
        token_count,
        &k_head,
        PAD_TOKEN_INDEX,
        compare_head_dims,
    )?;
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_v_linear.bin")),
        &format!("{block_prefix}_v_linear"),
        hidden_size,
        token_count,
        &v_head,
        0,
        compare_head_dims,
    )?;
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_v_linear.bin")),
        &format!("{block_prefix}_v_linear[token=2]"),
        hidden_size,
        token_count,
        &v_head,
        PAD_TOKEN_INDEX,
        compare_head_dims,
    )?;
    compare_stage_scores(
        &ggml_debug_dir.join(format!("{block_prefix}_scores.bin")),
        &format!("{block_prefix}_scores"),
        token_count,
        head_count,
        &scores,
        0,
        compare_score_keys,
    )?;
    let pad_scores = attention_scores_for_query(
        &q_head,
        &k_head,
        weights.relative_attention_bias(),
        token_count,
        HEAD_DIM_FULL.min(head_dim),
        head_count,
        weights.config.relative_attention_bucket_count,
        weights.config.relative_attention_max_distance,
        PAD_TOKEN_INDEX,
    )?;
    compare_stage_scores(
        &ggml_debug_dir.join(format!("{block_prefix}_scores.bin")),
        &format!("{block_prefix}_scores[token=2]"),
        token_count,
        head_count,
        &pad_scores,
        PAD_TOKEN_INDEX,
        compare_score_keys,
    )?;
    compare_stage_scores(
        &ggml_debug_dir.join(format!("{block_prefix}_probs.bin")),
        &format!("{block_prefix}_probs"),
        token_count,
        head_count,
        &probs,
        0,
        compare_score_keys,
    )?;
    let pad_probs = softmax(&pad_scores);
    compare_stage_scores(
        &ggml_debug_dir.join(format!("{block_prefix}_probs.bin")),
        &format!("{block_prefix}_probs[token=2]"),
        token_count,
        head_count,
        &pad_probs,
        PAD_TOKEN_INDEX,
        compare_score_keys,
    )?;
    compare_stage_token0_vector(
        &ggml_debug_dir.join(format!("{block_prefix}_attn.bin")),
        &format!("{block_prefix}_attn"),
        hidden_size,
        &attn,
        0,
        compare_head_dims,
    )?;
    let pad_attn = attention_output_for_query(
        &pad_probs,
        &v_head,
        compare_head_dims,
        token_count,
        PAD_TOKEN_INDEX,
    );
    compare_stage_token0_vector(
        &ggml_debug_dir.join(format!("{block_prefix}_attn.bin")),
        &format!("{block_prefix}_attn[token=2]"),
        hidden_size,
        &pad_attn,
        PAD_TOKEN_INDEX,
        compare_head_dims,
    )?;

    let attn_full = read_f32_file(&ggml_debug_dir.join(format!("{block_prefix}_attn.bin")))?;
    let attn_proj = linear_selected_outputs(
        &weights,
        &format!("{attn_prefix}.SelfAttention.o.weight"),
        &attn_full,
        hidden_size,
        token_count,
        &(0..compare_head_dims).collect::<Vec<_>>(),
        None,
    )?;
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_attn_proj.bin")),
        &format!("{block_prefix}_attn_proj"),
        hidden_size,
        token_count,
        &attn_proj,
        0,
        compare_head_dims,
    )?;

    let attn_proj_full = read_f32_file(&ggml_debug_dir.join(format!("{block_prefix}_attn_proj.bin")))?;
    let hidden_after_attn = add_tensors(&block_input, &attn_proj_full)?;
    let norm2_weight = vector_tensor(&weights, &format!("{ff_prefix}.layer_norm.weight"))?;
    let norm2 = rms_norm_all_tokens(
        &hidden_after_attn,
        &norm2_weight,
        hidden_size,
        token_count,
        weights.config.layer_norm_epsilon(),
    );
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_norm2.bin")),
        &format!("{block_prefix}_norm2"),
        hidden_size,
        token_count,
        &norm2,
        0,
        compare_head_dims,
    )?;

    let norm2_full = read_f32_file(&ggml_debug_dir.join(format!("{block_prefix}_norm2.bin")))?;
    let wi0_linear = linear_selected_outputs(
        &weights,
        &format!("{ff_prefix}.DenseReluDense.wi_0.weight"),
        &norm2_full,
        hidden_size,
        token_count,
        &(0..compare_head_dims).collect::<Vec<_>>(),
        None,
    )?;
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_wi0_linear.bin")),
        &format!("{block_prefix}_wi0_linear"),
        usize::try_from(weights.config.feedforward_dim)?,
        token_count,
        &wi0_linear,
        0,
        compare_head_dims,
    )?;
    let wi1_linear = linear_selected_outputs(
        &weights,
        &format!("{ff_prefix}.DenseReluDense.wi_1.weight"),
        &norm2_full,
        hidden_size,
        token_count,
        &(0..compare_head_dims).collect::<Vec<_>>(),
        None,
    )?;
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_wi1_linear.bin")),
        &format!("{block_prefix}_wi1_linear"),
        usize::try_from(weights.config.feedforward_dim)?,
        token_count,
        &wi1_linear,
        0,
        compare_head_dims,
    )?;

    let wi0_linear_full = read_f32_file(&ggml_debug_dir.join(format!("{block_prefix}_wi0_linear.bin")))?;
    let wi0_gelu = gelu_first_dims_all_tokens(&wi0_linear_full, usize::try_from(weights.config.feedforward_dim)?, token_count, compare_head_dims);
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_wi0_gelu.bin")),
        &format!("{block_prefix}_wi0_gelu"),
        usize::try_from(weights.config.feedforward_dim)?,
        token_count,
        &wi0_gelu,
        0,
        compare_head_dims,
    )?;

    let wi1_linear_full = read_f32_file(&ggml_debug_dir.join(format!("{block_prefix}_wi1_linear.bin")))?;
    let gated = mul_first_dims_all_tokens(
        &read_f32_file(&ggml_debug_dir.join(format!("{block_prefix}_wi0_gelu.bin")))?,
        &wi1_linear_full,
        usize::try_from(weights.config.feedforward_dim)?,
        token_count,
        compare_head_dims,
    );
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_gated.bin")),
        &format!("{block_prefix}_gated"),
        usize::try_from(weights.config.feedforward_dim)?,
        token_count,
        &gated,
        0,
        compare_head_dims,
    )?;

    let gated_full = read_f32_file(&ggml_debug_dir.join(format!("{block_prefix}_gated.bin")))?;
    let ff_out = linear_selected_outputs(
        &weights,
        &format!("{ff_prefix}.DenseReluDense.wo.weight"),
        &gated_full,
        usize::try_from(weights.config.feedforward_dim)?,
        token_count,
        &(0..compare_head_dims).collect::<Vec<_>>(),
        None,
    )?;
    compare_stage_2d(
        &ggml_debug_dir.join(format!("{block_prefix}_ff_out.bin")),
        &format!("{block_prefix}_ff_out"),
        hidden_size,
        token_count,
        &ff_out,
        0,
        compare_head_dims,
    )?;

    println!(
        "workflow: {}\nt5xxl model: {}\nprompt.t5xxl: {}",
        workflow.path.display(),
        t5xxl_path.display(),
        plan.prompts.t5xxl
    );
    println!(
        "compared T5 block {} slices: token_count={} hidden_size={} heads={} head_dim={} head={}",
        layer,
        token_count,
        hidden_size,
        head_count,
        head_dim,
        HEAD_INDEX
    );

    Ok(())
}

fn embedding_all_tokens(weights: &LoadedT5xxlWeights, token_ids: &[i32]) -> Result<Vec<f32>, String> {
    let tensor = named_tensor(weights, "shared.weight")?;
    let width = usize::try_from(tensor.ne[0]).map_err(|_| "shared.weight width exceeds usize".to_string())?;
    let rows = usize::try_from(tensor.ne[1]).map_err(|_| "shared.weight rows exceeds usize".to_string())?;
    let mut out = vec![0.0f32; width * token_ids.len()];
    for (token_index, &token_id) in token_ids.iter().enumerate() {
        let row = usize::try_from(token_id).map_err(|_| format!("negative token id {}", token_id))?;
        if row >= rows {
            return Err(format!("token id {} exceeds shared.weight rows {}", row, rows));
        }
        for dim in 0..width {
            out[dim + token_index * width] = tensor_value_2d(weights, "shared.weight", dim, row)?;
        }
    }
    Ok(out)
}

fn rms_norm_all_tokens(
    input: &[f32],
    weight: &[f32],
    hidden_size: usize,
    token_count: usize,
    epsilon: f32,
) -> Vec<f32> {
    let mut out = vec![0.0f32; input.len()];
    for token in 0..token_count {
        let base = token * hidden_size;
        let slice = &input[base..base + hidden_size];
        let mean_square = slice.iter().map(|value| value * value).sum::<f32>() / hidden_size as f32;
        let inv_rms = (mean_square + epsilon).sqrt().recip();
        for dim in 0..hidden_size {
            out[base + dim] = slice[dim] * inv_rms * weight[dim];
        }
    }
    out
}

fn linear_selected_outputs(
    weights: &LoadedT5xxlWeights,
    weight_name: &str,
    input: &[f32],
    input_size: usize,
    token_count: usize,
    output_indices: &[usize],
    token_limit: Option<usize>,
) -> Result<Vec<f32>, String> {
    let tensor = named_tensor(weights, weight_name)?;
    let in_features = usize::try_from(tensor.ne[0]).map_err(|_| format!("{weight_name} in_features exceeds usize"))?;
    let out_features = usize::try_from(tensor.ne[1]).map_err(|_| format!("{weight_name} out_features exceeds usize"))?;
    if in_features != input_size {
        return Err(format!(
            "{weight_name} input size mismatch: weight={} input={}",
            in_features, input_size
        ));
    }
    let used_tokens = token_limit.unwrap_or(token_count).min(token_count);
    let mut out = vec![0.0f32; output_indices.len() * token_count];
    for token in 0..used_tokens {
        let input_base = token * input_size;
        for (selected_index, &output_index) in output_indices.iter().enumerate() {
            if output_index >= out_features {
                return Err(format!(
                    "{weight_name} output index {} exceeds {}",
                    output_index, out_features
                ));
            }
            let mut acc = 0.0f32;
            for input_index in 0..input_size {
                acc += tensor_value_2d(weights, weight_name, input_index, output_index)?
                    * input[input_base + input_index];
            }
            out[selected_index + token * output_indices.len()] = acc;
        }
    }
    Ok(out)
}

fn attention_scores_for_query(
    q_head: &[f32],
    k_head: &[f32],
    relative_attention_bias: &[f32],
    token_count: usize,
    head_dim: usize,
    head_count: usize,
    bucket_count: u32,
    max_distance: u32,
    query: usize,
) -> Result<Vec<f32>, String> {
    let mut scores = vec![0.0f32; token_count];
    let q_base = query * head_dim;
    for key in 0..token_count {
        let mut acc = 0.0f32;
        for dim in 0..head_dim {
            acc += q_head[q_base + dim] * k_head[dim + key * head_dim];
        }
        let bucket = relative_position_bucket(query, key, bucket_count, max_distance)?;
        scores[key] = acc + relative_attention_bias[bucket * head_count + HEAD_INDEX];
    }
    Ok(scores)
}

fn attention_output_for_query(
    probs: &[f32],
    v_head: &[f32],
    compare_head_dims: usize,
    token_count: usize,
    _query: usize,
) -> Vec<f32> {
    let mut out = vec![0.0f32; compare_head_dims];
    for token in 0..token_count {
        let prob = probs[token];
        for dim in 0..compare_head_dims {
            out[dim] += prob * v_head[dim + token * compare_head_dims];
        }
    }
    out
}

fn vector_tensor(weights: &LoadedT5xxlWeights, name: &str) -> Result<Vec<f32>, String> {
    let tensor = named_tensor(weights, name)?;
    let len = usize::try_from(tensor.ne[0]).map_err(|_| format!("{name} length exceeds usize"))?;
    let mut out = Vec::with_capacity(len);
    for index in 0..len {
        out.push(tensor_value_1d(weights, name, index)?);
    }
    Ok(out)
}

fn compare_stage_2d(
    path: &Path,
    name: &str,
    ggml_width: usize,
    token_count: usize,
    cpu_values: &[f32],
    token: usize,
    preview_len: usize,
) -> Result<(), String> {
    let ggml_values = read_f32_file(path)?;
    let expected_len = ggml_width
        .checked_mul(token_count)
        .ok_or_else(|| format!("{name} expected length overflow"))?;
    if ggml_values.len() != expected_len {
        return Err(format!(
            "{name} expected {} values, got {} in {}",
            expected_len,
            ggml_values.len(),
            path.display()
        ));
    }
    let cpu_width = cpu_values
        .len()
        .checked_div(token_count.max(1))
        .ok_or_else(|| format!("{name} cpu width division failed"))?;
    if preview_len > cpu_width {
        return Err(format!(
            "{name} preview length {} exceeds cpu width {}",
            preview_len, cpu_width
        ));
    }
    let ggml_base = token
        .checked_mul(ggml_width)
        .ok_or_else(|| format!("{name} token base overflow"))?;
    let cpu_base = token
        .checked_mul(cpu_width)
        .ok_or_else(|| format!("{name} cpu token base overflow"))?;
    let mut max_abs = 0.0f32;
    let mut mean_abs = 0.0f32;
    for dim in 0..preview_len {
        let diff = (ggml_values[ggml_base + dim] - cpu_values[cpu_base + dim]).abs();
        max_abs = max_abs.max(diff);
        mean_abs += diff;
    }
    mean_abs /= preview_len as f32;
    println!(
        "{name}: max_abs={max_abs:.8} mean_abs={mean_abs:.8}\n  ggml={:?}\n  cpu ={:#?}",
        &ggml_values[ggml_base..ggml_base + preview_len],
        &cpu_values[cpu_base..cpu_base + preview_len]
    );
    Ok(())
}

fn compare_stage_scores(
    path: &Path,
    name: &str,
    token_count: usize,
    head_count: usize,
    cpu_query_row: &[f32],
    query: usize,
    preview_len: usize,
) -> Result<(), String> {
    let ggml_values = read_f32_file(path)?;
    let expected_len = token_count
        .checked_mul(token_count)
        .and_then(|value| value.checked_mul(head_count))
        .ok_or_else(|| format!("{name} expected length overflow"))?;
    if ggml_values.len() != expected_len {
        return Err(format!(
            "{name} expected {} values, got {} in {}",
            expected_len,
            ggml_values.len(),
            path.display()
        ));
    }
    let mut ggml_preview = Vec::with_capacity(preview_len);
    let mut cpu_preview = Vec::with_capacity(preview_len);
    let mut max_abs = 0.0f32;
    let mut mean_abs = 0.0f32;
    let query_base = query
        .checked_mul(token_count)
        .ok_or_else(|| format!("{name} query base overflow"))?;
    if cpu_query_row.len() < preview_len {
        return Err(format!(
            "{name} cpu preview length {} exceeds {}",
            preview_len,
            cpu_query_row.len()
        ));
    }
    for key in 0..preview_len {
        let index = query_base + key;
        let ggml = ggml_values[index];
        let cpu = cpu_query_row[key];
        ggml_preview.push(ggml);
        cpu_preview.push(cpu);
        let diff = (ggml - cpu).abs();
        max_abs = max_abs.max(diff);
        mean_abs += diff;
    }
    mean_abs /= preview_len as f32;
    println!(
        "{name}: max_abs={max_abs:.8} mean_abs={mean_abs:.8}\n  ggml={:?}\n  cpu ={:#?}",
        ggml_preview,
        cpu_preview
    );
    Ok(())
}

fn compare_stage_token0_vector(
    path: &Path,
    name: &str,
    ggml_width: usize,
    cpu_values: &[f32],
    token: usize,
    preview_len: usize,
) -> Result<(), String> {
    let ggml_values = read_f32_file(path)?;
    let ggml_base = token
        .checked_mul(ggml_width)
        .ok_or_else(|| format!("{name} ggml token base overflow"))?;
    if ggml_values.len() < ggml_base + ggml_width {
        return Err(format!(
            "{name} expected at least {} values, got {} in {}",
            ggml_base + ggml_width,
            ggml_values.len(),
            path.display()
        ));
    }
    if cpu_values.len() < preview_len {
        return Err(format!(
            "{name} cpu preview length {} exceeds {}",
            preview_len,
            cpu_values.len()
        ));
    }
    let mut max_abs = 0.0f32;
    let mut mean_abs = 0.0f32;
    for dim in 0..preview_len {
        let diff = (ggml_values[ggml_base + dim] - cpu_values[dim]).abs();
        max_abs = max_abs.max(diff);
        mean_abs += diff;
    }
    mean_abs /= preview_len as f32;
    println!(
        "{name}: max_abs={max_abs:.8} mean_abs={mean_abs:.8}\n  ggml={:?}\n  cpu ={:#?}",
        &ggml_values[ggml_base..ggml_base + preview_len],
        &cpu_values[..preview_len]
    );
    Ok(())
}

fn named_tensor<'a>(
    weights: &'a LoadedT5xxlWeights,
    name: &str,
) -> Result<&'a makepad_ggml::Tensor, String> {
    let tensor_id = weights
        .tensor_ids
        .get(name)
        .copied()
        .ok_or_else(|| format!("missing tensor '{name}'"))?;
    weights
        .ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid tensor '{name}' id {}", tensor_id))
}

fn tensor_value_1d(weights: &LoadedT5xxlWeights, name: &str, index: usize) -> Result<f32, String> {
    tensor_value_2d(weights, name, index, 0)
}

fn tensor_value_2d(
    weights: &LoadedT5xxlWeights,
    name: &str,
    x: usize,
    y: usize,
) -> Result<f32, String> {
    let tensor_id = weights
        .tensor_ids
        .get(name)
        .copied()
        .ok_or_else(|| format!("missing tensor '{name}'"))?;
    let tensor = weights
        .ctx
        .tensor(tensor_id)
        .ok_or_else(|| format!("invalid tensor '{name}' id {}", tensor_id))?;
    let width = usize::try_from(tensor.ne[0]).map_err(|_| format!("{name} width exceeds usize"))?;
    let height = usize::try_from(tensor.ne[1]).map_err(|_| format!("{name} height exceeds usize"))?;
    if x >= width || y >= height {
        return Err(format!(
            "{name} index ({x}, {y}) exceeds ({width}, {height})"
        ));
    }
    let flat = x + y * width;
    let bytes = weights
        .ctx
        .tensor_data(tensor_id)
        .map_err(|err| format!("failed to read tensor '{name}': {err}"))?;
    tensor_value_from_bytes(bytes, tensor.desc.ty, flat)
}

fn tensor_value_from_bytes(bytes: &[u8], ty: TensorType, index: usize) -> Result<f32, String> {
    match ty {
        TensorType::F32 => {
            let offset = index
                .checked_mul(4)
                .ok_or_else(|| "F32 tensor offset overflow".to_string())?;
            let chunk = bytes
                .get(offset..offset + 4)
                .ok_or_else(|| format!("F32 tensor offset {} out of bounds", offset))?;
            Ok(f32::from_le_bytes(chunk.try_into().unwrap()))
        }
        TensorType::F16 => {
            let offset = index
                .checked_mul(2)
                .ok_or_else(|| "F16 tensor offset overflow".to_string())?;
            let chunk = bytes
                .get(offset..offset + 2)
                .ok_or_else(|| format!("F16 tensor offset {} out of bounds", offset))?;
            Ok(f16_to_f32(u16::from_le_bytes(chunk.try_into().unwrap())))
        }
        TensorType::BF16 => {
            let offset = index
                .checked_mul(2)
                .ok_or_else(|| "BF16 tensor offset overflow".to_string())?;
            let chunk = bytes
                .get(offset..offset + 2)
                .ok_or_else(|| format!("BF16 tensor offset {} out of bounds", offset))?;
            Ok(bf16_to_f32(u16::from_le_bytes(chunk.try_into().unwrap())))
        }
        other => Err(format!("unsupported tensor dtype {}", other.name())),
    }
}

fn read_f32_file(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = fs::read(path).map_err(|err| format!("failed to read {}: {}", path.display(), err))?;
    if bytes.len() % 4 != 0 {
        return Err(format!(
            "file {} length {} is not divisible by 4",
            path.display(),
            bytes.len()
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect())
}

fn softmax(values: &[f32]) -> Vec<f32> {
    let max = values
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let mut out = Vec::with_capacity(values.len());
    let mut sum = 0.0f32;
    for &value in values {
        let exp = (value - max).exp();
        out.push(exp);
        sum += exp;
    }
    for value in &mut out {
        *value /= sum;
    }
    out
}

fn relative_position_bucket(
    query_position: usize,
    key_position: usize,
    bucket_count: u32,
    max_distance: u32,
) -> Result<usize, String> {
    let half_bucket_count = i32::try_from(bucket_count / 2)
        .map_err(|_| "relative bucket count exceeds i32".to_string())?;
    let relative_position = i64::try_from(key_position)
        .and_then(|key| i64::try_from(query_position).map(|query| key - query))
        .map_err(|_| "relative position exceeds i64".to_string())?;
    let positive_bucket_base = if relative_position > 0 {
        usize::try_from(half_bucket_count).map_err(|_| "positive bucket base exceeds usize".to_string())?
    } else {
        0
    };
    let relative_position = relative_position.unsigned_abs() as i64;
    let max_exact = half_bucket_count / 2;
    let bucket_in_half = if relative_position < i64::from(max_exact) {
        relative_position as i32
    } else {
        let relative_position = relative_position as f32;
        let max_exact_f = max_exact as f32;
        let half_bucket_count_f = half_bucket_count as f32;
        let max_distance_f = max_distance as f32;
        let scaled = max_exact_f
            + (relative_position / max_exact_f).ln()
                / (max_distance_f / max_exact_f).ln()
                * (half_bucket_count_f - max_exact_f);
        scaled.floor().min((half_bucket_count - 1) as f32) as i32
    };
    usize::try_from(bucket_in_half)
        .map(|bucket| positive_bucket_base + bucket)
        .map_err(|_| "relative bucket index exceeds usize".to_string())
}

fn add_tensors(lhs: &[f32], rhs: &[f32]) -> Result<Vec<f32>, String> {
    if lhs.len() != rhs.len() {
        return Err(format!(
            "tensor add length mismatch: lhs={} rhs={}",
            lhs.len(),
            rhs.len()
        ));
    }
    Ok(lhs.iter().zip(rhs).map(|(a, b)| a + b).collect())
}

fn gelu_first_dims_all_tokens(
    full_values: &[f32],
    width: usize,
    token_count: usize,
    preview_len: usize,
) -> Vec<f32> {
    let mut out = vec![0.0f32; preview_len * token_count];
    for token in 0..token_count {
        let src_base = token * width;
        let dst_base = token * preview_len;
        for dim in 0..preview_len {
            out[dst_base + dim] = gelu_scalar(full_values[src_base + dim]);
        }
    }
    out
}

fn mul_first_dims_all_tokens(
    lhs: &[f32],
    rhs: &[f32],
    width: usize,
    token_count: usize,
    preview_len: usize,
) -> Vec<f32> {
    let mut out = vec![0.0f32; preview_len * token_count];
    for token in 0..token_count {
        let src_base = token * width;
        let dst_base = token * preview_len;
        for dim in 0..preview_len {
            out[dst_base + dim] = lhs[src_base + dim] * rhs[src_base + dim];
        }
    }
    out
}

fn gelu_scalar(x: f32) -> f32 {
    const GELU_COEF_A: f32 = 0.044715;
    const SQRT_2_OVER_PI: f32 = 0.7978846;
    0.5 * x * (1.0 + (SQRT_2_OVER_PI * x * (1.0 + GELU_COEF_A * x * x)).tanh())
}
