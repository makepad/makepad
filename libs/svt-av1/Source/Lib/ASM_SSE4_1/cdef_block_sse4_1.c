/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */
#include "definitions.h"
#include <smmintrin.h>
#include "cdef.h"
#include "bitstream_unit.h"

typedef struct {
    __m128i val[2];
} v256;

SIMD_INLINE __m128i v128_shr_u8(__m128i a, unsigned int c) {
    return _mm_and_si128(_mm_set1_epi8((char)(0xff >> c)), _mm_srl_epi16(a, _mm_cvtsi32_si128(c)));
}

SIMD_INLINE v256 v256_from_v128(__m128i hi, __m128i lo) {
    v256 t;
    t.val[1] = hi;
    t.val[0] = lo;
    return t;
}

SIMD_INLINE v256 v256_dup_16(uint16_t x) {
    __m128i t = _mm_set1_epi16(x);
    return v256_from_v128(t, t);
}

SIMD_INLINE v256 v256_zero(void) {
    return v256_from_v128(_mm_setzero_si128(), _mm_setzero_si128());
}

SIMD_INLINE v256 v256_max_s16(v256 a, v256 b) {
    return v256_from_v128(_mm_max_epi16(a.val[1], b.val[1]), _mm_max_epi16(a.val[0], b.val[0]));
}

SIMD_INLINE v256 v256_andn(v256 a, v256 b) {
    return v256_from_v128(_mm_andnot_si128(b.val[1], a.val[1]), _mm_andnot_si128(b.val[0], a.val[0]));
}

SIMD_INLINE v256 v256_cmpeq_16(v256 a, v256 b) {
    return v256_from_v128(_mm_cmpeq_epi16(a.val[1], b.val[1]), _mm_cmpeq_epi16(a.val[0], b.val[0]));
}

SIMD_INLINE v256 v256_min_s16(v256 a, v256 b) {
    return v256_from_v128(_mm_min_epi16(a.val[1], b.val[1]), _mm_min_epi16(a.val[0], b.val[0]));
}

SIMD_INLINE v256 v256_sub_16(v256 a, v256 b) {
    return v256_from_v128(_mm_sub_epi16(a.val[1], b.val[1]), _mm_sub_epi16(a.val[0], b.val[0]));
}

SIMD_INLINE v256 v256_add_16(v256 a, v256 b) {
    return v256_from_v128(_mm_add_epi16(a.val[1], b.val[1]), _mm_add_epi16(a.val[0], b.val[0]));
}

SIMD_INLINE v256 v256_madd_us8(v256 a, v256 b) {
    return v256_from_v128(_mm_maddubs_epi16(a.val[1], b.val[1]), _mm_maddubs_epi16(a.val[0], b.val[0]));
}

SIMD_INLINE v256 v256_dup_8(uint8_t x) {
    __m128i t = _mm_set1_epi8(x);
    return v256_from_v128(t, t);
}

SIMD_INLINE v256 v256_cmplt_s16(v256 a, v256 b) {
    return v256_from_v128(_mm_cmplt_epi16(a.val[1], b.val[1]), _mm_cmplt_epi16(a.val[0], b.val[0]));
}

SIMD_INLINE v256 v256_pack_s16_u8(v256 a, v256 b) {
    return v256_from_v128(_mm_packus_epi16(a.val[0], a.val[1]), _mm_packus_epi16(b.val[0], b.val[1]));
}

SIMD_INLINE v256 v256_from_v64(__m128i a, __m128i b, __m128i c, __m128i d) {
    return v256_from_v128(_mm_unpacklo_epi64(b, a), _mm_unpacklo_epi64(d, c));
}

SIMD_INLINE v256 v256_mullo_s16(v256 a, v256 b) {
    return v256_from_v128(_mm_mullo_epi16(a.val[1], b.val[1]), _mm_mullo_epi16(a.val[0], b.val[0]));
}

SIMD_INLINE v256 v256_abs_s16(v256 a) {
    return v256_from_v128(_mm_abs_epi16(a.val[1]), _mm_abs_epi16(a.val[0]));
}

SIMD_INLINE v256 v256_ssub_u16(v256 a, v256 b) {
    return v256_from_v128(_mm_subs_epu16(a.val[1], b.val[1]), _mm_subs_epu16(a.val[0], b.val[0]));
}

SIMD_INLINE v256 v256_shr_u16(v256 a, const unsigned int c) {
    return v256_from_v128(_mm_srli_epi16(a.val[1], c), _mm_srli_epi16(a.val[0], c));
}

SIMD_INLINE v256 v256_xor(v256 a, v256 b) {
    return v256_from_v128(_mm_xor_si128(a.val[1], b.val[1]), _mm_xor_si128(a.val[0], b.val[0]));
}

#define v128_shr_n_s16(a, c) _mm_srai_epi16(a, c)
#define v256_shr_n_s16(a, n) v256_from_v128(v128_shr_n_s16(a.val[1], n), v128_shr_n_s16(a.val[0], n))

// sign(a - b) * min(abs(a - b), max(0, strength - (abs(a - b) >> adjdamp)))
SIMD_INLINE __m128i constrain(v256 a, v256 b, unsigned int strength, unsigned int adjdamp) {
    const v256    diff16 = v256_sub_16(a, b);
    __m128i       diff   = _mm_packs_epi16(diff16.val[0], diff16.val[1]);
    const __m128i sign   = _mm_cmplt_epi8(diff, _mm_setzero_si128());
    diff                 = _mm_abs_epi8(diff);
    return _mm_xor_si128(
        _mm_add_epi8(sign, _mm_min_epu8(diff, _mm_subs_epu8(_mm_set1_epi8(strength), v128_shr_u8(diff, adjdamp)))),
        sign);
}

SIMD_INLINE v256 constrain16(v256 a, v256 b, unsigned int threshold, unsigned int adjdamp) {
    v256       diff = v256_sub_16(a, b);
    const v256 sign = v256_shr_n_s16(diff, 15);
    diff            = v256_abs_s16(diff);
    const v256 s    = v256_ssub_u16(v256_dup_16(threshold), v256_shr_u16(diff, adjdamp));
    return v256_xor(v256_add_16(sign, v256_min_s16(diff, s)), sign);
}

