//! MiniMax-Music3 RVQ depth decoder (0.6B local LM) on CUDA ggml.
//! 4-layer causal transformer, hidden 4096, 16 heads, no RoPE / QK-norm.

use crate::backend::{
    gpu_add, gpu_attention_packed_causal, gpu_concat_rows, gpu_device_available, gpu_download,
    gpu_linear_nt_cached_bf16_f32acc, gpu_rms_norm_mul, gpu_slice_rows, gpu_upload,
    gpu_weight_cache_ensure, GpuLinearPart, GpuTensor,
};
use crate::music3::{
    MUSIC3_AUDIO_VOCAB, MUSIC3_LM_RMS_EPS, MUSIC3_NUM_CODEBOOKS, MUSIC3_RVQ_FF, MUSIC3_RVQ_HEADS,
    MUSIC3_RVQ_HIDDEN, MUSIC3_RVQ_LAYERS, MUSIC3_RVQ_MAX_POS,
};
use crate::music3_weights::{Music3Shards, MUSIC3_RVQ_NAMESPACE};
use crate::{DiffusionError, Result};

fn ensure_linear<'a>(
    weights: &Music3Shards,
    name: &'a str,
    n: usize,
    k: usize,
) -> Result<GpuLinearPart<'a>> {
    let ggml_type = weights.linear_ggml_type(name);
    if crate::backend::gpu_quant_linear_type_supported(ggml_type) {
        crate::backend::gpu_weight_cache_ensure_quant(MUSIC3_RVQ_NAMESPACE, name, ggml_type, n, k, || {
            weights.tensor_bytes(name).map_err(|err| err.to_string())
        })
        .map_err(DiffusionError::model)?;
    } else {
        gpu_weight_cache_ensure(MUSIC3_RVQ_NAMESPACE, name, ggml_type, n, k, false, || {
            weights.tensor_bytes(name).map_err(|err| err.to_string())
        })
        .map_err(DiffusionError::model)?;
    }
    Ok(GpuLinearPart {
        bt_ggml_type: ggml_type,
        n,
        cache_key: name,
        bytes: &[],
    })
}

