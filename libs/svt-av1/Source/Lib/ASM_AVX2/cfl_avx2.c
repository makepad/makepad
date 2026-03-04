/*
 * Copyright (c) 2017, Alliance for Open Media. All rights reserved
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
#include "common_dsp_rtcd.h"

static INLINE __m256i predict_unclipped(const __m256i* input, __m256i alpha_q12, __m256i alpha_sign, __m256i dc_q0) {
    __m256i ac_q3          = _mm256_loadu_si256(input);
    __m256i ac_sign        = _mm256_sign_epi16(alpha_sign, ac_q3);
    __m256i scaled_luma_q0 = _mm256_mulhrs_epi16(_mm256_abs_epi16(ac_q3), alpha_q12);
    scaled_luma_q0         = _mm256_sign_epi16(scaled_luma_q0, ac_sign);
    return _mm256_add_epi16(scaled_luma_q0, dc_q0);
}

static INLINE __m128i predict_unclipped_ssse3(const __m128i* input, __m128i alpha_q12, __m128i alpha_sign,
                                              __m128i dc_q0) {
    __m128i ac_q3          = _mm_loadu_si128(input);
    __m128i ac_sign        = _mm_sign_epi16(alpha_sign, ac_q3);
    __m128i scaled_luma_q0 = _mm_mulhrs_epi16(_mm_abs_epi16(ac_q3), alpha_q12);
    scaled_luma_q0         = _mm_sign_epi16(scaled_luma_q0, ac_sign);
    return _mm_add_epi16(scaled_luma_q0, dc_q0);
}

// Store 32-bit integer from the first element of a into memory.
static INLINE void _mm_storeh_epi32(__m128i const* mem_addr, __m128i a) {
    *((int32_t*)mem_addr) = _mm_cvtsi128_si32(a);
}

void svt_cfl_predict_lbd_avx2(const int16_t* pred_buf_q3, uint8_t* pred, int32_t pred_stride, uint8_t* dst,
                              int32_t dst_stride, int32_t alpha_q3, int32_t bit_depth, int32_t width, int32_t height) {
    (void)bit_depth;
    (void)pred_stride;
    if (width <= 16) {
        const __m128i  alpha_sign = _mm_set1_epi16(alpha_q3);
        const __m128i  alpha_q12  = _mm_slli_epi16(_mm_abs_epi16(alpha_sign), 9);
        const __m128i  dc_q0      = _mm_set1_epi16(*pred);
        __m128i*       row        = (__m128i*)pred_buf_q3;
        const __m128i* row_end    = row + height * CFL_BUF_LINE_I128;
        do {
            __m128i res = predict_unclipped_ssse3(row, alpha_q12, alpha_sign, dc_q0);
            if (width < 16) {
                res = _mm_packus_epi16(res, res);
                if (width == 4) {
                    _mm_storeh_epi32((__m128i*)dst, res);
                } else {
                    _mm_storel_epi64((__m128i*)dst, res);
                }
            } else {
                __m128i next = predict_unclipped_ssse3(row + 1, alpha_q12, alpha_sign, dc_q0);
                res          = _mm_packus_epi16(res, next);
                _mm_storeu_si128((__m128i*)dst, res);
            }
            dst += dst_stride;
        } while ((row += CFL_BUF_LINE_I128) < row_end);
    } else {
        const __m256i  alpha_sign = _mm256_set1_epi16(alpha_q3);
        const __m256i  alpha_q12  = _mm256_slli_epi16(_mm256_abs_epi16(alpha_sign), 9);
        const __m256i  dc_q0      = _mm256_set1_epi16(*pred);
        __m256i*       row        = (__m256i*)pred_buf_q3;
        const __m256i* row_end    = row + height * CFL_BUF_LINE_I256;

        do {
            __m256i res  = predict_unclipped(row, alpha_q12, alpha_sign, dc_q0);
            __m256i next = predict_unclipped(row + 1, alpha_q12, alpha_sign, dc_q0);
            res          = _mm256_packus_epi16(res, next);
            res          = _mm256_permute4x64_epi64(res, _MM_SHUFFLE(3, 1, 2, 0));
            _mm256_storeu_si256((__m256i*)dst, res);
            dst += dst_stride;
        } while ((row += CFL_BUF_LINE_I256) < row_end);
    }
}

static __m256i highbd_max_epi16(int32_t bd) {
    const __m256i neg_one = _mm256_set1_epi16(-1);
    // (1 << bd) - 1 => -(-1 << bd) -1 => -1 - (-1 << bd) => -1 ^ (-1 << bd)
    return _mm256_xor_si256(_mm256_slli_epi16(neg_one, bd), neg_one);
}

static INLINE __m128i highbd_max_epi16_ssse3(int32_t bd) {
    const __m128i neg_one = _mm_set1_epi16(-1);
    // (1 << bd) - 1 => -(-1 << bd) -1 => -1 - (-1 << bd) => -1 ^ (-1 << bd)
    return _mm_xor_si128(_mm_slli_epi16(neg_one, bd), neg_one);
}

static __m256i highbd_clamp_epi16(__m256i u, __m256i zero, __m256i max) {
    return _mm256_max_epi16(_mm256_min_epi16(u, max), zero);
}

static INLINE __m128i highbd_clamp_epi16_ssse3(__m128i u, __m128i zero, __m128i max) {
    return _mm_max_epi16(_mm_min_epi16(u, max), zero);
}

void svt_cfl_predict_hbd_avx2(const int16_t* pred_buf_q3,
                              uint16_t*      pred, // AMIR ADDED
                              int32_t        pred_stride,
                              uint16_t*      dst, // AMIR changed to 8 bit
                              int32_t dst_stride, int32_t alpha_q3, int32_t bit_depth, int32_t width, int32_t height) {
    (void)pred_stride;
    // Use SSSE3 version for smaller widths
    if (width < 16) {
        const __m128i  alpha_sign = _mm_set1_epi16(alpha_q3);
        const __m128i  alpha_q12  = _mm_slli_epi16(_mm_abs_epi16(alpha_sign), 9);
        const __m128i  dc_q0      = _mm_set1_epi16(*pred);
        const __m128i  max        = highbd_max_epi16_ssse3(bit_depth);
        const __m128i  zeros      = _mm_setzero_si128();
        __m128i*       row        = (__m128i*)pred_buf_q3;
        const __m128i* row_end    = row + height * CFL_BUF_LINE_I128;
        do {
            __m128i res = predict_unclipped_ssse3(row, alpha_q12, alpha_sign, dc_q0);
            res         = highbd_clamp_epi16_ssse3(res, zeros, max);
            if (width == 4) {
                _mm_storel_epi64((__m128i*)dst, res);
            } else {
                _mm_storeu_si128((__m128i*)dst, res);
            }
            dst += dst_stride;
        } while ((row += CFL_BUF_LINE_I128) < row_end);
    } else {
        assert(width == 16 || width == 32);
        const __m256i alpha_sign = _mm256_set1_epi16(alpha_q3);
        const __m256i alpha_q12  = _mm256_slli_epi16(_mm256_abs_epi16(alpha_sign), 9);
        const __m256i dc_q0      = _mm256_loadu_si256((__m256i*)pred);
        const __m256i max        = highbd_max_epi16(bit_depth);

        __m256i*       row     = (__m256i*)pred_buf_q3;
        const __m256i* row_end = row + height * CFL_BUF_LINE_I256;
        do {
            const __m256i res = predict_unclipped(row, alpha_q12, alpha_sign, dc_q0);
            _mm256_storeu_si256((__m256i*)dst, highbd_clamp_epi16(res, _mm256_setzero_si256(), max));
            if (width == 32) {
                const __m256i res_1 = predict_unclipped(row + 1, alpha_q12, alpha_sign, dc_q0);
                _mm256_storeu_si256((__m256i*)(dst + 16), highbd_clamp_epi16(res_1, _mm256_setzero_si256(), max));
            }
            dst += dst_stride;
        } while ((row += CFL_BUF_LINE_I256) < row_end);
    }
}

// Returns a vector where all the (32-bits) elements are the sum of all the
// lanes in a.
static INLINE __m256i fill_sum_epi32(__m256i a) {
    // Given that a == [A, b, C, D, E, F, G, H]
    a = _mm256_hadd_epi32(a, a);
    // Given that A' == A + b, C' == C + D, E' == E + F, G' == G + H
    // a == [A', C', A', C', E', G', E', G']
    a = _mm256_permute4x64_epi64(a, _MM_SHUFFLE(3, 1, 2, 0));
    // a == [A', C', E', G', A', C', E', G']
    a = _mm256_hadd_epi32(a, a);
    // Given that A'' == A' + C' and E'' == E' + G'
    // a == [A'', E'', A'', E'', A'', E'', A'', E'']
    return _mm256_hadd_epi32(a, a);
    // Given that A''' == A'' + E''
    // a == [A''', A''', A''', A''', A''', A''', A''', A''']
}

static INLINE __m128i fill_sum_epi32_sse2(__m128i l0) {
    l0 = _mm_add_epi32(l0, _mm_shuffle_epi32(l0, _MM_SHUFFLE(1, 0, 3, 2)));
    return _mm_add_epi32(l0, _mm_shuffle_epi32(l0, _MM_SHUFFLE(2, 3, 0, 1)));
}

static INLINE __m256i _mm256_addl_epi16(__m256i a) {
    return _mm256_add_epi32(_mm256_unpacklo_epi16(a, _mm256_setzero_si256()),
                            _mm256_unpackhi_epi16(a, _mm256_setzero_si256()));
}

/************************************************************************************************
* svt_subtract_average_avx2
* Calculate the DC value by averaging over all sample. Subtract DC value to get AC values In AVX2
************************************************************************************************/
/*staticINLINE*/ void svt_subtract_average_avx2(int16_t* pred_buf_q3, int32_t width, int32_t height,
                                                int32_t round_offset, int32_t num_pel_log2) {
    // Use SSE2 version for smaller widths

    if ((width == 4) || (width == 8)) {
        const __m128i        zeros              = _mm_setzero_si128();
        const __m128i        round_offset_epi32 = _mm_set1_epi32(round_offset);
        const __m128i*       src                = (__m128i*)pred_buf_q3;
        const __m128i* const end                = src + height * CFL_BUF_LINE_I128;
        const int32_t        step               = CFL_BUF_LINE_I128 * (1 + (width == 8) + 3 * (width == 4));

        __m128i sum = zeros;
        do {
            __m128i l0;
            if (width == 4) {
                l0         = _mm_add_epi16(_mm_loadl_epi64(src), _mm_loadl_epi64(src + CFL_BUF_LINE_I128));
                __m128i l1 = _mm_add_epi16(_mm_loadl_epi64(src + 2 * CFL_BUF_LINE_I128),
                                           _mm_loadl_epi64(src + 3 * CFL_BUF_LINE_I128));
                sum = _mm_add_epi32(sum, _mm_add_epi32(_mm_unpacklo_epi16(l0, zeros), _mm_unpacklo_epi16(l1, zeros)));
            } else {
                l0  = _mm_add_epi16(_mm_loadu_si128(src), _mm_loadu_si128(src + CFL_BUF_LINE_I128));
                sum = _mm_add_epi32(sum, _mm_add_epi32(_mm_unpacklo_epi16(l0, zeros), _mm_unpackhi_epi16(l0, zeros)));
            }
            src += step;
        } while (src < end);

        sum = fill_sum_epi32_sse2(sum);

        __m128i avg_epi16 = _mm_srli_epi32(_mm_add_epi32(sum, round_offset_epi32), num_pel_log2);
        avg_epi16         = _mm_packs_epi32(avg_epi16, avg_epi16);

        src          = (__m128i*)pred_buf_q3;
        __m128i* dst = (__m128i*)pred_buf_q3;
        do {
            if (width == 4) {
                _mm_storel_epi64(dst, _mm_sub_epi16(_mm_loadl_epi64(src), avg_epi16));
            } else {
                _mm_storeu_si128(dst, _mm_sub_epi16(_mm_loadu_si128(src), avg_epi16));
            }
            src += CFL_BUF_LINE_I128;
            dst += CFL_BUF_LINE_I128;
        } while (src < end);
    } else {
        const __m256i*       src = (__m256i*)pred_buf_q3;
        const __m256i* const end = src + height * CFL_BUF_LINE_I256;
        // To maximize usage of the AVX2 registers, we sum two rows per loop
        // iteration
        const int32_t step = 2 * CFL_BUF_LINE_I256;

        __m256i sum = _mm256_setzero_si256();
        // For width 32, we use a second sum accumulator to reduce accumulator
        // dependencies in the loop.
        __m256i sum2;
        if (width == 32) {
            sum2 = _mm256_setzero_si256();
        }

        do {
            // Add top row to the bottom row
            __m256i l0 = _mm256_add_epi16(_mm256_loadu_si256(src), _mm256_loadu_si256(src + CFL_BUF_LINE_I256));
            sum        = _mm256_add_epi32(sum, _mm256_addl_epi16(l0));
            if (width == 32) { /* Don't worry, this if it gets optimized out. */
                // Add the second part of the top row to the second part of the bottom row
                __m256i l1 = _mm256_add_epi16(_mm256_loadu_si256(src + 1),
                                              _mm256_loadu_si256(src + 1 + CFL_BUF_LINE_I256));
                sum2       = _mm256_add_epi32(sum2, _mm256_addl_epi16(l1));
            }
            src += step;
        } while (src < end);
        // Combine both sum accumulators
        if (width == 32) {
            sum = _mm256_add_epi32(sum, sum2);
        }

        __m256i fill = fill_sum_epi32(sum);

        __m256i avg_epi16 = _mm256_srli_epi32(_mm256_add_epi32(fill, _mm256_set1_epi32(round_offset)), num_pel_log2);
        avg_epi16         = _mm256_packs_epi32(avg_epi16, avg_epi16);

        // Store and subtract loop
        src          = (__m256i*)pred_buf_q3;
        __m256i* dst = (__m256i*)pred_buf_q3;
        do {
            _mm256_storeu_si256(dst, _mm256_sub_epi16(_mm256_loadu_si256(src), avg_epi16));
            if (width == 32) {
                _mm256_storeu_si256(dst + 1, _mm256_sub_epi16(_mm256_loadu_si256(src + 1), avg_epi16));
            }
            src += CFL_BUF_LINE_I256;
            dst += CFL_BUF_LINE_I256;
        } while (src < end);
    }
}

