// SPDX-License-Identifier: MIT
// Copyright (c) 2023-2026 The ggml authors
//
// Row dequantization for the quant kinds unsloth's Dynamic (UD-) GGUFs mix
// into otherwise-K-quant files: Q3_K, IQ4_XS, IQ4_NL and IQ3_S. Ported from
// ggml's `ggml-cuda/convert.cu` (`dequantize_block_q3_K`,
// `dequantize_block_iq4_xs`, `dequantize_block_iq4_nl`,
// `dequantize_block_iq3_s`) — MIT, see libs/ai/NOTICE.
//
// Two changes vs upstream:
//   * upstream assumes one contiguous block array per tensor; the executor
//     hands us row-strided weights, so the block index is
//     (row, super-block) with an explicit `src_row_bytes`.
//   * upstream writes a dense destination; `get_rows` needs an indirected
//     source row and a byte-strided destination, so both are parameters and
//     a null `row_map` means "identity".
//
// Semantics contract: these must agree bit for bit (modulo the destination's
// f32->bf16 rounding) with the scalar reference in
// libs/ai/cuda/src/quant_iq.rs, which is itself pinned against llama.cpp's
// gguf-py dequantizers by `cpu_reference_matches_gguf_py_oracle`.
//
// Include AFTER fattn/mmvq.cuh so ggml-common.h's block structs and codebook
// tables (kvalues_iq4nl, iq3s_grid, kmask_iq2xs) are in scope.

#pragma once

// One 256-value unit of source bytes for a kind, as the executor addresses it.
// Mirrors mkllm_kind_bytes_per_256 on the host side.
static __device__ __forceinline__ int mkllm_iq_bytes_per_256(int kind) {
    switch (kind) {
        case MKLLM_QUANT_Q3K:   return (int) sizeof(block_q3_K);
        case MKLLM_QUANT_IQ4XS: return (int) sizeof(block_iq4_xs);
        case MKLLM_QUANT_IQ4NL: return 8 * (int) sizeof(block_iq4_nl);
        case MKLLM_QUANT_IQ3S:  return (int) sizeof(block_iq3_s);
        default:                return 0;
    }
}

// dst addressing shared by both entry points: `row_map` null = identity.
struct mkllm_iq_dst {
    char * base;
    size_t row_bytes;
};

#define MKLLM_IQ_ROW_SETUP(BLOCK_BYTES)                                        \
    const int out_row = (int) blockIdx.y;                                      \
    if (out_row >= rows) return;                                               \
    const int sb = (int) blockIdx.x;                                           \
    const int src_row = row_map ? (int) row_map[out_row] : out_row;            \
    const uint8_t * blk_bytes =                                                \
        src + (size_t) src_row * src_row_bytes + (size_t) sb * (BLOCK_BYTES);  \
    dst_t * y_row = (dst_t *) ((char *) dst + (size_t) out_row * dst_row_bytes) \
        + (size_t) sb * QK_K;

// --- Q3_K -------------------------------------------------------------- 64 threads
template <typename dst_t>
static __global__ void mkllm_dq_q3_k_kernel(
        const uint8_t * __restrict__ src, size_t src_row_bytes,
        const int32_t * __restrict__ row_map,
        void * __restrict__ dst, size_t dst_row_bytes, int rows) {
    MKLLM_IQ_ROW_SETUP(sizeof(block_q3_K))
    const block_q3_K * x = (const block_q3_K *) blk_bytes;

    const int r    = (int) threadIdx.x / 4;
    const int tid  = r / 2;
    const int is0  = r % 2;
    const int l0   = 16 * is0 + 4 * ((int) threadIdx.x % 4);
    const int n    = tid / 4;
    const int j    = tid - 4 * n;

    const uint8_t m = 1 << (4 * n + j);
    const int is    = 8 * n + 2 * j + is0;
    const int shift = 2 * j;

    const int8_t us = is <  4 ? (x->scales[is-0] & 0xF) | (((x->scales[is+8] >> 0) & 3) << 4) :
                      is <  8 ? (x->scales[is-0] & 0xF) | (((x->scales[is+4] >> 2) & 3) << 4) :
                      is < 12 ? (x->scales[is-8] >>  4) | (((x->scales[is+0] >> 4) & 3) << 4) :
                                (x->scales[is-8] >>  4) | (((x->scales[is-4] >> 6) & 3) << 4);
    const float dl = __half2float(x->d) * (float) (us - 32);

    dst_t * y = y_row + 128 * n + 32 * j;
    const uint8_t * q  = x->qs + 32 * n;
    const uint8_t * hm = x->hmask;
    for (int l = l0; l < l0 + 4; ++l) {
        y[l] = (dst_t) (dl * (float) ((int8_t) ((q[l] >> shift) & 3) - ((hm[l] & m) ? 0 : 4)));
    }
}

