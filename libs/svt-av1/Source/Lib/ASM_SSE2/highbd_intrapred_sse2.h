/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef EbHighbdIntraPrediction_SSE2_h
#define EbHighbdIntraPrediction_SSE2_h

#include <emmintrin.h>
#include "definitions.h"
#include "common_dsp_rtcd.h"

static INLINE __m128i dc_sum_4x32bit(const __m128i src) {
    __m128i sum, sum_hi;
    sum_hi = _mm_srli_si128(src, 8);
    sum    = _mm_add_epi32(src, sum_hi);
    sum_hi = _mm_srli_si128(sum, 4);
    return _mm_add_epi32(sum, sum_hi);
}

static INLINE __m128i dc_sum_4x16bit(const __m128i src) {
    __m128i       sum, sum_hi;
    const __m128i src_hi = _mm_srli_si128(src, 4);
    sum                  = _mm_add_epi16(src, src_hi);
    sum_hi               = _mm_srli_si128(sum, 2);
    sum                  = _mm_add_epi16(sum, sum_hi);

    return sum;
}

static INLINE __m128i dc_sum_4x16bit_large(const __m128i src) {
    // Unpack to avoid 12-bit overflow.
    const __m128i src_32 = _mm_unpacklo_epi16(src, _mm_setzero_si128());
    return dc_sum_4x32bit(src_32);
}

static INLINE __m128i dc_sum_8x16bit(const __m128i src) {
    const __m128i src_hi = _mm_srli_si128(src, 8);
    const __m128i sum    = _mm_add_epi16(src, src_hi);
    return dc_sum_4x16bit(sum);
}

static INLINE __m128i dc_sum_8x16bit_large(const __m128i src) {
    const __m128i src_hi = _mm_srli_si128(src, 8);
    const __m128i sum    = _mm_add_epi16(src, src_hi);
    return dc_sum_4x16bit_large(sum);
}

static INLINE __m128i dc_sum_4(const uint16_t* const src) {
    const __m128i s = _mm_loadl_epi64((const __m128i*)src);
    return dc_sum_4x16bit(s);
}

static INLINE __m128i dc_sum_8(const uint16_t* const src) {
    const __m128i s = _mm_loadu_si128((const __m128i*)src);
    return dc_sum_8x16bit(s);
}

#endif // EbHighbdIntraPrediction_SSE2_h
