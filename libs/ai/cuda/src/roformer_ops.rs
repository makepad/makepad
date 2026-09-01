//! RoFormer (BS-RoFormer / stems) op surface — the two kernels the LLM
//! `mkllm_*` set structurally cannot serve.
//!
//! * [`roformer_attn_f32`] — batched, maskless, non-causal f32 attention.
//!   The `fattn_*` / `flash_decode` family is decode-shaped: f16 KV, a
//!   required mask, and no 4th dimension. BS-RoFormer's axial attention is
//!   f32, unmasked, head_dim 64, and batched over 62 bands or 1101 frames.
//! * [`roformer_rope_normal_f32`] — `GGML_ROPE_TYPE_NORMAL`, i.e. rotation of
//!   interleaved adjacent pairs. `mkllm_rope_multi` implements the NeoX
//!   split-half convention instead; using it here would be a silent quality
//!   collapse rather than an error.
//!
//! Kept in its own module (and its own `kernels/roformer.cu`) so the stems
//! lane neither waits on nor conflicts with in-flight LLM kernel work.

#[cfg(any(target_os = "linux", target_os = "windows"))]
use super::llm_ops::{CudaError, Stream};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::ffi::c_void;

/// Stable op names a Metal backend would implement next to these.
pub mod names {
    pub const ROFORMER_ATTN_F32: &str = "roformer_attn_f32";
    pub const ROFORMER_ATTN_F32_TILED: &str = "roformer_attn_f32_tiled";
    pub const ROFORMER_ROPE_NORMAL_F32: &str = "roformer_rope_normal_f32";
}

/// Shortest key axis for which [`roformer_attn_f32_tiled`] is the better of
/// the two kernels.
///
/// The tiled kernel amortises a 64-query tile and a 45 KB shared footprint
/// over the key loop, which only pays once the key axis is long; below this
/// the stems-shaped kernel — one query row per thread, a third of the shared
/// memory, three resident blocks — wins. BS-RoFormer's axial attention tops
/// out around a thousand keys and therefore never crosses this line, so the
/// stems parity gate keeps running on exactly the kernel it was measured on.
pub const ATTN_TILED_MIN_KEYS: i64 = 2048;

/// Head dims [`roformer_attn_f32`] is compiled and verified for. Anything else
/// is rejected by the kernel (and should be rejected earlier, at planning
/// time, so the failure names the tensor).
pub const ATTN_HEAD_DIMS: &[i64] = &[32, 64, 72];

