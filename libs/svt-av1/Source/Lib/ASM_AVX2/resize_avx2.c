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
#include "memory_avx2.h"
#include "common_dsp_rtcd.h"
#include "inter_prediction.h"
#include "resize.h"
#include "synonyms_avx2.h"

#ifndef _mm_storeu_si32
#define _mm_storeu_si32(p, a) (void)(*(int*)(p) = _mm_cvtsi128_si32((a)))
#endif

static INLINE __m256i RightShiftWithRounding_S32(const __m256i v_val_d, int bits) {
    const __m256i v_bias_d = _mm256_set1_epi32((1 << bits) >> 1);
    const __m256i v_tmp_d  = _mm256_add_epi32(v_val_d, v_bias_d);
    return _mm256_srai_epi32(v_tmp_d, bits);
}

static INLINE void mm256_clamp_epi32(__m256i* x, __m256i min, __m256i max) {
    *x = _mm256_min_epi32(*x, max);
    *x = _mm256_max_epi32(*x, min);
}

static INLINE void mm256_clamp_epi16(__m256i* x, __m256i min, __m256i max) {
    *x = _mm256_min_epi16(*x, max);
    *x = _mm256_max_epi16(*x, min);
}

static INLINE void mm_blend_load_lo(const uint8_t* const input, int blend_len, __m128i* dst) {
    const int      bits_to_mask = blend_len * 8;
    const uint64_t mask_c       = (1LL << bits_to_mask) - 1;
    const __m128i  blend_mask   = _mm_set1_epi64x(mask_c);
    const __m128i  src_0        = _mm_set1_epi8(input[0]);
    *dst                        = _mm_loadl_epi64((const __m128i*)(input)); // lo 64-bit: 0 1 2 3 4 5 6 7 8
    *dst                        = _mm_slli_epi64(*dst, bits_to_mask); // lo 64-bit: x x x 0 1 2 3 4 5
    *dst                        = _mm_blendv_epi8(*dst, src_0, blend_mask);
    *dst                        = _mm_cvtepu8_epi16(*dst);
    return;
}

static INLINE void mm_blend_load_hi(const uint8_t* const input, int blend_len, __m128i* dst) {
    const int      bits_to_mask = blend_len * 8;
    const uint64_t mask_c       = ~((1LL << (64 - bits_to_mask)) - 1);
    const __m128i  blend_mask   = _mm_set1_epi64x(mask_c);
    const __m128i  src_last     = _mm_set1_epi8(input[8 - blend_len - 1]);
    *dst                        = _mm_loadl_epi64((const __m128i*)(input - blend_len)); // lo 64-bit: x x x x 0 1 2 3
    *dst                        = _mm_srli_epi64(*dst, bits_to_mask); // lo 64-bit: 0 1 2 3 x x x x
    *dst                        = _mm_blendv_epi8(*dst, src_last, blend_mask);
    *dst                        = _mm_cvtepu8_epi16(*dst);
    return;
}

static INLINE void interpolate_core_w16_avx2(__m128i short_src[16], __m128i short_filter[16], uint8_t* optr) {
    const __m256i min = _mm256_set1_epi16(0);
    const __m256i max = _mm256_set1_epi16(255);

    __m256i sum[8];

    for (int j = 0; j < 4; ++j) {
        __m256i src_ab    = _mm256_set_m128i(short_src[j + 4], short_src[j]);
        __m256i filter_ab = _mm256_set_m128i(short_filter[j + 4], short_filter[j]);

        // two pixel per vector
        sum[j] = _mm256_madd_epi16(src_ab, filter_ab);
        // sum[0]: P00ab P00cd P00ef P00gh P04ab P04cd P04ef P04gh
        // sum[1]: P01ab P01cd P01ef P01gh P05ab P05cd P05ef P05gh
        // sum[2]: P02ab P02cd P02ef P02gh P06ab P06cd P06ef P06gh
        // sum[3]: P03ab P03cd P03ef P03gh P07ab P07cd P07ef P07gh
    }
    for (int j = 4; j < 8; ++j) {
        __m256i src_ab    = _mm256_set_m128i(short_src[j + 8], short_src[j + 4]);
        __m256i filter_ab = _mm256_set_m128i(short_filter[j + 8], short_filter[j + 4]);

        // two pixel per vector
        sum[j] = _mm256_madd_epi16(src_ab, filter_ab);
        // sum[0]: P00ab P00cd P00ef P00gh P04ab P04cd P04ef P04gh
        // sum[1]: P01ab P01cd P01ef P01gh P05ab P05cd P05ef P05gh
        // sum[2]: P02ab P02cd P02ef P02gh P06ab P06cd P06ef P06gh
        // sum[3]: P03ab P03cd P03ef P03gh P07ab P07cd P07ef P07gh
    }

    __m256i sum0to7, sum8to15;
    {
        // P00abcd P00efgh P01abcd P01efgh P04abcd P04efgh P05abcd P05efgh
        __m256i sum01 = _mm256_hadd_epi32(sum[0], sum[1]);
        // P02abcd P02efgh P03abcd P03efgh P06abcd P06efgh P07abcd P07efgh
        __m256i sum23 = _mm256_hadd_epi32(sum[2], sum[3]);
        // P00 P01 P02 P03 P04 P05 P06 P07
        __m256i sum0123 = _mm256_hadd_epi32(sum01, sum23);
        sum0to7         = RightShiftWithRounding_S32(sum0123, FILTER_BITS);
    }
    {
        __m256i sum01   = _mm256_hadd_epi32(sum[4], sum[5]);
        __m256i sum23   = _mm256_hadd_epi32(sum[6], sum[7]);
        __m256i sum0123 = _mm256_hadd_epi32(sum01, sum23);
        sum8to15        = RightShiftWithRounding_S32(sum0123, FILTER_BITS);
    }

    // 16* 16-bit: P00 P01 P02 P03 P08 P09 P10 P11 P04 P05 P06 P07 P12 P13 P14 P15
    __m256i sum0to15 = _mm256_packs_epi32(sum0to7, sum8to15);
    sum0to15         = _mm256_permute4x64_epi64(sum0to15, _MM_SHUFFLE(3, 1, 2, 0));
    // P00 000 P01 000 P02 000 P03 000 P04 000 P05 000 P06 000 P07 000
    // P08 000 P09 000 P10 000 P11 000 P12 000 P13 000 P14 000 P15 000
    mm256_clamp_epi16(&sum0to15, min, max);

    __m128i lo_lane    = _mm256_castsi256_si128(sum0to15);
    __m128i hi_lane    = _mm256_extracti128_si256(sum0to15, 1);
    __m128i m128_0to15 = _mm_packus_epi16(lo_lane, hi_lane);

    _mm_storeu_si128((__m128i*)optr, m128_0to15);
}

static INLINE void highbd_interpolate_core_w8_avx2(__m256i src[8], __m256i filter[8], uint16_t* optr, __m256i max) {
    const int     steps = 8;
    const __m256i min   = _mm256_set1_epi32(0);

    __m256i sum[8];
    for (int i = 0; i < steps; ++i) {
        sum[i] = _mm256_mullo_epi32(src[i], filter[i]);
    }

    __m256i vec04 = _mm256_hadd_epi32(sum[0], sum[4]);
    __m256i vec15 = _mm256_hadd_epi32(sum[1], sum[5]);
    __m256i vec26 = _mm256_hadd_epi32(sum[2], sum[6]);
    __m256i vec37 = _mm256_hadd_epi32(sum[3], sum[7]);
    // P00ab P00cd P00ef P00gh P04ab P04cd P04ef P04gh
    vec04 = _mm256_permute4x64_epi64(vec04, _MM_SHUFFLE(3, 1, 2, 0));
    // P01ab P01cd P01ef P01gh P05ab P05cd P05ef P05gh
    vec15 = _mm256_permute4x64_epi64(vec15, _MM_SHUFFLE(3, 1, 2, 0));
    vec26 = _mm256_permute4x64_epi64(vec26, _MM_SHUFFLE(3, 1, 2, 0));
    vec37 = _mm256_permute4x64_epi64(vec37, _MM_SHUFFLE(3, 1, 2, 0));

    // P00abcd P00efgh P01abcd P01efgh P04abcd P04efgh P05abcd P05efgh
    const __m256i vec0145 = _mm256_hadd_epi32(vec04, vec15);
    const __m256i vec2367 = _mm256_hadd_epi32(vec26, vec37);

    __m256i vec8 = _mm256_hadd_epi32(vec0145, vec2367);
    vec8         = RightShiftWithRounding_S32(vec8, FILTER_BITS);
    mm256_clamp_epi32(&vec8, min, max);

    const __m128i lo_lane   = _mm256_castsi256_si128(vec8);
    const __m128i hi_lane   = _mm256_extracti128_si256(vec8, 1);
    const __m128i m128_sum8 = _mm_packus_epi32(lo_lane, hi_lane); // 8x 16-bit

    _mm_storeu_si128((__m128i*)optr, m128_sum8);
}

static INLINE void highbd_interpolate_core_w8_mid_part_avx2(const uint16_t* const input, uint16_t** output,
                                                            const int16_t* interp_filters, int* py, const int delta,
                                                            int length, __m256i max) {
    const int steps = 8;
    uint16_t* optr  = *output;
    int       y     = *py;

    __m256i filter[8];
    __m256i src[8];
    for (int i = 0; i < length; i += steps) {
        for (int j = 0; j < steps; ++j) {
            int             int_pel  = y >> RS_SCALE_SUBPEL_BITS;
            int             sub_pel  = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK; // 0~63
            const int16_t*  filter_c = &interp_filters[sub_pel * SUBPEL_TAPS];
            const uint16_t* in_ptr   = &input[int_pel - SUBPEL_TAPS / 2 + 1];
            __m128i         src_128  = _mm_lddqu_si128((const __m128i*)in_ptr);
            src[j]                   = _mm256_cvtepu16_epi32(src_128);
            __m128i filter_128       = _mm_lddqu_si128((const __m128i*)filter_c);
            filter[j]                = _mm256_cvtepi16_epi32(filter_128);
            y += delta;
        }

        highbd_interpolate_core_w8_avx2(src, filter, optr, max);
        optr += steps;
    }
    *output = optr;
    *py     = y;
}

static INLINE void mm256_blend_load_lo(const uint16_t* const input, int blend_len, __m256i* dst) {
    uint16_t tmp[8];
    for (int i = 0; i < blend_len; ++i) {
        tmp[i] = input[0];
    }
    memcpy(tmp + blend_len, input, (8 - blend_len) * sizeof(uint16_t));

    __m128i src_128 = _mm_lddqu_si128((__m128i*)tmp);
    *dst            = _mm256_cvtepu16_epi32(src_128);
    return;
}

static INLINE void mm256_blend_load_hi(const uint16_t* const input, int blend_len, __m256i* dst) {
    uint16_t tmp[8];
    memcpy(tmp, input, (8 - blend_len) * sizeof(uint16_t));
    for (int i = 0; i < blend_len; ++i) {
        tmp[8 - blend_len + i] = input[8 - blend_len - 1];
    }

    __m128i src_128 = _mm_lddqu_si128((__m128i*)tmp);
    *dst            = _mm256_cvtepu16_epi32(src_128);
    return;
}

static INLINE void highbd_interpolate_core_w8_init_part_avx2(const uint16_t* const input, uint16_t** output,
                                                             const int16_t* interp_filters, int* py, const int delta,
                                                             __m256i max) {
    const int steps = 8;
    uint16_t* optr  = *output;
    int       y     = *py;

    __m256i filter[8];
    __m256i src[8];

    for (int j = 0; j < steps; ++j) {
        int            int_pel   = y >> RS_SCALE_SUBPEL_BITS;
        int            sub_pel   = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK; // 0~63
        const int16_t* filter_c  = &interp_filters[sub_pel * SUBPEL_TAPS];
        int            in_offset = int_pel - SUBPEL_TAPS / 2 + 1;
        if (in_offset < 0) {
            mm256_blend_load_lo(input, SUBPEL_TAPS / 2 - int_pel - 1, &src[j]);
        } else {
            const uint16_t* in_ptr  = &input[in_offset];
            __m128i         src_128 = _mm_lddqu_si128((const __m128i*)in_ptr);
            src[j]                  = _mm256_cvtepu16_epi32(src_128);
        }
        __m128i filter_128 = _mm_lddqu_si128((const __m128i*)filter_c);
        filter[j]          = _mm256_cvtepi16_epi32(filter_128);
        y += delta;
    }

    highbd_interpolate_core_w8_avx2(src, filter, optr, max);
    optr += steps;

    *output = optr;
    *py     = y;
}

static INLINE void highbd_interpolate_core_w8_end_part_avx2(const uint16_t* const input, int in_length,
                                                            uint16_t** output, const int16_t* interp_filters, int* py,
                                                            const int delta, __m256i max) {
    const int steps = 8;
    uint16_t* optr  = *output;
    int       y     = *py;

    __m256i filter[8];
    __m256i src[8];

    for (int j = 0; j < steps; ++j) {
        int            int_pel   = y >> RS_SCALE_SUBPEL_BITS;
        int            sub_pel   = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK; // 0~63
        const int16_t* filter_c  = &interp_filters[sub_pel * SUBPEL_TAPS];
        int            in_offset = int_pel - SUBPEL_TAPS / 2 + 1;
        if (int_pel - SUBPEL_TAPS / 2 + 1 + 8 > in_length) {
            int blend_length = int_pel - SUBPEL_TAPS / 2 + 1 + 8 - in_length;
            mm256_blend_load_hi(&input[int_pel - SUBPEL_TAPS / 2 + 1], blend_length, &src[j]);
        } else {
            const uint16_t* in_ptr  = &input[in_offset];
            __m128i         src_128 = _mm_lddqu_si128((const __m128i*)in_ptr);
            src[j]                  = _mm256_cvtepu16_epi32(src_128);
        }
        __m128i filter_128 = _mm_lddqu_si128((const __m128i*)filter_c);
        filter[j]          = _mm256_cvtepi16_epi32(filter_128);
        y += delta;
    }

    highbd_interpolate_core_w8_avx2(src, filter, optr, max);
    optr += steps;

    *output = optr;
    *py     = y;
}

static INLINE void interpolate_core_w16_mid_part_avx2(const uint8_t* const input, uint8_t** output,
                                                      const int16_t* interp_filters, int* py, const int delta,
                                                      int length) {
    uint8_t* optr = *output;
    int      y    = *py;

    __m128i short_filter[16];
    __m128i short_src[16];
    for (int i = 0; i < length; i += 16) {
        for (int j = 0; j < 8; ++j) {
            int            int_pel = y >> RS_SCALE_SUBPEL_BITS;
            int            sub_pel = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK; // 0~63
            const int16_t* filter  = &interp_filters[sub_pel * SUBPEL_TAPS];
            const uint8_t* in_ptr  = &input[int_pel - SUBPEL_TAPS / 2 + 1];
            short_src[2 * j]       = _mm_loadl_epi64((const __m128i*)in_ptr);
            short_src[2 * j]       = _mm_cvtepu8_epi16(short_src[2 * j]);
            short_filter[2 * j]    = _mm_loadu_si128((const __m128i*)filter);
            y += delta;

            int_pel                 = y >> RS_SCALE_SUBPEL_BITS;
            sub_pel                 = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK; // 0~63
            filter                  = &interp_filters[sub_pel * SUBPEL_TAPS];
            in_ptr                  = &input[int_pel - SUBPEL_TAPS / 2 + 1];
            short_src[2 * j + 1]    = _mm_loadl_epi64((const __m128i*)in_ptr);
            short_src[2 * j + 1]    = _mm_cvtepu8_epi16(short_src[2 * j + 1]);
            short_filter[2 * j + 1] = _mm_loadu_si128((const __m128i*)filter);
            y += delta;
        }

        interpolate_core_w16_avx2(short_src, short_filter, optr);
        optr += 16;
    }
    *output = optr;
    *py     = y;
}

static INLINE void interpolate_core_w16_init_part_avx2(const uint8_t* const input, uint8_t** output,
                                                       const int16_t* interp_filters, int* py, const int delta) {
    uint8_t* optr = *output;
    int      y    = *py;

    __m128i short_filter[16];
    __m128i short_src[16];

    for (int j = 0; j < 8; ++j) {
        int            int_pel = y >> RS_SCALE_SUBPEL_BITS;
        int            sub_pel = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK; // 0~63
        const int16_t* filter  = &interp_filters[sub_pel * SUBPEL_TAPS];
        if (int_pel - SUBPEL_TAPS / 2 + 1 < 0) {
            mm_blend_load_lo(input, SUBPEL_TAPS / 2 - int_pel - 1, &short_src[2 * j]);
        } else {
            const uint8_t* in_ptr = &input[int_pel - SUBPEL_TAPS / 2 + 1];
            short_src[2 * j]      = _mm_loadl_epi64((const __m128i*)in_ptr);
            short_src[2 * j]      = _mm_cvtepu8_epi16(short_src[2 * j]);
        }
        short_filter[2 * j] = _mm_loadu_si128((const __m128i*)filter);
        y += delta;

        int_pel = y >> RS_SCALE_SUBPEL_BITS;
        sub_pel = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK; // 0~63
        filter  = &interp_filters[sub_pel * SUBPEL_TAPS];
        if (int_pel - SUBPEL_TAPS / 2 + 1 < 0) {
            mm_blend_load_lo(input, SUBPEL_TAPS / 2 - int_pel - 1, &short_src[2 * j + 1]);
        } else {
            const uint8_t* in_ptr = &input[int_pel - SUBPEL_TAPS / 2 + 1];
            short_src[2 * j + 1]  = _mm_loadl_epi64((const __m128i*)in_ptr);
            short_src[2 * j + 1]  = _mm_cvtepu8_epi16(short_src[2 * j + 1]);
        }
        short_filter[2 * j + 1] = _mm_loadu_si128((const __m128i*)filter);
        y += delta;
    }

    interpolate_core_w16_avx2(short_src, short_filter, optr);
    optr += 16;

    *output = optr;
    *py     = y;
}