void svt_av1_cdef_filter_block_8xn_8_sse4_1(uint8_t* dst, int dstride, const uint16_t* in, int pri_strength,
                                            int sec_strength, int dir, int pri_damping, int sec_damping,
                                            int coeff_shift, uint8_t height, uint8_t subsampling_factor) {
    int           i;
    __m128i       p0, p1, p2, p3;
    v256          sum, row, res, tap;
    v256          max, min, large = v256_dup_16(CDEF_VERY_LARGE);
    const int32_t po1  = svt_aom_eb_cdef_directions[dir][0];
    const int32_t po2  = svt_aom_eb_cdef_directions[dir][1];
    const int32_t s1o1 = svt_aom_eb_cdef_directions[(dir + 2)][0];
    const int32_t s1o2 = svt_aom_eb_cdef_directions[(dir + 2)][1];
    const int32_t s2o1 = svt_aom_eb_cdef_directions[(dir - 2)][0];
    const int32_t s2o2 = svt_aom_eb_cdef_directions[(dir - 2)][1];

    const int* pri_taps = svt_aom_eb_cdef_pri_taps[(pri_strength >> coeff_shift) & 1];
    const int* sec_taps = svt_aom_eb_cdef_sec_taps[0];

    if (pri_strength) {
        pri_damping = AOMMAX(0, pri_damping - get_msb(pri_strength));
    }
    if (sec_strength) {
        sec_damping = AOMMAX(0, sec_damping - get_msb(sec_strength));
    }
    for (i = 0; i < height; i += (2 * subsampling_factor)) {
        sum = v256_zero();
        row = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE)));

        max = min = row;
        // Primary near taps
        tap = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + po1)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + po1)));
        max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
        min = v256_min_s16(min, tap);
        p0  = constrain(tap, row, pri_strength, pri_damping);
        tap = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - po1)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - po1)));
        max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
        min = v256_min_s16(min, tap);
        p1  = constrain(tap, row, pri_strength, pri_damping);

        // sum += pri_taps[0] * (p0 + p1)
        sum = v256_add_16(sum,
                          v256_madd_us8(v256_dup_8(pri_taps[0]),
                                        v256_from_v128(_mm_unpackhi_epi8(p1, p0), _mm_unpacklo_epi8(p1, p0))));

        // Primary far taps
        tap = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + po2)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + po2)));
        max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
        min = v256_min_s16(min, tap);
        p0  = constrain(tap, row, pri_strength, pri_damping);
        tap = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - po2)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - po2)));
        max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
        min = v256_min_s16(min, tap);
        p1  = constrain(tap, row, pri_strength, pri_damping);

        // sum += pri_taps[1] * (p0 + p1)
        sum = v256_add_16(sum,
                          v256_madd_us8(v256_dup_8(pri_taps[1]),
                                        v256_from_v128(_mm_unpackhi_epi8(p1, p0), _mm_unpacklo_epi8(p1, p0))));

        // Secondary near taps
        tap = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + s1o1)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + s1o1)));
        max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
        min = v256_min_s16(min, tap);
        p0  = constrain(tap, row, sec_strength, sec_damping);
        tap = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - s1o1)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - s1o1)));
        max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
        min = v256_min_s16(min, tap);
        p1  = constrain(tap, row, sec_strength, sec_damping);
        tap = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + s2o1)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + s2o1)));
        max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
        min = v256_min_s16(min, tap);
        p2  = constrain(tap, row, sec_strength, sec_damping);
        tap = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - s2o1)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - s2o1)));
        max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
        min = v256_min_s16(min, tap);
        p3  = constrain(tap, row, sec_strength, sec_damping);

        // sum += sec_taps[0] * (p0 + p1 + p2 + p3)
        p0  = _mm_add_epi8(p0, p1);
        p2  = _mm_add_epi8(p2, p3);
        sum = v256_add_16(sum,
                          v256_madd_us8(v256_dup_8(sec_taps[0]),
                                        v256_from_v128(_mm_unpackhi_epi8(p2, p0), _mm_unpacklo_epi8(p2, p0))));

        // Secondary far taps
        tap = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + s1o2)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + s1o2)));
        max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
        min = v256_min_s16(min, tap);
        p0  = constrain(tap, row, sec_strength, sec_damping);
        tap = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - s1o2)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - s1o2)));
        max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
        min = v256_min_s16(min, tap);
        p1  = constrain(tap, row, sec_strength, sec_damping);
        tap = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + s2o2)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + s2o2)));
        max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
        min = v256_min_s16(min, tap);
        p2  = constrain(tap, row, sec_strength, sec_damping);
        tap = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - s2o2)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - s2o2)));
        max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
        min = v256_min_s16(min, tap);
        p3  = constrain(tap, row, sec_strength, sec_damping);

        // sum += sec_taps[1] * (p0 + p1 + p2 + p3)
        p0  = _mm_add_epi8(p0, p1);
        p2  = _mm_add_epi8(p2, p3);
        sum = v256_add_16(sum,
                          v256_madd_us8(v256_dup_8(sec_taps[1]),
                                        v256_from_v128(_mm_unpackhi_epi8(p2, p0), _mm_unpacklo_epi8(p2, p0))));

        // res = row + ((sum - (sum < 0) + 8) >> 4)
        sum = v256_add_16(sum, v256_cmplt_s16(sum, v256_zero()));
        res = v256_add_16(sum, v256_dup_16(8));
        res = v256_shr_n_s16(res, 4);
        res = v256_add_16(row, res);
        res = v256_min_s16(v256_max_s16(res, min), max);
        res = v256_pack_s16_u8(res, res);

        p0 = res.val[0];
        _mm_storel_epi64((__m128i*)(dst + i * dstride), _mm_srli_si128(p0, 8));
        _mm_storel_epi64((__m128i*)(dst + (i + subsampling_factor) * dstride), p0);
    }
}

