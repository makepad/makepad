//! MiniMax H3 33B DiT: device-resident forward on the flux GpuTensor CUDA
//! machinery. One packed sequence [text | audio | video], 50 blocks with
//! per-row AdaLN-table modulation, a 2-block text token refiner, rotate-half
//! 3D rope on 96 of 128 head channels, and the two f32 output heads.
//!
//! Weight residency: bf16 stack weights STREAM from the sharded safetensors
//! into the device cache on first touch (bf16 -> f16 when the f16acc gemms
//! are on) — there is never a whole-model host copy (66GB model, 61GB-RAM
//! box). The deliberately-f32 modules (proj_in / audio_proj_in / heads /
//! time_embedder) run through resident f32 tensors, mirroring the reference's
//! mixed-precision contract.

use crate::backend::{
    gpu_attention_packed, gpu_concat_rows, gpu_download, gpu_gated_residual_indexed,
    gpu_linear_f32_resident, gpu_linear_nt_cached_f16_with_precision,
    gpu_linear_nt_cached_with_precision, gpu_rms_norm_mod_indexed, gpu_rms_norm_mul, gpu_rope_half,
    gpu_slice_cols, gpu_slice_rows, gpu_swiglu_value_gate, gpu_upload, gpu_upload_u32,
    gpu_weight_cache_ensure, gpu_weight_cache_ensure_quant, GpuLinearPart, GpuTensor,
    GemmPrecision,
};
use crate::h3::{
    H3PackedLayout, H3RopeTables, H3RowTimesteps, H3ShardedWeights, h3_timestep_embedding,
    H3_AUDIO_IN_CHANNELS, H3_DEPTH, H3_FFN_DIM, H3_FREQ_DIM, H3_HEAD_COUNT, H3_HEAD_DIM,
    H3_HIDDEN_SIZE, H3_NORM_EPS, H3_REFINER_DEPTH, H3_ROT_HALF, H3_TEXT_DIM,
    H3_TIME_EMBED_DIM, H3_TIME_EMBED_HIDDEN, H3_VIDEO_PATCH_DIM,
};
use crate::{DiffusionError, Result};

pub const H3_DIT_NAMESPACE: &str = "h3dit";
const ADALN_COLS: usize = 6 * H3_HIDDEN_SIZE * 3; // 96768 per block
const ADALN_STRIDE: usize = 6 * H3_HIDDEN_SIZE; // 32256 per (timestep, modality) row
const NORM_OUT_COLS: usize = 2 * H3_HIDDEN_SIZE; // shift | scale

/// Host-resident small weights + device-resident f32 modules, prepared once.
pub struct H3DitPrepared {
    // f32 modules (device-resident, exact).
    proj_in_w: GpuTensor,
    proj_in_b: GpuTensor,
    audio_proj_in_w: GpuTensor,
    audio_proj_in_b: GpuTensor,
    proj_out_w: GpuTensor,
    proj_out_b: GpuTensor,
    audio_proj_out_w: GpuTensor,
    audio_proj_out_b: GpuTensor,
    // Per-block norm weights for the fused indexed-AdaLN kernel.
    norm1_w: Vec<GpuTensor>,
    norm2_w: Vec<GpuTensor>,
    norm_out_norm_w: GpuTensor,
    // Host f32: timestep MLP + biases + per-head norm scales. The timestep
    // MLP is absent in the AdaLN-curve pruned checkpoints (the curve in the
    // weight source replaces it).
    time_l1_w: Option<Vec<f32>>,
    time_l1_b: Option<Vec<f32>>,
    time_l2_w: Option<Vec<f32>>,
    time_l2_b: Option<Vec<f32>>,
    context_embedder_bias: Vec<f32>,
    adaln_bias: Vec<Vec<f32>>,
    norm_out_bias: Vec<f32>,
    q_norm: Vec<Vec<f32>>,
    k_norm: Vec<Vec<f32>>,
    refiner_norm1: Vec<Vec<f32>>,
    refiner_norm2: Vec<Vec<f32>>,
    refiner_q_norm: Vec<Vec<f32>>,
    refiner_k_norm: Vec<Vec<f32>>,
    refiner_final_norm: Vec<f32>,
}

