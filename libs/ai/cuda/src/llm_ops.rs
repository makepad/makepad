//! Shared gen-AI op surface for llama (and a future Metal backend).
//!
//! These Rust names are the contract. CUDA forwards them to the `mkllm_*`
//! C ABI compiled once from makepad-ai-cuda's `kernels/llm/kernels.cu`
//! (libs/ai/cuda, absorbed lane T4, /aiarch.md §1 + §4). Metal should
//! later implement the same names with shaders — do not bury new launch
//! logic only in `makepad-llama`.
//!
//! C symbols stay `mkllm_*` this step. A later rename can be
//! `mkllm_*` → `makepad_cuda_*` without changing this Rust surface.

use std::ffi::c_void;

/// Stable op names a Metal backend should implement next to the CUDA kernels.
pub mod names {
    pub const DEVICE_INFO: &str = "device_info";
    pub const MMV_QUANT: &str = "mmv_quant";
    pub const MMV_QUANT_Q81: &str = "mmv_quant_q81";
    pub const MMV_QUANT_Q81_SWIGLU: &str = "mmv_quant_q81_swiglu";
    pub const MMQ_QUANT_Q81: &str = "mmq_quant_q81";
    pub const MMQ_KIND_J128: &str = "mmq_kind_j128";
    pub const QUANT_KIND_INFO: &str = "quant_kind_info";
    pub const MMQ_QUANT: &str = "mmq_quant";
    pub const MMVF_F32_M1: &str = "mmvf_f32_m1";
    pub const MMV_F32: &str = "mmv_f32";
    pub const QUANTIZE_Q81: &str = "quantize_q81";
    pub const QUANTIZE_Q81_BATCHED: &str = "quantize_q81_batched";
    pub const QUANTIZE_MMQ_DS4: &str = "quantize_mmq_ds4";
    pub const QUANTIZE_MMQ_D4: &str = "quantize_mmq_d4";
    pub const DEQUANT_ROWS_BF16: &str = "dequant_rows_bf16";
    pub const CAST_F32_BF16: &str = "cast_f32_bf16";
    pub const MUL_MAT_BATCHED: &str = "mul_mat_batched";
    pub const GET_ROWS_F32: &str = "get_rows_f32";
    pub const GET_ROWS_QUANT: &str = "get_rows_quant";
    pub const SET_ROWS: &str = "set_rows";
    pub const SOFTMAX_MASK: &str = "softmax_mask";
    pub const FLASH_DECODE: &str = "flash_decode";
    pub const FATTN_MMA_F16: &str = "fattn_mma_f16";
    pub const FATTN_VEC_F16: &str = "fattn_vec_f16";
    pub const NORM: &str = "norm";
    pub const RMS_NORM_MUL: &str = "rms_norm_mul";
    pub const ROPE_MULTI: &str = "rope_multi";
    pub const UNARY: &str = "unary";
    pub const UNARY_MUL: &str = "unary_mul";
    pub const GLU: &str = "glu";
    pub const BINARY: &str = "binary";
    pub const COPY_STRIDED: &str = "copy_strided";
    pub const SSM_CONV: &str = "ssm_conv";
    pub const GATED_DELTA_NET: &str = "gated_delta_net";
}

/// GGUF block kind passed to quantized MMV/MMQ launchers.
///
/// 0..=3 are the "legacy" kinds the hand-written kernels in `kernels.cu`
/// template over. 4..=7 are served only by the vendored official llama.cpp
/// templates (`fattn/mmq.cuh`, `fattn/mmvq.cuh`) plus the row dequant in
/// `iq_convert.cuh`; they exist because unsloth's Dynamic (UD-) GGUFs mix
/// these tensor types into otherwise-K-quant files. Must stay in sync with
/// `MKLLM_QUANT_*` in `kernels/llm/kernels.cu`.
pub const QUANT_Q4K: i32 = 0;
pub const QUANT_Q5K: i32 = 1;
pub const QUANT_Q6K: i32 = 2;
pub const QUANT_Q80: i32 = 3;
pub const QUANT_Q3K: i32 = 4;
pub const QUANT_IQ4XS: i32 = 5;
pub const QUANT_IQ4NL: i32 = 6;
pub const QUANT_IQ3S: i32 = 7;
pub const QUANT_COUNT: i32 = 8;

