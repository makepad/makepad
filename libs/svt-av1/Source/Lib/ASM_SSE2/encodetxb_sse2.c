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

#include <assert.h>
#include <emmintrin.h> // SSE2
#include <stdint.h>
#include "definitions.h"
#include "cabac_context_model.h"
#include "common_utils.h"
#include "full_loop.h"

static INLINE __m128i loadh_epi64(const void* const src, const __m128i s) {
    return _mm_castps_si128(_mm_loadh_pi(_mm_castsi128_ps(s), (const __m64*)src));
}

static INLINE __m128i load_8bit_4x4_to_1_reg_sse2(const void* const src, const int32_t byte_stride) {
    return _mm_setr_epi32(*(const int32_t*)((int8_t*)src + 0 * byte_stride),
                          *(const int32_t*)((int8_t*)src + 1 * byte_stride),
                          *(const int32_t*)((int8_t*)src + 2 * byte_stride),
                          *(const int32_t*)((int8_t*)src + 3 * byte_stride));
}

static INLINE __m128i load_8bit_8x2_to_1_reg_sse2(const void* const src, const int32_t byte_stride) {
    __m128i dst;
    dst = _mm_loadl_epi64((__m128i*)((int8_t*)src + 0 * byte_stride));
    dst = loadh_epi64((int8_t*)src + 1 * byte_stride, dst);
    return dst;
}

static INLINE void load_levels_4x4x5_sse2(const uint8_t* const src, const int32_t stride,
                                          const ptrdiff_t* const offsets, __m128i* const level) {
    level[0] = load_8bit_4x4_to_1_reg_sse2(src + 1, stride);
    level[1] = load_8bit_4x4_to_1_reg_sse2(src + stride, stride);
    level[2] = load_8bit_4x4_to_1_reg_sse2(src + offsets[0], stride);
    level[3] = load_8bit_4x4_to_1_reg_sse2(src + offsets[1], stride);
    level[4] = load_8bit_4x4_to_1_reg_sse2(src + offsets[2], stride);
}

static INLINE void load_levels_8x2x5_sse2(const uint8_t* const src, const int32_t stride,
                                          const ptrdiff_t* const offsets, __m128i* const level) {
    level[0] = load_8bit_8x2_to_1_reg_sse2(src + 1, stride);
    level[1] = load_8bit_8x2_to_1_reg_sse2(src + stride, stride);
    level[2] = load_8bit_8x2_to_1_reg_sse2(src + offsets[0], stride);
    level[3] = load_8bit_8x2_to_1_reg_sse2(src + offsets[1], stride);
    level[4] = load_8bit_8x2_to_1_reg_sse2(src + offsets[2], stride);
}

static INLINE void load_levels_16x1x5_sse2(const uint8_t* const src, const int32_t stride,
                                           const ptrdiff_t* const offsets, __m128i* const level) {
    level[0] = _mm_loadu_si128((__m128i*)(src + 1));
    level[1] = _mm_loadu_si128((__m128i*)(src + stride));
    level[2] = _mm_loadu_si128((__m128i*)(src + offsets[0]));
    level[3] = _mm_loadu_si128((__m128i*)(src + offsets[1]));
    level[4] = _mm_loadu_si128((__m128i*)(src + offsets[2]));
}

static INLINE __m128i get_coeff_contexts_kernel_sse2(__m128i* const level) {
    const __m128i const_3 = _mm_set1_epi8(3);
    const __m128i const_4 = _mm_set1_epi8(4);
    __m128i       count;

    count    = _mm_min_epu8(level[0], const_3);
    level[1] = _mm_min_epu8(level[1], const_3);
    level[2] = _mm_min_epu8(level[2], const_3);
    level[3] = _mm_min_epu8(level[3], const_3);
    level[4] = _mm_min_epu8(level[4], const_3);
    count    = _mm_add_epi8(count, level[1]);
    count    = _mm_add_epi8(count, level[2]);
    count    = _mm_add_epi8(count, level[3]);
    count    = _mm_add_epi8(count, level[4]);
    count    = _mm_avg_epu8(count, _mm_setzero_si128());
    count    = _mm_min_epu8(count, const_4);
    return count;
}