void svt_av1_cdef_filter_block_4xn_8_sse4_1(uint8_t* dst, int dstride, const uint16_t* in, int pri_strength,
                                            int sec_strength, int dir, int pri_damping, int sec_damping,
                                            int coeff_shift, uint8_t height, uint8_t subsampling_factor) {
    __m128i       p0, p1, p2, p3;
    v256          sum, row, tap, res;
    v256          max, min, large = v256_dup_16(CDEF_VERY_LARGE);
    const int32_t po1  = svt_aom_eb_cdef_directions[dir][0];
    const int32_t po2  = svt_aom_eb_cdef_directions[dir][1];
    const int32_t s1o1 = svt_aom_eb_cdef_directions[(dir + 2)][0];
    const int32_t s1o2 = svt_aom_eb_cdef_directions[(dir + 2)][1];
    const int32_t s2o1 = svt_aom_eb_cdef_directions[(dir - 2)][0];
    const int32_t s2o2 = svt_aom_eb_cdef_directions[(dir - 2)][1];

    const int* pri_taps = svt_aom_eb_cdef_pri_taps[(pri_strength >> coeff_shift) & 1];
    const int* sec_taps = svt_aom_eb_cdef_sec_taps[(pri_strength >> coeff_shift) & 1];

    if (pri_strength) {
        pri_damping = AOMMAX(0, pri_damping - get_msb(pri_strength));
    }
    if (sec_strength) {
        sec_damping = AOMMAX(0, sec_damping - get_msb(sec_strength));
    }

    for (uint32_t i = 0; i < height; i += (4 * subsampling_factor)) {
        sum = v256_zero();
        row = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE)),
                            _mm_loadl_epi64((__m128i*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE)),
                            _mm_loadl_epi64((__m128i*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE)),
                            _mm_loadl_epi64((__m128i*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE)));
        max = min = row;

        if (pri_strength) {
            // Primary near taps
            tap = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE + po1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + po1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + po1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + po1)));
            max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
            min = v256_min_s16(min, tap);
            p0  = constrain(tap, row, pri_strength, pri_damping);
            tap = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE - po1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - po1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - po1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - po1)));
            max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
            min = v256_min_s16(min, tap);
            p1  = constrain(tap, row, pri_strength, pri_damping);

            // sum += pri_taps[0] * (p0 + p1)
            sum = v256_add_16(sum,
                              v256_madd_us8(v256_dup_8(pri_taps[0]),
                                            v256_from_v128(_mm_unpackhi_epi8(p1, p0), _mm_unpacklo_epi8(p1, p0))));

            // Primary far taps
            tap = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE + po2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + po2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + po2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + po2)));
            max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
            min = v256_min_s16(min, tap);
            p0  = constrain(tap, row, pri_strength, pri_damping);
            tap = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE - po2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - po2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - po2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - po2)));
            max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
            min = v256_min_s16(min, tap);
            p1  = constrain(tap, row, pri_strength, pri_damping);

            // sum += pri_taps[1] * (p0 + p1)
            sum = v256_add_16(sum,
                              v256_madd_us8(v256_dup_8(pri_taps[1]),
                                            v256_from_v128(_mm_unpackhi_epi8(p1, p0), _mm_unpacklo_epi8(p1, p0))));
        }

        if (sec_strength) {
            // Secondary near taps
            tap = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE + s1o1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + s1o1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + s1o1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + s1o1)));
            max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
            min = v256_min_s16(min, tap);
            p0  = constrain(tap, row, sec_strength, sec_damping);
            tap = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE - s1o1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - s1o1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - s1o1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - s1o1)));
            max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
            min = v256_min_s16(min, tap);
            p1  = constrain(tap, row, sec_strength, sec_damping);
            tap = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE + s2o1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + s2o1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + s2o1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + s2o1)));
            max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
            min = v256_min_s16(min, tap);
            p2  = constrain(tap, row, sec_strength, sec_damping);
            tap = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE - s2o1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - s2o1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - s2o1)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - s2o1)));
            max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
            min = v256_min_s16(min, tap);
            p3  = constrain(tap, row, sec_strength, sec_damping);

            // sum += sec_taps[0] * (p0 + p1 + p2 + p3)
            p0  = _mm_add_epi8(p0, p1);
            p2  = _mm_add_epi8(p2, p3);
            sum = v256_add_16(sum,
                              v256_madd_us8(v256_dup_8(sec_taps[0]),
                                            v256_from_v128(_mm_unpackhi_epi8(p2, p0), _mm_unpacklo_epi8(p2, p0))));

            // Secondary far taps
            tap = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE + s1o2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + s1o2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + s1o2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + s1o2)));
            max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
            min = v256_min_s16(min, tap);
            p0  = constrain(tap, row, sec_strength, sec_damping);
            tap = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE - s1o2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - s1o2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - s1o2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - s1o2)));
            max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
            min = v256_min_s16(min, tap);
            p1  = constrain(tap, row, sec_strength, sec_damping);
            tap = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE + s2o2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE + s2o2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE + s2o2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE + s2o2)));
            max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
            min = v256_min_s16(min, tap);
            p2  = constrain(tap, row, sec_strength, sec_damping);
            tap = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE - s2o2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (1 * subsampling_factor)) * CDEF_BSTRIDE - s2o2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (2 * subsampling_factor)) * CDEF_BSTRIDE - s2o2)),
                                _mm_loadl_epi64((__m128i*)(in + (i + (3 * subsampling_factor)) * CDEF_BSTRIDE - s2o2)));
            max = v256_max_s16(max, v256_andn(tap, v256_cmpeq_16(tap, large)));
            min = v256_min_s16(min, tap);
            p3  = constrain(tap, row, sec_strength, sec_damping);

            // sum += sec_taps[1] * (p0 + p1 + p2 + p3)
            p0 = _mm_add_epi8(p0, p1);
            p2 = _mm_add_epi8(p2, p3);

            sum = v256_add_16(sum,
                              v256_madd_us8(v256_dup_8(sec_taps[1]),
                                            v256_from_v128(_mm_unpackhi_epi8(p2, p0), _mm_unpacklo_epi8(p2, p0))));
        }

        // res = row + ((sum - (sum < 0) + 8) >> 4)
        sum = v256_add_16(sum, v256_cmplt_s16(sum, v256_zero()));
        res = v256_add_16(sum, v256_dup_16(8));
        res = v256_shr_n_s16(res, 4);
        res = v256_add_16(row, res);
        res = v256_min_s16(v256_max_s16(res, min), max);
        res = v256_pack_s16_u8(res, res);

        p0                                                             = res.val[0];
        *((uint32_t*)(dst + i * dstride))                              = _mm_cvtsi128_si32(_mm_srli_si128(p0, 12));
        *((uint32_t*)(dst + (i + (1 * subsampling_factor)) * dstride)) = _mm_cvtsi128_si32(_mm_srli_si128(p0, 8));
        *((uint32_t*)(dst + (i + (2 * subsampling_factor)) * dstride)) = _mm_cvtsi128_si32(_mm_srli_si128(p0, 4));
        *((uint32_t*)(dst + (i + (3 * subsampling_factor)) * dstride)) = _mm_cvtsi128_si32(p0);
    }
}

