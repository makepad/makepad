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

static INLINE void mse_4x4_16bit_2x_subsampled_sse4_1(const uint16_t** src, const uint16_t* dst, const int32_t dstride,
                                                      __m128i* sum) {
    const __m128i s0 = _mm_loadu_si128((const __m128i*)*src);
    const __m128i s1 = _mm_loadu_si128((const __m128i*)(*src + 8));

    // set every line to src so distortion will be 0
    const __m128i d0 = _mm_set_epi64x(*(uint64_t*)(*src + 1 * 4), *(uint64_t*)(dst + 0 * dstride));
    const __m128i d1 = _mm_set_epi64x(*(uint64_t*)(*src + 3 * 4), *(uint64_t*)(dst + 2 * dstride));

    const __m128i diff_0 = _mm_sub_epi16(d0, s0);
    const __m128i diff_1 = _mm_sub_epi16(d1, s1);
    const __m128i mse_0  = _mm_madd_epi16(diff_0, diff_0);
    const __m128i mse_1  = _mm_madd_epi16(diff_1, diff_1);
    *sum                 = _mm_add_epi32(*sum, mse_0);
    *sum                 = _mm_add_epi32(*sum, mse_1);

    *src += 16;
}

static INLINE void mse_4x4_8bit_2x_subsampled_sse4_1(const uint8_t** src, const uint8_t* dst, const int32_t dstride,
                                                     __m128i* sum) {
    const __m128i s = _mm_loadu_si128((const __m128i*)*src);

    // set every line to src so distortion will be 0
    const __m128i d = _mm_setr_epi32(*(uint32_t*)(dst + 0 * dstride),
                                     *(uint32_t*)(*src + 1 * 4),
                                     *(uint32_t*)(dst + 2 * dstride),
                                     *(uint32_t*)(*src + 3 * 4));

    const __m128i s_16_0 = _mm_cvtepu8_epi16(s);
    const __m128i s_16_1 = _mm_cvtepu8_epi16(_mm_srli_si128(s, 8));
    const __m128i d_16_0 = _mm_cvtepu8_epi16(d);
    const __m128i d_16_1 = _mm_cvtepu8_epi16(_mm_srli_si128(d, 8));

    const __m128i diff_0 = _mm_sub_epi16(d_16_0, s_16_0);
    const __m128i diff_1 = _mm_sub_epi16(d_16_1, s_16_1);
    const __m128i mse_0  = _mm_madd_epi16(diff_0, diff_0);
    const __m128i mse_1  = _mm_madd_epi16(diff_1, diff_1);
    *sum                 = _mm_add_epi32(*sum, mse_0);
    *sum                 = _mm_add_epi32(*sum, mse_1);

    *src += 16;
}

static INLINE void mse_4xn_16bit_sse4_1(const uint16_t** src, const uint16_t* dst, const int32_t dstride, __m128i* sum,
                                        uint8_t height, uint8_t subsampling_factor) {
    for (int32_t r = 0; r < height; r += 4 * subsampling_factor) {
        const __m128i s0 = _mm_set_epi64x(*(uint64_t*)(*src + 0 * 4),
                                          *(uint64_t*)(*src + (1 * subsampling_factor) * 4));
        const __m128i s1 = _mm_set_epi64x(
            *(uint64_t*)(*src + (2 * subsampling_factor) * 4),
            *(uint64_t*)(*src +
                         (3 * subsampling_factor) * 4)); // don't add r * dstride b/c add it at end of loop iterations
        const __m128i d0 = _mm_set_epi64x(*(uint64_t*)(dst + r * dstride),
                                          *(uint64_t*)(dst + (r + (1 * subsampling_factor)) * dstride));
        const __m128i d1 = _mm_set_epi64x(*(uint64_t*)(dst + (r + (2 * subsampling_factor)) * dstride),
                                          *(uint64_t*)(dst + (r + (3 * subsampling_factor)) * dstride));

        const __m128i diff_0 = _mm_sub_epi16(d0, s0);
        const __m128i diff_1 = _mm_sub_epi16(d1, s1);
        const __m128i mse_0  = _mm_madd_epi16(diff_0, diff_0);
        const __m128i mse_1  = _mm_madd_epi16(diff_1, diff_1);
        *sum                 = _mm_add_epi32(*sum, mse_0);
        *sum                 = _mm_add_epi32(*sum, mse_1);

        *src += 4 * 4 * subsampling_factor; // with * 4 rows per iter * subsampling
    }
}