#[derive(Default)]
pub struct H3DitDebug {
    pub refiner_out: Option<Vec<f32>>,
    pub temb: Option<Vec<f32>>,
    pub block_adaln0: Option<Vec<f32>>,
    pub block_outputs: Vec<(usize, Vec<f32>)>,
}

pub struct H3DitForwardOutput {
    pub video_velocity: Vec<f32>,
    pub audio_velocity: Vec<f32>,
    pub debug: H3DitDebug,
}

fn upload_named_f32(
    weights: &H3ShardedWeights,
    name: &str,
    rows: usize,
    cols: usize,
) -> Result<GpuTensor> {
    let values = weights.tensor_f32(name)?;
    if values.len() != rows * cols {
        return Err(DiffusionError::model(format!(
            "h3 tensor '{name}' expected {rows}x{cols} = {} values, got {}",
            rows * cols,
            values.len()
        )));
    }
    gpu_upload(&values, rows, cols).map_err(DiffusionError::model)
}

fn host_named_f32(weights: &H3ShardedWeights, name: &str, len: usize) -> Result<Vec<f32>> {
    let values = weights.tensor_f32(name)?;
    if values.len() != len {
        return Err(DiffusionError::model(format!(
            "h3 tensor '{name}' expected {len} values, got {}",
            values.len()
        )));
    }
    Ok(values)
}

impl H3DitPrepared {
    pub fn prepare(weights: &H3ShardedWeights) -> Result<Self> {
        let h = H3_HIDDEN_SIZE;
        let curve_mode = weights.adaln_curve().is_some();
        let mut norm1_w = Vec::with_capacity(H3_DEPTH);
        let mut norm2_w = Vec::with_capacity(H3_DEPTH);
        let mut adaln_bias = Vec::with_capacity(H3_DEPTH);
        let mut q_norm = Vec::with_capacity(H3_DEPTH);
        let mut k_norm = Vec::with_capacity(H3_DEPTH);
        for layer in 0..H3_DEPTH {
            let prefix = format!("transformer_blocks.{layer}");
            norm1_w.push(upload_named_f32(weights, &format!("{prefix}.norm1.weight"), 1, h)?);
            norm2_w.push(upload_named_f32(weights, &format!("{prefix}.norm2.weight"), 1, h)?);
            adaln_bias.push(host_named_f32(
                weights,
                &format!("{prefix}.adaln_proj.linear.bias"),
                ADALN_COLS,
            )?);
            q_norm.push(host_named_f32(
                weights,
                &format!("{prefix}.attn.norm_q.weight"),
                H3_HEAD_DIM,
            )?);
            k_norm.push(host_named_f32(
                weights,
                &format!("{prefix}.attn.norm_k.weight"),
                H3_HEAD_DIM,
            )?);
        }
        let mut refiner_norm1 = Vec::new();
        let mut refiner_norm2 = Vec::new();
        let mut refiner_q_norm = Vec::new();
        let mut refiner_k_norm = Vec::new();
        for layer in 0..H3_REFINER_DEPTH {
            let prefix = format!("token_refiner.refiner_blocks.{layer}");
            refiner_norm1.push(host_named_f32(weights, &format!("{prefix}.norm1.weight"), h)?);
            refiner_norm2.push(host_named_f32(weights, &format!("{prefix}.norm2.weight"), h)?);
            refiner_q_norm.push(host_named_f32(
                weights,
                &format!("{prefix}.attn.norm_q.weight"),
                H3_HEAD_DIM,
            )?);
            refiner_k_norm.push(host_named_f32(
                weights,
                &format!("{prefix}.attn.norm_k.weight"),
                H3_HEAD_DIM,
            )?);
        }
        Ok(Self {
            proj_in_w: upload_named_f32(weights, "proj_in.weight", h, H3_VIDEO_PATCH_DIM)?,
            proj_in_b: upload_named_f32(weights, "proj_in.bias", 1, h)?,
            audio_proj_in_w: upload_named_f32(
                weights,
                "audio_proj_in.weight",
                h,
                H3_AUDIO_IN_CHANNELS,
            )?,
            audio_proj_in_b: upload_named_f32(weights, "audio_proj_in.bias", 1, h)?,
            proj_out_w: upload_named_f32(weights, "proj_out.weight", H3_VIDEO_PATCH_DIM, h)?,
            proj_out_b: upload_named_f32(weights, "proj_out.bias", 1, H3_VIDEO_PATCH_DIM)?,
            audio_proj_out_w: upload_named_f32(
                weights,
                "audio_proj_out.weight",
                H3_AUDIO_IN_CHANNELS,
                h,
            )?,
            audio_proj_out_b: upload_named_f32(
                weights,
                "audio_proj_out.bias",
                1,
                H3_AUDIO_IN_CHANNELS,
            )?,
            norm1_w,
            norm2_w,
            norm_out_norm_w: upload_named_f32(weights, "norm_out.norm.weight", 1, h)?,
            time_l1_w: if curve_mode {
                None
            } else {
                Some(host_named_f32(
                    weights,
                    "time_embedder.linear_1.weight",
                    H3_TIME_EMBED_HIDDEN * H3_FREQ_DIM,
                )?)
            },
            time_l1_b: if curve_mode {
                None
            } else {
                Some(host_named_f32(
                    weights,
                    "time_embedder.linear_1.bias",
                    H3_TIME_EMBED_HIDDEN,
                )?)
            },
            time_l2_w: if curve_mode {
                None
            } else {
                Some(host_named_f32(
                    weights,
                    "time_embedder.linear_2.weight",
                    H3_TIME_EMBED_DIM * H3_TIME_EMBED_HIDDEN,
                )?)
            },
            time_l2_b: if curve_mode {
                None
            } else {
                Some(host_named_f32(
                    weights,
                    "time_embedder.linear_2.bias",
                    H3_TIME_EMBED_DIM,
                )?)
            },
            context_embedder_bias: host_named_f32(weights, "context_embedder.bias", h)?,
            adaln_bias,
            norm_out_bias: host_named_f32(weights, "norm_out.linear.bias", NORM_OUT_COLS)?,
            q_norm,
            k_norm,
            refiner_norm1,
            refiner_norm2,
            refiner_q_norm,
            refiner_k_norm,
            refiner_final_norm: host_named_f32(weights, "token_refiner.final_norm.weight", h)?,
        })
    }
}