void svt_av1_cdef_filter_block_8xn_16_sse4_1(uint16_t* dst, int dstride, const uint16_t* in, int pri_strength,
                                             int sec_strength, int dir, int pri_damping, int sec_damping,
                                             int coeff_shift, uint8_t height, uint8_t subsampling_factor) {
    int           i;
    v256          sum, p0, p1, p2, p3, row, res;
    v256          max, min, large = v256_dup_16(CDEF_VERY_LARGE);
    const int32_t po1  = svt_aom_eb_cdef_directions[dir][0];
    const int32_t po2  = svt_aom_eb_cdef_directions[dir][1];
    const int32_t s1o1 = svt_aom_eb_cdef_directions[(dir + 2)][0];
    const int32_t s1o2 = svt_aom_eb_cdef_directions[(dir + 2)][1];
    const int32_t s2o1 = svt_aom_eb_cdef_directions[(dir - 2)][0];
    const int32_t s2o2 = svt_aom_eb_cdef_directions[(dir - 2)][1];

    const int* pri_taps = svt_aom_eb_cdef_pri_taps[(pri_strength >> coeff_shift) & 1];
    const int* sec_taps = svt_aom_eb_cdef_sec_taps[(pri_strength >> coeff_shift) & 1];

    if (pri_strength) {
        pri_damping = AOMMAX(0, pri_damping - get_msb(pri_strength));
    }
    if (sec_strength) {
        sec_damping = AOMMAX(0, sec_damping - get_msb(sec_strength));
    }

    for (i = 0; i < height; i += (2 * subsampling_factor)) {
        sum = v256_zero();
        row = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE)),
                             _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE)));

        min = max = row;
        // Primary near taps
        p0  = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + po1)),
                            _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + po1)));
        p1  = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - po1)),
                            _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - po1)));
        max = v256_max_s16(v256_max_s16(max, v256_andn(p0, v256_cmpeq_16(p0, large))),
                           v256_andn(p1, v256_cmpeq_16(p1, large)));
        min = v256_min_s16(v256_min_s16(min, p0), p1);
        p0  = constrain16(p0, row, pri_strength, pri_damping);
        p1  = constrain16(p1, row, pri_strength, pri_damping);

        // sum += pri_taps[0] * (p0 + p1)
        sum = v256_add_16(sum, v256_mullo_s16(v256_dup_16(pri_taps[0]), v256_add_16(p0, p1)));

        // Primary far taps
        p0  = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + po2)),
                            _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + po2)));
        p1  = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - po2)),
                            _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - po2)));
        max = v256_max_s16(v256_max_s16(max, v256_andn(p0, v256_cmpeq_16(p0, large))),
                           v256_andn(p1, v256_cmpeq_16(p1, large)));
        min = v256_min_s16(v256_min_s16(min, p0), p1);
        p0  = constrain16(p0, row, pri_strength, pri_damping);
        p1  = constrain16(p1, row, pri_strength, pri_damping);

        // sum += pri_taps[1] * (p0 + p1)
        sum = v256_add_16(sum, v256_mullo_s16(v256_dup_16(pri_taps[1]), v256_add_16(p0, p1)));

        // Secondary near taps
        p0  = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + s1o1)),
                            _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + s1o1)));
        p1  = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - s1o1)),
                            _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - s1o1)));
        p2  = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + s2o1)),
                            _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + s2o1)));
        p3  = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - s2o1)),
                            _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - s2o1)));
        max = v256_max_s16(v256_max_s16(max, v256_andn(p0, v256_cmpeq_16(p0, large))),
                           v256_andn(p1, v256_cmpeq_16(p1, large)));
        max = v256_max_s16(v256_max_s16(max, v256_andn(p2, v256_cmpeq_16(p2, large))),
                           v256_andn(p3, v256_cmpeq_16(p3, large)));
        min = v256_min_s16(v256_min_s16(v256_min_s16(v256_min_s16(min, p0), p1), p2), p3);
        p0  = constrain16(p0, row, sec_strength, sec_damping);
        p1  = constrain16(p1, row, sec_strength, sec_damping);
        p2  = constrain16(p2, row, sec_strength, sec_damping);
        p3  = constrain16(p3, row, sec_strength, sec_damping);

        // sum += sec_taps[0] * (p0 + p1 + p2 + p3)
        sum = v256_add_16(
            sum, v256_mullo_s16(v256_dup_16(sec_taps[0]), v256_add_16(v256_add_16(p0, p1), v256_add_16(p2, p3))));

        // Secondary far taps
        p0  = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + s1o2)),
                            _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + s1o2)));
        p1  = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - s1o2)),
                            _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - s1o2)));
        p2  = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE + s2o2)),
                            _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE + s2o2)));
        p3  = v256_from_v128(_mm_loadu_si128((__m128i*)(in + i * CDEF_BSTRIDE - s2o2)),
                            _mm_loadu_si128((__m128i*)(in + (i + subsampling_factor) * CDEF_BSTRIDE - s2o2)));
        max = v256_max_s16(v256_max_s16(max, v256_andn(p0, v256_cmpeq_16(p0, large))),
                           v256_andn(p1, v256_cmpeq_16(p1, large)));
        max = v256_max_s16(v256_max_s16(max, v256_andn(p2, v256_cmpeq_16(p2, large))),
                           v256_andn(p3, v256_cmpeq_16(p3, large)));
        min = v256_min_s16(v256_min_s16(v256_min_s16(v256_min_s16(min, p0), p1), p2), p3);
        p0  = constrain16(p0, row, sec_strength, sec_damping);
        p1  = constrain16(p1, row, sec_strength, sec_damping);
        p2  = constrain16(p2, row, sec_strength, sec_damping);
        p3  = constrain16(p3, row, sec_strength, sec_damping);

        // sum += sec_taps[1] * (p0 + p1 + p2 + p3)
        sum = v256_add_16(
            sum, v256_mullo_s16(v256_dup_16(sec_taps[1]), v256_add_16(v256_add_16(p0, p1), v256_add_16(p2, p3))));

        // res = row + ((sum - (sum < 0) + 8) >> 4)
        sum = v256_add_16(sum, v256_cmplt_s16(sum, v256_zero()));
        res = v256_add_16(sum, v256_dup_16(8));
        res = v256_shr_n_s16(res, 4);
        res = v256_add_16(row, res);
        res = v256_min_s16(v256_max_s16(res, min), max);

        _mm_storeu_si128((__m128i*)(dst + i * dstride), res.val[1]);
        _mm_storeu_si128((__m128i*)(dst + (i + subsampling_factor) * dstride), res.val[0]);
    }
}

