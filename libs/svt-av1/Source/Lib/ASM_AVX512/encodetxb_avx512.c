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

#include "definitions.h"

#if EN_AVX512_SUPPORT

#include <immintrin.h> /* AVX2 */
#include "synonyms.h"
#include "synonyms_avx2.h"

static INLINE __m256i txb_init_levels_32_avx512(const TranLow* const coeff) {
    const __m512i idx   = _mm512_setr_epi32(0, 4, 8, 12, 1, 5, 9, 13, 0, 0, 0, 0, 0, 0, 0, 0);
    const __m512i c0    = _mm512_loadu_si512((__m512i*)(coeff + 0 * 16));
    const __m512i c1    = _mm512_loadu_si512((__m512i*)(coeff + 1 * 16));
    const __m512i c01   = _mm512_packs_epi32(c0, c1);
    const __m512i abs01 = _mm512_abs_epi16(c01);
    const __m512i abs_8 = _mm512_packs_epi16(abs01, abs01);
    const __m512i res   = _mm512_permutexvar_epi32(idx, abs_8);
    return _mm512_castsi512_si256(res);
}

static INLINE __m512i txb_init_levels_64_avx512(const TranLow* const coeff) {
    const __m512i idx   = _mm512_setr_epi32(0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15);
    const __m512i c0    = _mm512_loadu_si512((__m512i*)(coeff + 0 * 16));
    const __m512i c1    = _mm512_loadu_si512((__m512i*)(coeff + 1 * 16));
    const __m512i c2    = _mm512_loadu_si512((__m512i*)(coeff + 2 * 16));
    const __m512i c3    = _mm512_loadu_si512((__m512i*)(coeff + 3 * 16));
    const __m512i c01   = _mm512_packs_epi32(c0, c1);
    const __m512i c23   = _mm512_packs_epi32(c2, c3);
    const __m512i abs01 = _mm512_abs_epi16(c01);
    const __m512i abs23 = _mm512_abs_epi16(c23);
    const __m512i abs_8 = _mm512_packs_epi16(abs01, abs23);
    return _mm512_permutexvar_epi32(idx, abs_8);
}

void svt_av1_txb_init_levels_avx512(const TranLow* const coeff, const int32_t width, const int32_t height,
                                    uint8_t* const levels) {
    const TranLow* cf      = coeff;
    const __m128i  x_zeros = _mm_setzero_si128();
    uint8_t*       ls      = levels;
    int32_t        i       = height;

    if (width == 4) {
        const __m256i y_zeros = _mm256_setzero_si256();

        xx_storeu_128(ls - 16, x_zeros);

        do {
            const __m256i c0    = yy_loadu_256(cf);
            const __m256i c1    = yy_loadu_256(cf + 8);
            const __m256i c01   = _mm256_packs_epi32(c0, c1);
            const __m256i abs01 = _mm256_abs_epi16(c01);
            const __m256i abs_8 = _mm256_packs_epi16(abs01, y_zeros);
            const __m256i res_  = _mm256_shuffle_epi32(abs_8, 0xd8);
            const __m256i res   = _mm256_permute4x64_epi64(res_, 0xd8);
            yy_storeu_256(ls, res);
            cf += 4 * 4;
            ls += 4 * 8;
            i -= 4;
        } while (i);

        yy_storeu_256(ls, y_zeros);
    } else if (width == 8) {
        const __m256i y_zeros = _mm256_setzero_si256();

        yy_storeu_256(ls - 24, y_zeros);

        do {
            const __m256i res  = txb_init_levels_32_avx512(cf);
            const __m128i res0 = _mm256_castsi256_si128(res);
            const __m128i res1 = _mm256_extracti128_si256(res, 1);
            xx_storel_64(ls + 0 * 12 + 0, res0);
            *(int32_t*)(ls + 0 * 12 + 8) = 0;
            _mm_storeh_epi64((__m128i*)(ls + 1 * 12 + 0), res0);
            *(int32_t*)(ls + 1 * 12 + 8) = 0;
            xx_storel_64(ls + 2 * 12 + 0, res1);
            *(int32_t*)(ls + 2 * 12 + 8) = 0;
            _mm_storeh_epi64((__m128i*)(ls + 3 * 12 + 0), res1);
            *(int32_t*)(ls + 3 * 12 + 8) = 0;
            cf += 4 * 8;
            ls += 4 * 12;
            i -= 4;
        } while (i);

        yy_storeu_256(ls + 0 * 32, y_zeros);
        xx_storeu_128(ls + 1 * 32, x_zeros);
    } else if (width == 16) {
        const __m256i y_zeros = _mm256_setzero_si256();
        const __m512i z_zeros = _mm512_setzero_si512();

        yy_storeu_256(ls - 40, y_zeros);
        xx_storel_64(ls - 8, x_zeros);

        do {
            const __m512i res  = txb_init_levels_64_avx512(cf);
            const __m256i r0   = _mm512_castsi512_si256(res);
            const __m256i r1   = _mm512_extracti64x4_epi64(res, 1);
            const __m128i res0 = _mm256_castsi256_si128(r0);
            const __m128i res1 = _mm256_extracti128_si256(r0, 1);
            const __m128i res2 = _mm256_castsi256_si128(r1);
            const __m128i res3 = _mm256_extracti128_si256(r1, 1);
            xx_storeu_128(ls + 0 * 20, res0);
            *(int32_t*)(ls + 0 * 20 + 16) = 0;
            xx_storeu_128(ls + 1 * 20, res1);
            *(int32_t*)(ls + 1 * 20 + 16) = 0;
            xx_storeu_128(ls + 2 * 20, res2);
            *(int32_t*)(ls + 2 * 20 + 16) = 0;
            xx_storeu_128(ls + 3 * 20, res3);
            *(int32_t*)(ls + 3 * 20 + 16) = 0;
            cf += 4 * 16;
            ls += 4 * 20;
            i -= 4;
        } while (i);

        _mm512_storeu_si512((__m512i*)(ls + 0 * 64), z_zeros);
        xx_storeu_128(ls + 1 * 64, x_zeros);
    } else {
        const __m512i z_zeros = _mm512_setzero_si512();

        _mm512_storeu_si512((__m512i*)(ls - 72), z_zeros);
        xx_storel_64(ls - 8, x_zeros);

        do {
            const __m512i res  = txb_init_levels_64_avx512(cf);
            const __m256i res0 = _mm512_castsi512_si256(res);
            const __m256i res1 = _mm512_extracti64x4_epi64(res, 1);
            yy_storeu_256(ls, res0);
            *(int32_t*)(ls + 32) = 0;
            yy_storeu_256(ls + 36, res1);
            *(int32_t*)(ls + 36 + 32) = 0;
            cf += 2 * 32;
            ls += 2 * 36;
            i -= 2;
        } while (i);

        _mm512_storeu_si512((__m512i*)(ls + 0 * 64), z_zeros);
        _mm512_storeu_si512((__m512i*)(ls + 1 * 64), z_zeros);
        xx_storeu_128(ls + 2 * 64, x_zeros);
    }
}
#endif // EN_AVX512_SUPPORT
