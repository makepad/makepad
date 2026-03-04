/*
 * Copyright (c) 2018, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */
#ifndef AV1_COMMON_X86_AV1_INV_TXFM_AVX2_H_
#define AV1_COMMON_X86_AV1_INV_TXFM_AVX2_H_

#include <immintrin.h>

#include "definitions.h"
#include "common_dsp_rtcd.h"
#include "txfm_common_avx2.h"

#ifdef __cplusplus
extern "C" {
#endif

// half input is zero
#define btf_16_w16_0_avx2(w0, w1, in, out0, out1)          \
    {                                                      \
        const __m256i _w0 = _mm256_set1_epi16(w0 * 8);     \
        const __m256i _w1 = _mm256_set1_epi16(w1 * 8);     \
        const __m256i _in = in;                            \
        out0              = _mm256_mulhrs_epi16(_in, _w0); \
        out1              = _mm256_mulhrs_epi16(_in, _w1); \
    }

static INLINE void round_shift_avx2(const __m256i* input, __m256i* output, int32_t size) {
    const __m256i scale = _mm256_set1_epi16(new_inv_sqrt2 * 8);
    for (int32_t i = 0; i < size; ++i) {
        output[i] = _mm256_mulhrs_epi16(input[i], scale);
    }
}

static INLINE void write_recon_w16_avx2(__m256i res, uint8_t* output_r, uint8_t* output_w) {
    __m128i pred = _mm_loadu_si128((__m128i const*)(output_r));
    __m256i u    = _mm256_adds_epi16(_mm256_cvtepu8_epi16(pred), res);
    __m128i y    = _mm256_castsi256_si128(_mm256_permute4x64_epi64(_mm256_packus_epi16(u, u), 168));
    _mm_storeu_si128((__m128i*)(output_w), y);
}

static INLINE void lowbd_write_buffer_16xn_avx2(__m256i* in, uint8_t* output_r, int32_t stride_r, uint8_t* output_w,
                                                int32_t stride_w, int32_t flipud, int32_t height) {
    int32_t       j    = flipud ? (height - 1) : 0;
    const int32_t step = flipud ? -1 : 1;
    for (int32_t i = 0; i < height; ++i, j += step) {
        write_recon_w16_avx2(in[j], output_r + i * stride_r, output_w + i * stride_w);
    }
}

#ifdef __cplusplus
}
#endif

#endif // AV1_COMMON_X86_AV1_INV_TXFM_AVX2_H_
