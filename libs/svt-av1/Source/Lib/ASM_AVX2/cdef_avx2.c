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
#include <immintrin.h>
#include <math.h>

#define TOTAL_STRENGTHS (CDEF_PRI_STRENGTHS * CDEF_SEC_STRENGTHS)

#ifndef _mm256_set_m128i
#define _mm256_set_m128i(/* __m128i */ hi, /* __m128i */ lo) \
    _mm256_insertf128_si256(_mm256_castsi128_si256(lo), (hi), 0x1)
#endif

#ifndef _mm256_setr_m128i
#define _mm256_setr_m128i(/* __m128i */ lo, /* __m128i */ hi) _mm256_set_m128i((hi), (lo))
#endif

/* Search for the best luma+chroma strength to add as an option, knowing we
already selected nb_strengths options. */
uint64_t svt_search_one_dual_avx2(int* lev0, int* lev1, int nb_strengths, uint64_t** mse[2], int sb_count, int start_gi,
                                  int end_gi) {
    DECLARE_ALIGNED(32, uint64_t, tot_mse[TOTAL_STRENGTHS][TOTAL_STRENGTHS]);
    uint64_t  best_tot_mse    = (uint64_t)1 << 62;
    int       best_id0        = 0;
    int       best_id1        = 0;
    const int total_strengths = end_gi;

    memset(tot_mse, 0, sizeof(tot_mse));

    for (int i = 0; i < sb_count; i++) {
        uint64_t best_mse = (uint64_t)1 << 62;
        /* Find best mse among already selected options. */
        for (int gi = 0; gi < nb_strengths; gi++) {
            uint64_t curr = mse[0][i][lev0[gi]] + mse[1][i][lev1[gi]];
            if (curr < best_mse) {
                best_mse = curr;
            }
        }
        __m256i best_mse_ = _mm256_set1_epi64x(best_mse);
        /* Find best mse when adding each possible new option. */
        //assert(~total_strengths % 4);
        for (int j = start_gi; j < total_strengths; ++j) { // process by 4x4
            __m256i tmp = _mm256_set1_epi64x(mse[0][i][j]);
            for (int k = 0; k < total_strengths; k += 4) {
                __m256i v_mse = _mm256_loadu_si256((const __m256i*)&mse[1][i][k]);
                __m256i v_tot = _mm256_loadu_si256((const __m256i*)&tot_mse[j][k]);
                __m256i curr  = _mm256_add_epi64(tmp, v_mse);
                __m256i mask  = _mm256_cmpgt_epi64(best_mse_, curr);
                v_tot         = _mm256_add_epi64(
                    v_tot, _mm256_or_si256(_mm256_andnot_si256(mask, best_mse_), _mm256_and_si256(mask, curr)));
                _mm256_storeu_si256((__m256i*)&tot_mse[j][k], v_tot);
            }
        }
    }
    for (int j = start_gi; j < total_strengths; j++) {
        for (int k = start_gi; k < total_strengths; k++) {
            if (tot_mse[j][k] < best_tot_mse) {
                best_tot_mse = tot_mse[j][k];
                best_id0     = j;
                best_id1     = k;
            }
        }
    }
    lev0[nb_strengths] = best_id0;
    lev1[nb_strengths] = best_id1;

    return best_tot_mse;
}

static INLINE void mse_4x4_16bit_2x_subsampled_avx2(const uint16_t** src, const uint16_t* dst, const int32_t dstride,
                                                    __m256i* sum) {
    const __m256i s = _mm256_loadu_si256((const __m256i*)*src);

    // set every line to src so distortion will be 0
    const __m256i d = _mm256_setr_epi64x(*(uint64_t*)(dst + 0 * dstride),
                                         *(uint64_t*)(*src + 1 * 4),
                                         *(uint64_t*)(dst + 2 * dstride),
                                         *(uint64_t*)(*src + 3 * 4));

    const __m256i diff = _mm256_sub_epi16(d, s);
    const __m256i mse  = _mm256_madd_epi16(diff, diff);
    *sum               = _mm256_add_epi32(*sum, mse);

    *src += 16;
}