/************************************************************************************************
* svt_cfl_luma_subsampling_420_hbd_avx2
* Subsample luma samples to match chroma size. High bit depth and avx2
************************************************************************************************/
void svt_cfl_luma_subsampling_420_hbd_avx2(const uint16_t* input, int32_t input_stride, int16_t* output_q3,
                                           int32_t width, int32_t height) {
    const int      luma_stride = input_stride << 1;
    __m256i*       row         = (__m256i*)output_q3;
    const __m256i* row_end     = row + (height >> 1) * CFL_BUF_LINE_I256;
    do {
        if (width == 4) {
            const __m128i top = _mm_loadl_epi64((__m128i*)input);
            const __m128i bot = _mm_loadl_epi64((__m128i*)(input + input_stride));
            __m128i       sum = _mm_add_epi16(top, bot);
            sum               = _mm_hadd_epi16(sum, sum);
            *((int*)row)      = _mm_cvtsi128_si32(_mm_add_epi16(sum, sum));
        } else if (width == 8) {
            const __m128i top = _mm_loadu_si128((__m128i*)input);
            const __m128i bot = _mm_loadu_si128((__m128i*)(input + input_stride));
            __m128i       sum = _mm_add_epi16(top, bot);
            sum               = _mm_hadd_epi16(sum, sum);
            _mm_storel_epi64((__m128i*)row, _mm_add_epi16(sum, sum));
        } else if (width == 16) {
            __m256i top = _mm256_loadu_si256((__m256i*)input);
            __m256i bot = _mm256_loadu_si256((__m256i*)(input + input_stride));
            __m256i sum = _mm256_add_epi16(top, bot);
            sum         = _mm256_hadd_epi16(sum, sum);
            sum         = _mm256_add_epi16(sum, sum);
            _mm_storel_epi64((__m128i*)row, _mm256_castsi256_si128(sum));
            int16_t* ptr = (int16_t*)row;
            _mm_storel_epi64((__m128i*)(ptr + 4), _mm256_extracti128_si256(sum, 1));
        } else {
            __m256i top   = _mm256_loadu_si256((__m256i*)input);
            __m256i bot   = _mm256_loadu_si256((__m256i*)(input + input_stride));
            __m256i sum   = _mm256_add_epi16(top, bot);
            __m256i top_1 = _mm256_loadu_si256((__m256i*)(input + 16));
            __m256i bot_1 = _mm256_loadu_si256((__m256i*)(input + 16 + input_stride));
            __m256i sum_1 = _mm256_add_epi16(top_1, bot_1);
            __m256i hsum  = _mm256_hadd_epi16(sum, sum_1);
            hsum          = _mm256_permute4x64_epi64(hsum, _MM_SHUFFLE(3, 1, 2, 0));
            hsum          = _mm256_add_epi16(hsum, hsum);
            _mm256_storeu_si256(row, hsum);
            if (width == 64) {
                top   = _mm256_loadu_si256((__m256i*)(input + 32));
                bot   = _mm256_loadu_si256((__m256i*)(input + 32 + input_stride));
                sum   = _mm256_add_epi16(top, bot);
                top_1 = _mm256_loadu_si256((__m256i*)(input + 48));
                bot_1 = _mm256_loadu_si256((__m256i*)(input + 48 + input_stride));
                sum_1 = _mm256_add_epi16(top_1, bot_1);
                hsum  = _mm256_hadd_epi16(sum, sum_1);
                hsum  = _mm256_permute4x64_epi64(hsum, _MM_SHUFFLE(3, 1, 2, 0));
                hsum  = _mm256_add_epi16(hsum, hsum);
                _mm256_storeu_si256(row + 1, hsum);
            }
        }
        input += luma_stride;
    } while ((row += CFL_BUF_LINE_I256) < row_end);
}

