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

#include <immintrin.h> /* AVX2 */

#include "definitions.h"
#include "synonyms.h"
#include "synonyms_avx2.h"

#include "cabac_context_model.h"
#include "full_loop.h"

static INLINE __m256i txb_init_levels_avx2(const TranLow* const coeff) {
    const __m256i idx   = _mm256_setr_epi32(0, 4, 1, 5, 2, 6, 3, 7);
    const __m256i c0    = yy_loadu_256(coeff + 0 * 8);
    const __m256i c1    = yy_loadu_256(coeff + 1 * 8);
    const __m256i c2    = yy_loadu_256(coeff + 2 * 8);
    const __m256i c3    = yy_loadu_256(coeff + 3 * 8);
    const __m256i c01   = _mm256_packs_epi32(c0, c1);
    const __m256i c23   = _mm256_packs_epi32(c2, c3);
    const __m256i abs01 = _mm256_abs_epi16(c01);
    const __m256i abs23 = _mm256_abs_epi16(c23);
    const __m256i res   = _mm256_packs_epi16(abs01, abs23);
    return _mm256_permutevar8x32_epi32(res, idx);
}

void svt_av1_txb_init_levels_avx2(const TranLow* const coeff, const int32_t width, const int32_t height,
                                  uint8_t* const levels) {
    const TranLow* cf      = coeff;
    const __m128i  x_zeros = _mm_setzero_si128();
    const __m256i  y_zeros = _mm256_setzero_si256();
    uint8_t*       ls      = levels;
    int32_t        i       = height;

    if (width == 4) {
        xx_storeu_128(ls - 16, x_zeros);

        do {
            const __m256i idx   = _mm256_setr_epi32(0, 2, 4, 6, 1, 3, 5, 7);
            const __m256i c0    = yy_loadu_256(cf);
            const __m256i c1    = yy_loadu_256(cf + 8);
            const __m256i c01   = _mm256_packs_epi32(c0, c1);
            const __m256i abs01 = _mm256_abs_epi16(c01);
            const __m256i res_  = _mm256_packs_epi16(abs01, y_zeros);
            const __m256i res   = _mm256_permutevar8x32_epi32(res_, idx);
            yy_storeu_256(ls, res);
            cf += 4 * 4;
            ls += 4 * 8;
            i -= 4;
        } while (i);

        yy_storeu_256(ls, y_zeros);
    } else if (width == 8) {
        yy_storeu_256(ls - 24, y_zeros);

        do {
            const __m256i res  = txb_init_levels_avx2(cf);
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
        yy_storeu_256(ls - 40, y_zeros);
        xx_storel_64(ls - 8, x_zeros);

        do {
            const __m256i res  = txb_init_levels_avx2(cf);
            const __m128i res0 = _mm256_castsi256_si128(res);
            const __m128i res1 = _mm256_extracti128_si256(res, 1);
            xx_storeu_128(ls, res0);
            *(int32_t*)(ls + 16) = 0;
            xx_storeu_128(ls + 20, res1);
            *(int32_t*)(ls + 20 + 16) = 0;
            cf += 2 * 16;
            ls += 2 * 20;
            i -= 2;
        } while (i);

        yy_storeu_256(ls + 0 * 32, y_zeros);
        yy_storeu_256(ls + 1 * 32, y_zeros);
        xx_storeu_128(ls + 2 * 32, x_zeros);
    } else {
        yy_storeu_256(ls - 72, y_zeros);
        yy_storeu_256(ls - 40, y_zeros);
        xx_storel_64(ls - 8, x_zeros);

        do {
            const __m256i res = txb_init_levels_avx2(cf);
            yy_storeu_256(ls, res);
            *(int32_t*)(ls + 32) = 0;
            cf += 32;
            ls += 36;
        } while (--i);

        yy_storeu_256(ls + 0 * 32, y_zeros);
        yy_storeu_256(ls + 1 * 32, y_zeros);
        yy_storeu_256(ls + 2 * 32, y_zeros);
        yy_storeu_256(ls + 3 * 32, y_zeros);
        xx_storeu_128(ls + 4 * 32, x_zeros);
    }
}

static INLINE __m256i set_128x2(__m128i val_lo, __m128i val_hi) {
    return _mm256_inserti128_si256(_mm256_castsi128_si256(val_lo), val_hi, 1);
}

static INLINE __m128i load_8bit_4x4_to_1_reg_sse2(const void* const src, const int32_t byte_stride) {
    return _mm_setr_epi32(*(const int32_t*)((int8_t*)src + 0 * byte_stride),
                          *(const int32_t*)((int8_t*)src + 1 * byte_stride),
                          *(const int32_t*)((int8_t*)src + 2 * byte_stride),
                          *(const int32_t*)((int8_t*)src + 3 * byte_stride));
}

static INLINE __m256i load_64x4(const uint8_t* const src, const int32_t stride) {
    return _mm256_setr_epi64x(*(uint64_t*)(src + 0 * stride),
                              *(uint64_t*)(src + 1 * stride),
                              *(uint64_t*)(src + 2 * stride),
                              *(uint64_t*)(src + 3 * stride));
}

static INLINE void load_levels_4x4x5_sse2(const uint8_t* const src, const int32_t stride,
                                          const ptrdiff_t* const offsets, __m128i* const level) {
    level[0] = load_8bit_4x4_to_1_reg_sse2(src + 1, stride);
    level[1] = load_8bit_4x4_to_1_reg_sse2(src + stride, stride);
    level[2] = load_8bit_4x4_to_1_reg_sse2(src + offsets[0], stride);
    level[3] = load_8bit_4x4_to_1_reg_sse2(src + offsets[1], stride);
    level[4] = load_8bit_4x4_to_1_reg_sse2(src + offsets[2], stride);
}

static INLINE void load_levels_8x4x5_avx2(const uint8_t* const src, const int32_t stride,
                                          const ptrdiff_t* const offsets, __m256i* const level) {
    level[0] = load_64x4(src + 1, stride);
    level[1] = load_64x4(src + 1 * stride, stride);
    level[2] = load_64x4(src + offsets[0], stride);
    level[3] = load_64x4(src + offsets[1], stride);
    level[4] = load_64x4(src + offsets[2], stride);
}

static INLINE void load_levels_16x2x5_avx2(const uint8_t* const src, const int32_t stride,
                                           const ptrdiff_t* const offsets, __m256i* const level) {
    level[0] = set_128x2(_mm_loadu_si128((__m128i*)(src + 1)), _mm_loadu_si128((__m128i*)(src + stride + 1)));
    level[1] = set_128x2(_mm_loadu_si128((__m128i*)(src + stride)), _mm_loadu_si128((__m128i*)(src + stride * 2)));
    level[2] = set_128x2(_mm_loadu_si128((__m128i*)(src + offsets[0])),
                         _mm_loadu_si128((__m128i*)(src + offsets[0] + stride)));
    level[3] = set_128x2(_mm_loadu_si128((__m128i*)(src + offsets[1])),
                         _mm_loadu_si128((__m128i*)(src + offsets[1] + stride)));
    level[4] = set_128x2(_mm_loadu_si128((__m128i*)(src + offsets[2])),
                         _mm_loadu_si128((__m128i*)(src + offsets[2] + stride)));
}

static INLINE void load_levels_32x1x5_avx2(const uint8_t* const src, const int32_t stride,
                                           const ptrdiff_t* const offsets, __m256i* const level) {
    level[0] = _mm256_loadu_si256((__m256i*)(src + 1));
    level[1] = _mm256_loadu_si256((__m256i*)(src + stride));
    level[2] = _mm256_loadu_si256((__m256i*)(src + offsets[0]));
    level[3] = _mm256_loadu_si256((__m256i*)(src + offsets[1]));
    level[4] = _mm256_loadu_si256((__m256i*)(src + offsets[2]));
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

static INLINE __m256i get_coeff_contexts_kernel_avx2(__m256i* const level) {
    const __m256i const_3 = _mm256_set1_epi8(3);
    const __m256i const_4 = _mm256_set1_epi8(4);
    __m256i       count;

    count    = _mm256_min_epu8(level[0], const_3);
    level[1] = _mm256_min_epu8(level[1], const_3);
    level[2] = _mm256_min_epu8(level[2], const_3);
    level[3] = _mm256_min_epu8(level[3], const_3);
    level[4] = _mm256_min_epu8(level[4], const_3);
    count    = _mm256_add_epi8(count, level[1]);
    count    = _mm256_add_epi8(count, level[2]);
    count    = _mm256_add_epi8(count, level[3]);
    count    = _mm256_add_epi8(count, level[4]);
    count    = _mm256_avg_epu8(count, _mm256_setzero_si256());
    count    = _mm256_min_epu8(count, const_4);
    return count;
}

DECLARE_ALIGNED(32, static const uint8_t, pos_to_offsets[256]) = {
    0,  1,  6,  6,  21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, //+0
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    1,  6,  6,  21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, //+32
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    6,  6,  21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, //+64
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    6,  21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, //+96
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, //+128
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    0,  16, 6,  6,  21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, //+160
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    16, 16, 6,  21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, //+192
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21,
    16, 16, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, //+224
    21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21};

static INLINE void get_4_nz_map_contexts_2d_sse2(const uint8_t* levels, const int32_t height,
                                                 const ptrdiff_t* const offsets, int8_t* const coeff_contexts) {
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

static INLINE void get_8_coeff_contexts_2d_avx2(const uint8_t* levels, const int32_t height,
                                                const ptrdiff_t* const offsets, int8_t* coeff_contexts) {
    const int32_t stride = 8 + TX_PAD_HOR;
    int8_t*       cc     = coeff_contexts;
    int32_t       row    = height;
    __m256i       count;
    __m256i       level[5];
    __m256i       pos_to_offset[2];

    assert(!(height % 4));

    if (height == 8) {
        __m128i tmp_a    = _mm_setr_epi8(0, 1, 6, 6, 21, 21, 21, 21, 1, 6, 6, 21, 21, 21, 21, 21);
        __m128i tmp_b    = _mm_setr_epi8(6, 6, 21, 21, 21, 21, 21, 21, 6, 21, 21, 21, 21, 21, 21, 21);
        pos_to_offset[0] = set_128x2(tmp_a, tmp_b);
    } else if (height < 8) {
        __m128i tmp_a    = _mm_setr_epi8(0, 16, 6, 6, 21, 21, 21, 21, 16, 16, 6, 21, 21, 21, 21, 21);
        __m128i tmp_b    = _mm_setr_epi8(16, 16, 21, 21, 21, 21, 21, 21, 16, 16, 21, 21, 21, 21, 21, 21);
        pos_to_offset[0] = set_128x2(tmp_a, tmp_b);
    } else {
        __m128i tmp_a    = _mm_setr_epi8(0, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11);
        __m128i tmp_b    = _mm_setr_epi8(6, 6, 21, 21, 21, 21, 21, 21, 6, 21, 21, 21, 21, 21, 21, 21);
        pos_to_offset[0] = set_128x2(tmp_a, tmp_b);
    }
    pos_to_offset[1] = _mm256_set1_epi8(21);

    do {
        load_levels_8x4x5_avx2(levels, stride, offsets, level);
        count = get_coeff_contexts_kernel_avx2(level);
        count = _mm256_add_epi8(count, pos_to_offset[0]);
        _mm256_storeu_si256((__m256i*)cc, count);
        pos_to_offset[0] = pos_to_offset[1];
        levels += 4 * stride;
        cc += 32;
        row -= 4;
    } while (row);

    coeff_contexts[0] = 0;
}

static INLINE void get_16_coeff_contexts_2d_avx2(const uint8_t* levels, const int32_t real_width,
                                                 const int32_t real_height, const int32_t width, const int32_t height,
                                                 const ptrdiff_t* const offsets, int8_t* coeff_contexts) {
    const int32_t stride = width + TX_PAD_HOR;
    int8_t*       cc     = coeff_contexts;
    int32_t       row    = height;
    __m256i       pos_to_offset[3];
    __m256i       count;
    __m256i       level[5];

    assert(!(width % 16));
    assert(!(height % 2));

    if (real_width == real_height) {
        pos_to_offset[0] = set_128x2(_mm_loadu_si128((__m128i*)(pos_to_offsets)),
                                     _mm_loadu_si128((__m128i*)(pos_to_offsets + 32)));
        pos_to_offset[1] = set_128x2(_mm_loadu_si128((__m128i*)(pos_to_offsets + 64)),
                                     _mm_loadu_si128((__m128i*)(pos_to_offsets + 96)));
        pos_to_offset[2] = _mm256_loadu_si256((__m256i*)(pos_to_offsets + 128));
    } else if (real_width > real_height) {
        pos_to_offset[0] = set_128x2(_mm_loadu_si128((__m128i*)(pos_to_offsets + 160)),
                                     _mm_loadu_si128((__m128i*)(pos_to_offsets + 192)));
        __m128i tmp      = _mm_loadu_si128((__m128i*)(pos_to_offsets + 224));
        pos_to_offset[1] = pos_to_offset[2] = set_128x2(tmp, tmp);
    } else { // real_width < real_height
        pos_to_offset[0] = _mm256_set1_epi8(11);
        pos_to_offset[1] = set_128x2(_mm_loadu_si128((__m128i*)(pos_to_offsets + 64)),
                                     _mm_loadu_si128((__m128i*)(pos_to_offsets + 96)));
        pos_to_offset[2] = _mm256_loadu_si256((__m256i*)(pos_to_offsets + 128));
    }

    do {
        load_levels_16x2x5_avx2(levels, stride, offsets, level);
        count = get_coeff_contexts_kernel_avx2(level);
        count = _mm256_add_epi8(count, pos_to_offset[0]);
        _mm256_storeu_si256((__m256i*)cc, count);

        pos_to_offset[0] = pos_to_offset[1];
        pos_to_offset[1] = pos_to_offset[2];

        levels += 32 + 2 * TX_PAD_HOR;
        cc += 32;
        row -= 2;
    } while (row);

    coeff_contexts[0] = 0;
}

static INLINE void get_32n_coeff_contexts_2d_avx2(const uint8_t* levels, const int32_t real_width,
                                                  const int32_t real_height, const int32_t width, const int32_t height,
                                                  const ptrdiff_t* const offsets, int8_t* coeff_contexts) {
    const int32_t stride = width + TX_PAD_HOR;
    int8_t*       cc     = coeff_contexts;
    int32_t       row    = height;
    __m256i       pos_to_offset[5];
    __m256i       pos_to_offset_large[3];
    __m256i       count;
    __m256i       level[5];

    assert(!(width % 32));

    pos_to_offset_large[2] = _mm256_loadu_si256((__m256i*)(pos_to_offsets + 128));
    if (real_width == real_height) {
        pos_to_offset[0] = _mm256_loadu_si256((__m256i*)(pos_to_offsets));
        pos_to_offset[1] = _mm256_loadu_si256((__m256i*)(pos_to_offsets + 32));
        pos_to_offset[2] = _mm256_loadu_si256((__m256i*)(pos_to_offsets + 64));
        pos_to_offset[3] = _mm256_loadu_si256((__m256i*)(pos_to_offsets + 96));

        pos_to_offset[4] = pos_to_offset_large[0] = pos_to_offset_large[1] = pos_to_offset_large[2];
    } else if (real_width > real_height) {
        pos_to_offset[0] = _mm256_loadu_si256((__m256i*)(pos_to_offsets + 160));
        pos_to_offset[1] = _mm256_loadu_si256((__m256i*)(pos_to_offsets + 192));
        pos_to_offset[2] = _mm256_loadu_si256((__m256i*)(pos_to_offsets + 224));

        pos_to_offset[3] = pos_to_offset[4] = pos_to_offset[2];
        pos_to_offset_large[0] = pos_to_offset_large[1] = pos_to_offset_large[2];
    } else { // real_width < real_height
        pos_to_offset[0] = _mm256_set1_epi8(11);
        pos_to_offset[2] = _mm256_loadu_si256((__m256i*)(pos_to_offsets + 64));
        pos_to_offset[3] = _mm256_loadu_si256((__m256i*)(pos_to_offsets + 96));

        pos_to_offset[1] = pos_to_offset_large[0] = pos_to_offset_large[1] = pos_to_offset[0];
        pos_to_offset[4]                                                   = pos_to_offset_large[2];
    }

    do {
        int32_t w = width;

        do {
            load_levels_32x1x5_avx2(levels, stride, offsets, level);
            count = get_coeff_contexts_kernel_avx2(level);
            count = _mm256_add_epi8(count, pos_to_offset[0]);
            _mm256_storeu_si256((__m256i*)cc, count);
            levels += 32;
            cc += 32;
            w -= 32;
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

static INLINE void get_4_nz_map_contexts_hor_sse2(const uint8_t* levels, const int32_t height,
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

static INLINE void get_8_coeff_contexts_hor_avx2(const uint8_t* levels, const int32_t height,
                                                 const ptrdiff_t* const offsets, int8_t* coeff_contexts) {
    const int32_t stride              = 8 + TX_PAD_HOR;
    const __m128i pos_to_offset_small = _mm_setr_epi8(SIG_COEF_CONTEXTS_2D + 0,
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

    __m256i pos_to_offset = set_128x2(pos_to_offset_small, pos_to_offset_small);

    int32_t row = height;
    __m256i count;
    __m256i level[5];

    assert(!(height % 4));

    do {
        load_levels_8x4x5_avx2(levels, stride, offsets, level);
        count = get_coeff_contexts_kernel_avx2(level);
        count = _mm256_add_epi8(count, pos_to_offset);
        _mm256_storeu_si256((__m256i*)coeff_contexts, count);
        levels += 4 * stride;
        coeff_contexts += 32;
        row -= 4;
    } while (row);
}

static INLINE void get_16_coeff_contexts_hor_avx2(const uint8_t* levels, const int32_t width, const int32_t height,
                                                  const ptrdiff_t* const offsets, int8_t* coeff_contexts) {
    const int32_t stride = width + TX_PAD_HOR;
    __m256i       count;
    __m256i       level[5];
    int32_t       row = height;

    assert(!(width % 16));
    assert(!(height % 2));
    __m128i pos_to_offset_small = _mm_setr_epi8(SIG_COEF_CONTEXTS_2D + 0,
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

    __m256i pos_to_offset = set_128x2(pos_to_offset_small, pos_to_offset_small);

    do {
        load_levels_16x2x5_avx2(levels, stride, offsets, level);
        count = get_coeff_contexts_kernel_avx2(level);
        count = _mm256_add_epi8(count, pos_to_offset);
        _mm256_storeu_si256((__m256i*)coeff_contexts, count);

        levels += 32 + 2 * TX_PAD_HOR;
        coeff_contexts += 32;
        row -= 2;
    } while (row);
}

static INLINE void get_32n_coeff_contexts_hor_avx2(const uint8_t* levels, const int32_t width, const int32_t height,
                                                   const ptrdiff_t* const offsets, int8_t* coeff_contexts) {
    const int32_t stride              = width + TX_PAD_HOR;
    const __m256i pos_to_offset_large = _mm256_set1_epi8(SIG_COEF_CONTEXTS_2D + 10);
    __m256i       count;
    __m256i       level[5];
    int32_t       row = height;

    assert(!(width % 16));

    do {
        __m256i pos_to_offset = _mm256_setr_epi8(SIG_COEF_CONTEXTS_2D + 0,
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
                                                 SIG_COEF_CONTEXTS_2D + 10,
                                                 SIG_COEF_CONTEXTS_2D + 10,
                                                 SIG_COEF_CONTEXTS_2D + 10);
        int32_t w             = width;

        do {
            load_levels_32x1x5_avx2(levels, stride, offsets, level);
            count = get_coeff_contexts_kernel_avx2(level);
            count = _mm256_add_epi8(count, pos_to_offset);
            _mm256_storeu_si256((__m256i*)coeff_contexts, count);
            pos_to_offset = pos_to_offset_large;

            levels += 32;
            coeff_contexts += 32;
            w -= 32;
        } while (w);

        levels += TX_PAD_HOR;
    } while (--row);
}

static INLINE void get_4_nz_map_contexts_ver_sse2(const uint8_t* levels, const int32_t height,
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

static INLINE void get_8_coeff_contexts_ver_avx2(const uint8_t* levels, const int32_t height,
                                                 const ptrdiff_t* const offsets, int8_t* coeff_contexts) {
    const int32_t stride         = 8 + TX_PAD_HOR;
    __m128i       pos_to_off_128 = _mm_setr_epi8(SIG_COEF_CONTEXTS_2D + 0,
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

    __m256i pos_to_offset       = set_128x2(pos_to_off_128, _mm_set1_epi8(SIG_COEF_CONTEXTS_2D + 10));
    __m256i pos_to_offset_large = _mm256_set1_epi8(SIG_COEF_CONTEXTS_2D + 10);

    int32_t row = height;
    __m256i count;
    __m256i level[5];

    assert(!(height % 4));

    do {
        load_levels_8x4x5_avx2(levels, stride, offsets, level);
        count = get_coeff_contexts_kernel_avx2(level);
        count = _mm256_add_epi8(count, pos_to_offset);
        _mm256_storeu_si256((__m256i*)coeff_contexts, count);
        pos_to_offset = pos_to_offset_large;
        levels += 4 * stride;
        coeff_contexts += 32;
        row -= 4;
    } while (row);
}

static INLINE void get_16_coeff_contexts_ver_avx2(const uint8_t* levels, const int32_t width, const int32_t height,
                                                  const ptrdiff_t* const offsets, int8_t* coeff_contexts) {
    const int32_t stride = width + TX_PAD_HOR;
    __m256i       pos_to_offset[2];
    __m256i       count;
    __m256i       level[5];
    int32_t       row = height;

    assert(!(width % 16));
    assert(!(height % 2));

    pos_to_offset[0] = set_128x2(_mm_set1_epi8(SIG_COEF_CONTEXTS_2D + 0), _mm_set1_epi8(SIG_COEF_CONTEXTS_2D + 5));
    pos_to_offset[1] = _mm256_set1_epi8(SIG_COEF_CONTEXTS_2D + 10);

    do {
        load_levels_16x2x5_avx2(levels, stride, offsets, level);
        count = get_coeff_contexts_kernel_avx2(level);
        count = _mm256_add_epi8(count, pos_to_offset[0]);
        _mm256_storeu_si256((__m256i*)coeff_contexts, count);
        pos_to_offset[0] = pos_to_offset[1];

        levels += 32 + 2 * TX_PAD_HOR;
        coeff_contexts += 32;
        row -= 2;
    } while (row);
}

static INLINE void get_32n_coeff_contexts_ver_avx2(const uint8_t* levels, const int32_t width, const int32_t height,
                                                   const ptrdiff_t* const offsets, int8_t* coeff_contexts) {
    const int32_t stride = width + TX_PAD_HOR;
    __m256i       pos_to_offset[3];
    __m256i       count;
    __m256i       level[5];
    int32_t       row = height;

    assert(!(width % 32));

    pos_to_offset[0] = _mm256_set1_epi8(SIG_COEF_CONTEXTS_2D + 0);
    pos_to_offset[1] = _mm256_set1_epi8(SIG_COEF_CONTEXTS_2D + 5);
    pos_to_offset[2] = _mm256_set1_epi8(SIG_COEF_CONTEXTS_2D + 10);

    do {
        int32_t w = width;

        do {
            load_levels_32x1x5_avx2(levels, stride, offsets, level);
            count = get_coeff_contexts_kernel_avx2(level);
            count = _mm256_add_epi8(count, pos_to_offset[0]);
            _mm256_storeu_si256((__m256i*)coeff_contexts, count);
            levels += 32;
            coeff_contexts += 32;
            w -= 32;
        } while (w);

        pos_to_offset[0] = pos_to_offset[1];
        pos_to_offset[1] = pos_to_offset[2];
        levels += TX_PAD_HOR;
    } while (--row);
}

void svt_av1_get_nz_map_contexts_avx2(const uint8_t* const levels, const int16_t* const scan, const uint16_t eob,
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
            get_4_nz_map_contexts_2d_sse2(levels, height, offsets, coeff_contexts);
        } else if (width == 8) {
            get_8_coeff_contexts_2d_avx2(levels, height, offsets, coeff_contexts);
        } else if (width == 16) {
            get_16_coeff_contexts_2d_avx2(levels, real_width, real_height, width, height, offsets, coeff_contexts);
        } else {
            get_32n_coeff_contexts_2d_avx2(levels, real_width, real_height, width, height, offsets, coeff_contexts);
        }
    } else if (tx_class == TX_CLASS_HORIZ) {
        offsets[0] = 2;
        offsets[1] = 3;
        offsets[2] = 4;
        if (width == 4) {
            get_4_nz_map_contexts_hor_sse2(levels, height, offsets, coeff_contexts);
        } else if (width == 8) {
            get_8_coeff_contexts_hor_avx2(levels, height, offsets, coeff_contexts);
        } else if (width == 16) {
            get_16_coeff_contexts_hor_avx2(levels, width, height, offsets, coeff_contexts);
        } else {
            get_32n_coeff_contexts_hor_avx2(levels, width, height, offsets, coeff_contexts);
        }
    } else { // TX_CLASS_VERT
        offsets[0] = 2 * stride;
        offsets[1] = 3 * stride;
        offsets[2] = 4 * stride;
        if (width == 4) {
            get_4_nz_map_contexts_ver_sse2(levels, height, offsets, coeff_contexts);
        } else if (width == 8) {
            get_8_coeff_contexts_ver_avx2(levels, height, offsets, coeff_contexts);
        } else if (width == 16) {
            get_16_coeff_contexts_ver_avx2(levels, width, height, offsets, coeff_contexts);
        } else {
            get_32n_coeff_contexts_ver_avx2(levels, width, height, offsets, coeff_contexts);
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

void svt_copy_mi_map_grid_avx2(MbModeInfo** mi_grid_ptr, uint32_t mi_stride, uint8_t num_rows, uint8_t num_cols) {
    MbModeInfo* target = mi_grid_ptr[0];
    if (num_cols == 1) {
        for (uint8_t mi_y = 0; mi_y < num_rows; mi_y++) {
            const int32_t mi_idx = 0 + mi_y * mi_stride;
            // width is 1 block (corresponds to block width 4)
            mi_grid_ptr[mi_idx] = target;
        }
    } else if (num_cols == 2) {
        __m128i target_sse = _mm_set1_epi64x((uint64_t)target);
        for (uint8_t mi_y = 0; mi_y < num_rows; mi_y++) {
            const int32_t mi_idx = 0 + mi_y * mi_stride;
            // width is 2 blocks, so can copy 2 at once (corresponds to block width 8)
            _mm_storeu_si128((__m128i*)(mi_grid_ptr + mi_idx), target_sse);
        }
    } else {
        __m256i target_avx = _mm256_set1_epi64x((uint64_t)target);
        for (uint8_t mi_y = 0; mi_y < num_rows; mi_y++) {
            for (uint8_t mi_x = 0; mi_x < num_cols; mi_x += 4) {
                const int32_t mi_idx = mi_x + mi_y * mi_stride;
                // width is >=4 blocks, so can copy 4 at once; (corresponds to block width >=16).
                // All blocks >= 16 have widths that are divisible by 16, so it is ok to copy 4 blocks at once
                _mm256_storeu_si256((__m256i*)(mi_grid_ptr + mi_idx), target_avx);
            }
        }
    }
}
