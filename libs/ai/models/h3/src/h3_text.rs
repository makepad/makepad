//! MiniMax H3 text conditioner: Qwen3-VL-32B's language model truncated to
//! its first 50 decoder layers, read UNNORMALIZED (no final norm, no LM
//! head). For the t2va workflow the input is the raw prompt tokenization
//! (no chat template, no special tokens) and mrope with equal t/h/w
//! positions collapses to plain 1D rotate-half RoPE at theta 5e6 — so the
//! t2va path needs no vision tower and no mrope sectioning at all.
//!
//! The fl2va (image-conditioned) path adds the Qwen3-VL vision tower: 27
//! bidirectional pre-LN blocks at hidden 1152 over 16x16 patches, learned
//! 48x48 position table bilinearly resampled (align-corners), 2D rotate-half
//! rope over the full head_dim 72, a main 2x2 patch merger to 5120-wide
//! image embeds plus three deepstack mergers (after blocks 8/16/24) whose
//! outputs are ADDED to the text hidden state at the image rows after
//! decoder layers 0/1/2. Vision rows use interleaved mrope ([24,20,20]
//! sections over the 64 rope channels) instead of equal-axis positions.
//!
//! Weights stream from the sharded safetensors into the device cache under
//! the `h3te` namespace (vision weights keep their `model.visual.*` names as
//! cache keys); after the encode pass the caller evicts the namespace to
//! make room for the DiT (`gpu_weight_cache_evict_prefix`).

use crate::backend::{
    gpu_add, gpu_attention_packed, gpu_attention_packed_causal, gpu_concat_cols,
    gpu_concat_rows, gpu_download, gpu_gelu, gpu_gelu_erf, gpu_gemm_f16acc_enabled,
    gpu_layer_norm_mul_add, gpu_linear_nt_cached, gpu_rms_norm_mul, gpu_rope_half,
    gpu_slice_cols, gpu_slice_rows, gpu_swiglu_value_gate, gpu_upload,
    gpu_weight_cache_ensure, gpu_weight_cache_ensure_quant, GpuLinearPart, GpuTensor,
};
use crate::h3::H3ShardedWeights;
use crate::{DiffusionError, Result};

pub const H3_TE_NAMESPACE: &str = "h3te";
pub const H3_TE_LAYERS: usize = 50;
pub const H3_TE_HIDDEN: usize = 5120;
pub const H3_TE_Q_HEADS: usize = 64;
pub const H3_TE_KV_HEADS: usize = 8;
pub const H3_TE_HEAD_DIM: usize = 128;
pub const H3_TE_FFN: usize = 25600;
pub const H3_TE_ROPE_THETA: f32 = 5_000_000.0;
pub const H3_TE_RMS_EPS: f32 = 1e-6;

fn layer_prefix(layer: usize) -> String {
    format!("model.language_model.layers.{layer}")
}

