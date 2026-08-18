//! MiniMax-Music3 flow-matching DiT (2.4B) on CUDA ggml.
//! Official ModularPipeline casts the on-disk F32 shards to bf16; we do the
//! same on upload so step-0 matches the dump.

use crate::backend::{
    gpu_add, gpu_attention_packed, gpu_concat_rows, gpu_device_available, gpu_download,
    gpu_layer_norm_mul_add, gpu_linear_nt_cached_bf16_f32acc, gpu_rope_half, gpu_slice_cols,
    gpu_slice_rows, gpu_swiglu_value_gate, gpu_upload, gpu_weight_cache_ensure,
    gpu_weight_cache_evict_prefix, GpuLinearPart, GpuTensor,
};
use crate::music3::{
    MUSIC3_DIT_CONCAT, MUSIC3_DIT_COND, MUSIC3_DIT_DIM, MUSIC3_DIT_FF, MUSIC3_DIT_FOURIER,
    MUSIC3_DIT_HEAD_DIM, MUSIC3_DIT_HEADS, MUSIC3_DIT_IN_CHANNELS, MUSIC3_DIT_LAYERS,
    MUSIC3_DIT_ROPE,
};
use crate::music3_weights::{Music3Shards, MUSIC3_DIT_NAMESPACE};
use crate::{DiffusionError, Result};
use makepad_ai_common::quant::GGML_TYPE_BF16;
use std::f32::consts::PI;

const LN_EPS: f32 = 1e-5;

fn f32_le_to_bf16(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(4) {
        let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let bits = v.to_bits();
        let bf = if bits & 0x0000_7fff > 0x0000_7fff - (bits >> 16 & 1) {
            ((bits >> 16) + 1) as u16
        } else {
            (bits >> 16) as u16
        };
        out.extend_from_slice(&bf.to_le_bytes());
    }
    out
}

fn ensure_linear<'a>(
    weights: &Music3Shards,
    name: &'a str,
    n: usize,
    k: usize,
) -> Result<GpuLinearPart<'a>> {
    let ggml_type = weights.linear_ggml_type(name);
    if crate::backend::gpu_quant_linear_type_supported(ggml_type) {
        crate::backend::gpu_weight_cache_ensure_quant(MUSIC3_DIT_NAMESPACE, name, ggml_type, n, k, || {
            weights.tensor_bytes(name).map_err(|err| err.to_string())
        })
        .map_err(DiffusionError::model)?;
        return Ok(GpuLinearPart {
            bt_ggml_type: ggml_type,
            n,
            cache_key: name,
            bytes: &[],
        });
    }
    // Safetensors DiT weights are F32 on disk (converted to bf16 here);
    // GGUF F32/BF16 members take the same length-switched path.
    gpu_weight_cache_ensure(MUSIC3_DIT_NAMESPACE, name, GGML_TYPE_BF16, n, k, false, || {
        let raw = weights.tensor_bytes(name).map_err(|err| err.to_string())?;
        if raw.len() == n * k * 4 {
            Ok(f32_le_to_bf16(&raw))
        } else {
            Ok(raw)
        }
    })
    .map_err(DiffusionError::model)?;
    Ok(GpuLinearPart {
        bt_ggml_type: GGML_TYPE_BF16,
        n,
        cache_key: name,
        bytes: &[],
    })
}

