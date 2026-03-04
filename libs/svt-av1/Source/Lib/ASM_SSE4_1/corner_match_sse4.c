/*
 * Copyright (c) 2018, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#include <smmintrin.h>
#include "corner_match.h"
#include "definitions.h"

DECLARE_ALIGNED(16, const uint8_t, svt_aom_compute_cross_byte_mask[8][16]) = {
    {255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0}, //1x1
    {255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0}, //3x3
    {255, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0}, //5x5
    {255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0}, //7x7
    {255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0}, //9x9
    {255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0, 0}, //11x11
    {255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0}, //13x13
    {255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0}, //15x15
};

/* Compute corr(im1, im2) * MATCH_SZ * stddev(im1), where the
   correlation/standard deviation are taken over MATCH_SZ by MATCH_SZ windows
   of each image, centered at (x1, y1) and (x2, y2) respectively.
*/
double svt_av1_compute_cross_correlation_sse4_1(unsigned char* im1, int stride1, int x1, int y1, unsigned char* im2,
                                                int stride2, int x2, int y2, uint8_t match_sz) {
    int i;
    // 2 16-bit partial sums in lanes 0, 4 (== 2 32-bit partial sums in lanes 0,
    // 2)
    __m128i sum1_vec = _mm_setzero_si128();
    __m128i sum2_vec = _mm_setzero_si128();
    // 4 32-bit partial sums of squares
    __m128i       sumsq2_vec   = _mm_setzero_si128();
    __m128i       cross_vec    = _mm_setzero_si128();
    const uint8_t match_sz_by2 = ((match_sz - 1) / 2);
    const uint8_t match_sz_sq  = (match_sz * match_sz);

    int           mask_idx = match_sz / 2;
    const __m128i mask     = _mm_loadu_si128((__m128i*)svt_aom_compute_cross_byte_mask[mask_idx]);
    const __m128i zero     = _mm_setzero_si128();

    im1 += (y1 - match_sz_by2) * stride1 + (x1 - match_sz_by2);
    im2 += (y2 - match_sz_by2) * stride2 + (x2 - match_sz_by2);

    for (i = 0; i < match_sz; ++i) {
        const __m128i v1 = _mm_and_si128(_mm_loadu_si128((__m128i*)&im1[i * stride1]), mask);
        const __m128i v2 = _mm_and_si128(_mm_loadu_si128((__m128i*)&im2[i * stride2]), mask);

        // Using the 'sad' intrinsic here is a bit faster than adding
        // v1_l + v1_r and v2_l + v2_r, plus it avoids the need for a 16->32 bit
        // conversion step later, for a net speedup of ~10%
        sum1_vec = _mm_add_epi16(sum1_vec, _mm_sad_epu8(v1, zero));
        sum2_vec = _mm_add_epi16(sum2_vec, _mm_sad_epu8(v2, zero));

        const __m128i v1_l = _mm_cvtepu8_epi16(v1);
        const __m128i v1_r = _mm_cvtepu8_epi16(_mm_srli_si128(v1, 8));
        const __m128i v2_l = _mm_cvtepu8_epi16(v2);
        const __m128i v2_r = _mm_cvtepu8_epi16(_mm_srli_si128(v2, 8));

        sumsq2_vec = _mm_add_epi32(sumsq2_vec, _mm_add_epi32(_mm_madd_epi16(v2_l, v2_l), _mm_madd_epi16(v2_r, v2_r)));
        cross_vec  = _mm_add_epi32(cross_vec, _mm_add_epi32(_mm_madd_epi16(v1_l, v2_l), _mm_madd_epi16(v1_r, v2_r)));
    }

    // Now we can treat the four registers (sum1_vec, sum2_vec, sumsq2_vec,
    // cross_vec)
    // as holding 4 32-bit elements each, which we want to sum horizontally.
    // We do this by transposing and then summing vertically.
    __m128i tmp_0 = _mm_unpacklo_epi32(sum1_vec, sum2_vec);
    __m128i tmp_1 = _mm_unpackhi_epi32(sum1_vec, sum2_vec);
    __m128i tmp_2 = _mm_unpacklo_epi32(sumsq2_vec, cross_vec);
    __m128i tmp_3 = _mm_unpackhi_epi32(sumsq2_vec, cross_vec);

    __m128i tmp_4 = _mm_unpacklo_epi64(tmp_0, tmp_2);
    __m128i tmp_5 = _mm_unpackhi_epi64(tmp_0, tmp_2);
    __m128i tmp_6 = _mm_unpacklo_epi64(tmp_1, tmp_3);
    __m128i tmp_7 = _mm_unpackhi_epi64(tmp_1, tmp_3);

    __m128i res = _mm_add_epi32(_mm_add_epi32(tmp_4, tmp_5), _mm_add_epi32(tmp_6, tmp_7));

    int sum1   = _mm_extract_epi32(res, 0);
    int sum2   = _mm_extract_epi32(res, 1);
    int sumsq2 = _mm_extract_epi32(res, 2);
    int cross  = _mm_extract_epi32(res, 3);

    int var2 = sumsq2 * match_sz_sq - sum2 * sum2;
    int cov  = cross * match_sz_sq - sum1 * sum2;

    if (cov < 0) {
        return 0;
    }
    return ((double)cov * cov) / ((double)var2);
}
