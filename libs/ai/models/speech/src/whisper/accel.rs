//! Accelerator dispatch for the plain-Rust Whisper implementation.
//!
//! `src/cpu/*.rs` calls `accel::try_*(...)`. Each entry point offers the work
//! to the compiled-in accelerators in turn and returns the first `Some`;
//! `None` means "no accelerator took it" and the caller runs its own scalar /
//! SIMD path. Nothing here ever fabricates a result — a backend that is absent,
//! unavailable, or has failed returns `None`, never zeros.
//!
//! Selection is honest rather than assumed:
//!
//! | build / box                                   | accelerator |
//! |-----------------------------------------------|-------------|
//! | macOS / iOS                                   | Metal (`src/metal/backend.rs`, gated by `settings::USE_METAL_BACKEND`) |
//! | Android (default), Linux with `MAKEPAD_WHISPER_BACKEND=vulkan` | Vulkan, CPU on failure |
//! | Linux/Windows, CUDA kernels compiled, GPU present | CUDA (`src/cuda/backend.rs`) |
//! | Linux/Windows, no CUDA toolkit at build time  | CPU |
//! | Linux/Windows, kernels built but no device    | CPU |
//! | anything else                                 | CPU |
//!
//! "CUDA kernels compiled" is `makepad-ai-cuda`'s own build-time verdict: its
//! build.rs emits `cargo:kernels=1` only after nvcc actually produced and
//! archived the objects, this crate's build.rs turns that into
//! `makepad_ai_cuda_kernels`, and `src/cuda/backend.rs` is a stub without it.
//! On top of that, `cuda::is_available()` asks the driver for a device count at
//! runtime. So a Windows box with no NVIDIA card, or with the card removed
//! after the build, transcribes on the CPU.
//!
use crate::whisper::model::{DecoderLayer, EncoderLayer};

/// Explicit CPU selection is useful for reference runs. Defaults leave the
/// established Metal/CUDA selection unchanged.
fn cpu_selected() -> bool {
    #[cfg(test)]
    if TEST_CPU.with(|v| v.get()) { return true; }
    std::env::var("MAKEPAD_WHISPER_BACKEND").as_deref() == Ok("cpu")
}

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_CPU: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn vulkan_selected() -> bool {
    if cpu_selected() { return false; }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    return crate::whisper::vulkan_backend::is_selected();
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    false
}

/// `"vulkan"`, `"metal"`, `"cuda"` or `"cpu"` — the available selected backend.
/// Reported by the test binaries so a run can never be mislabelled.
pub fn backend_name() -> &'static str {
    if cpu_selected() { return "cpu"; }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if vulkan_selected() {
        return if crate::whisper::vulkan_backend::is_available() { "vulkan" } else { "cpu" };
    }
    if crate::whisper::metal_backend::is_requested() {
        return "metal";
    }
    if crate::whisper::cuda_backend::is_available() {
        return "cuda";
    }
    "cpu"
}

/// True when *some* accelerator is present. The CPU code uses this to pick
/// between shapes of the same computation (see `src/cpu/decoder.rs`), so it
/// must not claim more than [`backend_name`] does.
pub(crate) fn is_requested() -> bool {
    backend_name() != "cpu"
}

/// An explicitly selected Vulkan path falls back to CPU, never to a different
/// GPU backend. The established ordering is unchanged otherwise.
macro_rules! dispatch {
    ($name:ident ( $($arg:expr),* $(,)? )) => {{
        if cpu_selected() { return None; }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if vulkan_selected() {
            return crate::whisper::vulkan_backend::$name($($arg),*);
        }
        if let Some(out) = crate::whisper::metal_backend::$name($($arg),*) {
            return Some(out);
        }
        crate::whisper::cuda_backend::$name($($arg),*)
    }};
}

pub(crate) fn try_matmul_nn_f32(
    a: &[f32],
    b: &[f32],
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    dispatch!(try_matmul_nn_f32(a, b, m, k, n))
}

