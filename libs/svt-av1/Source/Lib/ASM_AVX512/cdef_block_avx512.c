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
#include <immintrin.h>
#include "common_dsp_rtcd.h"
#include "bitstream_unit.h"
#include "cdef.h"
#include "memory_avx2.h"

static INLINE __m512i loadu_u16_8x4_avx512(const uint16_t* const src, const uint32_t stride) {
    const __m256i s0 = loadu_u16_8x2_avx2(src + 0 * stride, stride);
    const __m256i s1 = loadu_u16_8x2_avx2(src + 2 * stride, stride);
    return _mm512_inserti64x4(_mm512_castsi256_si512(s0), s1, 1);
}

// sign(a-b) * min(abs(a-b), max(0, threshold - (abs(a-b) >> adjdamp)))
static INLINE __m512i constrain16_avx512(const __m512i in0, const __m512i in1, const __m512i threshold,
                                         const __m128i damping) {
    const __m512i diff = _mm512_sub_epi16(in0, in1);
    const __m512i sign = _mm512_srai_epi16(diff, 15);
    const __m512i a    = _mm512_abs_epi16(diff);
    const __m512i l    = _mm512_srl_epi16(a, damping);
    const __m512i s    = _mm512_subs_epu16(threshold, l);
    const __m512i m    = _mm512_min_epi16(a, s);
    const __m512i d    = _mm512_add_epi16(sign, m);
    return _mm512_xor_si512(d, sign);
}

static INLINE void cdef_filter_block_8xn_16_pri_avx512(const uint16_t* const in, const __m128i damping,
                                                       const int32_t po, const __m512i row, const __m512i strength,
                                                       const __m512i pri_taps, __m512i* const max, __m512i* const min,
                                                       __m512i* const sum, uint8_t subsampling_factor) {
    const __m512i large = _mm512_set1_epi16(CDEF_VERY_LARGE);
    const __m512i p0    = loadu_u16_8x4_avx512(in + po, subsampling_factor * CDEF_BSTRIDE);
    const __m512i p1    = loadu_u16_8x4_avx512(in - po, subsampling_factor * CDEF_BSTRIDE);

    *max = _mm512_mask_max_epi16(*max, _mm512_cmpneq_epi16_mask(p0, large), p0, *max);
    *max = _mm512_mask_max_epi16(*max, _mm512_cmpneq_epi16_mask(p1, large), p1, *max);
    *min = _mm512_min_epi16(*min, p0);
    *min = _mm512_min_epi16(*min, p1);

    const __m512i q0 = constrain16_avx512(p0, row, strength, damping);
    const __m512i q1 = constrain16_avx512(p1, row, strength, damping);

    // sum += pri_taps * (p0 + p1)
    *sum = _mm512_add_epi16(*sum, _mm512_mullo_epi16(pri_taps, _mm512_add_epi16(q0, q1)));
}

static INLINE void cdef_filter_block_8xn_16_sec_avx512(const uint16_t* const in, const __m128i damping,
                                                       const int32_t so1, const int32_t so2, const __m512i row,
                                                       const __m512i strength, const __m512i sec_taps,
                                                       __m512i* const max, __m512i* const min, __m512i* const sum,
                                                       uint8_t subsampling_factor) {
    const __m512i large = _mm512_set1_epi16(CDEF_VERY_LARGE);
    const __m512i p0    = loadu_u16_8x4_avx512(in + so1, subsampling_factor * CDEF_BSTRIDE);
    const __m512i p1    = loadu_u16_8x4_avx512(in - so1, subsampling_factor * CDEF_BSTRIDE);
    const __m512i p2    = loadu_u16_8x4_avx512(in + so2, subsampling_factor * CDEF_BSTRIDE);
    const __m512i p3    = loadu_u16_8x4_avx512(in - so2, subsampling_factor * CDEF_BSTRIDE);

    *max = _mm512_mask_max_epi16(*max, _mm512_cmpneq_epi16_mask(p0, large), p0, *max);
    *max = _mm512_mask_max_epi16(*max, _mm512_cmpneq_epi16_mask(p1, large), p1, *max);
    *max = _mm512_mask_max_epi16(*max, _mm512_cmpneq_epi16_mask(p2, large), p2, *max);
    *max = _mm512_mask_max_epi16(*max, _mm512_cmpneq_epi16_mask(p3, large), p3, *max);
    *min = _mm512_min_epi16(*min, p0);
    *min = _mm512_min_epi16(*min, p1);
    *min = _mm512_min_epi16(*min, p2);
    *min = _mm512_min_epi16(*min, p3);

    const __m512i q0 = constrain16_avx512(p0, row, strength, damping);
    const __m512i q1 = constrain16_avx512(p1, row, strength, damping);
    const __m512i q2 = constrain16_avx512(p2, row, strength, damping);
    const __m512i q3 = constrain16_avx512(p3, row, strength, damping);

    // sum += sec_taps * (p0 + p1 + p2 + p3)
    *sum = _mm512_add_epi16(
        *sum, _mm512_mullo_epi16(sec_taps, _mm512_add_epi16(_mm512_add_epi16(q0, q1), _mm512_add_epi16(q2, q3))));
}