static INLINE void get_4_nz_map_contexts_2d(const uint8_t* levels, const int32_t height, const ptrdiff_t* const offsets,
                                            int8_t* const coeff_contexts) {
    const int32_t stride              = 4 + TX_PAD_HOR;
    const __m128i pos_to_offset_large = _mm_set1_epi8(21);
    __m128i       pos_to_offset = (height == 4) ? _mm_setr_epi8(0, 1, 6, 6, 1, 6, 6, 21, 6, 6, 21, 21, 6, 21, 21, 21)
                                                : _mm_setr_epi8(0, 11, 11, 11, 11, 11, 11, 11, 6, 6, 21, 21, 6, 21, 21, 21);
    __m128i       count;
    __m128i       level[5];
    int8_t*       cc  = coeff_contexts;
    int32_t       row = height;

    assert(!(height % 4));

    do {
        load_levels_4x4x5_sse2(levels, stride, offsets, level);
        count = get_coeff_contexts_kernel_sse2(level);
        count = _mm_add_epi8(count, pos_to_offset);
        _mm_storeu_si128((__m128i*)cc, count);
        pos_to_offset = pos_to_offset_large;
        levels += 4 * stride;
        cc += 16;
        row -= 4;
    } while (row);

    coeff_contexts[0] = 0;
}

static INLINE void get_8_coeff_contexts_2d(const uint8_t* levels, const int32_t height, const ptrdiff_t* const offsets,
                                           int8_t* coeff_contexts) {
    const int32_t stride = 8 + TX_PAD_HOR;
    int8_t*       cc     = coeff_contexts;
    int32_t       row    = height;
    __m128i       count;
    __m128i       level[5];
    __m128i       pos_to_offset[3];

    assert(!(height % 2));

    if (height == 8) {
        pos_to_offset[0] = _mm_setr_epi8(0, 1, 6, 6, 21, 21, 21, 21, 1, 6, 6, 21, 21, 21, 21, 21);
        pos_to_offset[1] = _mm_setr_epi8(6, 6, 21, 21, 21, 21, 21, 21, 6, 21, 21, 21, 21, 21, 21, 21);
    } else if (height < 8) {
        pos_to_offset[0] = _mm_setr_epi8(0, 16, 6, 6, 21, 21, 21, 21, 16, 16, 6, 21, 21, 21, 21, 21);
        pos_to_offset[1] = _mm_setr_epi8(16, 16, 21, 21, 21, 21, 21, 21, 16, 16, 21, 21, 21, 21, 21, 21);
    } else {
        pos_to_offset[0] = _mm_setr_epi8(0, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11);
        pos_to_offset[1] = _mm_setr_epi8(6, 6, 21, 21, 21, 21, 21, 21, 6, 21, 21, 21, 21, 21, 21, 21);
    }
    pos_to_offset[2] = _mm_set1_epi8(21);

    do {
        load_levels_8x2x5_sse2(levels, stride, offsets, level);
        count = get_coeff_contexts_kernel_sse2(level);
        count = _mm_add_epi8(count, pos_to_offset[0]);
        _mm_storeu_si128((__m128i*)cc, count);
        pos_to_offset[0] = pos_to_offset[1];
        pos_to_offset[1] = pos_to_offset[2];
        levels += 2 * stride;
        cc += 16;
        row -= 2;
    } while (row);

    coeff_contexts[0] = 0;
}

static INLINE void get_16n_coeff_contexts_2d(const uint8_t* levels, const int32_t real_width, const int32_t real_height,
                                             const int32_t width, const int32_t height, const ptrdiff_t* const offsets,
                                             int8_t* coeff_contexts) {
    const int32_t stride = width + TX_PAD_HOR;
    int8_t*       cc     = coeff_contexts;
    int32_t       row    = height;
    __m128i       pos_to_offset[5];
    __m128i       pos_to_offset_large[3];
    __m128i       count;
    __m128i       level[5];

    assert(!(width % 16));

    pos_to_offset_large[2] = _mm_set1_epi8(21);
    if (real_width == real_height) {
        pos_to_offset[0] = _mm_setr_epi8(0, 1, 6, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21);
        pos_to_offset[1] = _mm_setr_epi8(1, 6, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21);
        pos_to_offset[2] = _mm_setr_epi8(6, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21);
        pos_to_offset[3] = _mm_setr_epi8(6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21);
        pos_to_offset[4] = pos_to_offset_large[0] = pos_to_offset_large[1] = pos_to_offset_large[2];
    } else if (real_width > real_height) {
        pos_to_offset[0] = _mm_setr_epi8(0, 16, 6, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21);
        pos_to_offset[1] = _mm_setr_epi8(16, 16, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21);
        pos_to_offset[2] = pos_to_offset[3] = pos_to_offset[4] = _mm_setr_epi8(
            16, 16, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21);
        pos_to_offset_large[0] = pos_to_offset_large[1] = pos_to_offset_large[2];
    } else { // real_width < real_height
        pos_to_offset[0] = pos_to_offset[1] = _mm_setr_epi8(
            11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11);
        pos_to_offset[2]       = _mm_setr_epi8(6, 6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21);
        pos_to_offset[3]       = _mm_setr_epi8(6, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21);
        pos_to_offset[4]       = pos_to_offset_large[2];
        pos_to_offset_large[0] = pos_to_offset_large[1] = _mm_set1_epi8(11);
    }

    do {
        int32_t w = width;

        do {
            load_levels_16x1x5_sse2(levels, stride, offsets, level);
            count = get_coeff_contexts_kernel_sse2(level);
            count = _mm_add_epi8(count, pos_to_offset[0]);
            _mm_storeu_si128((__m128i*)cc, count);
            levels += 16;
            cc += 16;
            w -= 16;
            pos_to_offset[0] = pos_to_offset_large[0];
        } while (w);

        pos_to_offset[0]       = pos_to_offset[1];
        pos_to_offset[1]       = pos_to_offset[2];
        pos_to_offset[2]       = pos_to_offset[3];
        pos_to_offset[3]       = pos_to_offset[4];
        pos_to_offset_large[0] = pos_to_offset_large[1];
        pos_to_offset_large[1] = pos_to_offset_large[2];
        levels += TX_PAD_HOR;
    } while (--row);

    coeff_contexts[0] = 0;
}