fn linear_parts(
    x: &GpuTensor,
    parts: &[GpuLinearPart<'_>],
    bias: &[f32],
) -> Result<GpuTensor> {
    if crate::backend::gpu_quant_linear_type_supported(parts[0].bt_ggml_type) {
        return crate::backend::gpu_linear_nt_cached(x, MUSIC3_DIT_NAMESPACE, parts, bias)
            .map_err(DiffusionError::model);
    }
    gpu_linear_nt_cached_bf16_f32acc(x, MUSIC3_DIT_NAMESPACE, parts, bias)
        .map_err(DiffusionError::model)
}

fn linear(
    weights: &Music3Shards,
    x: &GpuTensor,
    name: &str,
    n: usize,
    bias: &[f32],
) -> Result<GpuTensor> {
    let part = ensure_linear(weights, name, n, x.cols())?;
    linear_parts(x, &[part], bias)
}

pub struct Music3DitPrepared {
    time_proj: Vec<f32>, // [128, 1]
    time_w1: Vec<f32>,
    time_b1: Vec<f32>,
    time_w2: Vec<f32>,
    time_b2: Vec<f32>,
    norm1_w: Vec<Vec<f32>>,
    norm1_b: Vec<Vec<f32>>,
    norm2_w: Vec<Vec<f32>>,
    norm2_b: Vec<Vec<f32>>,
    ff_in_b: Vec<Vec<f32>>,
    ff_out_b: Vec<Vec<f32>>,
    rope_inv_freq: Vec<f32>,
}

impl Music3DitPrepared {
    pub fn prepare(weights: &Music3Shards) -> Result<Self> {
        let mut norm1_w = Vec::with_capacity(MUSIC3_DIT_LAYERS);
        let mut norm1_b = Vec::with_capacity(MUSIC3_DIT_LAYERS);
        let mut norm2_w = Vec::with_capacity(MUSIC3_DIT_LAYERS);
        let mut norm2_b = Vec::with_capacity(MUSIC3_DIT_LAYERS);
        let mut ff_in_b = Vec::with_capacity(MUSIC3_DIT_LAYERS);
        let mut ff_out_b = Vec::with_capacity(MUSIC3_DIT_LAYERS);
        for layer in 0..MUSIC3_DIT_LAYERS {
            norm1_w.push(weights.tensor_f32(&format!("transformer_blocks.{layer}.norm1.weight"))?);
            norm1_b.push(weights.tensor_f32(&format!("transformer_blocks.{layer}.norm1.bias"))?);
            norm2_w.push(weights.tensor_f32(&format!("transformer_blocks.{layer}.norm2.weight"))?);
            norm2_b.push(weights.tensor_f32(&format!("transformer_blocks.{layer}.norm2.bias"))?);
            ff_in_b.push(weights.tensor_f32(&format!("transformer_blocks.{layer}.ff_in.bias"))?);
            ff_out_b.push(weights.tensor_f32(&format!("transformer_blocks.{layer}.ff_out.bias"))?);
        }
        let half = MUSIC3_DIT_ROPE / 2;
        let mut rope_inv_freq = Vec::with_capacity(half);
        for j in 0..half {
            rope_inv_freq.push(1.0 / 10_000f32.powf(2.0 * j as f32 / MUSIC3_DIT_ROPE as f32));
        }
        Ok(Self {
            time_proj: weights.tensor_f32("time_proj.weight")?,
            time_w1: weights.tensor_f32("time_embed.linear_1.weight")?,
            time_b1: weights.tensor_f32("time_embed.linear_1.bias")?,
            time_w2: weights.tensor_f32("time_embed.linear_2.weight")?,
            time_b2: weights.tensor_f32("time_embed.linear_2.bias")?,
            norm1_w,
            norm1_b,
            norm2_w,
            norm2_b,
            ff_in_b,
            ff_out_b,
            rope_inv_freq,
        })
    }
}

fn time_embed(prepared: &Music3DitPrepared, t: f32) -> Vec<f32> {
    let nfreq = MUSIC3_DIT_FOURIER / 2;
    let mut feat = vec![0f32; MUSIC3_DIT_FOURIER];
    for i in 0..nfreq {
        let angle = 2.0 * PI * t * prepared.time_proj[i];
        feat[i] = angle.cos();
        feat[nfreq + i] = angle.sin();
    }
    // linear_1: [2048, 256] @ feat + b, silu, linear_2 [2048, 2048]
    let mut h = vec![0f32; MUSIC3_DIT_DIM];
    for o in 0..MUSIC3_DIT_DIM {
        let mut acc = prepared.time_b1[o];
        let row = &prepared.time_w1[o * MUSIC3_DIT_FOURIER..(o + 1) * MUSIC3_DIT_FOURIER];
        for i in 0..MUSIC3_DIT_FOURIER {
            acc += row[i] * feat[i];
        }
        h[o] = silu_f32(acc);
    }
    let mut out = vec![0f32; MUSIC3_DIT_DIM];
    for o in 0..MUSIC3_DIT_DIM {
        let mut acc = prepared.time_b2[o];
        let row = &prepared.time_w2[o * MUSIC3_DIT_DIM..(o + 1) * MUSIC3_DIT_DIM];
        for i in 0..MUSIC3_DIT_DIM {
            acc += row[i] * h[i];
        }
        out[o] = acc;
    }
    out
}

fn silu_f32(x: f32) -> f32 {
    x / (1.0 + (-x).exp())
}

fn block(
    weights: &Music3Shards,
    prepared: &Music3DitPrepared,
    layer: usize,
    hidden: GpuTensor,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
) -> Result<GpuTensor> {
    let scale = 1.0 / (MUSIC3_DIT_HEAD_DIM as f32).sqrt();
    let rot_half = MUSIC3_DIT_ROPE / 2;
    let mut hidden = hidden;
    let normed = gpu_layer_norm_mul_add(
        &hidden,
        &prepared.norm1_w[layer],
        &prepared.norm1_b[layer],
        LN_EPS,
    )
    .map_err(DiffusionError::model)?;
    let q_name = format!("transformer_blocks.{layer}.attn.to_q.weight");
    let k_name = format!("transformer_blocks.{layer}.attn.to_k.weight");
    let v_name = format!("transformer_blocks.{layer}.attn.to_v.weight");
    let parts = [
        ensure_linear(weights, &q_name, MUSIC3_DIT_DIM, MUSIC3_DIT_DIM)?,
        ensure_linear(weights, &k_name, MUSIC3_DIT_DIM, MUSIC3_DIT_DIM)?,
        ensure_linear(weights, &v_name, MUSIC3_DIT_DIM, MUSIC3_DIT_DIM)?,
    ];
    let qkv = linear_parts(&normed, &parts, &[])?;
    drop(normed);
    let q = gpu_slice_cols(&qkv, 0, MUSIC3_DIT_DIM).map_err(DiffusionError::model)?;
    let k = gpu_slice_cols(&qkv, MUSIC3_DIT_DIM, MUSIC3_DIT_DIM).map_err(DiffusionError::model)?;
    let v = gpu_slice_cols(&qkv, 2 * MUSIC3_DIT_DIM, MUSIC3_DIT_DIM).map_err(DiffusionError::model)?;
    drop(qkv);
    let q = gpu_rope_half(&q, MUSIC3_DIT_HEADS, rot_half, rope_cos, rope_sin)
        .map_err(DiffusionError::model)?;
    let k = gpu_rope_half(&k, MUSIC3_DIT_HEADS, rot_half, rope_cos, rope_sin)
        .map_err(DiffusionError::model)?;
    let attn = gpu_attention_packed(&q, &k, &v, MUSIC3_DIT_HEADS, scale)
        .map_err(DiffusionError::model)?;
    drop((q, k, v));
    let attn = linear(
        weights,
        &attn,
        &format!("transformer_blocks.{layer}.attn.to_out.0.weight"),
        MUSIC3_DIT_DIM,
        &[],
    )?;
    hidden = gpu_add(&hidden, &attn).map_err(DiffusionError::model)?;
    drop(attn);

    let normed = gpu_layer_norm_mul_add(
        &hidden,
        &prepared.norm2_w[layer],
        &prepared.norm2_b[layer],
        LN_EPS,
    )
    .map_err(DiffusionError::model)?;
    let ff_in = linear(
        weights,
        &normed,
        &format!("transformer_blocks.{layer}.ff_in.weight"),
        MUSIC3_DIT_FF * 2,
        &prepared.ff_in_b[layer],
    )?;
    drop(normed);
    // Official: gate_states, gate = chunk(2); out = ff_out(gate_states * silu(gate))
    // gpu_swiglu_value_gate is value-first (up * silu(gate)) which matches
    // if we pack [gate_states | gate] = [value | gate].
    let ff = gpu_swiglu_value_gate(&ff_in).map_err(DiffusionError::model)?;
    drop(ff_in);
    let ff = linear(
        weights,
        &ff,
        &format!("transformer_blocks.{layer}.ff_out.weight"),
        MUSIC3_DIT_DIM,
        &prepared.ff_out_b[layer],
    )?;
    hidden = gpu_add(&hidden, &ff).map_err(DiffusionError::model)?;
    Ok(hidden)
}

/// One DiT forward. `latents` and `velocity` are channel-major `[C, T]`.
/// `cond` is token-major `[T, cond_dim]`.
pub fn music3_dit_forward(
    weights: &Music3Shards,
    prepared: &Music3DitPrepared,
    latents: &[f32],
    cond: &[f32],
    tokens: usize,
    timestep: f32,
) -> Result<Vec<f32>> {
    if !gpu_device_available() {
        return Err(DiffusionError::model("music3 DiT needs CUDA"));
    }
    if latents.len() != MUSIC3_DIT_IN_CHANNELS * tokens
        || cond.len() != tokens * MUSIC3_DIT_COND
    {
        return Err(DiffusionError::model(format!(
            "dit shapes latents={} cond={} tokens={tokens}",
            latents.len(),
            cond.len()
        )));
    }
    // concat [latent, zeros, cond] as (T, 2304)
    let mut cat = vec![0f32; tokens * MUSIC3_DIT_CONCAT];
    for t in 0..tokens {
        for c in 0..MUSIC3_DIT_IN_CHANNELS {
            cat[t * MUSIC3_DIT_CONCAT + c] = latents[c * tokens + t];
        }
        // zeros occupy [C, 2C)
        for c in 0..MUSIC3_DIT_COND {
            cat[t * MUSIC3_DIT_CONCAT + 2 * MUSIC3_DIT_IN_CHANNELS + c] = cond[t * MUSIC3_DIT_COND + c];
        }
    }
    let x = gpu_upload(&cat, tokens, MUSIC3_DIT_CONCAT).map_err(DiffusionError::model)?;
    let pre = linear(weights, &x, "preprocess_conv.weight", MUSIC3_DIT_CONCAT, &[])?;
    let x = gpu_add(&x, &pre).map_err(DiffusionError::model)?;
    drop(pre);
    let mut hidden = linear(weights, &x, "proj_in.weight", MUSIC3_DIT_DIM, &[])?;
    drop(x);

    let temb = time_embed(prepared, timestep);
    let temb_g = gpu_upload(&temb, 1, MUSIC3_DIT_DIM).map_err(DiffusionError::model)?;
    hidden = gpu_concat_rows(&temb_g, &hidden).map_err(DiffusionError::model)?;
    drop(temb_g);

    let seq = tokens + 1;
    let rot_half = MUSIC3_DIT_ROPE / 2;
    let mut cos = vec![0f32; seq * rot_half];
    let mut sin = vec![0f32; seq * rot_half];
    for pos in 0..seq {
        for j in 0..rot_half {
            let angle = pos as f32 * prepared.rope_inv_freq[j];
            cos[pos * rot_half + j] = angle.cos();
            sin[pos * rot_half + j] = angle.sin();
        }
    }
    let rope_cos = gpu_upload(&cos, seq, rot_half).map_err(DiffusionError::model)?;
    let rope_sin = gpu_upload(&sin, seq, rot_half).map_err(DiffusionError::model)?;

    for layer in 0..MUSIC3_DIT_LAYERS {
        hidden = block(weights, prepared, layer, hidden, &rope_cos, &rope_sin)?;
    }
    hidden = gpu_slice_rows(&hidden, 1, tokens).map_err(DiffusionError::model)?;
    let hidden = linear(weights, &hidden, "proj_out.weight", MUSIC3_DIT_IN_CHANNELS, &[])?;
    let post = linear(
        weights,
        &hidden,
        "postprocess_conv.weight",
        MUSIC3_DIT_IN_CHANNELS,
        &[],
    )?;
    let hidden = gpu_add(&hidden, &post).map_err(DiffusionError::model)?;
    let tok = gpu_download(&hidden).map_err(DiffusionError::model)?;
    // back to channel-major [C, T]
    let mut out = vec![0f32; MUSIC3_DIT_IN_CHANNELS * tokens];
    for t in 0..tokens {
        for c in 0..MUSIC3_DIT_IN_CHANNELS {
            out[c * tokens + t] = tok[t * MUSIC3_DIT_IN_CHANNELS + c];
        }
    }
    Ok(out)
}

/// Official Music3 scheduler (`scheduler_config.json`):
/// `set_timesteps(sigmas=linspace(1, 1/steps, steps))` with
/// `invert_sigmas=true`, `shift=1`, `num_train_timesteps=1`.
/// That yields transformer time 0 → 1−1/steps (0 = noise, 1 = data) and
/// appends a terminal sigma of **1**, so Euler `dt` is positive.
fn official_flow_sigmas(steps: usize) -> Vec<f32> {
    let n = steps.max(1);
    let start = 1.0f32;
    let end = 1.0 / n as f32;
    let mut sigmas = Vec::with_capacity(n + 1);
    for i in 0..n {
        let u = if n == 1 { 0.0 } else { i as f32 / (n - 1) as f32 };
        let raw = start + (end - start) * u;
        sigmas.push(1.0 - raw);
    }
    sigmas.push(1.0);
    sigmas
}

/// 30-step official Euler + CFG. `latents` channel-major `[C, T]`, `cond` token-major `[T, cond]`.
pub fn music3_dit_sample(
    weights: &Music3Shards,
    prepared: &Music3DitPrepared,
    latents: &[f32],
    cond: &[f32],
    tokens: usize,
    steps: usize,
    cfg: f32,
) -> Result<Vec<f32>> {
    music3_dit_sample_window(
        weights,
        prepared,
        latents,
        cond,
        tokens,
        steps,
        cfg,
        0,
        None,
        &mut |_, _| {},
    )
}

pub fn music3_dit_sample_with_progress(
    weights: &Music3Shards,
    prepared: &Music3DitPrepared,
    latents: &[f32],
    cond: &[f32],
    tokens: usize,
    steps: usize,
    cfg: f32,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Vec<f32>> {
    music3_dit_sample_window(
        weights,
        prepared,
        latents,
        cond,
        tokens,
        steps,
        cfg,
        0,
        None,
        progress,
    )
}

/// One official 200-frame window. When `overlap > 0`, each step injects
/// `latents[..., :overlap] = (1-(1-1e-6)*t)*noise_prompt + t*previous`
/// (`denoise.py` MiniMaxMusic3ChunkDenoiseInner).
pub fn music3_dit_sample_window(
    weights: &Music3Shards,
    prepared: &Music3DitPrepared,
    latents: &[f32],
    cond: &[f32],
    tokens: usize,
    steps: usize,
    cfg: f32,
    overlap: usize,
    previous: Option<&[f32]>,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Vec<f32>> {
    let zeros = vec![0f32; cond.len()];
    let mut x = latents.to_vec();
    let ov = if overlap > 0 && previous.map(|p| p.len() == MUSIC3_DIT_IN_CHANNELS * overlap).unwrap_or(false)
    {
        overlap
    } else {
        0
    };
    let noise_prompt = if ov > 0 {
        let mut p = vec![0f32; MUSIC3_DIT_IN_CHANNELS * ov];
        for c in 0..MUSIC3_DIT_IN_CHANNELS {
            p[c * ov..c * ov + ov].copy_from_slice(&x[c * tokens..c * tokens + ov]);
        }
        Some(p)
    } else {
        None
    };
    let sigmas = official_flow_sigmas(steps);
    progress(0, steps);
    for i in 0..steps {
        let t = sigmas[i];
        if ov > 0 {
            let prev = previous.unwrap();
            let prompt = noise_prompt.as_ref().unwrap();
            let a = 1.0 - (1.0 - 1e-6) * t;
            for c in 0..MUSIC3_DIT_IN_CHANNELS {
                for k in 0..ov {
                    x[c * tokens + k] = a * prompt[c * ov + k] + t * prev[c * ov + k];
                }
            }
        }
        let v_cond = music3_dit_forward(weights, prepared, &x, cond, tokens, t)?;
        let v_uncond = music3_dit_forward(weights, prepared, &x, &zeros, tokens, t)?;
        let dt = sigmas[i + 1] - sigmas[i];
        for j in 0..x.len() {
            let v = v_uncond[j] + cfg * (v_cond[j] - v_uncond[j]);
            x[j] += dt * v;
        }
        progress(i + 1, steps);
    }
    Ok(x)
}

pub fn music3_dit_evict() -> Result<usize> {
    gpu_weight_cache_evict_prefix(&format!("{MUSIC3_DIT_NAMESPACE}::")).map_err(DiffusionError::model)
}
