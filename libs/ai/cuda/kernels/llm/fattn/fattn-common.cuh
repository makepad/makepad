// SPDX-License-Identifier: MIT
// Copyright (c) 2023-2026 The ggml authors
//
// Substantial portions derived from ggml / llama.cpp
// (https://github.com/ggml-org/llama.cpp), MIT licensed.
// The original copyright notice and permission notice are retained.
// See libs/ai/NOTICE and, where present, LICENSE in this directory.
//
#pragma once
#include "common.cuh"

#define FATTN_KQ_STRIDE       256
#define HALF_MAX_HALF         __float2half(65504.0f/2)
#define SOFTMAX_FTZ_THRESHOLD -20.0f
#define FATTN_KQ_MAX_OFFSET (3.0f*0.6931f)

typedef void (* fattn_kernel_t)(
        const char * __restrict__ Q,
        const char * __restrict__ K,
        const char * __restrict__ V,
        const char * __restrict__ mask,
        const char * __restrict__ sinks,
        const int  * __restrict__ KV_max,
        float      * __restrict__ dst,
        float2     * __restrict__ dst_meta,
        const float scale,
        const float max_bias,
        const float m0,
        const float m1,
        const uint32_t n_head_log2,
        const float logit_softcap,
        const int32_t ne00, const uint3   ne01, const int32_t ne02, const int32_t ne03,
                            const int32_t nb01, const int32_t nb02, const int32_t nb03,
        const int32_t ne10, const int32_t ne11, const int32_t ne12, const int32_t ne13,
                            const int32_t nb11, const int32_t nb12, const int64_t nb13,
                            const int32_t nb21, const int32_t nb22, const int64_t nb23,
                            const int32_t ne31, const int32_t ne32, const int32_t ne33,
                            const int32_t nb31, const int32_t nb32, const int64_t nb33);

// llama.cpp fattn-common.cuh F16 vec-dot / V dequant (fattn-vec.cuh D=256).
typedef float (*vec_dot_KQ_t)(
    const char * __restrict__ K_c, const void * __restrict__ Q_v,
    const int * __restrict__ Q_q8, const void * __restrict__ Q_ds);

template <int D, int nthreads>
static __device__ __forceinline__ float vec_dot_fattn_vec_KQ_f16(
    const char * __restrict__ K_c, const void * __restrict__ Q_v,
    const int * __restrict__ Q_q8, const void * __restrict__ Q_ds_v) {
    const half2 * K_h2 = (const half2 *) K_c;
    GGML_UNUSED(Q_q8);
    GGML_UNUSED(Q_ds_v);
    constexpr int cpy_nb = ggml_cuda_get_max_cpy_bytes();
    constexpr int cpy_ne = cpy_nb / 4;
    float sum = 0.0f;
#pragma unroll
    for (int k_KQ_0 = 0; k_KQ_0 < D / 2; k_KQ_0 += nthreads * cpy_ne) {
        __align__(16) half2 tmp[cpy_ne];
        ggml_cuda_memcpy_1<sizeof(tmp)>(tmp, K_h2 + k_KQ_0 + (threadIdx.x % nthreads) * cpy_ne);
#pragma unroll
        for (int k_KQ_1 = 0; k_KQ_1 < cpy_ne; ++k_KQ_1) {
#ifdef V_DOT2_F32_F16_AVAILABLE
            ggml_cuda_mad(sum, tmp[k_KQ_1], ((const half2 *) Q_v)[k_KQ_0 / nthreads + k_KQ_1]);
#else
            ggml_cuda_mad(sum, __half22float2(tmp[k_KQ_1]), ((const float2 *) Q_v)[k_KQ_0 / nthreads + k_KQ_1]);
#endif
        }
    }
    return sum;
}

template <ggml_type type_K, int D, int nthreads>
constexpr __device__ vec_dot_KQ_t get_vec_dot_KQ() {
    static_assert(type_K == GGML_TYPE_F16, "fattn-vec executor path is F16 K");
    return vec_dot_fattn_vec_KQ_f16<D, nthreads>;
}

typedef void (*dequantize_V_t)(const void *, void *, const int64_t);

template <typename T, int ne>
static __device__ __forceinline__ void dequantize_V_f16(const void * __restrict__ vx, void * __restrict__ dst, const int64_t i0) {
    if constexpr (std::is_same_v<T, half>) {
        ggml_cuda_memcpy_1<ne * sizeof(half)>(dst, (const half *) vx + i0);
    } else if constexpr (std::is_same_v<T, float>) {
        static_assert(ne % 2 == 0, "bad ne");
        __align__(16) half2 tmp[ne / 2];
        ggml_cuda_memcpy_1<ne * sizeof(half)>(tmp, (const half *) vx + i0);
        float2 * dst_f2 = (float2 *) dst;
#pragma unroll
        for (int l = 0; l < ne / 2; ++l) {
            dst_f2[l] = __half22float2(tmp[l]);
        }
    } else {
        static_assert(std::is_same_v<T, void>, "unsupported type");
    }
}

template <ggml_type type_V, typename T, int ne>
constexpr __device__ dequantize_V_t get_dequantize_V() {
    static_assert(type_V == GGML_TYPE_F16, "fattn-vec executor path is F16 V");
    return dequantize_V_f16<T, ne>;
}