static INLINE void mse_4x4_8bit_2x_subsampled_avx2(const uint8_t** src, const uint8_t* dst, const int32_t dstride,
                                                   __m256i* sum) {
    const __m128i s = _mm_loadu_si128((const __m128i*)*src);

    // set every line to src so distortion will be 0
    const __m128i d = _mm_setr_epi32(*(uint32_t*)(dst + 0 * dstride),
                                     *(uint32_t*)(*src + 1 * 4),
                                     *(uint32_t*)(dst + 2 * dstride),
                                     *(uint32_t*)(*src + 3 * 4));

    const __m256i s_16 = _mm256_cvtepu8_epi16(s);
    const __m256i d_16 = _mm256_cvtepu8_epi16(d);

    const __m256i diff = _mm256_sub_epi16(d_16, s_16);
    const __m256i mse  = _mm256_madd_epi16(diff, diff);
    *sum               = _mm256_add_epi32(*sum, mse);

    *src += 16;
}

static INLINE void mse_4xn_16bit_avx2(const uint16_t** src, const uint16_t* dst, const int32_t dstride, __m256i* sum,
                                      uint8_t height, uint8_t subsampling_factor) {
    for (int32_t r = 0; r < height; r += 4 * subsampling_factor) {
        const __m256i s = _mm256_setr_epi64x(
            *(uint64_t*)(*src + 0 * 4),
            *(uint64_t*)(*src + (1 * subsampling_factor) * 4),
            *(uint64_t*)(*src + (2 * subsampling_factor) * 4),
            *(uint64_t*)(*src +
                         (3 * subsampling_factor) * 4)); // don't add r * dstride b/c add it at end of loop iterations
        const __m256i d = _mm256_setr_epi64x(*(uint64_t*)(dst + r * dstride),
                                             *(uint64_t*)(dst + (r + (1 * subsampling_factor)) * dstride),
                                             *(uint64_t*)(dst + (r + (2 * subsampling_factor)) * dstride),
                                             *(uint64_t*)(dst + (r + (3 * subsampling_factor)) * dstride));

        const __m256i diff = _mm256_sub_epi16(d, s);
        const __m256i mse  = _mm256_madd_epi16(diff, diff);
        *sum               = _mm256_add_epi32(*sum, mse);

        *src += 4 * 4 * subsampling_factor; // with * 4 rows per iter * subsampling
    }
}

static INLINE void mse_4xn_8bit_avx2(const uint8_t** src, const uint8_t* dst, const int32_t dstride, __m256i* sum,
                                     uint8_t height, uint8_t subsampling_factor) {
    for (int32_t r = 0; r < height; r += 4 * subsampling_factor) {
        const __m128i s = _mm_setr_epi32(
            *(uint32_t*)(*src + 0 * 4),
            *(uint32_t*)(*src + (1 * subsampling_factor) * 4),
            *(uint32_t*)(*src + (2 * subsampling_factor) * 4),
            *(uint32_t*)(*src +
                         (3 * subsampling_factor) * 4)); // don't add r * dstride b/c add it at end of loop iterations
        const __m128i d = _mm_setr_epi32(*(uint32_t*)(dst + r * dstride),
                                         *(uint32_t*)(dst + (r + (1 * subsampling_factor)) * dstride),
                                         *(uint32_t*)(dst + (r + (2 * subsampling_factor)) * dstride),
                                         *(uint32_t*)(dst + (r + (3 * subsampling_factor)) * dstride));

        const __m256i s_16 = _mm256_cvtepu8_epi16(s);
        const __m256i d_16 = _mm256_cvtepu8_epi16(d);

        const __m256i diff = _mm256_sub_epi16(d_16, s_16);
        const __m256i mse  = _mm256_madd_epi16(diff, diff);
        *sum               = _mm256_add_epi32(*sum, mse);

        *src += 4 * 4 * subsampling_factor; // with * 4 rows per iter * subsampling
    }
}