/// Kinds whose weights the vendored official MMQ/MMVQ templates serve but the
/// hand-written legacy kernels (`mmv_quant`, `mmq_quant`, `get_rows_quant`'s
/// per-element selector) do not.
pub const fn quant_kind_is_official_only(kind: i32) -> bool {
    kind > QUANT_Q80 && kind < QUANT_COUNT
}

/// Bits returned by `quant_kind_routes`: which quantized-matmul routes a kind
/// is verified on. Anything not set falls back to dequant-slab + cuBLAS, which
/// every kind supports.
pub const ROUTE_MMVQ: i32 = 1;
pub const ROUTE_MMQ: i32 = 2;

pub type Stream = *mut c_void;
pub type CudaError = i32;

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod ffi {
    use super::{CudaError, Stream};
    use std::ffi::c_void;

    #[allow(dead_code)]
    unsafe extern "C" {
        pub fn mkllm_device_info(
            device: i32,
            name: *mut u8,
            name_cap: i32,
            cc_major: *mut i32,
            cc_minor: *mut i32,
            total_mem: *mut usize,
            sm_count: *mut i32,
        ) -> CudaError;
        pub fn mkllm_mmv_quant(
            kind: i32,
            src0: *const c_void,
            src1: *const f32,
            dst: *mut f32,
            k: i32,
            n: i32,
            m: i32,
            src0_row_bytes: usize,
            src1_col_elems: usize,
            dst_col_elems: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_quantize_q81(
            x: *const f32,
            y: *mut c_void,
            k: i32,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_quantize_q81_batched(
            x: *const f32,
            q: *mut i8,
            d: *mut f32,
            k: i32,
            m: i32,
            col_stride: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_mmv_quant_q81(
            kind: i32,
            src0: *const c_void,
            y: *const c_void,
            dst: *mut f32,
            k: i32,
            n: i32,
            m: i32,
            src0_row_bytes: usize,
            dst_col_elems: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_mmv_quant_q81_swiglu(
            kind: i32,
            up: *const c_void,
            gate: *const c_void,
            y: *const c_void,
            dst: *mut f32,
            k: i32,
            n: i32,
            m: i32,
            up_row_bytes: usize,
            gate_row_bytes: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_mmq_quant_q81(
            kind: i32,
            src0: *const c_void,
            q8: *const i8,
            d8: *const f32,
            dst: *mut f32,
            k: i32,
            n: i32,
            m: i32,
            src0_row_bytes: usize,
            dst_col_elems: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_mmv_f32(
            src0: *const f32,
            src1: *const f32,
            dst: *mut f32,
            k: i32,
            n: i32,
            m: i32,
            src0_row_elems: usize,
            src1_col_elems: usize,
            dst_col_elems: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_dequant_rows_bf16(
            kind: i32,
            src: *const c_void,
            dst: *mut c_void,
            rows: i32,
            k: i32,
            src_row_bytes: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_quantize_mmq_ds4(
            x: *const f32,
            y: *mut c_void,
            k: i32,
            m: i32,
            stride_col: i32,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_mmq_kind_j128(
            kind: i32,
            x: *const c_void,
            y: *const c_void,
            dst: *mut f32,
            k: i32,
            n: i32,
            m: i32,
            stride_row_x: i32,
            stride_col_dst: i32,
            nsm: i32,
            tmp_fixup: *mut f32,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_quant_kind_routes(kind: i32) -> i32;
        pub fn mkllm_quant_kind_block_bytes(kind: i32) -> i32;
        pub fn mkllm_quant_kind_bytes_per_256(kind: i32) -> i32;
        pub fn mkllm_quant_kind_mmq_ds4(kind: i32) -> i32;
        pub fn mkllm_quantize_mmq_d4(
            x: *const f32,
            y: *mut c_void,
            k: i32,
            m: i32,
            stride_col: i32,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_mmq_quant(
            kind: i32,
            src0: *const c_void,
            src1: *const f32,
            dst: *mut f32,
            k: i32,
            n: i32,
            m: i32,
            src0_row_bytes: usize,
            src1_col_elems: usize,
            dst_col_elems: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_cast_f32_bf16(
            src: *const c_void,
            dst: *mut c_void,
            k: i32,
            m: i32,
            src_nb0: usize,
            src_nb1: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_mul_mat_batched(
            a_is_f16: i32,
            a: *const c_void,
            b: *const c_void,
            dst: *mut c_void,
            k: i32,
            n: i32,
            m: i32,
            ne2: i32,
            ne3: i32,
            a_ne2: i32,
            a_ne3: i32,
            a_nb0: usize,
            a_nb1: usize,
            a_nb2: usize,
            a_nb3: usize,
            b_nb0: usize,
            b_nb1: usize,
            b_nb2: usize,
            b_nb3: usize,
            d_nb0: usize,
            d_nb1: usize,
            d_nb2: usize,
            d_nb3: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_get_rows_f32(
            src: *const c_void,
            rows: *const i32,
            dst: *mut c_void,
            ne0: i32,
            nrows: i32,
            src_nb1: usize,
            dst_nb1: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_get_rows_quant(
            kind: i32,
            src: *const c_void,
            rows: *const i32,
            dst: *mut c_void,
            ne0: i32,
            nrows: i32,
            src_nb1: usize,
            dst_nb1: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_set_rows(
            dst_is_f16: i32,
            src: *const c_void,
            rows: *const i32,
            dst: *mut c_void,
            ne0: i32,
            nrows: i32,
            src_nb1: usize,
            dst_nb1: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_softmax_mask(
            x: *const c_void,
            mask: *const c_void,
            dst: *mut c_void,
            ncols: i32,
            ne1: i32,
            ne2: i32,
            scale: f32,
            x_nb1: usize,
            x_nb2: usize,
            mask_nb1: usize,
            d_nb1: usize,
            d_nb2: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_flash_decode(
            q: *const c_void,
            k: *const c_void,
            v: *const c_void,
            mask: *const c_void,
            dst: *mut c_void,
            d: i32,
            dv: i32,
            kc: i32,
            n_q: i32,
            h: i32,
            hkv: i32,
            scale: f32,
            q_nb1: usize,
            q_nb2: usize,
            k_nb1: usize,
            k_nb2: usize,
            v_nb1: usize,
            v_nb2: usize,
            m_nb1: usize,
            d_nb1: usize,
            d_nb2: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_fattn_mma_f16(
            q: *const c_void,
            k: *const c_void,
            v: *const c_void,
            mask: *const c_void,
            dst: *mut c_void,
            d: i32,
            dv: i32,
            kc: i32,
            n_q: i32,
            h: i32,
            hkv: i32,
            scale: f32,
            q_nb1: usize,
            q_nb2: usize,
            k_nb1: usize,
            k_nb2: usize,
            v_nb1: usize,
            v_nb2: usize,
            m_nb1: usize,
            d_nb1: usize,
            d_nb2: usize,
            nsm: i32,
            cc: i32,
            tmp_fixup: *mut f32,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_fattn_mma_fixup_bytes(nsm: i32) -> usize;
        pub fn mkllm_fattn_vec_tmp_bytes(n_q: i32, h: i32, kc: i32) -> usize;
        pub fn mkllm_fattn_vec_f16(
            q: *const c_void,
            k: *const c_void,
            v: *const c_void,
            mask: *const c_void,
            dst: *mut c_void,
            d: i32,
            dv: i32,
            kc: i32,
            n_q: i32,
            h: i32,
            hkv: i32,
            scale: f32,
            q_nb1: usize,
            q_nb2: usize,
            k_nb1: usize,
            k_nb2: usize,
            v_nb1: usize,
            v_nb2: usize,
            m_nb1: usize,
            d_nb1: usize,
            d_nb2: usize,
            nsm: i32,
            tmp: *mut f32,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_norm(
            l2: i32,
            x: *const c_void,
            dst: *mut c_void,
            ne0: i32,
            ne1: i32,
            ne2: i32,
            ne3: i32,
            eps: f32,
            x_nb1: usize,
            x_nb2: usize,
            x_nb3: usize,
            d_nb1: usize,
            d_nb2: usize,
            d_nb3: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_rms_norm_mul(
            x: *const c_void,
            mul: *const c_void,
            add: *const c_void,
            dst: *mut c_void,
            ncols: i32,
            nrows: i32,
            nchannels: i32,
            nsamples: i32,
            eps: f32,
            x_nb1: usize,
            x_nb2: usize,
            x_nb3: usize,
            d_nb1: usize,
            d_nb2: usize,
            d_nb3: usize,
            mul_nb1: usize,
            mul_nb2: usize,
            mul_nb3: usize,
            mul_ne0: i32,
            mul_ne1: i32,
            mul_ne2: i32,
            mul_ne3: i32,
            add_nb1: usize,
            add_nb2: usize,
            add_nb3: usize,
            add_ne0: i32,
            add_ne1: i32,
            add_ne2: i32,
            add_ne3: i32,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_rope_multi(
            src: *const c_void,
            pos: *const i32,
            dst: *mut c_void,
            ne0: i32,
            ne1: i32,
            ne2: i32,
            ne3: i32,
            is_imrope: i32,
            n_dims: i32,
            sect_0: i32,
            sect_1: i32,
            sect_2: i32,
            sect_3: i32,
            freq_base: f32,
            freq_scale: f32,
            ext_factor: f32,
            attn_factor: f32,
            corr_dim0: f32,
            corr_dim1: f32,
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
        pub fn mkllm_unary(
            x: *const c_void,
            dst: *mut c_void,
            n: usize,
            op: i32,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_unary_mul(
            x: *const c_void,
            g: *const c_void,
            dst: *mut c_void,
            k: usize,
            n: i32,
            op: i32,
            o0: usize,
            o1: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_glu(
            a: *const c_void,
            b: *const c_void,
            dst: *mut c_void,
            n: usize,
            op: i32,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_binary(
            op: i32,
            a: *const c_void,
            b: *const c_void,
            dst: *mut c_void,
            ne0: i32,
            ne1: i32,
            ne2: i32,
            ne3: i32,
            b_ne0: i32,
            b_ne1: i32,
            b_ne2: i32,
            b_ne3: i32,
            a_nb0: usize,
            a_nb1: usize,
            a_nb2: usize,
            a_nb3: usize,
            b_nb0: usize,
            b_nb1: usize,
            b_nb2: usize,
            b_nb3: usize,
            d_nb0: usize,
            d_nb1: usize,
            d_nb2: usize,
            d_nb3: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_copy_strided(
            elem_size: i32,
            src: *const c_void,
            dst: *mut c_void,
            s_ne0: i32,
            s_ne1: i32,
            s_ne2: i32,
            s_ne3: i32,
            d_ne0: i32,
            d_ne1: i32,
            d_ne2: i32,
            d_ne3: i32,
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
        pub fn mkllm_ssm_conv(
            src0: *const c_void,
            weight: *const c_void,
            dst: *mut c_void,
            d_conv: i32,
            d_inner: i32,
            n_tokens: i32,
            n_seqs: i32,
            apply_silu: i32,
            s_nb0: usize,
            s_nb1: usize,
            s_nb2: usize,
            w_nb1: usize,
            d_nb0: usize,
            d_nb1: usize,
            d_nb2: usize,
            stream: Stream,
        ) -> CudaError;
        pub fn mkllm_gated_delta_net(
            q: *const c_void,
            k: *const c_void,
            v: *const c_void,
            g: *const c_void,
            b: *const c_void,
            s: *const c_void,
            dst: *mut c_void,
            sv: i32,
            h: i32,
            n_tokens: i32,
            n_seqs: i32,
            gate_g: i32,
            q_heads: i32,
            k_heads: i32,
            q_h_elems: usize,
            q_t_elems: usize,
            q_s_elems: usize,
            k_h_elems: usize,
            k_t_elems: usize,
            k_s_elems: usize,
            v_h_elems: usize,
            v_t_elems: usize,
            v_s_elems: usize,
            state_checkpoints: i32,
            stream: Stream,
        ) -> CudaError;
    }
}

macro_rules! cuda_forward {
    ($name:ident ( $($arg:ident : $ty:ty),* $(,)? ) -> $ret:ty { $ffi:ident }) => {
        #[doc = concat!("`", stringify!($name), "` — CUDA forwards to `", stringify!($ffi), "`.")]
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        #[inline]
        pub unsafe fn $name($($arg: $ty),*) -> $ret {
            ffi::$ffi($($arg),*)
        }
    };
}

cuda_forward!(device_info(
    device: i32,
    name: *mut u8,
    name_cap: i32,
    cc_major: *mut i32,
    cc_minor: *mut i32,
    total_mem: *mut usize,
    sm_count: *mut i32,
) -> CudaError { mkllm_device_info });

cuda_forward!(mmv_quant(
    kind: i32,
    src0: *const c_void,
    src1: *const f32,
    dst: *mut f32,
    k: i32,
    n: i32,
    m: i32,
    src0_row_bytes: usize,
    src1_col_elems: usize,
    dst_col_elems: usize,
    stream: Stream,
) -> CudaError { mkllm_mmv_quant });

cuda_forward!(quantize_q81(
    x: *const f32,
    y: *mut c_void,
    k: i32,
    stream: Stream,
) -> CudaError { mkllm_quantize_q81 });

cuda_forward!(quantize_q81_batched(
    x: *const f32,
    q: *mut i8,
    d: *mut f32,
    k: i32,
    m: i32,
    col_stride: usize,
    stream: Stream,
) -> CudaError { mkllm_quantize_q81_batched });

cuda_forward!(mmv_quant_q81(
    kind: i32,
    src0: *const c_void,
    y: *const c_void,
    dst: *mut f32,
    k: i32,
    n: i32,
    m: i32,
    src0_row_bytes: usize,
    dst_col_elems: usize,
    stream: Stream,
) -> CudaError { mkllm_mmv_quant_q81 });

cuda_forward!(mmv_quant_q81_swiglu(
    kind: i32,
    up: *const c_void,
    gate: *const c_void,
    y: *const c_void,
    dst: *mut f32,
    k: i32,
    n: i32,
    m: i32,
    up_row_bytes: usize,
    gate_row_bytes: usize,
    stream: Stream,
) -> CudaError { mkllm_mmv_quant_q81_swiglu });

cuda_forward!(mmq_quant_q81(
    kind: i32,
    src0: *const c_void,
    q8: *const i8,
    d8: *const f32,
    dst: *mut f32,
    k: i32,
    n: i32,
    m: i32,
    src0_row_bytes: usize,
    dst_col_elems: usize,
    stream: Stream,
) -> CudaError { mkllm_mmq_quant_q81 });

cuda_forward!(mmvf_f32_m1(
    src0: *const f32,
    src1: *const f32,
    dst: *mut f32,
    k: i32,
    n: i32,
    m: i32,
    src0_row_elems: usize,
    src1_col_elems: usize,
    dst_col_elems: usize,
    stream: Stream,
) -> CudaError { mkllm_mmv_f32 });

cuda_forward!(mmv_f32(
    src0: *const f32,
    src1: *const f32,
    dst: *mut f32,
    k: i32,
    n: i32,
    m: i32,
    src0_row_elems: usize,
    src1_col_elems: usize,
    dst_col_elems: usize,
    stream: Stream,
) -> CudaError { mkllm_mmv_f32 });

cuda_forward!(dequant_rows_bf16(
    kind: i32,
    src: *const c_void,
    dst: *mut c_void,
    rows: i32,
    k: i32,
    src_row_bytes: usize,
    stream: Stream,
) -> CudaError { mkllm_dequant_rows_bf16 });

cuda_forward!(quantize_mmq_ds4(
    x: *const f32,
    y: *mut c_void,
    k: i32,
    m: i32,
    stride_col: i32,
    stream: Stream,
) -> CudaError { mkllm_quantize_mmq_ds4 });

cuda_forward!(quant_kind_routes(kind: i32) -> i32 { mkllm_quant_kind_routes });
cuda_forward!(quant_kind_block_bytes(kind: i32) -> i32 { mkllm_quant_kind_block_bytes });
cuda_forward!(quant_kind_bytes_per_256(kind: i32) -> i32 { mkllm_quant_kind_bytes_per_256 });
cuda_forward!(quant_kind_mmq_ds4(kind: i32) -> i32 { mkllm_quant_kind_mmq_ds4 });

cuda_forward!(mmq_kind_j128(
    kind: i32,
    x: *const c_void,
    y: *const c_void,
    dst: *mut f32,
    k: i32,
    n: i32,
    m: i32,
    stride_row_x: i32,
    stride_col_dst: i32,
    nsm: i32,
    tmp_fixup: *mut f32,
    stream: Stream,
) -> CudaError { mkllm_mmq_kind_j128 });

cuda_forward!(quantize_mmq_d4(
    x: *const f32,
    y: *mut c_void,
    k: i32,
    m: i32,
    stride_col: i32,
    stream: Stream,
) -> CudaError { mkllm_quantize_mmq_d4 });

cuda_forward!(mmq_quant(
    kind: i32,
    src0: *const c_void,
    src1: *const f32,
    dst: *mut f32,
    k: i32,
    n: i32,
    m: i32,
    src0_row_bytes: usize,
    src1_col_elems: usize,
    dst_col_elems: usize,
    stream: Stream,
) -> CudaError { mkllm_mmq_quant });

cuda_forward!(cast_f32_bf16(
    src: *const c_void,
    dst: *mut c_void,
    k: i32,
    m: i32,
    src_nb0: usize,
    src_nb1: usize,
    stream: Stream,
) -> CudaError { mkllm_cast_f32_bf16 });

cuda_forward!(mul_mat_batched(
    a_is_f16: i32,
    a: *const c_void,
    b: *const c_void,
    dst: *mut c_void,
    k: i32,
    n: i32,
    m: i32,
    ne2: i32,
    ne3: i32,
    a_ne2: i32,
    a_ne3: i32,
    a_nb0: usize,
    a_nb1: usize,
    a_nb2: usize,
    a_nb3: usize,
    b_nb0: usize,
    b_nb1: usize,
    b_nb2: usize,
    b_nb3: usize,
    d_nb0: usize,
    d_nb1: usize,
    d_nb2: usize,
    d_nb3: usize,
    stream: Stream,
) -> CudaError { mkllm_mul_mat_batched });

cuda_forward!(get_rows_f32(
    src: *const c_void,
    rows: *const i32,
    dst: *mut c_void,
    ne0: i32,
    nrows: i32,
    src_nb1: usize,
    dst_nb1: usize,
    stream: Stream,
) -> CudaError { mkllm_get_rows_f32 });

cuda_forward!(get_rows_quant(
    kind: i32,
    src: *const c_void,
    rows: *const i32,
    dst: *mut c_void,
    ne0: i32,
    nrows: i32,
    src_nb1: usize,
    dst_nb1: usize,
    stream: Stream,
) -> CudaError { mkllm_get_rows_quant });

cuda_forward!(set_rows(
    dst_is_f16: i32,
    src: *const c_void,
    rows: *const i32,
    dst: *mut c_void,
    ne0: i32,
    nrows: i32,
    src_nb1: usize,
    dst_nb1: usize,
    stream: Stream,
) -> CudaError { mkllm_set_rows });

cuda_forward!(softmax_mask(
    x: *const c_void,
    mask: *const c_void,
    dst: *mut c_void,
    ncols: i32,
    ne1: i32,
    ne2: i32,
    scale: f32,
    x_nb1: usize,
    x_nb2: usize,
    mask_nb1: usize,
    d_nb1: usize,
    d_nb2: usize,
    stream: Stream,
) -> CudaError { mkllm_softmax_mask });

cuda_forward!(flash_decode(
    q: *const c_void,
    k: *const c_void,
    v: *const c_void,
    mask: *const c_void,
    dst: *mut c_void,
    d: i32,
    dv: i32,
    kc: i32,
    n_q: i32,
    h: i32,
    hkv: i32,
    scale: f32,
    q_nb1: usize,
    q_nb2: usize,
    k_nb1: usize,
    k_nb2: usize,
    v_nb1: usize,
    v_nb2: usize,
    m_nb1: usize,
    d_nb1: usize,
    d_nb2: usize,
    stream: Stream,
) -> CudaError { mkllm_flash_decode });

cuda_forward!(fattn_mma_f16(
    q: *const c_void,
    k: *const c_void,
    v: *const c_void,
    mask: *const c_void,
    dst: *mut c_void,
    d: i32,
    dv: i32,
    kc: i32,
    n_q: i32,
    h: i32,
    hkv: i32,
    scale: f32,
    q_nb1: usize,
    q_nb2: usize,
    k_nb1: usize,
    k_nb2: usize,
    v_nb1: usize,
    v_nb2: usize,
    m_nb1: usize,
    d_nb1: usize,
    d_nb2: usize,
    nsm: i32,
    cc: i32,
    tmp_fixup: *mut f32,
    stream: Stream,
) -> CudaError { mkllm_fattn_mma_f16 });

cuda_forward!(fattn_vec_f16(
    q: *const c_void,
    k: *const c_void,
    v: *const c_void,
    mask: *const c_void,
    dst: *mut c_void,
    d: i32,
    dv: i32,
    kc: i32,
    n_q: i32,
    h: i32,
    hkv: i32,
    scale: f32,
    q_nb1: usize,
    q_nb2: usize,
    k_nb1: usize,
    k_nb2: usize,
    v_nb1: usize,
    v_nb2: usize,
    m_nb1: usize,
    d_nb1: usize,
    d_nb2: usize,
    nsm: i32,
    tmp: *mut f32,
    stream: Stream,
) -> CudaError { mkllm_fattn_vec_f16 });

cuda_forward!(norm(
    l2: i32,
    x: *const c_void,
    dst: *mut c_void,
    ne0: i32,
    ne1: i32,
    ne2: i32,
    ne3: i32,
    eps: f32,
    x_nb1: usize,
    x_nb2: usize,
    x_nb3: usize,
    d_nb1: usize,
    d_nb2: usize,
    d_nb3: usize,
    stream: Stream,
) -> CudaError { mkllm_norm });

cuda_forward!(rms_norm_mul(
    x: *const c_void,
    mul: *const c_void,
    add: *const c_void,
    dst: *mut c_void,
    ncols: i32,
    nrows: i32,
    nchannels: i32,
    nsamples: i32,
    eps: f32,
    x_nb1: usize,
    x_nb2: usize,
    x_nb3: usize,
    d_nb1: usize,
    d_nb2: usize,
    d_nb3: usize,
    mul_nb1: usize,
    mul_nb2: usize,
    mul_nb3: usize,
    mul_ne0: i32,
    mul_ne1: i32,
    mul_ne2: i32,
    mul_ne3: i32,
    add_nb1: usize,
    add_nb2: usize,
    add_nb3: usize,
    add_ne0: i32,
    add_ne1: i32,
    add_ne2: i32,
    add_ne3: i32,
    stream: Stream,
) -> CudaError { mkllm_rms_norm_mul });

cuda_forward!(rope_multi(
    src: *const c_void,
    pos: *const i32,
    dst: *mut c_void,
    ne0: i32,
    ne1: i32,
    ne2: i32,
    ne3: i32,
    is_imrope: i32,
    n_dims: i32,
    sect_0: i32,
    sect_1: i32,
    sect_2: i32,
    sect_3: i32,
    freq_base: f32,
    freq_scale: f32,
    ext_factor: f32,
    attn_factor: f32,
    corr_dim0: f32,
    corr_dim1: f32,
    s_nb0: usize,
    s_nb1: usize,
    s_nb2: usize,
    s_nb3: usize,
    d_nb0: usize,
    d_nb1: usize,
    d_nb2: usize,
    d_nb3: usize,
    stream: Stream,
) -> CudaError { mkllm_rope_multi });

cuda_forward!(unary(
    x: *const c_void,
    dst: *mut c_void,
    n: usize,
    op: i32,
    stream: Stream,
) -> CudaError { mkllm_unary });

cuda_forward!(unary_mul(
    x: *const c_void,
    g: *const c_void,
    dst: *mut c_void,
    k: usize,
    n: i32,
    op: i32,
    o0: usize,
    o1: usize,
    stream: Stream,
) -> CudaError { mkllm_unary_mul });

cuda_forward!(glu(
    a: *const c_void,
    b: *const c_void,
    dst: *mut c_void,
    n: usize,
    op: i32,
    stream: Stream,
) -> CudaError { mkllm_glu });

cuda_forward!(binary(
    op: i32,
    a: *const c_void,
    b: *const c_void,
    dst: *mut c_void,
    ne0: i32,
    ne1: i32,
    ne2: i32,
    ne3: i32,
    b_ne0: i32,
    b_ne1: i32,
    b_ne2: i32,
    b_ne3: i32,
    a_nb0: usize,
    a_nb1: usize,
    a_nb2: usize,
    a_nb3: usize,
    b_nb0: usize,
    b_nb1: usize,
    b_nb2: usize,
    b_nb3: usize,
    d_nb0: usize,
    d_nb1: usize,
    d_nb2: usize,
    d_nb3: usize,
    stream: Stream,
) -> CudaError { mkllm_binary });

cuda_forward!(copy_strided(
    elem_size: i32,
    src: *const c_void,
    dst: *mut c_void,
    s_ne0: i32,
    s_ne1: i32,
    s_ne2: i32,
    s_ne3: i32,
    d_ne0: i32,
    d_ne1: i32,
    d_ne2: i32,
    d_ne3: i32,
    s_nb0: usize,
    s_nb1: usize,
    s_nb2: usize,
    s_nb3: usize,
    d_nb0: usize,
    d_nb1: usize,
    d_nb2: usize,
    d_nb3: usize,
    stream: Stream,
) -> CudaError { mkllm_copy_strided });

cuda_forward!(ssm_conv(
    src0: *const c_void,
    weight: *const c_void,
    dst: *mut c_void,
    d_conv: i32,
    d_inner: i32,
    n_tokens: i32,
    n_seqs: i32,
    apply_silu: i32,
    s_nb0: usize,
    s_nb1: usize,
    s_nb2: usize,
    w_nb1: usize,
    d_nb0: usize,
    d_nb1: usize,
    d_nb2: usize,
    stream: Stream,
) -> CudaError { mkllm_ssm_conv });

cuda_forward!(gated_delta_net(
    q: *const c_void,
    k: *const c_void,
    v: *const c_void,
    g: *const c_void,
    b: *const c_void,
    s: *const c_void,
    dst: *mut c_void,
    sv: i32,
    h: i32,
    n_tokens: i32,
    n_seqs: i32,
    gate_g: i32,
    q_heads: i32,
    k_heads: i32,
    q_h_elems: usize,
    q_t_elems: usize,
    q_s_elems: usize,
    k_h_elems: usize,
    k_t_elems: usize,
    k_s_elems: usize,
    v_h_elems: usize,
    v_t_elems: usize,
    v_s_elems: usize,
    state_checkpoints: i32,
    stream: Stream,
) -> CudaError { mkllm_gated_delta_net });

/// Scratch bytes for the official fattn MMA stream-K fixup.
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[inline]
pub fn fattn_mma_fixup_bytes(nsm: i32) -> usize {
    unsafe { ffi::mkllm_fattn_mma_fixup_bytes(nsm) }
}

/// Scratch bytes for the official fattn-vec combine buffer.
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[inline]
pub fn fattn_vec_tmp_bytes(n_q: i32, h: i32, kc: i32) -> usize {
    unsafe { ffi::mkllm_fattn_vec_tmp_bytes(n_q, h, kc) }
}