static INLINE void interpolate_core_w16_end_part_avx2(const uint8_t* const input, int in_length, uint8_t** output,
                                                      const int16_t* interp_filters, int* py, const int delta) {
    uint8_t* optr = *output;
    int      y    = *py;

    __m128i short_filter[16];
    __m128i short_src[16];

    for (int j = 0; j < 8; ++j) {
        int            int_pel = y >> RS_SCALE_SUBPEL_BITS;
        int            sub_pel = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK; // 0~63
        const int16_t* filter  = &interp_filters[sub_pel * SUBPEL_TAPS];
        if (int_pel - SUBPEL_TAPS / 2 + 1 + 8 > in_length) {
            int blend_length = int_pel - SUBPEL_TAPS / 2 + 1 + 8 - in_length;
            mm_blend_load_hi(&input[int_pel - SUBPEL_TAPS / 2 + 1], blend_length, &short_src[2 * j]);
        } else {
            const uint8_t* in_ptr = &input[int_pel - SUBPEL_TAPS / 2 + 1];
            short_src[2 * j]      = _mm_loadl_epi64((const __m128i*)in_ptr);
            short_src[2 * j]      = _mm_cvtepu8_epi16(short_src[2 * j]);
        }
        short_filter[2 * j] = _mm_loadu_si128((const __m128i*)filter);
        y += delta;

        int_pel = y >> RS_SCALE_SUBPEL_BITS;
        sub_pel = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK; // 0~63
        filter  = &interp_filters[sub_pel * SUBPEL_TAPS];
        if (int_pel - SUBPEL_TAPS / 2 + 1 + 8 > in_length) {
            int blend_length = int_pel - SUBPEL_TAPS / 2 + 1 + 8 - in_length;
            mm_blend_load_hi(&input[int_pel - SUBPEL_TAPS / 2 + 1], blend_length, &short_src[2 * j + 1]);
        } else {
            const uint8_t* in_ptr = &input[int_pel - SUBPEL_TAPS / 2 + 1];
            short_src[2 * j + 1]  = _mm_loadl_epi64((const __m128i*)in_ptr);
            short_src[2 * j + 1]  = _mm_cvtepu8_epi16(short_src[2 * j + 1]);
        }
        short_filter[2 * j + 1] = _mm_loadu_si128((const __m128i*)filter);
        y += delta;
    }

    interpolate_core_w16_avx2(short_src, short_filter, optr);
    optr += 16;

    *output = optr;
    *py     = y;
}

static INLINE __m256i mm256_load2_m128i(__m128i hi, __m128i lo) {
    __m256i c = _mm256_castsi128_si256(lo);
    return _mm256_inserti128_si256(c, hi, 1);
}

static INLINE void down2_symeven_prepare_4vec(const __m128i* src, __m256i* dst) {
    __m128i tmp = _mm_alignr_epi8(src[1], src[0], 4);
    dst[0]      = mm256_load2_m128i(tmp, src[0]);
    dst[1]      = mm256_load2_m128i(_mm_alignr_epi8(src[1], src[0], 12), _mm_alignr_epi8(src[1], src[0], 8));
    dst[2]      = mm256_load2_m128i(_mm_alignr_epi8(src[2], src[1], 4), src[1]);
    dst[3]      = mm256_load2_m128i(_mm_alignr_epi8(src[2], src[1], 12), _mm_alignr_epi8(src[2], src[1], 8));
}

static INLINE __m256i down2_get_8sum(__m256i* vec, const __m256i* filter_4x, const __m256i* subtrahend) {
    vec[0]       = _mm256_shufflelo_epi16(vec[0], _MM_SHUFFLE(0, 1, 2, 3));
    vec[0]       = _mm256_shufflehi_epi16(vec[0], _MM_SHUFFLE(0, 1, 2, 3));
    __m256i sum0 = _mm256_add_epi16(vec[0], vec[1]);
    // sum0: P00a P00b P00c P00d P02a P02b P02c P02d   P01a P01b P01c P01d P03a P03b P03c P03d
    sum0 = _mm256_mullo_epi16(sum0, *filter_4x);
    sum0 = _mm256_subs_epi16(sum0, *subtrahend); // to avoid exceeding 16-bit after P00a+P00b
    // sum0: P00a P00b P00c P00d P01a P01b P01c P01d   P02a P02b P02c P02d P03a P03b P03c P03d
    sum0 = _mm256_permute4x64_epi64(sum0, _MM_SHUFFLE(3, 1, 2, 0));

    vec[2]       = _mm256_shufflelo_epi16(vec[2], _MM_SHUFFLE(0, 1, 2, 3));
    vec[2]       = _mm256_shufflehi_epi16(vec[2], _MM_SHUFFLE(0, 1, 2, 3));
    __m256i sum1 = _mm256_add_epi16(vec[2], vec[3]);
    // sum1: P04a P04b P04c P04d P06a P06b P06c P06d   P05a P05b P05c P05d P07a P07b P07c P07d
    sum1 = _mm256_mullo_epi16(sum1, *filter_4x);
    sum1 = _mm256_subs_epi16(sum1, *subtrahend); // to avoid exceeding 16-bit after P00a+P00b
    // sum1: P04a P04b P04c P04d P05a P05b P05c P05d   P06a P06b P06c P06d P07a P07b P07c P07d
    sum1 = _mm256_permute4x64_epi64(sum1, _MM_SHUFFLE(3, 1, 2, 0));

    // P00ab P00cd P01ab P01cd P04ab P04cd P05ab P05cd  P02ab P02cd P03ab P03cd P06ab P06cd P07ab P07cd
    __m256i sum01 = _mm256_hadd_epi16(sum0, sum1);
    // P00ab P00cd P01ab P01cd P02ab P02cd P03ab P03cd  P04ab P04cd P05ab P05cd P06ab P06cd P07ab P07cd
    sum01 = _mm256_permute4x64_epi64(sum01, _MM_SHUFFLE(3, 1, 2, 0));
    return sum01;
}

static INLINE void down2_symeven_w16_avx2(const __m128i* load, const __m256i* filter_4x, uint8_t* optr) {
    const __m256i min         = _mm256_set1_epi16(0);
    const __m256i max         = _mm256_set1_epi16(255);
    const __m256i base_sum    = _mm256_set1_epi16(64);
    const __m256i subtrahend  = _mm256_set1_epi16(1 << 11);
    const __m256i recover_sub = _mm256_set1_epi16(1 << (13 - FILTER_BITS));

    __m256i vec[8];
    down2_symeven_prepare_4vec(load, vec);
    down2_symeven_prepare_4vec(load + 2, vec + 4);

    // obtain: P00ab P00cd P01ab P01cd P02ab P02cd P03ab P03cd  P04ab P04cd P05ab P05cd P06ab P06cd P07ab P07cd
    __m256i sum01 = down2_get_8sum(&vec[0], filter_4x, &subtrahend);
    __m256i sum23 = down2_get_8sum(&vec[4], filter_4x, &subtrahend);

    // P00 P01 P02 P03 P08 P09 P10 P11  P04 P05 P06 P07 P12 P13 P14 P15
    __m256i sum0123 = _mm256_hadd_epi16(sum01, sum23);
    // P00 P01 P02 P03 P04 P05 P06 P07  P08 P09 P10 P11 P12 P13 P14 P15
    sum0123 = _mm256_permute4x64_epi64(sum0123, _MM_SHUFFLE(3, 1, 2, 0));
    sum0123 = _mm256_adds_epi16(sum0123, base_sum);

    sum0123 = _mm256_srai_epi16(sum0123, FILTER_BITS);
    sum0123 = _mm256_adds_epi16(sum0123, recover_sub); // recover subtracted 2^13/2^7
    mm256_clamp_epi16(&sum0123, min, max);

    const __m128i lo_lane    = _mm256_castsi256_si128(sum0123);
    const __m128i hi_lane    = _mm256_extracti128_si256(sum0123, 1);
    __m128i       m128_0to15 = _mm_packus_epi16(lo_lane, hi_lane);
    _mm_storeu_si128((__m128i*)optr, m128_0to15);
}

static INLINE void down2_symeven_w16_mid_part_avx2(const uint8_t* const input, int length, uint8_t** output,
                                                   const __m256i* filter_4x) {
    assert((length % 32) == 0);
    const uint8_t* in   = input - 3;
    uint8_t*       optr = *output;
    __m128i        load[5];

    int i   = 0;
    load[4] = _mm_loadl_epi64((const __m128i*)in);
    load[4] = _mm_cvtepu8_epi16(load[4]);
    do {
        load[0] = load[4];
        for (int n = 1; n < 5; ++n) {
            load[n] = _mm_loadl_epi64((const __m128i*)(in + n * 8));
            load[n] = _mm_cvtepu8_epi16(load[n]);
        }

        down2_symeven_w16_avx2(load, filter_4x, optr);

        optr += 16;
        i += 32;
        in += 32;
    } while (i < length);
    *output = optr;
}

static INLINE void down2_symeven_w16_init_part_avx2(const uint8_t* const input, uint8_t** output,
                                                    const __m256i* filter_4x) {
    const uint8_t* in   = input - 3;
    uint8_t*       optr = *output;
    __m128i        load[5];
    mm_blend_load_lo(input, 3, &load[0]);
    for (int n = 1; n < 5; ++n) {
        load[n] = _mm_loadl_epi64((const __m128i*)(in + n * 8));
        load[n] = _mm_cvtepu8_epi16(load[n]);
    }

    down2_symeven_w16_avx2(load, filter_4x, optr);
    optr += 16;

    *output = optr;
}

static INLINE void down2_symeven_w16_end_part_avx2(const uint8_t* const input, uint8_t** output,
                                                   const __m256i* filter_4x, int len_to_end) {
    const uint8_t* in   = input - 3;
    const uint8_t* end  = input + len_to_end;
    uint8_t*       optr = *output;
    __m128i        load[5];

    for (int n = 0; n < 4; ++n) {
        load[n] = _mm_loadl_epi64((const __m128i*)(in + n * 8));
        load[n] = _mm_cvtepu8_epi16(load[n]);
    }

    mm_blend_load_hi(in + 32, (int)(in + 32 + 8 - end), &load[4]);

    down2_symeven_w16_avx2(load, filter_4x, optr);
    optr += 16;

    *output = optr;
}

static INLINE void highbd_down2_symeven_prepare_4pt(const __m128i load_0, const __m128i load_1, __m256i filter_2x,
                                                    __m256i* vec_4pt) {
    // -3 -2 -1 00 01 02 03 04
    __m256i vec256_tmp_0 = _mm256_cvtepu16_epi32(load_0);
    // -1 00 01 02 03 04 05 06
    __m128i vec128_tmp_1 = _mm_alignr_epi8(load_1, load_0, 4); // cat and shift right 2x 16-bit (4 bytes)
    __m256i vec256_tmp_1 = _mm256_cvtepu16_epi32(vec128_tmp_1);
    // -3 -2 -1 00 -1 00 01 02
    __m256i vec0 = yy_unpacklo_epi128(vec256_tmp_0, vec256_tmp_1);
    // 00 -1 -2 -3 02 01 00 -1
    vec0 = _mm256_shuffle_epi32(vec0, _MM_SHUFFLE(0, 1, 2, 3));

    // 01 02 03 04 03 04 05 06
    __m256i vec1 = yy_unpackhi_epi128(vec256_tmp_0, vec256_tmp_1);

    __m256i p01 = _mm256_add_epi32(vec0, vec1);
    // P00a P00b P00c P00d P01a P01b P01c P01d
    p01 = _mm256_mullo_epi32(p01, filter_2x);

    // 04 03 02 01 06 05 04 03
    vec0 = _mm256_shuffle_epi32(vec1, _MM_SHUFFLE(0, 1, 2, 3));
    // 05 06 07 08 09 10 11 12
    vec256_tmp_0 = _mm256_cvtepu16_epi32(load_1);
    // 07 08 09 10 11 12 -3 -2
    vec128_tmp_1 = _mm_alignr_epi8(load_0, load_1, 4);
    vec256_tmp_1 = _mm256_cvtepu16_epi32(vec128_tmp_1);
    // 05 06 07 08 07 08 09 10
    vec1 = yy_unpacklo_epi128(vec256_tmp_0, vec256_tmp_1);

    __m256i p23 = _mm256_add_epi32(vec0, vec1);
    // P02a P02b P02c P02d P03a P03b P03c P03d
    p23 = _mm256_mullo_epi32(p23, filter_2x);

    // P00ab P00cd P02ab P02cd P01ab P01cd P03ab P03cd
    *vec_4pt = _mm256_hadd_epi32(p01, p23);
    // P00ab P00cd P01ab P01cd P02ab P02cd P03ab P03cd
    *vec_4pt = _mm256_permute4x64_epi64(*vec_4pt, _MM_SHUFFLE(3, 1, 2, 0));
}

static INLINE void highbd_down2_symeven_prepare_2pt(const __m256i a, const __m256i b, __m256i filter_2x,
                                                    __m256i* vec_2pt) {
    // a: -3 -2 -1 00 01 02 03 04
    // b: -1 00 01 02 03 04 05 06

    // -3 -2 -1 00 -1 00 01 02
    __m256i vec0 = yy_unpacklo_epi128(a, b);
    // 00 -1 -2 -3 02 01 00 -1
    vec0 = _mm256_shuffle_epi32(vec0, _MM_SHUFFLE(0, 1, 2, 3));

    // 01 02 03 04 03 04 05 06
    __m256i vec1 = yy_unpackhi_epi128(a, b);

    *vec_2pt = _mm256_add_epi32(vec0, vec1);
    // P00a P00b P00c P00d P01a P01b P01c P01d
    *vec_2pt = _mm256_mullo_epi32(*vec_2pt, filter_2x);
}

static INLINE void highbd_down2_symeven_w8_mid_part_avx2(const uint16_t* const input, int length, uint16_t** output,
                                                         __m256i filter_2x, __m256i max) {
    const int     steps    = 8; // output 8 pt by each loop
    const __m256i min      = _mm256_set1_epi32(0);
    const __m256i base_sum = _mm256_set1_epi32(64);

    const uint16_t* in   = input - 3;
    uint16_t*       optr = *output;
    int             i    = 0;

    __m128i load[2];
    // -3 -2 -1 00 01 02 03 04
    load[1] = _mm_lddqu_si128((__m128i*)in);

    do {
        load[0] = load[1];
        // 05 06 07 08 09 10 11 12
        load[1] = _mm_lddqu_si128((const __m128i*)(in + 8));

        {
            __m256i vec0123, vec4567;
            highbd_down2_symeven_prepare_4pt(load[0], load[1], filter_2x, &vec0123);

            load[0] = load[1];
            load[1] = _mm_lddqu_si128((const __m128i*)(in + 16));
            highbd_down2_symeven_prepare_4pt(load[0], load[1], filter_2x, &vec4567);

            // P00ab P00cd P01ab P01cd P02ab P02cd P03ab P03cd
            // P04ab P04cd P05ab P05cd P06ab P06cd P07ab P07cd

            // P00 P01 P04 P05 P02 P03 P06 P07
            __m256i vec_8pt = _mm256_hadd_epi32(vec0123, vec4567);
            vec_8pt         = _mm256_permute4x64_epi64(vec_8pt, _MM_SHUFFLE(3, 1, 2, 0));

            vec_8pt = _mm256_add_epi32(vec_8pt, base_sum);
            vec_8pt = _mm256_srai_epi32(vec_8pt, FILTER_BITS);
            mm256_clamp_epi32(&vec_8pt, min, max);

            // 8x 32-bit => 8x 16-bit
            __m128i lo_lane   = _mm256_castsi256_si128(vec_8pt);
            __m128i hi_lane   = _mm256_extracti128_si256(vec_8pt, 1);
            __m128i m128_sum8 = _mm_packus_epi32(lo_lane, hi_lane); // 8x 16-bit

            _mm_storeu_si128((__m128i*)optr, m128_sum8);
        }

        optr += steps;
        i += steps * 2;
        in += steps * 2;
    } while (i < length);

    *output = optr;
}

