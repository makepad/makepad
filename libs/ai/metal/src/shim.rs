/// Metal backend debug breadcrumbs (init, dispatch choices). Off by default —
/// set GGML_METAL_TRACE=1 to enable, mirroring MAKEPAD_CUDA_TRACE.
fn log_metal_trace() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("GGML_METAL_TRACE").is_some()
            || std::env::var_os("MAKEPAD_MUSIC3_TRACE").is_some()
    })
}

/// Missing-kernel fallbacks fire per matmul; print each distinct message once.
fn log_metal_error_once(msg: impl std::fmt::Display) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let text = msg.to_string();
    let mut seen = SEEN
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if seen.insert(text.clone()) {
        eprintln!("{text}");
    }
}

#[allow(dead_code)]
fn log_mul_mat_requested() -> bool {
    log_metal_trace()
}

pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

use crate::gpu_types::GpuTensor;
use makepad_ai_cuda::prof;

fn prof_rec(cat: usize, start: std::time::Instant, f32_count: usize) {
    prof::record(cat, start, (f32_count * 4) as u64);
}

pub fn try_matmul_nn_f32(a: &[f32], b: &[f32], m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_matmul_nn_f32(a, b, m, k, n);
    if let Some(v) = &out {
        prof_rec(prof::CAT_MATMUL_UNCACHED, t, a.len() + b.len() + v.len());
    }
    out
}

pub fn try_matmul_nt_f32(a: &[f32], bt: &[f32], m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_matmul_nt_f32(a, bt, m, k, n);
    if let Some(v) = &out {
        prof_rec(prof::CAT_MATMUL_UNCACHED, t, a.len() + bt.len() + v.len());
    }
    out
}

pub fn try_matmul_nt_f32_bytes(
    a: &[f32],
    bt_bytes: &[u8],
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    imp::try_matmul_nt_f32_bytes(a, bt_bytes, m, k, n)
}

pub fn try_matmul_nt_f16_bytes(
    a: &[f32],
    bt_f16_bytes: &[u8],
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    imp::try_matmul_nt_f16_bytes(a, bt_f16_bytes, m, k, n)
}

pub fn try_matmul_nt_ggml_bytes(
    a: &[f32],
    bt_bytes: &[u8],
    bt_ggml_type: u32,
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_matmul_nt_ggml_bytes(a, bt_bytes, bt_ggml_type, m, k, n);
    if let Some(v) = &out {
        prof_rec(
            prof::CAT_MATMUL_UNCACHED,
            t,
            a.len() + bt_bytes.len() / 4 + v.len(),
        );
    }
    out
}

pub fn try_matmul_nt_ggml_bytes_keyed<F>(
    a: &[f32],
    bt_ggml_type: u32,
    m: usize,
    k: usize,
    n: usize,
    namespace: &str,
    cache_key: &str,
    load: F,
) -> Option<Vec<f32>>
where
    F: FnOnce() -> Result<Vec<u8>, String>,
{
    let t = std::time::Instant::now();
    let out = imp::try_matmul_nt_ggml_bytes_keyed(
        a, bt_ggml_type, m, k, n, namespace, cache_key, load,
    );
    if let Some(v) = &out {
        prof_rec(prof::CAT_MATMUL_UNCACHED, t, a.len() + v.len());
    }
    out
}

#[derive(Clone, Copy, Debug)]
pub struct MatmulNtGgmlBytesMatrix<'a> {
    pub bt_bytes: &'a [u8],
    pub bt_ggml_type: u32,
    pub n: usize,
}

pub fn try_matmul_nt_ggml_bytes_multi(
    a: &[f32],
    m: usize,
    k: usize,
    matrices: &[MatmulNtGgmlBytesMatrix<'_>],
) -> Option<Vec<Vec<f32>>> {
    imp::try_matmul_nt_ggml_bytes_multi(a, m, k, matrices)
}

/// Same kernels as [`try_matmul_nt_ggml_bytes_keyed`], one activation upload,
/// one wait. Weights live in the named cache — do not use the uncached multi
/// path for AR (it re-uploads W whenever the host pointer moves).
#[derive(Clone, Copy, Debug)]
pub struct MatmulNtGgmlBytesKeyedMatrix<'a> {
    pub bt_ggml_type: u32,
    pub n: usize,
    pub namespace: &'a str,
    pub cache_key: &'a str,
}

pub fn try_matmul_nt_ggml_bytes_keyed_multi<F>(
    a: &[f32],
    m: usize,
    k: usize,
    matrices: &[MatmulNtGgmlBytesKeyedMatrix<'_>],
    load: F,
) -> Option<Vec<Vec<f32>>>
where
    F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
{
    let t = std::time::Instant::now();
    let out = imp::try_matmul_nt_ggml_bytes_keyed_multi(a, m, k, matrices, load);
    if let Some(vs) = &out {
        let n_out: usize = vs.iter().map(|v| v.len()).sum();
        prof_rec(prof::CAT_MATMUL_UNCACHED, t, a.len() + n_out);
    }
    out
}

/// Upload (optional) hidden, RMS + QKV (+ optional Q/K RMS) on GPU.
/// Returns host q,k,v. `qk_norm` is `(q_norm, k_norm, q_key, k_key)`;
/// pass `None` for stacks without per-head Q/K norms (Music3 RVQ).
pub fn try_ar_pre_attn<F>(
    hidden: Option<&[f32]>,
    m: usize,
    hidden_w: usize,
    head_dim: usize,
    in_norm: &[f32],
    qk_norm: Option<(&[f32], &[f32], &str, &str)>,
    in_norm_key: &str,
    eps: f32,
    q: MatmulNtGgmlBytesKeyedMatrix<'_>,
    k: MatmulNtGgmlBytesKeyedMatrix<'_>,
    v: MatmulNtGgmlBytesKeyedMatrix<'_>,
    load: F,
) -> Option<(Vec<f32>, Vec<f32>, Vec<f32>)>
where
    F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
{
    imp::try_ar_pre_attn(
        hidden,
        m,
        hidden_w,
        head_dim,
        in_norm,
        qk_norm,
        in_norm_key,
        eps,
        q,
        k,
        v,
        load,
    )
}

/// o_proj + residual + RMS + up/gate + SiLU*mul + down + residual. Hidden stays on GPU.
pub fn try_ar_post_attn<F>(
    attn: &[f32],
    m: usize,
    hidden_w: usize,
    post_norm: &[f32],
    post_norm_key: &str,
    eps: f32,
    o: MatmulNtGgmlBytesKeyedMatrix<'_>,
    up: MatmulNtGgmlBytesKeyedMatrix<'_>,
    gate: MatmulNtGgmlBytesKeyedMatrix<'_>,
    down: MatmulNtGgmlBytesKeyedMatrix<'_>,
    load: F,
) -> Option<()>
where
    F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
{
    imp::try_ar_post_attn(
        attn,
        m,
        hidden_w,
        post_norm,
        post_norm_key,
        eps,
        o,
        up,
        gate,
        down,
        load,
    )
}

pub fn try_ar_final_rms(
    m: usize,
    hidden_w: usize,
    gamma: &[f32],
    gamma_key: &str,
    eps: f32,
) -> Option<Vec<f32>> {
    imp::try_ar_final_rms(m, hidden_w, gamma, gamma_key, eps)
}

pub fn ar_resident_clear() {
    imp::ar_resident_clear();
}

/// Drop all recycled transient buffers (keeps act/weight caches). Call at a
/// phase edge when the previous phase used much larger shapes (e.g. after
/// prefill, before decode) so the pool's retained giants don't sit wired
/// through a long small-shape loop.
pub fn transient_pool_clear() {
    imp::transient_pool_clear();
}

pub fn try_dit_ffn_resident<F>(
    normed: &[f32],
    m: usize,
    hidden_w: usize,
    ff_dim: usize,
    ff_in_b: &[f32],
    ff_out_b: &[f32],
    ff_in_b_key: &str,
    ff_out_b_key: &str,
    swap: bool,
    ff_in: MatmulNtGgmlBytesKeyedMatrix<'_>,
    ff_out: MatmulNtGgmlBytesKeyedMatrix<'_>,
    load: F,
) -> Option<Vec<f32>>
where
    F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
{
    imp::try_dit_ffn_resident(
        normed,
        m,
        hidden_w,
        ff_dim,
        ff_in_b,
        ff_out_b,
        ff_in_b_key,
        ff_out_b_key,
        swap,
        ff_in,
        ff_out,
        load,
    )
}

pub fn try_matmul_nt_ggml_bytes_add_bias(
    a: &[f32],
    bt_bytes: &[u8],
    bt_ggml_type: u32,
    m: usize,
    k: usize,
    n: usize,
    bias: &[f32],
) -> Option<Vec<f32>> {
    imp::try_matmul_nt_ggml_bytes_add_bias(a, bt_bytes, bt_ggml_type, m, k, n, bias)
}

pub fn try_vision_mlp_bf16_fused(
    x: &[f32],
    gate_up_weight_bytes: &[u8],
    down_weight_bytes: &[u8],
    rows: usize,
    hidden_size: usize,
    intermediate_size: usize,
) -> Option<Vec<f32>> {
    imp::try_vision_mlp_bf16_fused(
        x,
        gate_up_weight_bytes,
        down_weight_bytes,
        rows,
        hidden_size,
        intermediate_size,
    )
}

pub fn try_flash_attn_f32_packed(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    n_q: usize,
    n_kv: usize,
    n_head: usize,
    d: usize,
    scale: f32,
) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_flash_attn_f32_packed(q, k, v, n_q, n_kv, n_head, d, scale);
    if let Some(o) = &out {
        prof_rec(
            prof::CAT_FLASH_ATTN,
            t,
            q.len() + k.len() + v.len() + o.len(),
        );
    }
    out
}

pub fn clear_decoder_kv_cache() {
    imp::clear_decoder_kv_cache();
}

#[allow(clippy::too_many_arguments)]
/// One linear of a device-resident two-way decoder layer: an f32 `[n, k]`
/// weight tensor (cached on the device under its content identity) and an
/// optional `[1, n]` bias.
#[derive(Clone, Copy)]
pub struct DecLinearRef<'a> {
    pub weight: &'a GpuTensor,
    pub bias: Option<&'a GpuTensor>,
}

#[derive(Clone, Copy)]
pub struct DecAttnRef<'a> {
    pub q: DecLinearRef<'a>,
    pub k: DecLinearRef<'a>,
    pub v: DecLinearRef<'a>,
    pub out: DecLinearRef<'a>,
}

/// One SAM-style two-way decoder layer over `hidden` `[n_tok, dim]` with
/// image context `[n_ctx, ctx_dim]`: token self-attention (queries/keys
/// carry the token PE when `pe_on_self`), token-to-image cross-attention
/// (query = LN(hidden) + token PE, key = LN(context) + image PE, value =
/// LN(context)), an erf-GELU feed-forward, then `ln_final`. All norms are
/// LayerNorm with affines; `ln_pe_1`/`ln_pe_2` normalise the two PEs.
#[derive(Clone, Copy)]
pub struct TwoWayLayerRef<'a> {
    pub ln_pe_1: (&'a [f32], &'a [f32]),
    pub ln_pe_2: (&'a [f32], &'a [f32]),
    pub ln1: (&'a [f32], &'a [f32]),
    pub ln2_1: (&'a [f32], &'a [f32]),
    pub ln2_2: (&'a [f32], &'a [f32]),
    pub ln3: (&'a [f32], &'a [f32]),
    pub ln_final: (&'a [f32], &'a [f32]),
    pub self_attn: DecAttnRef<'a>,
    pub cross_attn: DecAttnRef<'a>,
    pub ffn_first: DecLinearRef<'a>,
    pub ffn_second: DecLinearRef<'a>,
    pub n_head: usize,
    pub eps: f32,
    pub pe_on_self: bool,
}

/// Runs one two-way decoder layer device-resident (one command buffer,
/// weights cached) and returns `(hidden, ln_final(hidden))`, both
/// `[n_tok, dim]`.
#[allow(clippy::too_many_arguments)]
pub fn try_two_way_layer_resident_f32(
    hidden: &[f32],
    token_pe: &[f32],
    context: &[f32],
    context_pe: &[f32],
    n_tok: usize,
    dim: usize,
    n_ctx: usize,
    ctx_dim: usize,
    layer: &TwoWayLayerRef<'_>,
) -> Option<(Vec<f32>, Vec<f32>)> {
    imp::try_two_way_layer_resident_f32(
        hidden, token_pe, context, context_pe, n_tok, dim, n_ctx, ctx_dim, layer,
    )
}

/// One linear of a device-resident ViT layer: a row-major `[n, k]` weight in
/// a ggml dtype, its output width and its bias (empty for none).
#[derive(Clone, Copy)]
pub struct VitLinearRef<'a> {
    pub w_bytes: &'a [u8],
    pub w_ggml_type: u32,
    pub n: usize,
    pub bias: &'a [f32],
}

/// One pre-norm ViT layer with rotary attention and a SwiGLU feed-forward
/// (the DINOv3 block): `x += out(attn(rope(q(n1)), rope(k(n1)), v(n1)))`,
/// then `x += down(silu(gate(n2)) * up(n2))`, both norms LayerNorm with an
/// affine. Layer scales are expected folded into `out` and `down`.
#[derive(Clone, Copy)]
pub struct VitLayerRef<'a> {
    pub norm1_w: &'a [f32],
    pub norm1_b: &'a [f32],
    pub q: VitLinearRef<'a>,
    pub k: VitLinearRef<'a>,
    pub v: VitLinearRef<'a>,
    pub out: VitLinearRef<'a>,
    pub norm2_w: &'a [f32],
    pub norm2_b: &'a [f32],
    pub gate: VitLinearRef<'a>,
    pub up: VitLinearRef<'a>,
    pub down: VitLinearRef<'a>,
}

/// Runs a whole ViT stack device-resident: `x` (`[seq_len, n_state]`) goes
/// up once, every layer encodes into one command buffer against cached
/// weights, and only the final normalised activations come back.
/// `cos`/`sin` are `[seq_len, rot_half]` rotate-half tables.
#[allow(clippy::too_many_arguments)]
pub fn try_vit_backbone_resident_f32(
    x: &[f32],
    seq_len: usize,
    n_state: usize,
    n_head: usize,
    rot_half: usize,
    cos: &[f32],
    sin: &[f32],
    layers: &[VitLayerRef<'_>],
    final_norm_w: &[f32],
    final_norm_b: &[f32],
    eps: f32,
) -> Option<Vec<f32>> {
    imp::try_vit_backbone_resident_f32(
        x, seq_len, n_state, n_head, rot_half, cos, sin, layers, final_norm_w, final_norm_b, eps,
    )
}

pub fn try_flash_attn_f32_self_kv_cache(
    layer: usize,
    q: &[f32],
    k_all: &[f32],
    v_all: &[f32],
    n_kv: usize,
    n_head: usize,
    d: usize,
    scale: f32,
) -> Option<Vec<f32>> {
    imp::try_flash_attn_f32_self_kv_cache(layer, q, k_all, v_all, n_kv, n_head, d, scale)
}

#[allow(clippy::too_many_arguments)]
pub fn try_flash_attn_f32_cross_kv_cache(
    layer: usize,
    q: &[f32],
    k_cross: &[f32],
    v_cross: &[f32],
    n_q: usize,
    n_kv: usize,
    n_head: usize,
    d: usize,
    scale: f32,
) -> Option<Vec<f32>> {
    imp::try_flash_attn_f32_cross_kv_cache(layer, q, k_cross, v_cross, n_q, n_kv, n_head, d, scale)
}

pub fn try_add_f32(a: &[f32], a_shape: &[usize], b: &[f32], b_shape: &[usize]) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_add_f32(a, a_shape, b, b_shape);
    if let Some(v) = &out {
        prof_rec(prof::CAT_ELEMENTWISE, t, a.len() + b.len() + v.len());
    }
    out
}

pub fn try_mul_f32(a: &[f32], a_shape: &[usize], b: &[f32], b_shape: &[usize]) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_mul_f32(a, a_shape, b, b_shape);
    if let Some(v) = &out {
        prof_rec(prof::CAT_ELEMENTWISE, t, a.len() + b.len() + v.len());
    }
    out
}

pub fn try_gelu_f32(a: &[f32], shape: &[usize]) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_gelu_f32(a, shape);
    if let Some(v) = &out {
        prof_rec(prof::CAT_ELEMENTWISE, t, a.len() + v.len());
    }
    out
}

pub fn try_layer_norm_f32(x: &[f32], shape: &[usize], eps: f32) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_layer_norm_f32(x, shape, eps);
    if let Some(v) = &out {
        prof_rec(prof::CAT_LAYER_NORM, t, x.len() + v.len());
    }
    out
}

pub fn try_rms_norm_f32(x: &[f32], shape: &[usize], eps: f32) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_rms_norm_f32(x, shape, eps);
    if let Some(v) = &out {
        prof_rec(prof::CAT_RMS_NORM, t, x.len() + v.len());
    }
    out
}

pub fn try_rms_norm_mul_f32(
    x: &[f32],
    x_shape: &[usize],
    mul: &[f32],
    mul_shape: &[usize],
    eps: f32,
) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_rms_norm_mul_f32(x, x_shape, mul, mul_shape, eps);
    if let Some(v) = &out {
        prof_rec(prof::CAT_RMS_NORM, t, x.len() + v.len());
    }
    out
}

pub fn try_attention_softmax_weighted_sum_f32(
    logits: &[f32],
    values: &[f32],
    query_count: usize,
    seq_len: usize,
    head_dim: usize,
) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out =
        imp::try_attention_softmax_weighted_sum_f32(logits, values, query_count, seq_len, head_dim);
    if let Some(v) = &out {
        prof_rec(
            prof::CAT_ATTN_SOFTMAX_WS,
            t,
            logits.len() + values.len() + v.len(),
        );
    }
    out
}

pub fn try_layer_norm_mul_add_f32(
    x: &[f32],
    x_shape: &[usize],
    mul: &[f32],
    mul_shape: &[usize],
    add: &[f32],
    add_shape: &[usize],
    eps: f32,
) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_layer_norm_mul_add_f32(x, x_shape, mul, mul_shape, add, add_shape, eps);
    if let Some(v) = &out {
        prof_rec(prof::CAT_LAYER_NORM, t, x.len() + v.len());
    }
    out
}

pub fn try_get_rows_ggml_bytes(
    src: &[u8],
    src_ggml_type: u32,
    n_cols: usize,
    n_rows: usize,
    row_indices: &[i32],
) -> Option<Vec<f32>> {
    imp::try_get_rows_ggml_bytes(src, src_ggml_type, n_cols, n_rows, row_indices)
}

pub fn try_im2col_1d_f32(
    input: &[f32],
    ic: usize,
    iw: usize,
    kw: usize,
    stride: usize,
    pad: usize,
) -> Option<Vec<f32>> {
    imp::try_im2col_1d_f32(input, ic, iw, kw, stride, pad)
}

/// Planar ([c][y][x]) stride-1 "same" conv2d for the diffusion VAE lazy path.
/// `weights` is [out_c][in_c][kh][kw], `bias` is per out channel.
#[allow(clippy::too_many_arguments)]
pub fn try_conv2d_planar_f32(
    input: &[f32],
    width: usize,
    height: usize,
    in_channels: usize,
    weights: &[f32],
    bias: &[f32],
    out_channels: usize,
    kw: usize,
    kh: usize,
    pad_x: usize,
    pad_y: usize,
) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_conv2d_planar_f32(
        input,
        width,
        height,
        in_channels,
        weights,
        bias,
        out_channels,
        kw,
        kh,
        pad_x,
        pad_y,
    );
    if let Some(v) = &out {
        prof_rec(
            prof::CAT_CONV2D,
            t,
            input.len() + weights.len() + v.len(),
        );
    }
    out
}

/// Group norm over planar ([c][y][x]) data with per-channel gamma/beta.
#[allow(clippy::too_many_arguments)]
pub fn try_group_norm_planar_f32(
    input: &[f32],
    width: usize,
    height: usize,
    channels: usize,
    groups: usize,
    gamma: &[f32],
    beta: &[f32],
    eps: f32,
) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out =
        imp::try_group_norm_planar_f32(input, width, height, channels, groups, gamma, beta, eps);
    if let Some(v) = &out {
        prof_rec(prof::CAT_GROUP_NORM, t, input.len() + v.len());
    }
    out
}

