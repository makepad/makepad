// SPDX-License-Identifier: MIT
// Copyright (c) 2023-2026 The ggml authors
//
// Substantial portions derived from ggml / llama.cpp
// (https://github.com/ggml-org/llama.cpp), MIT licensed.
// The original copyright notice and permission notice are retained.
// See libs/ai/NOTICE and, where present, LICENSE in this directory.
//
// Native CUDA kernels for llama + gen-AI graphs. Compiled once by
// libs/ggml/build.rs from this file (not by makepad-llama).
//
// Semantics contracts:
// - K-quant dequantization is a transcription of the CPU references in
//   libs/ggml/src/quant.rs (dequantize_q4_k / dequantize_row_q5_k /
//   dequantize_q6_k), which are themselves bit-exact vs upstream ggml.
// - rope IMROPE/MROPE is a transcription of kernel_rope_multi in
//   libs/ggml/src/backend/metal/ggml/ggml-metal.metal (the Metal oracle).
// - All strides are BYTE strides unless the name says elems.
// - Every launcher takes the stream last and returns cudaGetLastError().

#include <cuda_runtime.h>
#include <cuda_fp16.h>
#include <cuda_bf16.h>
#include <mma.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define QK_K 256

static __device__ __forceinline__ void mkllm_scale_min_k4(
        int is, const uint8_t * scales, uint8_t * sc, uint8_t * m) {
    if (is < 4) {
        *sc = scales[is] & 63;
        *m  = scales[is + 4] & 63;
    } else {
        *sc = (scales[is + 4] & 0x0F) | ((scales[is - 4] >> 6) << 4);
        *m  = (scales[is + 4] >>   4) | ((scales[is]     >> 6) << 4);
    }
}

// Dequantize one 256-value super-block position `l` (0..256) for each kind.
static __device__ __forceinline__ float mkllm_deq_q4k_at(const uint8_t * b, int l) {
    const float d    = __half2float(((const __half *) b)[0]);
    const float dmin = __half2float(((const __half *) b)[1]);
    const uint8_t * scales = b + 4;
    const uint8_t * qs = b + 16;
    const int j64 = l >> 6;          // 64-value group
    const int rem = l & 63;
    const int is = 2 * j64 + (rem >> 5);
    uint8_t sc, m;
    mkllm_scale_min_k4(is, scales, &sc, &m);
    const uint8_t q = qs[32 * j64 + (rem & 31)];
    const int nib = (rem < 32) ? (q & 0x0F) : (q >> 4);
    return d * (float) sc * (float) nib - dmin * (float) m;
}

static __device__ __forceinline__ float mkllm_deq_q5k_at(const uint8_t * b, int l) {
    const float d    = __half2float(((const __half *) b)[0]);
    const float dmin = __half2float(((const __half *) b)[1]);
    const uint8_t * scales = b + 4;
    const uint8_t * qh = b + 16;
    const uint8_t * qs = b + 48;
    const int j64 = l >> 6;
    const int rem = l & 63;
    const int is = 2 * j64 + (rem >> 5);
    uint8_t sc, m;
    mkllm_scale_min_k4(is, scales, &sc, &m);
    const uint8_t q = qs[32 * j64 + (rem & 31)];
    const int nib = (rem < 32) ? (q & 0x0F) : (q >> 4);
    const uint8_t u = (uint8_t) (1u << is);
    const float hi = (qh[rem & 31] & u) ? 16.0f : 0.0f;
    return d * (float) sc * ((float) nib + hi) - dmin * (float) m;
}

static __device__ __forceinline__ float mkllm_deq_q6k_at(const uint8_t * b, int l) {
    const float d = __half2float(*(const __half *) (b + 208));
    const int n = l >> 7;            // 128-value half
    const int r = l & 127;           // position within half
    const uint8_t * ql = b + n * 64;
    const uint8_t * qh = b + 128 + n * 32;
    const int8_t  * sc = (const int8_t *) (b + 192 + n * 8);
    const int group = r >> 5;        // 0..3 (l, l+32, l+64, l+96 pattern)
    const int lo = r & 31;
    const int is = lo / 16;
    int q;
    switch (group) {
        case 0:  q = (int) ((int8_t) ((ql[lo]      & 0x0F) | ((qh[lo] & 3)        << 4))) - 32; break;
        case 1:  q = (int) ((int8_t) ((ql[lo + 32] & 0x0F) | (((qh[lo] >> 2) & 3) << 4))) - 32; break;
        case 2:  q = (int) ((int8_t) ((ql[lo]      >>   4) | (((qh[lo] >> 4) & 3) << 4))) - 32; break;
        default: q = (int) ((int8_t) ((ql[lo + 32] >>   4) | (((qh[lo] >> 6) & 3) << 4))) - 32; break;
    }
    return d * (float) sc[is + 2 * group] * (float) q;
}

// `kind` values crossing the Rust FFI. 0..3 are the "legacy" kinds that the
// hand-written kernels in this file template over; 4..7 are served only by the
// vendored official llama.cpp templates (mmq.cuh / mmvq.cuh) plus the row
// dequant in iq_convert.cuh. Keep in sync with QUANT_* in
// libs/ai/cuda/src/llm_ops.rs and `quant_kind` in
// libs/ai/llm/src/cuda_exec/real.rs.
#define MKLLM_QUANT_Q4K 0
#define MKLLM_QUANT_Q5K 1
#define MKLLM_QUANT_Q6K 2
// q8_0 uses 32-value/34-byte blocks; the executor addresses quants in
// 256-value units, so treat 8 packed q8_0 blocks (272 bytes) as one unit.
#define MKLLM_QUANT_Q80 3
#define MKLLM_QUANT_LEGACY_LAST MKLLM_QUANT_Q80
// unsloth Dynamic (UD-) GGUFs mix these into otherwise-K-quant files.
#define MKLLM_QUANT_Q3K   4
#define MKLLM_QUANT_IQ4XS 5
// iq4_nl is a 32-value/18-byte block; like q8_0 above, eight of them are
// addressed as one 256-value unit (144 bytes).
#define MKLLM_QUANT_IQ4NL 6
#define MKLLM_QUANT_IQ3S  7
#define MKLLM_QUANT_COUNT 8

static __device__ __forceinline__ float mkllm_deq_q80_at(const uint8_t * b, int l) {
    const uint8_t * blk = b + (l >> 5) * 34;
    const float d = __half2float(*(const __half *) blk);
    return d * (float) ((const int8_t *) (blk + 2))[l & 31];
}

// Legacy-kind selectors. These are reached with a compile-time KIND from the
// hand-written kernels only, so an unknown kind is a programming error, not a
// runtime input: return a NaN / zero-size sentinel rather than silently
// decoding the bytes as some other type. (House rule from the reclaimed-
// readback fix: a contract violation must be loud, never plausible.)
static __device__ __forceinline__ float mkllm_deq_at(int kind, const uint8_t * b, int l) {
    switch (kind) {
        case MKLLM_QUANT_Q4K: return mkllm_deq_q4k_at(b, l);
        case MKLLM_QUANT_Q5K: return mkllm_deq_q5k_at(b, l);
        case MKLLM_QUANT_Q6K: return mkllm_deq_q6k_at(b, l);
        case MKLLM_QUANT_Q80: return mkllm_deq_q80_at(b, l);
        default:              return __int_as_float(0x7fffffff); // NaN
    }
}

static __device__ __forceinline__ int mkllm_quant_block_bytes_dev(int kind) {
    switch (kind) {
        case MKLLM_QUANT_Q4K: return 144;
        case MKLLM_QUANT_Q5K: return 176;
        case MKLLM_QUANT_Q6K: return 210;
        case MKLLM_QUANT_Q80: return 272;
        default:              return 0;
    }
}

// ---------------------------------------------------------------------------
// Device info
// ---------------------------------------------------------------------------

extern "C" cudaError_t mkllm_device_info(
        int device, char * name, int name_cap,
        int * cc_major, int * cc_minor, size_t * total_mem, int * sm_count) {
    cudaDeviceProp prop;
    cudaError_t err = cudaGetDeviceProperties(&prop, device);
    if (err != cudaSuccess) {
        return err;
    }
    if (name != nullptr && name_cap > 0) {
        int n = (int) strlen(prop.name);
        if (n >= name_cap) n = name_cap - 1;
        memcpy(name, prop.name, n);
        name[n] = 0;
    }
    *cc_major = prop.major;
    *cc_minor = prop.minor;
    *total_mem = prop.totalGlobalMem;
    *sm_count = prop.multiProcessorCount;
    return cudaSuccess;
}

// ---------------------------------------------------------------------------
// Quantized / float mat-vec: dst[N, M] = src0[K, N]^T . src1[K, M], M small.
// One warp per (row, col-batch) output; f32 accumulation; reads the raw
// GGUF block stream directly (no dequant materialization).
// ---------------------------------------------------------------------------

static __device__ __forceinline__ float4 mkllm_ld_f4(const float * p) {
    return *(const float4 *) p;
}

// Warp-per-row quantized mat-vec, bandwidth-shaped: the 128-byte nibble/int8
// payload of each super-block is read as coalesced u32 words (one per lane),
// sub-block scales are decoded once into registers, and for M > 1 the decoded
// weights are reused across activation columns (M <= 8 held in registers).
//
// q4_K lane slice: lane l owns 4 payload bytes of 64-value group g = l/8 —
// 4 low-nibble values at g*64 + (l%8)*4 .. +3 and 4 high-nibble values at
// +32; the two touched sub-blocks are 2g (low) and 2g+1 (high).
//
// Lossless vs the scalar dequant oracles: same lane ownership, same
// `d*sc*q - dmin*m` then FMA into acc, same shfl_down reduction. Q6 is
// branchless (group is not warp-uniform) and uses 2-byte payload loads
// because Q6_K blocks are 210 bytes (2-aligned, not 4).
template <int KIND, int M_MAX>
static __global__ void mkllm_mmv_qk_kernel(
        const uint8_t * __restrict__ src0, const float * __restrict__ src1,
        float * __restrict__ dst,
        int K, int N, int M,
        size_t src0_row_bytes, size_t src1_col_elems, size_t dst_col_elems) {
    const int row = blockIdx.x * (blockDim.y) + threadIdx.y;
    if (row >= N) return;
    const int lane = threadIdx.x; // 32 lanes
    const uint8_t * row_bytes = src0 + (size_t) row * src0_row_bytes;
    const int blocks = K / QK_K;

    float acc[M_MAX];
#pragma unroll
    for (int c = 0; c < M_MAX; c++) {
        acc[c] = 0.0f;
    }

    for (int b = 0; b < blocks; b++) {
        const uint8_t * blk = row_bytes + (size_t) b * mkllm_quant_block_bytes_dev(KIND);
        const int xbase = b * QK_K;

        if (KIND == MKLLM_QUANT_Q4K || KIND == MKLLM_QUANT_Q5K) {
            const float d    = __half2float(((const __half *) blk)[0]);
            const float dmin = __half2float(((const __half *) blk)[1]);
            const uint8_t * scales = blk + 4;
            const uint8_t * qs = blk + (KIND == MKLLM_QUANT_Q4K ? 16 : 48);
            const int group = lane >> 3;           // 64-value group 0..3
            const int byte4 = (lane & 7) * 4;      // byte offset in group
            const uint32_t q = *(const uint32_t *) (qs + group * 32 + byte4);
            const int sb_lo = 2 * group;
            uint8_t sc_lo, m_lo, sc_hi, m_hi;
            mkllm_scale_min_k4(sb_lo, scales, &sc_lo, &m_lo);
            mkllm_scale_min_k4(sb_lo + 1, scales, &sc_hi, &m_hi);
            float w[8];
#pragma unroll
            for (int i = 0; i < 4; i++) {
                const uint32_t byte = (q >> (8 * i)) & 0xFF;
                float lo = (float) (byte & 0x0F);
                float hi = (float) (byte >> 4);
                if (KIND == MKLLM_QUANT_Q5K) {
                    const uint8_t qh = blk[16 + byte4 + i];
                    lo += (qh & (1u << sb_lo)) ? 16.0f : 0.0f;
                    hi += (qh & (2u << sb_lo)) ? 16.0f : 0.0f;
                }
                w[i] = d * (float) sc_lo * lo - dmin * (float) m_lo;
                w[4 + i] = d * (float) sc_hi * hi - dmin * (float) m_hi;
            }
            const int lo_at = xbase + group * 64 + byte4;
#pragma unroll
            for (int c = 0; c < M_MAX; c++) {
                if (c >= M) break;
                const float * x = src1 + (size_t) c * src1_col_elems;
#pragma unroll
                for (int i = 0; i < 4; i++) {
                    acc[c] += w[i] * x[lo_at + i] + w[4 + i] * x[lo_at + 32 + i];
                }
            }
        } else if (KIND == MKLLM_QUANT_Q6K) {
            // 8 consecutive values at l0 = lane*8. Group is not warp-uniform,
            // so extract with bit math (no switch). Q6_K block is 210 bytes.
            const float d = __half2float(*(const __half *) (blk + 208));
            const int l0 = lane * 8;
            const int n = l0 >> 7;
            const int r = l0 & 127;
            const int group = r >> 5;
            const int lo0 = r & 31;
            const int is = lo0 >> 4;
            const int ql_off = (group & 1) * 32;
            const int nibble = (group >> 1) & 1;
            const int qh_shift = group * 2;
            const uint8_t * ql = blk + n * 64 + ql_off + lo0;
            const uint8_t * qh = blk + 128 + n * 32 + lo0;
            const int8_t * sc = (const int8_t *) (blk + 192 + n * 8);
            const float dsc = d * (float) sc[is + 2 * group];
            const uint16_t * ql16 = (const uint16_t *) ql;
            const uint16_t * qh16 = (const uint16_t *) qh;
            const uint16_t ql_w[4] = { ql16[0], ql16[1], ql16[2], ql16[3] };
            const uint16_t qh_w[4] = { qh16[0], qh16[1], qh16[2], qh16[3] };
            float wv[8];
#pragma unroll
            for (int i = 0; i < 8; i++) {
                const int qlb = (int) ((ql_w[i >> 1] >> ((i & 1) * 8)) & 0xFF);
                const int qhb = (int) ((qh_w[i >> 1] >> ((i & 1) * 8)) & 0xFF);
                const int raw = ((qlb >> (4 * nibble)) & 0x0F)
                    | (((qhb >> qh_shift) & 3) << 4);
                const int q = (int) ((int8_t) raw) - 32;
                wv[i] = dsc * (float) q;
            }
#pragma unroll
            for (int c = 0; c < M_MAX; c++) {
                if (c >= M) break;
                const float * x = src1 + (size_t) c * src1_col_elems + xbase + l0;
                const float4 x0 = mkllm_ld_f4(x);
                const float4 x1 = mkllm_ld_f4(x + 4);
                acc[c] += wv[0] * x0.x;
                acc[c] += wv[1] * x0.y;
                acc[c] += wv[2] * x0.z;
                acc[c] += wv[3] * x0.w;
                acc[c] += wv[4] * x1.x;
                acc[c] += wv[5] * x1.y;
                acc[c] += wv[6] * x1.z;
                acc[c] += wv[7] * x1.w;
            }
        } else { // Q8_0 packed: 8 sub-blocks of 32 int8 + f16 d
            const int sub = lane >> 2;             // 0..7
            const int b4 = (lane & 3) * 8;         // 8 values per lane
            const uint8_t * q8 = blk + sub * 34;
            const float d = __half2float(*(const __half *) q8);
            const int8_t * qv = (const int8_t *) (q8 + 2);
            const int at = xbase + sub * 32 + b4;
#pragma unroll
            for (int c = 0; c < M_MAX; c++) {
                if (c >= M) break;
                const float * x = src1 + (size_t) c * src1_col_elems;
                float sub_acc = 0.0f;
#pragma unroll
                for (int i = 0; i < 8; i++) {
                    sub_acc += (float) qv[b4 + i] * x[at + i];
                }
                acc[c] += d * sub_acc;
            }
        }
    }

#pragma unroll
    for (int c = 0; c < M_MAX; c++) {
        if (c >= M) break;
        float total = acc[c];
        for (int off = 16; off > 0; off >>= 1) {
            total += __shfl_down_sync(0xffffffff, total, off);
        }
        if (lane == 0) {
            dst[(size_t) c * dst_col_elems + row] = total;
        }
    }
}

template <int KIND>
static void mkllm_launch_mmv_quant(
        const void * src0, const float * src1, float * dst,
        int K, int N, int M,
        size_t src0_row_bytes, size_t src1_col_elems, size_t dst_col_elems,
        cudaStream_t stream) {
    dim3 block(32, 4);
    dim3 grid((N + 3) / 4);
    if (M == 1) {
        mkllm_mmv_qk_kernel<KIND, 1><<<grid, block, 0, stream>>>(
            (const uint8_t *) src0, src1, dst, K, N, M, src0_row_bytes, src1_col_elems, dst_col_elems);
    } else if (M <= 2) {
        mkllm_mmv_qk_kernel<KIND, 2><<<grid, block, 0, stream>>>(
            (const uint8_t *) src0, src1, dst, K, N, M, src0_row_bytes, src1_col_elems, dst_col_elems);
    } else if (M <= 4) {
        mkllm_mmv_qk_kernel<KIND, 4><<<grid, block, 0, stream>>>(
            (const uint8_t *) src0, src1, dst, K, N, M, src0_row_bytes, src1_col_elems, dst_col_elems);
    } else {
        mkllm_mmv_qk_kernel<KIND, 8><<<grid, block, 0, stream>>>(
            (const uint8_t *) src0, src1, dst, K, N, M, src0_row_bytes, src1_col_elems, dst_col_elems);
    }
}