void svt_av1_cdef_filter_block_4xn_16_sse4_1(uint16_t* dst, int dstride, const uint16_t* in, int pri_strength,
                                             int sec_strength, int dir, int pri_damping, int sec_damping,
                                             int coeff_shift, uint8_t height, uint8_t subsampling_factor) {
    int           i;
    v256          p0, p1, p2, p3, sum, row, res;
    v256          max, min, large = v256_dup_16(CDEF_VERY_LARGE);
    const int32_t po1  = svt_aom_eb_cdef_directions[dir][0];
    const int32_t po2  = svt_aom_eb_cdef_directions[dir][1];
    const int32_t s1o1 = svt_aom_eb_cdef_directions[(dir + 2)][0];
    const int32_t s1o2 = svt_aom_eb_cdef_directions[(dir + 2)][1];
    const int32_t s2o1 = svt_aom_eb_cdef_directions[(dir - 2)][0];
    const int32_t s2o2 = svt_aom_eb_cdef_directions[(dir - 2)][1];

    const int* pri_taps = svt_aom_eb_cdef_pri_taps[(pri_strength >> coeff_shift) & 1];
    const int* sec_taps = svt_aom_eb_cdef_sec_taps[(pri_strength >> coeff_shift) & 1];

    if (pri_strength) {
        pri_damping = AOMMAX(0, pri_damping - get_msb(pri_strength));
    }
    if (sec_strength) {
        sec_damping = AOMMAX(0, sec_damping - get_msb(sec_strength));
    }
    for (i = 0; i < height; i += (4 * subsampling_factor)) {
        sum = v256_zero();
        row = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE)),
                            _mm_loadl_epi64((__m128i*)(in + (i + 1 * subsampling_factor) * CDEF_BSTRIDE)),
                            _mm_loadl_epi64((__m128i*)(in + (i + 2 * subsampling_factor) * CDEF_BSTRIDE)),
                            _mm_loadl_epi64((__m128i*)(in + (i + 3 * subsampling_factor) * CDEF_BSTRIDE)));
        min = max = row;

        // Primary near taps
        p0  = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE + po1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 1 * subsampling_factor) * CDEF_BSTRIDE + po1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 2 * subsampling_factor) * CDEF_BSTRIDE + po1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 3 * subsampling_factor) * CDEF_BSTRIDE + po1)));
        p1  = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE - po1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 1 * subsampling_factor) * CDEF_BSTRIDE - po1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 2 * subsampling_factor) * CDEF_BSTRIDE - po1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 3 * subsampling_factor) * CDEF_BSTRIDE - po1)));
        max = v256_max_s16(v256_max_s16(max, v256_andn(p0, v256_cmpeq_16(p0, large))),
                           v256_andn(p1, v256_cmpeq_16(p1, large)));
        min = v256_min_s16(v256_min_s16(min, p0), p1);
        p0  = constrain16(p0, row, pri_strength, pri_damping);
        p1  = constrain16(p1, row, pri_strength, pri_damping);

        // sum += pri_taps[0] * (p0 + p1)
        sum = v256_add_16(sum, v256_mullo_s16(v256_dup_16(pri_taps[0]), v256_add_16(p0, p1)));

        // Primary far taps
        p0  = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE + po2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 1 * subsampling_factor) * CDEF_BSTRIDE + po2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 2 * subsampling_factor) * CDEF_BSTRIDE + po2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 3 * subsampling_factor) * CDEF_BSTRIDE + po2)));
        p1  = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE - po2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 1 * subsampling_factor) * CDEF_BSTRIDE - po2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 2 * subsampling_factor) * CDEF_BSTRIDE - po2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 3 * subsampling_factor) * CDEF_BSTRIDE - po2)));
        max = v256_max_s16(v256_max_s16(max, v256_andn(p0, v256_cmpeq_16(p0, large))),
                           v256_andn(p1, v256_cmpeq_16(p1, large)));
        min = v256_min_s16(v256_min_s16(min, p0), p1);
        p0  = constrain16(p0, row, pri_strength, pri_damping);
        p1  = constrain16(p1, row, pri_strength, pri_damping);

        // sum += pri_taps[1] * (p0 + p1)
        sum = v256_add_16(sum, v256_mullo_s16(v256_dup_16(pri_taps[1]), v256_add_16(p0, p1)));

        // Secondary near taps
        p0  = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE + s1o1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 1 * subsampling_factor) * CDEF_BSTRIDE + s1o1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 2 * subsampling_factor) * CDEF_BSTRIDE + s1o1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 3 * subsampling_factor) * CDEF_BSTRIDE + s1o1)));
        p1  = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE - s1o1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 1 * subsampling_factor) * CDEF_BSTRIDE - s1o1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 2 * subsampling_factor) * CDEF_BSTRIDE - s1o1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 3 * subsampling_factor) * CDEF_BSTRIDE - s1o1)));
        p2  = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE + s2o1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 1 * subsampling_factor) * CDEF_BSTRIDE + s2o1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 2 * subsampling_factor) * CDEF_BSTRIDE + s2o1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 3 * subsampling_factor) * CDEF_BSTRIDE + s2o1)));
        p3  = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE - s2o1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 1 * subsampling_factor) * CDEF_BSTRIDE - s2o1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 2 * subsampling_factor) * CDEF_BSTRIDE - s2o1)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 3 * subsampling_factor) * CDEF_BSTRIDE - s2o1)));
        max = v256_max_s16(v256_max_s16(max, v256_andn(p0, v256_cmpeq_16(p0, large))),
                           v256_andn(p1, v256_cmpeq_16(p1, large)));
        max = v256_max_s16(v256_max_s16(max, v256_andn(p2, v256_cmpeq_16(p2, large))),
                           v256_andn(p3, v256_cmpeq_16(p3, large)));
        min = v256_min_s16(v256_min_s16(v256_min_s16(v256_min_s16(min, p0), p1), p2), p3);
        p0  = constrain16(p0, row, sec_strength, sec_damping);
        p1  = constrain16(p1, row, sec_strength, sec_damping);
        p2  = constrain16(p2, row, sec_strength, sec_damping);
        p3  = constrain16(p3, row, sec_strength, sec_damping);

        // sum += sec_taps[0] * (p0 + p1 + p2 + p3)
        sum = v256_add_16(
            sum, v256_mullo_s16(v256_dup_16(sec_taps[0]), v256_add_16(v256_add_16(p0, p1), v256_add_16(p2, p3))));

        // Secondary far taps
        p0  = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE + s1o2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 1 * subsampling_factor) * CDEF_BSTRIDE + s1o2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 2 * subsampling_factor) * CDEF_BSTRIDE + s1o2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 3 * subsampling_factor) * CDEF_BSTRIDE + s1o2)));
        p1  = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE - s1o2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 1 * subsampling_factor) * CDEF_BSTRIDE - s1o2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 2 * subsampling_factor) * CDEF_BSTRIDE - s1o2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 3 * subsampling_factor) * CDEF_BSTRIDE - s1o2)));
        p2  = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE + s2o2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 1 * subsampling_factor) * CDEF_BSTRIDE + s2o2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 2 * subsampling_factor) * CDEF_BSTRIDE + s2o2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 3 * subsampling_factor) * CDEF_BSTRIDE + s2o2)));
        p3  = v256_from_v64(_mm_loadl_epi64((__m128i*)(in + i * CDEF_BSTRIDE - s2o2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 1 * subsampling_factor) * CDEF_BSTRIDE - s2o2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 2 * subsampling_factor) * CDEF_BSTRIDE - s2o2)),
                           _mm_loadl_epi64((__m128i*)(in + (i + 3 * subsampling_factor) * CDEF_BSTRIDE - s2o2)));
        max = v256_max_s16(v256_max_s16(max, v256_andn(p0, v256_cmpeq_16(p0, large))),
                           v256_andn(p1, v256_cmpeq_16(p1, large)));
        max = v256_max_s16(v256_max_s16(max, v256_andn(p2, v256_cmpeq_16(p2, large))),
                           v256_andn(p3, v256_cmpeq_16(p3, large)));
        min = v256_min_s16(v256_min_s16(v256_min_s16(v256_min_s16(min, p0), p1), p2), p3);
        p0  = constrain16(p0, row, sec_strength, sec_damping);
        p1  = constrain16(p1, row, sec_strength, sec_damping);
        p2  = constrain16(p2, row, sec_strength, sec_damping);
        p3  = constrain16(p3, row, sec_strength, sec_damping);

        // sum += sec_taps[1] * (p0 + p1 + p2 + p3)
        sum = v256_add_16(
            sum, v256_mullo_s16(v256_dup_16(sec_taps[1]), v256_add_16(v256_add_16(p0, p1), v256_add_16(p2, p3))));

        // res = row + ((sum - (sum < 0) + 8) >> 4)
        sum = v256_add_16(sum, v256_cmplt_s16(sum, v256_zero()));
        res = v256_add_16(sum, v256_dup_16(8));
        res = v256_shr_n_s16(res, 4);
        res = v256_add_16(row, res);
        res = v256_min_s16(v256_max_s16(res, min), max);

        _mm_storel_epi64((__m128i*)(dst + i * dstride), _mm_srli_si128(res.val[1], 8));
        _mm_storel_epi64((__m128i*)(dst + (i + 1 * subsampling_factor) * dstride), res.val[1]);
        _mm_storel_epi64((__m128i*)(dst + (i + 2 * subsampling_factor) * dstride), _mm_srli_si128(res.val[0], 8));
        _mm_storel_epi64((__m128i*)(dst + (i + 3 * subsampling_factor) * dstride), res.val[0]);
    }
}