// subsampling_factor of 1 means no subsampling
// requires height/subsampling_factor >= 4
void svt_cdef_filter_block_8xn_16_avx512(const uint16_t* const in, const int32_t pri_strength,
                                         const int32_t sec_strength, const int32_t dir, int32_t pri_damping,
                                         int32_t sec_damping, const int32_t coeff_shift, uint16_t* const dst,
                                         const int32_t dstride, uint8_t height, uint8_t subsampling_factor) {
    const int32_t  po1              = svt_aom_eb_cdef_directions[dir][0];
    const int32_t  po2              = svt_aom_eb_cdef_directions[dir][1];
    const int32_t  s1o1             = svt_aom_eb_cdef_directions[(dir + 2)][0];
    const int32_t  s1o2             = svt_aom_eb_cdef_directions[(dir + 2)][1];
    const int32_t  s2o1             = svt_aom_eb_cdef_directions[(dir - 2)][0];
    const int32_t  s2o2             = svt_aom_eb_cdef_directions[(dir - 2)][1];
    const int32_t* pri_taps         = svt_aom_eb_cdef_pri_taps[(pri_strength >> coeff_shift) & 1];
    const int32_t* sec_taps         = svt_aom_eb_cdef_sec_taps[(pri_strength >> coeff_shift) & 1];
    const __m512i  pri_taps_0       = _mm512_set1_epi16(pri_taps[0]);
    const __m512i  pri_taps_1       = _mm512_set1_epi16(pri_taps[1]);
    const __m512i  sec_taps_0       = _mm512_set1_epi16(sec_taps[0]);
    const __m512i  sec_taps_1       = _mm512_set1_epi16(sec_taps[1]);
    const __m512i  duplicate_8      = _mm512_set1_epi16(8);
    const __m512i  pri_strength_256 = _mm512_set1_epi16(pri_strength);
    const __m512i  sec_strength_256 = _mm512_set1_epi16(sec_strength);
    const __m512i  zero             = _mm512_setzero_si512();

    if (pri_strength) {
        pri_damping = AOMMAX(0, pri_damping - get_msb(pri_strength));
    }
    if (sec_strength) {
        sec_damping = AOMMAX(0, sec_damping - get_msb(sec_strength));
    }

    const __m128i pri_d = _mm_cvtsi32_si128(pri_damping);
    const __m128i sec_d = _mm_cvtsi32_si128(sec_damping);

    for (uint32_t i = 0; i < height; i += (4 * subsampling_factor)) {
        const __m512i row = loadu_u16_8x4_avx512(in + i * CDEF_BSTRIDE, subsampling_factor * CDEF_BSTRIDE);
        __m512i       sum, res, max, min;

        min = max = row;
        sum       = zero;

        // Primary near taps
        cdef_filter_block_8xn_16_pri_avx512(
            in + i * CDEF_BSTRIDE, pri_d, po1, row, pri_strength_256, pri_taps_0, &max, &min, &sum, subsampling_factor);

        // Primary far taps
        cdef_filter_block_8xn_16_pri_avx512(
            in + i * CDEF_BSTRIDE, pri_d, po2, row, pri_strength_256, pri_taps_1, &max, &min, &sum, subsampling_factor);

        // Secondary near taps
        cdef_filter_block_8xn_16_sec_avx512(in + i * CDEF_BSTRIDE,
                                            sec_d,
                                            s1o1,
                                            s2o1,
                                            row,
                                            sec_strength_256,
                                            sec_taps_0,
                                            &max,
                                            &min,
                                            &sum,
                                            subsampling_factor);

        // Secondary far taps
        cdef_filter_block_8xn_16_sec_avx512(in + i * CDEF_BSTRIDE,
                                            sec_d,
                                            s1o2,
                                            s2o2,
                                            row,
                                            sec_strength_256,
                                            sec_taps_1,
                                            &max,
                                            &min,
                                            &sum,
                                            subsampling_factor);

        // res = row + ((sum - (sum < 0) + 8) >> 4)
        const __mmask32 mask = _mm512_cmpgt_epi16_mask(zero, sum);
        sum                  = _mm512_mask_add_epi16(sum, mask, sum, _mm512_set1_epi16(-1));
        res                  = _mm512_add_epi16(sum, duplicate_8);
        res                  = _mm512_srai_epi16(res, 4);
        res                  = _mm512_add_epi16(row, res);
        res                  = _mm512_max_epi16(res, min);
        res                  = _mm512_min_epi16(res, max);

        _mm_storeu_si128((__m128i*)&dst[i * dstride], _mm512_castsi512_si128(res));
        _mm_storeu_si128((__m128i*)&dst[(i + 1 * subsampling_factor) * dstride], _mm512_extracti32x4_epi32(res, 1));
        _mm_storeu_si128((__m128i*)&dst[(i + 2 * subsampling_factor) * dstride], _mm512_extracti32x4_epi32(res, 2));
        _mm_storeu_si128((__m128i*)&dst[(i + 3 * subsampling_factor) * dstride], _mm512_extracti32x4_epi32(res, 3));
    }
}
#endif // EN_AVX512_SUPPORT
