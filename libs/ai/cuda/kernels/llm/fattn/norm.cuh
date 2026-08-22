// SPDX-License-Identifier: MIT
// Copyright (c) 2023-2026 The ggml authors
//
// Substantial portions derived from ggml / llama.cpp
// (https://github.com/ggml-org/llama.cpp), MIT licensed.
// The original copyright notice and permission notice are retained.
// See libs/ai/NOTICE and, where present, LICENSE in this directory.
//
#pragma once
// Official llama.cpp ggml-cuda/norm.cu rms_norm_f32 fused with MUL (+ ADD).
// Source: /Users/admin/llama.cpp/ggml/src/ggml-cuda/norm.cu:74-384
// Dispatch: ggml-cuda.cu:3994-4004 ggml_cuda_op_rms_norm_fused[_add].

#include "common.cuh"

template <int block_size, bool do_multiply, bool do_add>
static __global__ void rms_norm_f32(
        const float * x,
        float *       dst,
        const int     ncols,
        const int64_t stride_row,
        const int64_t stride_channel,
        const int64_t stride_sample,
        const float   eps,
        const int64_t dst_stride_row,
        const int64_t dst_stride_channel,
        const int64_t dst_stride_sample,
        const float * mul,
        const int64_t mul_stride_row,
        const int64_t mul_stride_channel,
        const int64_t mul_stride_sample,
        const uint3   mul_ncols_packed,
        const uint3   mul_nrows_packed,
        const uint3   mul_nchannels_packed,
        const uint3   mul_nsamples_packed,
        const float * add,
        const int64_t add_stride_row,
        const int64_t add_stride_channel,
        const int64_t add_stride_sample,
        const uint3   add_ncols_packed,
        const uint3   add_nrows_packed,
        const uint3   add_nchannels_packed,
        const uint3   add_nsamples_packed) {
    const int nrows     = gridDim.x;
    const int nchannels = gridDim.y;

    const int row       = blockIdx.x;
    const int channel   = blockIdx.y;
    const int sample    = blockIdx.z;
    const int tid       = threadIdx.x;

    static_assert(!do_add || do_multiply, "fusing add is not supported without multiplying");

    x   += sample*stride_sample + channel*stride_channel + row*stride_row;
    dst += sample*dst_stride_sample + channel*dst_stride_channel + row*dst_stride_row;
    (void) nrows;
    (void) nchannels;

    if constexpr (do_multiply) {
        const uint32_t mul_row     = fastmodulo((uint32_t) row, mul_nrows_packed);
        const uint32_t mul_channel = fastmodulo((uint32_t) channel, mul_nchannels_packed);
        const uint32_t mul_sample  = fastmodulo((uint32_t) sample, mul_nsamples_packed);
        mul += (int64_t) mul_sample * mul_stride_sample
            + (int64_t) mul_channel * mul_stride_channel
            + (int64_t) mul_row * mul_stride_row;
    }

    if constexpr (do_add) {
        const int add_row     = (int) fastmodulo((uint32_t) row, add_nrows_packed);
        const int add_channel = (int) fastmodulo((uint32_t) channel, add_nchannels_packed);
        const int add_sample  = (int) fastmodulo((uint32_t) sample, add_nsamples_packed);
        add += (int64_t) add_sample * add_stride_sample
            + (int64_t) add_channel * add_stride_channel
            + (int64_t) add_row * add_stride_row;
    }

    float tmp = 0.0f;

    for (int col = tid; col < ncols; col += block_size) {
        const float xi = x[col];
        tmp += xi * xi;
    }

    extern __shared__ float s_sum[];
    tmp = block_reduce_sum<block_size>(tmp, s_sum);

    const float mean = tmp / ncols;
    const float scale = rsqrtf(mean + eps);

    for (int col = tid; col < ncols; col += block_size) {
        if constexpr (do_multiply && do_add) {
            const int mul_col = (int) fastmodulo((uint32_t) col, mul_ncols_packed);
            const int add_col = (int) fastmodulo((uint32_t) col, add_ncols_packed);
            dst[col]          = scale * x[col] * mul[mul_col] + add[add_col];
        } else if constexpr (do_multiply) {
            const int mul_col = (int) fastmodulo((uint32_t) col, mul_ncols_packed);
            dst[col]          = scale * x[col] * mul[mul_col];
        } else {
            dst[col] = scale * x[col];
        }
    }
}