static INLINE void mse_8xn_16bit_avx2(const uint16_t** src, const uint16_t* dst, const int32_t dstride, __m256i* sum,
                                      uint8_t height, uint8_t subsampling_factor) {
    for (int32_t r = 0; r < height; r += 2 * subsampling_factor) {
        const __m128i s0 = _mm_loadu_si128(
            (const __m128i*)(*src + 0 * 8)); // don't add r * dstride b/c add it at end of loop iterations
        const __m128i s1 = _mm_loadu_si128((const __m128i*)(*src + subsampling_factor * 8));
        const __m256i s  = _mm256_setr_m128i(s0, s1);
        const __m128i d0 = _mm_loadu_si128((const __m128i*)(dst + r * dstride));
        const __m128i d1 = _mm_loadu_si128((const __m128i*)(dst + (r + subsampling_factor) * dstride));
        const __m256i d  = _mm256_setr_m128i(d0, d1);

        const __m256i diff = _mm256_sub_epi16(d, s);
        const __m256i mse  = _mm256_madd_epi16(diff, diff);
        *sum               = _mm256_add_epi32(*sum, mse);

        *src += 8 * 2 * subsampling_factor;
    }
}

static INLINE void mse_8xn_8bit_avx2(const uint8_t** src, const uint8_t* dst, const int32_t dstride, __m256i* sum,
                                     uint8_t height, uint8_t subsampling_factor) {
    for (int32_t r = 0; r < height; r += 2 * subsampling_factor) {
        const __m128i s = _mm_set_epi64x(
            *(uint64_t*)(*src + subsampling_factor * 8),
            *(uint64_t*)(*src + 0 * 8)); // don't add r * dstride b/c add it at end of loop iterations
        const __m128i d = _mm_set_epi64x(*(uint64_t*)(dst + (r + subsampling_factor) * dstride),
                                         *(uint64_t*)(dst + r * dstride));

        const __m256i s_16 = _mm256_cvtepu8_epi16(s);
        const __m256i d_16 = _mm256_cvtepu8_epi16(d);

        const __m256i diff = _mm256_sub_epi16(d_16, s_16);
        const __m256i mse  = _mm256_madd_epi16(diff, diff);
        *sum               = _mm256_add_epi32(*sum, mse);

        *src += 8 * 2 * subsampling_factor;
    }
}

static INLINE void sum_32_to_64(const __m256i src, __m256i* dst) {
    const __m256i src_l = _mm256_unpacklo_epi32(src, _mm256_setzero_si256());
    const __m256i src_h = _mm256_unpackhi_epi32(src, _mm256_setzero_si256());
    *dst                = _mm256_add_epi64(*dst, src_l);
    *dst                = _mm256_add_epi64(*dst, src_h);
}

static INLINE uint64_t sum64(const __m256i src) {
    const __m128i src_l = _mm256_castsi256_si128(src);
    const __m128i src_h = _mm256_extracti128_si256(src, 1);
    const __m128i s     = _mm_add_epi64(src_l, src_h);
    const __m128i dst   = _mm_add_epi64(s, _mm_srli_si128(s, 8));

    return (uint64_t)_mm_cvtsi128_si64(dst);
}

