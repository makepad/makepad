/*
 * Copyright (c) 2022, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#include <immintrin.h>
#include "definitions.h"

#ifndef _mm_loadu_si32
#define _mm_loadu_si32(p) _mm_cvtsi32_si128(*(unsigned int const*)(p))
#endif
#ifndef _mm_loadu_si64
#define _mm_loadu_si64(p) _mm_loadl_epi64((__m128i const*)(p))
#endif
static INLINE uint32_t sum8(__m256i x) {
    // hiQuad = ( x7, x6, x5, x4 )
    const __m128i hiQuad = _mm256_extracti128_si256(x, 1);
    // loQuad = ( x3, x2, x1, x0 )
    const __m128i loQuad = _mm256_castsi256_si128(x);
    // sumQuad = ( x3 + x7, x2 + x6, x1 + x5, x0 + x4 )
    const __m128i sumQuad = _mm_add_epi32(loQuad, hiQuad);
    // loDual = ( -, -, x1 + x5, x0 + x4 )
    const __m128i loDual = sumQuad;
    // hiDual = ( -, -, x3 + x7, x2 + x6 )
    const __m128i hiDual = _mm_unpackhi_epi64(sumQuad, sumQuad);
    // sumDual = ( -, -, x1 + x3 + x5 + x7, x0 + x2 + x4 + x6 )
    const __m128i sumDual = _mm_add_epi32(loDual, hiDual);
    // lo = ( -, -, -, x0 + x2 + x4 + x6 )
    const __m128i lo = sumDual;
    // hi = ( -, -, -, x1 + x3 + x5 + x7 )
    const __m128i hi = _mm_shuffle_epi32(sumDual, 0x1);
    // sum = ( -, -, -, x0 + x1 + x2 + x3 + x4 + x5 + x6 + x7 )
    const __m128i sum = _mm_add_epi32(lo, hi);
    return _mm_extract_epi32(sum, 0);
}

extern double similarity(uint32_t sum_s, uint32_t sum_r, uint32_t sum_sq_s, uint32_t sum_sq_r, uint32_t sum_sxr,
                         int count, uint32_t bd);

double svt_ssim_8x8_avx2(const uint8_t* s, uint32_t sp, const uint8_t* r, uint32_t rp) {
    __m256i vec_sum_s    = _mm256_setzero_si256();
    __m256i vec_sum_r    = _mm256_setzero_si256();
    __m256i vec_sum_sq_s = _mm256_setzero_si256();
    __m256i vec_sum_sq_r = _mm256_setzero_si256();
    __m256i vec_sum_sxr  = _mm256_setzero_si256();
    for (int i = 0; i < 8; ++i) {
        __m256i vec_src = _mm256_cvtepu8_epi32(_mm_loadu_si64(s));
        __m256i vec_rec = _mm256_cvtepu8_epi32(_mm_loadu_si64(r));

        vec_sum_s = _mm256_add_epi32(vec_sum_s, vec_src);
        vec_sum_r = _mm256_add_epi32(vec_sum_r, vec_rec);

        __m256i vec_sq = _mm256_mullo_epi32(vec_src, vec_src);
        vec_sum_sq_s   = _mm256_add_epi32(vec_sum_sq_s, vec_sq);

        vec_sq       = _mm256_mullo_epi32(vec_rec, vec_rec);
        vec_sum_sq_r = _mm256_add_epi32(vec_sum_sq_r, vec_sq);

        __m256i vec_sxr = _mm256_mullo_epi32(vec_src, vec_rec);
        vec_sum_sxr     = _mm256_add_epi32(vec_sum_sxr, vec_sxr);

        s += sp;
        r += rp;
    }

    uint32_t sum_s    = sum8(vec_sum_s);
    uint32_t sum_r    = sum8(vec_sum_r);
    uint32_t sum_sq_s = sum8(vec_sum_sq_s);
    uint32_t sum_sq_r = sum8(vec_sum_sq_r);
    uint32_t sum_sxr  = sum8(vec_sum_sxr);
    double   score    = similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, 64, 8);
    return score;
}

double svt_ssim_4x4_avx2(const uint8_t* s, uint32_t sp, const uint8_t* r, uint32_t rp) {
    __m256i vec_sum_s    = _mm256_setzero_si256();
    __m256i vec_sum_r    = _mm256_setzero_si256();
    __m256i vec_sum_sq_s = _mm256_setzero_si256();
    __m256i vec_sum_sq_r = _mm256_setzero_si256();
    __m256i vec_sum_sxr  = _mm256_setzero_si256();
    for (int i = 0; i < 4; ++i) {
        __m256i vec_src = _mm256_cvtepu8_epi32(_mm_loadu_si32(s));
        __m256i vec_rec = _mm256_cvtepu8_epi32(_mm_loadu_si32(r));

        vec_sum_s = _mm256_add_epi32(vec_sum_s, vec_src);
        vec_sum_r = _mm256_add_epi32(vec_sum_r, vec_rec);

        __m256i vec_sq = _mm256_mullo_epi32(vec_src, vec_src);
        vec_sum_sq_s   = _mm256_add_epi32(vec_sum_sq_s, vec_sq);

        vec_sq       = _mm256_mullo_epi32(vec_rec, vec_rec);
        vec_sum_sq_r = _mm256_add_epi32(vec_sum_sq_r, vec_sq);

        __m256i vec_sxr = _mm256_mullo_epi32(vec_src, vec_rec);
        vec_sum_sxr     = _mm256_add_epi32(vec_sum_sxr, vec_sxr);

        s += sp;
        r += rp;
    }

    uint32_t sum_s    = sum8(vec_sum_s);
    uint32_t sum_r    = sum8(vec_sum_r);
    uint32_t sum_sq_s = sum8(vec_sum_sq_s);
    uint32_t sum_sq_r = sum8(vec_sum_sq_r);
    uint32_t sum_sxr  = sum8(vec_sum_sxr);
    double   score    = similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, 16, 8);
    return score;
}

double svt_ssim_8x8_hbd_avx2(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp) {
    __m256i vec_sum_s    = _mm256_setzero_si256();
    __m256i vec_sum_r    = _mm256_setzero_si256();
    __m256i vec_sum_sq_s = _mm256_setzero_si256();
    __m256i vec_sum_sq_r = _mm256_setzero_si256();
    __m256i vec_sum_sxr  = _mm256_setzero_si256();
    for (int i = 0; i < 8; ++i) {
        __m256i vec_src = _mm256_cvtepu16_epi32(_mm_loadu_si128((const __m128i*)s));
        __m256i vec_rec = _mm256_cvtepu16_epi32(_mm_loadu_si128((const __m128i*)r));

        vec_sum_s = _mm256_add_epi32(vec_sum_s, vec_src);
        vec_sum_r = _mm256_add_epi32(vec_sum_r, vec_rec);

        __m256i vec_sq = _mm256_mullo_epi32(vec_src, vec_src);
        vec_sum_sq_s   = _mm256_add_epi32(vec_sum_sq_s, vec_sq);

        vec_sq       = _mm256_mullo_epi32(vec_rec, vec_rec);
        vec_sum_sq_r = _mm256_add_epi32(vec_sum_sq_r, vec_sq);

        __m256i vec_sxr = _mm256_mullo_epi32(vec_src, vec_rec);
        vec_sum_sxr     = _mm256_add_epi32(vec_sum_sxr, vec_sxr);

        s += sp;
        r += rp;
    }

    uint32_t sum_s    = sum8(vec_sum_s);
    uint32_t sum_r    = sum8(vec_sum_r);
    uint32_t sum_sq_s = sum8(vec_sum_sq_s);
    uint32_t sum_sq_r = sum8(vec_sum_sq_r);
    uint32_t sum_sxr  = sum8(vec_sum_sxr);
    double   score    = similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, 64, 10);
    return score;
}

double svt_ssim_4x4_hbd_avx2(const uint16_t* s, uint32_t sp, const uint16_t* r, uint32_t rp) {
    __m256i vec_sum_s    = _mm256_setzero_si256();
    __m256i vec_sum_r    = _mm256_setzero_si256();
    __m256i vec_sum_sq_s = _mm256_setzero_si256();
    __m256i vec_sum_sq_r = _mm256_setzero_si256();
    __m256i vec_sum_sxr  = _mm256_setzero_si256();
    for (int i = 0; i < 4; ++i) {
        __m256i vec_src = _mm256_cvtepu16_epi32(_mm_loadu_si64(s));
        __m256i vec_rec = _mm256_cvtepu16_epi32(_mm_loadu_si64(r));

        vec_sum_s = _mm256_add_epi32(vec_sum_s, vec_src);
        vec_sum_r = _mm256_add_epi32(vec_sum_r, vec_rec);

        __m256i vec_sq = _mm256_mullo_epi32(vec_src, vec_src);
        vec_sum_sq_s   = _mm256_add_epi32(vec_sum_sq_s, vec_sq);

        vec_sq       = _mm256_mullo_epi32(vec_rec, vec_rec);
        vec_sum_sq_r = _mm256_add_epi32(vec_sum_sq_r, vec_sq);

        __m256i vec_sxr = _mm256_mullo_epi32(vec_src, vec_rec);
        vec_sum_sxr     = _mm256_add_epi32(vec_sum_sxr, vec_sxr);

        s += sp;
        r += rp;
    }

    uint32_t sum_s    = sum8(vec_sum_s);
    uint32_t sum_r    = sum8(vec_sum_r);
    uint32_t sum_sq_s = sum8(vec_sum_sq_s);
    uint32_t sum_sq_r = sum8(vec_sum_sq_r);
    uint32_t sum_sxr  = sum8(vec_sum_sxr);
    double   score    = similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, 16, 10);
    return score;
}