static INLINE void mse_4xn_8bit_sse4_1(const uint8_t** src, const uint8_t* dst, const int32_t dstride, __m128i* sum,
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

        const __m128i s_16_0 = _mm_cvtepu8_epi16(s);
        const __m128i s_16_1 = _mm_cvtepu8_epi16(_mm_srli_si128(s, 8));
        const __m128i d_16_0 = _mm_cvtepu8_epi16(d);
        const __m128i d_16_1 = _mm_cvtepu8_epi16(_mm_srli_si128(d, 8));

        const __m128i diff_0 = _mm_sub_epi16(d_16_0, s_16_0);
        const __m128i diff_1 = _mm_sub_epi16(d_16_1, s_16_1);
        const __m128i mse_0  = _mm_madd_epi16(diff_0, diff_0);
        const __m128i mse_1  = _mm_madd_epi16(diff_1, diff_1);
        *sum                 = _mm_add_epi32(*sum, mse_0);
        *sum                 = _mm_add_epi32(*sum, mse_1);

        *src += 4 * 4 * subsampling_factor; // with * 4 rows per iter * subsampling
    }
}

static INLINE void mse_8xn_16bit_sse4_1(const uint16_t** src, const uint16_t* dst, const int32_t dstride, __m128i* sum,
                                        uint8_t height, uint8_t subsampling_factor) {
    for (int32_t r = 0; r < height; r += 2 * subsampling_factor) {
        const __m128i s0 = _mm_loadu_si128(
            (const __m128i*)(*src + 0 * 8)); // don't add r * dstride b/c add it at end of loop iterations
        const __m128i s1 = _mm_loadu_si128((const __m128i*)(*src + subsampling_factor * 8));
        const __m128i d0 = _mm_loadu_si128((const __m128i*)(dst + r * dstride));
        const __m128i d1 = _mm_loadu_si128((const __m128i*)(dst + (r + subsampling_factor) * dstride));

        const __m128i diff_0 = _mm_sub_epi16(d0, s0);
        const __m128i diff_1 = _mm_sub_epi16(d1, s1);
        const __m128i mse_0  = _mm_madd_epi16(diff_0, diff_0);
        const __m128i mse_1  = _mm_madd_epi16(diff_1, diff_1);
        *sum                 = _mm_add_epi32(*sum, mse_0);
        *sum                 = _mm_add_epi32(*sum, mse_1);

        *src += 8 * 2 * subsampling_factor;
    }
}

static INLINE void mse_8xn_8bit_sse4_1(const uint8_t** src, const uint8_t* dst, const int32_t dstride, __m128i* sum,
                                       uint8_t height, uint8_t subsampling_factor) {
    for (int32_t r = 0; r < height; r += 2 * subsampling_factor) {
        const __m128i s_16_0 = _mm_cvtepu8_epi16(_mm_loadl_epi64((__m128i*)(*src + subsampling_factor * 8)));
        const __m128i s_16_1 = _mm_cvtepu8_epi16(_mm_loadl_epi64((__m128i*)(*src + 0 * 8)));
        const __m128i d_16_0 = _mm_cvtepu8_epi16(_mm_loadl_epi64((__m128i*)(dst + (r + subsampling_factor) * dstride)));
        const __m128i d_16_1 = _mm_cvtepu8_epi16(_mm_loadl_epi64((__m128i*)(dst + r * dstride)));

        const __m128i diff_0 = _mm_sub_epi16(d_16_0, s_16_0);
        const __m128i diff_1 = _mm_sub_epi16(d_16_1, s_16_1);
        const __m128i mse_0  = _mm_madd_epi16(diff_0, diff_0);
        const __m128i mse_1  = _mm_madd_epi16(diff_1, diff_1);
        *sum                 = _mm_add_epi32(*sum, mse_0);
        *sum                 = _mm_add_epi32(*sum, mse_1);

        *src += 8 * 2 * subsampling_factor;
    }
}

static INLINE void sum_32_to_64(const __m128i src, __m128i* dst) {
    const __m128i src_l = _mm_unpacklo_epi32(src, _mm_setzero_si128());
    const __m128i src_h = _mm_unpackhi_epi32(src, _mm_setzero_si128());
    *dst                = _mm_add_epi64(*dst, src_l);
    *dst                = _mm_add_epi64(*dst, src_h);
}

static INLINE uint64_t sum64(const __m128i src) {
    const __m128i dst = _mm_add_epi64(src, _mm_srli_si128(src, 8));

    return (uint64_t)_mm_cvtsi128_si64(dst);
}

