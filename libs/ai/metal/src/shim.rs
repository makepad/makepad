use std::borrow::Cow;
#[cfg(all(test, target_os = "macos"))]
pub(crate) use imp::{tensor_f32_weight, tensor_test_error_and_guard,
    tensor_test_drop_owner_locked, tensor_test_legacy_matmuls};
#[cfg(target_os = "macos")]
pub(crate) use imp::{TensorBuffer, tensor_thread_guard, tensor_available, tensor_profile, tensor_cow_record, tensor_buffer, tensor_host_written, tensor_flush_until, tensor_pool_clear, tensor_alloc_bytes, tensor_evict_owner, tensor_evict_keys, tensor_resident_weights, shape4_from_row_major, tensor_binary, tensor_unary, tensor_norm, tensor_matmul, tensor_named_weight, tensor_attention, tensor_im2col, tensor_copy_strided};

/// Missing-kernel fallbacks fire per matmul; print each distinct message once.
#[cfg(target_os = "macos")]
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

/// Run a tool's model work off the process main thread, preserving its stack size.
pub fn run_model_worker<F, T>(name: &str, f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let builder = std::thread::Builder::new().name(name.to_owned());
    #[cfg(target_os = "macos")]
    let builder = {
        unsafe extern "C" {
            fn pthread_self() -> *mut std::ffi::c_void;
            fn pthread_get_stacksize_np(thread: *mut std::ffi::c_void) -> usize;
        }
        // Called by the tool's main: retain the stack its model work had there.
        builder.stack_size(unsafe { pthread_get_stacksize_np(pthread_self()) })
    };
    match builder.spawn(f).expect("start model worker").join() {
        Ok(value) => value,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

pub fn is_available() -> bool {
    cfg!(target_os = "macos")
}

/// This process's device allocations and the advisory working-set size, in bytes.
/// Reads device properties without initializing a tensor runtime.
pub fn device_memory() -> Result<(u64, u64), String> {
    imp::device_memory()
}

pub fn device_name() -> Option<String> {
    imp::device_name()
}

use makepad_ai_loader::{F32Weight, WeightKey, WeightOwner};
use crate::gpu_types::GpuTensor;
use makepad_ai_loader::prof;

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

pub fn try_matmul_nt_f32_weight(a: &[f32], w: &F32Weight, m: usize, k: usize, n: usize) -> Option<Vec<f32>> {
    let t = std::time::Instant::now();
    let out = imp::try_matmul_nt_f32_weight(a, w, m, k, n);
    if let Some(v) = &out {
        prof_rec(prof::CAT_MATMUL_UNCACHED, t, a.len() + v.len());
    }
    out
}

pub fn try_matmul_nt_ggml_bytes_keyed<'a, F>(
    a: &[f32],
    bt_ggml_type: u32,
    m: usize,
    k: usize,
    n: usize,
    owner: &WeightOwner,
    cache_key: &str,
    load: F,
) -> Option<Vec<f32>>
where
    F: FnOnce() -> Result<Cow<'a, [u8]>, String>,
{
    let t = std::time::Instant::now();
    let out = imp::try_matmul_nt_ggml_bytes_keyed(
        a, bt_ggml_type, m, k, n, owner, cache_key, load,
    );
    if let Some(v) = &out {
        prof_rec(prof::CAT_MATMUL_UNCACHED, t, a.len() + v.len());
    }
    out
}

/// Same kernels as [`try_matmul_nt_ggml_bytes_keyed`], one activation upload,
/// one wait. Each weight lives in the named cache under its namespace and key.
#[derive(Clone, Copy, Debug)]
pub struct MatmulNtGgmlBytesKeyedMatrix<'a> {
    pub bt_ggml_type: u32,
    pub n: usize,
    pub owner: &'a WeightOwner,
    pub cache_key: &'a str,
}

pub fn try_matmul_nt_ggml_bytes_keyed_multi<'a, F>(
    a: &[f32],
    m: usize,
    k: usize,
    matrices: &[MatmulNtGgmlBytesKeyedMatrix<'_>],
    load: F,
) -> Option<Vec<Vec<f32>>>
where
    F: FnMut(&WeightOwner, &str) -> Result<Cow<'a, [u8]>, String>,
{
    let t = std::time::Instant::now();
    let out = imp::try_matmul_nt_ggml_bytes_keyed_multi(a, m, k, matrices, load);
    if let Some(vs) = &out {
        let n_out: usize = vs.iter().map(|v| v.len()).sum();
        prof_rec(prof::CAT_MATMUL_UNCACHED, t, a.len() + n_out);
    }
    out
}

/// The AR residual stream between `try_ar_*` calls, owned by the caller (one per decode
/// step or prefill). Dropping it releases the device buffer.
pub struct ArResidual(imp::ArResidual);
impl ArResidual {
    pub fn new() -> Self {
        Self(imp::ArResidual::default())
    }
}
impl Default for ArResidual {
    fn default() -> Self {
        Self::new()
    }
}

/// Upload (optional) hidden, RMS + QKV (+ optional Q/K RMS) on GPU.
/// Returns host q,k,v. `qk_norm` is `(q_norm, k_norm, q_key, k_key)`;
/// pass `None` for stacks without per-head Q/K norms (Music3 RVQ).
pub fn try_ar_pre_attn<F>(
    resid: &mut ArResidual,
    hidden: Option<&[f32]>,
    m: usize,
    hidden_w: usize,
    head_dim: usize,
    in_norm: &[f32],
    qk_norm: Option<(&[f32], &[f32], &WeightKey, &WeightKey)>,
    in_norm_key: &WeightKey,
    eps: f32,
    q: MatmulNtGgmlBytesKeyedMatrix<'_>,
    k: MatmulNtGgmlBytesKeyedMatrix<'_>,
    v: MatmulNtGgmlBytesKeyedMatrix<'_>,
    load: F,
) -> Option<(Vec<f32>, Vec<f32>, Vec<f32>)>
where
    F: FnMut(&WeightOwner, &str) -> Result<Vec<u8>, String>,
{
    imp::try_ar_pre_attn(
        &mut resid.0,
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
    resid: &mut ArResidual,
    attn: &[f32],
    m: usize,
    hidden_w: usize,
    post_norm: &[f32],
    post_norm_key: &WeightKey,
    eps: f32,
    o: MatmulNtGgmlBytesKeyedMatrix<'_>,
    up: MatmulNtGgmlBytesKeyedMatrix<'_>,
    gate: MatmulNtGgmlBytesKeyedMatrix<'_>,
    down: MatmulNtGgmlBytesKeyedMatrix<'_>,
    load: F,
) -> Option<()>
where
    F: FnMut(&WeightOwner, &str) -> Result<Vec<u8>, String>,
{
    imp::try_ar_post_attn(
        &mut resid.0,
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
    resid: &mut ArResidual,
    m: usize,
    hidden_w: usize,
    gamma: &[f32],
    gamma_key: &WeightKey,
    eps: f32,
) -> Option<Vec<f32>> {
    imp::try_ar_final_rms(&mut resid.0, m, hidden_w, gamma, gamma_key, eps)
}

/// Drop all recycled transient buffers in the tensor runtime (keeps weights). Call at a
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
    ff_in_b_key: &WeightKey,
    ff_out_b_key: &WeightKey,
    swap: bool,
    ff_in: MatmulNtGgmlBytesKeyedMatrix<'_>,
    ff_out: MatmulNtGgmlBytesKeyedMatrix<'_>,
    load: F,
) -> Option<Vec<f32>>
where
    F: FnMut(&WeightOwner, &str) -> Result<Vec<u8>, String>,
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

#[allow(clippy::too_many_arguments)]
/// One linear of a device-resident two-way decoder layer: an f32 `[n, k]`
/// weight tensor (cached on the device under its content identity) and an
/// optional `[1, n]` bias.
#[derive(Clone, Copy)]
pub struct DecLinearRef<'a> {
    pub owner: &'a WeightOwner,
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

#[cfg(all(
    test,
    any(
        target_os = "macos",
        all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels)
    )
))]
mod tests {
    use super::{try_add_f32, try_gelu_f32, try_mul_f32, try_rms_norm_f32, try_rms_norm_mul_f32};
    #[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
    use super::try_attention_softmax_weighted_sum_f32;

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
        let actual = try_add_f32(&a, &[2, 3], &b, &[2, 3]).expect("a working device backend is required");
        assert_eq!(actual, expected);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn conv2d_planar_matches_cpu_when_backend_available() {
        let (ic, oc, width, height) = (3, 4, 7, 5);
        let input: Vec<f32> = (0..ic * width * height)
            .map(|i| ((i * 7 % 23) as f32 - 11.0) / 16.0).collect();
        let weights: Vec<f32> = (0..oc * ic * 9)
            .map(|i| ((i * 11 % 19) as f32 - 9.0) / 32.0).collect();
        let bias = [0.125, -0.25, 0.375, -0.5];
        for (case, kernel, pad, budget) in [
            ("3x3 padded", 3, 1, 256 * 1024 * 1024),
            ("1x1 unpadded without bias", 1, 0, 256 * 1024 * 1024),
            ("3x3 smaller output", 3, 0, 256 * 1024 * 1024),
            ("3x3 two-row bands", 3, 1, 2 * width * ic * 9 * 4),
        ] {
            let ow = width + 2 * pad - kernel + 1;
            let oh = height + 2 * pad - kernel + 1;
            let weights = &weights[..oc * ic * kernel * kernel];
            let bias: &[f32] = if kernel == 1 { &[] } else { &bias };
            let mut expected = vec![0.0f32; oc * oh * ow];
            for c in 0..oc {
                for y in 0..oh {
                    for x in 0..ow {
                        let mut sum = 0.0f32;
                        for i in 0..ic {
                            for ky in 0..kernel {
                                for kx in 0..kernel {
                                    let iy = (y + ky) as isize - pad as isize;
                                    let ix = (x + kx) as isize - pad as isize;
                                    if iy >= 0 && iy < height as isize && ix >= 0 && ix < width as isize {
                                        sum += input[(i * height + iy as usize) * width + ix as usize]
                                            * weights[((c * ic + i) * kernel + ky) * kernel + kx];
                                    }
                                }
                            }
                        }
                        expected[(c * oh + y) * ow + x] = sum + bias.get(c).copied().unwrap_or(0.0);
                    }
                }
            }
            let actual = super::imp::try_conv2d_planar_f32_with_budget(
                &input, width, height, ic, weights, bias, oc,
                kernel, kernel, pad, pad, budget,
            ).unwrap_or_else(|| panic!("{case}: a working Metal backend is required"));
            assert_eq!(actual.len(), expected.len(), "{case}: output length");
            for (i, (&a, &e)) in actual.iter().zip(&expected).enumerate() {
                assert!((a - e).abs() <= 1.0e-5 * e.abs().max(f32::EPSILON),
                    "{case}: element {i}: actual={a} expected={e}");
            }
            if case == "3x3 smaller output" {
                let x = crate::gpu_tensor::upload(&input, ic, width * height).unwrap();
                let out = crate::gpu_tensor::conv2d_planar(
                    &x, width, height, weights, bias, oc, kernel, kernel, pad, pad,
                ).expect("3x3 smaller output: tensor convolution");
                assert_eq!((out.rows(), out.cols()), (oc, oh * ow), "{case}: tensor shape");
            }
        }
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
        let actual = try_mul_f32(&a, &[2, 3], &b, &[2, 3]).expect("a working device backend is required");
        assert_close(&actual, &expected, RMS_NORM_TOLERANCE);
    }

    #[test]
    fn gelu_f32_matches_cpu_when_backend_available() {
        let input = vec![-2.0, -0.5, 0.0, 0.5, 1.5, 3.0];
        let expected = input.iter().copied().map(cpu_gelu).collect::<Vec<_>>();
        let actual = try_gelu_f32(&input, &[2, 3]).expect("a working device backend is required");
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
        let actual = try_rms_norm_f32(&x, &[2, 3], eps).expect("a working device backend is required");
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
        let actual = try_rms_norm_mul_f32(&x, &[2, 3], &mul, &[3], eps).expect("a working device backend is required");
        assert_close(&actual, &expected, RMS_NORM_TOLERANCE);
    }

    // Only the CUDA backend implements this helper; on macOS it returns `None` and callers take their host path.
    #[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
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
        let actual = try_attention_softmax_weighted_sum_f32(&logits, &values, 2, 3, 2).expect("a working device backend is required");
        assert_close(&actual, &expected, RMS_NORM_TOLERANCE);
    }

    #[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
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

#[cfg(all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels))]
mod imp {
    pub(super) fn device_memory() -> Result<(u64, u64), String> {
        Err("no Metal device".to_string())
    }

    pub(super) fn device_name() -> Option<String> {
        None
    }

    #[derive(Default)]
    pub(super) struct ArResidual;

    use makepad_ai_loader::{F32Weight, WeightKey, WeightOwner};
    use super::Cow;
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

    pub(super) fn try_matmul_nt_f32_weight(
        a: &[f32], w: &F32Weight, m: usize, k: usize, n: usize,
    ) -> Option<Vec<f32>> {
        try_matmul_nt_f32(a, w.as_slice(), m, k, n)
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

    pub(super) fn try_matmul_nt_ggml_bytes_keyed<'a, F>(
        _a: &[f32],
        _bt_ggml_type: u32,
        _m: usize,
        _k: usize,
        _n: usize,
        _owner: &WeightOwner,
        _cache_key: &str,
        _load: F,
    ) -> Option<Vec<f32>>
    where
        F: FnOnce() -> Result<Cow<'a, [u8]>, String>,
    {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes_keyed_multi<'a, F>(
        _a: &[f32],
        _m: usize,
        _k: usize,
        _matrices: &[super::MatmulNtGgmlBytesKeyedMatrix<'_>],
        _load: F,
    ) -> Option<Vec<Vec<f32>>>
    where
        F: FnMut(&WeightOwner, &str) -> Result<Cow<'a, [u8]>, String>,
    {
        None
    }

    pub(super) fn try_ar_pre_attn<F>(
        _resid: &mut ArResidual,
        _hidden: Option<&[f32]>,
        _m: usize,
        _hidden_w: usize,
        _head_dim: usize,
        _in_norm: &[f32],
        _qk_norm: Option<(&[f32], &[f32], &WeightKey, &WeightKey)>,
        _in_norm_key: &WeightKey,
        _eps: f32,
        _q: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _k: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _v: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _load: F,
    ) -> Option<(Vec<f32>, Vec<f32>, Vec<f32>)>
    where
        F: FnMut(&WeightOwner, &str) -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_ar_post_attn<F>(
        _resid: &mut ArResidual,
        _attn: &[f32],
        _m: usize,
        _hidden_w: usize,
        _post_norm: &[f32],
        _post_norm_key: &WeightKey,
        _eps: f32,
        _o: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _up: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _gate: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _down: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _load: F,
    ) -> Option<()>
    where
        F: FnMut(&WeightOwner, &str) -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_ar_final_rms(
        _resid: &mut ArResidual,
        _m: usize,
        _hidden_w: usize,
        _gamma: &[f32],
        _gamma_key: &WeightKey,
        _eps: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn transient_pool_clear() {}

    pub(super) fn try_dit_ffn_resident<F>(
        _normed: &[f32],
        _m: usize,
        _hidden_w: usize,
        _ff_dim: usize,
        _ff_in_b: &[f32],
        _ff_out_b: &[f32],
        _ff_in_b_key: &WeightKey,
        _ff_out_b_key: &WeightKey,
        _swap: bool,
        _ff_in: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _ff_out: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _load: F,
    ) -> Option<Vec<f32>>
    where
        F: FnMut(&WeightOwner, &str) -> Result<Vec<u8>, String>,
    {
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
#[cfg(not(any(
    target_os = "macos",
    all(any(target_os = "linux", target_os = "windows"), makepad_ai_cuda_kernels)
)))]
#[allow(unused_variables)]
mod imp {
    pub(super) fn device_memory() -> Result<(u64, u64), String> {
        Err("no Metal device".to_string())
    }

    pub(super) fn device_name() -> Option<String> {
        None
    }

    #[derive(Default)]
    pub(super) struct ArResidual;