void svt_av1_cdef_filter_block_sse4_1(uint8_t* dst8, uint16_t* dst16, int dstride, const uint16_t* in, int pri_strength,
                                      int sec_strength, int dir, int pri_damping, int sec_damping, int bsize,
                                      int coeff_shift, uint8_t subsampling_factor) {
    if (dst8) {
        if (bsize == BLOCK_8X8) {
            svt_av1_cdef_filter_block_8xn_8_sse4_1(dst8,
                                                   dstride,
                                                   in,
                                                   pri_strength,
                                                   sec_strength,
                                                   dir,
                                                   pri_damping,
                                                   sec_damping,
                                                   coeff_shift,
                                                   8,
                                                   subsampling_factor);
        } else if (bsize == BLOCK_4X8) {
            svt_av1_cdef_filter_block_4xn_8_sse4_1(dst8,
                                                   dstride,
                                                   in,
                                                   pri_strength,
                                                   sec_strength,
                                                   dir,
                                                   pri_damping,
                                                   sec_damping,
                                                   coeff_shift,
                                                   8,
                                                   subsampling_factor);
        } else if (bsize == BLOCK_8X4) {
            svt_av1_cdef_filter_block_8xn_8_sse4_1(dst8,
                                                   dstride,
                                                   in,
                                                   pri_strength,
                                                   sec_strength,
                                                   dir,
                                                   pri_damping,
                                                   sec_damping,
                                                   coeff_shift,
                                                   4,
                                                   subsampling_factor);
        } else {
            svt_av1_cdef_filter_block_4xn_8_sse4_1(
                dst8, dstride, in, pri_strength, sec_strength, dir, pri_damping, sec_damping, coeff_shift, 4, 1);
        }
    } else {
        if (bsize == BLOCK_8X8) {
            svt_av1_cdef_filter_block_8xn_16_sse4_1(dst16,
                                                    dstride,
                                                    in,
                                                    pri_strength,
                                                    sec_strength,
                                                    dir,
                                                    pri_damping,
                                                    sec_damping,
                                                    coeff_shift,
                                                    8,
                                                    subsampling_factor);
        } else if (bsize == BLOCK_4X8) {
            svt_av1_cdef_filter_block_4xn_16_sse4_1(dst16,
                                                    dstride,
                                                    in,
                                                    pri_strength,
                                                    sec_strength,
                                                    dir,
                                                    pri_damping,
                                                    sec_damping,
                                                    coeff_shift,
                                                    8,
                                                    subsampling_factor);
        } else if (bsize == BLOCK_8X4) {
            svt_av1_cdef_filter_block_8xn_16_sse4_1(dst16,
                                                    dstride,
                                                    in,
                                                    pri_strength,
                                                    sec_strength,
                                                    dir,
                                                    pri_damping,
                                                    sec_damping,
                                                    coeff_shift,
                                                    4,
                                                    subsampling_factor);
        } else {
            assert(bsize == BLOCK_4X4);
            svt_av1_cdef_filter_block_4xn_16_sse4_1(
                dst16, dstride, in, pri_strength, sec_strength, dir, pri_damping, sec_damping, coeff_shift, 4, 1);
        }
    }
}