static INLINE void get_4_nz_map_contexts_hor(const uint8_t* levels, const int32_t height,
                                             const ptrdiff_t* const offsets, int8_t* coeff_contexts) {
    const int32_t stride        = 4 + TX_PAD_HOR;
    const __m128i pos_to_offset = _mm_setr_epi8(SIG_COEF_CONTEXTS_2D + 0,
                                                SIG_COEF_CONTEXTS_2D + 5,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 0,
                                                SIG_COEF_CONTEXTS_2D + 5,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 0,
                                                SIG_COEF_CONTEXTS_2D + 5,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 0,
                                                SIG_COEF_CONTEXTS_2D + 5,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10);
    __m128i       count;
    __m128i       level[5];
    int32_t       row = height;

    assert(!(height % 4));

    do {
        load_levels_4x4x5_sse2(levels, stride, offsets, level);
        count = get_coeff_contexts_kernel_sse2(level);
        count = _mm_add_epi8(count, pos_to_offset);
        _mm_storeu_si128((__m128i*)coeff_contexts, count);
        levels += 4 * stride;
        coeff_contexts += 16;
        row -= 4;
    } while (row);
}

static INLINE void get_4_nz_map_contexts_ver(const uint8_t* levels, const int32_t height,
                                             const ptrdiff_t* const offsets, int8_t* coeff_contexts) {
    const int32_t stride              = 4 + TX_PAD_HOR;
    const __m128i pos_to_offset_large = _mm_set1_epi8(SIG_COEF_CONTEXTS_2D + 10);
    __m128i       pos_to_offset       = _mm_setr_epi8(SIG_COEF_CONTEXTS_2D + 0,
                                          SIG_COEF_CONTEXTS_2D + 0,
                                          SIG_COEF_CONTEXTS_2D + 0,
                                          SIG_COEF_CONTEXTS_2D + 0,
                                          SIG_COEF_CONTEXTS_2D + 5,
                                          SIG_COEF_CONTEXTS_2D + 5,
                                          SIG_COEF_CONTEXTS_2D + 5,
                                          SIG_COEF_CONTEXTS_2D + 5,
                                          SIG_COEF_CONTEXTS_2D + 10,
                                          SIG_COEF_CONTEXTS_2D + 10,
                                          SIG_COEF_CONTEXTS_2D + 10,
                                          SIG_COEF_CONTEXTS_2D + 10,
                                          SIG_COEF_CONTEXTS_2D + 10,
                                          SIG_COEF_CONTEXTS_2D + 10,
                                          SIG_COEF_CONTEXTS_2D + 10,
                                          SIG_COEF_CONTEXTS_2D + 10);
    __m128i       count;
    __m128i       level[5];
    int32_t       row = height;

    assert(!(height % 4));

    do {
        load_levels_4x4x5_sse2(levels, stride, offsets, level);
        count = get_coeff_contexts_kernel_sse2(level);
        count = _mm_add_epi8(count, pos_to_offset);
        _mm_storeu_si128((__m128i*)coeff_contexts, count);
        pos_to_offset = pos_to_offset_large;
        levels += 4 * stride;
        coeff_contexts += 16;
        row -= 4;
    } while (row);
}