static void rms_norm_mul_f32_cuda(
        const float *  x,
        const float *  mul,
        const float *  add,
        float *        dst,
        const int      ncols,
        const int      nrows,
        const int      nchannels,
        const int      nsamples,
        const int64_t  stride_row,
        const int64_t  stride_channel,
        const int64_t  stride_sample,
        const int64_t  dst_stride_row,
        const int64_t  dst_stride_channel,
        const int64_t  dst_stride_sample,
        const int64_t  mul_stride_row,
        const int64_t  mul_stride_channel,
        const int64_t  mul_stride_sample,
        const uint32_t mul_ncols,
        const uint32_t mul_nrows,
        const uint32_t mul_nchannels,
        const uint32_t mul_nsamples,
        const int64_t  add_stride_row,
        const int64_t  add_stride_channel,
        const int64_t  add_stride_sample,
        const uint32_t add_ncols,
        const uint32_t add_nrows,
        const uint32_t add_nchannels,
        const uint32_t add_nsamples,
        const float    eps,
        cudaStream_t   stream) {
    const dim3 blocks_num((unsigned) nrows, (unsigned) nchannels, (unsigned) nsamples);
    const uint3 mul_ncols_packed     = init_fastdiv_values(mul_ncols);
    const uint3 mul_nrows_packed     = init_fastdiv_values(mul_nrows);
    const uint3 mul_nchannels_packed = init_fastdiv_values(mul_nchannels);
    const uint3 mul_nsamples_packed  = init_fastdiv_values(mul_nsamples);

    // llama.cpp norm.cu:342-367: do not pack add fastdiv when add is null
    // (init_fastdiv_values(0) is integer divide-by-zero).
    if (add == nullptr) {
        if (ncols < 1024) {
            const dim3 block_dims(256, 1, 1);
            rms_norm_f32<256, true, false><<<blocks_num, block_dims, 32 * sizeof(float), stream>>>(
                x, dst, ncols, stride_row, stride_channel, stride_sample, eps,
                dst_stride_row, dst_stride_channel, dst_stride_sample,
                mul, mul_stride_row, mul_stride_channel, mul_stride_sample,
                mul_ncols_packed, mul_nrows_packed, mul_nchannels_packed, mul_nsamples_packed,
                nullptr, 0, 0, 0, make_uint3(0, 0, 0), make_uint3(0, 0, 0),
                make_uint3(0, 0, 0), make_uint3(0, 0, 0));
        } else {
            const dim3 block_dims(1024, 1, 1);
            rms_norm_f32<1024, true, false><<<blocks_num, block_dims, 32 * sizeof(float), stream>>>(
                x, dst, ncols, stride_row, stride_channel, stride_sample, eps,
                dst_stride_row, dst_stride_channel, dst_stride_sample,
                mul, mul_stride_row, mul_stride_channel, mul_stride_sample,
                mul_ncols_packed, mul_nrows_packed, mul_nchannels_packed, mul_nsamples_packed,
                nullptr, 0, 0, 0, make_uint3(0, 0, 0), make_uint3(0, 0, 0),
                make_uint3(0, 0, 0), make_uint3(0, 0, 0));
        }
    } else {
        const uint3 add_ncols_packed     = init_fastdiv_values(add_ncols);
        const uint3 add_nrows_packed     = init_fastdiv_values(add_nrows);
        const uint3 add_nchannels_packed = init_fastdiv_values(add_nchannels);
        const uint3 add_nsamples_packed  = init_fastdiv_values(add_nsamples);
        if (ncols < 1024) {
            const dim3 block_dims(256, 1, 1);
            rms_norm_f32<256, true, true><<<blocks_num, block_dims, 32 * sizeof(float), stream>>>(
                x, dst, ncols, stride_row, stride_channel, stride_sample, eps,
                dst_stride_row, dst_stride_channel, dst_stride_sample,
                mul, mul_stride_row, mul_stride_channel, mul_stride_sample,
                mul_ncols_packed, mul_nrows_packed, mul_nchannels_packed, mul_nsamples_packed,
                add, add_stride_row, add_stride_channel, add_stride_sample,
                add_ncols_packed, add_nrows_packed, add_nchannels_packed, add_nsamples_packed);
        } else {
            const dim3 block_dims(1024, 1, 1);
            rms_norm_f32<1024, true, true><<<blocks_num, block_dims, 32 * sizeof(float), stream>>>(
                x, dst, ncols, stride_row, stride_channel, stride_sample, eps,
                dst_stride_row, dst_stride_channel, dst_stride_sample,
                mul, mul_stride_row, mul_stride_channel, mul_stride_sample,
                mul_ncols_packed, mul_nrows_packed, mul_nchannels_packed, mul_nsamples_packed,
                add, add_stride_row, add_stride_channel, add_stride_sample,
                add_ncols_packed, add_nrows_packed, add_nchannels_packed, add_nsamples_packed);
        }
    }
}
