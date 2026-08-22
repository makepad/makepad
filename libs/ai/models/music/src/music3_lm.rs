//! MiniMax-Music3 global LM: Qwen3-8B fine-tune on the in-repo CUDA ggml
//! path (same spine as H3 TE / SkinTokens Qwen3).
//!
//! Official semantic block calls the *inner* `language_model.model(...)`
//! (RMSNorm at the end, no lm_head) then `lm_head` on the last token.
//! Prefill matches dump `lm_prefill_last_hidden` / `lm_prefill_logits`.

use crate::backend::{
    gpu_add, gpu_add_bf16, gpu_attention_gqa_decode_bf16, gpu_attention_gqa_decode_pair_bf16,
    gpu_attention_packed_causal,
    gpu_attention_packed_causal_f32, gpu_attention_packed_causal_flash,
    gpu_bf16_round,
    gpu_concat_cols,
    gpu_concat_rows, gpu_copy_into, gpu_device_available, gpu_download,
    gpu_linear_nt_cached, gpu_linear_nt_cached_bf16_f32acc, gpu_linear_nt_cached_bf16_mm,
    gpu_quant_linear_type_supported, gpu_rms_norm_mul,
    gpu_rms_norm_qwen3, gpu_rope_half, gpu_rope_half_bf16,
    gpu_slice_cols,
    gpu_slice_rows, gpu_swiglu_value_gate, gpu_upload, gpu_weight_cache_ensure,
    gpu_weight_cache_ensure_quant, gpu_weight_cache_evict_prefix, GpuLinearPart, GpuTensor,
};

/// Full official Python stack (bf16 Linear/RMS/resid/rope/KV + MATH SDPA).
/// Isolated official ops + FA2 walk to eager. This flag is the coupled path.
fn official_py() -> bool {
    std::env::var("MAKEPAD_MUSIC3_OFFICIAL").ok().as_deref() == Some("1")
}

/// Official q/k_proj (bf16 mm) + Qwen3 knorm only. ATTN stays FA2.
/// Isolated knorm on f32 K was 641; official+MATH pack had broken knorm 1.44.
/// This is the still-live Fable rematch: official K + official knorm + FA2.
fn qk_official() -> bool {
    official_py()
        || std::env::var("MAKEPAD_MUSIC3_QK_OFFICIAL").ok().as_deref() == Some("1")
}

use crate::music3::{
    MUSIC3_LM_FF, MUSIC3_LM_HEAD_DIM, MUSIC3_LM_HEADS, MUSIC3_LM_HIDDEN, MUSIC3_LM_KV_HEADS,
    MUSIC3_LM_LAYERS, MUSIC3_LM_RMS_EPS, MUSIC3_LM_ROPE_THETA, MUSIC3_LM_VOCAB,
};
use crate::music3_weights::{Music3Shards, MUSIC3_LM_NAMESPACE};
use crate::{DiffusionError, Result};

fn layer_prefix(layer: usize) -> String {
    format!("model.layers.{layer}")
}