fn linear_parts(
    x: &GpuTensor,
    parts: &[GpuLinearPart<'_>],
) -> Result<GpuTensor> {
    if crate::backend::gpu_quant_linear_type_supported(parts[0].bt_ggml_type) {
        return crate::backend::gpu_linear_nt_cached(x, MUSIC3_RVQ_NAMESPACE, parts, &[])
            .map_err(DiffusionError::model);
    }
    gpu_linear_nt_cached_bf16_f32acc(x, MUSIC3_RVQ_NAMESPACE, parts, &[]).map_err(DiffusionError::model)
}

fn linear(
    weights: &Music3Shards,
    x: &GpuTensor,
    name: &str,
    n: usize,
) -> Result<GpuTensor> {
    let part = ensure_linear(weights, name, n, x.cols())?;
    linear_parts(x, &[part])
}

pub struct Music3RvqPrepared {
    input_norm: Vec<Vec<f32>>,
    post_attn_norm: Vec<Vec<f32>>,
    final_norm: Vec<f32>,
    pos_embedding: Vec<f32>,
}

impl Music3RvqPrepared {
    pub fn prepare(weights: &Music3Shards) -> Result<Self> {
        let mut input_norm = Vec::with_capacity(MUSIC3_RVQ_LAYERS);
        let mut post_attn_norm = Vec::with_capacity(MUSIC3_RVQ_LAYERS);
        for layer in 0..MUSIC3_RVQ_LAYERS {
            input_norm.push(weights.tensor_f32(&format!("layers.{layer}.input_layernorm.weight"))?);
            post_attn_norm.push(
                weights.tensor_f32(&format!("layers.{layer}.post_attention_layernorm.weight"))?,
            );
        }
        Ok(Self {
            input_norm,
            post_attn_norm,
            final_norm: weights.tensor_f32("norm.weight")?,
            pos_embedding: weights.tensor_f32("pos_embedding.weight")?,
        })
    }
}

fn layer_forward(
    weights: &Music3Shards,
    prepared: &Music3RvqPrepared,
    layer: usize,
    hidden: GpuTensor,
    n: usize,
    batch: usize,
) -> Result<GpuTensor> {
    let head_dim = MUSIC3_RVQ_HIDDEN / MUSIC3_RVQ_HEADS;
    let scale = 1.0 / (head_dim as f32).sqrt();
    let mut hidden = hidden;
    let normed = gpu_rms_norm_mul(
        &hidden,
        MUSIC3_RVQ_HIDDEN,
        MUSIC3_RVQ_NAMESPACE,
        &format!("layers.{layer}.input_layernorm"),
        &prepared.input_norm[layer],
        MUSIC3_LM_RMS_EPS,
    )
    .map_err(DiffusionError::model)?;
    let q_name = format!("layers.{layer}.attn.to_q.weight");
    let k_name = format!("layers.{layer}.attn.to_k.weight");
    let v_name = format!("layers.{layer}.attn.to_v.weight");
    let parts = [
        ensure_linear(weights, &q_name, MUSIC3_RVQ_HIDDEN, MUSIC3_RVQ_HIDDEN)?,
        ensure_linear(weights, &k_name, MUSIC3_RVQ_HIDDEN, MUSIC3_RVQ_HIDDEN)?,
        ensure_linear(weights, &v_name, MUSIC3_RVQ_HIDDEN, MUSIC3_RVQ_HIDDEN)?,
    ];
    let qkv = linear_parts(&normed, &parts)?;
    drop(normed);
    let q = crate::backend::gpu_slice_cols(&qkv, 0, MUSIC3_RVQ_HIDDEN).map_err(DiffusionError::model)?;
    let k = crate::backend::gpu_slice_cols(&qkv, MUSIC3_RVQ_HIDDEN, MUSIC3_RVQ_HIDDEN)
        .map_err(DiffusionError::model)?;
    let v = crate::backend::gpu_slice_cols(&qkv, 2 * MUSIC3_RVQ_HIDDEN, MUSIC3_RVQ_HIDDEN)
        .map_err(DiffusionError::model)?;
    drop(qkv);
    // Official CFG is batch=2 independent sequences. GEMM stays [2S, H];
    // packed causal must not cross the pair (same split as LM prefill_pair).
    let attn = if batch <= 1 {
        gpu_attention_packed_causal(&q, &k, &v, MUSIC3_RVQ_HEADS, scale)
            .map_err(DiffusionError::model)?
    } else {
        let mut attn_parts = Vec::with_capacity(batch);
        for b in 0..batch {
            let q_b = gpu_slice_rows(&q, b * n, n).map_err(DiffusionError::model)?;
            let k_b = gpu_slice_rows(&k, b * n, n).map_err(DiffusionError::model)?;
            let v_b = gpu_slice_rows(&v, b * n, n).map_err(DiffusionError::model)?;
            attn_parts.push(
                gpu_attention_packed_causal(&q_b, &k_b, &v_b, MUSIC3_RVQ_HEADS, scale)
                    .map_err(DiffusionError::model)?,
            );
        }
        let mut joined = attn_parts.remove(0);
        for part in attn_parts {
            joined = gpu_concat_rows(&joined, &part).map_err(DiffusionError::model)?;
        }
        joined
    };
    drop((q, k, v));
    let attn = linear(
        weights,
        &attn,
        &format!("layers.{layer}.attn.to_out.weight"),
        MUSIC3_RVQ_HIDDEN,
    )?;
    hidden = gpu_add(&hidden, &attn).map_err(DiffusionError::model)?;
    drop(attn);

    let normed = gpu_rms_norm_mul(
        &hidden,
        MUSIC3_RVQ_HIDDEN,
        MUSIC3_RVQ_NAMESPACE,
        &format!("layers.{layer}.post_attention_layernorm"),
        &prepared.post_attn_norm[layer],
        MUSIC3_LM_RMS_EPS,
    )
    .map_err(DiffusionError::model)?;
    let up_name = format!("layers.{layer}.up_proj.weight");
    let gate_name = format!("layers.{layer}.gate_proj.weight");
    let parts = [
        ensure_linear(weights, &up_name, MUSIC3_RVQ_FF, MUSIC3_RVQ_HIDDEN)?,
        ensure_linear(weights, &gate_name, MUSIC3_RVQ_FF, MUSIC3_RVQ_HIDDEN)?,
    ];
    let up_gate = linear_parts(&normed, &parts)?;
    drop(normed);
    let ff = crate::backend::gpu_swiglu_value_gate(&up_gate).map_err(DiffusionError::model)?;
    drop(up_gate);
    let ff = linear(
        weights,
        &ff,
        &format!("layers.{layer}.down_proj.weight"),
        MUSIC3_RVQ_HIDDEN,
    )?;
    hidden = gpu_add(&hidden, &ff).map_err(DiffusionError::model)?;
    let _ = (n, batch);
    Ok(hidden)
}

fn rvq_forward_gpu(
    weights: &Music3Shards,
    prepared: &Music3RvqPrepared,
    inputs: &GpuTensor,
) -> Result<GpuTensor> {
    rvq_forward_gpu_batch(weights, prepared, inputs, inputs.rows(), 1)
}

/// Official depth decoder: `batch` independent sequences of length `seq`.
/// `inputs` is `[batch * seq, H]` laid out `[row0_seq; row1_seq; ...]`.
fn rvq_forward_gpu_batch(
    weights: &Music3Shards,
    prepared: &Music3RvqPrepared,
    inputs: &GpuTensor,
    seq: usize,
    batch: usize,
) -> Result<GpuTensor> {
    let batch = batch.max(1);
    let rows = seq.saturating_mul(batch);
    if seq == 0 || seq > MUSIC3_RVQ_MAX_POS || rows == 0 || inputs.rows() != rows
        || inputs.cols() != MUSIC3_RVQ_HIDDEN
    {
        return Err(DiffusionError::model(format!(
            "music3 RVQ input {}x{} for batch {batch} seq {seq}",
            inputs.rows(),
            inputs.cols()
        )));
    }
    let mut pos_host = Vec::with_capacity(rows * MUSIC3_RVQ_HIDDEN);
    let one = &prepared.pos_embedding[..seq * MUSIC3_RVQ_HIDDEN];
    for _ in 0..batch {
        pos_host.extend_from_slice(one);
    }
    let pos = gpu_upload(&pos_host, rows, MUSIC3_RVQ_HIDDEN).map_err(DiffusionError::model)?;
    let mut hidden = gpu_add(inputs, &pos).map_err(DiffusionError::model)?;
    drop(pos);
    for layer in 0..MUSIC3_RVQ_LAYERS {
        hidden = layer_forward(weights, prepared, layer, hidden, seq, batch)?;
    }
    gpu_rms_norm_mul(
        &hidden,
        MUSIC3_RVQ_HIDDEN,
        MUSIC3_RVQ_NAMESPACE,
        "norm",
        &prepared.final_norm,
        MUSIC3_LM_RMS_EPS,
    )
    .map_err(DiffusionError::model)
}

/// `inputs_embeds` is `[seq, hidden]`. Returns same shape after pos+layers+norm.
pub fn music3_rvq_forward(
    weights: &Music3Shards,
    prepared: &Music3RvqPrepared,
    inputs: &[f32],
    seq: usize,
) -> Result<Vec<f32>> {
    if !gpu_device_available() {
        return Err(DiffusionError::model("music3 RVQ needs CUDA"));
    }
    if seq == 0 || inputs.len() != seq * MUSIC3_RVQ_HIDDEN {
        return Err(DiffusionError::model(format!(
            "music3 RVQ input {} for seq {seq}",
            inputs.len()
        )));
    }
    let hidden = gpu_upload(inputs, seq, MUSIC3_RVQ_HIDDEN).map_err(DiffusionError::model)?;
    let hidden = rvq_forward_gpu(weights, prepared, &hidden)?;
    gpu_download(&hidden).map_err(DiffusionError::model)
}

/// Official `_generate_depth_codes`: one decoder forward on `[2, seq, H]`.
/// Returns `(cond_out, uncond_out)` each `[seq, H]`.
pub fn music3_rvq_forward_pair(
    weights: &Music3Shards,
    prepared: &Music3RvqPrepared,
    cond: &[f32],
    uncond: &[f32],
    seq: usize,
) -> Result<(Vec<f32>, Vec<f32>)> {
    if !gpu_device_available() {
        return Err(DiffusionError::model("music3 RVQ needs CUDA"));
    }
    let width = seq.saturating_mul(MUSIC3_RVQ_HIDDEN);
    if seq == 0 || cond.len() != width || uncond.len() != width {
        return Err(DiffusionError::model(format!(
            "music3 RVQ pair input {}+{} for seq {seq}",
            cond.len(),
            uncond.len()
        )));
    }
    let mut both = Vec::with_capacity(2 * width);
    both.extend_from_slice(cond);
    both.extend_from_slice(uncond);
    let hidden = gpu_upload(&both, 2 * seq, MUSIC3_RVQ_HIDDEN).map_err(DiffusionError::model)?;
    let hidden = rvq_forward_gpu_batch(weights, prepared, &hidden, seq, 2)?;
    let host = gpu_download(&hidden).map_err(DiffusionError::model)?;
    let (c, u) = host.split_at(width);
    Ok((c.to_vec(), u.to_vec()))
}

/// Project a `[hidden]` vector through `projection` (used to build the depth sequence).
fn project_gpu(weights: &Music3Shards, hidden: &GpuTensor) -> Result<GpuTensor> {
    linear(weights, hidden, "projection.weight", MUSIC3_RVQ_HIDDEN)
}

fn audio_head_gpu(weights: &Music3Shards, hidden: &GpuTensor, head: usize) -> Result<GpuTensor> {
    if head >= MUSIC3_NUM_CODEBOOKS - 1 {
        return Err(DiffusionError::model(format!("rvq head {head}")));
    }
    linear(
        weights,
        hidden,
        &format!("audio_heads.{head}.weight"),
        MUSIC3_AUDIO_VOCAB,
    )
}

/// Device-resident residual sample. Downloads only 1024 logits per head
/// (not the growing sequence after every depth step).
pub fn music3_rvq_depth_sample_gpu(
    weights: &Music3Shards,
    prepared: &Music3RvqPrepared,
    last_hidden: &GpuTensor,
    semantic_embed: &GpuTensor,
    rng: &mut u64,
    top_k: usize,
) -> Result<(Vec<u32>, Vec<f32>)> {
    let p0 = project_gpu(weights, last_hidden)?;
    let p1 = project_gpu(weights, semantic_embed)?;
    let mut seq = gpu_concat_rows(&p0, &p1).map_err(DiffusionError::model)?;
    drop((p0, p1));
    let mut codes = Vec::with_capacity(MUSIC3_NUM_CODEBOOKS - 1);
    let mut parts = Vec::with_capacity((MUSIC3_NUM_CODEBOOKS - 1) * MUSIC3_RVQ_HIDDEN);
    for head in 0..(MUSIC3_NUM_CODEBOOKS - 1) {
        let out = rvq_forward_gpu(weights, prepared, &seq)?;
        let last = gpu_slice_rows(&out, out.rows() - 1, 1).map_err(DiffusionError::model)?;
        let last_host = gpu_download(&last).map_err(DiffusionError::model)?;
        parts.extend_from_slice(&last_host);
        let logits = gpu_download(&audio_head_gpu(weights, &last, head)?)
            .map_err(DiffusionError::model)?;
        let code = sample_top_k(&logits, top_k, rng);
        codes.push(code as u32);
        if head + 1 < MUSIC3_NUM_CODEBOOKS - 1 {
            let idx = code as u64 + head as u64 * MUSIC3_AUDIO_VOCAB as u64;
            let embed = weights.tensor_row_f32("audio_embeddings.weight", idx)?;
            let embed_g = gpu_upload(&embed, 1, MUSIC3_RVQ_HIDDEN).map_err(DiffusionError::model)?;
            let proj = project_gpu(weights, &embed_g)?;
            seq = gpu_concat_rows(&seq, &proj).map_err(DiffusionError::model)?;
        }
    }
    Ok((codes, parts))
}

/// GPU-tensor-level helpers for the AR loop's device-resident CFG depth
/// chain (music3_ar): same kernels/shapes as the host-roundtrip API.
pub(crate) fn rvq_project_t(weights: &Music3Shards, x: &GpuTensor) -> Result<GpuTensor> {
    project_gpu(weights, x)
}

pub(crate) fn rvq_forward_batch_t(
    weights: &Music3Shards,
    prepared: &Music3RvqPrepared,
    hidden: &GpuTensor,
    seq: usize,
    batch: usize,
) -> Result<GpuTensor> {
    rvq_forward_gpu_batch(weights, prepared, hidden, seq, batch)
}

pub(crate) fn rvq_audio_head_t(
    weights: &Music3Shards,
    x: &GpuTensor,
    head: usize,
) -> Result<GpuTensor> {
    audio_head_gpu(weights, x, head)
}

pub fn music3_rvq_project(weights: &Music3Shards, hidden: &[f32]) -> Result<Vec<f32>> {
    music3_rvq_project_rows(weights, hidden, 1)
}

pub fn music3_rvq_project_rows(
    weights: &Music3Shards,
    hidden: &[f32],
    rows: usize,
) -> Result<Vec<f32>> {
    if rows == 0 || hidden.len() != rows * MUSIC3_RVQ_HIDDEN {
        return Err(DiffusionError::model(format!(
            "rvq project rows {rows} got {}",
            hidden.len()
        )));
    }
    let x = gpu_upload(hidden, rows, MUSIC3_RVQ_HIDDEN).map_err(DiffusionError::model)?;
    let y = linear(weights, &x, "projection.weight", MUSIC3_RVQ_HIDDEN)?;
    gpu_download(&y).map_err(DiffusionError::model)
}

pub fn music3_rvq_audio_head(
    weights: &Music3Shards,
    hidden: &[f32],
    head: usize,
) -> Result<Vec<f32>> {
    music3_rvq_audio_head_rows(weights, hidden, head, 1)
}

pub fn music3_rvq_audio_head_rows(
    weights: &Music3Shards,
    hidden: &[f32],
    head: usize,
    rows: usize,
) -> Result<Vec<f32>> {
    if head >= MUSIC3_NUM_CODEBOOKS - 1 {
        return Err(DiffusionError::model(format!("rvq head {head}")));
    }
    if rows == 0 || hidden.len() != rows * MUSIC3_RVQ_HIDDEN {
        return Err(DiffusionError::model(format!(
            "rvq head {head} rows {rows} got {}",
            hidden.len()
        )));
    }
    let x = gpu_upload(hidden, rows, MUSIC3_RVQ_HIDDEN).map_err(DiffusionError::model)?;
    let y = linear(
        weights,
        &x,
        &format!("audio_heads.{head}.weight"),
        MUSIC3_AUDIO_VOCAB,
    )?;
    gpu_download(&y).map_err(DiffusionError::model)
}

pub fn music3_rvq_evict() -> Result<usize> {
    crate::backend::gpu_weight_cache_evict_prefix(&format!("{MUSIC3_RVQ_NAMESPACE}::"))
        .map_err(DiffusionError::model)
}

/// Replay one frame of residual codes. `last_hidden` and `semantic_embed` are
/// `[H]`. `resid` is 7 codebook indices. Returns concatenated depth hiddens
/// `[7 * H]` (the 7 residual-step last tokens), matching official
/// `depth_hidden`.
pub fn music3_rvq_depth_replay(
    weights: &Music3Shards,
    prepared: &Music3RvqPrepared,
    last_hidden: &[f32],
    semantic_embed: &[f32],
    resid: &[u32],
) -> Result<Vec<f32>> {
    if last_hidden.len() != MUSIC3_RVQ_HIDDEN
        || semantic_embed.len() != MUSIC3_RVQ_HIDDEN
        || resid.len() != MUSIC3_NUM_CODEBOOKS - 1
    {
        return Err(DiffusionError::model("rvq depth replay shapes"));
    }
    let p0 = music3_rvq_project(weights, last_hidden)?;
    let p1 = music3_rvq_project(weights, semantic_embed)?;
    let mut seq = Vec::with_capacity(MUSIC3_NUM_CODEBOOKS * MUSIC3_RVQ_HIDDEN);
    seq.extend_from_slice(&p0);
    seq.extend_from_slice(&p1);
    let mut parts = Vec::with_capacity((MUSIC3_NUM_CODEBOOKS - 1) * MUSIC3_RVQ_HIDDEN);
    for (index, &code) in resid.iter().enumerate() {
        let n = seq.len() / MUSIC3_RVQ_HIDDEN;
        let out = music3_rvq_forward(weights, prepared, &seq, n)?;
        let last = &out[(n - 1) * MUSIC3_RVQ_HIDDEN..];
        parts.extend_from_slice(last);
        if index + 1 < MUSIC3_NUM_CODEBOOKS - 1 {
            let idx = code as u64 + index as u64 * MUSIC3_AUDIO_VOCAB as u64;
            let embed = weights.tensor_row_f32("audio_embeddings.weight", idx)?;
            let proj = music3_rvq_project(weights, &embed)?;
            seq.extend_from_slice(&proj);
        }
    }
    Ok(parts)
}

fn sample_top_k(logits: &[f32], k: usize, rng: &mut u64) -> usize {
    let k = k.max(1).min(logits.len());
    let mut idx: Vec<usize> = (0..logits.len())
        .filter(|&i| logits[i].is_finite())
        .collect();
    if idx.is_empty() {
        return 0;
    }
    idx.sort_by(|&a, &b| logits[b].partial_cmp(&logits[a]).unwrap_or(std::cmp::Ordering::Equal));
    idx.truncate(k);
    let max = idx.iter().map(|&i| logits[i]).fold(f32::NEG_INFINITY, f32::max);
    let mut weights = Vec::with_capacity(idx.len());
    let mut sum = 0.0f64;
    for &i in &idx {
        let w = (logits[i] - max).exp() as f64;
        sum += w;
        weights.push(w);
    }
    let mut dart = xorshift_f64(rng) * sum;
    for (n, &w) in weights.iter().enumerate() {
        dart -= w;
        if dart <= 0.0 {
            return idx[n];
        }
    }
    *idx.last().unwrap_or(&0)
}

fn xorshift_f64(state: &mut u64) -> f64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    (x as f64) / (u64::MAX as f64)
}