    use makepad_ai_loader::{F32Weight, WeightKey, WeightOwner};
    use super::Cow;
    pub(super) fn try_matmul_nn_f32(
        a: &[f32],
        b: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn try_matmul_nt_f32_weight(
        a: &[f32], w: &F32Weight, m: usize, k: usize, n: usize,
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

    pub(super) fn try_matmul_nt_ggml_bytes_keyed<'a, F>(
        _a: &[f32],
        _bt_ggml_type: u32,
        _m: usize,
        _k: usize,
        _n: usize,
        _owner: &WeightOwner,
        _cache_key: &str,
        _load: F,
    ) -> Option<Vec<f32>>
    where
        F: FnOnce() -> Result<Cow<'a, [u8]>, String>,
    {
        None
    }

    pub(super) fn try_matmul_nt_ggml_bytes_keyed_multi<'a, F>(
        _a: &[f32],
        _m: usize,
        _k: usize,
        _matrices: &[super::MatmulNtGgmlBytesKeyedMatrix<'_>],
        _load: F,
    ) -> Option<Vec<Vec<f32>>>
    where
        F: FnMut(&WeightOwner, &str) -> Result<Cow<'a, [u8]>, String>,
    {
        None
    }

    pub(super) fn try_ar_pre_attn<F>(
        _resid: &mut ArResidual,
        _hidden: Option<&[f32]>,
        _m: usize,
        _hidden_w: usize,
        _head_dim: usize,
        _in_norm: &[f32],
        _qk_norm: Option<(&[f32], &[f32], &WeightKey, &WeightKey)>,
        _in_norm_key: &WeightKey,
        _eps: f32,
        _q: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _k: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _v: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _load: F,
    ) -> Option<(Vec<f32>, Vec<f32>, Vec<f32>)>
    where
        F: FnMut(&WeightOwner, &str) -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_ar_post_attn<F>(
        _resid: &mut ArResidual,
        _attn: &[f32],
        _m: usize,
        _hidden_w: usize,
        _post_norm: &[f32],
        _post_norm_key: &WeightKey,
        _eps: f32,
        _o: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _up: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _gate: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _down: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _load: F,
    ) -> Option<()>
    where
        F: FnMut(&WeightOwner, &str) -> Result<Vec<u8>, String>,
    {
        None
    }

    pub(super) fn try_ar_final_rms(
        _resid: &mut ArResidual,
        _m: usize,
        _hidden_w: usize,
        _gamma: &[f32],
        _gamma_key: &WeightKey,
        _eps: f32,
    ) -> Option<Vec<f32>> {
        None
    }

    pub(super) fn transient_pool_clear() {}

    pub(super) fn try_dit_ffn_resident<F>(
        _normed: &[f32],
        _m: usize,
        _hidden_w: usize,
        _ff_dim: usize,
        _ff_in_b: &[f32],
        _ff_out_b: &[f32],
        _ff_in_b_key: &WeightKey,
        _ff_out_b_key: &WeightKey,
        _swap: bool,
        _ff_in: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _ff_out: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        _load: F,
    ) -> Option<Vec<f32>>
    where
        F: FnMut(&WeightOwner, &str) -> Result<Vec<u8>, String>,
    {
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
    pub(super) fn device_memory() -> Result<(u64, u64), String> {
        let _pool = AutoreleasePool::new();
        let device = MetalContext::create_device().ok_or_else(|| "no Metal device".to_string())?;
        unsafe {
            let current: u64 = msg_send![device.as_id(), currentAllocatedSize];
            let recommended: u64 = msg_send![device.as_id(), recommendedMaxWorkingSetSize];
            Ok((current, recommended))
        }
    }

    pub(super) fn device_name() -> Option<String> {
        let _pool = AutoreleasePool::new();
        let device = MetalContext::create_device()?;
        let name: ObjcId = unsafe { msg_send![device.as_id(), name] };
        (!name.is_null()).then(|| nsstring_to_string(name))
    }

    #[derive(Default)]
    pub(super) struct ArResidual {
        h: Option<Arc<TensorBuffer>>,
        elems: usize,
    }

    use makepad_ai_loader::{F32Weight, WeightKey, WeightOwner, WeightOwnerTrace};
    use super::Cow;
    use makepad_ai_loader::quant::{
        block_elements, block_size, ggml_type_name, GGML_TYPE_BF16, GGML_TYPE_F16,
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
    use std::thread;
    use std::time::Duration;

    #[link(name = "System")]
    unsafe extern "C" {
        // Referenced by name from `objc_block!`.
        static _NSConcreteStackBlock: [*const c_void; 32];
    }

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
    const SCRATCH_ENC_NORM0: u8 = 10;
    const SCRATCH_ENC_NORM1: u8 = 11;

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
    pub(crate) struct Shape4 {
        ne: [i32; 4],
        nb: [u64; 4],
        numel: usize,
    }

    struct StrongId(NonNull<Object>);

    // Metal objects are thread-safe; the tensor mutex serialises encoding.
    unsafe impl Send for StrongId {}
    // Shared handles expose only Metal's thread-safe retain and object methods.
    unsafe impl Sync for StrongId {}

    impl Clone for StrongId {
        fn clone(&self) -> Self {
            unsafe { let _: () = msg_send![self.as_id(), retain]; }
            Self(self.0)
        }
    }

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

    pub(crate) fn shape4_from_row_major(shape: &[usize], elem_bytes: u64) -> Result<Shape4, String> {
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

    #[derive(Clone)]
    struct PipelineState {
        obj: StrongId,
        smem: usize,
        nsg: i32,
        nr0: i32,
        nr1: i32,
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

    struct MetalContext {
        tensor_mode: bool,
        tensor_prepare: bool,
        tensor_batch: u64,
        tensor_dispatches: usize,
        tensor_bindings: HashMap<usize, std::sync::Arc<TensorBuffer>>,
        device: StrongId,
        command_queue: StrongId,
        library: StrongId,
        pipeline_cache: HashMap<String, PipelineState>,
        cached_weight_buffers: HashMap<BufferKey, StrongId>,
        named_weight_buffers: HashMap<(String, String), StrongId>,
        scratch_buffers: HashMap<u8, ScratchBuffer>,
        matmul_out_buffers: HashMap<u8, ScratchBuffer>,
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

                return Ok(Self {
                    tensor_mode: false,
                    tensor_prepare: false,
                    tensor_batch: 0,
                    tensor_dispatches: 0,
                    tensor_bindings: HashMap::new(),
                    device,
                    command_queue,
                    library,
                    pipeline_cache: HashMap::new(),
                    cached_weight_buffers: HashMap::new(),
                    named_weight_buffers: HashMap::new(),
                    scratch_buffers: HashMap::new(),
                    matmul_out_buffers: HashMap::new(),
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
            if let Some(i) = self
                .pool_free
                .iter()
                .position(|p| p.cap_bytes == need)
            {
                return Ok(self.pool_free.swap_remove(i).buf);
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

        fn get_or_create_scratch_buffer(
            &mut self,
            kind: u8,
            need_bytes: usize,
        ) -> Result<ObjcId, String> {
            let need_bytes = need_bytes.max(1);
            if self.tensor_mode {
                if let Some(entry) = self.scratch_buffers.get(&kind) {
                    if entry.cap_bytes >= need_bytes { return Ok(entry.buf.as_id()); }
                }
                if !self.tensor_prepare { return Err("unprepared tensor scratch".into()); }
                let buffer = tensor_buffer(need_bytes, false)?;
                let id = buffer.id();
                self.scratch_buffers.insert(kind, ScratchBuffer {
                    buf: buffer.object()?.clone(), cap_bytes: buffer.capacity, is_private: false,
                });
                self.tensor_bindings.insert(id as usize, buffer);
                return Ok(id);
            }
            let is_private = matches!(
                kind,
                SCRATCH_FLASH_PAD
                    | SCRATCH_FLASH_BLK
                    | SCRATCH_FLASH_TMP
                    | SCRATCH_FLASH_OUT
                    | SCRATCH_ENC_NORM0
                    | SCRATCH_ENC_NORM1
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
            owner: &WeightOwner,
            cache_key: &str,
            load: impl FnOnce() -> Result<Vec<u8>, String>,
        ) -> Result<ObjcId, String> {
            Ok(tensor_named_weight(owner, cache_key, || load().map(Cow::Owned))?.id())
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
            if self.tensor_mode && !self.pipeline_cache.contains_key(&cache_name) {
                if !self.tensor_prepare {
                    return Err(format!("unprepared tensor pipeline: {cache_name}"));
                }
                let cached = tensor_locked(|state| Ok(state.context.pipeline_cache.get(&cache_name).cloned()))?;
                let pipeline = if let Some(pipeline) = cached { pipeline } else {
                    let obj = self.compile_pipeline(base_name, constants)?;
                    let candidate = PipelineState { obj, smem, nr0, nr1, nsg };
                    tensor_locked(|state| Ok(state.context.pipeline_cache.entry(cache_name.clone())
                        .or_insert_with(|| candidate.clone()).clone()))?
                };
                self.pipeline_cache.insert(cache_name.clone(), pipeline);
            }
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

            let p = self.pipeline_cache.get(&cache_name).ok_or_else(|| "pipeline missing".to_string())?;
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
            if self.tensor_mode && !self.tensor_prepare { self.tensor_dispatches += 1; }
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
            self.dispatch_mul_mv_ext_offsets(src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01, nb10, nb11, ne0, ne1, [0, 0, 0])
        }

        fn dispatch_mul_mv_ext_offsets(
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
                    offsets: [usize; 3],
) -> Result<(), String> {
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

            if self.tensor_prepare {
                return Ok(());
            }
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsMulMvExt as *const c_void
                    length: std::mem::size_of::<KArgsMulMvExt>() as u64
                    atIndex: 0u64
                ];
                self.bind_buffer(encoder, src0_id, offsets[0], 1)?;
                self.bind_buffer(encoder, src1_id, offsets[1], 2)?;
                self.bind_buffer(encoder, dst_id, offsets[2], 3)?;

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
            self.dispatch_mul_mm_offsets(src0, src0_id, src1_id, dst_id, ne00, ne01, nb01, ne12, nb10, nb11, ne0, ne1, [0, 0, 0])
        }

        fn dispatch_mul_mm_offsets(
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
                    offsets: [usize; 3],
) -> Result<(), String> {
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

            if self.tensor_prepare {
                return Ok(());
            }
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsMulMm as *const c_void
                    length: std::mem::size_of::<KArgsMulMm>() as u64
                    atIndex: 0u64
                ];
                self.bind_buffer(encoder, src0_id, offsets[0], 1)?;
                self.bind_buffer(encoder, src1_id, offsets[1], 2)?;
                self.bind_buffer(encoder, dst_id, offsets[2], 3)?;
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
            self.dispatch_mul_mv_offsets(src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01, nb10, nb11, ne0, ne1, [0, 0, 0])
        }

        fn dispatch_mul_mv_offsets(
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
                    offsets: [usize; 3],
) -> Result<(), String> {
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

            if self.tensor_prepare {
                return Ok(());
            }
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsMulMv as *const c_void
                    length: std::mem::size_of::<KArgsMulMv>() as u64
                    atIndex: 0u64
                ];
                self.bind_buffer(encoder, src0_id, offsets[0], 1)?;
                self.bind_buffer(encoder, src1_id, offsets[1], 2)?;
                self.bind_buffer(encoder, dst_id, offsets[2], 3)?;

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
            self.dispatch_unary_f32_offsets(op_num, src0_id, dst_id, shape, [0, 0])
        }

        fn dispatch_unary_f32_offsets(
            &mut self,
            op_num: i16,
            src0_id: ObjcId,
            dst_id: ObjcId,
            shape: &Shape4,
                    offsets: [usize; 2],
) -> Result<(), String> {
            let is_c4 = offsets.iter().all(|offset| offset % 16 == 0) && shape.ne[0] % 4 == 0;
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

            if self.tensor_prepare {
                return Ok(());
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
                self.bind_buffer(encoder, src0_id, offsets[0], 1)?;
                self.bind_buffer(encoder, dst_id, offsets[1], 2)?;

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
            self.matmul_nt_into_buffer_offsets(src0_ggml_type, src0_id, src1_id, dst_id, m, k, n, [0, 0, 0])
        }

        fn matmul_nt_into_buffer_offsets(
            &mut self,
            src0_ggml_type: u32,
            src0_id: ObjcId,
            src1_id: ObjcId,
            dst_id: ObjcId,
            m: usize,
            k: usize,
            n: usize,
                    offsets: [usize; 3],
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
                self.dispatch_mul_mv_ext_offsets(
                    src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01, nb10,
                    nb11, ne0, ne1, [offsets[0], offsets[1], offsets[2]])
            } else if ne00 >= 64 && ne11 > 8 {
                match self.dispatch_mul_mm_offsets(
                    src0, src0_id, src1_id, dst_id, ne00, ne01, nb01, 1, nb10, nb11, ne0, ne1, [offsets[0], offsets[1], offsets[2]]) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        if self.tensor_mode { return Err(e); }
                        super::log_metal_error_once(format!(
                            "[ggml][metal] mul_mm failed for type {:?}, falling back to mul_mv: {}",
                            src0, e
                        ));
                        self.dispatch_mul_mv_offsets(
                            src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01,
                            nb10, nb11, ne0, ne1, [offsets[0], offsets[1], offsets[2]])
                    }
                }
            } else {
                self.dispatch_mul_mv_offsets(
                    src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01, nb10,
                    nb11, ne0, ne1, [offsets[0], offsets[1], offsets[2]])
            }
        }

        /// The f32 contents of a host-backed tensor as little-endian bytes.
        fn tensor_f32_bytes(t: &crate::gpu_types::GpuTensor) -> Result<Vec<u8>, String> {
            let data = crate::gpu_tensor::data(t)?;
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
                lin.owner,
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
                    lin.owner,
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

        fn dispatch_bin_f32(
            &mut self,
            op_num: i16,
            src0_id: ObjcId,
            src1_id: ObjcId,
            dst_id: ObjcId,
            src0_shape: &Shape4,
            src1_shape: &Shape4,
        ) -> Result<(), String> {
            self.dispatch_bin_f32_offsets(op_num, src0_id, src1_id, dst_id, src0_shape, src1_shape, [0, 0, 0])
        }

        fn dispatch_bin_f32_offsets(
            &mut self,
            op_num: i16,
            src0_id: ObjcId,
            src1_id: ObjcId,
            dst_id: ObjcId,
            src0_shape: &Shape4,
            src1_shape: &Shape4,
                    offsets: [usize; 3],
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

            let is_c4 = offsets.iter().all(|offset| offset % 16 == 0) && src0_shape.ne[0] % 4 == 0 && src1_shape.ne[0] % 4 == 0;
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

            if self.tensor_prepare {
                return Ok(());
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
                self.bind_buffer(encoder, src0_id, offsets[0], 1)?;
                self.bind_buffer(encoder, src1_id, offsets[1], 2)?;
                self.bind_buffer(encoder, dst_id, offsets[2], 3)?;

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
            if self.tensor_prepare { return Ok(()); }
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
                self.bind_buffer(encoder, src_id, 0, 1)?;
                self.bind_buffer(encoder, src_id, 0, 2)?;
                self.bind_buffer(encoder, dst_id, 0, 3)?;
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
            self.dispatch_norm_f32_offsets(src0_id, src1_0_id, src1_1_id, dst_id, src0_shape, src1_0_shape, src1_1_shape, eps, n_fuse, [0, 0, 0, 0])
        }

        fn dispatch_norm_f32_offsets(
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
                    offsets: [usize; 4],
) -> Result<(), String> {
            if src0_shape.ne[0] <= 0 {
                return Err("norm ne0 must be positive".to_string());
            }

            let is_c4 = offsets.iter().all(|offset| offset % 16 == 0) && src0_shape.ne[0] % 4 == 0;
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

            if self.tensor_prepare {
                return Ok(());
            }
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsNorm as *const c_void
                    length: std::mem::size_of::<KArgsNorm>() as u64
                    atIndex: 0u64
                ];
                self.bind_buffer(encoder, src0_id, offsets[0], 1)?;
                self.bind_buffer(encoder, src1_0_id, offsets[1], 2)?;
                self.bind_buffer(encoder, src1_1_id, offsets[2], 3)?;
                self.bind_buffer(encoder, dst_id, offsets[3], 4)?;
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

        fn dispatch_rms_norm_f32_offsets(
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
            offsets: [usize; 4],
        ) -> Result<(), String> {
            if src0_shape.ne[0] <= 0 {
                return Err("rms_norm ne0 must be positive".to_string());
            }

            let is_c4 = offsets.iter().all(|offset| offset % 16 == 0) && src0_shape.ne[0] % 4 == 0;
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

            if self.tensor_prepare { return Ok(()); }
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsNorm as *const c_void
                    length: std::mem::size_of::<KArgsNorm>() as u64
                    atIndex: 0u64
                ];
                self.bind_buffer(encoder, src0_id, offsets[0], 1)?;
                self.bind_buffer(encoder, src1_0_id, offsets[1], 2)?;
                self.bind_buffer(encoder, src1_1_id, offsets[2], 3)?;
                self.bind_buffer(encoder, dst_id, offsets[3], 4)?;
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
            offsets: [usize; 3],
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

            if self.tensor_prepare {
                return Ok(());
            }
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsGetRows as *const c_void
                    length: std::mem::size_of::<KArgsGetRows>() as u64
                    atIndex: 0u64
                ];
                self.bind_buffer(encoder, src0_id, offsets[0], 1)?;
                self.bind_buffer(encoder, src1_id, offsets[1], 2)?;
                self.bind_buffer(encoder, dst_id, offsets[2], 3)?;

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

        fn dispatch_im2col_2d_f32_offsets(
            &mut self,
            src_id: ObjcId,
            src_offset: usize,
            dst_id: ObjcId,
            ic: usize,
            iw: usize,
            ih: usize,
            channel_stride: usize,
            kw: usize,
            kh: usize,
            pad_x: usize,
            pad_y: usize,
            ow: usize,
            oh: usize,
                    offsets: [usize; 2],
) -> Result<(), String> {
            let base = "kernel_im2col_f32";
            let (pipeline, _smem, _nr0, _nr1, _nsg) =
                self.get_or_compile_cached_pipeline(base.to_string(), base, &[], 0, 0, 0, 0)?;
            let dim = |v: usize| i32::try_from(v)
                .map_err(|_| format!("im2col dimension too large: {v}"));
            let khw = kh.checked_mul(kw)
                .ok_or_else(|| "overflow computing im2col kernel size".to_string())?;
            let chw = ic.checked_mul(khw)
                .ok_or_else(|| "overflow computing im2col CHW".to_string())?;
            let ofs0 = ic.checked_mul(channel_stride)
                .ok_or_else(|| "overflow computing im2col batch stride".to_string())?;
            let args = KArgsIm2Col {
                ofs0: ofs0 as u64,
                ofs1: channel_stride as u64,
                iw: dim(iw)?,
                ih: dim(ih)?,
                chw: dim(chw)?,
                s0: 1,
                s1: 1,
                p0: dim(pad_x)?,
                p1: dim(pad_y)?,
                d0: 1,
                d1: 1,
                n: 1,
                kh: dim(kh)?,
                kw: dim(kw)?,
                khw: dim(khw)?,
            };
            let max_threads = Self::pipeline_max_threads(pipeline);
            if khw == 0 || khw as u64 > max_threads {
                return Err(format!(
                    "invalid im2col thread shape: kh*kw={khw} max={max_threads}"
                ));
            }
            if self.tensor_prepare {
                return Ok(());
            }
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsIm2Col as *const c_void
                    length: std::mem::size_of::<KArgsIm2Col>() as u64
                    atIndex: 0u64
                ];
                self.bind_buffer(encoder, src_id, offsets[0] + src_offset, 1)?;
                self.bind_buffer(encoder, dst_id, offsets[1], 2)?;
                let tgs = MTLSize { width: ic as u64, height: oh as u64, depth: ow as u64 };
                let tpg = MTLSize { width: 1, height: kh as u64, depth: kw as u64 };
                let _: () = msg_send![encoder, dispatchThreadgroups: tgs threadsPerThreadgroup: tpg];
            }
            self.end_command_encoder(encoder_handles)
        }

        #[allow(clippy::too_many_arguments)]


        fn dispatch_flash_attn_ext_pad_offsets(
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
                    offsets: [usize; 4],
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

            if self.tensor_prepare {
                return Ok(());
            }
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExtPad as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExtPad>() as u64
                    atIndex: 0u64
                ];
                self.bind_buffer(encoder, k_id, offsets[0], 1)?;
                self.bind_buffer(encoder, v_id, offsets[1], 2)?;
                self.bind_buffer(encoder, mask_id, offsets[2], 3)?;
                self.bind_buffer(encoder, pad_id, offsets[3], 4)?;

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


        fn dispatch_flash_attn_ext_blk_offsets(
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
                    offsets: [usize; 2],
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

            if self.tensor_prepare {
                return Ok(());
            }
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExtBlk as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExtBlk>() as u64
                    atIndex: 0u64
                ];
                self.bind_buffer(encoder, mask_id, offsets[0], 1)?;
                self.bind_buffer(encoder, blk_id, offsets[1], 2)?;

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
            self.dispatch_flash_attn_ext_f32_offsets(q_id, k_id, v_id, mask_id, sinks_id, pad_id, blk_id, dst_id, n_q, n_kv, n_head, d, kv_type, scale, has_mask, has_sinks, max_bias, logit_softcap, [0, 0, 0, 0, 0, 0, 0, 0])
        }

        fn dispatch_flash_attn_ext_f32_offsets(
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
                    offsets: [usize; 8],
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
                self.dispatch_flash_attn_ext_pad_offsets(
                    k_id, v_id, mask_id, pad_id, has_mask, ncpsg, ne11, ne02, 1, nb11, nb12, nb13,
                    nb21, nb22, nb23, ne31, ne32, ne33, nb31, nb32, nb33, [offsets[1], offsets[2], offsets[3], offsets[5]])?;
            }
            if has_mask {
                self.dispatch_flash_attn_ext_blk_offsets(
                    mask_id, blk_id, n_q, n_kv, ne31, ne32, ne33, nb31, nb32, nb33, nqptg, ncpsg, [offsets[3], offsets[6]])?;
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

            if self.tensor_prepare {
                return Ok(());
            }
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExt as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExt>() as u64
                    atIndex: 0u64
                ];
                self.bind_buffer(encoder, q_id, offsets[0], 1)?;
                self.bind_buffer(encoder, k_id, offsets[1], 2)?;
                self.bind_buffer(encoder, v_id, offsets[2], 3)?;
                self.bind_buffer(encoder, mask_id, offsets[3], 4)?;
                self.bind_buffer(encoder, sinks_id, offsets[4], 5)?;
                self.bind_buffer(encoder, pad_id, offsets[5], 6)?;
                self.bind_buffer(encoder, blk_id, offsets[6], 7)?;
                self.bind_buffer(encoder, dst_id, offsets[7], 8)?;
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



        fn dispatch_flash_attn_ext_vec_reduce_f32_offsets(
            &mut self,
            tmp_id: ObjcId,
            dst_id: ObjcId,
            nrows: usize,
            d: usize,
            nwg: i32,
                    offsets: [usize; 2],
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

            if self.tensor_prepare {
                return Ok(());
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
                self.bind_buffer(encoder, tmp_id, offsets[0], 1)?;
                self.bind_buffer(encoder, dst_id, offsets[1], 2)?;

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
            self.dispatch_flash_attn_ext_vec_f32_offsets(q_id, k_id, v_id, mask_id, sinks_id, pad_id, tmp_id, dst_id, n_q, n_kv, n_head, d, kv_type, scale, has_mask, has_sinks, max_bias, logit_softcap, [0, 0, 0, 0, 0, 0, 0, 0])
        }

        fn dispatch_flash_attn_ext_vec_f32_offsets(
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
                    offsets: [usize; 8],
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
                self.dispatch_flash_attn_ext_pad_offsets(
                    k_id, v_id, mask_id, pad_id, has_mask, ncpsg, ne11, ne02, 1, nb11, nb12, nb13,
                    nb21, nb22, nb23, ne31, ne32, ne33, nb31, nb32, nb33, [offsets[1], offsets[2], offsets[3], offsets[5]])?;
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

            if self.tensor_prepare {
                if nwg > 1 {
                    self.dispatch_flash_attn_ext_vec_reduce_f32_offsets(
                        out_id, dst_id, nrows, d, nwg, [offsets[6], offsets[7]],
                    )?;
                }
                return Ok(());
            }
            let (_command_buffer, encoder, encoder_handles) = self.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![
                    encoder,
                    setBytes: &args as *const KArgsFlashAttnExtVec as *const c_void
                    length: std::mem::size_of::<KArgsFlashAttnExtVec>() as u64
                    atIndex: 0u64
                ];
                self.bind_buffer(encoder, q_id, offsets[0], 1)?;
                self.bind_buffer(encoder, k_id, offsets[1], 2)?;
                self.bind_buffer(encoder, v_id, offsets[2], 3)?;
                self.bind_buffer(encoder, mask_id, offsets[3], 4)?;
                self.bind_buffer(encoder, sinks_id, offsets[4], 5)?;
                self.bind_buffer(encoder, pad_id, offsets[5], 6)?;
                self.bind_buffer(encoder, out_id, if nwg == 1 { offsets[7] } else { offsets[6] }, 7)?;

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
                self.dispatch_flash_attn_ext_vec_reduce_f32_offsets(tmp_id, dst_id, nrows, d, nwg, [offsets[6], offsets[7]])?;
            }

            Ok(())
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

            let compute_res = if used_mul_mv_ext {
                self.dispatch_mul_mv_ext(
                        src0, src0_id, src1_id, dst_id, ne00, ne01, ne10, ne11, nb00, nb01, nb10,
                        nb11, ne0, ne1,
                    )
            } else if used_mul_mm {
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
            };
            compute_res?;
            drop(src0_temp);
            if let Some(dst_buffer) = dst_temp {
                Ok(dst_buffer)
            } else {
                unsafe { StrongId::from_unowned(dst_id) }
                    .ok_or_else(|| "matmul scratch output buffer returned nil".to_string())
            }
        }

        #[cfg(test)]
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


    }

    use std::sync::{Arc, Mutex, OnceLock, atomic::{AtomicU64, Ordering}, mpsc};
    use std::collections::{BTreeMap, BTreeSet, VecDeque};

    extern "C" { fn pthread_main_np() -> i32; }

    pub(crate) fn tensor_thread_guard() -> Result<(), String> {
        tensor_thread_guard_for(unsafe { pthread_main_np() } != 0)
    }

    fn tensor_thread_guard_for(main: bool) -> Result<(), String> {
        if main { Err("Metal tensor operations are forbidden on the process main/UI thread".into()) }
        else { Ok(()) }
    }

    struct ReturnedBuffer {
        object: StrongId,
        capacity: usize,
        last_batch: u64,
        writer_batch: u64,
        recycle: bool,
    }

    pub(crate) struct TensorBuffer {
        object: Option<StrongId>,
        capacity: usize,
        last_batch: AtomicU64,
        writer_batch: AtomicU64,
        returns: mpsc::Sender<ReturnedBuffer>,
        recycle: bool,
    }

    impl TensorBuffer {
        fn object(&self) -> Result<&StrongId, String> {
            self.object.as_ref().ok_or_else(|| "released tensor buffer".into())
        }
        fn id(&self) -> ObjcId { self.object.as_ref().map_or(nil, StrongId::as_id) }
        #[cfg(test)]
        pub(crate) fn test_identity(&self) -> usize { self.id() as usize }
        pub(crate) fn last_batch(&self) -> u64 { self.last_batch.load(Ordering::Acquire) }
        pub(crate) fn contents(&self) -> Result<*mut f32, String> {
            let ptr: *mut f32 = unsafe { msg_send![self.id(), contents] };
            if ptr.is_null() { Err("shared tensor buffer has no contents".into()) } else { Ok(ptr) }
        }
    }

    impl Drop for TensorBuffer {
        fn drop(&mut self) {
            if let Some(object) = self.object.take() {
                // No context lock, including when an op unwinds while holding it.
                let _ = self.returns.send(ReturnedBuffer {
                    object, capacity: self.capacity, last_batch: self.last_batch(),
                    writer_batch: self.writer_batch.load(Ordering::Acquire), recycle: self.recycle,
                });
            }
        }
    }

    const SLICE_NS: f64 = 4_000_000.0;

    #[derive(Clone, Copy)]
    struct TensorCost {
        ns_per_mac: f64,
        ns_per_byte: f64,
        ns_per_dispatch: f64,
        ns_per_commit: f64,
    }

    fn estimate_ns(cost: &TensorCost, macs: u64, bytes: u64, dispatches: usize) -> f64 {
        macs as f64 * cost.ns_per_mac + bytes as f64 * cost.ns_per_byte
            + dispatches as f64 * cost.ns_per_dispatch
    }

    fn should_commit(open_ns: f64, gpu_idle: bool, cost: &TensorCost) -> bool {
        open_ns >= SLICE_NS || (gpu_idle && open_ns >= cost.ns_per_commit)
    }

    fn row_chunks(
        rows: usize, est_ns: f64, align: usize, min_rows: usize,
    ) -> Vec<std::ops::Range<usize>> {
        if est_ns <= SLICE_NS || rows <= min_rows {
            return vec![0..rows];
        }
        let chunks = (est_ns / SLICE_NS).ceil() as usize;
        let chunk_rows = rows.div_ceil(chunks).max(min_rows).div_ceil(align) * align;
        let mut ranges = Vec::new();
        let mut start = 0;
        while start < rows {
            let mut end = start.saturating_add(chunk_rows).min(rows);
            if rows - end < min_rows {
                end = rows;
            }
            ranges.push(start..end);
            start = end;
        }
        ranges
    }

    fn back_pressure_target(
        mine: &VecDeque<u64>, open_batch: u64, completed: u64,
    ) -> Option<u64> {
        let mut unfinished = mine.iter().copied()
            .filter(|number| *number > completed && *number != open_batch);
        let oldest = unfinished.next()?;
        if unfinished.nth(1).is_some() { Some(oldest) } else { None }
    }

    fn measure_tensor_gpu_ns(
        context: &mut MetalContext,
        dispatch: impl Fn(&mut MetalContext) -> Result<(), String>,
    ) -> Result<f64, String> {
        // Three runs, the fastest kept: the first lets the GPU clocks ramp up.
        let mut fastest = f64::INFINITY;
        for _ in 0..3 {
            context.with_batch(|ctx| dispatch(ctx))?;
            context.wait_queue_idle()?;
            let command = context.last_command_buffer.as_ref()
                .ok_or("tensor calibration command buffer missing")?;
            let start: f64 = unsafe { msg_send![command.as_id(), GPUStartTime] };
            let end: f64 = unsafe { msg_send![command.as_id(), GPUEndTime] };
            if !start.is_finite() || !end.is_finite() || end <= start {
                return Err("tensor calibration GPU timestamps invalid".into());
            }
            fastest = fastest.min((end - start) * 1e9);
        }
        Ok(fastest)
    }

    fn measure_tensor_cost(context: &mut MetalContext) -> Result<TensorCost, String> {
        let elements = 8 * 1024 * 1024;
        let a = context.new_buffer_with_length(elements * 4)?;
        let b = context.new_buffer_with_length(elements * 4)?;
        let out = context.new_buffer_with_length(elements * 4)?;
        for buffer in [&a, &b] {
            let contents: *mut u8 = unsafe { msg_send![buffer.as_id(), contents] };
            if contents.is_null() {
                return Err("tensor calibration buffer contents unavailable".into());
            }
            unsafe { std::ptr::write_bytes(contents, 0, elements * 4); }
        }
        // Long enough (4.3 G multiply-adds, a few ms) to measure the GPU at its working clocks.
        let ns_per_mac = measure_tensor_gpu_ns(context, |ctx| {
            ctx.matmul_nt_into_buffer_offsets(
                GGML_TYPE_F32, a.as_id(), b.as_id(), out.as_id(),
                1024, 2048, 2048, [0; 3])
        })? / (1024.0 * 2048.0 * 2048.0);
        let shape = shape4_from_row_major(&[elements], 4)?;
        let ns_per_byte = measure_tensor_gpu_ns(context, |ctx| {
            ctx.dispatch_bin_f32_offsets(
                0, a.as_id(), b.as_id(), out.as_id(), &shape, &shape, [0; 3])
        })? / (3 * elements * 4) as f64;
        let shape = shape4_from_row_major(&[1024], 4)?;
        let dispatch_ns = measure_tensor_gpu_ns(context, |ctx| {
            for _ in 0..256 {
                ctx.dispatch_bin_f32_offsets(
                    0, a.as_id(), b.as_id(), out.as_id(), &shape, &shape, [0; 3])?;
            }
            Ok(())
        })?;
        let ns_per_dispatch =
            ((dispatch_ns - (256 * 3 * 1024 * 4) as f64 * ns_per_byte) / 256.0).max(0.0);
        let mut ns_per_commit = f64::INFINITY;
        for _ in 0..2 {
            let mut commands = Vec::new();
            for _ in 0..32 {
                let command: ObjcId = unsafe {
                    msg_send![context.command_queue.as_id(), commandBuffer]
                };
                commands.push(unsafe { StrongId::from_unowned(command) }
                    .ok_or("tensor calibration commandBuffer returned nil")?);
            }
            let start = std::time::Instant::now();
            for command in &commands {
                unsafe { let _: () = msg_send![command.as_id(), commit]; }
            }
            ns_per_commit = ns_per_commit.min(start.elapsed().as_nanos() as f64 / 32.0);
            context.wait_queue_idle()?; // One queue: this waits for all 32.
            drop(commands);
        }
        if !ns_per_commit.is_finite() || ns_per_commit <= 0.0 {
            return Err("tensor calibration commit timing invalid".into());
        }
        context.last_command_buffer.take();
        eprintln!(
            "[metal] tensor slices: {:.0} GMAC/s, {:.0} GB/s, {:.1} us/dispatch, {:.1} us/commit, slice {} ms",
            1.0 / ns_per_mac, 1.0 / ns_per_byte, ns_per_dispatch / 1000.0,
            ns_per_commit / 1000.0, SLICE_NS / 1_000_000.0);
        Ok(TensorCost { ns_per_mac, ns_per_byte, ns_per_dispatch, ns_per_commit })
    }

    struct TensorState {
        context: MetalContext,
        lock_phases: TensorLockPhases,
        returns: mpsc::Receiver<ReturnedBuffer>,
        free: Vec<ReturnedBuffer>,
        deferred_drop: Vec<StrongId>,
        dead_owners: makepad_ai_loader::DeadOwnerInbox,
        in_flight: VecDeque<(u64, StrongId)>,
        next_batch: u64,
        open_batch: u64,
        open_ns: f64,
        thread_batches: HashMap<std::thread::ThreadId, VecDeque<u64>>,
        completed_batch: u64,
        failed: BTreeSet<u64>,
        failed_oom: BTreeSet<u64>,
        failure_floor: Option<u64>,
        #[cfg(test)]
        injected_failure: Option<(u64, bool)>,
        live_writers: BTreeMap<u64, usize>,
    }

    struct TensorRuntime {
        state: Mutex<TensorState>,
        /// Measured once at init; read without the lock.
        cost: TensorCost,
        device: StrongId,
        library: StrongId,
        queue: StrongId,
        returns: mpsc::Sender<ReturnedBuffer>,
    }

    static TENSOR_CONTEXT: OnceLock<Result<TensorRuntime, String>> = OnceLock::new();
    static TENSOR_LOCK_HIGH_WATER_NS: AtomicU64 = AtomicU64::new(0);
    #[derive(Clone, Copy, Default, Debug)]
    struct TensorLockPhases {
        open_ns: u64,
        encode_ns: u64,
        commit_ns: u64,
        dispatches: usize,
    }
    #[derive(Clone, Copy)]
    struct TensorLockMaximum {
        ns: u64,
        dispatches: usize,
    }
    struct TensorLockSite {
        first_sequence: u64,
        first: TensorLockMaximum,
        later: TensorLockMaximum,
        calls: u64,
    }
    static TENSOR_LOCK_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    static TENSOR_LOCK_SITES: Mutex<BTreeMap<(&'static std::panic::Location<'static>, &'static str), TensorLockSite>> = Mutex::new(BTreeMap::new());
    static TENSOR_COW_COPIES: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn tensor_available() -> bool {
        !matches!(TENSOR_CONTEXT.get(), Some(Err(_)))
    }

    pub(crate) fn tensor_profile() -> (u64, u64) {
        let records: Vec<_> = {
            let sites = TENSOR_LOCK_SITES.lock().unwrap_or_else(|e| e.into_inner());
            sites.iter().map(|((location, phase), site)|
                (*location, *phase, site.first, site.later, site.calls)).collect()
        };
        for (location, phase, first, later, calls) in records {
            eprintln!("tensor lock: site={location}, phase={phase}, calls={calls}, first_ns={}, first_dispatches={}, later_max_ns={}, later_dispatches={}",
                first.ns, first.dispatches, later.ns, later.dispatches);
        }
        (TENSOR_LOCK_HIGH_WATER_NS.load(Ordering::Relaxed), TENSOR_COW_COPIES.load(Ordering::Relaxed))
    }

    pub(crate) fn tensor_cow_record() { TENSOR_COW_COPIES.fetch_add(1, Ordering::Relaxed); }

    fn tensor_runtime() -> Result<&'static TensorRuntime, String> {
        tensor_thread_guard()?;
        TENSOR_CONTEXT.get_or_init(|| {
            // OnceLock serialises initialisation, before any context mutex exists.
            let _pool = AutoreleasePool::new();
            let mut context = MetalContext::new()?;
            // Pay the driver's first command-buffer, encoder and commit setup
            // before creating the mutex used for tensor bookkeeping.
            context.begin_batch()?;
            context.end_batch()?;
            context.wait_queue_idle()?;
            context.last_command_buffer.take();
            let cost = measure_tensor_cost(&mut context)?;
            let (returns, receiver) = mpsc::channel();
            Ok(TensorRuntime {
                device: context.device.clone(), library: context.library.clone(),
                queue: context.command_queue.clone(), returns, cost,
                state: Mutex::new(TensorState {
                    context, lock_phases: TensorLockPhases::default(), returns: receiver, free: Vec::new(), deferred_drop: Vec::new(), dead_owners: makepad_ai_loader::DeadOwnerInbox::register(), in_flight: VecDeque::new(),
                    next_batch: 1, open_batch: 0, open_ns: 0.0,
                    thread_batches: HashMap::new(), completed_batch: 0,
                    failed: BTreeSet::new(), failed_oom: BTreeSet::new(), failure_floor: None,
                    #[cfg(test)]
                    injected_failure: None,
                    live_writers: BTreeMap::new(),
                }),
            })
        }).as_ref().map_err(Clone::clone)
    }

    #[track_caller]
    fn tensor_locked<T>(f: impl FnOnce(&mut TensorState) -> Result<T, String>) -> Result<T, String> {
        tensor_locked_in(tensor_runtime()?, f)
    }

    #[track_caller]
    fn tensor_locked_in<T>(runtime: &TensorRuntime, f: impl FnOnce(&mut TensorState) -> Result<T, String>) -> Result<T, String> {
        tensor_thread_guard()?;
        let _pool = AutoreleasePool::new();
        let mut state = match runtime.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                let open = state.open_batch;
                if open != 0 { state.record_failure(open, false); }
                runtime.state.clear_poison();
                state
            }
        };
        let start = std::time::Instant::now();
        let sequence = TENSOR_LOCK_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        state.lock_phases = TensorLockPhases::default();
        let result = f(&mut state);
        let released = std::mem::take(&mut state.deferred_drop);
        let phases = state.lock_phases;
        let elapsed = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        drop(state);
        let location = std::panic::Location::caller();
        {
            let mut sites = TENSOR_LOCK_SITES.lock().unwrap_or_else(|e| e.into_inner());
            for (phase, ns) in [("hold", elapsed), ("open", phases.open_ns),
                ("encode", phases.encode_ns), ("commit", phases.commit_ns)] {
                if phase != "hold" && ns == 0 { continue; }
                let sample = TensorLockMaximum { ns, dispatches: phases.dispatches };
                match sites.entry((location, phase)) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(TensorLockSite { first_sequence: sequence, first: sample,
                            later: TensorLockMaximum { ns: 0, dispatches: 0 }, calls: 1 });
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        let site = entry.get_mut();
                        // Recording happens after unlocking; another thread may record first.
                        let later = if sequence < site.first_sequence {
                            site.first_sequence = sequence;
                            std::mem::replace(&mut site.first, sample)
                        } else { sample };
                        if later.ns > site.later.ns { site.later = later; }
                        site.calls += 1;
                    }
                }
            }
        }
        TENSOR_LOCK_HIGH_WATER_NS.fetch_max(elapsed, Ordering::Relaxed);
        drop(released);
        result
    }

    impl TensorState {
        fn record_failure(&mut self, number: u64, oom: bool) {
            if oom { self.failed_oom.insert(number); }
            if self.failed.insert(number) {
                self.failure_floor = Some(self.failure_floor.map_or(number, |old| old.min(number)));
            }
        }
        fn remove_writer(&mut self, batch: u64) {
            if let Some(count) = self.live_writers.get_mut(&batch) {
                *count -= 1;
                if *count == 0 { self.live_writers.remove(&batch); }
            }
        }
        fn drain_returns(&mut self) {
            self.dead_owners.take(|owner| {
                let keys: Vec<_> = self.context.named_weight_buffers.keys()
                    .filter(|(ns, _)| ns.starts_with(owner)).cloned().collect();
                for key in keys {
                    if let Some(buffer) = self.context.named_weight_buffers.remove(&key) {
                        self.deferred_drop.push(buffer);
                    }
                }
            });
            while let Ok(mut buffer) = self.returns.try_recv() {
                self.remove_writer(buffer.writer_batch);
                buffer.writer_batch = 0;
                if buffer.recycle { self.free.push(buffer); }
                else { self.deferred_drop.push(buffer.object); }
            }
            if let Some(first) = self.failure_floor {
                if self.live_writers.range(first..).next().is_none() { self.failure_floor = None; }
            }
        }
        fn check_writer(&self, writer: u64) -> Result<(), String> {
            if writer != 0 {
                if let Some(first) = self.failure_floor {
                    if writer >= first {
                        let suffix = if self.failed_oom.contains(&first) { " (out of memory)" } else { "" };
                        return Err(format!("Metal tensor writer batch {writer} depends on failed batch {first}{suffix}"));
                    }
                }
            }
            Ok(())
        }
        fn set_writer(&mut self, buffer: &TensorBuffer, batch: u64) {
            let old = buffer.writer_batch.swap(batch, Ordering::AcqRel);
            self.remove_writer(old);
            if batch != 0 { *self.live_writers.entry(batch).or_default() += 1; }
        }
        fn prune_thread_batches(&mut self) {
            let completed = self.completed_batch;
            self.thread_batches.retain(|_, mine| {
                while mine.front().is_some_and(|number| *number <= completed) {
                    mine.pop_front();
                }
                !mine.is_empty()
            });
        }
        fn back_pressure_command(&self, thread: std::thread::ThreadId) -> Option<(u64, StrongId)> {
            let mine = self.thread_batches.get(&thread)?;
            let target = back_pressure_target(mine, self.open_batch, self.completed_batch)?;
            self.in_flight.iter().find(|(number, _)| *number == target)
                .map(|(number, command)| (*number, command.clone()))
        }
        fn reap(&mut self, retired: &mut Vec<StrongId>) {
            while let Some((number, command)) = self.in_flight.front() {
                let status: u64 = unsafe { msg_send![command.as_id(), status] };
                if status < 4 { break; }
                let number = *number;
                let failed = status == 5;
                let oom = failed && command_out_of_memory(command);
                #[cfg(test)]
                let (failed, oom) = match self.injected_failure {
                    Some((batch, injected_oom)) if batch == number => (true, oom || injected_oom),
                    _ => (failed, oom),
                };
                if failed { self.record_failure(number, oom); }
                self.completed_batch = self.completed_batch.max(number);
                if let Some((_, command)) = self.in_flight.pop_front() { retired.push(command); }
            }
            self.prune_thread_batches();
        }
        fn commit(&mut self, retired: &mut Vec<StrongId>) -> Result<(), String> {
            let start = std::time::Instant::now();
            self.reap(retired);
            if self.open_batch == 0 { return Ok(()); }
            let number = self.open_batch;
            let result = self.context.end_batch();
            self.lock_phases.commit_ns += start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            result?;
            let command = self.context.last_command_buffer.take()
                .ok_or_else(|| "committed tensor batch missing".to_string())?;
            self.in_flight.push_back((number, command));
            self.open_batch = 0;
            self.open_ns = 0.0;
            Ok(())
        }
    }

    pub(crate) fn tensor_alloc_bytes(bytes: usize) -> usize {
        bytes.max(4)
    }

    pub(crate) fn tensor_buffer(bytes: usize, host: bool) -> Result<Arc<TensorBuffer>, String> {
        let runtime = tensor_runtime()?;
        let bytes = tensor_alloc_bytes(bytes);
        let mut discarded = Vec::new();
        let pooled = tensor_locked(|state| {
            state.drain_returns();
            let index = state.free.iter().position(|b| b.capacity == bytes
                && (!host || b.last_batch <= state.completed_batch));
            let buffer = index.map(|i| state.free.swap_remove(i));
            let mut retained: usize = state.free.iter().map(|b| b.capacity).sum();
            while retained > POOL_CAP_BYTES {
                if let Some(buffer) = state.free.pop() {
                    retained -= buffer.capacity;
                    discarded.push(buffer);
                } else { break; }
            }
            Ok(buffer)
        })?;
        drop(discarded);
        let (object, last_batch) = if let Some(buffer) = pooled {
            (buffer.object, buffer.last_batch)
        } else {
            let _pool = AutoreleasePool::new();
            let object = unsafe {
                let id: ObjcId = msg_send![runtime.device.as_id(), newBufferWithLength: bytes as u64 options: MTL_RESOURCE_STORAGE_MODE_SHARED];
                StrongId::from_owned(id)
            }.ok_or_else(|| format!("Metal tensor allocation failed for {bytes} bytes (out of memory)"))?;
            (object, 0)
        };
        Ok(Arc::new(TensorBuffer {
            object: Some(object), capacity: bytes, last_batch: AtomicU64::new(last_batch),
            writer_batch: AtomicU64::new(0), returns: runtime.returns.clone(), recycle: true,
        }))
    }

    pub(crate) fn tensor_host_written(buffer: &TensorBuffer) -> Result<(), String> {
        tensor_locked(|state| { state.set_writer(buffer, 0); Ok(()) })
    }

    pub(crate) fn tensor_flush_until(number: u64) -> Result<(), String> {
        if number == 0 { return tensor_thread_guard(); }
        tensor_flush_in(tensor_runtime()?, number)
    }

    fn tensor_flush_in(runtime: &TensorRuntime, number: u64) -> Result<(), String> {
        let mut retired = Vec::new();
        let commands = tensor_locked_in(runtime, |state| {
            state.drain_returns();
            if number != 0 && state.open_batch != 0 && state.open_batch <= number {
                state.commit(&mut retired)?;
            }
            state.reap(&mut retired);
            Ok(state.in_flight.iter().take_while(|(batch, _)| *batch <= number)
                .map(|(batch, command)| (*batch, command.clone())).collect::<Vec<_>>())
        })?;
        drop(retired);
        wait_committed(runtime, commands)?;
        tensor_locked_in(runtime, |state| state.check_writer(number))
    }

    fn command_out_of_memory(command: &StrongId) -> bool {
        unsafe {
            let error: ObjcId = msg_send![command.as_id(), error];
            if error == nil { return false; }
            let code: isize = msg_send![error, code];
            code == 8 // MTLCommandBufferErrorOutOfMemory
        }
    }

    fn wait_committed(runtime: &TensorRuntime, commands: Vec<(u64, StrongId)>) -> Result<(), String> {
        for (batch, command) in commands {
            let _pool = AutoreleasePool::new();
            unsafe { let _: () = msg_send![command.as_id(), waitUntilCompleted]; }
            let status: u64 = unsafe { msg_send![command.as_id(), status] };
            let oom = status == 5 && command_out_of_memory(&command);
            tensor_locked_in(runtime, |state| {
                let failed = status == 5;
                #[cfg(test)]
                let (failed, oom) = match state.injected_failure {
                    Some((number, injected_oom)) if number == batch => (true, oom || injected_oom),
                    _ => (failed, oom),
                };
                if failed { state.record_failure(batch, oom); }
                state.completed_batch = state.completed_batch.max(batch);
                state.prune_thread_batches();
                Ok(())
            })?;
        }
        Ok(())
    }

    pub(crate) fn tensor_pool_clear() -> Result<(), String> {
        tensor_pool_clear_in(tensor_runtime()?)
    }

    fn tensor_pool_clear_in(runtime: &TensorRuntime) -> Result<(), String> {
        let mut retired = Vec::new();
        let buffers = tensor_locked_in(runtime, |state| {
            state.drain_returns();
            // Completed commands still retain every buffer they used (the last batch stays in
            // `in_flight` until the next commit): retire them so their buffers come back now.
            state.reap(&mut retired);
            Ok(std::mem::take(&mut state.free))
        })?;
        drop(buffers); // Unfinished commands retain their resources; clearing never waits.
        drop(retired);
        Ok(())
    }

    pub(crate) fn tensor_evict_owner(owner: &WeightOwner) -> Result<usize, String> {
        let buffers = tensor_locked(|state| {
            state.drain_returns();
            let keys: Vec<_> = state.context.named_weight_buffers.keys()
                .filter(|(namespace, _)| namespace.starts_with(owner.namespace())).cloned().collect();
            Ok(keys.into_iter().filter_map(|key| state.context.named_weight_buffers.remove(&key)).collect::<Vec<_>>())
        })?;
        let bytes = buffers.iter().map(|buffer| unsafe {
            let bytes: u64 = msg_send![buffer.as_id(), length]; bytes as usize
        }).sum();
        drop(buffers);
        Ok(bytes)
    }

    pub(crate) fn tensor_evict_keys(prefix: &WeightKey) -> Result<usize, String> {
        let buffers = tensor_locked(|state| {
            state.drain_returns();
            let keys: Vec<_> = state.context.named_weight_buffers.keys()
                .filter(|(namespace, key)| format!("{namespace}::{key}").starts_with(prefix.as_str())).cloned().collect();
            Ok(keys.into_iter().filter_map(|key| state.context.named_weight_buffers.remove(&key)).collect::<Vec<_>>())
        })?;
        let bytes = buffers.iter().map(|buffer| unsafe {
            let bytes: u64 = msg_send![buffer.as_id(), length]; bytes as usize
        }).sum();
        drop(buffers);
        Ok(bytes)
    }

    impl MetalContext {
        /// A per-operation preparation record. It shares the process device,
        /// queue and pipelines; it never opens or commits its own tensor batch.
        fn tensor_preparation() -> Result<Self, String> {
            let runtime = tensor_runtime()?;
            Ok(Self {
                tensor_mode: true, tensor_prepare: true, tensor_batch: 0, tensor_dispatches: 0, tensor_bindings: HashMap::new(),
                device: runtime.device.clone(), command_queue: runtime.queue.clone(), library: runtime.library.clone(),
                pipeline_cache: HashMap::new(), cached_weight_buffers: HashMap::new(), named_weight_buffers: HashMap::new(),
                scratch_buffers: HashMap::new(), matmul_out_buffers: HashMap::new(),
                batch_depth: 0,
                batch_command_buffer: None, batch_encoder: None, last_command_buffer: None,
                pool_free: Vec::new(), pool_in_flight: Vec::new(),
            })
        }

        fn bind_buffer(&self, encoder: ObjcId, buffer: ObjcId, offset: usize, index: u64) -> Result<(), String> {
            if self.tensor_mode {
                let handle = self.tensor_bindings.get(&(buffer as usize))
                    .ok_or_else(|| "tensor dispatch bound an unregistered buffer".to_string())?;
                handle.last_batch.fetch_max(self.tensor_batch, Ordering::Release);
            }
            unsafe { let _: () = msg_send![encoder, setBuffer: buffer offset: offset as u64 atIndex: index]; }
            Ok(())
        }
    }

    #[track_caller]
    fn tensor_encode(
        macs: u64, buffers: &[&Arc<TensorBuffer>], output: &Arc<TensorBuffer>,
        dispatch: impl Fn(&mut MetalContext) -> Result<(), String>,
    ) -> Result<u64, String> {
        tensor_encode_group(macs, buffers, &[output], dispatch)
    }

    #[track_caller]
    fn tensor_encode_group(
        macs: u64, inputs: &[&Arc<TensorBuffer>], outputs: &[&Arc<TensorBuffer>],
        dispatch: impl Fn(&mut MetalContext) -> Result<(), String>,
    ) -> Result<u64, String> {
        let _pool = AutoreleasePool::new();
        let runtime = tensor_runtime()?;
        let thread = std::thread::current().id();
        let bytes = inputs.iter().chain(outputs).map(|buffer| buffer.capacity as u64).sum();
        let mut prepared = MetalContext::tensor_preparation()?;
        for buffer in inputs.iter().chain(outputs).copied() {
            prepared.tensor_bindings.insert(buffer.id() as usize, buffer.clone());
        }
        dispatch(&mut prepared)?; // Compile and allocate, without the encoding lock.
        prepared.tensor_prepare = false;
        let mut retired = Vec::new();
        let result = tensor_locked_in(runtime, |state| {
            state.drain_returns();
            for buffer in inputs { state.check_writer(buffer.writer_batch.load(Ordering::Acquire))?; }
            if state.open_batch == 0 {
                let start = std::time::Instant::now();
                let result = state.context.begin_batch();
                state.lock_phases.open_ns = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
                result?;
                state.open_batch = state.next_batch;
                state.next_batch += 1;
            }
            let number = state.open_batch;
            prepared.batch_depth = 1;
            prepared.batch_command_buffer = state.context.batch_command_buffer.clone();
            prepared.batch_encoder = state.context.batch_encoder.clone();
            prepared.tensor_batch = number;
            for output in outputs { state.set_writer(output, number); }
            let start = std::time::Instant::now();
            let encoded = dispatch(&mut prepared);
            state.lock_phases.encode_ns = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            state.lock_phases.dispatches = prepared.tensor_dispatches;
            if let Err(error) = encoded {
                state.record_failure(number, false);
                return Err(error);
            }
            let mine = state.thread_batches.entry(thread).or_default();
            if mine.back() != Some(&number) { mine.push_back(number); }
            state.open_ns += estimate_ns(&runtime.cost, macs, bytes, prepared.tensor_dispatches);
            state.reap(&mut retired);
            if should_commit(state.open_ns, state.in_flight.is_empty(), &runtime.cost) {
                state.commit(&mut retired)?;
            }
            Ok((number, state.back_pressure_command(thread)))
        });
        drop(retired);
        let (number, mut waiting) = result?;
        while let Some(command) = waiting {
            wait_committed(runtime, vec![command])?;
            let mut retired = Vec::new();
            let next = tensor_locked_in(runtime, |state| {
                state.reap(&mut retired);
                state.check_writer(number)?;
                Ok(state.back_pressure_command(thread))
            });
            drop(retired);
            waiting = next?;
        }
        Ok(number)
    }

    /// Copy a logical 4-D tensor into fresh pool storage. Dimensions are in
    /// ggml order (innermost first); offsets and strides are bytes.
    pub(crate) fn tensor_copy_strided(
        src: &Arc<TensorBuffer>, src_offset: usize, src_ne: [usize; 4], src_nb: [usize; 4],
        dst: &Arc<TensorBuffer>, dst_offset: usize, dst_ne: [usize; 4], dst_nb: [usize; 4],
    ) -> Result<u64, String> {
        let overflow = || "metal strided copy extent overflow".to_string();
        let count = |ne: [usize; 4]| ne.into_iter().try_fold(1usize, |n, d| n.checked_mul(d).ok_or_else(overflow));
        let elements = count(src_ne)?;
        if elements != count(dst_ne)? || dst_nb[0] != 4
            || (elements != 0 && dst_ne[0] % src_ne[0] != 0) {
            return Err("metal strided copy requires matching extents and contiguous destination rows".into());
        }
        for (buffer, offset, ne, nb) in [(src, src_offset, src_ne, src_nb), (dst, dst_offset, dst_ne, dst_nb)] {
            if offset % 4 != 0 || nb.into_iter().any(|stride| stride % 4 != 0) {
                return Err("metal strided copy requires f32 alignment".into());
            }
            let end = if elements == 0 { offset } else {
                ne.into_iter().zip(nb).try_fold(offset, |end, (size, stride)| {
                    (size - 1).checked_mul(stride).and_then(|n| end.checked_add(n)).ok_or_else(overflow)
                })?.checked_add(4).ok_or_else(overflow)?
            };
            if end > buffer.capacity { return Err("metal strided copy outside buffer".into()); }
        }
        if elements == 0 { tensor_thread_guard()?; return Ok(0); }
        // The kernel's grid coordinates and row index use 32-bit integers.
        for size in src_ne.into_iter().chain(dst_ne) {
            i32::try_from(size).map_err(|_| overflow())?;
        }
        i64::try_from(elements).map_err(|_| overflow())?;
        let args = KArgsCpy {
            nk0: src_ne[0] as i64,
            ne00: src_ne[0] as i64, ne01: src_ne[1] as i64,
            ne02: src_ne[2] as i64, ne03: src_ne[3] as i64,
            nb00: src_nb[0] as u64, nb01: src_nb[1] as u64,
            nb02: src_nb[2] as u64, nb03: src_nb[3] as u64,
            ne0: dst_ne[0] as i64, ne1: dst_ne[1] as i64,
            ne2: dst_ne[2] as i64, ne3: dst_ne[3] as i64,
            nb0: dst_nb[0] as u64, nb1: dst_nb[1] as u64,
            nb2: dst_nb[2] as u64, nb3: dst_nb[3] as u64,
        };
        tensor_encode(0, &[src], dst, |ctx| {
            // This scalar kernel also handles offsets not divisible by 16.
            // It never makes float4 loads, even for aligned tensors.
            let base = "kernel_cpy_f32_f32";
            let (pipeline, _, _, _, _) = ctx.get_or_compile_cached_pipeline(
                base.to_string(), base, &[], 0, 0, 0, 0)?;
            let nth = (src_ne[0] as u64).min(MetalContext::pipeline_max_threads(pipeline).max(1));
            let groups = (src_ne[0] as u64).div_ceil(nth)
                .checked_mul(src_ne[1] as u64).ok_or_else(overflow)?;
            u32::try_from(groups).map_err(|_| overflow())?;
            if ctx.tensor_prepare { return Ok(()); }
            let (_, encoder, handles) = ctx.begin_command_encoder()?;
            unsafe {
                let _: () = msg_send![encoder, setComputePipelineState: pipeline];
                let _: () = msg_send![encoder, setBytes: &args as *const KArgsCpy as *const c_void
                    length: std::mem::size_of::<KArgsCpy>() as u64 atIndex: 0u64];
                ctx.bind_buffer(encoder, src.id(), src_offset, 1)?;
                ctx.bind_buffer(encoder, dst.id(), dst_offset, 2)?;
                let grid = MTLSize { width: groups, height: src_ne[2] as u64, depth: src_ne[3] as u64 };
                let threads = MTLSize { width: nth, height: 1, depth: 1 };
                let _: () = msg_send![encoder, dispatchThreadgroups: grid threadsPerThreadgroup: threads];
            }
            ctx.end_command_encoder(handles)
        })
    }

    pub(crate) fn tensor_binary(
        a: &Arc<TensorBuffer>, a_offset: usize, a_shape: &Shape4,
        b: &Arc<TensorBuffer>, b_offset: usize, b_shape: &Shape4,
        out: &Arc<TensorBuffer>, multiply: bool,
    ) -> Result<u64, String> {
        tensor_encode(0, &[a, b], out, |ctx| ctx.dispatch_bin_f32_offsets(
            if multiply { 2 } else { 0 }, a.id(), b.id(), out.id(), a_shape, b_shape, [a_offset, b_offset, 0]))
    }

    pub(crate) fn tensor_unary(
        x: &Arc<TensorBuffer>, offset: usize, shape: &Shape4, out: &Arc<TensorBuffer>, op: i16,
    ) -> Result<u64, String> {
        tensor_encode(0, &[x], out, |ctx| ctx.dispatch_unary_f32_offsets(op, x.id(), out.id(), shape, [offset, 0]))
    }

    fn check_norm_affine(x: &Shape4, affine: &Shape4) -> Result<(), String> {
        if affine.ne[0] != x.ne[0] {
            return Err("norm fuse ne0 mismatch".into());
        }
        for d in 1..4 {
            if affine.ne[d] != 1 && affine.ne[d] != x.ne[d] {
                return Err("norm fuse broadcast mismatch".into());
            }
        }
        Ok(())
    }

    pub(crate) fn tensor_norm(
        x: &Arc<TensorBuffer>, offset: usize, shape: &Shape4,
        mul: &Arc<TensorBuffer>, mul_offset: usize, mul_shape: &Shape4,
        add: &Arc<TensorBuffer>, add_offset: usize, add_shape: &Shape4,
        out: &Arc<TensorBuffer>, eps: f32, n_fuse: i32,
    ) -> Result<u64, String> {
        if n_fuse >= 2 { check_norm_affine(shape, mul_shape)?; }
        if n_fuse >= 3 { check_norm_affine(shape, add_shape)?; }
        tensor_encode(0, &[x, mul, add], out, |ctx| ctx.dispatch_norm_f32_offsets(
            x.id(), mul.id(), add.id(), out.id(), shape, mul_shape, add_shape, eps, n_fuse,
            [offset, mul_offset, add_offset, 0]))
    }

    pub(crate) fn tensor_rms_norm(
        x: &Arc<TensorBuffer>, offset: usize, shape: &Shape4,
        mul: &Arc<TensorBuffer>, mul_offset: usize, mul_shape: &Shape4,
        add: &Arc<TensorBuffer>, add_offset: usize, add_shape: &Shape4,
        out: &Arc<TensorBuffer>, eps: f32, n_fuse: i32,
    ) -> Result<u64, String> {
        if n_fuse >= 2 { check_norm_affine(shape, mul_shape)?; }
        if n_fuse >= 3 { check_norm_affine(shape, add_shape)?; }
        tensor_encode(0, &[x, mul, add], out, |ctx| ctx.dispatch_rms_norm_f32_offsets(
            x.id(), mul.id(), add.id(), out.id(), shape, mul_shape, add_shape, eps, n_fuse,
            [offset, mul_offset, add_offset, 0]))
    }

    fn get_rows_shapes(
        src_ggml_type: u32,
        n_cols: usize,
        n_rows: usize,
        n_indices: usize,
    ) -> Result<(Shape4, Shape4, Shape4), String> {
        let src_ty = src0_type_from_ggml(src_ggml_type)
            .ok_or_else(|| format!("unsupported ggml type for get_rows: {}", src_ggml_type))?;
        let (row_bytes, elem_or_block) = src0_layout_bytes_per_row(src_ty, n_cols)?;
        let source_bytes = n_rows
            .checked_mul(row_bytes)
            .ok_or_else(|| "overflow computing get_rows source bytes".to_string())?;
        let mut src_shape = shape4_from_row_major(&[n_rows, n_cols], elem_or_block)?;
        // Quantized rows contain blocks, not one block per logical element.
        src_shape.nb[1] = row_bytes as u64;
        src_shape.nb[2] = source_bytes as u64;
        src_shape.nb[3] = src_shape.nb[2];
        let idx_shape = shape4_from_row_major(&[n_indices], std::mem::size_of::<i32>() as u64)?;
        let dst_shape = shape4_from_row_major(&[n_indices, n_cols], 4)?;
        Ok((src_shape, idx_shape, dst_shape))
    }

    pub(crate) fn tensor_get_rows(
        src: &Arc<TensorBuffer>,
        src_offset: usize,
        src_ggml_type: u32,
        n_cols: usize,
        n_rows: usize,
        rows: &Arc<TensorBuffer>,
        rows_offset: usize,
        n_indices: usize,
        out: &Arc<TensorBuffer>,
    ) -> Result<u64, String> {
        let (src_shape, idx_shape, dst_shape) =
            get_rows_shapes(src_ggml_type, n_cols, n_rows, n_indices)?;
        tensor_encode(0, &[src, rows], out, |ctx| {
            ctx.dispatch_get_rows_ggml(
                src_ggml_type, src.id(), &src_shape, rows.id(), &idx_shape,
                out.id(), &dst_shape, [src_offset, rows_offset, 0],
            )
        })
    }

    fn tensor_upload_bytes(values: &[u8]) -> Result<Arc<TensorBuffer>, String> {
        let buffer = tensor_buffer(values.len(), true)?;
        unsafe {
            std::ptr::copy_nonoverlapping(
                values.as_ptr(), buffer.contents()?.cast::<u8>(), values.len(),
            );
        }
        Ok(buffer)
    }

    fn tensor_upload_f32(values: &[f32], shape: &Shape4) -> Result<Arc<TensorBuffer>, String> {
        if values.len() != shape.numel { return Err("metal upload size mismatch".into()); }
        let bytes = shape.numel.checked_mul(4).ok_or("metal upload extent overflow")?;
        let buffer = tensor_buffer(bytes, true)?;
        unsafe { std::ptr::copy_nonoverlapping(values.as_ptr(), buffer.contents()?, values.len()); }
        Ok(buffer)
    }

    fn tensor_host_result(shape: &Shape4, dispatch: impl FnOnce(&Arc<TensorBuffer>) -> Result<u64, String>) -> Result<Vec<f32>, String> {
        let bytes = shape.numel.checked_mul(4).ok_or("metal output extent overflow")?;
        let output = tensor_buffer(bytes, false)?;
        tensor_flush_until(dispatch(&output)?)?;
        Ok(unsafe { std::slice::from_raw_parts(output.contents()?, shape.numel) }.to_vec())
    }

    fn tensor_try<T>(operation: impl FnOnce() -> Result<T, String>) -> Option<T> {
        match operation() {
            Ok(value) => Some(value),
            Err(error) => {
                super::log_metal_error_once(format!("[ggml][metal] compute failed: {error}"));
                None
            }
        }
    }

    fn tensor_host_binary(a: &[f32], a_shape: &[usize], b: &[f32], b_shape: &[usize], multiply: bool) -> Option<Vec<f32>> {
        tensor_try(|| {
            let a_shape = shape4_from_row_major(a_shape, 4)?;
            let b_shape = shape4_from_row_major(b_shape, 4)?;
            let a = tensor_upload_f32(a, &a_shape)?;
            let b = tensor_upload_f32(b, &b_shape)?;
            tensor_host_result(&a_shape, |out| tensor_binary(&a, 0, &a_shape, &b, 0, &b_shape, out, multiply))
        })
    }

    pub(crate) fn tensor_matmul(
        x: &Arc<TensorBuffer>, x_offset: usize, w: &Arc<TensorBuffer>, w_offset: usize,
        out: &Arc<TensorBuffer>, m: usize, k: usize, n: usize, ty: u32,
    ) -> Result<u64, String> {
        let macs = m as u64 * k as u64 * n as u64;
        let bytes = x.capacity as u64 + w.capacity as u64 + out.capacity as u64;
        let est = estimate_ns(&tensor_runtime()?.cost, macs, bytes, 1);
        let rows = if m > 8 && k >= 64 {
            row_chunks(m, est, 32, 32)
        } else { vec![0..m] };
        tensor_matmul_rows(x, x_offset, w, w_offset, out, m, k, n, ty, &rows)
    }

    fn tensor_matmul_rows(
        x: &Arc<TensorBuffer>, x_offset: usize, w: &Arc<TensorBuffer>, w_offset: usize,
        out: &Arc<TensorBuffer>, m: usize, k: usize, n: usize, ty: u32,
        rows: &[std::ops::Range<usize>],
    ) -> Result<u64, String> {
        let src0 = src0_type_from_ggml(ty).ok_or_else(|| "unsupported tensor weight type".to_string())?;
        let (row, _) = src0_layout_bytes_per_row(src0, k)?;
        let fits = |offset: usize, rows: usize, bytes: usize, cap: usize| {
            rows.checked_mul(bytes).and_then(|n| n.checked_add(offset)).is_some_and(|end| end <= cap)
        };
        if !fits(w_offset, n, row, w.capacity) || !fits(x_offset, m, k.checked_mul(4).ok_or("tensor matmul overflow")?, x.capacity)
            || !fits(0, m, n.checked_mul(4).ok_or("tensor matmul overflow")?, out.capacity) {
            return Err("tensor matmul buffer extent mismatch".into());
        }
        let mut batch = 0;
        for r in rows {
            batch = tensor_encode(r.len() as u64 * k as u64 * n as u64, &[x, w], out,
                |ctx| ctx.matmul_nt_into_buffer_offsets(
                    ty, w.id(), x.id(), out.id(), r.len(), k, n,
                    [w_offset, x_offset + r.start * k * 4, r.start * n * 4]))?;
        }
        Ok(batch)
    }

    pub(crate) fn tensor_named_weight<'a>(owner: &WeightOwner, key: &str, load: impl FnOnce() -> Result<Cow<'a, [u8]>, String>) -> Result<Arc<TensorBuffer>, String> {
        tensor_named_weight_buffer((owner.namespace().to_owned(), key.to_owned()), load)
    }

    pub(crate) fn tensor_keyed_weight<'a>(key: &WeightKey, load: impl FnOnce() -> Result<Cow<'a, [u8]>, String>) -> Result<Arc<TensorBuffer>, String> {
        tensor_named_weight_buffer((key.as_str().to_owned(), "act-norm".to_owned()), load)
    }

    fn tensor_named_weight_buffer<'a>(key: (String, String), load: impl FnOnce() -> Result<Cow<'a, [u8]>, String>) -> Result<Arc<TensorBuffer>, String> {
        tensor_cached_weight_buffer(key, |device| {
            let bytes = load()?;
            unsafe {
                let obj: ObjcId = msg_send![device, newBufferWithBytes: bytes.as_ptr() as *const c_void length: bytes.len() as u64 options: MTL_RESOURCE_STORAGE_MODE_SHARED];
                StrongId::from_owned(obj)
            }.ok_or_else(|| "named tensor weight allocation failed (out of memory)".to_string())
        })
    }

    pub(crate) fn tensor_f32_weight(w: &F32Weight) -> Result<Arc<TensorBuffer>, String> {
        let key = (w.owner().namespace().to_owned(), w.name().to_owned());
        tensor_cached_weight_buffer(key, |device| {
            let storage = Arc::clone(w.storage());
            let bytes = storage.page_bytes();
            // Metal copies the block and holds its Arc until the buffer's
            // last reference (including in-flight commands) is released.
            unsafe {
                let deallocator = makepad_objc_sys::objc_block!(move |_pointer: *mut c_void, _length: u64| {
                    let _ = &storage;
                });
                let obj: ObjcId = msg_send![device, newBufferWithBytesNoCopy: w.storage().as_ptr() as *mut c_void length: bytes as u64 options: MTL_RESOURCE_STORAGE_MODE_SHARED deallocator: &deallocator];
                StrongId::from_owned(obj)
            }.ok_or_else(|| format!("f32 weight no-copy buffer failed for {bytes} bytes (out of memory)"))
        })
    }

    fn tensor_cached_weight_buffer(
        key: (String, String),
        create: impl FnOnce(ObjcId) -> Result<StrongId, String>,
    ) -> Result<Arc<TensorBuffer>, String> {
        let runtime = tensor_runtime()?;
        let existing = tensor_locked(|state| {
            state.drain_returns();
            Ok(state.context.named_weight_buffers.get(&key).cloned())
        })?;
        let object = if let Some(buffer) = existing { buffer } else {
            let _pool = AutoreleasePool::new();
            let candidate = create(runtime.device.as_id())?;
            tensor_locked(|state| {
                state.dead_owners.holds(&key.0);
                Ok(state.context.named_weight_buffers.entry(key)
                    .or_insert_with(|| candidate.clone()).clone())
            })?
        };
        let capacity: u64 = unsafe { msg_send![object.as_id(), length] };
        Ok(Arc::new(TensorBuffer { object: Some(object), capacity: capacity as usize,
            last_batch: AtomicU64::new(0), writer_batch: AtomicU64::new(0), returns: runtime.returns.clone(), recycle: false }))
    }

    pub(crate) fn tensor_attention(
        q: &Arc<TensorBuffer>, qo: usize, k: &Arc<TensorBuffer>, ko: usize,
        v: &Arc<TensorBuffer>, vo: usize, out: &Arc<TensorBuffer>,
        nq: usize, nk: usize, heads: usize, d: usize, scale: f32,
    ) -> Result<u64, String> {
        let rows = if !flash_attn_use_vec(nq, d) {
            let macs = 2 * nq as u64 * nk as u64 * heads as u64 * d as u64;
            let bytes = q.capacity as u64 + k.capacity as u64
                + v.capacity as u64 + out.capacity as u64;
            let dispatches = 1 + usize::from(nk % OP_FLASH_ATTN_EXT_NCPSG as usize != 0);
            let est = estimate_ns(&tensor_runtime()?.cost, macs, bytes, dispatches);
            // Chunks hold whole query tiles: a partial tile costs a full one in every chunk.
            row_chunks(nq, est, OP_FLASH_ATTN_EXT_NQPSG as usize, 20)
        } else { vec![0..nq] };
        tensor_attention_rows(q, qo, k, ko, v, vo, out, nq, nk, heads, d, scale, &rows)
    }

    fn tensor_attention_rows(
        q: &Arc<TensorBuffer>, qo: usize, k: &Arc<TensorBuffer>, ko: usize,
        v: &Arc<TensorBuffer>, vo: usize, out: &Arc<TensorBuffer>,
        nq: usize, nk: usize, heads: usize, d: usize, scale: f32,
        rows: &[std::ops::Range<usize>],
    ) -> Result<u64, String> {
        let vector = flash_attn_use_vec(nq, d);
        let mut batch = 0;
        for r in rows {
            let nq = r.len();
            let q_offset = qo + r.start * heads * d * 4;
            let out_offset = r.start * heads * d * 4;
            batch = tensor_encode(2 * nq as u64 * nk as u64 * heads as u64 * d as u64,
                &[q, k, v], out, |ctx| {
                let pad = ctx.get_or_create_scratch_buffer(SCRATCH_FLASH_PAD,
                    flash_attn_ext_extra_pad_bytes(nq, nk, heads, d, 4, false, vector)?)?;
                if vector {
                    let tmp = ctx.get_or_create_scratch_buffer(SCRATCH_FLASH_TMP,
                        flash_attn_ext_extra_tmp_bytes(nq, heads, d, 32)?)?;
                    ctx.dispatch_flash_attn_ext_vec_f32_offsets(
                        q.id(), k.id(), v.id(), q.id(), q.id(), pad, Some(tmp), out.id(),
                        nq, nk, heads, d, Src0Type::F32, scale, false, false, 0.0, 0.0,
                        [q_offset, ko, vo, q_offset, q_offset, 0, 0, out_offset])
                } else {
                    let blk = ctx.get_or_create_scratch_buffer(SCRATCH_FLASH_BLK,
                        flash_attn_ext_extra_blk_bytes(nq, nk, false, false)?)?;
                    ctx.dispatch_flash_attn_ext_f32_offsets(
                        q.id(), k.id(), v.id(), q.id(), q.id(), pad, blk, out.id(),
                        nq, nk, heads, d, Src0Type::F32, scale, false, false, 0.0, 0.0,
                        [q_offset, ko, vo, q_offset, q_offset, 0, 0, out_offset])
                }
            })?;
        }
        Ok(batch)
    }

    pub(crate) fn tensor_im2col(
        x: &Arc<TensorBuffer>, offset: usize, out: &Arc<TensorBuffer>,
        ic: usize, iw: usize, ih: usize, stride: usize, kw: usize, kh: usize,
        px: usize, py: usize, ow: usize, oh: usize,
    ) -> Result<u64, String> {
        tensor_encode(0, &[x], out, |ctx| ctx.dispatch_im2col_2d_f32_offsets(
            x.id(), 0, out.id(), ic, iw, ih, stride, kw, kh, px, py, ow, oh, [offset, 0]))
    }

    pub(crate) fn tensor_resident_weights(owner: &WeightOwnerTrace) -> Result<(usize, usize), String> {
        tensor_locked(|state| {
            state.drain_returns();
            Ok(state.context.named_weight_buffers.iter()
                .filter(|((namespace, _), _)| namespace.starts_with(owner.namespace()))
                .fold((0, 0), |(count, bytes), (_, buffer)| {
                    let length: u64 = unsafe { msg_send![buffer.as_id(), length] };
                    (count + 1, bytes + length as usize)
                }))
        })
    }

    #[cfg(test)]
    pub(crate) fn tensor_test_drop_owner_locked(owner: makepad_ai_loader::WeightOwner) -> Result<(), String> {
        tensor_locked(move |_| { drop(owner); Ok(()) })
    }

    #[cfg(test)]
    pub(crate) fn tensor_test_legacy_matmuls(a: &[f32], weights: &[(u32, Vec<u8>)],
        m: usize, k: usize, n: usize) -> Result<Vec<Vec<f32>>, String> {
        let _pool = AutoreleasePool::new();
        let mut legacy = MetalContext::new()?;
        weights.iter().map(|(ty, bytes)| {
            let (dst, mr, nr) = legacy.matmul_nt_ggml_bytes_impl(a, bytes, *ty, m, k, n, None)?;
            legacy.read_f32_buffer(dst.as_id(), mr * nr)
        }).collect()
    }

    #[cfg(test)]
    pub(crate) fn tensor_test_error_and_guard() -> Result<(), String> {
        assert!(tensor_thread_guard_for(true).unwrap_err().contains("main/UI"), "main-thread refusal");
        assert!(tensor_thread_guard_for(false).is_ok(), "worker allowed");
        let buffer = tensor_buffer(16, false)?;
        let result: Result<(), String> = tensor_locked(move |_| {
            drop(buffer);
            Err("injected op error".into())
        });
        assert!(result.is_err(), "drop under encoding lock returns normally");
        tensor_pool_clear()?;
        // Isolate injected failures from other tests, but exercise the same
        // commit/wait/read gate and return-channel accounting used in production.
        let (sender, returns) = mpsc::channel();
        let cost = tensor_runtime()?.cost;
        let mut context = MetalContext::tensor_preparation()?;
        context.tensor_mode = false;
        context.tensor_prepare = false;
        let first_object = context.new_buffer_with_length(16)?;
        let second_object = context.new_buffer_with_length(16)?;
        let first = Arc::new(TensorBuffer { object: Some(first_object), capacity: 16,
            last_batch: AtomicU64::new(1), writer_batch: AtomicU64::new(1), returns: sender.clone(), recycle: true });
        let second = Arc::new(TensorBuffer { object: Some(second_object), capacity: 16,
            last_batch: AtomicU64::new(2), writer_batch: AtomicU64::new(2), returns: sender.clone(), recycle: true });
        let runtime = Arc::new(TensorRuntime {
            device: context.device.clone(), queue: context.command_queue.clone(), library: context.library.clone(), returns: sender, cost,
            state: Mutex::new(TensorState {
                context, lock_phases: TensorLockPhases::default(), returns, free: Vec::new(), deferred_drop: Vec::new(), dead_owners: makepad_ai_loader::DeadOwnerInbox::register(),
                in_flight: VecDeque::new(), next_batch: 3, open_batch: 0,
                open_ns: 0.0, thread_batches: HashMap::new(),
                completed_batch: 0, failed: BTreeSet::new(), failed_oom: BTreeSet::new(), failure_floor: None, injected_failure: Some((1, false)),
                live_writers: BTreeMap::from([(1, 1), (2, 1)]),
            }),
        });
        tensor_locked_in(&runtime, |state| {
            state.context.begin_batch()?;
            state.open_batch = 1;
            Ok(())
        })?;
        let error = tensor_flush_in(&runtime, 1).unwrap_err();
        assert!(error.contains("failed batch 1"), "failed producer read rejected on A: {error}");
        assert!(!error.contains("out of memory"), "plain failure is not OOM: {error}");
        assert!(tensor_flush_in(&runtime, 0).is_ok(), "host weights survive failed batch");
        let other = runtime.clone();
        std::thread::spawn(move || {
            assert!(tensor_flush_in(&other, 2).is_err(), "dependent batch read rejected on B");
            drop(second);
        }).join().unwrap();
        drop(first);
        tensor_locked_in(&runtime, |state| {
            state.drain_returns();
            state.context.begin_batch()?;
            state.open_batch = 3;
            Ok(())
        })?;
        let other = runtime.clone();
        std::thread::spawn(move || tensor_pool_clear_in(&other)).join().unwrap()?;
        assert_eq!(tensor_locked_in(&runtime, |state| Ok(state.open_batch))?, 3,
            "pool clear on B neither commits nor waits for A's open batch");
        assert!(tensor_flush_in(&runtime, 3).is_ok(), "next job commits and reads after failed outputs drop");
        tensor_locked_in(&runtime, |state| {
            state.record_failure(1, false);
            state.check_writer(3)
        })?;
        tensor_locked_in(&runtime, |state| {
            state.injected_failure = Some((4, true));
            state.live_writers.insert(4, 1);
            state.context.begin_batch()?;
            state.open_batch = 4;
            Ok(())
        })?;
        let error = tensor_flush_in(&runtime, 4).unwrap_err();
        assert!(error.contains("out of memory"), "OOM failure keeps its classification: {error}");
        Ok(())
    }

    #[cfg(test)]
    mod tensor_slice_tests {
        use super::*;

        #[test]
        fn row_plans_cover_and_preserve_kernel_rows() {
            assert_eq!(row_chunks(300, SLICE_NS, 32, 32), vec![0..300]);
            assert_eq!(row_chunks(300, 3.5 * SLICE_NS, 32, 32),
                vec![0..96, 96..192, 192..300], "short tail merges");
            assert_eq!(row_chunks(100, 9.0 * SLICE_NS, 1, 20),
                vec![0..20, 20..40, 40..60, 60..80, 80..100],
                "minimum rows preserve non-vector attention");
            for (rows, slices, align, min_rows) in [
                (300, 3.5, 32, 32), (100, 9.0, 1, 20),
                (320, 4.0, 32, 32), (33, 20.0, 32, 32),
                (101, 3.0, 1, 20), (101, 3.0, 8, 20), (8220, 36.0, 8, 20),
            ] {
                let plan = row_chunks(rows, slices * SLICE_NS, align, min_rows);
                let mut end = 0;
                for (index, range) in plan.iter().enumerate() {
                    assert_eq!(range.start, end, "contiguous ordered cover");
                    assert_eq!(range.start % align, 0, "aligned start");
                    assert!(range.len() >= min_rows, "minimum kernel rows");
                    if index + 1 < plan.len() {
                        assert_eq!(range.len() % align, 0, "aligned non-tail size");
                    }
                    end = range.end;
                }
                assert_eq!(end, rows, "exact final extent");
            }
        }

        #[test]
        fn commit_and_back_pressure_rules() {
            let cost = TensorCost {
                ns_per_mac: 2.0, ns_per_byte: 3.0,
                ns_per_dispatch: 5.0, ns_per_commit: 1000.0,
            };
            assert_eq!(estimate_ns(&cost, 7, 11, 13), 112.0);
            assert!(!should_commit(999.0, true, &cost), "idle commit floor");
            assert!(should_commit(1000.0, true, &cost), "idle starts work");
            assert!(!should_commit(1000.0, false, &cost), "busy accumulates");
            assert!(!should_commit(SLICE_NS - 1.0, false, &cost));
            assert!(should_commit(SLICE_NS, false, &cost), "slice boundary");
            assert!(should_commit(SLICE_NS + 1.0, true, &cost));
            for (mine, open, completed, expected) in [
                (vec![], 0, 0, None),
                (vec![1, 2], 0, 0, None),
                (vec![1, 2, 3], 3, 0, None),
                (vec![1, 2, 3], 0, 0, Some(1)),
                (vec![1, 2, 3, 4], 4, 1, None),
                (vec![1, 2, 3, 4, 5], 5, 1, Some(2)),
                (vec![1, 3, 7], 0, 7, None),
            ] {
                assert_eq!(back_pressure_target(&VecDeque::from(mine), open, completed),
                    expected, "only committed unfinished batches count");
            }
        }

        fn next_random(state: &mut u32) -> u32 {
            *state ^= *state << 13;
            *state ^= *state >> 17;
            *state ^= *state << 5;
            *state
        }

        fn random_input(state: &mut u32, elements: usize) -> Vec<f32> {
            (0..elements).map(|_| next_random(state) as i32 as f32 / i32::MAX as f32)
                .collect()
        }

        fn output_bits(buffer: &TensorBuffer, elements: usize, batch: u64) -> Vec<u32> {
            tensor_flush_until(batch).unwrap();
            unsafe { std::slice::from_raw_parts(buffer.contents().unwrap(), elements) }
                .iter().map(|value| value.to_bits()).collect()
        }

        #[test]
        fn matmul_row_slices_preserve_f32_and_q8_bits() {
            crate::run_model_worker("matmul-slices-test", || {
                let (m, k, n) = (300, 256, 512);
                let mut seed = 0x6d2b79f5;
                let x = random_input(&mut seed, m * k);
                let shape = shape4_from_row_major(&[m, k], 4).unwrap();
                let x = tensor_upload_f32(&x, &shape).unwrap();
                let weights = random_input(&mut seed, n * k);
                let f32_bytes: Vec<_> = weights.iter().flat_map(|v| v.to_le_bytes()).collect();
                let mut q8_bytes = Vec::new();
                for _ in 0..n * k / 32 {
                    q8_bytes.extend(makepad_ai_loader::quant::f32_to_f16(0.03125).to_le_bytes());
                    q8_bytes.extend((0..32).map(|_| next_random(&mut seed) as u8));
                }
                for (ty, bytes) in [(GGML_TYPE_F32, f32_bytes), (GGML_TYPE_Q8_0, q8_bytes)] {
                    let w = tensor_buffer(bytes.len(), true).unwrap();
                    unsafe { std::ptr::copy_nonoverlapping(
                        bytes.as_ptr(), w.contents().unwrap().cast::<u8>(), bytes.len()); }
                    let whole = tensor_buffer(m * n * 4, false).unwrap();
                    let split = tensor_buffer(m * n * 4, false).unwrap();
                    let whole_batch = tensor_matmul_rows(
                        &x, 0, &w, 0, &whole, m, k, n, ty, &[0..m]).unwrap();
                    let split_batch = tensor_matmul_rows(
                        &x, 0, &w, 0, &split, m, k, n, ty,
                        &[0..96, 96..192, 192..300]).unwrap();
                    assert_eq!(output_bits(&whole, m * n, whole_batch),
                        output_bits(&split, m * n, split_batch),
                        "GEMM row slices preserve every bit for type {ty}");
                }
            });
        }

        #[test]
        fn attention_row_slices_preserve_bits() {
            crate::run_model_worker("attention-slices-test", || {
                let (nq, nk, heads, d) = (100, 64, 2, 64);
                let mut seed = 0xa341316c;
                let q_shape = shape4_from_row_major(&[nq, heads, d], 4).unwrap();
                let kv_shape = shape4_from_row_major(&[nk, heads, d], 4).unwrap();
                let q = tensor_upload_f32(&random_input(&mut seed, q_shape.numel), &q_shape).unwrap();
                let k = tensor_upload_f32(&random_input(&mut seed, kv_shape.numel), &kv_shape).unwrap();
                let v = tensor_upload_f32(&random_input(&mut seed, kv_shape.numel), &kv_shape).unwrap();
                let whole = tensor_buffer(q_shape.numel * 4, false).unwrap();
                let split = tensor_buffer(q_shape.numel * 4, false).unwrap();
                let scale = 1.0 / (d as f32).sqrt();
                let whole_batch = tensor_attention_rows(
                    &q, 0, &k, 0, &v, 0, &whole, nq, nk, heads, d, scale, &[0..nq]).unwrap();
                let split_batch = tensor_attention_rows(
                    &q, 0, &k, 0, &v, 0, &split, nq, nk, heads, d, scale,
                    &[0..34, 34..68, 68..100]).unwrap();
                assert_eq!(output_bits(&whole, q_shape.numel, whole_batch),
                    output_bits(&split, q_shape.numel, split_batch),
                    "attention row slices preserve every bit");
            });
        }
    }

    #[test]
    fn host_matmul_wrappers_preserve_kernel_bits() {
        let _pool = AutoreleasePool::new();
        let mut legacy = MetalContext::new().expect("a working Metal backend is required");
        // mul_mv_ext requires k divisible by 128 and 2..=8 input rows.
        // mul_mm uses k >= 64 and m > 8; the odd short row uses mul_mv.
        for (m, k, n, kernel) in [
            (3, 128, 5, "mul_mv_ext"),
            (9, 65, 7, "mul_mm"),
            (5, 7, 3, "mul_mv"),
        ] {
            let a: Vec<f32> = (0..m * k)
                .map(|i| (i as i32 % 23 - 11) as f32 * 0.0317)
                .collect();
            let b: Vec<f32> = (0..k * n)
                .map(|i| (i as i32 % 19 - 9) as f32 * 0.0731)
                .collect();
            let mut bt = vec![0.0; n * k];
            for i in 0..k {
                for j in 0..n {
                    bt[j * k + i] = b[i * n + j];
                }
            }
            let (dst, mr, nr) = legacy
                .matmul_nt_ggml_bytes_impl(&a, f32_slice_as_bytes(&bt), GGML_TYPE_F32, m, k, n, None)
                .expect("legacy matmul");
            let expected = legacy.read_f32_buffer(dst.as_id(), mr * nr).unwrap();
            for (layout, actual) in [
                ("NT", super::try_matmul_nt_f32(&a, &bt, m, k, n)),
                ("NN", super::try_matmul_nn_f32(&a, &b, m, k, n)),
            ] {
                let actual = actual.expect("tensor matmul");
                assert_eq!(actual.len(), expected.len(), "{layout} {kernel} shape");
                for (i, (&actual, &expected)) in actual.iter().zip(&expected).enumerate() {
                    assert_eq!(actual.to_bits(), expected.to_bits(),
                        "{layout} {kernel} m={m} k={k} n={n} index={i}: actual={actual} expected={expected} abs_diff={}",
                        (actual - expected).abs());
                }
            }
        }
        assert!(super::try_matmul_nt_f32(&[1.0; 6], &[1.0; 5], 2, 3, 2).is_none());
        assert!(super::try_matmul_nn_f32(&[1.0; 6], &[1.0; 5], 2, 3, 2).is_none());
        assert!(super::try_matmul_nt_f32(&[1.0; 5], &[1.0; 6], 2, 3, 2).is_none());
        assert!(super::try_matmul_nn_f32(&[1.0; 5], &[1.0; 6], 2, 3, 2).is_none());
        // An empty product declines the device, as the legacy path did (no zero-byte buffers).
        for (m, k, n) in [(0, 7, 3), (3, 0, 5), (3, 7, 0)] {
            let a = vec![1.0; m * k];
            let b = vec![1.0; k * n];
            assert!(super::try_matmul_nt_f32(&a, &b, m, k, n).is_none(), "NT m={m} k={k} n={n}");
            assert!(super::try_matmul_nn_f32(&a, &b, m, k, n).is_none(), "NN m={m} k={k} n={n}");
        }
    }

    #[test]
    fn host_elementwise_wrappers_preserve_kernel_bits() {
        fn bits(values: &[f32]) -> Vec<u32> {
            values.iter().map(|value| value.to_bits()).collect()
        }
        fn legacy_buffer(ctx: &MetalContext, values: &[f32]) -> StrongId {
            let bytes = unsafe {
                std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), values.len() * 4)
            };
            ctx.new_buffer_with_bytes(bytes).expect("legacy upload")
        }

        let _pool = AutoreleasePool::new();
        let mut legacy = MetalContext::new().expect("a working Metal backend is required");
        // Ranks 1-4, independent affine broadcasting, odd scalar rows and a
        // vector row. The reference uses the old upload/dispatch/read sequence.
        for dims in [vec![5], vec![3, 5], vec![3, 3, 5], vec![3, 3, 3, 5], vec![3, 8]] {
            let shape = shape4_from_row_major(&dims, 4).unwrap();
            let width = *dims.last().unwrap();
            let mut mul_dims = dims.clone();
            if mul_dims.len() > 1 { mul_dims[0] = 1; }
            let mut add_dims = dims.clone();
            if add_dims.len() > 2 { add_dims[1] = 1; }
            let ms = shape4_from_row_major(&mul_dims, 4).unwrap();
            let ads = shape4_from_row_major(&add_dims, 4).unwrap();
            let x: Vec<f32> = (0..shape.numel).map(|i| (i as i32 % 13 - 6) as f32 * 0.125).collect();
            let mul: Vec<f32> = (0..ms.numel).map(|i| 0.5 + (i % 7) as f32 * 0.25).collect();
            let add: Vec<f32> = (0..ads.numel).map(|i| (i as i32 % 5 - 2) as f32 * 0.25).collect();
            let xb = legacy_buffer(&legacy, &x);
            let mb = legacy_buffer(&legacy, &mul);
            let ab = legacy_buffer(&legacy, &add);
            let dst = legacy.new_buffer_with_length(x.len() * 4).unwrap();
            let eps = 1e-5;
            for op in 0..7 {
                let actual = match op {
                    0 | 1 => {
                        legacy.dispatch_bin_f32(if op == 0 { 0 } else { 2 }, xb.as_id(), mb.as_id(), dst.as_id(), &shape, &ms).unwrap();
                        if op == 0 { try_add_f32(&x, &dims, &mul, &mul_dims) }
                        else { try_mul_f32(&x, &dims, &mul, &mul_dims) }
                    }
                    2 => {
                        legacy.dispatch_unary_f32(OP_UNARY_NUM_GELU, xb.as_id(), dst.as_id(), &shape).unwrap();
                        try_gelu_f32(&x, &dims)
                    }
                    3 => {
                        legacy.dispatch_norm_f32(xb.as_id(), xb.as_id(), xb.as_id(), dst.as_id(), &shape, &shape, &shape, eps, 1).unwrap();
                        try_layer_norm_f32(&x, &dims, eps)
                    }
                    4 => {
                        legacy.dispatch_rms_norm_f32_offsets(xb.as_id(), xb.as_id(), xb.as_id(), dst.as_id(), &shape, &shape, &shape, eps, 1, [0; 4]).unwrap();
                        try_rms_norm_f32(&x, &dims, eps)
                    }
                    5 => {
                        legacy.dispatch_rms_norm_f32_offsets(xb.as_id(), mb.as_id(), xb.as_id(), dst.as_id(), &shape, &ms, &shape, eps, 2, [0; 4]).unwrap();
                        try_rms_norm_mul_f32(&x, &dims, &mul, &mul_dims, eps)
                    }
                    _ => {
                        legacy.dispatch_norm_f32(xb.as_id(), mb.as_id(), ab.as_id(), dst.as_id(), &shape, &ms, &ads, eps, 3).unwrap();
                        try_layer_norm_mul_add_f32(&x, &dims, &mul, &mul_dims, &add, &add_dims, eps)
                    }
                }.expect("host wrapper must run the device operation");
                let expected = legacy.read_f32_buffer(dst.as_id(), x.len()).unwrap();
                assert_eq!(bits(&actual), bits(&expected), "wrapper op {op}, shape {dims:?}");
                if op <= 1 {
                    let host: Vec<f32> = x.iter().enumerate().map(|(i, value)| {
                        if op == 0 { value + mul[i % mul.len()] } else { value * mul[i % mul.len()] }
                    }).collect();
                    assert_eq!(bits(&actual), bits(&host), "binary host reference op {op}, shape {dims:?}");
                }

                if width == 5 && dims.len() == 4 {
                    // The host APIs have no offset. Exercise the tensor entries
                    // with a 4-byte prefix, preserving the scalar kernel choice.
                    let padded = |values: &[f32]| {
                        let mut values = values.to_vec();
                        values.insert(0, -123.0);
                        let ps = shape4_from_row_major(&[values.len()], 4).unwrap();
                        tensor_upload_f32(&values, &ps).unwrap()
                    };
                    let tx = padded(&x);
                    let tm = padded(&mul);
                    let ta = padded(&add);
                    let offset_result = tensor_host_result(&shape, |out| match op {
                        0 | 1 => tensor_binary(&tx, 4, &shape, &tm, 4, &ms, out, op == 1),
                        2 => tensor_unary(&tx, 4, &shape, out, OP_UNARY_NUM_GELU),
                        3 => tensor_norm(&tx, 4, &shape, &tx, 4, &shape, &tx, 4, &shape, out, eps, 1),
                        4 => tensor_rms_norm(&tx, 4, &shape, &tx, 4, &shape, &tx, 4, &shape, out, eps, 1),
                        5 => tensor_rms_norm(&tx, 4, &shape, &tm, 4, &ms, &tx, 4, &shape, out, eps, 2),
                        _ => tensor_norm(&tx, 4, &shape, &tm, 4, &ms, &ta, 4, &ads, out, eps, 3),
                    }).unwrap();
                    assert_eq!(bits(&offset_result), bits(&expected), "unaligned tensor op {op}");
                }
            }
        }
        assert!(try_silu_f32(&[0.25, -0.5, 1.0]).is_none(), "macOS SiLU shim stays unsupported");
        assert!(try_add_f32(&[1.0; 15], &[3, 5], &[1.0; 3], &[3]).is_none(), "binary rejects incompatible broadcast");
        assert!(try_gelu_f32(&[1.0; 5], &[3, 5]).is_none(), "unary rejects wrong length");
        assert!(try_rms_norm_mul_f32(&[1.0; 15], &[3, 5], &[1.0; 3], &[3], 1e-5).is_none(), "RMS rejects wrong affine width");
        assert!(try_layer_norm_mul_add_f32(&[1.0; 15], &[3, 5], &[1.0; 10], &[2, 5], &[0.0; 5], &[5], 1e-5).is_none(), "norm rejects incompatible affine broadcast");
    }

    #[test]
    fn host_get_rows_matches_cpu_bits() {
        use makepad_ai_loader::quant::{
            f32_to_f16, get_rows_ggml_bytes_cpu, quantize_f32_to_q8_0,
        };

        fn bits(values: &[f32]) -> Vec<u32> {
            values.iter().map(|value| value.to_bits()).collect()
        }

        let indices = [2i32, 0, 2, 1];
        let n_rows = 3;
        for (ty, n_cols) in [
            (GGML_TYPE_F32, 5),
            (GGML_TYPE_F16, 5),
            (GGML_TYPE_BF16, 5),
            (GGML_TYPE_Q8_0, 32),
            (GGML_TYPE_Q8_0, 64),
            (GGML_TYPE_Q4_0, 32),
            (GGML_TYPE_Q4_0, 64),
        ] {
            let values: Vec<f32> = (0..n_rows * n_cols)
                .map(|i| (i as i32 % 29 - 14) as f32 * 0.125)
                .collect();
            let src: Vec<u8> = match ty {
                GGML_TYPE_F32 => values.iter().flat_map(|v| v.to_le_bytes()).collect(),
                GGML_TYPE_F16 => values.iter().flat_map(|v| f32_to_f16(*v).to_le_bytes()).collect(),
                GGML_TYPE_BF16 => values.iter()
                    .flat_map(|v| ((v.to_bits() >> 16) as u16).to_le_bytes()).collect(),
                GGML_TYPE_Q8_0 => quantize_f32_to_q8_0(&values),
                GGML_TYPE_Q4_0 => {
                    // No Q4_0 quantizer in the loader. Use deterministic blocks
                    // with distinct scales and both positive and negative nibbles.
                    let mut bytes = Vec::new();
                    for block in 0..n_rows * n_cols / 32 {
                        bytes.extend_from_slice(&f32_to_f16((block + 1) as f32 * 0.125).to_le_bytes());
                        bytes.extend((0..16).map(|i| ((i + block * 3) % 16) as u8 | (((15 - i + block * 5) % 16) as u8) << 4));
                    }
                    bytes
                }
                _ => unreachable!(),
            };
            let expected = get_rows_ggml_bytes_cpu(&src, ty, n_cols, n_rows, &indices)
                .expect("CPU get-rows reference");
            let actual = try_get_rows_ggml_bytes(&src, ty, n_cols, n_rows, &indices)
                .expect("host wrapper must run Metal get-rows");
            assert_eq!(bits(&actual), bits(&expected), "get-rows wrapper type {ty}, columns {n_cols}");

            let mut padded_src = vec![0xa5; 4];
            padded_src.extend_from_slice(&src);
            let mut padded_rows = (-1i32).to_le_bytes().to_vec();
            padded_rows.extend(indices.iter().flat_map(|row| row.to_le_bytes()));
            let src_buffer = tensor_upload_bytes(&padded_src).unwrap();
            let rows_buffer = tensor_upload_bytes(&padded_rows).unwrap();
            let shape = shape4_from_row_major(&[indices.len(), n_cols], 4).unwrap();
            let offset_result = tensor_host_result(&shape, |out| {
                tensor_get_rows(&src_buffer, 4, ty, n_cols, n_rows,
                    &rows_buffer, 4, indices.len(), out)
            }).unwrap();
            assert_eq!(bits(&offset_result), bits(&expected), "get-rows offsets type {ty}, columns {n_cols}");
            assert!(try_get_rows_ggml_bytes(&src, ty, n_cols, n_rows, &[3]).is_none(), "get-rows rejects upper row bound for type {ty}");
            assert!(try_get_rows_ggml_bytes(&src, ty, n_cols, n_rows, &[-1]).is_none(), "get-rows rejects negative row for type {ty}");
            assert!(try_get_rows_ggml_bytes(&src[..src.len() - 1], ty, n_cols, n_rows, &indices).is_none(), "get-rows rejects wrong source length for type {ty}");
            assert_eq!(try_get_rows_ggml_bytes(&src, ty, n_cols, n_rows, &[]), Some(Vec::new()), "get-rows accepts empty indices for type {ty}");
        }
        assert_eq!(try_get_rows_ggml_bytes(&[], u32::MAX, 0, 0, &[]), Some(Vec::new()), "empty indices return before validation");
        assert!(try_get_rows_ggml_bytes(&[0; 4], u32::MAX, 1, 1, &[0]).is_none(), "get-rows rejects unsupported types");
        assert!(try_get_rows_ggml_bytes(&[0; 34], GGML_TYPE_Q8_0, 31, 1, &[0]).is_none(), "get-rows rejects incomplete quantized blocks");
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
        if a.len() != m.checked_mul(k)? || b.len() != k.checked_mul(n)? {
            return None;
        }
        let mut bt = vec![0.0f32; n * k];
        for i in 0..k {
            for j in 0..n {
                bt[j * k + i] = b[i * n + j];
            }
        }
        try_matmul_nt_f32(a, &bt, m, k, n)
    }

    pub(super) fn try_matmul_nt_f32_weight(
        a: &[f32], w: &F32Weight, m: usize, k: usize, n: usize,
    ) -> Option<Vec<f32>> {
        if a.len() != m.checked_mul(k)? || w.len() != n.checked_mul(k)?
            || m == 0 || k == 0 || n == 0 {
            return None;
        }
        tensor_try(|| {
            let a_shape = shape4_from_row_major(&[m, k], 4)?;
            let out_shape = shape4_from_row_major(&[m, n], 4)?;
            let a = tensor_upload_f32(a, &a_shape)?;
            let weight = tensor_f32_weight(w)?;
            tensor_host_result(&out_shape, |out| {
                tensor_matmul(&a, 0, &weight, 0, out, m, k, n, GGML_TYPE_F32)
            })
        })
    }

    pub(super) fn try_matmul_nt_f32(
        a: &[f32],
        bt: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        if a.len() != m.checked_mul(k)? {
            return None;
        }
        // Legacy Metal allocations reject zero-byte input or output buffers.
        if m == 0 || k == 0 || n == 0 {
            return None;
        }
        if bt.len() != n.checked_mul(k)? {
            return None;
        }
        tensor_try(|| {
            let a_shape = shape4_from_row_major(&[m, k], 4)?;
            let bt_shape = shape4_from_row_major(&[n, k], 4)?;
            let out_shape = shape4_from_row_major(&[m, n], 4)?;
            let a = tensor_upload_f32(a, &a_shape)?;
            let bt = tensor_upload_f32(bt, &bt_shape)?;
            tensor_host_result(&out_shape, |out| {
                tensor_matmul(&a, 0, &bt, 0, out, m, k, n, GGML_TYPE_F32)
            })
        })
    }

    pub(super) fn try_matmul_nt_ggml_bytes(
        a: &[f32],
        bt_bytes: &[u8],
        bt_ggml_type: u32,
        m: usize,
        k: usize,
        n: usize,
    ) -> Option<Vec<f32>> {
        if a.len() != m.checked_mul(k)? || m == 0 || k == 0 || n == 0 {
            return None;
        }
        tensor_try(|| {
            let a_shape = shape4_from_row_major(&[m, k], 4)?;
            let out_shape = shape4_from_row_major(&[m, n], 4)?;
            let a = tensor_upload_f32(a, &a_shape)?;
            let weight = tensor_upload_bytes(bt_bytes)?;
            tensor_host_result(&out_shape, |out| {
                tensor_matmul(&a, 0, &weight, 0, out, m, k, n, bt_ggml_type)
            })
        })
    }

    pub(super) fn try_matmul_nt_ggml_bytes_keyed<'a, F>(
        a: &[f32],
        bt_ggml_type: u32,
        m: usize,
        k: usize,
        n: usize,
        owner: &WeightOwner,
        cache_key: &str,
        load: F,
    ) -> Option<Vec<f32>>
    where
        F: FnOnce() -> Result<Cow<'a, [u8]>, String>,
    {
        if a.len() != m.checked_mul(k)? || m == 0 || k == 0 || n == 0 {
            return None;
        }
        tensor_try(|| {
            let a_shape = shape4_from_row_major(&[m, k], 4)?;
            let out_shape = shape4_from_row_major(&[m, n], 4)?;
            let a = tensor_upload_f32(a, &a_shape)?;
            let weight = tensor_named_weight(owner, cache_key, load)?;
            tensor_host_result(&out_shape, |out| {
                tensor_matmul(&a, 0, &weight, 0, out, m, k, n, bt_ggml_type)
            })
        })
    }

    pub(super) fn try_matmul_nt_ggml_bytes_keyed_multi<'a, F>(
        a: &[f32],
        m: usize,
        k: usize,
        matrices: &[super::MatmulNtGgmlBytesKeyedMatrix<'_>],
        load: F,
    ) -> Option<Vec<Vec<f32>>>
    where
        F: FnMut(&WeightOwner, &str) -> Result<Cow<'a, [u8]>, String>,
    {
        if matrices.is_empty() { return Some(Vec::new()); }
        if a.len() != m.checked_mul(k)? || m == 0 || k == 0 || matrices.iter().any(|matrix| matrix.n == 0) {
            return None;
        }
        let mut load = load;
        tensor_try(|| {
            let a_shape = shape4_from_row_major(&[m, k], 4)?;
            let a = tensor_upload_f32(a, &a_shape)?;
            let mut outputs = Vec::with_capacity(matrices.len());
            let mut last_batch = 0;
            for matrix in matrices {
                let weight = tensor_named_weight(matrix.owner, matrix.cache_key,
                    || load(matrix.owner, matrix.cache_key))?;
                let shape = shape4_from_row_major(&[m, matrix.n], 4)?;
                let bytes = shape.numel.checked_mul(4).ok_or("metal output extent overflow")?;
                let out = tensor_buffer(bytes, false)?;
                last_batch = tensor_matmul(&a, 0, &weight, 0, &out, m, k, matrix.n, matrix.bt_ggml_type)?;
                outputs.push((out, shape.numel));
            }
            tensor_flush_until(last_batch)?;
            outputs.into_iter().map(|(out, numel)| {
                Ok(unsafe { std::slice::from_raw_parts(out.contents()?, numel) }.to_vec())
            }).collect()
        })
    }

    pub(super) fn try_ar_pre_attn<F>(
        resid: &mut ArResidual,
        hidden: Option<&[f32]>,
        m: usize,
        hidden_w: usize,
        head_dim: usize,
        in_norm: &[f32],
        qk_norm: Option<(&[f32], &[f32], &WeightKey, &WeightKey)>,
        in_norm_key: &WeightKey,
        eps: f32,
        q: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        k: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        v: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        load: F,
    ) -> Option<(Vec<f32>, Vec<f32>, Vec<f32>)>
    where
        F: FnMut(&WeightOwner, &str) -> Result<Vec<u8>, String>,
    {
        let mut load = load;
        tensor_try(|| {
            let x_s = shape4_from_row_major(&[m, hidden_w], 4)?;
            let g_s = shape4_from_row_major(&[hidden_w], 4)?;
            if let Some(hidden) = hidden {
                if hidden.len() != x_s.numel { return Err("ar hidden len".into()); }
                resid.h = Some(tensor_upload_f32(hidden, &x_s)?);
                resid.elems = x_s.numel;
            }
            let h = resid.h.as_ref().filter(|_| resid.elems == x_s.numel).ok_or("ar residual missing")?;
            let q_s = shape4_from_row_major(&[m, q.n], 4)?;
            let k_s = shape4_from_row_major(&[m, k.n], 4)?;
            let v_s = shape4_from_row_major(&[m, v.n], 4)?;
            let g = tensor_keyed_weight(in_norm_key, || Ok(Cow::Owned(in_norm.iter().flat_map(|v| v.to_le_bytes()).collect())))?;
            let qw = tensor_named_weight(q.owner, q.cache_key, || load(q.owner, q.cache_key).map(Cow::Owned))?;
            let kw = tensor_named_weight(k.owner, k.cache_key, || load(k.owner, k.cache_key).map(Cow::Owned))?;
            let vw = tensor_named_weight(v.owner, v.cache_key, || load(v.owner, v.cache_key).map(Cow::Owned))?;
            let qk = if let Some((q_norm, k_norm, q_key, k_key)) = qk_norm {
                let qg = tensor_keyed_weight(q_key, || Ok(Cow::Owned(q_norm.iter().flat_map(|v| v.to_le_bytes()).collect())))?;
                let kg = tensor_keyed_weight(k_key, || Ok(Cow::Owned(k_norm.iter().flat_map(|v| v.to_le_bytes()).collect())))?;
                let qs = shape4_from_row_major(&[q_s.numel / head_dim, head_dim], 4)?;
                let ks = shape4_from_row_major(&[k_s.numel / head_dim, head_dim], 4)?;
                let gs = shape4_from_row_major(&[head_dim], 4)?;
                Some((qg, kg, qs, ks, gs))
            } else { None };
            let n1 = tensor_buffer((x_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let qo = tensor_buffer((q_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let ko = tensor_buffer((k_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let vo = tensor_buffer((v_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let normalized = if qk.is_some() {
                Some((tensor_buffer(q_s.numel.checked_mul(4).ok_or("metal output extent overflow")?, false)?,
                      tensor_buffer(k_s.numel.checked_mul(4).ok_or("metal output extent overflow")?, false)?))
            } else { None };
            let mut inputs = vec![h, &g, &qw, &kw, &vw];
            let mut outputs = vec![&n1, &qo, &ko, &vo];
            if let Some((qg, kg, _, _, _)) = &qk { inputs.extend([qg, kg]); }
            if let Some((qn, kn)) = &normalized { outputs.extend([qn, kn]); }
            let batch = tensor_encode_group(
                m as u64 * hidden_w as u64 * (q.n as u64 + k.n as u64 + v.n as u64),
                &inputs, &outputs, |ctx| {
                ctx.dispatch_rms_norm_f32_offsets(h.id(), g.id(), h.id(), n1.id(), &x_s, &g_s, &x_s, eps, 2, [0; 4])?;
                ctx.matmul_nt_into_buffer_offsets(q.bt_ggml_type, qw.id(), n1.id(), qo.id(), m, hidden_w, q.n, [0; 3])?;
                ctx.matmul_nt_into_buffer_offsets(k.bt_ggml_type, kw.id(), n1.id(), ko.id(), m, hidden_w, k.n, [0; 3])?;
                ctx.matmul_nt_into_buffer_offsets(v.bt_ggml_type, vw.id(), n1.id(), vo.id(), m, hidden_w, v.n, [0; 3])?;
                if let (Some((qg, kg, qs, ks, gs)), Some((qn, kn))) = (&qk, &normalized) {
                    ctx.dispatch_rms_norm_f32_offsets(qo.id(), qg.id(), qo.id(), qn.id(), qs, gs, qs, eps, 2, [0; 4])?;
                    ctx.dispatch_rms_norm_f32_offsets(ko.id(), kg.id(), ko.id(), kn.id(), ks, gs, ks, eps, 2, [0; 4])?;
                }
                Ok(())
            })?;
            tensor_flush_until(batch)?;
            let (q_src, k_src) = normalized.as_ref().map(|(q, k)| (q, k)).unwrap_or((&qo, &ko));
            Ok((unsafe { std::slice::from_raw_parts(q_src.contents()?, q_s.numel) }.to_vec(),
                unsafe { std::slice::from_raw_parts(k_src.contents()?, k_s.numel) }.to_vec(),
                unsafe { std::slice::from_raw_parts(vo.contents()?, v_s.numel) }.to_vec()))
        })
    }

    pub(super) fn try_ar_post_attn<F>(
        resid: &mut ArResidual,
        attn: &[f32],
        m: usize,
        hidden_w: usize,
        post_norm: &[f32],
        post_norm_key: &WeightKey,
        eps: f32,
        o: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        up: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        gate: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        down: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        load: F,
    ) -> Option<()>
    where
        F: FnMut(&WeightOwner, &str) -> Result<Vec<u8>, String>,
    {
        let mut load = load;
        tensor_try(|| {
            let x_s = shape4_from_row_major(&[m, hidden_w], 4)?;
            let g_s = shape4_from_row_major(&[hidden_w], 4)?;
            let h_s = shape4_from_row_major(&[x_s.numel], 4)?;
            let up_s = shape4_from_row_major(&[m, up.n], 4)?;
            let ff_s = shape4_from_row_major(&[up_s.numel], 4)?;
            let o_s = shape4_from_row_major(&[m, o.n], 4)?;
            let gate_s = shape4_from_row_major(&[m, gate.n], 4)?;
            let down_s = shape4_from_row_major(&[m, down.n], 4)?;
            if attn.len() != x_s.numel { return Err("ar attn len".into()); }
            let h = resid.h.as_ref().filter(|_| resid.elems == x_s.numel).ok_or("ar residual missing")?;
            let attn = tensor_upload_f32(attn, &x_s)?;
            let g = tensor_keyed_weight(post_norm_key, || Ok(Cow::Owned(post_norm.iter().flat_map(|v| v.to_le_bytes()).collect())))?;
            let ow = tensor_named_weight(o.owner, o.cache_key, || load(o.owner, o.cache_key).map(Cow::Owned))?;
            let uw = tensor_named_weight(up.owner, up.cache_key, || load(up.owner, up.cache_key).map(Cow::Owned))?;
            let gw = tensor_named_weight(gate.owner, gate.cache_key, || load(gate.owner, gate.cache_key).map(Cow::Owned))?;
            let dw = tensor_named_weight(down.owner, down.cache_key, || load(down.owner, down.cache_key).map(Cow::Owned))?;
            let oo = tensor_buffer((o_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let h2 = tensor_buffer((x_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let n2 = tensor_buffer((x_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let uo = tensor_buffer((up_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let go = tensor_buffer((gate_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let gs = tensor_buffer((up_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let ff = tensor_buffer((up_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let d = tensor_buffer((down_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let h3 = tensor_buffer((x_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            tensor_encode_group(m as u64 * (
                hidden_w as u64 * o.n as u64 + hidden_w as u64 * up.n as u64
                + hidden_w as u64 * gate.n as u64 + up.n as u64 * down.n as u64),
                &[h, &attn, &g, &ow, &uw, &gw, &dw],
                &[&oo, &h2, &n2, &uo, &go, &gs, &ff, &d, &h3], |ctx| {
                ctx.matmul_nt_into_buffer_offsets(o.bt_ggml_type, ow.id(), attn.id(), oo.id(), m, hidden_w, o.n, [0; 3])?;
                ctx.dispatch_bin_f32_offsets(0, h.id(), oo.id(), h2.id(), &h_s, &h_s, [0; 3])?;
                ctx.dispatch_rms_norm_f32_offsets(h2.id(), g.id(), h2.id(), n2.id(), &x_s, &g_s, &x_s, eps, 2, [0; 4])?;
                ctx.matmul_nt_into_buffer_offsets(up.bt_ggml_type, uw.id(), n2.id(), uo.id(), m, hidden_w, up.n, [0; 3])?;
                ctx.matmul_nt_into_buffer_offsets(gate.bt_ggml_type, gw.id(), n2.id(), go.id(), m, hidden_w, gate.n, [0; 3])?;
                ctx.dispatch_unary_f32_offsets(OP_UNARY_NUM_SILU, go.id(), gs.id(), &ff_s, [0; 2])?;
                ctx.dispatch_bin_f32_offsets(2, uo.id(), gs.id(), ff.id(), &ff_s, &ff_s, [0; 3])?;
                ctx.matmul_nt_into_buffer_offsets(down.bt_ggml_type, dw.id(), ff.id(), d.id(), m, up.n, down.n, [0; 3])?;
                ctx.dispatch_bin_f32_offsets(0, h2.id(), d.id(), h3.id(), &h_s, &h_s, [0; 3])?;
                Ok(())
            })?;
            resid.h = Some(h3);
            Ok(())
        })
    }

    pub(super) fn try_ar_final_rms(
        resid: &mut ArResidual,
        m: usize,
        hidden_w: usize,
        gamma: &[f32],
        gamma_key: &WeightKey,
        eps: f32,
    ) -> Option<Vec<f32>> {
        tensor_try(|| {
            let x_s = shape4_from_row_major(&[m, hidden_w], 4)?;
            let g_s = shape4_from_row_major(&[hidden_w], 4)?;
            let h = resid.h.as_ref().filter(|_| resid.elems == x_s.numel).ok_or("ar residual missing")?;
            let g = tensor_keyed_weight(gamma_key, || Ok(Cow::Owned(gamma.iter().flat_map(|v| v.to_le_bytes()).collect())))?;
            let out = tensor_buffer((x_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let batch = tensor_encode_group(0, &[h, &g], &[&out], |ctx| {
                ctx.dispatch_rms_norm_f32_offsets(h.id(), g.id(), h.id(), out.id(), &x_s, &g_s, &x_s, eps, 2, [0; 4])?;
                Ok(())
            })?;
            tensor_flush_until(batch)?;
            Ok(unsafe { std::slice::from_raw_parts(out.contents()?, x_s.numel) }.to_vec())
        })
    }

    pub(super) fn transient_pool_clear() {
        if let Err(error) = tensor_pool_clear() {
            super::log_metal_error_once(format!("[ggml][metal] transient pool clear failed: {error}"));
        }
    }

    pub(super) fn try_dit_ffn_resident<F>(
        normed: &[f32],
        m: usize,
        hidden_w: usize,
        ff_dim: usize,
        ff_in_b: &[f32],
        ff_out_b: &[f32],
        ff_in_b_key: &WeightKey,
        ff_out_b_key: &WeightKey,
        swap: bool,
        ff_in: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        ff_out: super::MatmulNtGgmlBytesKeyedMatrix<'_>,
        load: F,
    ) -> Option<Vec<f32>>
    where
        F: FnMut(&WeightOwner, &str) -> Result<Vec<u8>, String>,
    {
        let mut load = load;
        tensor_try(|| {
            if normed.len() != m * hidden_w { return Err("dit ffn normed len".into()); }
            if ff_in.n != ff_dim * 2 || ff_out.n != hidden_w { return Err("dit ffn weight n".into()); }
            let y_s = shape4_from_row_major(&[m, hidden_w], 4)?;
            let x_s = shape4_from_row_major(&[m, ff_in.n], 4)?;
            let b_s = shape4_from_row_major(&[ff_in.n], 4)?;
            let ob_s = shape4_from_row_major(&[hidden_w], 4)?;
            let gated_s = shape4_from_row_major(&[m, ff_dim], 4)?;
            let dn = tensor_upload_f32(normed, &y_s)?;
            let iw = tensor_named_weight(ff_in.owner, ff_in.cache_key, || load(ff_in.owner, ff_in.cache_key).map(Cow::Owned))?;
            let ow = tensor_named_weight(ff_out.owner, ff_out.cache_key, || load(ff_out.owner, ff_out.cache_key).map(Cow::Owned))?;
            let ib = tensor_keyed_weight(ff_in_b_key, || Ok(Cow::Owned(ff_in_b.iter().flat_map(|v| v.to_le_bytes()).collect())))?;
            let ob = tensor_keyed_weight(ff_out_b_key, || Ok(Cow::Owned(ff_out_b.iter().flat_map(|v| v.to_le_bytes()).collect())))?;
            let ffin = tensor_buffer((x_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let ffinb = tensor_buffer((x_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let gated = tensor_buffer((gated_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let ffout = tensor_buffer((y_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let y = tensor_buffer((y_s.numel).checked_mul(4).ok_or("metal output extent overflow")?, false)?;
            let batch = tensor_encode_group(
                m as u64 * (hidden_w as u64 * ff_in.n as u64 + ff_dim as u64 * ff_out.n as u64),
                &[&dn, &iw, &ow, &ib, &ob], &[&ffin, &ffinb, &gated, &ffout, &y], |ctx| {
                ctx.matmul_nt_into_buffer_offsets(ff_in.bt_ggml_type, iw.id(), dn.id(), ffin.id(), m, hidden_w, ff_in.n, [0; 3])?;
                ctx.dispatch_bin_f32_offsets(0, ffin.id(), ib.id(), ffinb.id(), &x_s, &b_s, [0; 3])?;
                ctx.dispatch_swiglu_packed(ffinb.id(), gated.id(), m, ff_dim, swap)?;
                ctx.matmul_nt_into_buffer_offsets(ff_out.bt_ggml_type, ow.id(), gated.id(), ffout.id(), m, ff_dim, ff_out.n, [0; 3])?;
                ctx.dispatch_bin_f32_offsets(0, ffout.id(), ob.id(), y.id(), &y_s, &ob_s, [0; 3])?;
                Ok(())
            })?;
            tensor_flush_until(batch)?;
            Ok(unsafe { std::slice::from_raw_parts(y.contents()?, y_s.numel) }.to_vec())
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
        if n_q == 0 || n_kv == 0 || n_head == 0 || d == 0 {
            return Some(Vec::new());
        }
        let width = n_head.checked_mul(d)?;
        if q.len() != n_q.checked_mul(width)? || k.len() != n_kv.checked_mul(width)? || v.len() != k.len() {
            return None;
        }
        tensor_try(|| {
            let q_shape = shape4_from_row_major(&[n_q, width], 4)?;
            let kv_shape = shape4_from_row_major(&[n_kv, width], 4)?;
            let q = tensor_upload_f32(q, &q_shape)?;
            let k = tensor_upload_f32(k, &kv_shape)?;
            let v = tensor_upload_f32(v, &kv_shape)?;
            tensor_host_result(&q_shape, |out| tensor_attention(&q, 0, &k, 0, &v, 0, out, n_q, n_kv, n_head, d, scale))
        })
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

    pub(super) fn try_add_f32(
        a: &[f32],
        a_shape: &[usize],
        b: &[f32],
        b_shape: &[usize],
    ) -> Option<Vec<f32>> {
        tensor_host_binary(a, a_shape, b, b_shape, false)
    }

    pub(super) fn try_mul_f32(
        a: &[f32],
        a_shape: &[usize],
        b: &[f32],
        b_shape: &[usize],
    ) -> Option<Vec<f32>> {
        tensor_host_binary(a, a_shape, b, b_shape, true)
    }

    pub(super) fn try_gelu_f32(a: &[f32], shape: &[usize]) -> Option<Vec<f32>> {
        tensor_try(|| {
            let shape = shape4_from_row_major(shape, 4)?;
            let input = tensor_upload_f32(a, &shape)?;
            tensor_host_result(&shape, |out| tensor_unary(&input, 0, &shape, out, OP_UNARY_NUM_GELU))
        })
    }

    pub(super) fn try_layer_norm_f32(x: &[f32], shape: &[usize], eps: f32) -> Option<Vec<f32>> {
        tensor_try(|| {
            let shape = shape4_from_row_major(shape, 4)?;
            let input = tensor_upload_f32(x, &shape)?;
            tensor_host_result(&shape, |out| tensor_norm(&input, 0, &shape, &input, 0, &shape,
                &input, 0, &shape, out, eps, 1))
        })
    }

    pub(super) fn try_rms_norm_f32(x: &[f32], shape: &[usize], eps: f32) -> Option<Vec<f32>> {
        tensor_try(|| {
            let shape = shape4_from_row_major(shape, 4)?;
            let input = tensor_upload_f32(x, &shape)?;
            tensor_host_result(&shape, |out| tensor_rms_norm(&input, 0, &shape, &input, 0, &shape,
                &input, 0, &shape, out, eps, 1))
        })
    }

    pub(super) fn try_rms_norm_mul_f32(
        x: &[f32],
        x_shape: &[usize],
        mul: &[f32],
        mul_shape: &[usize],
        eps: f32,
    ) -> Option<Vec<f32>> {
        tensor_try(|| {
            let shape = shape4_from_row_major(x_shape, 4)?;
            let mul_shape = shape4_from_row_major(mul_shape, 4)?;
            let input = tensor_upload_f32(x, &shape)?;
            let mul = tensor_upload_f32(mul, &mul_shape)?;
            tensor_host_result(&shape, |out| tensor_rms_norm(&input, 0, &shape, &mul, 0, &mul_shape,
                &input, 0, &shape, out, eps, 2))
        })
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
        tensor_try(|| {
            let shape = shape4_from_row_major(x_shape, 4)?;
            let mul_shape = shape4_from_row_major(mul_shape, 4)?;
            let add_shape = shape4_from_row_major(add_shape, 4)?;
            let input = tensor_upload_f32(x, &shape)?;
            let mul = tensor_upload_f32(mul, &mul_shape)?;
            let add = tensor_upload_f32(add, &add_shape)?;
            tensor_host_result(&shape, |out| tensor_norm(&input, 0, &shape, &mul, 0, &mul_shape,
                &add, 0, &add_shape, out, eps, 3))
        })
    }

    pub(super) fn try_get_rows_ggml_bytes(
        src: &[u8],
        src_ggml_type: u32,
        n_cols: usize,
        n_rows: usize,
        row_indices: &[i32],
    ) -> Option<Vec<f32>> {
        tensor_try(|| {
            if row_indices.is_empty() {
                return Ok(Vec::new());
            }
            let (src_shape, _, dst_shape) =
                get_rows_shapes(src_ggml_type, n_cols, n_rows, row_indices.len())?;
            let expected_src = src_shape.nb[2] as usize;
            if src.len() != expected_src {
                return Err(format!(
                    "get_rows source len mismatch: got {}, expected {}",
                    src.len(), expected_src,
                ));
            }
            for &row in row_indices {
                let row_ok = usize::try_from(row).ok().is_some_and(|row| row < n_rows);
                if !row_ok {
                    return Err(format!(
                        "get_rows row index {} is out of range {}", row, n_rows,
                    ));
                }
            }
            // Validate the output allocation before uploading either input.
            dst_shape.numel.checked_mul(4).ok_or("metal output extent overflow")?;
            let idx_bytes = unsafe {
                std::slice::from_raw_parts(
                    row_indices.as_ptr().cast::<u8>(), std::mem::size_of_val(row_indices),
                )
            };
            let src = tensor_upload_bytes(src)?;
            let rows = tensor_upload_bytes(idx_bytes)?;
            tensor_host_result(&dst_shape, |out| {
                tensor_get_rows(&src, 0, src_ggml_type, n_cols, n_rows,
                    &rows, 0, row_indices.len(), out)
            })
        })
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
        try_conv2d_planar_f32_with_budget(
            input, width, height, in_channels, weights, bias, out_channels,
            kw, kh, pad_x, pad_y, 256 * 1024 * 1024,
        )
    }

    pub(super) fn try_conv2d_planar_f32_with_budget(
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
        im2col_budget_bytes: usize,
    ) -> Option<Vec<f32>> {
        tensor_try(|| {
            if width == 0 || height == 0 || in_channels == 0 || out_channels == 0 || kw == 0 || kh == 0 {
                return Err("conv2d requires non-empty extents".into());
            }
            let plane = width.checked_mul(height).ok_or("conv2d extent overflow")?;
            if input.len() != in_channels.checked_mul(plane).ok_or("conv2d extent overflow")? {
                return Err("conv2d input shape mismatch".into());
            }
            let x = crate::gpu_tensor::upload(input, in_channels, plane)?;
            let out = crate::gpu_tensor::conv2d_planar_banded(
                &x, width, height, weights, bias, out_channels, kw, kh, pad_x, pad_y, im2col_budget_bytes,
            )?;
            crate::gpu_tensor::download(&out)
        })
    }

    // The diffusion VAE uses the compiled graph path on macOS; these remaining
    // planar group-norm and silu helpers still only have CUDA implementations.
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

}