fn ensure_linear<'a>(
    weights: &Music3Shards,
    name: &'a str,
    n: usize,
    k: usize,
    m: usize,
) -> Result<GpuLinearPart<'a>> {
    let ggml_type = weights.linear_ggml_type(name);
    let _ = m;
    if gpu_quant_linear_type_supported(ggml_type) {
        // Q4_0 pack: the packed block stream stays resident; the gemm
        // bulk-dequantizes into bf16 scratch (same f32-accumulate contract).
        gpu_weight_cache_ensure_quant(MUSIC3_LM_NAMESPACE, name, ggml_type, n, k, || {
            weights.tensor_bytes(name).map_err(|err| err.to_string())
        })
        .map_err(DiffusionError::model)?;
    } else {
        gpu_weight_cache_ensure(MUSIC3_LM_NAMESPACE, name, ggml_type, n, k, false, || {
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

fn linear_cached(
    weights: &Music3Shards,
    x: &GpuTensor,
    name: &str,
    n: usize,
) -> Result<GpuTensor> {
    let part = ensure_linear(weights, name, n, x.cols(), x.rows())?;
    if gpu_quant_linear_type_supported(part.bt_ggml_type) {
        return gpu_linear_nt_cached(x, MUSIC3_LM_NAMESPACE, &[part], &[])
            .map_err(DiffusionError::model);
    }
    if official_py() {
        return gpu_linear_nt_cached_bf16_mm(x, MUSIC3_LM_NAMESPACE, &[part])
            .map_err(DiffusionError::model);
    }
    gpu_linear_nt_cached_bf16_f32acc(x, MUSIC3_LM_NAMESPACE, &[part], &[])
        .map_err(DiffusionError::model)
}

fn linear_qk(
    weights: &Music3Shards,
    x: &GpuTensor,
    name: &str,
    n: usize,
) -> Result<GpuTensor> {
    let part = ensure_linear(weights, name, n, x.cols(), x.rows())?;
    if gpu_quant_linear_type_supported(part.bt_ggml_type) {
        return gpu_linear_nt_cached(x, MUSIC3_LM_NAMESPACE, &[part], &[])
            .map_err(DiffusionError::model);
    }
    if qk_official() {
        return gpu_linear_nt_cached_bf16_mm(x, MUSIC3_LM_NAMESPACE, &[part])
            .map_err(DiffusionError::model);
    }
    gpu_linear_nt_cached_bf16_f32acc(x, MUSIC3_LM_NAMESPACE, &[part], &[])
        .map_err(DiffusionError::model)
}

fn residual_add(a: &GpuTensor, b: &GpuTensor) -> Result<GpuTensor> {
    if official_py() {
        return gpu_add_bf16(a, b).map_err(DiffusionError::model);
    }
    gpu_add(a, b).map_err(DiffusionError::model)
}

/// Isolated official mm on down_proj rejected 2026-08-17: L0_down 0.100→0.0625
/// but teacher_f0 0.155→0.241, f12 0.962→1.152, RVQ[10] 776→641. Stay f32acc.
fn down_proj_cached(
    weights: &Music3Shards,
    x: &GpuTensor,
    name: &str,
    n: usize,
) -> Result<GpuTensor> {
    linear_cached(weights, x, name, n)
}

/// Official-input L0 MLP intermediates (last-token pair `[B, 4096]`).
pub struct Music3MlpDump {
    pub post_norm: Vec<f32>,
    pub gate: Vec<f32>,
    pub up: Vec<f32>,
    pub swiglu: Vec<f32>,
    pub down: Vec<f32>,
    pub layer_out: Vec<f32>,
}

fn download_mat(t: &GpuTensor) -> Result<Vec<f32>> {
    gpu_download(t).map_err(DiffusionError::model)
}

fn mlp_from_normed(
    weights: &Music3Shards,
    layer: usize,
    hidden: &GpuTensor,
    normed: &GpuTensor,
) -> Result<Music3MlpDump> {
    let prefix = layer_prefix(layer);
    let up_name = format!("{prefix}.mlp.up_proj.weight");
    let gate_name = format!("{prefix}.mlp.gate_proj.weight");
    let up_t = linear_cached(weights, normed, &up_name, MUSIC3_LM_FF)?;
    let gate_t = linear_cached(weights, normed, &gate_name, MUSIC3_LM_FF)?;
    let up = download_mat(&up_t)?;
    let gate = download_mat(&gate_t)?;
    let up_gate = gpu_concat_cols(&[&up_t, &gate_t]).map_err(DiffusionError::model)?;
    let ff = gpu_swiglu_value_gate(&up_gate).map_err(DiffusionError::model)?;
    let swiglu = download_mat(&ff)?;
    let down_t = down_proj_cached(
        weights,
        &ff,
        &format!("{prefix}.mlp.down_proj.weight"),
        MUSIC3_LM_HIDDEN,
    )?;
    let down = download_mat(&down_t)?;
    let out = residual_add(hidden, &down_t)?;
    Ok(Music3MlpDump {
        post_norm: download_mat(normed)?,
        gate,
        up,
        swiglu,
        down,
        layer_out: download_mat(&out)?,
    })
}

/// Official attn residual `[B, 4096]` → native post-norm + SwiGLU + down.
pub fn music3_mlp_from_attn_resid(
    weights: &Music3Shards,
    prepared: &Music3LmPrepared,
    layer: usize,
    resid: &[f32],
) -> Result<Music3MlpDump> {
    if resid.len() % MUSIC3_LM_HIDDEN != 0 {
        return Err(DiffusionError::model("mlp resid cols"));
    }
    let batch = resid.len() / MUSIC3_LM_HIDDEN;
    let hidden = gpu_upload(resid, batch, MUSIC3_LM_HIDDEN).map_err(DiffusionError::model)?;
    let prefix = layer_prefix(layer);
    let normed = music3_hidden_rms(
        &hidden,
        format!("{prefix}.post_attention_layernorm"),
        &prepared.post_attn_norm[layer],
    )?;
    mlp_from_normed(weights, layer, &hidden, &normed)
}

/// Official post_attention_layernorm `[B, 4096]` → native gate/up/swiglu/down.
/// `resid` is only used for `layer_out = resid + down`.
pub fn music3_mlp_from_post_norm(
    weights: &Music3Shards,
    layer: usize,
    resid: &[f32],
    post_norm: &[f32],
) -> Result<Music3MlpDump> {
    if resid.len() != post_norm.len() || resid.len() % MUSIC3_LM_HIDDEN != 0 {
        return Err(DiffusionError::model("mlp post_norm shape"));
    }
    let batch = resid.len() / MUSIC3_LM_HIDDEN;
    let hidden = gpu_upload(resid, batch, MUSIC3_LM_HIDDEN).map_err(DiffusionError::model)?;
    let normed =
        gpu_upload(post_norm, batch, MUSIC3_LM_HIDDEN).map_err(DiffusionError::model)?;
    mlp_from_normed(weights, layer, &hidden, &normed)
}

/// Official SwiGLU `[B, 12288]` → native down_proj.
pub fn music3_down_from_swiglu(
    weights: &Music3Shards,
    layer: usize,
    swiglu: &[f32],
) -> Result<Vec<f32>> {
    if swiglu.len() % MUSIC3_LM_FF != 0 {
        return Err(DiffusionError::model("mlp swiglu cols"));
    }
    let batch = swiglu.len() / MUSIC3_LM_FF;
    let ff = gpu_upload(swiglu, batch, MUSIC3_LM_FF).map_err(DiffusionError::model)?;
    let prefix = layer_prefix(layer);
    let down = down_proj_cached(
        weights,
        &ff,
        &format!("{prefix}.mlp.down_proj.weight"),
        MUSIC3_LM_HIDDEN,
    )?;
    download_mat(&down)
}

/// Official-input one prefill layer: hidden `[2, T, 4096]` → next hidden.
pub fn music3_layer_prefill_pair(
    weights: &Music3Shards,
    prepared: &Music3LmPrepared,
    layer: usize,
    hidden: &[f32],
    tokens: usize,
) -> Result<Vec<f32>> {
    if tokens == 0 || hidden.len() != 2 * tokens * MUSIC3_LM_HIDDEN {
        return Err(DiffusionError::model("layer prefill hidden shape"));
    }
    let x = gpu_upload(hidden, 2 * tokens, MUSIC3_LM_HIDDEN).map_err(DiffusionError::model)?;
    let (rope_one_c, rope_one_s) = rope_range(prepared, 0, tokens)?;
    let rope_cos = gpu_concat_rows(&rope_one_c, &rope_one_c).map_err(DiffusionError::model)?;
    let rope_sin = gpu_concat_rows(&rope_one_s, &rope_one_s).map_err(DiffusionError::model)?;
    let out = layer_forward(
        weights,
        prepared,
        layer,
        x,
        &rope_cos,
        &rope_sin,
        tokens,
        2,
    )?;
    download_mat(&out.hidden)
}

/// Official-input L0 attn: qrope [2T,4096], krope/v [2T,1024] → post-o_proj [2T,4096].
pub fn music3_l0_attn_from_qkv(
    weights: &Music3Shards,
    q: &[f32],
    k: &[f32],
    v: &[f32],
    tokens: usize,
) -> Result<Vec<f32>> {
    if tokens == 0 {
        return Err(DiffusionError::model("l0 attn tokens"));
    }
    let q_inner = MUSIC3_LM_HEADS * MUSIC3_LM_HEAD_DIM;
    let kv_inner = MUSIC3_LM_KV_HEADS * MUSIC3_LM_HEAD_DIM;
    let rows = 2 * tokens;
    if q.len() != rows * q_inner || k.len() != rows * kv_inner || v.len() != rows * kv_inner {
        return Err(DiffusionError::model("l0 attn qkv shape"));
    }
    let q_t = gpu_upload(q, rows, q_inner).map_err(DiffusionError::model)?;
    let k_t = gpu_upload(k, rows, kv_inner).map_err(DiffusionError::model)?;
    let v_t = gpu_upload(v, rows, kv_inner).map_err(DiffusionError::model)?;
    let k_full = music3_repeat_kv(&k_t)?;
    let v_full = music3_repeat_kv(&v_t)?;
    let scale = 1.0 / (MUSIC3_LM_HEAD_DIM as f32).sqrt();
    let mut parts = Vec::with_capacity(2);
    for b in 0..2 {
        let q_b = gpu_slice_rows(&q_t, b * tokens, tokens).map_err(DiffusionError::model)?;
        let k_b = gpu_slice_rows(&k_full, b * tokens, tokens).map_err(DiffusionError::model)?;
        let v_b = gpu_slice_rows(&v_full, b * tokens, tokens).map_err(DiffusionError::model)?;
        parts.push(music3_lm_prefill_attn(&q_b, &k_b, &v_b, scale)?);
    }
    let mut attn = parts.remove(0);
    for part in parts {
        attn = gpu_concat_rows(&attn, &part).map_err(DiffusionError::model)?;
    }
    let out = linear_cached(
        weights,
        &attn,
        "model.layers.0.self_attn.o_proj.weight",
        MUSIC3_LM_HIDDEN,
    )?;
    download_mat(&out)
}

/// Official-input decode attn: last-token Q `[2, q_inner]` + KV `[2*seq, kv_inner]`
/// → pre-`o_proj` or post-`o_proj` `[2, hidden]`. Uses the same GQA decode
/// kernel as `music3_lm_decode_attn`.
pub fn music3_decode_attn_from_qkv(
    weights: &Music3Shards,
    q: &[f32],
    k: &[f32],
    v: &[f32],
    seq: usize,
    with_o_proj: bool,
) -> Result<Vec<f32>> {
    let q_inner = MUSIC3_LM_HEADS * MUSIC3_LM_HEAD_DIM;
    let kv_inner = MUSIC3_LM_KV_HEADS * MUSIC3_LM_HEAD_DIM;
    if seq == 0 || q.len() != 2 * q_inner || k.len() != 2 * seq * kv_inner || v.len() != 2 * seq * kv_inner
    {
        return Err(DiffusionError::model(format!(
            "decode attn qkv shape q={} k={} v={} seq={seq}",
            q.len(),
            k.len(),
            v.len()
        )));
    }
    let q_t = gpu_upload(q, 2, q_inner).map_err(DiffusionError::model)?;
    let k_t = gpu_upload(k, 2 * seq, kv_inner).map_err(DiffusionError::model)?;
    let v_t = gpu_upload(v, 2 * seq, kv_inner).map_err(DiffusionError::model)?;
    let scale = 1.0 / (MUSIC3_LM_HEAD_DIM as f32).sqrt();
    let attn = music3_lm_decode_attn(&q_t, &k_t, &v_t, scale)?;
    if !with_o_proj {
        return download_mat(&attn);
    }
    let out = linear_cached(
        weights,
        &attn,
        "model.layers.0.self_attn.o_proj.weight",
        MUSIC3_LM_HIDDEN,
    )?;
    download_mat(&out)
}

pub struct Music3LmPrepared {
    input_norm: Vec<Vec<f32>>,
    post_attn_norm: Vec<Vec<f32>>,
    q_norm: Vec<Vec<f32>>,
    k_norm: Vec<Vec<f32>>,
    final_norm: Vec<f32>,
    rope_inv_freq: Vec<f32>,
}

impl Music3LmPrepared {
    pub fn prepare(weights: &Music3Shards) -> Result<Self> {
        let mut input_norm = Vec::with_capacity(MUSIC3_LM_LAYERS);
        let mut post_attn_norm = Vec::with_capacity(MUSIC3_LM_LAYERS);
        let mut q_norm = Vec::with_capacity(MUSIC3_LM_LAYERS);
        let mut k_norm = Vec::with_capacity(MUSIC3_LM_LAYERS);
        for layer in 0..MUSIC3_LM_LAYERS {
            let prefix = layer_prefix(layer);
            input_norm.push(weights.tensor_f32(&format!("{prefix}.input_layernorm.weight"))?);
            post_attn_norm
                .push(weights.tensor_f32(&format!("{prefix}.post_attention_layernorm.weight"))?);
            q_norm.push(weights.tensor_f32(&format!("{prefix}.self_attn.q_norm.weight"))?);
            k_norm.push(weights.tensor_f32(&format!("{prefix}.self_attn.k_norm.weight"))?);
        }
        let final_norm = weights.tensor_f32("model.norm.weight")?;
        let half = MUSIC3_LM_HEAD_DIM / 2;
        let mut rope_inv_freq = Vec::with_capacity(half);
        for j in 0..half {
            rope_inv_freq
                .push(1.0 / MUSIC3_LM_ROPE_THETA.powf(2.0 * j as f32 / MUSIC3_LM_HEAD_DIM as f32));
        }
        Ok(Self {
            input_norm,
            post_attn_norm,
            q_norm,
            k_norm,
            final_norm,
            rope_inv_freq,
        })
    }
}

struct LayerOut {
    hidden: GpuTensor,
    key: GpuTensor,
    value: GpuTensor,
}

/// Official Music3 LM is bf16 SDPA/FlashAttention. Default is causal FA2
/// with bf16 tensor-core operands (`MAKEPAD_MUSIC3_ATTN=fa2bf16`).
/// `fa2` is the same kernel in f16; `composite` is the old cuBLAS path.
fn music3_lm_prefill_attn(
    q: &GpuTensor,
    k: &GpuTensor,
    v: &GpuTensor,
    scale: f32,
) -> Result<GpuTensor> {
    let kind = std::env::var("MAKEPAD_MUSIC3_ATTN").unwrap_or_else(|_| {
        if official_py() {
            "math".into()
        } else {
            String::new()
        }
    });
    let attn = match kind.as_str() {
        "math" | "sdpa" => {
            // Official dump60 SDPA is torch MATH (Flash not compiled in 169 venv).
            gpu_attention_packed_causal_f32(q, k, v, MUSIC3_LM_HEADS, scale)
        }
        "composite" | "0" => {
            gpu_attention_packed_causal(q, k, v, MUSIC3_LM_HEADS, scale)
        }
        "fa2" | "f16" => {
            gpu_attention_packed_causal_flash(q, k, v, MUSIC3_LM_HEADS, scale, false)
        }
        _ => gpu_attention_packed_causal_flash(q, k, v, MUSIC3_LM_HEADS, scale, true),
    };
    attn.map_err(DiffusionError::model)
}

fn music3_cast_hidden(x: GpuTensor) -> Result<GpuTensor> {
    if official_py() {
        return gpu_bf16_round(&x).map_err(DiffusionError::model);
    }
    Ok(x)
}

fn music3_kv_cache(x: GpuTensor) -> Result<GpuTensor> {
    if official_py() {
        return gpu_bf16_round(&x).map_err(DiffusionError::model);
    }
    Ok(x)
}

fn music3_rope(
    x: GpuTensor,
    heads: usize,
    half: usize,
    cos: &GpuTensor,
    sin: &GpuTensor,
) -> Result<GpuTensor> {
    if official_py() {
        return gpu_rope_half_bf16(&x, heads, half, cos, sin).map_err(DiffusionError::model);
    }
    gpu_rope_half(&x, heads, half, cos, sin).map_err(DiffusionError::model)
}

fn music3_qk_rms(x: GpuTensor, cache_key: String, scale: &[f32]) -> Result<GpuTensor> {
    if qk_official() {
        return gpu_rms_norm_qwen3(
            &x,
            MUSIC3_LM_HEAD_DIM,
            MUSIC3_LM_NAMESPACE,
            &cache_key,
            scale,
            MUSIC3_LM_RMS_EPS,
        )
        .map_err(DiffusionError::model);
    }
    gpu_rms_norm_mul(
        &x,
        MUSIC3_LM_HEAD_DIM,
        MUSIC3_LM_NAMESPACE,
        &cache_key,
        scale,
        MUSIC3_LM_RMS_EPS,
    )
    .map_err(DiffusionError::model)
}

fn music3_hidden_rms(x: &GpuTensor, cache_key: String, scale: &[f32]) -> Result<GpuTensor> {
    if official_py() {
        return gpu_rms_norm_qwen3(
            x,
            MUSIC3_LM_HIDDEN,
            MUSIC3_LM_NAMESPACE,
            &cache_key,
            scale,
            MUSIC3_LM_RMS_EPS,
        )
        .map_err(DiffusionError::model);
    }
    gpu_rms_norm_mul(
        x,
        MUSIC3_LM_HIDDEN,
        MUSIC3_LM_NAMESPACE,
        &cache_key,
        scale,
        MUSIC3_LM_RMS_EPS,
    )
    .map_err(DiffusionError::model)
}

fn music3_repeat_kv(x: &GpuTensor) -> Result<GpuTensor> {
    let mut heads = Vec::with_capacity(MUSIC3_LM_KV_HEADS);
    for head in 0..MUSIC3_LM_KV_HEADS {
        heads.push(
            gpu_slice_cols(x, head * MUSIC3_LM_HEAD_DIM, MUSIC3_LM_HEAD_DIM)
                .map_err(DiffusionError::model)?,
        );
    }
    let group = MUSIC3_LM_HEADS / MUSIC3_LM_KV_HEADS;
    let mut refs = Vec::with_capacity(MUSIC3_LM_HEADS);
    for head in 0..MUSIC3_LM_KV_HEADS {
        for _ in 0..group {
            refs.push(&heads[head]);
        }
    }
    gpu_concat_cols(&refs).map_err(DiffusionError::model)
}

/// One-token decode attention. Official SDPA flash-decode is q_len=1 over the
/// KV cache; the GQA decode kernel is the measured token-best winner (RVQ
/// rows 0..11 exact; FA2 decode moved the fork earlier and was rejected).
fn music3_lm_decode_attn(
    q: &GpuTensor,
    k: &GpuTensor,
    v: &GpuTensor,
    scale: f32,
) -> Result<GpuTensor> {
    gpu_attention_gqa_decode_bf16(q, k, v, MUSIC3_LM_HEADS, MUSIC3_LM_KV_HEADS, scale)
        .map_err(DiffusionError::model)
}

fn layer_forward(
    weights: &Music3Shards,
    prepared: &Music3LmPrepared,
    layer: usize,
    hidden: GpuTensor,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
    tokens: usize,
    batch: usize,
) -> Result<LayerOut> {
    let n = tokens.saturating_mul(batch.max(1));
    let half = MUSIC3_LM_HEAD_DIM / 2;
    let q_inner = MUSIC3_LM_HEADS * MUSIC3_LM_HEAD_DIM;
    let kv_inner = MUSIC3_LM_KV_HEADS * MUSIC3_LM_HEAD_DIM;
    let scale = 1.0 / (MUSIC3_LM_HEAD_DIM as f32).sqrt();
    let prefix = layer_prefix(layer);
    let mut hidden = hidden;

    let normed = music3_hidden_rms(
        &hidden,
        format!("{prefix}.input_layernorm"),
        &prepared.input_norm[layer],
    )?;

    let q_name = format!("{prefix}.self_attn.q_proj.weight");
    let k_name = format!("{prefix}.self_attn.k_proj.weight");
    let v_name = format!("{prefix}.self_attn.v_proj.weight");
    if layer == 0 {
        if let Ok(dir) = std::env::var("MAKEPAD_MUSIC3_DUMP_L0_QKV") {
            if let Ok(nh) = gpu_download(&normed) {
                let last_rows = |data: &[f32], cols: usize| -> Vec<f32> {
                    let mut out = Vec::with_capacity(batch.max(1) * cols);
                    for b in 0..batch.max(1) {
                        let row = (b + 1) * tokens - 1;
                        let s = row * cols;
                        out.extend_from_slice(&data[s..s + cols]);
                    }
                    out
                };
                let _ = write_npy_f32_lm(
                    &format!("{dir}/native_l0_norm.npy"),
                    &last_rows(&nh, MUSIC3_LM_HIDDEN),
                    &[batch.max(1), MUSIC3_LM_HIDDEN],
                );
            }
        }
    }
    let q = linear_qk(weights, &normed, &q_name, q_inner)?;
    let k = linear_qk(weights, &normed, &k_name, kv_inner)?;
    let v = linear_cached(weights, &normed, &v_name, kv_inner)?;
    drop(normed);
    if layer == 0 {
        dump_fullseq_if(&q, tokens, batch, "native_fullseq_L0_qproj.npy");
        dump_fullseq_if(&k, tokens, batch, "native_fullseq_L0_kproj.npy");
    }
    if layer == 0 {
        if let Ok(dir) = std::env::var("MAKEPAD_MUSIC3_DUMP_L0_QKV") {
            if let (Ok(qp), Ok(kp)) = (gpu_download(&q), gpu_download(&k)) {
                let last_rows = |data: &[f32], cols: usize| -> Vec<f32> {
                    let mut out = Vec::with_capacity(batch.max(1) * cols);
                    for b in 0..batch.max(1) {
                        let row = (b + 1) * tokens - 1;
                        let s = row * cols;
                        out.extend_from_slice(&data[s..s + cols]);
                    }
                    out
                };
                let _ = write_npy_f32_lm(
                    &format!("{dir}/native_l0_qproj.npy"),
                    &last_rows(&qp, q_inner),
                    &[batch.max(1), q_inner],
                );
                let _ = write_npy_f32_lm(
                    &format!("{dir}/native_l0_kproj.npy"),
                    &last_rows(&kp, kv_inner),
                    &[batch.max(1), kv_inner],
                );
                let _ = write_npy_f32_lm(
                    &format!("{dir}/native_l0_qnorm_w.npy"),
                    &prepared.q_norm[layer],
                    &[prepared.q_norm[layer].len()],
                );
                let _ = write_npy_f32_lm(
                    &format!("{dir}/native_l0_knorm_w.npy"),
                    &prepared.k_norm[layer],
                    &[prepared.k_norm[layer].len()],
                );
            }
        }
    }

    // Official Qwen3RMSNorm returns bf16: weight * (x*rsqrt).to(bf16).
    // Precise f32 RMS is correct (GPU==CPU 8e-6) but last-token knorm
    // maxabs 0.244 is one bf16 ULP at |x|~92.5; attn must see bf16 q/k.
    let q = music3_qk_rms(
        q,
        format!("{prefix}.self_attn.q_norm"),
        &prepared.q_norm[layer],
    )?;
    let k = music3_qk_rms(
        k,
        format!("{prefix}.self_attn.k_norm"),
        &prepared.k_norm[layer],
    )?;
    if layer == 0 {
        dump_fullseq_if(&q, tokens, batch, "native_fullseq_L0_qnorm.npy");
        dump_fullseq_if(&k, tokens, batch, "native_fullseq_L0_knorm.npy");
    }
    if layer == 0 {
        if let Ok(dir) = std::env::var("MAKEPAD_MUSIC3_DUMP_L0_QKV") {
            if let (Ok(qh), Ok(kh), Ok(vh)) =
                (gpu_download(&q), gpu_download(&k), gpu_download(&v))
            {
                let last_rows = |data: &[f32], cols: usize| -> Vec<f32> {
                    let mut out = Vec::with_capacity(batch.max(1) * cols);
                    for b in 0..batch.max(1) {
                        let row = (b + 1) * tokens - 1;
                        let s = row * cols;
                        out.extend_from_slice(&data[s..s + cols]);
                    }
                    out
                };
                let _ = write_npy_f32_lm(
                    &format!("{dir}/native_l0_qnorm.npy"),
                    &last_rows(&qh, q_inner),
                    &[batch.max(1), q_inner],
                );
                let _ = write_npy_f32_lm(
                    &format!("{dir}/native_l0_knorm.npy"),
                    &last_rows(&kh, kv_inner),
                    &[batch.max(1), kv_inner],
                );
                let _ = write_npy_f32_lm(
                    &format!("{dir}/native_l0_vproj.npy"),
                    &last_rows(&vh, kv_inner),
                    &[batch.max(1), kv_inner],
                );
            }
        }
    }
    let q = music3_rope(q, MUSIC3_LM_HEADS, half, rope_cos, rope_sin)?;
    let k = music3_rope(k, MUSIC3_LM_KV_HEADS, half, rope_cos, rope_sin)?;
    if layer == 0 {
        dump_l0_last_rows(&q, tokens, batch, q_inner, "native_l0_qrope.npy");
        dump_l0_last_rows(&k, tokens, batch, kv_inner, "native_l0_krope.npy");
        dump_fullseq_if(&q, tokens, batch, "native_fullseq_L0_qrope.npy");
        dump_fullseq_if(&k, tokens, batch, "native_fullseq_L0_krope.npy");
        dump_fullseq_if(&v, tokens, batch, "native_fullseq_L0_v.npy");
    }

    let mut k_heads = Vec::with_capacity(MUSIC3_LM_KV_HEADS);
    let mut v_heads = Vec::with_capacity(MUSIC3_LM_KV_HEADS);
    for head in 0..MUSIC3_LM_KV_HEADS {
        k_heads.push(
            gpu_slice_cols(&k, head * MUSIC3_LM_HEAD_DIM, MUSIC3_LM_HEAD_DIM)
                .map_err(DiffusionError::model)?,
        );
        v_heads.push(
            gpu_slice_cols(&v, head * MUSIC3_LM_HEAD_DIM, MUSIC3_LM_HEAD_DIM)
                .map_err(DiffusionError::model)?,
        );
    }
    let group = MUSIC3_LM_HEADS / MUSIC3_LM_KV_HEADS;
    let mut k_refs = Vec::with_capacity(MUSIC3_LM_HEADS);
    let mut v_refs = Vec::with_capacity(MUSIC3_LM_HEADS);
    for head in 0..MUSIC3_LM_KV_HEADS {
        for _ in 0..group {
            k_refs.push(&k_heads[head]);
            v_refs.push(&v_heads[head]);
        }
    }
    let k_full = gpu_concat_cols(&k_refs).map_err(DiffusionError::model)?;
    let v_full = gpu_concat_cols(&v_refs).map_err(DiffusionError::model)?;
    drop((k_heads, v_heads));

    let attn = if batch <= 1 {
        music3_lm_prefill_attn(&q, &k_full, &v_full, scale)?
    } else {
        // Official CFG prefill is two independent sequences. GEMM is
        // batched [2T, H]; causal attn must not cross the pair.
        let mut parts = Vec::with_capacity(batch);
        for b in 0..batch {
            let q_b = gpu_slice_rows(&q, b * tokens, tokens).map_err(DiffusionError::model)?;
            let k_b = gpu_slice_rows(&k_full, b * tokens, tokens).map_err(DiffusionError::model)?;
            let v_b = gpu_slice_rows(&v_full, b * tokens, tokens).map_err(DiffusionError::model)?;
            parts.push(music3_lm_prefill_attn(&q_b, &k_b, &v_b, scale)?);
        }
        let mut joined = parts.remove(0);
        for part in parts {
            joined = gpu_concat_rows(&joined, &part).map_err(DiffusionError::model)?;
        }
        joined
    };
    drop((q, k_full, v_full));
    if layer == 0 {
        dump_l0_last_rows(&attn, tokens, batch, q_inner, "native_l0_attn_out.npy");
    }
    let attn = linear_cached(
        weights,
        &attn,
        &format!("{prefix}.self_attn.o_proj.weight"),
        MUSIC3_LM_HIDDEN,
    )?;
    if layer == 0 {
        dump_l0_last_rows(&attn, tokens, batch, MUSIC3_LM_HIDDEN, "native_l0_oproj.npy");
        // Official self_attn hook is post-o_proj [B,T,H]; last-token dumps stay last-token.
        dump_fullseq_if(&attn, tokens, batch, "native_fullseq_L0_attn.npy");
    }
    if layer == 1 {
        dump_layer1_if(&attn, tokens, batch, "native_offin_L1_attn.npy");
    }
    if layer == 10 || layer == 20 || layer == 35 {
        dump_fullseq_if(
            &attn,
            tokens,
            batch,
            &format!("native_fullseq_L{layer}_attn.npy"),
        );
    }
    hidden = residual_add(&hidden, &attn)?;
    hidden = music3_cast_hidden(hidden)?;
    if layer == 0 {
        dump_l0_last_rows(
            &hidden,
            tokens,
            batch,
            MUSIC3_LM_HIDDEN,
            "native_l0_attn_resid.npy",
        );
    }
    if layer == 1 {
        dump_layer1_if(&hidden, tokens, batch, "native_offin_L1_attn_resid.npy");
    }
    drop(attn);

    let normed = music3_hidden_rms(
        &hidden,
        format!("{prefix}.post_attention_layernorm"),
        &prepared.post_attn_norm[layer],
    )?;
    if layer == 0 {
        dump_l0_last_rows(
            &normed,
            tokens,
            batch,
            MUSIC3_LM_HIDDEN,
            "native_l0_post_norm.npy",
        );
    }
    let up_name = format!("{prefix}.mlp.up_proj.weight");
    let gate_name = format!("{prefix}.mlp.gate_proj.weight");
    let up = linear_cached(weights, &normed, &up_name, MUSIC3_LM_FF)?;
    let gate = linear_cached(weights, &normed, &gate_name, MUSIC3_LM_FF)?;
    drop(normed);
    if layer == 0 {
        dump_l0_last_rows(&up, tokens, batch, MUSIC3_LM_FF, "native_l0_up.npy");
        dump_l0_last_rows(&gate, tokens, batch, MUSIC3_LM_FF, "native_l0_gate.npy");
    }
    let up_gate = gpu_concat_cols(&[&up, &gate]).map_err(DiffusionError::model)?;
    let ff = gpu_swiglu_value_gate(&up_gate).map_err(DiffusionError::model)?;
    if layer == 0 {
        dump_l0_last_rows(&ff, tokens, batch, MUSIC3_LM_FF, "native_l0_swiglu.npy");
    }
    drop(up_gate);
    let ff = down_proj_cached(
        weights,
        &ff,
        &format!("{prefix}.mlp.down_proj.weight"),
        MUSIC3_LM_HIDDEN,
    )?;
    if layer == 0 {
        dump_l0_last_rows(&ff, tokens, batch, MUSIC3_LM_HIDDEN, "native_l0_down.npy");
    }
    hidden = residual_add(&hidden, &ff)?;
    hidden = music3_cast_hidden(hidden)?;
    if layer == 0 || layer == 10 || layer == 20 || layer == 35 {
        dump_fullseq_if(
            &hidden,
            tokens,
            batch,
            &format!("native_fullseq_L{layer}.npy"),
        );
    }
    let k = music3_kv_cache(k)?;
    let v = music3_kv_cache(v)?;
    Ok(LayerOut { hidden, key: k, value: v })
}

/// One sequence: token ids -> last_hidden `[T, 4096]` after final RMSNorm.
pub fn music3_lm_prefill(
    weights: &Music3Shards,
    prepared: &Music3LmPrepared,
    token_ids: &[u32],
) -> Result<Vec<f32>> {
    if !gpu_device_available() {
        return Err(DiffusionError::model(
            "music3 LM prefill needs CUDA (gpu_device_available=false)",
        ));
    }
    let n = token_ids.len();
    if n == 0 {
        return Err(DiffusionError::workflow("music3 LM prefill: empty ids"));
    }
    let mut embeds = vec![0.0f32; n * MUSIC3_LM_HIDDEN];
    for (row, id) in token_ids.iter().enumerate() {
        let values = weights.tensor_row_f32("model.embed_tokens.weight", *id as u64)?;
        if values.len() != MUSIC3_LM_HIDDEN {
            return Err(DiffusionError::model(format!(
                "music3 embed row {id} width {}",
                values.len()
            )));
        }
        embeds[row * MUSIC3_LM_HIDDEN..(row + 1) * MUSIC3_LM_HIDDEN].copy_from_slice(&values);
    }
    let mut hidden = gpu_upload(&embeds, n, MUSIC3_LM_HIDDEN).map_err(DiffusionError::model)?;

    let half = MUSIC3_LM_HEAD_DIM / 2;
    let mut cos = vec![0.0f32; n * half];
    let mut sin = vec![0.0f32; n * half];
    for pos in 0..n {
        for j in 0..half {
            let angle = pos as f32 * prepared.rope_inv_freq[j];
            cos[pos * half + j] = angle.cos();
            sin[pos * half + j] = angle.sin();
        }
    }
    let rope_cos = gpu_upload(&cos, n, half).map_err(DiffusionError::model)?;
    let rope_sin = gpu_upload(&sin, n, half).map_err(DiffusionError::model)?;

    for layer in 0..MUSIC3_LM_LAYERS {
        let out = layer_forward(weights, prepared, layer, hidden, &rope_cos, &rope_sin, n, 1)?;
        hidden = out.hidden;
    }

    hidden = music3_hidden_rms(&hidden, "model.norm".to_string(), &prepared.final_norm)?;
    gpu_download(&hidden).map_err(DiffusionError::model)
}

/// `lm_head` on the last token of a `[T, H]` prefill hidden. Returns `[vocab]`.
pub fn music3_lm_head_last(
    weights: &Music3Shards,
    hidden: &[f32],
    tokens: usize,
) -> Result<Vec<f32>> {
    if hidden.len() != tokens * MUSIC3_LM_HIDDEN || tokens == 0 {
        return Err(DiffusionError::model(format!(
            "music3 lm_head: hidden {} for {tokens} tokens",
            hidden.len()
        )));
    }
    let last = &hidden[(tokens - 1) * MUSIC3_LM_HIDDEN..];
    let x = gpu_upload(last, 1, MUSIC3_LM_HIDDEN).map_err(DiffusionError::model)?;
    // Official mm lm_head left last_hidden unchanged and widened f12
    // logit gap 0.077→0.125; tokens still sem[15]/RVQ[12,2]. Revert.
    let part = ensure_linear(weights, "lm_head.weight", MUSIC3_LM_VOCAB, MUSIC3_LM_HIDDEN, 1)?;
    let y = if gpu_quant_linear_type_supported(part.bt_ggml_type) {
        gpu_linear_nt_cached(&x, MUSIC3_LM_NAMESPACE, &[part], &[])
    } else {
        gpu_linear_nt_cached_bf16_f32acc(&x, MUSIC3_LM_NAMESPACE, &[part], &[])
    }
    .map_err(DiffusionError::model)?;
    gpu_download(&y).map_err(DiffusionError::model)
}

/// AR sampler: GEMM only audio_end + semantic codebook (not the 200k vocab).
pub fn music3_lm_head_audio(
    weights: &Music3Shards,
    hidden: &GpuTensor,
) -> Result<(f32, Vec<f32>)> {
    use crate::music3::{MUSIC3_AUDIO_CODE_OFFSET, MUSIC3_AUDIO_END_TOKEN_ID, MUSIC3_SEMANTIC_VOCAB};
    if hidden.rows() != 1 || hidden.cols() != MUSIC3_LM_HIDDEN {
        return Err(DiffusionError::model("music3 lm_head_audio hidden"));
    }
    let lo = MUSIC3_AUDIO_END_TOKEN_ID as usize;
    let n = (MUSIC3_AUDIO_CODE_OFFSET as usize + MUSIC3_SEMANTIC_VOCAB) - lo;
    const AUDIO_HEAD: &str = "lm_head.weight.audio";
    let ggml_type = weights.linear_ggml_type("lm_head.weight");
    let quant = gpu_quant_linear_type_supported(ggml_type);
    let load = || {
        weights
            .tensor_row_range_bytes("lm_head.weight", lo as u64, n as u64)
            .map_err(|err| err.to_string())
    };
    if quant {
        // Q4_0 rows are self-contained block runs, so the head slice stays
        // packed on device just like the full linears.
        gpu_weight_cache_ensure_quant(MUSIC3_LM_NAMESPACE, AUDIO_HEAD, ggml_type, n, MUSIC3_LM_HIDDEN, load)
    } else {
        gpu_weight_cache_ensure(MUSIC3_LM_NAMESPACE, AUDIO_HEAD, ggml_type, n, MUSIC3_LM_HIDDEN, false, load)
    }
    .map_err(DiffusionError::model)?;
    let part = GpuLinearPart {
        bt_ggml_type: ggml_type,
        n,
        cache_key: AUDIO_HEAD,
        bytes: &[],
    };
    let y = if quant {
        gpu_linear_nt_cached(hidden, MUSIC3_LM_NAMESPACE, &[part], &[])
    } else {
        gpu_linear_nt_cached_bf16_f32acc(hidden, MUSIC3_LM_NAMESPACE, &[part], &[])
    }
    .map_err(DiffusionError::model)?;
    let slim = gpu_download(&y).map_err(DiffusionError::model)?;
    let end = slim[0];
    let sem_off = MUSIC3_AUDIO_CODE_OFFSET as usize - lo;
    Ok((end, slim[sem_off..sem_off + MUSIC3_SEMANTIC_VOCAB].to_vec()))
}

/// `music3_lm_head_audio` on a host `[H]` hidden (uploads one row).
pub fn music3_lm_head_audio_host(
    weights: &Music3Shards,
    hidden: &[f32],
) -> Result<(f32, Vec<f32>)> {
    if hidden.len() != MUSIC3_LM_HIDDEN {
        return Err(DiffusionError::model("music3 lm_head_audio host width"));
    }
    let x = gpu_upload(hidden, 1, MUSIC3_LM_HIDDEN).map_err(DiffusionError::model)?;
    music3_lm_head_audio(weights, &x)
}

/// Prefill cond+uncond rows independently (no cross-batch attention) and
/// stack to `[2, T, H]` + last-token logits `[2, V]`.
pub fn music3_lm_prefill_pair(
    weights: &Music3Shards,
    prepared: &Music3LmPrepared,
    cond_ids: &[u32],
    uncond_ids: &[u32],
) -> Result<(Vec<f32>, Vec<f32>)> {
    if cond_ids.len() != uncond_ids.len() {
        return Err(DiffusionError::model("music3 CFG pair length mismatch"));
    }
    let t = cond_ids.len();
    let h_cond = music3_lm_prefill(weights, prepared, cond_ids)?;
    let h_uncond = music3_lm_prefill(weights, prepared, uncond_ids)?;
    let mut hidden = Vec::with_capacity(2 * t * MUSIC3_LM_HIDDEN);
    hidden.extend_from_slice(&h_cond);
    hidden.extend_from_slice(&h_uncond);
    let mut logits = Vec::with_capacity(2 * MUSIC3_LM_VOCAB);
    logits.extend(music3_lm_head_last(weights, &h_cond, t)?);
    logits.extend(music3_lm_head_last(weights, &h_uncond, t)?);
    Ok((hidden, logits))
}

pub fn music3_lm_evict() -> Result<usize> {
    gpu_weight_cache_evict_prefix(&format!("{MUSIC3_LM_NAMESPACE}::")).map_err(DiffusionError::model)
}

/// Device KV cache: un-repeated GQA K/V per layer, `[seq, kv_heads * head_dim]`.
///
/// `k`/`v` tensors may be longer than `seq` when [`Music3LmSession::reserve`]
/// has padded a tail. Decode writes the new row in place so a 3-minute song
/// does not recopy the whole cache every frame (that path is O(n²) traffic).
pub struct Music3LmSession {
    k: Vec<GpuTensor>,
    v: Vec<GpuTensor>,
    pub seq: usize,
}

impl Music3LmSession {
    /// Prefill one sequence; returns last-token hidden `[H]` after final RMSNorm.
    pub fn prefill(
        weights: &Music3Shards,
        prepared: &Music3LmPrepared,
        token_ids: &[u32],
    ) -> Result<(Self, Vec<f32>)> {
        Self::prefill_with_progress(weights, prepared, token_ids, &mut |_, _| {})
    }

    pub fn prefill_with_progress(
        weights: &Music3Shards,
        prepared: &Music3LmPrepared,
        token_ids: &[u32],
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<(Self, Vec<f32>)> {
        if !gpu_device_available() {
            return Err(DiffusionError::model("music3 LM session needs CUDA"));
        }
        let n = token_ids.len();
        if n == 0 {
            return Err(DiffusionError::workflow("music3 LM prefill: empty ids"));
        }
        let mut embeds = vec![0.0f32; n * MUSIC3_LM_HIDDEN];
        for (row, id) in token_ids.iter().enumerate() {
            let values = weights.tensor_row_f32("model.embed_tokens.weight", *id as u64)?;
            embeds[row * MUSIC3_LM_HIDDEN..(row + 1) * MUSIC3_LM_HIDDEN].copy_from_slice(&values);
        }
        let mut hidden = gpu_upload(&embeds, n, MUSIC3_LM_HIDDEN).map_err(DiffusionError::model)?;
        let (rope_cos, rope_sin) = rope_range(prepared, 0, n)?;
        let mut k = Vec::with_capacity(MUSIC3_LM_LAYERS);
        let mut v = Vec::with_capacity(MUSIC3_LM_LAYERS);
        progress(0, MUSIC3_LM_LAYERS);
        for layer in 0..MUSIC3_LM_LAYERS {
            let out = layer_forward(weights, prepared, layer, hidden, &rope_cos, &rope_sin, n, 1)?;
            hidden = out.hidden;
            k.push(out.key);
            v.push(out.value);
            progress(layer + 1, MUSIC3_LM_LAYERS);
        }
        hidden = music3_hidden_rms(&hidden, "model.norm".to_string(), &prepared.final_norm)?;
        let last = gpu_slice_rows(&hidden, n - 1, 1).map_err(DiffusionError::model)?;
        let last = gpu_download(&last).map_err(DiffusionError::model)?;
        Ok((Self { k, v, seq: n }, last))
    }

    /// Official CFG prefill: one 8B stack over `[2, T]` (GEMMs batched,
    /// causal attn split so the pair cannot see each other).
    pub fn prefill_pair_with_progress(
        weights: &Music3Shards,
        prepared: &Music3LmPrepared,
        cond_ids: &[u32],
        uncond_ids: &[u32],
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<(Self, Vec<f32>, Self, Vec<f32>)> {
        if cond_ids.len() != uncond_ids.len() || cond_ids.is_empty() {
            return Err(DiffusionError::model("music3 CFG prefill pair length"));
        }
        if !gpu_device_available() {
            return Err(DiffusionError::model("music3 LM session needs CUDA"));
        }
        let t = cond_ids.len();
        let mut embeds = vec![0.0f32; 2 * t * MUSIC3_LM_HIDDEN];
        for (row, id) in cond_ids.iter().chain(uncond_ids.iter()).enumerate() {
            let values = weights.tensor_row_f32("model.embed_tokens.weight", *id as u64)?;
            embeds[row * MUSIC3_LM_HIDDEN..(row + 1) * MUSIC3_LM_HIDDEN].copy_from_slice(&values);
        }
        let mut hidden = gpu_upload(&embeds, 2 * t, MUSIC3_LM_HIDDEN).map_err(DiffusionError::model)?;
        let dump_dir = std::env::var("MAKEPAD_MUSIC3_DUMP_PREFILL_LAYERS").ok();
        if let Some(dir) = dump_dir.as_deref() {
            dump_prefill_last_pair(&hidden, t, dir, "native_prefill_embed.npy");
        }
        let fullseq_dir = std::env::var("MAKEPAD_MUSIC3_DUMP_FULLSEQ").ok();
        if let Some(dir) = fullseq_dir.as_deref() {
            dump_fullseq_pair(&hidden, t, dir, "native_fullseq_embed.npy");
        }
        let (rope_one_c, rope_one_s) = rope_range(prepared, 0, t)?;
        let rope_cos = gpu_concat_rows(&rope_one_c, &rope_one_c).map_err(DiffusionError::model)?;
        let rope_sin = gpu_concat_rows(&rope_one_s, &rope_one_s).map_err(DiffusionError::model)?;
        let mut k_cond = Vec::with_capacity(MUSIC3_LM_LAYERS);
        let mut v_cond = Vec::with_capacity(MUSIC3_LM_LAYERS);
        let mut k_uncond = Vec::with_capacity(MUSIC3_LM_LAYERS);
        let mut v_uncond = Vec::with_capacity(MUSIC3_LM_LAYERS);
        progress(0, MUSIC3_LM_LAYERS);
        for layer in 0..MUSIC3_LM_LAYERS {
            let out = layer_forward(
                weights,
                prepared,
                layer,
                hidden,
                &rope_cos,
                &rope_sin,
                t,
                2,
            )?;
            hidden = out.hidden;
            if let Some(dir) = dump_dir.as_deref() {
                dump_prefill_last_pair(
                    &hidden,
                    t,
                    dir,
                    &format!("native_prefill_layer{layer}.npy"),
                );
            }
            if let Some(dir) = fullseq_dir.as_deref() {
                if layer == 1 || layer == 5 || layer == 10 || layer == 20 || layer == 35 {
                    dump_fullseq_pair(
                        &hidden,
                        t,
                        dir,
                        &format!("native_fullseq_L{layer}.npy"),
                    );
                }
                if layer + 1 == MUSIC3_LM_LAYERS {
                    dump_fullseq_pair(&out.key, t, dir, "native_fullseq_L35_k.npy");
                    dump_fullseq_pair(&out.value, t, dir, "native_fullseq_L35_v.npy");
                }
            }
            k_cond.push(gpu_slice_rows(&out.key, 0, t).map_err(DiffusionError::model)?);
            k_uncond.push(gpu_slice_rows(&out.key, t, t).map_err(DiffusionError::model)?);
            v_cond.push(gpu_slice_rows(&out.value, 0, t).map_err(DiffusionError::model)?);
            v_uncond.push(gpu_slice_rows(&out.value, t, t).map_err(DiffusionError::model)?);
            progress(layer + 1, MUSIC3_LM_LAYERS);
        }
        hidden = music3_hidden_rms(&hidden, "model.norm".to_string(), &prepared.final_norm)?;
        if let Some(dir) = dump_dir.as_deref() {
            dump_prefill_last_pair(&hidden, t, dir, "native_prefill_norm.npy");
            dump_prefill_last_pair(&hidden, t, dir, "native_last_hidden_f0.npy");
        }
        if let Some(dir) = fullseq_dir.as_deref() {
            dump_fullseq_pair(&hidden, t, dir, "native_fullseq_norm.npy");
            dump_fullseq_pair(&hidden, t, dir, "native_fullseq_last_hidden.npy");
        }
        let last_c = gpu_download(
            &gpu_slice_rows(&hidden, t - 1, 1).map_err(DiffusionError::model)?,
        )
        .map_err(DiffusionError::model)?;
        let last_u = gpu_download(
            &gpu_slice_rows(&hidden, 2 * t - 1, 1).map_err(DiffusionError::model)?,
        )
        .map_err(DiffusionError::model)?;
        Ok((
            Self {
                k: k_cond,
                v: v_cond,
                seq: t,
            },
            last_c,
            Self {
                k: k_uncond,
                v: v_uncond,
                seq: t,
            },
            last_u,
        ))
    }

    /// Grow each layer K/V to `seq + extra` rows so decode can append in place.
    pub fn reserve(&mut self, extra: usize) -> Result<()> {
        let cap = self.seq.saturating_add(extra).max(self.seq + 1);
        for i in 0..self.k.len() {
            self.k[i] = pad_kv_rows(&self.k[i], cap)?;
            self.v[i] = pad_kv_rows(&self.v[i], cap)?;
        }
        Ok(())
    }

    /// One new token from `inputs_embeds` `[H]` (not token ids). RoPE at `self.seq`.
    pub fn step_embeds(
        &mut self,
        weights: &Music3Shards,
        prepared: &Music3LmPrepared,
        embeds: &[f32],
    ) -> Result<Vec<f32>> {
        if embeds.len() != MUSIC3_LM_HIDDEN {
            return Err(DiffusionError::model(format!(
                "music3 decode embeds {} expected {MUSIC3_LM_HIDDEN}",
                embeds.len()
            )));
        }
        let pos = self.seq;
        let mut hidden = gpu_upload(embeds, 1, MUSIC3_LM_HIDDEN).map_err(DiffusionError::model)?;
        let (rope_cos, rope_sin) = rope_range(prepared, pos, 1)?;
        let scale = 1.0 / (MUSIC3_LM_HEAD_DIM as f32).sqrt();
        for layer in 0..MUSIC3_LM_LAYERS {
            hidden = decode_layer(
                weights,
                prepared,
                layer,
                hidden,
                &mut self.k[layer],
                &mut self.v[layer],
                &rope_cos,
                &rope_sin,
                scale,
                self.seq,
            )?;
        }
        hidden = music3_hidden_rms(&hidden, "model.norm".to_string(), &prepared.final_norm)?;
        self.seq += 1;
        gpu_download(&hidden).map_err(DiffusionError::model)
    }

    /// Official CFG decode: one 8B forward with two rows (cond, uncond).
    /// Same independent sequences as two `step_embeds` calls.
    pub fn step_embeds_pair(
        cond: &mut Self,
        uncond: &mut Self,
        weights: &Music3Shards,
        prepared: &Music3LmPrepared,
        cond_embeds: &[f32],
        uncond_embeds: &[f32],
    ) -> Result<(Vec<f32>, Vec<f32>)> {
        if cond.seq != uncond.seq {
            return Err(DiffusionError::model("music3 CFG pair seq mismatch"));
        }
        if cond_embeds.len() != MUSIC3_LM_HIDDEN || uncond_embeds.len() != MUSIC3_LM_HIDDEN {
            return Err(DiffusionError::model("music3 CFG pair embed width"));
        }
        let pos = cond.seq;
        let mut both = Vec::with_capacity(2 * MUSIC3_LM_HIDDEN);
        both.extend_from_slice(cond_embeds);
        both.extend_from_slice(uncond_embeds);
        let mut hidden = gpu_upload(&both, 2, MUSIC3_LM_HIDDEN).map_err(DiffusionError::model)?;
        let dump_f1 = want_f1_dump(pos);
        if dump_f1 {
            dump_f1_rows(&hidden, MUSIC3_LM_HIDDEN, "native_f1_embed.npy");
        }
        let (rope_one_c, rope_one_s) = rope_range(prepared, pos, 1)?;
        let rope_cos = gpu_concat_rows(&rope_one_c, &rope_one_c).map_err(DiffusionError::model)?;
        let rope_sin = gpu_concat_rows(&rope_one_s, &rope_one_s).map_err(DiffusionError::model)?;
        let scale = 1.0 / (MUSIC3_LM_HEAD_DIM as f32).sqrt();
        for layer in 0..MUSIC3_LM_LAYERS {
            hidden = decode_layer_pair(
                weights,
                prepared,
                layer,
                hidden,
                &mut cond.k[layer],
                &mut cond.v[layer],
                &mut uncond.k[layer],
                &mut uncond.v[layer],
                &rope_cos,
                &rope_sin,
                scale,
                pos,
            )?;
        }
        hidden = music3_hidden_rms(&hidden, "model.norm".to_string(), &prepared.final_norm)?;
        if dump_f1 {
            dump_f1_rows(&hidden, MUSIC3_LM_HIDDEN, "native_f1_model_norm.npy");
            dump_f1_rows(&hidden, MUSIC3_LM_HIDDEN, "native_f1_last_hidden.npy");
        }
        cond.seq += 1;
        uncond.seq += 1;
        let host = gpu_download(&hidden).map_err(DiffusionError::model)?;
        let (c, u) = host.split_at(MUSIC3_LM_HIDDEN);
        Ok((c.to_vec(), u.to_vec()))
    }
}

fn rope_range(
    prepared: &Music3LmPrepared,
    start: usize,
    len: usize,
) -> Result<(GpuTensor, GpuTensor)> {
    let half = MUSIC3_LM_HEAD_DIM / 2;
    let mut cos = vec![0.0f32; len * half];
    let mut sin = vec![0.0f32; len * half];
    for i in 0..len {
        let pos = (start + i) as f32;
        for j in 0..half {
            let angle = pos * prepared.rope_inv_freq[j];
            cos[i * half + j] = angle.cos();
            sin[i * half + j] = angle.sin();
        }
    }
    Ok((
        gpu_upload(&cos, len, half).map_err(DiffusionError::model)?,
        gpu_upload(&sin, len, half).map_err(DiffusionError::model)?,
    ))
}

fn pad_kv_rows(tensor: &GpuTensor, cap: usize) -> Result<GpuTensor> {
    if tensor.rows() >= cap {
        return Ok(gpu_slice_rows(tensor, 0, tensor.rows()).map_err(DiffusionError::model)?);
    }
    let pad_rows = cap - tensor.rows();
    let zeros = vec![0.0f32; pad_rows * tensor.cols()];
    let pad = gpu_upload(&zeros, pad_rows, tensor.cols()).map_err(DiffusionError::model)?;
    gpu_concat_rows(tensor, &pad).map_err(DiffusionError::model)
}

fn decode_layer(
    weights: &Music3Shards,
    prepared: &Music3LmPrepared,
    layer: usize,
    hidden: GpuTensor,
    k_cache: &mut GpuTensor,
    v_cache: &mut GpuTensor,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
    scale: f32,
    seq: usize,
) -> Result<GpuTensor> {
    let half = MUSIC3_LM_HEAD_DIM / 2;
    let q_inner = MUSIC3_LM_HEADS * MUSIC3_LM_HEAD_DIM;
    let kv_inner = MUSIC3_LM_KV_HEADS * MUSIC3_LM_HEAD_DIM;
    let prefix = layer_prefix(layer);
    let normed = music3_hidden_rms(
        &hidden,
        format!("{prefix}.input_layernorm"),
        &prepared.input_norm[layer],
    )?;
    let q_name = format!("{prefix}.self_attn.q_proj.weight");
    let k_name = format!("{prefix}.self_attn.k_proj.weight");
    let v_name = format!("{prefix}.self_attn.v_proj.weight");
    let q = linear_qk(weights, &normed, &q_name, q_inner)?;
    let k_step = linear_qk(weights, &normed, &k_name, kv_inner)?;
    let v_step = linear_cached(weights, &normed, &v_name, kv_inner)?;
    drop(normed);
    let q = music3_qk_rms(
        q,
        format!("{prefix}.self_attn.q_norm"),
        &prepared.q_norm[layer],
    )?;
    let k_step = music3_qk_rms(
        k_step,
        format!("{prefix}.self_attn.k_norm"),
        &prepared.k_norm[layer],
    )?;
    let q = music3_rope(q, MUSIC3_LM_HEADS, half, rope_cos, rope_sin)?;
    let k_step = music3_rope(k_step, MUSIC3_LM_KV_HEADS, half, rope_cos, rope_sin)?;
    let k_step = music3_kv_cache(k_step)?;
    let v_step = music3_kv_cache(v_step)?;
    let reserved = k_cache.rows() > seq && v_cache.rows() > seq;
    let (k, v) = if reserved {
        gpu_copy_into(&k_step, &gpu_slice_rows(k_cache, seq, 1).map_err(DiffusionError::model)?)
            .map_err(DiffusionError::model)?;
        gpu_copy_into(&v_step, &gpu_slice_rows(v_cache, seq, 1).map_err(DiffusionError::model)?)
            .map_err(DiffusionError::model)?;
        (
            gpu_slice_rows(k_cache, 0, seq + 1).map_err(DiffusionError::model)?,
            gpu_slice_rows(v_cache, 0, seq + 1).map_err(DiffusionError::model)?,
        )
    } else {
        (
            gpu_concat_rows(k_cache, &k_step).map_err(DiffusionError::model)?,
            gpu_concat_rows(v_cache, &v_step).map_err(DiffusionError::model)?,
        )
    };
    drop((k_step, v_step));
    let attn = music3_lm_decode_attn(&q, &k, &v, scale)?;
    drop(q);
    let attn = linear_cached(
        weights,
        &attn,
        &format!("{prefix}.self_attn.o_proj.weight"),
        MUSIC3_LM_HIDDEN,
    )?;
    let hidden = residual_add(&hidden, &attn)?;
    let hidden = music3_cast_hidden(hidden)?;
    drop(attn);
    let normed = music3_hidden_rms(
        &hidden,
        format!("{prefix}.post_attention_layernorm"),
        &prepared.post_attn_norm[layer],
    )?;
    let up_name = format!("{prefix}.mlp.up_proj.weight");
    let gate_name = format!("{prefix}.mlp.gate_proj.weight");
    let up = linear_cached(weights, &normed, &up_name, MUSIC3_LM_FF)?;
    let gate = linear_cached(weights, &normed, &gate_name, MUSIC3_LM_FF)?;
    drop(normed);
    let up_gate = gpu_concat_cols(&[&up, &gate]).map_err(DiffusionError::model)?;
    let ff = gpu_swiglu_value_gate(&up_gate).map_err(DiffusionError::model)?;
    drop(up_gate);
    let ff = down_proj_cached(
        weights,
        &ff,
        &format!("{prefix}.mlp.down_proj.weight"),
        MUSIC3_LM_HIDDEN,
    )?;
    let hidden = residual_add(&hidden, &ff)?;
    let hidden = music3_cast_hidden(hidden)?;
    if !reserved {
        *k_cache = k;
        *v_cache = v;
    }
    Ok(hidden)
}

fn decode_layer_pair(
    weights: &Music3Shards,
    prepared: &Music3LmPrepared,
    layer: usize,
    hidden: GpuTensor,
    k_cond: &mut GpuTensor,
    v_cond: &mut GpuTensor,
    k_uncond: &mut GpuTensor,
    v_uncond: &mut GpuTensor,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
    scale: f32,
    seq: usize,
) -> Result<GpuTensor> {
    if hidden.rows() != 2 {
        return Err(DiffusionError::model("music3 decode pair wants 2 rows"));
    }
    let half = MUSIC3_LM_HEAD_DIM / 2;
    let q_inner = MUSIC3_LM_HEADS * MUSIC3_LM_HEAD_DIM;
    let kv_inner = MUSIC3_LM_KV_HEADS * MUSIC3_LM_HEAD_DIM;
    let prefix = layer_prefix(layer);
    let dump = want_f1_dump(seq);
    if dump {
        if layer == 0 {
            eprintln!("f1 dump decode seq={seq} rows={}", hidden.rows());
        }
        dump_f1_rows(&hidden, MUSIC3_LM_HIDDEN, &format!("native_f1_L{layer}_in.npy"));
    }
    let normed = music3_hidden_rms(
        &hidden,
        format!("{prefix}.input_layernorm"),
        &prepared.input_norm[layer],
    )?;
    if dump {
        dump_f1_rows(
            &normed,
            MUSIC3_LM_HIDDEN,
            &format!("native_f1_L{layer}_norm.npy"),
        );
    }
    let q_name = format!("{prefix}.self_attn.q_proj.weight");
    let k_name = format!("{prefix}.self_attn.k_proj.weight");
    let v_name = format!("{prefix}.self_attn.v_proj.weight");
    let q = linear_qk(weights, &normed, &q_name, q_inner)?;
    let k_step = linear_qk(weights, &normed, &k_name, kv_inner)?;
    let v_step = linear_cached(weights, &normed, &v_name, kv_inner)?;
    drop(normed);
    if dump {
        dump_f1_rows(&q, q_inner, &format!("native_f1_L{layer}_qproj.npy"));
        dump_f1_rows(&k_step, kv_inner, &format!("native_f1_L{layer}_kproj.npy"));
        dump_f1_rows(&v_step, kv_inner, &format!("native_f1_L{layer}_vproj.npy"));
    }
    let q = music3_qk_rms(
        q,
        format!("{prefix}.self_attn.q_norm"),
        &prepared.q_norm[layer],
    )?;
    let k_step = music3_qk_rms(
        k_step,
        format!("{prefix}.self_attn.k_norm"),
        &prepared.k_norm[layer],
    )?;
    if dump {
        dump_f1_rows(&q, q_inner, &format!("native_f1_L{layer}_qnorm.npy"));
        dump_f1_rows(&k_step, kv_inner, &format!("native_f1_L{layer}_knorm.npy"));
    }
    let q = music3_rope(q, MUSIC3_LM_HEADS, half, rope_cos, rope_sin)?;
    let k_step = music3_rope(k_step, MUSIC3_LM_KV_HEADS, half, rope_cos, rope_sin)?;
    if dump {
        dump_f1_rows(&q, q_inner, &format!("native_f1_L{layer}_qrope.npy"));
        dump_f1_rows(&k_step, kv_inner, &format!("native_f1_L{layer}_krope.npy"));
    }
    let k_step = music3_kv_cache(k_step)?;
    let v_step = music3_kv_cache(v_step)?;
    let k0 = gpu_slice_rows(&k_step, 0, 1).map_err(DiffusionError::model)?;
    let k1 = gpu_slice_rows(&k_step, 1, 1).map_err(DiffusionError::model)?;
    let v0 = gpu_slice_rows(&v_step, 0, 1).map_err(DiffusionError::model)?;
    let v1 = gpu_slice_rows(&v_step, 1, 1).map_err(DiffusionError::model)?;
    if k_cond.rows() > seq && k_uncond.rows() > seq {
        gpu_copy_into(&k0, &gpu_slice_rows(k_cond, seq, 1).map_err(DiffusionError::model)?)
            .map_err(DiffusionError::model)?;
        gpu_copy_into(&k1, &gpu_slice_rows(k_uncond, seq, 1).map_err(DiffusionError::model)?)
            .map_err(DiffusionError::model)?;
        gpu_copy_into(&v0, &gpu_slice_rows(v_cond, seq, 1).map_err(DiffusionError::model)?)
            .map_err(DiffusionError::model)?;
        gpu_copy_into(&v1, &gpu_slice_rows(v_uncond, seq, 1).map_err(DiffusionError::model)?)
            .map_err(DiffusionError::model)?;
    } else {
        *k_cond = gpu_concat_rows(k_cond, &k0).map_err(DiffusionError::model)?;
        *k_uncond = gpu_concat_rows(k_uncond, &k1).map_err(DiffusionError::model)?;
        *v_cond = gpu_concat_rows(v_cond, &v0).map_err(DiffusionError::model)?;
        *v_uncond = gpu_concat_rows(v_uncond, &v1).map_err(DiffusionError::model)?;
    }
    drop((k_step, v_step, k0, k1, v0, v1));
    // In-place pair kernel over the four caches (rows 0..seq+1); skips the
    // per-frame O(seq) cond/uncond concat entirely. Byte-identical to the
    // serial GQA kernel on the concatenated caches (--stage decodepair).
    let attn = gpu_attention_gqa_decode_pair_bf16(
        &q,
        k_cond,
        v_cond,
        k_uncond,
        v_uncond,
        seq + 1,
        MUSIC3_LM_HEADS,
        MUSIC3_LM_KV_HEADS,
        scale,
    )
    .map_err(DiffusionError::model)?;
    drop(q);
    if dump {
        dump_f1_rows(&attn, q_inner, &format!("native_f1_L{layer}_attn_out.npy"));
    }
    let attn = linear_cached(
        weights,
        &attn,
        &format!("{prefix}.self_attn.o_proj.weight"),
        MUSIC3_LM_HIDDEN,
    )?;
    if dump {
        dump_f1_rows(
            &attn,
            MUSIC3_LM_HIDDEN,
            &format!("native_f1_L{layer}_oproj.npy"),
        );
    }
    let hidden = residual_add(&hidden, &attn)?;
    let hidden = music3_cast_hidden(hidden)?;
    if dump {
        dump_f1_rows(
            &hidden,
            MUSIC3_LM_HIDDEN,
            &format!("native_f1_L{layer}_attn_resid.npy"),
        );
    }
    drop(attn);
    let normed = music3_hidden_rms(
        &hidden,
        format!("{prefix}.post_attention_layernorm"),
        &prepared.post_attn_norm[layer],
    )?;
    if dump {
        dump_f1_rows(
            &normed,
            MUSIC3_LM_HIDDEN,
            &format!("native_f1_L{layer}_post_norm.npy"),
        );
    }
    let up_name = format!("{prefix}.mlp.up_proj.weight");
    let gate_name = format!("{prefix}.mlp.gate_proj.weight");
    let up = linear_cached(weights, &normed, &up_name, MUSIC3_LM_FF)?;
    let gate = linear_cached(weights, &normed, &gate_name, MUSIC3_LM_FF)?;
    drop(normed);
    if dump {
        dump_f1_rows(&up, MUSIC3_LM_FF, &format!("native_f1_L{layer}_up.npy"));
        dump_f1_rows(&gate, MUSIC3_LM_FF, &format!("native_f1_L{layer}_gate.npy"));
    }
    let up_gate = gpu_concat_cols(&[&up, &gate]).map_err(DiffusionError::model)?;
    let ff = gpu_swiglu_value_gate(&up_gate).map_err(DiffusionError::model)?;
    if dump {
        dump_f1_rows(&ff, MUSIC3_LM_FF, &format!("native_f1_L{layer}_swiglu.npy"));
    }
    drop(up_gate);
    let ff = down_proj_cached(
        weights,
        &ff,
        &format!("{prefix}.mlp.down_proj.weight"),
        MUSIC3_LM_HIDDEN,
    )?;
    if dump {
        dump_f1_rows(
            &ff,
            MUSIC3_LM_HIDDEN,
            &format!("native_f1_L{layer}_down.npy"),
        );
    }
    let hidden = residual_add(&hidden, &ff)?;
    let hidden = music3_cast_hidden(hidden)?;
    if dump {
        dump_f1_rows(
            &hidden,
            MUSIC3_LM_HIDDEN,
            &format!("native_f1_layer{layer}.npy"),
        );
    }
    Ok(hidden)
}

/// `lm_head` on a single `[H]` hidden. Returns `[vocab]`.
pub fn music3_lm_head(weights: &Music3Shards, hidden: &[f32]) -> Result<Vec<f32>> {
    music3_lm_head_last(weights, hidden, 1)
}

/// Embed one complete RVQ frame for LM feedback.
/// `semantic_token` is the raw LM token (`code + AUDIO_CODE_OFFSET`).
/// `resid` is 7 codebook indices in `0..1024`.
pub fn music3_embed_audio_frame(
    lm: &Music3Shards,
    rvq: &Music3Shards,
    semantic_token: u32,
    resid: &[u32],
) -> Result<Vec<f32>> {
    use crate::music3::{MUSIC3_AUDIO_VOCAB, MUSIC3_NUM_CODEBOOKS};
    if resid.len() != MUSIC3_NUM_CODEBOOKS - 1 {
        return Err(DiffusionError::model("audio frame residual count"));
    }
    let mut embeds = lm.tensor_row_f32("model.embed_tokens.weight", semantic_token as u64)?;
    for (i, &code) in resid.iter().enumerate() {
        let idx = code as u64 + (i as u64) * MUSIC3_AUDIO_VOCAB as u64;
        let extra = rvq.tensor_row_f32("audio_embeddings.weight", idx)?;
        for (e, x) in embeds.iter_mut().zip(extra.iter()) {
            *e += *x;
        }
    }
    let scale = (MUSIC3_NUM_CODEBOOKS as f32).sqrt().recip();
    for e in &mut embeds {
        *e *= scale;
    }
    Ok(embeds)
}

fn want_f1_dump(seq: usize) -> bool {
    // Oracle prompt prefill is 18 tokens; first decode is f1.
    seq == 18 && std::env::var_os("MAKEPAD_MUSIC3_DUMP_F1").is_some()
}

fn dump_f1_rows(tensor: &GpuTensor, cols: usize, name: &str) {
    let Ok(dir) = std::env::var("MAKEPAD_MUSIC3_DUMP_F1") else {
        return;
    };
    let Ok(data) = gpu_download(tensor) else {
        return;
    };
    let rows = tensor.rows().min(2).max(1);
    let mut out = Vec::with_capacity(rows * cols);
    for b in 0..rows {
        let s = b * cols;
        if s + cols <= data.len() {
            out.extend_from_slice(&data[s..s + cols]);
        }
    }
    let _ = write_npy_f32_lm(&format!("{dir}/{name}"), &out, &[rows, cols]);
}

fn dump_l0_last_rows(
    tensor: &GpuTensor,
    tokens: usize,
    batch: usize,
    cols: usize,
    name: &str,
) {
    let Ok(dir) = std::env::var("MAKEPAD_MUSIC3_DUMP_L0_QKV") else {
        return;
    };
    let Ok(data) = gpu_download(tensor) else {
        return;
    };
    let batch = batch.max(1);
    let mut out = Vec::with_capacity(batch * cols);
    for b in 0..batch {
        let row = (b + 1) * tokens - 1;
        let s = row * cols;
        if s + cols <= data.len() {
            out.extend_from_slice(&data[s..s + cols]);
        }
    }
    let _ = write_npy_f32_lm(&format!("{dir}/{name}"), &out, &[batch, cols]);
}

fn dump_fullseq_if(hidden: &GpuTensor, tokens: usize, batch: usize, name: &str) {
    if batch != 2 || tokens == 0 {
        return;
    }
    let Ok(dir) = std::env::var("MAKEPAD_MUSIC3_DUMP_FULLSEQ") else {
        return;
    };
    dump_fullseq_pair(hidden, tokens, &dir, name);
}

fn dump_layer1_if(hidden: &GpuTensor, tokens: usize, batch: usize, name: &str) {
    if batch != 2 || tokens == 0 {
        return;
    }
    let Ok(dir) = std::env::var("MAKEPAD_MUSIC3_DUMP_LAYER1") else {
        return;
    };
    dump_fullseq_pair(hidden, tokens, &dir, name);
}

/// Full pair sequence `[2, T, C]` (row-major same as `[2T, C]`). Not last-token.
fn dump_fullseq_pair(hidden: &GpuTensor, tokens: usize, dir: &str, name: &str) {
    if tokens == 0 {
        return;
    }
    let path = std::path::Path::new(dir).join(name);
    let path = match path.to_str() {
        Some(p) => p.to_string(),
        None => return,
    };
    let cols = hidden.cols();
    let rows = hidden.rows();
    if rows != 2 * tokens {
        eprintln!("dump {path}: rows={rows} expected 2*{tokens}");
        return;
    }
    let data = match gpu_download(hidden) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("dump {path}: {err}");
            return;
        }
    };
    if data.len() != 2 * tokens * cols {
        eprintln!(
            "dump {path}: len={} expected {}",
            data.len(),
            2 * tokens * cols
        );
        return;
    }
    if let Err(err) = write_npy_f32_lm(&path, &data, &[2, tokens, cols]) {
        eprintln!("dump {path}: {err}");
    } else {
        eprintln!("dumped {path} [2,{tokens},{cols}]");
    }
}

fn dump_prefill_last_pair(hidden: &GpuTensor, tokens: usize, dir: &str, name: &str) {
    if tokens == 0 {
        return;
    }
    let path = std::path::Path::new(dir).join(name);
    let path = match path.to_str() {
        Some(p) => p.to_string(),
        None => return,
    };
    let last_c = match gpu_slice_rows(hidden, tokens - 1, 1).and_then(|t| gpu_download(&t)) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("dump {path}: {err}");
            return;
        }
    };
    let last_u = match gpu_slice_rows(hidden, 2 * tokens - 1, 1).and_then(|t| gpu_download(&t)) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("dump {path}: {err}");
            return;
        }
    };
    let mut both = last_c;
    both.extend_from_slice(&last_u);
    if let Err(err) = write_npy_f32_lm(&path, &both, &[2, MUSIC3_LM_HIDDEN]) {
        eprintln!("dump {path}: {err}");
    } else {
        eprintln!("dumped {path}");
    }
}

fn write_npy_f32_lm(path: &str, data: &[f32], shape: &[usize]) -> std::io::Result<()> {
    use std::io::Write;
    let shape_txt = if shape.len() == 1 {
        format!("({},)", shape[0])
    } else {
        format!(
            "({})",
            shape
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut dict = format!("{{'descr': '<f4', 'fortran_order': False, 'shape': {shape_txt}, }}");
    let prefix = 10usize;
    let mut header_len = dict.len() + 1;
    let pad = (16 - ((prefix + header_len) % 16)) % 16;
    header_len += pad;
    dict.push_str(&" ".repeat(pad));
    dict.push('\n');
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"\x93NUMPY\x01\x00")?;
    f.write_all(&(header_len as u16).to_le_bytes())?;
    f.write_all(dict.as_bytes())?;
    for v in data {
        f.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}