/* Compute MSE only on the blocks we filtered. */
uint64_t svt_aom_compute_cdef_dist_16bit_sse4_1(const uint16_t* dst, int32_t dstride, const uint16_t* src,
                                                const CdefList* dlist, int32_t cdef_count, BlockSize bsize,
                                                int32_t coeff_shift, uint8_t subsampling_factor) {
    uint64_t sum;
    int32_t  bi, bx, by;

    __m128i mse64 = _mm_setzero_si128();

    if (bsize == BLOCK_8X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            __m128i mse32 = _mm_setzero_si128();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            mse_8xn_16bit_sse4_1(&src, dst + (8 * by + 0) * dstride + 8 * bx, dstride, &mse32, 8, subsampling_factor);
            sum_32_to_64(mse32, &mse64);
        }
    } else if (bsize == BLOCK_4X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            __m128i mse32 = _mm_setzero_si128();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            mse_4xn_16bit_sse4_1(&src, dst + (8 * by + 0) * dstride + 4 * bx, dstride, &mse32, 8, subsampling_factor);
            sum_32_to_64(mse32, &mse64);
        }
    } else if (bsize == BLOCK_8X4) {
        for (bi = 0; bi < cdef_count; bi++) {
            __m128i mse32 = _mm_setzero_si128();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            mse_8xn_16bit_sse4_1(&src, dst + 4 * by * dstride + 8 * bx, dstride, &mse32, 4, subsampling_factor);
            sum_32_to_64(mse32, &mse64);
        }
    } else {
        assert(bsize == BLOCK_4X4);
        for (bi = 0; bi < cdef_count; bi++) {
            __m128i mse32 = _mm_setzero_si128();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            // For 4x4 blocks, all points can be computed at once.  Subsampling is done in a special function
            // to avoid accessing memory that doesn't belong to the current picture (since subsampling is implemented
            // as a multiplier to the step size).
            if (subsampling_factor == 2) {
                mse_4x4_16bit_2x_subsampled_sse4_1(&src, dst + 4 * by * dstride + 4 * bx, dstride, &mse32);
            } else {
                mse_4xn_16bit_sse4_1(&src, dst + 4 * by * dstride + 4 * bx, dstride, &mse32, 4,
                                     1); // no subsampling
            }
            sum_32_to_64(mse32, &mse64);
        }
    }

    sum = sum64(mse64);

    return sum >> 2 * coeff_shift;
}

uint64_t svt_aom_compute_cdef_dist_8bit_sse4_1(const uint8_t* dst8, int32_t dstride, const uint8_t* src8,
                                               const CdefList* dlist, int32_t cdef_count, BlockSize bsize,
                                               int32_t coeff_shift, uint8_t subsampling_factor) {
    uint64_t sum;
    int32_t  bi, bx, by;

    __m128i mse64 = _mm_setzero_si128();

    if (bsize == BLOCK_8X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            __m128i mse32 = _mm_setzero_si128();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            mse_8xn_8bit_sse4_1(&src8, dst8 + (8 * by + 0) * dstride + 8 * bx, dstride, &mse32, 8, subsampling_factor);
            sum_32_to_64(mse32, &mse64);
        }
    } else if (bsize == BLOCK_4X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            __m128i mse32 = _mm_setzero_si128();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            mse_4xn_8bit_sse4_1(&src8, dst8 + (8 * by + 0) * dstride + 4 * bx, dstride, &mse32, 8, subsampling_factor);
            sum_32_to_64(mse32, &mse64);
        }
    } else if (bsize == BLOCK_8X4) {
        for (bi = 0; bi < cdef_count; bi++) {
            __m128i mse32 = _mm_setzero_si128();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            mse_8xn_8bit_sse4_1(&src8, dst8 + 4 * by * dstride + 8 * bx, dstride, &mse32, 4, subsampling_factor);
            sum_32_to_64(mse32, &mse64);
        }
    } else {
        assert(bsize == BLOCK_4X4);
        for (bi = 0; bi < cdef_count; bi++) {
            __m128i mse32 = _mm_setzero_si128();
            by            = dlist[bi].by;
            bx            = dlist[bi].bx;
            // For 4x4 blocks, all points can be computed at once.  Subsampling is done in a special function
            // to avoid accessing memory that doesn't belong to the current picture (since subsampling is implemented
            // as a multiplier to the step size).
            if (subsampling_factor == 2) {
                mse_4x4_8bit_2x_subsampled_sse4_1(&src8, dst8 + 4 * by * dstride + 4 * bx, dstride, &mse32);
            } else {
                mse_4xn_8bit_sse4_1(&src8, dst8 + 4 * by * dstride + 4 * bx, dstride, &mse32, 4,
                                    1); // no subsampling
            }
            sum_32_to_64(mse32, &mse64);
        }
    }

    sum = sum64(mse64);
    return sum >> 2 * coeff_shift;
}