/// Stream-ensure one linear weight into the device cache, then hand back the
/// empty-bytes part for the cached gemm call. Safetensors sources stream raw
/// bf16; GGUF/NVFP4 sources stream their per-tensor payload (raw bf16/f16 or
/// a quantized block stream the gemm bulk-dequantizes into bf16 scratch).
fn ensure_linear<'a>(
    weights: &H3ShardedWeights,
    name: &'a str,
    n: usize,
    k: usize,
    m: usize,
    precision: GemmPrecision,
) -> Result<GpuLinearPart<'a>> {
    let ggml_type = weights.linear_ggml_type(name)?;
    if crate::backend::gpu_quant_linear_type_supported(ggml_type) {
        gpu_weight_cache_ensure_quant(weights.dit_namespace(), name, ggml_type, n, k, || {
            weights.tensor_bytes(name).map_err(|err| err.to_string())
        })
        .map_err(DiffusionError::model)?;
    } else {
        let want_a16 = precision.f16_accumulate && m > 1;
        gpu_weight_cache_ensure(weights.dit_namespace(), name, ggml_type, n, k, want_a16, || {
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
    weights: &H3ShardedWeights,
    x: &GpuTensor,
    name: &str,
    n: usize,
    bias: &[f32],
    out_half: bool,
    precision: GemmPrecision,
) -> Result<GpuTensor> {
    let part = ensure_linear(weights, name, n, x.cols(), x.rows(), precision)?;
    let parts = [part];
    if out_half {
        gpu_linear_nt_cached_f16_with_precision(
            x,
            weights.dit_namespace(),
            &parts,
            bias,
            precision,
        )
    } else {
        gpu_linear_nt_cached_with_precision(
            x,
            weights.dit_namespace(),
            &parts,
            bias,
            precision,
        )
    }
    .map_err(DiffusionError::model)
}

fn qkv_cached(
    weights: &H3ShardedWeights,
    x: &GpuTensor,
    prefix: &str,
    out_half: bool,
    precision: GemmPrecision,
) -> Result<(GpuTensor, GpuTensor, GpuTensor)> {
    let inner = H3_HEAD_COUNT * H3_HEAD_DIM;
    let q_name = format!("{prefix}.attn.to_q.weight");
    let k_name = format!("{prefix}.attn.to_k.weight");
    let v_name = format!("{prefix}.attn.to_v.weight");
    let q_part = ensure_linear(weights, &q_name, inner, x.cols(), x.rows(), precision)?;
    let k_part = ensure_linear(weights, &k_name, inner, x.cols(), x.rows(), precision)?;
    let v_part = ensure_linear(weights, &v_name, inner, x.cols(), x.rows(), precision)?;
    let parts = [q_part, k_part, v_part];
    let qkv = if out_half {
        gpu_linear_nt_cached_f16_with_precision(
            x,
            weights.dit_namespace(),
            &parts,
            &[],
            precision,
        )
    } else {
        gpu_linear_nt_cached_with_precision(
            x,
            weights.dit_namespace(),
            &parts,
            &[],
            precision,
        )
    }
    .map_err(DiffusionError::model)?;
    let q = gpu_slice_cols(&qkv, 0, inner).map_err(DiffusionError::model)?;
    let k = gpu_slice_cols(&qkv, inner, inner).map_err(DiffusionError::model)?;
    let v = gpu_slice_cols(&qkv, inner * 2, inner).map_err(DiffusionError::model)?;
    Ok((q, k, v))
}

fn host_silu(values: &mut [f32]) {
    for value in values.iter_mut() {
        *value = *value / (1.0 + (-*value).exp());
    }
}

fn host_linear(x: &[f32], rows: usize, k: usize, w: &[f32], n: usize, bias: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; rows * n];
    for row in 0..rows {
        for j in 0..n {
            let mut sum = bias[j];
            let wr = &w[j * k..(j + 1) * k];
            let xr = &x[row * k..(row + 1) * k];
            for i in 0..k {
                sum += wr[i] * xr[i];
            }
            out[row * n + j] = sum;
        }
    }
    out
}

/// The full timestep MLP on the host (f32, mirroring the reference's f32
/// time_embedder): sinusoid -> linear_1 -> silu -> linear_2. Errors for the
/// pruned checkpoints, whose curve replaces this module.
fn compute_temb(prepared: &H3DitPrepared, timesteps: &[f32]) -> Result<Vec<f32>> {
    let (Some(l1_w), Some(l1_b), Some(l2_w), Some(l2_b)) = (
        prepared.time_l1_w.as_ref(),
        prepared.time_l1_b.as_ref(),
        prepared.time_l2_w.as_ref(),
        prepared.time_l2_b.as_ref(),
    ) else {
        return Err(DiffusionError::model(
            "h3 time_embedder absent (pruned checkpoint uses the AdaLN curve)".to_string(),
        ));
    };
    let sinusoid = h3_timestep_embedding(timesteps);
    let mut hidden = host_linear(
        &sinusoid,
        timesteps.len(),
        H3_FREQ_DIM,
        l1_w,
        H3_TIME_EMBED_HIDDEN,
        l1_b,
    );
    host_silu(&mut hidden);
    Ok(host_linear(
        &hidden,
        timesteps.len(),
        H3_TIME_EMBED_HIDDEN,
        l2_w,
        H3_TIME_EMBED_DIM,
        l2_b,
    ))
}

/// One plain refiner block over the text stream (no rope, no AdaLN).
fn refiner_block(
    weights: &H3ShardedWeights,
    prepared: &H3DitPrepared,
    hidden: GpuTensor,
    layer: usize,
    precision: GemmPrecision,
) -> Result<GpuTensor> {
    let prefix = format!("token_refiner.refiner_blocks.{layer}");
    let scale = 1.0 / (H3_HEAD_DIM as f32).sqrt();
    let normed = gpu_rms_norm_mul(
        &hidden,
        H3_HIDDEN_SIZE,
        weights.dit_namespace(),
        &format!("{prefix}.norm1"),
        &prepared.refiner_norm1[layer],
        H3_NORM_EPS,
    )
    .map_err(DiffusionError::model)?;
    let (q, k, v) = qkv_cached(weights, &normed, &prefix, false, precision)?;
    drop(normed);
    let q = gpu_rms_norm_mul(
        &q,
        H3_HEAD_DIM,
        weights.dit_namespace(),
        &format!("{prefix}.attn.norm_q"),
        &prepared.refiner_q_norm[layer],
        H3_NORM_EPS,
    )
    .map_err(DiffusionError::model)?;
    let k = gpu_rms_norm_mul(
        &k,
        H3_HEAD_DIM,
        weights.dit_namespace(),
        &format!("{prefix}.attn.norm_k"),
        &prepared.refiner_k_norm[layer],
        H3_NORM_EPS,
    )
    .map_err(DiffusionError::model)?;
    let attn = gpu_attention_packed(&q, &k, &v, H3_HEAD_COUNT, scale)
        .map_err(DiffusionError::model)?;
    drop((q, k, v));
    let attn = linear_cached(
        weights,
        &attn,
        &format!("{prefix}.attn.to_out.0.weight"),
        H3_HIDDEN_SIZE,
        &[],
        false,
        precision,
    )?;
    let hidden = crate::backend::gpu_add(&hidden, &attn).map_err(DiffusionError::model)?;
    drop(attn);
    let normed = gpu_rms_norm_mul(
        &hidden,
        H3_HIDDEN_SIZE,
        weights.dit_namespace(),
        &format!("{prefix}.norm2"),
        &prepared.refiner_norm2[layer],
        H3_NORM_EPS,
    )
    .map_err(DiffusionError::model)?;
    let gateval = linear_cached(
        weights,
        &normed,
        &format!("{prefix}.ff.net.0.proj.weight"),
        2 * H3_FFN_DIM,
        &[],
        false,
        precision,
    )?;
    drop(normed);
    let ff = gpu_swiglu_value_gate(&gateval).map_err(DiffusionError::model)?;
    drop(gateval);
    let ff = linear_cached(
        weights,
        &ff,
        &format!("{prefix}.ff.net.2.weight"),
        H3_HIDDEN_SIZE,
        &[],
        false,
        precision,
    )?;
    crate::backend::gpu_add(&hidden, &ff).map_err(DiffusionError::model)
}

/// One full DiT forward: packed inputs on the host, velocities on the host.
#[allow(clippy::too_many_arguments)]
pub fn h3_dit_forward(
    weights: &H3ShardedWeights,
    prepared: &H3DitPrepared,
    layout: &H3PackedLayout,
    plan: &H3RowTimesteps,
    rope: &H3RopeTables,
    video_rows: &[f32],
    audio_rows: &[f32],
    text_embeds: &[f32],
    precision: GemmPrecision,
    debug_blocks: &[usize],
) -> Result<H3DitForwardOutput> {
    let act16 = precision.f16_activations;
    let seq = layout.sequence_length;
    // `video_rows` carries the fl2va conditioning rows FIRST, then the
    // generated rows (the reference's `video_indices` order); t2va has zero
    // conditioning rows.
    let all_video_rows = layout.num_condition_rows + layout.num_video_rows;
    if rope.rows != seq
        || plan.indices.len() != seq
        || plan.adaln_indices.len() != seq
        || video_rows.len() != all_video_rows * H3_VIDEO_PATCH_DIM
        || audio_rows.len() != layout.num_audio_rows * H3_AUDIO_IN_CHANNELS
        || text_embeds.len() != layout.num_text_tokens * H3_TEXT_DIM
    {
        return Err(DiffusionError::workflow("h3 dit forward shape mismatch"));
    }
    let mut debug = H3DitDebug::default();
    let n_t = plan.values.len();
    let attention_scale = 1.0 / (H3_HEAD_DIM as f32).sqrt();

    // 1. Input projections. The text stream goes through the bf16
    // context_embedder + refiner; video/audio through their f32 projections.
    let text_in = gpu_upload(text_embeds, layout.num_text_tokens, H3_TEXT_DIM)
        .map_err(DiffusionError::model)?;
    let mut text = linear_cached(
        weights,
        &text_in,
        "context_embedder.weight",
        H3_HIDDEN_SIZE,
        &prepared.context_embedder_bias,
        false,
        precision,
    )?;
    drop(text_in);
    for layer in 0..H3_REFINER_DEPTH {
        text = refiner_block(weights, prepared, text, layer, precision)?;
    }
    text = gpu_rms_norm_mul(
        &text,
        H3_HIDDEN_SIZE,
        weights.dit_namespace(),
        "token_refiner.final_norm",
        &prepared.refiner_final_norm,
        H3_NORM_EPS,
    )
    .map_err(DiffusionError::model)?;
    if !debug_blocks.is_empty() {
        debug.refiner_out = Some(gpu_download(&text).map_err(DiffusionError::model)?);
    }

    let video_in = gpu_upload(video_rows, all_video_rows, H3_VIDEO_PATCH_DIM)
        .map_err(DiffusionError::model)?;
    let video = gpu_linear_f32_resident(&video_in, &prepared.proj_in_w, Some(&prepared.proj_in_b))
        .map_err(DiffusionError::model)?;
    drop(video_in);
    let audio_in = gpu_upload(audio_rows, layout.num_audio_rows, H3_AUDIO_IN_CHANNELS)
        .map_err(DiffusionError::model)?;
    let audio = gpu_linear_f32_resident(
        &audio_in,
        &prepared.audio_proj_in_w,
        Some(&prepared.audio_proj_in_b),
    )
    .map_err(DiffusionError::model)?;
    drop(audio_in);

    // Packed sequence: [text | keyframe conditions | audio | video]. The
    // conditioning rows share the video projection and split off the front of
    // the projected video block (t2va: zero conditioning rows, plain concat).
    let mut hidden = if layout.num_condition_rows == 0 {
        let text_audio = gpu_concat_rows(&text, &audio).map_err(DiffusionError::model)?;
        drop((text, audio));
        let hidden = gpu_concat_rows(&text_audio, &video).map_err(DiffusionError::model)?;
        drop((text_audio, video));
        hidden
    } else {
        let cond = gpu_slice_rows(&video, 0, layout.num_condition_rows)
            .map_err(DiffusionError::model)?;
        let generated = gpu_slice_rows(&video, layout.num_condition_rows, layout.num_video_rows)
            .map_err(DiffusionError::model)?;
        drop(video);
        let text_cond = gpu_concat_rows(&text, &cond).map_err(DiffusionError::model)?;
        drop((text, cond));
        let text_cond_audio =
            gpu_concat_rows(&text_cond, &audio).map_err(DiffusionError::model)?;
        drop((text_cond, audio));
        let hidden =
            gpu_concat_rows(&text_cond_audio, &generated).map_err(DiffusionError::model)?;
        drop((text_cond_audio, generated));
        hidden
    };

    // 2. Timestep embedding (host f32) + the padded silu(temb) row block for
    // the AdaLN projections (padding to m >= 2 keeps every cached gemm on
    // the one f16acc cache variant). Pruned checkpoints replace the timestep
    // MLP with the rank-`dim` AdaLN curve — its values are the already
    // silu'd basis, so no silu is applied on top (reference semantics).
    let (temb, temb_dim, curve_mode) = match weights.adaln_curve() {
        Some(curve) => {
            let mut rows = Vec::with_capacity(plan.values.len() * curve.dim);
            for &t in &plan.values {
                rows.extend_from_slice(&curve.temb(t));
            }
            (rows, curve.dim, true)
        }
        None => (compute_temb(prepared, &plan.values)?, H3_TIME_EMBED_DIM, false),
    };
    if !debug_blocks.is_empty() {
        debug.temb = Some(temb.clone());
    }
    let m_pad = n_t.max(2);
    let mut silu_temb = vec![0.0f32; m_pad * temb_dim];
    for row in 0..m_pad {
        let src = row.min(n_t - 1);
        silu_temb[row * temb_dim..(row + 1) * temb_dim]
            .copy_from_slice(&temb[src * temb_dim..(src + 1) * temb_dim]);
    }
    if !curve_mode {
        host_silu(&mut silu_temb);
    }
    let silu_temb_gpu =
        gpu_upload(&silu_temb, m_pad, temb_dim).map_err(DiffusionError::model)?;

    // 3. Per-row index buffers + rope tables, device-resident for the run.
    let adaln_idx = gpu_upload_u32(&plan.adaln_indices).map_err(DiffusionError::model)?;
    let t_idx = gpu_upload_u32(&plan.indices).map_err(DiffusionError::model)?;
    let rope_cos = gpu_upload(&rope.cos, seq, H3_ROT_HALF).map_err(DiffusionError::model)?;
    let rope_sin = gpu_upload(&rope.sin, seq, H3_ROT_HALF).map_err(DiffusionError::model)?;

    // 4. The 50-block stack.
    for layer in 0..H3_DEPTH {
        let prefix = format!("transformer_blocks.{layer}");

        // Per-block AdaLN table: (m_pad, 96768) viewed as (m_pad * 3, 32256)
        // rows addressed by adaln_idx. Chunks: shift_msa, scale_msa,
        // gate_msa, shift_mlp, scale_mlp, gate_mlp.
        let table = linear_cached(
            weights,
            &silu_temb_gpu,
            &format!("{prefix}.adaln_proj.linear.weight"),
            ADALN_COLS,
            &prepared.adaln_bias[layer],
            false,
            precision,
        )?;
        if layer == 0 && !debug_blocks.is_empty() {
            debug.block_adaln0 = Some(gpu_download(&table).map_err(DiffusionError::model)?);
        }

        let normed = gpu_rms_norm_mod_indexed(
            &hidden,
            &prepared.norm1_w[layer],
            &table,
            &adaln_idx,
            ADALN_STRIDE,
            H3_HIDDEN_SIZE,     // scale_msa = chunk 1
            0,                  // shift_msa = chunk 0
            H3_NORM_EPS,
            act16,
        )
        .map_err(DiffusionError::model)?;
        let (q, k, v) = qkv_cached(weights, &normed, &prefix, act16, precision)?;
        drop(normed);
        let q = gpu_rms_norm_mul(
            &q,
            H3_HEAD_DIM,
            weights.dit_namespace(),
            &format!("{prefix}.attn.norm_q"),
            &prepared.q_norm[layer],
            H3_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        let k = gpu_rms_norm_mul(
            &k,
            H3_HEAD_DIM,
            weights.dit_namespace(),
            &format!("{prefix}.attn.norm_k"),
            &prepared.k_norm[layer],
            H3_NORM_EPS,
        )
        .map_err(DiffusionError::model)?;
        let q = gpu_rope_half(&q, H3_HEAD_COUNT, H3_ROT_HALF, &rope_cos, &rope_sin)
            .map_err(DiffusionError::model)?;
        let k = gpu_rope_half(&k, H3_HEAD_COUNT, H3_ROT_HALF, &rope_cos, &rope_sin)
            .map_err(DiffusionError::model)?;
        let attn = gpu_attention_packed(&q, &k, &v, H3_HEAD_COUNT, attention_scale)
            .map_err(DiffusionError::model)?;
        drop((q, k, v));
        let attn = linear_cached(
            weights,
            &attn,
            &format!("{prefix}.attn.to_out.0.weight"),
            H3_HIDDEN_SIZE,
            &[],
            false,
            precision,
        )?;
        hidden = gpu_gated_residual_indexed(
            &hidden,
            &attn,
            &table,
            &adaln_idx,
            ADALN_STRIDE,
            2 * H3_HIDDEN_SIZE, // gate_msa = chunk 2
        )
        .map_err(DiffusionError::model)?;
        drop(attn);

        let normed = gpu_rms_norm_mod_indexed(
            &hidden,
            &prepared.norm2_w[layer],
            &table,
            &adaln_idx,
            ADALN_STRIDE,
            4 * H3_HIDDEN_SIZE, // scale_mlp = chunk 4
            3 * H3_HIDDEN_SIZE, // shift_mlp = chunk 3
            H3_NORM_EPS,
            act16,
        )
        .map_err(DiffusionError::model)?;
        let gateval = linear_cached(
            weights,
            &normed,
            &format!("{prefix}.ff.net.0.proj.weight"),
            2 * H3_FFN_DIM,
            &[],
            act16,
            precision,
        )?;
        drop(normed);
        let ff = gpu_swiglu_value_gate(&gateval).map_err(DiffusionError::model)?;
        drop(gateval);
        let ff = linear_cached(
            weights,
            &ff,
            &format!("{prefix}.ff.net.2.weight"),
            H3_HIDDEN_SIZE,
            &[],
            false,
            precision,
        )?;
        hidden = gpu_gated_residual_indexed(
            &hidden,
            &ff,
            &table,
            &adaln_idx,
            ADALN_STRIDE,
            5 * H3_HIDDEN_SIZE, // gate_mlp = chunk 5
        )
        .map_err(DiffusionError::model)?;
        drop(ff);

        if debug_blocks.contains(&layer) {
            debug
                .block_outputs
                .push((layer, gpu_download(&hidden).map_err(DiffusionError::model)?));
        }
    }

    // 5. norm_out: RMS + per-TIMESTEP (not AdaLN) shift/scale — halves are
    // SHIFT then SCALE. Table (m_pad, 10752), stride 10752 addressed by the
    // timestep index directly.
    let out_table = linear_cached(
        weights,
        &silu_temb_gpu,
        "norm_out.linear.weight",
        NORM_OUT_COLS,
        &prepared.norm_out_bias,
        false,
        precision,
    )?;
    let hidden = gpu_rms_norm_mod_indexed(
        &hidden,
        &prepared.norm_out_norm_w,
        &out_table,
        &t_idx,
        NORM_OUT_COLS,
        H3_HIDDEN_SIZE, // scale = second half
        0,              // shift = first half
        H3_NORM_EPS,
        false,
    )
    .map_err(DiffusionError::model)?;
    drop(out_table);

    // 6. The two f32 heads over their own row ranges (equivalent to running
    // them over every row and selecting after — the head is per-row linear).
    // Video velocity rows come out conditioning-first, mirroring the
    // reference's `video_indices` selection.
    let video_hidden = if layout.num_condition_rows == 0 {
        gpu_slice_rows(&hidden, layout.video_start, layout.num_video_rows)
            .map_err(DiffusionError::model)?
    } else {
        let cond = gpu_slice_rows(&hidden, layout.condition_start, layout.num_condition_rows)
            .map_err(DiffusionError::model)?;
        let generated = gpu_slice_rows(&hidden, layout.video_start, layout.num_video_rows)
            .map_err(DiffusionError::model)?;
        gpu_concat_rows(&cond, &generated).map_err(DiffusionError::model)?
    };
    let audio_hidden = gpu_slice_rows(&hidden, layout.audio_start, layout.num_audio_rows)
        .map_err(DiffusionError::model)?;
    drop(hidden);
    let video_out =
        gpu_linear_f32_resident(&video_hidden, &prepared.proj_out_w, Some(&prepared.proj_out_b))
            .map_err(DiffusionError::model)?;
    let audio_out = gpu_linear_f32_resident(
        &audio_hidden,
        &prepared.audio_proj_out_w,
        Some(&prepared.audio_proj_out_b),
    )
    .map_err(DiffusionError::model)?;
    let video_velocity = gpu_download(&video_out).map_err(DiffusionError::model)?;
    let audio_velocity = gpu_download(&audio_out).map_err(DiffusionError::model)?;

    Ok(H3DitForwardOutput {
        video_velocity,
        audio_velocity,
        debug,
    })
}