/************************************************************************************************
* svt_cfl_luma_subsampling_420_lbd_avx2
* Subsample luma samples to match chroma size. Low bit depth and avx2
************************************************************************************************/
void svt_cfl_luma_subsampling_420_lbd_avx2(const uint8_t* input, int32_t input_stride, int16_t* output_q3,
                                           int32_t width, int32_t height) {
    const __m128i  twos_128    = _mm_set1_epi8(2);
    const __m256i  twos_256    = _mm256_set1_epi8(2); // Thirty two twos
    const int      luma_stride = input_stride << 1;
    __m256i*       row         = (__m256i*)output_q3;
    const __m256i* row_end     = row + (height >> 1) * CFL_BUF_LINE_I256;
    do {
        if (width == 4) {
            __m128i top       = _mm_cvtsi32_si128(*((int*)input));
            top               = _mm_maddubs_epi16(top, twos_128);
            __m128i bot       = _mm_cvtsi32_si128(*((int*)(input + input_stride)));
            bot               = _mm_maddubs_epi16(bot, twos_128);
            const __m128i sum = _mm_add_epi16(top, bot);
            _mm_storeh_epi32((__m128i*)row, sum);
        } else if (width == 8) {
            __m128i top       = _mm_loadl_epi64((__m128i*)input);
            top               = _mm_maddubs_epi16(top, twos_128);
            __m128i bot       = _mm_loadl_epi64((__m128i*)(input + input_stride));
            bot               = _mm_maddubs_epi16(bot, twos_128);
            const __m128i sum = _mm_add_epi16(top, bot);
            _mm_storel_epi64((__m128i*)row, sum);
        } else if (width == 16) {
            __m128i top       = _mm_loadu_si128((__m128i*)input);
            top               = _mm_maddubs_epi16(top, twos_128);
            __m128i bot       = _mm_loadu_si128((__m128i*)(input + input_stride));
            bot               = _mm_maddubs_epi16(bot, twos_128);
            const __m128i sum = _mm_add_epi16(top, bot);
            _mm_storeu_si128((__m128i*)row, sum);
        } else {
            __m256i top = _mm256_loadu_si256((__m256i*)input);
            __m256i bot = _mm256_loadu_si256((__m256i*)(input + input_stride));

            __m256i top_16x16 = _mm256_maddubs_epi16(top, twos_256);
            __m256i bot_16x16 = _mm256_maddubs_epi16(bot, twos_256);
            __m256i sum_16x16 = _mm256_add_epi16(top_16x16, bot_16x16);

            _mm256_storeu_si256(row, sum_16x16);
            if (width == 64) {
                top       = _mm256_loadu_si256(((__m256i*)input) + 1);
                bot       = _mm256_loadu_si256(((__m256i*)(input + input_stride)) + 1);
                top_16x16 = _mm256_maddubs_epi16(top, twos_256);
                bot_16x16 = _mm256_maddubs_epi16(bot, twos_256);
                sum_16x16 = _mm256_add_epi16(top_16x16, bot_16x16);
                _mm256_storeu_si256(row + 1, sum_16x16);
            }
        }
        input += luma_stride;
    } while ((row += CFL_BUF_LINE_I256) < row_end);
}
