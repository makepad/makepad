/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#ifndef EBVARIANCE_SSE2_H
#define EBVARIANCE_SSE2_H

#include "definitions.h"
#include <assert.h>
#include <emmintrin.h> // SSE2
#include "aom_dsp_rtcd.h"

// Read 4 samples from each of row and row + 1. Interleave the two rows and
// zero-extend them to 16 bit samples stored in the lower half of an SSE
// register.
//static __m128i read64(const uint8_t *p, int32_t stride, int32_t row) {
//  __m128i row1 = xx_loadl_32(p + (row + 1) * stride);
//  return _mm_unpacklo_epi8(_mm_unpacklo_epi8(row0, row1), _mm_setzero_si128());
//}

static INLINE __m128i load4x2_sse2(const uint8_t* const p, const int32_t stride) {
    const __m128i p0 = _mm_cvtsi32_si128(*(const uint32_t*)(p + 0 * stride));
    const __m128i p1 = _mm_cvtsi32_si128(*(const uint32_t*)(p + 1 * stride));
    return _mm_unpacklo_epi8(_mm_unpacklo_epi32(p0, p1), _mm_setzero_si128());
}

static INLINE __m128i load8_8to16_sse2(const uint8_t* const p) {
    const __m128i p0 = _mm_loadl_epi64((const __m128i*)p);
    return _mm_unpacklo_epi8(p0, _mm_setzero_si128());
}

// Accumulate 4 32bit numbers in val to 1 32bit number
static INLINE uint32_t add32x4_sse2(__m128i val) {
    val = _mm_add_epi32(val, _mm_srli_si128(val, 8));
    val = _mm_add_epi32(val, _mm_srli_si128(val, 4));
    return _mm_cvtsi128_si32(val);
}

#endif // EBVARIANCE_SSE2_H
