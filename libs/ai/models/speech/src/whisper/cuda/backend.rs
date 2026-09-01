//! CUDA acceleration primitives for the plain-Rust Whisper implementation in
//! `src/cpu/`.
//!
//! This is the exact same seam `src/metal/backend.rs` implements: the CPU code
//! calls `accel::try_*(...)`, and a backend either returns `Some(result)` or
//! `None`, in which case the scalar/SIMD CPU path runs. It is deliberately NOT
//! a compiled-graph executor.
//!
//! ## Why only a subset is implemented
//!
//! On Metal the seam is nearly free: `newBufferWithBytes` on unified memory is
//! a pointer wrap, so even a one-FLOP-per-element kernel is a win. On CUDA
//! every `try_*` call is a full host->device->host round trip over PCIe. That
//! inverts the economics:
//!
//! * **Worth it** — ops whose arithmetic dwarfs the bytes moved, *and* whose
//!   big operand (the weight, the cross-attention K/V) can stay device
//!   resident across calls: the matmul family, the encoder's self-attention,
//!   and the decoder's cross-attention.
//! * **A pessimization** — elementwise ops (`add`/`mul`/`gelu`/`layer_norm`).
//!   One float in, one float out, a handful of FLOPs: the PCIe round trip
//!   costs more than the CPU spends computing them. They are implemented here
//!   (so the on-box harness can A/B them) but are **off unless**
//!   `MAKEPAD_VOICE_CUDA_ELEMENTWISE=1`.
//! * **Deliberately `None`** — every fused whole-block op
//!   (`try_encoder_stack_f32`, `try_encoder_layer_f32`,
//!   `try_encoder_attn_block_f32`, `try_encoder_ffn_block_f32`,
//!   `try_decoder_self_qkv_step_f32`, `try_decoder_cross_ffn_step_f32`,
//!   `try_decoder_self_cross_ffn_step_f32`) and
//!   `try_flash_attn_f32_self_kv_cache`. A correct-but-unfused CUDA backend is
//!   a real deliverable; a fused-but-wrong one is not. Fusing them is the
//!   obvious next step and is where the remaining decoder win lives, because
//!   it would keep activations on the device between ops instead of paying a
//!   round trip per primitive.
//!
//! ## Numerics
//!
//! Weights are dequantized **to f32 once** on first use and kept device
//! resident; every GEMM is `cublasSgemm` f32 in / f32 accumulate / f32 out,
//! and attention uses the explicit-f32 `gpu_attention_packed_cross_f32`
//! composite. That is the closest reachable match to the CPU ground truth
//! (which also dequantizes to f32 and accumulates in f32), which is what makes
//! the "token-identical at temperature 0" parity gate meaningful. No f16/bf16
//! tensor-core path is taken.
//!
//! Resident f32 weights cost 4 bytes/parameter: ~300 MB for `base`, ~1 GB for
//! `small`, ~3.2 GB for `large-v3-turbo`, ~6.2 GB for `large-v3`.

use crate::whisper::model::{DecoderLayer, EncoderLayer};

/// `MAKEPAD_VOICE_CUDA=0|false|no|off` forces this backend off entirely.
fn env_off(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => false,
    }
}

fn env_on(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

/// True when the CUDA kernels were compiled into this build *and* the driver
/// reports at least one device. Probed once; never panics.
pub(crate) fn is_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        if env_off("MAKEPAD_VOICE_CUDA") {
            return false;
        }
        imp::device_available()
    })
}

/// Set on the first hard CUDA error. Everything after returns `None` so the
/// CPU path takes over; a partially-failed device must never yield zeros.
static DEGRADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn degraded() -> bool {
    DEGRADED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Report the first failure loudly (once), then disable the backend.
fn fail<T>(what: &str, err: String) -> Option<T> {
    if !DEGRADED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!("[voice] CUDA backend disabled after {what} failed: {err}");
    }
    None
}

fn ok<T>(what: &str, r: Result<T, String>) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(err) => fail(what, err),
    }
}

fn ready() -> bool {
    is_available() && !degraded()
}

/// Elementwise ops are a net loss through a host round trip; opt in explicitly.
fn elementwise_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| env_on("MAKEPAD_VOICE_CUDA_ELEMENTWISE"))
}

/// Minimum query count for the packed (non-resident) attention path. The
/// encoder runs it at n_q = n_ctx = 1500 and wins big; the decoder would run it
/// at n_q = 1 against a 1500-row K/V that has to be re-uploaded every token,
/// which is far slower than the CPU. Cross-attention gets the resident cache
/// instead (see `try_flash_attn_f32_cross_kv_cache`).
fn min_attn_q() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| env_usize("MAKEPAD_VOICE_CUDA_MIN_ATTN_Q", 64))
}

/// Degenerate-shape guard for the matmul family: below this many weight
/// elements the launch + round-trip latency dominates. Every Whisper
/// projection is far above it (the smallest, `tiny`, is 384x384 = 147456).
fn min_matmul_weight_elems() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| env_usize("MAKEPAD_VOICE_CUDA_MIN_MATMUL", 65536))
}