// --- IQ4_XS ------------------------------------------------------------ 32 threads
template <typename dst_t>
static __global__ void mkllm_dq_iq4_xs_kernel(
        const uint8_t * __restrict__ src, size_t src_row_bytes,
        const int32_t * __restrict__ row_map,
        void * __restrict__ dst, size_t dst_row_bytes, int rows) {
    MKLLM_IQ_ROW_SETUP(sizeof(block_iq4_xs))
    const block_iq4_xs * x = (const block_iq4_xs *) blk_bytes;

    const int tid = (int) threadIdx.x;
    const int il  = tid / 8; // 0..3
    const int ib  = tid % 8; // 0..7
    dst_t * y = y_row + 32 * ib + 4 * il;
    const uint8_t * q4 = x->qs + 16 * ib + 4 * il;
    const int ls = (int) (((x->scales_l[ib/2] >> 4*(ib%2)) & 0xf)
                        | (((x->scales_h >> 2*ib) & 3) << 4));
    const float d = __half2float(x->d) * (float) (ls - 32);
    for (int j = 0; j < 4; ++j) {
        y[j +  0] = (dst_t) (d * (float) kvalues_iq4nl[q4[j] & 0xf]);
        y[j + 16] = (dst_t) (d * (float) kvalues_iq4nl[q4[j] >>  4]);
    }
}

// --- IQ4_NL ------------------------------------------------------------ 32 threads
// 32-value blocks: eight of them make up one 256-value addressing unit.
template <typename dst_t>
static __global__ void mkllm_dq_iq4_nl_kernel(
        const uint8_t * __restrict__ src, size_t src_row_bytes,
        const int32_t * __restrict__ row_map,
        void * __restrict__ dst, size_t dst_row_bytes, int rows) {
    MKLLM_IQ_ROW_SETUP(8 * sizeof(block_iq4_nl))
    const block_iq4_nl * x = (const block_iq4_nl *) blk_bytes;

    const int tid = (int) threadIdx.x;
    const int il  = tid / 8; // 0..3
    const int ib  = tid % 8; // 0..7
    dst_t * y = y_row + 32 * ib + 4 * il;
    const uint8_t * q4 = x[ib].qs + 4 * il;
    const float d = __half2float(x[ib].d);
    for (int j = 0; j < 4; ++j) {
        y[j +  0] = (dst_t) (d * (float) kvalues_iq4nl[q4[j] & 0xf]);
        y[j + 16] = (dst_t) (d * (float) kvalues_iq4nl[q4[j] >>  4]);
    }
}