static INLINE void highbd_down2_symeven_w8_init_part_avx2(const uint16_t* const input, uint16_t** output,
                                                          __m256i filter_2x, __m256i max) {
    const int     steps    = 8;
    const __m256i min      = _mm256_set1_epi32(0);
    const __m256i base_sum = _mm256_set1_epi32(64);

    const uint16_t* in   = input - 3;
    uint16_t*       optr = *output;

    __m128i load[2];
    {
        int      blend_len = 3;
        uint16_t tmp[8];
        for (int i = 0; i < blend_len; ++i) {
            tmp[i] = input[0];
        }
        memcpy(tmp + blend_len, input, (8 - blend_len) * sizeof(uint16_t));

        // -3 -2 -1 00 01 02 03 04
        load[0] = _mm_lddqu_si128((__m128i*)tmp);
    }
    // 05 06 07 08 09 10 11 12
    load[1] = _mm_lddqu_si128((const __m128i*)(in + 8));

    __m256i vec0123, vec4567;
    highbd_down2_symeven_prepare_4pt(load[0], load[1], filter_2x, &vec0123);

    load[0] = load[1];
    load[1] = _mm_lddqu_si128((const __m128i*)(in + 16));
    highbd_down2_symeven_prepare_4pt(load[0], load[1], filter_2x, &vec4567);

    // P00ab P00cd P01ab P01cd P02ab P02cd P03ab P03cd
    // P04ab P04cd P05ab P05cd P06ab P06cd P07ab P07cd

    // P00 P01 P04 P05 P02 P03 P06 P07
    __m256i vec_8pt = _mm256_hadd_epi32(vec0123, vec4567);
    vec_8pt         = _mm256_permute4x64_epi64(vec_8pt, _MM_SHUFFLE(3, 1, 2, 0));

    vec_8pt = _mm256_add_epi32(vec_8pt, base_sum);
    vec_8pt = _mm256_srai_epi32(vec_8pt, FILTER_BITS);
    mm256_clamp_epi32(&vec_8pt, min, max);

    // 8x 32-bit => 8x 16-bit
    __m128i lo_lane   = _mm256_castsi256_si128(vec_8pt);
    __m128i hi_lane   = _mm256_extracti128_si256(vec_8pt, 1);
    __m128i m128_sum8 = _mm_packus_epi32(lo_lane, hi_lane); // 8x 16-bit

    _mm_storeu_si128((__m128i*)optr, m128_sum8);
    optr += steps;

    *output = optr;
}

static INLINE void highbd_down2_symeven_w8_end_part_avx2(const uint16_t* const input, uint16_t** output,
                                                         const __m256i filter_2x, int len_to_end, __m256i max) {
    const int     steps    = 8;
    const __m256i min      = _mm256_set1_epi32(0);
    const __m256i base_sum = _mm256_set1_epi32(64);

    const uint16_t* in   = input - 3;
    const uint16_t* end  = input + len_to_end;
    uint16_t*       optr = *output;

    __m128i load[2];

    load[0] = _mm_lddqu_si128((const __m128i*)in);
    // first 4 points of the 8 points to output will not access padding source
    assert(in + 16 < end);
    load[1] = _mm_lddqu_si128((const __m128i*)(in + 8));

    {
        __m256i vec0123, vec4567;
        highbd_down2_symeven_prepare_4pt(load[0], load[1], filter_2x, &vec0123);

        load[0] = load[1];
        if (in + 24 > end) {
            int      blend_len = (int)(in + 24 - end);
            uint16_t tmp[8];
            memcpy(tmp, in + 16, (8 - blend_len) * sizeof(uint16_t));
            for (int i = 0; i < blend_len; ++i) {
                tmp[8 - blend_len + i] = *(end - 1);
            }

            load[1] = _mm_lddqu_si128((__m128i*)tmp);
        } else {
            load[1] = _mm_lddqu_si128((const __m128i*)(in + 16));
        }
        highbd_down2_symeven_prepare_4pt(load[0], load[1], filter_2x, &vec4567);

        // P00ab P00cd P01ab P01cd P02ab P02cd P03ab P03cd
        // P04ab P04cd P05ab P05cd P06ab P06cd P07ab P07cd

        // P00 P01 P04 P05 P02 P03 P06 P07
        __m256i vec_8pt = _mm256_hadd_epi32(vec0123, vec4567);
        vec_8pt         = _mm256_permute4x64_epi64(vec_8pt, _MM_SHUFFLE(3, 1, 2, 0));

        vec_8pt = _mm256_add_epi32(vec_8pt, base_sum);
        vec_8pt = _mm256_srai_epi32(vec_8pt, FILTER_BITS);
        mm256_clamp_epi32(&vec_8pt, min, max);

        // 8x 32-bit => 8x 16-bit
        __m128i lo_lane   = _mm256_castsi256_si128(vec_8pt);
        __m128i hi_lane   = _mm256_extracti128_si256(vec_8pt, 1);
        __m128i m128_sum8 = _mm_packus_epi32(lo_lane, hi_lane); // 8x 16-bit

        _mm_storeu_si128((__m128i*)optr, m128_sum8);
    }
    optr += steps;

    *output = optr;
}

void svt_av1_interpolate_core_avx2(const uint8_t* const input, int in_length, uint8_t* output, int out_length,
                                   const int16_t* interp_filters) {
    const int32_t steps  = 16;
    const int32_t delta  = (((uint32_t)in_length << RS_SCALE_SUBPEL_BITS) + out_length / 2) / out_length;
    const int32_t offset = in_length > out_length
        ? (((int32_t)(in_length - out_length) << (RS_SCALE_SUBPEL_BITS - 1)) + out_length / 2) / out_length
        : -(((int32_t)(out_length - in_length) << (RS_SCALE_SUBPEL_BITS - 1)) + out_length / 2) / out_length;
    uint8_t*      optr   = output;
    int32_t       x, x1, x2;
    int32_t       y;

    x = 0;
    y = offset + RS_SCALE_EXTRA_OFF;
    while ((y >> RS_SCALE_SUBPEL_BITS) < (SUBPEL_TAPS / 2 - 1)) { // (y >> 14) < 3 ?
        x++;
        y += delta;
    }
    x1 = x;
    x  = out_length - 1;
    y  = delta * x + offset + RS_SCALE_EXTRA_OFF;
    while ((y >> RS_SCALE_SUBPEL_BITS) + (int32_t)(SUBPEL_TAPS / 2) >= in_length) {
        x--;
        y -= delta;
    }
    x2 = x;
    assert(x1 <= x2);
    (void)x1;

    // Initial part.
    x = 0;
    y = offset + RS_SCALE_EXTRA_OFF;
    interpolate_core_w16_init_part_avx2(input, &optr, interp_filters, &y, delta);
    x += steps;

    // Middle part.
    int start = x;
    int mid   = (x2 - start + 1) & (~(steps - 1));
    interpolate_core_w16_mid_part_avx2(input, &optr, interp_filters, &y, delta, mid);
    x += mid;

    // Middle part + End part.
    if (out_length - x >= steps) {
        interpolate_core_w16_end_part_avx2(input, in_length, &optr, interp_filters, &y, delta);
        x += steps;
    }

    // End part.
    for (; x < out_length; ++x, y += delta) {
        int32_t        int_pel = y >> RS_SCALE_SUBPEL_BITS;
        int32_t        sub_pel = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
        const int16_t* filter  = &interp_filters[sub_pel * SUBPEL_TAPS];
        int32_t        sum     = 0;
        for (int32_t k = 0; k < SUBPEL_TAPS; ++k) {
            sum += filter[k] *
                input[AOMMIN(int_pel - SUBPEL_TAPS / 2 + 1 + k,
                             in_length - 1)]; // clamp input offset to (any, in_length - 1)
        }
        *optr++ = clip_pixel(ROUND_POWER_OF_TWO(sum, FILTER_BITS));
    }
}

void svt_av1_down2_symeven_avx2(const uint8_t* const input, int length, uint8_t* output) {
    const int16_t* filter          = svt_aom_av1_down2_symeven_half_filter;
    const int      filter_len_half = sizeof(svt_aom_av1_down2_symeven_half_filter) / 2;
    const int      steps           = 32;

    const __m128i filter_1x = _mm_loadl_epi64((const __m128i*)filter);
    const __m256i filter_4x = _mm256_broadcastq_epi64(filter_1x);

    int       i, j;
    uint8_t*  optr = output;
    const int l1   = steps;
    int       l2   = (length - filter_len_half);
    l2 += (l2 & 1);

    // Initial part.
    down2_symeven_w16_init_part_avx2(input, &optr, &filter_4x);
    i = l1;

    // Middle part.
    int mid = (l2 - l1 + 1) & (~(steps - 1));
    if (mid > 0) {
        down2_symeven_w16_mid_part_avx2(input + i, mid, &optr, &filter_4x);
        i += mid;
    }

    // Middle part + End part.
    if (length - i >= steps) {
        int len_to_end = length - i;
        down2_symeven_w16_end_part_avx2(input + i, &optr, &filter_4x, len_to_end);
        i += steps;
    }

    // End part.
    for (; i < length; i += 2) {
        int sum = (1 << (FILTER_BITS - 1));
        for (j = 0; j < filter_len_half; ++j) {
            sum += (input[i - j] + input[AOMMIN(i + 1 + j, length - 1)]) * filter[j];
        }
        sum >>= FILTER_BITS;
        *optr++ = clip_pixel(sum);
    }
}

void svt_av1_highbd_interpolate_core_avx2(const uint16_t* const input, int in_length, uint16_t* output, int out_length,
                                          int bd, const int16_t* interp_filters) {
    const int32_t steps = 8;
    __m256i       max;
    if (bd == 10) {
        max = _mm256_set1_epi32(1023);
    } else {
        max = _mm256_set1_epi32(4095);
    }
    const int32_t delta  = (((uint32_t)in_length << RS_SCALE_SUBPEL_BITS) + out_length / 2) / out_length;
    const int32_t offset = in_length > out_length
        ? (((int32_t)(in_length - out_length) << (RS_SCALE_SUBPEL_BITS - 1)) + out_length / 2) / out_length
        : -(((int32_t)(out_length - in_length) << (RS_SCALE_SUBPEL_BITS - 1)) + out_length / 2) / out_length;
    uint16_t*     optr   = output;
    int32_t       x, x1, x2;
    int32_t       y;

    x = 0;
    y = offset + RS_SCALE_EXTRA_OFF;
    while ((y >> RS_SCALE_SUBPEL_BITS) < (SUBPEL_TAPS / 2 - 1)) { // (y >> 14) < 3 ?
        x++;
        y += delta;
    }
    x1 = x;
    x  = out_length - 1;
    y  = delta * x + offset + RS_SCALE_EXTRA_OFF;
    while ((y >> RS_SCALE_SUBPEL_BITS) + (int32_t)(SUBPEL_TAPS / 2) >= in_length) {
        x--;
        y -= delta;
    }
    x2 = x;
    assert(x1 <= x2);
    (void)x1;

    // Initial part.
    x = 0;
    y = offset + RS_SCALE_EXTRA_OFF;
    highbd_interpolate_core_w8_init_part_avx2(input, &optr, interp_filters, &y, delta, max);
    x += steps;

    // Middle part.
    int start = x;
    int mid2  = (x2 - start + 1) & (~(steps - 1));
    highbd_interpolate_core_w8_mid_part_avx2(input, &optr, interp_filters, &y, delta, mid2, max);
    x += mid2;

    // Middle part + End part.
    if (out_length - x >= steps) {
        highbd_interpolate_core_w8_end_part_avx2(input, in_length, &optr, interp_filters, &y, delta, max);
        x += steps;
    }

    // End part.
    for (; x < out_length; ++x, y += delta) {
        int32_t        int_pel = y >> RS_SCALE_SUBPEL_BITS;
        int32_t        sub_pel = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
        const int16_t* filter  = &interp_filters[sub_pel * SUBPEL_TAPS];
        int32_t        sum     = 0;
        for (int32_t k = 0; k < SUBPEL_TAPS; ++k) {
            sum += filter[k] *
                input[AOMMIN(int_pel - SUBPEL_TAPS / 2 + 1 + k,
                             in_length - 1)]; // clamp input offset to (any, in_length - 1)
        }
        *optr++ = clip_pixel_highbd(ROUND_POWER_OF_TWO(sum, FILTER_BITS), bd);
    }
}

static inline int32_t hsums_epi32(const __m256i v) {
    // v = a b c d e f g h
    // x = e f g h a b c d
    __m256i x = _mm256_permute2f128_si256(v, v, 1);
    // y = a+e b+f c+g d+h e+a f+b g+c h+d
    __m256i y = _mm256_add_epi32(v, x);
    // x = b+f a+e d+h c+g f+b e+a h+d g+c
    x = _mm256_shuffle_epi32(y, _MM_SHUFFLE(2, 3, 0, 1));
    // x = abef abef cdgh cdgh abef abef cdgh cdgh
    x = _mm256_add_epi32(x, y);
    // y = cdgh cdgh abef abef cdgh cdgh abef abef
    y = _mm256_shuffle_epi32(x, _MM_SHUFFLE(1, 0, 3, 2));
    // x = abcdefgh ... abcdefgh
    x = _mm256_add_epi32(x, y);
    return _mm256_extract_epi32(x, 0);
}

static INLINE void highbd_interpolate_gather_load_8x8(const uint16_t* const in, const __m256i vindex, __m256i dst[8]) {
    __m256i load = _mm256_i32gather_epi32((const int*)in, vindex, 2);
    dst[0]       = _mm256_and_si256(load, _mm256_set1_epi32(0xffff)); // col 0
    dst[1]       = _mm256_srli_epi32(load, 16); // col 1

    load   = _mm256_i32gather_epi32((const int*)(in + 2), vindex, 2);
    dst[2] = _mm256_and_si256(load, _mm256_set1_epi32(0xffff)); // col 2
    dst[3] = _mm256_srli_epi32(load, 16); // col 3

    load   = _mm256_i32gather_epi32((const int*)(in + 4), vindex, 2);
    dst[4] = _mm256_and_si256(load, _mm256_set1_epi32(0xffff)); // col 4
    dst[5] = _mm256_srli_epi32(load, 16); // col 5

    load   = _mm256_i32gather_epi32((const int*)(in + 6), vindex, 2);
    dst[6] = _mm256_and_si256(load, _mm256_set1_epi32(0xffff)); // col 6
    dst[7] = _mm256_srli_epi32(load, 16); // col 7
}

static INLINE uint16_t highbd_interpolate_compute_1pt(const uint16_t* in, const int16_t* filter, int bd) {
    const __m256i vec_filter = _mm256_cvtepi16_epi32(_mm_lddqu_si128((const __m128i*)filter));
    const __m256i vec_src    = _mm256_cvtepu16_epi32(_mm_lddqu_si128((const __m128i*)in));
    const __m256i one_pt     = _mm256_mullo_epi32(vec_src, vec_filter);
    const int32_t sum        = hsums_epi32(one_pt);
    return clip_pixel_highbd(ROUND_POWER_OF_TWO(sum, FILTER_BITS), bd);
}

