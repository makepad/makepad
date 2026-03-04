/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved.
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#include <immintrin.h>
#include <stddef.h>
#include "aom_dsp_rtcd.h"
#include "definitions.h"

static void highbd_hadamard_col8_avx2(__m256i* in, int iter) {
    __m256i a0 = in[0];
    __m256i a1 = in[1];
    __m256i a2 = in[2];
    __m256i a3 = in[3];
    __m256i a4 = in[4];
    __m256i a5 = in[5];
    __m256i a6 = in[6];
    __m256i a7 = in[7];

    __m256i b0 = _mm256_add_epi32(a0, a1);
    __m256i b1 = _mm256_sub_epi32(a0, a1);
    __m256i b2 = _mm256_add_epi32(a2, a3);
    __m256i b3 = _mm256_sub_epi32(a2, a3);
    __m256i b4 = _mm256_add_epi32(a4, a5);
    __m256i b5 = _mm256_sub_epi32(a4, a5);
    __m256i b6 = _mm256_add_epi32(a6, a7);
    __m256i b7 = _mm256_sub_epi32(a6, a7);

    a0 = _mm256_add_epi32(b0, b2);
    a1 = _mm256_add_epi32(b1, b3);
    a2 = _mm256_sub_epi32(b0, b2);
    a3 = _mm256_sub_epi32(b1, b3);
    a4 = _mm256_add_epi32(b4, b6);
    a5 = _mm256_add_epi32(b5, b7);
    a6 = _mm256_sub_epi32(b4, b6);
    a7 = _mm256_sub_epi32(b5, b7);

    if (iter == 0) {
        b0 = _mm256_add_epi32(a0, a4);
        b7 = _mm256_add_epi32(a1, a5);
        b3 = _mm256_add_epi32(a2, a6);
        b4 = _mm256_add_epi32(a3, a7);
        b2 = _mm256_sub_epi32(a0, a4);
        b6 = _mm256_sub_epi32(a1, a5);
        b1 = _mm256_sub_epi32(a2, a6);
        b5 = _mm256_sub_epi32(a3, a7);

        a0 = _mm256_unpacklo_epi32(b0, b1);
        a1 = _mm256_unpacklo_epi32(b2, b3);
        a2 = _mm256_unpackhi_epi32(b0, b1);
        a3 = _mm256_unpackhi_epi32(b2, b3);
        a4 = _mm256_unpacklo_epi32(b4, b5);
        a5 = _mm256_unpacklo_epi32(b6, b7);
        a6 = _mm256_unpackhi_epi32(b4, b5);
        a7 = _mm256_unpackhi_epi32(b6, b7);

        b0 = _mm256_unpacklo_epi64(a0, a1);
        b1 = _mm256_unpacklo_epi64(a4, a5);
        b2 = _mm256_unpackhi_epi64(a0, a1);
        b3 = _mm256_unpackhi_epi64(a4, a5);
        b4 = _mm256_unpacklo_epi64(a2, a3);
        b5 = _mm256_unpacklo_epi64(a6, a7);
        b6 = _mm256_unpackhi_epi64(a2, a3);
        b7 = _mm256_unpackhi_epi64(a6, a7);

        in[0] = _mm256_permute2x128_si256(b0, b1, 0x20);
        in[1] = _mm256_permute2x128_si256(b0, b1, 0x31);
        in[2] = _mm256_permute2x128_si256(b2, b3, 0x20);
        in[3] = _mm256_permute2x128_si256(b2, b3, 0x31);
        in[4] = _mm256_permute2x128_si256(b4, b5, 0x20);
        in[5] = _mm256_permute2x128_si256(b4, b5, 0x31);
        in[6] = _mm256_permute2x128_si256(b6, b7, 0x20);
        in[7] = _mm256_permute2x128_si256(b6, b7, 0x31);
    } else {
        in[0] = _mm256_add_epi32(a0, a4);
        in[7] = _mm256_add_epi32(a1, a5);
        in[3] = _mm256_add_epi32(a2, a6);
        in[4] = _mm256_add_epi32(a3, a7);
        in[2] = _mm256_sub_epi32(a0, a4);
        in[6] = _mm256_sub_epi32(a1, a5);
        in[1] = _mm256_sub_epi32(a2, a6);
        in[5] = _mm256_sub_epi32(a3, a7);
    }
}

void svt_aom_highbd_hadamard_8x8_avx2(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff) {
    __m128i src16[8];
    __m256i src32[8];

    src16[0] = _mm_loadu_si128((const __m128i*)src_diff);
    src16[1] = _mm_loadu_si128((const __m128i*)(src_diff += src_stride));
    src16[2] = _mm_loadu_si128((const __m128i*)(src_diff += src_stride));
    src16[3] = _mm_loadu_si128((const __m128i*)(src_diff += src_stride));
    src16[4] = _mm_loadu_si128((const __m128i*)(src_diff += src_stride));
    src16[5] = _mm_loadu_si128((const __m128i*)(src_diff += src_stride));
    src16[6] = _mm_loadu_si128((const __m128i*)(src_diff += src_stride));
    src16[7] = _mm_loadu_si128((const __m128i*)(src_diff + src_stride));

    src32[0] = _mm256_cvtepi16_epi32(src16[0]);
    src32[1] = _mm256_cvtepi16_epi32(src16[1]);
    src32[2] = _mm256_cvtepi16_epi32(src16[2]);
    src32[3] = _mm256_cvtepi16_epi32(src16[3]);
    src32[4] = _mm256_cvtepi16_epi32(src16[4]);
    src32[5] = _mm256_cvtepi16_epi32(src16[5]);
    src32[6] = _mm256_cvtepi16_epi32(src16[6]);
    src32[7] = _mm256_cvtepi16_epi32(src16[7]);

    highbd_hadamard_col8_avx2(src32, 0);
    highbd_hadamard_col8_avx2(src32, 1);

    _mm256_storeu_si256((__m256i*)coeff, src32[0]);
    coeff += 8;
    _mm256_storeu_si256((__m256i*)coeff, src32[1]);
    coeff += 8;
    _mm256_storeu_si256((__m256i*)coeff, src32[2]);
    coeff += 8;
    _mm256_storeu_si256((__m256i*)coeff, src32[3]);
    coeff += 8;
    _mm256_storeu_si256((__m256i*)coeff, src32[4]);
    coeff += 8;
    _mm256_storeu_si256((__m256i*)coeff, src32[5]);
    coeff += 8;
    _mm256_storeu_si256((__m256i*)coeff, src32[6]);
    coeff += 8;
    _mm256_storeu_si256((__m256i*)coeff, src32[7]);
}