fn ensure_linear<'a>(
    weights: &H3ShardedWeights,
    name: &'a str,
    n: usize,
    k: usize,
    m: usize,
) -> Result<GpuLinearPart<'a>> {
    // Safetensors sources stream raw bf16; GGUF/NVFP4 sources stream their
    // per-tensor payload (raw bf16 for the vision tower, quantized blocks
    // for the language stack) which the gemm bulk-dequantizes into bf16
    // scratch.
    let ggml_type = weights.linear_ggml_type(name)?;
    if crate::backend::gpu_quant_linear_type_supported(ggml_type) {
        gpu_weight_cache_ensure_quant(weights.te_namespace(), name, ggml_type, n, k, || {
            weights.tensor_bytes(name).map_err(|err| err.to_string())
        })
        .map_err(DiffusionError::model)?;
    } else {
        let want_a16 = gpu_gemm_f16acc_enabled() && m > 1;
        gpu_weight_cache_ensure(weights.te_namespace(), name, ggml_type, n, k, want_a16, || {
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
) -> Result<GpuTensor> {
    let part = ensure_linear(weights, name, n, x.cols(), x.rows())?;
    let parts = [part];
    gpu_linear_nt_cached(x, weights.te_namespace(), &parts, &[]).map_err(DiffusionError::model)
}

/// Host-cached per-layer norm scales (small, read once).
pub struct H3TextEncoderPrepared {
    input_norm: Vec<Vec<f32>>,
    post_attn_norm: Vec<Vec<f32>>,
    q_norm: Vec<Vec<f32>>,
    k_norm: Vec<Vec<f32>>,
    /// 1D rotate-half rope tables grow with the token count; cached per call.
    rope_inv_freq: Vec<f32>,
}

impl H3TextEncoderPrepared {
    pub fn prepare(weights: &H3ShardedWeights) -> Result<Self> {
        let mut input_norm = Vec::with_capacity(H3_TE_LAYERS);
        let mut post_attn_norm = Vec::with_capacity(H3_TE_LAYERS);
        let mut q_norm = Vec::with_capacity(H3_TE_LAYERS);
        let mut k_norm = Vec::with_capacity(H3_TE_LAYERS);
        for layer in 0..H3_TE_LAYERS {
            let prefix = layer_prefix(layer);
            input_norm.push(weights.tensor_f32(&format!("{prefix}.input_layernorm.weight"))?);
            post_attn_norm
                .push(weights.tensor_f32(&format!("{prefix}.post_attention_layernorm.weight"))?);
            q_norm.push(weights.tensor_f32(&format!("{prefix}.self_attn.q_norm.weight"))?);
            k_norm.push(weights.tensor_f32(&format!("{prefix}.self_attn.k_norm.weight"))?);
        }
        let half = H3_TE_HEAD_DIM / 2;
        let mut rope_inv_freq = Vec::with_capacity(half);
        for j in 0..half {
            rope_inv_freq
                .push(1.0 / H3_TE_ROPE_THETA.powf(2.0 * j as f32 / H3_TE_HEAD_DIM as f32));
        }
        Ok(Self {
            input_norm,
            post_attn_norm,
            q_norm,
            k_norm,
            rope_inv_freq,
        })
    }
}

/// Encode one tokenized prompt: token ids -> (n_tokens, 5120) hidden state
/// after decoder layer 50, unnormalized, f32 host values (bf16-exact).
pub fn h3_text_encode(
    weights: &H3ShardedWeights,
    prepared: &H3TextEncoderPrepared,
    token_ids: &[u32],
) -> Result<Vec<f32>> {
    h3_text_encode_progress(weights, prepared, token_ids, None)
}

/// [`h3_text_encode`] with a per-layer progress callback `(done, total)` —
/// the encode re-streams every layer's weights from disk (the namespace is
/// evicted after each prompt), so this is what makes the 30s+ TE phase move.
pub fn h3_text_encode_progress(
    weights: &H3ShardedWeights,
    prepared: &H3TextEncoderPrepared,
    token_ids: &[u32],
    mut on_layer: Option<&mut dyn FnMut(usize, usize)>,
) -> Result<Vec<f32>> {
    let n = token_ids.len();
    if n == 0 {
        return Err(DiffusionError::workflow("h3 text encode: empty prompt"));
    }
    // Embedding rows straight off the shard file (no device copy of the
    // 1.5GB embedding matrix for a ~hundred-token prompt).
    let mut embeds = vec![0.0f32; n * H3_TE_HIDDEN];
    for (row, id) in token_ids.iter().enumerate() {
        let values = weights
            .tensor_row_f32("model.language_model.embed_tokens.weight", *id as u64)?;
        if values.len() != H3_TE_HIDDEN {
            return Err(DiffusionError::model(format!(
                "h3 te embed row {} width {}",
                id,
                values.len()
            )));
        }
        embeds[row * H3_TE_HIDDEN..(row + 1) * H3_TE_HIDDEN].copy_from_slice(&values);
    }
    let mut hidden = gpu_upload(&embeds, n, H3_TE_HIDDEN).map_err(DiffusionError::model)?;

    // Rope tables: positions 0..n-1, full-dim rotate-half.
    let half = H3_TE_HEAD_DIM / 2;
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

    for layer in 0..H3_TE_LAYERS {
        if let Some(on_layer) = on_layer.as_deref_mut() {
            on_layer(layer + 1, H3_TE_LAYERS);
        }
        hidden = text_layer_forward(weights, prepared, layer, hidden, &rope_cos, &rope_sin, n)?;
    }

    gpu_download(&hidden).map_err(DiffusionError::model)
}

/// One Qwen3-VL text decoder layer (pre-RMS attention with per-head QK
/// norms + rotate-half rope + causal GQA attention, then SwiGLU FFN).
/// Shared verbatim between the t2va and fl2va encode paths — the two only
/// differ in the embedding rows, the rope tables, and the fl2va deepstack
/// adds between the first layers.
fn text_layer_forward(
    weights: &H3ShardedWeights,
    prepared: &H3TextEncoderPrepared,
    layer: usize,
    hidden: GpuTensor,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
    n: usize,
) -> Result<GpuTensor> {
    let half = H3_TE_HEAD_DIM / 2;
    let q_inner = H3_TE_Q_HEADS * H3_TE_HEAD_DIM; // 8192
    let kv_inner = H3_TE_KV_HEADS * H3_TE_HEAD_DIM; // 1024
    let scale = 1.0 / (H3_TE_HEAD_DIM as f32).sqrt();
    let prefix = layer_prefix(layer);
    let mut hidden = hidden;

    let normed = gpu_rms_norm_mul(
        &hidden,
        H3_TE_HIDDEN,
        weights.te_namespace(),
        &format!("{prefix}.input_layernorm"),
        &prepared.input_norm[layer],
        H3_TE_RMS_EPS,
    )
    .map_err(DiffusionError::model)?;

    // Fused q|k|v projection when the storage type is uniform. The pinned
    // Q4_K_M text encoder deliberately keeps V as Q6_K while Q/K are Q4_K;
    // the CUDA cache rejects mixed-type parts, so that checkpoint needs
    // three independent projections. BF16 and uniform quant checkpoints
    // retain the single-GEMM fast path.
    let q_name = format!("{prefix}.self_attn.q_proj.weight");
    let k_name = format!("{prefix}.self_attn.k_proj.weight");
    let v_name = format!("{prefix}.self_attn.v_proj.weight");
    let q_part = ensure_linear(weights, &q_name, q_inner, H3_TE_HIDDEN, n)?;
    let k_part = ensure_linear(weights, &k_name, kv_inner, H3_TE_HIDDEN, n)?;
    let v_part = ensure_linear(weights, &v_name, kv_inner, H3_TE_HIDDEN, n)?;
    let (q, k, v) = if q_part.bt_ggml_type == k_part.bt_ggml_type
        && q_part.bt_ggml_type == v_part.bt_ggml_type
    {
        let parts = [q_part, k_part, v_part];
        let qkv = gpu_linear_nt_cached(&normed, weights.te_namespace(), &parts, &[])
            .map_err(DiffusionError::model)?;
        let q = gpu_slice_cols(&qkv, 0, q_inner).map_err(DiffusionError::model)?;
        let k = gpu_slice_cols(&qkv, q_inner, kv_inner).map_err(DiffusionError::model)?;
        let v = gpu_slice_cols(&qkv, q_inner + kv_inner, kv_inner)
            .map_err(DiffusionError::model)?;
        drop(qkv);
        (q, k, v)
    } else {
        let q = gpu_linear_nt_cached(&normed, weights.te_namespace(), &[q_part], &[])
            .map_err(DiffusionError::model)?;
        let k = gpu_linear_nt_cached(&normed, weights.te_namespace(), &[k_part], &[])
            .map_err(DiffusionError::model)?;
        let v = gpu_linear_nt_cached(&normed, weights.te_namespace(), &[v_part], &[])
            .map_err(DiffusionError::model)?;
        (q, k, v)
    };
    drop(normed);

    // Per-head RMS norms (eps 1e-6), then full-dim rotate-half rope.
    let q = gpu_rms_norm_mul(
        &q,
        H3_TE_HEAD_DIM,
        weights.te_namespace(),
        &format!("{prefix}.self_attn.q_norm"),
        &prepared.q_norm[layer],
        H3_TE_RMS_EPS,
    )
    .map_err(DiffusionError::model)?;
    let k = gpu_rms_norm_mul(
        &k,
        H3_TE_HEAD_DIM,
        weights.te_namespace(),
        &format!("{prefix}.self_attn.k_norm"),
        &prepared.k_norm[layer],
        H3_TE_RMS_EPS,
    )
    .map_err(DiffusionError::model)?;
    let q = gpu_rope_half(&q, H3_TE_Q_HEADS, half, rope_cos, rope_sin)
        .map_err(DiffusionError::model)?;
    let k = gpu_rope_half(&k, H3_TE_KV_HEADS, half, rope_cos, rope_sin)
        .map_err(DiffusionError::model)?;

    // GQA 64/8: expand the kv heads 8x by column-slicing per kv head and
    // concatenating each slice 8 times (tiny at prompt lengths).
    let mut k_heads = Vec::with_capacity(H3_TE_KV_HEADS);
    let mut v_heads = Vec::with_capacity(H3_TE_KV_HEADS);
    for head in 0..H3_TE_KV_HEADS {
        k_heads.push(
            gpu_slice_cols(&k, head * H3_TE_HEAD_DIM, H3_TE_HEAD_DIM)
                .map_err(DiffusionError::model)?,
        );
        v_heads.push(
            gpu_slice_cols(&v, head * H3_TE_HEAD_DIM, H3_TE_HEAD_DIM)
                .map_err(DiffusionError::model)?,
        );
    }
    drop((k, v));
    let group = H3_TE_Q_HEADS / H3_TE_KV_HEADS;
    let mut k_refs = Vec::with_capacity(H3_TE_Q_HEADS);
    let mut v_refs = Vec::with_capacity(H3_TE_Q_HEADS);
    for head in 0..H3_TE_KV_HEADS {
        for _ in 0..group {
            k_refs.push(&k_heads[head]);
            v_refs.push(&v_heads[head]);
        }
    }
    let k_full = gpu_concat_cols(&k_refs).map_err(DiffusionError::model)?;
    let v_full = gpu_concat_cols(&v_refs).map_err(DiffusionError::model)?;
    drop((k_heads, v_heads));

    let attn = gpu_attention_packed_causal(&q, &k_full, &v_full, H3_TE_Q_HEADS, scale)
        .map_err(DiffusionError::model)?;
    drop((q, k_full, v_full));
    let attn = linear_cached(
        weights,
        &attn,
        &format!("{prefix}.self_attn.o_proj.weight"),
        H3_TE_HIDDEN,
    )?;
    hidden = gpu_add(&hidden, &attn).map_err(DiffusionError::model)?;
    drop(attn);

    // FFN: [up | gate] fused gemm feeds the value-first SwiGLU kernel
    // (out = up * silu(gate)), then down.
    let normed = gpu_rms_norm_mul(
        &hidden,
        H3_TE_HIDDEN,
        weights.te_namespace(),
        &format!("{prefix}.post_attention_layernorm"),
        &prepared.post_attn_norm[layer],
        H3_TE_RMS_EPS,
    )
    .map_err(DiffusionError::model)?;
    let up_name = format!("{prefix}.mlp.up_proj.weight");
    let gate_name = format!("{prefix}.mlp.gate_proj.weight");
    let parts = [
        ensure_linear(weights, &up_name, H3_TE_FFN, H3_TE_HIDDEN, n)?,
        ensure_linear(weights, &gate_name, H3_TE_FFN, H3_TE_HIDDEN, n)?,
    ];
    let up_gate = gpu_linear_nt_cached(&normed, weights.te_namespace(), &parts, &[])
        .map_err(DiffusionError::model)?;
    drop(normed);
    let ff = gpu_swiglu_value_gate(&up_gate).map_err(DiffusionError::model)?;
    drop(up_gate);
    let ff = linear_cached(
        weights,
        &ff,
        &format!("{prefix}.mlp.down_proj.weight"),
        H3_TE_HIDDEN,
    )?;
    hidden = gpu_add(&hidden, &ff).map_err(DiffusionError::model)?;
    drop(ff);
    Ok(hidden)
}

/// Free every cached TE weight buffer (before loading the DiT). Covers the
/// vision tower too — its weights cache under the same namespace with their
/// `model.visual.*` tensor names as keys.
pub fn h3_text_encoder_evict() -> Result<usize> {
    crate::backend::gpu_weight_cache_evict_prefix(&format!("{H3_TE_NAMESPACE}::"))
        .map_err(DiffusionError::model)
}

// ---------------------------------------------------------------------------
// Qwen3-VL-32B vision tower (fl2va image conditioning).
// ---------------------------------------------------------------------------

pub const H3_VIS_DEPTH: usize = 27;
pub const H3_VIS_HIDDEN: usize = 1152;
pub const H3_VIS_HEADS: usize = 16;
pub const H3_VIS_HEAD_DIM: usize = 72;
pub const H3_VIS_FFN: usize = 4304;
pub const H3_VIS_OUT_HIDDEN: usize = 5120;
pub const H3_VIS_PATCH: usize = 16;
pub const H3_VIS_TEMPORAL_PATCH: usize = 2;
pub const H3_VIS_MERGE: usize = 2;
pub const H3_VIS_PATCH_DIM: usize =
    3 * H3_VIS_TEMPORAL_PATCH * H3_VIS_PATCH * H3_VIS_PATCH; // 1536
pub const H3_VIS_POS_SIDE: usize = 48; // sqrt(num_position_embeddings 2304)
pub const H3_VIS_LN_EPS: f32 = 1e-6;
pub const H3_VIS_ROPE_THETA: f32 = 10_000.0;
pub const H3_VIS_DEEPSTACK_BLOCKS: [usize; 3] = [8, 16, 24];
/// `<|image_pad|>` — the placeholder token whose embedding rows are replaced
/// by the vision tower's image embeds.
pub const H3_IMAGE_PAD_TOKEN: u32 = 151655;

fn vis_name(suffix: &str) -> String {
    format!("model.visual.{suffix}")
}

/// Canvas RGB u8 -> Qwen2-VL pixel_values rows. The canvas is already a
/// 32-multiple (smart_resize with factor 32 is a no-op there), so this is
/// only normalize ((x/255 - 0.5)/0.5) + patchify: rows in block-major order
/// over 2x2 merge blocks of 16x16 patches, each row laid out as
/// [C=3][T=2 duplicated][py=16][px=16] = 1536 floats. Returns
/// (pixel_values rows, grid_h, grid_w).
pub fn h3_vision_preprocess(
    canvas_rgb: &[u8],
    width: usize,
    height: usize,
) -> Result<(Vec<f32>, usize, usize)> {
    if width == 0 || height == 0 || width % 32 != 0 || height % 32 != 0 {
        return Err(DiffusionError::workflow(format!(
            "h3 vision preprocess: canvas {width}x{height} must be a positive 32-multiple"
        )));
    }
    if canvas_rgb.len() != width * height * 3 {
        return Err(DiffusionError::workflow(format!(
            "h3 vision preprocess: canvas byte length {} != {width}x{height}x3",
            canvas_rgb.len()
        )));
    }
    let gh = height / H3_VIS_PATCH;
    let gw = width / H3_VIS_PATCH;
    // (x * (1/255) - 0.5) / 0.5, mirroring the reference rescale-then-
    // normalize order in f32.
    let mut norm = [0.0f32; 256];
    for (value, out) in norm.iter_mut().enumerate() {
        *out = (value as f32 * (1.0 / 255.0) - 0.5) / 0.5;
    }
    let mut rows = vec![0.0f32; gh * gw * H3_VIS_PATCH_DIM];
    let mut row = 0usize;
    for block_row in 0..gh / 2 {
        for block_col in 0..gw / 2 {
            for merge_y in 0..2 {
                for merge_x in 0..2 {
                    let gy = block_row * 2 + merge_y;
                    let gx = block_col * 2 + merge_x;
                    let mut e = row * H3_VIS_PATCH_DIM;
                    for c in 0..3 {
                        for _t in 0..H3_VIS_TEMPORAL_PATCH {
                            for py in 0..H3_VIS_PATCH {
                                let y = gy * H3_VIS_PATCH + py;
                                let base = (y * width + gx * H3_VIS_PATCH) * 3 + c;
                                for px in 0..H3_VIS_PATCH {
                                    rows[e] = norm[canvas_rgb[base + px * 3] as usize];
                                    e += 1;
                                }
                            }
                        }
                    }
                    row += 1;
                }
            }
        }
    }
    Ok((rows, gh, gw))
}

/// Decode a block-major patch index into its (row, col) grid position — the
/// shared ordering of patchify rows, vision rope positions, and the pos-embed
/// interpolation (merge blocks raster-major, merge_y then merge_x inside).
fn vis_block_major_row_col(index: usize, gw: usize) -> (usize, usize) {
    let blocks_w = gw / H3_VIS_MERGE;
    let in_col = index % H3_VIS_MERGE;
    let in_row = (index / H3_VIS_MERGE) % H3_VIS_MERGE;
    let block_col = (index / (H3_VIS_MERGE * H3_VIS_MERGE)) % blocks_w;
    let block_row = index / (H3_VIS_MERGE * H3_VIS_MERGE * blocks_w);
    (
        block_row * H3_VIS_MERGE + in_row,
        block_col * H3_VIS_MERGE + in_col,
    )
}

/// Per-axis bilinear align-corners taps into a `side`-length table for target
/// position `index` on an axis of length `size` — the f32 arithmetic mirrors
/// `_interpolation_axis_taps_weights` (transformers vision_utils.py).
fn vis_interp_axis_taps(index: usize, size: usize, side: usize) -> ([usize; 2], [f32; 2]) {
    let denom = size.saturating_sub(1).max(1) as f32;
    let src = index as f32 * (side as f32 - 1.0) / denom;
    let floor = src.floor();
    let mut taps = [0usize; 2];
    let mut weights = [0.0f32; 2];
    for offset in 0..2usize {
        let tap = (floor as i64 + offset as i64).clamp(0, side as i64 - 1);
        taps[offset] = tap as usize;
        let distance = (src - floor - offset as f32).abs();
        weights[offset] = (1.0 - distance).max(0.0);
    }
    (taps, weights)
}

/// Vision rope angles per patch row: (gh*gw, 36) where columns 0..18 are
/// h_pos * inv_freq and 18..36 are w_pos * inv_freq (inv_freq =
/// 10000^-(2j/36), j<18). The rotate-half tables duplicate these over the
/// full head_dim 72 (cos[i+36] == cos[i]), which is exactly the shared-half
/// table `gpu_rope_half` consumes.
pub fn h3_vision_rope_angles(gh: usize, gw: usize) -> Vec<f32> {
    let half = H3_VIS_HEAD_DIM / 2; // 36
    let freq_dim = half / 2; // 18
    let mut inv_freq = [0.0f32; 18];
    for (j, inv) in inv_freq.iter_mut().enumerate() {
        *inv = 1.0 / H3_VIS_ROPE_THETA.powf(2.0 * j as f32 / half as f32);
    }
    let seq = gh * gw;
    let mut angles = vec![0.0f32; seq * half];
    for i in 0..seq {
        let (row, col) = vis_block_major_row_col(i, gw);
        for j in 0..freq_dim {
            angles[i * half + j] = row as f32 * inv_freq[j];
            angles[i * half + freq_dim + j] = col as f32 * inv_freq[j];
        }
    }
    angles
}

/// Host-cached vision-tower constants: every LayerNorm weight/bias, every
/// linear bias, and the 48x48 learned position table (~11MB, read once).
pub struct H3VisionPrepared {
    patch_bias: Vec<f32>,
    pos_embed: Vec<f32>, // 2304 x 1152
    norm1_w: Vec<Vec<f32>>,
    norm1_b: Vec<Vec<f32>>,
    norm2_w: Vec<Vec<f32>>,
    norm2_b: Vec<Vec<f32>>,
    qkv_bias: Vec<Vec<f32>>,
    proj_bias: Vec<Vec<f32>>,
    fc1_bias: Vec<Vec<f32>>,
    fc2_bias: Vec<Vec<f32>>,
    merger_norm_w: Vec<f32>,
    merger_norm_b: Vec<f32>,
    merger_fc1_bias: Vec<f32>,
    merger_fc2_bias: Vec<f32>,
    ds_norm_w: Vec<Vec<f32>>,
    ds_norm_b: Vec<Vec<f32>>,
    ds_fc1_bias: Vec<Vec<f32>>,
    ds_fc2_bias: Vec<Vec<f32>>,
}

impl H3VisionPrepared {
    pub fn prepare(weights: &H3ShardedWeights) -> Result<Self> {
        let read = |name: String, want: usize| -> Result<Vec<f32>> {
            let values = weights.tensor_f32(&name)?;
            if values.len() != want {
                return Err(DiffusionError::model(format!(
                    "h3 vision tensor {name}: {} values, expected {want}",
                    values.len()
                )));
            }
            Ok(values)
        };
        let merge_hidden = H3_VIS_HIDDEN * H3_VIS_MERGE * H3_VIS_MERGE; // 4608
        let patch_bias = read(vis_name("patch_embed.proj.bias"), H3_VIS_HIDDEN)?;
        let pos_embed = read(
            vis_name("pos_embed.weight"),
            H3_VIS_POS_SIDE * H3_VIS_POS_SIDE * H3_VIS_HIDDEN,
        )?;
        let mut norm1_w = Vec::with_capacity(H3_VIS_DEPTH);
        let mut norm1_b = Vec::with_capacity(H3_VIS_DEPTH);
        let mut norm2_w = Vec::with_capacity(H3_VIS_DEPTH);
        let mut norm2_b = Vec::with_capacity(H3_VIS_DEPTH);
        let mut qkv_bias = Vec::with_capacity(H3_VIS_DEPTH);
        let mut proj_bias = Vec::with_capacity(H3_VIS_DEPTH);
        let mut fc1_bias = Vec::with_capacity(H3_VIS_DEPTH);
        let mut fc2_bias = Vec::with_capacity(H3_VIS_DEPTH);
        for block in 0..H3_VIS_DEPTH {
            let prefix = format!("blocks.{block}");
            norm1_w.push(read(vis_name(&format!("{prefix}.norm1.weight")), H3_VIS_HIDDEN)?);
            norm1_b.push(read(vis_name(&format!("{prefix}.norm1.bias")), H3_VIS_HIDDEN)?);
            norm2_w.push(read(vis_name(&format!("{prefix}.norm2.weight")), H3_VIS_HIDDEN)?);
            norm2_b.push(read(vis_name(&format!("{prefix}.norm2.bias")), H3_VIS_HIDDEN)?);
            qkv_bias.push(read(
                vis_name(&format!("{prefix}.attn.qkv.bias")),
                3 * H3_VIS_HIDDEN,
            )?);
            proj_bias.push(read(
                vis_name(&format!("{prefix}.attn.proj.bias")),
                H3_VIS_HIDDEN,
            )?);
            fc1_bias.push(read(
                vis_name(&format!("{prefix}.mlp.linear_fc1.bias")),
                H3_VIS_FFN,
            )?);
            fc2_bias.push(read(
                vis_name(&format!("{prefix}.mlp.linear_fc2.bias")),
                H3_VIS_HIDDEN,
            )?);
        }
        let merger_norm_w = read(vis_name("merger.norm.weight"), H3_VIS_HIDDEN)?;
        let merger_norm_b = read(vis_name("merger.norm.bias"), H3_VIS_HIDDEN)?;
        let merger_fc1_bias = read(vis_name("merger.linear_fc1.bias"), merge_hidden)?;
        let merger_fc2_bias = read(vis_name("merger.linear_fc2.bias"), H3_VIS_OUT_HIDDEN)?;
        let mut ds_norm_w = Vec::with_capacity(H3_VIS_DEEPSTACK_BLOCKS.len());
        let mut ds_norm_b = Vec::with_capacity(H3_VIS_DEEPSTACK_BLOCKS.len());
        let mut ds_fc1_bias = Vec::with_capacity(H3_VIS_DEEPSTACK_BLOCKS.len());
        let mut ds_fc2_bias = Vec::with_capacity(H3_VIS_DEEPSTACK_BLOCKS.len());
        for k in 0..H3_VIS_DEEPSTACK_BLOCKS.len() {
            let prefix = format!("deepstack_merger_list.{k}");
            // Postshuffle mergers: the LayerNorm spans the merged 4608 row.
            ds_norm_w.push(read(vis_name(&format!("{prefix}.norm.weight")), merge_hidden)?);
            ds_norm_b.push(read(vis_name(&format!("{prefix}.norm.bias")), merge_hidden)?);
            ds_fc1_bias.push(read(
                vis_name(&format!("{prefix}.linear_fc1.bias")),
                merge_hidden,
            )?);
            ds_fc2_bias.push(read(
                vis_name(&format!("{prefix}.linear_fc2.bias")),
                H3_VIS_OUT_HIDDEN,
            )?);
        }
        Ok(Self {
            patch_bias,
            pos_embed,
            norm1_w,
            norm1_b,
            norm2_w,
            norm2_b,
            qkv_bias,
            proj_bias,
            fc1_bias,
            fc2_bias,
            merger_norm_w,
            merger_norm_b,
            merger_fc1_bias,
            merger_fc2_bias,
            ds_norm_w,
            ds_norm_b,
            ds_fc1_bias,
            ds_fc2_bias,
        })
    }
}

/// Vision tower output, host-resident: the main merger's image embeds
/// (gh*gw/4 rows x 5120) and the three deepstack merger outputs (same shape),
/// consumed by `h3_text_encode_fl2va`.
pub struct H3VisionOutput {
    pub image_embeds: Vec<f32>,
    pub deepstack: [Vec<f32>; 3],
}

/// Validation taps out of the vision tower (dump tensor names in comments).
#[derive(Default)]
pub struct H3VisionTaps {
    /// After patch_embed linear+bias, BEFORE the pos-embed add
    /// (vis_patch_embed_out).
    pub patch_embed_out: Vec<f32>,
    /// After the pos-embed add (vis_block0_in).
    pub block0_in: Vec<f32>,
    /// Hidden AFTER the requested block indices (vis_block{N}_out).
    pub block_outs: Vec<(usize, Vec<f32>)>,
}

fn linear_cached_bias(
    weights: &H3ShardedWeights,
    x: &GpuTensor,
    name: &str,
    n: usize,
    bias: &[f32],
) -> Result<GpuTensor> {
    let part = ensure_linear(weights, name, n, x.cols(), x.rows())?;
    let parts = [part];
    gpu_linear_nt_cached(x, weights.te_namespace(), &parts, bias).map_err(DiffusionError::model)
}

/// Reinterpret (rows, cols) as (rows/4, cols*4): 4 consecutive block-major
/// rows == one 2x2 merge block. Rows are contiguous row-major on the device,
/// so a download/upload round trip is a byte-identity (seq is tiny).
fn vis_regroup_rows_4x(x: &GpuTensor) -> Result<GpuTensor> {
    if x.rows() % 4 != 0 {
        return Err(DiffusionError::model(format!(
            "h3 vision merge: {} rows not a multiple of 4",
            x.rows()
        )));
    }
    let host = gpu_download(x).map_err(DiffusionError::model)?;
    gpu_upload(&host, x.rows() / 4, x.cols() * 4).map_err(DiffusionError::model)
}

fn vision_block_forward(
    weights: &H3ShardedWeights,
    prepared: &H3VisionPrepared,
    block: usize,
    hidden: GpuTensor,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
    seq: usize,
) -> Result<GpuTensor> {
    let prefix = format!("blocks.{block}");
    let scale = 1.0 / (H3_VIS_HEAD_DIM as f32).sqrt();
    let rot_half = H3_VIS_HEAD_DIM / 2; // rotate-half over the FULL head dim

    // Pre-LN (LayerNorm with bias) -> fused qkv + bias.
    let normed = gpu_layer_norm_mul_add(
        &hidden,
        &prepared.norm1_w[block],
        &prepared.norm1_b[block],
        H3_VIS_LN_EPS,
    )
    .map_err(DiffusionError::model)?;
    let qkv_name = vis_name(&format!("{prefix}.attn.qkv.weight"));
    let qkv_part = ensure_linear(weights, &qkv_name, 3 * H3_VIS_HIDDEN, H3_VIS_HIDDEN, seq)?;
    let parts = [qkv_part];
    let qkv = gpu_linear_nt_cached(&normed, weights.te_namespace(), &parts, &prepared.qkv_bias[block])
        .map_err(DiffusionError::model)?;
    drop(normed);
    let q = gpu_slice_cols(&qkv, 0, H3_VIS_HIDDEN).map_err(DiffusionError::model)?;
    let k = gpu_slice_cols(&qkv, H3_VIS_HIDDEN, H3_VIS_HIDDEN).map_err(DiffusionError::model)?;
    let v = gpu_slice_cols(&qkv, 2 * H3_VIS_HIDDEN, H3_VIS_HIDDEN)
        .map_err(DiffusionError::model)?;
    drop(qkv);

    // 2D rope over the full head_dim 72, then full bidirectional attention
    // (a single image is one attention segment).
    let q = gpu_rope_half(&q, H3_VIS_HEADS, rot_half, rope_cos, rope_sin)
        .map_err(DiffusionError::model)?;
    let k = gpu_rope_half(&k, H3_VIS_HEADS, rot_half, rope_cos, rope_sin)
        .map_err(DiffusionError::model)?;
    let attn = gpu_attention_packed(&q, &k, &v, H3_VIS_HEADS, scale)
        .map_err(DiffusionError::model)?;
    drop((q, k, v));
    let attn = linear_cached_bias(
        weights,
        &attn,
        &vis_name(&format!("{prefix}.attn.proj.weight")),
        H3_VIS_HIDDEN,
        &prepared.proj_bias[block],
    )?;
    let hidden = gpu_add(&hidden, &attn).map_err(DiffusionError::model)?;
    drop(attn);

    // MLP: LN -> fc1 + bias -> gelu (tanh approximation) -> fc2 + bias.
    let normed = gpu_layer_norm_mul_add(
        &hidden,
        &prepared.norm2_w[block],
        &prepared.norm2_b[block],
        H3_VIS_LN_EPS,
    )
    .map_err(DiffusionError::model)?;
    let fc1 = linear_cached_bias(
        weights,
        &normed,
        &vis_name(&format!("{prefix}.mlp.linear_fc1.weight")),
        H3_VIS_FFN,
        &prepared.fc1_bias[block],
    )?;
    drop(normed);
    let act = gpu_gelu(&fc1).map_err(DiffusionError::model)?;
    drop(fc1);
    let fc2 = linear_cached_bias(
        weights,
        &act,
        &vis_name(&format!("{prefix}.mlp.linear_fc2.weight")),
        H3_VIS_HIDDEN,
        &prepared.fc2_bias[block],
    )?;
    drop(act);
    gpu_add(&hidden, &fc2).map_err(DiffusionError::model)
}

/// Main merger (use_postshuffle_norm=false): LayerNorm(1152) on the patch
/// rows FIRST, then group 4 rows -> fc1 4608 -> exact-erf GELU -> fc2 5120.
fn vision_main_merger(
    weights: &H3ShardedWeights,
    prepared: &H3VisionPrepared,
    hidden: &GpuTensor,
) -> Result<Vec<f32>> {
    let merge_hidden = H3_VIS_HIDDEN * 4;
    let normed = gpu_layer_norm_mul_add(
        hidden,
        &prepared.merger_norm_w,
        &prepared.merger_norm_b,
        H3_VIS_LN_EPS,
    )
    .map_err(DiffusionError::model)?;
    let grouped = vis_regroup_rows_4x(&normed)?;
    drop(normed);
    let fc1 = linear_cached_bias(
        weights,
        &grouped,
        &vis_name("merger.linear_fc1.weight"),
        merge_hidden,
        &prepared.merger_fc1_bias,
    )?;
    drop(grouped);
    let act = gpu_gelu_erf(&fc1).map_err(DiffusionError::model)?;
    drop(fc1);
    let fc2 = linear_cached_bias(
        weights,
        &act,
        &vis_name("merger.linear_fc2.weight"),
        H3_VIS_OUT_HIDDEN,
        &prepared.merger_fc2_bias,
    )?;
    gpu_download(&fc2).map_err(DiffusionError::model)
}

/// Deepstack merger k (use_postshuffle_norm=true): group 4 rows FIRST, then
/// LayerNorm(4608) -> fc1 -> exact-erf GELU -> fc2 5120.
fn vision_deepstack_merger(
    weights: &H3ShardedWeights,
    prepared: &H3VisionPrepared,
    k: usize,
    hidden: &GpuTensor,
) -> Result<Vec<f32>> {
    let merge_hidden = H3_VIS_HIDDEN * 4;
    let prefix = format!("deepstack_merger_list.{k}");
    let grouped = vis_regroup_rows_4x(hidden)?;
    let normed = gpu_layer_norm_mul_add(
        &grouped,
        &prepared.ds_norm_w[k],
        &prepared.ds_norm_b[k],
        H3_VIS_LN_EPS,
    )
    .map_err(DiffusionError::model)?;
    drop(grouped);
    let fc1 = linear_cached_bias(
        weights,
        &normed,
        &vis_name(&format!("{prefix}.linear_fc1.weight")),
        merge_hidden,
        &prepared.ds_fc1_bias[k],
    )?;
    drop(normed);
    let act = gpu_gelu_erf(&fc1).map_err(DiffusionError::model)?;
    drop(fc1);
    let fc2 = linear_cached_bias(
        weights,
        &act,
        &vis_name(&format!("{prefix}.linear_fc2.weight")),
        H3_VIS_OUT_HIDDEN,
        &prepared.ds_fc2_bias[k],
    )?;
    gpu_download(&fc2).map_err(DiffusionError::model)
}

/// Run the vision tower on preprocessed pixel_values rows (gh*gw, 1536).
/// Returns the main merger image embeds and the three deepstack embeds
/// (each gh*gw/4 rows x 5120, host f32).
pub fn h3_vision_encode(
    weights: &H3ShardedWeights,
    prepared: &H3VisionPrepared,
    pixel_values: &[f32],
    gh: usize,
    gw: usize,
) -> Result<H3VisionOutput> {
    let (output, _taps) =
        h3_vision_encode_with_taps(weights, prepared, pixel_values, gh, gw, &[])?;
    Ok(output)
}

/// Like `h3_vision_encode` but also downloads validation taps: the
/// patch-embed output, the block-0 input, and the hidden state after each
/// block index listed in `tap_blocks`.
pub fn h3_vision_encode_with_taps(
    weights: &H3ShardedWeights,
    prepared: &H3VisionPrepared,
    pixel_values: &[f32],
    gh: usize,
    gw: usize,
    tap_blocks: &[usize],
) -> Result<(H3VisionOutput, H3VisionTaps)> {
    if gh == 0 || gw == 0 || gh % 2 != 0 || gw % 2 != 0 {
        return Err(DiffusionError::workflow(format!(
            "h3 vision encode: grid {gh}x{gw} must be positive and even"
        )));
    }
    let seq = gh * gw;
    if pixel_values.len() != seq * H3_VIS_PATCH_DIM {
        return Err(DiffusionError::workflow(format!(
            "h3 vision encode: {} pixel values != {seq} rows x {H3_VIS_PATCH_DIM}",
            pixel_values.len()
        )));
    }
    let mut taps = H3VisionTaps::default();

    // Patch embed: the Conv3d(3,1152,k=(2,16,16)) is exactly a 1536->1152
    // linear on the patchified rows (the conv weight's [C][T][ky][kx]
    // flattening matches the row layout).
    let x = gpu_upload(pixel_values, seq, H3_VIS_PATCH_DIM).map_err(DiffusionError::model)?;
    let patch = linear_cached_bias(
        weights,
        &x,
        &vis_name("patch_embed.proj.weight"),
        H3_VIS_HIDDEN,
        &prepared.patch_bias,
    )?;
    drop(x);
    if !tap_blocks.is_empty() {
        taps.patch_embed_out = gpu_download(&patch).map_err(DiffusionError::model)?;
    }

    // Learned pos embed, bilinearly resampled align-corners from the 48x48
    // table to (gh, gw), gathered host-side in patch (block-major) order.
    let mut pos = vec![0.0f32; seq * H3_VIS_HIDDEN];
    for i in 0..seq {
        let (row, col) = vis_block_major_row_col(i, gw);
        let (h_taps, h_weights) = vis_interp_axis_taps(row, gh, H3_VIS_POS_SIDE);
        let (w_taps, w_weights) = vis_interp_axis_taps(col, gw, H3_VIS_POS_SIDE);
        let out = &mut pos[i * H3_VIS_HIDDEN..(i + 1) * H3_VIS_HIDDEN];
        for a in 0..2 {
            for b in 0..2 {
                let weight = h_weights[a] * w_weights[b];
                let table_row = h_taps[a] * H3_VIS_POS_SIDE + w_taps[b];
                let table =
                    &prepared.pos_embed[table_row * H3_VIS_HIDDEN..(table_row + 1) * H3_VIS_HIDDEN];
                for (o, t) in out.iter_mut().zip(table.iter()) {
                    *o += weight * t;
                }
            }
        }
    }
    let pos_dev = gpu_upload(&pos, seq, H3_VIS_HIDDEN).map_err(DiffusionError::model)?;
    drop(pos);
    let mut hidden = gpu_add(&patch, &pos_dev).map_err(DiffusionError::model)?;
    drop((patch, pos_dev));
    if !tap_blocks.is_empty() {
        taps.block0_in = gpu_download(&hidden).map_err(DiffusionError::model)?;
    }

    // Rope tables (shared-half layout: both rotated halves read the same
    // 36-wide angle row).
    let rot_half = H3_VIS_HEAD_DIM / 2;
    let angles = h3_vision_rope_angles(gh, gw);
    let mut cos = vec![0.0f32; seq * rot_half];
    let mut sin = vec![0.0f32; seq * rot_half];
    for (i, angle) in angles.iter().enumerate() {
        cos[i] = angle.cos();
        sin[i] = angle.sin();
    }
    let rope_cos = gpu_upload(&cos, seq, rot_half).map_err(DiffusionError::model)?;
    let rope_sin = gpu_upload(&sin, seq, rot_half).map_err(DiffusionError::model)?;

    let mut deepstack: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for block in 0..H3_VIS_DEPTH {
        hidden = vision_block_forward(
            weights, prepared, block, hidden, &rope_cos, &rope_sin, seq,
        )?;
        if tap_blocks.contains(&block) {
            taps.block_outs
                .push((block, gpu_download(&hidden).map_err(DiffusionError::model)?));
        }
        if let Some(k) = H3_VIS_DEEPSTACK_BLOCKS.iter().position(|&d| d == block) {
            deepstack[k] = vision_deepstack_merger(weights, prepared, k, &hidden)?;
        }
    }
    let image_embeds = vision_main_merger(weights, prepared, &hidden)?;
    Ok((
        H3VisionOutput {
            image_embeds,
            deepstack,
        },
        taps,
    ))
}

// ---------------------------------------------------------------------------
// fl2va text encode: vision-conditioned prompt encoding.
// ---------------------------------------------------------------------------

/// Interleaved-mrope axis per rope channel (mrope_section [24,20,20] over 64
/// channels): 0 = T, 1 = H, 2 = W. Channels c < 60 with c%3==1 use H,
/// c%3==2 use W; everything else (including c >= 60) uses T.
fn mrope_axis(channel: usize) -> usize {
    if channel < 60 {
        match channel % 3 {
            1 => 1,
            2 => 2,
            _ => 0,
        }
    } else {
        0
    }
}

/// mrope position planes (t, h, w) for the single-image fl2va sequence, per
/// `Qwen3VLModel.get_rope_index`: text before the image counts positions
/// linearly; the image group holds t constant and spreads h/w over the
/// (gh/2, gw/2) block grid; after the image the running position advances by
/// max(gh, gw)/2 (NOT by the token count) and text continues linearly.
pub fn h3_fl2va_mrope_positions(
    seq: usize,
    vision_start_row: usize,
    vision_len: usize,
    gh: usize,
    gw: usize,
) -> Result<[Vec<i64>; 3]> {
    let blocks_h = gh / 2;
    let blocks_w = gw / 2;
    if vision_len != blocks_h * blocks_w {
        return Err(DiffusionError::workflow(format!(
            "h3 fl2va positions: vision_len {vision_len} != {blocks_h}x{blocks_w}"
        )));
    }
    if vision_start_row + vision_len > seq {
        return Err(DiffusionError::workflow(format!(
            "h3 fl2va positions: vision span {vision_start_row}+{vision_len} exceeds seq {seq}"
        )));
    }
    let mut t = Vec::with_capacity(seq);
    let mut h = Vec::with_capacity(seq);
    let mut w = Vec::with_capacity(seq);
    for i in 0..vision_start_row {
        t.push(i as i64);
        h.push(i as i64);
        w.push(i as i64);
    }
    let image_pos = vision_start_row as i64;
    for j in 0..vision_len {
        t.push(image_pos);
        h.push(image_pos + (j / blocks_w) as i64);
        w.push(image_pos + (j % blocks_w) as i64);
    }
    let after = image_pos + (gh.max(gw) / 2) as i64;
    for i in 0..seq - vision_start_row - vision_len {
        t.push(after + i as i64);
        h.push(after + i as i64);
        w.push(after + i as i64);
    }
    Ok([t, h, w])
}

/// Interleaved-mrope rotate-half tables: angle[i][c] =
/// positions[axis(c)][i] * inv_freq[c] over the 64 rope channels. With all
/// three planes equal this reduces exactly to the plain t2va table.
fn mrope_tables(positions: &[Vec<i64>; 3], inv_freq: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let half = inv_freq.len();
    let seq = positions[0].len();
    let mut cos = vec![0.0f32; seq * half];
    let mut sin = vec![0.0f32; seq * half];
    for i in 0..seq {
        for (c, inv) in inv_freq.iter().enumerate() {
            let angle = positions[mrope_axis(c)][i] as f32 * inv;
            cos[i * half + c] = angle.cos();
            sin[i * half + c] = angle.sin();
        }
    }
    (cos, sin)
}

/// hidden[start..start+len] += update (row span add via slice/concat — the
/// image rows are contiguous).
fn add_rows_span(
    hidden: &GpuTensor,
    start: usize,
    len: usize,
    update: &GpuTensor,
) -> Result<GpuTensor> {
    let rows = hidden.rows();
    let span = gpu_slice_rows(hidden, start, len).map_err(DiffusionError::model)?;
    let span = gpu_add(&span, update).map_err(DiffusionError::model)?;
    let mut out = if start > 0 {
        let pre = gpu_slice_rows(hidden, 0, start).map_err(DiffusionError::model)?;
        gpu_concat_rows(&pre, &span).map_err(DiffusionError::model)?
    } else {
        span
    };
    if start + len < rows {
        let post = gpu_slice_rows(hidden, start + len, rows - start - len)
            .map_err(DiffusionError::model)?;
        out = gpu_concat_rows(&out, &post).map_err(DiffusionError::model)?;
    }
    Ok(out)
}

/// A validation tap out of the fl2va text encode. `hidden_index` follows the
/// HF hidden_states convention: 0 = the input embeddings (after the image
/// scatter), k = after decoder layer k-1. `post` includes the deepstack add
/// of that layer (the value the next layer consumes); `pre_deepstack` is the
/// raw layer output, captured only where the two differ (layers 0/1/2).
pub struct H3Fl2vaTap {
    pub hidden_index: usize,
    pub post: Vec<f32>,
    pub pre_deepstack: Option<Vec<f32>>,
}

/// Vision-conditioned prompt encode (fl2va): like `h3_text_encode` but the
/// `<|image_pad|>` embedding rows [vision_start_row..+vision_len) are
/// replaced by the vision tower's image embeds, rope runs on interleaved
/// mrope positions, and the three deepstack embeds are added to the image
/// rows after decoder layers 0/1/2. Attention stays causal over the whole
/// sequence. Output: (seq, 5120) f32 host values after decoder layer 50.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
pub fn h3_text_encode_fl2va(
    weights: &H3ShardedWeights,
    prepared: &H3TextEncoderPrepared,
    token_ids: &[u32],
    vision_start_row: usize,
    vision_len: usize,
    pixel_values: &[f32],
    gh: usize,
    gw: usize,
    on_layer: Option<&mut dyn FnMut(usize, usize)>,
) -> Result<Vec<f32>> {
    let (hidden, _taps) = h3_text_encode_fl2va_taps(
        weights,
        prepared,
        token_ids,
        vision_start_row,
        vision_len,
        pixel_values,
        gh,
        gw,
        &[],
        on_layer,
    )?;
    Ok(hidden)
}

/// `h3_text_encode_fl2va` with validation taps (see `H3Fl2vaTap`).
/// `on_layer(done, total)` ticks per text decoder layer (the vision tower
/// runs before layer 1/50 fires).
#[allow(clippy::too_many_arguments)]
pub fn h3_text_encode_fl2va_taps(
    weights: &H3ShardedWeights,
    prepared: &H3TextEncoderPrepared,
    token_ids: &[u32],
    vision_start_row: usize,
    vision_len: usize,
    pixel_values: &[f32],
    gh: usize,
    gw: usize,
    taps: &[usize],
    mut on_layer: Option<&mut dyn FnMut(usize, usize)>,
) -> Result<(Vec<f32>, Vec<H3Fl2vaTap>)> {
    let n = token_ids.len();
    if n == 0 {
        return Err(DiffusionError::workflow("h3 fl2va encode: empty prompt"));
    }
    // Positions are validated first (they also check the span geometry).
    let positions = h3_fl2va_mrope_positions(n, vision_start_row, vision_len, gh, gw)?;

    // Vision tower first: its embeds replace the image_pad embedding rows.
    let vis_prepared = H3VisionPrepared::prepare(weights)?;
    let vision = h3_vision_encode(weights, &vis_prepared, pixel_values, gh, gw)?;
    drop(vis_prepared);

    let mut embeds = vec![0.0f32; n * H3_TE_HIDDEN];
    for (row, id) in token_ids.iter().enumerate() {
        if row >= vision_start_row && row < vision_start_row + vision_len {
            let src = row - vision_start_row;
            embeds[row * H3_TE_HIDDEN..(row + 1) * H3_TE_HIDDEN].copy_from_slice(
                &vision.image_embeds[src * H3_TE_HIDDEN..(src + 1) * H3_TE_HIDDEN],
            );
            continue;
        }
        let values = weights
            .tensor_row_f32("model.language_model.embed_tokens.weight", *id as u64)?;
        if values.len() != H3_TE_HIDDEN {
            return Err(DiffusionError::model(format!(
                "h3 te embed row {} width {}",
                id,
                values.len()
            )));
        }
        embeds[row * H3_TE_HIDDEN..(row + 1) * H3_TE_HIDDEN].copy_from_slice(&values);
    }
    let mut tap_out = Vec::new();
    if taps.contains(&0) {
        tap_out.push(H3Fl2vaTap {
            hidden_index: 0,
            post: embeds.clone(),
            pre_deepstack: None,
        });
    }
    let mut hidden = gpu_upload(&embeds, n, H3_TE_HIDDEN).map_err(DiffusionError::model)?;
    drop(embeds);

    // Interleaved mrope tables (same rotate-half kernel as t2va).
    let half = H3_TE_HEAD_DIM / 2;
    let (cos, sin) = mrope_tables(&positions, &prepared.rope_inv_freq);
    let rope_cos = gpu_upload(&cos, n, half).map_err(DiffusionError::model)?;
    let rope_sin = gpu_upload(&sin, n, half).map_err(DiffusionError::model)?;

    // Deepstack embeds stay device-resident across the early layers.
    let mut ds_dev = Vec::with_capacity(vision.deepstack.len());
    for values in &vision.deepstack {
        ds_dev.push(
            gpu_upload(values, vision_len, H3_TE_HIDDEN).map_err(DiffusionError::model)?,
        );
    }

    for layer in 0..H3_TE_LAYERS {
        if let Some(on_layer) = on_layer.as_deref_mut() {
            on_layer(layer + 1, H3_TE_LAYERS);
        }
        hidden = text_layer_forward(weights, prepared, layer, hidden, &rope_cos, &rope_sin, n)?;
        let hidden_index = layer + 1;
        let want_tap = taps.contains(&hidden_index);
        let mut pre_deepstack = None;
        if layer < ds_dev.len() {
            if want_tap {
                pre_deepstack = Some(gpu_download(&hidden).map_err(DiffusionError::model)?);
            }
            hidden = add_rows_span(&hidden, vision_start_row, vision_len, &ds_dev[layer])?;
        }
        if want_tap {
            tap_out.push(H3Fl2vaTap {
                hidden_index,
                post: gpu_download(&hidden).map_err(DiffusionError::model)?,
                pre_deepstack,
            });
        }
    }

    let out = gpu_download(&hidden).map_err(DiffusionError::model)?;
    Ok((out, tap_out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interp_taps_match_reference_formula() {
        // Hand-computed against `_interpolation_axis_taps_weights` (bilinear,
        // align_corners=True): src = idx * (side-1) / max(size-1, 1),
        // taps = clamp(floor(src) + [0,1]), weights = (1 - |src-tap_off|)+.
        // side=4, size=5 -> src = idx * 0.75.
        let cases = [
            (0usize, 5usize, 4usize, [0usize, 1], [1.0f32, 0.0]),
            (1, 5, 4, [0, 1], [0.25, 0.75]),
            (2, 5, 4, [1, 2], [0.5, 0.5]),
            (3, 5, 4, [2, 3], [0.75, 0.25]),
            (4, 5, 4, [3, 3], [1.0, 0.0]), // upper tap clamps to side-1
        ];
        for (idx, size, side, want_taps, want_weights) in cases {
            let (taps, weights) = vis_interp_axis_taps(idx, size, side);
            assert_eq!(taps, want_taps, "taps idx={idx}");
            for (got, want) in weights.iter().zip(want_weights.iter()) {
                assert!((got - want).abs() < 1e-6, "weights idx={idx}: {weights:?}");
            }
        }
        // size == 1 divides by the clamped 1 (src stays 0).
        let (taps, weights) = vis_interp_axis_taps(0, 1, 48);
        assert_eq!(taps, [0, 1]);
        assert!((weights[0] - 1.0).abs() < 1e-6 && weights[1].abs() < 1e-6);
        // size == side: identity mapping (src == idx exactly).
        for idx in 0..48 {
            let (taps, weights) = vis_interp_axis_taps(idx, 48, 48);
            assert_eq!(taps[0], idx);
            assert!((weights[0] - 1.0).abs() < 1e-6 && weights[1].abs() < 1e-6);
        }
    }

    #[test]
    fn mrope_axis_matches_interleave_rule() {
        // Reference apply_interleaved_mrope: start from T everywhere, then
        // freqs[..., slice(offset, section*3, 3)] takes H at 1,4,..,58 and
        // W at 2,5,..,59 (mrope_section [24,20,20] -> length 60 for both).
        let mut expected = [0usize; 64];
        for (dim, offset) in [(1usize, 1usize), (2, 2)] {
            let section = [24usize, 20, 20][dim];
            let mut c = offset;
            while c < section * 3 {
                expected[c] = dim;
                c += 3;
            }
        }
        for (c, want) in expected.iter().enumerate() {
            assert_eq!(mrope_axis(c), *want, "channel {c}");
        }
    }

    #[test]
    fn vision_position_order_matches_torch_block_major() {
        // Emulate get_vision_position_ids for a 4x8 grid, merge 2:
        // hpos/wpos = meshgrid(arange(4), arange(8), ij), then
        // reshape(2, 2, 4, 2).transpose(1, 2).flatten(): element
        // (a, c, b, d) of the transposed view reads raster (a*2+b, c*2+d).
        let (gh, gw) = (4usize, 8usize);
        let mut expected = Vec::new();
        for a in 0..gh / 2 {
            for c in 0..gw / 2 {
                for b in 0..2 {
                    for d in 0..2 {
                        expected.push((a * 2 + b, c * 2 + d));
                    }
                }
            }
        }
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(vis_block_major_row_col(i, gw), *want, "patch {i}");
        }
    }

    #[test]
    fn preprocess_row_order_matches_reference_patchify() {
        // 64x32 canvas -> gh=2, gw=4: independent index calculation per the
        // reference permute [gh/2, gw/2, merge_y, merge_x, C, T, py, px].
        let (width, height) = (64usize, 32usize);
        let mut canvas = vec![0u8; width * height * 3];
        for y in 0..height {
            for x in 0..width {
                for c in 0..3 {
                    canvas[(y * width + x) * 3 + c] = ((y * 7 + x * 3 + c * 11) % 256) as u8;
                }
            }
        }
        let (rows, gh, gw) = h3_vision_preprocess(&canvas, width, height).unwrap();
        assert_eq!((gh, gw), (2, 4));
        assert_eq!(rows.len(), gh * gw * H3_VIS_PATCH_DIM);
        let norm = |v: u8| (v as f32 * (1.0 / 255.0) - 0.5) / 0.5;
        let blocks_w = gw / 2;
        for row in 0..gh * gw {
            // Independent decode of the row -> patch grid position.
            let in_col = row % 2;
            let in_row = (row / 2) % 2;
            let block_col = (row / 4) % blocks_w;
            let block_row = row / (4 * blocks_w);
            let gy = block_row * 2 + in_row;
            let gx = block_col * 2 + in_col;
            for c in 0..3 {
                for t in 0..2 {
                    for py in 0..16 {
                        for px in 0..16 {
                            let elem = ((c * 2 + t) * 16 + py) * 16 + px;
                            let y = gy * 16 + py;
                            let x = gx * 16 + px;
                            let want = norm(canvas[(y * width + x) * 3 + c]);
                            let got = rows[row * H3_VIS_PATCH_DIM + elem];
                            assert_eq!(got, want, "row {row} c {c} t {t} py {py} px {px}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fl2va_positions_small_case() {
        // 3 text tokens, a 4x4 grid image (2x2 blocks = 4 tokens), 2 text
        // tokens after. After the image the position advances by
        // max(4,4)/2 = 2, not by the 4 image tokens.
        let [t, h, w] = h3_fl2va_mrope_positions(9, 3, 4, 4, 4).unwrap();
        assert_eq!(t, vec![0, 1, 2, 3, 3, 3, 3, 5, 6]);
        assert_eq!(h, vec![0, 1, 2, 3, 3, 4, 4, 5, 6]);
        assert_eq!(w, vec![0, 1, 2, 3, 4, 3, 4, 5, 6]);
    }

    #[test]
    fn fl2va_positions_rectangular_grid() {
        // gh=4, gw=8 -> 2x4 blocks: h repeats per block row, w tiles per
        // block col; advance = max(4,8)/2 = 4.
        let [t, h, w] = h3_fl2va_mrope_positions(10, 1, 8, 4, 8).unwrap();
        assert_eq!(t, vec![0, 1, 1, 1, 1, 1, 1, 1, 1, 5]);
        assert_eq!(h, vec![0, 1, 1, 1, 1, 2, 2, 2, 2, 5]);
        assert_eq!(w, vec![0, 1, 2, 3, 4, 1, 2, 3, 4, 5]);
    }

    /// |a - b| in units of b's last place; equality-class check for values
    /// that may come from separate sinf/cosf vs a fused sincosf.
    fn ulp_distance(a: f32, b: f32) -> u32 {
        let to_ordered = |x: f32| {
            let bits = x.to_bits() as i32;
            if bits < 0 { i32::MIN - bits } else { bits }
        };
        to_ordered(a).abs_diff(to_ordered(b))
    }

    #[test]
    fn mrope_equal_positions_reduce_to_t2va_table() {
        // With t == h == w the axis interleave must be irrelevant: the table
        // reduces to the plain 1D rotate-half table h3_text_encode builds.
        // The reduction is exact in the angle argument; the final sin/cos may
        // differ from separately-evaluated sinf/cosf by an ulp when the
        // optimizer pairs them into the platform's sincosf.
        let half = H3_TE_HEAD_DIM / 2;
        let mut inv_freq = Vec::with_capacity(half);
        for j in 0..half {
            inv_freq.push(1.0 / H3_TE_ROPE_THETA.powf(2.0 * j as f32 / H3_TE_HEAD_DIM as f32));
        }
        let n = 17usize;
        let plane: Vec<i64> = (0..n as i64).collect();
        let positions = [plane.clone(), plane.clone(), plane];
        let (cos, sin) = mrope_tables(&positions, &inv_freq);
        for pos in 0..n {
            for j in 0..half {
                let angle = pos as f32 * inv_freq[j];
                let cos_got = cos[pos * half + j];
                let sin_got = sin[pos * half + j];
                assert!(
                    ulp_distance(cos_got, angle.cos()) <= 2,
                    "cos {pos} {j}: {cos_got} vs {}",
                    angle.cos()
                );
                assert!(
                    ulp_distance(sin_got, angle.sin()) <= 2,
                    "sin {pos} {j}: {sin_got} vs {}",
                    angle.sin()
                );
            }
        }
    }

    #[test]
    fn vision_rope_angles_follow_block_major_hw() {
        let (gh, gw) = (4usize, 6usize);
        let angles = h3_vision_rope_angles(gh, gw);
        let half = H3_VIS_HEAD_DIM / 2;
        assert_eq!(angles.len(), gh * gw * half);
        let mut inv_freq = [0.0f32; 18];
        for (j, inv) in inv_freq.iter_mut().enumerate() {
            *inv = 1.0 / H3_VIS_ROPE_THETA.powf(2.0 * j as f32 / half as f32);
        }
        for i in 0..gh * gw {
            let (row, col) = vis_block_major_row_col(i, gw);
            for j in 0..18 {
                assert_eq!(angles[i * half + j], row as f32 * inv_freq[j]);
                assert_eq!(angles[i * half + 18 + j], col as f32 * inv_freq[j]);
            }
        }
    }
}