static EbErrorType svt_av1_highbd_interpolate_core_col_avx2(const uint16_t* const input, int in_width, int in_height,
                                                            int in_stride, uint16_t* output, int out_height,
                                                            int out_stride, int bd, const int16_t* interp_filters) {
    const int32_t steps = 8;
    __m256i       max;
    if (bd == 10) {
        max = _mm256_set1_epi32(1023);
    } else {
        max = _mm256_set1_epi32(4095);
    }
    const __m256i vindex_0 = _mm256_set_epi32(
        in_stride * 7, in_stride * 6, in_stride * 5, in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0);
    const __m256i vindex_neg1 = _mm256_set_epi32(
        in_stride * 6, in_stride * 5, in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0, 0);
    const __m256i vindex_neg2 = _mm256_set_epi32(
        in_stride * 5, in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0, 0, 0);
    const __m256i vindex_neg3 = _mm256_set_epi32(in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0, 0, 0, 0);
    const __m256i vindex_neg4 = _mm256_set_epi32(in_stride * 3, in_stride * 2, in_stride, 0, 0, 0, 0, 0);
    const __m256i vindex_pos1 = _mm256_set_epi32(
        in_stride * 6, in_stride * 6, in_stride * 5, in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0);
    const __m256i vindex_pos2 = _mm256_set_epi32(
        in_stride * 5, in_stride * 5, in_stride * 5, in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0);
    const __m256i vindex_pos3 = _mm256_set_epi32(
        in_stride * 4, in_stride * 4, in_stride * 4, in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0);
    const __m256i vindex_pos4 = _mm256_set_epi32(
        in_stride * 3, in_stride * 3, in_stride * 3, in_stride * 3, in_stride * 3, in_stride * 2, in_stride, 0);

    const int32_t delta  = (((uint32_t)in_height << RS_SCALE_SUBPEL_BITS) + out_height / 2) / out_height;
    const int32_t offset = in_height > out_height
        ? (((int32_t)(in_height - out_height) << (RS_SCALE_SUBPEL_BITS - 1)) + out_height / 2) / out_height
        : -(((int32_t)(out_height - in_height) << (RS_SCALE_SUBPEL_BITS - 1)) + out_height / 2) / out_height;
    int32_t       x, x1, x2;
    int32_t       y;

    x = 0;
    y = offset + RS_SCALE_EXTRA_OFF;
    while ((y >> RS_SCALE_SUBPEL_BITS) < (SUBPEL_TAPS / 2 - 1)) { // (y >> 14) < 3 ?
        x++;
        y += delta;
    }
    x1 = x;
    x  = out_height - 1;
    y  = delta * x + offset + RS_SCALE_EXTRA_OFF;
    while ((y >> RS_SCALE_SUBPEL_BITS) + (int32_t)(SUBPEL_TAPS / 2) >= in_height) {
        x--;
        y -= delta;
    }
    x2 = x;
    assert(x1 <= x2);

    x = 0;
    y = offset + RS_SCALE_EXTRA_OFF;

    __m256i vec_src[8];
    __m256i vec_filter[8];

    // Initial part: write first x1 rows to output
    __m256i vindex = vindex_0;
    // In initial part, 'in' always points to 'input'
    for (; x < x1; ++x) {
        int            int_pel    = y >> RS_SCALE_SUBPEL_BITS;
        int            sub_pel    = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
        const int16_t* filter_c   = &interp_filters[sub_pel * SUBPEL_TAPS];
        __m128i        filter_128 = _mm_lddqu_si128((const __m128i*)filter_c);
        vec_filter[0]             = _mm256_cvtepi16_epi32(filter_128);
        for (int k = 1; k < 8; ++k) {
            vec_filter[k] = vec_filter[0];
        }

        if (int_pel - 3 < 0) {
            assert(int_pel - 3 >= -4);
            switch (int_pel - 3) {
            case -4:
                vindex = vindex_neg4;
                break;
            case -3:
                vindex = vindex_neg3;
                break;
            case -2:
                vindex = vindex_neg2;
                break;
            default:
                vindex = vindex_neg1;
                break;
            }
        }

        const uint16_t* in   = input;
        uint16_t*       optr = &output[x * out_stride];

        int j;
        for (j = 0; j < (in_width & (~7)); j += 8) {
            highbd_interpolate_gather_load_8x8(in, vindex, vec_src);
            highbd_interpolate_core_w8_avx2(vec_src, vec_filter, optr, max);

            optr += steps;
            in += steps;
        }

        // up to seven columns left. two columns by each loop
        for (; j < (in_width & (~1)); j += 2) {
            __m256i load = _mm256_i32gather_epi32((const int*)in, vindex, 2);
            vec_src[0]   = _mm256_and_si256(load, _mm256_set1_epi32(0xffff)); // col 0
            vec_src[1]   = _mm256_srli_epi32(load, 16); // col 1

            __m256i one_pt = _mm256_mullo_epi32(vec_src[0], vec_filter[0]);
            int32_t sum    = hsums_epi32(one_pt);
            *optr++        = clip_pixel_highbd(ROUND_POWER_OF_TWO(sum, FILTER_BITS), bd);

            one_pt  = _mm256_mullo_epi32(vec_src[1], vec_filter[0]);
            sum     = hsums_epi32(one_pt);
            *optr++ = clip_pixel_highbd(ROUND_POWER_OF_TWO(sum, FILTER_BITS), bd);

            in += 2;
        }

        y += delta;
    }

    // Middle part.
    vindex = vindex_0;
    for (; x <= x2; ++x) {
        int            int_pel    = y >> RS_SCALE_SUBPEL_BITS;
        int            sub_pel    = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
        const int16_t* filter_c   = &interp_filters[sub_pel * SUBPEL_TAPS];
        __m128i        filter_128 = _mm_lddqu_si128((const __m128i*)filter_c);
        vec_filter[0]             = _mm256_cvtepi16_epi32(filter_128);
        for (int k = 1; k < 8; ++k) {
            vec_filter[k] = vec_filter[0];
        }

        const uint16_t* in   = &input[(int_pel - 3) * in_stride];
        uint16_t*       optr = &output[x * out_stride];

        int j;
        for (j = 0; j < (in_width & (~7)); j += 8) {
            highbd_interpolate_gather_load_8x8(in, vindex, vec_src);
            highbd_interpolate_core_w8_avx2(vec_src, vec_filter, optr, max);

            optr += steps;
            in += steps;
        }

        // up to seven columns left. two columns by each loop
        for (; j < (in_width & (~1)); j += 2) {
            __m256i load = _mm256_i32gather_epi32((const int*)in, vindex, 2);
            vec_src[0]   = _mm256_and_si256(load, _mm256_set1_epi32(0xffff)); // col 0
            vec_src[1]   = _mm256_srli_epi32(load, 16); // col 1

            __m256i one_pt = _mm256_mullo_epi32(vec_src[0], vec_filter[0]);
            int32_t sum    = hsums_epi32(one_pt);
            *optr++        = clip_pixel_highbd(ROUND_POWER_OF_TWO(sum, FILTER_BITS), bd);

            one_pt  = _mm256_mullo_epi32(vec_src[1], vec_filter[0]);
            sum     = hsums_epi32(one_pt);
            *optr++ = clip_pixel_highbd(ROUND_POWER_OF_TWO(sum, FILTER_BITS), bd);

            in += 2;
        }

        y += delta;
    }

    // End part.
    for (; x < out_height; ++x) {
        int            int_pel    = y >> RS_SCALE_SUBPEL_BITS;
        int            sub_pel    = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
        const int16_t* filter_c   = &interp_filters[sub_pel * SUBPEL_TAPS];
        __m128i        filter_128 = _mm_lddqu_si128((const __m128i*)filter_c);
        vec_filter[0]             = _mm256_cvtepi16_epi32(filter_128);
        for (int k = 1; k < 8; ++k) {
            vec_filter[k] = vec_filter[0];
        }

        if (int_pel + 4 >= in_height) {
            assert(int_pel + 4 - in_height + 1 <= 4);
            switch (int_pel + 4 - in_height + 1) {
            case 4:
                vindex = vindex_pos4;
                break;
            case 3:
                vindex = vindex_pos3;
                break;
            case 2:
                vindex = vindex_pos2;
                break;
            default:
                vindex = vindex_pos1;
                break;
            }
        }

        const uint16_t* in   = &input[(int_pel - 3) * in_stride];
        uint16_t*       optr = &output[x * out_stride];

        int j;
        for (j = 0; j < (in_width & (~7)); j += 8) {
            highbd_interpolate_gather_load_8x8(in, vindex, vec_src);
            highbd_interpolate_core_w8_avx2(vec_src, vec_filter, optr, max);

            optr += steps;
            in += steps;
        }

        // up to seven columns left. two columns by each loop
        for (; j < (in_width & (~1)); j += 2) {
            __m256i load = _mm256_i32gather_epi32((const int*)in, vindex, 2);
            vec_src[0]   = _mm256_and_si256(load, _mm256_set1_epi32(0xffff)); // col 0
            vec_src[1]   = _mm256_srli_epi32(load, 16); // col 1

            __m256i one_pt = _mm256_mullo_epi32(vec_src[0], vec_filter[0]);
            int32_t sum    = hsums_epi32(one_pt);
            *optr++        = clip_pixel_highbd(ROUND_POWER_OF_TWO(sum, FILTER_BITS), bd);

            one_pt  = _mm256_mullo_epi32(vec_src[1], vec_filter[0]);
            sum     = hsums_epi32(one_pt);
            *optr++ = clip_pixel_highbd(ROUND_POWER_OF_TWO(sum, FILTER_BITS), bd);
            in += 2;
        }

        y += delta;
    }

    // last column
    if ((in_width & 1) != 0) {
        x = 0;
        y = offset + RS_SCALE_EXTRA_OFF;
        uint16_t        src_c[8];
        const uint16_t* in   = input + in_width - 1;
        uint16_t*       optr = &output[in_width - 1];
        for (; x < x1; ++x) {
            int            int_pel  = y >> RS_SCALE_SUBPEL_BITS;
            int            sub_pel  = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
            const int16_t* filter_c = &interp_filters[sub_pel * SUBPEL_TAPS];
            for (int k = 0; k < SUBPEL_TAPS; ++k) {
                const int pk = int_pel - SUBPEL_TAPS / 2 + 1 + k;
                src_c[k]     = in[in_stride * AOMMAX(pk, 0)];
            }

            *optr = highbd_interpolate_compute_1pt(src_c, filter_c, bd);
            optr += out_stride;
            y += delta;
        }
        for (; x <= x2; ++x) {
            int            int_pel  = y >> RS_SCALE_SUBPEL_BITS;
            int            sub_pel  = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
            const int16_t* filter_c = &interp_filters[sub_pel * SUBPEL_TAPS];
            for (int k = 0; k < SUBPEL_TAPS; ++k) {
                const int pk = int_pel - SUBPEL_TAPS / 2 + 1 + k;
                src_c[k]     = in[in_stride * pk];
            }

            *optr = highbd_interpolate_compute_1pt(src_c, filter_c, bd);
            optr += out_stride;
            y += delta;
        }
        for (; x < out_height; ++x) {
            int            int_pel  = y >> RS_SCALE_SUBPEL_BITS;
            int            sub_pel  = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
            const int16_t* filter_c = &interp_filters[sub_pel * SUBPEL_TAPS];
            for (int k = 0; k < SUBPEL_TAPS; ++k) {
                const int pk = int_pel - SUBPEL_TAPS / 2 + 1 + k;
                src_c[k]     = in[in_stride * AOMMIN(pk, in_height - 1)];
            }

            *optr = highbd_interpolate_compute_1pt(src_c, filter_c, bd);
            optr += out_stride;
            y += delta;
        }
    }

    return EB_ErrorNone;
}

// Transpose 32x 8bit src in the form of 8r x 4c to 32x 16bit dst in the form of 4r x 8c. Each row is stored in one __m128i
static INLINE void interpolate_transpose_8x4(const __m256i src, __m128i dst[4]) {
    __m256i tmp     = _mm256_and_si256(src, _mm256_set1_epi32(0xff)); // col 0
    __m128i lo_lane = _mm256_castsi256_si128(tmp);
    __m128i hi_lane = _mm256_extracti128_si256(tmp, 1);
    dst[0]          = _mm_packus_epi32(lo_lane, hi_lane);

    tmp     = _mm256_and_si256(src, _mm256_set1_epi32(0xff00)); // col 1
    tmp     = _mm256_srli_epi32(tmp, 8);
    lo_lane = _mm256_castsi256_si128(tmp);
    hi_lane = _mm256_extracti128_si256(tmp, 1);
    dst[1]  = _mm_packus_epi32(lo_lane, hi_lane);

    tmp     = _mm256_and_si256(src, _mm256_set1_epi32(0xff0000)); // col 2
    tmp     = _mm256_srli_epi32(tmp, 16);
    lo_lane = _mm256_castsi256_si128(tmp);
    hi_lane = _mm256_extracti128_si256(tmp, 1);
    dst[2]  = _mm_packus_epi32(lo_lane, hi_lane);

    tmp     = _mm256_and_si256(src, _mm256_set1_epi32(0xff000000)); // col 3
    tmp     = _mm256_srli_epi32(tmp, 24);
    lo_lane = _mm256_castsi256_si128(tmp);
    hi_lane = _mm256_extracti128_si256(tmp, 1);
    dst[3]  = _mm_packus_epi32(lo_lane, hi_lane);
}

static INLINE void interpolate_gather_load_8x16(const uint8_t* const in, const __m256i vindex, __m128i dst[16]) {
    __m256i load = _mm256_i32gather_epi32((const int*)in, vindex, 1);
    interpolate_transpose_8x4(load, &dst[0]);

    load = _mm256_i32gather_epi32((const int*)(in + 4), vindex, 1);
    interpolate_transpose_8x4(load, &dst[4]);

    load = _mm256_i32gather_epi32((const int*)(in + 8), vindex, 1);
    interpolate_transpose_8x4(load, &dst[8]);

    load = _mm256_i32gather_epi32((const int*)(in + 12), vindex, 1);
    interpolate_transpose_8x4(load, &dst[12]);
}

static INLINE void down2_symeven_gather_load_8x4(const uint8_t* const in, const __m256i vindex, __m128i dst[4]) {
    __m256i load = _mm256_i32gather_epi32((const int*)in, vindex, 1);
    interpolate_transpose_8x4(load, &dst[0]);
}

static INLINE uint8_t interpolate_compute_1pt(const uint8_t* in, const int16_t* filter) {
    const __m256i vec_filter = _mm256_cvtepi16_epi32(_mm_lddqu_si128((const __m128i*)filter));
    const __m128i src_16bit  = _mm_cvtepu8_epi16(_mm_loadl_epi64((const __m128i*)in));
    const __m256i src_32bit  = _mm256_cvtepu16_epi32(src_16bit);
    const __m256i one_pt     = _mm256_mullo_epi32(src_32bit, vec_filter);
    const int32_t sum        = hsums_epi32(one_pt);
    return clip_pixel(ROUND_POWER_OF_TWO(sum, FILTER_BITS));
}

static INLINE void interpolate_core_col_4pt(const uint8_t* in, const __m256i filter_2x, const __m256i vindex,
                                            const __m256i max, uint8_t* output) {
    __m128i       vec_src[4];
    const __m256i zero = _mm256_setzero_si256();

    __m256i load = _mm256_i32gather_epi32((const int*)in, vindex, 1);
    interpolate_transpose_8x4(load, vec_src);

    __m256i src_02 = _mm256_set_m128i(vec_src[2], vec_src[0]);
    __m256i src_13 = _mm256_set_m128i(vec_src[3], vec_src[1]);

    __m256i vec_2pt[2];
    // P00 ab cd ef gh  P02 ab cd ef gh
    vec_2pt[0] = _mm256_madd_epi16(src_02, filter_2x);
    // P01 ab cd ef gh  P03 ab cd ef gh
    vec_2pt[1] = _mm256_madd_epi16(src_13, filter_2x);
    // P00 abcd efgh P01 abcd efgh  P02 abcd efgh P03 abcd efgh
    __m256i vec_4pt = _mm256_hadd_epi32(vec_2pt[0], vec_2pt[1]);

    // P00 P01 ZERO ZERO   P02 P03 ZERO ZERO
    vec_4pt = _mm256_hadd_epi32(vec_4pt, zero);
    // P00 P01 P02 P03    ZERO ZERO ZERO ZERO
    vec_4pt = _mm256_permute4x64_epi64(vec_4pt, _MM_SHUFFLE(3, 1, 2, 0));
    vec_4pt = RightShiftWithRounding_S32(vec_4pt, FILTER_BITS);
    mm256_clamp_epi32(&vec_4pt, zero, max);

    // 4x 16bit: P00 P01 P02 P03  ZERO ...
    vec_4pt = _mm256_packus_epi32(vec_4pt, zero);
    // 4x 8bit:  P00 P01 P02 P03  ZERO ...
    __m128i vec_4x8i = _mm_packus_epi16(_mm256_castsi256_si128(vec_4pt), _mm256_castsi256_si128(zero));
    _mm_storeu_si32(output, vec_4x8i);
}

static INLINE void interpolate_core_col_w16_avx2(__m128i short_src[16], __m256i filter_x2, uint8_t* optr) {
    const __m256i min = _mm256_setzero_si256();
    const __m256i max = _mm256_set1_epi16(255);

    __m256i sum[8];

    for (int j = 0; j < 4; ++j) {
        __m256i src_ab = _mm256_set_m128i(short_src[j + 4], short_src[j]);

        // two pixel per vector
        sum[j] = _mm256_madd_epi16(src_ab, filter_x2);
        // sum[0]: P00ab P00cd P00ef P00gh P04ab P04cd P04ef P04gh
        // sum[1]: P01ab P01cd P01ef P01gh P05ab P05cd P05ef P05gh
        // sum[2]: P02ab P02cd P02ef P02gh P06ab P06cd P06ef P06gh
        // sum[3]: P03ab P03cd P03ef P03gh P07ab P07cd P07ef P07gh
    }
    for (int j = 4; j < 8; ++j) {
        __m256i src_ab = _mm256_set_m128i(short_src[j + 8], short_src[j + 4]);

        // two pixel per vector
        sum[j] = _mm256_madd_epi16(src_ab, filter_x2);
        // sum[0]: P00ab P00cd P00ef P00gh P04ab P04cd P04ef P04gh
        // sum[1]: P01ab P01cd P01ef P01gh P05ab P05cd P05ef P05gh
        // sum[2]: P02ab P02cd P02ef P02gh P06ab P06cd P06ef P06gh
        // sum[3]: P03ab P03cd P03ef P03gh P07ab P07cd P07ef P07gh
    }

    __m256i sum0to7, sum8to15;
    {
        // P00abcd P00efgh P01abcd P01efgh P04abcd P04efgh P05abcd P05efgh
        __m256i sum01 = _mm256_hadd_epi32(sum[0], sum[1]);
        // P02abcd P02efgh P03abcd P03efgh P06abcd P06efgh P07abcd P07efgh
        __m256i sum23 = _mm256_hadd_epi32(sum[2], sum[3]);
        // P00 P01 P02 P03 P04 P05 P06 P07
        __m256i sum0123 = _mm256_hadd_epi32(sum01, sum23);
        sum0to7         = RightShiftWithRounding_S32(sum0123, FILTER_BITS);
    }
    {
        __m256i sum01   = _mm256_hadd_epi32(sum[4], sum[5]);
        __m256i sum23   = _mm256_hadd_epi32(sum[6], sum[7]);
        __m256i sum0123 = _mm256_hadd_epi32(sum01, sum23);
        sum8to15        = RightShiftWithRounding_S32(sum0123, FILTER_BITS);
    }

    // 16* 16-bit: P00 P01 P02 P03 P08 P09 P10 P11 P04 P05 P06 P07 P12 P13 P14 P15
    __m256i sum0to15 = _mm256_packs_epi32(sum0to7, sum8to15);
    sum0to15         = _mm256_permute4x64_epi64(sum0to15, _MM_SHUFFLE(3, 1, 2, 0));
    // P00 000 P01 000 P02 000 P03 000 P04 000 P05 000 P06 000 P07 000
    // P08 000 P09 000 P10 000 P11 000 P12 000 P13 000 P14 000 P15 000
    mm256_clamp_epi16(&sum0to15, min, max);

    __m128i lo_lane    = _mm256_castsi256_si128(sum0to15);
    __m128i hi_lane    = _mm256_extracti128_si256(sum0to15, 1);
    __m128i m128_0to15 = _mm_packus_epi16(lo_lane, hi_lane);

    _mm_storeu_si128((__m128i*)optr, m128_0to15);
}