static INLINE void get_8_coeff_contexts_hor(const uint8_t* levels, const int32_t height, const ptrdiff_t* const offsets,
                                            int8_t* coeff_contexts) {
    const int32_t stride        = 8 + TX_PAD_HOR;
    const __m128i pos_to_offset = _mm_setr_epi8(SIG_COEF_CONTEXTS_2D + 0,
                                                SIG_COEF_CONTEXTS_2D + 5,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 0,
                                                SIG_COEF_CONTEXTS_2D + 5,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10,
                                                SIG_COEF_CONTEXTS_2D + 10);
    int32_t       row           = height;
    __m128i       count;
    __m128i       level[5];

    assert(!(height % 2));

    do {
        load_levels_8x2x5_sse2(levels, stride, offsets, level);
        count = get_coeff_contexts_kernel_sse2(level);
        count = _mm_add_epi8(count, pos_to_offset);
        _mm_storeu_si128((__m128i*)coeff_contexts, count);
        levels += 2 * stride;
        coeff_contexts += 16;
        row -= 2;
    } while (row);
}

static INLINE void get_8_coeff_contexts_ver(const uint8_t* levels, const int32_t height, const ptrdiff_t* const offsets,
                                            int8_t* coeff_contexts) {
    const int32_t stride              = 8 + TX_PAD_HOR;
    const __m128i pos_to_offset_large = _mm_set1_epi8(SIG_COEF_CONTEXTS_2D + 10);
    __m128i       pos_to_offset       = _mm_setr_epi8(SIG_COEF_CONTEXTS_2D + 0,
                                          SIG_COEF_CONTEXTS_2D + 0,
                                          SIG_COEF_CONTEXTS_2D + 0,
                                          SIG_COEF_CONTEXTS_2D + 0,
                                          SIG_COEF_CONTEXTS_2D + 0,
                                          SIG_COEF_CONTEXTS_2D + 0,
                                          SIG_COEF_CONTEXTS_2D + 0,
                                          SIG_COEF_CONTEXTS_2D + 0,
                                          SIG_COEF_CONTEXTS_2D + 5,
                                          SIG_COEF_CONTEXTS_2D + 5,
                                          SIG_COEF_CONTEXTS_2D + 5,
                                          SIG_COEF_CONTEXTS_2D + 5,
                                          SIG_COEF_CONTEXTS_2D + 5,
                                          SIG_COEF_CONTEXTS_2D + 5,
                                          SIG_COEF_CONTEXTS_2D + 5,
                                          SIG_COEF_CONTEXTS_2D + 5);
    int32_t       row                 = height;
    __m128i       count;
    __m128i       level[5];

    assert(!(height % 2));

    do {
        load_levels_8x2x5_sse2(levels, stride, offsets, level);
        count = get_coeff_contexts_kernel_sse2(level);
        count = _mm_add_epi8(count, pos_to_offset);
        _mm_storeu_si128((__m128i*)coeff_contexts, count);
        pos_to_offset = pos_to_offset_large;
        levels += 2 * stride;
        coeff_contexts += 16;
        row -= 2;
    } while (row);
}

static INLINE void get_16n_coeff_contexts_hor(const uint8_t* levels, const int32_t width, const int32_t height,
                                              const ptrdiff_t* const offsets, int8_t* coeff_contexts) {
    const int32_t stride              = width + TX_PAD_HOR;
    const __m128i pos_to_offset_large = _mm_setr_epi8(SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10,
                                                      SIG_COEF_CONTEXTS_2D + 10);
    __m128i       count;
    __m128i       level[5];
    int32_t       row = height;

    assert(!(width % 16));

    do {
        __m128i pos_to_offset = _mm_setr_epi8(SIG_COEF_CONTEXTS_2D + 0,
                                              SIG_COEF_CONTEXTS_2D + 5,
                                              SIG_COEF_CONTEXTS_2D + 10,
                                              SIG_COEF_CONTEXTS_2D + 10,
                                              SIG_COEF_CONTEXTS_2D + 10,
                                              SIG_COEF_CONTEXTS_2D + 10,
                                              SIG_COEF_CONTEXTS_2D + 10,
                                              SIG_COEF_CONTEXTS_2D + 10,
                                              SIG_COEF_CONTEXTS_2D + 10,
                                              SIG_COEF_CONTEXTS_2D + 10,
                                              SIG_COEF_CONTEXTS_2D + 10,
                                              SIG_COEF_CONTEXTS_2D + 10,
                                              SIG_COEF_CONTEXTS_2D + 10,
                                              SIG_COEF_CONTEXTS_2D + 10,
                                              SIG_COEF_CONTEXTS_2D + 10,
                                              SIG_COEF_CONTEXTS_2D + 10);
        int32_t w             = width;

        do {
            load_levels_16x1x5_sse2(levels, stride, offsets, level);
            count = get_coeff_contexts_kernel_sse2(level);
            count = _mm_add_epi8(count, pos_to_offset);
            _mm_storeu_si128((__m128i*)coeff_contexts, count);
            pos_to_offset = pos_to_offset_large;
            levels += 16;
            coeff_contexts += 16;
            w -= 16;
        } while (w);

        levels += TX_PAD_HOR;
    } while (--row);
}