/* partial A is a 16-bit vector of the form:
   [x8 x7 x6 x5 x4 x3 x2 x1] and partial B has the form:
   [0  y1 y2 y3 y4 y5 y6 y7].
   This function computes (x1^2+y1^2)*C1 + (x2^2+y2^2)*C2 + ...
   (x7^2+y2^7)*C7 + (x8^2+0^2)*C8 where the C1..C8 constants are in const1
   and const2. */
static INLINE __m128i fold_mul_and_sum(__m128i partiala, __m128i partialb, __m128i const1, __m128i const2) {
    __m128i tmp;
    /* Reverse partial B. */
    partialb = _mm_shuffle_epi8(partialb, _mm_set_epi32(0x0f0e0100, 0x03020504, 0x07060908, 0x0b0a0d0c));
    /* Interleave the x and y values of identical indices and pair x8 with 0. */
    tmp      = partiala;
    partiala = _mm_unpacklo_epi16(partialb, partiala);
    partialb = _mm_unpackhi_epi16(partialb, tmp);
    /* Square and add the corresponding x and y values. */
    partiala = _mm_madd_epi16(partiala, partiala);
    partialb = _mm_madd_epi16(partialb, partialb);
    /* Multiply by constant. */
    partiala = _mm_mullo_epi32(partiala, const1);
    partialb = _mm_mullo_epi32(partialb, const2);
    /* Sum all results. */
    partiala = _mm_add_epi32(partiala, partialb);
    return partiala;
}

static INLINE __m128i hsum4(__m128i x0, __m128i x1, __m128i x2, __m128i x3) {
    __m128i t0, t1, t2, t3;
    t0 = _mm_unpacklo_epi32(x0, x1);
    t1 = _mm_unpacklo_epi32(x2, x3);
    t2 = _mm_unpackhi_epi32(x0, x1);
    t3 = _mm_unpackhi_epi32(x2, x3);
    x0 = _mm_unpacklo_epi64(t0, t1);
    x1 = _mm_unpackhi_epi64(t0, t1);
    x2 = _mm_unpacklo_epi64(t2, t3);
    x3 = _mm_unpackhi_epi64(t2, t3);
    return _mm_add_epi32(_mm_add_epi32(x0, x1), _mm_add_epi32(x2, x3));
}

static INLINE void compute_directions(__m128i lines[8], int32_t tmp_cost1[4]) {
    __m128i partial4a, partial4b, partial5a, partial5b, partial7a, partial7b;
    __m128i partial6;
    __m128i tmp;
    /* Partial sums for lines 0 and 1. */
    partial4a = _mm_slli_si128(lines[0], 14);
    partial4b = _mm_srli_si128(lines[0], 2);
    partial4a = _mm_add_epi16(partial4a, _mm_slli_si128(lines[1], 12));
    partial4b = _mm_add_epi16(partial4b, _mm_srli_si128(lines[1], 4));
    tmp       = _mm_add_epi16(lines[0], lines[1]);
    partial5a = _mm_slli_si128(tmp, 10);
    partial5b = _mm_srli_si128(tmp, 6);
    partial7a = _mm_slli_si128(tmp, 4);
    partial7b = _mm_srli_si128(tmp, 12);
    partial6  = tmp;

    /* Partial sums for lines 2 and 3. */
    partial4a = _mm_add_epi16(partial4a, _mm_slli_si128(lines[2], 10));
    partial4b = _mm_add_epi16(partial4b, _mm_srli_si128(lines[2], 6));
    partial4a = _mm_add_epi16(partial4a, _mm_slli_si128(lines[3], 8));
    partial4b = _mm_add_epi16(partial4b, _mm_srli_si128(lines[3], 8));
    tmp       = _mm_add_epi16(lines[2], lines[3]);
    partial5a = _mm_add_epi16(partial5a, _mm_slli_si128(tmp, 8));
    partial5b = _mm_add_epi16(partial5b, _mm_srli_si128(tmp, 8));
    partial7a = _mm_add_epi16(partial7a, _mm_slli_si128(tmp, 6));
    partial7b = _mm_add_epi16(partial7b, _mm_srli_si128(tmp, 10));
    partial6  = _mm_add_epi16(partial6, tmp);

    /* Partial sums for lines 4 and 5. */
    partial4a = _mm_add_epi16(partial4a, _mm_slli_si128(lines[4], 6));
    partial4b = _mm_add_epi16(partial4b, _mm_srli_si128(lines[4], 10));
    partial4a = _mm_add_epi16(partial4a, _mm_slli_si128(lines[5], 4));
    partial4b = _mm_add_epi16(partial4b, _mm_srli_si128(lines[5], 12));
    tmp       = _mm_add_epi16(lines[4], lines[5]);
    partial5a = _mm_add_epi16(partial5a, _mm_slli_si128(tmp, 6));
    partial5b = _mm_add_epi16(partial5b, _mm_srli_si128(tmp, 10));
    partial7a = _mm_add_epi16(partial7a, _mm_slli_si128(tmp, 8));
    partial7b = _mm_add_epi16(partial7b, _mm_srli_si128(tmp, 8));
    partial6  = _mm_add_epi16(partial6, tmp);

    /* Partial sums for lines 6 and 7. */
    partial4a = _mm_add_epi16(partial4a, _mm_slli_si128(lines[6], 2));
    partial4b = _mm_add_epi16(partial4b, _mm_srli_si128(lines[6], 14));
    partial4a = _mm_add_epi16(partial4a, lines[7]);
    tmp       = _mm_add_epi16(lines[6], lines[7]);
    partial5a = _mm_add_epi16(partial5a, _mm_slli_si128(tmp, 4));
    partial5b = _mm_add_epi16(partial5b, _mm_srli_si128(tmp, 12));
    partial7a = _mm_add_epi16(partial7a, _mm_slli_si128(tmp, 10));
    partial7b = _mm_add_epi16(partial7b, _mm_srli_si128(tmp, 6));
    partial6  = _mm_add_epi16(partial6, tmp);

    /* Compute costs in terms of partial sums. */
    partial4a = fold_mul_and_sum(
        partial4a, partial4b, _mm_set_epi32(210, 280, 420, 840), _mm_set_epi32(105, 120, 140, 168));
    partial7a = fold_mul_and_sum(
        partial7a, partial7b, _mm_set_epi32(210, 420, 0, 0), _mm_set_epi32(105, 105, 105, 140));
    partial5a = fold_mul_and_sum(
        partial5a, partial5b, _mm_set_epi32(210, 420, 0, 0), _mm_set_epi32(105, 105, 105, 140));
    partial6 = _mm_madd_epi16(partial6, partial6);
    partial6 = _mm_mullo_epi32(partial6, _mm_set1_epi32(105));

    partial4a = hsum4(partial4a, partial5a, partial6, partial7a);
    _mm_storeu_si128((__m128i*)tmp_cost1, partial4a);
}