static EbErrorType svt_av1_interpolate_core_col_avx2(const uint8_t* const input, int in_width, int in_height,
                                                     int in_stride, uint8_t* output, int out_height, int out_stride,
                                                     const int16_t* interp_filters) {
    const int32_t steps = 16; // output steps
    const __m256i max   = _mm256_set1_epi32(255);
    // number of columns that will be processed with steps set to 'steps'
    const int32_t cols           = in_width & (~15);
    const int32_t left_over_cols = in_width - cols;
    const __m256i vindex_0       = _mm256_set_epi32(
        in_stride * 7, in_stride * 6, in_stride * 5, in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0);
    const __m256i vindex_neg1 = _mm256_set_epi32(
        in_stride * 6, in_stride * 5, in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0, 0);
    const __m256i vindex_neg2 = _mm256_set_epi32(
        in_stride * 5, in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0, 0, 0);
    const __m256i vindex_neg3 = _mm256_set_epi32(in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0, 0, 0, 0);
    const __m256i vindex_neg4 = _mm256_set_epi32(in_stride * 3, in_stride * 2, in_stride, 0, 0, 0, 0, 0);
    const __m256i vindex_pos1 = _mm256_set_epi32(
        in_stride * 6, in_stride * 6, in_stride * 5, in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0);
    const __m256i vindex_pos2 = _mm256_set_epi32(
        in_stride * 5, in_stride * 5, in_stride * 5, in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0);
    const __m256i vindex_pos3 = _mm256_set_epi32(
        in_stride * 4, in_stride * 4, in_stride * 4, in_stride * 4, in_stride * 3, in_stride * 2, in_stride, 0);
    const __m256i vindex_pos4 = _mm256_set_epi32(
        in_stride * 3, in_stride * 3, in_stride * 3, in_stride * 3, in_stride * 3, in_stride * 2, in_stride, 0);

    const int32_t  delta  = (((uint32_t)in_height << RS_SCALE_SUBPEL_BITS) + out_height / 2) / out_height;
    const int32_t  offset = in_height > out_height
         ? (((int32_t)(in_height - out_height) << (RS_SCALE_SUBPEL_BITS - 1)) + out_height / 2) / out_height
         : -(((int32_t)(out_height - in_height) << (RS_SCALE_SUBPEL_BITS - 1)) + out_height / 2) / out_height;
    int32_t        x, x1, x2;
    int32_t        y;
    const uint8_t* in   = NULL;
    uint8_t*       optr = NULL;

    x = 0;
    y = offset + RS_SCALE_EXTRA_OFF;
    while ((y >> RS_SCALE_SUBPEL_BITS) < (SUBPEL_TAPS / 2 - 1)) { // (y >> 14) < 3 ?
        x++;
        y += delta;
    }
    x1 = x;
    x  = out_height - 1;
    y  = delta * x + offset + RS_SCALE_EXTRA_OFF;
    while ((y >> RS_SCALE_SUBPEL_BITS) + (int32_t)(SUBPEL_TAPS / 2) >= in_height) {
        x--;
        y -= delta;
    }
    x2 = x;
    assert(x1 <= x2);

    x = 0;
    y = offset + RS_SCALE_EXTRA_OFF;

    __m128i vec_src[16];
    __m256i vec_filter_2x;

    // Initial part: write first x1 rows to output
    __m256i vindex = vindex_0;
    // In initial part, 'in' always points to 'input'
    for (; x < x1; ++x) {
        int            int_pel  = y >> RS_SCALE_SUBPEL_BITS;
        int            sub_pel  = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
        const int16_t* filter_c = &interp_filters[sub_pel * SUBPEL_TAPS];
        vec_filter_2x           = _mm256_broadcastsi128_si256(_mm_lddqu_si128((const __m128i*)filter_c));

        if (int_pel - 3 < 0) {
            assert(int_pel - 3 <= -1);
            assert(int_pel - 3 >= -4);
            switch (int_pel - 3) {
            case -4:
                vindex = vindex_neg4;
                break;
            case -3:
                vindex = vindex_neg3;
                break;
            case -2:
                vindex = vindex_neg2;
                break;
            default:
                vindex = vindex_neg1;
                break;
            }
        }

        in   = input;
        optr = &output[x * out_stride];

        int j;
        for (j = 0; j < cols; j += steps) {
            interpolate_gather_load_8x16(in, vindex, vec_src);
            interpolate_core_col_w16_avx2(vec_src, vec_filter_2x, optr);

            optr += steps;
            in += steps;
        }

        // up to fifteen columns left. process four columns by each loop because gather load loads 32-bit in a row
        if (left_over_cols >= 4) {
            for (j = 0; j + 4 < left_over_cols; j += 4) {
                interpolate_core_col_4pt(in, vec_filter_2x, vindex, max, optr);

                optr += 4;
                in += 4;
            }
        }

        y += delta;
    }

    // Middle part.
    vindex = vindex_0;
    for (; x <= x2; ++x) {
        int            int_pel  = y >> RS_SCALE_SUBPEL_BITS;
        int            sub_pel  = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
        const int16_t* filter_c = &interp_filters[sub_pel * SUBPEL_TAPS];
        vec_filter_2x           = _mm256_broadcastsi128_si256(_mm_lddqu_si128((const __m128i*)filter_c));

        in   = &input[(int_pel - 3) * in_stride];
        optr = &output[x * out_stride];

        int j;
        for (j = 0; j < cols; j += steps) {
            interpolate_gather_load_8x16(in, vindex, vec_src);
            interpolate_core_col_w16_avx2(vec_src, vec_filter_2x, optr);

            optr += steps;
            in += steps;
        }

        // up to fifteen columns left. process four columns by each loop because gather load loads 32-bit in a row
        if (left_over_cols >= 4) {
            for (j = 0; j + 4 < left_over_cols; j += 4) {
                interpolate_core_col_4pt(in, vec_filter_2x, vindex, max, optr);

                optr += 4;
                in += 4;
            }
        }

        y += delta;
    }

    // End part.
    for (; x < out_height; ++x) {
        int            int_pel  = y >> RS_SCALE_SUBPEL_BITS;
        int            sub_pel  = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
        const int16_t* filter_c = &interp_filters[sub_pel * SUBPEL_TAPS];
        vec_filter_2x           = _mm256_broadcastsi128_si256(_mm_lddqu_si128((const __m128i*)filter_c));

        if (int_pel + 4 >= in_height) {
            assert(int_pel + 4 - in_height + 1 <= 4);
            switch (int_pel + 4 - in_height + 1) {
            case 4:
                vindex = vindex_pos4;
                break;
            case 3:
                vindex = vindex_pos3;
                break;
            case 2:
                vindex = vindex_pos2;
                break;
            default:
                vindex = vindex_pos1;
                break;
            }
        }

        in   = &input[(int_pel - 3) * in_stride];
        optr = &output[x * out_stride];

        int j;
        for (j = 0; j < cols; j += steps) {
            interpolate_gather_load_8x16(in, vindex, vec_src);
            interpolate_core_col_w16_avx2(vec_src, vec_filter_2x, optr);

            optr += steps;
            in += steps;
        }

        // up to fifteen columns left. process four columns by each loop because gather load loads 32-bit in a row
        if (left_over_cols >= 4) {
            for (j = 0; j + 4 < left_over_cols; j += 4) {
                interpolate_core_col_4pt(in, vec_filter_2x, vindex, max, optr);

                optr += 4;
                in += 4;
            }
        }

        y += delta;
    }

    // up to 15 columns at right edge
    int32_t col = in_width - left_over_cols;
    for (; col < in_width; ++col) {
        x = 0;
        y = offset + RS_SCALE_EXTRA_OFF;
        uint8_t src_c[8];
        in   = input + col;
        optr = output + col;
        for (; x < x1; ++x) {
            int            int_pel  = y >> RS_SCALE_SUBPEL_BITS;
            int            sub_pel  = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
            const int16_t* filter_c = &interp_filters[sub_pel * SUBPEL_TAPS];
            for (int k = 0; k < SUBPEL_TAPS; ++k) {
                const int pk = int_pel - SUBPEL_TAPS / 2 + 1 + k;
                src_c[k]     = in[in_stride * AOMMAX(pk, 0)];
            }

            *optr = interpolate_compute_1pt(src_c, filter_c);
            optr += out_stride;
            y += delta;
        }
        for (; x <= x2; ++x) {
            int            int_pel  = y >> RS_SCALE_SUBPEL_BITS;
            int            sub_pel  = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
            const int16_t* filter_c = &interp_filters[sub_pel * SUBPEL_TAPS];
            for (int k = 0; k < SUBPEL_TAPS; ++k) {
                const int pk = int_pel - SUBPEL_TAPS / 2 + 1 + k;
                src_c[k]     = in[in_stride * pk];
            }

            *optr = interpolate_compute_1pt(src_c, filter_c);
            optr += out_stride;
            y += delta;
        }
        for (; x < out_height; ++x) {
            int            int_pel  = y >> RS_SCALE_SUBPEL_BITS;
            int            sub_pel  = (y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK;
            const int16_t* filter_c = &interp_filters[sub_pel * SUBPEL_TAPS];
            for (int k = 0; k < SUBPEL_TAPS; ++k) {
                const int pk = int_pel - SUBPEL_TAPS / 2 + 1 + k;
                src_c[k]     = in[in_stride * AOMMIN(pk, in_height - 1)];
            }

            *optr = interpolate_compute_1pt(src_c, filter_c);
            optr += out_stride;
            y += delta;
        }
    }

    return EB_ErrorNone;
}

void svt_av1_highbd_down2_symeven_avx2(const uint16_t* const input, int length, uint16_t* output, int bd) {
    const int16_t* filter          = svt_aom_av1_down2_symeven_half_filter;
    const int      filter_len_half = sizeof(svt_aom_av1_down2_symeven_half_filter) / 2;
    const int      steps           = 16;
    __m256i        max;
    if (bd == 10) {
        max = _mm256_set1_epi32(1023);
    } else {
        max = _mm256_set1_epi32(4095);
    }

    const __m128i filter_2x_128i = _mm_broadcastq_epi64(_mm_loadl_epi64((const __m128i*)filter));
    const __m256i filter_2x      = _mm256_cvtepi16_epi32(filter_2x_128i);

    int       i, j;
    uint16_t* optr = output;
    const int l1   = steps;
    int       l2   = (length - filter_len_half);
    l2 += (l2 & 1);

    // Initial part.
    highbd_down2_symeven_w8_init_part_avx2(input, &optr, filter_2x, max);
    i = l1;

    // Middle part.
    int mid = (l2 - l1 + 1) & (~(steps - 1));
    if (mid > 0) {
        highbd_down2_symeven_w8_mid_part_avx2(input + i, mid, &optr, filter_2x, max);
        i += mid;
    }

    // Middle part + End part.
    if (length - i >= steps) {
        int len_to_end = length - i;
        highbd_down2_symeven_w8_end_part_avx2(input + i, &optr, filter_2x, len_to_end, max);
        i += steps;
    }

    // End part.
    for (; i < length; i += 2) {
        int sum = (1 << (FILTER_BITS - 1));
        for (j = 0; j < filter_len_half; ++j) {
            sum += (input[i - j] + input[AOMMIN(i + 1 + j, length - 1)]) * filter[j];
        }
        sum >>= FILTER_BITS;
        *optr++ = clip_pixel_highbd(sum, bd);
    }
}

static INLINE __m128i mm_32i_to_16i(__m256i a) {
    __m128i lo_lane = _mm256_castsi256_si128(a);
    __m128i hi_lane = _mm256_extracti128_si256(a, 1);
    return _mm_packus_epi32(lo_lane, hi_lane); // 8x 16-bit
}

static INLINE void highbd_down2_symeven_mm_gather_load_8x8(const uint16_t* in, __m128i dst[8], __m256i vindex) {
    __m256i load = _mm256_i32gather_epi32((const int*)in, vindex, 2);
    dst[0]       = mm_32i_to_16i(_mm256_and_si256(load,
                                            _mm256_set1_epi32(0xffff))); // col 0
    dst[1]       = mm_32i_to_16i(_mm256_srli_epi32(load, 16)); // col 1

    load   = _mm256_i32gather_epi32((const int*)(in + 2), vindex, 2);
    dst[2] = mm_32i_to_16i(_mm256_and_si256(load,
                                            _mm256_set1_epi32(0xffff))); // col 2
    dst[3] = mm_32i_to_16i(_mm256_srli_epi32(load, 16)); // col 3

    load   = _mm256_i32gather_epi32((const int*)(in + 4), vindex, 2);
    dst[4] = mm_32i_to_16i(_mm256_and_si256(load,
                                            _mm256_set1_epi32(0xffff))); // col 4
    dst[5] = mm_32i_to_16i(_mm256_srli_epi32(load, 16)); // col 5

    load   = _mm256_i32gather_epi32((const int*)(in + 6), vindex, 2);
    dst[6] = mm_32i_to_16i(_mm256_and_si256(load,
                                            _mm256_set1_epi32(0xffff))); // col 6
    dst[7] = mm_32i_to_16i(_mm256_srli_epi32(load, 16)); // col 7
}

static INLINE void highbd_down2_symeven_mm256_gather_load_8x8(const uint16_t* in, __m256i dst[8], __m256i vindex) {
    __m256i load = _mm256_i32gather_epi32((const int*)in, vindex, 2);
    dst[0]       = _mm256_and_si256(load,
                              _mm256_set1_epi32(0xffff)); // col 0
    dst[1]       = _mm256_srli_epi32(load, 16); // col 1

    load   = _mm256_i32gather_epi32((const int*)(in + 2), vindex, 2);
    dst[2] = _mm256_and_si256(load,
                              _mm256_set1_epi32(0xffff)); // col 2
    dst[3] = _mm256_srli_epi32(load, 16); // col 3

    load   = _mm256_i32gather_epi32((const int*)(in + 4), vindex, 2);
    dst[4] = _mm256_and_si256(load,
                              _mm256_set1_epi32(0xffff)); // col 4
    dst[5] = _mm256_srli_epi32(load, 16); // col 5

    load   = _mm256_i32gather_epi32((const int*)(in + 6), vindex, 2);
    dst[6] = _mm256_and_si256(load,
                              _mm256_set1_epi32(0xffff)); // col 6
    dst[7] = _mm256_srli_epi32(load, 16); // col 7
}

static INLINE void highbd_down2_symeven_mm_gather_load_16x2(const uint16_t* in, int32_t stride, __m128i dst[4],
                                                            __m256i vindex_0, __m256i vindex_1, int32_t row_offset) {
    __m256i load = _mm256_i32gather_epi32((const int*)in, vindex_0, 2);
    dst[0]       = mm_32i_to_16i(_mm256_and_si256(load, _mm256_set1_epi32(0xffff))); // col 0
    dst[2]       = mm_32i_to_16i(_mm256_srli_epi32(load, 16)); // col 1

    in += stride * row_offset; // move down 'row_offset' rows
    load   = _mm256_i32gather_epi32((const int*)in, vindex_1, 2);
    dst[1] = mm_32i_to_16i(_mm256_and_si256(load, _mm256_set1_epi32(0xffff))); // col 0
    dst[3] = mm_32i_to_16i(_mm256_srli_epi32(load, 16)); // col 1
}

static INLINE void highbd_down2_symeven_mm256_gather_load_16x2(const uint16_t* in, int32_t stride, __m256i dst[4],
                                                               __m256i vindex_0, __m256i vindex_1, int32_t row_offset) {
    __m256i load = _mm256_i32gather_epi32((const int*)in, vindex_0, 2);
    dst[0]       = _mm256_and_si256(load, _mm256_set1_epi32(0xffff)); // col 0
    dst[2]       = _mm256_srli_epi32(load, 16); // col 1

    in += stride * row_offset; // move down 'row_offset' rows
    load   = _mm256_i32gather_epi32((const int*)in, vindex_1, 2);
    dst[1] = _mm256_and_si256(load, _mm256_set1_epi32(0xffff)); // col 0
    dst[3] = _mm256_srli_epi32(load, 16); // col 1
}

static INLINE void highbd_down2_symeven_obtain_4x8(const __m256i vec_4pt[8], const __m256i base, const __m256i min,
                                                   const __m256i max, __m128i vec_8pt_i16[4]) {
    __m256i vec_8pt_i32[4];
    for (int k = 0; k < 4; ++k) {
        // P00 P01 P04 P05 P02 P03 P06 P07
        // P08 P09 P12 P13 P10 P11 P14 P15
        // P16 P17 P20 P21 P18 P19 P22 P23
        // P24 P25 P28 P29 P26 P27 P30 P31
        vec_8pt_i32[k] = _mm256_hadd_epi32(vec_4pt[k * 2], vec_4pt[k * 2 + 1]);
    }

    // P00 P04 P01 P05 P02 P06 P03 P07
    vec_8pt_i32[0] = _mm256_shuffle_epi32(vec_8pt_i32[0], _MM_SHUFFLE(3, 1, 2, 0));
    // P08 P12 P09 P13 P10 P14 P11 P15
    vec_8pt_i32[1] = _mm256_shuffle_epi32(vec_8pt_i32[1], _MM_SHUFFLE(3, 1, 2, 0));
    // P16 P20 P17 P21 P18 P22 P19 P23
    vec_8pt_i32[2] = _mm256_shuffle_epi32(vec_8pt_i32[2], _MM_SHUFFLE(3, 1, 2, 0));
    // P24 P28 P25 P29 P26 P30 P27 P31
    vec_8pt_i32[3] = _mm256_shuffle_epi32(vec_8pt_i32[3], _MM_SHUFFLE(3, 1, 2, 0));

    // P00 P04 P08 P12 P02 P06 P10 P14
    __m256i vec_8pt_i3_tmp0 = _mm256_unpacklo_epi64(vec_8pt_i32[0], vec_8pt_i32[1]);
    // P16 P20 P24 P28 P18 P22 P26 P30
    __m256i vec_8pt_i3_tmp1 = _mm256_unpacklo_epi64(vec_8pt_i32[2], vec_8pt_i32[3]);
    // P01 P05 P09 P13 P03 P07 P11 P15
    __m256i vec_8pt_i3_tmp2 = _mm256_unpackhi_epi64(vec_8pt_i32[0], vec_8pt_i32[1]);
    // P17 P21 P25 P29 P19 P23 P27 P31
    __m256i vec_8pt_i3_tmp3 = _mm256_unpackhi_epi64(vec_8pt_i32[2], vec_8pt_i32[3]);

    // P00 P04 P08 P12 P16 P20 P24 P28
    vec_8pt_i32[0] = _mm256_set_m128i(_mm256_castsi256_si128(vec_8pt_i3_tmp1), _mm256_castsi256_si128(vec_8pt_i3_tmp0));
    // P01 P05 P09 P13 P17 P21 P25 P29
    vec_8pt_i32[1] = _mm256_set_m128i(_mm256_castsi256_si128(vec_8pt_i3_tmp3), _mm256_castsi256_si128(vec_8pt_i3_tmp2));
    // P02 P06 P10 P14 P18 P22 P26 P30
    vec_8pt_i32[2] = _mm256_set_m128i(_mm256_extracti128_si256(vec_8pt_i3_tmp1, 1),
                                      _mm256_extracti128_si256(vec_8pt_i3_tmp0, 1));
    // P03 P07 P11 P15 P19 P23 P27 P31
    vec_8pt_i32[3] = _mm256_set_m128i(_mm256_extracti128_si256(vec_8pt_i3_tmp3, 1),
                                      _mm256_extracti128_si256(vec_8pt_i3_tmp2, 1));

    for (int k = 0; k < 4; ++k) {
        vec_8pt_i32[k] = _mm256_add_epi32(vec_8pt_i32[k], base);
        vec_8pt_i32[k] = _mm256_srai_epi32(vec_8pt_i32[k], FILTER_BITS);
        mm256_clamp_epi32(&vec_8pt_i32[k], min, max);

        // 8x 32-bit => 8x 16-bit
        vec_8pt_i16[k] = mm_32i_to_16i(vec_8pt_i32[k]);
    }
}

static INLINE void down2_symeven_obtain_4x16(const __m256i vec_4pt[16], const __m256i base, const __m256i min,
                                             const __m256i max, __m128i vec_16pt_i8[4]) {
    __m256i vec_8pt_i32[8];
    for (int k = 0; k < 8; ++k) {
        // P00 P01 P04 P05 P02 P03 P06 P07
        // P08 P09 P12 P13 P10 P11 P14 P15
        // P16 P17 P20 P21 P18 P19 P22 P23
        // P24 P25 P28 P29 P26 P27 P30 P31
        // ...
        // P56 P57 P60 P61 P58 P59 P62 P63
        vec_8pt_i32[k] = _mm256_hadd_epi32(vec_4pt[k * 2], vec_4pt[k * 2 + 1]);

        // P00 P04 P01 P05 P02 P06 P03 P07
        // P08 P12 P09 P13 P10 P14 P11 P15
        // P16 P20 P17 P21 P18 P22 P19 P23
        // P24 P28 P25 P29 P26 P30 P27 P31
        // P32 P36 P33 P37 P34 P38 P35 P39
        // P40 P44 P41 P45 P42 P46 P43 P47
        // P48 P52 P49 P53 P50 P54 P51 P55
        // P56 P60 P57 P61 P58 P62 P59 P63
        vec_8pt_i32[k] = _mm256_shuffle_epi32(vec_8pt_i32[k], _MM_SHUFFLE(3, 1, 2, 0));
    }

    __m256i vec_8pt_i3_tmp_lo[4];
    __m256i vec_8pt_i3_tmp_hi[4];
    // P00 P04 P08 P12 P02 P06 P10 P14
    vec_8pt_i3_tmp_lo[0] = _mm256_unpacklo_epi64(vec_8pt_i32[0], vec_8pt_i32[1]);
    // P01 P05 P09 P13 P03 P07 P11 P15
    vec_8pt_i3_tmp_lo[1] = _mm256_unpackhi_epi64(vec_8pt_i32[0], vec_8pt_i32[1]);
    // P32 P36 P40 P44 P34 P38 P42 P46
    vec_8pt_i3_tmp_lo[2] = _mm256_unpacklo_epi64(vec_8pt_i32[4], vec_8pt_i32[5]);
    // P33 P37 P41 P45 P35 P39 P43 P47
    vec_8pt_i3_tmp_lo[3] = _mm256_unpackhi_epi64(vec_8pt_i32[4], vec_8pt_i32[5]);

    // P16 P20 P24 P28 P18 P22 P26 P30
    vec_8pt_i3_tmp_hi[0] = _mm256_unpacklo_epi64(vec_8pt_i32[2], vec_8pt_i32[3]);
    // P17 P21 P25 P29 P19 P23 P27 P31
    vec_8pt_i3_tmp_hi[1] = _mm256_unpackhi_epi64(vec_8pt_i32[2], vec_8pt_i32[3]);
    // P48 P52 P56 P60 P50 P54 P58 P62
    vec_8pt_i3_tmp_hi[2] = _mm256_unpacklo_epi64(vec_8pt_i32[6], vec_8pt_i32[7]);
    // P49 P53 P57 P61 P51 P55 P59 P63
    vec_8pt_i3_tmp_hi[3] = _mm256_unpackhi_epi64(vec_8pt_i32[6], vec_8pt_i32[7]);

    for (int k = 0; k < 8; k += 2) {
        // v0: P00 P04 P08 P12 P16 P20 P24 P28
        // v2: P01 P05 P09 P13 P17 P21 P25 P29
        // v4: P32 P36 P40 P44 P48 P52 P56 P60
        // v6: P33 P37 P41 P45 P49 P53 P57 P61
        vec_8pt_i32[k] = _mm256_set_m128i(_mm256_castsi256_si128(vec_8pt_i3_tmp_hi[k / 2]),
                                          _mm256_castsi256_si128(vec_8pt_i3_tmp_lo[k / 2]));
        // v1: P02 P06 P10 P14 P18 P22 P26 P30
        // v3: P03 P07 P11 P15 P19 P23 P27 P31
        // v5: P34 P38 P42 P46 P50 P54 P58 P62
        // v7: P35 P39 P43 P47 P51 P55 P59 P63
        vec_8pt_i32[k + 1] = _mm256_set_m128i(_mm256_extracti128_si256(vec_8pt_i3_tmp_hi[k / 2], 1),
                                              _mm256_extracti128_si256(vec_8pt_i3_tmp_lo[k / 2], 1));
    }

    __m128i vec_8pt_i16[8];
    for (int k = 0; k < 8; ++k) {
        vec_8pt_i32[k] = _mm256_add_epi32(vec_8pt_i32[k], base);
        vec_8pt_i32[k] = _mm256_srai_epi32(vec_8pt_i32[k], FILTER_BITS);
        mm256_clamp_epi32(&vec_8pt_i32[k], min, max);

        // 8x 32-bit => 8x 16-bit
        vec_8pt_i16[k] = mm_32i_to_16i(vec_8pt_i32[k]);
    }

    vec_16pt_i8[0] = _mm_packus_epi16(vec_8pt_i16[0], vec_8pt_i16[4]);
    vec_16pt_i8[1] = _mm_packus_epi16(vec_8pt_i16[2], vec_8pt_i16[6]);
    vec_16pt_i8[2] = _mm_packus_epi16(vec_8pt_i16[1], vec_8pt_i16[5]);
    vec_16pt_i8[3] = _mm_packus_epi16(vec_8pt_i16[3], vec_8pt_i16[7]);
}

static INLINE void down2_symeven_obtain_4x4(const __m256i vec_4pt[4], const __m256i base, const __m256i min,
                                            const __m256i max, __m128i* vec_16pt_i8) {
    __m256i vec_8pt_i32[2];
    for (int k = 0; k < 2; ++k) {
        // P00 P01 P04 P05 P02 P03 P06 P07
        // P08 P09 P12 P13 P10 P11 P14 P15
        vec_8pt_i32[k] = _mm256_hadd_epi32(vec_4pt[k * 2], vec_4pt[k * 2 + 1]);
        // P00 P04 P01 P05 P02 P06 P03 P07
        // P08 P12 P09 P13 P10 P14 P11 P15
        vec_8pt_i32[k] = _mm256_shuffle_epi32(vec_8pt_i32[k], _MM_SHUFFLE(3, 1, 2, 0));
    }

    __m256i vec_8pt_i32_lo, vec_8pt_i32_hi;
    // P00 P04 P08 P12 P02 P06 P10 P14
    vec_8pt_i32_lo = _mm256_unpacklo_epi64(vec_8pt_i32[0], vec_8pt_i32[1]);
    // P01 P05 P09 P13 P03 P07 P11 P15
    vec_8pt_i32_hi = _mm256_unpackhi_epi64(vec_8pt_i32[0], vec_8pt_i32[1]);

    // P00 P04 P08 P12 P01 P05 P09 P13
    vec_8pt_i32[0] = _mm256_set_m128i(_mm256_castsi256_si128(vec_8pt_i32_hi), _mm256_castsi256_si128(vec_8pt_i32_lo));
    // P02 P06 P10 P14 P03 P07 P11 P15
    vec_8pt_i32[1] = _mm256_set_m128i(_mm256_extracti128_si256(vec_8pt_i32_hi, 1),
                                      _mm256_extracti128_si256(vec_8pt_i32_lo, 1));
    __m128i vec_8pt_i16[2];
    for (int k = 0; k < 2; ++k) {
        vec_8pt_i32[k] = _mm256_add_epi32(vec_8pt_i32[k], base);
        vec_8pt_i32[k] = _mm256_srai_epi32(vec_8pt_i32[k], FILTER_BITS);
        mm256_clamp_epi32(&vec_8pt_i32[k], min, max);

        // 8x 32-bit => 8x 16-bit
        vec_8pt_i16[k] = mm_32i_to_16i(vec_8pt_i32[k]);
    }

    *vec_16pt_i8 = _mm_packus_epi16(vec_8pt_i16[0], vec_8pt_i16[1]);
}

static INLINE void assign_vec(__m128i* dst, const __m128i* src, int32_t n) {
    for (int i = 0; i < n; i++) {
        dst[i] = src[i];
    }
}

static INLINE void assign_vec_i16_to_i32(__m256i* dst, const __m128i* src, int32_t n) {
    for (int i = 0; i < n; i++) {
        dst[i] = _mm256_cvtepu16_epi32(src[i]);
    }
}

static INLINE void highbd_down2_symeven_output_4x8_kernel(const __m128i vec_src[16], const __m256i filter_2x,
                                                          const __m256i base_sum, const __m256i min, const __m256i max,
                                                          uint16_t* optr, int32_t out_stride) {
    __m256i vec_4pt[8];
    for (int k = 0; k < 8; ++k) {
        // vec_4pt[k] contains four points to write to the output image in a column,
        // in the form of: P00ab P00cd P01ab P01cd P02ab P02cd P03ab P03cd
        highbd_down2_symeven_prepare_4pt(vec_src[k], vec_src[k + 8], filter_2x, &vec_4pt[k]);
    }

    __m128i vec_8pt_i16[4];
    highbd_down2_symeven_obtain_4x8(vec_4pt, base_sum, min, max, vec_8pt_i16);
    for (int k = 0; k < 4; ++k) {
        _mm_storeu_si128((__m128i*)(optr + out_stride * k), vec_8pt_i16[k]);
    }
}

static INLINE void highbd_down2_symeven_output_4x2_kernel(const __m128i vec_src[4], const __m256i filter_2x,
                                                          const __m256i base_sum, const __m256i min, const __m256i max,
                                                          uint16_t* optr, int32_t out_stride) {
    __m256i vec_4pt[2];
    for (int k = 0; k < 2; ++k) {
        // vec0123[k] contains four points to write to the output image in a column,
        // in the form of: P00ab P00cd P01ab P01cd P02ab P02cd P03ab P03cd
        highbd_down2_symeven_prepare_4pt(vec_src[k * 2], vec_src[k * 2 + 1], filter_2x, &vec_4pt[k]);
    }

    __m256i vec_8pt_i32;
    // P00 P01 P04 P05 P02 P03 P06 P07
    vec_8pt_i32 = _mm256_hadd_epi32(vec_4pt[0], vec_4pt[1]);
    // P00 P04 P01 P05 P02 P06 P03 P07
    vec_8pt_i32 = _mm256_shuffle_epi32(vec_8pt_i32, _MM_SHUFFLE(3, 1, 2, 0));

    vec_8pt_i32 = _mm256_add_epi32(vec_8pt_i32, base_sum);
    vec_8pt_i32 = _mm256_srai_epi32(vec_8pt_i32, FILTER_BITS);
    mm256_clamp_epi32(&vec_8pt_i32, min, max);

    // 8x 32-bit => 8x 16-bit
    __m128i vec_8pt_i16 = mm_32i_to_16i(vec_8pt_i32);

    *(uint32_t*)(optr + 0)              = _mm_extract_epi32(vec_8pt_i16, 0); // write 2x 16-bit
    *(uint32_t*)(optr + out_stride)     = _mm_extract_epi32(vec_8pt_i16, 1);
    *(uint32_t*)(optr + out_stride * 2) = _mm_extract_epi32(vec_8pt_i16, 2);
    *(uint32_t*)(optr + out_stride * 3) = _mm_extract_epi32(vec_8pt_i16, 3);
}

static INLINE void highbd_down2_symeven_output_2x2_kernel(const __m256i vec_src[4], const __m256i filter_2x,
                                                          const __m256i base_sum, const __m256i min, const __m256i max,
                                                          uint16_t* optr, int32_t out_stride) {
    __m256i vec_2pt[2];
    // P00a P00b P00c P00d P01a P01b P01c P01d
    // P02a P02b P02c P02d P03a P03b P03c P03d
    highbd_down2_symeven_prepare_2pt(vec_src[0], vec_src[1], filter_2x, &vec_2pt[0]);
    highbd_down2_symeven_prepare_2pt(vec_src[2], vec_src[3], filter_2x, &vec_2pt[1]);

    __m256i vec_4pt;
    // P00ab P00cd P02ab P02cd P01ab P01cd P03ab P03cd
    vec_4pt = _mm256_hadd_epi32(vec_2pt[0], vec_2pt[1]);
    // P00 P02 00 00 P01 P03 00 00
    vec_4pt = _mm256_hadd_epi32(vec_4pt, _mm256_setzero_si256());

    vec_4pt = _mm256_add_epi32(vec_4pt, base_sum);
    vec_4pt = _mm256_srai_epi32(vec_4pt, FILTER_BITS);
    mm256_clamp_epi32(&vec_4pt, min, max);

    // 8x 32-bit => 8x 16-bit
    __m128i vec_4pt_i16 = mm_32i_to_16i(vec_4pt);

    *(uint32_t*)(optr + 0)          = _mm_extract_epi32(vec_4pt_i16, 0); // write 2x 16-bit
    *(uint32_t*)(optr + out_stride) = _mm_extract_epi32(vec_4pt_i16, 2);
}

static INLINE void highbd_down2_symeven_output_2x8_kernel(const __m256i vec_src[16], const __m256i filter_2x,
                                                          const __m256i base_sum, const __m256i min, const __m256i max,
                                                          uint16_t* optr, int32_t out_stride) {
    __m256i vec_2pt[8];
    // P00a P00b P00c P00d P08a P08b P08c P08d
    // P02a P02b P02c P02d P10a P10b P10c P10d
    // P04a P04b P04c P04d P12a P12b P12c P12d
    // P06a P06b P06c P06d P14a P14b P14c P14d
    highbd_down2_symeven_prepare_2pt(vec_src[0], vec_src[4], filter_2x, &vec_2pt[0]);
    highbd_down2_symeven_prepare_2pt(vec_src[1], vec_src[5], filter_2x, &vec_2pt[1]);
    highbd_down2_symeven_prepare_2pt(vec_src[2], vec_src[6], filter_2x, &vec_2pt[2]);
    highbd_down2_symeven_prepare_2pt(vec_src[3], vec_src[7], filter_2x, &vec_2pt[3]);
    highbd_down2_symeven_prepare_2pt(vec_src[8], vec_src[12], filter_2x, &vec_2pt[4]);
    highbd_down2_symeven_prepare_2pt(vec_src[9], vec_src[13], filter_2x, &vec_2pt[5]);
    highbd_down2_symeven_prepare_2pt(vec_src[10], vec_src[14], filter_2x, &vec_2pt[6]);
    highbd_down2_symeven_prepare_2pt(vec_src[11], vec_src[15], filter_2x, &vec_2pt[7]);

    // P00ab P00cd P02ab P02cd P08ab P08cd P10ab P10cd
    __m256i vec_4pt_a = _mm256_hadd_epi32(vec_2pt[0], vec_2pt[1]);
    // P04ab P04cd P06ab P06cd P12ab P12cd P14ab P14cd
    __m256i vec_4pt_b = _mm256_hadd_epi32(vec_2pt[2], vec_2pt[3]);
    // P00 P02 P04 P06 P08 P10 P12 P14
    __m256i vec_8pt_a = _mm256_hadd_epi32(vec_4pt_a, vec_4pt_b);

    vec_4pt_a = _mm256_hadd_epi32(vec_2pt[4], vec_2pt[5]);
    vec_4pt_b = _mm256_hadd_epi32(vec_2pt[6], vec_2pt[7]);
    // P01 P03 P05 P07 P09 P11 P13 P15
    __m256i vec_8pt_b = _mm256_hadd_epi32(vec_4pt_a, vec_4pt_b);

    vec_8pt_a = _mm256_add_epi32(vec_8pt_a, base_sum);
    vec_8pt_b = _mm256_add_epi32(vec_8pt_b, base_sum);
    vec_8pt_a = _mm256_srai_epi32(vec_8pt_a, FILTER_BITS);
    vec_8pt_b = _mm256_srai_epi32(vec_8pt_b, FILTER_BITS);

    mm256_clamp_epi32(&vec_8pt_a, min, max);
    mm256_clamp_epi32(&vec_8pt_b, min, max);

    // 8x 32-bit => 8x 16-bit
    __m128i vec_8pt_i16 = mm_32i_to_16i(vec_8pt_a);
    _mm_storeu_si128((__m128i*)(optr), vec_8pt_i16);
    vec_8pt_i16 = mm_32i_to_16i(vec_8pt_b);
    _mm_storeu_si128((__m128i*)(optr + out_stride), vec_8pt_i16);
}

static EbErrorType svt_av1_highbd_down2_symeven_col_avx2(const uint16_t* const input, int in_width, int in_height,
                                                         int in_stride, uint16_t* output, int out_stride, int bd) {
    EbErrorType ret = EB_ErrorNone;

    const int      out_height      = in_height / 2;
    const int16_t* filter          = svt_aom_av1_down2_symeven_half_filter;
    const int      filter_len_half = sizeof(svt_aom_av1_down2_symeven_half_filter) / 2;
    __m256i        max;
    if (bd == 10) {
        max = _mm256_set1_epi32(1023);
    } else {
        max = _mm256_set1_epi32(4095);
    }

    const __m128i filter_2x_128i = _mm_broadcastq_epi64(_mm_loadl_epi64((const __m128i*)filter));
    const __m256i filter_2x      = _mm256_cvtepi16_epi32(filter_2x_128i);

    const __m256i min      = _mm256_set1_epi32(0);
    const __m256i base_sum = _mm256_set1_epi32(64);

    const __m256i vindex_0 = _mm256_set_epi32(
        in_width * 7, in_width * 6, in_width * 5, in_width * 4, in_width * 3, in_width * 2, in_width, 0);
    const __m256i vindex_neg3 = _mm256_set_epi32(in_width * 4, in_width * 3, in_width * 2, in_width, 0, 0, 0, 0);

    const int x1 = filter_len_half;
    int       x3 = (out_height - filter_len_half);
    int       x2 = ((x3 - x1) & ~3) + x1;

    __m128i vec_src[16];

    const int steps = 8;
    int       col;
    for (col = 0; col < (in_width & (~7)); col += steps) {
        const uint16_t* in   = &input[col];
        uint16_t*       optr = &output[col];

        int x = 0;
        // Initial part: write first 'x1' rows to output
        {
            // load 16r x 8c and output 4r x 8c
            highbd_down2_symeven_mm_gather_load_8x8(in, &vec_src[0], vindex_neg3);
            highbd_down2_symeven_mm_gather_load_8x8(in + 5 * in_stride, &vec_src[8], vindex_0);

            highbd_down2_symeven_output_4x8_kernel(vec_src, filter_2x, base_sum, min, max, optr, out_stride);

            optr += steps / 2 * out_stride;
            in += (8 + 5) * in_stride;
        }
        x += x1;

        // Middle part
        assert(x + 4 <= x2);
        assert(in == &input[col + (5 + 8) * in_stride]); // start from row 5
        for (; x < x2; x += 4) // step 'steps/2'
        {
            assign_vec(&vec_src[0], &vec_src[8], steps);
            highbd_down2_symeven_mm_gather_load_8x8(in, &vec_src[8], vindex_0);

            highbd_down2_symeven_output_4x8_kernel(vec_src, filter_2x, base_sum, min, max, optr, out_stride);

            optr += steps / 2 * out_stride;
            in += steps * in_stride; // TODO: 8 rows already loaded by this loop, should step 8 rows instead of 4
        }

        // Middle part b
        if (x2 != x3) {
            // two rows between middle part and end part
            assert(x2 + 2 == x3);
            assert(in == &input[col + (x * 2 + 5) * in_stride]);
            in -= 6 * in_stride;

            __m256i vec_src_256i[16];
            assign_vec_i16_to_i32(&vec_src_256i[0], &vec_src[8], steps);
            highbd_down2_symeven_mm256_gather_load_8x8(in, &vec_src_256i[8], vindex_0);

            highbd_down2_symeven_output_2x8_kernel(vec_src_256i, filter_2x, base_sum, min, max, optr, out_stride);

            optr += 2 * out_stride;

            x += 2;
            in = &input[col + (x * 2 - 3) * in_stride];
            highbd_down2_symeven_mm_gather_load_8x8(in, &vec_src[8], vindex_0);
            in += 8 * in_stride;
        }

        // End part
        assert(x + 4 == out_height);
        assert(in == &input[col + (x * 2 + 5) * in_stride]);
        {
            const __m256i vindex_pos5 = _mm256_set_epi32(
                in_width * 2, in_width * 2, in_width * 2, in_width * 2, in_width * 2, in_width * 2, in_width, 0);
            assign_vec(&vec_src[0], &vec_src[8], steps);
            highbd_down2_symeven_mm_gather_load_8x8(in, &vec_src[8], vindex_pos5);

            highbd_down2_symeven_output_4x8_kernel(vec_src, filter_2x, base_sum, min, max, optr, out_stride);

            optr += steps / 2 * out_stride;
            in += steps * in_stride;
        }
    }

    // up to six columns left
    for (; col < (in_width & (~1)); col += 2) {
        const uint16_t* in   = &input[col];
        uint16_t*       optr = &output[col];
        int             x    = 0;

        // Initial part
        {
            // load 16r x 2c from source and output 4r x 2c by each loop
            highbd_down2_symeven_mm_gather_load_16x2(in, in_stride, vec_src, vindex_neg3, vindex_0, 5);

            highbd_down2_symeven_output_4x2_kernel(vec_src, filter_2x, base_sum, min, max, optr, out_stride);

            optr += steps / 2 * out_stride;
            in += (8 - 3) * in_stride;
        }
        x += steps / 2;

        // Middle part
        for (; x < x2; x += 4) {
            highbd_down2_symeven_mm_gather_load_16x2(in, in_stride, vec_src, vindex_0, vindex_0, 8);

            highbd_down2_symeven_output_4x2_kernel(vec_src, filter_2x, base_sum, min, max, optr, out_stride);

            optr += steps / 2 * out_stride;
            in += steps * in_stride;
        }

        // Middle part b
        if (x2 != x3) {
            // two rows between middle part and end part
            assert(x2 + 2 == x3);
            assert(in == &input[col + (x * 2 - 3) * in_stride]);

            __m256i vec_src_256i[4];
            highbd_down2_symeven_mm256_gather_load_16x2(in, in_stride, vec_src_256i, vindex_0, vindex_0, 2);

            highbd_down2_symeven_output_2x2_kernel(vec_src_256i, filter_2x, base_sum, min, max, optr, out_stride);

            optr += 2 * out_stride;
            in += 4 * in_stride;

            x += 2;
        }

        // End part
        assert(x + 4 == out_height);
        {
            const __m256i vindex_pos5 = _mm256_set_epi32(
                in_width * 2, in_width * 2, in_width * 2, in_width * 2, in_width * 2, in_width * 2, in_width, 0);
            highbd_down2_symeven_mm_gather_load_16x2(in, in_stride, vec_src, vindex_0, vindex_pos5, 8);

            highbd_down2_symeven_output_4x2_kernel(vec_src, filter_2x, base_sum, min, max, optr, out_stride);

            optr += 2 * out_stride;
            in += 4 * in_stride;
        }
    }

    return ret;
}

#define down2_symeven_prepare_4pt highbd_down2_symeven_prepare_4pt
#define down2_symeven_prepare_2pt highbd_down2_symeven_prepare_2pt

static INLINE void down2_symeven_output_4x16_kernel(const __m128i vec_src[32], const __m256i filter_2x,
                                                    const __m256i base_sum, const __m256i min, const __m256i max,
                                                    uint8_t* optr, int32_t out_stride) {
    __m256i vec_4pt[16];
    for (int k = 0; k < 16; ++k) {
        // vec_4pt[k] contains four points to write to the output image in a column,
        // in the form of: P00ab P00cd P01ab P01cd P02ab P02cd P03ab P03cd
        down2_symeven_prepare_4pt(vec_src[k], vec_src[k + 16], filter_2x, &vec_4pt[k]);
    }

    // vec_8pt_i8[k] contains 16 points to write to the output image in a row,
    __m128i vec_8pt_i8[4];
    down2_symeven_obtain_4x16(vec_4pt, base_sum, min, max, vec_8pt_i8);
    for (int k = 0; k < 4; ++k) {
        _mm_storeu_si128((__m128i*)(optr + out_stride * k), vec_8pt_i8[k]);
    }
}

static INLINE void down2_symeven_output_4x4_kernel(const __m128i vec_src[8], const __m256i filter_2x,
                                                   const __m256i base_sum, const __m256i min, const __m256i max,
                                                   uint8_t* optr, int32_t out_stride) {
    __m256i vec_4pt[4];
    for (int k = 0; k < 4; ++k) {
        // vec_4pt[k] contains four points to write to the output image in a column,
        // in the form of: P00ab P00cd P01ab P01cd P02ab P02cd P03ab P03cd
        down2_symeven_prepare_4pt(vec_src[k], vec_src[k + 4], filter_2x, &vec_4pt[k]);
    }

    __m128i vec_16pt_i8;
    down2_symeven_obtain_4x4(vec_4pt, base_sum, min, max, &vec_16pt_i8);

    *(uint32_t*)(optr + 0)              = _mm_extract_epi32(vec_16pt_i8, 0); // write 4x 8-bit
    *(uint32_t*)(optr + out_stride)     = _mm_extract_epi32(vec_16pt_i8, 1);
    *(uint32_t*)(optr + out_stride * 2) = _mm_extract_epi32(vec_16pt_i8, 2);
    *(uint32_t*)(optr + out_stride * 3) = _mm_extract_epi32(vec_16pt_i8, 3);
}

static INLINE void down2_symeven_output_2x16_kernel(const __m256i vec_src[32], const __m256i filter_2x,
                                                    const __m256i base_sum, const __m256i min, const __m256i max,
                                                    uint8_t* optr, int32_t out_stride) {
    // vec_2pt[k] contains two points to write to the output image in a column
    __m256i vec_2pt[16];
    for (int k = 0; k < 16; ++k) {
        // v0: P00: a b c d  P01: a b c d
        // v1: P02: a b c d  P03: a b c d
        // v2: P04: a b c d  P05: a b c d
        // v3: P06: a b c d  P07: a b c d
        // v4: P08: a b c d  P09: a b c d
        // v5: P10: a b c d  P11: a b c d
        // v6: P12: a b c d  P13: a b c d
        // v7: P14: a b c d  P15: a b c d
        down2_symeven_prepare_2pt(vec_src[k], vec_src[k + 16], filter_2x, &vec_2pt[k]);
    }

    __m256i vec_8pt[4];
    for (int k = 0; k < 4; ++k) {
        __m256i vec_4pt_a = _mm256_hadd_epi32(vec_2pt[k * 4], vec_2pt[k * 4 + 1]);
        __m256i vec_4pt_b = _mm256_hadd_epi32(vec_2pt[k * 4 + 2], vec_2pt[k * 4 + 3]);
        // P00 P02 P04 P06  P01 P03 P05 P07
        // P08 P10 P12 P14  P09 P11 P13 P15
        // P16 P18 P20 P22  P17 P19 P21 P23
        // P24 P26 P28 P30  P25 P27 P29 P31
        vec_8pt[k] = _mm256_hadd_epi32(vec_4pt_a, vec_4pt_b);
        vec_8pt[k] = _mm256_add_epi32(vec_8pt[k], base_sum);
        vec_8pt[k] = _mm256_srai_epi32(vec_8pt[k], FILTER_BITS);
        // now only the lowest 8-bit of each 32-bit integer is non zero
        mm256_clamp_epi32(&vec_8pt[k], min, max);
    }

    // combine two 8x 32-bit vectors to one 16x 16-bit vector
    // P00 P02 P04 P06 P08 P10 P12 P14   P01 P03 P05 P07 P09 P11 P13 P15
    __m256i vec_16pt_a = _mm256_packus_epi32(vec_8pt[0], vec_8pt[1]);
    // P16 P18 P20 P22 P24 P26 P28 P30   P17 P19 P21 P23 P25 P27 P29 P31
    __m256i vec_16pt_b = _mm256_packus_epi32(vec_8pt[2], vec_8pt[3]);

    // two 16x 16-bit vectors => one 32x 8-bit vector
    __m256i vec_32pt = _mm256_packus_epi16(vec_16pt_a, vec_16pt_b);

    _mm_storeu_si128((__m128i*)(optr), _mm256_castsi256_si128(vec_32pt));
    _mm_storeu_si128((__m128i*)(optr + out_stride), _mm256_extracti128_si256(vec_32pt, 1));
}

static INLINE void down2_symeven_output_2x4_kernel(const __m256i vec_src[8], const __m256i filter_2x,
                                                   const __m256i base_sum, const __m256i min, const __m256i max,
                                                   uint8_t* optr, int32_t out_stride) {
    // vec_2pt[k] contains two points to write to the output image in a column
    __m256i vec_2pt[4];
    for (int k = 0; k < 4; ++k) {
        // v0: P00: a b c d  P01: a b c d
        // v1: P02: a b c d  P03: a b c d
        // v2: P04: a b c d  P05: a b c d
        // v3: P06: a b c d  P07: a b c d
        down2_symeven_prepare_2pt(vec_src[k], vec_src[k + 4], filter_2x, &vec_2pt[k]);
    }

    __m256i vec_4pt_a = _mm256_hadd_epi32(vec_2pt[0], vec_2pt[1]);
    __m256i vec_4pt_b = _mm256_hadd_epi32(vec_2pt[2], vec_2pt[3]);
    // P00 P02 P04 P06  P01 P03 P05 P07
    __m256i vec_8pt = _mm256_hadd_epi32(vec_4pt_a, vec_4pt_b);
    vec_8pt         = _mm256_add_epi32(vec_8pt, base_sum);
    vec_8pt         = _mm256_srai_epi32(vec_8pt, FILTER_BITS);
    // now only the lowest 8-bit of each 32-bit integer is non zero
    mm256_clamp_epi32(&vec_8pt, min, max);

    // 8x 32bit => 16x 16-bit: P00 P02 P04 P06 xx xx xx xx    P01 P03 P05 P07 xx xx xx xx
    vec_8pt = _mm256_packus_epi32(vec_8pt, min);
    // 16x 16bit => 32x 8-bit: P00 P02 P04 P06 xx... xx xx    P01 P03 P05 P07 xx... xx xx
    vec_8pt = _mm256_packus_epi16(vec_8pt, min);

    *(uint32_t*)(optr + 0)          = _mm256_extract_epi32(vec_8pt, 0);
    *(uint32_t*)(optr + out_stride) = _mm256_extract_epi32(vec_8pt, 4);
}

static void fill_arr_to_col(uint8_t* img, int stride, int len, const uint8_t* arr) {
    int            i;
    uint8_t*       iptr = img;
    const uint8_t* aptr = arr;
    for (i = 0; i < len; ++i, iptr += stride) {
        *iptr = *aptr++;
    }
}

static void fill_col_to_arr(const uint8_t* img, int stride, int len, uint8_t* arr) {
    int            i;
    const uint8_t* iptr = img;
    uint8_t*       aptr = arr;
    for (i = 0; i < len; ++i, iptr += stride) {
        *aptr++ = *iptr;
    }
}

static EbErrorType svt_av1_down2_symeven_col_avx2(const uint8_t* const input, int in_width, int in_height,
                                                  int in_stride, uint8_t* output, int out_stride) {
    EbErrorType ret = EB_ErrorNone;

    const int      out_height      = in_height / 2;
    const int16_t* filter          = svt_aom_av1_down2_symeven_half_filter;
    const int      filter_len_half = sizeof(svt_aom_av1_down2_symeven_half_filter) / 2;
    const __m256i  max             = _mm256_set1_epi32(255);

    const __m128i filter_2x_128i = _mm_broadcastq_epi64(_mm_loadl_epi64((const __m128i*)filter));
    const __m256i filter_2x      = _mm256_cvtepi16_epi32(filter_2x_128i);

    const __m256i min      = _mm256_set1_epi32(0);
    const __m256i base_sum = _mm256_set1_epi32(64);

    const __m256i vindex_0 = _mm256_set_epi32(
        in_width * 7, in_width * 6, in_width * 5, in_width * 4, in_width * 3, in_width * 2, in_width, 0);
    const __m256i vindex_neg3 = _mm256_set_epi32(in_width * 4, in_width * 3, in_width * 2, in_width, 0, 0, 0, 0);

    const uint8_t* in   = NULL;
    uint8_t*       optr = NULL;

    const int x1 = filter_len_half;
    int       x3 = (out_height - filter_len_half);
    int       x2 = ((x3 - x1) & ~3) + x1;

#define down2_symeven_mm_gather_load_8x16 interpolate_gather_load_8x16
    const int steps = 16;
    __m128i   vec_src[32];
    int       col;
    for (col = 0; col < (in_width & (~15)); col += steps) {
        in   = &input[col];
        optr = &output[col];

        int x = 0;
        // Initial part: write first 'x1' rows to output
        {
            // load 16r x 16c and output 4r x 16c
            down2_symeven_mm_gather_load_8x16(in, vindex_neg3, &vec_src[0]);
            down2_symeven_mm_gather_load_8x16(in + 5 * in_stride, vindex_0, &vec_src[16]);

            down2_symeven_output_4x16_kernel(vec_src, filter_2x, base_sum, min, max, optr, out_stride);

            optr += 4 * out_stride;
            in += (8 + 5) * in_stride;
        }
        x += x1;

        // Middle part
        assert(x + 4 <= x2);
        assert(in == &input[col + (5 + 8) * in_stride]); // start from row 5
        for (; x < x2; x += 4) {
            assign_vec(&vec_src[0], &vec_src[16], 16);
            down2_symeven_mm_gather_load_8x16(in, vindex_0, &vec_src[16]);

            down2_symeven_output_4x16_kernel(vec_src, filter_2x, base_sum, min, max, optr, out_stride);

            optr += 4 * out_stride;
            in += 8 * in_stride;
        }

        // Middle part b: two rows between middle part and end part
        if (x2 != x3) {
            assert(x2 + 2 == x3);
            assert(in == &input[col + (x * 2 + 5) * in_stride]);
            in -= 6 * in_stride;

            __m256i vec_src_256i[32];
            assign_vec_i16_to_i32(&vec_src_256i[0], &vec_src[16], 16);
            down2_symeven_mm_gather_load_8x16(in, vindex_0, &vec_src[16]);
            assign_vec_i16_to_i32(&vec_src_256i[16], &vec_src[16], 16);

            down2_symeven_output_2x16_kernel(vec_src_256i, filter_2x, base_sum, min, max, optr, out_stride);

            optr += 2 * out_stride;

            x += 2;
            in = &input[col + (x * 2 - 3) * in_stride];
            down2_symeven_mm_gather_load_8x16(in, vindex_0, &vec_src[16]);
            in += 8 * in_stride;
        }

        // End part
        assert(x + 4 == out_height);
        assert(in == &input[col + (x * 2 + 5) * in_stride]);
        {
            const __m256i vindex_pos5 = _mm256_set_epi32(
                in_width * 2, in_width * 2, in_width * 2, in_width * 2, in_width * 2, in_width * 2, in_width, 0);
            assign_vec(&vec_src[0], &vec_src[16], 16);
            down2_symeven_mm_gather_load_8x16(in, vindex_pos5, &vec_src[16]);

            down2_symeven_output_4x16_kernel(vec_src, filter_2x, base_sum, min, max, optr, out_stride);

            optr += 4 * out_stride;
            in += 8 * in_stride;
        }
    }

    // Up to fourteen columns left. Process four columns by each loop because gather load loads 32-bit in a row
    for (; col + 4 <= in_width; col += 4) {
        in   = &input[col];
        optr = &output[col];

        int x = 0;

        // Initial part
        {
            // load 16r x 4c from source. output 4r x 4c. 4c <= 32bit consists of 4x 8bit
            down2_symeven_gather_load_8x4(in, vindex_neg3, &vec_src[0]); // fill in vec_src[0..3]
            down2_symeven_gather_load_8x4(in + 5 * in_stride, vindex_0, &vec_src[4]); // fill in vec_src[4..7]

            down2_symeven_output_4x4_kernel(vec_src, filter_2x, base_sum, min, max, optr, out_stride);

            optr += 4 * out_stride; // steps 4 rows
            in += (5 + 8) * in_stride; // steps (8-3+8) rows
        }
        x += 4;

        // Middle part
        for (; x < x2; x += 4) {
            // assign vec_src[4..7] to vec_src[0..3]
            assign_vec(&vec_src[0], &vec_src[4], 4);
            down2_symeven_gather_load_8x4(in, vindex_0, &vec_src[4]); // fill in vec_src[4..7]

            down2_symeven_output_4x4_kernel(vec_src, filter_2x, base_sum, min, max, optr, out_stride);

            optr += 4 * out_stride; // steps 4 rows
            in += 8 * in_stride; // steps 8 rows
        }

        // Middle part b
        if (x2 != x3) {
            // two rows between middle part and end part
            assert(x2 + 2 == x3);
            assert(in == &input[col + (x * 2 + 5) * in_stride]);

            in -= 6 * in_stride;

            __m256i vec_src_256i[8];
            assign_vec_i16_to_i32(&vec_src_256i[0], &vec_src[4], 4);
            down2_symeven_gather_load_8x4(in, vindex_0, &vec_src[4]);
            assign_vec_i16_to_i32(&vec_src_256i[4], &vec_src[4], 4);

            down2_symeven_output_2x4_kernel(vec_src_256i, filter_2x, base_sum, min, max, optr, out_stride);

            optr += 2 * out_stride;

            x += 2;
            in = &input[col + (x * 2 - 3) * in_stride];
            down2_symeven_gather_load_8x4(in, vindex_0, &vec_src[4]);
            in += 8 * in_stride;
        }

        // End part
        assert(x + 4 == out_height);
        {
            const __m256i vindex_pos5 = _mm256_set_epi32(
                in_width * 2, in_width * 2, in_width * 2, in_width * 2, in_width * 2, in_width * 2, in_width, 0);
            assign_vec(&vec_src[0], &vec_src[4], 4);
            down2_symeven_gather_load_8x4(in, vindex_pos5, &vec_src[4]);

            down2_symeven_output_4x4_kernel(vec_src, filter_2x, base_sum, min, max, optr, out_stride);

            optr += 4 * out_stride; // steps 4 rows
            in += 8 * in_stride; // steps 8 rows
        }
    }
    // up to two columns left
    if (col < in_width) {
        uint8_t *arrbuf, *arrbuf2;
        EB_MALLOC_ARRAY(arrbuf, in_height);
        EB_MALLOC_ARRAY(arrbuf2, out_height);
        if (arrbuf == NULL || arrbuf2 == NULL) {
            EB_FREE_ARRAY(arrbuf);
            EB_FREE_ARRAY(arrbuf2);
            return EB_ErrorInsufficientResources;
        }

        // workaround for gcc-12 error: 'malloced_p' may be used uninitialized
        arrbuf[0] = 0;

        for (; col < in_width; ++col) {
            in   = &input[col];
            optr = &output[col];

            fill_col_to_arr(in, in_stride, in_height, arrbuf);
            svt_av1_down2_symeven_avx2(arrbuf, in_height, arrbuf2);
            fill_arr_to_col(optr, out_stride, out_height, arrbuf2);
        }

        EB_FREE_ARRAY(arrbuf);
        EB_FREE_ARRAY(arrbuf2);
    }
    return ret;
}

static int get_down2_length(int length, int steps) {
    for (int s = 0; s < steps; ++s) {
        length = (length + 1) >> 1;
    }
    return length;
}

static int get_down2_steps(int in_length, int out_length) {
    int steps = 0;
    int proj_in_length;
    while ((proj_in_length = get_down2_length(in_length, 1)) >= out_length) {
        ++steps;
        in_length = proj_in_length;
        if (in_length == 1) {
            // Special case: we break because any further calls to get_down2_length()
            // with be with length == 1, which return 1, resulting in an infinite
            // loop.
            break;
        }
    }
    return steps;
}

static const InterpKernel* choose_interp_filter(int in_length, int out_length) {
    int out_length16 = out_length * 16;
    if (out_length16 >= in_length * 16) {
        return filteredinterp_filters1000;
    } else if (out_length16 >= in_length * 13) {
        return svt_aom_av1_filteredinterp_filters875;
    } else if (out_length16 >= in_length * 11) {
        return svt_aom_av1_filteredinterp_filters750;
    } else if (out_length16 >= in_length * 9) {
        return svt_aom_av1_filteredinterp_filters625;
    } else {
        return svt_aom_av1_filteredinterp_filters500;
    }
}

static EbErrorType highbd_resize_multistep(const uint16_t* const input, int length, uint16_t* output, int olength,
                                           int bd) {
    const int steps = get_down2_steps(length, olength);

    if (steps > 0) {
        // downscale 2x or more
        uint16_t* output_tmp     = NULL;
        uint16_t* out            = NULL;
        int       filteredlength = length;

        for (int s = 0; s < steps; ++s) {
            const int             proj_filteredlength = get_down2_length(filteredlength, 1);
            const uint16_t* const in                  = (s == 0 ? input : out);
            if (s == steps - 1 && proj_filteredlength == olength) {
                out = output;
            } else {
                if (output_tmp == NULL) {
                    EB_MALLOC_ARRAY(output_tmp, length * filteredlength);
                    if (output_tmp == NULL) {
                        return EB_ErrorInsufficientResources;
                    }
                }
                out = output_tmp;
            }
            if (filteredlength & 1) {
                assert(0);
            } else {
                svt_av1_highbd_down2_symeven_avx2(in, filteredlength, out, bd);
            }
            filteredlength = proj_filteredlength;
        }
        if (filteredlength != olength) {
            const InterpKernel* interp_filters = choose_interp_filter(filteredlength, olength);
            svt_av1_highbd_interpolate_core_avx2(out, filteredlength, output, olength, bd, &interp_filters[0][0]);
        }

        if (output_tmp != NULL) {
            EB_FREE_ARRAY(output_tmp);
        }
    } else {
        const InterpKernel* interp_filters = choose_interp_filter(length, olength);
        svt_av1_highbd_interpolate_core_avx2(input, length, output, olength, bd, &interp_filters[0][0]);
    }

    return EB_ErrorNone;
}

static EbErrorType highbd_resize_multistep_vertical(const uint16_t* const input, int in_width, int in_height,
                                                    int in_stride, uint16_t* output, int out_height, int out_stride,
                                                    int bd) {
    const int steps = get_down2_steps(in_height, out_height);

    if (steps > 0) {
        // downscale 2x or more
        uint16_t* output_tmp     = NULL;
        uint16_t* out            = NULL;
        int       filteredlength = in_height;

        for (int s = 0; s < steps; ++s) {
            const int             proj_filteredlength = get_down2_length(filteredlength, 1);
            const uint16_t* const in                  = (s == 0 ? input : out);
            if (s == steps - 1 && proj_filteredlength == out_height) {
                out = output;
            } else {
                if (output_tmp == NULL) {
                    EB_MALLOC_ARRAY(output_tmp, in_width * filteredlength);
                    if (output_tmp == NULL) {
                        return EB_ErrorInsufficientResources;
                    }
                }
                out = output_tmp;
            }
            if (filteredlength & 1) {
                assert(0); // inactive code path
            } else {
                svt_av1_highbd_down2_symeven_col_avx2(in, in_width, in_height, in_stride, out, out_stride, bd);
            }
            filteredlength = proj_filteredlength;
        }
        if (filteredlength != out_height) {
            const InterpKernel* interp_filters = choose_interp_filter(filteredlength, out_height);
            svt_av1_highbd_interpolate_core_col_avx2(out,
                                                     in_width,
                                                     filteredlength,
                                                     filteredlength,
                                                     output,
                                                     out_height,
                                                     out_stride,
                                                     bd,
                                                     &interp_filters[0][0]);
        }

        if (output_tmp != NULL) {
            EB_FREE_ARRAY(output_tmp);
        }
    } else {
        const InterpKernel* interp_filters = choose_interp_filter(in_height, out_height);
        svt_av1_highbd_interpolate_core_col_avx2(
            input, in_width, in_height, in_stride, output, out_height, out_stride, bd, &interp_filters[0][0]);
    }

    return EB_ErrorNone;
}

EbErrorType svt_av1_highbd_resize_plane_avx2(const uint16_t* const input, int in_height, int in_width, int in_stride,
                                             uint16_t* output, int out_height, int out_width, int out_stride, int bd) {
    EbErrorType ret = EB_ErrorNone;
    uint16_t*   intbuf;

    EB_MALLOC_ARRAY(intbuf, out_width * in_height);
    if (intbuf == NULL) {
        EB_FREE_ARRAY(intbuf);
        return EB_ErrorInsufficientResources;
    }

    for (int i = 0; i < in_height; ++i) {
        highbd_resize_multistep(input + in_stride * i, in_width, intbuf + out_width * i, out_width, bd);
    }

    highbd_resize_multistep_vertical(intbuf, out_width, in_height, out_width, output, out_height, out_stride, bd);

    /*FILE *file = fopen("dump.yuv", "wb");
    if (file) {
        fwrite(output, out_stride * out_height, 1, file);
        fclose(file);
    }*/
    EB_FREE_ARRAY(intbuf);
    return ret;
}

static EbErrorType resize_multistep(const uint8_t* const input, int length, uint8_t* output, int olength) {
    const int steps = get_down2_steps(length, olength);

    if (steps > 0) {
        // downscale 2x or more
        uint8_t* output_tmp     = NULL;
        uint8_t* out            = NULL;
        int      filteredlength = length;

        for (int s = 0; s < steps; ++s) {
            const int            proj_filteredlength = get_down2_length(filteredlength, 1);
            const uint8_t* const in                  = (s == 0 ? input : out);
            if (s == steps - 1 && proj_filteredlength == olength) {
                out = output;
            } else {
                if (output_tmp == NULL) {
                    EB_MALLOC_ARRAY(output_tmp, length * filteredlength);
                    if (output_tmp == NULL) {
                        return EB_ErrorInsufficientResources;
                    }
                }
                out = output_tmp;
            }
            if (filteredlength & 1) {
                assert(0);
            } else {
                svt_av1_down2_symeven_avx2(in, filteredlength, out);
            }
            filteredlength = proj_filteredlength;
        }
        if (filteredlength != olength) {
            const InterpKernel* interp_filters = choose_interp_filter(filteredlength, olength);
            svt_av1_interpolate_core_avx2(out, filteredlength, output, olength, &interp_filters[0][0]);
        }

        if (output_tmp != NULL) {
            EB_FREE_ARRAY(output_tmp);
        }
    } else {
        const InterpKernel* interp_filters = choose_interp_filter(length, olength);
        svt_av1_interpolate_core_avx2(input, length, output, olength, &interp_filters[0][0]);
    }

    return EB_ErrorNone;
}

static EbErrorType resize_multistep_vertical(const uint8_t* const input, int in_width, int in_height, int in_stride,
                                             uint8_t* output, int out_height, int out_stride) {
    const int steps = get_down2_steps(in_height, out_height);

    if (steps > 0) {
        // downscale 2x or more
        uint8_t* output_tmp     = NULL;
        uint8_t* out            = NULL;
        int      filteredlength = in_height;

        for (int s = 0; s < steps; ++s) {
            const int            proj_filteredlength = get_down2_length(filteredlength, 1);
            const uint8_t* const in                  = (s == 0 ? input : out);
            if (s == steps - 1 && proj_filteredlength == out_height) {
                out = output;
            } else {
                if (output_tmp == NULL) {
                    EB_MALLOC_ARRAY(output_tmp, in_width * filteredlength);
                    if (output_tmp == NULL) {
                        return EB_ErrorInsufficientResources;
                    }
                }
                out = output_tmp;
            }
            if (filteredlength & 1) {
                assert(0); // inactive code path
            } else {
                svt_av1_down2_symeven_col_avx2(in, in_width, in_height, in_stride, out, out_stride);
            }
            filteredlength = proj_filteredlength;
        }
        if (filteredlength != out_height) {
            const InterpKernel* interp_filters = choose_interp_filter(filteredlength, out_height);
            svt_av1_interpolate_core_col_avx2(
                out, in_width, filteredlength, filteredlength, output, out_height, out_stride, &interp_filters[0][0]);
        }

        if (output_tmp != NULL) {
            EB_FREE_ARRAY(output_tmp);
        }
    } else {
        const InterpKernel* interp_filters = choose_interp_filter(in_height, out_height);
        svt_av1_interpolate_core_col_avx2(
            input, in_width, in_height, in_stride, output, out_height, out_stride, &interp_filters[0][0]);
    }

    return EB_ErrorNone;
}

EbErrorType svt_av1_resize_plane_avx2(const uint8_t* const input, int in_height, int in_width, int in_stride,
                                      uint8_t* output, int out_height, int out_width, int out_stride) {
    EbErrorType ret = EB_ErrorNone;
    uint8_t*    intbuf;

    EB_MALLOC_ARRAY(intbuf, out_width * in_height);
    if (intbuf == NULL) {
        EB_FREE_ARRAY(intbuf);
        return EB_ErrorInsufficientResources;
    }

    for (int i = 0; i < in_height; ++i) {
        resize_multistep(input + in_stride * i, in_width, intbuf + out_width * i, out_width);
    }

    resize_multistep_vertical(intbuf, out_width, in_height, out_width, output, out_height, out_stride);

    /*FILE *file = fopen("dump.yuv", "wb");
    if (file) {
        fwrite(output, out_stride * out_height, 1, file);
        fclose(file);
    }*/
    EB_FREE_ARRAY(intbuf);
    return ret;
}