/* Compute MSE only on the blocks we filtered. */
uint64_t svt_aom_compute_cdef_dist_16bit_avx2(const uint16_t* dst, int32_t dstride, const uint16_t* src,
                                              const CdefList* dlist, int32_t cdef_count, BlockSize bsize,
                                              int32_t coeff_shift, uint8_t subsampling_factor) {
    uint64_t sum;
    int32_t  bi, bx, by;

    __m256i mse64 = _mm256_setzero_si256();

    if (bsize == BLOCK_8X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            __m256i mse32 = _mm256_setzero_si256();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            mse_8xn_16bit_avx2(&src, dst + (8 * by + 0) * dstride + 8 * bx, dstride, &mse32, 8, subsampling_factor);
            sum_32_to_64(mse32, &mse64);
        }
    } else if (bsize == BLOCK_4X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            __m256i mse32 = _mm256_setzero_si256();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            mse_4xn_16bit_avx2(&src, dst + (8 * by + 0) * dstride + 4 * bx, dstride, &mse32, 8, subsampling_factor);
            sum_32_to_64(mse32, &mse64);
        }
    } else if (bsize == BLOCK_8X4) {
        for (bi = 0; bi < cdef_count; bi++) {
            __m256i mse32 = _mm256_setzero_si256();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            mse_8xn_16bit_avx2(&src, dst + 4 * by * dstride + 8 * bx, dstride, &mse32, 4, subsampling_factor);
            sum_32_to_64(mse32, &mse64);
        }
    } else {
        assert(bsize == BLOCK_4X4);
        for (bi = 0; bi < cdef_count; bi++) {
            __m256i mse32 = _mm256_setzero_si256();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            // For 4x4 blocks, all points can be computed at once.  Subsampling is done in a special function
            // to avoid accessing memory that doesn't belong to the current picture (since subsampling is implemented
            // as a multiplier to the step size).
            if (subsampling_factor == 2) {
                mse_4x4_16bit_2x_subsampled_avx2(&src, dst + 4 * by * dstride + 4 * bx, dstride, &mse32);
            } else {
                mse_4xn_16bit_avx2(&src, dst + 4 * by * dstride + 4 * bx, dstride, &mse32, 4,
                                   1); // no subsampling
            }
            sum_32_to_64(mse32, &mse64);
        }
    }

    sum = sum64(mse64);

    return sum >> 2 * coeff_shift;
}

uint64_t svt_aom_compute_cdef_dist_8bit_avx2(const uint8_t* dst8, int32_t dstride, const uint8_t* src8,
                                             const CdefList* dlist, int32_t cdef_count, BlockSize bsize,
                                             int32_t coeff_shift, uint8_t subsampling_factor) {
    uint64_t sum;
    int32_t  bi, bx, by;

    __m256i mse64 = _mm256_setzero_si256();

    if (bsize == BLOCK_8X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            __m256i mse32 = _mm256_setzero_si256();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            mse_8xn_8bit_avx2(&src8, dst8 + (8 * by + 0) * dstride + 8 * bx, dstride, &mse32, 8, subsampling_factor);
            sum_32_to_64(mse32, &mse64);
        }
    } else if (bsize == BLOCK_4X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            __m256i mse32 = _mm256_setzero_si256();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            mse_4xn_8bit_avx2(&src8, dst8 + (8 * by + 0) * dstride + 4 * bx, dstride, &mse32, 8, subsampling_factor);
            sum_32_to_64(mse32, &mse64);
        }
    } else if (bsize == BLOCK_8X4) {
        for (bi = 0; bi < cdef_count; bi++) {
            __m256i mse32 = _mm256_setzero_si256();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            mse_8xn_8bit_avx2(&src8, dst8 + 4 * by * dstride + 8 * bx, dstride, &mse32, 4, subsampling_factor);
            sum_32_to_64(mse32, &mse64);
        }
    } else {
        assert(bsize == BLOCK_4X4);
        for (bi = 0; bi < cdef_count; bi++) {
            __m256i mse32 = _mm256_setzero_si256();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            // For 4x4 blocks, all points can be computed at once.  Subsampling is done in a special function
            // to avoid accessing memory that doesn't belong to the current picture (since subsampling is implemented
            // as a multiplier to the step size).
            if (subsampling_factor == 2) {
                mse_4x4_8bit_2x_subsampled_avx2(&src8, dst8 + 4 * by * dstride + 4 * bx, dstride, &mse32);
            } else {
                mse_4xn_8bit_avx2(&src8, dst8 + 4 * by * dstride + 4 * bx, dstride, &mse32, 4,
                                  1); // no subsampling
            }
            sum_32_to_64(mse32, &mse64);
        }
    }

    sum = sum64(mse64);
    return sum >> 2 * coeff_shift;
}
