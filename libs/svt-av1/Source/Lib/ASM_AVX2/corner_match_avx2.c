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

#include <immintrin.h>
#include "corner_match.h"
#include "definitions.h"

/* Compute quad of corr(im1, im2) * MATCH_SZ * stddev(im1), where the
correlation/standard deviation are taken over MATCH_SZ by MATCH_SZ windows
of each image, centered at (x1, y1) and (x2, y2) respectively.
*/
double svt_av1_compute_cross_correlation_avx2(unsigned char* im1, int stride1, int x1, int y1, unsigned char* im2,
                                              int stride2, int x2, int y2, uint8_t match_sz) {
    int           i, stride1_i = 0, stride2_i = 0;
    __m256i       temp1, sum_vec, sumsq2_vec, cross_vec, v, v1_1, v2_1;
    const uint8_t match_sz_by2 = ((match_sz - 1) / 2);
    const uint8_t match_sz_sq  = (match_sz * match_sz);

    int           mask_idx = match_sz / 2;
    const __m128i mask     = _mm_loadu_si128((__m128i*)svt_aom_compute_cross_byte_mask[mask_idx]);
    const __m256i zero     = _mm256_setzero_si256();
    __m128i       v1, v2;

    sum_vec    = zero;
    sumsq2_vec = zero;
    cross_vec  = zero;
    im1 += (y1 - match_sz_by2) * stride1 + (x1 - match_sz_by2);
    im2 += (y2 - match_sz_by2) * stride2 + (x2 - match_sz_by2);

    for (i = 0; i < match_sz; ++i) {
        v1   = _mm_and_si128(_mm_loadu_si128((__m128i*)&im1[stride1_i]), mask);
        v1_1 = _mm256_cvtepu8_epi16(v1);
        v2   = _mm_and_si128(_mm_loadu_si128((__m128i*)&im2[stride2_i]), mask);
        v2_1 = _mm256_cvtepu8_epi16(v2);

        v          = _mm256_insertf128_si256(_mm256_castsi128_si256(v1), v2, 1);
        sumsq2_vec = _mm256_add_epi32(sumsq2_vec, _mm256_madd_epi16(v2_1, v2_1));

        sum_vec   = _mm256_add_epi16(sum_vec, _mm256_sad_epu8(v, zero));
        cross_vec = _mm256_add_epi32(cross_vec, _mm256_madd_epi16(v1_1, v2_1));
        stride1_i += stride1;
        stride2_i += stride2;
    }
    __m256i sum_vec1 = _mm256_srli_si256(sum_vec, 8);
    sum_vec          = _mm256_add_epi32(sum_vec, sum_vec1);
    int sum1_acc     = _mm_cvtsi128_si32(_mm256_castsi256_si128(sum_vec));
    int sum2_acc     = _mm256_extract_epi32(sum_vec, 4);

    __m256i unp_low = _mm256_unpacklo_epi64(sumsq2_vec, cross_vec);
    __m256i unp_hig = _mm256_unpackhi_epi64(sumsq2_vec, cross_vec);
    temp1           = _mm256_add_epi32(unp_low, unp_hig);

    __m128i low_sumsq = _mm256_castsi256_si128(temp1);
    low_sumsq         = _mm_add_epi32(low_sumsq, _mm256_extractf128_si256(temp1, 1));
    low_sumsq         = _mm_add_epi32(low_sumsq, _mm_srli_epi64(low_sumsq, 32));
    int sumsq2_acc    = _mm_cvtsi128_si32(low_sumsq);
    int cross_acc     = _mm_extract_epi32(low_sumsq, 2);

    int var2 = sumsq2_acc * match_sz_sq - sum2_acc * sum2_acc;
    int cov  = cross_acc * match_sz_sq - sum1_acc * sum2_acc;
    if (cov < 0) {
        return 0;
    }
    return ((double)cov * cov) / ((double)var2);
}
