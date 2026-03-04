/*
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef AV1_TXMF1D_SSE2_H_
#define AV1_TXMF1D_SSE2_H_
#include <emmintrin.h>
#include <smmintrin.h>
#include "av1_txfm_sse4.h"

#ifdef __cplusplus
extern "C" {
#endif

static INLINE void transpose_32_4x4(int32_t stride, const __m128i* input, __m128i* output) {
    __m128i temp0 = _mm_unpacklo_epi32(input[0 * stride], input[2 * stride]);
    __m128i temp1 = _mm_unpackhi_epi32(input[0 * stride], input[2 * stride]);
    __m128i temp2 = _mm_unpacklo_epi32(input[1 * stride], input[3 * stride]);
    __m128i temp3 = _mm_unpackhi_epi32(input[1 * stride], input[3 * stride]);

    output[0 * stride] = _mm_unpacklo_epi32(temp0, temp2);
    output[1 * stride] = _mm_unpackhi_epi32(temp0, temp2);
    output[2 * stride] = _mm_unpacklo_epi32(temp1, temp3);
    output[3 * stride] = _mm_unpackhi_epi32(temp1, temp3);
}

// the entire input block can be represent by a grid of 4x4 blocks
// each 4x4 blocks can be represent by 4 vertical __m128i
// we first transpose each 4x4 block internally
// then transpose the grid
static INLINE void transpose_32(int32_t txfm_size, const __m128i* input, __m128i* output) {
    const int32_t num_per_128 = 4;
    const int32_t row_size    = txfm_size;
    const int32_t col_size    = txfm_size / num_per_128;
    int32_t       r, c;

    // transpose each 4x4 block internally
    for (r = 0; r < row_size; r += 4) {
        for (c = 0; c < col_size; c++) {
            transpose_32_4x4(col_size, &input[r * col_size + c], &output[c * 4 * col_size + r / 4]);
        }
    }
}

// out0 = in0*w0 + in1*w1
// out1 = -in1*w0 + in0*w1
#define btf_32_sse4_1_type0(w0, w1, in0, in1, out0, out1, bit)       \
    do {                                                             \
        const __m128i ww0    = _mm_set1_epi32(w0);                   \
        const __m128i ww1    = _mm_set1_epi32(w1);                   \
        const __m128i in0_w0 = _mm_mullo_epi32(in0, ww0);            \
        const __m128i in1_w1 = _mm_mullo_epi32(in1, ww1);            \
        out0                 = _mm_add_epi32(in0_w0, in1_w1);        \
        out0                 = av1_round_shift_32_sse4_1(out0, bit); \
        const __m128i in0_w1 = _mm_mullo_epi32(in0, ww1);            \
        const __m128i in1_w0 = _mm_mullo_epi32(in1, ww0);            \
        out1                 = _mm_sub_epi32(in0_w1, in1_w0);        \
        out1                 = av1_round_shift_32_sse4_1(out1, bit); \
    } while (0)

// out0 = in0*w0 + in1*w1
// out1 = in1*w0 - in0*w1
#define btf_32_sse4_1_type1(w0, w1, in0, in1, out0, out1, bit)  \
    do {                                                        \
        btf_32_sse4_1_type0(w1, w0, in1, in0, out0, out1, bit); \
    } while (0)

// out0 = in0*w0 + in1*w1
// out1 = -in1*w0 + in0*w1
#define btf_32_type0_sse4_1_new(ww0, ww1, in0, in1, out0, out1, r, bit) \
    do {                                                                \
        const __m128i in0_w0 = _mm_mullo_epi32(in0, ww0);               \
        const __m128i in1_w1 = _mm_mullo_epi32(in1, ww1);               \
        out0                 = _mm_add_epi32(in0_w0, in1_w1);           \
        out0                 = _mm_add_epi32(out0, r);                  \
        out0                 = _mm_srai_epi32(out0, bit);               \
        const __m128i in0_w1 = _mm_mullo_epi32(in0, ww1);               \
        const __m128i in1_w0 = _mm_mullo_epi32(in1, ww0);               \
        out1                 = _mm_sub_epi32(in0_w1, in1_w0);           \
        out1                 = _mm_add_epi32(out1, r);                  \
        out1                 = _mm_srai_epi32(out1, bit);               \
    } while (0)

// out0 = in0*w0 + in1*w1
// out1 = in1*w0 - in0*w1
#define btf_32_type1_sse4_1_new(ww0, ww1, in0, in1, out0, out1, r, bit)  \
    do {                                                                 \
        btf_32_type0_sse4_1_new(ww1, ww0, in1, in0, out0, out1, r, bit); \
    } while (0)

static INLINE __m128i mm_reverse_epi16(const __m128i x) {
    const __m128i a = _mm_shufflelo_epi16(x, 0x1b);
    const __m128i b = _mm_shufflehi_epi16(a, 0x1b);
    return _mm_shuffle_epi32(b, 0x4e);
}

static INLINE __m128i half_btf_sse4_1(const __m128i* w0, const __m128i* n0, const __m128i* w1, const __m128i* n1,
                                      const __m128i* rounding, int bit) {
    __m128i x, y;

    x = _mm_mullo_epi32(*w0, *n0);
    y = _mm_mullo_epi32(*w1, *n1);
    x = _mm_add_epi32(x, y);
    x = _mm_add_epi32(x, *rounding);
    x = _mm_srai_epi32(x, bit);
    return x;
}

#ifdef __cplusplus
}
#endif

#endif // AV1_TXMF1D_SSE2_H_