pub fn attn_head_dim_supported(head_dim: i64) -> bool {
    ATTN_HEAD_DIMS.contains(&head_dim)
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod ffi {
    use super::{CudaError, Stream};
    use std::ffi::c_void;

    // `cuda_ffi!` (see `link_gate.rs`): the real kernel declarations when the
    // .cu build ran, link-clean stubs when it did not.
    cuda_ffi! {
        pub fn makepad_cuda_roformer_attn_f32(
            q: *const c_void,
            k: *const c_void,
            v: *const c_void,
            dst: *mut c_void,
            d: i32,
            n_q: i32,
            kc: i32,
            heads: i32,
            kv_heads: i32,
            batch: i32,
            scale: f32,
            q_nb1: usize,
            q_nb2: usize,
            q_nb3: usize,
            k_nb1: usize,
            k_nb2: usize,
            k_nb3: usize,
            v_nb1: usize,
            v_nb2: usize,
            v_nb3: usize,
            d_nb1: usize,
            d_nb2: usize,
            d_nb3: usize,
            stream: Stream,
        ) -> CudaError;

        pub fn makepad_cuda_roformer_attn_f32_tiled(
            q: *const c_void,
            k: *const c_void,
            v: *const c_void,
            dst: *mut c_void,
            d: i32,
            n_q: i32,
            kc: i32,
            heads: i32,
            kv_heads: i32,
            batch: i32,
            scale: f32,
            q_nb1: usize,
            q_nb2: usize,
            q_nb3: usize,
            k_nb1: usize,
            k_nb2: usize,
            k_nb3: usize,
            v_nb1: usize,
            v_nb2: usize,
            v_nb3: usize,
            d_nb1: usize,
            d_nb2: usize,
            d_nb3: usize,
            stream: Stream,
        ) -> CudaError;

        pub fn makepad_cuda_roformer_rope_normal_f32(
            src: *const c_void,
            pos: *const i32,
            dst: *mut c_void,
            ne0: i32,
            ne1: i32,
            ne2: i32,
            ne3: i32,
            n_dims: i32,
            freq_base: f32,
            freq_scale: f32,
            s_nb0: usize,
            s_nb1: usize,
            s_nb2: usize,
            s_nb3: usize,
            d_nb0: usize,
            d_nb1: usize,
            d_nb2: usize,
            d_nb3: usize,
            stream: Stream,
        ) -> CudaError;
    }
}

/// Batched non-causal attention, f32 throughout.
///
/// Layout is ggml's `flash_attn_ext` contract verbatim — including the output's
/// transposed head/query axes:
/// `q [d, n_q, heads, batch]`, `k`/`v `[d, kc, kv_heads, batch]`,
/// `dst [d, heads, n_q, batch]`. All `*_nb*` strides are BYTES; dim 0 is
/// contiguous f32.
///
/// # Safety
/// All pointers must be device allocations valid for the described extents and
/// strides, and `stream` must be a live CUDA stream.
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[allow(clippy::too_many_arguments)]
#[inline]
pub unsafe fn roformer_attn_f32(
    q: *const c_void,
    k: *const c_void,
    v: *const c_void,
    dst: *mut c_void,
    d: i32,
    n_q: i32,
    kc: i32,
    heads: i32,
    kv_heads: i32,
    batch: i32,
    scale: f32,
    q_nb1: usize,
    q_nb2: usize,
    q_nb3: usize,
    k_nb1: usize,
    k_nb2: usize,
    k_nb3: usize,
    v_nb1: usize,
    v_nb2: usize,
    v_nb3: usize,
    d_nb1: usize,
    d_nb2: usize,
    d_nb3: usize,
    stream: Stream,
) -> CudaError {
    ffi::makepad_cuda_roformer_attn_f32(
        q, k, v, dst, d, n_q, kc, heads, kv_heads, batch, scale, q_nb1, q_nb2, q_nb3, k_nb1,
        k_nb2, k_nb3, v_nb1, v_nb2, v_nb3, d_nb1, d_nb2, d_nb3, stream,
    )
}

/// The same op as [`roformer_attn_f32`], register-tiled for a long key axis.
///
/// Identical contract — same layouts, same f32 arithmetic, same online
/// softmax over the same tile order, every accumulation in the same order —
/// and therefore bit-identical results, not merely close ones. It
/// differs only in how the work is cut up: 4x4 score patches and 4 x D/8
/// accumulator patches per thread instead of one query row each, which is
/// what the vision tower's 14k-24k key axis needs and what BS-RoFormer's few
/// hundred does not. Pick it above [`ATTN_TILED_MIN_KEYS`] keys.
///
/// `d` must additionally be divisible by 8 (32, 64 and 72 are compiled).
///
/// # Safety
/// As [`roformer_attn_f32`].
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[allow(clippy::too_many_arguments)]
#[inline]
pub unsafe fn roformer_attn_f32_tiled(
    q: *const c_void,
    k: *const c_void,
    v: *const c_void,
    dst: *mut c_void,
    d: i32,
    n_q: i32,
    kc: i32,
    heads: i32,
    kv_heads: i32,
    batch: i32,
    scale: f32,
    q_nb1: usize,
    q_nb2: usize,
    q_nb3: usize,
    k_nb1: usize,
    k_nb2: usize,
    k_nb3: usize,
    v_nb1: usize,
    v_nb2: usize,
    v_nb3: usize,
    d_nb1: usize,
    d_nb2: usize,
    d_nb3: usize,
    stream: Stream,
) -> CudaError {
    ffi::makepad_cuda_roformer_attn_f32_tiled(
        q, k, v, dst, d, n_q, kc, heads, kv_heads, batch, scale, q_nb1, q_nb2, q_nb3, k_nb1,
        k_nb2, k_nb3, v_nb1, v_nb2, v_nb3, d_nb1, d_nb2, d_nb3, stream,
    )
}

/// RoPE with ggml's NORMAL (interleaved adjacent pairs) convention.
///
/// `pos` holds one i32 per `ne2` entry. Elements at or beyond `n_dims` pass
/// through unrotated. All `*_nb*` strides are BYTES.
///
/// # Safety
/// As [`roformer_attn_f32`].
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[allow(clippy::too_many_arguments)]
#[inline]
pub unsafe fn roformer_rope_normal_f32(
    src: *const c_void,
    pos: *const i32,
    dst: *mut c_void,
    ne0: i32,
    ne1: i32,
    ne2: i32,
    ne3: i32,
    n_dims: i32,
    freq_base: f32,
    freq_scale: f32,
    s_nb0: usize,
    s_nb1: usize,
    s_nb2: usize,
    s_nb3: usize,
    d_nb0: usize,
    d_nb1: usize,
    d_nb2: usize,
    d_nb3: usize,
    stream: Stream,
) -> CudaError {
    ffi::makepad_cuda_roformer_rope_normal_f32(
        src, pos, dst, ne0, ne1, ne2, ne3, n_dims, freq_base, freq_scale, s_nb0, s_nb1, s_nb2,
        s_nb3, d_nb0, d_nb1, d_nb2, d_nb3, stream,
    )
}