// ---------------------------------------------------------------------------
// Pure host helpers (compiled everywhere so they are unit-testable on macOS).
// ---------------------------------------------------------------------------

/// Where a weight's values come from. Keeping this an enum (rather than a
/// `&[u8]` plus an "actually it's f32" flag) is what lets the already-f32
/// callers skip both a dequantize copy and a byte reinterpretation.
#[allow(dead_code)]
enum WeightSrc<'a> {
    /// Raw ggml bytes plus their type; dequantized to f32 on upload.
    Ggml { bytes: &'a [u8], ggml_type: u32 },
    /// Values the caller already holds as f32.
    F32(&'a [f32]),
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[allow(dead_code)]
fn fnv_mix(h: &mut u64, v: u64) {
    for b in v.to_le_bytes() {
        *h ^= b as u64;
        *h = h.wrapping_mul(FNV_PRIME);
    }
}

/// FNV-1a over the byte length plus a bounded, evenly spaced sample of the
/// contents (always including the last word).
///
/// Used to disambiguate device weight-cache entries. `src/metal/backend.rs`
/// keys its cached weight buffers on `(host pointer, length)` alone; a freed
/// tensor whose allocation is reused by a different tensor of the same size
/// would hit a stale entry and silently compute with the wrong weights.
/// Sampling the contents is O(1) and removes that class of bug outright.
#[allow(dead_code)]
fn fingerprint_words(n: usize, byte_len: usize, word: impl Fn(usize) -> u64) -> u64 {
    let mut h = FNV_OFFSET;
    fnv_mix(&mut h, byte_len as u64);
    if n == 0 {
        return h;
    }
    const SAMPLES: usize = 64;
    let step = (n / SAMPLES).max(1);
    let mut i = 0;
    while i < n {
        fnv_mix(&mut h, word(i));
        i += step;
    }
    fnv_mix(&mut h, word(n - 1));
    h
}

#[allow(dead_code)]
fn fingerprint(bytes: &[u8]) -> u64 {
    let n = bytes.len().div_ceil(8);
    fingerprint_words(n, bytes.len(), |i| {
        let off = i * 8;
        let end = (off + 8).min(bytes.len());
        let mut w = [0u8; 8];
        w[..end - off].copy_from_slice(&bytes[off..end]);
        u64::from_le_bytes(w)
    })
}

#[allow(dead_code)]
fn fingerprint_f32(values: &[f32]) -> u64 {
    fingerprint_words(values.len(), std::mem::size_of_val(values), |i| {
        values[i].to_bits() as u64
    })
}

/// Dequantize a whole ggml weight blob to f32 in the same element order.
///
/// Mirrors `RawTensor::to_f32` (`src/cpu/tensor.rs`) exactly, but borrows the
/// bytes instead of cloning them into a `RawTensor` first. Returns `None` for
/// types the CPU path does not produce either.
#[allow(dead_code)]
fn dequant_to_f32(bytes: &[u8], ggml_type: u32, n_elements: usize) -> Option<Vec<f32>> {
    use crate::whisper::quant::*;
    let mut out = vec![0.0f32; n_elements];
    match ggml_type {
        GGML_TYPE_F32 => {
            if bytes.len() < n_elements * 4 {
                return None;
            }
            for (i, slot) in out.iter_mut().enumerate() {
                let o = i * 4;
                *slot = f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
            }
        }
        GGML_TYPE_F16 => {
            if bytes.len() < n_elements * 2 {
                return None;
            }
            for (i, slot) in out.iter_mut().enumerate() {
                let o = i * 2;
                *slot = f16_to_f32(u16::from_le_bytes([bytes[o], bytes[o + 1]]));
            }
        }
        GGML_TYPE_Q4_0 | GGML_TYPE_Q4_1 | GGML_TYPE_Q5_0 | GGML_TYPE_Q5_1 | GGML_TYPE_Q8_0 => {
            if n_elements % QK != 0 {
                return None;
            }
            let bs = block_size(ggml_type);
            let nb = n_elements / QK;
            if bytes.len() < nb * bs {
                return None;
            }
            let mut tmp = [0.0f32; QK];
            for b in 0..nb {
                let block = &bytes[b * bs..];
                match ggml_type {
                    GGML_TYPE_Q4_0 => dequantize_q4_0(block, &mut tmp),
                    GGML_TYPE_Q4_1 => dequantize_q4_1(block, &mut tmp),
                    GGML_TYPE_Q5_0 => dequantize_q5_0(block, &mut tmp),
                    GGML_TYPE_Q5_1 => dequantize_q5_1(block, &mut tmp),
                    _ => dequantize_q8_0(block, &mut tmp),
                }
                out[b * QK..(b + 1) * QK].copy_from_slice(&tmp);
            }
        }
        _ => return None,
    }
    Some(out)
}

/// Host-side 1-D im2col, byte-for-byte the layout `Tensor::conv1d` expects.
///
/// `input` is `[ic, iw]` row-major; the result is `[ow, ic * kw]` row-major
/// with `out[t][ci * kw + kk] = input[ci][t * stride + kk - pad]` (zero outside
/// the input). This one stays on the host on purpose: it is a pure gather with
/// zero arithmetic, so running it on the device would only add a round trip.
/// The GEMM that consumes it is what goes to the GPU.
#[allow(dead_code)]
fn im2col_1d(
    input: &[f32],
    ic: usize,
    iw: usize,
    kw: usize,
    stride: usize,
    pad: usize,
) -> Option<Vec<f32>> {
    if kw == 0 || stride == 0 {
        return None;
    }
    if input.len() != ic.checked_mul(iw)? {
        return None;
    }
    let num = iw.checked_add(pad.checked_mul(2)?)?;
    if num < kw {
        return Some(Vec::new());
    }
    let ow = (num - kw) / stride + 1;
    let chw = ic.checked_mul(kw)?;
    let mut out = vec![0.0f32; ow.checked_mul(chw)?];
    for t in 0..ow {
        let base = t * stride;
        let row = &mut out[t * chw..(t + 1) * chw];
        for ci in 0..ic {
            let in_row = &input[ci * iw..(ci + 1) * iw];
            for kk in 0..kw {
                let p = (base + kk) as isize - pad as isize;
                if p >= 0 && (p as usize) < iw {
                    row[ci * kw + kk] = in_row[p as usize];
                }
            }
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// The seam. Signatures mirror `src/metal/backend.rs` one for one.
// ---------------------------------------------------------------------------

pub(crate) fn try_matmul_nn_f32(
    _a: &[f32],
    _b: &[f32],
    _m: usize,
    _k: usize,
    _n: usize,
) -> Option<Vec<f32>> {
    // `Tensor::matmul` (the only caller) is unused by the Whisper graph — every
    // real matmul goes through the NT forms. Left unimplemented rather than
    // carrying an untested host transpose.
    None
}

pub(crate) fn try_matmul_nt_f32(
    a: &[f32],
    bt: &[f32],
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    if !ready() || k.saturating_mul(n) < min_matmul_weight_elems() {
        return None;
    }
    if a.len() != m.checked_mul(k)? || bt.len() != n.checked_mul(k)? {
        return None;
    }
    // `bt` is already f32, so hand it through directly: the decoder's logits
    // projection reuses `model.d_te`, which is 265 MB on large-v3, and a
    // dequantize-into-a-new-Vec step would spike host memory by that much.
    ok(
        "matmul_nt_f32",
        imp::matmul_nt(a, WeightSrc::F32(bt), m, k, n, None),
    )
}

pub(crate) fn try_matmul_nt_f32_bytes(
    a: &[f32],
    bt_bytes: &[u8],
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    matmul_nt_bytes(a, bt_bytes, crate::whisper::quant::GGML_TYPE_F32, m, k, n, None)
}

pub(crate) fn try_matmul_nt_f16_bytes(
    a: &[f32],
    bt_f16_bytes: &[u8],
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    matmul_nt_bytes(a, bt_f16_bytes, crate::whisper::quant::GGML_TYPE_F16, m, k, n, None)
}

pub(crate) fn try_matmul_nt_ggml_bytes(
    a: &[f32],
    bt_bytes: &[u8],
    bt_ggml_type: u32,
    m: usize,
    k: usize,
    n: usize,
) -> Option<Vec<f32>> {
    matmul_nt_bytes(a, bt_bytes, bt_ggml_type, m, k, n, None)
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
    if bias.len() != n {
        return None;
    }
    matmul_nt_bytes(a, bt_bytes, bt_ggml_type, m, k, n, Some(bias))
}

fn matmul_nt_bytes(
    a: &[f32],
    bt_bytes: &[u8],
    bt_ggml_type: u32,
    m: usize,
    k: usize,
    n: usize,
    bias: Option<&[f32]>,
) -> Option<Vec<f32>> {
    if !ready() || k.saturating_mul(n) < min_matmul_weight_elems() {
        return None;
    }
    if a.len() != m.checked_mul(k)? {
        return None;
    }
    ok(
        "matmul_nt",
        imp::matmul_nt(
            a,
            WeightSrc::Ggml {
                bytes: bt_bytes,
                ggml_type: bt_ggml_type,
            },
            m,
            k,
            n,
            bias,
        ),
    )
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
    if !ready() || n_q < min_attn_q() {
        return None;
    }
    let hidden = n_head.checked_mul(d)?;
    if q.len() != n_q.checked_mul(hidden)?
        || k.len() != n_kv.checked_mul(hidden)?
        || v.len() != k.len()
    {
        return None;
    }
    ok(
        "flash_attn_packed",
        imp::attention(q, k, v, n_q, n_kv, n_head, d, scale),
    )
}

pub(crate) fn clear_decoder_kv_cache() {
    if is_available() {
        imp::clear_cross_kv_cache();
    }
}

/// Deliberately `None`: the decoder's self-attention is `n_q = 1` against at
/// most `n_text_ctx` (448) cached rows — under 1 MFLOP. Making it resident
/// would still cost a per-token upload of the new K/V row plus a device
/// round trip for a result the CPU produces in microseconds.
#[allow(clippy::too_many_arguments)]
pub(crate) fn try_flash_attn_f32_self_kv_cache(
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

/// Cross-attention with a device-resident K/V cache.
///
/// NOTE on `n_kv`: the decoder passes `pad_to(n_audio_ctx, 256)` here whenever
/// an accelerator is present (`src/cpu/decoder.rs`), which is a Metal capacity
/// hint, *not* the number of valid rows — `k_cross`/`v_cross` still hold
/// exactly `n_audio_ctx` rows. The true length is taken from the slice, and
/// `n_kv` is only used as an upper bound. Getting this backwards would attend
/// over 36 rows of garbage.
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
    if !ready() {
        return None;
    }
    let hidden = n_head.checked_mul(d)?;
    if hidden == 0 || k_cross.len() % hidden != 0 || v_cross.len() != k_cross.len() {
        return None;
    }
    let true_n_kv = k_cross.len() / hidden;
    if true_n_kv == 0 || true_n_kv > n_kv || q.len() != n_q.checked_mul(hidden)? {
        return None;
    }
    ok(
        "flash_attn_cross_kv_cache",
        imp::attention_cross_cached(layer, q, k_cross, v_cross, n_q, true_n_kv, n_head, d, scale),
    )
}

pub(crate) fn try_add_f32(
    a: &[f32],
    a_shape: &[usize],
    b: &[f32],
    b_shape: &[usize],
) -> Option<Vec<f32>> {
    if !ready() || !elementwise_enabled() {
        return None;
    }
    let _ = (a_shape, b_shape);
    // Broadcast (`b` shorter than `a`) is the bias case; tiling it on the host
    // would cost as much as the CPU add itself, so leave it to the CPU.
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    ok("add_f32", imp::binary(a, b, false))
}

pub(crate) fn try_mul_f32(
    a: &[f32],
    a_shape: &[usize],
    b: &[f32],
    b_shape: &[usize],
) -> Option<Vec<f32>> {
    if !ready() || !elementwise_enabled() {
        return None;
    }
    let _ = (a_shape, b_shape);
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    ok("mul_f32", imp::binary(a, b, true))
}

pub(crate) fn try_gelu_f32(a: &[f32], shape: &[usize]) -> Option<Vec<f32>> {
    if !ready() || !elementwise_enabled() || a.is_empty() {
        return None;
    }
    let _ = shape;
    ok("gelu_f32", imp::gelu(a))
}

pub(crate) fn try_layer_norm_f32(x: &[f32], shape: &[usize], eps: f32) -> Option<Vec<f32>> {
    if !ready() || !elementwise_enabled() {
        return None;
    }
    let cols = *shape.last()?;
    if cols == 0 || x.is_empty() || x.len() % cols != 0 {
        return None;
    }
    let ones = vec![1.0f32; cols];
    let zeros = vec![0.0f32; cols];
    ok(
        "layer_norm_f32",
        imp::layer_norm(x, x.len() / cols, cols, &ones, &zeros, eps),
    )
}

pub(crate) fn try_layer_norm_mul_add_f32(
    x: &[f32],
    x_shape: &[usize],
    mul: &[f32],
    mul_shape: &[usize],
    add: &[f32],
    add_shape: &[usize],
    eps: f32,
) -> Option<Vec<f32>> {
    if !ready() || !elementwise_enabled() {
        return None;
    }
    let _ = (mul_shape, add_shape);
    let cols = *x_shape.last()?;
    if cols == 0 || x.is_empty() || x.len() % cols != 0 || mul.len() != cols || add.len() != cols {
        return None;
    }
    ok(
        "layer_norm_mul_add_f32",
        imp::layer_norm(x, x.len() / cols, cols, mul, add, eps),
    )
}

pub(crate) fn try_im2col_1d_f32(
    input: &[f32],
    ic: usize,
    iw: usize,
    kw: usize,
    stride: usize,
    pad: usize,
) -> Option<Vec<f32>> {
    // Only worth producing when the GEMM that consumes it will run on the
    // device; otherwise `Tensor::conv1d`'s direct loop is strictly better.
    if !ready() {
        return None;
    }
    im2col_1d(input, ic, iw, kw, stride, pad)
}

// --- fused whole-block ops: deliberately unimplemented on CUDA -------------
//
// Each of these would keep a whole encoder/decoder block resident on the
// device. That is the right next step (it is where the remaining decoder win
// is), but an unverified fused block is worse than no fused block: the
// unfused path below is exercised by the same parity harness and is known to
// match the CPU.

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_encoder_attn_block_f32(
    _x: &[f32],
    _seq_len: usize,
    _n_state: usize,
    _n_head: usize,
    _ln_w: &[f32],
    _ln_b: &[f32],
    _q_w_bytes: &[u8],
    _q_w_ggml_type: u32,
    _q_b: &[f32],
    _k_w_bytes: &[u8],
    _k_w_ggml_type: u32,
    _v_w_bytes: &[u8],
    _v_w_ggml_type: u32,
    _v_b: &[f32],
    _out_w_bytes: &[u8],
    _out_w_ggml_type: u32,
    _out_b: &[f32],
) -> Option<Vec<f32>> {
    None
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_encoder_ffn_block_f32(
    _x: &[f32],
    _seq_len: usize,
    _n_state: usize,
    _ln_w: &[f32],
    _ln_b: &[f32],
    _w0_bytes: &[u8],
    _w0_ggml_type: u32,
    _b0: &[f32],
    _w1_bytes: &[u8],
    _w1_ggml_type: u32,
    _b1: &[f32],
) -> Option<Vec<f32>> {
    None
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_encoder_layer_f32(
    _x: &[f32],
    _seq_len: usize,
    _n_state: usize,
    _n_head: usize,
    _attn_ln_w: &[f32],
    _attn_ln_b: &[f32],
    _q_w_bytes: &[u8],
    _q_w_ggml_type: u32,
    _q_b: &[f32],
    _k_w_bytes: &[u8],
    _k_w_ggml_type: u32,
    _v_w_bytes: &[u8],
    _v_w_ggml_type: u32,
    _v_b: &[f32],
    _out_w_bytes: &[u8],
    _out_w_ggml_type: u32,
    _out_b: &[f32],
    _mlp_ln_w: &[f32],
    _mlp_ln_b: &[f32],
    _w0_bytes: &[u8],
    _w0_ggml_type: u32,
    _b0: &[f32],
    _w1_bytes: &[u8],
    _w1_ggml_type: u32,
    _b1: &[f32],
) -> Option<Vec<f32>> {
    None
}

pub(crate) fn try_encoder_stack_f32(
    _x: &[f32],
    _seq_len: usize,
    _n_state: usize,
    _n_head: usize,
    _layers: &[EncoderLayer],
    _final_ln_w: &[f32],
    _final_ln_b: &[f32],
) -> Option<Vec<f32>> {
    None
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_decoder_self_qkv_step_f32(
    _x: &[f32],
    _n_state: usize,
    _attn_ln_w: &[f32],
    _attn_ln_b: &[f32],
    _q_w_bytes: &[u8],
    _q_w_ggml_type: u32,
    _q_b: &[f32],
    _k_w_bytes: &[u8],
    _k_w_ggml_type: u32,
    _v_w_bytes: &[u8],
    _v_w_ggml_type: u32,
    _v_b: &[f32],
) -> Option<(Vec<f32>, Vec<f32>, Vec<f32>)> {
    None
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_decoder_cross_ffn_step_f32(
    _layer_idx: usize,
    _x: &[f32],
    _n_state: usize,
    _n_head: usize,
    _k_cross: &[f32],
    _v_cross: &[f32],
    _n_audio_ctx: usize,
    _layer: &DecoderLayer,
) -> Option<Vec<f32>> {
    None
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_decoder_self_cross_ffn_step_f32(
    _layer_idx: usize,
    _x: &[f32],
    _q_self: &[f32],
    _k_all: &[f32],
    _v_all: &[f32],
    _n_kv: usize,
    _n_state: usize,
    _n_head: usize,
    _k_cross: &[f32],
    _v_cross: &[f32],
    _n_audio_ctx: usize,
    _layer: &DecoderLayer,
) -> Option<Vec<f32>> {
    None
}

// ---------------------------------------------------------------------------
// Device implementation. Present only where `makepad-ai-cuda`'s build.rs
// actually compiled and linked the kernels (it emits `cargo:kernels=1`, which
// this crate's build.rs turns into `makepad_ai_cuda_kernels`). Anywhere else —
// macOS, a Windows box with no CUDA toolkit, a Linux box with no nvcc — the
// stub module below compiles instead and every entry point above degrades to
// the CPU path.
// ---------------------------------------------------------------------------

#[cfg(all(
    any(target_os = "linux", target_os = "windows"),
    makepad_ai_cuda_kernels
))]
mod imp {
    use super::{dequant_to_f32, fingerprint, fingerprint_f32, WeightSrc};
    use makepad_ai_cuda::launch::{
        gpu_add, gpu_attention_packed_cross_f32, gpu_device_available, gpu_download, gpu_gelu,
        gpu_layer_norm_mul_add, gpu_linear_f32_resident, gpu_mul, gpu_upload, GpuTensor,
    };
    use std::cell::RefCell;
    use std::collections::HashMap;

    pub(super) fn device_available() -> bool {
        gpu_device_available()
    }

    /// Identity of a device-resident host blob: address, byte length, a
    /// content fingerprint, and the shape it was uploaded for.
    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    struct ResidentKey {
        ptr: usize,
        len: usize,
        fp: u64,
        rows: usize,
        cols: usize,
    }

    impl ResidentKey {
        fn of(bytes: &[u8], rows: usize, cols: usize) -> Self {
            Self {
                ptr: bytes.as_ptr() as usize,
                len: bytes.len(),
                fp: fingerprint(bytes),
                rows,
                cols,
            }
        }

        fn of_f32(values: &[f32], rows: usize, cols: usize) -> Self {
            Self {
                ptr: values.as_ptr() as usize,
                len: std::mem::size_of_val(values),
                fp: fingerprint_f32(values),
                rows,
                cols,
            }
        }

        fn of_src(src: &WeightSrc<'_>, rows: usize, cols: usize) -> Self {
            match src {
                WeightSrc::Ggml { bytes, .. } => Self::of(bytes, rows, cols),
                WeightSrc::F32(values) => Self::of_f32(values, rows, cols),
            }
        }
    }

    // `makepad-ai-cuda`'s whole `gpu_*` surface hangs off a thread-local
    // backend (stream, cuBLAS handle, memory pool) and `GpuTensor` holds an
    // `Rc`, so the residency maps must be thread-local too. Whisper drives all
    // of its `try_*` calls from one thread; a thread hop costs a re-upload,
    // never a wrong answer.
    thread_local! {
        static WEIGHTS: RefCell<HashMap<ResidentKey, GpuTensor>> =
            RefCell::new(HashMap::new());
        static CROSS_KV: RefCell<HashMap<usize, CrossEntry>> = RefCell::new(HashMap::new());
    }

    struct CrossEntry {
        k_key: ResidentKey,
        k: GpuTensor,
        v: GpuTensor,
    }

    /// Guard against unbounded growth if a caller ever hands us a temporary
    /// weight (the `Tensor::conv1d` fallback can). Whisper large-v3 has 1220
    /// weight tensors; 4096 leaves generous headroom while still bounding a
    /// pathological loop.
    const MAX_RESIDENT_WEIGHTS: usize = 4096;

    /// Run `f` with the device-resident f32 copy of `src` (uploading it the
    /// first time it is seen).
    fn with_weight<R>(
        src: WeightSrc<'_>,
        n: usize,
        k: usize,
        f: impl FnOnce(&GpuTensor) -> Result<R, String>,
    ) -> Result<R, String> {
        let key = ResidentKey::of_src(&src, n, k);
        WEIGHTS.with(|cache| {
            let mut cache = cache.borrow_mut();
            if !cache.contains_key(&key) {
                let elems = n
                    .checked_mul(k)
                    .ok_or_else(|| "weight size overflow".to_string())?;
                if cache.len() >= MAX_RESIDENT_WEIGHTS {
                    cache.clear();
                }
                let uploaded = match src {
                    WeightSrc::F32(values) if values.len() == elems => {
                        gpu_upload(values, n, k)?
                    }
                    WeightSrc::F32(values) => {
                        return Err(format!(
                            "f32 weight length {} does not match n={n} k={k}",
                            values.len()
                        ))
                    }
                    WeightSrc::Ggml { bytes, ggml_type } => {
                        let values =
                            dequant_to_f32(bytes, ggml_type, elems).ok_or_else(|| {
                                format!(
                                    "unsupported ggml weight type {ggml_type} for n={n} k={k}"
                                )
                            })?;
                        gpu_upload(&values, n, k)?
                    }
                };
                cache.insert(key, uploaded);
            }
            let tensor = cache
                .get(&key)
                .ok_or_else(|| "weight was just inserted".to_string())?;
            f(tensor)
        })
    }

    /// `out[m, n] = a[m, k] @ W[n, k]^T (+ bias[n])`, f32 throughout.
    pub(super) fn matmul_nt(
        a: &[f32],
        weight: WeightSrc<'_>,
        m: usize,
        k: usize,
        n: usize,
        bias: Option<&[f32]>,
    ) -> Result<Vec<f32>, String> {
        let x = gpu_upload(a, m, k)?;
        let bias_dev = match bias {
            Some(b) => Some(gpu_upload(b, 1, n)?),
            None => None,
        };
        with_weight(weight, n, k, |w| {
            let out = gpu_linear_f32_resident(&x, w, bias_dev.as_ref())?;
            gpu_download(&out)
        })
    }

    /// Non-causal, maskless multi-head attention on token-major packed q/k/v
    /// (`[rows, n_head * d]`, head `h` at columns `h * d .. h * d + d`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn attention(
        q: &[f32],
        k: &[f32],
        v: &[f32],
        n_q: usize,
        n_kv: usize,
        n_head: usize,
        d: usize,
        scale: f32,
    ) -> Result<Vec<f32>, String> {
        let hidden = n_head * d;
        let qd = gpu_upload(q, n_q, hidden)?;
        let kd = gpu_upload(k, n_kv, hidden)?;
        let vd = gpu_upload(v, n_kv, hidden)?;
        let out = gpu_attention_packed_cross_f32(&qd, &kd, &vd, n_head, scale)?;
        gpu_download(&out)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn attention_cross_cached(
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
        let hidden = n_head * d;
        let k_key = ResidentKey::of_f32(k_cross, n_kv, hidden);
        let qd = gpu_upload(q, n_q, hidden)?;
        CROSS_KV.with(|cache| {
            let mut cache = cache.borrow_mut();
            let stale = cache
                .get(&layer)
                .map(|entry| entry.k_key != k_key)
                .unwrap_or(true);
            if stale {
                cache.insert(
                    layer,
                    CrossEntry {
                        k_key,
                        k: gpu_upload(k_cross, n_kv, hidden)?,
                        v: gpu_upload(v_cross, n_kv, hidden)?,
                    },
                );
            }
            let entry = cache
                .get(&layer)
                .ok_or_else(|| "cross kv entry was just inserted".to_string())?;
            let out =
                gpu_attention_packed_cross_f32(&qd, &entry.k, &entry.v, n_head, scale)?;
            gpu_download(&out)
        })
    }

    pub(super) fn clear_cross_kv_cache() {
        CROSS_KV.with(|cache| cache.borrow_mut().clear());
    }

    pub(super) fn binary(a: &[f32], b: &[f32], is_mul: bool) -> Result<Vec<f32>, String> {
        let ad = gpu_upload(a, 1, a.len())?;
        let bd = gpu_upload(b, 1, b.len())?;
        let out = if is_mul {
            gpu_mul(&ad, &bd)?
        } else {
            gpu_add(&ad, &bd)?
        };
        gpu_download(&out)
    }

    pub(super) fn gelu(a: &[f32]) -> Result<Vec<f32>, String> {
        let ad = gpu_upload(a, 1, a.len())?;
        let out = gpu_gelu(&ad)?;
        gpu_download(&out)
    }

    pub(super) fn layer_norm(
        x: &[f32],
        rows: usize,
        cols: usize,
        mul: &[f32],
        add: &[f32],
        eps: f32,
    ) -> Result<Vec<f32>, String> {
        let xd = gpu_upload(x, rows, cols)?;
        let out = gpu_layer_norm_mul_add(&xd, mul, add, eps)?;
        gpu_download(&out)
    }
}

#[cfg(not(all(
    any(target_os = "linux", target_os = "windows"),
    makepad_ai_cuda_kernels
)))]
mod imp {
    pub(super) fn device_available() -> bool {
        false
    }

    pub(super) fn matmul_nt(
        _a: &[f32],
        _weight: super::WeightSrc<'_>,
        _m: usize,
        _k: usize,
        _n: usize,
        _bias: Option<&[f32]>,
    ) -> Result<Vec<f32>, String> {
        Err("CUDA kernels are not compiled into this build".to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn attention(
        _q: &[f32],
        _k: &[f32],
        _v: &[f32],
        _n_q: usize,
        _n_kv: usize,
        _n_head: usize,
        _d: usize,
        _scale: f32,
    ) -> Result<Vec<f32>, String> {
        Err("CUDA kernels are not compiled into this build".to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn attention_cross_cached(
        _layer: usize,
        _q: &[f32],
        _k_cross: &[f32],
        _v_cross: &[f32],
        _n_q: usize,
        _n_kv: usize,
        _n_head: usize,
        _d: usize,
        _scale: f32,
    ) -> Result<Vec<f32>, String> {
        Err("CUDA kernels are not compiled into this build".to_string())
    }

    pub(super) fn clear_cross_kv_cache() {}

    pub(super) fn binary(_a: &[f32], _b: &[f32], _is_mul: bool) -> Result<Vec<f32>, String> {
        Err("CUDA kernels are not compiled into this build".to_string())
    }

    pub(super) fn gelu(_a: &[f32]) -> Result<Vec<f32>, String> {
        Err("CUDA kernels are not compiled into this build".to_string())
    }

    pub(super) fn layer_norm(
        _x: &[f32],
        _rows: usize,
        _cols: usize,
        _mul: &[f32],
        _add: &[f32],
        _eps: f32,
    ) -> Result<Vec<f32>, String> {
        Err("CUDA kernels are not compiled into this build".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::whisper::tensor::Tensor;

    /// `im2col_1d` + a plain NT matmul must reproduce `Tensor::conv1d` exactly.
    /// This is the contract the CUDA conv path relies on, and it is checkable
    /// without a GPU.
    #[test]
    fn im2col_matches_conv1d() {
        for &(ic, iw, kw, stride, ch_out) in &[
            (3usize, 11usize, 3usize, 1usize, 4usize),
            (5, 16, 3, 2, 7),
            (2, 9, 5, 1, 3),
        ] {
            let pad = kw / 2;
            let out_len = (iw + 2 * pad - kw) / stride + 1;

            let input = Tensor {
                data: (0..ic * iw).map(|i| (i as f32) * 0.017 - 0.4).collect(),
                shape: vec![ic, iw],
            };
            let weight = Tensor {
                data: (0..ch_out * ic * kw)
                    .map(|i| ((i % 13) as f32) * 0.031 - 0.2)
                    .collect(),
                shape: vec![ch_out, ic, kw],
            };
            let bias = Tensor {
                data: (0..ch_out).map(|i| (i as f32) * 0.5).collect(),
                shape: vec![1, ch_out],
            };

            let reference = Tensor::conv1d(&input, &weight, &bias, stride);

            let cols = im2col_1d(&input.data, ic, iw, kw, stride, pad).expect("im2col");
            assert_eq!(cols.len(), out_len * ic * kw);
            let k = ic * kw;
            for co in 0..ch_out {
                for t in 0..out_len {
                    let mut sum = bias.data[co];
                    for i in 0..k {
                        sum += cols[t * k + i] * weight.data[co * k + i];
                    }
                    let got = reference.data[co * out_len + t];
                    assert!(
                        (sum - got).abs() < 1e-4,
                        "ic={ic} iw={iw} kw={kw} s={stride} co={co} t={t}: {sum} vs {got}"
                    );
                }
            }
        }
    }

    #[test]
    fn dequant_f32_and_f16_roundtrip() {
        use crate::whisper::quant::{f32_to_f16, GGML_TYPE_F16, GGML_TYPE_F32};
        let values: Vec<f32> = (0..64).map(|i| (i as f32) * 0.25 - 8.0).collect();

        let mut f32_bytes = Vec::new();
        for v in &values {
            f32_bytes.extend_from_slice(&v.to_le_bytes());
        }
        let got = dequant_to_f32(&f32_bytes, GGML_TYPE_F32, values.len()).expect("f32");
        assert_eq!(got, values);

        let mut f16_bytes = Vec::new();
        for v in &values {
            f16_bytes.extend_from_slice(&f32_to_f16(*v).to_le_bytes());
        }
        let got = dequant_to_f32(&f16_bytes, GGML_TYPE_F16, values.len()).expect("f16");
        for (a, b) in got.iter().zip(&values) {
            assert!((a - b).abs() < 0.01, "{a} vs {b}");
        }
    }

    /// The dequantized blob must match `RawTensor::to_f32` element for element,
    /// because the CPU fallback and the device path must see the same weights.
    #[test]
    fn dequant_matches_raw_tensor_to_f32() {
        use crate::whisper::quant::{quantize_f32_to_q8_0, GGML_TYPE_Q8_0};
        use crate::whisper::tensor::RawTensor;
        let values: Vec<f32> = (0..256).map(|i| ((i * 37 % 91) as f32) * 0.13 - 5.0).collect();
        let q8 = quantize_f32_to_q8_0(&values);

        let raw = RawTensor {
            data: q8.clone(),
            shape: vec![32, 8],
            ggml_type: GGML_TYPE_Q8_0,
        };
        let reference = raw.to_f32();
        let got = dequant_to_f32(&q8, GGML_TYPE_Q8_0, values.len()).expect("q8_0");
        assert_eq!(got, reference.data);
    }

    #[test]
    fn fingerprint_separates_same_length_blobs() {
        let a = vec![7u8; 4096];
        let mut b = a.clone();
        assert_eq!(fingerprint(&a), fingerprint(&b));
        *b.last_mut().unwrap() = 9;
        assert_ne!(fingerprint(&a), fingerprint(&b));
        b = a.clone();
        b[0] = 9;
        assert_ne!(fingerprint(&a), fingerprint(&b));

        let x = vec![0.5f32; 1024];
        let mut y = x.clone();
        assert_eq!(fingerprint_f32(&x), fingerprint_f32(&y));
        *y.last_mut().unwrap() = 0.25;
        assert_ne!(fingerprint_f32(&x), fingerprint_f32(&y));
        y = x.clone();
        y[0] = 0.25;
        assert_ne!(fingerprint_f32(&x), fingerprint_f32(&y));
    }

    /// Without CUDA compiled in, every entry point must be a clean `None` (the
    /// CPU path), never a zero-filled buffer.
    #[test]
    fn degrades_to_none_without_cuda() {
        if is_available() {
            return;
        }
        assert!(try_matmul_nt_f32(&[1.0; 512], &[1.0; 512 * 512], 1, 512, 512).is_none());
        assert!(try_im2col_1d_f32(&[0.0; 30], 3, 10, 3, 1, 1).is_none());
        assert!(try_gelu_f32(&[1.0, 2.0], &[2]).is_none());
        assert!(try_flash_attn_f32_packed(
            &[0.0; 64],
            &[0.0; 64],
            &[0.0; 64],
            1,
            1,
            1,
            64,
            0.125
        )
        .is_none());
    }
}