static INLINE void get_16n_coeff_contexts_ver(const uint8_t* levels, const int32_t width, const int32_t height,
                                              const ptrdiff_t* const offsets, int8_t* coeff_contexts) {
    const int32_t stride = width + TX_PAD_HOR;
    __m128i       pos_to_offset[3];
    __m128i       count;
    __m128i       level[5];
    int32_t       row = height;

    assert(!(width % 16));

    pos_to_offset[0] = _mm_set1_epi8(SIG_COEF_CONTEXTS_2D + 0);
    pos_to_offset[1] = _mm_set1_epi8(SIG_COEF_CONTEXTS_2D + 5);
    pos_to_offset[2] = _mm_set1_epi8(SIG_COEF_CONTEXTS_2D + 10);

    do {
        int32_t w = width;

        do {
            load_levels_16x1x5_sse2(levels, stride, offsets, level);
            count = get_coeff_contexts_kernel_sse2(level);
            count = _mm_add_epi8(count, pos_to_offset[0]);
            _mm_storeu_si128((__m128i*)coeff_contexts, count);
            levels += 16;
            coeff_contexts += 16;
            w -= 16;
        } while (w);

        pos_to_offset[0] = pos_to_offset[1];
        pos_to_offset[1] = pos_to_offset[2];
        levels += TX_PAD_HOR;
    } while (--row);
}

void svt_av1_get_nz_map_contexts_sse2(const uint8_t* const levels, const int16_t* const scan, const uint16_t eob,
                                      TxSize tx_size, const TxClass tx_class, int8_t* const coeff_contexts) {
    const int32_t last_idx = eob - 1;
    if (!last_idx) {
        coeff_contexts[0] = 0;
        return;
    }

    const int32_t real_width  = tx_size_wide[tx_size];
    const int32_t real_height = tx_size_high[tx_size];
    const int32_t width       = get_txb_wide(tx_size);
    const int32_t height      = get_txb_high(tx_size);

    const int32_t stride = width + TX_PAD_HOR;

    ptrdiff_t offsets[3];

    /* coeff_contexts must be 16 byte aligned. */
    assert(!((intptr_t)coeff_contexts & 0xf));

    if (tx_class == TX_CLASS_2D) {
        offsets[0] = 0 * stride + 2;
        offsets[1] = 1 * stride + 1;
        offsets[2] = 2 * stride + 0;

        if (width == 4) {
            get_4_nz_map_contexts_2d(levels, height, offsets, coeff_contexts);
        } else if (width == 8) {
            get_8_coeff_contexts_2d(levels, height, offsets, coeff_contexts);
        } else {
            get_16n_coeff_contexts_2d(levels, real_width, real_height, width, height, offsets, coeff_contexts);
        }
    } else if (tx_class == TX_CLASS_HORIZ) {
        offsets[0] = 2;
        offsets[1] = 3;
        offsets[2] = 4;
        if (width == 4) {
            get_4_nz_map_contexts_hor(levels, height, offsets, coeff_contexts);
        } else if (width == 8) {
            get_8_coeff_contexts_hor(levels, height, offsets, coeff_contexts);
        } else {
            get_16n_coeff_contexts_hor(levels, width, height, offsets, coeff_contexts);
        }
    } else { // TX_CLASS_VERT
        offsets[0] = 2 * stride;
        offsets[1] = 3 * stride;
        offsets[2] = 4 * stride;
        if (width == 4) {
            get_4_nz_map_contexts_ver(levels, height, offsets, coeff_contexts);
        } else if (width == 8) {
            get_8_coeff_contexts_ver(levels, height, offsets, coeff_contexts);
        } else {
            get_16n_coeff_contexts_ver(levels, width, height, offsets, coeff_contexts);
        }
    }

    const int32_t bwl = get_txb_bwl(tx_size);
    const int32_t pos = scan[last_idx];
    if (last_idx <= (height << bwl) / 8) {
        coeff_contexts[pos] = 1;
    } else if (last_idx <= (height << bwl) / 4) {
        coeff_contexts[pos] = 2;
    } else {
        coeff_contexts[pos] = 3;
    }
}