// --- IQ3_S ------------------------------------------------------------- 32 threads
template <typename dst_t>
static __global__ void mkllm_dq_iq3_s_kernel(
        const uint8_t * __restrict__ src, size_t src_row_bytes,
        const int32_t * __restrict__ row_map,
        void * __restrict__ dst, size_t dst_row_bytes, int rows) {
    MKLLM_IQ_ROW_SETUP(sizeof(block_iq3_s))
    const block_iq3_s * x = (const block_iq3_s *) blk_bytes;

    const int tid = (int) threadIdx.x;
    const int il  = tid / 8; // 0..3
    const int ib  = tid % 8; // 0..7
    dst_t * y = y_row + 32 * ib + 8 * il;
    const uint8_t * qs = x->qs + 8 * ib;
    const uint8_t * grid1 =
        (const uint8_t *) (iq3s_grid + (qs[2*il+0] | ((x->qh[ib] << (8-2*il)) & 256)));
    const uint8_t * grid2 =
        (const uint8_t *) (iq3s_grid + (qs[2*il+1] | ((x->qh[ib] << (7-2*il)) & 256)));
    const float d = __half2float(x->d) * (float) (1 + 2*((x->scales[ib/2] >> 4*(ib%2)) & 0xf));
    const uint8_t signs = x->signs[4*ib + il];
    for (int j = 0; j < 4; ++j) {
        y[j + 0] = (dst_t) (d * (float) grid1[j] * (signs & kmask_iq2xs[j + 0] ? -1.f : 1.f));
        y[j + 4] = (dst_t) (d * (float) grid2[j] * (signs & kmask_iq2xs[j + 4] ? -1.f : 1.f));
    }
}

#undef MKLLM_IQ_ROW_SETUP

// One dispatcher for both entry points. `K` must be a multiple of QK_K.
template <typename dst_t>
static cudaError_t mkllm_launch_dq_iq(
        int kind, const void * src, size_t src_row_bytes,
        const int32_t * row_map, void * dst, size_t dst_row_bytes,
        int rows, int K, cudaStream_t stream) {
    if (rows <= 0 || K <= 0 || (K % QK_K) != 0) {
        return cudaErrorInvalidValue;
    }
    const dim3 grid((unsigned) (K / QK_K), (unsigned) rows, 1);
    const uint8_t * s = (const uint8_t *) src;
    switch (kind) {
        case MKLLM_QUANT_Q3K:
            mkllm_dq_q3_k_kernel<dst_t><<<grid, 64, 0, stream>>>(
                s, src_row_bytes, row_map, dst, dst_row_bytes, rows);
            break;
        case MKLLM_QUANT_IQ4XS:
            mkllm_dq_iq4_xs_kernel<dst_t><<<grid, 32, 0, stream>>>(
                s, src_row_bytes, row_map, dst, dst_row_bytes, rows);
            break;
        case MKLLM_QUANT_IQ4NL:
            mkllm_dq_iq4_nl_kernel<dst_t><<<grid, 32, 0, stream>>>(
                s, src_row_bytes, row_map, dst, dst_row_bytes, rows);
            break;
        case MKLLM_QUANT_IQ3S:
            mkllm_dq_iq3_s_kernel<dst_t><<<grid, 32, 0, stream>>>(
                s, src_row_bytes, row_map, dst, dst_row_bytes, rows);
            break;
        default:
            return cudaErrorInvalidValue;
    }
    return cudaGetLastError();
}

// Weight-slab dequant for the cuBLAS prefill fallback: dense bf16 [rows, K].
static cudaError_t mkllm_dequant_iq_rows_bf16(
        int kind, const void * src, void * dst, int rows, int K,
        size_t src_row_bytes, cudaStream_t stream) {
    return mkllm_launch_dq_iq<__nv_bfloat16>(
        kind, src, src_row_bytes, nullptr, dst, (size_t) K * sizeof(__nv_bfloat16),
        rows, K, stream);
}

// get_rows: gather `nrows` indexed source rows into a byte-strided f32 dst.
static cudaError_t mkllm_get_rows_iq_f32(
        int kind, const void * src, const int32_t * row_map, void * dst,
        int ne0, int nrows, size_t src_nb1, size_t dst_nb1, cudaStream_t stream) {
    return mkllm_launch_dq_iq<float>(
        kind, src, src_nb1, row_map, dst, dst_nb1, nrows, ne0, stream);
}