pub(crate) fn try_matmul_nt_f32(
    a: &[f32],
    bt: &[f32],
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    dispatch!(try_matmul_nt_f32(a, bt, m, k, n))
}

pub(crate) fn try_matmul_nt_f32_bytes(
    a: &[f32],
    bt_bytes: &[u8],
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    dispatch!(try_matmul_nt_f32_bytes(a, bt_bytes, m, k, n))
}

pub(crate) fn try_matmul_nt_f16_bytes(
    a: &[f32],
    bt_f16_bytes: &[u8],
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    dispatch!(try_matmul_nt_f16_bytes(a, bt_f16_bytes, m, k, n))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_matmul_nt_ggml_bytes(
    a: &[f32],
    bt_bytes: &[u8],
    bt_ggml_type: u32,
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    dispatch!(try_matmul_nt_ggml_bytes(
        a,
        bt_bytes,
        bt_ggml_type,
        m,
        k,
        n
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_matmul_nt_ggml_bytes_add_bias(
    a: &[f32],
    bt_bytes: &[u8],
    bt_ggml_type: u32,
    m: usize,
    k: usize,
    n: usize,
    bias: &[f32],
) -> Option<Vec<f32>> {
    dispatch!(try_matmul_nt_ggml_bytes_add_bias(
        a,
        bt_bytes,
        bt_ggml_type,
        m,
        k,
        n,
        bias
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_flash_attn_f32_packed(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    n_q: usize,
    n_kv: usize,
    n_head: usize,
    d: usize,
    scale: f32,
) -> Option<Vec<f32>> {
    dispatch!(try_flash_attn_f32_packed(
        q, k, v, n_q, n_kv, n_head, d, scale
    ))
}

/// Not gated on [`set_enabled`]: dropping stale device-side K/V is never
/// harmful, and skipping it while acceleration is off would leave a cache from
/// a previous chunk alive for the next one that re-enables it.
/// Free this thread's device-resident Whisper state (see the CUDA backend).
pub fn release_device_state() {
    crate::whisper::cuda_backend::release_device_state();
}

pub(crate) fn clear_decoder_kv_cache() {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    crate::whisper::vulkan_backend::clear_decoder_kv_cache();
    crate::whisper::metal_backend::clear_decoder_kv_cache();
    crate::whisper::cuda_backend::clear_decoder_kv_cache();
}

/// Larger Vulkan seams avoid intermediate host transfers while preserving the
/// existing primitive seams for CPU fallback and alignment capture.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn try_encode(model: &crate::whisper::model::WhisperModel, mel: &[f32], frames: usize) -> Option<Vec<f32>> {
    if !vulkan_selected() { return None; }
    crate::whisper::vulkan_backend::try_encode(model, mel, frames)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn try_cross_kv(model: &crate::whisper::model::WhisperModel, input: &crate::whisper::tensor::Tensor) -> Option<Vec<(crate::whisper::tensor::Tensor, crate::whisper::tensor::Tensor)>> {
    if !vulkan_selected() { return None; }
    crate::whisper::vulkan_backend::try_cross_kv(model, input)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn try_decode(model: &crate::whisper::model::WhisperModel, tokens: &[i32], positions: &[i32], cache: &mut crate::whisper::decoder::KvCache, cross: &[(crate::whisper::tensor::Tensor, crate::whisper::tensor::Tensor)]) -> Option<Vec<f32>> {
    if !vulkan_selected() { return None; }
    crate::whisper::vulkan_backend::try_decode(model, tokens, positions, cache, cross)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_flash_attn_f32_self_kv_cache(
    layer: usize,
    q: &[f32],
    k_all: &[f32],
    v_all: &[f32],
    n_kv: usize,
    n_head: usize,
    d: usize,
    scale: f32,
) -> Option<Vec<f32>> {
    dispatch!(try_flash_attn_f32_self_kv_cache(
        layer, q, k_all, v_all, n_kv, n_head, d, scale
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_flash_attn_f32_cross_kv_cache(
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
    dispatch!(try_flash_attn_f32_cross_kv_cache(
        layer, q, k_cross, v_cross, n_q, n_kv, n_head, d, scale
    ))
}

pub(crate) fn try_add_f32(
    a: &[f32],
    a_shape: &[usize],
    b: &[f32],
    b_shape: &[usize],
) -> Option<Vec<f32>> {
    dispatch!(try_add_f32(a, a_shape, b, b_shape))
}

pub(crate) fn try_mul_f32(
    a: &[f32],
    a_shape: &[usize],
    b: &[f32],
    b_shape: &[usize],
) -> Option<Vec<f32>> {
    dispatch!(try_mul_f32(a, a_shape, b, b_shape))
}

pub(crate) fn try_gelu_f32(a: &[f32], shape: &[usize]) -> Option<Vec<f32>> {
    dispatch!(try_gelu_f32(a, shape))
}

pub(crate) fn try_layer_norm_f32(x: &[f32], shape: &[usize], eps: f32) -> Option<Vec<f32>> {
    dispatch!(try_layer_norm_f32(x, shape, eps))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_layer_norm_mul_add_f32(
    x: &[f32],
    x_shape: &[usize],
    mul: &[f32],
    mul_shape: &[usize],
    add: &[f32],
    add_shape: &[usize],
    eps: f32,
) -> Option<Vec<f32>> {
    dispatch!(try_layer_norm_mul_add_f32(
        x, x_shape, mul, mul_shape, add, add_shape, eps
    ))
}

pub(crate) fn try_im2col_1d_f32(
    input: &[f32],
    ic: usize,
    iw: usize,
    kw: usize,
    stride: usize,
    pad: usize,
) -> Option<Vec<f32>> {
    dispatch!(try_im2col_1d_f32(input, ic, iw, kw, stride, pad))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_encoder_attn_block_f32(
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
    dispatch!(try_encoder_attn_block_f32(
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
        out_b
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_encoder_ffn_block_f32(
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
    dispatch!(try_encoder_ffn_block_f32(
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
        b1
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_encoder_layer_f32(
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
    dispatch!(try_encoder_layer_f32(
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
        b1
    ))
}

pub(crate) fn try_encoder_stack_f32(
    x: &[f32],
    seq_len: usize,
    n_state: usize,
    n_head: usize,
    layers: &[EncoderLayer],
    final_ln_w: &[f32],
    final_ln_b: &[f32],
) -> Option<Vec<f32>> {
    dispatch!(try_encoder_stack_f32(
        x, seq_len, n_state, n_head, layers, final_ln_w, final_ln_b
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_decoder_self_qkv_step_f32(
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
    dispatch!(try_decoder_self_qkv_step_f32(
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
        v_b
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_decoder_cross_ffn_step_f32(
    layer_idx: usize,
    x: &[f32],
    n_state: usize,
    n_head: usize,
    k_cross: &[f32],
    v_cross: &[f32],
    n_audio_ctx: usize,
    layer: &DecoderLayer,
) -> Option<Vec<f32>> {
    dispatch!(try_decoder_cross_ffn_step_f32(
        layer_idx,
        x,
        n_state,
        n_head,
        k_cross,
        v_cross,
        n_audio_ctx,
        layer
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_decoder_self_cross_ffn_step_f32(
    layer_idx: usize,
    x: &[f32],
    q_self: &[f32],
    k_all: &[f32],
    v_all: &[f32],
    n_kv: usize,
    n_state: usize,
    n_head: usize,
    k_cross: &[f32],
    v_cross: &[f32],
    n_audio_ctx: usize,
    layer: &DecoderLayer,
) -> Option<Vec<f32>> {
    dispatch!(try_decoder_self_cross_ffn_step_f32(
        layer_idx,
        x,
        q_self,
        k_all,
        v_all,
        n_kv,
        n_state,
        n_head,
        k_cross,
        v_cross,
        n_audio_ctx,
        layer
    ))
}