extern "C" cudaError_t mkllm_mmv_quant(
        int kind,
        const void * src0, const float * src1, float * dst,
        int K, int N, int M,
        size_t src0_row_bytes, size_t src1_col_elems, size_t dst_col_elems,
        cudaStream_t stream) {
    switch (kind) {
        case MKLLM_QUANT_Q4K:
            mkllm_launch_mmv_quant<MKLLM_QUANT_Q4K>(
                src0, src1, dst, K, N, M, src0_row_bytes, src1_col_elems, dst_col_elems, stream);
            break;
        case MKLLM_QUANT_Q5K:
            mkllm_launch_mmv_quant<MKLLM_QUANT_Q5K>(
                src0, src1, dst, K, N, M, src0_row_bytes, src1_col_elems, dst_col_elems, stream);
            break;
        case MKLLM_QUANT_Q80:
            mkllm_launch_mmv_quant<MKLLM_QUANT_Q80>(
                src0, src1, dst, K, N, M, src0_row_bytes, src1_col_elems, dst_col_elems, stream);
            break;
        case MKLLM_QUANT_Q6K:
            mkllm_launch_mmv_quant<MKLLM_QUANT_Q6K>(
                src0, src1, dst, K, N, M, src0_row_bytes, src1_col_elems, dst_col_elems, stream);
            break;
        default:
            // This hand-written mat-vec only templates over the legacy kinds.
            // Falling back to Q6_K here would silently decode e.g. iq4_xs
            // bytes as q6_K and return plausible garbage.
            return cudaErrorInvalidValue;
    }
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// M=1 packed K-quant x dynamic Q8_1 activation MMV.
//
// Quantizer and vec-dot structure follow llama.cpp b10430
// (commit 4c1a0af40d88c7fbb3b15c85bf2e8016d1d5b64c):
//   ggml-cuda/quantize.cu, mmvq.{cu,cuh}, vecdotq.cuh
// Copyright (c) ggml authors. MIT license. Retained because this is a
// substantial port of the warp Q8_1 quant and packed Q4/Q5/Q6 dp4a dots.
//
// Warp-parallel Q8_1: one lane per value, warp max + sum, d = amax/127,
// q = roundf(xi/d). Q4_K/Q5_K min terms use d8 * sum(q8) via dp4a with
// 0x01010101. Q6_K uses signed (ql|qh)-32. Decode launch is llama.cpp
// GENERIC MMVQ: dim3(32, 4) on 3090/4090/5090 (ncols_dst=1).
// Fail closed unless M==1, K%256==0, and kind is Q4_K/Q5_K/Q6_K/Q8_0.
// The float MMV kernel above remains the fallback.
// ---------------------------------------------------------------------------

#define MKLLM_Q81_GS 32
#define MKLLM_QK8_1 32
#define MKLLM_QI8_1 8
#define MKLLM_QR4_K 2
#define MKLLM_QI4_K (QK_K / (4 * MKLLM_QR4_K))
#define MKLLM_QR5_K 2
#define MKLLM_QI5_K (QK_K / (4 * MKLLM_QR5_K))
#define MKLLM_QR6_K 2
#define MKLLM_QI6_K (QK_K / (4 * MKLLM_QR6_K))
#define MKLLM_VDR_Q4_K_MMVQ 2
#define MKLLM_VDR_Q5_K_MMVQ 2
#define MKLLM_VDR_Q6_K_MMVQ 1
#define MKLLM_MMVQ_NWARPS 4

struct __align__(4) mkllm_block_q8_1 {
    half2 ds;
    int8_t qs[MKLLM_QK8_1];
};
static_assert(sizeof(mkllm_block_q8_1) == 36, "block_q8_1 size");

struct mkllm_block_q4_K {
    half2 dm;
    uint8_t scales[12];
    uint8_t qs[QK_K / 2];
};
static_assert(sizeof(mkllm_block_q4_K) == 144, "block_q4_K size");

struct mkllm_block_q5_K {
    half2 dm;
    uint8_t scales[12];
    uint8_t qh[QK_K / 8];
    uint8_t qs[QK_K / 2];
};
static_assert(sizeof(mkllm_block_q5_K) == 176, "block_q5_K size");

struct mkllm_block_q6_K {
    uint8_t ql[QK_K / 2];
    uint8_t qh[QK_K / 4];
    int8_t scales[QK_K / 16];
    half d;
};
static_assert(sizeof(mkllm_block_q6_K) == 210, "block_q6_K size");

static __device__ __forceinline__ int mkllm_get_int_b2(const void * x, int i32) {
    const uint16_t * x16 = (const uint16_t *) x;
    return (int) x16[2 * i32] | ((int) x16[2 * i32 + 1] << 16);
}

static __device__ __forceinline__ int mkllm_get_int_b4(const void * x, int i32) {
    return ((const int *) x)[i32];
}

static __device__ __forceinline__ float mkllm_warp_reduce_max32(float v) {
#pragma unroll
    for (int off = 16; off > 0; off >>= 1) {
        v = fmaxf(v, __shfl_xor_sync(0xffffffff, v, off));
    }
    return v;
}

static __device__ __forceinline__ float mkllm_warp_reduce_sum32(float v) {
#pragma unroll
    for (int off = 16; off > 0; off >>= 1) {
        v += __shfl_xor_sync(0xffffffff, v, off);
    }
    return v;
}

// llama.cpp quantize.cu:5-48 + :282-285. CUDA_QUANTIZE_BLOCK_SIZE=256,
// one thread per value, warp_reduce<QK8_1> inside each 32-lane group.
#define MKLLM_QUANTIZE_BLOCK_SIZE 256
static __global__ void __launch_bounds__(MKLLM_QUANTIZE_BLOCK_SIZE, 1)
mkllm_quantize_q81_kernel(
        const float * __restrict__ x, mkllm_block_q8_1 * __restrict__ y, int ne0) {
    const int i0 = (int) blockDim.x * (int) blockIdx.x + (int) threadIdx.x;
    if (i0 >= ne0) {
        return;
    }
    const int ib = i0 / MKLLM_QK8_1;
    const int iqs = i0 % MKLLM_QK8_1;
    const float xi = x[i0];
    float amax = fabsf(xi);
    float sum = xi;
    amax = mkllm_warp_reduce_max32(amax);
    sum = mkllm_warp_reduce_sum32(sum);
    const float d = amax / 127.0f;
    const int8_t q = amax == 0.0f ? 0 : (int8_t) roundf(xi / d);
    y[ib].qs[iqs] = q;
    if (iqs > 0) {
        return;
    }
    y[ib].ds = make_half2(d, sum);
}

extern "C" cudaError_t mkllm_quantize_q81(
        const float * x, void * y, int k, cudaStream_t stream) {
    if (k <= 0 || (k % MKLLM_Q81_GS) != 0) {
        return cudaErrorInvalidValue;
    }
    const int block_num = (k + MKLLM_QUANTIZE_BLOCK_SIZE - 1) / MKLLM_QUANTIZE_BLOCK_SIZE;
    mkllm_quantize_q81_kernel<<<block_num, MKLLM_QUANTIZE_BLOCK_SIZE, 0, stream>>>(
        x, (mkllm_block_q8_1 *) y, k);
    return cudaGetLastError();
}

#include "fattn/mmvq.cuh"
#include "fattn/mmq.cuh"
#include "fattn/norm.cuh"
// Needs ggml-common.h's block structs + codebook tables, which arrive with
// mmvq.cuh above.
#include "iq_convert.cuh"

// llama.cpp ggml-cuda.cu:4000 ggml_cuda_op_rms_norm_fused /
// ggml-cuda.cu:3994 ggml_cuda_op_rms_norm_fused_add. Strides are BYTES.
extern "C" cudaError_t mkllm_rms_norm_mul(
        const void * x, const void * mul, const void * add, void * dst,
        int ncols, int nrows, int nchannels, int nsamples, float eps,
        size_t x_nb1, size_t x_nb2, size_t x_nb3,
        size_t d_nb1, size_t d_nb2, size_t d_nb3,
        size_t mul_nb1, size_t mul_nb2, size_t mul_nb3,
        int mul_ne0, int mul_ne1, int mul_ne2, int mul_ne3,
        size_t add_nb1, size_t add_nb2, size_t add_nb3,
        int add_ne0, int add_ne1, int add_ne2, int add_ne3,
        cudaStream_t stream) {
    if (ncols <= 0 || nrows <= 0 || nchannels <= 0 || nsamples <= 0 || mul == nullptr) {
        return cudaErrorInvalidValue;
    }
    rms_norm_mul_f32_cuda(
        (const float *) x, (const float *) mul, (const float *) add, (float *) dst,
        ncols, nrows, nchannels, nsamples,
        (int64_t) (x_nb1 / sizeof(float)), (int64_t) (x_nb2 / sizeof(float)),
        (int64_t) (x_nb3 / sizeof(float)),
        (int64_t) (d_nb1 / sizeof(float)), (int64_t) (d_nb2 / sizeof(float)),
        (int64_t) (d_nb3 / sizeof(float)),
        (int64_t) (mul_nb1 / sizeof(float)), (int64_t) (mul_nb2 / sizeof(float)),
        (int64_t) (mul_nb3 / sizeof(float)),
        (uint32_t) mul_ne0, (uint32_t) mul_ne1, (uint32_t) mul_ne2, (uint32_t) mul_ne3,
        (int64_t) (add_nb1 / sizeof(float)), (int64_t) (add_nb2 / sizeof(float)),
        (int64_t) (add_nb3 / sizeof(float)),
        (uint32_t) add_ne0, (uint32_t) add_ne1, (uint32_t) add_ne2, (uint32_t) add_ne3,
        eps, stream);
    return cudaGetLastError();
}

// kind -> ggml_type for the vendored official templates. GGML_TYPE_COUNT is
// the "no such kind" sentinel; every caller must reject it rather than
// defaulting, or a new kind added to only some of these switches would decode
// as Q4_K and return plausible-but-wrong numbers.
static ggml_type mkllm_kind_to_ggml(int kind) {
    switch (kind) {
        case MKLLM_QUANT_Q4K:   return GGML_TYPE_Q4_K;
        case MKLLM_QUANT_Q5K:   return GGML_TYPE_Q5_K;
        case MKLLM_QUANT_Q6K:   return GGML_TYPE_Q6_K;
        case MKLLM_QUANT_Q80:   return GGML_TYPE_Q8_0;
        case MKLLM_QUANT_Q3K:   return GGML_TYPE_Q3_K;
        case MKLLM_QUANT_IQ4XS: return GGML_TYPE_IQ4_XS;
        case MKLLM_QUANT_IQ4NL: return GGML_TYPE_IQ4_NL;
        case MKLLM_QUANT_IQ3S:  return GGML_TYPE_IQ3_S;
        default:                return GGML_TYPE_COUNT;
    }
}

// Bytes per ggml storage block (NOT per 256 values: iq4_nl and q8_0 are
// 32-value blocks). Returns 0 for an unknown kind; callers must reject it.
static int mkllm_kind_block_bytes(int kind) {
    switch (kind) {
        case MKLLM_QUANT_Q4K:   return (int) sizeof(block_q4_K);
        case MKLLM_QUANT_Q5K:   return (int) sizeof(block_q5_K);
        case MKLLM_QUANT_Q6K:   return (int) sizeof(block_q6_K);
        case MKLLM_QUANT_Q80:   return (int) sizeof(block_q8_0);
        case MKLLM_QUANT_Q3K:   return (int) sizeof(block_q3_K);
        case MKLLM_QUANT_IQ4XS: return (int) sizeof(block_iq4_xs);
        case MKLLM_QUANT_IQ4NL: return (int) sizeof(block_iq4_nl);
        case MKLLM_QUANT_IQ3S:  return (int) sizeof(block_iq3_s);
        default:                return 0;
    }
}

// The MMQ path quantizes activations to block_q8_1_mmq in one of two scale
// layouts; picking the wrong one silently corrupts the result. Mirrors
// mmq_get_q8_1_ds_layout() in fattn/mmq.cuh. Returns -1 for an unknown kind.
static int mkllm_kind_mmq_ds4(int kind) {
    switch (kind) {
        case MKLLM_QUANT_Q4K:
        case MKLLM_QUANT_Q5K:   return 1; // MMQ_Q8_1_DS_LAYOUT_DS4
        case MKLLM_QUANT_Q6K:
        case MKLLM_QUANT_Q3K:
        case MKLLM_QUANT_IQ4XS:
        case MKLLM_QUANT_IQ4NL:
        case MKLLM_QUANT_IQ3S:  return 0; // MMQ_Q8_1_DS_LAYOUT_D4
        default:                return -1;
    }
}

// Bytes the FFI caller must reserve per (row-of-256-values) unit when it
// addresses a weight row in 256-value units. Returns 0 for an unknown kind.
static int mkllm_kind_bytes_per_256(int kind) {
    const int blk = mkllm_kind_block_bytes(kind);
    if (blk == 0) {
        return 0;
    }
    switch (kind) {
        case MKLLM_QUANT_Q80:
        case MKLLM_QUANT_IQ4NL: return blk * 8; // 32-value blocks
        default:                return blk;     // 256-value super-blocks
    }
}

// Which quantized-matmul routes a kind is VERIFIED on, bit 0 = official MMVQ
// (decode), bit 1 = official J=128 MMQ (prefill). A kind may always fall back
// to `mkllm_dequant_rows_bf16` + cuBLAS, which is checked separately and is
// exact up to bf16 rounding, so clearing a bit costs speed, never support.
//
// IQ3_S is cleared on both: `llama-cuda-canary opcheck` shows its tiles
// disagreeing with the dequant by ~1e-2 of the summed term magnitude on
// sm_120, where every other kind lands near 3e-5 — while `getrows_iq3s`
// proves the dequant itself is bit-exact against the scalar reference (which
// is in turn pinned to llama.cpp's gguf-py dequantizers). The vendored
// vec_dot_iq3_s_q8_1 / load_tiles_iq3_s are byte-identical to upstream and the
// iq3s_grid table matches, so this is an open upstream-kernel question, not a
// porting slip — parked with the evidence rather than shipped with a route
// that returns plausible-but-wrong numbers. IQ3_S is 4 tensors of 866 in
// Qwen3.8-27B-UD-Q4_K_M, so the cost of the dequant fallback is noise.
// Re-enable by flipping a bit here and re-running the canary.
#define MKLLM_ROUTE_MMVQ 1
#define MKLLM_ROUTE_MMQ  2

static int mkllm_kind_route_mask(int kind) {
    switch (kind) {
        case MKLLM_QUANT_Q4K:
        case MKLLM_QUANT_Q5K:
        case MKLLM_QUANT_Q6K:
        case MKLLM_QUANT_Q3K:
        case MKLLM_QUANT_IQ4NL:
        case MKLLM_QUANT_IQ4XS: return MKLLM_ROUTE_MMVQ | MKLLM_ROUTE_MMQ;
        case MKLLM_QUANT_IQ3S:  return 0;
        // q8_0 decode is the hand-written mmv_quant kernel, not official MMVQ.
        case MKLLM_QUANT_Q80:   return 0;
        default:                return -1;
    }
}

extern "C" int mkllm_quant_kind_routes(int kind) {
    return mkllm_kind_route_mask(kind);
}

extern "C" int mkllm_quant_kind_block_bytes(int kind) {
    return mkllm_kind_block_bytes(kind);
}

// Exposed so the Rust dispatcher picks the activation layout from the same
// table the kernels use, instead of keeping a second copy that can drift.
extern "C" int mkllm_quant_kind_mmq_ds4(int kind) {
    return mkllm_kind_mmq_ds4(kind);
}

extern "C" int mkllm_quant_kind_bytes_per_256(int kind) {
    return mkllm_kind_bytes_per_256(kind);
}

// llama.cpp mmvq.cu ggml_cuda_mul_mat_vec_q: ONE weight read serves up to
// MMVQ_MAX_BATCH_SIZE destination columns. Pinning this at ncols_dst=1 sent
// every 2..8-token batch to the generic float mat-vec, which costs ~2/3 of a
// full forward PER EXTRA TOKEN — exactly what stops speculative decoding from
// paying. The kernel template already spans the whole range; only the
// launcher had to learn it.
template <ggml_type type, int ncols_dst, bool has_fusion>
static void mkllm_launch_mmvq_official_ncols(
        const void * vx, const void * vgate, const void * vy, float * dst,
        int k, int n, int stride_row_x, cudaStream_t stream) {
    ggml_cuda_mm_fusion_args_device fusion{};
    if constexpr (has_fusion) {
        fusion.gate = vgate;
        fusion.glu_op = GGML_GLU_OP_SWIGLU;
    }
    // mmvq.cu:729-731 + :807-814: ids=null, small_k=false. nchannels_y_fd is
    // zero when ids is null; channel/sample ratios are fastdiv(1) and their
    // strides 1 (ggml_cuda_op_mul_mat_vec_q).
    const uint3 nchannels_y = make_uint3(0, 0, 0);
    const uint3 channel_ratio = init_fastdiv_values(1);
    const uint3 sample_ratio = init_fastdiv_values(1);
    const int nwarps = calc_nwarps(type, ncols_dst, MMVQ_PARAMETERS_GENERIC);
    const int rows_per_block =
        calc_rows_per_block(ncols_dst, MMVQ_PARAMETERS_GENERIC, false, nwarps);
    const dim3 block_dims(32, (unsigned) nwarps, 1);
    const dim3 block_nums((unsigned) ((n + rows_per_block - 1) / rows_per_block), 1, 1);
    const uint32_t stride_col_y = (uint32_t) (k / QK8_1);
    static int trace_mode = -1;
    static int traces = 0;
    if (trace_mode < 0) {
        trace_mode = getenv("MAKEPAD_LLAMA_MMVQ_TRACE") ? 1 : 0;
    }
    if (trace_mode && traces < 16) {
        fprintf(stderr,
            "mmvq.launch: official=mmvq.cu:389 type=%d ncols_dst=%d fuse=%d nwarps=%d "
            "rows_per_block=%d grid=(%u,1,1) k=%d n=%d stride_row_x=%d stride_col_y=%u\n",
            (int) type, ncols_dst, (int) has_fusion, nwarps, rows_per_block,
            block_nums.x, k, n, stride_row_x, stride_col_y);
        ++traces;
    }
    mul_mat_vec_q<type, ncols_dst, has_fusion, false><<<block_nums, block_dims, 0, stream>>>(
        vx, vy, nullptr, fusion, dst,
        (uint32_t) k, nchannels_y, (uint32_t) stride_row_x, stride_col_y,
        (uint32_t) n, channel_ratio, 1u, 1u, 1u,
        sample_ratio, 1u, 1u, 1u, 0u);
}

template <ggml_type type, bool has_fusion>
static cudaError_t mkllm_launch_mmvq_official(
        const void * vx, const void * vgate, const void * vy, float * dst,
        int k, int n, int m, int stride_row_x, cudaStream_t stream) {
#define MKLLM_MMVQ_CASE(NC) \
    case NC: mkllm_launch_mmvq_official_ncols<type, NC, has_fusion>( \
        vx, vgate, vy, dst, k, n, stride_row_x, stream); break;
    switch (m) {
        MKLLM_MMVQ_CASE(1)
        MKLLM_MMVQ_CASE(2)
        MKLLM_MMVQ_CASE(3)
        MKLLM_MMVQ_CASE(4)
        MKLLM_MMVQ_CASE(5)
        MKLLM_MMVQ_CASE(6)
        MKLLM_MMVQ_CASE(7)
        MKLLM_MMVQ_CASE(8)
        default: return cudaErrorInvalidValue;
    }
#undef MKLLM_MMVQ_CASE
    return cudaSuccess;
}

// llama.cpp mmq.cuh:3960 launch_mul_mat_q (shared size, stream-k nsm, fixup).
// Host ggml_backend_cuda_context trimmed; kernel is the official template.
template <ggml_type type, bool need_check>
static cudaError_t mkllm_launch_mul_mat_q_checked(
        const void * x, const void * y, float * dst,
        int k, int n, int m, int stride_row_x, int stride_col_dst,
        int nsm, float * tmp_fixup, cudaStream_t stream) {
    constexpr int mmq_x = 128;
    static int mmq_prints = 0;
    if (mmq_prints < 8 && (m % 128 != 0)) {
        fprintf(stderr, "mmq.launch: type=%d m=%d n=%d k=%d mmq_x=%d need_check=%d nsm=%d\n",
            (int) type, m, n, k, mmq_x, (int) need_check, nsm);
        ++mmq_prints;
    }
    const int cc = 890;
    const int warp_size = 32;
    const int nwarps = mmq_get_nwarps_host(cc, warp_size);
    const int mmq_y = get_mmq_y_host(cc);
    const int nbytes_shared = (int) mmq_get_nbytes_shared<type>(
        mmq_x, mmq_y, cc, warp_size, nwarps);
    const dim3 block_dims((unsigned) warp_size, (unsigned) nwarps, 1);
    const int nty = (n + mmq_y - 1) / mmq_y;
    const int ntx = (m + mmq_x - 1) / mmq_x;
    cudaError_t err = cudaFuncSetAttribute(
        mul_mat_q<type, mmq_x, need_check>,
        cudaFuncAttributeMaxDynamicSharedMemorySize, nbytes_shared);
    if (err != cudaSuccess) {
        return err;
    }
    const bool use_stream_k = nsm > 0 && tmp_fixup != nullptr;
    if (!use_stream_k) {
        dim3 grid((unsigned) ntx * (unsigned) nty, 1, 1);
        mul_mat_q<type, mmq_x, need_check><<<grid, block_dims, nbytes_shared, stream>>>(
            (const char *) x, (const int *) y, nullptr, nullptr, dst, nullptr,
            k, n, m, stride_row_x, m, stride_col_dst,
            1, 1, 0, 0, 0,
            1, 1, 0, 0, 0,
            m);
        return cudaGetLastError();
    }
    dim3 grid_sk((unsigned) nsm, 1, 1);
    const bool fixup_needed = ((int64_t) ntx * nty) % nsm != 0;
    mul_mat_q<type, mmq_x, need_check><<<grid_sk, block_dims, nbytes_shared, stream>>>(
        (const char *) x, (const int *) y, nullptr, nullptr, dst, tmp_fixup,
        k, n, m, stride_row_x, m, stride_col_dst,
        1, 1, 0, 0, 0,
        1, 1, 0, 0, 0,
        m);
    err = cudaGetLastError();
    if (err != cudaSuccess) {
        return err;
    }
    if (!fixup_needed) {
        return cudaSuccess;
    }
    mul_mat_q_stream_k_fixup<type, mmq_x, need_check><<<grid_sk, block_dims, 0, stream>>>(
        nullptr, nullptr, dst, tmp_fixup,
        k, n, m, (size_t) stride_col_dst,
        1, 0, 1, 0, m);
    return cudaGetLastError();
}

template <ggml_type type>
static cudaError_t mkllm_launch_mul_mat_q(
        const void * x, const void * y, float * dst,
        int k, int n, int m, int stride_row_x, int stride_col_dst,
        int nsm, float * tmp_fixup, cudaStream_t stream) {
    const int mmq_y = get_mmq_y_host(890);
    if (n % mmq_y == 0) {
        return mkllm_launch_mul_mat_q_checked<type, false>(
            x, y, dst, k, n, m, stride_row_x, stride_col_dst, nsm, tmp_fixup, stream);
    }
    return mkllm_launch_mul_mat_q_checked<type, true>(
        x, y, dst, k, n, m, stride_row_x, stride_col_dst, nsm, tmp_fixup, stream);
}

#if 0
// Homemade MMVQ inner loop retired: official vecdotq.cuh + mul_mat_vec_q.
static __device__ __forceinline__ int mkllm_ld_i32_b2(const void * p) {
    const uint16_t * x = (const uint16_t *) p;
    return (int) x[0] | ((int) x[1] << 16);
}

static __device__ __forceinline__ int mkllm_dp4a(int a, int b, int c) {
#if defined(__CUDA_ARCH__) && __CUDA_ARCH__ >= 610
    return __dp4a(a, b, c);
#else
    const signed char * aa = (const signed char *) &a;
    const signed char * bb = (const signed char *) &b;
    return c + (int) aa[0] * bb[0] + (int) aa[1] * bb[1]
        + (int) aa[2] * bb[2] + (int) aa[3] * bb[3];
#endif
}

// llama.cpp vecdotq.cuh:502-524 vec_dot_q4_K_q8_1_impl_vmmq
static __device__ __forceinline__ float mkllm_vec_dot_q4_K_q8_1_impl_vmmq(
        const int * __restrict__ v, const int * __restrict__ u, const uint8_t * __restrict__ sc,
        const uint8_t * __restrict__ m, const half2 & dm4, const float * __restrict__ d8) {
    float sumf_d = 0.0f;
    float sumf_m = 0.0f;
#pragma unroll
    for (int i = 0; i < MKLLM_QR4_K; ++i) {
        const int v0i = (v[0] >> (4 * i)) & 0x0F0F0F0F;
        const int v1i = (v[1] >> (4 * i)) & 0x0F0F0F0F;
        const int dot1 = mkllm_dp4a(v1i, u[2 * i + 1], mkllm_dp4a(v0i, u[2 * i + 0], 0));
        const int dot2 = mkllm_dp4a(0x01010101, u[2 * i + 1], mkllm_dp4a(0x01010101, u[2 * i + 0], 0));
        sumf_d += d8[i] * (dot1 * sc[i]);
        sumf_m += d8[i] * (dot2 * m[i]);
    }
    const float2 dm4f = __half22float2(dm4);
    return dm4f.x * sumf_d - dm4f.y * sumf_m;
}

// llama.cpp vecdotq.cuh:816-859 vec_dot_q4_K_q8_1
static __device__ __forceinline__ float mkllm_vec_dot_q4_K_q8_1(
        const void * vbq, const mkllm_block_q8_1 * bq8_1, int kbx, int iqs) {
    const mkllm_block_q4_K * bq4_K = (const mkllm_block_q4_K *) vbq + kbx;
    int v[2];
    int u[2 * MKLLM_QR4_K];
    float d8[MKLLM_QR4_K];
    const int bq8_offset = MKLLM_QR4_K * ((iqs / 2) / (MKLLM_QI8_1 / 2));
    const int * q4 = (const int *) (bq4_K->qs + 16 * bq8_offset + 4 * ((iqs / 2) % 4));
    v[0] = q4[0];
    v[1] = q4[4];
    const uint16_t * scales = (const uint16_t *) bq4_K->scales;
    uint16_t aux[2];
    const int j = bq8_offset / 2;
    if (j < 2) {
        aux[0] = scales[j + 0] & 0x3f3f;
        aux[1] = scales[j + 2] & 0x3f3f;
    } else {
        aux[0] = ((scales[j + 2] >> 0) & 0x0f0f) | ((scales[j - 2] & 0xc0c0) >> 2);
        aux[1] = ((scales[j + 2] >> 4) & 0x0f0f) | ((scales[j - 0] & 0xc0c0) >> 2);
    }
    const uint8_t * sc = (const uint8_t *) aux;
    const uint8_t * m = sc + 2;
#pragma unroll
    for (int i = 0; i < MKLLM_QR4_K; ++i) {
        const mkllm_block_q8_1 * bq8i = bq8_1 + bq8_offset + i;
        d8[i] = __low2float(bq8i->ds);
        const int * q8 = (const int *) bq8i->qs + ((iqs / 2) % 4);
        u[2 * i + 0] = q8[0];
        u[2 * i + 1] = q8[4];
    }
    return mkllm_vec_dot_q4_K_q8_1_impl_vmmq(v, u, sc, m, bq4_K->dm, d8);
}

static __device__ __forceinline__ float mkllm_vec_dot_q5_K_q8_1(
        const void * vbq, const mkllm_block_q8_1 * bq8_1, int kbx, int iqs) {
    const mkllm_block_q5_K * bq5_K = (const mkllm_block_q5_K *) vbq + kbx;
    int vl[2];
    int vh[2];
    int u[2 * MKLLM_QR5_K];
    float d8[MKLLM_QR5_K];
    const int bq8_offset = MKLLM_QR5_K * ((iqs / 2) / (MKLLM_QI8_1 / 2));
    const int * ql = (const int *) (bq5_K->qs + 16 * bq8_offset + 4 * ((iqs / 2) % 4));
    const int * qh = (const int *) (bq5_K->qh + 4 * ((iqs / 2) % 4));
    vl[0] = ql[0];
    vl[1] = ql[4];
    vh[0] = qh[0] >> bq8_offset;
    vh[1] = qh[4] >> bq8_offset;
    const uint16_t * scales = (const uint16_t *) bq5_K->scales;
    uint16_t aux[2];
    const int j = bq8_offset / 2;
    if (j < 2) {
        aux[0] = scales[j + 0] & 0x3f3f;
        aux[1] = scales[j + 2] & 0x3f3f;
    } else {
        aux[0] = ((scales[j + 2] >> 0) & 0x0f0f) | ((scales[j - 2] & 0xc0c0) >> 2);
        aux[1] = ((scales[j + 2] >> 4) & 0x0f0f) | ((scales[j - 0] & 0xc0c0) >> 2);
    }
    const uint8_t * sc = (const uint8_t *) aux;
    const uint8_t * m = sc + 2;
#pragma unroll
    for (int i = 0; i < MKLLM_QR5_K; ++i) {
        const mkllm_block_q8_1 * bq8i = bq8_1 + bq8_offset + i;
        d8[i] = __low2float(bq8i->ds);
        const int * q8 = (const int *) bq8i->qs + ((iqs / 2) % 4);
        u[2 * i + 0] = q8[0];
        u[2 * i + 1] = q8[4];
    }
    float sumf_d = 0.0f;
    float sumf_m = 0.0f;
#pragma unroll
    for (int i = 0; i < MKLLM_QR5_K; ++i) {
        const int vl0i = (vl[0] >> (4 * i)) & 0x0F0F0F0F;
        const int vl1i = (vl[1] >> (4 * i)) & 0x0F0F0F0F;
        const int vh0i = ((vh[0] >> i) << 4) & 0x10101010;
        const int vh1i = ((vh[1] >> i) << 4) & 0x10101010;
        const int v0i = vl0i | vh0i;
        const int v1i = vl1i | vh1i;
        const int dot1 = mkllm_dp4a(v0i, u[2 * i + 0], mkllm_dp4a(v1i, u[2 * i + 1], 0));
        const int dot2 = mkllm_dp4a(0x01010101, u[2 * i + 0], mkllm_dp4a(0x01010101, u[2 * i + 1], 0));
        sumf_d += d8[i] * (dot1 * sc[i]);
        sumf_m += d8[i] * (dot2 * m[i]);
    }
    const float2 dm5f = __half22float2(bq5_K->dm);
    return dm5f.x * sumf_d - dm5f.y * sumf_m;
}

static __device__ __forceinline__ float mkllm_vec_dot_q6_K_q8_1(
        const void * vbq, const mkllm_block_q8_1 * bq8_1, int kbx, int iqs) {
    const mkllm_block_q6_K * bq6_K = (const mkllm_block_q6_K *) vbq + kbx;
    const int bq8_offset = 2 * MKLLM_QR6_K * (iqs / (MKLLM_QI6_K / 2))
        + (iqs % (MKLLM_QI6_K / 2)) / (MKLLM_QI6_K / 4);
    const int scale_offset = (MKLLM_QI6_K / 4) * (iqs / (MKLLM_QI6_K / 2))
        + (iqs % (MKLLM_QI6_K / 2)) / (MKLLM_QI6_K / 8);
    const int vh_shift = 2 * ((iqs % (MKLLM_QI6_K / 2)) / (MKLLM_QI6_K / 4));
    const int vl = mkllm_get_int_b2(bq6_K->ql, iqs);
    const int vh = mkllm_get_int_b2(
        bq6_K->qh, (MKLLM_QI6_K / 4) * (iqs / (MKLLM_QI6_K / 2)) + iqs % (MKLLM_QI6_K / 4)) >> vh_shift;
    const int8_t * scales = bq6_K->scales + scale_offset;
    int u[MKLLM_QR6_K];
    float d8[MKLLM_QR6_K];
#pragma unroll
    for (int i = 0; i < MKLLM_QR6_K; ++i) {
        u[i] = mkllm_get_int_b4(bq8_1[bq8_offset + 2 * i].qs, iqs % MKLLM_QI8_1);
        d8[i] = __low2float(bq8_1[bq8_offset + 2 * i].ds);
    }
    float sumf = 0.0f;
#pragma unroll
    for (int i = 0; i < MKLLM_QR6_K; ++i) {
        const int sc = scales[4 * i];
        const int vil = (vl >> (4 * i)) & 0x0F0F0F0F;
        const int vih = ((vh >> (4 * i)) << 4) & 0x30303030;
#if defined(__CUDA_ARCH__) && __CUDA_ARCH__ >= 300
        const int vi = __vsubss4(vil | vih, 0x20202020);
#else
        const int vi = (vil | vih) - 0x20202020;
#endif
        sumf += d8[i] * (mkllm_dp4a(vi, u[i], 0) * sc);
    }
    return __half2float(bq6_K->d) * sumf;
}

template <int KIND>
static __device__ __forceinline__ float mkllm_vec_dot_q_templ(
        const void * vx, const mkllm_block_q8_1 * y, int kbx, int iqs) {
    if constexpr (KIND == MKLLM_QUANT_Q5K) {
        return mkllm_vec_dot_q5_K_q8_1(vx, y, kbx, iqs);
    } else if constexpr (KIND == MKLLM_QUANT_Q6K) {
        return mkllm_vec_dot_q6_K_q8_1(vx, y, kbx, iqs);
    } else {
        return mkllm_vec_dot_q4_K_q8_1(vx, y, kbx, iqs);
    }
}

// llama.cpp mmvq.cu:389-589 mul_mat_vec_q specialized to
// ncols_dst=1, rows_per_cuda_block=1, GENERIC nwarps=4, no ids.
// tmp[ncols][rpb], warp_reduce_sum<warp_size>, write threadIdx.x < rpb.
template <int KIND, int FUSE>
static __global__ void __launch_bounds__(MKLLM_MMVQ_NWARPS * 32, 1) mkllm_mmvq_q81_kernel(
        const void * __restrict__ vx,
        const void * __restrict__ vgate,
        const mkllm_block_q8_1 * __restrict__ vy,
        float * __restrict__ dst,
        int k, int n, size_t src0_row_bytes, size_t gate_row_bytes) {
    constexpr int ncols_dst = 1;
    constexpr int rows_per_cuda_block = 1;
    constexpr int nwarps = MKLLM_MMVQ_NWARPS;
    constexpr int warp_size = 32;
    constexpr int vdr = (KIND == MKLLM_QUANT_Q6K) ? MKLLM_VDR_Q6_K_MMVQ : MKLLM_VDR_Q4_K_MMVQ;
    constexpr int qi = (KIND == MKLLM_QUANT_Q6K) ? MKLLM_QI6_K : MKLLM_QI4_K;
    constexpr int qk = QK_K;
    constexpr int blk_bytes = KIND == MKLLM_QUANT_Q6K ? (int) sizeof(mkllm_block_q6_K)
        : KIND == MKLLM_QUANT_Q5K ? (int) sizeof(mkllm_block_q5_K)
        : (int) sizeof(mkllm_block_q4_K);
    const int tid = warp_size * (int) threadIdx.y + (int) threadIdx.x;
    const int row0 = rows_per_cuda_block * (int) blockIdx.x;
    const int blocks_per_row_x = k / qk;
    constexpr int blocks_per_iter = vdr * nwarps * warp_size / qi;
    const int stride_row_x = (int) (src0_row_bytes / (size_t) blk_bytes);
    const int kbx_offset = row0 * stride_row_x;
    const mkllm_block_q8_1 * y = vy;
    float tmp[ncols_dst][rows_per_cuda_block] = {{0.0f}};
    float tmp_gate[ncols_dst][rows_per_cuda_block] = {{0.0f}};
    for (int kbx = tid / (qi / vdr); kbx < blocks_per_row_x; kbx += blocks_per_iter) {
        const int kby = kbx * (qk / MKLLM_QK8_1);
        const int kqs = vdr * (tid % (qi / vdr));
#pragma unroll
        for (int j = 0; j < ncols_dst; ++j) {
#pragma unroll
            for (int i = 0; i < rows_per_cuda_block; ++i) {
                tmp[j][i] += mkllm_vec_dot_q_templ<KIND>(
                    vx, &y[j + kby], kbx_offset + i * stride_row_x + kbx, kqs);
                if constexpr (FUSE) {
                    const int stride_gate = (int) (gate_row_bytes / (size_t) blk_bytes);
                    tmp_gate[j][i] += mkllm_vec_dot_q_templ<KIND>(
                        vgate, &y[j + kby], row0 * stride_gate + i * stride_gate + kbx, kqs);
                }
            }
        }
    }
    __shared__ float tmp_shared[nwarps - 1 > 0 ? nwarps - 1 : 1][ncols_dst][rows_per_cuda_block][warp_size];
    __shared__ float tmp_shared_gate[(FUSE && (nwarps - 1 > 0)) ? nwarps - 1 : 1][ncols_dst][rows_per_cuda_block][warp_size];
    if (threadIdx.y > 0) {
#pragma unroll
        for (int j = 0; j < ncols_dst; ++j) {
#pragma unroll
            for (int i = 0; i < rows_per_cuda_block; ++i) {
                tmp_shared[threadIdx.y - 1][j][i][threadIdx.x] = tmp[j][i];
                if constexpr (FUSE) {
                    tmp_shared_gate[threadIdx.y - 1][j][i][threadIdx.x] = tmp_gate[j][i];
                }
            }
        }
    }
    __syncthreads();
    if (threadIdx.y > 0) {
        return;
    }
    dst += row0;
#pragma unroll
    for (int j = 0; j < ncols_dst; ++j) {
#pragma unroll
        for (int i = 0; i < rows_per_cuda_block; ++i) {
#pragma unroll
            for (int l = 0; l < nwarps - 1; ++l) {
                tmp[j][i] += tmp_shared[l][j][i][threadIdx.x];
                if constexpr (FUSE) {
                    tmp_gate[j][i] += tmp_shared_gate[l][j][i][threadIdx.x];
                }
            }
            tmp[j][i] = mkllm_warp_reduce_sum32(tmp[j][i]);
            if constexpr (FUSE) {
                tmp_gate[j][i] = mkllm_warp_reduce_sum32(tmp_gate[j][i]);
            }
        }
        // mmvq.cu:554: threadIdx.x < rows_per_cuda_block
        if (threadIdx.x < rows_per_cuda_block
                && (rows_per_cuda_block == 1 || uint32_t(row0 + threadIdx.x) < (uint32_t) n)) {
            float result = tmp[j][threadIdx.x];
            if constexpr (FUSE) {
                const float gate_value = tmp_gate[j][threadIdx.x];
                result *= gate_value / (1.0f + expf(-gate_value));
            }
            dst[j + threadIdx.x] = result;
        }
    }
}
#endif

static cudaError_t mkllm_launch_mmvq_q81(
        int kind, int fuse,
        const void * src0, const void * gate, const void * y, float * dst,
        int K, int N, int M, size_t src0_row_bytes, size_t gate_row_bytes,
        cudaStream_t stream) {
    if (K <= 0 || N <= 0 || (K % QK_K) != 0 || M < 1 || M > MMVQ_MAX_BATCH_SIZE) {
        return cudaErrorInvalidValue;
    }
    if (fuse && (gate == nullptr || gate_row_bytes == 0)) {
        return cudaErrorInvalidValue;
    }
    // Bytes per ggml block (34 for q8_0, 18 for iq4_nl, 256-value super-block
    // otherwise) — mul_mat_vec_q's stride_row_x counts BLOCKS of src0's type.
    const int blk_bytes = mkllm_kind_block_bytes(kind);
    const int per_256 = mkllm_kind_bytes_per_256(kind);
    if (blk_bytes == 0 || per_256 == 0) {
        return cudaErrorInvalidValue;
    }
    if ((mkllm_kind_route_mask(kind) & MKLLM_ROUTE_MMVQ) == 0) {
        return cudaErrorInvalidValue;
    }
    if (src0_row_bytes < (size_t) (K / QK_K) * (size_t) per_256
            || (src0_row_bytes % (size_t) blk_bytes) != 0) {
        return cudaErrorInvalidValue;
    }
    if (fuse && gate_row_bytes != src0_row_bytes) {
        return cudaErrorInvalidValue;
    }
    const int stride_row_x = (int) (src0_row_bytes / (size_t) blk_bytes);
    cudaError_t launch_err = cudaSuccess;
    // One arm per kind, no permissive default: a kind added here but not to
    // the other switches must fail loudly instead of decoding as Q4_K.
#define MKLLM_MMVQ_KIND(KIND, TYPE)                                            \
    case KIND:                                                                 \
        launch_err = fuse                                                      \
            ? mkllm_launch_mmvq_official<TYPE, true>(                          \
                src0, gate, y, dst, K, N, M, stride_row_x, stream)             \
            : mkllm_launch_mmvq_official<TYPE, false>(                         \
                src0, nullptr, y, dst, K, N, M, stride_row_x, stream);         \
        break;
    switch (kind) {
        MKLLM_MMVQ_KIND(MKLLM_QUANT_Q4K,   GGML_TYPE_Q4_K)
        MKLLM_MMVQ_KIND(MKLLM_QUANT_Q5K,   GGML_TYPE_Q5_K)
        MKLLM_MMVQ_KIND(MKLLM_QUANT_Q6K,   GGML_TYPE_Q6_K)
        MKLLM_MMVQ_KIND(MKLLM_QUANT_Q80,   GGML_TYPE_Q8_0)
        MKLLM_MMVQ_KIND(MKLLM_QUANT_Q3K,   GGML_TYPE_Q3_K)
        MKLLM_MMVQ_KIND(MKLLM_QUANT_IQ4XS, GGML_TYPE_IQ4_XS)
        MKLLM_MMVQ_KIND(MKLLM_QUANT_IQ4NL, GGML_TYPE_IQ4_NL)
        MKLLM_MMVQ_KIND(MKLLM_QUANT_IQ3S,  GGML_TYPE_IQ3_S)
        default: return cudaErrorInvalidValue;
    }
#undef MKLLM_MMVQ_KIND
    if (launch_err != cudaSuccess) {
        return launch_err;
    }
    return cudaGetLastError();
}

extern "C" cudaError_t mkllm_mmv_quant_q81(
        int kind,
        const void * src0, const void * y, float * dst,
        int K, int N, int M,
        size_t src0_row_bytes, size_t dst_col_elems,
        cudaStream_t stream) {
    (void) dst_col_elems;
    return mkllm_launch_mmvq_q81(
        kind, 0, src0, nullptr, y, dst, K, N, M, src0_row_bytes, 0, stream);
}

extern "C" cudaError_t mkllm_mmv_quant_q81_swiglu(
        int kind,
        const void * up, const void * gate, const void * y, float * dst,
        int K, int N, int M,
        size_t up_row_bytes, size_t gate_row_bytes,
        cudaStream_t stream) {
    return mkllm_launch_mmvq_q81(
        kind, 1, up, gate, y, dst, K, N, M, up_row_bytes, gate_row_bytes, stream);
}

#if 0
// Retired unfaithful Q8_1 MMV (f32 scale, 2 warps) starts here.
template <int KIND>
static __device__ __forceinline__ float mkllm_q81_dot_sb(
        const uint8_t * __restrict__ blk,
        const int8_t * __restrict__ q8,
        const float * __restrict__ d8,
        int lane) {
    float acc = 0.0f;
    if (KIND == MKLLM_QUANT_Q4K || KIND == MKLLM_QUANT_Q5K) {
        const float d    = __half2float(((const __half *) blk)[0]);
        const float dmin = __half2float(((const __half *) blk)[1]);
        const uint8_t * scales = blk + 4;
        const uint8_t * qs = blk + (KIND == MKLLM_QUANT_Q4K ? 16 : 48);
        const int group = lane >> 3;
        const int byte4 = (lane & 7) * 4;
        const uint32_t packed = *(const uint32_t *) (qs + group * 32 + byte4);
        const int sb_lo = 2 * group;
        uint8_t sc_lo, m_lo, sc_hi, m_hi;
        mkllm_scale_min_k4(sb_lo, scales, &sc_lo, &m_lo);
        mkllm_scale_min_k4(sb_lo + 1, scales, &sc_hi, &m_hi);
        int q_lo = (int) (packed & 0x0F0F0F0Fu);
        int q_hi = (int) ((packed >> 4) & 0x0F0F0F0Fu);
        if (KIND == MKLLM_QUANT_Q5K) {
            int hi_lo = 0;
            int hi_hi = 0;
#pragma unroll
            for (int i = 0; i < 4; i++) {
                const uint8_t qh = blk[16 + byte4 + i];
                if (qh & (1u << sb_lo)) hi_lo |= 16 << (8 * i);
                if (qh & (2u << sb_lo)) hi_hi |= 16 << (8 * i);
            }
            q_lo |= hi_lo;
            q_hi |= hi_hi;
        }
        const int lo_at = group * 64 + byte4;
        const int u_lo = *(const int *) (q8 + lo_at);
        const int u_hi = *(const int *) (q8 + lo_at + 32);
        const float ds_lo = d8[group * 2];
        const float ds_hi = d8[group * 2 + 1];
        const int dot_lo = mkllm_dp4a(q_lo, u_lo, 0);
        const int dot_hi = mkllm_dp4a(q_hi, u_hi, 0);
        const int sum_lo = mkllm_dp4a(0x01010101, u_lo, 0);
        const int sum_hi = mkllm_dp4a(0x01010101, u_hi, 0);
        acc += d * (float) sc_lo * ds_lo * (float) dot_lo
            - dmin * (float) m_lo * ds_lo * (float) sum_lo;
        acc += d * (float) sc_hi * ds_hi * (float) dot_hi
            - dmin * (float) m_hi * ds_hi * (float) sum_hi;
    } else if (KIND == MKLLM_QUANT_Q6K) {
        const float d = __half2float(*(const __half *) (blk + 208));
        const int l0 = lane * 8;
        const int n = l0 >> 7;
        const int r = l0 & 127;
        const int group = r >> 5;
        const int lo0 = r & 31;
        const int is = lo0 / 16;
        const uint8_t * ql = blk + n * 64;
        const uint8_t * qh = blk + 128 + n * 32;
        const int8_t * sc = (const int8_t *) (blk + 192) + n * 8;
        int8_t qv[8];
#pragma unroll
        for (int i = 0; i < 8; i++) {
            const int lo = lo0 + i;
            int q;
            switch (group) {
                case 0:
                    q = (int) ((ql[lo] & 0x0F) | ((qh[lo] & 3) << 4));
                    break;
                case 1:
                    q = (int) ((ql[lo + 32] & 0x0F) | (((qh[lo] >> 2) & 3) << 4));
                    break;
                case 2:
                    q = (int) ((ql[lo] >> 4) | (((qh[lo] >> 4) & 3) << 4));
                    break;
                default:
                    q = (int) ((ql[lo + 32] >> 4) | (((qh[lo] >> 6) & 3) << 4));
                    break;
            }
            qv[i] = (int8_t) (q - 32);
        }
        const int u0 = *(const int *) (q8 + l0);
        const int u1 = *(const int *) (q8 + l0 + 4);
        const int v0 = *(const int *) qv;
        const int v1 = *(const int *) (qv + 4);
        const int dot = mkllm_dp4a(v0, u0, mkllm_dp4a(v1, u1, 0));
        acc += d * (float) sc[is + 2 * group] * d8[l0 / MKLLM_Q81_GS] * (float) dot;
    } else {
        const int sub = lane >> 2;
        const int b4 = (lane & 3) * 8;
        const uint8_t * q8w = blk + sub * 34;
        const float dw = __half2float(*(const __half *) q8w);
        const int at = sub * 32 + b4;
        const int w0 = mkllm_ld_i32_b2(q8w + 2 + b4);
        const int w1 = mkllm_ld_i32_b2(q8w + 2 + b4 + 4);
        const int u0 = *(const int *) (q8 + at);
        const int u1 = *(const int *) (q8 + at + 4);
        const int dot = mkllm_dp4a(w0, u0, mkllm_dp4a(w1, u1, 0));
        acc += dw * d8[at / MKLLM_Q81_GS] * (float) dot;
    }
    return acc;
}

template <int KIND>
static __global__ void mkllm_mmv_qk_q81_kernel(
        const uint8_t * __restrict__ src0,
        const int8_t * __restrict__ q8,
        const float * __restrict__ d8,
        float * __restrict__ dst,
        int K, int N,
        size_t src0_row_bytes, size_t dst_col_elems) {
    // Ada decode: dim3(32, 2) — two warps cooperate on one output row.
    const int row = (int) blockIdx.x;
    if (row >= N) return;
    const int lane = (int) threadIdx.x;
    const int warp = (int) threadIdx.y;
    const uint8_t * row_bytes = src0 + (size_t) row * src0_row_bytes;
    const int blocks = K / QK_K;
    float acc = 0.0f;

    for (int b = warp; b < blocks; b += MKLLM_MMVQ_NWARPS) {
        const uint8_t * blk = row_bytes + (size_t) b * mkllm_quant_block_bytes_dev(KIND);
        const int xbase = b * QK_K;
        acc += mkllm_q81_dot_sb<KIND>(blk, q8 + xbase, d8 + xbase / MKLLM_Q81_GS, lane);
    }

    for (int off = 16; off > 0; off >>= 1) {
        acc += __shfl_down_sync(0xffffffff, acc, off);
    }
    __shared__ float warp_sum[MKLLM_MMVQ_NWARPS];
    if (lane == 0) {
        warp_sum[warp] = acc;
    }
    __syncthreads();
    if (warp == 0 && lane == 0) {
        float total = 0.0f;
#pragma unroll
        for (int w = 0; w < MKLLM_MMVQ_NWARPS; w++) {
            total += warp_sum[w];
        }
        dst[row] = total;
    }
    (void) dst_col_elems;
}

extern "C" cudaError_t mkllm_mmv_quant_q81(
        int kind,
        const void * src0, const int8_t * q8, const float * d8, float * dst,
        int K, int N,
        size_t src0_row_bytes, size_t dst_col_elems,
        cudaStream_t stream) {
    if (K <= 0 || N <= 0 || (K % QK_K) != 0) {
        return cudaErrorInvalidValue;
    }
    if (kind < MKLLM_QUANT_Q4K || kind > MKLLM_QUANT_Q80) {
        return cudaErrorInvalidValue;
    }
    dim3 block(32, MKLLM_MMVQ_NWARPS);
    dim3 grid(N);
    switch (kind) {
        case MKLLM_QUANT_Q4K:
            mkllm_mmv_qk_q81_kernel<MKLLM_QUANT_Q4K><<<grid, block, 0, stream>>>(
                (const uint8_t *) src0, q8, d8, dst, K, N, src0_row_bytes, dst_col_elems);
            break;
        case MKLLM_QUANT_Q5K:
            mkllm_mmv_qk_q81_kernel<MKLLM_QUANT_Q5K><<<grid, block, 0, stream>>>(
                (const uint8_t *) src0, q8, d8, dst, K, N, src0_row_bytes, dst_col_elems);
            break;
        case MKLLM_QUANT_Q80:
            mkllm_mmv_qk_q81_kernel<MKLLM_QUANT_Q80><<<grid, block, 0, stream>>>(
                (const uint8_t *) src0, q8, d8, dst, K, N, src0_row_bytes, dst_col_elems);
            break;
        case MKLLM_QUANT_Q6K:
            mkllm_mmv_qk_q81_kernel<MKLLM_QUANT_Q6K><<<grid, block, 0, stream>>>(
                (const uint8_t *) src0, q8, d8, dst, K, N, src0_row_bytes, dst_col_elems);
            break;
        default:
            return cudaErrorInvalidValue;
    }
    return cudaGetLastError();
}
#endif

// Batched Q8_1 quant: one warp per (column, 32-value block). q is packed [M, K].
static __global__ void mkllm_quantize_q81_batched_kernel(
        const float * __restrict__ x, int8_t * __restrict__ q,
        float * __restrict__ d, int nblk, size_t col_stride) {
    const int blk = (int) blockIdx.x;
    const int col = (int) blockIdx.y;
    const int lane = (int) threadIdx.x;
    if (blk >= nblk || lane >= MKLLM_Q81_GS) {
        return;
    }
    const float xi = x[(size_t) col * col_stride + (size_t) blk * MKLLM_Q81_GS + lane];
    const float amax = mkllm_warp_reduce_max32(fabsf(xi));
    const float d_blk = amax / 127.0f;
    const int8_t qi = (amax == 0.0f) ? 0 : (int8_t) roundf(xi / d_blk);
    q[((size_t) col * (size_t) nblk + (size_t) blk) * MKLLM_Q81_GS + lane] = qi;
    if (lane == 0) {
        d[(size_t) col * (size_t) nblk + (size_t) blk] = d_blk;
    }
}

extern "C" cudaError_t mkllm_quantize_q81_batched(
        const float * x, int8_t * q, float * d, int k, int m,
        size_t col_stride, cudaStream_t stream) {
    if (k <= 0 || m <= 0 || (k % MKLLM_Q81_GS) != 0) {
        return cudaErrorInvalidValue;
    }
    const int nblk = k / MKLLM_Q81_GS;
    dim3 grid(nblk, m);
    mkllm_quantize_q81_batched_kernel<<<grid, MKLLM_Q81_GS, 0, stream>>>(
        x, q, d, nblk, col_stride);
    return cudaGetLastError();
}

// Unused compiled-only helper for the disabled Q8_1 prefill MMQ path.
template <int KIND>
static __device__ __forceinline__ float mkllm_q81_dot_sb(
        const uint8_t * __restrict__ blk,
        const int8_t * __restrict__ q8,
        const float * __restrict__ d8,
        int lane) {
    (void) blk;
    (void) q8;
    (void) d8;
    (void) lane;
    return 0.0f;
}

// Prefill packed MMQ: Q4/Q5/Q6/Q8_0 x Q8_1, no BF16 expansion.
// One warp per output row, BM=8 act columns sharing a super-block in smem.
#define MKLLM_MMQ_Q81_BM 8
#define MKLLM_MMQ_Q81_ROWS 8

template <int KIND>
static __global__ void mkllm_mmq_q81_kernel(
        const uint8_t * __restrict__ src0,
        const int8_t * __restrict__ q8,
        const float * __restrict__ d8,
        float * __restrict__ dst,
        int K, int N, int M,
        size_t src0_row_bytes, size_t dst_col_elems) {
    const int row = (int) blockIdx.x * MKLLM_MMQ_Q81_ROWS + (int) threadIdx.y;
    const int col0 = (int) blockIdx.y * MKLLM_MMQ_Q81_BM;
    const int lane = (int) threadIdx.x;
    const int tid = (int) threadIdx.y * 32 + lane;
    const int nblk = K / MKLLM_Q81_GS;
    const int blocks = K / QK_K;

    __shared__ int8_t smem_q[MKLLM_MMQ_Q81_BM * QK_K];
    __shared__ float smem_d[MKLLM_MMQ_Q81_BM * (QK_K / MKLLM_Q81_GS)];

    float acc[MKLLM_MMQ_Q81_BM];
#pragma unroll
    for (int c = 0; c < MKLLM_MMQ_Q81_BM; c++) {
        acc[c] = 0.0f;
    }

    const uint8_t * row_bytes = (row < N) ? src0 + (size_t) row * src0_row_bytes : src0;
    for (int b = 0; b < blocks; b++) {
        const int xbase = b * QK_K;
        const int dbase = xbase / MKLLM_Q81_GS;
        // 256 threads load 8*256 Q8 bytes (8 iters) + 8*8 scales.
        for (int p = tid; p < MKLLM_MMQ_Q81_BM * QK_K; p += MKLLM_MMQ_Q81_ROWS * 32) {
            const int c = p / QK_K;
            const int kk = p - c * QK_K;
            const int col = col0 + c;
            int8_t v = 0;
            if (col < M) {
                v = q8[(size_t) col * (size_t) K + (size_t) xbase + kk];
            }
            smem_q[p] = v;
        }
        if (tid < MKLLM_MMQ_Q81_BM * (QK_K / MKLLM_Q81_GS)) {
            const int c = tid / (QK_K / MKLLM_Q81_GS);
            const int db = tid - c * (QK_K / MKLLM_Q81_GS);
            const int col = col0 + c;
            float dv = 0.0f;
            if (col < M) {
                dv = d8[(size_t) col * (size_t) nblk + (size_t) dbase + db];
            }
            smem_d[tid] = dv;
        }
        __syncthreads();

        if (row < N) {
            const uint8_t * blk = row_bytes + (size_t) b * mkllm_quant_block_bytes_dev(KIND);
#pragma unroll
            for (int c = 0; c < MKLLM_MMQ_Q81_BM; c++) {
                acc[c] += mkllm_q81_dot_sb<KIND>(
                    blk,
                    smem_q + c * QK_K,
                    smem_d + c * (QK_K / MKLLM_Q81_GS),
                    lane);
            }
        }
        __syncthreads();
    }

    if (row >= N) {
        return;
    }
#pragma unroll
    for (int c = 0; c < MKLLM_MMQ_Q81_BM; c++) {
        const int col = col0 + c;
        if (col >= M) {
            break;
        }
        float total = acc[c];
        for (int off = 16; off > 0; off >>= 1) {
            total += __shfl_down_sync(0xffffffff, total, off);
        }
        if (lane == 0) {
            dst[(size_t) col * dst_col_elems + row] = total;
        }
    }
}

extern "C" cudaError_t mkllm_mmq_quant_q81(
        int kind,
        const void * src0, const int8_t * q8, const float * d8, float * dst,
        int K, int N, int M,
        size_t src0_row_bytes, size_t dst_col_elems,
        cudaStream_t stream) {
    if (K <= 0 || N <= 0 || M <= 0 || (K % QK_K) != 0) {
        return cudaErrorInvalidValue;
    }
    if (kind < MKLLM_QUANT_Q4K || kind > MKLLM_QUANT_Q80) {
        return cudaErrorInvalidValue;
    }
    dim3 block(32, MKLLM_MMQ_Q81_ROWS);
    dim3 grid((N + MKLLM_MMQ_Q81_ROWS - 1) / MKLLM_MMQ_Q81_ROWS,
              (M + MKLLM_MMQ_Q81_BM - 1) / MKLLM_MMQ_Q81_BM);
    switch (kind) {
        case MKLLM_QUANT_Q4K:
            mkllm_mmq_q81_kernel<MKLLM_QUANT_Q4K><<<grid, block, 0, stream>>>(
                (const uint8_t *) src0, q8, d8, dst, K, N, M, src0_row_bytes, dst_col_elems);
            break;
        case MKLLM_QUANT_Q5K:
            mkllm_mmq_q81_kernel<MKLLM_QUANT_Q5K><<<grid, block, 0, stream>>>(
                (const uint8_t *) src0, q8, d8, dst, K, N, M, src0_row_bytes, dst_col_elems);
            break;
        case MKLLM_QUANT_Q80:
            mkllm_mmq_q81_kernel<MKLLM_QUANT_Q80><<<grid, block, 0, stream>>>(
                (const uint8_t *) src0, q8, d8, dst, K, N, M, src0_row_bytes, dst_col_elems);
            break;
        case MKLLM_QUANT_Q6K:
            mkllm_mmq_q81_kernel<MKLLM_QUANT_Q6K><<<grid, block, 0, stream>>>(
                (const uint8_t *) src0, q8, d8, dst, K, N, M, src0_row_bytes, dst_col_elems);
            break;
        default:
            return cudaErrorInvalidValue;
    }
    return cudaGetLastError();
}

// f32/f16 weight mat-vec twin (small M), same output convention.
template <typename W>
static __global__ void mkllm_mmv_float_kernel(
        const W * __restrict__ src0, const float * __restrict__ src1,
        float * __restrict__ dst,
        int K, int N, int M,
        size_t src0_row_elems, size_t src1_col_elems, size_t dst_col_elems) {
    const int row = blockIdx.x * blockDim.y + threadIdx.y;
    if (row >= N) return;
    const int lane = threadIdx.x;
    const W * w = src0 + (size_t) row * src0_row_elems;
    for (int col = 0; col < M; col++) {
        const float * x = src1 + (size_t) col * src1_col_elems;
        float acc = 0.0f;
        for (int l = lane; l < K; l += 32) {
            acc += (float) w[l] * x[l];
        }
        for (int off = 16; off > 0; off >>= 1) {
            acc += __shfl_down_sync(0xffffffff, acc, off);
        }
        if (lane == 0) {
            dst[(size_t) col * dst_col_elems + row] = acc;
        }
    }
}

// llama.cpp mmvf.cu:123-154 + :301-369, F32 ncols_dst=1, no fusion/ids.
// One block per output row; float2 loads; block_size picked to minimize
// (K/2)/block_size iterations (mmvf.cu:426-438). Decode alpha/beta are
// N=48 K=5120 F32 — the naive warp-per-row kernel above is ~33us each.
template <int BLOCK>
static __global__ void mkllm_mmvf_f32_m1(
        const float * __restrict__ x, const float * __restrict__ y,
        float * __restrict__ dst, int ncols2, int nrows, size_t stride_row) {
    const int row = blockIdx.x;
    if (row >= nrows) {
        return;
    }
    const int tid = threadIdx.x;
    const float2 * x2 = (const float2 *) (x + (size_t) row * stride_row);
    const float2 * y2 = (const float2 *) y;
    __shared__ float buf_iw[32];
    float sumf = 0.0f;
    for (int col2 = tid; col2 < ncols2; col2 += BLOCK) {
        const float2 tmpx = x2[col2];
        const float2 tmpy = y2[col2];
        sumf += tmpx.x * tmpy.x;
        sumf += tmpx.y * tmpy.y;
    }
    sumf = mkllm_warp_reduce_sum32(sumf);
    if (BLOCK > 32) {
        if (tid < 32) {
            buf_iw[tid] = 0.0f;
        }
        __syncthreads();
        buf_iw[tid / 32] = sumf;
        __syncthreads();
        if (tid < 32) {
            sumf = mkllm_warp_reduce_sum32(buf_iw[tid]);
        }
    }
    if (tid == 0) {
        dst[row] = sumf;
    }
}

extern "C" cudaError_t mkllm_mmv_f32(
        const float * src0, const float * src1, float * dst,
        int K, int N, int M,
        size_t src0_row_elems, size_t src1_col_elems, size_t dst_col_elems,
        cudaStream_t stream) {
    // mmvf.cu:414: ncols % 2 == 0. M=1 decode path only; M>1 keeps the
    // existing warp-per-row kernel (opcheck mmv_f32 uses M=3).
    if (M == 1 && (K % 2) == 0 && src0_row_elems % 2 == 0
        && src1_col_elems == (size_t) K) {
        int block_size_best = 32;
        int niter_best = (K + 2 * 32 - 1) / (2 * 32);
        for (int block_size = 64; block_size <= 256; block_size += 32) {
            const int niter = (K + 2 * block_size - 1) / (2 * block_size);
            if (niter < niter_best) {
                niter_best = niter;
                block_size_best = block_size;
            }
        }
        const int ncols2 = K / 2;
        const dim3 grid((unsigned) N);
        switch (block_size_best) {
            case 32:
                mkllm_mmvf_f32_m1<32><<<grid, 32, 0, stream>>>(
                    src0, src1, dst, ncols2, N, src0_row_elems);
                break;
            case 64:
                mkllm_mmvf_f32_m1<64><<<grid, 64, 0, stream>>>(
                    src0, src1, dst, ncols2, N, src0_row_elems);
                break;
            case 96:
                mkllm_mmvf_f32_m1<96><<<grid, 96, 0, stream>>>(
                    src0, src1, dst, ncols2, N, src0_row_elems);
                break;
            case 128:
                mkllm_mmvf_f32_m1<128><<<grid, 128, 0, stream>>>(
                    src0, src1, dst, ncols2, N, src0_row_elems);
                break;
            case 160:
                mkllm_mmvf_f32_m1<160><<<grid, 160, 0, stream>>>(
                    src0, src1, dst, ncols2, N, src0_row_elems);
                break;
            case 192:
                mkllm_mmvf_f32_m1<192><<<grid, 192, 0, stream>>>(
                    src0, src1, dst, ncols2, N, src0_row_elems);
                break;
            case 224:
                mkllm_mmvf_f32_m1<224><<<grid, 224, 0, stream>>>(
                    src0, src1, dst, ncols2, N, src0_row_elems);
                break;
            default:
                mkllm_mmvf_f32_m1<256><<<grid, 256, 0, stream>>>(
                    src0, src1, dst, ncols2, N, src0_row_elems);
                break;
        }
        return cudaGetLastError();
    }
    dim3 block(32, 4);
    dim3 grid((N + 3) / 4);
    mkllm_mmv_float_kernel<float><<<grid, block, 0, stream>>>(
        src0, src1, dst, K, N, M, src0_row_elems, src1_col_elems, dst_col_elems);
    return cudaGetLastError();
}

extern "C" cudaError_t mkllm_mmv_f16(
        const void * src0, const float * src1, float * dst,
        int K, int N, int M,
        size_t src0_row_elems, size_t src1_col_elems, size_t dst_col_elems,
        cudaStream_t stream) {
    dim3 block(32, 4);
    dim3 grid((N + 3) / 4);
    mkllm_mmv_float_kernel<__half><<<grid, block, 0, stream>>>(
        (const __half *) src0, src1, dst, K, N, M, src0_row_elems, src1_col_elems, dst_col_elems);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Slab dequantization to bf16 (prefill GEMM path; transient slabs only).
//
// K-quant payload bytes encode several output values. Decode those values in
// one thread so scale metadata and payload loads are shared rather than doing
// one full block decode per scalar. Keep __float2bfloat16 here: the executor's
// parity contract uses CUDA's round-to-nearest-even BF16 conversion.
// ---------------------------------------------------------------------------

static __global__ void mkllm_dequant_q4k_rows_bf16_kernel(
        const uint8_t * __restrict__ src, __nv_bfloat16 * __restrict__ dst,
        int rows, int K, size_t src_row_bytes) {
    const int row = blockIdx.y;
    const int sb = blockIdx.x;
    const int t = threadIdx.x;       // one thread per packed q byte
    if (row >= rows || t >= 128) return;
    const int group = t >> 5;        // 64-value group, 0..3
    const int l = t & 31;
    const uint8_t * blk = src + (size_t) row * src_row_bytes + (size_t) sb * 144;
    const float d = __half2float(((const __half *) blk)[0]);
    const float dmin = __half2float(((const __half *) blk)[1]);
    const uint8_t * scales = blk + 4;
    const uint8_t q = blk[16 + 32 * group + l];
    uint8_t sc0, m0, sc1, m1;
    mkllm_scale_min_k4(2 * group, scales, &sc0, &m0);
    mkllm_scale_min_k4(2 * group + 1, scales, &sc1, &m1);
    __nv_bfloat16 * out = dst + (size_t) row * K + (size_t) sb * QK_K + group * 64 + l;
    out[0] = __float2bfloat16(
        d * (float) sc0 * (float) (q & 0x0F) - dmin * (float) m0);
    out[32] = __float2bfloat16(
        d * (float) sc1 * (float) (q >> 4) - dmin * (float) m1);
}

static __global__ void mkllm_dequant_q5k_rows_bf16_kernel(
        const uint8_t * __restrict__ src, __nv_bfloat16 * __restrict__ dst,
        int rows, int K, size_t src_row_bytes) {
    const int row = blockIdx.y;
    const int sb = blockIdx.x;
    const int t = threadIdx.x;
    if (row >= rows || t >= 128) return;
    const int group = t >> 5;
    const int l = t & 31;
    const int is0 = 2 * group;
    const int is1 = is0 + 1;
    const uint8_t * blk = src + (size_t) row * src_row_bytes + (size_t) sb * 176;
    const float d = __half2float(((const __half *) blk)[0]);
    const float dmin = __half2float(((const __half *) blk)[1]);
    const uint8_t * scales = blk + 4;
    const uint8_t * qh = blk + 16;
    const uint8_t q = blk[48 + 32 * group + l];
    uint8_t sc0, m0, sc1, m1;
    mkllm_scale_min_k4(is0, scales, &sc0, &m0);
    mkllm_scale_min_k4(is1, scales, &sc1, &m1);
    const float q0 = (float) (q & 0x0F) + ((qh[l] & (1u << is0)) ? 16.0f : 0.0f);
    const float q1 = (float) (q >> 4) + ((qh[l] & (1u << is1)) ? 16.0f : 0.0f);
    __nv_bfloat16 * out = dst + (size_t) row * K + (size_t) sb * QK_K + group * 64 + l;
    out[0] = __float2bfloat16(d * (float) sc0 * q0 - dmin * (float) m0);
    out[32] = __float2bfloat16(d * (float) sc1 * q1 - dmin * (float) m1);
}

static __global__ void mkllm_dequant_q6k_rows_bf16_kernel(
        const uint8_t * __restrict__ src, __nv_bfloat16 * __restrict__ dst,
        int rows, int K, size_t src_row_bytes) {
    const int row = blockIdx.y;
    const int sb = blockIdx.x;
    const int t = threadIdx.x;       // one thread produces four values
    if (row >= rows || t >= 64) return;
    const int half = t >> 5;
    const int l = t & 31;
    const int is = l >> 4;
    const uint8_t * blk = src + (size_t) row * src_row_bytes + (size_t) sb * 210;
    const uint8_t * ql = blk + half * 64;
    const uint8_t * qh = blk + 128 + half * 32;
    const int8_t * sc = (const int8_t *) (blk + 192) + half * 8;
    const float d = __half2float(*(const __half *) (blk + 208));
    const int q0 = (int) ((int8_t) ((ql[l] & 0x0F) | ((qh[l] & 3) << 4))) - 32;
    const int q1 = (int) ((int8_t) ((ql[l + 32] & 0x0F) | (((qh[l] >> 2) & 3) << 4))) - 32;
    const int q2 = (int) ((int8_t) ((ql[l] >> 4) | (((qh[l] >> 4) & 3) << 4))) - 32;
    const int q3 = (int) ((int8_t) ((ql[l + 32] >> 4) | (((qh[l] >> 6) & 3) << 4))) - 32;
    __nv_bfloat16 * out = dst + (size_t) row * K + (size_t) sb * QK_K + half * 128 + l;
    out[0] = __float2bfloat16(d * (float) sc[is] * (float) q0);
    out[32] = __float2bfloat16(d * (float) sc[is + 2] * (float) q1);
    out[64] = __float2bfloat16(d * (float) sc[is + 4] * (float) q2);
    out[96] = __float2bfloat16(d * (float) sc[is + 6] * (float) q3);
}

static __global__ void mkllm_dequant_q80_rows_bf16_kernel(
        const uint8_t * __restrict__ src, __nv_bfloat16 * __restrict__ dst,
        int rows, int K, size_t src_row_bytes) {
    const int row = blockIdx.y;
    const int idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (row >= rows || idx >= K) return;
    const uint8_t * row_bytes = src + (size_t) row * src_row_bytes;
    const uint8_t * blk = row_bytes + (size_t) (idx / QK_K) * 272;
    dst[(size_t) row * K + idx] = __float2bfloat16(mkllm_deq_q80_at(blk, idx % QK_K));
}

extern "C" cudaError_t mkllm_dequant_rows_bf16(
        int kind, const void * src, void * dst, int rows, int K,
        size_t src_row_bytes, cudaStream_t stream) {
    dim3 grid(K / QK_K, rows);
    switch (kind) {
        case MKLLM_QUANT_Q4K:
            mkllm_dequant_q4k_rows_bf16_kernel<<<grid, 128, 0, stream>>>(
                (const uint8_t *) src, (__nv_bfloat16 *) dst, rows, K, src_row_bytes);
            break;
        case MKLLM_QUANT_Q5K:
            mkllm_dequant_q5k_rows_bf16_kernel<<<grid, 128, 0, stream>>>(
                (const uint8_t *) src, (__nv_bfloat16 *) dst, rows, K, src_row_bytes);
            break;
        case MKLLM_QUANT_Q80:
            mkllm_dequant_q80_rows_bf16_kernel<<<grid, 256, 0, stream>>>(
                (const uint8_t *) src, (__nv_bfloat16 *) dst, rows, K, src_row_bytes);
            break;
        case MKLLM_QUANT_Q6K:
            mkllm_dequant_q6k_rows_bf16_kernel<<<grid, 64, 0, stream>>>(
                (const uint8_t *) src, (__nv_bfloat16 *) dst, rows, K, src_row_bytes);
            break;
        // Q3_K and the IQ codebook kinds: the ported llama.cpp convert.cu
        // kernels in iq_convert.cuh, adapted to this row-strided layout.
        case MKLLM_QUANT_Q3K:
        case MKLLM_QUANT_IQ4XS:
        case MKLLM_QUANT_IQ4NL:
        case MKLLM_QUANT_IQ3S:
            return mkllm_dequant_iq_rows_bf16(
                kind, src, dst, rows, K, src_row_bytes, stream);
        default:
            return cudaErrorInvalidValue;
    }
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Fused packed-quant MMQ: dst[N, M] = W[K, N]^T . X[K, M] without writing a
// global dequant slab. Super-blocks are decoded with the same Q4_K/Q5_K/Q6_K
// /Q8_0 helpers as the CPU reference, then rounded with __float2bfloat16
// (RN-even) so the observable GEMM path matches the previous slab+cublas
// numerics. Activations are rounded the same way in registers. Accumulation
// is f32 on BF16 tensor cores (sm_80+).
//
// Tile: BN=64 weight rows, BM=32 activation columns, BK=256 (one super-block).
// Eight warps: 4 along N x 2 along M, each owning a 16x16 WMMA fragment.
// ---------------------------------------------------------------------------

#define MKLLM_MMQ_BN 64
#define MKLLM_MMQ_BM 32
#define MKLLM_MMQ_BK 256
#define MKLLM_MMQ_SLD 256

template <int KIND>
static __global__ void mkllm_mmq_qk_kernel(
        const uint8_t * __restrict__ src0, const float * __restrict__ src1,
        float * __restrict__ dst,
        int K, int N, int M,
        size_t src0_row_bytes, size_t src1_col_elems, size_t dst_col_elems) {
    const int n0 = (int) blockIdx.x * MKLLM_MMQ_BN;
    const int m0 = (int) blockIdx.y * MKLLM_MMQ_BM;
    const int tid = (int) threadIdx.x;
    const int block_bytes = mkllm_quant_block_bytes_dev(KIND);
    const int rows = min(MKLLM_MMQ_BN, N - n0);
    const int cols = min(MKLLM_MMQ_BM, M - m0);

    extern __shared__ char smem_raw[];
    __nv_bfloat16 * smem_w = (__nv_bfloat16 *) smem_raw;
    __nv_bfloat16 * smem_x = smem_w + MKLLM_MMQ_BN * MKLLM_MMQ_SLD;

#if defined(__CUDA_ARCH__) && __CUDA_ARCH__ >= 800
    using namespace nvcuda;
    wmma::fragment<wmma::matrix_a, 16, 16, 16, __nv_bfloat16, wmma::row_major> a_frag;
    wmma::fragment<wmma::matrix_b, 16, 16, 16, __nv_bfloat16, wmma::col_major> b_frag;
    wmma::fragment<wmma::accumulator, 16, 16, 16, float> c_frag;
    wmma::fill_fragment(c_frag, 0.0f);
    const int warp = tid >> 5;
    const int warp_n = warp >> 1;
    const int warp_m = warp & 1;
#else
    float acc[8];
#pragma unroll
    for (int i = 0; i < 8; i++) acc[i] = 0.0f;
    const int lane = tid & 31;
    const int warp = tid >> 5;
    const int warp_n = warp >> 1;
    const int warp_m = warp & 1;
#endif

    const int superblocks = K / QK_K;
    for (int sb = 0; sb < superblocks; sb++) {
        const int k0 = sb * QK_K;
        // Packed decode into SMEM (same formula as slab dequant, not per-l scalar).
        if (KIND == MKLLM_QUANT_Q4K || KIND == MKLLM_QUANT_Q5K) {
            for (int p = tid; p < MKLLM_MMQ_BN * 128; p += 256) {
                const int row = p / 128;
                const int t = p - row * 128;
                const int group = t >> 5;
                const int l = t & 31;
                __nv_bfloat16 v0 = __float2bfloat16(0.0f);
                __nv_bfloat16 v1 = __float2bfloat16(0.0f);
                if (row < rows) {
                    const uint8_t * blk = src0 + (size_t) (n0 + row) * src0_row_bytes
                        + (size_t) sb * block_bytes;
                    const float d = __half2float(((const __half *) blk)[0]);
                    const float dmin = __half2float(((const __half *) blk)[1]);
                    const uint8_t * scales = blk + 4;
                    const uint8_t q = blk[(KIND == MKLLM_QUANT_Q4K ? 16 : 48)
                        + 32 * group + l];
                    uint8_t sc0, m0, sc1, m1;
                    mkllm_scale_min_k4(2 * group, scales, &sc0, &m0);
                    mkllm_scale_min_k4(2 * group + 1, scales, &sc1, &m1);
                    float q0 = (float) (q & 0x0F);
                    float q1 = (float) (q >> 4);
                    if (KIND == MKLLM_QUANT_Q5K) {
                        const uint8_t qh = blk[16 + l];
                        q0 += (qh & (1u << (2 * group))) ? 16.0f : 0.0f;
                        q1 += (qh & (1u << (2 * group + 1))) ? 16.0f : 0.0f;
                    }
                    v0 = __float2bfloat16(d * (float) sc0 * q0 - dmin * (float) m0);
                    v1 = __float2bfloat16(d * (float) sc1 * q1 - dmin * (float) m1);
                }
                smem_w[row * MKLLM_MMQ_SLD + group * 64 + l] = v0;
                smem_w[row * MKLLM_MMQ_SLD + group * 64 + l + 32] = v1;
            }
        } else if (KIND == MKLLM_QUANT_Q6K) {
            for (int p = tid; p < MKLLM_MMQ_BN * 64; p += 256) {
                const int row = p / 64;
                const int t = p - row * 64;
                const int half = t >> 5;
                const int l = t & 31;
                const int is = l >> 4;
                __nv_bfloat16 v0 = __float2bfloat16(0.0f);
                __nv_bfloat16 v1 = __float2bfloat16(0.0f);
                __nv_bfloat16 v2 = __float2bfloat16(0.0f);
                __nv_bfloat16 v3 = __float2bfloat16(0.0f);
                if (row < rows) {
                    const uint8_t * blk = src0 + (size_t) (n0 + row) * src0_row_bytes
                        + (size_t) sb * block_bytes;
                    const uint8_t * ql = blk + half * 64;
                    const uint8_t * qh = blk + 128 + half * 32;
                    const int8_t * sc = (const int8_t *) (blk + 192) + half * 8;
                    const float d = __half2float(*(const __half *) (blk + 208));
                    const int q0 = (int) ((int8_t) ((ql[l] & 0x0F) | ((qh[l] & 3) << 4))) - 32;
                    const int q1 = (int) ((int8_t) ((ql[l + 32] & 0x0F) | (((qh[l] >> 2) & 3) << 4))) - 32;
                    const int q2 = (int) ((int8_t) ((ql[l] >> 4) | (((qh[l] >> 4) & 3) << 4))) - 32;
                    const int q3 = (int) ((int8_t) ((ql[l + 32] >> 4) | (((qh[l] >> 6) & 3) << 4))) - 32;
                    v0 = __float2bfloat16(d * (float) sc[is] * (float) q0);
                    v1 = __float2bfloat16(d * (float) sc[is + 2] * (float) q1);
                    v2 = __float2bfloat16(d * (float) sc[is + 4] * (float) q2);
                    v3 = __float2bfloat16(d * (float) sc[is + 6] * (float) q3);
                }
                smem_w[row * MKLLM_MMQ_SLD + half * 128 + l] = v0;
                smem_w[row * MKLLM_MMQ_SLD + half * 128 + l + 32] = v1;
                smem_w[row * MKLLM_MMQ_SLD + half * 128 + l + 64] = v2;
                smem_w[row * MKLLM_MMQ_SLD + half * 128 + l + 96] = v3;
            }
        } else {
            for (int p = tid; p < MKLLM_MMQ_BN * QK_K; p += 256) {
                const int row = p / QK_K;
                const int col = p - row * QK_K;
                __nv_bfloat16 v = __float2bfloat16(0.0f);
                if (row < rows) {
                    const uint8_t * blk = src0 + (size_t) (n0 + row) * src0_row_bytes
                        + (size_t) sb * block_bytes;
                    v = __float2bfloat16(mkllm_deq_q80_at(blk, col));
                }
                smem_w[row * MKLLM_MMQ_SLD + col] = v;
            }
        }
        // Cast BM x 256 activations to RN-even BF16.
        for (int p = 0; p < (MKLLM_MMQ_BM * QK_K) / 256; p++) {
            const int idx = tid + p * 256;
            const int col = idx / QK_K;
            const int kk = idx - col * QK_K;
            __nv_bfloat16 v = __float2bfloat16(0.0f);
            if (col < cols) {
                v = __float2bfloat16(src1[(size_t) (m0 + col) * src1_col_elems + k0 + kk]);
            }
            smem_x[col * MKLLM_MMQ_SLD + kk] = v;
        }
        __syncthreads();

#if defined(__CUDA_ARCH__) && __CUDA_ARCH__ >= 800
#pragma unroll
        for (int kk = 0; kk < QK_K; kk += 16) {
            wmma::load_matrix_sync(
                a_frag, smem_w + (warp_n * 16) * MKLLM_MMQ_SLD + kk, MKLLM_MMQ_SLD);
            wmma::load_matrix_sync(
                b_frag, smem_x + (warp_m * 16) * MKLLM_MMQ_SLD + kk, MKLLM_MMQ_SLD);
            wmma::mma_sync(c_frag, a_frag, b_frag, c_frag);
        }
#else
        const int n_base = warp_n * 16;
        const int m_base = warp_m * 16;
#pragma unroll
        for (int i = 0; i < 8; i++) {
            const int nr = n_base + (i >> 2) * 8 + (lane >> 2);
            const int mc = m_base + (i & 3) * 4 + (lane & 3);
            float sum = acc[i];
            const __nv_bfloat16 * wr = smem_w + nr * MKLLM_MMQ_SLD;
            const __nv_bfloat16 * xr = smem_x + mc * MKLLM_MMQ_SLD;
            for (int kk = 0; kk < QK_K; kk++) {
                sum += (float) wr[kk] * (float) xr[kk];
            }
            acc[i] = sum;
        }
#endif
        __syncthreads();
    }

    // Safe edge store: reuse the weight tile SMEM so partial N/M never
    // writes past dst. Combined static+dynamic shared stays under 48 KiB.
#if defined(__CUDA_ARCH__) && __CUDA_ARCH__ >= 800
    float * c_tile = (float *) smem_raw;
    wmma::store_matrix_sync(
        c_tile + (warp_m * 16) * MKLLM_MMQ_BN + (warp_n * 16),
        c_frag, MKLLM_MMQ_BN, wmma::mem_col_major);
    __syncthreads();
    for (int p = tid; p < MKLLM_MMQ_BN * MKLLM_MMQ_BM; p += 256) {
        const int nr = p % MKLLM_MMQ_BN;
        const int mc = p / MKLLM_MMQ_BN;
        if (nr < rows && mc < cols) {
            dst[(size_t) (m0 + mc) * dst_col_elems + (n0 + nr)] =
                c_tile[mc * MKLLM_MMQ_BN + nr];
        }
    }
#else
    for (int i = 0; i < 8; i++) {
        const int nr = warp_n * 16 + (i >> 2) * 8 + (lane >> 2);
        const int mc = warp_m * 16 + (i & 3) * 4 + (lane & 3);
        if (nr < rows && mc < cols) {
            dst[(size_t) (m0 + mc) * dst_col_elems + (n0 + nr)] = acc[i];
        }
    }
#endif
}

extern "C" cudaError_t mkllm_mmq_quant(
        int kind,
        const void * src0, const float * src1, float * dst,
        int K, int N, int M,
        size_t src0_row_bytes, size_t src1_col_elems, size_t dst_col_elems,
        cudaStream_t stream) {
    if (K <= 0 || N <= 0 || M <= 0 || (K % QK_K) != 0) {
        return cudaErrorInvalidValue;
    }
    dim3 block(256);
    dim3 grid((N + MKLLM_MMQ_BN - 1) / MKLLM_MMQ_BN, (M + MKLLM_MMQ_BM - 1) / MKLLM_MMQ_BM);
    const size_t shared = (size_t) (MKLLM_MMQ_BN + MKLLM_MMQ_BM) * MKLLM_MMQ_SLD
        * sizeof(__nv_bfloat16);
    switch (kind) {
        case MKLLM_QUANT_Q4K:
            mkllm_mmq_qk_kernel<MKLLM_QUANT_Q4K><<<grid, block, shared, stream>>>(
                (const uint8_t *) src0, src1, dst, K, N, M,
                src0_row_bytes, src1_col_elems, dst_col_elems);
            break;
        case MKLLM_QUANT_Q5K:
            mkllm_mmq_qk_kernel<MKLLM_QUANT_Q5K><<<grid, block, shared, stream>>>(
                (const uint8_t *) src0, src1, dst, K, N, M,
                src0_row_bytes, src1_col_elems, dst_col_elems);
            break;
        case MKLLM_QUANT_Q80:
            mkllm_mmq_qk_kernel<MKLLM_QUANT_Q80><<<grid, block, shared, stream>>>(
                (const uint8_t *) src0, src1, dst, K, N, M,
                src0_row_bytes, src1_col_elems, dst_col_elems);
            break;
        case MKLLM_QUANT_Q6K:
            mkllm_mmq_qk_kernel<MKLLM_QUANT_Q6K><<<grid, block, shared, stream>>>(
                (const uint8_t *) src0, src1, dst, K, N, M,
                src0_row_bytes, src1_col_elems, dst_col_elems);
            break;
        default:
            return cudaErrorInvalidValue;
    }
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Q4_K prefill MMQ, llama.cpp b10430 (4c1a0af40d88c7fbb3b15c85bf2e8016d1d5b64c).
// Ported from ggml-cuda/mmq.cuh, mmq-load-tiles.cuh, mmq-vec-dot.cuh,
// mma.cuh, quantize.cu. Copyright (c) ggml authors. MIT license.
//
// Fixed tile: I=128, J=128, K=256, 256 threads / 8 warps.
// Same NVIDIA GENERIC table on 3090/4090/5090 (sm80+ m16n8k32).
// Compiled on the target box (MAKEPAD_GGML_CUDA_ARCH); not a fat binary.
// block_q8_1_mmq is K-block-major across tokens; DS4 stores half2(d, sum)
// where sum is the unquantized float partial (not reconstructed from q8).
// Q4_K tile: padded stride 76, s8 nibbles + half2(d*sc, -dmin*m).
// vec_dot is llama.cpp q8_1 x q8_1 MMA: one m16n8k32 per expanded
// 32-value group. Host rejects M tails.
// Q4_K J=128 is default-on in the llama executor (MKLLM_DISABLE_Q4K_MMQ=1
// restores slab). Q6_K J=128 is the matching llama.cpp MMA path for the
// Q6 weights in Q4_K_M. Neither touches libs/ggml CUDA.
// ---------------------------------------------------------------------------

#define MKLLM_QK8_1 32
#define MKLLM_QK8_1_MMQ 128
#define MKLLM_QI8_1 8
#define MKLLM_MMQ_I 128
#define MKLLM_MMQ_J 128
#define MKLLM_MMQ_NWARPS 8
#define MKLLM_MMQ_TILE_NE_K 32
#define MKLLM_MMQ_TILE_Y_K (MKLLM_MMQ_TILE_NE_K + MKLLM_MMQ_TILE_NE_K / MKLLM_QI8_1)
#define MKLLM_MMQ_SRAM_STRIDE 76
#define MKLLM_MMQ_ITER_K 256

struct mkllm_block_q8_1_mmq {
    union {
        float d4[4];
        half2 ds4[4];
    };
    int8_t qs[MKLLM_QK8_1_MMQ];
};

static_assert(sizeof(mkllm_block_q8_1_mmq) == 144, "block_q8_1_mmq size");

static __device__ __forceinline__ int mkllm_unpack_scales_q45_K(const int * scales, const int ksc) {
    return ((scales[(ksc % 2) + (ksc != 0)] >> (4 * (ksc & (ksc / 2)))) & 0x0F0F0F0F)
        | ((scales[ksc / 2] >> (2 * (ksc % 2))) & 0x30303030);
}

struct mkllm_tile_16x8 {
    int x[4];
};
struct mkllm_tile_8x8 {
    int x[2];
};

static __device__ __forceinline__ int mkllm_tile_c_i(int l) {
    return ((l / 2) * 8) + ((int) threadIdx.x / 4);
}

static __device__ __forceinline__ int mkllm_tile_c_j(int l) {
    return (((int) threadIdx.x % 4) * 2) + (l % 2);
}

// llama.cpp mma.cuh:791-798 load_ldmatrix<tile<16,8>>:
// xs = xs0 + (tid % I)*stride + (tid / I)*(J/2), I=16 J=8.
static __device__ __forceinline__ void mkllm_ldmatrix_a(mkllm_tile_16x8 & t, const int * xs0, int stride) {
    const int * xs = (const int *) xs0
        + ((int) threadIdx.x % 16) * stride + ((int) threadIdx.x / 16) * 4;
    asm volatile("ldmatrix.sync.aligned.m8n8.x4.b16 {%0, %1, %2, %3}, [%4];"
        : "=r"(t.x[0]), "=r"(t.x[1]), "=r"(t.x[2]), "=r"(t.x[3])
        : "l"(xs));
}

static __device__ __forceinline__ void mkllm_load_generic_b(mkllm_tile_8x8 & t, const int * xs0, int stride) {
    const int i = (int) threadIdx.x / 4;
    const int j0 = (int) threadIdx.x % 4;
    t.x[0] = xs0[i * stride + j0];
    t.x[1] = xs0[i * stride + 4 + j0];
}

static __device__ __forceinline__ void mkllm_mma_s8_16x8x32(
        mkllm_tile_16x8 & d, const mkllm_tile_16x8 & a, const mkllm_tile_8x8 & b) {
#if __CUDA_ARCH__ >= 800
    // b10430 mma.cuh ~942-948: Ampere/Ada m16n8k32. Incomplete Turing
    // 2x m8n8k16 is not equivalent; first port is sm80+ only.
    asm volatile(
        "mma.sync.aligned.m16n8k32.row.col.s32.s8.s8.s32"
        " {%0, %1, %2, %3}, {%4, %5, %6, %7}, {%8, %9}, {%0, %1, %2, %3};"
        : "+r"(d.x[0]), "+r"(d.x[1]), "+r"(d.x[2]), "+r"(d.x[3])
        : "r"(a.x[0]), "r"(a.x[1]), "r"(a.x[2]), "r"(a.x[3]), "r"(b.x[0]), "r"(b.x[1]));
#else
    (void) a;
    (void) b;
    d.x[0] = d.x[1] = d.x[2] = d.x[3] = 0;
#endif
}

#if 0
// Homemade MMQ process/load/dot/write retired: official mul_mat_q_process_tile.
static __device__ __forceinline__ void mkllm_mmq_q4k_load_tiles(
        const char * __restrict__ x, int * __restrict__ x_tile,
        int kbx0, int i_max, int stride) {
    int * x_qs = x_tile;
    half2 * x_dm = (half2 *) (x_qs + 2 * MKLLM_MMQ_TILE_NE_K);
#pragma unroll
    for (int i0 = 0; i0 < MKLLM_MMQ_I; i0 += MKLLM_MMQ_NWARPS) {
        // llama.cpp load_tiles_q4_K need_check=false (mmq.cuh:2073-2075):
        // no i=min(i,i_max); Qwen N is a multiple of 128.
        const int i = i0 + (int) threadIdx.y;
        (void) i_max;
        const uint8_t * blk = (const uint8_t *) x + ((size_t) (kbx0 + i * stride) * 144);
        const int txi = (int) threadIdx.x;
        const int qs0 = ((const int *) (blk + 16))[txi];
        x_qs[i * MKLLM_MMQ_SRAM_STRIDE + 16 * (txi / 8) + (txi % 8) + 0] = (qs0 >> 0) & 0x0F0F0F0F;
        x_qs[i * MKLLM_MMQ_SRAM_STRIDE + 16 * (txi / 8) + (txi % 8) + 8] = (qs0 >> 4) & 0x0F0F0F0F;
    }
#pragma unroll
    for (int i0 = 0; i0 < MKLLM_MMQ_I; i0 += MKLLM_MMQ_NWARPS * 16) {
        const int i = (i0 + (int) threadIdx.y * 16 + (int) threadIdx.x / 2) % MKLLM_MMQ_I;
        const uint8_t * blk = (const uint8_t *) x + ((size_t) (kbx0 + i * stride) * 144);
        const int * scales = (const int *) (blk + 4);
        const int ksc = (int) threadIdx.x % 2;
        const int sc32 = mkllm_unpack_scales_q45_K(scales, ksc + 0);
        const int m32 = mkllm_unpack_scales_q45_K(scales, ksc + 2);
        const uint8_t * sc8 = (const uint8_t *) &sc32;
        const uint8_t * m8 = (const uint8_t *) &m32;
        const half2 dm = __hmul2(((const half2 *) blk)[0], make_half2(1.0f, -1.0f));
#pragma unroll
        for (int l = 0; l < 4; ++l) {
            x_dm[i * MKLLM_MMQ_SRAM_STRIDE + 4 * ksc + l] =
                __hmul2(dm, make_half2((float) sc8[l], (float) m8[l]));
        }
    }
}

static __device__ __forceinline__ void mkllm_mmq_q4k_vec_dot(
        const int * __restrict__ x, const int * __restrict__ y,
        float * __restrict__ sum, int k00) {
    constexpr int ntx = 2;
    constexpr int rows_per_warp = 32;
    y += ((int) threadIdx.y % ntx) * (8 * MKLLM_MMQ_TILE_Y_K);
    const int * x_qs = x;
    const half2 * x_dm = (const half2 *) x_qs + 2 * MKLLM_MMQ_TILE_NE_K;
    const int * y_qs = y + 4;
    const half2 * y_dm = (const half2 *) y;
    mkllm_tile_16x8 A[ntx][MKLLM_MMQ_TILE_NE_K / MKLLM_QI8_1];
    float2 dmA[ntx][2][MKLLM_MMQ_TILE_NE_K / MKLLM_QI8_1];
    const int i0 = ((int) threadIdx.y / ntx) * rows_per_warp;
#pragma unroll
    for (int n = 0; n < ntx; ++n) {
#pragma unroll
        for (int k01 = 0; k01 < MKLLM_MMQ_TILE_NE_K; k01 += MKLLM_QI8_1) {
            mkllm_ldmatrix_a(A[n][k01 / MKLLM_QI8_1],
                x_qs + (i0 + n * 16) * MKLLM_MMQ_SRAM_STRIDE + (k00 + k01),
                MKLLM_MMQ_SRAM_STRIDE);
        }
#pragma unroll
        for (int l = 0; l < 2; ++l) {
            const int i = i0 + n * 16 + mkllm_tile_c_i(2 * l);
#pragma unroll
            for (int k01 = 0; k01 < MKLLM_MMQ_TILE_NE_K; k01 += MKLLM_QI8_1) {
                dmA[n][l][k01 / MKLLM_QI8_1] =
                    __half22float2(x_dm[i * MKLLM_MMQ_SRAM_STRIDE + (k00 + k01) / MKLLM_QI8_1]);
            }
        }
    }
#pragma unroll
    for (int j0 = 0; j0 < MKLLM_MMQ_J; j0 += ntx * 8) {
#pragma unroll
        for (int k01 = 0; k01 < MKLLM_MMQ_TILE_NE_K; k01 += MKLLM_QI8_1) {
            mkllm_tile_8x8 B;
            float2 dsB[2];
            mkllm_load_generic_b(B, y_qs + j0 * MKLLM_MMQ_TILE_Y_K + k01, MKLLM_MMQ_TILE_Y_K);
#pragma unroll
            for (int l = 0; l < 2; ++l) {
                const int j = j0 + mkllm_tile_c_j(l);
                dsB[l] = __half22float2(y_dm[j * MKLLM_MMQ_TILE_Y_K + k01 / MKLLM_QI8_1]);
            }
#pragma unroll
            for (int n = 0; n < ntx; ++n) {
                mkllm_tile_16x8 C;
                C.x[0] = C.x[1] = C.x[2] = C.x[3] = 0;
                mkllm_mma_s8_16x8x32(C, A[n][k01 / MKLLM_QI8_1], B);
#pragma unroll
                for (int l = 0; l < 4; ++l) {
                    sum[(j0 / 8 + n) * 4 + l] +=
                        dmA[n][l / 2][k01 / MKLLM_QI8_1].x * dsB[l % 2].x * (float) C.x[l];
                    sum[(j0 / 8 + n) * 4 + l] +=
                        dmA[n][l / 2][k01 / MKLLM_QI8_1].y * dsB[l % 2].y;
                }
            }
        }
    }
}

static __device__ __forceinline__ void mkllm_mmq_q4k_process_tile(
        const char * __restrict__ x, const int * __restrict__ y,
        int * __restrict__ tile_x, int * __restrict__ tile_y, float * __restrict__ sum,
        int offset_x, int j0, int i_max, int stride_row_x, int y_stride,
        int kb0_start, int kb0_stop) {
#pragma unroll
    for (int s = 0; s < 64; ++s) {
        sum[s] = 0.0f;
    }
    for (int kb0 = kb0_start; kb0 < kb0_stop; ++kb0) {
        mkllm_mmq_q4k_load_tiles(x, tile_x, offset_x + kb0, i_max, stride_row_x);
        {
            const int * by0 = y + (j0 * (int) (sizeof(mkllm_block_q8_1_mmq) / sizeof(int)))
                + (kb0 * 2) * y_stride;
#pragma unroll
            for (int l0 = 0; l0 < MKLLM_MMQ_J * MKLLM_MMQ_TILE_Y_K; l0 += 256) {
                const int l = l0 + (int) threadIdx.y * 32 + (int) threadIdx.x;
                tile_y[l] = by0[l];
            }
        }
        __syncthreads();
        mkllm_mmq_q4k_vec_dot(tile_x, tile_y, sum, 0);
        __syncthreads();
        {
            const int * by0 = y + (j0 * (int) (sizeof(mkllm_block_q8_1_mmq) / sizeof(int)))
                + (kb0 * 2 + 1) * y_stride;
#pragma unroll
            for (int l0 = 0; l0 < MKLLM_MMQ_J * MKLLM_MMQ_TILE_Y_K; l0 += 256) {
                const int l = l0 + (int) threadIdx.y * 32 + (int) threadIdx.x;
                tile_y[l] = by0[l];
            }
        }
        __syncthreads();
        mkllm_mmq_q4k_vec_dot(tile_x, tile_y, sum, MKLLM_MMQ_TILE_NE_K);
        __syncthreads();
    }
}

// llama.cpp mmq.cuh:3224 mmq_write_back_mma<…, need_check=false>:
// j>j_max stays (always), i>i_max only if need_check. Official Qwen path
// is N%128==0 and M%128==0 so both checks are dead; omit them so the
// store is a straight indexed write like the false specialization.
static __device__ __forceinline__ void mkllm_mmq_write_j128(
        const float * __restrict__ sum, float * __restrict__ dst,
        int i0, int j0, int i_max, int j_max, int stride_col_dst) {
    (void) i_max;
    (void) j_max;
    const int ntx = 2;
    const int wi0 = ((int) threadIdx.y / ntx) * 32;
#pragma unroll
    for (int jj = 0; jj < MKLLM_MMQ_J; jj += 16) {
#pragma unroll
        for (int nt = 0; nt < ntx; ++nt) {
#pragma unroll
            for (int l = 0; l < 4; ++l) {
                const int j = jj + ((int) threadIdx.y % ntx) * 8 + mkllm_tile_c_j(l);
                const int i = wi0 + nt * 16 + mkllm_tile_c_i(l);
                dst[(size_t) (j0 + j) * (size_t) stride_col_dst + (size_t) (i0 + i)] =
                    sum[(jj / 8 + nt) * 4 + l];
            }
        }
    }
}

static __global__ void __launch_bounds__(256, 1) mkllm_mmq_q4k_j128_kernel(
        const char * __restrict__ x, const int * __restrict__ y, float * __restrict__ dst,
        int k, int n, int m, int stride_row_x, int stride_col_dst) {
    const int i0 = (int) blockIdx.x * MKLLM_MMQ_I;
    const int j0 = (int) blockIdx.y * MKLLM_MMQ_J;
    const int i_max = n - i0 - 1;
    const int j_max = m - j0 - 1;
    extern __shared__ int smem[];
    int * tile_y = smem + MKLLM_MMQ_J;
    int * tile_x = tile_y + MKLLM_MMQ_J * MKLLM_MMQ_TILE_Y_K;
    float sum[64];
    const int nblocks = k / QK_K;
    const int y_stride = m * (int) (sizeof(mkllm_block_q8_1_mmq) / sizeof(int));
    mkllm_mmq_q4k_process_tile(
        x, y, tile_x, tile_y, sum, i0 * stride_row_x, j0, i_max, stride_row_x, y_stride,
        0, nblocks);
    mkllm_mmq_write_j128(sum, dst, i0, j0, i_max, j_max, stride_col_dst);
}

// llama.cpp mul_mat_q stream-K (mmq.cuh ~3524): nsm blocks walk a 1D
// (it, jt, kb0) space. Complete tiles write dst; a trailing partial K
// slice goes to tmp_fixup and is added by the fixup kernel.
static __global__ void __launch_bounds__(256, 1) mkllm_mmq_q4k_streamk_kernel(
        const char * __restrict__ x, const int * __restrict__ y, float * __restrict__ dst,
        float * __restrict__ tmp_fixup,
        int k, int n, int m, int stride_row_x, int stride_col_dst) {
    const int ntx = m / MKLLM_MMQ_J;
    const int nty = (n + MKLLM_MMQ_I - 1) / MKLLM_MMQ_I;
    const int nblocks = k / QK_K;
    const int64_t total = (int64_t) ntx * nty * nblocks;
    int64_t kbc = (int64_t) blockIdx.x * total / gridDim.x;
    int64_t kbc_stop = (int64_t) (blockIdx.x + 1) * total / gridDim.x;
    int kb0_start = (int) (kbc % nblocks);
    int kb0_stop = (int) min((int64_t) nblocks, (int64_t) kb0_start + kbc_stop - kbc);
    extern __shared__ int smem[];
    int * tile_y = smem + MKLLM_MMQ_J;
    int * tile_x = tile_y + MKLLM_MMQ_J * MKLLM_MMQ_TILE_Y_K;
    float sum[64];
    const int y_stride = m * (int) (sizeof(mkllm_block_q8_1_mmq) / sizeof(int));
    while (kbc < kbc_stop && kb0_stop == nblocks) {
        const int it = (int) (kbc / ((int64_t) ntx * nblocks));
        const int jt = (int) ((kbc / nblocks) % ntx);
        const int i0 = it * MKLLM_MMQ_I;
        const int j0 = jt * MKLLM_MMQ_J;
        mkllm_mmq_q4k_process_tile(
            x, y, tile_x, tile_y, sum, i0 * stride_row_x, j0, n - i0 - 1,
            stride_row_x, y_stride, kb0_start, kb0_stop);
        mkllm_mmq_write_j128(sum, dst, i0, j0, n - i0 - 1, m - j0 - 1, stride_col_dst);
        kbc += nblocks;
        kbc -= kbc % nblocks;
        kb0_start = 0;
        kb0_stop = (int) min((int64_t) nblocks, kbc_stop - kbc);
    }
    if (kbc >= kbc_stop) {
        return;
    }
    const int it = (int) (kbc / ((int64_t) ntx * nblocks));
    const int jt = (int) ((kbc / nblocks) % ntx);
    const int i0 = it * MKLLM_MMQ_I;
    const int j0 = jt * MKLLM_MMQ_J;
    mkllm_mmq_q4k_process_tile(
        x, y, tile_x, tile_y, sum, i0 * stride_row_x, j0, n - i0 - 1,
        stride_row_x, y_stride, kb0_start, kb0_stop);
    mkllm_mmq_write_j128(
        sum, tmp_fixup + (size_t) blockIdx.x * MKLLM_MMQ_I * MKLLM_MMQ_J,
        0, 0, MKLLM_MMQ_I - 1, MKLLM_MMQ_J - 1, MKLLM_MMQ_I);
}

static __global__ void mkllm_mmq_streamk_fixup_kernel(
        const float * __restrict__ tmp_last_tile, float * __restrict__ dst,
        int k, int n, int m, int stride_col_dst) {
    const int ntx = m / MKLLM_MMQ_J;
    const int nty = (n + MKLLM_MMQ_I - 1) / MKLLM_MMQ_I;
    const int nblocks = k / QK_K;
    const int64_t total = (int64_t) ntx * nty * nblocks;
    const int bidx0 = (int) blockIdx.x;
    int64_t kbc0 = (int64_t) bidx0 * total / gridDim.x;
    int64_t kbc0_stop = (int64_t) (bidx0 + 1) * total / gridDim.x;
    const bool did_not_have_any_data = kbc0 == kbc0_stop;
    const bool wrote_beginning_of_tile = (kbc0 % nblocks) == 0;
    const bool did_not_write_last = (kbc0 / nblocks == kbc0_stop / nblocks) && (kbc0_stop % nblocks != 0);
    if (did_not_have_any_data || wrote_beginning_of_tile || did_not_write_last) {
        return;
    }
    float sum[64];
#pragma unroll
    for (int s = 0; s < 64; ++s) {
        sum[s] = 0.0f;
    }
    bool any_fixup = false;
    int64_t bidx = bidx0 - 1;
    int64_t kbc_stop = kbc0;
    while (bidx >= 0) {
        int64_t kbc = bidx * total / gridDim.x;
        if (kbc == kbc_stop) {
            bidx--;
            kbc_stop = kbc;
            continue;
        }
        any_fixup = true;
        const float * tile = tmp_last_tile + (size_t) bidx * MKLLM_MMQ_I * MKLLM_MMQ_J;
        const int ntxw = 2;
        const int wi0 = ((int) threadIdx.y / ntxw) * 32;
#pragma unroll
        for (int jj = 0; jj < MKLLM_MMQ_J; jj += 16) {
#pragma unroll
            for (int nt = 0; nt < ntxw; ++nt) {
#pragma unroll
                for (int l = 0; l < 4; ++l) {
                    const int j = jj + ((int) threadIdx.y % ntxw) * 8 + mkllm_tile_c_j(l);
                    const int i = wi0 + nt * 16 + mkllm_tile_c_i(l);
                    sum[(jj / 8 + nt) * 4 + l] += tile[(size_t) j * MKLLM_MMQ_I + i];
                }
            }
        }
        if ((kbc % nblocks) == 0 || (kbc / nblocks) < (kbc0 / nblocks)) {
            break;
        }
        bidx--;
        kbc_stop = kbc;
    }
    if (!any_fixup) {
        return;
    }
    const int it = (int) (kbc0 / ((int64_t) ntx * nblocks));
    const int jt = (int) ((kbc0 / nblocks) % ntx);
    const int i0 = it * MKLLM_MMQ_I;
    const int j0 = jt * MKLLM_MMQ_J;
    const int ntxw = 2;
    const int wi0 = ((int) threadIdx.y / ntxw) * 32;
#pragma unroll
    for (int jj = 0; jj < MKLLM_MMQ_J; jj += 16) {
#pragma unroll
        for (int nt = 0; nt < ntxw; ++nt) {
#pragma unroll
            for (int l = 0; l < 4; ++l) {
                const int j = jj + ((int) threadIdx.y % ntxw) * 8 + mkllm_tile_c_j(l);
                const int i = wi0 + nt * 16 + mkllm_tile_c_i(l);
                if (j0 + j < m && i0 + i < n) {
                    dst[(size_t) (j0 + j) * (size_t) stride_col_dst + (size_t) (i0 + i)] +=
                        sum[(jj / 8 + nt) * 4 + l];
                }
            }
        }
    }
}
#endif

static __global__ void mkllm_quantize_mmq_ds4_kernel(
        const float * __restrict__ x, mkllm_block_q8_1_mmq * __restrict__ y,
        int k, int m, int stride_col) {
    const int64_t i0 = ((int64_t) blockDim.x * blockIdx.y + threadIdx.x) * 4;
    if (i0 >= k) {
        return;
    }
    const int col = (int) blockIdx.x;
    if (col >= m) {
        return;
    }
    const float4 xi = ((const float4 *) (x + (size_t) col * (size_t) stride_col))[i0 / 4];
    float amax = fabsf(xi.x);
    amax = fmaxf(amax, fabsf(xi.y));
    amax = fmaxf(amax, fabsf(xi.z));
    amax = fmaxf(amax, fabsf(xi.w));
#pragma unroll
    for (int off = 4; off > 0; off >>= 1) {
        amax = fmaxf(amax, __shfl_xor_sync(0xffffffffu, amax, off, 32));
    }
    float sum = xi.x + xi.y + xi.z + xi.w;
#pragma unroll
    for (int off = 4; off > 0; off >>= 1) {
        sum += __shfl_xor_sync(0xffffffffu, sum, off, 32);
    }
    // b10430 quantize.cu ~515-521: d = 1.0f/d_inv; zero-guard amax==0.
    const float d_inv = amax > 0.0f ? 127.0f / amax : 0.0f;
    char4 q;
    q.x = (int8_t) roundf(xi.x * d_inv);
    q.y = (int8_t) roundf(xi.y * d_inv);
    q.z = (int8_t) roundf(xi.z * d_inv);
    q.w = (int8_t) roundf(xi.w * d_inv);
    const float d = d_inv > 0.0f ? 1.0f / d_inv : 0.0f;
    const int k_block = (int) (i0 / MKLLM_QK8_1_MMQ);
    const int iqs = (int) (i0 % MKLLM_QK8_1_MMQ);
    mkllm_block_q8_1_mmq * blk = &y[(size_t) k_block * (size_t) m + (size_t) col];
    ((char4 *) blk->qs)[iqs / 4] = q;
    if (iqs % MKLLM_QK8_1 == 0) {
        blk->ds4[iqs / MKLLM_QK8_1] = make_half2(d, sum);
    }
}

extern "C" cudaError_t mkllm_quantize_mmq_ds4(
        const float * x, void * y, int k, int m, int stride_col, cudaStream_t stream) {
    if (k <= 0 || m <= 0 || stride_col < k || (k % MKLLM_QK8_1_MMQ) != 0
            || (stride_col % 4) != 0) {
        return cudaErrorInvalidValue;
    }
    const int block_num_y = (k + 4 * 128 - 1) / (4 * 128);
    dim3 grid(m, block_num_y);
    mkllm_quantize_mmq_ds4_kernel<<<grid, 128, 0, stream>>>(
        x, (mkllm_block_q8_1_mmq *) y, k, m, stride_col);
    return cudaGetLastError();
}

// llama.cpp J=128 MMQ for every kind the executor can hand us. Replaces the
// old per-type mkllm_mmq_q{4,5,6}k_j128 entry points: the launcher was already
// templated on ggml_type, so one dispatch covers Q4_K/Q5_K/Q6_K and the
// UD- kinds (Q3_K, IQ4_XS, IQ4_NL, IQ3_S) alike.
//
// `stride_row_x` counts BLOCKS of x's type, so the lower bound depends on the
// kind's block length (32 values for iq4_nl, 256 otherwise).
extern "C" cudaError_t mkllm_mmq_kind_j128(
        int kind, const void * x, const void * y, float * dst,
        int k, int n, int m, int stride_row_x, int stride_col_dst,
        int nsm, float * tmp_fixup, cudaStream_t stream) {
    if (k <= 0 || n <= 0 || m <= 0 || (k % QK_K) != 0 || (m % 128) != 0
            || stride_col_dst < n) {
        return cudaErrorInvalidValue;
    }
    const int blk_elems = (kind == MKLLM_QUANT_IQ4NL) ? QK4_NL : QK_K;
    if (stride_row_x < k / blk_elems) {
        return cudaErrorInvalidValue;
    }
    if ((mkllm_kind_route_mask(kind) & MKLLM_ROUTE_MMQ) == 0) {
        return cudaErrorInvalidValue;
    }
    switch (kind) {
        case MKLLM_QUANT_Q4K:
            return mkllm_launch_mul_mat_q<GGML_TYPE_Q4_K>(
                x, y, dst, k, n, m, stride_row_x, stride_col_dst, nsm, tmp_fixup, stream);
        case MKLLM_QUANT_Q5K:
            return mkllm_launch_mul_mat_q<GGML_TYPE_Q5_K>(
                x, y, dst, k, n, m, stride_row_x, stride_col_dst, nsm, tmp_fixup, stream);
        case MKLLM_QUANT_Q6K:
            return mkllm_launch_mul_mat_q<GGML_TYPE_Q6_K>(
                x, y, dst, k, n, m, stride_row_x, stride_col_dst, nsm, tmp_fixup, stream);
        case MKLLM_QUANT_Q3K:
            return mkllm_launch_mul_mat_q<GGML_TYPE_Q3_K>(
                x, y, dst, k, n, m, stride_row_x, stride_col_dst, nsm, tmp_fixup, stream);
        case MKLLM_QUANT_IQ4XS:
            return mkllm_launch_mul_mat_q<GGML_TYPE_IQ4_XS>(
                x, y, dst, k, n, m, stride_row_x, stride_col_dst, nsm, tmp_fixup, stream);
        case MKLLM_QUANT_IQ4NL:
            return mkllm_launch_mul_mat_q<GGML_TYPE_IQ4_NL>(
                x, y, dst, k, n, m, stride_row_x, stride_col_dst, nsm, tmp_fixup, stream);
        case MKLLM_QUANT_IQ3S:
            return mkllm_launch_mul_mat_q<GGML_TYPE_IQ3_S>(
                x, y, dst, k, n, m, stride_row_x, stride_col_dst, nsm, tmp_fixup, stream);
        default:
            return cudaErrorInvalidValue;
    }
}

// Q5_K J=128: llama.cpp load_tiles_q5_K + the same q8_1 MMA vec_dot as Q4_K.
#define MKLLM_QI5_K_MMQ (QK_K / (4 * 2))

#if 0
static __device__ __forceinline__ void mkllm_mmq_q5k_load_tiles(
        const char * __restrict__ x, int * __restrict__ x_tile,
        int kbx0, int i_max, int stride) {
    (void) i_max;
    int * x_qs = x_tile;
    half2 * x_dm = (half2 *) (x_qs + 2 * MKLLM_MMQ_TILE_NE_K);
#pragma unroll
    for (int i0 = 0; i0 < MKLLM_MMQ_I; i0 += MKLLM_MMQ_NWARPS) {
        const int i = i0 + (int) threadIdx.y;
        const mkllm_block_q5_K * bxi =
            (const mkllm_block_q5_K *) x + kbx0 + i * stride;
        const int txi = (int) threadIdx.x;
        const int ky = 2 * txi;
        const int ql = mkllm_get_int_b4(bxi->qs, txi);
        const int ql0 = (ql >> 0) & 0x0F0F0F0F;
        const int ql1 = (ql >> 4) & 0x0F0F0F0F;
        const int qh = mkllm_get_int_b4(bxi->qh, txi % (MKLLM_QI5_K_MMQ / 4));
        const int qh0 = ((qh >> (2 * (txi / (MKLLM_QI5_K_MMQ / 4)) + 0)) << 4) & 0x10101010;
        const int qh1 = ((qh >> (2 * (txi / (MKLLM_QI5_K_MMQ / 4)) + 1)) << 4) & 0x10101010;
        const int kq0 = ky - ky % (MKLLM_QI5_K_MMQ / 2) + txi % (MKLLM_QI5_K_MMQ / 4) + 0;
        const int kq1 = ky - ky % (MKLLM_QI5_K_MMQ / 2) + txi % (MKLLM_QI5_K_MMQ / 4)
            + MKLLM_QI5_K_MMQ / 4;
        x_qs[i * MKLLM_MMQ_SRAM_STRIDE + kq0] = ql0 | qh0;
        x_qs[i * MKLLM_MMQ_SRAM_STRIDE + kq1] = ql1 | qh1;
    }
#pragma unroll
    for (int i0 = 0; i0 < MKLLM_MMQ_I; i0 += MKLLM_MMQ_NWARPS * 16) {
        const int i = (i0 + (int) threadIdx.y * 16 + (int) threadIdx.x / 2) % MKLLM_MMQ_I;
        const mkllm_block_q5_K * bxi =
            (const mkllm_block_q5_K *) x + kbx0 + i * stride;
        const int * scales = (const int *) bxi->scales;
        const int ksc = (int) threadIdx.x % 2;
        const int sc32 = mkllm_unpack_scales_q45_K(scales, ksc + 0);
        const int m32 = mkllm_unpack_scales_q45_K(scales, ksc + 2);
        const uint8_t * sc8 = (const uint8_t *) &sc32;
        const uint8_t * m8 = (const uint8_t *) &m32;
        const half2 dm = __hmul2(bxi->dm, make_half2(1.0f, -1.0f));
#pragma unroll
        for (int l = 0; l < 4; ++l) {
            x_dm[i * MKLLM_MMQ_SRAM_STRIDE + 4 * ksc + l] =
                __hmul2(dm, make_half2((float) sc8[l], (float) m8[l]));
        }
    }
}

static __device__ __forceinline__ void mkllm_mmq_q5k_process_tile(
        const char * __restrict__ x, const int * __restrict__ y,
        int * __restrict__ tile_x, int * __restrict__ tile_y, float * __restrict__ sum,
        int offset_x, int j0, int i_max, int stride_row_x, int y_stride,
        int kb0_start, int kb0_stop) {
#pragma unroll
    for (int s = 0; s < 64; ++s) {
        sum[s] = 0.0f;
    }
    for (int kb0 = kb0_start; kb0 < kb0_stop; ++kb0) {
        mkllm_mmq_q5k_load_tiles(x, tile_x, offset_x + kb0, i_max, stride_row_x);
        {
            const int * by0 = y + (j0 * (int) (sizeof(mkllm_block_q8_1_mmq) / sizeof(int)))
                + (kb0 * 2) * y_stride;
#pragma unroll
            for (int l0 = 0; l0 < MKLLM_MMQ_J * MKLLM_MMQ_TILE_Y_K; l0 += 256) {
                const int l = l0 + (int) threadIdx.y * 32 + (int) threadIdx.x;
                tile_y[l] = by0[l];
            }
        }
        __syncthreads();
        mkllm_mmq_q4k_vec_dot(tile_x, tile_y, sum, 0);
        __syncthreads();
        {
            const int * by0 = y + (j0 * (int) (sizeof(mkllm_block_q8_1_mmq) / sizeof(int)))
                + (kb0 * 2 + 1) * y_stride;
#pragma unroll
            for (int l0 = 0; l0 < MKLLM_MMQ_J * MKLLM_MMQ_TILE_Y_K; l0 += 256) {
                const int l = l0 + (int) threadIdx.y * 32 + (int) threadIdx.x;
                tile_y[l] = by0[l];
            }
        }
        __syncthreads();
        mkllm_mmq_q4k_vec_dot(tile_x, tile_y, sum, MKLLM_MMQ_TILE_NE_K);
        __syncthreads();
    }
}

static __global__ void __launch_bounds__(256, 1) mkllm_mmq_q5k_j128_kernel(
        const char * __restrict__ x, const int * __restrict__ y, float * __restrict__ dst,
        int k, int n, int m, int stride_row_x, int stride_col_dst) {
    const int i0 = (int) blockIdx.x * MKLLM_MMQ_I;
    const int j0 = (int) blockIdx.y * MKLLM_MMQ_J;
    extern __shared__ int smem[];
    int * tile_y = smem + MKLLM_MMQ_J;
    int * tile_x = tile_y + MKLLM_MMQ_J * MKLLM_MMQ_TILE_Y_K;
    float sum[64];
    const int nblocks = k / QK_K;
    const int y_stride = m * (int) (sizeof(mkllm_block_q8_1_mmq) / sizeof(int));
    mkllm_mmq_q5k_process_tile(
        x, y, tile_x, tile_y, sum, i0 * stride_row_x, j0, n - i0 - 1, stride_row_x, y_stride,
        0, nblocks);
    mkllm_mmq_write_j128(sum, dst, i0, j0, n - i0 - 1, m - j0 - 1, stride_col_dst);
}

// Same stream-K walker as Q4_K: llama.cpp uses the same q8_1 MMA for Q5_K.
static __global__ void __launch_bounds__(256, 1) mkllm_mmq_q5k_streamk_kernel(
        const char * __restrict__ x, const int * __restrict__ y, float * __restrict__ dst,
        float * __restrict__ tmp_fixup,
        int k, int n, int m, int stride_row_x, int stride_col_dst) {
    const int ntx = m / MKLLM_MMQ_J;
    const int nty = (n + MKLLM_MMQ_I - 1) / MKLLM_MMQ_I;
    const int nblocks = k / QK_K;
    const int64_t total = (int64_t) ntx * nty * nblocks;
    int64_t kbc = (int64_t) blockIdx.x * total / gridDim.x;
    int64_t kbc_stop = (int64_t) (blockIdx.x + 1) * total / gridDim.x;
    int kb0_start = (int) (kbc % nblocks);
    int kb0_stop = (int) min((int64_t) nblocks, (int64_t) kb0_start + kbc_stop - kbc);
    extern __shared__ int smem[];
    int * tile_y = smem + MKLLM_MMQ_J;
    int * tile_x = tile_y + MKLLM_MMQ_J * MKLLM_MMQ_TILE_Y_K;
    float sum[64];
    const int y_stride = m * (int) (sizeof(mkllm_block_q8_1_mmq) / sizeof(int));
    while (kbc < kbc_stop && kb0_stop == nblocks) {
        const int it = (int) (kbc / ((int64_t) ntx * nblocks));
        const int jt = (int) ((kbc / nblocks) % ntx);
        const int i0 = it * MKLLM_MMQ_I;
        const int j0 = jt * MKLLM_MMQ_J;
        mkllm_mmq_q5k_process_tile(
            x, y, tile_x, tile_y, sum, i0 * stride_row_x, j0, n - i0 - 1,
            stride_row_x, y_stride, kb0_start, kb0_stop);
        mkllm_mmq_write_j128(sum, dst, i0, j0, n - i0 - 1, m - j0 - 1, stride_col_dst);
        kbc += nblocks;
        kbc -= kbc % nblocks;
        kb0_start = 0;
        kb0_stop = (int) min((int64_t) nblocks, kbc_stop - kbc);
    }
    if (kbc >= kbc_stop) {
        return;
    }
    const int it = (int) (kbc / ((int64_t) ntx * nblocks));
    const int jt = (int) ((kbc / nblocks) % ntx);
    const int i0 = it * MKLLM_MMQ_I;
    const int j0 = jt * MKLLM_MMQ_J;
    mkllm_mmq_q5k_process_tile(
        x, y, tile_x, tile_y, sum, i0 * stride_row_x, j0, n - i0 - 1,
        stride_row_x, y_stride, kb0_start, kb0_stop);
    mkllm_mmq_write_j128(
        sum, tmp_fixup + (size_t) blockIdx.x * MKLLM_MMQ_I * MKLLM_MMQ_J,
        0, 0, MKLLM_MMQ_I - 1, MKLLM_MMQ_J - 1, MKLLM_MMQ_I);
}

extern "C" cudaError_t mkllm_mmq_q5k_j128(
        const void * x, const void * y, float * dst,
        int k, int n, int m, int stride_row_x, int stride_col_dst,
        int nsm, float * tmp_fixup, cudaStream_t stream) {
    if (k <= 0 || n <= 0 || m <= 0 || (k % QK_K) != 0 || (m % 128) != 0
            || stride_row_x < (k / QK_K) || stride_col_dst < n) {
        return cudaErrorInvalidValue;
    }
    return mkllm_launch_mul_mat_q<GGML_TYPE_Q5_K>(
        x, y, dst, k, n, m, stride_row_x, stride_col_dst, nsm, tmp_fixup, stream);
}

// ---------------------------------------------------------------------------
// Q6_K J=128 MMA. llama.cpp mmq.cuh load_tiles_q6_K + vec_dot_q6_K_q8_1_mma
// (Turing/Ampere). Y uses D4 (float d per 32 values), not DS4. X tile
// stride is MMQ_MMA_TILE_X_K_Q6_K = 76. MMA is m16n8k16, not k32.
// Isolated to the llama executor; ggml CUDA is not modified.
// ---------------------------------------------------------------------------

#define MKLLM_MMQ_TILE_X_K_Q6 (2 * MKLLM_MMQ_TILE_NE_K + MKLLM_MMQ_TILE_NE_K / MKLLM_QI6_K \
    + MKLLM_MMQ_TILE_NE_K / 8 + 7)
static_assert(MKLLM_MMQ_TILE_X_K_Q6 == 76, "Q6 MMA tile stride");
static_assert(MKLLM_MMQ_TILE_X_K_Q6 % 8 == 4, "Q6 MMA tile padding");

struct mkllm_tile_16x4 {
    int x[2];
};
struct mkllm_tile_8x4 {
    int x[1];
};

static __device__ __forceinline__ int mkllm_vsubss4(int a, int b) {
    return __vsubss4(a, b);
}

static __device__ __forceinline__ void mkllm_ldmatrix_a16x4(
        mkllm_tile_16x4 & t, const int * xs0, int stride) {
    const int * xs = xs0 + ((int) threadIdx.x % 16) * stride;
    asm volatile("ldmatrix.sync.aligned.m8n8.x2.b16 {%0, %1}, [%2];"
        : "=r"(t.x[0]), "=r"(t.x[1])
        : "l"(xs));
}

static __device__ __forceinline__ void mkllm_load_generic_b8x4(
        mkllm_tile_8x4 & t, const int * xs0, int stride) {
    const int i = (int) threadIdx.x / 4;
    const int j0 = (int) threadIdx.x % 4;
    t.x[0] = xs0[i * stride + j0];
}

static __device__ __forceinline__ void mkllm_mma_s8_16x8x16(
        mkllm_tile_16x8 & d, const mkllm_tile_16x4 & a, const mkllm_tile_8x4 & b) {
#if __CUDA_ARCH__ >= 800
    asm volatile(
        "mma.sync.aligned.m16n8k16.row.col.s32.s8.s8.s32"
        " {%0, %1, %2, %3}, {%4, %5}, {%6}, {%0, %1, %2, %3};"
        : "+r"(d.x[0]), "+r"(d.x[1]), "+r"(d.x[2]), "+r"(d.x[3])
        : "r"(a.x[0]), "r"(a.x[1]), "r"(b.x[0]));
#else
    (void) a;
    (void) b;
    d.x[0] = d.x[1] = d.x[2] = d.x[3] = 0;
#endif
}

static __device__ __forceinline__ void mkllm_mmq_q6k_load_tiles(
        const char * __restrict__ x, int * __restrict__ x_tile,
        int kbx0, int i_max, int stride) {
    (void) i_max;
    int * x_qs = x_tile;
    float * x_df = (float *) (x_qs + 2 * MKLLM_MMQ_TILE_NE_K);
    int * x_sc = (int *) (x_df + MKLLM_MMQ_TILE_NE_K / MKLLM_QI6_K);
#pragma unroll
    for (int i0 = 0; i0 < MKLLM_MMQ_I; i0 += MKLLM_MMQ_NWARPS) {
        const int i = i0 + (int) threadIdx.y;
        const mkllm_block_q6_K * bxi =
            (const mkllm_block_q6_K *) x + kbx0 + i * stride;
        const int txi = (int) threadIdx.x;
        const int ql = mkllm_get_int_b2(bxi->ql, txi);
        const int ql0 = (ql >> 0) & 0x0F0F0F0F;
        const int ql1 = (ql >> 4) & 0x0F0F0F0F;
        const int qh = mkllm_get_int_b2(
            bxi->qh, (MKLLM_QI6_K / 4) * (txi / (MKLLM_QI6_K / 2)) + txi % (MKLLM_QI6_K / 4));
        const int qh0 = ((qh >> ((txi & 0x08) >> 2)) << 4) & 0x30303030;
        const int qh1 = (qh >> ((txi & 0x08) >> 2)) & 0x30303030;
        const int kq0 = 2 * txi - txi % (MKLLM_QI6_K / 2) + 0;
        const int kq1 = 2 * txi - txi % (MKLLM_QI6_K / 2) + MKLLM_QI6_K / 2;
        x_qs[i * MKLLM_MMQ_TILE_X_K_Q6 + kq0] = mkllm_vsubss4(ql0 | qh0, 0x20202020);
        x_qs[i * MKLLM_MMQ_TILE_X_K_Q6 + kq1] = mkllm_vsubss4(ql1 | qh1, 0x20202020);
    }
#pragma unroll
    for (int i0 = 0; i0 < MKLLM_MMQ_I; i0 += MKLLM_MMQ_NWARPS * 32) {
        const int i = (i0 + (int) threadIdx.y * 32 + (int) threadIdx.x) % MKLLM_MMQ_I;
        const mkllm_block_q6_K * bxi =
            (const mkllm_block_q6_K *) x + kbx0 + i * stride;
        x_df[i * MKLLM_MMQ_TILE_X_K_Q6] = bxi->d;
    }
#pragma unroll
    for (int i0 = 0; i0 < MKLLM_MMQ_I; i0 += MKLLM_MMQ_NWARPS * 8) {
        const int i = (i0 + (int) threadIdx.y * 8 + (int) threadIdx.x / 4) % MKLLM_MMQ_I;
        const mkllm_block_q6_K * bxi =
            (const mkllm_block_q6_K *) x + kbx0 + i * stride
            + ((int) threadIdx.x % 4) / 4;
        x_sc[i * MKLLM_MMQ_TILE_X_K_Q6 + (int) threadIdx.x % 4] =
            mkllm_get_int_b2(bxi->scales, (int) threadIdx.x % 4);
    }
}

static __device__ __forceinline__ void mkllm_mmq_q6k_vec_dot(
        const int * __restrict__ x, const int * __restrict__ y,
        float * __restrict__ sum, int k00) {
    constexpr int ntx = 2;
    constexpr int rows_per_warp = 32;
    y += ((int) threadIdx.y % ntx) * (8 * MKLLM_MMQ_TILE_Y_K);
    const int * x_qs = x;
    const float * x_df = (const float *) x_qs + 2 * MKLLM_MMQ_TILE_NE_K;
    const int * x_sc = (const int *) x_df + MKLLM_MMQ_TILE_NE_K / MKLLM_QI6_K;
    const int * y_qs = y + 4;
    const float * y_df = (const float *) y;
    const int i0 = ((int) threadIdx.y / ntx) * rows_per_warp;
    mkllm_tile_16x4 A[ntx][8];
    int scA[ntx][2][8];
    float dA[ntx][2];
#pragma unroll
    for (int n = 0; n < ntx; ++n) {
#pragma unroll
        for (int k01 = 0; k01 < MKLLM_MMQ_TILE_NE_K; k01 += 8) {
            const int k0 = k00 + k01;
            mkllm_ldmatrix_a16x4(
                A[n][k01 / 4 + 0],
                x_qs + (i0 + n * 16) * MKLLM_MMQ_TILE_X_K_Q6 + (k0 + 0),
                MKLLM_MMQ_TILE_X_K_Q6);
            mkllm_ldmatrix_a16x4(
                A[n][k01 / 4 + 1],
                x_qs + (i0 + n * 16) * MKLLM_MMQ_TILE_X_K_Q6 + (k0 + 4),
                MKLLM_MMQ_TILE_X_K_Q6);
        }
#pragma unroll
        for (int k01 = 0; k01 < MKLLM_MMQ_TILE_NE_K; k01 += 16) {
            const int k0 = k00 + k01;
#pragma unroll
            for (int l = 0; l < 2; ++l) {
                const int i = i0 + n * 16 + mkllm_tile_c_i(2 * l);
                const int sc_packed = x_sc[i * MKLLM_MMQ_TILE_X_K_Q6 + k0 / 16];
                const int8_t * sc = (const int8_t *) &sc_packed;
#pragma unroll
                for (int ksc = 0; ksc < 4; ++ksc) {
                    scA[n][l][k01 / 4 + ksc] = sc[ksc];
                }
            }
        }
#pragma unroll
        for (int l = 0; l < 2; ++l) {
            const int i = i0 + n * 16 + mkllm_tile_c_i(2 * l);
            dA[n][l] = x_df[i * MKLLM_MMQ_TILE_X_K_Q6];
        }
    }
#pragma unroll
    for (int j0 = 0; j0 < MKLLM_MMQ_J; j0 += ntx * 8) {
        float tmp[ntx][4];
#pragma unroll
        for (int n = 0; n < ntx; ++n) {
#pragma unroll
            for (int l = 0; l < 4; ++l) {
                tmp[n][l] = 0.0f;
            }
        }
#pragma unroll
        for (int k01 = 0; k01 < MKLLM_MMQ_TILE_NE_K; k01 += 8) {
            mkllm_tile_8x4 B[2];
            float dB[2];
            mkllm_load_generic_b8x4(B[0], y_qs + j0 * MKLLM_MMQ_TILE_Y_K + 0 + k01,
                MKLLM_MMQ_TILE_Y_K);
            mkllm_load_generic_b8x4(B[1], y_qs + j0 * MKLLM_MMQ_TILE_Y_K + 4 + k01,
                MKLLM_MMQ_TILE_Y_K);
#pragma unroll
            for (int l = 0; l < 2; ++l) {
                const int j = j0 + mkllm_tile_c_j(l);
                dB[l] = y_df[j * MKLLM_MMQ_TILE_Y_K + k01 / MKLLM_QI8_1];
            }
#pragma unroll
            for (int n = 0; n < ntx; ++n) {
                mkllm_tile_16x8 C0;
                mkllm_tile_16x8 C1;
                C0.x[0] = C0.x[1] = C0.x[2] = C0.x[3] = 0;
                C1.x[0] = C1.x[1] = C1.x[2] = C1.x[3] = 0;
                mkllm_mma_s8_16x8x16(C0, A[n][k01 / 4 + 0], B[0]);
                mkllm_mma_s8_16x8x16(C1, A[n][k01 / 4 + 1], B[1]);
#pragma unroll
                for (int l = 0; l < 4; ++l) {
                    tmp[n][l] += (C0.x[l] * scA[n][l / 2][k01 / 4 + 0]
                        + C1.x[l] * scA[n][l / 2][k01 / 4 + 1]) * dB[l % 2];
                }
            }
        }
#pragma unroll
        for (int n = 0; n < ntx; ++n) {
#pragma unroll
            for (int l = 0; l < 4; ++l) {
                sum[(j0 / 8 + n) * 4 + l] += tmp[n][l] * dA[n][l / 2];
            }
        }
    }
}

static __device__ __forceinline__ void mkllm_mmq_q6k_process_tile(
        const char * __restrict__ x, const int * __restrict__ y,
        int * __restrict__ tile_x, int * __restrict__ tile_y, float * __restrict__ sum,
        int offset_x, int j0, int i_max, int stride_row_x, int y_stride,
        int kb0_start, int kb0_stop) {
#pragma unroll
    for (int s = 0; s < 64; ++s) {
        sum[s] = 0.0f;
    }
    for (int kb0 = kb0_start; kb0 < kb0_stop; ++kb0) {
        mkllm_mmq_q6k_load_tiles(x, tile_x, offset_x + kb0, i_max, stride_row_x);
        {
            const int * by0 = y + (j0 * (int) (sizeof(mkllm_block_q8_1_mmq) / sizeof(int)))
                + (kb0 * 2) * y_stride;
#pragma unroll
            for (int l0 = 0; l0 < MKLLM_MMQ_J * MKLLM_MMQ_TILE_Y_K; l0 += 256) {
                const int l = l0 + (int) threadIdx.y * 32 + (int) threadIdx.x;
                tile_y[l] = by0[l];
            }
        }
        __syncthreads();
        mkllm_mmq_q6k_vec_dot(tile_x, tile_y, sum, 0);
        __syncthreads();
        {
            const int * by0 = y + (j0 * (int) (sizeof(mkllm_block_q8_1_mmq) / sizeof(int)))
                + (kb0 * 2 + 1) * y_stride;
#pragma unroll
            for (int l0 = 0; l0 < MKLLM_MMQ_J * MKLLM_MMQ_TILE_Y_K; l0 += 256) {
                const int l = l0 + (int) threadIdx.y * 32 + (int) threadIdx.x;
                tile_y[l] = by0[l];
            }
        }
        __syncthreads();
        mkllm_mmq_q6k_vec_dot(tile_x, tile_y, sum, MKLLM_MMQ_TILE_NE_K);
        __syncthreads();
    }
}

static __global__ void __launch_bounds__(256, 1) mkllm_mmq_q6k_j128_kernel(
        const char * __restrict__ x, const int * __restrict__ y, float * __restrict__ dst,
        int k, int n, int m, int stride_row_x, int stride_col_dst) {
    const int i0 = (int) blockIdx.x * MKLLM_MMQ_I;
    const int j0 = (int) blockIdx.y * MKLLM_MMQ_J;
    extern __shared__ int smem[];
    int * tile_y = smem + MKLLM_MMQ_J;
    int * tile_x = tile_y + MKLLM_MMQ_J * MKLLM_MMQ_TILE_Y_K;
    float sum[64];
    const int nblocks = k / QK_K;
    const int y_stride = m * (int) (sizeof(mkllm_block_q8_1_mmq) / sizeof(int));
    mkllm_mmq_q6k_process_tile(
        x, y, tile_x, tile_y, sum, i0 * stride_row_x, j0, n - i0 - 1, stride_row_x, y_stride,
        0, nblocks);
    mkllm_mmq_write_j128(sum, dst, i0, j0, n - i0 - 1, m - j0 - 1, stride_col_dst);
}

static __global__ void __launch_bounds__(256, 1) mkllm_mmq_q6k_streamk_kernel(
        const char * __restrict__ x, const int * __restrict__ y, float * __restrict__ dst,
        float * __restrict__ tmp_fixup,
        int k, int n, int m, int stride_row_x, int stride_col_dst) {
    const int ntx = m / MKLLM_MMQ_J;
    const int nty = (n + MKLLM_MMQ_I - 1) / MKLLM_MMQ_I;
    const int nblocks = k / QK_K;
    const int64_t total = (int64_t) ntx * nty * nblocks;
    int64_t kbc = (int64_t) blockIdx.x * total / gridDim.x;
    int64_t kbc_stop = (int64_t) (blockIdx.x + 1) * total / gridDim.x;
    int kb0_start = (int) (kbc % nblocks);
    int kb0_stop = (int) min((int64_t) nblocks, (int64_t) kb0_start + kbc_stop - kbc);
    extern __shared__ int smem[];
    int * tile_y = smem + MKLLM_MMQ_J;
    int * tile_x = tile_y + MKLLM_MMQ_J * MKLLM_MMQ_TILE_Y_K;
    float sum[64];
    const int y_stride = m * (int) (sizeof(mkllm_block_q8_1_mmq) / sizeof(int));
    while (kbc < kbc_stop && kb0_stop == nblocks) {
        const int it = (int) (kbc / ((int64_t) ntx * nblocks));
        const int jt = (int) ((kbc / nblocks) % ntx);
        const int i0 = it * MKLLM_MMQ_I;
        const int j0 = jt * MKLLM_MMQ_J;
        mkllm_mmq_q6k_process_tile(
            x, y, tile_x, tile_y, sum, i0 * stride_row_x, j0, n - i0 - 1,
            stride_row_x, y_stride, kb0_start, kb0_stop);
        mkllm_mmq_write_j128(sum, dst, i0, j0, n - i0 - 1, m - j0 - 1, stride_col_dst);
        kbc += nblocks;
        kbc -= kbc % nblocks;
        kb0_start = 0;
        kb0_stop = (int) min((int64_t) nblocks, kbc_stop - kbc);
    }
    if (kbc >= kbc_stop) {
        return;
    }
    const int it = (int) (kbc / ((int64_t) ntx * nblocks));
    const int jt = (int) ((kbc / nblocks) % ntx);
    const int i0 = it * MKLLM_MMQ_I;
    const int j0 = jt * MKLLM_MMQ_J;
    mkllm_mmq_q6k_process_tile(
        x, y, tile_x, tile_y, sum, i0 * stride_row_x, j0, n - i0 - 1,
        stride_row_x, y_stride, kb0_start, kb0_stop);
    mkllm_mmq_write_j128(
        sum, tmp_fixup + (size_t) blockIdx.x * MKLLM_MMQ_I * MKLLM_MMQ_J,
        0, 0, MKLLM_MMQ_I - 1, MKLLM_MMQ_J - 1, MKLLM_MMQ_I);
}
#endif

static __global__ void mkllm_quantize_mmq_d4_kernel(
        const float * __restrict__ x, mkllm_block_q8_1_mmq * __restrict__ y,
        int k, int m, int stride_col) {
    const int64_t i0 = ((int64_t) blockDim.x * blockIdx.y + threadIdx.x) * 4;
    if (i0 >= k) {
        return;
    }
    const int col = (int) blockIdx.x;
    if (col >= m) {
        return;
    }
    const float4 xi = ((const float4 *) (x + (size_t) col * (size_t) stride_col))[i0 / 4];
    float amax = fabsf(xi.x);
    amax = fmaxf(amax, fabsf(xi.y));
    amax = fmaxf(amax, fabsf(xi.z));
    amax = fmaxf(amax, fabsf(xi.w));
#pragma unroll
    for (int off = 4; off > 0; off >>= 1) {
        amax = fmaxf(amax, __shfl_xor_sync(0xffffffffu, amax, off, 32));
    }
    const float d_inv = amax > 0.0f ? 127.0f / amax : 0.0f;
    char4 q;
    q.x = (int8_t) roundf(xi.x * d_inv);
    q.y = (int8_t) roundf(xi.y * d_inv);
    q.z = (int8_t) roundf(xi.z * d_inv);
    q.w = (int8_t) roundf(xi.w * d_inv);
    const float d = d_inv > 0.0f ? 1.0f / d_inv : 0.0f;
    const int k_block = (int) (i0 / MKLLM_QK8_1_MMQ);
    const int iqs = (int) (i0 % MKLLM_QK8_1_MMQ);
    mkllm_block_q8_1_mmq * blk = &y[(size_t) k_block * (size_t) m + (size_t) col];
    ((char4 *) blk->qs)[iqs / 4] = q;
    if (iqs % MKLLM_QK8_1 == 0) {
        blk->d4[iqs / MKLLM_QK8_1] = d;
    }
}

extern "C" cudaError_t mkllm_quantize_mmq_d4(
        const float * x, void * y, int k, int m, int stride_col, cudaStream_t stream) {
    if (k <= 0 || m <= 0 || stride_col < k || (k % MKLLM_QK8_1_MMQ) != 0
            || (stride_col % 4) != 0) {
        return cudaErrorInvalidValue;
    }
    const int block_num_y = (k + 4 * 128 - 1) / (4 * 128);
    dim3 grid(m, block_num_y);
    mkllm_quantize_mmq_d4_kernel<<<grid, 128, 0, stream>>>(
        x, (mkllm_block_q8_1_mmq *) y, k, m, stride_col);
    return cudaGetLastError();
}

// Strided f32 -> contiguous bf16 (activation cast for GEMM), 2D [K, M].
static __global__ void mkllm_cast_f32_bf16_kernel(
        const uint8_t * __restrict__ src, __nv_bfloat16 * __restrict__ dst,
        int K, int M, size_t src_nb0, size_t src_nb1) {
    const int k = blockIdx.x * blockDim.x + threadIdx.x;
    const int m = blockIdx.y;
    if (k >= K || m >= M) return;
    const float v = *(const float *) (src + (size_t) m * src_nb1 + (size_t) k * src_nb0);
    dst[(size_t) m * K + k] = __float2bfloat16(v);
}

extern "C" cudaError_t mkllm_cast_f32_bf16(
        const void * src, void * dst, int K, int M,
        size_t src_nb0, size_t src_nb1, cudaStream_t stream) {
    dim3 block(256);
    dim3 grid((K + 255) / 256, M);
    mkllm_cast_f32_bf16_kernel<<<grid, block, 0, stream>>>(
        (const uint8_t *) src, (__nv_bfloat16 *) dst, K, M, src_nb0, src_nb1);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// General strided batched mat-mul, f16 or f32 A x f32 B -> f32:
//   dst[n, m, b2, b3] = sum_k A[k, n, b2 % a_ne2, b3 % a_ne3] * B[k, m, b2, b3]
// Used for the non-flash attention QK^T and PV products (arbitrary views).
// ---------------------------------------------------------------------------

template <typename W>
static __global__ void mkllm_mul_mat_batched_kernel(
        const uint8_t * __restrict__ a, const uint8_t * __restrict__ b,
        uint8_t * __restrict__ dst,
        int K, int N, int M, int ne2, int ne3, int a_ne2, int a_ne3,
        size_t a_nb0, size_t a_nb1, size_t a_nb2, size_t a_nb3,
        size_t b_nb0, size_t b_nb1, size_t b_nb2, size_t b_nb3,
        size_t d_nb0, size_t d_nb1, size_t d_nb2, size_t d_nb3) {
    const int n = blockIdx.x * blockDim.y + threadIdx.y;
    const int m = blockIdx.y;
    const int batch = blockIdx.z;
    if (n >= N) return;
    const int i2 = batch % ne2;
    const int i3 = batch / ne2;
    const int lane = threadIdx.x;
    // ggml mul_mat batch broadcast: src0 batch = src1 batch / (ne / a_ne)
    // (consecutive grouping — GQA heads), NOT modulo.
    const int a_i2 = i2 / (ne2 / a_ne2);
    const int a_i3 = i3 / (ne3 / a_ne3);
    const uint8_t * a_base = a + (size_t) a_i2 * a_nb2 + (size_t) a_i3 * a_nb3
        + (size_t) n * a_nb1;
    const uint8_t * b_base = b + (size_t) i2 * b_nb2 + (size_t) i3 * b_nb3 + (size_t) m * b_nb1;
    float acc = 0.0f;
    for (int k = lane; k < K; k += 32) {
        const float av = (float) *(const W *) (a_base + (size_t) k * a_nb0);
        const float bv = *(const float *) (b_base + (size_t) k * b_nb0);
        acc += av * bv;
    }
    for (int off = 16; off > 0; off >>= 1) {
        acc += __shfl_down_sync(0xffffffff, acc, off);
    }
    if (lane == 0) {
        *(float *) (dst + (size_t) i3 * d_nb3 + (size_t) i2 * d_nb2 + (size_t) m * d_nb1
            + (size_t) n * d_nb0) = acc;
    }
}

extern "C" cudaError_t mkllm_mul_mat_batched(
        int a_is_f16,
        const void * a, const void * b, void * dst,
        int K, int N, int M, int ne2, int ne3, int a_ne2, int a_ne3,
        size_t a_nb0, size_t a_nb1, size_t a_nb2, size_t a_nb3,
        size_t b_nb0, size_t b_nb1, size_t b_nb2, size_t b_nb3,
        size_t d_nb0, size_t d_nb1, size_t d_nb2, size_t d_nb3,
        cudaStream_t stream) {
    dim3 block(32, 4);
    dim3 grid((N + 3) / 4, M, ne2 * ne3);
    if (a_is_f16) {
        mkllm_mul_mat_batched_kernel<__half><<<grid, block, 0, stream>>>(
            (const uint8_t *) a, (const uint8_t *) b, (uint8_t *) dst,
            K, N, M, ne2, ne3, a_ne2, a_ne3,
            a_nb0, a_nb1, a_nb2, a_nb3, b_nb0, b_nb1, b_nb2, b_nb3,
            d_nb0, d_nb1, d_nb2, d_nb3);
    } else {
        mkllm_mul_mat_batched_kernel<float><<<grid, block, 0, stream>>>(
            (const uint8_t *) a, (const uint8_t *) b, (uint8_t *) dst,
            K, N, M, ne2, ne3, a_ne2, a_ne3,
            a_nb0, a_nb1, a_nb2, a_nb3, b_nb0, b_nb1, b_nb2, b_nb3,
            d_nb0, d_nb1, d_nb2, d_nb3);
    }
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// get_rows / set_rows
// ---------------------------------------------------------------------------

static __global__ void mkllm_get_rows_f32_kernel(
        const uint8_t * __restrict__ src, const int32_t * __restrict__ rows,
        float * __restrict__ dst, int ne0, int nrows, size_t src_nb1, size_t dst_nb1) {
    const int r = blockIdx.y;
    const int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (r >= nrows || i >= ne0) return;
    const float * s = (const float *) (src + (size_t) rows[r] * src_nb1);
    *(float *) ((uint8_t *) dst + (size_t) r * dst_nb1 + (size_t) i * 4) = s[i];
}

extern "C" cudaError_t mkllm_get_rows_f32(
        const void * src, const int32_t * rows, void * dst,
        int ne0, int nrows, size_t src_nb1, size_t dst_nb1, cudaStream_t stream) {
    dim3 block(256);
    dim3 grid((ne0 + 255) / 256, nrows);
    mkllm_get_rows_f32_kernel<<<grid, block, 0, stream>>>(
        (const uint8_t *) src, rows, (float *) dst, ne0, nrows, src_nb1, dst_nb1);
    return cudaGetLastError();
}

template <int KIND>
static __global__ void mkllm_get_rows_quant_kernel(
        const uint8_t * __restrict__ src, const int32_t * __restrict__ rows,
        float * __restrict__ dst, int ne0, int nrows, size_t src_nb1, size_t dst_nb1) {
    const int r = blockIdx.y;
    const int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (r >= nrows || i >= ne0) return;
    const uint8_t * row_bytes = src + (size_t) rows[r] * src_nb1;
    const uint8_t * blk = row_bytes + (size_t) (i / QK_K) * mkllm_quant_block_bytes_dev(KIND);
    *(float *) ((uint8_t *) dst + (size_t) r * dst_nb1 + (size_t) i * 4) =
        mkllm_deq_at(KIND, blk, i % QK_K);
}

extern "C" cudaError_t mkllm_get_rows_quant(
        int kind, const void * src, const int32_t * rows, void * dst,
        int ne0, int nrows, size_t src_nb1, size_t dst_nb1, cudaStream_t stream) {
    dim3 block(256);
    dim3 grid((ne0 + 255) / 256, nrows);
    switch (kind) {
        case MKLLM_QUANT_Q4K:
            mkllm_get_rows_quant_kernel<MKLLM_QUANT_Q4K><<<grid, block, 0, stream>>>(
                (const uint8_t *) src, rows, (float *) dst, ne0, nrows, src_nb1, dst_nb1);
            break;
        case MKLLM_QUANT_Q5K:
            mkllm_get_rows_quant_kernel<MKLLM_QUANT_Q5K><<<grid, block, 0, stream>>>(
                (const uint8_t *) src, rows, (float *) dst, ne0, nrows, src_nb1, dst_nb1);
            break;
        case MKLLM_QUANT_Q80:
            mkllm_get_rows_quant_kernel<MKLLM_QUANT_Q80><<<grid, block, 0, stream>>>(
                (const uint8_t *) src, rows, (float *) dst, ne0, nrows, src_nb1, dst_nb1);
            break;
        case MKLLM_QUANT_Q6K:
            mkllm_get_rows_quant_kernel<MKLLM_QUANT_Q6K><<<grid, block, 0, stream>>>(
                (const uint8_t *) src, rows, (float *) dst, ne0, nrows, src_nb1, dst_nb1);
            break;
        // The IQ/Q3_K kinds route through the shared row-dequant in
        // iq_convert.cuh instead of the legacy per-element selector.
        case MKLLM_QUANT_Q3K:
        case MKLLM_QUANT_IQ4XS:
        case MKLLM_QUANT_IQ4NL:
        case MKLLM_QUANT_IQ3S:
            return mkllm_get_rows_iq_f32(
                kind, src, rows, dst, ne0, nrows, src_nb1, dst_nb1, stream);
        default:
            return cudaErrorInvalidValue;
    }
    return cudaGetLastError();
}

// set_rows: dst[rows[r], :] = src[r, :], f32 src rows into f32 or f16 dst.
template <typename D>
static __global__ void mkllm_set_rows_kernel(
        const uint8_t * __restrict__ src, const int32_t * __restrict__ rows,
        uint8_t * __restrict__ dst, int ne0, int nrows,
        size_t src_nb1, size_t dst_nb1) {
    const int r = blockIdx.y;
    const int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (r >= nrows || i >= ne0) return;
    const float v = *(const float *) (src + (size_t) r * src_nb1 + (size_t) i * 4);
    D * out = (D *) (dst + (size_t) rows[r] * dst_nb1);
    out[i] = (D) v;
}

extern "C" cudaError_t mkllm_set_rows(
        int dst_is_f16, const void * src, const int32_t * rows, void * dst,
        int ne0, int nrows, size_t src_nb1, size_t dst_nb1, cudaStream_t stream) {
    dim3 block(256);
    dim3 grid((ne0 + 255) / 256, nrows);
    if (dst_is_f16) {
        mkllm_set_rows_kernel<__half><<<grid, block, 0, stream>>>(
            (const uint8_t *) src, rows, (uint8_t *) dst, ne0, nrows, src_nb1, dst_nb1);
    } else {
        mkllm_set_rows_kernel<float><<<grid, block, 0, stream>>>(
            (const uint8_t *) src, rows, (uint8_t *) dst, ne0, nrows, src_nb1, dst_nb1);
    }
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Masked softmax (soft_max_ext): dst = softmax(x*scale + mask) per row.
// x [ncols, ne1, ne2], mask f32 [ncols, mask_ne1] broadcast over ne2.
// ---------------------------------------------------------------------------

static __global__ void mkllm_softmax_mask_kernel(
        const uint8_t * __restrict__ x, const uint8_t * __restrict__ mask,
        uint8_t * __restrict__ dst,
        int ncols, int ne1, float scale,
        size_t x_nb1, size_t x_nb2, size_t mask_nb1, size_t d_nb1, size_t d_nb2) {
    const int i1 = blockIdx.x;
    const int i2 = blockIdx.y;
    const int lane = threadIdx.x;
    const float * xr = (const float *) (x + (size_t) i2 * x_nb2 + (size_t) i1 * x_nb1);
    const float * mr = mask ? (const float *) (mask + (size_t) (i1 % ne1) * mask_nb1) : nullptr;
    float * dr = (float *) (dst + (size_t) i2 * d_nb2 + (size_t) i1 * d_nb1);

    extern __shared__ float sm_red[];
    float maxv = -INFINITY;
    for (int c = lane; c < ncols; c += blockDim.x) {
        const float v = xr[c] * scale + (mr ? mr[c] : 0.0f);
        maxv = fmaxf(maxv, v);
    }
    sm_red[lane] = maxv;
    __syncthreads();
    for (int off = blockDim.x >> 1; off > 0; off >>= 1) {
        if (lane < off) sm_red[lane] = fmaxf(sm_red[lane], sm_red[lane + off]);
        __syncthreads();
    }
    maxv = sm_red[0];
    __syncthreads();
    float sum = 0.0f;
    for (int c = lane; c < ncols; c += blockDim.x) {
        const float v = expf(xr[c] * scale + (mr ? mr[c] : 0.0f) - maxv);
        dr[c] = v;
        sum += v;
    }
    sm_red[lane] = sum;
    __syncthreads();
    for (int off = blockDim.x >> 1; off > 0; off >>= 1) {
        if (lane < off) sm_red[lane] += sm_red[lane + off];
        __syncthreads();
    }
    sum = sm_red[0];
    const float inv = sum > 0.0f ? 1.0f / sum : 0.0f;
    for (int c = lane; c < ncols; c += blockDim.x) {
        dr[c] *= inv;
    }
}

extern "C" cudaError_t mkllm_softmax_mask(
        const void * x, const void * mask, void * dst,
        int ncols, int ne1, int ne2, float scale,
        size_t x_nb1, size_t x_nb2, size_t mask_nb1, size_t d_nb1, size_t d_nb2,
        cudaStream_t stream) {
    dim3 block(256);
    dim3 grid(ne1, ne2);
    mkllm_softmax_mask_kernel<<<grid, block, 256 * sizeof(float), stream>>>(
        (const uint8_t *) x, (const uint8_t *) mask, (uint8_t *) dst,
        ncols, ne1, scale, x_nb1, x_nb2, mask_nb1, d_nb1, d_nb2);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Flash-attention decode (small n_q): one block per (head, query).
// q f32 [D, nq, H], k f16 [D, kc, Hkv], v f16 [Dv, kc, Hkv], mask f16
// [kc, nq_pad], dst f32 [Dv, H, nq]. Online softmax, f32 accumulation.
// ---------------------------------------------------------------------------

static __global__ void mkllm_flash_decode_kernel(
        const uint8_t * __restrict__ q, const uint8_t * __restrict__ k,
        const uint8_t * __restrict__ v, const uint8_t * __restrict__ mask,
        uint8_t * __restrict__ dst,
        int D, int Dv, int kc, int n_q, int H, int Hkv, float scale,
        size_t q_nb1, size_t q_nb2,
        size_t k_nb1, size_t k_nb2,
        size_t v_nb1, size_t v_nb2,
        size_t m_nb1,
        size_t d_nb1, size_t d_nb2) {
    const int head = blockIdx.x;
    const int iq = blockIdx.y;
    const int lane = threadIdx.x;      // blockDim.x = 128
    const int kv_head = head / (H / Hkv);

    const float * qv = (const float *) (q + (size_t) head * q_nb2 + (size_t) iq * q_nb1);
    const __half * mrow = (const __half *) (mask + (size_t) iq * m_nb1);
    const uint8_t * kh = k + (size_t) kv_head * k_nb2;
    const uint8_t * vh = v + (size_t) kv_head * v_nb2;

    extern __shared__ float fd_smem[];
    float * qs = fd_smem;              // D floats
    float * red = fd_smem + D;         // blockDim.x floats
    float * acc = fd_smem + D + blockDim.x; // Dv floats
    for (int i = lane; i < D; i += blockDim.x) {
        qs[i] = qv[i];
    }
    for (int i = lane; i < Dv; i += blockDim.x) {
        acc[i] = 0.0f;
    }
    __shared__ float m_running, l_running;
    if (lane == 0) { m_running = -INFINITY; l_running = 0.0f; }
    __syncthreads();

    for (int key = 0; key < kc; key++) {
        const float mval = __half2float(mrow[key]);
        float s;
        if (mval == -INFINITY || mval < -65500.0f) {
            s = -INFINITY;
        } else {
            const __half * krow = (const __half *) (kh + (size_t) key * k_nb1);
            float partial = 0.0f;
            for (int i = lane; i < D; i += blockDim.x) {
                partial += qs[i] * __half2float(krow[i]);
            }
            red[lane] = partial;
            __syncthreads();
            for (int off = blockDim.x >> 1; off > 0; off >>= 1) {
                if (lane < off) red[lane] += red[lane + off];
                __syncthreads();
            }
            s = red[0] * scale + mval;
        }
        __syncthreads();
        if (s != -INFINITY) {
            // Every lane reads m_running before lane 0 rewrites it below;
            // the barrier separates the read phase from the write.
            const float m_new = fmaxf(m_running, s);
            const float corr = expf(m_running - m_new);
            const float p = expf(s - m_new);
            __syncthreads();
            const __half * vrow = (const __half *) (vh + (size_t) key * v_nb1);
            for (int i = lane; i < Dv; i += blockDim.x) {
                acc[i] = acc[i] * corr + p * __half2float(vrow[i]);
            }
            if (lane == 0) {
                l_running = l_running * corr + p;
                m_running = m_new;
            }
        }
        __syncthreads();
    }

    float * out = (float *) (dst + (size_t) iq * d_nb2 + (size_t) head * d_nb1);
    const float inv = l_running > 0.0f ? 1.0f / l_running : 0.0f;
    for (int i = lane; i < Dv; i += blockDim.x) {
        out[i] = acc[i] * inv;
    }
}

extern "C" cudaError_t mkllm_flash_decode(
        const void * q, const void * k, const void * v, const void * mask, void * dst,
        int D, int Dv, int kc, int n_q, int H, int Hkv, float scale,
        size_t q_nb1, size_t q_nb2, size_t k_nb1, size_t k_nb2,
        size_t v_nb1, size_t v_nb2, size_t m_nb1, size_t d_nb1, size_t d_nb2,
        cudaStream_t stream) {
    dim3 block(128);
    dim3 grid(H, n_q);
    const size_t shared = (size_t) (D + 128 + Dv) * sizeof(float);
    mkllm_flash_decode_kernel<<<grid, block, shared, stream>>>(
        (const uint8_t *) q, (const uint8_t *) k, (const uint8_t *) v,
        (const uint8_t *) mask, (uint8_t *) dst,
        D, Dv, kc, n_q, H, Hkv, scale,
        q_nb1, q_nb2, k_nb1, k_nb2, v_nb1, v_nb2, m_nb1, d_nb1, d_nb2);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Norms: rows along dim0 (contiguous), arbitrary higher-dim strides.
// ---------------------------------------------------------------------------

template <int L2>
static __global__ void mkllm_norm_kernel(
        const uint8_t * __restrict__ x, uint8_t * __restrict__ dst,
        int ne0, int ne1, int ne2, float eps,
        size_t x_nb1, size_t x_nb2, size_t x_nb3,
        size_t d_nb1, size_t d_nb2, size_t d_nb3) {
    const int i1 = blockIdx.x % ne1;
    const int i23 = blockIdx.x / ne1;
    const int i2 = i23 % ne2;
    const int i3 = i23 / ne2;
    const int lane = threadIdx.x;
    const float * xr = (const float *) (x + (size_t) i3 * x_nb3 + (size_t) i2 * x_nb2
        + (size_t) i1 * x_nb1);
    float * dr = (float *) (dst + (size_t) i3 * d_nb3 + (size_t) i2 * d_nb2
        + (size_t) i1 * d_nb1);
    extern __shared__ float nrm_red[];
    float sum = 0.0f;
    for (int c = lane; c < ne0; c += blockDim.x) {
        const float v = xr[c];
        sum += v * v;
    }
    nrm_red[lane] = sum;
    __syncthreads();
    for (int off = blockDim.x >> 1; off > 0; off >>= 1) {
        if (lane < off) nrm_red[lane] += nrm_red[lane + off];
        __syncthreads();
    }
    sum = nrm_red[0];
    float denom;
    if (L2) {
        denom = 1.0f / sqrtf(fmaxf(sum, eps));
    } else {
        denom = rsqrtf(sum / (float) ne0 + eps);
    }
    for (int c = lane; c < ne0; c += blockDim.x) {
        dr[c] = xr[c] * denom;
    }
}

extern "C" cudaError_t mkllm_norm(
        int l2, const void * x, void * dst,
        int ne0, int ne1, int ne2, int ne3, float eps,
        size_t x_nb1, size_t x_nb2, size_t x_nb3,
        size_t d_nb1, size_t d_nb2, size_t d_nb3, cudaStream_t stream) {
    // llama.cpp norm.cu:297-307: 256 threads if ncols<1024, else 1024.
    const int nthreads = (!l2 && ne0 >= 1024) ? 1024 : 256;
    dim3 block((unsigned) nthreads);
    dim3 grid(ne1 * ne2 * ne3);
    const size_t shared = (size_t) nthreads * sizeof(float);
    if (l2) {
        mkllm_norm_kernel<1><<<grid, block, shared, stream>>>(
            (const uint8_t *) x, (uint8_t *) dst, ne0, ne1, ne2, eps,
            x_nb1, x_nb2, x_nb3, d_nb1, d_nb2, d_nb3);
    } else {
        mkllm_norm_kernel<0><<<grid, block, shared, stream>>>(
            (const uint8_t *) x, (uint8_t *) dst, ne0, ne1, ne2, eps,
            x_nb1, x_nb2, x_nb3, d_nb1, d_nb2, d_nb3);
    }
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// rope multi (MROPE/IMROPE), f32, transcribed from kernel_rope_multi.
// src [ne0, ne1, ne2, ne3] (dim0 = head_dim, ne2 = tokens), pos i32 with 4
// planes of ne2 entries.
// ---------------------------------------------------------------------------

static __device__ __forceinline__ float mkllm_rope_yarn_ramp(float low, float high, int i0) {
    const float y = ((float) (i0 / 2) - low) / fmaxf(0.001f, high - low);
    return 1.0f - fminf(1.0f, fmaxf(0.0f, y));
}

static __global__ void mkllm_rope_multi_kernel(
        const uint8_t * __restrict__ src, const int32_t * __restrict__ pos,
        uint8_t * __restrict__ dst,
        int ne0, int ne1, int ne2, int is_imrope,
        int n_dims, int sect_0, int sect_1, int sect_2, int sect_3,
        float freq_base, float freq_scale, float ext_factor, float attn_factor,
        float corr_dim0, float corr_dim1,
        size_t s_nb0, size_t s_nb1, size_t s_nb2, size_t s_nb3,
        size_t d_nb0, size_t d_nb1, size_t d_nb2, size_t d_nb3) {
    const int i1 = blockIdx.x;
    const int i2 = blockIdx.y;
    const int i3 = blockIdx.z;
    const float inv_ndims = -1.0f / (float) n_dims;

    for (int i0 = 2 * threadIdx.x; i0 < ne0; i0 += 2 * blockDim.x) {
        if (i0 < n_dims) {
            const int ic = i0 / 2;
            const int sect_dims = sect_0 + sect_1 + sect_2 + sect_3;
            const int sec_w01 = sect_0 + sect_1;
            const int sec_w012 = sect_0 + sect_1 + sect_2;
            const int sector = ic % sect_dims;
            float theta_base;
            if (is_imrope) {
                if (sector % 3 == 1 && sector < 3 * sect_1) {
                    theta_base = (float) pos[i2 + ne2 * 1];
                } else if (sector % 3 == 2 && sector < 3 * sect_2) {
                    theta_base = (float) pos[i2 + ne2 * 2];
                } else if (sector % 3 == 0 && sector < 3 * sect_0) {
                    theta_base = (float) pos[i2 + ne2 * 0];
                } else {
                    theta_base = (float) pos[i2 + ne2 * 3];
                }
            } else {
                if (sector < sect_0) {
                    theta_base = (float) pos[i2];
                } else if (sector < sec_w01) {
                    theta_base = (float) pos[i2 + ne2 * 1];
                } else if (sector < sec_w012) {
                    theta_base = (float) pos[i2 + ne2 * 2];
                } else {
                    theta_base = (float) pos[i2 + ne2 * 3];
                }
            }
            const float theta_extrap = theta_base * powf(freq_base, inv_ndims * (float) i0);
            float mscale = attn_factor;
            float theta = freq_scale * theta_extrap;
            if (ext_factor != 0.0f) {
                const float ramp = mkllm_rope_yarn_ramp(corr_dim0, corr_dim1, i0) * ext_factor;
                theta = theta * (1.0f - ramp) + theta_extrap * ramp;
                mscale *= 1.0f + 0.1f * logf(1.0f / freq_scale);
            }
            const float cos_t = cosf(theta) * mscale;
            const float sin_t = sinf(theta) * mscale;
            const float * sp = (const float *) (src + (size_t) i3 * s_nb3 + (size_t) i2 * s_nb2
                + (size_t) i1 * s_nb1 + (size_t) ic * s_nb0);
            float * dp = (float *) (dst + (size_t) i3 * d_nb3 + (size_t) i2 * d_nb2
                + (size_t) i1 * d_nb1 + (size_t) ic * d_nb0);
            const float x0 = sp[0];
            const float x1 = sp[n_dims / 2];
            dp[0] = x0 * cos_t - x1 * sin_t;
            dp[n_dims / 2] = x0 * sin_t + x1 * cos_t;
        } else {
            const float * sp = (const float *) (src + (size_t) i3 * s_nb3 + (size_t) i2 * s_nb2
                + (size_t) i1 * s_nb1 + (size_t) i0 * s_nb0);
            float * dp = (float *) (dst + (size_t) i3 * d_nb3 + (size_t) i2 * d_nb2
                + (size_t) i1 * d_nb1 + (size_t) i0 * d_nb0);
            dp[0] = sp[0];
            dp[1] = sp[1];
        }
    }
}

extern "C" cudaError_t mkllm_rope_multi(
        const void * src, const int32_t * pos, void * dst,
        int ne0, int ne1, int ne2, int ne3, int is_imrope,
        int n_dims, int sect_0, int sect_1, int sect_2, int sect_3,
        float freq_base, float freq_scale, float ext_factor, float attn_factor,
        float corr_dim0, float corr_dim1,
        size_t s_nb0, size_t s_nb1, size_t s_nb2, size_t s_nb3,
        size_t d_nb0, size_t d_nb1, size_t d_nb2, size_t d_nb3,
        cudaStream_t stream) {
    dim3 block(64);
    dim3 grid(ne1, ne2, ne3);
    mkllm_rope_multi_kernel<<<grid, block, 0, stream>>>(
        (const uint8_t *) src, pos, (uint8_t *) dst,
        ne0, ne1, ne2, is_imrope, n_dims, sect_0, sect_1, sect_2, sect_3,
        freq_base, freq_scale, ext_factor, attn_factor, corr_dim0, corr_dim1,
        s_nb0, s_nb1, s_nb2, s_nb3, d_nb0, d_nb1, d_nb2, d_nb3);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Elementwise: unary, glu (split), binary with ggml modulo-broadcast.
// ---------------------------------------------------------------------------

#define MKLLM_UNARY_ABS 0
#define MKLLM_UNARY_SGN 1
#define MKLLM_UNARY_NEG 2
#define MKLLM_UNARY_STEP 3
#define MKLLM_UNARY_TANH 4
#define MKLLM_UNARY_ELU 5
#define MKLLM_UNARY_RELU 6
#define MKLLM_UNARY_SIGMOID 7
#define MKLLM_UNARY_GELU 8
#define MKLLM_UNARY_GELU_QUICK 9
#define MKLLM_UNARY_SILU 10
#define MKLLM_UNARY_HARDSWISH 11
#define MKLLM_UNARY_HARDSIGMOID 12
#define MKLLM_UNARY_EXP 13
#define MKLLM_UNARY_EXPM1 14
#define MKLLM_UNARY_SOFTPLUS 15
#define MKLLM_UNARY_GELU_ERF 16

static __device__ __forceinline__ float mkllm_unary_apply(int op, float x) {
    switch (op) {
        case MKLLM_UNARY_ABS: return fabsf(x);
        case MKLLM_UNARY_SGN: return x > 0.0f ? 1.0f : (x < 0.0f ? -1.0f : 0.0f);
        case MKLLM_UNARY_NEG: return -x;
        case MKLLM_UNARY_STEP: return x > 0.0f ? 1.0f : 0.0f;
        case MKLLM_UNARY_TANH: return tanhf(x);
        case MKLLM_UNARY_ELU: return x > 0.0f ? x : expm1f(x);
        case MKLLM_UNARY_RELU: return fmaxf(x, 0.0f);
        case MKLLM_UNARY_SIGMOID: return 1.0f / (1.0f + expf(-x));
        case MKLLM_UNARY_GELU:
            return 0.5f * x * (1.0f + tanhf(0.79788456080286535588f * (x + 0.044715f * x * x * x)));
        case MKLLM_UNARY_GELU_QUICK: return x * (1.0f / (1.0f + expf(-1.702f * x)));
        case MKLLM_UNARY_SILU: return x / (1.0f + expf(-x));
        case MKLLM_UNARY_HARDSWISH: return x * fminf(1.0f, fmaxf(0.0f, (x + 3.0f) / 6.0f));
        case MKLLM_UNARY_HARDSIGMOID: return fminf(1.0f, fmaxf(0.0f, (x + 3.0f) / 6.0f));
        case MKLLM_UNARY_EXP: return expf(x);
        case MKLLM_UNARY_EXPM1: return expm1f(x);
        case MKLLM_UNARY_SOFTPLUS: return x > 20.0f ? x : log1pf(expf(x));
        case MKLLM_UNARY_GELU_ERF: return 0.5f * x * (1.0f + erff(x * 0.70710678118654752440f));
        default: return x;
    }
}

static __global__ void mkllm_unary_kernel(
        const float * __restrict__ x, float * __restrict__ dst, size_t n, int op) {
    const size_t i = (size_t) blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
    dst[i] = mkllm_unary_apply(op, x[i]);
}

extern "C" cudaError_t mkllm_unary(
        const void * x, void * dst, size_t n, int op, cudaStream_t stream) {
    const int block = 256;
    const int grid = (int) ((n + block - 1) / block);
    mkllm_unary_kernel<<<grid, block, 0, stream>>>((const float *) x, (float *) dst, n, op);
    return cudaGetLastError();
}

// llama.cpp unary.cu:254-273 unary_gated_op_kernel / unary_gated_cuda,
// launched by ggml_cuda_op_unary_mul (unary.cu:603) for SILU/SIGMOID/SOFTPLUS.
// k = nelements, n = ncols (ne[0]), o0/o1 = row strides in elements.
static __global__ void mkllm_unary_mul_kernel(
        const float * __restrict__ x, const float * __restrict__ g,
        float * __restrict__ dst, size_t k, int n, int op,
        size_t o0, size_t o1) {
    const size_t i = (size_t) blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= k) return;
    const size_t j0 = (i / (size_t) n) * o0 + (i % (size_t) n);
    const size_t j1 = o0 == o1 ? j0 : (i / (size_t) n) * o1 + (i % (size_t) n);
    dst[i] = mkllm_unary_apply(op, x[j0]) * g[j1];
}

extern "C" cudaError_t mkllm_unary_mul(
        const void * x, const void * g, void * dst,
        size_t k, int n, int op, size_t o0, size_t o1, cudaStream_t stream) {
    if (k == 0 || n <= 0) {
        return cudaErrorInvalidValue;
    }
    const int block = 256;
    const int grid = (int) ((k + (size_t) block - 1) / (size_t) block);
    mkllm_unary_mul_kernel<<<grid, block, 0, stream>>>(
        (const float *) x, (const float *) g, (float *) dst, k, n, op, o0, o1);
    return cudaGetLastError();
}

#define MKLLM_GLU_REGLU 0
#define MKLLM_GLU_GEGLU 1
#define MKLLM_GLU_SWIGLU 2
#define MKLLM_GLU_SWIGLU_OAI 3
#define MKLLM_GLU_GEGLU_ERF 4
#define MKLLM_GLU_GEGLU_QUICK 5

static __global__ void mkllm_glu_kernel(
        const float * __restrict__ a, const float * __restrict__ b,
        float * __restrict__ dst, size_t n, int op) {
    const size_t i = (size_t) blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
    const float x = a[i];
    float g;
    switch (op) {
        case MKLLM_GLU_REGLU: g = fmaxf(x, 0.0f); break;
        case MKLLM_GLU_GEGLU:
            g = 0.5f * x * (1.0f + tanhf(0.79788456080286535588f * (x + 0.044715f * x * x * x)));
            break;
        case MKLLM_GLU_SWIGLU: g = x / (1.0f + expf(-x)); break;
        case MKLLM_GLU_GEGLU_ERF: g = 0.5f * x * (1.0f + erff(x * 0.70710678118654752440f)); break;
        case MKLLM_GLU_GEGLU_QUICK: g = x * (1.0f / (1.0f + expf(-1.702f * x))); break;
        default: g = x / (1.0f + expf(-x)); break;
    }
    dst[i] = g * b[i];
}

extern "C" cudaError_t mkllm_glu(
        const void * a, const void * b, void * dst, size_t n, int op, cudaStream_t stream) {
    const int block = 256;
    const int grid = (int) ((n + block - 1) / block);
    mkllm_glu_kernel<<<grid, block, 0, stream>>>(
        (const float *) a, (const float *) b, (float *) dst, n, op);
    return cudaGetLastError();
}

#define MKLLM_BIN_ADD 0
#define MKLLM_BIN_SUB 1
#define MKLLM_BIN_MUL 2
#define MKLLM_BIN_DIV 3

static __global__ void mkllm_binary_kernel(
        const uint8_t * __restrict__ a, const uint8_t * __restrict__ b,
        uint8_t * __restrict__ dst, int op,
        int ne0, int ne1, int ne2, int ne3,
        int b_ne0, int b_ne1, int b_ne2, int b_ne3,
        size_t a_nb0, size_t a_nb1, size_t a_nb2, size_t a_nb3,
        size_t b_nb0, size_t b_nb1, size_t b_nb2, size_t b_nb3,
        size_t d_nb0, size_t d_nb1, size_t d_nb2, size_t d_nb3) {
    const int i0 = blockIdx.x * blockDim.x + threadIdx.x;
    if (i0 >= ne0) return;
    // Rows on y, higher dims on z, each grid-strided: ne1 * ne2 * ne3 packed
    // into one grid dimension overflows the 65535 limit on y/z at shapes this
    // runtime really produces (BS-RoFormer's trunk is [384, 1101, 62], so the
    // packed form asks for 68262 blocks and the launch fails outright with
    // "invalid argument"). Striding keeps every shape launchable.
    const int n23 = ne2 * ne3;
    for (int i23 = blockIdx.z; i23 < n23; i23 += gridDim.z) {
        const int i2 = i23 % ne2;
        const int i3 = i23 / ne2;
        for (int i1 = blockIdx.y; i1 < ne1; i1 += gridDim.y) {
            const float av = *(const float *) (a + (size_t) i3 * a_nb3 + (size_t) i2 * a_nb2
                + (size_t) i1 * a_nb1 + (size_t) i0 * a_nb0);
            const float bv = *(const float *) (b + (size_t) (i3 % b_ne3) * b_nb3
                + (size_t) (i2 % b_ne2) * b_nb2 + (size_t) (i1 % b_ne1) * b_nb1
                + (size_t) (i0 % b_ne0) * b_nb0);
            float r;
            switch (op) {
                case MKLLM_BIN_ADD: r = av + bv; break;
                case MKLLM_BIN_SUB: r = av - bv; break;
                case MKLLM_BIN_MUL: r = av * bv; break;
                default: r = av / bv; break;
            }
            *(float *) (dst + (size_t) i3 * d_nb3 + (size_t) i2 * d_nb2 + (size_t) i1 * d_nb1
                + (size_t) i0 * d_nb0) = r;
        }
    }
}

extern "C" cudaError_t mkllm_binary(
        int op, const void * a, const void * b, void * dst,
        int ne0, int ne1, int ne2, int ne3,
        int b_ne0, int b_ne1, int b_ne2, int b_ne3,
        size_t a_nb0, size_t a_nb1, size_t a_nb2, size_t a_nb3,
        size_t b_nb0, size_t b_nb1, size_t b_nb2, size_t b_nb3,
        size_t d_nb0, size_t d_nb1, size_t d_nb2, size_t d_nb3,
        cudaStream_t stream) {
    dim3 block(256);
    const unsigned max_dim = 65535;
    dim3 grid(
        (unsigned) ((ne0 + 255) / 256),
        (unsigned) (ne1 > 0 ? (ne1 < (int) max_dim ? ne1 : (int) max_dim) : 1),
        (unsigned) (ne2 * ne3 > 0 ? (ne2 * ne3 < (int) max_dim ? ne2 * ne3 : (int) max_dim) : 1));
    mkllm_binary_kernel<<<grid, block, 0, stream>>>(
        (const uint8_t *) a, (const uint8_t *) b, (uint8_t *) dst, op,
        ne0, ne1, ne2, ne3, b_ne0, b_ne1, b_ne2, b_ne3,
        a_nb0, a_nb1, a_nb2, a_nb3, b_nb0, b_nb1, b_nb2, b_nb3,
        d_nb0, d_nb1, d_nb2, d_nb3);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// gated_delta_net — exact transcription of kernel_gated_delta_net_impl in
// libs/ggml/src/backend/metal/ggml/ggml-metal.metal (the oracle semantics):
// value-head -> q/k-head mapping is MODULO (i01 = i21 % ne01), the gate
// decays the state BEFORE the k-dot, beta/gate are indexed flat [G, H, T, B],
// and the state is stored transposed (column-major rows contiguous).
// One warp per state column; each lane holds sv/32 state elements.
// ---------------------------------------------------------------------------

// llama.cpp gated_delta_net.cu S_v=128: one warp per column, coalesced
// i = r*32+lane, 4 warps/block, launch_bounds(128,2). Same recurrence as
// the generic kernel below (KDA and scalar-gate both).
template <int GATE_G>
static __global__ void __launch_bounds__(128, 2) mkllm_gdn_sv128_kernel(
        const float * __restrict__ q, const float * __restrict__ k,
        const float * __restrict__ v, const float * __restrict__ g,
        const float * __restrict__ b, const float * __restrict__ s,
        float * __restrict__ dst,
        int h, int n_tokens, int n_seqs,
        int q_heads, int k_heads,
        size_t q_h_elems, size_t q_t_elems, size_t q_s_elems,
        size_t k_h_elems, size_t k_t_elems, size_t k_s_elems,
        size_t v_h_elems, size_t v_t_elems, size_t v_s_elems) {
    constexpr int sv = 128;
    constexpr int warp_size = 32;
    constexpr int rows_per_lane = sv / warp_size;
    const int col = (int) blockIdx.x * (int) blockDim.y + (int) threadIdx.y;
    const int head = (int) blockIdx.y;
    const int seq = (int) blockIdx.z;
    const int lane = (int) threadIdx.x;
    if (col >= sv) {
        return;
    }
    const int i01 = head % q_heads;
    const int i11 = head % k_heads;
    const float scale = rsqrtf((float) sv);
    const float * s_ptr = s + ((size_t) seq * h + head) * sv * sv + (size_t) col * sv;
    float s_shard[rows_per_lane];
#pragma unroll
    for (int r = 0; r < rows_per_lane; ++r) {
        s_shard[r] = s_ptr[r * warp_size + lane];
    }
    const float * q_ptr = q + (size_t) seq * q_s_elems + (size_t) i01 * q_h_elems;
    const float * k_ptr = k + (size_t) seq * k_s_elems + (size_t) i11 * k_h_elems;
    const float * v_ptr = v + (size_t) seq * v_s_elems + (size_t) head * v_h_elems;
    const float * b_ptr = b + ((size_t) seq * n_tokens * h + head);
    const float * g_ptr = g + ((size_t) seq * n_tokens * h + head) * GATE_G;
    float * dst_attn = dst + ((size_t) seq * n_tokens * h + head) * sv + col;
    for (int t = 0; t < n_tokens; ++t) {
        float k_reg[rows_per_lane];
        float q_reg[rows_per_lane];
#pragma unroll
        for (int r = 0; r < rows_per_lane; ++r) {
            const int i = r * warp_size + lane;
            k_reg[r] = k_ptr[i];
            q_reg[r] = q_ptr[i];
        }
        float s_k = 0.0f;
        if (GATE_G == 1) {
            const float g_exp = expf(g_ptr[0]);
#pragma unroll
            for (int r = 0; r < rows_per_lane; ++r) {
                s_shard[r] *= g_exp;
                s_k += s_shard[r] * k_reg[r];
            }
        } else {
#pragma unroll
            for (int r = 0; r < rows_per_lane; ++r) {
                const int i = r * warp_size + lane;
                s_shard[r] *= expf(g_ptr[i]);
                s_k += s_shard[r] * k_reg[r];
            }
        }
#pragma unroll
        for (int off = 16; off > 0; off >>= 1) {
            s_k += __shfl_xor_sync(0xffffffff, s_k, off);
        }
        const float d = (v_ptr[col] - s_k) * b_ptr[0];
        float y = 0.0f;
#pragma unroll
        for (int r = 0; r < rows_per_lane; ++r) {
            s_shard[r] += k_reg[r] * d;
            y += s_shard[r] * q_reg[r];
        }
#pragma unroll
        for (int off = 16; off > 0; off >>= 1) {
            y += __shfl_xor_sync(0xffffffff, y, off);
        }
        if (lane == 0) {
            dst_attn[(size_t) t * h * sv] = y * scale;
        }
        q_ptr += q_t_elems;
        k_ptr += k_t_elems;
        v_ptr += v_t_elems;
        b_ptr += h;
        g_ptr += (size_t) h * GATE_G;
    }
    float * dst_state = dst + (size_t) n_seqs * n_tokens * h * sv
        + ((size_t) seq * h + head) * sv * sv + (size_t) col * sv;
#pragma unroll
    for (int r = 0; r < rows_per_lane; ++r) {
        dst_state[r * warp_size + lane] = s_shard[r];
    }
}

static __global__ void mkllm_gdn_kernel(
        const float * __restrict__ q, const float * __restrict__ k,
        const float * __restrict__ v, const float * __restrict__ g,
        const float * __restrict__ b, const float * __restrict__ s,
        float * __restrict__ dst,
        int sv, int h, int n_tokens, int n_seqs, int gate_g,
        int q_heads, int k_heads,
        size_t q_h_elems, size_t q_t_elems, size_t q_s_elems,
        size_t k_h_elems, size_t k_t_elems, size_t k_s_elems,
        size_t v_h_elems, size_t v_t_elems, size_t v_s_elems) {
    const int per_lane = sv / 32;
    const int col = blockIdx.x * blockDim.y + threadIdx.y;
    const int head = blockIdx.y;
    const int seq = blockIdx.z;
    const int lane = threadIdx.x;
    if (col >= sv) return;

    const int i01 = head % q_heads;
    const int i11 = head % k_heads;
    const float scale = rsqrtf((float) sv);

    const float * s_ptr = s + ((size_t) seq * h + head) * sv * sv + (size_t) col * sv;
    float ls[8]; // sv <= 256
    for (int j = 0; j < per_lane; j++) {
        ls[j] = s_ptr[lane * per_lane + j];
    }

    const float * q_ptr = q + (size_t) seq * q_s_elems + (size_t) i01 * q_h_elems;
    const float * k_ptr = k + (size_t) seq * k_s_elems + (size_t) i11 * k_h_elems;
    const float * v_ptr = v + (size_t) seq * v_s_elems + (size_t) head * v_h_elems;
    const float * b_ptr = b + ((size_t) seq * n_tokens * h + head);
    const float * g_ptr = g + ((size_t) seq * n_tokens * h + head) * gate_g;
    float * dst_attn = dst + ((size_t) seq * n_tokens * h + head) * sv + col;

    for (int t = 0; t < n_tokens; t++) {
        float s_k = 0.0f;
        if (gate_g == 1) {
            const float g_exp = expf(g_ptr[0]);
            for (int j = 0; j < per_lane; j++) {
                const int is = lane * per_lane + j;
                ls[j] *= g_exp;
                s_k += ls[j] * k_ptr[is];
            }
        } else {
            for (int j = 0; j < per_lane; j++) {
                const int is = lane * per_lane + j;
                ls[j] *= expf(g_ptr[is]);
                s_k += ls[j] * k_ptr[is];
            }
        }
        for (int off = 16; off > 0; off >>= 1) {
            s_k += __shfl_xor_sync(0xffffffff, s_k, off);
        }
        const float d = (v_ptr[col] - s_k) * b_ptr[0];
        float y = 0.0f;
        for (int j = 0; j < per_lane; j++) {
            const int is = lane * per_lane + j;
            ls[j] += k_ptr[is] * d;
            y += ls[j] * q_ptr[is];
        }
        for (int off = 16; off > 0; off >>= 1) {
            y += __shfl_xor_sync(0xffffffff, y, off);
        }
        if (lane == 0) {
            dst_attn[(size_t) t * h * sv] = y * scale;
        }
        q_ptr += q_t_elems;
        k_ptr += k_t_elems;
        v_ptr += v_t_elems;
        b_ptr += h;
        g_ptr += (size_t) h * gate_g;
    }

    float * dst_state = dst + (size_t) n_seqs * n_tokens * h * sv
        + ((size_t) seq * h + head) * sv * sv + (size_t) col * sv;
    for (int j = 0; j < per_lane; j++) {
        dst_state[lane * per_lane + j] = ls[j];
    }
}

#include "fattn/gated_delta_net.cuh"

extern "C" cudaError_t mkllm_gated_delta_net(
        const void * q, const void * k, const void * v, const void * g,
        const void * b, const void * s, void * dst,
        int sv, int h, int n_tokens, int n_seqs, int gate_g,
        int q_heads, int k_heads,
        size_t q_h_elems, size_t q_t_elems, size_t q_s_elems,
        size_t k_h_elems, size_t k_t_elems, size_t k_s_elems,
        size_t v_h_elems, size_t v_t_elems, size_t v_s_elems,
        int state_checkpoints,
        cudaStream_t stream) {
    if (sv % 32 != 0 || sv > 256) {
        return cudaErrorInvalidValue;
    }
    // Non-zero => write one state per token, `sv*sv*h*n_seqs` floats apart.
    const int64_t state_ckpt_stride = state_checkpoints
        ? (int64_t) sv * sv * h * n_seqs
        : 0;
    const bool official_sv = sv == 16 || sv == 32 || sv == 64 || sv == 128;
    const bool official_ok = official_sv && q_heads == k_heads
        && q_h_elems == k_h_elems && q_t_elems == k_t_elems
        && q_s_elems == k_s_elems && (gate_g == 1 || gate_g == sv)
        && h > 0 && n_tokens > 0 && n_seqs > 0 && q_heads > 0;
    if (official_ok) {
        const float scale = 1.0f / sqrtf((float) sv);
        const int64_t sb1 = 1;
        const int64_t sb2 = h;
        const int64_t sb3 = (int64_t) n_tokens * h;
        const int64_t rq3 = 1;
        if (gate_g == sv) {
            launch_gated_delta_net<true>(
                (const float *) q, (const float *) k, (const float *) v,
                (const float *) g, (const float *) b, (const float *) s,
                (float *) dst, sv, h, n_tokens, n_seqs,
                (int64_t) q_h_elems, (int64_t) q_t_elems, (int64_t) q_s_elems,
                (int64_t) v_h_elems, (int64_t) v_t_elems, (int64_t) v_s_elems,
                sb1, sb2, sb3, q_heads, rq3, scale, state_ckpt_stride, stream);
        } else {
            launch_gated_delta_net<false>(
                (const float *) q, (const float *) k, (const float *) v,
                (const float *) g, (const float *) b, (const float *) s,
                (float *) dst, sv, h, n_tokens, n_seqs,
                (int64_t) q_h_elems, (int64_t) q_t_elems, (int64_t) q_s_elems,
                (int64_t) v_h_elems, (int64_t) v_t_elems, (int64_t) v_s_elems,
                sb1, sb2, sb3, q_heads, rq3, scale, state_ckpt_stride, stream);
        }
        return cudaGetLastError();
    }
    if (state_checkpoints) {
        // Only the official kernel emits per-token states; fail closed rather
        // than silently leaving the checkpoint rows unwritten.
        return cudaErrorInvalidValue;
    }
    const int cols_per_block = 4;
    dim3 block(32, cols_per_block);
    dim3 grid((sv + cols_per_block - 1) / cols_per_block, h, n_seqs);
    if (sv == 128 && gate_g == 1) {
        mkllm_gdn_sv128_kernel<1><<<grid, block, 0, stream>>>(
            (const float *) q, (const float *) k, (const float *) v, (const float *) g,
            (const float *) b, (const float *) s, (float *) dst,
            h, n_tokens, n_seqs, q_heads, k_heads,
            q_h_elems, q_t_elems, q_s_elems, k_h_elems, k_t_elems, k_s_elems,
            v_h_elems, v_t_elems, v_s_elems);
    } else if (sv == 128 && gate_g == 128) {
        mkllm_gdn_sv128_kernel<128><<<grid, block, 0, stream>>>(
            (const float *) q, (const float *) k, (const float *) v, (const float *) g,
            (const float *) b, (const float *) s, (float *) dst,
            h, n_tokens, n_seqs, q_heads, k_heads,
            q_h_elems, q_t_elems, q_s_elems, k_h_elems, k_t_elems, k_s_elems,
            v_h_elems, v_t_elems, v_s_elems);
    } else {
        mkllm_gdn_kernel<<<grid, block, 0, stream>>>(
            (const float *) q, (const float *) k, (const float *) v, (const float *) g,
            (const float *) b, (const float *) s, (float *) dst,
            sv, h, n_tokens, n_seqs, gate_g, q_heads, k_heads,
            q_h_elems, q_t_elems, q_s_elems, k_h_elems, k_t_elems, k_s_elems,
            v_h_elems, v_t_elems, v_s_elems);
    }
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// ssm_conv (ggml op layout): src0 [T + d_conv - 1, d_inner, n_seqs] with the
// position axis on dim0, weight [d_conv, d_inner], dst [d_inner, T, n_seqs].
//   dst(i, t, s) = sum_k src0(t + k, i, s) * w(k, i)
// ---------------------------------------------------------------------------

// llama.cpp ssm-conv.cu:3-47 ssm_conv_f32, apply_silu from
// ggml-cuda.cu:4006 GGML_OP_SSM_CONV + SILU.
template <int apply_silu>
static __global__ void mkllm_ssm_conv_kernel(
        const uint8_t * __restrict__ src0, const uint8_t * __restrict__ weight,
        uint8_t * __restrict__ dst,
        int d_conv, int d_inner, int n_tokens,
        size_t s_nb0, size_t s_nb1, size_t s_nb2,
        size_t w_nb1, size_t d_nb0, size_t d_nb1, size_t d_nb2) {
    const int i = blockIdx.x * blockDim.x + threadIdx.x;
    const int t = blockIdx.y;
    const int s = blockIdx.z;
    if (i >= d_inner) return;
    (void) n_tokens;
    const uint8_t * base = src0 + (size_t) s * s_nb2 + (size_t) i * s_nb1;
    const float * w = (const float *) (weight + (size_t) i * w_nb1);
    float sum = 0.0f;
    for (int k = 0; k < d_conv; k++) {
        sum += *(const float *) (base + (size_t) (t + k) * s_nb0) * w[k];
    }
    if (apply_silu) {
        sum = sum / (1.0f + expf(-sum));
    }
    *(float *) (dst + (size_t) s * d_nb2 + (size_t) t * d_nb1 + (size_t) i * d_nb0) = sum;
}

extern "C" cudaError_t mkllm_ssm_conv(
        const void * src0, const void * weight, void * dst,
        int d_conv, int d_inner, int n_tokens, int n_seqs, int apply_silu,
        size_t s_nb0, size_t s_nb1, size_t s_nb2,
        size_t w_nb1, size_t d_nb0, size_t d_nb1, size_t d_nb2,
        cudaStream_t stream) {
    dim3 block(256);
    dim3 grid((d_inner + 255) / 256, n_tokens, n_seqs);
    if (apply_silu) {
        mkllm_ssm_conv_kernel<1><<<grid, block, 0, stream>>>(
            (const uint8_t *) src0, (const uint8_t *) weight, (uint8_t *) dst,
            d_conv, d_inner, n_tokens, s_nb0, s_nb1, s_nb2, w_nb1, d_nb0, d_nb1, d_nb2);
    } else {
        mkllm_ssm_conv_kernel<0><<<grid, block, 0, stream>>>(
            (const uint8_t *) src0, (const uint8_t *) weight, (uint8_t *) dst,
            d_conv, d_inner, n_tokens, s_nb0, s_nb1, s_nb2, w_nb1, d_nb0, d_nb1, d_nb2);
    }
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Strided copy (Cont / Concat lowering / Cpy): element-typed, src and dst
// arbitrary strides, same element size (2 or 4 bytes).
// ---------------------------------------------------------------------------

// Source and destination may have DIFFERENT extents with the same element
// count (Cont can flatten/reshape while materializing). Each thread owns one
// flat logical index and decomposes it independently through the source and
// destination extents before applying that side's byte strides.
template <typename T>
static __global__ void mkllm_copy_strided_kernel(
        const uint8_t * __restrict__ src, uint8_t * __restrict__ dst,
        size_t total,
        int s_ne0, int s_ne1, int s_ne2,
        int d_ne0, int d_ne1, int d_ne2,
        size_t s_nb0, size_t s_nb1, size_t s_nb2, size_t s_nb3,
        size_t d_nb0, size_t d_nb1, size_t d_nb2, size_t d_nb3) {
    const size_t flat = (size_t) blockIdx.x * blockDim.x + threadIdx.x;
    if (flat >= total) return;

    size_t rem = flat;
    const size_t si0 = rem % s_ne0; rem /= s_ne0;
    const size_t si1 = rem % s_ne1; rem /= s_ne1;
    const size_t si2 = rem % s_ne2;
    const size_t si3 = rem / s_ne2;

    rem = flat;
    const size_t di0 = rem % d_ne0; rem /= d_ne0;
    const size_t di1 = rem % d_ne1; rem /= d_ne1;
    const size_t di2 = rem % d_ne2;
    const size_t di3 = rem / d_ne2;

    *(T *) (dst + di3 * d_nb3 + di2 * d_nb2 + di1 * d_nb1 + di0 * d_nb0) =
        *(const T *) (src + si3 * s_nb3 + si2 * s_nb2 + si1 * s_nb1 + si0 * s_nb0);
}

extern "C" cudaError_t mkllm_copy_strided(
        int elem_size, const void * src, void * dst,
        int s_ne0, int s_ne1, int s_ne2, int s_ne3,
        int d_ne0, int d_ne1, int d_ne2, int d_ne3,
        size_t s_nb0, size_t s_nb1, size_t s_nb2, size_t s_nb3,
        size_t d_nb0, size_t d_nb1, size_t d_nb2, size_t d_nb3,
        cudaStream_t stream) {
    const size_t total = (size_t) d_ne0 * d_ne1 * d_ne2 * d_ne3;
    const size_t src_total = (size_t) s_ne0 * s_ne1 * s_ne2 * s_ne3;
    if (total != src_total) {
        return cudaErrorInvalidValue;
    }
    dim3 block(256);
    dim3 grid((unsigned) ((total + 255) / 256));
    if (elem_size == 2) {
        mkllm_copy_strided_kernel<uint16_t><<<grid, block, 0, stream>>>(
            (const uint8_t *) src, (uint8_t *) dst, total,
            s_ne0, s_ne1, s_ne2, d_ne0, d_ne1, d_ne2,
            s_nb0, s_nb1, s_nb2, s_nb3, d_nb0, d_nb1, d_nb2, d_nb3);
    } else {
        mkllm_copy_strided_kernel<uint32_t><<<grid, block, 0, stream>>>(
            (const uint8_t *) src, (uint8_t *) dst, total,
            s_ne0, s_ne1, s_ne2, d_ne0, d_ne1, d_ne2,
            s_nb0, s_nb1, s_nb2, s_nb3, d_nb0, d_nb1, d_nb2, d_nb3);
    }
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// llama.cpp fattn-mma-f16 for Qwen3.8-27B prefill: D=256, GQA>4, n_tokens
// large -> ncols1=8, ncols2=8 (fattn.cu switch_ncols2 / switch_ncols1).
// Ampere config is used on sm86/89/120. Ada stream-K when cc>=890.
// (device lambdas rewritten as functors; host sees Ampere configs.)
// ---------------------------------------------------------------------------

#include "fattn/fattn-mma-f16.cuh"

static void mkllm_fattn_trace_launch(
        const char * kind, int D, int n_q, int kc, int H, int Hkv,
        int nsm, int max_blocks_per_sm, int parallel_blocks, int ntiles_dst,
        int ntiles_KV, int stream_k, unsigned gx, unsigned gy, unsigned gz) {
    // First few host launches only. Official launch_fattn (fattn-common.cuh:990)
    // for VEC is stream_k=false; pb is occupancy-capped then efficiency-swept.
    static int prints = 0;
    if (prints >= 8) {
        return;
    }
    fprintf(stderr,
        "fattn.launch: kind=%s D=%d n_q=%d n_kv=%d H=%d Hkv=%d nsm=%d occ=%d "
        "pb=%d ntiles_dst=%d ntiles_KV=%d stream_k=%d grid=(%u,%u,%u) "
        "pad256=%d\n",
        kind, D, n_q, kc, H, Hkv, nsm, max_blocks_per_sm, parallel_blocks,
        ntiles_dst, ntiles_KV, stream_k, gx, gy, gz,
        (kc + 255) & ~255);
    ++prints;
}

extern "C" cudaError_t mkllm_fattn_mma_f16(
        const void * q, const void * k, const void * v, const void * mask, void * dst,
        int D, int Dv, int kc, int n_q, int H, int Hkv, float scale,
        size_t q_nb1, size_t q_nb2, size_t k_nb1, size_t k_nb2,
        size_t v_nb1, size_t v_nb2, size_t m_nb1, size_t d_nb1, size_t d_nb2,
        int nsm, int cc, float * tmp_fixup, cudaStream_t stream) {
    (void) d_nb1;
    (void) d_nb2;
    if (D != 256 || Dv != 256 || n_q <= 0 || kc <= 0 || H <= 0 || Hkv <= 0
            || H % Hkv != 0 || q == nullptr || k == nullptr || v == nullptr
            || mask == nullptr || dst == nullptr) {
        return cudaErrorInvalidValue;
    }
    constexpr int DKQ = 256;
    constexpr int DV = 256;
    constexpr int ncols1 = 8;
    constexpr int ncols2 = 8;
    constexpr int ncols = ncols1 * ncols2;
    constexpr bool use_logit_softcap = false;
    constexpr bool V_is_K_view = false;
    const int nthreads = ggml_cuda_fattn_mma_get_nthreads(DKQ, DV, ncols, cc);
    const int nbatch_fa = ggml_cuda_fattn_mma_get_nbatch_fa(DKQ, DV, ncols, cc);
    const int nbatch_K2 = ggml_cuda_fattn_mma_get_nbatch_K2(DKQ, DV, ncols, cc);
    const int nbatch_V2 = ggml_cuda_fattn_mma_get_nbatch_V2(DKQ, DV, ncols, cc);
    const int nbatch_combine = ggml_cuda_fattn_mma_get_nbatch_combine(DKQ, DV, ncols, cc);
    const bool Q_in_reg = ggml_cuda_fattn_mma_get_Q_in_reg(DKQ, DV, ncols, cc);
    const int nstages = ggml_cuda_fattn_mma_get_nstages(DKQ, DV, ncols1, ncols2, cc);
    const int cols_per_warp = 16;
    const int nwarps = nthreads / 32;
    const size_t nbytes_shared_KV_1stage = (size_t) nbatch_fa
        * (size_t) (nbatch_K2 > nbatch_V2 ? nbatch_K2 + 4 : nbatch_V2 + 4) * sizeof(half2);
    const size_t nbytes_shared_KV_2stage = (size_t) nbatch_fa
        * (size_t) (nbatch_K2 + 4 + nbatch_V2 + 4) * sizeof(half2);
    const size_t nbytes_shared_Q = (size_t) ncols * (size_t) (DKQ / 2 + 4) * sizeof(half2);
    const size_t nbytes_shared_mask = (size_t) ncols1 * (size_t) (nbatch_fa / 2 + 4) * sizeof(half2);
    const size_t nbytes_shared_combine = (size_t) nwarps * (size_t) cols_per_warp
        * (size_t) (nbatch_combine + 4) * sizeof(half2);
    const size_t nbytes_shared_KV = nstages <= 1 ? nbytes_shared_KV_1stage : nbytes_shared_KV_2stage;
    const size_t nbytes_shared_total = nbytes_shared_combine > (Q_in_reg
        ? (nbytes_shared_Q > nbytes_shared_KV + nbytes_shared_mask
            ? nbytes_shared_Q : nbytes_shared_KV + nbytes_shared_mask)
        : nbytes_shared_Q + nbytes_shared_KV + nbytes_shared_mask)
        ? nbytes_shared_combine
        : (Q_in_reg
            ? (nbytes_shared_Q > nbytes_shared_KV + nbytes_shared_mask
                ? nbytes_shared_Q : nbytes_shared_KV + nbytes_shared_mask)
            : nbytes_shared_Q + nbytes_shared_KV + nbytes_shared_mask);

    fattn_kernel_t fattn_kernel =
        flash_attn_ext_f16<DKQ, DV, ncols1, ncols2, use_logit_softcap, V_is_K_view>;
    cudaError_t err = cudaFuncSetAttribute(
        fattn_kernel, cudaFuncAttributeMaxDynamicSharedMemorySize, (int) nbytes_shared_total);
    if (err != cudaSuccess) {
        return err;
    }

    const int ntiles_x = (n_q + ncols1 - 1) / ncols1;
    const int gqa_ratio = H / Hkv;
    const int ntiles_z_gqa = (gqa_ratio + ncols2 - 1) / ncols2;
    const int ntiles_dst = ntiles_x * ntiles_z_gqa * Hkv;
    const int ntiles_KV = (kc + nbatch_fa - 1) / nbatch_fa;
    dim3 block_dim(32, nwarps, 1);
    int max_blocks_per_sm = 1;
    err = cudaOccupancyMaxActiveBlocksPerMultiprocessor(
        &max_blocks_per_sm, fattn_kernel, block_dim.x * block_dim.y, nbytes_shared_total);
    if (err != cudaSuccess || max_blocks_per_sm <= 0) {
        max_blocks_per_sm = 1;
    }
    const bool ada_stream_k = cc >= GGML_CUDA_CC_ADA_LOVELACE;
    const int max_blocks = max_blocks_per_sm * (nsm > 0 ? nsm : 1);
    const int tiles_nwaves = (ntiles_dst + max_blocks - 1) / max_blocks;
    const int tiles_efficiency_percent = max_blocks * tiles_nwaves > 0
        ? 100 * ntiles_dst / (max_blocks * tiles_nwaves) : 0;
    const int nblocks_stream_k = ntiles_KV * ntiles_dst < max_blocks
        ? ntiles_KV * ntiles_dst : max_blocks;
    const bool use_stream_k = (ada_stream_k || tiles_efficiency_percent < 75)
        && tmp_fixup != nullptr && nsm > 0;
    dim3 blocks_num;
    if (use_stream_k) {
        blocks_num = dim3(nblocks_stream_k, 1, 1);
    } else {
        blocks_num = dim3(ntiles_x, 1, ntiles_z_gqa * Hkv);
    }
    mkllm_fattn_trace_launch(
        "mma8x8", D, n_q, kc, H, Hkv, nsm, max_blocks_per_sm,
        use_stream_k ? nblocks_stream_k : 1, ntiles_dst, ntiles_KV,
        use_stream_k ? 1 : 0, blocks_num.x, blocks_num.y, blocks_num.z);

    const uint3 ne01 = init_fastdiv_values((uint32_t) n_q);
    const uint32_t n_head_log2 = 1u << (uint32_t) floorf(log2f((float) H));
    fattn_kernel<<<blocks_num, block_dim, nbytes_shared_total, stream>>>(
        (const char *) q, (const char *) k, (const char *) v, (const char *) mask,
        nullptr, nullptr, (float *) dst, use_stream_k ? (float2 *) tmp_fixup : nullptr,
        scale, 0.0f, 1.0f, 1.0f, n_head_log2, 0.0f,
        D, ne01, H, 1,
        (int32_t) q_nb1, (int32_t) q_nb2, 0,
        D, kc, Hkv, 1,
        (int32_t) k_nb1, (int32_t) k_nb2, 0,
        (int32_t) v_nb1, (int32_t) v_nb2, 0,
        n_q, 1, 1,
        (int32_t) m_nb1, 0, 0);
    err = cudaGetLastError();
    if (err != cudaSuccess) {
        return err;
    }
    if (use_stream_k && (ntiles_dst % (int) blocks_num.x != 0)) {
        dim3 block_dim_combine(DV, 1, 1);
        dim3 blocks_num_combine(blocks_num.x, ncols1, ncols2);
        flash_attn_stream_k_fixup<DV, ncols1, ncols2>
            <<<blocks_num_combine, block_dim_combine, 0, stream>>>(
                (float *) dst, (const float2 *) tmp_fixup,
                n_q, H, 1, kc, Hkv, nbatch_fa);
        return cudaGetLastError();
    }
    return cudaSuccess;
}

extern "C" size_t mkllm_fattn_mma_fixup_bytes(int nsm) {
    // llama.cpp dst_tmp_meta: nblocks * ncols * (2 + DV/2) float2
    // nblocks <= nsm * occupancy; occupancy 2, use 8 for slack.
    if (nsm <= 0) {
        return 0;
    }
    return (size_t) nsm * 8 * 64 * 130 * sizeof(float2);
}

// llama.cpp fattn.cu:402-404 + fattn-vec.cuh:522-532:
// Ada+ decode, F16 K/V, n_q==1, not (GQA>4 && KV>=8192) ->
// flash_attn_ext_vec<256, 1, F16, F16, false>, launch_fattn stream_k=false.
#include "fattn/fattn-vec.cuh"

extern "C" size_t mkllm_fattn_vec_tmp_bytes(int n_q, int H, int kc) {
    if (n_q <= 0 || H <= 0 || kc <= 0) {
        return 0;
    }
    const int ntiles_KV = (kc + 256 - 1) / 256;
    const int pb = ntiles_KV < 1 ? 1 : ntiles_KV;
    return (size_t) pb * (size_t) n_q * (size_t) H
        * (256 * sizeof(float) + sizeof(float2));
}

extern "C" cudaError_t mkllm_fattn_vec_f16(
        const void * q, const void * k, const void * v, const void * mask, void * dst,
        int D, int Dv, int kc, int n_q, int H, int Hkv, float scale,
        size_t q_nb1, size_t q_nb2, size_t k_nb1, size_t k_nb2,
        size_t v_nb1, size_t v_nb2, size_t m_nb1, size_t d_nb1, size_t d_nb2,
        int nsm, float * tmp, cudaStream_t stream) {
    (void) d_nb1;
    (void) d_nb2;
    (void) Dv;
    if (D != 256 || n_q != 1 || kc <= 0 || H <= 0 || Hkv <= 0
            || H % Hkv != 0 || q == nullptr || k == nullptr || v == nullptr
            || mask == nullptr || dst == nullptr || tmp == nullptr) {
        return cudaErrorInvalidValue;
    }
    constexpr int DKQ = 256;
    constexpr int ncols1 = 1;
    constexpr int ncols2 = 1;
    constexpr int nthreads = 128;
    constexpr int nwarps = nthreads / 32;
    constexpr int nbatch_fa = DKQ;
    fattn_kernel_t fattn_kernel =
        flash_attn_ext_vec<DKQ, ncols1, GGML_TYPE_F16, GGML_TYPE_F16, false>;
    dim3 block_dim(32, nwarps, 1);
    int max_blocks_per_sm = 1;
    cudaError_t err = cudaOccupancyMaxActiveBlocksPerMultiprocessor(
        &max_blocks_per_sm, fattn_kernel, block_dim.x * block_dim.y, 0);
    if (err != cudaSuccess || max_blocks_per_sm <= 0) {
        max_blocks_per_sm = 1;
    }
    const int ntiles_x = (n_q + ncols1 - 1) / ncols1;
    const int gqa_ratio = H / Hkv;
    const int ntiles_z_gqa = (gqa_ratio + ncols2 - 1) / ncols2;
    const int ntiles_dst = ntiles_x * ntiles_z_gqa * Hkv;
    const int ntiles_KV = (kc + nbatch_fa - 1) / nbatch_fa;
    int parallel_blocks = max_blocks_per_sm < ntiles_KV ? max_blocks_per_sm : ntiles_KV;
    const int blocks_per_wave = (nsm > 0 ? nsm : 1) * max_blocks_per_sm;
    int nwaves_best = 0;
    int efficiency_percent_best = 0;
    for (int pb_test = parallel_blocks; pb_test <= ntiles_KV; ++pb_test) {
        const int nblocks_total = ntiles_dst * pb_test;
        const int nwaves = (nblocks_total + blocks_per_wave - 1) / blocks_per_wave;
        const int efficiency_percent = 100 * nblocks_total / (nwaves * blocks_per_wave);
        if (efficiency_percent_best >= 95 && nwaves > nwaves_best) {
            break;
        }
        if (efficiency_percent > efficiency_percent_best) {
            nwaves_best = nwaves;
            efficiency_percent_best = efficiency_percent;
            parallel_blocks = pb_test;
        }
    }
    dim3 blocks_num((unsigned) ntiles_x, (unsigned) parallel_blocks,
        (unsigned) (ntiles_z_gqa * Hkv));
    mkllm_fattn_trace_launch(
        "vec", D, n_q, kc, H, Hkv, nsm, max_blocks_per_sm, parallel_blocks,
        ntiles_dst, ntiles_KV, 0, blocks_num.x, blocks_num.y, blocks_num.z);
    float * dst_ptr = (float *) dst;
    float2 * meta_ptr = nullptr;
    if (parallel_blocks > 1) {
        dst_ptr = tmp;
        meta_ptr = (float2 *) (tmp + (size_t) parallel_blocks * (size_t) n_q
            * (size_t) H * DKQ);
    }
    const uint3 ne01 = init_fastdiv_values((uint32_t) n_q);
    const uint32_t n_head_log2 = 1u << (uint32_t) floorf(log2f((float) H));
    fattn_kernel<<<blocks_num, block_dim, 0, stream>>>(
        (const char *) q, (const char *) k, (const char *) v, (const char *) mask,
        nullptr, nullptr, dst_ptr, meta_ptr,
        scale, 0.0f, 1.0f, 1.0f, n_head_log2, 0.0f,
        D, ne01, H, 1,
        (int32_t) q_nb1, (int32_t) q_nb2, 0,
        D, kc, Hkv, 1,
        (int32_t) k_nb1, (int32_t) k_nb2, 0,
        (int32_t) v_nb1, (int32_t) v_nb2, 0,
        n_q, 1, 1,
        (int32_t) m_nb1, 0, 0);
    err = cudaGetLastError();
    if (err != cudaSuccess) {
        return err;
    }
    if (parallel_blocks > 1) {
        dim3 block_combine(DKQ, 1, 1);
        dim3 grid_combine((unsigned) n_q, (unsigned) H, 1);
        const size_t shared = (size_t) parallel_blocks * sizeof(float2);
        flash_attn_combine_results<DKQ><<<grid_combine, block_combine, shared, stream>>>(
            dst_ptr, meta_ptr, (float *) dst, parallel_blocks);
        return cudaGetLastError();
    }
    return cudaSuccess;
}