// llama.cpp fattn-common.cuh flash_attn_combine_results
template <int D>
__launch_bounds__(D, 1)
static __global__ void flash_attn_combine_results(
        const float * __restrict__ VKQ_parts,
        const float2 * __restrict__ VKQ_meta,
        float * __restrict__ dst,
        const int parallel_blocks) {
    const int ne01 = gridDim.x;
    const int ne02 = gridDim.y;
    const int col = blockIdx.x;
    const int head = blockIdx.y;
    const int sequence = blockIdx.z;
    const int j_dst_unrolled = (sequence * ne01 + col) * ne02 + head;
    VKQ_parts += j_dst_unrolled * parallel_blocks * D;
    VKQ_meta += j_dst_unrolled * parallel_blocks;
    dst += j_dst_unrolled * D;
    const int tid = threadIdx.x;
    __builtin_assume(tid < D);
    extern __shared__ float2 meta[];
    for (int i = tid; i < 2 * parallel_blocks; i += D) {
        ((float *) meta)[i] = ((const float *) VKQ_meta)[i];
    }
    __syncthreads();
    float kqmax = meta[0].x;
    for (int l = 1; l < parallel_blocks; ++l) {
        kqmax = max(kqmax, meta[l].x);
    }
    float VKQ_numerator = 0.0f;
    float VKQ_denominator = 0.0f;
    for (int l = 0; l < parallel_blocks; ++l) {
        const float KQ_max_scale = expf(meta[l].x - kqmax);
        VKQ_numerator += KQ_max_scale * VKQ_parts[l * D + tid];
        VKQ_denominator += KQ_max_scale * meta[l].y;
    }
    dst[tid] = VKQ_numerator / VKQ_denominator;
}

// llama.cpp fattn-common.cuh flash_attn_stream_k_fixup
template<int D, int ncols1, int ncols2>
__launch_bounds__(D, 1)
static __global__ void flash_attn_stream_k_fixup(
        float * __restrict__ dst, const float2 * __restrict__ dst_fixup,
        const int ne01, const int ne02, const int ne03,
        const int ne11, const int ne12, const int nbatch_fa) {
    constexpr int ncols = ncols1*ncols2;
    const int bidx0 = blockIdx.x;
    const int j     = blockIdx.y;
    const int c     = blockIdx.z;
    const int jc    = j*ncols2 + c;
    const int tid   = threadIdx.x;
    const float * dst_fixup_data = ((const float *) dst_fixup) + gridDim.x*(2*2*ncols);
    const int gqa_ratio = ne02 / ne12;
    const int iter_k     = (ne11      + (nbatch_fa - 1)) / nbatch_fa;
    const int iter_j     = (ne01      + (ncols1    - 1)) / ncols1;
    const int iter_z_gqa = (gqa_ratio + (ncols2    - 1)) / ncols2;
    const int kbc0      = int64_t(bidx0 + 0)*(iter_k*iter_j*iter_z_gqa*ne12*ne03) / gridDim.x;
    const int kbc0_stop = int64_t(bidx0 + 1)*(iter_k*iter_j*iter_z_gqa*ne12*ne03) / gridDim.x;
    const bool did_not_have_any_data   = kbc0 == kbc0_stop;
    const bool wrote_beginning_of_tile = kbc0 % iter_k == 0;
    const bool did_not_write_last      = kbc0/iter_k == kbc0_stop/iter_k && kbc0_stop % iter_k != 0;
    if (did_not_have_any_data || wrote_beginning_of_tile || did_not_write_last) {
        return;
    }
    const int sequence =  kbc0 /(iter_k*iter_j*iter_z_gqa*ne12);
    const int z_KV     = (kbc0 - iter_k*iter_j*iter_z_gqa*ne12 * sequence)/(iter_k*iter_j*iter_z_gqa);
    const int zt_gqa   = (kbc0 - iter_k*iter_j*iter_z_gqa*ne12 * sequence - iter_k*iter_j*iter_z_gqa * z_KV)/(iter_k*iter_j);
    const int jt       = (kbc0 - iter_k*iter_j*iter_z_gqa*ne12 * sequence - iter_k*iter_j*iter_z_gqa * z_KV - iter_k*iter_j * zt_gqa) / iter_k;
    const int zt_Q = z_KV*gqa_ratio + zt_gqa*ncols2;
    if (jt*ncols1 + j >= ne01 || zt_gqa*ncols2 + c >= gqa_ratio) {
        return;
    }
    dst += sequence*ne02*ne01*D + jt*ne02*(ncols1*D) + zt_Q*D + (j*ne02 + c)*D + tid;
    float dst_val = *dst;
    float2 tmp0 = dst_fixup[bidx0*ncols + jc];
    float max_val = tmp0.x;
    float rowsum  = tmp0.y;
    int bidx = bidx0 - 1;
    int kbc_stop = kbc0;
    while (true) {
        const int kbc = int64_t(bidx)*(iter_k*iter_j*iter_z_gqa*ne12*ne03) / gridDim.x;
        if (kbc == kbc_stop) {
            bidx--;
            kbc_stop = kbc;
            continue;
        }
        const float dst_add = dst_fixup_data[bidx*ncols*D + jc*D + tid];
        const float2 tmp = dst_fixup[(gridDim.x + bidx)*ncols + jc];
        const float max_val_new = fmaxf(max_val, tmp.x);
        const float diff_val = max_val - max_val_new;
        const float diff_add = tmp.x   - max_val_new;
        const float scale_val = diff_val >= SOFTMAX_FTZ_THRESHOLD ? expf(diff_val) : 0.0f;
        const float scale_add = diff_add >= SOFTMAX_FTZ_THRESHOLD ? expf(diff_add) : 0.0f;
        dst_val = scale_val*dst_val + scale_add*dst_add;
        rowsum  = scale_val*rowsum  + scale_add*tmp.y;
        max_val = max_val_new;
        if (kbc % iter_k == 0 || kbc/iter_k < kbc0/iter_k) {
            break;
        }
        bidx--;
        kbc_stop = kbc;
    }
    *dst = dst_val / rowsum;
}