pub fn try_silu_f32(a: &[f32]) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_silu_f32(a);
    if let Some(v) = &out {
        prof_rec(prof::CAT_ELEMENTWISE, t, a.len() + v.len());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        try_add_f32, try_attention_softmax_weighted_sum_f32, try_gelu_f32, try_mul_f32,
        try_rms_norm_f32, try_rms_norm_mul_f32,
    };

    // The non-macOS path currently routes through CUDA kernels that do not
    // match the CPU reference bit-for-bit on these tiny synthetic cases.
    const RMS_NORM_TOLERANCE: f32 = 1.0e-2;

    fn assert_close(actual: &[f32], expected: &[f32], tol: f32) {
        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert!(
                (a - e).abs() <= tol,
                "mismatch: actual={} expected={} tol={}",
                a,
                e,
                tol
            );
        }
    }

    #[test]
    fn add_f32_matches_cpu_when_backend_available() {
        let a = vec![1.0, -2.0, 3.5, 0.25, 4.0, -1.5];
        let b = vec![0.5, 3.0, -1.5, 1.75, -2.0, 2.5];
        let expected = a
            .iter()
            .zip(b.iter())
            .map(|(lhs, rhs)| lhs + rhs)
            .collect::<Vec<_>>();
        let Some(actual) = try_add_f32(&a, &[2, 3], &b, &[2, 3]) else {
            return;
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn mul_f32_matches_cpu_when_backend_available() {
        let a = vec![1.0, -2.0, 3.5, 0.25, 4.0, -1.5];
        let b = vec![0.5, 3.0, -1.5, 1.75, -2.0, 2.5];
        let expected = a
            .iter()
            .zip(b.iter())
            .map(|(lhs, rhs)| lhs * rhs)
            .collect::<Vec<_>>();
        let Some(actual) = try_mul_f32(&a, &[2, 3], &b, &[2, 3]) else {
            return;
        };
        assert_close(&actual, &expected, RMS_NORM_TOLERANCE);
    }

    #[test]
    fn gelu_f32_matches_cpu_when_backend_available() {
        let input = vec![-2.0, -0.5, 0.0, 0.5, 1.5, 3.0];
        let expected = input.iter().copied().map(cpu_gelu).collect::<Vec<_>>();
        let Some(actual) = try_gelu_f32(&input, &[2, 3]) else {
            return;
        };
        assert_close(&actual, &expected, RMS_NORM_TOLERANCE);
    }

    #[test]
    fn rms_norm_f32_matches_cpu_when_backend_available() {
        let x = vec![1.0, -2.0, 3.0, 0.5, -1.0, 2.5];
        let eps = 1.0e-5;
        let expected = x
            .chunks_exact(3)
            .flat_map(|row| {
                let mean_square =
                    row.iter().map(|value| value * value).sum::<f32>() / row.len() as f32;
                let inv_rms = 1.0 / (mean_square + eps).sqrt();
                row.iter().map(move |value| value * inv_rms)
            })
            .collect::<Vec<_>>();
        let Some(actual) = try_rms_norm_f32(&x, &[2, 3], eps) else {
            return;
        };
        assert_close(&actual, &expected, RMS_NORM_TOLERANCE);
    }

    #[test]
    fn rms_norm_mul_f32_matches_cpu_when_backend_available() {
        let x = vec![1.0, -2.0, 3.0, 0.5, -1.0, 2.5];
        let mul = vec![0.25, 1.5, -0.75];
        let eps = 1.0e-5;
        let expected = x
            .chunks_exact(3)
            .flat_map(|row| {
                let mean_square =
                    row.iter().map(|value| value * value).sum::<f32>() / row.len() as f32;
                let inv_rms = 1.0 / (mean_square + eps).sqrt();
                row.iter()
                    .zip(mul.iter())
                    .map(move |(value, scale)| value * inv_rms * scale)
            })
            .collect::<Vec<_>>();
        let Some(actual) = try_rms_norm_mul_f32(&x, &[2, 3], &mul, &[3], eps) else {
            return;
        };
        assert_close(&actual, &expected, RMS_NORM_TOLERANCE);
    }

    #[test]
    fn attention_softmax_weighted_sum_matches_cpu_when_backend_available() {
        let logits = vec![
            0.5, -0.25, 1.0, //
            -0.5, 0.25, 0.75,
        ];
        let values = vec![
            1.0, 0.5, //
            -0.25, 2.0, //
            0.75, -1.5,
        ];
        let expected = cpu_attention_softmax_weighted_sum(&logits, &values, 2, 3, 2);
        let Some(actual) = try_attention_softmax_weighted_sum_f32(&logits, &values, 2, 3, 2) else {
            return;
        };
        assert_close(&actual, &expected, RMS_NORM_TOLERANCE);
    }

    fn cpu_attention_softmax_weighted_sum(
        logits: &[f32],
        values: &[f32],
        query_count: usize,
        seq_len: usize,
        head_dim: usize,
    ) -> Vec<f32> {
        let mut output = vec![0.0f32; query_count * head_dim];
        for query_idx in 0..query_count {
            let logits_row = &logits[query_idx * seq_len..(query_idx + 1) * seq_len];
            let max_logit = logits_row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let probs = logits_row
                .iter()
                .copied()
                .map(|value| (value - max_logit).exp())
                .collect::<Vec<_>>();
            let denom = probs.iter().copied().sum::<f32>();
            for token_idx in 0..seq_len {
                let prob = probs[token_idx] / denom;
                let value_row = &values[token_idx * head_dim..(token_idx + 1) * head_dim];
                for dim_idx in 0..head_dim {
                    output[query_idx * head_dim + dim_idx] += prob * value_row[dim_idx];
                }
            }
        }
        output
    }

    fn cpu_gelu(value: f32) -> f32 {
        let squared = value * value;
        let cubic = squared * value;
        let poly = value + 0.044715 * cubic;
        let tanh_input = 0.7978846 * poly;
        0.5 * value * (1.0 + tanh_input.tanh())
    }
}

#[cfg(all(not(target_os = "macos"), makepad_ai_cuda_kernels))]
mod imp {
    use makepad_ai_cuda as cuda;
    use std::cell::RefCell;
    use std::mem::size_of;

    thread_local! {
        static CUDA_RUNTIME: RefCell<Option<cuda::CudaRuntime>> = const { RefCell::new(None) };
    }

    fn with_cuda_runtime<T, F>(f: F) -> Option<T>
    where
        F: FnOnce(&cuda::CudaRuntime) -> Result<T, String>,
    {
        if !cuda::is_available() {
            return None;
        }

        CUDA_RUNTIME.with(|runtime| {
            let mut runtime = runtime.borrow_mut();
            if runtime.is_none() {
                *runtime = Some(cuda::CudaRuntime::load().ok()?);
            }
            f(runtime.as_ref()?).ok()
        })
    }

    fn f32s_as_bytes(values: &[f32]) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                values.as_ptr().cast::<u8>(),
                values.len() * size_of::<f32>(),
            )
        }
    }

    fn f32_to_bf16_word(value: f32) -> u16 {
        let bits = value.to_bits();
        let lsb = (bits >> 16) & 1;
        let rounding_bias = 0x7FFF + lsb;
        ((bits.wrapping_add(rounding_bias)) >> 16) as u16
    }

    fn f32s_to_bf16_words(values: &[f32]) -> Vec<u16> {
        values.iter().copied().map(f32_to_bf16_word).collect()
    }

    fn u16_words_as_le_bytes(words: &[u16]) -> &[u8] {
        #[cfg(target_endian = "little")]
        unsafe {
            std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * size_of::<u16>())
        }

        #[cfg(not(target_endian = "little"))]
        {
            unreachable!("u16 byte reinterpreting currently assumes little-endian targets")
        }
    }

    fn shape_numel(shape: &[usize]) -> Option<usize> {
        shape
            .iter()
            .copied()
            .try_fold(1usize, |acc, dim| acc.checked_mul(dim))
    }

    fn rows_cols_for_last_dim(shape: &[usize], len: usize) -> Option<(usize, usize)> {
        let cols = *shape.last()?;
        let numel = shape_numel(shape)?;
        if numel != len {
            return None;
        }
        if cols == 0 {
            return Some((0, 0));
        }
        Some((numel / cols, cols))
    }

    fn is_last_dim_vector(shape: &[usize], cols: usize, len: usize) -> bool {
        if shape.is_empty() || len != cols || *shape.last().unwrap() != cols {
            return false;
        }
        shape[..shape.len() - 1].iter().all(|&dim| dim == 1)
    }

    pub(super) fn try_matmul_nn_f32(
        a: &[f32],
        b: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        if a.len() != m.checked_mul(k)? || b.len() != k.checked_mul(n)? {
            return None;
        }
        if a.is_empty() || b.is_empty() {
            return Some(Vec::new());
        }

        with_cuda_runtime(|cuda| {
            let a_buf = cuda.load_bytes(f32s_as_bytes(a))?;
            let b_buf = cuda.load_bytes(f32s_as_bytes(b))?;
            let out_len = m
                .checked_mul(n)
                .ok_or_else(|| "CUDA matmul output length overflow".to_string())?;
            let out_buf = cuda.alloc_f32(out_len)?;
            cuda.matmul_nn_f32(&a_buf, &b_buf, &out_buf, m, k, n)?;
            cuda.read_f32s(&out_buf, out_len)
        })
    }

    pub(super) fn try_matmul_nt_f32(
        a: &[f32],
        bt: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        if a.len() != m.checked_mul(k)? || bt.len() != n.checked_mul(k)? {
            return None;
        }
        if a.is_empty() || bt.is_empty() {
            return Some(Vec::new());
        }

        with_cuda_runtime(|cuda| {
            let a_buf = cuda.load_bytes(f32s_as_bytes(a))?;
            let bt_buf = cuda.load_bytes(f32s_as_bytes(bt))?;
            let out_len = m
                .checked_mul(n)
                .ok_or_else(|| "CUDA matmul output length overflow".to_string())?;
            let out_buf = cuda.alloc_f32(out_len)?;
            cuda.matmul_nt_f32(&a_buf, &bt_buf, &out_buf, m, k, n)?;
            cuda.read_f32s(&out_buf, out_len)
        })
    }

    pub(super) fn try_matmul_nt_f32_bytes(
        _a: &[f32],
        _bt_bytes: &[u8],
        _m: usize,
        _k: usize,
        _n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_f16_bytes(
        _a: &[f32],
        _bt_f16_bytes: &[u8],
        _m: usize,
        _k: usize,
        _n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes(
        _a: &[f32],
        _bt_bytes: &[u8],
        _bt_ggml_type: u32,
        _m: usize,
        _k: usize,
        _n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes_keyed<F>(
        _a: &[f32],
        _bt_ggml_type: u32,
        _m: usize,
        _k: usize,
        _n: usize,
        _namespace: &str,
        _cache_key: &str,
        _load: F,
    ) -> Option<Vec<f32>>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes_multi(
        _a: &[f32],
        _m: usize,
        _k: usize,
        _matrices: &[super::MatmulNtGgmlBytesMatrix<'_>],
    ) -> Option<Vec<Vec<f32>>> {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes_keyed_multi<F>(
        _a: &[f32],
        _m: usize,
        _k: usize,
        _matrices: &[super::MatmulNtGgmlBytesKeyedMatrix<'_>],
        _load: F,
    ) -> Option<Vec<Vec<f32>>>
    where
        F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_ar_pre_attn<F>(
        _hidden: Option<&[f32]>,
        _m: usize,
        _hidden_w: usize,
        _head_dim: usize,
        _in_norm: &[f32],
        _qk_norm: Option<(&[f32], &[f32], &str, &str)>,
        _in_norm_key: &str,
        _eps: f32,
        _q: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _k: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _v: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _load: F,
    ) -> Option<(Vec<f32>, Vec<f32>, Vec<f32>)>
    where
        F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_ar_post_attn<F>(
        _attn: &[f32],
        _m: usize,
        _hidden_w: usize,
        _post_norm: &[f32],
        _post_norm_key: &str,
        _eps: f32,
        _o: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _up: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _gate: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _down: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _load: F,
    ) -> Option<()>
    where
        F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_ar_final_rms(
        _m: usize,
        _hidden_w: usize,
        _gamma: &[f32],
        _gamma_key: &str,
        _eps: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn ar_resident_clear() {}

    pub(super) fn transient_pool_clear() {}

    pub(super) fn try_dit_ffn_resident<F>(
        _normed: &[f32],
        _m: usize,
        _hidden_w: usize,
        _ff_dim: usize,
        _ff_in_b: &[f32],
        _ff_out_b: &[f32],
        _ff_in_b_key: &str,
        _ff_out_b_key: &str,
        _swap: bool,
        _ff_in: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _ff_out: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _load: F,
    ) -> Option<Vec<f32>>
    where
        F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes_add_bias(
        _a: &[f32],
        _bt_bytes: &[u8],
        _bt_ggml_type: u32,
        _m: usize,
        _k: usize,
        _n: usize,
        _bias: &[f32],
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_vision_mlp_bf16_fused(
        _x: &[f32],
        _gate_up_weight_bytes: &[u8],
        _down_weight_bytes: &[u8],
        _rows: usize,
        _hidden_size: usize,
        _intermediate_size: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_flash_attn_f32_packed(
        q: &[f32],
        k: &[f32],
        v: &[f32],
        n_q: usize,
        n_kv: usize,
        n_head: usize,
        d: usize,
        scale: f32,
    ) -> Option<Vec<f32>> {
        // The CUDA kernel shares the Metal packed layout: token-major
        // [token][head][dim], full bidirectional attention, caller-provided
        // scale. It requires n_q == n_kv (self attention), which it checks.
        cuda::try_flash_attn_f32_packed(q, k, v, n_q, n_kv, n_head, d, scale)
    }

    pub(super) fn clear_decoder_kv_cache() {}

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_vit_backbone_resident_f32(
        _x: &[f32],
        _seq_len: usize,
        _n_state: usize,
        _n_head: usize,
        _rot_half: usize,
        _cos: &[f32],
        _sin: &[f32],
        _layers: &[super::VitLayerRef<'_>],
        _final_norm_w: &[f32],
        _final_norm_b: &[f32],
        _eps: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_two_way_layer_resident_f32(
        _hidden: &[f32],
        _token_pe: &[f32],
        _context: &[f32],
        _context_pe: &[f32],
        _n_tok: usize,
        _dim: usize,
        _n_ctx: usize,
        _ctx_dim: usize,
        _layer: &super::TwoWayLayerRef<'_>,
    ) -> Option<(Vec<f32>, Vec<f32>)> {
        None
    }

    pub(super) fn try_flash_attn_f32_self_kv_cache(
        _layer: usize,
        _q: &[f32],
        _k_all: &[f32],
        _v_all: &[f32],
        _n_kv: usize,
        _n_head: usize,
        _d: usize,
        _scale: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_flash_attn_f32_cross_kv_cache(
        _layer: usize,
        _q: &[f32],
        _k_cross: &[f32],
        _v_cross: &[f32],
        _n_q: usize,
        _n_kv: usize,
        _n_head: usize,
        _d: usize,
        _scale: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_add_f32(
        a: &[f32],
        a_shape: &[usize],
        b: &[f32],
        b_shape: &[usize],
    ) -> Option<Vec<f32>> {
        if shape_numel(a_shape)? != a.len()
            || shape_numel(b_shape)? != b.len()
            || a_shape != b_shape
        {
            return None;
        }
        if a.is_empty() {
            return Some(Vec::new());
        }

        with_cuda_runtime(|cuda| {
            let a_buf = cuda.load_bytes(f32s_as_bytes(a))?;
            let b_buf = cuda.load_bytes(f32s_as_bytes(b))?;
            let out_buf = cuda.alloc_f32(a.len())?;
            cuda.add_f32_precise(&a_buf, &b_buf, &out_buf, a.len())?;
            cuda.read_f32s(&out_buf, a.len())
        })
    }

    pub(super) fn try_mul_f32(
        a: &[f32],
        a_shape: &[usize],
        b: &[f32],
        b_shape: &[usize],
    ) -> Option<Vec<f32>> {
        if shape_numel(a_shape)? != a.len() || shape_numel(b_shape)? != b.len() {
            return None;
        }
        if a.is_empty() {
            return Some(Vec::new());
        }

        if a_shape == b_shape {
            return with_cuda_runtime(|cuda| {
                let a_buf = cuda.load_bytes(f32s_as_bytes(a))?;
                let b_buf = cuda.load_bytes(f32s_as_bytes(b))?;
                let out_buf = cuda.alloc_f32(a.len())?;
                cuda.mul_f32_precise(&a_buf, &b_buf, &out_buf, a.len())?;
                cuda.read_f32s(&out_buf, a.len())
            });
        }

        // Row-broadcast: a is [rows, cols], b is a last-dim vector [cols]
        // (the Metal path broadcasts modulo per dim; this covers the case
        // the diffusion lazy path uses for gated residuals).
        let (rows, cols) = rows_cols_for_last_dim(a_shape, a.len())?;
        if !is_last_dim_vector(b_shape, cols, b.len()) {
            return None;
        }
        with_cuda_runtime(|cuda| {
            let a_buf = cuda.load_bytes(f32s_as_bytes(a))?;
            let b_buf = cuda.load_bytes(f32s_as_bytes(b))?;
            let out_buf = cuda.alloc_f32(a.len())?;
            cuda.mul_rows_vec_f32(&a_buf, &b_buf, &out_buf, rows, cols)?;
            cuda.read_f32s(&out_buf, a.len())
        })
    }

    pub(super) fn try_gelu_f32(a: &[f32], shape: &[usize]) -> Option<Vec<f32>> {
        if shape_numel(shape)? != a.len() {
            return None;
        }
        if a.is_empty() {
            return Some(Vec::new());
        }

        with_cuda_runtime(|cuda| {
            let input_buf = cuda.load_bytes(f32s_as_bytes(a))?;
            let out_buf = cuda.alloc_f32(a.len())?;
            cuda.gelu_f32_precise(&input_buf, &out_buf, a.len())?;
            cuda.read_f32s(&out_buf, a.len())
        })
    }

    pub(super) fn try_layer_norm_f32(_x: &[f32], _shape: &[usize], _eps: f32) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_rms_norm_f32(x: &[f32], shape: &[usize], eps: f32) -> Option<Vec<f32>> {
        let (rows, cols) = rows_cols_for_last_dim(shape, x.len())?;
        if x.is_empty() {
            return Some(Vec::new());
        }

        with_cuda_runtime(|cuda| {
            let x_buf = cuda.load_bytes(f32s_as_bytes(x))?;
            let out_buf = cuda.alloc_f32(x.len())?;
            cuda.rms_norm_rows_no_scale_f32_precise(&x_buf, &out_buf, rows, cols, cols, eps)?;
            cuda.read_f32s(&out_buf, x.len())
        })
    }

    pub(super) fn try_rms_norm_mul_f32(
        x: &[f32],
        x_shape: &[usize],
        mul: &[f32],
        mul_shape: &[usize],
        eps: f32,
    ) -> Option<Vec<f32>> {
        let (rows, cols) = rows_cols_for_last_dim(x_shape, x.len())?;
        if !is_last_dim_vector(mul_shape, cols, mul.len()) {
            return None;
        }
        if x.is_empty() {
            return Some(Vec::new());
        }

        with_cuda_runtime(|cuda| {
            let x_buf = cuda.load_bytes(f32s_as_bytes(x))?;
            let mul_buf = cuda.load_bytes(f32s_as_bytes(mul))?;
            let out_buf = cuda.alloc_f32(x.len())?;
            cuda.rms_norm_rows_weighted_f32_f32weights_precise(
                &x_buf, &mul_buf, &out_buf, rows, cols, cols, eps,
            )?;
            cuda.read_f32s(&out_buf, x.len())
        })
    }

    pub(super) fn try_attention_softmax_weighted_sum_f32(
        logits: &[f32],
        values: &[f32],
        query_count: usize,
        seq_len: usize,
        head_dim: usize,
    ) -> Option<Vec<f32>> {
        if logits.len() != query_count.checked_mul(seq_len)? {
            return None;
        }
        if values.len() != seq_len.checked_mul(head_dim)? {
            return None;
        }
        if logits.is_empty() || values.is_empty() {
            return Some(Vec::new());
        }

        // Dense full-precision path: row softmax over the logits followed by
        // probs @ values through cuBLAS. (The previous wiring reused a bf16
        // KV-cache ring-buffer kernel whose value layout did not match this
        // entry point's [seq_len][head_dim] contract.)
        with_cuda_runtime(|cuda| {
            let logits_buf = cuda.load_bytes(f32s_as_bytes(logits))?;
            let probs_buf = cuda.alloc_f32(logits.len())?;
            cuda.softmax_rows_precise_f32(&logits_buf, &probs_buf, query_count, seq_len, seq_len)?;
            let values_buf = cuda.load_bytes(f32s_as_bytes(values))?;
            let out_len = query_count
                .checked_mul(head_dim)
                .ok_or_else(|| "CUDA attention output length overflow".to_string())?;
            let out_buf = cuda.alloc_f32(out_len)?;
            cuda.matmul_nn_f32(
                &probs_buf,
                &values_buf,
                &out_buf,
                query_count,
                seq_len,
                head_dim,
            )?;
            cuda.read_f32s(&out_buf, out_len)
        })
    }

    pub(super) fn try_layer_norm_mul_add_f32(
        x: &[f32],
        x_shape: &[usize],
        mul: &[f32],
        mul_shape: &[usize],
        add: &[f32],
        add_shape: &[usize],
        eps: f32,
    ) -> Option<Vec<f32>> {
        let (rows, cols) = rows_cols_for_last_dim(x_shape, x.len())?;
        if !is_last_dim_vector(mul_shape, cols, mul.len())
            || !is_last_dim_vector(add_shape, cols, add.len())
        {
            return None;
        }
        if x.is_empty() {
            return Some(Vec::new());
        }

        with_cuda_runtime(|cuda| {
            let x_buf = cuda.load_bytes(f32s_as_bytes(x))?;
            let mul_buf = cuda.load_bytes(f32s_as_bytes(mul))?;
            let add_buf = cuda.load_bytes(f32s_as_bytes(add))?;
            let out_buf = cuda.alloc_f32(x.len())?;
            cuda.layer_norm_mul_add_f32(&x_buf, &mul_buf, &add_buf, &out_buf, rows, cols, eps)?;
            cuda.read_f32s(&out_buf, x.len())
        })
    }

    pub(super) fn try_get_rows_ggml_bytes(
        _src: &[u8],
        _src_ggml_type: u32,
        _n_cols: usize,
        _n_rows: usize,
        _row_indices: &[i32],
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_im2col_1d_f32(
        _input: &[f32],
        _ic: usize,
        _iw: usize,
        _kw: usize,
        _stride: usize,
        _pad: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_conv2d_planar_f32(
        input: &[f32],
        width: usize,
        height: usize,
        in_channels: usize,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
        kw: usize,
        kh: usize,
        pad_x: usize,
        pad_y: usize,
    ) -> Option<Vec<f32>> {
        let plane = width.checked_mul(height)?;
        if input.len() != plane.checked_mul(in_channels)?
            || weights.len()
                != out_channels
                    .checked_mul(in_channels)?
                    .checked_mul(kw.checked_mul(kh)?)?
            || bias.len() != out_channels
        {
            return None;
        }
        let out_len = plane.checked_mul(out_channels)?;
        if out_len == 0 {
            return Some(Vec::new());
        }

        with_cuda_runtime(|cuda| {
            let input_buf = cuda.load_bytes(f32s_as_bytes(input))?;
            let weights_buf = cuda.load_bytes(f32s_as_bytes(weights))?;
            let bias_buf = cuda.load_bytes(f32s_as_bytes(bias))?;
            let out_buf = cuda.alloc_f32(out_len)?;
            cuda.conv2d_planar_f32(
                &input_buf,
                &weights_buf,
                &bias_buf,
                &out_buf,
                width,
                height,
                in_channels,
                out_channels,
                kw,
                kh,
                pad_x,
                pad_y,
            )?;
            cuda.read_f32s(&out_buf, out_len)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_group_norm_planar_f32(
        input: &[f32],
        width: usize,
        height: usize,
        channels: usize,
        groups: usize,
        gamma: &[f32],
        beta: &[f32],
        eps: f32,
    ) -> Option<Vec<f32>> {
        let plane = width.checked_mul(height)?;
        if groups == 0
            || channels % groups != 0
            || input.len() != plane.checked_mul(channels)?
            || gamma.len() != channels
            || beta.len() != channels
        {
            return None;
        }
        if input.is_empty() {
            return Some(Vec::new());
        }

        with_cuda_runtime(|cuda| {
            let input_buf = cuda.load_bytes(f32s_as_bytes(input))?;
            let gamma_buf = cuda.load_bytes(f32s_as_bytes(gamma))?;
            let beta_buf = cuda.load_bytes(f32s_as_bytes(beta))?;
            let stats_buf = cuda.alloc_f32(groups * 2)?;
            let out_buf = cuda.alloc_f32(input.len())?;
            cuda.group_norm_planar_f32(
                &input_buf, &gamma_buf, &beta_buf, &stats_buf, &out_buf, width, height,
                channels, groups, eps,
            )?;
            cuda.read_f32s(&out_buf, input.len())
        })
    }

    pub(super) fn try_silu_f32(a: &[f32]) -> Option<Vec<f32>> {
        if a.is_empty() {
            return Some(Vec::new());
        }
        with_cuda_runtime(|cuda| {
            let input_buf = cuda.load_bytes(f32s_as_bytes(a))?;
            let out_buf = cuda.alloc_f32(a.len())?;
            cuda.silu_f32_precise(&input_buf, &out_buf, a.len())?;
            cuda.read_f32s(&out_buf, a.len())
        })
    }
}

// CUDA kernels absent (no nvcc at build time): every bounce reports
// "not handled" so callers fall back to their CPU paths, matching the
// old makepad-ggml stub semantics. See makepad-ai-cuda/build.rs for the
// links-metadata handshake that drives the cfg.
#[cfg(all(not(target_os = "macos"), not(makepad_ai_cuda_kernels)))]
#[allow(unused_variables)]
mod imp {
    pub(super) fn try_matmul_nn_f32(
        a: &[f32],
        b: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_f32(
        a: &[f32],
        bt: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_f32_bytes(
        _a: &[f32],
        _bt_bytes: &[u8],
        _m: usize,
        _k: usize,
        _n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_f16_bytes(
        _a: &[f32],
        _bt_f16_bytes: &[u8],
        _m: usize,
        _k: usize,
        _n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes(
        _a: &[f32],
        _bt_bytes: &[u8],
        _bt_ggml_type: u32,
        _m: usize,
        _k: usize,
        _n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes_keyed<F>(
        _a: &[f32],
        _bt_ggml_type: u32,
        _m: usize,
        _k: usize,
        _n: usize,
        _namespace: &str,
        _cache_key: &str,
        _load: F,
    ) -> Option<Vec<f32>>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes_multi(
        _a: &[f32],
        _m: usize,
        _k: usize,
        _matrices: &[super::MatmulNtGgmlBytesMatrix<'_>],
    ) -> Option<Vec<Vec<f32>>> {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes_keyed_multi<F>(
        _a: &[f32],
        _m: usize,
        _k: usize,
        _matrices: &[super::MatmulNtGgmlBytesKeyedMatrix<'_>],
        _load: F,
    ) -> Option<Vec<Vec<f32>>>
    where
        F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_ar_pre_attn<F>(
        _hidden: Option<&[f32]>,
        _m: usize,
        _hidden_w: usize,
        _head_dim: usize,
        _in_norm: &[f32],
        _qk_norm: Option<(&[f32], &[f32], &str, &str)>,
        _in_norm_key: &str,
        _eps: f32,
        _q: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _k: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _v: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _load: F,
    ) -> Option<(Vec<f32>, Vec<f32>, Vec<f32>)>
    where
        F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_ar_post_attn<F>(
        _attn: &[f32],
        _m: usize,
        _hidden_w: usize,
        _post_norm: &[f32],
        _post_norm_key: &str,
        _eps: f32,
        _o: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _up: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _gate: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _down: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _load: F,
    ) -> Option<()>
    where
        F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_ar_final_rms(
        _m: usize,
        _hidden_w: usize,
        _gamma: &[f32],
        _gamma_key: &str,
        _eps: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn ar_resident_clear() {}

    pub(super) fn transient_pool_clear() {}

    pub(super) fn try_dit_ffn_resident<F>(
        _normed: &[f32],
        _m: usize,
        _hidden_w: usize,
        _ff_dim: usize,
        _ff_in_b: &[f32],
        _ff_out_b: &[f32],
        _ff_in_b_key: &str,
        _ff_out_b_key: &str,
        _swap: bool,
        _ff_in: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _ff_out: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _load: F,
    ) -> Option<Vec<f32>>
    where
        F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes_add_bias(
        _a: &[f32],
        _bt_bytes: &[u8],
        _bt_ggml_type: u32,
        _m: usize,
        _k: usize,
        _n: usize,
        _bias: &[f32],
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_vision_mlp_bf16_fused(
        _x: &[f32],
        _gate_up_weight_bytes: &[u8],
        _down_weight_bytes: &[u8],
        _rows: usize,
        _hidden_size: usize,
        _intermediate_size: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_flash_attn_f32_packed(
        q: &[f32],
        k: &[f32],
        v: &[f32],
        n_q: usize,
        n_kv: usize,
        n_head: usize,
        d: usize,
        scale: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn clear_decoder_kv_cache() {}

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_vit_backbone_resident_f32(
        _x: &[f32],
        _seq_len: usize,
        _n_state: usize,
        _n_head: usize,
        _rot_half: usize,
        _cos: &[f32],
        _sin: &[f32],
        _layers: &[super::VitLayerRef<'_>],
        _final_norm_w: &[f32],
        _final_norm_b: &[f32],
        _eps: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_two_way_layer_resident_f32(
        _hidden: &[f32],
        _token_pe: &[f32],
        _context: &[f32],
        _context_pe: &[f32],
        _n_tok: usize,
        _dim: usize,
        _n_ctx: usize,
        _ctx_dim: usize,
        _layer: &super::TwoWayLayerRef<'_>,
    ) -> Option<(Vec<f32>, Vec<f32>)> {
        None
    }

    pub(super) fn try_flash_attn_f32_self_kv_cache(
        _layer: usize,
        _q: &[f32],
        _k_all: &[f32],
        _v_all: &[f32],
        _n_kv: usize,
        _n_head: usize,
        _d: usize,
        _scale: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_flash_attn_f32_cross_kv_cache(
        _layer: usize,
        _q: &[f32],
        _k_cross: &[f32],
        _v_cross: &[f32],
        _n_q: usize,
        _n_kv: usize,
        _n_head: usize,
        _d: usize,
        _scale: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_add_f32(
        a: &[f32],
        a_shape: &[usize],
        b: &[f32],
        b_shape: &[usize],
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_mul_f32(
        a: &[f32],
        a_shape: &[usize],
        b: &[f32],
        b_shape: &[usize],
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_gelu_f32(a: &[f32], shape: &[usize]) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_layer_norm_f32(_x: &[f32], _shape: &[usize], _eps: f32) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_rms_norm_f32(x: &[f32], shape: &[usize], eps: f32) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_rms_norm_mul_f32(
        x: &[f32],
        x_shape: &[usize],
        mul: &[f32],
        mul_shape: &[usize],
        eps: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_attention_softmax_weighted_sum_f32(
        logits: &[f32],
        values: &[f32],
        query_count: usize,
        seq_len: usize,
        head_dim: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_layer_norm_mul_add_f32(
        x: &[f32],
        x_shape: &[usize],
        mul: &[f32],
        mul_shape: &[usize],
        add: &[f32],
        add_shape: &[usize],
        eps: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_get_rows_ggml_bytes(
        _src: &[u8],
        _src_ggml_type: u32,
        _n_cols: usize,
        _n_rows: usize,
        _row_indices: &[i32],
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_im2col_1d_f32(
        _input: &[f32],
        _ic: usize,
        _iw: usize,
        _kw: usize,
        _stride: usize,
        _pad: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_conv2d_planar_f32(
        input: &[f32],
        width: usize,
        height: usize,
        in_channels: usize,
        weights: &[f32],
        bias: &[f32],
        out_channels: usize,
        kw: usize,
        kh: usize,
        pad_x: usize,
        pad_y: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_group_norm_planar_f32(
        input: &[f32],
        width: usize,
        height: usize,
        channels: usize,
        groups: usize,
        gamma: &[f32],
        beta: &[f32],
        eps: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_silu_f32(a: &[f32]) -> Option<Vec<f32>> {
        None
    }
}


#[cfg(target_os = "macos")]
mod imp {
    use makepad_ai_cuda::quant::{
        block_elements, block_size, f32_to_f16, ggml_type_name, GGML_TYPE_BF16, GGML_TYPE_F16,
        GGML_TYPE_F32, GGML_TYPE_I32, GGML_TYPE_Q2_K, GGML_TYPE_Q3_K, GGML_TYPE_Q4_0,
        GGML_TYPE_Q4_1, GGML_TYPE_Q4_K, GGML_TYPE_Q5_0, GGML_TYPE_Q5_1, GGML_TYPE_Q5_K,
        GGML_TYPE_Q6_K, GGML_TYPE_Q8_0,
    };
    use makepad_objc_sys::runtime::{nil, ObjcId, Object, NO};
    use makepad_objc_sys::{class, msg_send, sel, sel_impl};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::{c_char, c_void, CStr};
    use std::ptr::NonNull;
    use std::sync::OnceLock;
    use std::thread;
    use std::time::Duration;

    const LOG_METAL_PIPELINES: bool = false;
    const DISABLE_GGML_METAL_BF16: bool = false;
    const DISABLE_GGML_METAL_TENSOR: bool = false;
    const FORCE_ENABLE_GGML_METAL_TENSOR: bool = false;
    const UTF8_ENCODING: u64 = 4;
    const MTL_RESOURCE_STORAGE_MODE_SHARED: u64 = 0;
    const MTL_RESOURCE_OPTIONS_STORAGE_MODE_PRIVATE: u64 = 32;
    const MTL_STORAGE_MODE_PRIVATE: u64 = 2;
    const MTL_GPU_FAMILY_APPLE6: u64 = 1006;
    const MTL_GPU_FAMILY_METAL3: u64 = 5001;
    const MTL_GPU_FAMILY_METAL4: u64 = 5002;

    const MTL_DATA_TYPE_INT: u64 = 29;
    const MTL_DATA_TYPE_SHORT: u64 = 37;
    const MTL_DATA_TYPE_BOOL: u64 = 53;

    const FC_FLASH_ATTN_EXT_PAD: i32 = 100;
    const FC_FLASH_ATTN_EXT_BLK: i32 = 200;
    const FC_FLASH_ATTN_EXT: i32 = 300;
    const FC_FLASH_ATTN_EXT_VEC: i32 = 400;
    const FC_FLASH_ATTN_EXT_VEC_REDUCE: i32 = 500;
    const FC_MUL_MV: i32 = 600;
    const FC_MUL_MM: i32 = 700;
    const FC_UNARY: i32 = 1200;
    const FC_BIN: i32 = 1300;
    const OP_FLASH_ATTN_EXT_NQPSG: i32 = 8;
    const OP_FLASH_ATTN_EXT_NCPSG: i32 = 64;
    const OP_FLASH_ATTN_EXT_VEC_NQPSG: i32 = 1;
    const OP_FLASH_ATTN_EXT_VEC_NCPSG: i32 = 32;
    const OP_UNARY_NUM_GELU: i16 = 103;
    const OP_UNARY_NUM_SILU: i16 = 106;
    /// Persistent-scratch tag range `[VIT_TAG_BASE, VIT_TAG_BASE + 20)` of the
    /// resident ViT stack (`vit_layer_from_buffer_f32`).
    const VIT_TAG_BASE: u8 = 200;
    const OP_UNARY_NUM_GELU_ERF: i16 = 104;
    /// Cached-affine tag range `[TWO_WAY_TAG_BASE, TWO_WAY_TAG_BASE + 14)` of
    /// the resident two-way decoder layer.
    const TWO_WAY_TAG_BASE: u8 = 180;
    const SCRATCH_FLASH_PAD: u8 = 1;
    const SCRATCH_FLASH_BLK: u8 = 2;
    const SCRATCH_FLASH_TMP: u8 = 3;
    const METAL_INIT_ATTEMPTS: usize = 12;
    const METAL_INIT_RETRY_DELAY_MS: u64 = 100;
    const SCRATCH_FLASH_OUT: u8 = 4;
    const SCRATCH_FLASH_MASK: u8 = 5;
    const SCRATCH_ENC_NORM0: u8 = 10;
    const SCRATCH_ENC_NORM1: u8 = 11;
    const SCRATCH_DEC_NORM0: u8 = 12;
    const SCRATCH_DEC_NORM1: u8 = 13;
    #[allow(dead_code)]
    const SCRATCH_ENC_FLASH_K_F16: u8 = 14;
    #[allow(dead_code)]
    const SCRATCH_ENC_FLASH_V_F16: u8 = 15;

    const N_R0_Q4_0: i32 = 4;
    const N_SG_Q4_0: i32 = 2;

    const N_R0_Q4_1: i32 = 4;
    const N_SG_Q4_1: i32 = 2;

    const N_R0_Q5_0: i32 = 4;
    const N_SG_Q5_0: i32 = 2;

    const N_R0_Q5_1: i32 = 4;
    const N_SG_Q5_1: i32 = 2;

    const N_R0_Q8_0: i32 = 2;
    const N_SG_Q8_0: i32 = 4;
    const N_R0_Q2_K: i32 = 4;
    const N_SG_Q2_K: i32 = 2;
    const N_R0_Q3_K: i32 = 2;
    const N_SG_Q3_K: i32 = 2;
    const N_R0_Q4_K: i32 = 2;
    const N_SG_Q4_K: i32 = 2;
    const N_R0_Q5_K: i32 = 1;
    const N_SG_Q5_K: i32 = 2;
    const N_R0_Q6_K: i32 = 2;
    const N_SG_Q6_K: i32 = 2;

    // Shader tree now lives in makepad-ai-metal (libs/ai/metal, lane T5,
    // /aiarch.md §1 + §8b); reached via the MAKEPAD_GGML_METAL_SHADER_DIR
    // env, re-emitted by this crate's build.rs from the
    // DEP_MAKEPAD_AI_METAL_SHADER_DIR links-metadata handshake. File
    // contents are byte-identical to before the move.
    const _GGML_METAL_SOURCE_RAW: &str = include_str!(concat!(
        env!("MAKEPAD_GGML_METAL_SHADER_DIR"),
        "/ggml-metal.metal"
    ));
    const _GGML_COMMON_H: &str = include_str!(concat!(
        env!("MAKEPAD_GGML_METAL_SHADER_DIR"),
        "/ggml-common.h"
    ));
    const _GGML_METAL_IMPL_H: &str = include_str!(concat!(
        env!("MAKEPAD_GGML_METAL_SHADER_DIR"),
        "/ggml-metal-impl.h"
    ));
    const _GGML_METALLIB_BYTES: &[u8] = include_bytes!(env!("MAKEPAD_GGML_METALLIB"));

    #[link(name = "Metal", kind = "framework")]
    extern "C" {
        fn MTLCreateSystemDefaultDevice() -> ObjcId;
        fn MTLCopyAllDevices() -> ObjcId;
    }

    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct MTLSize {
        width: u64,
        height: u64,
        depth: u64,
    }

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    struct BufferKey {
        ptr: usize,
        len: usize,
        tag: u8,
    }

    #[allow(non_camel_case_types)]
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    enum Src0Type {
        F32,
        F16,
        BF16,
        Q4_0,
        Q4_1,
        Q5_0,
        Q5_1,
        Q8_0,
        Q2_K,
        Q3_K,
        Q4_K,
        Q5_K,
        Q6_K,
    }

    #[derive(Clone, Copy, Debug)]
    enum FunctionConstantValue {
        Int32(i32),
        Int16(i16),
        Bool(bool),
    }

    #[derive(Clone, Copy, Debug)]
    struct FunctionConstant {
        idx: i32,
        value: FunctionConstantValue,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsMulMm {
        ne00: i32,
        ne02: i32,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne12: i32,
        nb10: u64,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        ne0: i32,
        ne1: i32,
        r2: i16,
        r3: i16,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsMulMv {
        ne00: i32,
        ne01: i32,
        ne02: i32,
        nb00: u64,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne10: i32,
        ne11: i32,
        ne12: i32,
        nb10: u64,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        ne0: i32,
        ne1: i32,
        nr0: i32,
        r2: i16,
        r3: i16,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsMulMvExt {
        ne00: i32,
        ne01: i32,
        ne02: i32,
        nb00: u64,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne10: i32,
        ne11: i32,
        ne12: i32,
        nb10: u64,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        ne0: i32,
        ne1: i32,
        r2: i16,
        r3: i16,
    }

    #[allow(dead_code)]
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsCpy {
        nk0: i64,
        ne00: i64,
        ne01: i64,
        ne02: i64,
        ne03: i64,
        nb00: u64,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne0: i64,
        ne1: i64,
        ne2: i64,
        ne3: i64,
        nb0: u64,
        nb1: u64,
        nb2: u64,
        nb3: u64,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsFlashAttnExtPad {
        ne11: i32,
        ne_12_2: i32,
        ne_12_3: i32,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        nb21: u64,
        nb22: u64,
        nb23: u64,
        ne31: i32,
        ne32: i32,
        ne33: i32,
        nb31: u64,
        nb32: u64,
        nb33: u64,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsFlashAttnExtBlk {
        ne01: i32,
        ne30: i32,
        ne31: i32,
        ne32: i32,
        ne33: i32,
        nb31: u64,
        nb32: u64,
        nb33: u64,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsFlashAttnExt {
        ne01: i32,
        ne02: i32,
        ne03: i32,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne11: i32,
        ne_12_2: i32,
        ne_12_3: i32,
        ns10: i32,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        ns20: i32,
        nb21: u64,
        nb22: u64,
        nb23: u64,
        ne31: i32,
        ne32: i32,
        ne33: i32,
        nb31: u64,
        nb32: u64,
        nb33: u64,
        ne1: i32,
        ne2: i32,
        ne3: i32,
        scale: f32,
        max_bias: f32,
        m0: f32,
        m1: f32,
        n_head_log2: i32,
        logit_softcap: f32,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsFlashAttnExtVec {
        ne01: i32,
        ne02: i32,
        ne03: i32,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne11: i32,
        ne_12_2: i32,
        ne_12_3: i32,
        ns10: i32,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        ns20: i32,
        nb21: u64,
        nb22: u64,
        nb23: u64,
        ne31: i32,
        ne32: i32,
        ne33: i32,
        nb31: u64,
        nb32: u64,
        nb33: u64,
        ne1: i32,
        ne2: i32,
        ne3: i32,
        scale: f32,
        max_bias: f32,
        m0: f32,
        m1: f32,
        n_head_log2: i32,
        logit_softcap: f32,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsFlashAttnExtVecReduce {
        nrows: i32,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsRopeHalfTables {
        token_count: i32,
        head_count: i32,
        head_dim: i32,
        rot_half: i32,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsUnary {
        ne00: i32,
        ne01: i32,
        ne02: i32,
        ne03: i32,
        nb00: u64,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne0: i32,
        ne1: i32,
        ne2: i32,
        ne3: i32,
        nb0: u64,
        nb1: u64,
        nb2: u64,
        nb3: u64,
        slope: f32,
        scale: f32,
        bias: f32,
        val: f32,
        min: f32,
        max: f32,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsBin {
        ne00: i32,
        ne01: i32,
        ne02: i32,
        ne03: i32,
        nb00: u64,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne10: i32,
        ne11: i32,
        ne12: i32,
        ne13: i32,
        nb10: u64,
        nb11: u64,
        nb12: u64,
        nb13: u64,
        ne0: i32,
        ne1: i32,
        ne2: i32,
        ne3: i32,
        nb0: u64,
        nb1: u64,
        nb2: u64,
        nb3: u64,
        offs: u64,
        o1: [u64; 8],
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsNorm {
        ne00: i32,
        ne00_t: i32,
        nb1: u64,
        nb2: u64,
        nb3: u64,
        eps: f32,
        nef1: [i32; 3],
        nef2: [i32; 3],
        nef3: [i32; 3],
        nbf1: [u64; 3],
        nbf2: [u64; 3],
        nbf3: [u64; 3],
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsGetRows {
        ne00t: i32,
        ne00: i32,
        nb01: u64,
        nb02: u64,
        nb03: u64,
        ne10: i32,
        nb10: u64,
        nb11: u64,
        nb12: u64,
        nb1: u64,
        nb2: u64,
        nb3: u64,
    }

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct KArgsIm2Col {
        ofs0: u64,
        ofs1: u64,
        iw: i32,
        ih: i32,
        chw: i32,
        s0: i32,
        s1: i32,
        p0: i32,
        p1: i32,
        d0: i32,
        d1: i32,
        n: i32,
        kh: i32,
        kw: i32,
        khw: i32,
    }

    #[derive(Copy, Clone)]
    struct Shape4 {
        ne: [i32; 4],
        nb: [u64; 4],
        numel: usize,
    }

    struct StrongId(NonNull<Object>);

    impl StrongId {
        unsafe fn from_owned(id: ObjcId) -> Option<Self> {
            NonNull::new(id).map(Self)
        }

        unsafe fn from_unowned(id: ObjcId) -> Option<Self> {
            if id.is_null() {
                return None;
            }
            let _: () = msg_send![id, retain];
            NonNull::new(id).map(Self)
        }

        fn as_id(&self) -> ObjcId {
            self.0.as_ptr()
        }
    }

    impl Drop for StrongId {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0.as_ptr(), release];
            }
        }
    }

    struct AutoreleasePool(ObjcId);

    impl AutoreleasePool {
        fn new() -> Self {
            let pool: ObjcId = unsafe { msg_send![class!(NSAutoreleasePool), new] };
            Self(pool)
        }
    }

    impl Drop for AutoreleasePool {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _: () = msg_send![self.0, release];
                }
            }
        }
    }

    fn nsstring_to_string(ns_string: ObjcId) -> String {
        if ns_string.is_null() {
            return String::new();
        }
        unsafe {
            let utf8_ptr: *const c_char = msg_send![ns_string, UTF8String];
            if utf8_ptr.is_null() {
                return String::new();
            }
            CStr::from_ptr(utf8_ptr).to_string_lossy().into_owned()
        }
    }

    fn str_to_nsstring_owned(s: &str) -> ObjcId {
        unsafe {
            let ns_string: ObjcId = msg_send![class!(NSString), alloc];
            if ns_string.is_null() {
                return nil;
            }
            msg_send![
                ns_string,
                initWithBytes: s.as_ptr() as *const c_void
                length: s.len() as u64
                encoding: UTF8_ENCODING
            ]
        }
    }

    fn ns_error_to_string(error: ObjcId) -> String {
        if error.is_null() {
            return "unknown Metal error".to_string();
        }
        unsafe {
            let desc: ObjcId = msg_send![error, localizedDescription];
            nsstring_to_string(desc)
        }
    }

    fn device_supports_family(device: ObjcId, family: u64) -> bool {
        unsafe { msg_send![device, supportsFamily: family] }
    }

    fn metal_compile_feature_macros(device: ObjcId) -> (bool, bool) {
        let mut has_bfloat = device_supports_family(device, MTL_GPU_FAMILY_METAL3)
            || device_supports_family(device, MTL_GPU_FAMILY_APPLE6);
        if DISABLE_GGML_METAL_BF16 {
            has_bfloat = false;
        }

        let mut has_tensor = device_supports_family(device, MTL_GPU_FAMILY_METAL4);
        if DISABLE_GGML_METAL_TENSOR {
            has_tensor = false;
        }

        if !FORCE_ENABLE_GGML_METAL_TENSOR && has_tensor {
            let dev_name_obj: ObjcId = unsafe { msg_send![device, name] };
            let dev_name = nsstring_to_string(dev_name_obj);
            let tensor_whitelisted = dev_name.contains("M5")
                || dev_name.contains("M6")
                || dev_name.contains("A19")
                || dev_name.contains("A20");
            if !tensor_whitelisted {
                has_tensor = false;
            }
        }

        (has_bfloat, has_tensor)
    }

    fn read_text_with_fallback(paths: &[&str], fallback: &str) -> String {
        for path in paths {
            if let Ok(text) = std::fs::read_to_string(path) {
                return text;
            }
        }
        fallback.to_string()
    }

    fn build_ggml_source() -> String {
        // Development path: prefer source files from disk so shader-pruning tools can
        // iterate without rebuilding the Rust crate every edit.
        let mut src = read_text_with_fallback(
            &[
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/src/backend/metal/ggml/ggml-metal.metal"
                ),
                "libs/ggml/src/backend/metal/ggml/ggml-metal.metal",
            ],
            _GGML_METAL_SOURCE_RAW,
        );
        let common_h = read_text_with_fallback(
            &[
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/src/backend/metal/ggml/ggml-common.h"
                ),
                "libs/ggml/src/backend/metal/ggml/ggml-common.h",
            ],
            _GGML_COMMON_H,
        );
        let impl_h = read_text_with_fallback(
            &[
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/src/backend/metal/ggml/ggml-metal-impl.h"
                ),
                "libs/ggml/src/backend/metal/ggml/ggml-metal-impl.h",
            ],
            _GGML_METAL_IMPL_H,
        );
        src = src.replace("__embed_ggml-common.h__", &common_h);
        src = src.replace("#include \"ggml-common.h\"", &common_h);
        src = src.replace("#include \"ggml-metal-impl.h\"", &impl_h);
        src
    }

    fn src0_type_from_ggml(t: u32) -> Option<Src0Type> {
        match t {
            GGML_TYPE_F32 => Some(Src0Type::F32),
            GGML_TYPE_F16 => Some(Src0Type::F16),
            GGML_TYPE_BF16 => Some(Src0Type::BF16),
            GGML_TYPE_Q4_0 => Some(Src0Type::Q4_0),
            GGML_TYPE_Q4_1 => Some(Src0Type::Q4_1),
            GGML_TYPE_Q5_0 => Some(Src0Type::Q5_0),
            GGML_TYPE_Q5_1 => Some(Src0Type::Q5_1),
            GGML_TYPE_Q8_0 => Some(Src0Type::Q8_0),
            GGML_TYPE_Q2_K => Some(Src0Type::Q2_K),
            GGML_TYPE_Q3_K => Some(Src0Type::Q3_K),
            GGML_TYPE_Q4_K => Some(Src0Type::Q4_K),
            GGML_TYPE_Q5_K => Some(Src0Type::Q5_K),
            GGML_TYPE_Q6_K => Some(Src0Type::Q6_K),
            _ => None,
        }
    }

    fn src0_type_name(t: Src0Type) -> &'static str {
        match t {
            Src0Type::F32 => "f32",
            Src0Type::F16 => "f16",
            Src0Type::BF16 => "bf16",
            Src0Type::Q4_0 => "q4_0",
            Src0Type::Q4_1 => "q4_1",
            Src0Type::Q5_0 => "q5_0",
            Src0Type::Q5_1 => "q5_1",
            Src0Type::Q8_0 => "q8_0",
            Src0Type::Q2_K => "q2_K",
            Src0Type::Q3_K => "q3_K",
            Src0Type::Q4_K => "q4_K",
            Src0Type::Q5_K => "q5_K",
            Src0Type::Q6_K => "q6_K",
        }
    }

    fn src0_layout_bytes_per_row(t: Src0Type, k: usize) -> Result<(usize, u64), String> {
        match t {
            Src0Type::F32 => Ok((
                k.checked_mul(4)
                    .ok_or_else(|| "overflow computing f32 row bytes".to_string())?,
                4,
            )),
            Src0Type::F16 => Ok((
                k.checked_mul(2)
                    .ok_or_else(|| "overflow computing f16 row bytes".to_string())?,
                2,
            )),
            Src0Type::BF16 => Ok((
                k.checked_mul(2)
                    .ok_or_else(|| "overflow computing bf16 row bytes".to_string())?,
                2,
            )),
            Src0Type::Q4_0
            | Src0Type::Q4_1
            | Src0Type::Q5_0
            | Src0Type::Q5_1
            | Src0Type::Q8_0
            | Src0Type::Q2_K
            | Src0Type::Q3_K
            | Src0Type::Q4_K
            | Src0Type::Q5_K
            | Src0Type::Q6_K => {
                let ggml_type = match t {
                    Src0Type::Q4_0 => GGML_TYPE_Q4_0,
                    Src0Type::Q4_1 => GGML_TYPE_Q4_1,
                    Src0Type::Q5_0 => GGML_TYPE_Q5_0,
                    Src0Type::Q5_1 => GGML_TYPE_Q5_1,
                    Src0Type::Q8_0 => GGML_TYPE_Q8_0,
                    Src0Type::Q2_K => GGML_TYPE_Q2_K,
                    Src0Type::Q3_K => GGML_TYPE_Q3_K,
                    Src0Type::Q4_K => GGML_TYPE_Q4_K,
                    Src0Type::Q5_K => GGML_TYPE_Q5_K,
                    Src0Type::Q6_K => GGML_TYPE_Q6_K,
                    _ => unreachable!(),
                };
                let blck = block_elements(ggml_type);
                if k % blck != 0 {
                    return Err(format!(
                        "quantized kernel requires K multiple of {}, got {}",
                        blck, k
                    ));
                }
                let bs = block_size(ggml_type);
                let row = (k / blck)
                    .checked_mul(bs)
                    .ok_or_else(|| "overflow computing quantized row bytes".to_string())?;
                Ok((row, bs as u64))
            }
        }
    }

    fn shape4_from_row_major(shape: &[usize], elem_bytes: u64) -> Result<Shape4, String> {
        if shape.is_empty() {
            return Err("shape must be non-empty".to_string());
        }
        if shape.len() > 4 {
            return Err(format!(
                "shape rank > 4 is unsupported in metal elementwise path: {:?}",
                shape
            ));
        }
        let mut ne = [1i32; 4];
        for (i, &d) in shape.iter().rev().enumerate() {
            ne[i] = i32::try_from(d).map_err(|_| format!("shape dim too large: {}", d))?;
        }
        let mut nb = [0u64; 4];
        nb[0] = elem_bytes;
        for i in 1..4 {
            nb[i] = nb[i - 1]
                .checked_mul(ne[i - 1] as u64)
                .ok_or_else(|| "overflow computing strides".to_string())?;
        }
        let numel = shape.iter().try_fold(1usize, |acc, &d| {
            acc.checked_mul(d)
                .ok_or_else(|| "overflow computing tensor numel".to_string())
        })?;
        Ok(Shape4 { ne, nb, numel })
    }

    fn nrows(s: &Shape4) -> usize {
        (s.ne[1] as usize)
            .saturating_mul(s.ne[2] as usize)
            .saturating_mul(s.ne[3] as usize)
    }

    fn f32_slice_as_bytes(s: &[f32]) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                s.as_ptr() as *const u8,
                s.len() * std::mem::size_of::<f32>(),
            )
        }
    }

    fn flash_attn_supported_head_dim(d: usize) -> bool {
        matches!(
            d,
            32 | 40 | 48 | 64 | 72 | 80 | 96 | 112 | 128 | 192 | 256 | 576
        )
    }

    fn flash_attn_use_vec(n_q: usize, d: usize) -> bool {
        n_q < 20 && d % 32 == 0
    }

    fn flash_attn_kv_elem_bytes(src_type: Src0Type) -> Result<usize, String> {
        match src_type {
            Src0Type::F32 => Ok(std::mem::size_of::<f32>()),
            Src0Type::F16 => Ok(std::mem::size_of::<u16>()),
            _ => Err(format!(
                "unsupported flash-attn kv type for Metal path: {:?}",
                src_type
            )),
        }
    }

    #[derive(Clone, Copy, Default)]
    struct FlashAttnExtParams {
        has_mask: bool,
        has_sinks: bool,
        max_bias: f32,
        logit_softcap: f32,
    }

    fn pad_to(v: usize, align: usize) -> usize {
        if align == 0 {
            return v;
        }
        let rem = v % align;
        if rem == 0 {
            v
        } else {
            v + (align - rem)
        }
    }

    fn flash_attn_smem_bytes(dk: usize, dv: usize, _nsg: i32) -> usize {
        let nqptg = OP_FLASH_ATTN_EXT_NQPSG as usize;
        let ncpsg = OP_FLASH_ATTN_EXT_NCPSG as usize;

        // Matches ggml-metal FATTN_SMEM() for non-quantized f32 K/V.
        let words = nqptg.saturating_mul(dk + 2 * pad_to(dv, 64) + 2 * (2 * ncpsg));
        pad_to(words.saturating_mul(std::mem::size_of::<f32>() / 2), 16)
    }

    fn flash_attn_vec_smem_bytes(dk: usize, dv: usize, nsg: i32) -> usize {
        let ncpsg = OP_FLASH_ATTN_EXT_VEC_NCPSG as usize;
        let words =
            (pad_to(dk, 128) + 4 * ncpsg + 2 * pad_to(dv, 128)).saturating_mul(nsg.max(1) as usize);
        pad_to(words.saturating_mul(std::mem::size_of::<f32>() / 2), 16)
    }

    fn matmul_cache_tag(bt_ggml_type: u32) -> u8 {
        match bt_ggml_type {
            GGML_TYPE_F32 => 2u8,
            GGML_TYPE_F16 => 3u8,
            GGML_TYPE_BF16 => 9u8,
            GGML_TYPE_Q4_0 => 4u8,
            GGML_TYPE_Q4_1 => 5u8,
            GGML_TYPE_Q5_0 => 6u8,
            GGML_TYPE_Q5_1 => 7u8,
            GGML_TYPE_Q8_0 => 8u8,
            GGML_TYPE_Q2_K => 10u8,
            GGML_TYPE_Q3_K => 11u8,
            GGML_TYPE_Q4_K => 12u8,
            GGML_TYPE_Q5_K => 13u8,
            GGML_TYPE_Q6_K => 14u8,
            _ => 0u8,
        }
    }

    fn matmul_batch_tag(bt_ggml_type: u32, index: usize) -> Result<u8, String> {
        let base = matmul_cache_tag(bt_ggml_type);
        let slot = u8::try_from(index).map_err(|_| format!("matmul batch index too large: {}", index))?;
        if slot >= 8 {
            return Err(format!("matmul batch supports at most 8 outputs, got {}", index + 1));
        }
        Ok(128u8
            .wrapping_add(base.wrapping_mul(8))
            .wrapping_add(slot))
    }

    fn flash_attn_ext_extra_pad_bytes(
        n_q: usize,
        n_kv: usize,
        n_head: usize,
        d: usize,
        kv_elem_bytes: usize,
        has_mask: bool,
        use_vec: bool,
    ) -> Result<usize, String> {
        // Match ggml-metal: reserve non-vec sized padding space, but gate by the active kernel kvpad.
        let reserve_ncpsg = OP_FLASH_ATTN_EXT_NCPSG as usize;
        let active_ncpsg = if use_vec {
            OP_FLASH_ATTN_EXT_VEC_NCPSG as usize
        } else {
            OP_FLASH_ATTN_EXT_NCPSG as usize
        };
        let has_kvpad = n_kv % active_ncpsg != 0;
        if !has_kvpad {
            return Ok(0);
        }

        let n_state = n_head
            .checked_mul(d)
            .ok_or_else(|| "overflow computing flash n_state".to_string())?;
        let nb11 = n_state
            .checked_mul(kv_elem_bytes)
            .ok_or_else(|| "overflow computing flash nb11".to_string())?;
        let nb21 = nb11;

        let k_term = nb11
            .checked_mul(n_head)
            .ok_or_else(|| "overflow computing flash extra pad K bytes".to_string())?;
        let v_term = nb21
            .checked_mul(n_head)
            .ok_or_else(|| "overflow computing flash extra pad V bytes".to_string())?;
        let mask_term = if has_mask {
            std::mem::size_of::<u16>()
                .checked_mul(n_q)
                .ok_or_else(|| "overflow computing flash extra pad mask bytes".to_string())?
        } else {
            0
        };

        reserve_ncpsg
            .checked_mul(
                k_term
                    .checked_add(v_term)
                    .and_then(|v| v.checked_add(mask_term))
                    .ok_or_else(|| "overflow computing flash extra pad size".to_string())?,
            )
            .ok_or_else(|| "overflow computing flash extra pad size".to_string())
    }

    fn flash_attn_ext_extra_blk_bytes(
        n_q: usize,
        n_kv: usize,
        has_mask: bool,
        use_vec: bool,
    ) -> Result<usize, String> {
        if !has_mask {
            return Ok(0);
        }

        let nqptg = if use_vec {
            OP_FLASH_ATTN_EXT_VEC_NQPSG as usize
        } else {
            OP_FLASH_ATTN_EXT_NQPSG as usize
        };
        let ncpsg = if use_vec {
            OP_FLASH_ATTN_EXT_VEC_NCPSG as usize
        } else {
            OP_FLASH_ATTN_EXT_NCPSG as usize
        };

        let ne1 = (n_q + nqptg - 1) / nqptg;
        let ne0 = (n_kv + ncpsg - 1) / ncpsg;
        let raw = ne0
            .checked_mul(ne1)
            .ok_or_else(|| "overflow computing flash extra blk size".to_string())?;

        Ok(pad_to(raw, 32))
    }

    fn flash_attn_ext_extra_tmp_bytes(
        n_q: usize,
        n_head: usize,
        d: usize,
        nwg: usize,
    ) -> Result<usize, String> {
        let ne01_max = n_q.min(32);
        std::mem::size_of::<f32>()
            .checked_mul(ne01_max)
            .and_then(|v| v.checked_mul(n_head))
            .and_then(|v| v.checked_mul(nwg))
            .and_then(|v| v.checked_mul(d + 2))
            .ok_or_else(|| "overflow computing flash extra tmp size".to_string())
    }

    fn can_use_mul_mv_ext(src0: Src0Type, ne00: i32, ne11: i32) -> bool {
        if ne00 % 128 != 0 {
            return false;
        }
        if !(2..=8).contains(&ne11) {
            return false;
        }
        matches!(
            src0,
            Src0Type::F32
                | Src0Type::F16
                | Src0Type::BF16
                | Src0Type::Q4_0
                | Src0Type::Q4_1
                | Src0Type::Q5_0
                | Src0Type::Q5_1
                | Src0Type::Q8_0
                | Src0Type::Q2_K
                | Src0Type::Q3_K
                | Src0Type::Q4_K
                | Src0Type::Q5_K
                | Src0Type::Q6_K
        )
    }

    struct PipelineState {
        obj: StrongId,
        smem: usize,
        nsg: i32,
        nr0: i32,
        nr1: i32,
    }

    struct DecoderKvLayer {
        k: StrongId,
        v: StrongId,
        n_state: usize,
        cap_rows: usize,
        len_rows: usize,
    }

    struct CrossKvLayer {
        k: StrongId,
        v: StrongId,
        n_state: usize,
        n_rows: usize,
        src_k_ptr: usize,
        src_v_ptr: usize,
        src_k_len: usize,
        src_v_len: usize,
    }

    struct ScratchBuffer {
        buf: StrongId,
        cap_bytes: usize,
        is_private: bool,
    }

    struct PoolBuf {
        cap_bytes: usize,
        buf: StrongId,
    }

    /// Keep at most this many bytes of recycled transient buffers. Shapes in
    /// the decode loops repeat every layer, so the pool stays small; the cap
    /// only matters for one-off giants (DiT prefill strips).
    const POOL_CAP_BYTES: usize = 1280 * 1024 * 1024;

    fn pool_disabled() -> bool {
        static OFF: OnceLock<bool> = OnceLock::new();
        *OFF.get_or_init(|| std::env::var_os("MAKEPAD_MUSIC3_NO_POOL").is_some())
    }

    struct MetalContext {
        device: StrongId,
        command_queue: StrongId,
        library: StrongId,
        pipeline_cache: HashMap<String, PipelineState>,
        cached_weight_buffers: HashMap<BufferKey, StrongId>,
        named_weight_buffers: HashMap<(String, String), StrongId>,
        act_buffers: HashMap<String, (StrongId, usize)>,
        scratch_buffers: HashMap<u8, ScratchBuffer>,
        matmul_out_buffers: HashMap<u8, ScratchBuffer>,
        decoder_kv_layers: HashMap<usize, DecoderKvLayer>,
        cross_kv_layers: HashMap<usize, CrossKvLayer>,
        batch_depth: usize,
        batch_command_buffer: Option<StrongId>,
        batch_encoder: Option<StrongId>,
        last_command_buffer: Option<StrongId>,
        pool_free: Vec<PoolBuf>,
        pool_in_flight: Vec<PoolBuf>,
    }

    impl MetalContext {
        fn create_device() -> Option<StrongId> {
            unsafe {
                let dev = MTLCreateSystemDefaultDevice();
                if let Some(dev) = StrongId::from_owned(dev) {
                    return Some(dev);
                }

                let all = MTLCopyAllDevices();
                if all.is_null() {
                    return None;
                }

                let count: u64 = msg_send![all, count];
                let first: ObjcId = if count > 0 {
                    msg_send![all, objectAtIndex: 0u64]
                } else {
                    nil
                };
                let _: () = msg_send![all, release];

                StrongId::from_unowned(first)
            }
        }

        fn new() -> Result<Self, String> {
            let _pool = AutoreleasePool::new();
            let mut last_err = None;
            for attempt in 0..METAL_INIT_ATTEMPTS {
                let Some(device) = Self::create_device() else {
                    last_err = Some(
                        "unable to create Metal device (MTLCreateSystemDefaultDevice and MTLCopyAllDevices returned nil)"
                            .to_string(),
                    );
                    if attempt + 1 < METAL_INIT_ATTEMPTS {
                        thread::sleep(Duration::from_millis(METAL_INIT_RETRY_DELAY_MS));
                    }
                    continue;
                };

                let command_queue_obj: ObjcId =
                    unsafe { msg_send![device.as_id(), newCommandQueue] };
                let Some(command_queue) = (unsafe { StrongId::from_owned(command_queue_obj) })
                else {
                    last_err = Some("newCommandQueue returned nil".to_string());
                    if attempt + 1 < METAL_INIT_ATTEMPTS {
                        thread::sleep(Duration::from_millis(METAL_INIT_RETRY_DELAY_MS));
                    }
                    continue;
                };

                let library = match Self::load_library_from_metallib(device.as_id()) {
                    Ok(Some(lib)) => lib,
                    Ok(None) => {
                        let source = build_ggml_source();
                        Self::compile_library(device.as_id(), &source)?
                    }
                    Err(err) => {
                        eprintln!(
                            "[ggml][metal] precompiled metallib load failed, compiling source: {}",
                            err
                        );
                        let source = build_ggml_source();
                        Self::compile_library(device.as_id(), &source)?
                    }
                };

                if super::log_metal_trace() {
                    eprintln!("[ggml][metal] backend initialized (shared kernels)");
                }

                return Ok(Self {
                    device,
                    command_queue,
                    library,
                    pipeline_cache: HashMap::new(),
                    cached_weight_buffers: HashMap::new(),
                    named_weight_buffers: HashMap::new(),
                    act_buffers: HashMap::new(),
                    scratch_buffers: HashMap::new(),
                    matmul_out_buffers: HashMap::new(),
                    decoder_kv_layers: HashMap::new(),
                    cross_kv_layers: HashMap::new(),
                    batch_depth: 0,
                    batch_command_buffer: None,
                    batch_encoder: None,
                    last_command_buffer: None,
                    pool_free: Vec::new(),
                    pool_in_flight: Vec::new(),
                });
            }

            Err(last_err.unwrap_or_else(|| "unable to create Metal backend".to_string()))
        }

        fn load_library_from_metallib(device: ObjcId) -> Result<Option<StrongId>, String> {
            if _GGML_METALLIB_BYTES.is_empty() {
                return Ok(None);
            }

            let _pool = AutoreleasePool::new();

            let data_obj: ObjcId = unsafe {
                msg_send![
                    class!(NSData),
                    dataWithBytes: _GGML_METALLIB_BYTES.as_ptr() as *const c_void
                    length: _GGML_METALLIB_BYTES.len() as u64
                ]
            };
            if data_obj.is_null() {
                return Err("NSData::dataWithBytes returned nil".to_string());
            }

            let mut error: ObjcId = nil;
            let library_obj: ObjcId =
                unsafe { msg_send![device, newLibraryWithData: data_obj error: &mut error] };
            if library_obj.is_null() {
                return Err(format!(
                    "newLibraryWithData failed: {}",
                    ns_error_to_string(error)
                ));
            }

            let library = unsafe { StrongId::from_owned(library_obj) }
                .ok_or_else(|| "newLibraryWithData returned nil".to_string())?;
            Ok(Some(library))
        }

        fn compile_library(device: ObjcId, source: &str) -> Result<StrongId, String> {
            let _pool = AutoreleasePool::new();

            let options_obj: ObjcId = unsafe { msg_send![class!(MTLCompileOptions), new] };
            let options = unsafe { StrongId::from_owned(options_obj) }
                .ok_or_else(|| "MTLCompileOptions::new returned nil".to_string())?;
            unsafe {
                let _: () = msg_send![options.as_id(), setFastMathEnabled: NO];
            }

            let (has_bfloat, has_tensor) = metal_compile_feature_macros(device);
            if has_bfloat || has_tensor {
                let prep_obj: ObjcId =
                    unsafe { msg_send![class!(NSMutableDictionary), dictionary] };
                if !prep_obj.is_null() {
                    if has_bfloat {
                        let key_obj = str_to_nsstring_owned("GGML_METAL_HAS_BF16");
                        let val_obj = str_to_nsstring_owned("1");
                        let key = unsafe { StrongId::from_owned(key_obj) }
                            .ok_or_else(|| "failed to build metal macro key".to_string())?;
                        let val = unsafe { StrongId::from_owned(val_obj) }
                            .ok_or_else(|| "failed to build metal macro value".to_string())?;
                        unsafe {
                            let _: () =
                                msg_send![prep_obj, setObject: val.as_id() forKey: key.as_id()];
                        }
                    }
                    if has_tensor {
                        let key_obj = str_to_nsstring_owned("GGML_METAL_HAS_TENSOR");
                        let val_obj = str_to_nsstring_owned("1");
                        let key = unsafe { StrongId::from_owned(key_obj) }
                            .ok_or_else(|| "failed to build metal macro key".to_string())?;
                        let val = unsafe { StrongId::from_owned(val_obj) }
                            .ok_or_else(|| "failed to build metal macro value".to_string())?;
                        unsafe {
                            let _: () =
                                msg_send![prep_obj, setObject: val.as_id() forKey: key.as_id()];
                        }
                    }
                    unsafe {
                        let _: () = msg_send![options.as_id(), setPreprocessorMacros: prep_obj];
                    }
                }
            }

            let source_obj = str_to_nsstring_owned(source);
            let source_obj = unsafe { StrongId::from_owned(source_obj) }
                .ok_or_else(|| "failed to create NSString for Metal source".to_string())?;

            let mut error: ObjcId = nil;
            let library_obj: ObjcId = unsafe {
                msg_send![
                    device,
                    newLibraryWithSource: source_obj.as_id()
                    options: options.as_id()
                    error: &mut error
                ]
            };

            unsafe { StrongId::from_owned(library_obj) }.ok_or_else(|| {
                format!("newLibraryWithSource failed: {}", ns_error_to_string(error))
            })
        }

        fn new_buffer_with_bytes(&self, bytes: &[u8]) -> Result<StrongId, String> {
            let obj: ObjcId = unsafe {
                msg_send![
                    self.device.as_id(),
                    newBufferWithBytes: bytes.as_ptr() as *const c_void
                    length: bytes.len() as u64
                    options: MTL_RESOURCE_STORAGE_MODE_SHARED
                ]
            };
            unsafe { StrongId::from_owned(obj) }
                .ok_or_else(|| format!("newBufferWithBytes failed for {} bytes", bytes.len()))
        }

        fn new_buffer_with_length(&self, byte_len: usize) -> Result<StrongId, String> {
            let obj: ObjcId = unsafe {
                msg_send![
                    self.device.as_id(),
                    newBufferWithLength: byte_len as u64
                    options: MTL_RESOURCE_STORAGE_MODE_SHARED
                ]
            };
            unsafe { StrongId::from_owned(obj) }
                .ok_or_else(|| format!("newBufferWithLength failed for {} bytes", byte_len))
        }

        fn new_buffer_with_length_private(&self, byte_len: usize) -> Result<StrongId, String> {
            let obj: ObjcId = unsafe {
                msg_send![
                    self.device.as_id(),
                    newBufferWithLength: byte_len as u64
                    options: MTL_RESOURCE_OPTIONS_STORAGE_MODE_PRIVATE
                ]
            };
            unsafe { StrongId::from_owned(obj) }.ok_or_else(|| {
                format!("newBufferWithLength(private) failed for {} bytes", byte_len)
            })
        }

        /// Take a shared transient buffer (upload or GEMM dst). Exact-size
        /// reuse: the decode loops re-issue the same shapes every layer, so
        /// after one layer the free list serves every request without
        /// touching the Metal allocator.
        fn pool_take(&mut self, byte_len: usize) -> Result<StrongId, String> {
            let need = byte_len.max(4);
            if !pool_disabled() {
                if let Some(i) = self
                    .pool_free
                    .iter()
                    .position(|p| p.cap_bytes == need)
                {
                    return Ok(self.pool_free.swap_remove(i).buf);
                }
            }
            self.new_buffer_with_length(need)
        }

        fn pool_take_filled(&mut self, bytes: &[u8]) -> Result<StrongId, String> {
            let buf = self.pool_take(bytes.len())?;
            let ptr: *mut c_void = unsafe { msg_send![buf.as_id(), contents] };
            if ptr.is_null() {
                return Err("pool buffer contents null".to_string());
            }
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
            }
            Ok(buf)
        }

        /// Park a transient buffer until the next queue wait proves the GPU
        /// is done with it.
        fn pool_give(&mut self, buf: StrongId) {
            if pool_disabled() {
                return;
            }
            let cap_bytes = unsafe {
                let len: u64 = msg_send![buf.as_id(), length];
                len as usize
            };
            self.pool_in_flight.push(PoolBuf { cap_bytes, buf });
        }

        /// Call only right after `wait_queue_idle` succeeded: everything the
        /// GPU could have touched is complete, so parked buffers become
        /// reusable.
        fn pool_recycle(&mut self) {
            if self.pool_in_flight.is_empty() {
                return;
            }
            self.pool_free.append(&mut self.pool_in_flight);
            let mut total: usize = self.pool_free.iter().map(|p| p.cap_bytes).sum();
            while total > POOL_CAP_BYTES {
                // Drop the largest first: giants are one-off strips.
                let Some(i) = self
                    .pool_free
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, p)| p.cap_bytes)
                    .map(|(i, _)| i)
                else {
                    break;
                };
                total -= self.pool_free[i].cap_bytes;
                self.pool_free.swap_remove(i);
            }
        }

        fn pool_clear(&mut self) {
            self.pool_free.clear();
            self.pool_in_flight.clear();
        }

        fn get_or_create_scratch_buffer(
            &mut self,
            kind: u8,
            need_bytes: usize,
        ) -> Result<ObjcId, String> {
            let need_bytes = need_bytes.max(1);
            let is_private = matches!(
                kind,
                SCRATCH_FLASH_PAD
                    | SCRATCH_FLASH_BLK
                    | SCRATCH_FLASH_TMP
                    | SCRATCH_FLASH_OUT
                    | SCRATCH_ENC_NORM0
                    | SCRATCH_ENC_NORM1
                    | SCRATCH_DEC_NORM0
                    | SCRATCH_DEC_NORM1
            );
            if let Some(entry) = self.scratch_buffers.get(&kind) {
                if entry.cap_bytes >= need_bytes && entry.is_private == is_private {
                    return Ok(entry.buf.as_id());
                }
            }

            let buf = if is_private {
                self.new_buffer_with_length_private(need_bytes)?
            } else {
                self.new_buffer_with_length(need_bytes)?
            };
            self.scratch_buffers.insert(
                kind,
                ScratchBuffer {
                    buf,
                    cap_bytes: need_bytes,
                    is_private,
                },
            );

            Ok(self.scratch_buffers.get(&kind).unwrap().buf.as_id())
        }

        fn get_or_create_matmul_out_buffer(
            &mut self,
            tag: u8,
            need_bytes: usize,
        ) -> Result<ObjcId, String> {
            let need_bytes = need_bytes.max(1);
            if let Some(entry) = self.matmul_out_buffers.get(&tag) {
                if entry.cap_bytes >= need_bytes {
                    return Ok(entry.buf.as_id());
                }
            }

            // Mirror ggml-metal backend allocation for compute intermediates:
            // matrix outputs are GPU-only private buffers.
            let buf = self.new_buffer_with_length_private(need_bytes)?;
            self.matmul_out_buffers.insert(
                tag,
                ScratchBuffer {
                    buf,
                    cap_bytes: need_bytes,
                    is_private: true,
                },
            );

            Ok(self.matmul_out_buffers.get(&tag).unwrap().buf.as_id())
        }

        fn buffer_length_bytes(&self, buffer: ObjcId) -> Result<usize, String> {
            let len_u64: u64 = unsafe { msg_send![buffer, length] };
            usize::try_from(len_u64).map_err(|_| format!("buffer length too large: {}", len_u64))
        }

        fn zero_buffer_range(
            &self,
            buffer: ObjcId,
            offset_bytes: usize,
            len_bytes: usize,
        ) -> Result<(), String> {
            if len_bytes == 0 {
                return Ok(());
            }
            let buf_len = self.buffer_length_bytes(buffer)?;
            let end = offset_bytes
                .checked_add(len_bytes)
                .ok_or_else(|| "overflow computing zero range end".to_string())?;
            if end > buf_len {
                return Err(format!(
                    "zero range out of bounds: offset={}, len={}, buffer_len={}",
                    offset_bytes, len_bytes, buf_len
                ));
            }
            let ptr: *mut u8 = unsafe { msg_send![buffer, contents] };
            if ptr.is_null() {
                return Err("buffer contents returned null".to_string());
            }
            unsafe {
                std::ptr::write_bytes(ptr.add(offset_bytes), 0u8, len_bytes);
            }
            Ok(())
        }

        fn prepare_decoder_self_mask_f16(
            &mut self,
            n_valid: usize,
            n_total: usize,
        ) -> Result<ObjcId, String> {
            if n_total == 0 || n_valid > n_total {
                return Err(format!(
                    "invalid decoder self mask sizes: n_valid={}, n_total={}",
                    n_valid, n_total
                ));
            }

            let mask_bytes = n_total
                .checked_mul(std::mem::size_of::<u16>())
                .ok_or_else(|| "overflow computing decoder self mask bytes".to_string())?;
            let mask_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_MASK, mask_bytes)?;
            let mask_cap = self.buffer_length_bytes(mask_id)?;
            if mask_bytes > mask_cap {
                return Err(format!(
                    "decoder self mask write exceeds buffer: need={}, cap={}",
                    mask_bytes, mask_cap
                ));
            }
            let ptr: *mut u16 = unsafe { msg_send![mask_id, contents] };
            if ptr.is_null() {
                return Err("decoder self mask buffer contents returned null".to_string());
            }

            let neg_inf_h = f32_to_f16(f32::NEG_INFINITY);
            unsafe {
                let dst = std::slice::from_raw_parts_mut(ptr, n_total);
                for v in dst.iter_mut().take(n_valid) {
                    *v = 0;
                }
                for v in dst.iter_mut().skip(n_valid) {
                    *v = neg_inf_h;
                }
            }

            Ok(mask_id)
        }

        fn copy_f32_buffer_contents(
            &self,
            buffer: ObjcId,
            elems: usize,
        ) -> Result<Vec<f32>, String> {
            let byte_len = elems
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing f32 copy byte length".to_string())?;
            let cap = self.buffer_length_bytes(buffer)?;
            if byte_len > cap {
                return Err(format!(
                    "f32 read exceeds buffer: need={} bytes, cap={} bytes",
                    byte_len, cap
                ));
            }
            let out_ptr: *const c_void = unsafe { msg_send![buffer, contents] };
            if out_ptr.is_null() {
                return Err("output buffer contents returned null".to_string());
            }

            let mut out = vec![0.0f32; elems];
            unsafe {
                std::ptr::copy_nonoverlapping(out_ptr as *const f32, out.as_mut_ptr(), elems);
            }
            Ok(out)
        }

        fn copy_buffer_to_shared_staging(
            &self,
            src_buffer: ObjcId,
            len_bytes: usize,
        ) -> Result<StrongId, String> {
            let len_bytes = len_bytes.max(1);
            let src_len = self.buffer_length_bytes(src_buffer)?;
            if len_bytes > src_len {
                return Err(format!(
                    "staging copy exceeds source buffer: need={} bytes, src_len={} bytes",
                    len_bytes, src_len
                ));
            }
            let dst_buffer = self.new_buffer_with_length(len_bytes)?;

            let command_buffer_obj: ObjcId =
                unsafe { msg_send![self.command_queue.as_id(), commandBuffer] };
            let command_buffer = unsafe { StrongId::from_unowned(command_buffer_obj) }
                .ok_or_else(|| "commandBuffer returned nil".to_string())?;

            let blit_encoder_obj: ObjcId =
                unsafe { msg_send![command_buffer.as_id(), blitCommandEncoder] };
            let blit_encoder = unsafe { StrongId::from_unowned(blit_encoder_obj) }
                .ok_or_else(|| "blitCommandEncoder returned nil".to_string())?;

            unsafe {
                let _: () = msg_send![
                    blit_encoder.as_id(),
                    copyFromBuffer: src_buffer
                    sourceOffset: 0u64
                    toBuffer: dst_buffer.as_id()
                    destinationOffset: 0u64
                    size: len_bytes as u64
                ];
                let _: () = msg_send![blit_encoder.as_id(), endEncoding];
                let _: () = msg_send![command_buffer.as_id(), commit];
                let _: () = msg_send![command_buffer.as_id(), waitUntilCompleted];
            }

            let status: u64 = unsafe { msg_send![command_buffer.as_id(), status] };
            if status == 5 {
                let error: ObjcId = unsafe { msg_send![command_buffer.as_id(), error] };
                return Err(format!(
                    "Metal command buffer error (buffer staging copy): {}",
                    ns_error_to_string(error)
                ));
            }

            Ok(dst_buffer)
        }

        fn copy_f32_buffer_contents_readable(
            &self,
            buffer: ObjcId,
            elems: usize,
        ) -> Result<Vec<f32>, String> {
            let byte_len = elems
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing readback byte length".to_string())?;
            let storage_mode: u64 = unsafe { msg_send![buffer, storageMode] };
            if storage_mode == MTL_STORAGE_MODE_PRIVATE {
                let staging = self.copy_buffer_to_shared_staging(buffer, byte_len)?;
                self.copy_f32_buffer_contents(staging.as_id(), elems)
            } else {
                self.copy_f32_buffer_contents(buffer, elems)
            }
        }

        fn read_f32_buffer(&self, buffer: ObjcId, elems: usize) -> Result<Vec<f32>, String> {
            self.wait_queue_idle()?;
            self.copy_f32_buffer_contents_readable(buffer, elems)
        }

        #[allow(dead_code)]
        fn read_f32_buffers3(
            &self,
            b0: ObjcId,
            b1: ObjcId,
            b2: ObjcId,
            elems: usize,
        ) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>), String> {
            self.wait_queue_idle()?;
            let o0 = self.copy_f32_buffer_contents_readable(b0, elems)?;
            let o1 = self.copy_f32_buffer_contents_readable(b1, elems)?;
            let o2 = self.copy_f32_buffer_contents_readable(b2, elems)?;
            Ok((o0, o1, o2))
        }

        #[allow(dead_code)]
        fn get_or_create_cached_f32_buffer(
            &mut self,
            data: &[f32],
            tag: u8,
        ) -> Result<ObjcId, String> {
            let bytes = f32_slice_as_bytes(data);
            let key = BufferKey {
                ptr: data.as_ptr() as usize,
                len: bytes.len(),
                tag,
            };
            self.get_or_create_weight_buffer(key, bytes)
        }

        fn get_or_create_weight_buffer(
            &mut self,
            key: BufferKey,
            bytes: &[u8],
        ) -> Result<ObjcId, String> {
            if !self.cached_weight_buffers.contains_key(&key) {
                let buf = self.new_buffer_with_bytes(bytes)?;
                self.cached_weight_buffers.insert(key, buf);
            }
            Ok(self.cached_weight_buffers.get(&key).unwrap().as_id())
        }

        fn get_or_create_named_weight_buffer(
            &mut self,
            namespace: &str,
            cache_key: &str,
            load: impl FnOnce() -> Result<Vec<u8>, String>,
        ) -> Result<ObjcId, String> {
            let key = (namespace.to_string(), cache_key.to_string());
            if !self.named_weight_buffers.contains_key(&key) {
                let bytes = load()?;
                let buf = self.new_buffer_with_bytes(&bytes)?;
                self.named_weight_buffers.insert(key.clone(), buf);
            }
            Ok(self.named_weight_buffers.get(&key).unwrap().as_id())
        }

        fn clear_decoder_kv_cache(&mut self) {
            self.decoder_kv_layers.clear();
            self.cross_kv_layers.clear();
        }

        fn ensure_decoder_kv_layer(
            &mut self,
            layer: usize,
            n_state: usize,
            need_rows: usize,
        ) -> Result<(ObjcId, ObjcId), String> {
            let need_rows = need_rows.max(1);

            if let Some(entry) = self.decoder_kv_layers.get(&layer) {
                if entry.n_state == n_state && entry.cap_rows >= need_rows {
                    return Ok((entry.k.as_id(), entry.v.as_id()));
                }
            }

            let row_bytes = n_state
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing decoder kv row bytes".to_string())?;

            let old = self.decoder_kv_layers.remove(&layer);
            let cap_rows = if let Some(ref old) = old {
                if old.n_state == n_state {
                    old.cap_rows.saturating_mul(2).max(need_rows).max(32)
                } else {
                    need_rows.max(32)
                }
            } else {
                need_rows.max(32)
            };
            let total_bytes = cap_rows
                .checked_mul(row_bytes)
                .ok_or_else(|| "overflow computing decoder kv bytes".to_string())?;

            let new_k = self.new_buffer_with_length(total_bytes)?;
            let new_v = self.new_buffer_with_length(total_bytes)?;

            let mut len_rows = 0usize;
            if let Some(old) = old {
                if old.n_state == n_state && old.len_rows > 0 {
                    let copy_rows = old.len_rows.min(cap_rows);
                    let copy_bytes = copy_rows
                        .checked_mul(row_bytes)
                        .ok_or_else(|| "overflow computing decoder kv copy bytes".to_string())?;
                    let old_k_ptr: *const u8 = unsafe { msg_send![old.k.as_id(), contents] };
                    let old_v_ptr: *const u8 = unsafe { msg_send![old.v.as_id(), contents] };
                    let new_k_ptr: *mut u8 = unsafe { msg_send![new_k.as_id(), contents] };
                    let new_v_ptr: *mut u8 = unsafe { msg_send![new_v.as_id(), contents] };
                    if old_k_ptr.is_null()
                        || old_v_ptr.is_null()
                        || new_k_ptr.is_null()
                        || new_v_ptr.is_null()
                    {
                        return Err("decoder kv buffer contents returned null".to_string());
                    }
                    unsafe {
                        std::ptr::copy_nonoverlapping(old_k_ptr, new_k_ptr, copy_bytes);
                        std::ptr::copy_nonoverlapping(old_v_ptr, new_v_ptr, copy_bytes);
                    }
                    len_rows = copy_rows;
                }
            }

            self.decoder_kv_layers.insert(
                layer,
                DecoderKvLayer {
                    k: new_k,
                    v: new_v,
                    n_state,
                    cap_rows,
                    len_rows,
                },
            );
            let entry = self
                .decoder_kv_layers
                .get(&layer)
                .ok_or_else(|| "decoder kv layer insertion failed".to_string())?;
            Ok((entry.k.as_id(), entry.v.as_id()))
        }

        fn ensure_cross_kv_layer(
            &mut self,
            layer: usize,
            n_state: usize,
            n_rows: usize,
            k_cross: &[f32],
            v_cross: &[f32],
        ) -> Result<(ObjcId, ObjcId), String> {
            if n_state == 0 || n_rows == 0 {
                return Err(format!(
                    "invalid cross kv dimensions: n_state={}, n_rows={}",
                    n_state, n_rows
                ));
            }
            if k_cross.len() != v_cross.len() {
                return Err(format!(
                    "cross kv k/v len mismatch: k={}, v={}",
                    k_cross.len(),
                    v_cross.len(),
                ));
            }
            if k_cross.len() % n_state != 0 {
                return Err(format!(
                    "cross kv len not divisible by n_state: len={}, n_state={}",
                    k_cross.len(),
                    n_state
                ));
            }

            let src_rows = k_cross.len() / n_state;
            if src_rows == 0 {
                return Err("cross kv has zero rows".to_string());
            }
            if src_rows > n_rows {
                return Err(format!(
                    "cross kv source rows exceed requested rows: src_rows={}, n_rows={}",
                    src_rows, n_rows
                ));
            }

            let src_k_ptr = k_cross.as_ptr() as usize;
            let src_v_ptr = v_cross.as_ptr() as usize;
            let src_k_len = k_cross.len();
            let src_v_len = v_cross.len();

            if let Some(entry) = self.cross_kv_layers.get(&layer) {
                if entry.n_state == n_state
                    && entry.n_rows == n_rows
                    && entry.src_k_ptr == src_k_ptr
                    && entry.src_v_ptr == src_v_ptr
                    && entry.src_k_len == src_k_len
                    && entry.src_v_len == src_v_len
                {
                    return Ok((entry.k.as_id(), entry.v.as_id()));
                }
            }

            let row_bytes = n_state
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing cross kv row bytes".to_string())?;
            let total_bytes = n_rows
                .checked_mul(row_bytes)
                .ok_or_else(|| "overflow computing cross kv total bytes".to_string())?;
            let copy_bytes = src_rows
                .checked_mul(row_bytes)
                .ok_or_else(|| "overflow computing cross kv copy bytes".to_string())?;

            let (k_buf, v_buf) = if src_rows == n_rows {
                (
                    self.new_buffer_with_bytes(f32_slice_as_bytes(k_cross))?,
                    self.new_buffer_with_bytes(f32_slice_as_bytes(v_cross))?,
                )
            } else {
                let k_buf = self.new_buffer_with_length(total_bytes)?;
                let v_buf = self.new_buffer_with_length(total_bytes)?;

                let dst_k: *mut u8 = unsafe { msg_send![k_buf.as_id(), contents] };
                let dst_v: *mut u8 = unsafe { msg_send![v_buf.as_id(), contents] };
                if dst_k.is_null() || dst_v.is_null() {
                    return Err("cross kv buffer contents returned null".to_string());
                }
                unsafe {
                    std::ptr::write_bytes(dst_k, 0u8, total_bytes);
                    std::ptr::write_bytes(dst_v, 0u8, total_bytes);
                    std::ptr::copy_nonoverlapping(k_cross.as_ptr() as *const u8, dst_k, copy_bytes);
                    std::ptr::copy_nonoverlapping(v_cross.as_ptr() as *const u8, dst_v, copy_bytes);
                }

                (k_buf, v_buf)
            };
            self.cross_kv_layers.insert(
                layer,
                CrossKvLayer {
                    k: k_buf,
                    v: v_buf,
                    n_state,
                    n_rows,
                    src_k_ptr,
                    src_v_ptr,
                    src_k_len,
                    src_v_len,
                },
            );

            let entry = self
                .cross_kv_layers
                .get(&layer)
                .ok_or_else(|| "cross kv layer insertion failed".to_string())?;
            Ok((entry.k.as_id(), entry.v.as_id()))
        }

        fn compile_pipeline(
            &self,
            base_name: &str,
            constants: &[FunctionConstant],
        ) -> Result<StrongId, String> {
            let _pool = AutoreleasePool::new();

            let base_obj = str_to_nsstring_owned(base_name);
            let base_obj = unsafe { StrongId::from_owned(base_obj) }
                .ok_or_else(|| format!("failed to create NSString for function '{}'", base_name))?;

            let mut error: ObjcId = nil;
            let func_obj: ObjcId = if constants.is_empty() {
                unsafe { msg_send![self.library.as_id(), newFunctionWithName: base_obj.as_id()] }
            } else {
                let cv_obj: ObjcId = unsafe { msg_send![class!(MTLFunctionConstantValues), new] };
                let cv = unsafe { StrongId::from_owned(cv_obj) }
                    .ok_or_else(|| "MTLFunctionConstantValues::new returned nil".to_string())?;

                for c in constants {
                    unsafe {
                        match c.value {
                            FunctionConstantValue::Int32(v) => {
                                let _: () = msg_send![
                                    cv.as_id(),
                                    setConstantValue: &v as *const i32 as *const c_void
                                    type: MTL_DATA_TYPE_INT
                                    atIndex: c.idx as u64
                                ];
                            }
                            FunctionConstantValue::Int16(v) => {
                                let _: () = msg_send![
                                    cv.as_id(),
                                    setConstantValue: &v as *const i16 as *const c_void
                                    type: MTL_DATA_TYPE_SHORT
                                    atIndex: c.idx as u64
                                ];
                            }
                            FunctionConstantValue::Bool(v) => {
                                let b: u8 = if v { 1 } else { 0 };
                                let _: () = msg_send![
                                    cv.as_id(),
                                    setConstantValue: &b as *const u8 as *const c_void
                                    type: MTL_DATA_TYPE_BOOL
                                    atIndex: c.idx as u64
                                ];
                            }
                        }
                    }
                }

                unsafe {
                    msg_send![
                        self.library.as_id(),
                        newFunctionWithName: base_obj.as_id()
                        constantValues: cv.as_id()
                        error: &mut error
                    ]
                }
            };

            let func = unsafe { StrongId::from_owned(func_obj) }.ok_or_else(|| {
                format!(
                    "newFunctionWithName('{}') failed: {}",
                    base_name,
                    ns_error_to_string(error)
                )
            })?;

            let mut error: ObjcId = nil;
            let pipeline_obj: ObjcId = unsafe {
                msg_send![
                    self.device.as_id(),
                    newComputePipelineStateWithFunction: func.as_id()
                    error: &mut error
                ]
            };

            unsafe { StrongId::from_owned(pipeline_obj) }.ok_or_else(|| {
                format!(
                    "newComputePipelineStateWithFunction('{}') failed: {}",
                    base_name,
                    ns_error_to_string(error)
                )
            })
        }

        fn get_or_compile_cached_pipeline(
            &mut self,
            cache_name: String,
            base_name: &str,
            constants: &[FunctionConstant],
            smem: usize,
            nr0: i32,
            nr1: i32,
            nsg: i32,
        ) -> Result<(ObjcId, usize, i32, i32, i32), String> {
            if !self.pipeline_cache.contains_key(&cache_name) {
                if LOG_METAL_PIPELINES {
                    eprintln!("[ggml][metal] compile_pipeline base={}", base_name);
                }
                let compiled = self.compile_pipeline(base_name, constants)?;
                self.pipeline_cache.insert(
                    cache_name.clone(),
                    PipelineState {
                        obj: compiled,
                        smem,
                        nsg,
                        nr0,
                        nr1,
                    },
                );
            }

            let p = self.pipeline_cache.get(&cache_name).unwrap();
            Ok((p.obj.as_id(), p.smem, p.nr0, p.nr1, p.nsg))
        }

        fn pipeline_max_threads(pipeline: ObjcId) -> u64 {
            unsafe { msg_send![pipeline, maxTotalThreadsPerThreadgroup] }
        }

        fn create_compute_encoder(&self, command_buffer: ObjcId) -> Result<StrongId, String> {
            let encoder_obj: ObjcId = unsafe { msg_send![command_buffer, computeCommandEncoder] };

            unsafe { StrongId::from_unowned(encoder_obj) }
                .ok_or_else(|| "computeCommandEncoder returned nil".to_string())
        }

        fn begin_batch(&mut self) -> Result<(), String> {
            if self.batch_depth == 0 {
                let command_buffer_obj: ObjcId =
                    unsafe { msg_send![self.command_queue.as_id(), commandBuffer] };
                let command_buffer = unsafe { StrongId::from_unowned(command_buffer_obj) }
                    .ok_or_else(|| "commandBuffer returned nil".to_string())?;

                let encoder = self.create_compute_encoder(command_buffer.as_id())?;

                self.batch_command_buffer = Some(command_buffer);
                self.batch_encoder = Some(encoder);
            }
            self.batch_depth += 1;
            Ok(())
        }

        fn end_batch(&mut self) -> Result<(), String> {
            if self.batch_depth == 0 {
                return Err("end_batch called with no active batch".to_string());
            }

            self.batch_depth -= 1;
            if self.batch_depth == 0 {
                let command_buffer = self
                    .batch_command_buffer
                    .take()
                    .ok_or_else(|| "batch command buffer missing".to_string())?;
                let encoder = self
                    .batch_encoder
                    .take()
                    .ok_or_else(|| "batch encoder missing".to_string())?;

                unsafe {
                    let _: () = msg_send![encoder.as_id(), endEncoding];
                    let _: () = msg_send![command_buffer.as_id(), commit];
                }

                self.last_command_buffer = Some(command_buffer);
            }

            Ok(())
        }

        fn with_batch<T, F>(&mut self, f: F) -> Result<T, String>
        where
            F: FnOnce(&mut Self) -> Result<T, String>,
        {
            self.begin_batch()?;
            let out = f(self);
            let end_res = self.end_batch();
            match (out, end_res) {
                (Ok(v), Ok(())) => Ok(v),
                (Err(e), Ok(())) => Err(e),
                (Ok(_), Err(e)) => Err(e),
                (Err(e), Err(_)) => Err(e),
            }
        }

        fn begin_command_encoder(
            &self,
        ) -> Result<(ObjcId, ObjcId, Option<(StrongId, StrongId)>), String> {
            if self.batch_depth > 0 {
                let command_buffer = self
                    .batch_command_buffer
                    .as_ref()
                    .ok_or_else(|| "batch command buffer missing".to_string())?;
                let encoder = self
                    .batch_encoder
                    .as_ref()
                    .ok_or_else(|| "batch encoder missing".to_string())?;
                return Ok((command_buffer.as_id(), encoder.as_id(), None));
            }

            let command_buffer_obj: ObjcId =
                unsafe { msg_send![self.command_queue.as_id(), commandBuffer] };
            let command_buffer = unsafe { StrongId::from_unowned(command_buffer_obj) }
                .ok_or_else(|| "commandBuffer returned nil".to_string())?;

            let encoder = self.create_compute_encoder(command_buffer.as_id())?;

            Ok((
                command_buffer.as_id(),
                encoder.as_id(),
                Some((command_buffer, encoder)),
            ))
        }

        fn wait_queue_idle(&self) -> Result<(), String> {
            if self.batch_depth > 0 {
                return Err("wait_queue_idle called while command batch is active".to_string());
            }

            if let Some(command_buffer) = self.last_command_buffer.as_ref() {
                let command_buffer_id = command_buffer.as_id();
                unsafe {
                    let _: () = msg_send![command_buffer_id, waitUntilCompleted];
                }
                let status: u64 = unsafe { msg_send![command_buffer_id, status] };
                if status == 5 {
                    let error: ObjcId = unsafe { msg_send![command_buffer_id, error] };
                    return Err(format!(
                        "Metal command buffer error (queue idle wait): {}",
                        ns_error_to_string(error)
                    ));
                }
                return Ok(());
            }

            let command_buffer_obj: ObjcId =
                unsafe { msg_send![self.command_queue.as_id(), commandBuffer] };
            let command_buffer = unsafe { StrongId::from_unowned(command_buffer_obj) }
                .ok_or_else(|| "commandBuffer returned nil".to_string())?;
            unsafe {
                let _: () = msg_send![command_buffer.as_id(), commit];
                let _: () = msg_send![command_buffer.as_id(), waitUntilCompleted];
            }
            let status: u64 = unsafe { msg_send![command_buffer.as_id(), status] };
            if status == 5 {
                let error: ObjcId = unsafe { msg_send![command_buffer.as_id(), error] };
                return Err(format!(
                    "Metal command buffer error (queue idle wait): {}",
                    ns_error_to_string(error)
                ));
            }
            Ok(())
        }

        fn end_command_encoder(
            &mut self,
            handles: Option<(StrongId, StrongId)>,
        ) -> Result<(), String> {
            let Some((command_buffer, encoder)) = handles else {
                return Ok(());
            };

            unsafe {
                let _: () = msg_send![encoder.as_id(), endEncoding];
                let _: () = msg_send![command_buffer.as_id(), commit];
            }

            self.last_command_buffer = Some(command_buffer);

            Ok(())
        }

        fn dispatch_mul_mv_ext(
            &mut self,
            src0: Src0Type,
            src0_id: ObjcId,
            src1_id: ObjcId,
            dst_id: ObjcId,
            ne00: i32,
            ne01: i32,
            ne10: i32,
            ne11: i32,
            nb00: u64,
            nb01: u64,
            nb10: u64,
            nb11: u64,
            ne0: i32,
            ne1: i32,
        ) -> Result<(), String> {
            static LOG_ONCE: OnceLock<()> = OnceLock::new();
            if super::log_metal_trace() && LOG_ONCE.set(()).is_ok() {
                eprintln!("[ggml][metal] mul_mat dispatch: mul_mv_ext");
            }

            let nsg = 2i32;
            let nxpsg = if ne00 % 256 == 0 && ne11 < 3 {
                16i32
            } else if ne00 % 128 == 0 {
                8i32
            } else {
                4i32
            };
            let nypsg = 32 / nxpsg;
            let r0ptg = nypsg * nsg;
            let r1ptg = match ne11 {
                2 => 2,
                3 | 6 => 3,
                4 | 7 | 8 => 4,
                5 => 5,
                _ => return Err(format!("unsupported ne11 for mul_mv_ext: {}", ne11)),
            };

            let base = format!(
                "kernel_mul_mv_ext_{}_{}_r1_{}",
                src0_type_name(src0),
                "f32",
                r1ptg
            );
            let name = format!("{}_nsg={}_nxpsg={}_ne12=1_r2=1_r3=1", base, nsg, nxpsg);

            let constants = [
                FunctionConstant {
                    idx: FC_MUL_MV + 0,
                    value: FunctionConstantValue::Int16(nsg as i16),
                },
                FunctionConstant {
                    idx: FC_MUL_MV + 1,
                    value: FunctionConstantValue::Int16(nxpsg as i16),
                },
                FunctionConstant {
                    idx: FC_MUL_MV + 2,
                    value: FunctionConstantValue::Int16(1),
                },
                FunctionConstant {
                    idx: FC_MUL_MV + 3,
                    value: FunctionConstantValue::Int16(1),
                },
                FunctionConstant {
                    idx: FC_MUL_MV + 4,
                    value: FunctionConstantValue::Int16(1),
                },
            ];

            let (pipeline, _smem, _nr0, _nr1, _pnsg) =
                self.get_or_compile_cached_pipeline(name, &base, &constants, 0, 0, 0, nsg)?;

            let args = KArgsMulMvExt {
                ne00,
                ne01,
                ne02: 1,
                nb00,
                nb01,
                nb02: nb01 * ne01 as u64,
                nb03: nb01 * ne01 as u64,
                ne10,
                ne11,
                ne12: 1,
                nb10,
                nb11,
                nb12: nb11 * ne11 as u64,
                nb13: nb11 * ne11 as u64,
                ne0,
                ne1,
                r2: 1,
                r3: 1,
            };

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsMulMvExt as *const c_void
                    length: std::mem::size_of::<KArgsMulMvExt>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: src1_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 3u64];

                let tgs = MTLSize {
                    width: ((ne01 + r0ptg - 1) / r0ptg) as u64,
                    height: ((ne11 + r1ptg - 1) / r1ptg) as u64,
                    depth: 1,
                };
                let tpg = MTLSize {
                    width: 32,
                    height: nsg as u64,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_mul_mm(
            &mut self,
            src0: Src0Type,
            src0_id: ObjcId,
            src1_id: ObjcId,
            dst_id: ObjcId,
            ne00: i32,
            ne01: i32,
            nb01: u64,
            ne12: i32,
            nb10: u64,
            nb11: u64,
            ne0: i32,
            ne1: i32,
        ) -> Result<(), String> {
            static LOG_ONCE: OnceLock<()> = OnceLock::new();
            if super::log_metal_trace() && LOG_ONCE.set(()).is_ok() {
                eprintln!("[ggml][metal] mul_mat dispatch: mul_mm");
            }

            let bc_inp = ne00 % 32 != 0;
            let bc_out = ne0 % 64 != 0 || ne1 % 32 != 0;

            let base = format!("kernel_mul_mm_{}_{}", src0_type_name(src0), "f32");
            let name = format!(
                "{}_bci={}_bco={}_ne12={}_ne13=1_r2=1_r3=1",
                base, bc_inp as i32, bc_out as i32, ne12
            );

            let smem = if bc_out {
                8192usize
            } else {
                4096usize + 2048usize
            };
            let constants = [
                FunctionConstant {
                    idx: FC_MUL_MM + 0,
                    value: FunctionConstantValue::Bool(bc_inp),
                },
                FunctionConstant {
                    idx: FC_MUL_MM + 1,
                    value: FunctionConstantValue::Bool(bc_out),
                },
                FunctionConstant {
                    idx: FC_MUL_MM + 2,
                    value: FunctionConstantValue::Int16(ne12 as i16),
                },
                FunctionConstant {
                    idx: FC_MUL_MM + 3,
                    value: FunctionConstantValue::Int16(1),
                },
                FunctionConstant {
                    idx: FC_MUL_MM + 4,
                    value: FunctionConstantValue::Int16(1),
                },
                FunctionConstant {
                    idx: FC_MUL_MM + 5,
                    value: FunctionConstantValue::Int16(1),
                },
            ];

            let (pipeline, pipeline_smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(name, &base, &constants, smem, 0, 0, 0)?;

            let args = KArgsMulMm {
                ne00,
                ne02: 1,
                nb01,
                nb02: nb01 * ne01 as u64,
                nb03: nb01 * ne01 as u64,
                ne12,
                nb10,
                nb11,
                nb12: nb11 * ne1 as u64,
                nb13: nb11 * ne1 as u64,
                ne0,
                ne1,
                r2: 1,
                r3: 1,
            };

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsMulMm as *const c_void
                    length: std::mem::size_of::<KArgsMulMm>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: src1_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 3u64];
                let _: () = msg_send![
                    encoder,
                    setThreadgroupMemoryLength: pipeline_smem as u64
                    atIndex: 0u64
                ];

                let tgs = MTLSize {
                    width: ((ne1 + 31) / 32) as u64,
                    height: ((ne01 + 63) / 64) as u64,
                    depth: ne12 as u64,
                };
                let tpg = MTLSize {
                    width: 128,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_mul_mv(
            &mut self,
            src0: Src0Type,
            src0_id: ObjcId,
            src1_id: ObjcId,
            dst_id: ObjcId,
            ne00: i32,
            ne01: i32,
            ne10: i32,
            ne11: i32,
            nb00: u64,
            nb01: u64,
            nb10: u64,
            nb11: u64,
            ne0: i32,
            ne1: i32,
        ) -> Result<(), String> {
            static LOG_ONCE: OnceLock<()> = OnceLock::new();
            if super::log_metal_trace() && LOG_ONCE.set(()).is_ok() {
                eprintln!("[ggml][metal] mul_mat dispatch: mul_mv");
            }

            let (nsg, nr0, nr1, smem, suffix) = match src0 {
                Src0Type::F32 | Src0Type::F16 | Src0Type::BF16 => {
                    if ne00 < 32 {
                        (1, 32, 1, 0usize, "_short")
                    } else {
                        let nsg = ((ne00 + 127) / 128).min(4);
                        let nr0 = 2;
                        let smem = 32usize * std::mem::size_of::<f32>() * nr0 as usize;
                        let suffix = if ne00 % 4 == 0 { "_4" } else { "" };
                        (nsg, nr0, 1, smem, suffix)
                    }
                }
                Src0Type::Q4_0 => (N_SG_Q4_0, N_R0_Q4_0, 1, 0usize, ""),
                Src0Type::Q4_1 => (N_SG_Q4_1, N_R0_Q4_1, 1, 0usize, ""),
                Src0Type::Q5_0 => (N_SG_Q5_0, N_R0_Q5_0, 1, 0usize, ""),
                Src0Type::Q5_1 => (N_SG_Q5_1, N_R0_Q5_1, 1, 0usize, ""),
                Src0Type::Q8_0 => (
                    N_SG_Q8_0,
                    N_R0_Q8_0,
                    1,
                    32usize * std::mem::size_of::<f32>() * N_R0_Q8_0 as usize,
                    "",
                ),
                Src0Type::Q2_K => (N_SG_Q2_K, N_R0_Q2_K, 1, 0usize, ""),
                Src0Type::Q3_K => (N_SG_Q3_K, N_R0_Q3_K, 1, 0usize, ""),
                Src0Type::Q4_K => (N_SG_Q4_K, N_R0_Q4_K, 1, 0usize, ""),
                Src0Type::Q5_K => (N_SG_Q5_K, N_R0_Q5_K, 1, 0usize, ""),
                Src0Type::Q6_K => (N_SG_Q6_K, N_R0_Q6_K, 1, 0usize, ""),
            };

            let base = format!("kernel_mul_mv_{}_{}{}", src0_type_name(src0), "f32", suffix);
            let name = format!("{}_nsg={}_ne12=1_r2=1_r3=1", base, nsg);
            let constants = [
                FunctionConstant {
                    idx: FC_MUL_MV + 0,
                    value: FunctionConstantValue::Int16(nsg as i16),
                },
                FunctionConstant {
                    idx: FC_MUL_MV + 2,
                    value: FunctionConstantValue::Int16(1),
                },
                FunctionConstant {
                    idx: FC_MUL_MV + 3,
                    value: FunctionConstantValue::Int16(1),
                },
                FunctionConstant {
                    idx: FC_MUL_MV + 4,
                    value: FunctionConstantValue::Int16(1),
                },
            ];

            let (pipeline, _pipeline_smem, pn0, pn1, pnsg) =
                self.get_or_compile_cached_pipeline(name, &base, &constants, smem, nr0, nr1, nsg)?;

            let args = KArgsMulMv {
                ne00,
                ne01,
                ne02: 1,
                nb00,
                nb01,
                nb02: nb01 * ne01 as u64,
                nb03: nb01 * ne01 as u64,
                ne10,
                ne11,
                ne12: 1,
                nb10,
                nb11,
                nb12: nb11 * ne11 as u64,
                nb13: nb11 * ne11 as u64,
                ne0,
                ne1,
                nr0: pn0,
                r2: 1,
                r3: 1,
            };

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsMulMv as *const c_void
                    length: std::mem::size_of::<KArgsMulMv>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: src1_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 3u64];

                if smem > 0 {
                    let _: () = msg_send![
                        encoder,
                        setThreadgroupMemoryLength: smem as u64
                        atIndex: 0u64
                    ];
                }

                let tg_x = if matches!(
                    src0,
                    Src0Type::F32 | Src0Type::F16 | Src0Type::BF16 | Src0Type::Q8_0
                ) {
                    (ne01 + pn0 - 1) / pn0
                } else {
                    (ne01 + pn0 * pnsg - 1) / (pn0 * pnsg)
                };
                let tg_y = (ne11 + pn1 - 1) / pn1;

                let tgs = MTLSize {
                    width: tg_x as u64,
                    height: tg_y as u64,
                    depth: 1,
                };
                let tpg = MTLSize {
                    width: 32,
                    height: pnsg as u64,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_unary_f32(
            &mut self,
            op_num: i16,
            src0_id: ObjcId,
            dst_id: ObjcId,
            shape: &Shape4,
        ) -> Result<(), String> {
            let is_c4 = shape.ne[0] % 4 == 0;
            let is_cnt = shape.numel < 32768;

            let base = if is_c4 {
                "kernel_unary_f32_f32_4"
            } else {
                "kernel_unary_f32_f32"
            };
            let name = format!("{}_op={}_cnt={}", base, op_num, is_cnt as i32);

            let constants = [
                FunctionConstant {
                    idx: FC_UNARY + 0,
                    value: FunctionConstantValue::Int16(op_num),
                },
                FunctionConstant {
                    idx: FC_UNARY + 1,
                    value: FunctionConstantValue::Bool(is_cnt),
                },
            ];

            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(name, base, &constants, 0, 0, 0, 0)?;

            let mut args = KArgsUnary {
                ne00: shape.ne[0],
                ne01: shape.ne[1],
                ne02: shape.ne[2],
                ne03: shape.ne[3],
                nb00: shape.nb[0],
                nb01: shape.nb[1],
                nb02: shape.nb[2],
                nb03: shape.nb[3],
                ne0: shape.ne[0],
                ne1: shape.ne[1],
                ne2: shape.ne[2],
                ne3: shape.ne[3],
                nb0: shape.nb[0],
                nb1: shape.nb[1],
                nb2: shape.nb[2],
                nb3: shape.nb[3],
                slope: 0.0,
                scale: 0.0,
                bias: 0.0,
                val: 0.0,
                min: 0.0,
                max: 0.0,
            };

            if is_c4 {
                args.ne00 /= 4;
                args.ne0 /= 4;
            }

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsUnary as *const c_void
                    length: std::mem::size_of::<KArgsUnary>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 2u64];

                if is_cnt {
                    let n = if is_c4 { shape.numel / 4 } else { shape.numel };
                    let tgs = MTLSize {
                        width: n as u64,
                        height: 1,
                        depth: 1,
                    };
                    let tpg = MTLSize {
                        width: 1,
                        height: 1,
                        depth: 1,
                    };
                    let _: () = msg_send![
                        encoder,
                        dispatchThreadgroups: tgs
                        threadsPerThreadgroup: tpg
                    ];
                } else {
                    let nth_max =
                        std::cmp::min(256u64, Self::pipeline_max_threads(pipeline)).max(1u64);
                    let nth = std::cmp::min(args.ne00 as u64, nth_max).max(1u64);
                    let nk0 = ((args.ne00 as u64) + nth - 1) / nth;

                    let tgs = MTLSize {
                        width: nk0.saturating_mul(shape.ne[1] as u64),
                        height: shape.ne[2] as u64,
                        depth: shape.ne[3] as u64,
                    };
                    let tpg = MTLSize {
                        width: nth,
                        height: 1,
                        depth: 1,
                    };
                    let _: () = msg_send![
                        encoder,
                        dispatchThreadgroups: tgs
                        threadsPerThreadgroup: tpg
                    ];
                }
            }

            self.end_command_encoder(encoder_handles)
        }

        /// Rotate-half rope from per-token cos/sin tables on a row-major
        /// `[token_count, head_count * head_dim]` f32 buffer; `dst_id` may be
        /// `x_id` (each thread owns both halves of its pair).
        #[allow(clippy::too_many_arguments)]
        fn dispatch_rope_half_tables_f32(
            &mut self,
            x_id: ObjcId,
            cos_id: ObjcId,
            sin_id: ObjcId,
            dst_id: ObjcId,
            token_count: usize,
            head_count: usize,
            head_dim: usize,
            rot_half: usize,
        ) -> Result<(), String> {
            if token_count == 0 || head_count == 0 {
                return Ok(());
            }
            if rot_half * 2 > head_dim {
                return Err(format!(
                    "rope_half_tables: rot_half {} exceeds half of head_dim {}",
                    rot_half, head_dim
                ));
            }
            let name = "kernel_makepad_rope_half_tables_f32";
            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(name.to_string(), name, &[], 0, 0, 0, 0)?;
            let args = KArgsRopeHalfTables {
                token_count: i32::try_from(token_count)
                    .map_err(|_| format!("rope token_count too large: {}", token_count))?,
                head_count: i32::try_from(head_count)
                    .map_err(|_| format!("rope head_count too large: {}", head_count))?,
                head_dim: i32::try_from(head_dim)
                    .map_err(|_| format!("rope head_dim too large: {}", head_dim))?,
                rot_half: i32::try_from(rot_half)
                    .map_err(|_| format!("rope rot_half too large: {}", rot_half))?,
            };
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsRopeHalfTables as *const c_void
                    length: std::mem::size_of::<KArgsRopeHalfTables>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: x_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: cos_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: sin_id offset: 0u64 atIndex: 3u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 4u64];
                let nth_max = Self::pipeline_max_threads(pipeline).max(1u64);
                let nth = (rot_half.max(1) as u64).min(nth_max).min(256);
                let tgs = MTLSize {
                    width: token_count as u64,
                    height: head_count as u64,
                    depth: 1,
                };
                let tpg = MTLSize {
                    width: nth,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }
            self.end_command_encoder(encoder_handles)
        }

        fn vit_linear_from_buffer(
            &mut self,
            src_id: ObjcId,
            m: usize,
            k: usize,
            lin: &super::VitLinearRef<'_>,
            weight_tag: u8,
            bias_tag: u8,
        ) -> Result<StrongId, String> {
            let bias = if lin.bias.is_empty() {
                None
            } else {
                Some(lin.bias)
            };
            self.linear_from_src_buffer(
                src_id,
                m,
                k,
                lin.w_bytes,
                lin.w_ggml_type,
                lin.n,
                bias,
                weight_tag,
                bias_tag,
            )
        }

        /// One resident ViT layer (see `VitLayerRef`), updating `x_id` in
        /// place. The seven GEMM outputs use `tag_base + {2,4,6,8,12,14,16}`
        /// as their persistent scratch tags; a stack reuses one `tag_base`
        /// because layers execute sequentially inside the batch.
        #[allow(clippy::too_many_arguments)]
        fn vit_layer_from_buffer_f32(
            &mut self,
            x_id: ObjcId,
            seq_len: usize,
            n_state: usize,
            n_head: usize,
            rot_half: usize,
            cos_id: ObjcId,
            sin_id: ObjcId,
            layer: &super::VitLayerRef<'_>,
            eps: f32,
            tag_base: u8,
        ) -> Result<(), String> {
            let head_dim = n_state / n_head;
            let x_shape = shape4_from_row_major(&[seq_len, n_state], 4)?;
            let ln_shape = shape4_from_row_major(&[n_state], 4)?;
            let norm_bytes = x_shape
                .numel
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing vit norm buffer bytes".to_string())?;

            // Attention: n1 = LN(x); x += out(attn(rope(q), rope(k), v)).
            let n1_w = self.get_or_create_cached_f32_buffer(layer.norm1_w, tag_base)?;
            let n1_b = self.get_or_create_cached_f32_buffer(layer.norm1_b, tag_base + 1)?;
            let norm0_id = self.get_or_create_scratch_buffer(SCRATCH_ENC_NORM0, norm_bytes)?;
            self.dispatch_norm_f32(
                x_id, n1_w, n1_b, norm0_id, &x_shape, &ln_shape, &ln_shape, eps, 3,
            )?;
            let q = self.vit_linear_from_buffer(
                norm0_id, seq_len, n_state, &layer.q, tag_base + 2, tag_base + 3,
            )?;
            let k = self.vit_linear_from_buffer(
                norm0_id, seq_len, n_state, &layer.k, tag_base + 4, tag_base + 5,
            )?;
            let v = self.vit_linear_from_buffer(
                norm0_id, seq_len, n_state, &layer.v, tag_base + 6, tag_base + 7,
            )?;
            if rot_half > 0 {
                self.dispatch_rope_half_tables_f32(
                    q.as_id(), cos_id, sin_id, q.as_id(), seq_len, n_head, head_dim, rot_half,
                )?;
                self.dispatch_rope_half_tables_f32(
                    k.as_id(), cos_id, sin_id, k.as_id(), seq_len, n_head, head_dim, rot_half,
                )?;
            }
            let scale = 1.0 / (head_dim as f32).sqrt();
            let attn = self.flash_attn_f32_from_buffers(
                q.as_id(), k.as_id(), v.as_id(), seq_len, seq_len, n_head, head_dim, scale,
            )?;
            let out = self.vit_linear_from_buffer(
                attn.as_id(), seq_len, n_state, &layer.out, tag_base + 8, tag_base + 9,
            )?;
            self.dispatch_bin_f32(0, x_id, out.as_id(), x_id, &x_shape, &x_shape)?;

            // Feed-forward: n2 = LN(x); x += down(silu(gate(n2)) * up(n2)).
            let n2_w = self.get_or_create_cached_f32_buffer(layer.norm2_w, tag_base + 10)?;
            let n2_b = self.get_or_create_cached_f32_buffer(layer.norm2_b, tag_base + 11)?;
            let norm1_id = self.get_or_create_scratch_buffer(SCRATCH_ENC_NORM1, norm_bytes)?;
            self.dispatch_norm_f32(
                x_id, n2_w, n2_b, norm1_id, &x_shape, &ln_shape, &ln_shape, eps, 3,
            )?;
            let gate = self.vit_linear_from_buffer(
                norm1_id, seq_len, n_state, &layer.gate, tag_base + 12, tag_base + 13,
            )?;
            let up = self.vit_linear_from_buffer(
                norm1_id, seq_len, n_state, &layer.up, tag_base + 14, tag_base + 15,
            )?;
            let ff_shape = shape4_from_row_major(&[seq_len, layer.gate.n], 4)?;
            self.dispatch_unary_f32(OP_UNARY_NUM_SILU, gate.as_id(), gate.as_id(), &ff_shape)?;
            self.dispatch_bin_f32(2, gate.as_id(), up.as_id(), gate.as_id(), &ff_shape, &ff_shape)?;
            let down = self.vit_linear_from_buffer(
                gate.as_id(), seq_len, layer.gate.n, &layer.down, tag_base + 16, tag_base + 17,
            )?;
            self.dispatch_bin_f32(0, x_id, down.as_id(), x_id, &x_shape, &x_shape)?;
            Ok(())
        }

        #[allow(clippy::too_many_arguments)]
        fn vit_backbone_resident_f32(
            &mut self,
            x: &[f32],
            seq_len: usize,
            n_state: usize,
            n_head: usize,
            rot_half: usize,
            cos: &[f32],
            sin: &[f32],
            layers: &[super::VitLayerRef<'_>],
            final_norm_w: &[f32],
            final_norm_b: &[f32],
            eps: f32,
        ) -> Result<Vec<f32>, String> {
            if seq_len == 0 || n_state == 0 || n_head == 0 || n_state % n_head != 0 {
                return Err(format!(
                    "invalid vit dimensions: seq_len={}, n_state={}, n_head={}",
                    seq_len, n_state, n_head
                ));
            }
            let head_dim = n_state / n_head;
            if rot_half * 2 > head_dim {
                return Err(format!(
                    "vit rot_half {} exceeds half of head_dim {}",
                    rot_half, head_dim
                ));
            }
            let x_need = seq_len
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing vit x size".to_string())?;
            if x.len() != x_need {
                return Err(format!("vit x len mismatch: got {}, expected {}", x.len(), x_need));
            }
            let table_need = seq_len * rot_half;
            if cos.len() != table_need || sin.len() != table_need {
                return Err(format!(
                    "vit rope table len mismatch: cos {} sin {} expected {}",
                    cos.len(),
                    sin.len(),
                    table_need
                ));
            }
            if final_norm_w.len() != n_state || final_norm_b.len() != n_state {
                return Err("vit final layernorm affine size mismatch".to_string());
            }
            for (index, layer) in layers.iter().enumerate() {
                let lin_ok = |lin: &super::VitLinearRef<'_>, n: usize| {
                    lin.n == n && (lin.bias.is_empty() || lin.bias.len() == n)
                };
                if layer.norm1_w.len() != n_state
                    || layer.norm1_b.len() != n_state
                    || layer.norm2_w.len() != n_state
                    || layer.norm2_b.len() != n_state
                    || !lin_ok(&layer.q, n_state)
                    || !lin_ok(&layer.k, n_state)
                    || !lin_ok(&layer.v, n_state)
                    || !lin_ok(&layer.out, n_state)
                    || layer.gate.n == 0
                    || !lin_ok(&layer.gate, layer.gate.n)
                    || !lin_ok(&layer.up, layer.gate.n)
                    || !lin_ok(&layer.down, n_state)
                {
                    return Err(format!("vit layer {} has mismatched shapes", index));
                }
            }

            let x_shape = shape4_from_row_major(&[seq_len, n_state], 4)?;
            let ln_shape = shape4_from_row_major(&[n_state], 4)?;
            let x_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(x))?;
            let (cos_buf, sin_buf) = if rot_half > 0 {
                (
                    Some(self.new_buffer_with_bytes(f32_slice_as_bytes(cos))?),
                    Some(self.new_buffer_with_bytes(f32_slice_as_bytes(sin))?),
                )
            } else {
                (None, None)
            };
            let out_buf = self.new_buffer_with_length(x_need * std::mem::size_of::<f32>())?;
            let cos_id = cos_buf.as_ref().map(|b| b.as_id()).unwrap_or(x_buf.as_id());
            let sin_id = sin_buf.as_ref().map(|b| b.as_id()).unwrap_or(x_buf.as_id());
            let x_id = x_buf.as_id();
            let out_id = out_buf.as_id();
            self.with_batch(|ctx| {
                for layer in layers {
                    ctx.vit_layer_from_buffer_f32(
                        x_id, seq_len, n_state, n_head, rot_half, cos_id, sin_id, layer, eps,
                        VIT_TAG_BASE,
                    )?;
                }
                let fw = ctx.get_or_create_cached_f32_buffer(final_norm_w, VIT_TAG_BASE + 18)?;
                let fb = ctx.get_or_create_cached_f32_buffer(final_norm_b, VIT_TAG_BASE + 19)?;
                ctx.dispatch_norm_f32(
                    x_id, fw, fb, out_id, &x_shape, &ln_shape, &ln_shape, eps, 3,
                )
            })?;
            let out = self.read_f32_buffer(out_id, x_need)?;
            drop(cos_buf);
            drop(sin_buf);
            drop(x_buf);
            drop(out_buf);
            Ok(out)
        }

        /// `C(m, n) = X(m, k) @ W^T` from device buffers: `src0_id` holds the
        /// `[n, k]` weight in `src0_ggml_type`, `src1_id` the f32 `[m, k]`
        /// input, and the f32 `[m, n]` result lands in `dst_id`.
        #[allow(clippy::too_many_arguments)]
        fn matmul_nt_into_buffer(
            &mut self,
            src0_ggml_type: u32,
            src0_id: ObjcId,
            src1_id: ObjcId,
            dst_id: ObjcId,
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<(), String> {
            let src0 = src0_type_from_ggml(src0_ggml_type).ok_or_else(|| {
                format!("unsupported src0 ggml_type for metal matmul: {}", src0_ggml_type)
            })?;
            let (src0_row_bytes, nb00) = src0_layout_bytes_per_row(src0, k)?;
            let ne00 = i32::try_from(k).map_err(|_| format!("k too large: {}", k))?;
            let ne01 = i32::try_from(n).map_err(|_| format!("n too large: {}", n))?;
            let ne10 = ne00;
            let ne11 = i32::try_from(m).map_err(|_| format!("m too large: {}", m))?;
            let ne0 = ne01;
            let ne1 = ne11;
            let nb01 = src0_row_bytes as u64;
            let nb10 = 4u64;
            let nb11 = (k as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing nb11".to_string())?;
            if can_use_mul_mv_ext(src0, ne00, ne11) {
                self.dispatch_mul_mv_ext(
                    src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01, nb10,
                    nb11, ne0, ne1,
                )
            } else if ne00 >= 64 && ne11 > 8 {
                match self.dispatch_mul_mm(
                    src0, src0_id, src1_id, dst_id, ne00, ne01, nb01, 1, nb10, nb11, ne0, ne1,
                ) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        super::log_metal_error_once(format!(
                            "[ggml][metal] mul_mm failed for type {:?}, falling back to mul_mv: {}",
                            src0, e
                        ));
                        self.dispatch_mul_mv(
                            src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01,
                            nb10, nb11, ne0, ne1,
                        )
                    }
                }
            } else {
                self.dispatch_mul_mv(
                    src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01, nb10,
                    nb11, ne0, ne1,
                )
            }
        }

        /// The f32 contents of a host-backed tensor as little-endian bytes.
        fn tensor_f32_bytes(t: &crate::gpu_types::GpuTensor) -> Result<Vec<u8>, String> {
            let data = t
                .data
                .try_borrow()
                .map_err(|_| "metal GpuTensor already borrowed".to_string())?;
            let mut bytes = Vec::with_capacity(data.len() * 4);
            for value in data.iter() {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            Ok(bytes)
        }

        /// A pooled f32 `[rows, cols]` buffer registered in `keep` (given back
        /// to the pool after the layer's read-back).
        fn two_way_scratch(
            &mut self,
            rows: usize,
            cols: usize,
            keep: &mut Vec<StrongId>,
        ) -> Result<ObjcId, String> {
            let bytes = rows
                .checked_mul(cols)
                .and_then(|v| v.checked_mul(4))
                .ok_or_else(|| "overflow computing two-way scratch bytes".to_string())?;
            let buf = self.pool_take(bytes)?;
            let id = buf.as_id();
            keep.push(buf);
            Ok(id)
        }

        fn two_way_linear(
            &mut self,
            src_id: ObjcId,
            m: usize,
            lin: &super::DecLinearRef<'_>,
            keep: &mut Vec<StrongId>,
        ) -> Result<ObjcId, String> {
            let n = lin.weight.rows;
            let k = lin.weight.cols;
            let weight = lin.weight;
            let w_id = self.get_or_create_named_weight_buffer(
                "two_way",
                &format!("w{}", weight.id.get()),
                || Self::tensor_f32_bytes(weight),
            )?;
            let dst_id = self.two_way_scratch(m, n, keep)?;
            self.matmul_nt_into_buffer(GGML_TYPE_F32, w_id, src_id, dst_id, m, k, n)?;
            if let Some(bias) = lin.bias {
                if bias.rows * bias.cols != n {
                    return Err(format!(
                        "two-way linear bias {}x{} does not match n {}",
                        bias.rows, bias.cols, n
                    ));
                }
                let b_id = self.get_or_create_named_weight_buffer(
                    "two_way",
                    &format!("b{}", bias.id.get()),
                    || Self::tensor_f32_bytes(bias),
                )?;
                let dst_shape = shape4_from_row_major(&[m, n], 4)?;
                let b_shape = shape4_from_row_major(&[n], 4)?;
                self.dispatch_bin_f32(0, dst_id, b_id, dst_id, &dst_shape, &b_shape)?;
            }
            Ok(dst_id)
        }

        #[allow(clippy::too_many_arguments)]
        fn two_way_norm(
            &mut self,
            src_id: ObjcId,
            rows: usize,
            cols: usize,
            affine: (&[f32], &[f32]),
            tag: u8,
            eps: f32,
            keep: &mut Vec<StrongId>,
        ) -> Result<ObjcId, String> {
            if affine.0.len() != cols || affine.1.len() != cols {
                return Err(format!(
                    "two-way layernorm affine {}/{} does not match {} cols",
                    affine.0.len(),
                    affine.1.len(),
                    cols
                ));
            }
            let w_id = self.get_or_create_cached_f32_buffer(affine.0, tag)?;
            let b_id = self.get_or_create_cached_f32_buffer(affine.1, tag + 1)?;
            let dst_id = self.two_way_scratch(rows, cols, keep)?;
            let x_shape = shape4_from_row_major(&[rows, cols], 4)?;
            let ln_shape = shape4_from_row_major(&[cols], 4)?;
            self.dispatch_norm_f32(
                src_id, w_id, b_id, dst_id, &x_shape, &ln_shape, &ln_shape, eps, 3,
            )?;
            Ok(dst_id)
        }

        fn two_way_add(
            &mut self,
            a_id: ObjcId,
            b_id: ObjcId,
            dst_id: ObjcId,
            rows: usize,
            cols: usize,
        ) -> Result<(), String> {
            let shape = shape4_from_row_major(&[rows, cols], 4)?;
            self.dispatch_bin_f32(0, a_id, b_id, dst_id, &shape, &shape)
        }

        #[allow(clippy::too_many_arguments)]
        fn two_way_layer_resident_f32(
            &mut self,
            hidden: &[f32],
            token_pe: &[f32],
            context: &[f32],
            context_pe: &[f32],
            n_tok: usize,
            dim: usize,
            n_ctx: usize,
            ctx_dim: usize,
            layer: &super::TwoWayLayerRef<'_>,
        ) -> Result<(Vec<f32>, Vec<f32>), String> {
            if n_tok == 0 || dim == 0 || n_ctx == 0 || ctx_dim == 0 || layer.n_head == 0 {
                return Err("two-way layer: empty dimension".to_string());
            }
            let tok_elems = n_tok * dim;
            let ctx_elems = n_ctx * ctx_dim;
            if hidden.len() != tok_elems || token_pe.len() != tok_elems {
                return Err(format!(
                    "two-way layer: hidden {} / token_pe {} vs {}x{}",
                    hidden.len(),
                    token_pe.len(),
                    n_tok,
                    dim
                ));
            }
            if context.len() != ctx_elems || context_pe.len() != ctx_elems {
                return Err(format!(
                    "two-way layer: context {} / context_pe {} vs {}x{}",
                    context.len(),
                    context_pe.len(),
                    n_ctx,
                    ctx_dim
                ));
            }
            let sa = &layer.self_attn;
            let ca = &layer.cross_attn;
            let inner = sa.q.weight.rows;
            if inner == 0 || inner % layer.n_head != 0 {
                return Err(format!(
                    "two-way layer: attention width {} not divisible by {} heads",
                    inner, layer.n_head
                ));
            }
            let head_dim = inner / layer.n_head;
            if !flash_attn_supported_head_dim(head_dim) {
                return Err(format!("two-way layer: head dim {} unsupported", head_dim));
            }
            let shape_ok = |lin: &super::DecLinearRef<'_>, n: usize, k: usize| {
                lin.weight.rows == n && lin.weight.cols == k
            };
            if !shape_ok(&sa.q, inner, dim)
                || !shape_ok(&sa.k, inner, dim)
                || !shape_ok(&sa.v, inner, dim)
                || !shape_ok(&sa.out, dim, inner)
                || !shape_ok(&ca.q, inner, dim)
                || !shape_ok(&ca.k, inner, ctx_dim)
                || !shape_ok(&ca.v, inner, ctx_dim)
                || !shape_ok(&ca.out, dim, inner)
                || layer.ffn_first.weight.cols != dim
                || !shape_ok(&layer.ffn_second, dim, layer.ffn_first.weight.rows)
            {
                return Err("two-way layer: weight shapes do not match the layer".to_string());
            }
            let ffn_width = layer.ffn_first.weight.rows;
            let scale = 1.0 / (head_dim as f32).sqrt();
            let eps = layer.eps;
            let tag = TWO_WAY_TAG_BASE;

            let mut keep: Vec<StrongId> = Vec::new();
            let x_buf = self.pool_take_filled(f32_slice_as_bytes(hidden))?;
            let tpe_buf = self.pool_take_filled(f32_slice_as_bytes(token_pe))?;
            let ctx_buf = self.pool_take_filled(f32_slice_as_bytes(context))?;
            let cpe_buf = self.pool_take_filled(f32_slice_as_bytes(context_pe))?;
            let normed_buf = self.pool_take(tok_elems * 4)?;
            let x_id = x_buf.as_id();
            let normed_id = normed_buf.as_id();

            let run = self.with_batch(|ctx| {
                let keep = &mut keep;
                // Positional embeddings, normalised once per layer.
                let tpe_n = ctx.two_way_norm(tpe_buf.as_id(), n_tok, dim, layer.ln_pe_1, tag, eps, keep)?;
                let ipe_n =
                    ctx.two_way_norm(cpe_buf.as_id(), n_ctx, ctx_dim, layer.ln_pe_2, tag + 2, eps, keep)?;

                // Token self-attention.
                let n1 = ctx.two_way_norm(x_id, n_tok, dim, layer.ln1, tag + 4, eps, keep)?;
                let qk = if layer.pe_on_self {
                    let qk = ctx.two_way_scratch(n_tok, dim, keep)?;
                    ctx.two_way_add(n1, tpe_n, qk, n_tok, dim)?;
                    qk
                } else {
                    n1
                };
                let q = ctx.two_way_linear(qk, n_tok, &sa.q, keep)?;
                let k = ctx.two_way_linear(qk, n_tok, &sa.k, keep)?;
                let v = ctx.two_way_linear(n1, n_tok, &sa.v, keep)?;
                let attn = ctx.flash_attn_f32_from_buffers(
                    q, k, v, n_tok, n_tok, layer.n_head, head_dim, scale,
                )?;
                let o = ctx.two_way_linear(attn.as_id(), n_tok, &sa.out, keep)?;
                ctx.two_way_add(x_id, o, x_id, n_tok, dim)?;
                drop(attn);

                // Token-to-image cross-attention.
                let q2n = ctx.two_way_norm(x_id, n_tok, dim, layer.ln2_1, tag + 6, eps, keep)?;
                let q2 = ctx.two_way_scratch(n_tok, dim, keep)?;
                ctx.two_way_add(q2n, tpe_n, q2, n_tok, dim)?;
                let cn = ctx.two_way_norm(ctx_buf.as_id(), n_ctx, ctx_dim, layer.ln2_2, tag + 8, eps, keep)?;
                let kx = ctx.two_way_scratch(n_ctx, ctx_dim, keep)?;
                ctx.two_way_add(cn, ipe_n, kx, n_ctx, ctx_dim)?;
                let q = ctx.two_way_linear(q2, n_tok, &ca.q, keep)?;
                let k = ctx.two_way_linear(kx, n_ctx, &ca.k, keep)?;
                let v = ctx.two_way_linear(cn, n_ctx, &ca.v, keep)?;
                let attn = ctx.flash_attn_f32_from_buffers(
                    q, k, v, n_tok, n_ctx, layer.n_head, head_dim, scale,
                )?;
                let o = ctx.two_way_linear(attn.as_id(), n_tok, &ca.out, keep)?;
                ctx.two_way_add(x_id, o, x_id, n_tok, dim)?;
                drop(attn);

                // Feed-forward with the exact GELU.
                let n3 = ctx.two_way_norm(x_id, n_tok, dim, layer.ln3, tag + 10, eps, keep)?;
                let f = ctx.two_way_linear(n3, n_tok, &layer.ffn_first, keep)?;
                let f_shape = shape4_from_row_major(&[n_tok, ffn_width], 4)?;
                ctx.dispatch_unary_f32(OP_UNARY_NUM_GELU_ERF, f, f, &f_shape)?;
                let f2 = ctx.two_way_linear(f, n_tok, &layer.ffn_second, keep)?;
                ctx.two_way_add(x_id, f2, x_id, n_tok, dim)?;

                // Final norm, read back alongside the hidden state.
                let fw = ctx.get_or_create_cached_f32_buffer(layer.ln_final.0, tag + 12)?;
                let fb = ctx.get_or_create_cached_f32_buffer(layer.ln_final.1, tag + 13)?;
                let x_shape = shape4_from_row_major(&[n_tok, dim], 4)?;
                let ln_shape = shape4_from_row_major(&[dim], 4)?;
                ctx.dispatch_norm_f32(
                    x_id, fw, fb, normed_id, &x_shape, &ln_shape, &ln_shape, eps, 3,
                )
            });
            let result = run.and_then(|()| {
                let hidden_out = self.read_f32_buffer(x_id, tok_elems)?;
                let normed_out = self.read_f32_buffer(normed_id, tok_elems)?;
                Ok((hidden_out, normed_out))
            });
            // Everything above waited on the queue, so the transients are free.
            let _ = self.wait_queue_idle();
            for buf in keep.drain(..) {
                self.pool_give(buf);
            }
            for buf in [x_buf, tpe_buf, ctx_buf, cpe_buf, normed_buf] {
                self.pool_give(buf);
            }
            self.pool_recycle();
            result
        }

        fn dispatch_cpy_f32_to_f16(
            &mut self,
            src_id: ObjcId,
            dst_id: ObjcId,
            ne00: usize,
            ne01: usize,
            nb00: u64,
            nb01: u64,
            nb0: u64,
            nb1: u64,
        ) -> Result<(), String> {
            let base = "kernel_cpy_f32_f16";
            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(base.to_string(), base, &[], 0, 0, 0, 0)?;

            let ne00_i64 = i64::try_from(ne00).map_err(|_| format!("ne00 too large: {}", ne00))?;
            let ne01_i64 = i64::try_from(ne01).map_err(|_| format!("ne01 too large: {}", ne01))?;
            let nb02 = nb01
                .checked_mul(ne01 as u64)
                .ok_or_else(|| "overflow computing cpy nb02".to_string())?;
            let nb2 = nb1
                .checked_mul(ne01 as u64)
                .ok_or_else(|| "overflow computing cpy nb2".to_string())?;
            let args = KArgsCpy {
                nk0: ne00_i64,
                ne00: ne00_i64,
                ne01: ne01_i64,
                ne02: 1,
                ne03: 1,
                nb00,
                nb01,
                nb02,
                nb03: nb02,
                ne0: ne00_i64,
                ne1: ne01_i64,
                ne2: 1,
                ne3: 1,
                nb0,
                nb1,
                nb2,
                nb3: nb2,
            };

            let max_threads = Self::pipeline_max_threads(pipeline).max(1);
            let nth = std::cmp::min(ne00.max(1) as u64, max_threads).max(1);
            let nw0 = ((ne00.max(1) as u64) + nth - 1) / nth;

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsCpy as *const c_void
                    length: std::mem::size_of::<KArgsCpy>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: src_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 2u64];

                let tgs = MTLSize {
                    width: nw0
                        .checked_mul(ne01 as u64)
                        .ok_or_else(|| "overflow computing cpy threadgroups width".to_string())?,
                    height: 1,
                    depth: 1,
                };
                let tpg = MTLSize {
                    width: nth,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_bin_f32(
            &mut self,
            op_num: i16,
            src0_id: ObjcId,
            src1_id: ObjcId,
            dst_id: ObjcId,
            src0_shape: &Shape4,
            src1_shape: &Shape4,
        ) -> Result<(), String> {
            for d in 0..4 {
                let b = src1_shape.ne[d];
                let a = src0_shape.ne[d];
                if b != 1 && b != a {
                    return Err(format!(
                        "binary broadcast mismatch at dim {}: lhs={}, rhs={}",
                        d, a, b
                    ));
                }
            }

            let is_c4 = src0_shape.ne[0] % 4 == 0 && src1_shape.ne[0] % 4 == 0;
            let is_rb = nrows(src1_shape) == 1 && src0_shape.numel < 65536;

            let base = if is_c4 {
                "kernel_bin_fuse_f32_f32_f32_4"
            } else {
                "kernel_bin_fuse_f32_f32_f32"
            };
            let name = format!("{}_op={}_nf=1_rb={}", base, op_num, is_rb as i32);

            let constants = [
                FunctionConstant {
                    idx: FC_BIN + 0,
                    value: FunctionConstantValue::Int16(op_num),
                },
                FunctionConstant {
                    idx: FC_BIN + 1,
                    value: FunctionConstantValue::Int16(1),
                },
                FunctionConstant {
                    idx: FC_BIN + 2,
                    value: FunctionConstantValue::Bool(is_rb),
                },
            ];

            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(name, base, &constants, 0, 0, 0, 0)?;

            let mut args = KArgsBin {
                ne00: src0_shape.ne[0],
                ne01: src0_shape.ne[1],
                ne02: src0_shape.ne[2],
                ne03: src0_shape.ne[3],
                nb00: src0_shape.nb[0],
                nb01: src0_shape.nb[1],
                nb02: src0_shape.nb[2],
                nb03: src0_shape.nb[3],
                ne10: src1_shape.ne[0],
                ne11: src1_shape.ne[1],
                ne12: src1_shape.ne[2],
                ne13: src1_shape.ne[3],
                nb10: src1_shape.nb[0],
                nb11: src1_shape.nb[1],
                nb12: src1_shape.nb[2],
                nb13: src1_shape.nb[3],
                ne0: src0_shape.ne[0],
                ne1: src0_shape.ne[1],
                ne2: src0_shape.ne[2],
                ne3: src0_shape.ne[3],
                nb0: src0_shape.nb[0],
                nb1: src0_shape.nb[1],
                nb2: src0_shape.nb[2],
                nb3: src0_shape.nb[3],
                offs: 0,
                o1: [0u64; 8],
            };

            if is_c4 {
                args.ne00 /= 4;
                args.ne10 /= 4;
                args.ne0 /= 4;
            }

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsBin as *const c_void
                    length: std::mem::size_of::<KArgsBin>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: src1_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 3u64];

                if is_rb {
                    let n = if is_c4 {
                        src0_shape.numel / 4
                    } else {
                        src0_shape.numel
                    };
                    let tgs = MTLSize {
                        width: n as u64,
                        height: 1,
                        depth: 1,
                    };
                    let tpg = MTLSize {
                        width: 1,
                        height: 1,
                        depth: 1,
                    };
                    let _: () = msg_send![
                        encoder,
                        dispatchThreadgroups: tgs
                        threadsPerThreadgroup: tpg
                    ];
                } else {
                    let nth_max =
                        std::cmp::min(256u64, Self::pipeline_max_threads(pipeline)).max(1u64);
                    let mut nth = 1u64;
                    while 2 * nth < args.ne0 as u64 && nth < nth_max {
                        nth *= 2;
                    }
                    let tgs = MTLSize {
                        width: src0_shape.ne[1] as u64,
                        height: src0_shape.ne[2] as u64,
                        depth: src0_shape.ne[3] as u64,
                    };
                    let tpg = MTLSize {
                        width: nth,
                        height: 1,
                        depth: 1,
                    };
                    let _: () = msg_send![
                        encoder,
                        dispatchThreadgroups: tgs
                        threadsPerThreadgroup: tpg
                    ];
                }
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_geglu_strided_rows_f32(
            &mut self,
            src_id: ObjcId,
            dst_id: ObjcId,
            row_count: usize,
            row_width: usize,
            input_row_stride: usize,
            input_split_offset: usize,
        ) -> Result<(), String> {
            #[repr(C)]
            struct MlxGegluStridedRowsArgsCompat {
                n: u32,
                row_width: u32,
                input_row_stride: u32,
                input_split_offset: u32,
            }

            let n = row_count
                .checked_mul(row_width)
                .ok_or_else(|| "overflow computing fused vision geglu size".to_string())?;
            let base = "kernel_mlx_geglu_strided_rows_f32";
            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(base.to_string(), base, &[], 0, 0, 0, 0)?;
            let args = MlxGegluStridedRowsArgsCompat {
                n: n as u32,
                row_width: row_width as u32,
                input_row_stride: input_row_stride as u32,
                input_split_offset: input_split_offset as u32,
            };
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const MlxGegluStridedRowsArgsCompat as *const c_void
                    length: std::mem::size_of::<MlxGegluStridedRowsArgsCompat>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: src_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 2u64];
                let tgs = MTLSize {
                    width: (n as u64).div_ceil(256),
                    height: 1,
                    depth: 1,
                };
                let tpg = MTLSize {
                    width: 256,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }
            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_swiglu_packed(
            &mut self,
            src_id: ObjcId,
            dst_id: ObjcId,
            rows: usize,
            ff_dim: usize,
            swap: bool,
        ) -> Result<(), String> {
            #[repr(C)]
            struct KArgsGluCompat {
                ne00: i32,
                nb01: u64,
                ne10: i32,
                nb11: u64,
                ne0: i32,
                nb1: u64,
                i00: i32,
                i10: i32,
                alpha: f32,
                limit: f32,
            }
            let src_w = (ff_dim * 2) as i32;
            let row_bytes = (ff_dim * 2 * std::mem::size_of::<f32>()) as u64;
            let dst_bytes = (ff_dim * std::mem::size_of::<f32>()) as u64;
            let (i00, i10) = if swap {
                (0, ff_dim as i32)
            } else {
                (ff_dim as i32, 0)
            };
            let args = KArgsGluCompat {
                ne00: src_w,
                nb01: row_bytes,
                ne10: src_w,
                nb11: row_bytes,
                ne0: ff_dim as i32,
                nb1: dst_bytes,
                i00,
                i10,
                alpha: 0.0,
                limit: 0.0,
            };
            let base = "kernel_swiglu_f32";
            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(base.to_string(), base, &[], 0, 0, 0, 0)?;
            let nth = (ff_dim as u64 / 2).max(1).min(256);
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsGluCompat as *const c_void
                    length: std::mem::size_of::<KArgsGluCompat>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: src_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: src_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 3u64];
                let tgs = MTLSize {
                    width: rows.max(1) as u64,
                    height: 1,
                    depth: 1,
                };
                let tpg = MTLSize {
                    width: nth,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }
            self.end_command_encoder(encoder_handles)
        }

        #[allow(clippy::too_many_arguments)]
        fn dispatch_norm_f32(
            &mut self,
            src0_id: ObjcId,
            src1_0_id: ObjcId,
            src1_1_id: ObjcId,
            dst_id: ObjcId,
            src0_shape: &Shape4,
            src1_0_shape: &Shape4,
            src1_1_shape: &Shape4,
            eps: f32,
            n_fuse: i32,
        ) -> Result<(), String> {
            if src0_shape.ne[0] <= 0 {
                return Err("norm ne0 must be positive".to_string());
            }

            let is_c4 = src0_shape.ne[0] % 4 == 0;
            let suffix = if is_c4 { "_4" } else { "" };
            let base = match n_fuse {
                1 => format!("kernel_norm_f32{}", suffix),
                2 => format!("kernel_norm_mul_f32{}", suffix),
                3 => format!("kernel_norm_mul_add_f32{}", suffix),
                _ => return Err(format!("unsupported norm fuse level: {}", n_fuse)),
            };

            let (pipeline, pipeline_smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(base.clone(), &base, &[], 32 * 4, 0, 0, 0)?;

            let ne00_t = if is_c4 {
                src0_shape.ne[0] / 4
            } else {
                src0_shape.ne[0]
            };
            let args = KArgsNorm {
                ne00: src0_shape.ne[0],
                ne00_t,
                nb1: src0_shape.nb[1],
                nb2: src0_shape.nb[2],
                nb3: src0_shape.nb[3],
                eps,
                nef1: [src0_shape.ne[1], src1_0_shape.ne[1], src1_1_shape.ne[1]],
                nef2: [src0_shape.ne[2], src1_0_shape.ne[2], src1_1_shape.ne[2]],
                nef3: [src0_shape.ne[3], src1_0_shape.ne[3], src1_1_shape.ne[3]],
                nbf1: [src0_shape.nb[1], src1_0_shape.nb[1], src1_1_shape.nb[1]],
                nbf2: [src0_shape.nb[2], src1_0_shape.nb[2], src1_1_shape.nb[2]],
                nbf3: [src0_shape.nb[3], src1_0_shape.nb[3], src1_1_shape.nb[3]],
            };

            let mut nth = 32u64;
            let nth_max = Self::pipeline_max_threads(pipeline).max(1u64);
            while nth < args.ne00_t as u64 && nth < nth_max {
                nth *= 2;
            }
            nth = std::cmp::min(nth, nth_max);
            nth = std::cmp::min(nth, args.ne00_t.max(1) as u64);

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsNorm as *const c_void
                    length: std::mem::size_of::<KArgsNorm>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: src1_0_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: src1_1_id offset: 0u64 atIndex: 3u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 4u64];
                let _: () = msg_send![
                    encoder,
                    setThreadgroupMemoryLength: pipeline_smem as u64
                    atIndex: 0u64
                ];

                let tgs = MTLSize {
                    width: src0_shape.ne[1] as u64,
                    height: src0_shape.ne[2] as u64,
                    depth: src0_shape.ne[3] as u64,
                };
                let tpg = MTLSize {
                    width: nth,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        #[allow(clippy::too_many_arguments)]
        fn dispatch_rms_norm_f32(
            &mut self,
            src0_id: ObjcId,
            src1_0_id: ObjcId,
            src1_1_id: ObjcId,
            dst_id: ObjcId,
            src0_shape: &Shape4,
            src1_0_shape: &Shape4,
            src1_1_shape: &Shape4,
            eps: f32,
            n_fuse: i32,
        ) -> Result<(), String> {
            if src0_shape.ne[0] <= 0 {
                return Err("rms_norm ne0 must be positive".to_string());
            }

            let is_c4 = src0_shape.ne[0] % 4 == 0;
            let suffix = if is_c4 { "_4" } else { "" };
            let base = match n_fuse {
                1 => format!("kernel_rms_norm_f32{}", suffix),
                2 => format!("kernel_rms_norm_mul_f32{}", suffix),
                3 => format!("kernel_rms_norm_mul_add_f32{}", suffix),
                _ => return Err(format!("unsupported rms_norm fuse level: {}", n_fuse)),
            };

            let (pipeline, pipeline_smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(base.clone(), &base, &[], 32 * 4, 0, 0, 0)?;

            let ne00_t = if is_c4 {
                src0_shape.ne[0] / 4
            } else {
                src0_shape.ne[0]
            };
            let args = KArgsNorm {
                ne00: src0_shape.ne[0],
                ne00_t,
                nb1: src0_shape.nb[1],
                nb2: src0_shape.nb[2],
                nb3: src0_shape.nb[3],
                eps,
                nef1: [src0_shape.ne[1], src1_0_shape.ne[1], src1_1_shape.ne[1]],
                nef2: [src0_shape.ne[2], src1_0_shape.ne[2], src1_1_shape.ne[2]],
                nef3: [src0_shape.ne[3], src1_0_shape.ne[3], src1_1_shape.ne[3]],
                nbf1: [src0_shape.nb[1], src1_0_shape.nb[1], src1_1_shape.nb[1]],
                nbf2: [src0_shape.nb[2], src1_0_shape.nb[2], src1_1_shape.nb[2]],
                nbf3: [src0_shape.nb[3], src1_0_shape.nb[3], src1_1_shape.nb[3]],
            };

            let mut nth = 32u64;
            let nth_max = Self::pipeline_max_threads(pipeline).max(1u64);
            while nth < args.ne00_t as u64 && nth < nth_max {
                nth *= 2;
            }
            nth = std::cmp::min(nth, nth_max);
            nth = std::cmp::min(nth, args.ne00_t.max(1) as u64);

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsNorm as *const c_void
                    length: std::mem::size_of::<KArgsNorm>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: src1_0_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: src1_1_id offset: 0u64 atIndex: 3u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 4u64];
                let _: () = msg_send![
                    encoder,
                    setThreadgroupMemoryLength: pipeline_smem as u64
                    atIndex: 0u64
                ];

                let tgs = MTLSize {
                    width: src0_shape.ne[1] as u64,
                    height: src0_shape.ne[2] as u64,
                    depth: src0_shape.ne[3] as u64,
                };
                let tpg = MTLSize {
                    width: nth,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_get_rows_ggml(
            &mut self,
            src0_ggml_type: u32,
            src0_id: ObjcId,
            src0_shape: &Shape4,
            src1_id: ObjcId,
            src1_shape: &Shape4,
            dst_id: ObjcId,
            dst_shape: &Shape4,
        ) -> Result<(), String> {
            let base = format!("kernel_get_rows_{}", ggml_type_name(src0_ggml_type));
            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(base.clone(), &base, &[], 0, 0, 0, 0)?;

            let is_quantized = !matches!(
                src0_ggml_type,
                GGML_TYPE_F32 | GGML_TYPE_F16 | GGML_TYPE_BF16 | GGML_TYPE_I32
            );
            let ne00t = if is_quantized {
                src0_shape.ne[0] / 16
            } else {
                src0_shape.ne[0]
            };
            let args = KArgsGetRows {
                ne00t,
                ne00: src0_shape.ne[0],
                nb01: src0_shape.nb[1],
                nb02: src0_shape.nb[2],
                nb03: src0_shape.nb[3],
                ne10: src1_shape.ne[0],
                nb10: src1_shape.nb[0],
                nb11: src1_shape.nb[1],
                nb12: src1_shape.nb[2],
                nb1: dst_shape.nb[1],
                nb2: dst_shape.nb[2],
                nb3: dst_shape.nb[3],
            };

            let nth = std::cmp::min(
                args.ne00t.max(1) as u64,
                Self::pipeline_max_threads(pipeline).max(1),
            );
            let nw0 = ((args.ne00t.max(1) as u64) + nth - 1) / nth;

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsGetRows as *const c_void
                    length: std::mem::size_of::<KArgsGetRows>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: src0_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: src1_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 3u64];

                let tgs = MTLSize {
                    width: nw0 * (src1_shape.ne[0] as u64),
                    height: src1_shape.ne[1] as u64,
                    depth: src1_shape.ne[2] as u64,
                };
                let tpg = MTLSize {
                    width: nth,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_im2col_1d_f32(
            &mut self,
            src_id: ObjcId,
            dst_id: ObjcId,
            ic: usize,
            iw: usize,
            kw: usize,
            stride: usize,
            pad: usize,
            ow: usize,
        ) -> Result<(), String> {
            let base = "kernel_im2col_f32";
            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(base.to_string(), base, &[], 0, 0, 0, 0)?;

            let ic_i32 = i32::try_from(ic).map_err(|_| format!("ic too large: {}", ic))?;
            let iw_i32 = i32::try_from(iw).map_err(|_| format!("iw too large: {}", iw))?;
            let kw_i32 = i32::try_from(kw).map_err(|_| format!("kw too large: {}", kw))?;
            let ow_i32 = i32::try_from(ow).map_err(|_| format!("ow too large: {}", ow))?;
            let stride_i32 =
                i32::try_from(stride).map_err(|_| format!("stride too large: {}", stride))?;
            let pad_i32 = i32::try_from(pad).map_err(|_| format!("pad too large: {}", pad))?;

            let chw = ic
                .checked_mul(kw)
                .ok_or_else(|| "overflow computing im2col CHW".to_string())?;
            let ofs0 = ic
                .checked_mul(iw)
                .ok_or_else(|| "overflow computing im2col ofs0".to_string())?;

            let args = KArgsIm2Col {
                ofs0: ofs0 as u64,
                ofs1: iw as u64,
                iw: iw_i32,
                ih: 1,
                chw: i32::try_from(chw).map_err(|_| format!("CHW too large: {}", chw))?,
                s0: stride_i32,
                s1: 1,
                p0: pad_i32,
                p1: 0,
                d0: 1,
                d1: 1,
                n: 1,
                kh: 1,
                kw: kw_i32,
                khw: kw_i32,
            };

            let max_threads = Self::pipeline_max_threads(pipeline);
            let khkw = kw_i32 as u64;
            if khkw == 0 || khkw > max_threads {
                return Err(format!(
                    "invalid im2col thread shape: kh*kw={} max={}",
                    khkw, max_threads
                ));
            }
            let ntptg0 = (max_threads / khkw).min(1).max(1);

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsIm2Col as *const c_void
                    length: std::mem::size_of::<KArgsIm2Col>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: src_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 2u64];

                let tgs = MTLSize {
                    width: ic_i32 as u64,
                    height: 1,
                    depth: ow_i32 as u64,
                };
                let tpg = MTLSize {
                    width: ntptg0,
                    height: 1,
                    depth: kw_i32 as u64,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        #[allow(clippy::too_many_arguments)]
        fn dispatch_flash_attn_ext_pad(
            &mut self,
            k_id: ObjcId,
            v_id: ObjcId,
            mask_id: ObjcId,
            pad_id: ObjcId,
            has_mask: bool,
            ncpsg: i32,
            ne11: i32,
            ne_12_2: i32,
            ne_12_3: i32,
            nb11: u64,
            nb12: u64,
            nb13: u64,
            nb21: u64,
            nb22: u64,
            nb23: u64,
            ne31: i32,
            ne32: i32,
            ne33: i32,
            nb31: u64,
            nb32: u64,
            nb33: u64,
        ) -> Result<(), String> {
            let base = "kernel_flash_attn_ext_pad";
            let name = format!("{}_mask={}_ncpsg={}", base, has_mask as i32, ncpsg);
            let constants = [
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_PAD + 0,
                    value: FunctionConstantValue::Bool(has_mask),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_PAD + 25,
                    value: FunctionConstantValue::Int32(ncpsg),
                },
            ];
            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(name, base, &constants, 0, 0, 0, 0)?;

            let args = KArgsFlashAttnExtPad {
                ne11,
                ne_12_2,
                ne_12_3,
                nb11,
                nb12,
                nb13,
                nb21,
                nb22,
                nb23,
                ne31,
                ne32,
                ne33,
                nb31,
                nb32,
                nb33,
            };

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExtPad as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExtPad>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: k_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: v_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: mask_id offset: 0u64 atIndex: 3u64];
                let _: () = msg_send![encoder, setBuffer: pad_id offset: 0u64 atIndex: 4u64];

                let tgs = MTLSize {
                    width: ncpsg as u64,
                    height: ne_12_2.max(ne32) as u64,
                    depth: ne_12_3.max(ne33) as u64,
                };
                let tpg = MTLSize {
                    width: 32,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        #[allow(clippy::too_many_arguments)]
        fn dispatch_flash_attn_ext_blk(
            &mut self,
            mask_id: ObjcId,
            blk_id: ObjcId,
            n_q: usize,
            n_kv: usize,
            ne31: i32,
            ne32: i32,
            ne33: i32,
            nb31: u64,
            nb32: u64,
            nb33: u64,
            nqptg: i32,
            ncpsg: i32,
        ) -> Result<(), String> {
            let base = "kernel_flash_attn_ext_blk";
            let name = format!("{}_nqptg={}_ncpsg={}", base, nqptg, ncpsg);
            let constants = [
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_BLK + 24,
                    value: FunctionConstantValue::Int32(nqptg),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_BLK + 25,
                    value: FunctionConstantValue::Int32(ncpsg),
                },
            ];
            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(name, base, &constants, 0, 0, 0, 0)?;

            let ne01 = i32::try_from(n_q).map_err(|_| format!("n_q too large: {}", n_q))?;
            let ne30 = i32::try_from(n_kv).map_err(|_| format!("n_kv too large: {}", n_kv))?;
            let args = KArgsFlashAttnExtBlk {
                ne01,
                ne30,
                ne31,
                ne32,
                ne33,
                nb31,
                nb32,
                nb33,
            };

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExtBlk as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExtBlk>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: mask_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: blk_id offset: 0u64 atIndex: 2u64];

                let nblk1 = ((ne01 + nqptg - 1) / nqptg) as u64;
                let nblk0 = ((ne30 + ncpsg - 1) / ncpsg) as u64;
                let tgs = MTLSize {
                    width: nblk0,
                    height: nblk1,
                    depth: (ne32 * ne33) as u64,
                };
                let tpg = MTLSize {
                    width: 32,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_flash_attn_ext_f32(
            &mut self,
            q_id: ObjcId,
            k_id: ObjcId,
            v_id: ObjcId,
            mask_id: ObjcId,
            sinks_id: ObjcId,
            pad_id: ObjcId,
            blk_id: ObjcId,
            dst_id: ObjcId,
            n_q: usize,
            n_kv: usize,
            n_head: usize,
            d: usize,
            kv_type: Src0Type,
            scale: f32,
            has_mask: bool,
            has_sinks: bool,
            max_bias: f32,
            logit_softcap: f32,
        ) -> Result<(), String> {
            if !flash_attn_supported_head_dim(d) {
                return Err(format!(
                    "unsupported flash-attn head dim for kv kernel: {}",
                    d
                ));
            }
            if d % 4 != 0 {
                return Err(format!(
                    "flash-attn requires head dim divisible by 4 (float4 store), got {}",
                    d
                ));
            }

            let nsg = if d >= 512 { 8 } else { 4 };
            let nqptg = OP_FLASH_ATTN_EXT_NQPSG;
            let ncpsg = OP_FLASH_ATTN_EXT_NCPSG;
            let has_kvpad = n_kv % (ncpsg as usize) != 0;
            let has_bias = max_bias != 0.0;
            let has_scap = logit_softcap != 0.0;

            let ne01 = i32::try_from(n_q).map_err(|_| format!("n_q too large: {}", n_q))?;
            let ne11 = i32::try_from(n_kv).map_err(|_| format!("n_kv too large: {}", n_kv))?;
            let ne02 =
                i32::try_from(n_head).map_err(|_| format!("n_head too large: {}", n_head))?;
            let n_state = n_head
                .checked_mul(d)
                .ok_or_else(|| "overflow computing flash n_state".to_string())?;
            let kv_elem_bytes = flash_attn_kv_elem_bytes(kv_type)? as u64;

            let nb01 = (n_state as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing flash nb01".to_string())?;
            let nb02 = (d as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing flash nb02".to_string())?;
            let nb03 = nb01
                .checked_mul(n_q as u64)
                .ok_or_else(|| "overflow computing flash nb03".to_string())?;

            let nb11 = (n_state as u64)
                .checked_mul(kv_elem_bytes)
                .ok_or_else(|| "overflow computing flash nb11".to_string())?;
            let nb12 = (d as u64)
                .checked_mul(kv_elem_bytes)
                .ok_or_else(|| "overflow computing flash nb12".to_string())?;
            let nb13 = nb11
                .checked_mul(n_kv as u64)
                .ok_or_else(|| "overflow computing flash nb13".to_string())?;

            let nb21 = nb11;
            let nb22 = nb12;
            let nb23 = nb13;

            let ne31 = ne01;
            let ne32 = 1i32;
            let ne33 = 1i32;
            let nb31 = (n_kv as u64)
                .checked_mul(2)
                .ok_or_else(|| "overflow computing flash nb31".to_string())?;
            let nb32 = nb31
                .checked_mul(n_q as u64)
                .ok_or_else(|| "overflow computing flash nb32".to_string())?;
            let nb33 = nb32;

            let n_head_log2 = if n_head <= 1 {
                1i32
            } else {
                let p = (usize::BITS - 1) - n_head.leading_zeros();
                (1usize << p) as i32
            };
            let m0 = (2.0f32).powf(-(max_bias) / (n_head_log2 as f32));
            let m1 = (2.0f32).powf(-(max_bias / 2.0) / (n_head_log2 as f32));
            let scale_k = if has_scap {
                scale / logit_softcap
            } else {
                scale
            };
            let bc_mask = has_mask && (ne31 % 8 != 0);
            let ns10 = i32::try_from(nb11 / kv_elem_bytes)
                .map_err(|_| "overflow computing flash ns10".to_string())?;
            let ns20 = i32::try_from(nb21 / kv_elem_bytes)
                .map_err(|_| "overflow computing flash ns20".to_string())?;

            if has_kvpad {
                self.dispatch_flash_attn_ext_pad(
                    k_id, v_id, mask_id, pad_id, has_mask, ncpsg, ne11, ne02, 1, nb11, nb12, nb13,
                    nb21, nb22, nb23, ne31, ne32, ne33, nb31, nb32, nb33,
                )?;
            }
            if has_mask {
                self.dispatch_flash_attn_ext_blk(
                    mask_id, blk_id, n_q, n_kv, ne31, ne32, ne33, nb31, nb32, nb33, nqptg, ncpsg,
                )?;
            }

            let base = format!(
                "kernel_flash_attn_ext_{}_dk{}_dv{}",
                src0_type_name(kv_type),
                d,
                d
            );
            let name = format!(
                "{}_mask={}_sinks={}_bias={}_scap={}_kvpad={}_bcm={}_ns10={}_ns20={}_nsg={}",
                base,
                has_mask as i32,
                has_sinks as i32,
                has_bias as i32,
                has_scap as i32,
                has_kvpad as i32,
                bc_mask as i32,
                ns10,
                ns20,
                nsg
            );
            let constants = [
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 0,
                    value: FunctionConstantValue::Bool(has_mask),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 1,
                    value: FunctionConstantValue::Bool(has_sinks),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 2,
                    value: FunctionConstantValue::Bool(has_bias),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 3,
                    value: FunctionConstantValue::Bool(has_scap),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 4,
                    value: FunctionConstantValue::Bool(has_kvpad),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 10,
                    value: FunctionConstantValue::Bool(bc_mask),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 20,
                    value: FunctionConstantValue::Int32(ns10),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 21,
                    value: FunctionConstantValue::Int32(ns20),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT + 22,
                    value: FunctionConstantValue::Int32(nsg),
                },
            ];

            let smem = flash_attn_smem_bytes(d, d, nsg);
            let (pipeline, pipeline_smem, _nr0, _nr1, _pnsg) =
                self.get_or_compile_cached_pipeline(name, &base, &constants, smem, 0, 0, nsg)?;

            let args = KArgsFlashAttnExt {
                ne01,
                ne02,
                ne03: 1,
                nb01,
                nb02,
                nb03,
                ne11,
                ne_12_2: ne02,
                ne_12_3: 1,
                ns10,
                nb11,
                nb12,
                nb13,
                ns20,
                nb21,
                nb22,
                nb23,
                ne31,
                ne32,
                ne33,
                nb31,
                nb32,
                nb33,
                ne1: ne02,
                ne2: ne01,
                ne3: 1,
                scale: scale_k,
                max_bias,
                m0,
                m1,
                n_head_log2,
                logit_softcap,
            };

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExt as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExt>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: q_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: k_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: v_id offset: 0u64 atIndex: 3u64];
                let _: () = msg_send![encoder, setBuffer: mask_id offset: 0u64 atIndex: 4u64];
                let _: () = msg_send![encoder, setBuffer: sinks_id offset: 0u64 atIndex: 5u64];
                let _: () = msg_send![encoder, setBuffer: pad_id offset: 0u64 atIndex: 6u64];
                let _: () = msg_send![encoder, setBuffer: blk_id offset: 0u64 atIndex: 7u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 8u64];
                let _: () = msg_send![
                    encoder,
                    setThreadgroupMemoryLength: pipeline_smem as u64
                    atIndex: 0u64
                ];

                let tgs = MTLSize {
                    width: ((n_q as i32 + nqptg - 1) / nqptg) as u64,
                    height: n_head as u64,
                    depth: 1,
                };
                let tpg = MTLSize {
                    width: 32,
                    height: nsg as u64,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_flash_attn_ext_vec_reduce_f32(
            &mut self,
            tmp_id: ObjcId,
            dst_id: ObjcId,
            nrows: usize,
            d: usize,
            nwg: i32,
        ) -> Result<(), String> {
            let base = "kernel_flash_attn_ext_vec_reduce";
            let name = format!("{}_dv={}_nwg={}", base, d, nwg);
            let d_i32 =
                i32::try_from(d).map_err(|_| format!("d too large for vec reduce: {}", d))?;
            let constants = [
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC_REDUCE + 0,
                    value: FunctionConstantValue::Int32(d_i32),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC_REDUCE + 1,
                    value: FunctionConstantValue::Int32(nwg),
                },
            ];
            let (pipeline, _smem, _nr0, _nr1, _pnsg) =
                self.get_or_compile_cached_pipeline(name, base, &constants, 0, 0, 0, 0)?;

            let nrows_i32 = i32::try_from(nrows)
                .map_err(|_| format!("nrows too large for vec reduce: {}", nrows))?;
            let args = KArgsFlashAttnExtVecReduce { nrows: nrows_i32 };

            let tpg_width = (32i32)
                .checked_mul(nwg)
                .ok_or_else(|| "overflow computing vec reduce tpg width".to_string())?;
            let max_threads = Self::pipeline_max_threads(pipeline);
            if tpg_width as u64 > max_threads {
                return Err(format!(
                    "vec reduce threadsPerThreadgroup={} exceeds max={}",
                    tpg_width, max_threads
                ));
            }

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExtVecReduce as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExtVecReduce>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: tmp_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: dst_id offset: 0u64 atIndex: 2u64];

                let tgs = MTLSize {
                    width: nrows as u64,
                    height: 1,
                    depth: 1,
                };
                let tpg = MTLSize {
                    width: tpg_width as u64,
                    height: 1,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)
        }

        fn dispatch_flash_attn_ext_vec_f32(
            &mut self,
            q_id: ObjcId,
            k_id: ObjcId,
            v_id: ObjcId,
            mask_id: ObjcId,
            sinks_id: ObjcId,
            pad_id: ObjcId,
            tmp_id: Option<ObjcId>,
            dst_id: ObjcId,
            n_q: usize,
            n_kv: usize,
            n_head: usize,
            d: usize,
            kv_type: Src0Type,
            scale: f32,
            has_mask: bool,
            has_sinks: bool,
            max_bias: f32,
            logit_softcap: f32,
        ) -> Result<(), String> {
            if d % 32 != 0 {
                return Err(format!(
                    "flash-attn vec requires head dim divisible by 32, got {}",
                    d
                ));
            }
            if !flash_attn_supported_head_dim(d) {
                return Err(format!(
                    "unsupported flash-attn vec head dim for kv kernel: {}",
                    d
                ));
            }

            let nqptg = OP_FLASH_ATTN_EXT_VEC_NQPSG;
            let ncpsg = OP_FLASH_ATTN_EXT_VEC_NCPSG;
            let nhptg = 1i32;
            let has_kvpad = n_kv % (ncpsg as usize) != 0;
            let has_bias = max_bias != 0.0;
            let has_scap = logit_softcap != 0.0;

            let nwg = 32i32;
            let mut nsg = 1i32;
            while (2i64)
                .saturating_mul(nwg as i64)
                .saturating_mul(nsg as i64)
                .saturating_mul(ncpsg as i64)
                < n_kv as i64
                && nsg < 4
            {
                nsg *= 2;
            }

            let ne01 = i32::try_from(n_q).map_err(|_| format!("n_q too large: {}", n_q))?;
            let ne11 = i32::try_from(n_kv).map_err(|_| format!("n_kv too large: {}", n_kv))?;
            let ne02 =
                i32::try_from(n_head).map_err(|_| format!("n_head too large: {}", n_head))?;
            let n_state = n_head
                .checked_mul(d)
                .ok_or_else(|| "overflow computing flash n_state".to_string())?;
            let kv_elem_bytes = flash_attn_kv_elem_bytes(kv_type)? as u64;

            let nb01 = (n_state as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing flash nb01".to_string())?;
            let nb02 = (d as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing flash nb02".to_string())?;
            let nb03 = nb01
                .checked_mul(n_q as u64)
                .ok_or_else(|| "overflow computing flash nb03".to_string())?;

            let nb11 = (n_state as u64)
                .checked_mul(kv_elem_bytes)
                .ok_or_else(|| "overflow computing flash nb11".to_string())?;
            let nb12 = (d as u64)
                .checked_mul(kv_elem_bytes)
                .ok_or_else(|| "overflow computing flash nb12".to_string())?;
            let nb13 = nb11
                .checked_mul(n_kv as u64)
                .ok_or_else(|| "overflow computing flash nb13".to_string())?;

            let nb21 = nb11;
            let nb22 = nb12;
            let nb23 = nb13;

            let ne31 = ne01;
            let ne32 = 1i32;
            let ne33 = 1i32;
            let nb31 = (n_kv as u64)
                .checked_mul(2)
                .ok_or_else(|| "overflow computing flash nb31".to_string())?;
            let nb32 = nb31
                .checked_mul(n_q as u64)
                .ok_or_else(|| "overflow computing flash nb32".to_string())?;
            let nb33 = nb32;

            let n_head_log2 = if n_head <= 1 {
                1i32
            } else {
                let p = (usize::BITS - 1) - n_head.leading_zeros();
                (1usize << p) as i32
            };
            let m0 = (2.0f32).powf(-(max_bias) / (n_head_log2 as f32));
            let m1 = (2.0f32).powf(-(max_bias / 2.0) / (n_head_log2 as f32));
            let scale_k = if has_scap {
                scale / logit_softcap
            } else {
                scale
            };
            let ns10 = i32::try_from(nb11 / kv_elem_bytes)
                .map_err(|_| "overflow computing flash vec ns10".to_string())?;
            let ns20 = i32::try_from(nb21 / kv_elem_bytes)
                .map_err(|_| "overflow computing flash vec ns20".to_string())?;

            if has_kvpad {
                self.dispatch_flash_attn_ext_pad(
                    k_id, v_id, mask_id, pad_id, has_mask, ncpsg, ne11, ne02, 1, nb11, nb12, nb13,
                    nb21, nb22, nb23, ne31, ne32, ne33, nb31, nb32, nb33,
                )?;
            }

            let base = format!(
                "kernel_flash_attn_ext_vec_{}_dk{}_dv{}",
                src0_type_name(kv_type),
                d,
                d
            );
            let name = format!(
                "{}_mask={}_sink={}_bias={}_scap={}_kvpad={}_ns10={}_ns20={}_nsg={}_nwg={}",
                base,
                has_mask as i32,
                has_sinks as i32,
                has_bias as i32,
                has_scap as i32,
                has_kvpad as i32,
                ns10,
                ns20,
                nsg,
                nwg
            );
            let constants = [
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 0,
                    value: FunctionConstantValue::Bool(has_mask),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 1,
                    value: FunctionConstantValue::Bool(has_sinks),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 2,
                    value: FunctionConstantValue::Bool(has_bias),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 3,
                    value: FunctionConstantValue::Bool(has_scap),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 4,
                    value: FunctionConstantValue::Bool(has_kvpad),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 20,
                    value: FunctionConstantValue::Int32(ns10),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 21,
                    value: FunctionConstantValue::Int32(ns20),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 22,
                    value: FunctionConstantValue::Int32(nsg),
                },
                FunctionConstant {
                    idx: FC_FLASH_ATTN_EXT_VEC + 23,
                    value: FunctionConstantValue::Int32(nwg),
                },
            ];

            let smem = flash_attn_vec_smem_bytes(d, d, nsg);
            let (pipeline, pipeline_smem, _nr0, _nr1, _pnsg) =
                self.get_or_compile_cached_pipeline(name, &base, &constants, smem, 0, 0, nsg)?;
            let max_threads = Self::pipeline_max_threads(pipeline);
            let thread_width = (32i32)
                .checked_mul(nsg)
                .ok_or_else(|| "overflow computing vec thread width".to_string())?;
            if thread_width as u64 > max_threads {
                return Err(format!(
                    "flash-attn vec threadsPerThreadgroup={} exceeds max={}",
                    thread_width, max_threads
                ));
            }

            let args = KArgsFlashAttnExtVec {
                ne01,
                ne02,
                ne03: 1,
                nb01,
                nb02,
                nb03,
                ne11,
                ne_12_2: ne02,
                ne_12_3: 1,
                ns10,
                nb11,
                nb12,
                nb13,
                ns20,
                nb21,
                nb22,
                nb23,
                ne31,
                ne32,
                ne33,
                nb31,
                nb32,
                nb33,
                ne1: ne02,
                ne2: ne01,
                ne3: 1,
                scale: scale_k,
                max_bias,
                m0,
                m1,
                n_head_log2,
                logit_softcap,
            };

            let nrows = n_q
                .checked_mul(n_head)
                .ok_or_else(|| "overflow computing flash vec nrows".to_string())?;

            let out_id = if nwg == 1 {
                dst_id
            } else {
                tmp_id
                    .ok_or_else(|| "flash-attn vec requires tmp buffer when nwg > 1".to_string())?
            };

            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExtVec as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExtVec>() as u64
                    atIndex: 0u64
                ];
                let _: () = msg_send![encoder, setBuffer: q_id offset: 0u64 atIndex: 1u64];
                let _: () = msg_send![encoder, setBuffer: k_id offset: 0u64 atIndex: 2u64];
                let _: () = msg_send![encoder, setBuffer: v_id offset: 0u64 atIndex: 3u64];
                let _: () = msg_send![encoder, setBuffer: mask_id offset: 0u64 atIndex: 4u64];
                let _: () = msg_send![encoder, setBuffer: sinks_id offset: 0u64 atIndex: 5u64];
                let _: () = msg_send![encoder, setBuffer: pad_id offset: 0u64 atIndex: 6u64];
                let _: () = msg_send![encoder, setBuffer: out_id offset: 0u64 atIndex: 7u64];

                let _: () = msg_send![
                    encoder,
                    setThreadgroupMemoryLength: pipeline_smem as u64
                    atIndex: 0u64
                ];

                let tgs = MTLSize {
                    width: ((n_q as i32 + nqptg - 1) / nqptg) as u64,
                    height: ((n_head as i32 + nhptg - 1) / nhptg) as u64,
                    depth: nwg as u64,
                };
                let tpg = MTLSize {
                    width: 32,
                    height: nsg as u64,
                    depth: 1,
                };
                let _: () = msg_send![
                    encoder,
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
            }

            self.end_command_encoder(encoder_handles)?;

            if nwg > 1 {
                let tmp_id = tmp_id
                    .ok_or_else(|| "flash-attn vec requires tmp buffer when nwg > 1".to_string())?;
                self.dispatch_flash_attn_ext_vec_reduce_f32(tmp_id, dst_id, nrows, d, nwg)?;
            }

            Ok(())
        }

        fn flash_attn_f32_packed(
            &mut self,
            q: &[f32],
            k: &[f32],
            v: &[f32],
            n_q: usize,
            n_kv: usize,
            n_head: usize,
            d: usize,
            scale: f32,
        ) -> Result<Vec<f32>, String> {
            if n_q == 0 || n_kv == 0 || n_head == 0 || d == 0 {
                return Ok(Vec::new());
            }

            let q_need = n_q
                .checked_mul(n_head)
                .and_then(|v| v.checked_mul(d))
                .ok_or_else(|| "overflow computing flash q size".to_string())?;
            if q.len() != q_need {
                return Err(format!(
                    "flash q len mismatch: got {}, expected {}",
                    q.len(),
                    q_need
                ));
            }

            let kv_need = n_kv
                .checked_mul(n_head)
                .and_then(|v| v.checked_mul(d))
                .ok_or_else(|| "overflow computing flash kv size".to_string())?;
            if k.len() != kv_need {
                return Err(format!(
                    "flash k len mismatch: got {}, expected {}",
                    k.len(),
                    kv_need
                ));
            }
            if v.len() != kv_need {
                return Err(format!(
                    "flash v len mismatch: got {}, expected {}",
                    v.len(),
                    kv_need
                ));
            }

            let out_elems = n_q
                .checked_mul(n_head)
                .and_then(|v| v.checked_mul(d))
                .ok_or_else(|| "overflow computing flash output size".to_string())?;
            let out_bytes = out_elems
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing flash output bytes".to_string())?;

            let q_bytes = unsafe {
                std::slice::from_raw_parts(
                    q.as_ptr() as *const u8,
                    q.len() * std::mem::size_of::<f32>(),
                )
            };
            let k_bytes = unsafe {
                std::slice::from_raw_parts(
                    k.as_ptr() as *const u8,
                    k.len() * std::mem::size_of::<f32>(),
                )
            };
            let v_bytes = unsafe {
                std::slice::from_raw_parts(
                    v.as_ptr() as *const u8,
                    v.len() * std::mem::size_of::<f32>(),
                )
            };

            let q_buf = self.new_buffer_with_bytes(q_bytes)?;
            let k_buf = self.new_buffer_with_bytes(k_bytes)?;
            let v_buf = self.new_buffer_with_bytes(v_bytes)?;
            let dst_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_OUT, out_bytes)?;

            let params = FlashAttnExtParams::default();
            let kv_type = Src0Type::F32;
            let kv_elem_bytes = flash_attn_kv_elem_bytes(kv_type)?;
            let use_vec = flash_attn_use_vec(n_q, d);
            let pad_bytes = flash_attn_ext_extra_pad_bytes(
                n_q,
                n_kv,
                n_head,
                d,
                kv_elem_bytes,
                params.has_mask,
                use_vec,
            )?;
            let pad_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_PAD, pad_bytes)?;

            if use_vec {
                let tmp_bytes = flash_attn_ext_extra_tmp_bytes(n_q, n_head, d, 32)?;
                let tmp_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_TMP, tmp_bytes)?;

                self.dispatch_flash_attn_ext_vec_f32(
                    q_buf.as_id(),
                    k_buf.as_id(),
                    v_buf.as_id(),
                    q_buf.as_id(), // unused when has_mask=false
                    q_buf.as_id(), // unused when has_sinks=false
                    pad_id,
                    Some(tmp_id),
                    dst_id,
                    n_q,
                    n_kv,
                    n_head,
                    d,
                    kv_type,
                    scale,
                    params.has_mask,
                    params.has_sinks,
                    params.max_bias,
                    params.logit_softcap,
                )?;
            } else {
                let blk_bytes = flash_attn_ext_extra_blk_bytes(n_q, n_kv, params.has_mask, false)?;
                let blk_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_BLK, blk_bytes)?;

                self.dispatch_flash_attn_ext_f32(
                    q_buf.as_id(),
                    k_buf.as_id(),
                    v_buf.as_id(),
                    q_buf.as_id(), // unused when has_mask=false
                    q_buf.as_id(), // unused when has_sinks=false
                    pad_id,
                    blk_id,
                    dst_id,
                    n_q,
                    n_kv,
                    n_head,
                    d,
                    kv_type,
                    scale,
                    params.has_mask,
                    params.has_sinks,
                    params.max_bias,
                    params.logit_softcap,
                )?;
            }

            self.read_f32_buffer(dst_id, out_elems)
        }

        #[allow(clippy::too_many_arguments)]
        fn flash_attn_f32_self_kv_cache(
            &mut self,
            layer: usize,
            q: &[f32],
            k_all: &[f32],
            v_all: &[f32],
            n_kv: usize,
            n_head: usize,
            d: usize,
            scale: f32,
        ) -> Result<Vec<f32>, String> {
            if n_kv == 0 || n_head == 0 || d == 0 {
                return Ok(Vec::new());
            }
            let n_state = n_head
                .checked_mul(d)
                .ok_or_else(|| "overflow computing n_state".to_string())?;
            let kv_need = n_kv
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing decoder self kv size".to_string())?;
            if q.len() != n_state || k_all.len() != kv_need || v_all.len() != kv_need {
                return Err(format!(
                    "decoder self kv len mismatch: q={}, k_all={}, v_all={}, expected q={}, kv={}",
                    q.len(),
                    k_all.len(),
                    v_all.len(),
                    n_state,
                    kv_need
                ));
            }

            // Mirror whisper.cpp Metal path: self-KV cache length is padded to 32.
            let n_kv_flash = pad_to(n_kv, 32);
            let (k_id, v_id) = self.ensure_decoder_kv_layer(layer, n_state, n_kv_flash)?;
            let row_bytes = n_state
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing decoder kv row bytes".to_string())?;
            let start_row = self
                .decoder_kv_layers
                .get(&layer)
                .map(|e| e.len_rows.min(n_kv))
                .unwrap_or(0);
            if start_row < n_kv {
                let copy_rows = n_kv - start_row;
                let copy_bytes = copy_rows
                    .checked_mul(row_bytes)
                    .ok_or_else(|| "overflow computing decoder kv copy bytes".to_string())?;
                let offset = start_row
                    .checked_mul(row_bytes)
                    .ok_or_else(|| "overflow computing decoder kv copy offset".to_string())?;
                let src_k = f32_slice_as_bytes(&k_all[start_row * n_state..n_kv * n_state]);
                let src_v = f32_slice_as_bytes(&v_all[start_row * n_state..n_kv * n_state]);
                let dst_k: *mut u8 = unsafe { msg_send![k_id, contents] };
                let dst_v: *mut u8 = unsafe { msg_send![v_id, contents] };
                if dst_k.is_null() || dst_v.is_null() {
                    return Err("decoder kv buffer contents returned null".to_string());
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(src_k.as_ptr(), dst_k.add(offset), copy_bytes);
                    std::ptr::copy_nonoverlapping(src_v.as_ptr(), dst_v.add(offset), copy_bytes);
                }
                if let Some(entry) = self.decoder_kv_layers.get_mut(&layer) {
                    entry.len_rows = n_kv;
                }
            }

            if n_kv < n_kv_flash {
                let tail_off = n_kv
                    .checked_mul(row_bytes)
                    .ok_or_else(|| "overflow computing decoder kv tail offset".to_string())?;
                let tail_len = (n_kv_flash - n_kv)
                    .checked_mul(row_bytes)
                    .ok_or_else(|| "overflow computing decoder kv tail bytes".to_string())?;
                self.zero_buffer_range(k_id, tail_off, tail_len)?;
                self.zero_buffer_range(v_id, tail_off, tail_len)?;
            }

            let mask_id = self.prepare_decoder_self_mask_f16(n_kv, n_kv_flash)?;
            let q_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(q))?;
            let out_buf = self.flash_attn_f32_from_buffers_with_params(
                q_buf.as_id(),
                k_id,
                v_id,
                1,
                n_kv_flash,
                n_head,
                d,
                scale,
                FlashAttnExtParams {
                    has_mask: true,
                    ..FlashAttnExtParams::default()
                },
                Some(mask_id),
            )?;
            self.read_f32_buffer(out_buf.as_id(), n_state)
        }

        #[allow(clippy::too_many_arguments)]
        fn flash_attn_f32_cross_kv_cache(
            &mut self,
            layer: usize,
            q: &[f32],
            k_cross: &[f32],
            v_cross: &[f32],
            n_q: usize,
            n_kv: usize,
            n_head: usize,
            d: usize,
            scale: f32,
        ) -> Result<Vec<f32>, String> {
            if n_q == 0 || n_kv == 0 || n_head == 0 || d == 0 {
                return Ok(Vec::new());
            }
            let n_state = n_head
                .checked_mul(d)
                .ok_or_else(|| "overflow computing n_state".to_string())?;
            let q_need = n_q
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing cross q size".to_string())?;
            if q.len() != q_need {
                return Err(format!(
                    "cross q len mismatch: got {}, expected {}",
                    q.len(),
                    q_need
                ));
            }

            let (k_id, v_id) =
                self.ensure_cross_kv_layer(layer, n_state, n_kv, k_cross, v_cross)?;
            let q_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(q))?;
            let out_buf = self.flash_attn_f32_from_buffers(
                q_buf.as_id(),
                k_id,
                v_id,
                n_q,
                n_kv,
                n_head,
                d,
                scale,
            )?;
            self.read_f32_buffer(out_buf.as_id(), q_need)
        }

        fn flash_attn_f32_from_buffers(
            &mut self,
            q_id: ObjcId,
            k_id: ObjcId,
            v_id: ObjcId,
            n_q: usize,
            n_kv: usize,
            n_head: usize,
            d: usize,
            scale: f32,
        ) -> Result<StrongId, String> {
            self.flash_attn_f32_from_buffers_with_params(
                q_id,
                k_id,
                v_id,
                n_q,
                n_kv,
                n_head,
                d,
                scale,
                FlashAttnExtParams::default(),
                None,
            )
        }

        #[allow(clippy::too_many_arguments)]
        fn flash_attn_f32_from_buffers_with_params(
            &mut self,
            q_id: ObjcId,
            k_id: ObjcId,
            v_id: ObjcId,
            n_q: usize,
            n_kv: usize,
            n_head: usize,
            d: usize,
            scale: f32,
            params: FlashAttnExtParams,
            mask_id: Option<ObjcId>,
        ) -> Result<StrongId, String> {
            self.flash_attn_f32_from_buffers_with_params_typed(
                q_id,
                k_id,
                v_id,
                n_q,
                n_kv,
                n_head,
                d,
                scale,
                params,
                mask_id,
                Src0Type::F32,
            )
        }

        #[allow(clippy::too_many_arguments)]
        fn flash_attn_f32_from_buffers_with_params_typed(
            &mut self,
            q_id: ObjcId,
            k_id: ObjcId,
            v_id: ObjcId,
            n_q: usize,
            n_kv: usize,
            n_head: usize,
            d: usize,
            scale: f32,
            params: FlashAttnExtParams,
            mask_id: Option<ObjcId>,
            kv_type: Src0Type,
        ) -> Result<StrongId, String> {
            let out_elems = n_q
                .checked_mul(n_head)
                .and_then(|v| v.checked_mul(d))
                .ok_or_else(|| "overflow computing flash output size".to_string())?;
            let out_bytes = out_elems
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing flash output bytes".to_string())?;
            let kv_elem_bytes = flash_attn_kv_elem_bytes(kv_type)?;

            let dst_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_OUT, out_bytes)?;

            let use_vec = flash_attn_use_vec(n_q, d);
            let pad_bytes = flash_attn_ext_extra_pad_bytes(
                n_q,
                n_kv,
                n_head,
                d,
                kv_elem_bytes,
                params.has_mask,
                use_vec,
            )?;
            let pad_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_PAD, pad_bytes)?;
            let mask_buf_id = if params.has_mask {
                mask_id.ok_or_else(|| {
                    "flash-attn requested mask but no mask buffer was provided".to_string()
                })?
            } else {
                q_id
            };

            if use_vec {
                let tmp_bytes = flash_attn_ext_extra_tmp_bytes(n_q, n_head, d, 32)?;
                let tmp_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_TMP, tmp_bytes)?;

                self.dispatch_flash_attn_ext_vec_f32(
                    q_id,
                    k_id,
                    v_id,
                    mask_buf_id,
                    q_id, // unused when has_sinks=false
                    pad_id,
                    Some(tmp_id),
                    dst_id,
                    n_q,
                    n_kv,
                    n_head,
                    d,
                    kv_type,
                    scale,
                    params.has_mask,
                    params.has_sinks,
                    params.max_bias,
                    params.logit_softcap,
                )?;
            } else {
                let blk_bytes = flash_attn_ext_extra_blk_bytes(n_q, n_kv, params.has_mask, false)?;
                let blk_id = self.get_or_create_scratch_buffer(SCRATCH_FLASH_BLK, blk_bytes)?;

                self.dispatch_flash_attn_ext_f32(
                    q_id,
                    k_id,
                    v_id,
                    mask_buf_id,
                    q_id, // unused when has_sinks=false
                    pad_id,
                    blk_id,
                    dst_id,
                    n_q,
                    n_kv,
                    n_head,
                    d,
                    kv_type,
                    scale,
                    params.has_mask,
                    params.has_sinks,
                    params.max_bias,
                    params.logit_softcap,
                )?;
            }

            unsafe { StrongId::from_unowned(dst_id) }
                .ok_or_else(|| "flash-attn output buffer returned nil".to_string())
        }

        #[allow(dead_code)]
        fn encoder_flash_kv_f16_buffer(
            &mut self,
            src_f32_id: ObjcId,
            seq_len: usize,
            n_kv_flash: usize,
            n_state: usize,
            scratch_kind: u8,
        ) -> Result<ObjcId, String> {
            let row_bytes_f32 = n_state
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing encoder f32 row bytes".to_string())?;
            let row_bytes_f16 = n_state
                .checked_mul(std::mem::size_of::<u16>())
                .ok_or_else(|| "overflow computing encoder f16 row bytes".to_string())?;
            let total_bytes = n_kv_flash
                .checked_mul(row_bytes_f16)
                .ok_or_else(|| "overflow computing encoder f16 kv bytes".to_string())?;
            let dst_id = self.get_or_create_scratch_buffer(scratch_kind, total_bytes)?;

            self.dispatch_cpy_f32_to_f16(
                src_f32_id,
                dst_id,
                n_state,
                seq_len,
                std::mem::size_of::<f32>() as u64,
                row_bytes_f32 as u64,
                std::mem::size_of::<u16>() as u64,
                row_bytes_f16 as u64,
            )?;

            if seq_len < n_kv_flash {
                let tail_off = seq_len
                    .checked_mul(row_bytes_f16)
                    .ok_or_else(|| "overflow computing encoder kv tail offset".to_string())?;
                let tail_bytes = (n_kv_flash - seq_len)
                    .checked_mul(row_bytes_f16)
                    .ok_or_else(|| "overflow computing encoder kv tail bytes".to_string())?;
                self.zero_buffer_range(dst_id, tail_off, tail_bytes)?;
            }

            Ok(dst_id)
        }

        #[allow(dead_code)]
        #[allow(clippy::too_many_arguments)]
        fn linear_from_src_buffer(
            &mut self,
            src_id: ObjcId,
            m: usize,
            k: usize,
            w_bytes: &[u8],
            w_ggml_type: u32,
            n: usize,
            bias: Option<&[f32]>,
            weight_tag: u8,
            bias_tag: u8,
        ) -> Result<StrongId, String> {
            let dst = self.matmul_nt_ggml_from_src1_buffer(
                src_id,
                w_bytes,
                w_ggml_type,
                m,
                k,
                n,
                Some(weight_tag),
            )?;

            if let Some(bias) = bias {
                if bias.len() != n {
                    return Err(format!(
                        "linear bias len mismatch: got {}, expected {}",
                        bias.len(),
                        n
                    ));
                }
                let bias_id = self.get_or_create_cached_f32_buffer(bias, bias_tag)?;
                let dst_shape = shape4_from_row_major(&[m, n], 4)?;
                let bias_shape = shape4_from_row_major(&[n], 4)?;
                self.dispatch_bin_f32(
                    0,
                    dst.as_id(),
                    bias_id,
                    dst.as_id(),
                    &dst_shape,
                    &bias_shape,
                )?;
            }

            Ok(dst)
        }

        #[allow(dead_code)]
        #[allow(clippy::too_many_arguments)]
        fn encoder_attn_block_f32(
            &mut self,
            x: &[f32],
            seq_len: usize,
            n_state: usize,
            n_head: usize,
            ln_w: &[f32],
            ln_b: &[f32],
            q_w_bytes: &[u8],
            q_w_ggml_type: u32,
            q_b: &[f32],
            k_w_bytes: &[u8],
            k_w_ggml_type: u32,
            v_w_bytes: &[u8],
            v_w_ggml_type: u32,
            v_b: &[f32],
            out_w_bytes: &[u8],
            out_w_ggml_type: u32,
            out_b: &[f32],
        ) -> Result<Vec<f32>, String> {
            if n_state == 0 || seq_len == 0 || n_head == 0 || n_state % n_head != 0 {
                return Err(format!(
                    "invalid attn dimensions: seq_len={}, n_state={}, n_head={}",
                    seq_len, n_state, n_head
                ));
            }
            let x_need = seq_len
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing x size".to_string())?;
            if x.len() != x_need {
                return Err(format!(
                    "x len mismatch: got {}, expected {}",
                    x.len(),
                    x_need
                ));
            }
            if ln_w.len() != n_state || ln_b.len() != n_state {
                return Err("layernorm affine size mismatch".to_string());
            }
            if q_b.len() != n_state || v_b.len() != n_state || out_b.len() != n_state {
                return Err("attention bias size mismatch".to_string());
            }

            let x_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(x))?;
            let x_shape = shape4_from_row_major(&[seq_len, n_state], 4)?;
            let ln_shape = shape4_from_row_major(&[n_state], 4)?;
            let ln_w_id = self.get_or_create_cached_f32_buffer(ln_w, 110)?;
            let ln_b_id = self.get_or_create_cached_f32_buffer(ln_b, 111)?;

            let norm_buf = self.new_buffer_with_length(x_need * std::mem::size_of::<f32>())?;
            self.dispatch_norm_f32(
                x_buf.as_id(),
                ln_w_id,
                ln_b_id,
                norm_buf.as_id(),
                &x_shape,
                &ln_shape,
                &ln_shape,
                1e-5f32,
                3,
            )?;

            let q_buf = self.linear_from_src_buffer(
                norm_buf.as_id(),
                seq_len,
                n_state,
                q_w_bytes,
                q_w_ggml_type,
                n_state,
                Some(q_b),
                112,
                113,
            )?;
            let k_buf = self.linear_from_src_buffer(
                norm_buf.as_id(),
                seq_len,
                n_state,
                k_w_bytes,
                k_w_ggml_type,
                n_state,
                None,
                114,
                0,
            )?;
            let v_buf = self.linear_from_src_buffer(
                norm_buf.as_id(),
                seq_len,
                n_state,
                v_w_bytes,
                v_w_ggml_type,
                n_state,
                Some(v_b),
                115,
                116,
            )?;

            let d = n_state / n_head;
            let scale = 1.0f32 / (d as f32).sqrt();
            let attn_buf = self.flash_attn_f32_from_buffers(
                q_buf.as_id(),
                k_buf.as_id(),
                v_buf.as_id(),
                seq_len,
                seq_len,
                n_head,
                d,
                scale,
            )?;

            let proj_buf = self.linear_from_src_buffer(
                attn_buf.as_id(),
                seq_len,
                n_state,
                out_w_bytes,
                out_w_ggml_type,
                n_state,
                Some(out_b),
                117,
                118,
            )?;

            self.dispatch_bin_f32(
                0,
                proj_buf.as_id(),
                x_buf.as_id(),
                proj_buf.as_id(),
                &x_shape,
                &x_shape,
            )?;

            self.read_f32_buffer(proj_buf.as_id(), x_need)
        }

        #[allow(dead_code)]
        #[allow(clippy::too_many_arguments)]
        fn encoder_ffn_block_f32(
            &mut self,
            x: &[f32],
            seq_len: usize,
            n_state: usize,
            ln_w: &[f32],
            ln_b: &[f32],
            w0_bytes: &[u8],
            w0_ggml_type: u32,
            b0: &[f32],
            w1_bytes: &[u8],
            w1_ggml_type: u32,
            b1: &[f32],
        ) -> Result<Vec<f32>, String> {
            if n_state == 0 || seq_len == 0 {
                return Err(format!(
                    "invalid ffn dimensions: seq_len={}, n_state={}",
                    seq_len, n_state
                ));
            }
            let x_need = seq_len
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing x size".to_string())?;
            if x.len() != x_need {
                return Err(format!(
                    "x len mismatch: got {}, expected {}",
                    x.len(),
                    x_need
                ));
            }
            if ln_w.len() != n_state || ln_b.len() != n_state {
                return Err("layernorm affine size mismatch".to_string());
            }
            let n_ff = b0.len();
            if n_ff == 0 || b1.len() != n_state {
                return Err("ffn bias size mismatch".to_string());
            }

            let x_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(x))?;
            let x_shape = shape4_from_row_major(&[seq_len, n_state], 4)?;
            let ln_shape = shape4_from_row_major(&[n_state], 4)?;
            let ln_w_id = self.get_or_create_cached_f32_buffer(ln_w, 120)?;
            let ln_b_id = self.get_or_create_cached_f32_buffer(ln_b, 121)?;

            let norm_buf = self.new_buffer_with_length(x_need * std::mem::size_of::<f32>())?;
            self.dispatch_norm_f32(
                x_buf.as_id(),
                ln_w_id,
                ln_b_id,
                norm_buf.as_id(),
                &x_shape,
                &ln_shape,
                &ln_shape,
                1e-5f32,
                3,
            )?;

            let ff0_buf = self.linear_from_src_buffer(
                norm_buf.as_id(),
                seq_len,
                n_state,
                w0_bytes,
                w0_ggml_type,
                n_ff,
                Some(b0),
                122,
                123,
            )?;

            let ff0_shape = shape4_from_row_major(&[seq_len, n_ff], 4)?;
            self.dispatch_unary_f32(
                OP_UNARY_NUM_GELU,
                ff0_buf.as_id(),
                ff0_buf.as_id(),
                &ff0_shape,
            )?;

            let ff1_buf = self.linear_from_src_buffer(
                ff0_buf.as_id(),
                seq_len,
                n_ff,
                w1_bytes,
                w1_ggml_type,
                n_state,
                Some(b1),
                124,
                125,
            )?;

            self.dispatch_bin_f32(
                0,
                ff1_buf.as_id(),
                x_buf.as_id(),
                ff1_buf.as_id(),
                &x_shape,
                &x_shape,
            )?;

            self.read_f32_buffer(ff1_buf.as_id(), x_need)
        }

        #[allow(dead_code)]
        #[allow(clippy::too_many_arguments)]
        fn encoder_layer_from_buffer_f32(
            &mut self,
            x_id: ObjcId,
            seq_len: usize,
            n_state: usize,
            n_head: usize,
            attn_ln_w: &[f32],
            attn_ln_b: &[f32],
            q_w_bytes: &[u8],
            q_w_ggml_type: u32,
            q_b: &[f32],
            k_w_bytes: &[u8],
            k_w_ggml_type: u32,
            v_w_bytes: &[u8],
            v_w_ggml_type: u32,
            v_b: &[f32],
            out_w_bytes: &[u8],
            out_w_ggml_type: u32,
            out_b: &[f32],
            mlp_ln_w: &[f32],
            mlp_ln_b: &[f32],
            w0_bytes: &[u8],
            w0_ggml_type: u32,
            b0: &[f32],
            w1_bytes: &[u8],
            w1_ggml_type: u32,
            b1: &[f32],
            tag_base: u8,
        ) -> Result<StrongId, String> {
            if n_state == 0 || seq_len == 0 || n_head == 0 || n_state % n_head != 0 {
                return Err(format!(
                    "invalid encoder layer dimensions: seq_len={}, n_state={}, n_head={}",
                    seq_len, n_state, n_head
                ));
            }
            if attn_ln_w.len() != n_state
                || attn_ln_b.len() != n_state
                || mlp_ln_w.len() != n_state
                || mlp_ln_b.len() != n_state
            {
                return Err("layernorm affine size mismatch".to_string());
            }
            if q_b.len() != n_state || v_b.len() != n_state || out_b.len() != n_state {
                return Err("attention bias size mismatch".to_string());
            }
            let n_ff = b0.len();
            if n_ff == 0 || b1.len() != n_state {
                return Err("ffn bias size mismatch".to_string());
            }

            let x_shape = shape4_from_row_major(&[seq_len, n_state], 4)?;
            let ln_shape = shape4_from_row_major(&[n_state], 4)?;

            // Attention sub-block
            let attn_ln_w_id =
                self.get_or_create_cached_f32_buffer(attn_ln_w, tag_base.wrapping_add(0))?;
            let attn_ln_b_id =
                self.get_or_create_cached_f32_buffer(attn_ln_b, tag_base.wrapping_add(1))?;
            let norm_bytes = x_shape
                .numel
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "overflow computing encoder norm buffer bytes".to_string())?;
            let norm0_id = self.get_or_create_scratch_buffer(SCRATCH_ENC_NORM0, norm_bytes)?;
            self.dispatch_norm_f32(
                x_id,
                attn_ln_w_id,
                attn_ln_b_id,
                norm0_id,
                &x_shape,
                &ln_shape,
                &ln_shape,
                1e-5f32,
                3,
            )?;

            let q_buf = self.linear_from_src_buffer(
                norm0_id,
                seq_len,
                n_state,
                q_w_bytes,
                q_w_ggml_type,
                n_state,
                Some(q_b),
                tag_base.wrapping_add(2),
                tag_base.wrapping_add(3),
            )?;

            // Match whisper.cpp encoder flash-attn usage: keep KV rows padded.
            let n_kv_flash = pad_to(seq_len, 256);

            let k_buf_f32 = self.linear_from_src_buffer(
                norm0_id,
                seq_len,
                n_state,
                k_w_bytes,
                k_w_ggml_type,
                n_state,
                None,
                tag_base.wrapping_add(4),
                0,
            )?;
            let v_buf_f32 = self.linear_from_src_buffer(
                norm0_id,
                seq_len,
                n_state,
                v_w_bytes,
                v_w_ggml_type,
                n_state,
                Some(v_b),
                tag_base.wrapping_add(5),
                tag_base.wrapping_add(6),
            )?;
            let k_buf_f16 = self.encoder_flash_kv_f16_buffer(
                k_buf_f32.as_id(),
                seq_len,
                n_kv_flash,
                n_state,
                SCRATCH_ENC_FLASH_K_F16,
            )?;
            let v_buf_f16 = self.encoder_flash_kv_f16_buffer(
                v_buf_f32.as_id(),
                seq_len,
                n_kv_flash,
                n_state,
                SCRATCH_ENC_FLASH_V_F16,
            )?;

            let d = n_state / n_head;
            let scale = 1.0f32 / (d as f32).sqrt();
            let attn_buf = self.flash_attn_f32_from_buffers_with_params_typed(
                q_buf.as_id(),
                k_buf_f16,
                v_buf_f16,
                seq_len,
                n_kv_flash,
                n_head,
                d,
                scale,
                FlashAttnExtParams::default(),
                None,
                Src0Type::F16,
            )?;
            let attn_res_buf = self.linear_from_src_buffer(
                attn_buf.as_id(),
                seq_len,
                n_state,
                out_w_bytes,
                out_w_ggml_type,
                n_state,
                Some(out_b),
                tag_base.wrapping_add(7),
                tag_base.wrapping_add(8),
            )?;
            self.dispatch_bin_f32(
                0,
                attn_res_buf.as_id(),
                x_id,
                attn_res_buf.as_id(),
                &x_shape,
                &x_shape,
            )?;

            // FFN sub-block
            let mlp_ln_w_id =
                self.get_or_create_cached_f32_buffer(mlp_ln_w, tag_base.wrapping_add(9))?;
            let mlp_ln_b_id =
                self.get_or_create_cached_f32_buffer(mlp_ln_b, tag_base.wrapping_add(10))?;
            let norm1_id = self.get_or_create_scratch_buffer(SCRATCH_ENC_NORM1, norm_bytes)?;
            self.dispatch_norm_f32(
                attn_res_buf.as_id(),
                mlp_ln_w_id,
                mlp_ln_b_id,
                norm1_id,
                &x_shape,
                &ln_shape,
                &ln_shape,
                1e-5f32,
                3,
            )?;

            let ff0_buf = self.linear_from_src_buffer(
                norm1_id,
                seq_len,
                n_state,
                w0_bytes,
                w0_ggml_type,
                n_ff,
                Some(b0),
                tag_base.wrapping_add(11),
                tag_base.wrapping_add(12),
            )?;
            let ff0_shape = shape4_from_row_major(&[seq_len, n_ff], 4)?;
            self.dispatch_unary_f32(
                OP_UNARY_NUM_GELU,
                ff0_buf.as_id(),
                ff0_buf.as_id(),
                &ff0_shape,
            )?;
            let ff1_buf = self.linear_from_src_buffer(
                ff0_buf.as_id(),
                seq_len,
                n_ff,
                w1_bytes,
                w1_ggml_type,
                n_state,
                Some(b1),
                tag_base.wrapping_add(13),
                tag_base.wrapping_add(14),
            )?;
            self.dispatch_bin_f32(
                0,
                ff1_buf.as_id(),
                attn_res_buf.as_id(),
                ff1_buf.as_id(),
                &x_shape,
                &x_shape,
            )?;

            Ok(ff1_buf)
        }

        #[allow(dead_code)]
        #[allow(clippy::too_many_arguments)]
        fn encoder_layer_f32(
            &mut self,
            x: &[f32],
            seq_len: usize,
            n_state: usize,
            n_head: usize,
            attn_ln_w: &[f32],
            attn_ln_b: &[f32],
            q_w_bytes: &[u8],
            q_w_ggml_type: u32,
            q_b: &[f32],
            k_w_bytes: &[u8],
            k_w_ggml_type: u32,
            v_w_bytes: &[u8],
            v_w_ggml_type: u32,
            v_b: &[f32],
            out_w_bytes: &[u8],
            out_w_ggml_type: u32,
            out_b: &[f32],
            mlp_ln_w: &[f32],
            mlp_ln_b: &[f32],
            w0_bytes: &[u8],
            w0_ggml_type: u32,
            b0: &[f32],
            w1_bytes: &[u8],
            w1_ggml_type: u32,
            b1: &[f32],
        ) -> Result<Vec<f32>, String> {
            let x_need = seq_len
                .checked_mul(n_state)
                .ok_or_else(|| "overflow computing x size".to_string())?;
            if x.len() != x_need {
                return Err(format!(
                    "x len mismatch: got {}, expected {}",
                    x.len(),
                    x_need
                ));
            }
            let x_buf = self.new_buffer_with_bytes(f32_slice_as_bytes(x))?;

            let ff1_buf = self.encoder_layer_from_buffer_f32(
                x_buf.as_id(),
                seq_len,
                n_state,
                n_head,
                attn_ln_w,
                attn_ln_b,
                q_w_bytes,
                q_w_ggml_type,
                q_b,
                k_w_bytes,
                k_w_ggml_type,
                v_w_bytes,
                v_w_ggml_type,
                v_b,
                out_w_bytes,
                out_w_ggml_type,
                out_b,
                mlp_ln_w,
                mlp_ln_b,
                w0_bytes,
                w0_ggml_type,
                b0,
                w1_bytes,
                w1_ggml_type,
                b1,
                130,
            )?;
            self.read_f32_buffer(ff1_buf.as_id(), x_need)
        }

        #[allow(dead_code)]
        #[allow(clippy::too_many_arguments)]
        fn decoder_self_qkv_step_f32(
            &mut self,
            x: &[f32],
            n_state: usize,
            attn_ln_w: &[f32],
            attn_ln_b: &[f32],
            q_w_bytes: &[u8],
            q_w_ggml_type: u32,
            q_b: &[f32],
            k_w_bytes: &[u8],
            k_w_ggml_type: u32,
            v_w_bytes: &[u8],
            v_w_ggml_type: u32,
            v_b: &[f32],
        ) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>), String> {
            if n_state == 0 {
                return Err("decoder qkv: n_state is zero".to_string());
            }
            if x.len() != n_state
                || attn_ln_w.len() != n_state
                || attn_ln_b.len() != n_state
                || q_b.len() != n_state
                || v_b.len() != n_state
            {
                return Err("decoder qkv: input/affine size mismatch".to_string());
            }

            let x_shape = shape4_from_row_major(&[1, n_state], 4)?;
            let ln_shape = shape4_from_row_major(&[n_state], 4)?;

            let (q_buf, k_buf, v_buf) = self.with_batch(|ctx| {
                let x_buf = ctx.new_buffer_with_bytes(f32_slice_as_bytes(x))?;

                let ln_w_id = ctx.get_or_create_cached_f32_buffer(attn_ln_w, 240)?;
                let ln_b_id = ctx.get_or_create_cached_f32_buffer(attn_ln_b, 241)?;
                let norm_bytes = n_state
                    .checked_mul(std::mem::size_of::<f32>())
                    .ok_or_else(|| "overflow computing decoder qkv norm bytes".to_string())?;
                let norm_id = ctx.get_or_create_scratch_buffer(SCRATCH_DEC_NORM0, norm_bytes)?;
                ctx.dispatch_norm_f32(
                    x_buf.as_id(),
                    ln_w_id,
                    ln_b_id,
                    norm_id,
                    &x_shape,
                    &ln_shape,
                    &ln_shape,
                    1e-5f32,
                    3,
                )?;

                let q_buf = ctx.linear_from_src_buffer(
                    norm_id,
                    1,
                    n_state,
                    q_w_bytes,
                    q_w_ggml_type,
                    n_state,
                    Some(q_b),
                    242,
                    243,
                )?;
                let k_buf = ctx.linear_from_src_buffer(
                    norm_id,
                    1,
                    n_state,
                    k_w_bytes,
                    k_w_ggml_type,
                    n_state,
                    None,
                    244,
                    0,
                )?;
                let v_buf = ctx.linear_from_src_buffer(
                    norm_id,
                    1,
                    n_state,
                    v_w_bytes,
                    v_w_ggml_type,
                    n_state,
                    Some(v_b),
                    245,
                    246,
                )?;

                Ok((q_buf, k_buf, v_buf))
            })?;

            self.read_f32_buffers3(q_buf.as_id(), k_buf.as_id(), v_buf.as_id(), n_state)
        }

        #[allow(clippy::too_many_arguments)]
        fn bin_f32(
            &mut self,
            op_num: i16,
            a: &[f32],
            a_shape: &[usize],
            b: &[f32],
            b_shape: &[usize],
        ) -> Result<Vec<f32>, String> {
            let a_s = shape4_from_row_major(a_shape, 4)?;
            let b_s = shape4_from_row_major(b_shape, 4)?;
            if a.len() != a_s.numel {
                return Err(format!(
                    "lhs len mismatch: got {}, expected {}",
                    a.len(),
                    a_s.numel
                ));
            }
            if b.len() != b_s.numel {
                return Err(format!(
                    "rhs len mismatch: got {}, expected {}",
                    b.len(),
                    b_s.numel
                ));
            }

            let a_bytes = unsafe {
                std::slice::from_raw_parts(
                    a.as_ptr() as *const u8,
                    a.len() * std::mem::size_of::<f32>(),
                )
            };
            let b_bytes = unsafe {
                std::slice::from_raw_parts(
                    b.as_ptr() as *const u8,
                    b.len() * std::mem::size_of::<f32>(),
                )
            };

            let a_buf = self.new_buffer_with_bytes(a_bytes)?;
            let b_buf = self.new_buffer_with_bytes(b_bytes)?;
            let dst_buf = self.new_buffer_with_length(a_s.numel * std::mem::size_of::<f32>())?;

            self.dispatch_bin_f32(
                op_num,
                a_buf.as_id(),
                b_buf.as_id(),
                dst_buf.as_id(),
                &a_s,
                &b_s,
            )?;
            self.read_f32_buffer(dst_buf.as_id(), a_s.numel)
        }

        fn unary_gelu_f32(&mut self, a: &[f32], shape: &[usize]) -> Result<Vec<f32>, String> {
            let s = shape4_from_row_major(shape, 4)?;
            if a.len() != s.numel {
                return Err(format!(
                    "unary len mismatch: got {}, expected {}",
                    a.len(),
                    s.numel
                ));
            }

            let a_bytes = unsafe {
                std::slice::from_raw_parts(
                    a.as_ptr() as *const u8,
                    a.len() * std::mem::size_of::<f32>(),
                )
            };

            let a_buf = self.new_buffer_with_bytes(a_bytes)?;
            let dst_buf = self.new_buffer_with_length(s.numel * std::mem::size_of::<f32>())?;
            self.dispatch_unary_f32(OP_UNARY_NUM_GELU, a_buf.as_id(), dst_buf.as_id(), &s)?;
            self.read_f32_buffer(dst_buf.as_id(), s.numel)
        }

        fn norm_f32(&mut self, x: &[f32], x_shape: &[usize], eps: f32) -> Result<Vec<f32>, String> {
            let s = shape4_from_row_major(x_shape, 4)?;
            if x.len() != s.numel {
                return Err(format!(
                    "norm len mismatch: got {}, expected {}",
                    x.len(),
                    s.numel
                ));
            }

            let x_bytes = unsafe {
                std::slice::from_raw_parts(
                    x.as_ptr() as *const u8,
                    x.len() * std::mem::size_of::<f32>(),
                )
            };

            let x_buf = self.new_buffer_with_bytes(x_bytes)?;
            let dst_buf = self.new_buffer_with_length(s.numel * std::mem::size_of::<f32>())?;
            self.dispatch_norm_f32(
                x_buf.as_id(),
                x_buf.as_id(),
                x_buf.as_id(),
                dst_buf.as_id(),
                &s,
                &s,
                &s,
                eps,
                1,
            )?;
            self.read_f32_buffer(dst_buf.as_id(), s.numel)
        }

        fn rms_norm_f32(
            &mut self,
            x: &[f32],
            x_shape: &[usize],
            eps: f32,
        ) -> Result<Vec<f32>, String> {
            let s = shape4_from_row_major(x_shape, 4)?;
            if x.len() != s.numel {
                return Err(format!(
                    "rms_norm len mismatch: got {}, expected {}",
                    x.len(),
                    s.numel
                ));
            }

            let x_bytes = unsafe {
                std::slice::from_raw_parts(
                    x.as_ptr() as *const u8,
                    x.len() * std::mem::size_of::<f32>(),
                )
            };

            let x_buf = self.new_buffer_with_bytes(x_bytes)?;
            let dst_buf = self.new_buffer_with_length(s.numel * std::mem::size_of::<f32>())?;
            self.dispatch_rms_norm_f32(
                x_buf.as_id(),
                x_buf.as_id(),
                x_buf.as_id(),
                dst_buf.as_id(),
                &s,
                &s,
                &s,
                eps,
                1,
            )?;
            self.read_f32_buffer(dst_buf.as_id(), s.numel)
        }

        fn rms_norm_mul_f32(
            &mut self,
            x: &[f32],
            x_shape: &[usize],
            mul: &[f32],
            mul_shape: &[usize],
            eps: f32,
        ) -> Result<Vec<f32>, String> {
            let x_s = shape4_from_row_major(x_shape, 4)?;
            let m_s = shape4_from_row_major(mul_shape, 4)?;

            if x.len() != x_s.numel {
                return Err(format!(
                    "rms_norm src len mismatch: got {}, expected {}",
                    x.len(),
                    x_s.numel
                ));
            }
            if mul.len() != m_s.numel {
                return Err(format!(
                    "rms_norm mul len mismatch: got {}, expected {}",
                    mul.len(),
                    m_s.numel
                ));
            }

            if m_s.ne[0] != x_s.ne[0] {
                return Err(format!(
                    "rms_norm fuse ne0 mismatch: x={} mul={}",
                    x_s.ne[0], m_s.ne[0]
                ));
            }
            for d in 1..4 {
                if m_s.ne[d] != 1 && m_s.ne[d] != x_s.ne[d] {
                    return Err("rms_norm fuse broadcast mismatch".to_string());
                }
            }

            let x_bytes = unsafe {
                std::slice::from_raw_parts(
                    x.as_ptr() as *const u8,
                    x.len() * std::mem::size_of::<f32>(),
                )
            };
            let mul_bytes = unsafe {
                std::slice::from_raw_parts(
                    mul.as_ptr() as *const u8,
                    mul.len() * std::mem::size_of::<f32>(),
                )
            };

            let x_buf = self.new_buffer_with_bytes(x_bytes)?;
            let mul_buf = self.new_buffer_with_bytes(mul_bytes)?;
            let dst_buf = self.new_buffer_with_length(x_s.numel * std::mem::size_of::<f32>())?;

            self.dispatch_rms_norm_f32(
                x_buf.as_id(),
                mul_buf.as_id(),
                x_buf.as_id(),
                dst_buf.as_id(),
                &x_s,
                &m_s,
                &x_s,
                eps,
                2,
            )?;
            self.read_f32_buffer(dst_buf.as_id(), x_s.numel)
        }

        #[allow(clippy::too_many_arguments)]
        fn norm_mul_add_f32(
            &mut self,
            x: &[f32],
            x_shape: &[usize],
            mul: &[f32],
            mul_shape: &[usize],
            add: &[f32],
            add_shape: &[usize],
            eps: f32,
        ) -> Result<Vec<f32>, String> {
            let x_s = shape4_from_row_major(x_shape, 4)?;
            let m_s = shape4_from_row_major(mul_shape, 4)?;
            let a_s = shape4_from_row_major(add_shape, 4)?;

            if x.len() != x_s.numel {
                return Err(format!(
                    "norm src len mismatch: got {}, expected {}",
                    x.len(),
                    x_s.numel
                ));
            }
            if mul.len() != m_s.numel {
                return Err(format!(
                    "norm mul len mismatch: got {}, expected {}",
                    mul.len(),
                    m_s.numel
                ));
            }
            if add.len() != a_s.numel {
                return Err(format!(
                    "norm add len mismatch: got {}, expected {}",
                    add.len(),
                    a_s.numel
                ));
            }

            if m_s.ne[0] != x_s.ne[0] || a_s.ne[0] != x_s.ne[0] {
                return Err(format!(
                    "norm fuse ne0 mismatch: x={} mul={} add={}",
                    x_s.ne[0], m_s.ne[0], a_s.ne[0]
                ));
            }
            for d in 1..4 {
                if (m_s.ne[d] != 1 && m_s.ne[d] != x_s.ne[d])
                    || (a_s.ne[d] != 1 && a_s.ne[d] != x_s.ne[d])
                {
                    return Err("norm fuse broadcast mismatch".to_string());
                }
            }

            let x_bytes = unsafe {
                std::slice::from_raw_parts(
                    x.as_ptr() as *const u8,
                    x.len() * std::mem::size_of::<f32>(),
                )
            };
            let mul_bytes = unsafe {
                std::slice::from_raw_parts(
                    mul.as_ptr() as *const u8,
                    mul.len() * std::mem::size_of::<f32>(),
                )
            };
            let add_bytes = unsafe {
                std::slice::from_raw_parts(
                    add.as_ptr() as *const u8,
                    add.len() * std::mem::size_of::<f32>(),
                )
            };

            let x_buf = self.new_buffer_with_bytes(x_bytes)?;
            let mul_buf = self.new_buffer_with_bytes(mul_bytes)?;
            let add_buf = self.new_buffer_with_bytes(add_bytes)?;
            let dst_buf = self.new_buffer_with_length(x_s.numel * std::mem::size_of::<f32>())?;

            self.dispatch_norm_f32(
                x_buf.as_id(),
                mul_buf.as_id(),
                add_buf.as_id(),
                dst_buf.as_id(),
                &x_s,
                &m_s,
                &a_s,
                eps,
                3,
            )?;
            self.read_f32_buffer(dst_buf.as_id(), x_s.numel)
        }

        fn get_rows_ggml_bytes(
            &mut self,
            src: &[u8],
            src_ggml_type: u32,
            n_cols: usize,
            n_rows: usize,
            row_indices: &[i32],
        ) -> Result<Vec<f32>, String> {
            if row_indices.is_empty() {
                return Ok(Vec::new());
            }

            let src_ty = src0_type_from_ggml(src_ggml_type)
                .ok_or_else(|| format!("unsupported ggml type for get_rows: {}", src_ggml_type))?;
            let (row_bytes, elem_or_block) = src0_layout_bytes_per_row(src_ty, n_cols)?;
            let expected_src = n_rows
                .checked_mul(row_bytes)
                .ok_or_else(|| "overflow computing get_rows source bytes".to_string())?;
            if src.len() != expected_src {
                return Err(format!(
                    "get_rows source len mismatch: got {}, expected {}",
                    src.len(),
                    expected_src
                ));
            }

            for &row in row_indices {
                let row_ok = usize::try_from(row).ok().is_some_and(|row| row < n_rows);
                if !row_ok {
                    return Err(format!(
                        "get_rows row index {} is out of range {}",
                        row, n_rows
                    ));
                }
            }

            let src_shape = shape4_from_row_major(&[n_rows, n_cols], elem_or_block)?;
            let idx_shape =
                shape4_from_row_major(&[row_indices.len()], std::mem::size_of::<i32>() as u64)?;
            let dst_shape = shape4_from_row_major(&[row_indices.len(), n_cols], 4)?;

            let idx_bytes = unsafe {
                std::slice::from_raw_parts(
                    row_indices.as_ptr() as *const u8,
                    row_indices.len() * std::mem::size_of::<i32>(),
                )
            };

            let src_buf = self.new_buffer_with_bytes(src)?;
            let idx_buf = self.new_buffer_with_bytes(idx_bytes)?;
            let dst_buf =
                self.new_buffer_with_length(dst_shape.numel * std::mem::size_of::<f32>())?;
            self.dispatch_get_rows_ggml(
                src_ggml_type,
                src_buf.as_id(),
                &src_shape,
                idx_buf.as_id(),
                &idx_shape,
                dst_buf.as_id(),
                &dst_shape,
            )?;
            self.read_f32_buffer(dst_buf.as_id(), dst_shape.numel)
        }

        fn im2col_1d_f32(
            &mut self,
            input: &[f32],
            ic: usize,
            iw: usize,
            kw: usize,
            stride: usize,
            pad: usize,
        ) -> Result<Vec<f32>, String> {
            if kw == 0 || stride == 0 {
                return Err("im2col requires kw>0 and stride>0".to_string());
            }
            let expect = ic
                .checked_mul(iw)
                .ok_or_else(|| "overflow computing input size".to_string())?;
            if input.len() != expect {
                return Err(format!(
                    "im2col input len mismatch: got {}, expected {}",
                    input.len(),
                    expect
                ));
            }
            let num = iw
                .checked_add(pad.saturating_mul(2))
                .ok_or_else(|| "overflow computing im2col output numerator".to_string())?;
            if num < kw {
                return Ok(Vec::new());
            }
            let ow = (num - kw) / stride + 1;
            let chw = ic
                .checked_mul(kw)
                .ok_or_else(|| "overflow computing im2col CHW".to_string())?;
            let out_elems = ow
                .checked_mul(chw)
                .ok_or_else(|| "overflow computing im2col output size".to_string())?;

            let input_bytes = unsafe {
                std::slice::from_raw_parts(
                    input.as_ptr() as *const u8,
                    input.len() * std::mem::size_of::<f32>(),
                )
            };
            let src_buf = self.new_buffer_with_bytes(input_bytes)?;
            let dst_buf = self.new_buffer_with_length(out_elems * std::mem::size_of::<f32>())?;
            self.dispatch_im2col_1d_f32(
                src_buf.as_id(),
                dst_buf.as_id(),
                ic,
                iw,
                kw,
                stride,
                pad,
                ow,
            )?;
            self.read_f32_buffer(dst_buf.as_id(), out_elems)
        }

        fn matmul_nt_ggml_from_src1_buffer(
            &mut self,
            src1_id: ObjcId,
            bt_bytes: &[u8],
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
            weight_cache_tag: Option<u8>,
        ) -> Result<StrongId, String> {
            if m == 0 || k == 0 || n == 0 {
                return self
                    .new_buffer_with_length(m.saturating_mul(n) * std::mem::size_of::<f32>());
            }

            let src0 = src0_type_from_ggml(bt_ggml_type).ok_or_else(|| {
                format!(
                    "unsupported src0 ggml_type for metal matmul: {}",
                    bt_ggml_type
                )
            })?;
            let (src0_row_bytes, nb00) = src0_layout_bytes_per_row(src0, k)?;
            let expected_src0 = n
                .checked_mul(src0_row_bytes)
                .ok_or_else(|| "matmul overflow computing src0 bytes".to_string())?;
            if bt_bytes.len() != expected_src0 {
                return Err(format!(
                    "rhs len mismatch: got {}, expected {} (type {:?}, k={}, n={})",
                    bt_bytes.len(),
                    expected_src0,
                    src0,
                    k,
                    n
                ));
            }

            let ne00 = i32::try_from(k).map_err(|_| format!("k too large: {}", k))?;
            let ne01 = i32::try_from(n).map_err(|_| format!("n too large: {}", n))?;
            let ne10 = i32::try_from(k).map_err(|_| format!("k too large: {}", k))?;
            let ne11 = i32::try_from(m).map_err(|_| format!("m too large: {}", m))?;
            let ne0 = ne01;
            let ne1 = ne11;
            let nb01 = src0_row_bytes as u64;
            let nb10 = 4u64;
            let nb11 = (k as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing nb11".to_string())?;
            let mn = m
                .checked_mul(n)
                .ok_or_else(|| "matmul overflow computing m*n".to_string())?;

            let mut src0_temp = None;
            let src0_id = if let Some(tag) = weight_cache_tag {
                let key = BufferKey {
                    ptr: bt_bytes.as_ptr() as usize,
                    len: bt_bytes.len(),
                    tag,
                };
                self.get_or_create_weight_buffer(key, bt_bytes)?
            } else {
                let b = self.new_buffer_with_bytes(bt_bytes)?;
                let id = b.as_id();
                src0_temp = Some(b);
                id
            };

            let dst_bytes = mn
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "matmul overflow computing dst bytes".to_string())?;
            let mut dst_temp = None;
            let dst_id = if let Some(tag) = weight_cache_tag {
                self.get_or_create_matmul_out_buffer(tag, dst_bytes)?
            } else {
                let b = self.new_buffer_with_length(dst_bytes)?;
                let id = b.as_id();
                dst_temp = Some(b);
                id
            };
            let used_mul_mv_ext = can_use_mul_mv_ext(src0, ne00, ne11);
            let used_mul_mm = ne00 >= 64 && ne11 > 8;

            let (kernel, compute_res) = if used_mul_mv_ext {
                (
                    "mul_mv_ext",
                    self.dispatch_mul_mv_ext(
                        src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01, nb10,
                        nb11, ne0, ne1,
                    ),
                )
            } else if used_mul_mm {
                match self.dispatch_mul_mm(
                    src0, src0_id, src1_id, dst_id, ne00, ne01, nb01, 1, nb10, nb11, ne0, ne1,
                ) {
                    Ok(()) => ("mul_mm", Ok(())),
                    Err(e) => ("mul_mv", {
                        super::log_metal_error_once(format!(
                            "[ggml][metal] mul_mm failed for type {:?}, falling back to mul_mv: {}",
                            src0, e
                        ));
                        self.dispatch_mul_mv(
                            src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01,
                            nb10, nb11, ne0, ne1,
                        )
                    }),
                }
            } else {
                (
                    "mul_mv",
                    self.dispatch_mul_mv(
                        src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01, nb10,
                        nb11, ne0, ne1,
                    ),
                )
            };
            compute_res?;
            if super::log_mul_mat_requested() {
                eprintln!(
                    "[ggml][metal] mul_mat kernel={} src0={} src1=f32 ne00={} ne01={} ne11={} ne12=1 nb01={} nb11={}",
                    kernel,
                    src0_type_name(src0),
                    ne00,
                    ne01,
                    ne11,
                    nb01,
                    nb11
                );
            }
            drop(src0_temp);
            if let Some(dst_buffer) = dst_temp {
                Ok(dst_buffer)
            } else {
                unsafe { StrongId::from_unowned(dst_id) }
                    .ok_or_else(|| "matmul scratch output buffer returned nil".to_string())
            }
        }

        fn matmul_nt_ggml_bytes_impl(
            &mut self,
            a: &[f32],
            bt_bytes: &[u8],
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
            weight_cache_tag: Option<u8>,
        ) -> Result<(StrongId, usize, usize), String> {
            let mk = m
                .checked_mul(k)
                .ok_or_else(|| "matmul overflow computing m*k".to_string())?;
            if a.len() != mk {
                return Err(format!(
                    "lhs len mismatch: got {}, expected {}",
                    a.len(),
                    mk
                ));
            }

            let a_bytes = unsafe {
                std::slice::from_raw_parts(
                    a.as_ptr() as *const u8,
                    a.len() * std::mem::size_of::<f32>(),
                )
            };
            let src1_buffer = self.new_buffer_with_bytes(a_bytes)?;
            let dst_buffer = self.matmul_nt_ggml_from_src1_buffer(
                src1_buffer.as_id(),
                bt_bytes,
                bt_ggml_type,
                m,
                k,
                n,
                weight_cache_tag,
            )?;

            Ok((dst_buffer, m, n))
        }

        fn matmul_nn_f32(
            &mut self,
            a: &[f32],
            b: &[f32],
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<Vec<f32>, String> {
            let mk = m
                .checked_mul(k)
                .ok_or_else(|| "overflow computing m*k".to_string())?;
            let kn = k
                .checked_mul(n)
                .ok_or_else(|| "overflow computing k*n".to_string())?;
            if a.len() != mk {
                return Err(format!(
                    "lhs len mismatch: got {}, expected {}",
                    a.len(),
                    mk
                ));
            }
            if b.len() != kn {
                return Err(format!(
                    "rhs len mismatch: got {}, expected {}",
                    b.len(),
                    kn
                ));
            }

            let mut bt = vec![0.0f32; n * k];
            for i in 0..k {
                for j in 0..n {
                    bt[j * k + i] = b[i * n + j];
                }
            }

            let bt_bytes = unsafe {
                std::slice::from_raw_parts(
                    bt.as_ptr() as *const u8,
                    bt.len() * std::mem::size_of::<f32>(),
                )
            };

            let (dst, mr, nr) =
                self.matmul_nt_ggml_bytes_impl(a, bt_bytes, GGML_TYPE_F32, m, k, n, None)?;
            self.read_f32_buffer(dst.as_id(), mr * nr)
        }

        fn matmul_nt_f32(
            &mut self,
            a: &[f32],
            bt: &[f32],
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<Vec<f32>, String> {
            let bt_bytes = unsafe {
                std::slice::from_raw_parts(
                    bt.as_ptr() as *const u8,
                    bt.len() * std::mem::size_of::<f32>(),
                )
            };
            // Raw f32 RHS buffers are often transient activation vectors.
            // Pointer-based caching is unsafe here because allocators can reuse the same address
            // for different contents across layers or heads.
            let (dst, mr, nr) =
                self.matmul_nt_ggml_bytes_impl(a, bt_bytes, GGML_TYPE_F32, m, k, n, None)?;
            self.read_f32_buffer(dst.as_id(), mr * nr)
        }

        fn matmul_nt_f32_bytes(
            &mut self,
            a: &[f32],
            bt_bytes: &[u8],
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<Vec<f32>, String> {
            let (dst, mr, nr) =
                self.matmul_nt_ggml_bytes_impl(a, bt_bytes, GGML_TYPE_F32, m, k, n, None)?;
            self.read_f32_buffer(dst.as_id(), mr * nr)
        }

        fn matmul_nt_f16_bytes(
            &mut self,
            a: &[f32],
            bt_f16_bytes: &[u8],
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<Vec<f32>, String> {
            let (dst, mr, nr) =
                self.matmul_nt_ggml_bytes_impl(a, bt_f16_bytes, GGML_TYPE_F16, m, k, n, None)?;
            self.read_f32_buffer(dst.as_id(), mr * nr)
        }

        fn matmul_nt_ggml_bytes(
            &mut self,
            a: &[f32],
            bt_bytes: &[u8],
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<Vec<f32>, String> {
            let tag = matmul_cache_tag(bt_ggml_type);
            let (dst, mr, nr) =
                self.matmul_nt_ggml_bytes_impl(a, bt_bytes, bt_ggml_type, m, k, n, Some(tag))?;
            self.read_f32_buffer(dst.as_id(), mr * nr)
        }

        fn matmul_nt_ggml_from_resident_src(
            &mut self,
            src0_id: ObjcId,
            src1_id: ObjcId,
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<StrongId, String> {
            let mn = m
                .checked_mul(n)
                .ok_or_else(|| "matmul overflow computing m*n".to_string())?;
            let dst_bytes = mn
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "matmul overflow computing dst bytes".to_string())?;
            let dst_buffer = self.pool_take(dst_bytes)?;
            if m == 0 || k == 0 || n == 0 {
                return Ok(dst_buffer);
            }
            self.matmul_nt_ggml_resident_into(
                src0_id,
                src1_id,
                dst_buffer.as_id(),
                bt_ggml_type,
                m,
                k,
                n,
            )?;
            Ok(dst_buffer)
        }

        /// Same dispatch as [`Self::matmul_nt_ggml_from_resident_src`] but the
        /// caller owns the destination buffer (act pool / recycled transient).
        fn matmul_nt_ggml_resident_into(
            &mut self,
            src0_id: ObjcId,
            src1_id: ObjcId,
            dst_id: ObjcId,
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
        ) -> Result<(), String> {
            if m == 0 || k == 0 || n == 0 {
                return Ok(());
            }
            let src0 = src0_type_from_ggml(bt_ggml_type).ok_or_else(|| {
                format!(
                    "unsupported src0 ggml_type for metal matmul: {}",
                    bt_ggml_type
                )
            })?;
            let (src0_row_bytes, nb00) = src0_layout_bytes_per_row(src0, k)?;
            let ne00 = i32::try_from(k).map_err(|_| format!("k too large: {}", k))?;
            let ne01 = i32::try_from(n).map_err(|_| format!("n too large: {}", n))?;
            let ne10 = ne00;
            let ne11 = i32::try_from(m).map_err(|_| format!("m too large: {}", m))?;
            let ne0 = ne01;
            let ne1 = ne11;
            let nb01 = src0_row_bytes as u64;
            let nb10 = 4u64;
            let nb11 = (k as u64)
                .checked_mul(4)
                .ok_or_else(|| "overflow computing nb11".to_string())?;
            let used_mul_mv_ext = can_use_mul_mv_ext(src0, ne00, ne11);
            let used_mul_mm = ne00 >= 64 && ne11 > 8;
            if used_mul_mv_ext {
                self.dispatch_mul_mv_ext(
                    src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01, nb10,
                    nb11, ne0, ne1,
                )?;
            } else if used_mul_mm {
                if let Err(e) = self.dispatch_mul_mm(
                    src0, src0_id, src1_id, dst_id, ne00, ne01, nb01, 1, nb10, nb11, ne0, ne1,
                ) {
                    super::log_metal_error_once(format!(
                        "[ggml][metal] mul_mm failed for type {:?}, falling back to mul_mv: {}",
                        src0, e
                    ));
                    self.dispatch_mul_mv(
                        src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01,
                        nb10, nb11, ne0, ne1,
                    )?;
                }
            } else {
                self.dispatch_mul_mv(
                    src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01, nb10,
                    nb11, ne0, ne1,
                )?;
            }
            Ok(())
        }

        fn matmul_nt_ggml_bytes_keyed<F>(
            &mut self,
            a: &[f32],
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
            namespace: &str,
            cache_key: &str,
            load: F,
        ) -> Result<Vec<f32>, String>
        where
            F: FnOnce() -> Result<Vec<u8>, String>,
        {
            let src0_id = self.get_or_create_named_weight_buffer(namespace, cache_key, load)?;
            let mk = m
                .checked_mul(k)
                .ok_or_else(|| "matmul overflow computing m*k".to_string())?;
            if a.len() != mk {
                return Err(format!(
                    "lhs len mismatch: got {}, expected {}",
                    a.len(),
                    mk
                ));
            }
            let a_bytes = unsafe {
                std::slice::from_raw_parts(
                    a.as_ptr() as *const u8,
                    a.len() * std::mem::size_of::<f32>(),
                )
            };
            let src1_buffer = self.pool_take_filled(a_bytes)?;
            let dst = self.matmul_nt_ggml_from_resident_src(
                src0_id,
                src1_buffer.as_id(),
                bt_ggml_type,
                m,
                k,
                n,
            )?;
            let out = self.read_f32_buffer(dst.as_id(), m * n)?;
            self.pool_give(src1_buffer);
            self.pool_give(dst);
            self.pool_recycle();
            Ok(out)
        }

        fn matmul_nt_ggml_bytes_keyed_multi<F>(
            &mut self,
            a: &[f32],
            m: usize,
            k: usize,
            matrices: &[super::MatmulNtGgmlBytesKeyedMatrix<'_>],
            mut load: F,
        ) -> Result<Vec<Vec<f32>>, String>
        where
            F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
        {
            if matrices.is_empty() {
                return Ok(Vec::new());
            }
            let mk = m
                .checked_mul(k)
                .ok_or_else(|| "matmul overflow computing m*k".to_string())?;
            if a.len() != mk {
                return Err(format!(
                    "lhs len mismatch: got {}, expected {}",
                    a.len(),
                    mk
                ));
            }
            let a_bytes = unsafe {
                std::slice::from_raw_parts(
                    a.as_ptr() as *const u8,
                    a.len() * std::mem::size_of::<f32>(),
                )
            };
            let src1_buffer = self.pool_take_filled(a_bytes)?;
            let outputs = self.with_batch(|this| {
                let mut outputs = Vec::with_capacity(matrices.len());
                for matrix in matrices {
                    let src0_id = this.get_or_create_named_weight_buffer(
                        matrix.namespace,
                        matrix.cache_key,
                        || load(matrix.namespace, matrix.cache_key),
                    )?;
                    let dst = this.matmul_nt_ggml_from_resident_src(
                        src0_id,
                        src1_buffer.as_id(),
                        matrix.bt_ggml_type,
                        m,
                        k,
                        matrix.n,
                    )?;
                    outputs.push((dst, matrix.n));
                }
                Ok(outputs)
            })?;
            self.wait_queue_idle()?;
            let mut result = Vec::with_capacity(outputs.len());
            for (buffer, n_out) in outputs {
                result.push(self.copy_f32_buffer_contents_readable(buffer.as_id(), m * n_out)?);
                self.pool_give(buffer);
            }
            self.pool_give(src1_buffer);
            self.pool_recycle();
            Ok(result)
        }

        fn act_buf(&mut self, name: &str, elems: usize) -> Result<ObjcId, String> {
            let bytes = elems
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "act buffer overflow".to_string())?;
            if let Some((buf, cap)) = self.act_buffers.get(name) {
                if *cap >= bytes {
                    return Ok(buf.as_id());
                }
            }
            let buf = self.new_buffer_with_length(bytes.max(4))?;
            let id = buf.as_id();
            self.act_buffers.insert(name.to_string(), (buf, bytes));
            Ok(id)
        }

        fn act_write(&mut self, name: &str, data: &[f32]) -> Result<ObjcId, String> {
            let id = self.act_buf(name, data.len())?;
            let ptr: *mut c_void = unsafe { msg_send![id, contents] };
            if ptr.is_null() {
                return Err(format!("act {name} contents null"));
            }
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut f32, data.len());
            }
            Ok(id)
        }

        fn act_read(&self, name: &str, elems: usize) -> Result<Vec<f32>, String> {
            let (buf, cap) = self
                .act_buffers
                .get(name)
                .ok_or_else(|| format!("act {name} missing"))?;
            let need = elems
                .checked_mul(std::mem::size_of::<f32>())
                .ok_or_else(|| "act read overflow".to_string())?;
            if need > *cap {
                return Err(format!("act {name} read {need} > cap {cap}"));
            }
            self.copy_f32_buffer_contents_readable(buf.as_id(), elems)
        }

        fn act_norm_gamma(&mut self, key: &str, gamma: &[f32]) -> Result<ObjcId, String> {
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    gamma.as_ptr() as *const u8,
                    gamma.len() * std::mem::size_of::<f32>(),
                )
            };
            self.get_or_create_named_weight_buffer("music3-act-norm", key, || Ok(bytes.to_vec()))
        }

        fn act_rms_norm_mul(
            &mut self,
            src: ObjcId,
            dst: ObjcId,
            gamma: ObjcId,
            rows: usize,
            width: usize,
            eps: f32,
        ) -> Result<(), String> {
            let x_s = shape4_from_row_major(&[rows, width], 4)?;
            let m_s = shape4_from_row_major(&[width], 4)?;
            self.dispatch_rms_norm_f32(src, gamma, src, dst, &x_s, &m_s, &x_s, eps, 2)
        }

        fn act_add_inplace(
            &mut self,
            a: ObjcId,
            b: ObjcId,
            dst: ObjcId,
            elems: usize,
        ) -> Result<(), String> {
            let s = shape4_from_row_major(&[elems], 4)?;
            self.dispatch_bin_f32(0, a, b, dst, &s, &s)
        }

        fn act_silu(&mut self, src: ObjcId, dst: ObjcId, elems: usize) -> Result<(), String> {
            let s = shape4_from_row_major(&[elems], 4)?;
            self.dispatch_unary_f32(OP_UNARY_NUM_SILU, src, dst, &s)
        }

        fn act_mul(&mut self, a: ObjcId, b: ObjcId, dst: ObjcId, elems: usize) -> Result<(), String> {
            let s = shape4_from_row_major(&[elems], 4)?;
            self.dispatch_bin_f32(2, a, b, dst, &s, &s)
        }

        fn act_linear_keyed<F>(
            &mut self,
            src1: ObjcId,
            dst_name: &str,
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
            namespace: &str,
            cache_key: &str,
            load: F,
        ) -> Result<ObjcId, String>
        where
            F: FnOnce() -> Result<Vec<u8>, String>,
        {
            let src0 = self.get_or_create_named_weight_buffer(namespace, cache_key, load)?;
            let elems = m
                .checked_mul(n)
                .ok_or_else(|| "act linear overflow".to_string())?;
            let dst = self.act_buf(dst_name, elems)?;
            self.matmul_nt_ggml_resident_into(src0, src1, dst, bt_ggml_type, m, k, n)?;
            Ok(dst)
        }

        fn ar_pre_attn<F>(
            &mut self,
            hidden: Option<&[f32]>,
            m: usize,
            hidden_w: usize,
            head_dim: usize,
            in_norm: &[f32],
            qk_norm: Option<(&[f32], &[f32], &str, &str)>,
            in_norm_key: &str,
            eps: f32,
            q: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
            k: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
            v: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
            mut load: F,
        ) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>), String>
        where
            F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
        {
            let h_elems = m * hidden_w;
            if let Some(h) = hidden {
                if h.len() != h_elems {
                    return Err("ar hidden len".to_string());
                }
                self.act_write("h", h)?;
            }
            let q_elems = m * q.n;
            let k_elems = m * k.n;
            let v_elems = m * v.n;
            self.with_batch(|this| {
                let h = this.act_buf("h", h_elems)?;
                let n1 = this.act_buf("n1", h_elems)?;
                let gamma = this.act_norm_gamma(in_norm_key, in_norm)?;
                this.act_rms_norm_mul(h, n1, gamma, m, hidden_w, eps)?;
                this.act_linear_keyed(
                    n1,
                    "q",
                    q.bt_ggml_type,
                    m,
                    hidden_w,
                    q.n,
                    q.namespace,
                    q.cache_key,
                    || load(q.namespace, q.cache_key),
                )?;
                this.act_linear_keyed(
                    n1,
                    "k",
                    k.bt_ggml_type,
                    m,
                    hidden_w,
                    k.n,
                    k.namespace,
                    k.cache_key,
                    || load(k.namespace, k.cache_key),
                )?;
                this.act_linear_keyed(
                    n1,
                    "v",
                    v.bt_ggml_type,
                    m,
                    hidden_w,
                    v.n,
                    v.namespace,
                    v.cache_key,
                    || load(v.namespace, v.cache_key),
                )?;
                if let Some((q_norm, k_norm, q_norm_key, k_norm_key)) = qk_norm {
                    let q_id = this.act_buf("q", q_elems)?;
                    let k_id = this.act_buf("k", k_elems)?;
                    let qn = this.act_buf("qn", q_elems)?;
                    let kn = this.act_buf("kn", k_elems)?;
                    let qg = this.act_norm_gamma(q_norm_key, q_norm)?;
                    let kg = this.act_norm_gamma(k_norm_key, k_norm)?;
                    this.act_rms_norm_mul(q_id, qn, qg, q_elems / head_dim, head_dim, eps)?;
                    this.act_rms_norm_mul(k_id, kn, kg, k_elems / head_dim, head_dim, eps)?;
                }
                Ok(())
            })?;
            self.wait_queue_idle()?;
            let (q_src, k_src) = if qk_norm.is_some() {
                ("qn", "kn")
            } else {
                ("q", "k")
            };
            let qv = self.act_read(q_src, q_elems)?;
            let kv = self.act_read(k_src, k_elems)?;
            let vv = self.act_read("v", v_elems)?;
            Ok((qv, kv, vv))
        }

        fn ar_post_attn<F>(
            &mut self,
            attn: &[f32],
            m: usize,
            hidden_w: usize,
            post_norm: &[f32],
            post_norm_key: &str,
            eps: f32,
            o: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
            up: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
            gate: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
            down: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
            mut load: F,
        ) -> Result<(), String>
        where
            F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
        {
            let h_elems = m * hidden_w;
            let ff_elems = m * up.n;
            if attn.len() != h_elems {
                return Err("ar attn len".to_string());
            }
            self.act_write("attn", attn)?;
            self.with_batch(|this| {
                let attn_id = this.act_buf("attn", h_elems)?;
                this.act_linear_keyed(
                    attn_id,
                    "o",
                    o.bt_ggml_type,
                    m,
                    hidden_w,
                    o.n,
                    o.namespace,
                    o.cache_key,
                    || load(o.namespace, o.cache_key),
                )?;
                let h = this.act_buf("h", h_elems)?;
                let o_id = this.act_buf("o", h_elems)?;
                let h2 = this.act_buf("h2", h_elems)?;
                this.act_add_inplace(h, o_id, h2, h_elems)?;
                let n2 = this.act_buf("n2", h_elems)?;
                let pg = this.act_norm_gamma(post_norm_key, post_norm)?;
                this.act_rms_norm_mul(h2, n2, pg, m, hidden_w, eps)?;
                this.act_linear_keyed(
                    n2,
                    "up",
                    up.bt_ggml_type,
                    m,
                    hidden_w,
                    up.n,
                    up.namespace,
                    up.cache_key,
                    || load(up.namespace, up.cache_key),
                )?;
                this.act_linear_keyed(
                    n2,
                    "gate",
                    gate.bt_ggml_type,
                    m,
                    hidden_w,
                    gate.n,
                    gate.namespace,
                    gate.cache_key,
                    || load(gate.namespace, gate.cache_key),
                )?;
                let up_id = this.act_buf("up", ff_elems)?;
                let gate_id = this.act_buf("gate", ff_elems)?;
                let gs = this.act_buf("gs", ff_elems)?;
                let ff = this.act_buf("ff", ff_elems)?;
                this.act_silu(gate_id, gs, ff_elems)?;
                this.act_mul(up_id, gs, ff, ff_elems)?;
                this.act_linear_keyed(
                    ff,
                    "down",
                    down.bt_ggml_type,
                    m,
                    up.n,
                    down.n,
                    down.namespace,
                    down.cache_key,
                    || load(down.namespace, down.cache_key),
                )?;
                let down_id = this.act_buf("down", h_elems)?;
                let h3 = this.act_buf("h", h_elems)?;
                this.act_add_inplace(h2, down_id, h3, h_elems)?;
                Ok(())
            })?;
            Ok(())
        }

        fn ar_final_rms(
            &mut self,
            m: usize,
            hidden_w: usize,
            gamma: &[f32],
            gamma_key: &str,
            eps: f32,
        ) -> Result<Vec<f32>, String> {
            let h_elems = m * hidden_w;
            self.with_batch(|this| {
                let h = this.act_buf("h", h_elems)?;
                let out = this.act_buf("hout", h_elems)?;
                let g = this.act_norm_gamma(gamma_key, gamma)?;
                this.act_rms_norm_mul(h, out, g, m, hidden_w, eps)?;
                Ok(())
            })?;
            self.wait_queue_idle()?;
            self.act_read("hout", h_elems)
        }

        fn ar_clear_acts(&mut self) {
            self.act_buffers.clear();
            self.pool_clear();
        }

        fn dit_ffn_resident<F>(
            &mut self,
            normed: &[f32],
            m: usize,
            hidden_w: usize,
            ff_dim: usize,
            ff_in_b: &[f32],
            ff_out_b: &[f32],
            ff_in_b_key: &str,
            ff_out_b_key: &str,
            swap: bool,
            ff_in: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
            ff_out: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
            mut load: F,
        ) -> Result<Vec<f32>, String>
        where
            F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
        {
            if normed.len() != m * hidden_w {
                return Err("dit ffn normed len".to_string());
            }
            if ff_in.n != ff_dim * 2 || ff_out.n != hidden_w {
                return Err("dit ffn weight n".to_string());
            }
            self.act_write("dn", normed)?;
            self.with_batch(|this| {
                let dn = this.act_buf("dn", m * hidden_w)?;
                this.act_linear_keyed(
                    dn,
                    "ffin",
                    ff_in.bt_ggml_type,
                    m,
                    hidden_w,
                    ff_in.n,
                    ff_in.namespace,
                    ff_in.cache_key,
                    || load(ff_in.namespace, ff_in.cache_key),
                )?;
                let ffin = this.act_buf("ffin", m * ff_in.n)?;
                let in_b = this.act_norm_gamma(ff_in_b_key, ff_in_b)?;
                let ffinb = this.act_buf("ffinb", m * ff_in.n)?;
                let x_s = shape4_from_row_major(&[m, ff_in.n], 4)?;
                let b_s = shape4_from_row_major(&[ff_in.n], 4)?;
                this.dispatch_bin_f32(0, ffin, in_b, ffinb, &x_s, &b_s)?;
                let gated = this.act_buf("gated", m * ff_dim)?;
                this.dispatch_swiglu_packed(ffinb, gated, m, ff_dim, swap)?;
                this.act_linear_keyed(
                    gated,
                    "ffout",
                    ff_out.bt_ggml_type,
                    m,
                    ff_dim,
                    ff_out.n,
                    ff_out.namespace,
                    ff_out.cache_key,
                    || load(ff_out.namespace, ff_out.cache_key),
                )?;
                let ffout = this.act_buf("ffout", m * hidden_w)?;
                let out_b = this.act_norm_gamma(ff_out_b_key, ff_out_b)?;
                let y = this.act_buf("ffy", m * hidden_w)?;
                let y_s = shape4_from_row_major(&[m, hidden_w], 4)?;
                let ob_s = shape4_from_row_major(&[hidden_w], 4)?;
                this.dispatch_bin_f32(0, ffout, out_b, y, &y_s, &ob_s)?;
                Ok(())
            })?;
            self.wait_queue_idle()?;
            self.act_read("ffy", m * hidden_w)
        }

        fn matmul_nt_ggml_bytes_multi(
            &mut self,
            a: &[f32],
            m: usize,
            k: usize,
            matrices: &[super::MatmulNtGgmlBytesMatrix<'_>],
        ) -> Result<Vec<Vec<f32>>, String> {
            if matrices.is_empty() {
                return Ok(Vec::new());
            }
            let mk = m
                .checked_mul(k)
                .ok_or_else(|| "matmul overflow computing m*k".to_string())?;
            if a.len() != mk {
                return Err(format!(
                    "lhs len mismatch: got {}, expected {}",
                    a.len(),
                    mk
                ));
            }

            let a_bytes = unsafe {
                std::slice::from_raw_parts(
                    a.as_ptr() as *const u8,
                    a.len() * std::mem::size_of::<f32>(),
                )
            };
            let src1_buffer = self.new_buffer_with_bytes(a_bytes)?;
            let outputs = self.with_batch(|this| {
                let mut outputs = Vec::with_capacity(matrices.len());
                for (index, matrix) in matrices.iter().enumerate() {
                    let tag = matmul_batch_tag(matrix.bt_ggml_type, index)?;
                    let dst = this.matmul_nt_ggml_from_src1_buffer(
                        src1_buffer.as_id(),
                        matrix.bt_bytes,
                        matrix.bt_ggml_type,
                        m,
                        k,
                        matrix.n,
                        Some(tag),
                    )?;
                    outputs.push((dst, matrix.n));
                }
                Ok(outputs)
            })?;
            self.wait_queue_idle()?;
            let mut result = Vec::with_capacity(outputs.len());
            for (buffer, n_out) in outputs {
                result.push(self.copy_f32_buffer_contents_readable(buffer.as_id(), m * n_out)?);
            }
            Ok(result)
        }

        fn vision_mlp_bf16_fused(
            &mut self,
            x: &[f32],
            gate_up_weight_bytes: &[u8],
            down_weight_bytes: &[u8],
            rows: usize,
            hidden_size: usize,
            intermediate_size: usize,
        ) -> Result<Vec<f32>, String> {
            let expected_x = rows
                .checked_mul(hidden_size)
                .ok_or_else(|| "overflow computing fused vision mlp input size".to_string())?;
            if x.len() != expected_x {
                return Err(format!(
                    "fused vision mlp input len mismatch: got {}, expected {}",
                    x.len(),
                    expected_x
                ));
            }
            let expected_gate_up_bytes = (intermediate_size * 2)
                .checked_mul(hidden_size)
                .and_then(|elems| elems.checked_mul(std::mem::size_of::<u16>()))
                .ok_or_else(|| "overflow computing fused vision mlp gate_up bytes".to_string())?;
            if gate_up_weight_bytes.len() != expected_gate_up_bytes {
                return Err(format!(
                    "fused vision mlp gate_up len mismatch: got {}, expected {}",
                    gate_up_weight_bytes.len(),
                    expected_gate_up_bytes
                ));
            }
            let expected_down_bytes = hidden_size
                .checked_mul(intermediate_size)
                .and_then(|elems| elems.checked_mul(std::mem::size_of::<u16>()))
                .ok_or_else(|| "overflow computing fused vision mlp down bytes".to_string())?;
            if down_weight_bytes.len() != expected_down_bytes {
                return Err(format!(
                    "fused vision mlp down len mismatch: got {}, expected {}",
                    down_weight_bytes.len(),
                    expected_down_bytes
                ));
            }

            let x_bytes = unsafe {
                std::slice::from_raw_parts(
                    x.as_ptr() as *const u8,
                    x.len() * std::mem::size_of::<f32>(),
                )
            };
            let src1_buffer = self.new_buffer_with_bytes(x_bytes)?;
            let geglu_bytes = rows
                .checked_mul(intermediate_size)
                .and_then(|elems| elems.checked_mul(std::mem::size_of::<f32>()))
                .ok_or_else(|| "overflow computing fused vision mlp geglu bytes".to_string())?;

            let down_buffer = self.with_batch(|this| {
                let gate_up_buffer = this.matmul_nt_ggml_from_src1_buffer(
                    src1_buffer.as_id(),
                    gate_up_weight_bytes,
                    GGML_TYPE_BF16,
                    rows,
                    hidden_size,
                    intermediate_size * 2,
                    Some(31u8),
                )?;
                let geglu_buffer = this.get_or_create_matmul_out_buffer(32u8, geglu_bytes)?;
                this.dispatch_geglu_strided_rows_f32(
                    gate_up_buffer.as_id(),
                    geglu_buffer,
                    rows,
                    intermediate_size,
                    intermediate_size * 2,
                    intermediate_size,
                )?;
                this.matmul_nt_ggml_from_src1_buffer(
                    geglu_buffer,
                    down_weight_bytes,
                    GGML_TYPE_BF16,
                    rows,
                    intermediate_size,
                    hidden_size,
                    Some(33u8),
                )
            })?;
            self.read_f32_buffer(down_buffer.as_id(), rows * hidden_size)
        }

        fn matmul_nt_ggml_bytes_add_bias(
            &mut self,
            a: &[f32],
            bt_bytes: &[u8],
            bt_ggml_type: u32,
            m: usize,
            k: usize,
            n: usize,
            bias: &[f32],
        ) -> Result<Vec<f32>, String> {
            if bias.len() != n {
                return Err(format!(
                    "bias len mismatch for matmul+add: got {}, expected {}",
                    bias.len(),
                    n
                ));
            }

            let tag = match bt_ggml_type {
                GGML_TYPE_F32 => 2u8,
                GGML_TYPE_F16 => 3u8,
                GGML_TYPE_BF16 => 9u8,
                GGML_TYPE_Q4_0 => 4u8,
                GGML_TYPE_Q4_1 => 5u8,
                GGML_TYPE_Q5_0 => 6u8,
                GGML_TYPE_Q5_1 => 7u8,
                GGML_TYPE_Q8_0 => 8u8,
                GGML_TYPE_Q2_K => 10u8,
                GGML_TYPE_Q3_K => 11u8,
                GGML_TYPE_Q4_K => 12u8,
                GGML_TYPE_Q5_K => 13u8,
                GGML_TYPE_Q6_K => 14u8,
                _ => 0u8,
            };

            let (dst, mr, nr) =
                self.matmul_nt_ggml_bytes_impl(a, bt_bytes, bt_ggml_type, m, k, n, Some(tag))?;

            let bias_shape = shape4_from_row_major(&[n], 4)?;
            let dst_shape = shape4_from_row_major(&[m, n], 4)?;
            let bias_bytes = unsafe {
                std::slice::from_raw_parts(
                    bias.as_ptr() as *const u8,
                    bias.len() * std::mem::size_of::<f32>(),
                )
            };
            let bias_buf = self.new_buffer_with_bytes(bias_bytes)?;
            self.dispatch_bin_f32(
                0,
                dst.as_id(),
                bias_buf.as_id(),
                dst.as_id(),
                &dst_shape,
                &bias_shape,
            )?;

            self.read_f32_buffer(dst.as_id(), mr * nr)
        }
    }

    fn with_context<T>(f: impl FnOnce(&mut MetalContext) -> Result<T, String>) -> Option<T> {
        enum ContextState {
            Uninitialized,
            Disabled,
            Ready(MetalContext),
        }

        thread_local! {
            static CONTEXT: RefCell<ContextState> = const { RefCell::new(ContextState::Uninitialized) };
        }

        CONTEXT.with(|ctx| {
            let mut ctx = ctx.borrow_mut();

            if matches!(&*ctx, ContextState::Uninitialized) {
                *ctx = match MetalContext::new() {
                    Ok(created) => ContextState::Ready(created),
                    Err(err) => {
                        eprintln!("[ggml][metal] backend disabled: {}", err);
                        ContextState::Disabled
                    }
                };
            }

            let ctx = match &mut *ctx {
                ContextState::Ready(ctx) => ctx,
                ContextState::Disabled | ContextState::Uninitialized => return None,
            };
            match f(ctx) {
                Ok(v) => Some(v),
                Err(err) => {
                    super::log_metal_error_once(format!(
                        "[ggml][metal] compute failed: {}",
                        err
                    ));
                    None
                }
            }
        })
    }

    pub(super) fn try_matmul_nn_f32(
        a: &[f32],
        b: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.matmul_nn_f32(a, b, m, k, n))
    }

    pub(super) fn try_matmul_nt_f32(
        a: &[f32],
        bt: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.matmul_nt_f32(a, bt, m, k, n))
    }

    pub(super) fn try_matmul_nt_f32_bytes(
        a: &[f32],
        bt_bytes: &[u8],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.matmul_nt_f32_bytes(a, bt_bytes, m, k, n))
    }

    pub(super) fn try_matmul_nt_f16_bytes(
        a: &[f32],
        bt_f16_bytes: &[u8],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.matmul_nt_f16_bytes(a, bt_f16_bytes, m, k, n))
    }

    pub(super) fn try_matmul_nt_ggml_bytes(
        a: &[f32],
        bt_bytes: &[u8],
        bt_ggml_type: u32,
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.matmul_nt_ggml_bytes(a, bt_bytes, bt_ggml_type, m, k, n))
    }

    pub(super) fn try_matmul_nt_ggml_bytes_keyed<F>(
        a: &[f32],
        bt_ggml_type: u32,
        m: usize,
        k: usize,
        n: usize,
        namespace: &str,
        cache_key: &str,
        load: F,
    ) -> Option<Vec<f32>>
    where
        F: FnOnce() -> Result<Vec<u8>, String>,
    {
        with_context(|ctx| {
            ctx.matmul_nt_ggml_bytes_keyed(a, bt_ggml_type, m, k, n, namespace, cache_key, load)
        })
    }

    pub(super) fn try_matmul_nt_ggml_bytes_multi(
        a: &[f32],
        m: usize,
        k: usize,
        matrices: &[super::MatmulNtGgmlBytesMatrix<'_>],
    ) -> Option<Vec<Vec<f32>>> {
        with_context(|ctx| ctx.matmul_nt_ggml_bytes_multi(a, m, k, matrices))
    }

    pub(super) fn try_matmul_nt_ggml_bytes_keyed_multi<F>(
        a: &[f32],
        m: usize,
        k: usize,
        matrices: &[super::MatmulNtGgmlBytesKeyedMatrix<'_>],
        load: F,
    ) -> Option<Vec<Vec<f32>>>
    where
        F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
    {
        with_context(|ctx| ctx.matmul_nt_ggml_bytes_keyed_multi(a, m, k, matrices, load))
    }

    pub(super) fn try_ar_pre_attn<F>(
        hidden: Option<&[f32]>,
        m: usize,
        hidden_w: usize,
        head_dim: usize,
        in_norm: &[f32],
        qk_norm: Option<(&[f32], &[f32], &str, &str)>,
        in_norm_key: &str,
        eps: f32,
        q: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        k: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        v: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        load: F,
    ) -> Option<(Vec<f32>, Vec<f32>, Vec<f32>)>
    where
        F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
    {
        with_context(|ctx| {
            ctx.ar_pre_attn(
                hidden,
                m,
                hidden_w,
                head_dim,
                in_norm,
                qk_norm,
                in_norm_key,
                eps,
                q,
                k,
                v,
                load,
            )
        })
    }

    pub(super) fn try_ar_post_attn<F>(
        attn: &[f32],
        m: usize,
        hidden_w: usize,
        post_norm: &[f32],
        post_norm_key: &str,
        eps: f32,
        o: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        up: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        gate: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        down: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        load: F,
    ) -> Option<()>
    where
        F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
    {
        with_context(|ctx| {
            ctx.ar_post_attn(
                attn,
                m,
                hidden_w,
                post_norm,
                post_norm_key,
                eps,
                o,
                up,
                gate,
                down,
                load,
            )
        })
    }

    pub(super) fn try_ar_final_rms(
        m: usize,
        hidden_w: usize,
        gamma: &[f32],
        gamma_key: &str,
        eps: f32,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.ar_final_rms(m, hidden_w, gamma, gamma_key, eps))
    }

    pub(super) fn ar_resident_clear() {
        let _ = with_context(|ctx| {
            ctx.ar_clear_acts();
            Ok(())
        });
    }

    pub(super) fn transient_pool_clear() {
        let _ = with_context(|ctx| {
            ctx.pool_clear();
            Ok(())
        });
    }

    pub(super) fn try_dit_ffn_resident<F>(
        normed: &[f32],
        m: usize,
        hidden_w: usize,
        ff_dim: usize,
        ff_in_b: &[f32],
        ff_out_b: &[f32],
        ff_in_b_key: &str,
        ff_out_b_key: &str,
        swap: bool,
        ff_in: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        ff_out: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        load: F,
    ) -> Option<Vec<f32>>
    where
        F: FnMut(&str, &str) -> Result<Vec<u8>, String>,
    {
        with_context(|ctx| {
            ctx.dit_ffn_resident(
                normed,
                m,
                hidden_w,
                ff_dim,
                ff_in_b,
                ff_out_b,
                ff_in_b_key,
                ff_out_b_key,
                swap,
                ff_in,
                ff_out,
                load,
            )
        })
    }

    pub(super) fn try_matmul_nt_ggml_bytes_add_bias(
        a: &[f32],
        bt_bytes: &[u8],
        bt_ggml_type: u32,
        m: usize,
        k: usize,
        n: usize,
        bias: &[f32],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.matmul_nt_ggml_bytes_add_bias(a, bt_bytes, bt_ggml_type, m, k, n, bias)
        })
    }

    pub(super) fn try_vision_mlp_bf16_fused(
        x: &[f32],
        gate_up_weight_bytes: &[u8],
        down_weight_bytes: &[u8],
        rows: usize,
        hidden_size: usize,
        intermediate_size: usize,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.vision_mlp_bf16_fused(
                x,
                gate_up_weight_bytes,
                down_weight_bytes,
                rows,
                hidden_size,
                intermediate_size,
            )
        })
    }

    pub(super) fn try_flash_attn_f32_packed(
        q: &[f32],
        k: &[f32],
        v: &[f32],
        n_q: usize,
        n_kv: usize,
        n_head: usize,
        d: usize,
        scale: f32,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.flash_attn_f32_packed(q, k, v, n_q, n_kv, n_head, d, scale))
    }

    pub(super) fn clear_decoder_kv_cache() {
        let _ = with_context(|ctx| {
            ctx.clear_decoder_kv_cache();
            Ok(())
        });
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_vit_backbone_resident_f32(
        x: &[f32],
        seq_len: usize,
        n_state: usize,
        n_head: usize,
        rot_half: usize,
        cos: &[f32],
        sin: &[f32],
        layers: &[super::VitLayerRef<'_>],
        final_norm_w: &[f32],
        final_norm_b: &[f32],
        eps: f32,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.vit_backbone_resident_f32(
                x, seq_len, n_state, n_head, rot_half, cos, sin, layers, final_norm_w,
                final_norm_b, eps,
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_two_way_layer_resident_f32(
        hidden: &[f32],
        token_pe: &[f32],
        context: &[f32],
        context_pe: &[f32],
        n_tok: usize,
        dim: usize,
        n_ctx: usize,
        ctx_dim: usize,
        layer: &super::TwoWayLayerRef<'_>,
    ) -> Option<(Vec<f32>, Vec<f32>)> {
        with_context(|ctx| {
            ctx.two_way_layer_resident_f32(
                hidden, token_pe, context, context_pe, n_tok, dim, n_ctx, ctx_dim, layer,
            )
        })
    }

    pub(super) fn try_flash_attn_f32_self_kv_cache(
        layer: usize,
        q: &[f32],
        k_all: &[f32],
        v_all: &[f32],
        n_kv: usize,
        n_head: usize,
        d: usize,
        scale: f32,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.flash_attn_f32_self_kv_cache(layer, q, k_all, v_all, n_kv, n_head, d, scale)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_flash_attn_f32_cross_kv_cache(
        layer: usize,
        q: &[f32],
        k_cross: &[f32],
        v_cross: &[f32],
        n_q: usize,
        n_kv: usize,
        n_head: usize,
        d: usize,
        scale: f32,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.flash_attn_f32_cross_kv_cache(
                layer, q, k_cross, v_cross, n_q, n_kv, n_head, d, scale,
            )
        })
    }

    pub(super) fn try_add_f32(
        a: &[f32],
        a_shape: &[usize],
        b: &[f32],
        b_shape: &[usize],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.bin_f32(0, a, a_shape, b, b_shape))
    }

    pub(super) fn try_mul_f32(
        a: &[f32],
        a_shape: &[usize],
        b: &[f32],
        b_shape: &[usize],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.bin_f32(2, a, a_shape, b, b_shape))
    }

    pub(super) fn try_gelu_f32(a: &[f32], shape: &[usize]) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.unary_gelu_f32(a, shape))
    }

    pub(super) fn try_layer_norm_f32(x: &[f32], shape: &[usize], eps: f32) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.norm_f32(x, shape, eps))
    }

    pub(super) fn try_rms_norm_f32(x: &[f32], shape: &[usize], eps: f32) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.rms_norm_f32(x, shape, eps))
    }

    pub(super) fn try_rms_norm_mul_f32(
        x: &[f32],
        x_shape: &[usize],
        mul: &[f32],
        mul_shape: &[usize],
        eps: f32,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.rms_norm_mul_f32(x, x_shape, mul, mul_shape, eps))
    }

    pub(super) fn try_attention_softmax_weighted_sum_f32(
        _logits: &[f32],
        _values: &[f32],
        _query_count: usize,
        _seq_len: usize,
        _head_dim: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_layer_norm_mul_add_f32(
        x: &[f32],
        x_shape: &[usize],
        mul: &[f32],
        mul_shape: &[usize],
        add: &[f32],
        add_shape: &[usize],
        eps: f32,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.norm_mul_add_f32(x, x_shape, mul, mul_shape, add, add_shape, eps))
    }

    pub(super) fn try_get_rows_ggml_bytes(
        src: &[u8],
        src_ggml_type: u32,
        n_cols: usize,
        n_rows: usize,
        row_indices: &[i32],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.get_rows_ggml_bytes(src, src_ggml_type, n_cols, n_rows, row_indices))
    }

    pub(super) fn try_im2col_1d_f32(
        input: &[f32],
        ic: usize,
        iw: usize,
        kw: usize,
        stride: usize,
        pad: usize,
    ) -> Option<Vec<f32>> {
        with_context(|ctx| ctx.im2col_1d_f32(input, ic, iw, kw, stride, pad))
    }

    // The diffusion VAE runs through the compiled graph path on macOS, so
    // these planar helpers only have CUDA implementations today.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_conv2d_planar_f32(
        _input: &[f32],
        _width: usize,
        _height: usize,
        _in_channels: usize,
        _weights: &[f32],
        _bias: &[f32],
        _out_channels: usize,
        _kw: usize,
        _kh: usize,
        _pad_x: usize,
        _pad_y: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_group_norm_planar_f32(
        _input: &[f32],
        _width: usize,
        _height: usize,
        _channels: usize,
        _groups: usize,
        _gamma: &[f32],
        _beta: &[f32],
        _eps: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_silu_f32(_a: &[f32]) -> Option<Vec<f32>> {
        None
    }

    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_encoder_attn_block_f32(
        x: &[f32],
        seq_len: usize,
        n_state: usize,
        n_head: usize,
        ln_w: &[f32],
        ln_b: &[f32],
        q_w_bytes: &[u8],
        q_w_ggml_type: u32,
        q_b: &[f32],
        k_w_bytes: &[u8],
        k_w_ggml_type: u32,
        v_w_bytes: &[u8],
        v_w_ggml_type: u32,
        v_b: &[f32],
        out_w_bytes: &[u8],
        out_w_ggml_type: u32,
        out_b: &[f32],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.encoder_attn_block_f32(
                x,
                seq_len,
                n_state,
                n_head,
                ln_w,
                ln_b,
                q_w_bytes,
                q_w_ggml_type,
                q_b,
                k_w_bytes,
                k_w_ggml_type,
                v_w_bytes,
                v_w_ggml_type,
                v_b,
                out_w_bytes,
                out_w_ggml_type,
                out_b,
            )
        })
    }

    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_encoder_ffn_block_f32(
        x: &[f32],
        seq_len: usize,
        n_state: usize,
        ln_w: &[f32],
        ln_b: &[f32],
        w0_bytes: &[u8],
        w0_ggml_type: u32,
        b0: &[f32],
        w1_bytes: &[u8],
        w1_ggml_type: u32,
        b1: &[f32],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.encoder_ffn_block_f32(
                x,
                seq_len,
                n_state,
                ln_w,
                ln_b,
                w0_bytes,
                w0_ggml_type,
                b0,
                w1_bytes,
                w1_ggml_type,
                b1,
            )
        })
    }

    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_encoder_layer_f32(
        x: &[f32],
        seq_len: usize,
        n_state: usize,
        n_head: usize,
        attn_ln_w: &[f32],
        attn_ln_b: &[f32],
        q_w_bytes: &[u8],
        q_w_ggml_type: u32,
        q_b: &[f32],
        k_w_bytes: &[u8],
        k_w_ggml_type: u32,
        v_w_bytes: &[u8],
        v_w_ggml_type: u32,
        v_b: &[f32],
        out_w_bytes: &[u8],
        out_w_ggml_type: u32,
        out_b: &[f32],
        mlp_ln_w: &[f32],
        mlp_ln_b: &[f32],
        w0_bytes: &[u8],
        w0_ggml_type: u32,
        b0: &[f32],
        w1_bytes: &[u8],
        w1_ggml_type: u32,
        b1: &[f32],
    ) -> Option<Vec<f32>> {
        with_context(|ctx| {
            ctx.encoder_layer_f32(
                x,
                seq_len,
                n_state,
                n_head,
                attn_ln_w,
                attn_ln_b,
                q_w_bytes,
                q_w_ggml_type,
                q_b,
                k_w_bytes,
                k_w_ggml_type,
                v_w_bytes,
                v_w_ggml_type,
                v_b,
                out_w_bytes,
                out_w_ggml_type,
                out_b,
                mlp_ln_w,
                mlp_ln_b,
                w0_bytes,
                w0_ggml_type,
                b0,
                w1_bytes,
                w1_ggml_type,
                b1,
            )
        })
    }

    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_decoder_self_qkv_step_f32(
        x: &[f32],
        n_state: usize,
        attn_ln_w: &[f32],
        attn_ln_b: &[f32],
        q_w_bytes: &[u8],
        q_w_ggml_type: u32,
        q_b: &[f32],
        k_w_bytes: &[u8],
        k_w_ggml_type: u32,
        v_w_bytes: &[u8],
        v_w_ggml_type: u32,
        v_b: &[f32],
    ) -> Option<(Vec<f32>, Vec<f32>, Vec<f32>)> {
        with_context(|ctx| {
            ctx.decoder_self_qkv_step_f32(
                x,
                n_state,
                attn_ln_w,
                attn_ln_b,
                q_w_bytes,
                q_w_ggml_type,
                q_b,
                k_w_bytes,
                k_w_ggml_type,
                v_w_bytes,
                v_w_ggml_type,
                v_b,
            )
        })
    }
}