/* transpose and reverse the order of the lines -- equivalent to a 90-degree
counter-clockwise rotation of the pixels. */
static INLINE void array_reverse_transpose_8x8(__m128i* in, __m128i* res) {
    const __m128i tr0_0 = _mm_unpacklo_epi16(in[0], in[1]);
    const __m128i tr0_1 = _mm_unpacklo_epi16(in[2], in[3]);
    const __m128i tr0_2 = _mm_unpackhi_epi16(in[0], in[1]);
    const __m128i tr0_3 = _mm_unpackhi_epi16(in[2], in[3]);
    const __m128i tr0_4 = _mm_unpacklo_epi16(in[4], in[5]);
    const __m128i tr0_5 = _mm_unpacklo_epi16(in[6], in[7]);
    const __m128i tr0_6 = _mm_unpackhi_epi16(in[4], in[5]);
    const __m128i tr0_7 = _mm_unpackhi_epi16(in[6], in[7]);

    const __m128i tr1_0 = _mm_unpacklo_epi32(tr0_0, tr0_1);
    const __m128i tr1_1 = _mm_unpacklo_epi32(tr0_4, tr0_5);
    const __m128i tr1_2 = _mm_unpackhi_epi32(tr0_0, tr0_1);
    const __m128i tr1_3 = _mm_unpackhi_epi32(tr0_4, tr0_5);
    const __m128i tr1_4 = _mm_unpacklo_epi32(tr0_2, tr0_3);
    const __m128i tr1_5 = _mm_unpacklo_epi32(tr0_6, tr0_7);
    const __m128i tr1_6 = _mm_unpackhi_epi32(tr0_2, tr0_3);
    const __m128i tr1_7 = _mm_unpackhi_epi32(tr0_6, tr0_7);

    res[7] = _mm_unpacklo_epi64(tr1_0, tr1_1);
    res[6] = _mm_unpackhi_epi64(tr1_0, tr1_1);
    res[5] = _mm_unpacklo_epi64(tr1_2, tr1_3);
    res[4] = _mm_unpackhi_epi64(tr1_2, tr1_3);
    res[3] = _mm_unpacklo_epi64(tr1_4, tr1_5);
    res[2] = _mm_unpackhi_epi64(tr1_4, tr1_5);
    res[1] = _mm_unpacklo_epi64(tr1_6, tr1_7);
    res[0] = _mm_unpackhi_epi64(tr1_6, tr1_7);
}

uint8_t svt_aom_cdef_find_dir_sse4_1(const uint16_t* img, int32_t stride, int32_t* var, int32_t coeff_shift) {
    int     i;
    int32_t cost[8];
    int32_t best_cost = 0;
    int     best_dir  = 0;
    __m128i lines[8];
    __m128i const_128 = _mm_set1_epi16(128);
    for (i = 0; i < 8; i++) {
        lines[i] = _mm_lddqu_si128((__m128i*)&img[i * stride]);
        lines[i] = _mm_sub_epi16(_mm_sra_epi16(lines[i], _mm_cvtsi32_si128(coeff_shift)), const_128);
    }

    /* Compute "mostly vertical" directions. */
    compute_directions(lines, cost + 4);

    array_reverse_transpose_8x8(lines, lines);

    /* Compute "mostly horizontal" directions. */
    compute_directions(lines, cost);

    for (i = 0; i < 8; i++) {
        if (cost[i] > best_cost) {
            best_cost = cost[i];
            best_dir  = i;
        }
    }

    /* Difference between the optimal variance and the variance along the
     orthogonal direction. Again, the sum(x^2) terms cancel out. */
    *var = best_cost - cost[(best_dir + 4) & 7];
    /* We'd normally divide by 840, but dividing by 1024 is close enough
     for what we're going to do with this. */
    *var >>= 10;
    return best_dir;
}

void svt_aom_cdef_find_dir_dual_sse4_1(const uint16_t* img1, const uint16_t* img2, int stride, int32_t* var_out_1st,
                                       int32_t* var_out_2nd, int32_t coeff_shift, uint8_t* out_dir_1st_8x8,
                                       uint8_t* out_dir_2nd_8x8) {
    // Process first 8x8.
    *out_dir_1st_8x8 = svt_aom_cdef_find_dir_sse4_1(img1, stride, var_out_1st, coeff_shift);

    // Process second 8x8.
    *out_dir_2nd_8x8 = svt_aom_cdef_find_dir_sse4_1(img2, stride, var_out_2nd, coeff_shift);
}

void svt_aom_copy_rect8_8bit_to_16bit_sse4_1(uint16_t* dst, int32_t dstride, const uint8_t* src, int32_t sstride,
                                             int32_t v, int32_t h) {
    int32_t i, j;
    for (i = 0; i < v; i++) {
        for (j = 0; j < (h & ~0x7); j += 8) {
            __m128i row = _mm_loadl_epi64((__m128i*)&src[i * sstride + j]);
            _mm_storeu_si128((__m128i*)&dst[i * dstride + j], _mm_unpacklo_epi8(row, _mm_setzero_si128()));
        }
        for (; j < h; j++) {
            dst[i * dstride + j] = src[i * sstride + j];
        }
    }
}
