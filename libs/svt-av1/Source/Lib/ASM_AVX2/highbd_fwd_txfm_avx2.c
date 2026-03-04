/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <assert.h>
#include "definitions.h"
#include "aom_dsp_rtcd.h"
#include "transforms.h"
#include <immintrin.h>
#include "txfm_common_avx2.h"
#include "synonyms_avx2.h"

void svt_aom_transform_config(TxType tx_type, TxSize tx_size, Txfm2dFlipCfg* cfg);

typedef void (*FwdTransform1dAvx2)(const __m256i* in, __m256i* out, int8_t bit, const int32_t num_cols);

#define TRANSPOSE_4X4_AVX2(x0, x1, x2, x3, y0, y1, y2, y3) \
    do {                                                   \
        __m256i u0, u1, u2, u3;                            \
        u0 = _mm256_unpacklo_epi32(x0, x1);                \
        u1 = _mm256_unpackhi_epi32(x0, x1);                \
        u2 = _mm256_unpacklo_epi32(x2, x3);                \
        u3 = _mm256_unpackhi_epi32(x2, x3);                \
        y0 = _mm256_unpacklo_epi64(u0, u2);                \
        y1 = _mm256_unpackhi_epi64(u0, u2);                \
        y2 = _mm256_unpacklo_epi64(u1, u3);                \
        y3 = _mm256_unpackhi_epi64(u1, u3);                \
    } while (0)

static INLINE void transpose_8x8_avx2(const __m256i* in, __m256i* out) {
    __m256i out1[8];
    TRANSPOSE_4X4_AVX2(in[0], in[1], in[2], in[3], out1[0], out1[1], out1[4], out1[5]);
    TRANSPOSE_4X4_AVX2(in[4], in[5], in[6], in[7], out1[2], out1[3], out1[6], out1[7]);
    out[0] = yy_unpacklo_epi128(out1[0], out1[2]);
    out[1] = yy_unpacklo_epi128(out1[1], out1[3]);
    out[2] = yy_unpacklo_epi128(out1[4], out1[6]);
    out[3] = yy_unpacklo_epi128(out1[5], out1[7]);
    out[4] = yy_unpackhi_epi128(out1[0], out1[2]);
    out[5] = yy_unpackhi_epi128(out1[1], out1[3]);
    out[6] = yy_unpackhi_epi128(out1[4], out1[6]);
    out[7] = yy_unpackhi_epi128(out1[5], out1[7]);
}

static INLINE void transpose_16x16_avx2(const __m256i* in, __m256i* out) {
    __m256i temp[32];
    TRANSPOSE_4X4_AVX2(in[0], in[2], in[4], in[6], temp[0], temp[2], temp[4], temp[6]);
    TRANSPOSE_4X4_AVX2(in[8], in[10], in[12], in[14], temp[17], temp[19], temp[21], temp[23]);
    TRANSPOSE_4X4_AVX2(in[1], in[3], in[5], in[7], temp[16], temp[18], temp[20], temp[22]);
    TRANSPOSE_4X4_AVX2(in[9], in[11], in[13], in[15], temp[25], temp[27], temp[29], temp[31]);
    TRANSPOSE_4X4_AVX2(in[16], in[18], in[20], in[22], temp[1], temp[3], temp[5], temp[7]);
    TRANSPOSE_4X4_AVX2(in[24], in[26], in[28], in[30], temp[9], temp[11], temp[13], temp[15]);
    TRANSPOSE_4X4_AVX2(in[17], in[19], in[21], in[23], temp[8], temp[10], temp[12], temp[14]);
    TRANSPOSE_4X4_AVX2(in[25], in[27], in[29], in[31], temp[24], temp[26], temp[28], temp[30]);

    out[0]  = yy_unpacklo_epi128(temp[0], temp[17]);
    out[1]  = yy_unpacklo_epi128(temp[1], temp[9]);
    out[2]  = yy_unpacklo_epi128(temp[2], temp[19]);
    out[3]  = yy_unpacklo_epi128(temp[3], temp[11]);
    out[4]  = yy_unpacklo_epi128(temp[4], temp[21]);
    out[5]  = yy_unpacklo_epi128(temp[5], temp[13]);
    out[6]  = yy_unpacklo_epi128(temp[6], temp[23]);
    out[7]  = yy_unpacklo_epi128(temp[7], temp[15]);
    out[8]  = yy_unpackhi_epi128(temp[0], temp[17]);
    out[9]  = yy_unpackhi_epi128(temp[1], temp[9]);
    out[10] = yy_unpackhi_epi128(temp[2], temp[19]);
    out[11] = yy_unpackhi_epi128(temp[3], temp[11]);
    out[12] = yy_unpackhi_epi128(temp[4], temp[21]);
    out[13] = yy_unpackhi_epi128(temp[5], temp[13]);
    out[14] = yy_unpackhi_epi128(temp[6], temp[23]);
    out[15] = yy_unpackhi_epi128(temp[7], temp[15]);
    out[16] = yy_unpacklo_epi128(temp[16], temp[25]);
    out[17] = yy_unpacklo_epi128(temp[8], temp[24]);
    out[18] = yy_unpacklo_epi128(temp[18], temp[27]);
    out[19] = yy_unpacklo_epi128(temp[10], temp[26]);
    out[20] = yy_unpacklo_epi128(temp[20], temp[29]);
    out[21] = yy_unpacklo_epi128(temp[12], temp[28]);
    out[22] = yy_unpacklo_epi128(temp[22], temp[31]);
    out[23] = yy_unpacklo_epi128(temp[14], temp[30]);
    out[24] = yy_unpackhi_epi128(temp[16], temp[25]);
    out[25] = yy_unpackhi_epi128(temp[8], temp[24]);
    out[26] = yy_unpackhi_epi128(temp[18], temp[27]);
    out[27] = yy_unpackhi_epi128(temp[10], temp[26]);
    out[28] = yy_unpackhi_epi128(temp[20], temp[29]);
    out[29] = yy_unpackhi_epi128(temp[12], temp[28]);
    out[30] = yy_unpackhi_epi128(temp[22], temp[31]);
    out[31] = yy_unpackhi_epi128(temp[14], temp[30]);
}

static INLINE void transpose_32_8x8_avx2(int32_t stride, const __m256i* in, __m256i* out) {
    __m256i out1[8];
    __m256i temp0 = _mm256_unpacklo_epi32(in[0 * stride], in[2 * stride]);
    __m256i temp1 = _mm256_unpackhi_epi32(in[0 * stride], in[2 * stride]);
    __m256i temp2 = _mm256_unpacklo_epi32(in[1 * stride], in[3 * stride]);
    __m256i temp3 = _mm256_unpackhi_epi32(in[1 * stride], in[3 * stride]);
    __m256i temp4 = _mm256_unpacklo_epi32(in[4 * stride], in[6 * stride]);
    __m256i temp5 = _mm256_unpackhi_epi32(in[4 * stride], in[6 * stride]);
    __m256i temp6 = _mm256_unpacklo_epi32(in[5 * stride], in[7 * stride]);
    __m256i temp7 = _mm256_unpackhi_epi32(in[5 * stride], in[7 * stride]);

    out1[0] = _mm256_unpacklo_epi32(temp0, temp2);
    out1[1] = _mm256_unpackhi_epi32(temp0, temp2);
    out1[4] = _mm256_unpacklo_epi32(temp1, temp3);
    out1[5] = _mm256_unpackhi_epi32(temp1, temp3);
    out1[2] = _mm256_unpacklo_epi32(temp4, temp6);
    out1[3] = _mm256_unpackhi_epi32(temp4, temp6);
    out1[6] = _mm256_unpacklo_epi32(temp5, temp7);
    out1[7] = _mm256_unpackhi_epi32(temp5, temp7);

    out[0 * stride] = yy_unpacklo_epi128(out1[0], out1[2]);
    out[1 * stride] = yy_unpacklo_epi128(out1[1], out1[3]);
    out[2 * stride] = yy_unpacklo_epi128(out1[4], out1[6]);
    out[3 * stride] = yy_unpacklo_epi128(out1[5], out1[7]);
    out[4 * stride] = yy_unpackhi_epi128(out1[0], out1[2]);
    out[5 * stride] = yy_unpackhi_epi128(out1[1], out1[3]);
    out[6 * stride] = yy_unpackhi_epi128(out1[4], out1[6]);
    out[7 * stride] = yy_unpackhi_epi128(out1[5], out1[7]);
}

static INLINE void transpose_32_avx2(int32_t txfm_size, const __m256i* input, __m256i* output) {
    const int32_t num_per_256 = 8;
    const int32_t row_size    = txfm_size;
    const int32_t col_size    = txfm_size / num_per_256;
    int32_t       r, c;

    // transpose each 8x8 block internally
    for (r = 0; r < row_size; r += 8) {
        for (c = 0; c < col_size; c++) {
            transpose_32_8x8_avx2(col_size, &input[r * col_size + c], &output[c * 8 * col_size + r / 8]);
        }
    }
}

static INLINE void transpose_8nx8n(const __m256i* input, __m256i* output, const int32_t width, const int32_t height) {
    const int32_t numcol = height >> 3;
    const int32_t numrow = width >> 3;
    __m256i       out1[8];
    for (int32_t j = 0; j < numrow; j++) {
        for (int32_t i = 0; i < numcol; i++) {
            TRANSPOSE_4X4_AVX2(input[i * width + j + (numrow * 0)],
                               input[i * width + j + (numrow * 1)],
                               input[i * width + j + (numrow * 2)],
                               input[i * width + j + (numrow * 3)],
                               out1[0],
                               out1[1],
                               out1[4],
                               out1[5]);
            TRANSPOSE_4X4_AVX2(input[i * width + j + (numrow * 4)],
                               input[i * width + j + (numrow * 5)],
                               input[i * width + j + (numrow * 6)],
                               input[i * width + j + (numrow * 7)],
                               out1[2],
                               out1[3],
                               out1[6],
                               out1[7]);
            output[j * height + i + (numcol * 0)] = yy_unpacklo_epi128(out1[0], out1[2]);
            output[j * height + i + (numcol * 1)] = yy_unpacklo_epi128(out1[1], out1[3]);
            output[j * height + i + (numcol * 2)] = yy_unpacklo_epi128(out1[4], out1[6]);
            output[j * height + i + (numcol * 3)] = yy_unpacklo_epi128(out1[5], out1[7]);
            output[j * height + i + (numcol * 4)] = yy_unpackhi_epi128(out1[0], out1[2]);
            output[j * height + i + (numcol * 5)] = yy_unpackhi_epi128(out1[1], out1[3]);
            output[j * height + i + (numcol * 6)] = yy_unpackhi_epi128(out1[4], out1[6]);
            output[j * height + i + (numcol * 7)] = yy_unpackhi_epi128(out1[5], out1[7]);
        }
    }
}

static INLINE void transpose_8nx8n_N2_half(const __m256i* input, __m256i* output, const int32_t width,
                                           const int32_t height) {
    const int32_t numcol      = height >> 3;
    const int32_t numrow      = width >> 3;
    int32_t       calc_numcol = numcol >> 1;
    if (!calc_numcol) {
        calc_numcol = 1;
    }

    __m256i out1[8];
    for (int32_t j = 0; j < numrow; j++) {
        for (int32_t i = 0; i < calc_numcol; i++) {
            TRANSPOSE_4X4_AVX2(input[i * width + j + (numrow * 0)],
                               input[i * width + j + (numrow * 1)],
                               input[i * width + j + (numrow * 2)],
                               input[i * width + j + (numrow * 3)],
                               out1[0],
                               out1[1],
                               out1[4],
                               out1[5]);
            TRANSPOSE_4X4_AVX2(input[i * width + j + (numrow * 4)],
                               input[i * width + j + (numrow * 5)],
                               input[i * width + j + (numrow * 6)],
                               input[i * width + j + (numrow * 7)],
                               out1[2],
                               out1[3],
                               out1[6],
                               out1[7]);
            output[j * height + i + (numcol * 0)] = yy_unpacklo_epi128(out1[0], out1[2]);
            output[j * height + i + (numcol * 1)] = yy_unpacklo_epi128(out1[1], out1[3]);
            output[j * height + i + (numcol * 2)] = yy_unpacklo_epi128(out1[4], out1[6]);
            output[j * height + i + (numcol * 3)] = yy_unpacklo_epi128(out1[5], out1[7]);
            output[j * height + i + (numcol * 4)] = yy_unpackhi_epi128(out1[0], out1[2]);
            output[j * height + i + (numcol * 5)] = yy_unpackhi_epi128(out1[1], out1[3]);
            output[j * height + i + (numcol * 6)] = yy_unpackhi_epi128(out1[4], out1[6]);
            output[j * height + i + (numcol * 7)] = yy_unpackhi_epi128(out1[5], out1[7]);
        }
    }
}

static INLINE void transpose_8nx8n_N2_quad(const __m256i* input, __m256i* output, const int32_t width,
                                           const int32_t height) {
    const int32_t numcol = height >> 3;
    const int32_t numrow = width >> 3;

    int32_t calc_numcol = numcol >> 1;
    int32_t calc_numrow = numrow >> 1;
    if (!calc_numcol) {
        calc_numcol = 1;
    }
    if (!calc_numrow) {
        calc_numrow = 1;
    }

    __m256i out1[8];
    for (int32_t j = 0; j < calc_numrow; j++) {
        for (int32_t i = 0; i < calc_numcol; i++) {
            TRANSPOSE_4X4_AVX2(input[i * width + j + (numrow * 0)],
                               input[i * width + j + (numrow * 1)],
                               input[i * width + j + (numrow * 2)],
                               input[i * width + j + (numrow * 3)],
                               out1[0],
                               out1[1],
                               out1[4],
                               out1[5]);
            TRANSPOSE_4X4_AVX2(input[i * width + j + (numrow * 4)],
                               input[i * width + j + (numrow * 5)],
                               input[i * width + j + (numrow * 6)],
                               input[i * width + j + (numrow * 7)],
                               out1[2],
                               out1[3],
                               out1[6],
                               out1[7]);
            output[j * height + i + (numcol * 0)] = yy_unpacklo_epi128(out1[0], out1[2]);
            output[j * height + i + (numcol * 1)] = yy_unpacklo_epi128(out1[1], out1[3]);
            output[j * height + i + (numcol * 2)] = yy_unpacklo_epi128(out1[4], out1[6]);
            output[j * height + i + (numcol * 3)] = yy_unpacklo_epi128(out1[5], out1[7]);
            output[j * height + i + (numcol * 4)] = yy_unpackhi_epi128(out1[0], out1[2]);
            output[j * height + i + (numcol * 5)] = yy_unpackhi_epi128(out1[1], out1[3]);
            output[j * height + i + (numcol * 6)] = yy_unpackhi_epi128(out1[4], out1[6]);
            output[j * height + i + (numcol * 7)] = yy_unpackhi_epi128(out1[5], out1[7]);
        }
    }
}

static INLINE void transpose_4x8_avx2(const __m256i* in, __m256i* out) {
    __m256i perm = _mm256_set_epi32(7, 3, 6, 2, 5, 1, 4, 0);

    TRANSPOSE_4X4_AVX2(in[0], in[1], in[2], in[3], out[0], out[1], out[2], out[3]);
    out[0] = _mm256_permutevar8x32_epi32(out[0], perm);
    out[1] = _mm256_permutevar8x32_epi32(out[1], perm);
    out[2] = _mm256_permutevar8x32_epi32(out[2], perm);
    out[3] = _mm256_permutevar8x32_epi32(out[3], perm);
}

static INLINE void transpose_4x16_avx2(const __m256i* in, __m256i* out) {
    __m256i perm = _mm256_set_epi32(7, 3, 6, 2, 5, 1, 4, 0);

    TRANSPOSE_4X4_AVX2(in[0], in[1], in[2], in[3], out[0], out[2], out[4], out[6]);
    TRANSPOSE_4X4_AVX2(in[4], in[5], in[6], in[7], out[1], out[3], out[5], out[7]);

    out[0] = _mm256_permutevar8x32_epi32(out[0], perm);
    out[1] = _mm256_permutevar8x32_epi32(out[1], perm);
    out[2] = _mm256_permutevar8x32_epi32(out[2], perm);
    out[3] = _mm256_permutevar8x32_epi32(out[3], perm);
    out[4] = _mm256_permutevar8x32_epi32(out[4], perm);
    out[5] = _mm256_permutevar8x32_epi32(out[5], perm);
    out[6] = _mm256_permutevar8x32_epi32(out[6], perm);
    out[7] = _mm256_permutevar8x32_epi32(out[7], perm);
}

// Note:
//  rounding = 1 << (bit - 1)
static INLINE __m256i half_btf_avx2(const __m256i* w0, const __m256i* n0, const __m256i* w1, const __m256i* n1,
                                    const __m256i* rounding, int32_t bit) {
    __m256i x, y;

    x = _mm256_mullo_epi32(*w0, *n0);
    y = _mm256_mullo_epi32(*w1, *n1);
    x = _mm256_add_epi32(x, y);
    x = _mm256_add_epi32(x, *rounding);
    x = _mm256_srai_epi32(x, bit);
    return x;
}

static INLINE __m128i half_btf_small(const __m128i* w0, const __m128i* n0, const __m128i* w1, const __m128i* n1,
                                     const __m128i* rounding, int32_t bit) {
    __m128i x, y;

    x = _mm_mullo_epi32(*w0, *n0);
    y = _mm_mullo_epi32(*w1, *n1);
    x = _mm_add_epi32(x, y);
    x = _mm_add_epi32(x, *rounding);
    x = _mm_srai_epi32(x, bit);
    return x;
}

// out0 = in0*w0 + in1*w1
// out1 = -in1*w0 + in0*w1
#define btf_32_type0_avx2_new(ww0, ww1, in0, in1, out0, out1, r, bit) \
    do {                                                              \
        const __m256i in0_w0 = _mm256_mullo_epi32(in0, ww0);          \
        const __m256i in1_w1 = _mm256_mullo_epi32(in1, ww1);          \
        out0                 = _mm256_add_epi32(in0_w0, in1_w1);      \
        out0                 = _mm256_add_epi32(out0, r);             \
        out0                 = _mm256_srai_epi32(out0, bit);          \
        const __m256i in0_w1 = _mm256_mullo_epi32(in0, ww1);          \
        const __m256i in1_w0 = _mm256_mullo_epi32(in1, ww0);          \
        out1                 = _mm256_sub_epi32(in0_w1, in1_w0);      \
        out1                 = _mm256_add_epi32(out1, r);             \
        out1                 = _mm256_srai_epi32(out1, bit);          \
    } while (0)

// out0 = in0*w0 + in1*w1
// out1 = in1*w0 - in0*w1
#define btf_32_type1_avx2_new(ww0, ww1, in0, in1, out0, out1, r, bit)  \
    do {                                                               \
        btf_32_type0_avx2_new(ww1, ww0, in1, in0, out0, out1, r, bit); \
    } while (0)

static INLINE void load_buffer_8x8(const int16_t* input, __m256i* in, int32_t stride, int32_t flipud, int32_t fliplr,
                                   int32_t shift) {
    __m128i temp[8];
    if (!flipud) {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 0 * stride));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 1 * stride));
        temp[2] = _mm_loadu_si128((const __m128i*)(input + 2 * stride));
        temp[3] = _mm_loadu_si128((const __m128i*)(input + 3 * stride));
        temp[4] = _mm_loadu_si128((const __m128i*)(input + 4 * stride));
        temp[5] = _mm_loadu_si128((const __m128i*)(input + 5 * stride));
        temp[6] = _mm_loadu_si128((const __m128i*)(input + 6 * stride));
        temp[7] = _mm_loadu_si128((const __m128i*)(input + 7 * stride));
    } else {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 7 * stride));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 6 * stride));
        temp[2] = _mm_loadu_si128((const __m128i*)(input + 5 * stride));
        temp[3] = _mm_loadu_si128((const __m128i*)(input + 4 * stride));
        temp[4] = _mm_loadu_si128((const __m128i*)(input + 3 * stride));
        temp[5] = _mm_loadu_si128((const __m128i*)(input + 2 * stride));
        temp[6] = _mm_loadu_si128((const __m128i*)(input + 1 * stride));
        temp[7] = _mm_loadu_si128((const __m128i*)(input + 0 * stride));
    }

    if (fliplr) {
        temp[0] = mm_reverse_epi16(temp[0]);
        temp[1] = mm_reverse_epi16(temp[1]);
        temp[2] = mm_reverse_epi16(temp[2]);
        temp[3] = mm_reverse_epi16(temp[3]);
        temp[4] = mm_reverse_epi16(temp[4]);
        temp[5] = mm_reverse_epi16(temp[5]);
        temp[6] = mm_reverse_epi16(temp[6]);
        temp[7] = mm_reverse_epi16(temp[7]);
    }

    in[0] = _mm256_cvtepi16_epi32(temp[0]);
    in[1] = _mm256_cvtepi16_epi32(temp[1]);
    in[2] = _mm256_cvtepi16_epi32(temp[2]);
    in[3] = _mm256_cvtepi16_epi32(temp[3]);
    in[4] = _mm256_cvtepi16_epi32(temp[4]);
    in[5] = _mm256_cvtepi16_epi32(temp[5]);
    in[6] = _mm256_cvtepi16_epi32(temp[6]);
    in[7] = _mm256_cvtepi16_epi32(temp[7]);

    in[0] = _mm256_slli_epi32(in[0], shift);
    in[1] = _mm256_slli_epi32(in[1], shift);
    in[2] = _mm256_slli_epi32(in[2], shift);
    in[3] = _mm256_slli_epi32(in[3], shift);
    in[4] = _mm256_slli_epi32(in[4], shift);
    in[5] = _mm256_slli_epi32(in[5], shift);
    in[6] = _mm256_slli_epi32(in[6], shift);
    in[7] = _mm256_slli_epi32(in[7], shift);
}

static INLINE void load_buffer_8x8_N2(const int16_t* input, __m256i* in, int32_t stride, int32_t flipud, int32_t fliplr,
                                      int32_t shift) {
    __m128i temp[8];
    if (!flipud) {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 0 * stride));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 1 * stride));
        temp[2] = _mm_loadu_si128((const __m128i*)(input + 2 * stride));
        temp[3] = _mm_loadu_si128((const __m128i*)(input + 3 * stride));
        temp[4] = _mm_loadu_si128((const __m128i*)(input + 4 * stride));
        temp[5] = _mm_loadu_si128((const __m128i*)(input + 5 * stride));
        temp[6] = _mm_loadu_si128((const __m128i*)(input + 6 * stride));
        temp[7] = _mm_loadu_si128((const __m128i*)(input + 7 * stride));
    } else {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 7 * stride));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 6 * stride));
        temp[2] = _mm_loadu_si128((const __m128i*)(input + 5 * stride));
        temp[3] = _mm_loadu_si128((const __m128i*)(input + 4 * stride));
        temp[4] = _mm_loadu_si128((const __m128i*)(input + 3 * stride));
        temp[5] = _mm_loadu_si128((const __m128i*)(input + 2 * stride));
        temp[6] = _mm_loadu_si128((const __m128i*)(input + 1 * stride));
        temp[7] = _mm_loadu_si128((const __m128i*)(input + 0 * stride));
    }

    if (fliplr) {
        temp[0] = mm_reverse_epi16(temp[0]);
        temp[1] = mm_reverse_epi16(temp[1]);
        temp[2] = mm_reverse_epi16(temp[2]);
        temp[3] = mm_reverse_epi16(temp[3]);
        temp[4] = mm_reverse_epi16(temp[4]);
        temp[5] = mm_reverse_epi16(temp[5]);
        temp[6] = mm_reverse_epi16(temp[6]);
        temp[7] = mm_reverse_epi16(temp[7]);
    }

    in[0]  = _mm256_cvtepi16_epi32(temp[0]);
    in[2]  = _mm256_cvtepi16_epi32(temp[1]);
    in[4]  = _mm256_cvtepi16_epi32(temp[2]);
    in[6]  = _mm256_cvtepi16_epi32(temp[3]);
    in[8]  = _mm256_cvtepi16_epi32(temp[4]);
    in[10] = _mm256_cvtepi16_epi32(temp[5]);
    in[12] = _mm256_cvtepi16_epi32(temp[6]);
    in[14] = _mm256_cvtepi16_epi32(temp[7]);

    in[0]  = _mm256_slli_epi32(in[0], shift);
    in[2]  = _mm256_slli_epi32(in[2], shift);
    in[4]  = _mm256_slli_epi32(in[4], shift);
    in[6]  = _mm256_slli_epi32(in[6], shift);
    in[8]  = _mm256_slli_epi32(in[8], shift);
    in[10] = _mm256_slli_epi32(in[10], shift);
    in[12] = _mm256_slli_epi32(in[12], shift);
    in[14] = _mm256_slli_epi32(in[14], shift);
}

static INLINE void load_buffer_4x4_avx2(const int16_t* input, __m256i* in, int32_t stride, int32_t flipud,
                                        int32_t fliplr, int32_t shift) {
    if (!flipud) {
        in[0] = _mm256_setr_epi64x(*(uint64_t*)(input + 0 * stride), *(uint64_t*)(input + 1 * stride), 0, 0);
        in[1] = _mm256_setr_epi64x(*(uint64_t*)(input + 2 * stride), *(uint64_t*)(input + 3 * stride), 0, 0);
    } else {
        in[0] = _mm256_setr_epi64x(*(uint64_t*)(input + 3 * stride), *(uint64_t*)(input + 2 * stride), 0, 0);
        in[1] = _mm256_setr_epi64x(*(uint64_t*)(input + 1 * stride), *(uint64_t*)(input + 0 * stride), 0, 0);
    }

    if (fliplr) {
        in[0] = _mm256_shufflelo_epi16(in[0], 0x1b);
        in[0] = _mm256_shufflehi_epi16(in[0], 0x1b);
        in[1] = _mm256_shufflelo_epi16(in[1], 0x1b);
        in[1] = _mm256_shufflehi_epi16(in[1], 0x1b);
    }

    in[0] = _mm256_cvtepi16_epi32(_mm256_castsi256_si128(in[0]));
    in[1] = _mm256_cvtepi16_epi32(_mm256_castsi256_si128(in[1]));

    in[0] = _mm256_slli_epi32(in[0], shift);
    in[1] = _mm256_slli_epi32(in[1], shift);
}

static INLINE void load_buffer_4x8_avx2(const int16_t* input, __m256i* out, int32_t stride, int32_t flipud,
                                        int32_t fliplr, int32_t shift) {
    const int16_t* top_l = input;
    const int16_t* bot_l = input + 4 * stride;

    if (flipud) {
        load_buffer_4x4_avx2(bot_l, out, stride, flipud, fliplr, shift);
        load_buffer_4x4_avx2(top_l, out + 2, stride, flipud, fliplr, shift);
    } else {
        load_buffer_4x4_avx2(top_l, out, stride, flipud, fliplr, shift);
        load_buffer_4x4_avx2(bot_l, out + 2, stride, flipud, fliplr, shift);
    }
}

static INLINE void load_buffer_8x4_avx2(const int16_t* input, __m256i* out, int32_t stride, int32_t flipud,
                                        int32_t fliplr, int32_t shift) {
    const int16_t* top_l = input;
    const int16_t* top_r = input + 4;

    if (fliplr) {
        load_buffer_4x4_avx2(top_r, out, stride, flipud, fliplr, shift);
        load_buffer_4x4_avx2(top_l, out + 2, stride, flipud, fliplr, shift);
    } else {
        load_buffer_4x4_avx2(top_l, out, stride, flipud, fliplr, shift);
        load_buffer_4x4_avx2(top_r, out + 2, stride, flipud, fliplr, shift);
    }
}

static INLINE void load_buffer_4x16_avx2(const int16_t* input, __m256i* out, const int32_t stride, const int32_t flipud,
                                         const int32_t fliplr, const int32_t shift) {
    const int16_t* top_l = input;
    const int16_t* bot_l = input + 8 * stride;

    if (flipud) {
        load_buffer_4x8_avx2(bot_l, out, stride, flipud, fliplr, shift);
        load_buffer_4x8_avx2(top_l, out + 4, stride, flipud, fliplr, shift);
    } else {
        load_buffer_4x8_avx2(top_l, out, stride, flipud, fliplr, shift);
        load_buffer_4x8_avx2(bot_l, out + 4, stride, flipud, fliplr, shift);
    }
}

static INLINE void load_buffer_16x4_avx2(const int16_t* input, __m256i* out, int32_t stride, int32_t flipud,
                                         int32_t fliplr, int32_t shift) {
    const int16_t* top_l = input;
    const int16_t* top_r = input + 8;

    if (fliplr) {
        load_buffer_8x4_avx2(top_r, out, stride, flipud, fliplr, shift);
        load_buffer_8x4_avx2(top_l, out + 4, stride, flipud, fliplr, shift);
    } else {
        load_buffer_8x4_avx2(top_l, out, stride, flipud, fliplr, shift);
        load_buffer_8x4_avx2(top_r, out + 4, stride, flipud, fliplr, shift);
    }
}

static INLINE void col_txfm_8x8_rounding(__m256i* in, int32_t shift) {
    const __m256i rounding = _mm256_set1_epi32(1 << (shift - 1));

    in[0] = _mm256_add_epi32(in[0], rounding);
    in[1] = _mm256_add_epi32(in[1], rounding);
    in[2] = _mm256_add_epi32(in[2], rounding);
    in[3] = _mm256_add_epi32(in[3], rounding);
    in[4] = _mm256_add_epi32(in[4], rounding);
    in[5] = _mm256_add_epi32(in[5], rounding);
    in[6] = _mm256_add_epi32(in[6], rounding);
    in[7] = _mm256_add_epi32(in[7], rounding);

    in[0] = _mm256_srai_epi32(in[0], shift);
    in[1] = _mm256_srai_epi32(in[1], shift);
    in[2] = _mm256_srai_epi32(in[2], shift);
    in[3] = _mm256_srai_epi32(in[3], shift);
    in[4] = _mm256_srai_epi32(in[4], shift);
    in[5] = _mm256_srai_epi32(in[5], shift);
    in[6] = _mm256_srai_epi32(in[6], shift);
    in[7] = _mm256_srai_epi32(in[7], shift);
}

static void fidtx8x8_avx2(const __m256i* in, __m256i* out, int8_t bit, int32_t col_num) {
    (void)bit;
    out[0] = _mm256_slli_epi32(in[0 * col_num], 1);
    out[1] = _mm256_slli_epi32(in[1 * col_num], 1);
    out[2] = _mm256_slli_epi32(in[2 * col_num], 1);
    out[3] = _mm256_slli_epi32(in[3 * col_num], 1);
    out[4] = _mm256_slli_epi32(in[4 * col_num], 1);
    out[5] = _mm256_slli_epi32(in[5 * col_num], 1);
    out[6] = _mm256_slli_epi32(in[6 * col_num], 1);
    out[7] = _mm256_slli_epi32(in[7 * col_num], 1);
}

static INLINE void fidtx16x8_avx2(const __m256i* in, __m256i* out, int8_t bit, int32_t col_num) {
    (void)bit;
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 2 * 5793; // 2 * new_sqrt2
    const __m256i newsqrt  = _mm256_set1_epi32(sqrt);
    const __m256i rounding = _mm256_set1_epi32(1 << (bits - 1));
    __m256i       temp;
    int32_t       num_iters = 8 * col_num;
    for (int32_t i = 0; i < num_iters; i++) {
        temp   = _mm256_mullo_epi32(in[i], newsqrt);
        temp   = _mm256_add_epi32(temp, rounding);
        out[i] = _mm256_srai_epi32(temp, bits);
    }
}

static INLINE void write_buffer_4x8(const __m256i* res, int32_t* output) {
    _mm256_storeu_si256((__m256i*)(output + 0 * 8), res[0]);
    _mm256_storeu_si256((__m256i*)(output + 1 * 8), res[1]);
    _mm256_storeu_si256((__m256i*)(output + 2 * 8), res[2]);
    _mm256_storeu_si256((__m256i*)(output + 3 * 8), res[3]);
}

static INLINE void write_buffer_8x8(const __m256i* res, int32_t* output) {
    _mm256_storeu_si256((__m256i*)(output + 0 * 8), res[0]);
    _mm256_storeu_si256((__m256i*)(output + 1 * 8), res[1]);
    _mm256_storeu_si256((__m256i*)(output + 2 * 8), res[2]);
    _mm256_storeu_si256((__m256i*)(output + 3 * 8), res[3]);

    _mm256_storeu_si256((__m256i*)(output + 4 * 8), res[4]);
    _mm256_storeu_si256((__m256i*)(output + 5 * 8), res[5]);
    _mm256_storeu_si256((__m256i*)(output + 6 * 8), res[6]);
    _mm256_storeu_si256((__m256i*)(output + 7 * 8), res[7]);
}

static void fdct8x8_avx2(const __m256i* in, __m256i* out, int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospi56  = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi8   = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi24  = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospi40  = _mm256_set1_epi32(cospi[40]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    __m256i        u[8], v[8];

    // stage 0
    // stage 1
    u[0] = _mm256_add_epi32(in[0 * col_num], in[7 * col_num]);
    v[7] = _mm256_sub_epi32(in[0 * col_num], in[7 * col_num]);
    u[1] = _mm256_add_epi32(in[1 * col_num], in[6 * col_num]);
    u[6] = _mm256_sub_epi32(in[1 * col_num], in[6 * col_num]);
    u[2] = _mm256_add_epi32(in[2 * col_num], in[5 * col_num]);
    u[5] = _mm256_sub_epi32(in[2 * col_num], in[5 * col_num]);
    u[3] = _mm256_add_epi32(in[3 * col_num], in[4 * col_num]);
    v[4] = _mm256_sub_epi32(in[3 * col_num], in[4 * col_num]);

    // stage 2
    v[0] = _mm256_add_epi32(u[0], u[3]);
    v[3] = _mm256_sub_epi32(u[0], u[3]);
    v[1] = _mm256_add_epi32(u[1], u[2]);
    v[2] = _mm256_sub_epi32(u[1], u[2]);

    v[5] = _mm256_mullo_epi32(u[5], cospim32);
    v[6] = _mm256_mullo_epi32(u[6], cospi32);
    v[5] = _mm256_add_epi32(v[5], v[6]);
    v[5] = _mm256_add_epi32(v[5], rnding);
    v[5] = _mm256_srai_epi32(v[5], bit);

    u[0] = _mm256_mullo_epi32(u[5], cospi32);
    v[6] = _mm256_mullo_epi32(u[6], cospim32);
    v[6] = _mm256_sub_epi32(u[0], v[6]);
    v[6] = _mm256_add_epi32(v[6], rnding);
    v[6] = _mm256_srai_epi32(v[6], bit);

    // stage 3
    // type 0
    v[0]             = _mm256_mullo_epi32(v[0], cospi32);
    v[1]             = _mm256_mullo_epi32(v[1], cospi32);
    u[0]             = _mm256_add_epi32(v[0], v[1]);
    u[0]             = _mm256_add_epi32(u[0], rnding);
    out[0 * col_num] = _mm256_srai_epi32(u[0], bit);

    u[1]             = _mm256_sub_epi32(v[0], v[1]);
    u[1]             = _mm256_add_epi32(u[1], rnding);
    out[4 * col_num] = _mm256_srai_epi32(u[1], bit);

    // type 1
    v[0]             = _mm256_mullo_epi32(v[2], cospi48);
    v[1]             = _mm256_mullo_epi32(v[3], cospi16);
    u[2]             = _mm256_add_epi32(v[0], v[1]);
    u[2]             = _mm256_add_epi32(u[2], rnding);
    out[2 * col_num] = _mm256_srai_epi32(u[2], bit);

    v[0]             = _mm256_mullo_epi32(v[2], cospi16);
    v[1]             = _mm256_mullo_epi32(v[3], cospi48);
    u[3]             = _mm256_sub_epi32(v[1], v[0]);
    u[3]             = _mm256_add_epi32(u[3], rnding);
    out[6 * col_num] = _mm256_srai_epi32(u[3], bit);

    u[4] = _mm256_add_epi32(v[4], v[5]);
    u[5] = _mm256_sub_epi32(v[4], v[5]);
    u[6] = _mm256_sub_epi32(v[7], v[6]);
    u[7] = _mm256_add_epi32(v[7], v[6]);

    // stage 4
    // stage 5
    v[0]             = _mm256_mullo_epi32(u[4], cospi56);
    v[1]             = _mm256_mullo_epi32(u[7], cospi8);
    v[0]             = _mm256_add_epi32(v[0], v[1]);
    v[0]             = _mm256_add_epi32(v[0], rnding);
    out[1 * col_num] = _mm256_srai_epi32(v[0], bit);

    v[0]             = _mm256_mullo_epi32(u[4], cospi8);
    v[1]             = _mm256_mullo_epi32(u[7], cospi56);
    v[0]             = _mm256_sub_epi32(v[1], v[0]);
    v[0]             = _mm256_add_epi32(v[0], rnding);
    out[7 * col_num] = _mm256_srai_epi32(v[0], bit);

    v[0]             = _mm256_mullo_epi32(u[5], cospi24);
    v[1]             = _mm256_mullo_epi32(u[6], cospi40);
    v[0]             = _mm256_add_epi32(v[0], v[1]);
    v[0]             = _mm256_add_epi32(v[0], rnding);
    out[5 * col_num] = _mm256_srai_epi32(v[0], bit);

    v[0]             = _mm256_mullo_epi32(u[5], cospi40);
    v[1]             = _mm256_mullo_epi32(u[6], cospi24);
    v[0]             = _mm256_sub_epi32(v[1], v[0]);
    v[0]             = _mm256_add_epi32(v[0], rnding);
    out[3 * col_num] = _mm256_srai_epi32(v[0], bit);
}

static void fadst8x8_avx2(const __m256i* in, __m256i* out, int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospi4   = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospim4  = _mm256_set1_epi32(-cospi[4]);
    const __m256i  cospi60  = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi20  = _mm256_set1_epi32(cospi[20]);
    const __m256i  cospim20 = _mm256_set1_epi32(-cospi[20]);
    const __m256i  cospi44  = _mm256_set1_epi32(cospi[44]);
    const __m256i  cospi28  = _mm256_set1_epi32(cospi[28]);
    const __m256i  cospi36  = _mm256_set1_epi32(cospi[36]);
    const __m256i  cospim36 = _mm256_set1_epi32(-cospi[36]);
    const __m256i  cospi52  = _mm256_set1_epi32(cospi[52]);
    const __m256i  cospim52 = _mm256_set1_epi32(-cospi[52]);
    const __m256i  cospi12  = _mm256_set1_epi32(cospi[12]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  zero     = _mm256_setzero_si256();
    __m256i        u0, u1, u2, u3, u4, u5, u6, u7;
    __m256i        v0, v1, v2, v3, v4, v5, v6, v7;
    __m256i        x, y;

    u0 = in[0 * col_num];
    u1 = _mm256_sub_epi32(zero, in[7 * col_num]);
    u2 = _mm256_sub_epi32(zero, in[3 * col_num]);
    u3 = in[4 * col_num];
    u4 = _mm256_sub_epi32(zero, in[1 * col_num]);
    u5 = in[6 * col_num];
    u6 = in[2 * col_num];
    u7 = _mm256_sub_epi32(zero, in[5 * col_num]);

    // stage 2
    v0 = u0;
    v1 = u1;

    x  = _mm256_mullo_epi32(u2, cospi32);
    y  = _mm256_mullo_epi32(u3, cospi32);
    v2 = _mm256_add_epi32(x, y);
    v2 = _mm256_add_epi32(v2, rnding);
    v2 = _mm256_srai_epi32(v2, bit);

    v3 = _mm256_sub_epi32(x, y);
    v3 = _mm256_add_epi32(v3, rnding);
    v3 = _mm256_srai_epi32(v3, bit);

    v4 = u4;
    v5 = u5;

    x  = _mm256_mullo_epi32(u6, cospi32);
    y  = _mm256_mullo_epi32(u7, cospi32);
    v6 = _mm256_add_epi32(x, y);
    v6 = _mm256_add_epi32(v6, rnding);
    v6 = _mm256_srai_epi32(v6, bit);

    v7 = _mm256_sub_epi32(x, y);
    v7 = _mm256_add_epi32(v7, rnding);
    v7 = _mm256_srai_epi32(v7, bit);

    // stage 3
    u0 = _mm256_add_epi32(v0, v2);
    u1 = _mm256_add_epi32(v1, v3);
    u2 = _mm256_sub_epi32(v0, v2);
    u3 = _mm256_sub_epi32(v1, v3);
    u4 = _mm256_add_epi32(v4, v6);
    u5 = _mm256_add_epi32(v5, v7);
    u6 = _mm256_sub_epi32(v4, v6);
    u7 = _mm256_sub_epi32(v5, v7);

    // stage 4
    v0 = u0;
    v1 = u1;
    v2 = u2;
    v3 = u3;

    x  = _mm256_mullo_epi32(u4, cospi16);
    y  = _mm256_mullo_epi32(u5, cospi48);
    v4 = _mm256_add_epi32(x, y);
    v4 = _mm256_add_epi32(v4, rnding);
    v4 = _mm256_srai_epi32(v4, bit);

    x  = _mm256_mullo_epi32(u4, cospi48);
    y  = _mm256_mullo_epi32(u5, cospim16);
    v5 = _mm256_add_epi32(x, y);
    v5 = _mm256_add_epi32(v5, rnding);
    v5 = _mm256_srai_epi32(v5, bit);

    x  = _mm256_mullo_epi32(u6, cospim48);
    y  = _mm256_mullo_epi32(u7, cospi16);
    v6 = _mm256_add_epi32(x, y);
    v6 = _mm256_add_epi32(v6, rnding);
    v6 = _mm256_srai_epi32(v6, bit);

    x  = _mm256_mullo_epi32(u6, cospi16);
    y  = _mm256_mullo_epi32(u7, cospi48);
    v7 = _mm256_add_epi32(x, y);
    v7 = _mm256_add_epi32(v7, rnding);
    v7 = _mm256_srai_epi32(v7, bit);

    // stage 5
    u0 = _mm256_add_epi32(v0, v4);
    u1 = _mm256_add_epi32(v1, v5);
    u2 = _mm256_add_epi32(v2, v6);
    u3 = _mm256_add_epi32(v3, v7);
    u4 = _mm256_sub_epi32(v0, v4);
    u5 = _mm256_sub_epi32(v1, v5);
    u6 = _mm256_sub_epi32(v2, v6);
    u7 = _mm256_sub_epi32(v3, v7);

    // stage 6
    x  = _mm256_mullo_epi32(u0, cospi4);
    y  = _mm256_mullo_epi32(u1, cospi60);
    v0 = _mm256_add_epi32(x, y);
    v0 = _mm256_add_epi32(v0, rnding);
    v0 = _mm256_srai_epi32(v0, bit);

    x  = _mm256_mullo_epi32(u0, cospi60);
    y  = _mm256_mullo_epi32(u1, cospim4);
    v1 = _mm256_add_epi32(x, y);
    v1 = _mm256_add_epi32(v1, rnding);
    v1 = _mm256_srai_epi32(v1, bit);

    x  = _mm256_mullo_epi32(u2, cospi20);
    y  = _mm256_mullo_epi32(u3, cospi44);
    v2 = _mm256_add_epi32(x, y);
    v2 = _mm256_add_epi32(v2, rnding);
    v2 = _mm256_srai_epi32(v2, bit);

    x  = _mm256_mullo_epi32(u2, cospi44);
    y  = _mm256_mullo_epi32(u3, cospim20);
    v3 = _mm256_add_epi32(x, y);
    v3 = _mm256_add_epi32(v3, rnding);
    v3 = _mm256_srai_epi32(v3, bit);

    x  = _mm256_mullo_epi32(u4, cospi36);
    y  = _mm256_mullo_epi32(u5, cospi28);
    v4 = _mm256_add_epi32(x, y);
    v4 = _mm256_add_epi32(v4, rnding);
    v4 = _mm256_srai_epi32(v4, bit);

    x  = _mm256_mullo_epi32(u4, cospi28);
    y  = _mm256_mullo_epi32(u5, cospim36);
    v5 = _mm256_add_epi32(x, y);
    v5 = _mm256_add_epi32(v5, rnding);
    v5 = _mm256_srai_epi32(v5, bit);

    x  = _mm256_mullo_epi32(u6, cospi52);
    y  = _mm256_mullo_epi32(u7, cospi12);
    v6 = _mm256_add_epi32(x, y);
    v6 = _mm256_add_epi32(v6, rnding);
    v6 = _mm256_srai_epi32(v6, bit);

    x  = _mm256_mullo_epi32(u6, cospi12);
    y  = _mm256_mullo_epi32(u7, cospim52);
    v7 = _mm256_add_epi32(x, y);
    v7 = _mm256_add_epi32(v7, rnding);
    v7 = _mm256_srai_epi32(v7, bit);

    // stage 7
    out[0 * col_num] = v1;
    out[1 * col_num] = v6;
    out[2 * col_num] = v3;
    out[3 * col_num] = v4;
    out[4 * col_num] = v5;
    out[5 * col_num] = v2;
    out[6 * col_num] = v7;
    out[7 * col_num] = v0;
}

void svt_av1_fwd_txfm2d_8x8_avx2(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[8], out[8];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_8X8];
    const int32_t txw_idx = get_txw_idx(TX_8X8);
    const int32_t txh_idx = get_txh_idx(TX_8X8);

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fdct8x8_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_avx2(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fdct8x8_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_avx2(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fadst8x8_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_avx2(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fadst8x8_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_avx2(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fdct8x8_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_avx2(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fdct8x8_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fadst8x8_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_avx2(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 1, shift[0]);
        fadst8x8_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fadst8x8_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_avx2(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fadst8x8_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fadst8x8_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_avx2(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fadst8x8_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_avx2(out, in);
        write_buffer_8x8(in, coeff);
        break;
    case IDTX:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8x8_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(out, -shift[1]);
        fidtx8x8_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8(out, coeff);
        break;
    case V_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(out, -shift[1]);
        fidtx8x8_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8(out, coeff);
        break;
    case H_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8x8_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fdct8x8_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_avx2(in, out);
        write_buffer_8x8(out, coeff);
        break;
    case V_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(out, -shift[1]);
        fidtx8x8_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8(out, coeff);
        break;
    case H_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8x8_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fadst8x8_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_avx2(in, out);
        write_buffer_8x8(out, coeff);
        break;
    case V_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(out, -shift[1]);
        fidtx8x8_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8(out, coeff);
        break;
    case H_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fidtx8x8_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fadst8x8_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_avx2(in, out);
        write_buffer_8x8(out, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

static INLINE void convert_8x8_to_16x16(const __m256i* in, __m256i* out) {
    int32_t row_index = 0;
    int32_t dst_index = 0;
    int32_t src_index = 0;

    // row 0, 1, .., 7
    do {
        out[dst_index]     = in[src_index];
        out[dst_index + 1] = in[src_index + 8];
        dst_index += 2;
        src_index += 1;
        row_index += 1;
    } while (row_index < 8);

    // row 8, 9, ..., 15
    src_index += 8;
    do {
        out[dst_index]     = in[src_index];
        out[dst_index + 1] = in[src_index + 8];
        dst_index += 2;
        src_index += 1;
        row_index += 1;
    } while (row_index < 16);
}

static INLINE void load_buffer_16x16(const int16_t* input, __m256i* out, int32_t stride, int32_t flipud, int32_t fliplr,
                                     int32_t shift) {
    __m256i in[32];
    // Load 4 8x8 blocks
    const int16_t* top_l = input;
    const int16_t* top_r = input + 8;
    const int16_t* bot_l = input + 8 * stride;
    const int16_t* bot_r = input + 8 * stride + 8;

    const int16_t* tmp;

    if (flipud) {
        // Swap left columns
        tmp   = top_l;
        top_l = bot_l;
        bot_l = tmp;
        // Swap right columns
        tmp   = top_r;
        top_r = bot_r;
        bot_r = tmp;
    }

    if (fliplr) {
        // Swap top rows
        tmp   = top_l;
        top_l = top_r;
        top_r = tmp;
        // Swap bottom rows
        tmp   = bot_l;
        bot_l = bot_r;
        bot_r = tmp;
    }

    // load first 8 columns
    load_buffer_8x8(top_l, &in[0], stride, flipud, fliplr, shift);
    load_buffer_8x8(bot_l, &in[16], stride, flipud, fliplr, shift);

    // load second 8 columns
    load_buffer_8x8(top_r, &in[8], stride, flipud, fliplr, shift);
    load_buffer_8x8(bot_r, &in[24], stride, flipud, fliplr, shift);

    convert_8x8_to_16x16(in, out);
}

static AOM_FORCE_INLINE void load_buffer_16x16_N2(const int16_t* input, __m256i* out, int32_t stride, int32_t flipud,
                                                  int32_t fliplr, int32_t shift) {
    // Load 4 8x8 blocks
    const int16_t* top_l = input;
    const int16_t* top_r = input + 8;
    const int16_t* bot_l = input + 8 * stride;
    const int16_t* bot_r = input + 8 * stride + 8;

    const int16_t* tmp;

    if (flipud) {
        // Swap left columns
        tmp   = top_l;
        top_l = bot_l;
        bot_l = tmp;
        // Swap right columns
        tmp   = top_r;
        top_r = bot_r;
        bot_r = tmp;
    }

    if (fliplr) {
        // Swap top rows
        tmp   = top_l;
        top_l = top_r;
        top_r = tmp;
        // Swap bottom rows
        tmp   = bot_l;
        bot_l = bot_r;
        bot_r = tmp;
    }

    // load first 8 columns
    load_buffer_8x8_N2(top_l, out, stride, flipud, fliplr, shift);
    load_buffer_8x8_N2(bot_l, &out[16], stride, flipud, fliplr, shift);
}

static AOM_FORCE_INLINE void load_buffer_16x16_N2_H(const int16_t* input, __m256i* out, int32_t stride, int32_t flipud,
                                                    int32_t fliplr, int32_t shift) {
    // Load 4 8x8 blocks
    const int16_t* top_l = input;
    const int16_t* top_r = input + 8;
    const int16_t* bot_l = input + 8 * stride;
    const int16_t* bot_r = input + 8 * stride + 8;

    const int16_t* tmp;

    if (flipud) {
        // Swap left columns
        tmp   = top_l;
        top_l = bot_l;
        bot_l = tmp;
        // Swap right columns
        tmp   = top_r;
        top_r = bot_r;
        bot_r = tmp;
    }

    if (fliplr) {
        // Swap top rows
        tmp   = top_l;
        top_l = top_r;
        top_r = tmp;
        // Swap bottom rows
        tmp   = bot_l;
        bot_l = bot_r;
        bot_r = tmp;
    }

    // load first 8 columns
    load_buffer_8x8_N2(top_l, out, stride, flipud, fliplr, shift);

    //// load second 8 columns
    load_buffer_8x8_N2(top_r, &out[1], stride, flipud, fliplr, shift);
}

static AOM_FORCE_INLINE void load_buffer_16x16_N2_half(const int16_t* input, __m256i* out, int32_t stride,
                                                       int32_t flipud, int32_t fliplr, int32_t shift) {
    // Load 4 8x8 blocks
    const int16_t* top_l = input;
    const int16_t* top_r = input + 8;
    const int16_t* bot_l = input + 8 * stride;
    const int16_t* bot_r = input + 8 * stride + 8;

    const int16_t* tmp;

    if (flipud) {
        // Swap left columns
        tmp   = top_l;
        top_l = bot_l;
        bot_l = tmp;
        // Swap right columns
        tmp   = top_r;
        top_r = bot_r;
        bot_r = tmp;
    }

    if (fliplr) {
        // Swap top rows
        tmp   = top_l;
        top_l = top_r;
        top_r = tmp;
        // Swap bottom rows
        tmp   = bot_l;
        bot_l = bot_r;
        bot_r = tmp;
    }

    // load first 8 columns
    load_buffer_8x8_N2(top_l, out /*& in[0]*/, stride, flipud, fliplr, shift);
}

static INLINE void col_txfm_16x16_rounding(__m256i* in, int32_t shift) {
    col_txfm_8x8_rounding(&in[0], shift);
    col_txfm_8x8_rounding(&in[8], shift);
    col_txfm_8x8_rounding(&in[16], shift);
    col_txfm_8x8_rounding(&in[24], shift);
}

static void fidtx16x16_avx2(const __m256i* in, __m256i* out, int8_t bit, int32_t col_num) {
    (void)bit;
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 2 * 5793; // 2 * new_sqrt2
    const __m256i newsqrt  = _mm256_set1_epi32(sqrt);
    const __m256i rounding = _mm256_set1_epi32(1 << (bits - 1));
    __m256i       temp;
    int32_t       num_iters = 16 * col_num;
    for (int32_t i = 0; i < num_iters; i++) {
        temp   = _mm256_mullo_epi32(in[i], newsqrt);
        temp   = _mm256_add_epi32(temp, rounding);
        out[i] = _mm256_srai_epi32(temp, bits);
    }
}

static INLINE void write_buffer_16x16(const __m256i* res, int32_t* output) {
    int32_t fact = -1, index = -1;
    for (int32_t i = 0; i < 8; i++) {
        _mm256_storeu_si256((__m256i*)(output + (++fact) * 16), res[++index]);
        _mm256_storeu_si256((__m256i*)(output + (fact) * 16 + 8), res[++index]);
        _mm256_storeu_si256((__m256i*)(output + (++fact) * 16), res[++index]);
        _mm256_storeu_si256((__m256i*)(output + (fact) * 16 + 8), res[++index]);
    }
}

static INLINE void fdct4x8_row_avx2(__m256i* input, __m256i* output, int32_t bit, const int32_t num_col) {
    const int32_t* cospi   = cospi_arr(bit);
    const __m256i  cospi32 = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi48 = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16 = _mm256_set1_epi32(cospi[16]);
    const __m256i  rnding  = _mm256_set1_epi32(1 << (bit - 1));
    __m256i        in[4];
    __m256i        out[4];
    __m256i        s0, s1, s2, s3;
    __m256i        u0, u1, u2, u3;
    __m256i        v0, v1, v2, v3;
    int32_t        endidx = 3 * num_col;

    in[0] = yy_unpacklo_epi128(input[0], input[2]);
    in[1] = yy_unpackhi_epi128(input[0], input[2]);
    in[2] = yy_unpacklo_epi128(input[1], input[3]);
    in[3] = yy_unpackhi_epi128(input[1], input[3]);

    s0 = _mm256_add_epi32(in[0], in[endidx]);
    s3 = _mm256_sub_epi32(in[0], in[endidx]);
    endidx -= num_col;
    s1 = _mm256_add_epi32(in[num_col], in[endidx]);
    s2 = _mm256_sub_epi32(in[num_col], in[endidx]);

    // btf_32_sse4_1_type0(cospi32, cospi32, s[01], u[02], bit);
    u0 = _mm256_mullo_epi32(s0, cospi32);
    u1 = _mm256_mullo_epi32(s1, cospi32);
    u2 = _mm256_add_epi32(u0, u1);
    v0 = _mm256_sub_epi32(u0, u1);

    u3 = _mm256_add_epi32(u2, rnding);
    v1 = _mm256_add_epi32(v0, rnding);

    u0 = _mm256_srai_epi32(u3, bit);
    u2 = _mm256_srai_epi32(v1, bit);

    // btf_32_sse4_1_type1(cospi48, cospi16, s[23], u[13], bit);
    v0 = _mm256_mullo_epi32(s2, cospi48);
    v1 = _mm256_mullo_epi32(s3, cospi16);
    v2 = _mm256_add_epi32(v0, v1);

    v3 = _mm256_add_epi32(v2, rnding);
    u1 = _mm256_srai_epi32(v3, bit);

    v0 = _mm256_mullo_epi32(s2, cospi16);
    v1 = _mm256_mullo_epi32(s3, cospi48);
    v2 = _mm256_sub_epi32(v1, v0);

    v3 = _mm256_add_epi32(v2, rnding);
    u3 = _mm256_srai_epi32(v3, bit);

    // Note: shift[1] and shift[2] are zeros

    // Transpose 4x4 32-bit
    v0 = _mm256_unpacklo_epi32(u0, u1);
    v1 = _mm256_unpackhi_epi32(u0, u1);
    v2 = _mm256_unpacklo_epi32(u2, u3);
    v3 = _mm256_unpackhi_epi32(u2, u3);

    out[0] = _mm256_unpacklo_epi64(v0, v2);
    out[1] = _mm256_unpackhi_epi64(v0, v2);
    out[2] = _mm256_unpacklo_epi64(v1, v3);
    out[3] = _mm256_unpackhi_epi64(v1, v3);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static INLINE void fdct4x8_col_avx2(__m256i* in, __m256i* output, int32_t bit, const int32_t num_col) {
    const int32_t* cospi   = cospi_arr(bit);
    const __m256i  cospi32 = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi48 = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16 = _mm256_set1_epi32(cospi[16]);
    const __m256i  rnding  = _mm256_set1_epi32(1 << (bit - 1));
    __m256i        s0, s1, s2, s3;
    __m256i        u0, u1, u2, u3;
    __m256i        v0, v1, v2, v3;
    __m256i        out[4];

    int32_t endidx = 3 * num_col;
    s0             = _mm256_add_epi32(in[0], in[endidx]);
    s3             = _mm256_sub_epi32(in[0], in[endidx]);
    endidx -= num_col;
    s1 = _mm256_add_epi32(in[num_col], in[endidx]);
    s2 = _mm256_sub_epi32(in[num_col], in[endidx]);

    // btf_32_sse4_1_type0(cospi32, cospi32, s[01], u[02], bit);
    u0 = _mm256_mullo_epi32(s0, cospi32);
    u1 = _mm256_mullo_epi32(s1, cospi32);
    u2 = _mm256_add_epi32(u0, u1);
    v0 = _mm256_sub_epi32(u0, u1);

    u3 = _mm256_add_epi32(u2, rnding);
    v1 = _mm256_add_epi32(v0, rnding);

    u0 = _mm256_srai_epi32(u3, bit);
    u2 = _mm256_srai_epi32(v1, bit);

    // btf_32_sse4_1_type1(cospi48, cospi16, s[23], u[13], bit);
    v0 = _mm256_mullo_epi32(s2, cospi48);
    v1 = _mm256_mullo_epi32(s3, cospi16);
    v2 = _mm256_add_epi32(v0, v1);

    v3 = _mm256_add_epi32(v2, rnding);
    u1 = _mm256_srai_epi32(v3, bit);

    v0 = _mm256_mullo_epi32(s2, cospi16);
    v1 = _mm256_mullo_epi32(s3, cospi48);
    v2 = _mm256_sub_epi32(v1, v0);

    v3 = _mm256_add_epi32(v2, rnding);
    u3 = _mm256_srai_epi32(v3, bit);

    // Note: shift[1] and shift[2] are zeros

    // Transpose 4x4 32-bit
    v0 = _mm256_unpacklo_epi32(u0, u1);
    v1 = _mm256_unpackhi_epi32(u0, u1);
    v2 = _mm256_unpacklo_epi32(u2, u3);
    v3 = _mm256_unpackhi_epi32(u2, u3);

    out[0] = _mm256_unpacklo_epi64(v0, v2);
    out[1] = _mm256_unpackhi_epi64(v0, v2);
    out[2] = _mm256_unpacklo_epi64(v1, v3);
    out[3] = _mm256_unpackhi_epi64(v1, v3);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static INLINE void fdct16x4_avx2(__m256i* input, __m256i* output, int32_t bit) {
    __m128i* in  = (__m128i*)input;
    __m128i* out = (__m128i*)output;

    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospi4   = _mm_set1_epi32(cospi[4]);
    const __m128i  cospi28  = _mm_set1_epi32(cospi[28]);
    const __m128i  cospi36  = _mm_set1_epi32(cospi[36]);
    const __m128i  cospi44  = _mm_set1_epi32(cospi[44]);
    const __m128i  cospi20  = _mm_set1_epi32(cospi[20]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[16], v[16], x;

    // stage 0
    // stage 1
    u[0]  = _mm_add_epi32(in[0], in[15]);
    v[15] = _mm_sub_epi32(in[0], in[15]);
    u[1]  = _mm_add_epi32(in[1], in[14]);
    v[14] = _mm_sub_epi32(in[1], in[14]);
    u[2]  = _mm_add_epi32(in[2], in[13]);
    u[13] = _mm_sub_epi32(in[2], in[13]);
    u[3]  = _mm_add_epi32(in[3], in[12]);
    u[12] = _mm_sub_epi32(in[3], in[12]);
    u[4]  = _mm_add_epi32(in[4], in[11]);
    u[11] = _mm_sub_epi32(in[4], in[11]);
    u[5]  = _mm_add_epi32(in[5], in[10]);
    u[10] = _mm_sub_epi32(in[5], in[10]);
    u[6]  = _mm_add_epi32(in[6], in[9]);
    v[9]  = _mm_sub_epi32(in[6], in[9]);
    u[7]  = _mm_add_epi32(in[7], in[8]);
    v[8]  = _mm_sub_epi32(in[7], in[8]);

    // stage 2
    v[0] = _mm_add_epi32(u[0], u[7]);
    u[7] = _mm_sub_epi32(u[0], u[7]);
    v[1] = _mm_add_epi32(u[1], u[6]);
    v[6] = _mm_sub_epi32(u[1], u[6]);
    v[2] = _mm_add_epi32(u[2], u[5]);
    v[5] = _mm_sub_epi32(u[2], u[5]);
    v[3] = _mm_add_epi32(u[3], u[4]);
    u[4] = _mm_sub_epi32(u[3], u[4]);

    v[10] = _mm_mullo_epi32(u[10], cospim32);
    x     = _mm_mullo_epi32(u[13], cospi32);
    v[10] = _mm_add_epi32(v[10], x);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[13] = _mm_mullo_epi32(u[10], cospi32);
    x     = _mm_mullo_epi32(u[13], cospim32);
    v[13] = _mm_sub_epi32(v[13], x);
    v[13] = _mm_add_epi32(v[13], rnding);
    v[13] = _mm_srai_epi32(v[13], bit);

    v[11] = _mm_mullo_epi32(u[11], cospim32);
    x     = _mm_mullo_epi32(u[12], cospi32);
    v[11] = _mm_add_epi32(v[11], x);
    v[11] = _mm_add_epi32(v[11], rnding);
    v[11] = _mm_srai_epi32(v[11], bit);

    v[12] = _mm_mullo_epi32(u[11], cospi32);
    x     = _mm_mullo_epi32(u[12], cospim32);
    v[12] = _mm_sub_epi32(v[12], x);
    v[12] = _mm_add_epi32(v[12], rnding);
    v[12] = _mm_srai_epi32(v[12], bit);

    // stage 3
    u[0] = _mm_add_epi32(v[0], v[3]);
    u[3] = _mm_sub_epi32(v[0], v[3]);
    u[1] = _mm_add_epi32(v[1], v[2]);
    u[2] = _mm_sub_epi32(v[1], v[2]);

    u[5] = _mm_mullo_epi32(v[5], cospim32);
    x    = _mm_mullo_epi32(v[6], cospi32);
    u[5] = _mm_add_epi32(u[5], x);
    u[5] = _mm_add_epi32(u[5], rnding);
    u[5] = _mm_srai_epi32(u[5], bit);

    u[6] = _mm_mullo_epi32(v[5], cospi32);
    x    = _mm_mullo_epi32(v[6], cospim32);
    u[6] = _mm_sub_epi32(u[6], x);
    u[6] = _mm_add_epi32(u[6], rnding);
    u[6] = _mm_srai_epi32(u[6], bit);

    u[8]  = _mm_add_epi32(v[8], v[11]);
    v[11] = _mm_sub_epi32(v[8], v[11]);
    u[9]  = _mm_add_epi32(v[9], v[10]);
    u[10] = _mm_sub_epi32(v[9], v[10]);
    u[12] = _mm_sub_epi32(v[15], v[12]);
    v[15] = _mm_add_epi32(v[15], v[12]);
    u[13] = _mm_sub_epi32(v[14], v[13]);
    u[14] = _mm_add_epi32(v[14], v[13]);

    // stage 4
    u[0]   = _mm_mullo_epi32(u[0], cospi32);
    u[1]   = _mm_mullo_epi32(u[1], cospi32);
    v[0]   = _mm_add_epi32(u[0], u[1]);
    v[0]   = _mm_add_epi32(v[0], rnding);
    out[0] = _mm_srai_epi32(v[0], bit);

    v[1]   = _mm_sub_epi32(u[0], u[1]);
    v[1]   = _mm_add_epi32(v[1], rnding);
    out[8] = _mm_srai_epi32(v[1], bit);

    v[2]   = _mm_mullo_epi32(u[2], cospi48);
    x      = _mm_mullo_epi32(u[3], cospi16);
    v[2]   = _mm_add_epi32(v[2], x);
    v[2]   = _mm_add_epi32(v[2], rnding);
    out[4] = _mm_srai_epi32(v[2], bit);

    v[3]    = _mm_mullo_epi32(u[2], cospi16);
    x       = _mm_mullo_epi32(u[3], cospi48);
    v[3]    = _mm_sub_epi32(x, v[3]);
    v[3]    = _mm_add_epi32(v[3], rnding);
    out[12] = _mm_srai_epi32(v[3], bit);

    v[4] = _mm_add_epi32(u[4], u[5]);
    v[5] = _mm_sub_epi32(u[4], u[5]);
    v[6] = _mm_sub_epi32(u[7], u[6]);
    v[7] = _mm_add_epi32(u[7], u[6]);
    v[8] = u[8];

    v[9] = _mm_mullo_epi32(u[9], cospim16);
    x    = _mm_mullo_epi32(u[14], cospi48);
    v[9] = _mm_add_epi32(v[9], x);
    v[9] = _mm_add_epi32(v[9], rnding);
    v[9] = _mm_srai_epi32(v[9], bit);

    v[14] = _mm_mullo_epi32(u[9], cospi48);
    x     = _mm_mullo_epi32(u[14], cospim16);
    v[14] = _mm_sub_epi32(v[14], x);
    v[14] = _mm_add_epi32(v[14], rnding);
    v[14] = _mm_srai_epi32(v[14], bit);

    v[10] = _mm_mullo_epi32(u[10], cospim48);
    x     = _mm_mullo_epi32(u[13], cospim16);
    v[10] = _mm_add_epi32(v[10], x);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[13] = _mm_mullo_epi32(u[10], cospim16);
    x     = _mm_mullo_epi32(u[13], cospim48);
    v[13] = _mm_sub_epi32(v[13], x);
    v[13] = _mm_add_epi32(v[13], rnding);
    v[13] = _mm_srai_epi32(v[13], bit);

    v[12] = u[12];

    // stage 5
    u[4]   = _mm_mullo_epi32(v[4], cospi56);
    x      = _mm_mullo_epi32(v[7], cospi8);
    u[4]   = _mm_add_epi32(u[4], x);
    u[4]   = _mm_add_epi32(u[4], rnding);
    out[2] = _mm_srai_epi32(u[4], bit);

    u[7]    = _mm_mullo_epi32(v[4], cospi8);
    x       = _mm_mullo_epi32(v[7], cospi56);
    u[7]    = _mm_sub_epi32(x, u[7]);
    u[7]    = _mm_add_epi32(u[7], rnding);
    out[14] = _mm_srai_epi32(u[7], bit);

    u[5]    = _mm_mullo_epi32(v[5], cospi24);
    x       = _mm_mullo_epi32(v[6], cospi40);
    u[5]    = _mm_add_epi32(u[5], x);
    u[5]    = _mm_add_epi32(u[5], rnding);
    out[10] = _mm_srai_epi32(u[5], bit);

    u[6]   = _mm_mullo_epi32(v[5], cospi40);
    x      = _mm_mullo_epi32(v[6], cospi24);
    u[6]   = _mm_sub_epi32(x, u[6]);
    u[6]   = _mm_add_epi32(u[6], rnding);
    out[6] = _mm_srai_epi32(u[6], bit);

    u[8]  = _mm_add_epi32(v[8], v[9]);
    u[9]  = _mm_sub_epi32(v[8], v[9]);
    u[10] = _mm_sub_epi32(v[11], v[10]);
    u[11] = _mm_add_epi32(v[11], v[10]);
    u[12] = _mm_add_epi32(v[12], v[13]);
    u[13] = _mm_sub_epi32(v[12], v[13]);
    u[14] = _mm_sub_epi32(v[15], v[14]);
    u[15] = _mm_add_epi32(v[15], v[14]);

    // stage 6
    v[8]   = _mm_mullo_epi32(u[8], cospi60);
    x      = _mm_mullo_epi32(u[15], cospi4);
    v[8]   = _mm_add_epi32(v[8], x);
    v[8]   = _mm_add_epi32(v[8], rnding);
    out[1] = _mm_srai_epi32(v[8], bit);

    v[15]   = _mm_mullo_epi32(u[8], cospi4);
    x       = _mm_mullo_epi32(u[15], cospi60);
    v[15]   = _mm_sub_epi32(x, v[15]);
    v[15]   = _mm_add_epi32(v[15], rnding);
    out[15] = _mm_srai_epi32(v[15], bit);

    v[9]   = _mm_mullo_epi32(u[9], cospi28);
    x      = _mm_mullo_epi32(u[14], cospi36);
    v[9]   = _mm_add_epi32(v[9], x);
    v[9]   = _mm_add_epi32(v[9], rnding);
    out[9] = _mm_srai_epi32(v[9], bit);

    v[14]  = _mm_mullo_epi32(u[9], cospi36);
    x      = _mm_mullo_epi32(u[14], cospi28);
    v[14]  = _mm_sub_epi32(x, v[14]);
    v[14]  = _mm_add_epi32(v[14], rnding);
    out[7] = _mm_srai_epi32(v[14], bit);

    v[10]  = _mm_mullo_epi32(u[10], cospi44);
    x      = _mm_mullo_epi32(u[13], cospi20);
    v[10]  = _mm_add_epi32(v[10], x);
    v[10]  = _mm_add_epi32(v[10], rnding);
    out[5] = _mm_srai_epi32(v[10], bit);

    v[13]   = _mm_mullo_epi32(u[10], cospi20);
    x       = _mm_mullo_epi32(u[13], cospi44);
    v[13]   = _mm_sub_epi32(x, v[13]);
    v[13]   = _mm_add_epi32(v[13], rnding);
    out[11] = _mm_srai_epi32(v[13], bit);

    v[11]   = _mm_mullo_epi32(u[11], cospi12);
    x       = _mm_mullo_epi32(u[12], cospi52);
    v[11]   = _mm_add_epi32(v[11], x);
    v[11]   = _mm_add_epi32(v[11], rnding);
    out[13] = _mm_srai_epi32(v[11], bit);

    v[12]  = _mm_mullo_epi32(u[11], cospi52);
    x      = _mm_mullo_epi32(u[12], cospi12);
    v[12]  = _mm_sub_epi32(x, v[12]);
    v[12]  = _mm_add_epi32(v[12], rnding);
    out[3] = _mm_srai_epi32(v[12], bit);
}

static INLINE void fadst8x4_avx2(__m256i* input, __m256i* output, int32_t bit, const int32_t col_num) {
    __m128i*       in       = (__m128i*)input;
    __m128i*       out      = (__m128i*)output;
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospi4   = _mm_set1_epi32(cospi[4]);
    const __m128i  cospim4  = _mm_set1_epi32(-cospi[4]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospi20  = _mm_set1_epi32(cospi[20]);
    const __m128i  cospim20 = _mm_set1_epi32(-cospi[20]);
    const __m128i  cospi44  = _mm_set1_epi32(cospi[44]);
    const __m128i  cospi28  = _mm_set1_epi32(cospi[28]);
    const __m128i  cospi36  = _mm_set1_epi32(cospi[36]);
    const __m128i  cospim36 = _mm_set1_epi32(-cospi[36]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  cospim52 = _mm_set1_epi32(-cospi[52]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();
    __m128i        u0, u1, u2, u3, u4, u5, u6, u7;
    __m128i        v0, v1, v2, v3, v4, v5, v6, v7;
    __m128i        x, y;
    int32_t        col;

    // Note:
    //  Even column: 0, 2, ..., 14
    //  Odd column: 1, 3, ..., 15
    //  one even column plus one odd column constructs one row (8 coeffs)
    //  total we have 8 rows (8x8).
    for (col = 0; col < col_num; ++col) {
        // stage 0
        // stage 1
        u0 = in[col_num * 0 + col];
        u1 = _mm_sub_epi32(zero, in[col_num * 7 + col]);
        u2 = _mm_sub_epi32(zero, in[col_num * 3 + col]);
        u3 = in[col_num * 4 + col];
        u4 = _mm_sub_epi32(zero, in[col_num * 1 + col]);
        u5 = in[col_num * 6 + col];
        u6 = in[col_num * 2 + col];
        u7 = _mm_sub_epi32(zero, in[col_num * 5 + col]);

        // stage 2
        v0 = u0;
        v1 = u1;

        x  = _mm_mullo_epi32(u2, cospi32);
        y  = _mm_mullo_epi32(u3, cospi32);
        v2 = _mm_add_epi32(x, y);
        v2 = _mm_add_epi32(v2, rnding);
        v2 = _mm_srai_epi32(v2, bit);

        v3 = _mm_sub_epi32(x, y);
        v3 = _mm_add_epi32(v3, rnding);
        v3 = _mm_srai_epi32(v3, bit);

        v4 = u4;
        v5 = u5;

        x  = _mm_mullo_epi32(u6, cospi32);
        y  = _mm_mullo_epi32(u7, cospi32);
        v6 = _mm_add_epi32(x, y);
        v6 = _mm_add_epi32(v6, rnding);
        v6 = _mm_srai_epi32(v6, bit);

        v7 = _mm_sub_epi32(x, y);
        v7 = _mm_add_epi32(v7, rnding);
        v7 = _mm_srai_epi32(v7, bit);

        // stage 3
        u0 = _mm_add_epi32(v0, v2);
        u1 = _mm_add_epi32(v1, v3);
        u2 = _mm_sub_epi32(v0, v2);
        u3 = _mm_sub_epi32(v1, v3);
        u4 = _mm_add_epi32(v4, v6);
        u5 = _mm_add_epi32(v5, v7);
        u6 = _mm_sub_epi32(v4, v6);
        u7 = _mm_sub_epi32(v5, v7);

        // stage 4
        v0 = u0;
        v1 = u1;
        v2 = u2;
        v3 = u3;

        x  = _mm_mullo_epi32(u4, cospi16);
        y  = _mm_mullo_epi32(u5, cospi48);
        v4 = _mm_add_epi32(x, y);
        v4 = _mm_add_epi32(v4, rnding);
        v4 = _mm_srai_epi32(v4, bit);

        x  = _mm_mullo_epi32(u4, cospi48);
        y  = _mm_mullo_epi32(u5, cospim16);
        v5 = _mm_add_epi32(x, y);
        v5 = _mm_add_epi32(v5, rnding);
        v5 = _mm_srai_epi32(v5, bit);

        x  = _mm_mullo_epi32(u6, cospim48);
        y  = _mm_mullo_epi32(u7, cospi16);
        v6 = _mm_add_epi32(x, y);
        v6 = _mm_add_epi32(v6, rnding);
        v6 = _mm_srai_epi32(v6, bit);

        x  = _mm_mullo_epi32(u6, cospi16);
        y  = _mm_mullo_epi32(u7, cospi48);
        v7 = _mm_add_epi32(x, y);
        v7 = _mm_add_epi32(v7, rnding);
        v7 = _mm_srai_epi32(v7, bit);

        // stage 5
        u0 = _mm_add_epi32(v0, v4);
        u1 = _mm_add_epi32(v1, v5);
        u2 = _mm_add_epi32(v2, v6);
        u3 = _mm_add_epi32(v3, v7);
        u4 = _mm_sub_epi32(v0, v4);
        u5 = _mm_sub_epi32(v1, v5);
        u6 = _mm_sub_epi32(v2, v6);
        u7 = _mm_sub_epi32(v3, v7);

        // stage 6
        x                      = _mm_mullo_epi32(u0, cospi4);
        y                      = _mm_mullo_epi32(u1, cospi60);
        v0                     = _mm_add_epi32(x, y);
        v0                     = _mm_add_epi32(v0, rnding);
        out[col_num * 7 + col] = _mm_srai_epi32(v0, bit);

        x                      = _mm_mullo_epi32(u0, cospi60);
        y                      = _mm_mullo_epi32(u1, cospim4);
        v1                     = _mm_add_epi32(x, y);
        v1                     = _mm_add_epi32(v1, rnding);
        out[col_num * 0 + col] = _mm_srai_epi32(v1, bit);

        x                      = _mm_mullo_epi32(u2, cospi20);
        y                      = _mm_mullo_epi32(u3, cospi44);
        v2                     = _mm_add_epi32(x, y);
        v2                     = _mm_add_epi32(v2, rnding);
        out[col_num * 5 + col] = _mm_srai_epi32(v2, bit);

        x                      = _mm_mullo_epi32(u2, cospi44);
        y                      = _mm_mullo_epi32(u3, cospim20);
        v3                     = _mm_add_epi32(x, y);
        v3                     = _mm_add_epi32(v3, rnding);
        out[col_num * 2 + col] = _mm_srai_epi32(v3, bit);

        x                      = _mm_mullo_epi32(u4, cospi36);
        y                      = _mm_mullo_epi32(u5, cospi28);
        v4                     = _mm_add_epi32(x, y);
        v4                     = _mm_add_epi32(v4, rnding);
        out[col_num * 3 + col] = _mm_srai_epi32(v4, bit);

        x                      = _mm_mullo_epi32(u4, cospi28);
        y                      = _mm_mullo_epi32(u5, cospim36);
        v5                     = _mm_add_epi32(x, y);
        v5                     = _mm_add_epi32(v5, rnding);
        out[col_num * 4 + col] = _mm_srai_epi32(v5, bit);

        x                      = _mm_mullo_epi32(u6, cospi52);
        y                      = _mm_mullo_epi32(u7, cospi12);
        v6                     = _mm_add_epi32(x, y);
        v6                     = _mm_add_epi32(v6, rnding);
        out[col_num * 1 + col] = _mm_srai_epi32(v6, bit);

        x                      = _mm_mullo_epi32(u6, cospi12);
        y                      = _mm_mullo_epi32(u7, cospim52);
        v7                     = _mm_add_epi32(x, y);
        v7                     = _mm_add_epi32(v7, rnding);
        out[col_num * 6 + col] = _mm_srai_epi32(v7, bit);
    }
}

static INLINE void fadst16x4_avx2(__m256i* input, __m256i* output, int32_t bit) {
    __m128i* in  = (__m128i*)input;
    __m128i* out = (__m128i*)output;

    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospim56 = _mm_set1_epi32(-cospi[56]);
    const __m128i  cospim8  = _mm_set1_epi32(-cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospim24 = _mm_set1_epi32(-cospi[24]);
    const __m128i  cospim40 = _mm_set1_epi32(-cospi[40]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  cospi2   = _mm_set1_epi32(cospi[2]);
    const __m128i  cospi62  = _mm_set1_epi32(cospi[62]);
    const __m128i  cospim2  = _mm_set1_epi32(-cospi[2]);
    const __m128i  cospi10  = _mm_set1_epi32(cospi[10]);
    const __m128i  cospi54  = _mm_set1_epi32(cospi[54]);
    const __m128i  cospim10 = _mm_set1_epi32(-cospi[10]);
    const __m128i  cospi18  = _mm_set1_epi32(cospi[18]);
    const __m128i  cospi46  = _mm_set1_epi32(cospi[46]);
    const __m128i  cospim18 = _mm_set1_epi32(-cospi[18]);
    const __m128i  cospi26  = _mm_set1_epi32(cospi[26]);
    const __m128i  cospi38  = _mm_set1_epi32(cospi[38]);
    const __m128i  cospim26 = _mm_set1_epi32(-cospi[26]);
    const __m128i  cospi34  = _mm_set1_epi32(cospi[34]);
    const __m128i  cospi30  = _mm_set1_epi32(cospi[30]);
    const __m128i  cospim34 = _mm_set1_epi32(-cospi[34]);
    const __m128i  cospi42  = _mm_set1_epi32(cospi[42]);
    const __m128i  cospi22  = _mm_set1_epi32(cospi[22]);
    const __m128i  cospim42 = _mm_set1_epi32(-cospi[42]);
    const __m128i  cospi50  = _mm_set1_epi32(cospi[50]);
    const __m128i  cospi14  = _mm_set1_epi32(cospi[14]);
    const __m128i  cospim50 = _mm_set1_epi32(-cospi[50]);
    const __m128i  cospi58  = _mm_set1_epi32(cospi[58]);
    const __m128i  cospi6   = _mm_set1_epi32(cospi[6]);
    const __m128i  cospim58 = _mm_set1_epi32(-cospi[58]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();

    __m128i u[16], v[16], x, y;
    __m128i tmp[13];

    tmp[0] = _mm_sub_epi32(zero, in[15]);
    u[2]   = _mm_sub_epi32(zero, in[7]);
    tmp[1] = _mm_sub_epi32(zero, in[3]);
    u[7]   = _mm_sub_epi32(zero, in[11]);
    tmp[2] = _mm_sub_epi32(zero, in[1]);
    u[11]  = _mm_sub_epi32(zero, in[9]);
    tmp[3] = _mm_sub_epi32(zero, in[13]);
    u[14]  = _mm_sub_epi32(zero, in[5]);

    // stage 2

    x    = _mm_mullo_epi32(u[2], cospi32);
    y    = _mm_mullo_epi32(in[8], cospi32);
    v[2] = _mm_add_epi32(x, y);
    v[2] = _mm_add_epi32(v[2], rnding);
    v[2] = _mm_srai_epi32(v[2], bit);

    v[3] = _mm_sub_epi32(x, y);
    v[3] = _mm_add_epi32(v[3], rnding);
    v[3] = _mm_srai_epi32(v[3], bit);

    x    = _mm_mullo_epi32(in[4], cospi32);
    y    = _mm_mullo_epi32(u[7], cospi32);
    v[6] = _mm_add_epi32(x, y);
    v[6] = _mm_add_epi32(v[6], rnding);
    v[6] = _mm_srai_epi32(v[6], bit);

    v[7] = _mm_sub_epi32(x, y);
    v[7] = _mm_add_epi32(v[7], rnding);
    v[7] = _mm_srai_epi32(v[7], bit);

    x     = _mm_mullo_epi32(in[6], cospi32);
    y     = _mm_mullo_epi32(u[11], cospi32);
    v[10] = _mm_add_epi32(x, y);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[11] = _mm_sub_epi32(x, y);
    v[11] = _mm_add_epi32(v[11], rnding);
    v[11] = _mm_srai_epi32(v[11], bit);

    x     = _mm_mullo_epi32(u[14], cospi32);
    y     = _mm_mullo_epi32(in[10], cospi32);
    v[14] = _mm_add_epi32(x, y);
    v[14] = _mm_add_epi32(v[14], rnding);
    v[14] = _mm_srai_epi32(v[14], bit);

    v[15] = _mm_sub_epi32(x, y);
    v[15] = _mm_add_epi32(v[15], rnding);
    v[15] = _mm_srai_epi32(v[15], bit);

    // stage 3
    tmp[4] = _mm_add_epi32(in[0], v[2]);
    tmp[5] = _mm_add_epi32(tmp[0], v[3]);
    tmp[6] = _mm_sub_epi32(in[0], v[2]);
    tmp[0] = _mm_sub_epi32(tmp[0], v[3]);
    u[4]   = _mm_add_epi32(tmp[1], v[6]);
    u[5]   = _mm_add_epi32(in[12], v[7]);
    u[6]   = _mm_sub_epi32(tmp[1], v[6]);
    u[7]   = _mm_sub_epi32(in[12], v[7]);
    tmp[1] = _mm_add_epi32(tmp[2], v[10]);
    tmp[7] = _mm_add_epi32(in[14], v[11]);
    tmp[2] = _mm_sub_epi32(tmp[2], v[10]);
    tmp[8] = _mm_sub_epi32(in[14], v[11]);
    u[12]  = _mm_add_epi32(in[2], v[14]);
    u[13]  = _mm_add_epi32(tmp[3], v[15]);
    u[14]  = _mm_sub_epi32(in[2], v[14]);
    u[15]  = _mm_sub_epi32(tmp[3], v[15]);

    // stage 4
    v[4]  = half_btf_small(&cospi16, &u[4], &cospi48, &u[5], &rnding, bit);
    v[5]  = half_btf_small(&cospi48, &u[4], &cospim16, &u[5], &rnding, bit);
    v[6]  = half_btf_small(&cospim48, &u[6], &cospi16, &u[7], &rnding, bit);
    v[7]  = half_btf_small(&cospi16, &u[6], &cospi48, &u[7], &rnding, bit);
    v[12] = half_btf_small(&cospi16, &u[12], &cospi48, &u[13], &rnding, bit);
    v[13] = half_btf_small(&cospi48, &u[12], &cospim16, &u[13], &rnding, bit);
    v[14] = half_btf_small(&cospim48, &u[14], &cospi16, &u[15], &rnding, bit);
    v[15] = half_btf_small(&cospi16, &u[14], &cospi48, &u[15], &rnding, bit);

    // stage 5
    tmp[9]  = _mm_add_epi32(tmp[4], v[4]);
    tmp[10] = _mm_add_epi32(tmp[5], v[5]);
    tmp[11] = _mm_add_epi32(tmp[6], v[6]);
    tmp[12] = _mm_add_epi32(tmp[0], v[7]);
    tmp[4]  = _mm_sub_epi32(tmp[4], v[4]);
    tmp[5]  = _mm_sub_epi32(tmp[5], v[5]);
    tmp[6]  = _mm_sub_epi32(tmp[6], v[6]);
    tmp[0]  = _mm_sub_epi32(tmp[0], v[7]);
    u[8]    = _mm_add_epi32(tmp[1], v[12]);
    u[9]    = _mm_add_epi32(tmp[7], v[13]);
    u[10]   = _mm_add_epi32(tmp[2], v[14]);
    u[11]   = _mm_add_epi32(tmp[8], v[15]);
    u[12]   = _mm_sub_epi32(tmp[1], v[12]);
    u[13]   = _mm_sub_epi32(tmp[7], v[13]);
    u[14]   = _mm_sub_epi32(tmp[2], v[14]);
    u[15]   = _mm_sub_epi32(tmp[8], v[15]);

    // stage 6
    v[8]  = half_btf_small(&cospi8, &u[8], &cospi56, &u[9], &rnding, bit);
    v[9]  = half_btf_small(&cospi56, &u[8], &cospim8, &u[9], &rnding, bit);
    v[10] = half_btf_small(&cospi40, &u[10], &cospi24, &u[11], &rnding, bit);
    v[11] = half_btf_small(&cospi24, &u[10], &cospim40, &u[11], &rnding, bit);
    v[12] = half_btf_small(&cospim56, &u[12], &cospi8, &u[13], &rnding, bit);
    v[13] = half_btf_small(&cospi8, &u[12], &cospi56, &u[13], &rnding, bit);
    v[14] = half_btf_small(&cospim24, &u[14], &cospi40, &u[15], &rnding, bit);
    v[15] = half_btf_small(&cospi40, &u[14], &cospi24, &u[15], &rnding, bit);

    // stage 7
    u[0]  = _mm_add_epi32(tmp[9], v[8]);
    u[1]  = _mm_add_epi32(tmp[10], v[9]);
    u[2]  = _mm_add_epi32(tmp[11], v[10]);
    u[3]  = _mm_add_epi32(tmp[12], v[11]);
    u[4]  = _mm_add_epi32(tmp[4], v[12]);
    u[5]  = _mm_add_epi32(tmp[5], v[13]);
    u[6]  = _mm_add_epi32(tmp[6], v[14]);
    u[7]  = _mm_add_epi32(tmp[0], v[15]);
    u[8]  = _mm_sub_epi32(tmp[9], v[8]);
    u[9]  = _mm_sub_epi32(tmp[10], v[9]);
    u[10] = _mm_sub_epi32(tmp[11], v[10]);
    u[11] = _mm_sub_epi32(tmp[12], v[11]);
    u[12] = _mm_sub_epi32(tmp[4], v[12]);
    u[13] = _mm_sub_epi32(tmp[5], v[13]);
    u[14] = _mm_sub_epi32(tmp[6], v[14]);
    u[15] = _mm_sub_epi32(tmp[0], v[15]);

    // stage 8
    out[15] = half_btf_small(&cospi2, &u[0], &cospi62, &u[1], &rnding, bit);
    out[0]  = half_btf_small(&cospi62, &u[0], &cospim2, &u[1], &rnding, bit);
    out[13] = half_btf_small(&cospi10, &u[2], &cospi54, &u[3], &rnding, bit);
    out[2]  = half_btf_small(&cospi54, &u[2], &cospim10, &u[3], &rnding, bit);
    out[11] = half_btf_small(&cospi18, &u[4], &cospi46, &u[5], &rnding, bit);
    out[4]  = half_btf_small(&cospi46, &u[4], &cospim18, &u[5], &rnding, bit);
    out[9]  = half_btf_small(&cospi26, &u[6], &cospi38, &u[7], &rnding, bit);
    out[6]  = half_btf_small(&cospi38, &u[6], &cospim26, &u[7], &rnding, bit);
    out[7]  = half_btf_small(&cospi34, &u[8], &cospi30, &u[9], &rnding, bit);
    out[8]  = half_btf_small(&cospi30, &u[8], &cospim34, &u[9], &rnding, bit);
    out[5]  = half_btf_small(&cospi42, &u[10], &cospi22, &u[11], &rnding, bit);
    out[10] = half_btf_small(&cospi22, &u[10], &cospim42, &u[11], &rnding, bit);
    out[3]  = half_btf_small(&cospi50, &u[12], &cospi14, &u[13], &rnding, bit);
    out[12] = half_btf_small(&cospi14, &u[12], &cospim50, &u[13], &rnding, bit);
    out[1]  = half_btf_small(&cospi58, &u[14], &cospi6, &u[15], &rnding, bit);
    out[14] = half_btf_small(&cospi6, &u[14], &cospim58, &u[15], &rnding, bit);
}

static void fdct16x16_avx2(const __m256i* in, __m256i* out, int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospi56  = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi8   = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi24  = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospi40  = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi60  = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi4   = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospi28  = _mm256_set1_epi32(cospi[28]);
    const __m256i  cospi36  = _mm256_set1_epi32(cospi[36]);
    const __m256i  cospi44  = _mm256_set1_epi32(cospi[44]);
    const __m256i  cospi20  = _mm256_set1_epi32(cospi[20]);
    const __m256i  cospi12  = _mm256_set1_epi32(cospi[12]);
    const __m256i  cospi52  = _mm256_set1_epi32(cospi[52]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    __m256i        u[16], v[16], x;
    int32_t        col;

    for (col = 0; col < col_num; ++col) {
        // stage 0
        // stage 1
        u[0]  = _mm256_add_epi32(in[0 * col_num + col], in[15 * col_num + col]);
        u[15] = _mm256_sub_epi32(in[0 * col_num + col], in[15 * col_num + col]);
        u[1]  = _mm256_add_epi32(in[1 * col_num + col], in[14 * col_num + col]);
        u[14] = _mm256_sub_epi32(in[1 * col_num + col], in[14 * col_num + col]);
        u[2]  = _mm256_add_epi32(in[2 * col_num + col], in[13 * col_num + col]);
        u[13] = _mm256_sub_epi32(in[2 * col_num + col], in[13 * col_num + col]);
        u[3]  = _mm256_add_epi32(in[3 * col_num + col], in[12 * col_num + col]);
        u[12] = _mm256_sub_epi32(in[3 * col_num + col], in[12 * col_num + col]);
        u[4]  = _mm256_add_epi32(in[4 * col_num + col], in[11 * col_num + col]);
        u[11] = _mm256_sub_epi32(in[4 * col_num + col], in[11 * col_num + col]);
        u[5]  = _mm256_add_epi32(in[5 * col_num + col], in[10 * col_num + col]);
        u[10] = _mm256_sub_epi32(in[5 * col_num + col], in[10 * col_num + col]);
        u[6]  = _mm256_add_epi32(in[6 * col_num + col], in[9 * col_num + col]);
        u[9]  = _mm256_sub_epi32(in[6 * col_num + col], in[9 * col_num + col]);
        u[7]  = _mm256_add_epi32(in[7 * col_num + col], in[8 * col_num + col]);
        u[8]  = _mm256_sub_epi32(in[7 * col_num + col], in[8 * col_num + col]);

        // stage 2
        v[0] = _mm256_add_epi32(u[0], u[7]);
        v[7] = _mm256_sub_epi32(u[0], u[7]);
        v[1] = _mm256_add_epi32(u[1], u[6]);
        v[6] = _mm256_sub_epi32(u[1], u[6]);
        v[2] = _mm256_add_epi32(u[2], u[5]);
        v[5] = _mm256_sub_epi32(u[2], u[5]);
        v[3] = _mm256_add_epi32(u[3], u[4]);
        v[4] = _mm256_sub_epi32(u[3], u[4]);

        v[10] = _mm256_mullo_epi32(u[10], cospim32);
        x     = _mm256_mullo_epi32(u[13], cospi32);
        v[10] = _mm256_add_epi32(v[10], x);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[13] = _mm256_mullo_epi32(u[10], cospi32);
        x     = _mm256_mullo_epi32(u[13], cospim32);
        v[13] = _mm256_sub_epi32(v[13], x);
        v[13] = _mm256_add_epi32(v[13], rnding);
        v[13] = _mm256_srai_epi32(v[13], bit);

        v[11] = _mm256_mullo_epi32(u[11], cospim32);
        x     = _mm256_mullo_epi32(u[12], cospi32);
        v[11] = _mm256_add_epi32(v[11], x);
        v[11] = _mm256_add_epi32(v[11], rnding);
        v[11] = _mm256_srai_epi32(v[11], bit);

        v[12] = _mm256_mullo_epi32(u[11], cospi32);
        x     = _mm256_mullo_epi32(u[12], cospim32);
        v[12] = _mm256_sub_epi32(v[12], x);
        v[12] = _mm256_add_epi32(v[12], rnding);
        v[12] = _mm256_srai_epi32(v[12], bit);

        // stage 3
        u[0] = _mm256_add_epi32(v[0], v[3]);
        u[3] = _mm256_sub_epi32(v[0], v[3]);
        u[1] = _mm256_add_epi32(v[1], v[2]);
        u[2] = _mm256_sub_epi32(v[1], v[2]);

        u[5] = _mm256_mullo_epi32(v[5], cospim32);
        x    = _mm256_mullo_epi32(v[6], cospi32);
        u[5] = _mm256_add_epi32(u[5], x);
        u[5] = _mm256_add_epi32(u[5], rnding);
        u[5] = _mm256_srai_epi32(u[5], bit);

        u[6] = _mm256_mullo_epi32(v[5], cospi32);
        x    = _mm256_mullo_epi32(v[6], cospim32);
        u[6] = _mm256_sub_epi32(u[6], x);
        u[6] = _mm256_add_epi32(u[6], rnding);
        u[6] = _mm256_srai_epi32(u[6], bit);

        u[11] = _mm256_sub_epi32(u[8], v[11]);
        u[8]  = _mm256_add_epi32(u[8], v[11]);
        u[10] = _mm256_sub_epi32(u[9], v[10]);
        u[9]  = _mm256_add_epi32(u[9], v[10]);
        u[12] = _mm256_sub_epi32(u[15], v[12]);
        u[15] = _mm256_add_epi32(u[15], v[12]);
        u[13] = _mm256_sub_epi32(u[14], v[13]);
        u[14] = _mm256_add_epi32(u[14], v[13]);

        // stage 4
        u[0]                   = _mm256_mullo_epi32(u[0], cospi32);
        u[1]                   = _mm256_mullo_epi32(u[1], cospi32);
        v[0]                   = _mm256_add_epi32(u[0], u[1]);
        v[0]                   = _mm256_add_epi32(v[0], rnding);
        out[0 * col_num + col] = _mm256_srai_epi32(v[0], bit);

        v[1]                   = _mm256_sub_epi32(u[0], u[1]);
        v[1]                   = _mm256_add_epi32(v[1], rnding);
        out[8 * col_num + col] = _mm256_srai_epi32(v[1], bit);

        v[2]                   = _mm256_mullo_epi32(u[2], cospi48);
        x                      = _mm256_mullo_epi32(u[3], cospi16);
        v[2]                   = _mm256_add_epi32(v[2], x);
        v[2]                   = _mm256_add_epi32(v[2], rnding);
        out[4 * col_num + col] = _mm256_srai_epi32(v[2], bit);

        v[3]                    = _mm256_mullo_epi32(u[2], cospi16);
        x                       = _mm256_mullo_epi32(u[3], cospi48);
        v[3]                    = _mm256_sub_epi32(x, v[3]);
        v[3]                    = _mm256_add_epi32(v[3], rnding);
        out[12 * col_num + col] = _mm256_srai_epi32(v[3], bit);

        v[5] = _mm256_sub_epi32(v[4], u[5]);
        v[4] = _mm256_add_epi32(v[4], u[5]);
        v[6] = _mm256_sub_epi32(v[7], u[6]);
        v[7] = _mm256_add_epi32(v[7], u[6]);

        v[9] = _mm256_mullo_epi32(u[9], cospim16);
        x    = _mm256_mullo_epi32(u[14], cospi48);
        v[9] = _mm256_add_epi32(v[9], x);
        v[9] = _mm256_add_epi32(v[9], rnding);
        v[9] = _mm256_srai_epi32(v[9], bit);

        v[14] = _mm256_mullo_epi32(u[9], cospi48);
        x     = _mm256_mullo_epi32(u[14], cospim16);
        v[14] = _mm256_sub_epi32(v[14], x);
        v[14] = _mm256_add_epi32(v[14], rnding);
        v[14] = _mm256_srai_epi32(v[14], bit);

        v[10] = _mm256_mullo_epi32(u[10], cospim48);
        x     = _mm256_mullo_epi32(u[13], cospim16);
        v[10] = _mm256_add_epi32(v[10], x);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[13] = _mm256_mullo_epi32(u[10], cospim16);
        x     = _mm256_mullo_epi32(u[13], cospim48);
        v[13] = _mm256_sub_epi32(v[13], x);
        v[13] = _mm256_add_epi32(v[13], rnding);
        v[13] = _mm256_srai_epi32(v[13], bit);

        // stage 5
        u[4]                   = _mm256_mullo_epi32(v[4], cospi56);
        x                      = _mm256_mullo_epi32(v[7], cospi8);
        u[4]                   = _mm256_add_epi32(u[4], x);
        u[4]                   = _mm256_add_epi32(u[4], rnding);
        out[2 * col_num + col] = _mm256_srai_epi32(u[4], bit);

        u[7]                    = _mm256_mullo_epi32(v[4], cospi8);
        x                       = _mm256_mullo_epi32(v[7], cospi56);
        u[7]                    = _mm256_sub_epi32(x, u[7]);
        u[7]                    = _mm256_add_epi32(u[7], rnding);
        out[14 * col_num + col] = _mm256_srai_epi32(u[7], bit);

        u[5]                    = _mm256_mullo_epi32(v[5], cospi24);
        x                       = _mm256_mullo_epi32(v[6], cospi40);
        u[5]                    = _mm256_add_epi32(u[5], x);
        u[5]                    = _mm256_add_epi32(u[5], rnding);
        out[10 * col_num + col] = _mm256_srai_epi32(u[5], bit);

        u[6]                   = _mm256_mullo_epi32(v[5], cospi40);
        x                      = _mm256_mullo_epi32(v[6], cospi24);
        u[6]                   = _mm256_sub_epi32(x, u[6]);
        u[6]                   = _mm256_add_epi32(u[6], rnding);
        out[6 * col_num + col] = _mm256_srai_epi32(u[6], bit);

        u[9]  = _mm256_sub_epi32(u[8], v[9]);
        u[8]  = _mm256_add_epi32(u[8], v[9]);
        u[10] = _mm256_sub_epi32(u[11], v[10]);
        u[11] = _mm256_add_epi32(u[11], v[10]);
        u[13] = _mm256_sub_epi32(u[12], v[13]);
        u[12] = _mm256_add_epi32(u[12], v[13]);
        u[14] = _mm256_sub_epi32(u[15], v[14]);
        u[15] = _mm256_add_epi32(u[15], v[14]);

        // stage 6
        v[8]                   = _mm256_mullo_epi32(u[8], cospi60);
        x                      = _mm256_mullo_epi32(u[15], cospi4);
        v[8]                   = _mm256_add_epi32(v[8], x);
        v[8]                   = _mm256_add_epi32(v[8], rnding);
        out[1 * col_num + col] = _mm256_srai_epi32(v[8], bit);

        v[15]                   = _mm256_mullo_epi32(u[8], cospi4);
        x                       = _mm256_mullo_epi32(u[15], cospi60);
        v[15]                   = _mm256_sub_epi32(x, v[15]);
        v[15]                   = _mm256_add_epi32(v[15], rnding);
        out[15 * col_num + col] = _mm256_srai_epi32(v[15], bit);

        v[9]                   = _mm256_mullo_epi32(u[9], cospi28);
        x                      = _mm256_mullo_epi32(u[14], cospi36);
        v[9]                   = _mm256_add_epi32(v[9], x);
        v[9]                   = _mm256_add_epi32(v[9], rnding);
        out[9 * col_num + col] = _mm256_srai_epi32(v[9], bit);

        v[14]                  = _mm256_mullo_epi32(u[9], cospi36);
        x                      = _mm256_mullo_epi32(u[14], cospi28);
        v[14]                  = _mm256_sub_epi32(x, v[14]);
        v[14]                  = _mm256_add_epi32(v[14], rnding);
        out[7 * col_num + col] = _mm256_srai_epi32(v[14], bit);

        v[10]                  = _mm256_mullo_epi32(u[10], cospi44);
        x                      = _mm256_mullo_epi32(u[13], cospi20);
        v[10]                  = _mm256_add_epi32(v[10], x);
        v[10]                  = _mm256_add_epi32(v[10], rnding);
        out[5 * col_num + col] = _mm256_srai_epi32(v[10], bit);

        v[13]                   = _mm256_mullo_epi32(u[10], cospi20);
        x                       = _mm256_mullo_epi32(u[13], cospi44);
        v[13]                   = _mm256_sub_epi32(x, v[13]);
        v[13]                   = _mm256_add_epi32(v[13], rnding);
        out[11 * col_num + col] = _mm256_srai_epi32(v[13], bit);

        v[11]                   = _mm256_mullo_epi32(u[11], cospi12);
        x                       = _mm256_mullo_epi32(u[12], cospi52);
        v[11]                   = _mm256_add_epi32(v[11], x);
        v[11]                   = _mm256_add_epi32(v[11], rnding);
        out[13 * col_num + col] = _mm256_srai_epi32(v[11], bit);

        v[12]                  = _mm256_mullo_epi32(u[11], cospi52);
        x                      = _mm256_mullo_epi32(u[12], cospi12);
        v[12]                  = _mm256_sub_epi32(x, v[12]);
        v[12]                  = _mm256_add_epi32(v[12], rnding);
        out[3 * col_num + col] = _mm256_srai_epi32(v[12], bit);
    }
}

static INLINE void fadst4x8_row_avx2(__m256i* input, __m256i* output, int32_t bit, const int32_t num_col) {
    const int32_t* sinpi  = sinpi_arr(bit);
    const __m256i  rnding = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  sinpi1 = _mm256_set1_epi32((int32_t)sinpi[1]);
    const __m256i  sinpi2 = _mm256_set1_epi32((int32_t)sinpi[2]);
    const __m256i  sinpi3 = _mm256_set1_epi32((int32_t)sinpi[3]);
    const __m256i  sinpi4 = _mm256_set1_epi32((int32_t)sinpi[4]);
    __m256i        t;
    __m256i        s0, s1, s2, s3, s4, s5, s6, s7;
    __m256i        x0, x1, x2, x3;
    __m256i        u0, u1, u2, u3;
    __m256i        v0, v1, v2, v3;
    __m256i        in[4];
    __m256i        out[4];

    in[0] = yy_unpacklo_epi128(input[0], input[2]);
    in[1] = yy_unpackhi_epi128(input[0], input[2]);
    in[2] = yy_unpacklo_epi128(input[1], input[3]);
    in[3] = yy_unpackhi_epi128(input[1], input[3]);

    int32_t idx = 0 * num_col;
    s0          = _mm256_mullo_epi32(in[idx], sinpi1);
    s1          = _mm256_mullo_epi32(in[idx], sinpi4);
    t           = _mm256_add_epi32(in[idx], in[idx + num_col]);
    idx += num_col;
    s2 = _mm256_mullo_epi32(in[idx], sinpi2);
    s3 = _mm256_mullo_epi32(in[idx], sinpi1);
    idx += num_col;
    s4 = _mm256_mullo_epi32(in[idx], sinpi3);
    idx += num_col;
    s5 = _mm256_mullo_epi32(in[idx], sinpi4);
    s6 = _mm256_mullo_epi32(in[idx], sinpi2);
    s7 = _mm256_sub_epi32(t, in[idx]);

    t  = _mm256_add_epi32(s0, s2);
    x0 = _mm256_add_epi32(t, s5);
    x1 = _mm256_mullo_epi32(s7, sinpi3);
    t  = _mm256_sub_epi32(s1, s3);
    x2 = _mm256_add_epi32(t, s6);
    x3 = s4;

    s0 = _mm256_add_epi32(x0, x3);
    s1 = x1;
    s2 = _mm256_sub_epi32(x2, x3);
    t  = _mm256_sub_epi32(x2, x0);
    s3 = _mm256_add_epi32(t, x3);

    u0 = _mm256_add_epi32(s0, rnding);
    u0 = _mm256_srai_epi32(u0, bit);

    u1 = _mm256_add_epi32(s1, rnding);
    u1 = _mm256_srai_epi32(u1, bit);

    u2 = _mm256_add_epi32(s2, rnding);
    u2 = _mm256_srai_epi32(u2, bit);

    u3 = _mm256_add_epi32(s3, rnding);
    u3 = _mm256_srai_epi32(u3, bit);

    v0 = _mm256_unpacklo_epi32(u0, u1);
    v1 = _mm256_unpackhi_epi32(u0, u1);
    v2 = _mm256_unpacklo_epi32(u2, u3);
    v3 = _mm256_unpackhi_epi32(u2, u3);

    out[0] = _mm256_unpacklo_epi64(v0, v2);
    out[1] = _mm256_unpackhi_epi64(v0, v2);
    out[2] = _mm256_unpacklo_epi64(v1, v3);
    out[3] = _mm256_unpackhi_epi64(v1, v3);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static INLINE void fadst4x8_col_avx2(__m256i* in, __m256i* output, int32_t bit, const int32_t num_col) {
    const int32_t* sinpi  = sinpi_arr(bit);
    const __m256i  rnding = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  sinpi1 = _mm256_set1_epi32((int32_t)sinpi[1]);
    const __m256i  sinpi2 = _mm256_set1_epi32((int32_t)sinpi[2]);
    const __m256i  sinpi3 = _mm256_set1_epi32((int32_t)sinpi[3]);
    const __m256i  sinpi4 = _mm256_set1_epi32((int32_t)sinpi[4]);
    __m256i        t;
    __m256i        s0, s1, s2, s3, s4, s5, s6, s7;
    __m256i        x0, x1, x2, x3;
    __m256i        u0, u1, u2, u3;
    __m256i        v0, v1, v2, v3;
    __m256i        out[4];

    int32_t idx = 0 * num_col;
    s0          = _mm256_mullo_epi32(in[idx], sinpi1);
    s1          = _mm256_mullo_epi32(in[idx], sinpi4);
    t           = _mm256_add_epi32(in[idx], in[idx + num_col]);
    idx += num_col;
    s2 = _mm256_mullo_epi32(in[idx], sinpi2);
    s3 = _mm256_mullo_epi32(in[idx], sinpi1);
    idx += num_col;
    s4 = _mm256_mullo_epi32(in[idx], sinpi3);
    idx += num_col;
    s5 = _mm256_mullo_epi32(in[idx], sinpi4);
    s6 = _mm256_mullo_epi32(in[idx], sinpi2);
    s7 = _mm256_sub_epi32(t, in[idx]);

    t  = _mm256_add_epi32(s0, s2);
    x0 = _mm256_add_epi32(t, s5);
    x1 = _mm256_mullo_epi32(s7, sinpi3);
    t  = _mm256_sub_epi32(s1, s3);
    x2 = _mm256_add_epi32(t, s6);
    x3 = s4;

    s0 = _mm256_add_epi32(x0, x3);
    s1 = x1;
    s2 = _mm256_sub_epi32(x2, x3);
    t  = _mm256_sub_epi32(x2, x0);
    s3 = _mm256_add_epi32(t, x3);

    u0 = _mm256_add_epi32(s0, rnding);
    u0 = _mm256_srai_epi32(u0, bit);

    u1 = _mm256_add_epi32(s1, rnding);
    u1 = _mm256_srai_epi32(u1, bit);

    u2 = _mm256_add_epi32(s2, rnding);
    u2 = _mm256_srai_epi32(u2, bit);

    u3 = _mm256_add_epi32(s3, rnding);
    u3 = _mm256_srai_epi32(u3, bit);

    v0 = _mm256_unpacklo_epi32(u0, u1);
    v1 = _mm256_unpackhi_epi32(u0, u1);
    v2 = _mm256_unpacklo_epi32(u2, u3);
    v3 = _mm256_unpackhi_epi32(u2, u3);

    out[0] = _mm256_unpacklo_epi64(v0, v2);
    out[1] = _mm256_unpackhi_epi64(v0, v2);
    out[2] = _mm256_unpacklo_epi64(v1, v3);
    out[3] = _mm256_unpackhi_epi64(v1, v3);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static INLINE void fdct4x8_avx2(__m256i* input, __m256i* output, int32_t bit) {
    __m128i*       in       = (__m128i*)input;
    __m128i*       out      = (__m128i*)output;
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[8], v[8];

    // Even 8 points 0, 2, ..., 14
    // stage 0
    // stage 1
    u[0] = _mm_add_epi32(in[0], in[7]);
    v[7] = _mm_sub_epi32(in[0], in[7]); // v[7]
    u[1] = _mm_add_epi32(in[1], in[6]);
    u[6] = _mm_sub_epi32(in[1], in[6]);
    u[2] = _mm_add_epi32(in[2], in[5]);
    u[5] = _mm_sub_epi32(in[2], in[5]);
    u[3] = _mm_add_epi32(in[3], in[4]);
    v[4] = _mm_sub_epi32(in[3], in[4]); // v[4]

    // stage 2
    v[0] = _mm_add_epi32(u[0], u[3]);
    v[3] = _mm_sub_epi32(u[0], u[3]);
    v[1] = _mm_add_epi32(u[1], u[2]);
    v[2] = _mm_sub_epi32(u[1], u[2]);

    v[5] = _mm_mullo_epi32(u[5], cospim32);
    v[6] = _mm_mullo_epi32(u[6], cospi32);
    v[5] = _mm_add_epi32(v[5], v[6]);
    v[5] = _mm_add_epi32(v[5], rnding);
    v[5] = _mm_srai_epi32(v[5], bit);

    u[0] = _mm_mullo_epi32(u[5], cospi32);
    v[6] = _mm_mullo_epi32(u[6], cospim32);
    v[6] = _mm_sub_epi32(u[0], v[6]);
    v[6] = _mm_add_epi32(v[6], rnding);
    v[6] = _mm_srai_epi32(v[6], bit);

    // stage 3
    // type 0
    v[0]   = _mm_mullo_epi32(v[0], cospi32);
    v[1]   = _mm_mullo_epi32(v[1], cospi32);
    u[0]   = _mm_add_epi32(v[0], v[1]);
    u[0]   = _mm_add_epi32(u[0], rnding);
    out[0] = _mm_srai_epi32(u[0], bit);

    u[1]   = _mm_sub_epi32(v[0], v[1]);
    u[1]   = _mm_add_epi32(u[1], rnding);
    out[4] = _mm_srai_epi32(u[1], bit);

    // type 1
    v[0]   = _mm_mullo_epi32(v[2], cospi48);
    v[1]   = _mm_mullo_epi32(v[3], cospi16);
    u[2]   = _mm_add_epi32(v[0], v[1]);
    u[2]   = _mm_add_epi32(u[2], rnding);
    out[2] = _mm_srai_epi32(u[2], bit);

    v[0]   = _mm_mullo_epi32(v[2], cospi16);
    v[1]   = _mm_mullo_epi32(v[3], cospi48);
    u[3]   = _mm_sub_epi32(v[1], v[0]);
    u[3]   = _mm_add_epi32(u[3], rnding);
    out[6] = _mm_srai_epi32(u[3], bit);

    u[4] = _mm_add_epi32(v[4], v[5]);
    u[5] = _mm_sub_epi32(v[4], v[5]);
    u[6] = _mm_sub_epi32(v[7], v[6]);
    u[7] = _mm_add_epi32(v[7], v[6]);

    // stage 4
    // stage 5
    v[0]   = _mm_mullo_epi32(u[4], cospi56);
    v[1]   = _mm_mullo_epi32(u[7], cospi8);
    v[0]   = _mm_add_epi32(v[0], v[1]);
    v[0]   = _mm_add_epi32(v[0], rnding);
    out[1] = _mm_srai_epi32(v[0], bit); // buf0[4]

    v[0]   = _mm_mullo_epi32(u[4], cospi8);
    v[1]   = _mm_mullo_epi32(u[7], cospi56);
    v[0]   = _mm_sub_epi32(v[1], v[0]);
    v[0]   = _mm_add_epi32(v[0], rnding);
    out[7] = _mm_srai_epi32(v[0], bit); // buf0[7]

    v[0]   = _mm_mullo_epi32(u[5], cospi24);
    v[1]   = _mm_mullo_epi32(u[6], cospi40);
    v[0]   = _mm_add_epi32(v[0], v[1]);
    v[0]   = _mm_add_epi32(v[0], rnding);
    out[5] = _mm_srai_epi32(v[0], bit); // buf0[5]

    v[0]   = _mm_mullo_epi32(u[5], cospi40);
    v[1]   = _mm_mullo_epi32(u[6], cospi24);
    v[0]   = _mm_sub_epi32(v[1], v[0]);
    v[0]   = _mm_add_epi32(v[0], rnding);
    out[3] = _mm_srai_epi32(v[0], bit); // buf0[6]
}

static void fadst16x16_avx2(const __m256i* in, __m256i* out, int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospi8   = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi56  = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospim56 = _mm256_set1_epi32(-cospi[56]);
    const __m256i  cospim8  = _mm256_set1_epi32(-cospi[8]);
    const __m256i  cospi24  = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospim24 = _mm256_set1_epi32(-cospi[24]);
    const __m256i  cospim40 = _mm256_set1_epi32(-cospi[40]);
    const __m256i  cospi40  = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi2   = _mm256_set1_epi32(cospi[2]);
    const __m256i  cospi62  = _mm256_set1_epi32(cospi[62]);
    const __m256i  cospim2  = _mm256_set1_epi32(-cospi[2]);
    const __m256i  cospi10  = _mm256_set1_epi32(cospi[10]);
    const __m256i  cospi54  = _mm256_set1_epi32(cospi[54]);
    const __m256i  cospim10 = _mm256_set1_epi32(-cospi[10]);
    const __m256i  cospi18  = _mm256_set1_epi32(cospi[18]);
    const __m256i  cospi46  = _mm256_set1_epi32(cospi[46]);
    const __m256i  cospim18 = _mm256_set1_epi32(-cospi[18]);
    const __m256i  cospi26  = _mm256_set1_epi32(cospi[26]);
    const __m256i  cospi38  = _mm256_set1_epi32(cospi[38]);
    const __m256i  cospim26 = _mm256_set1_epi32(-cospi[26]);
    const __m256i  cospi34  = _mm256_set1_epi32(cospi[34]);
    const __m256i  cospi30  = _mm256_set1_epi32(cospi[30]);
    const __m256i  cospim34 = _mm256_set1_epi32(-cospi[34]);
    const __m256i  cospi42  = _mm256_set1_epi32(cospi[42]);
    const __m256i  cospi22  = _mm256_set1_epi32(cospi[22]);
    const __m256i  cospim42 = _mm256_set1_epi32(-cospi[42]);
    const __m256i  cospi50  = _mm256_set1_epi32(cospi[50]);
    const __m256i  cospi14  = _mm256_set1_epi32(cospi[14]);
    const __m256i  cospim50 = _mm256_set1_epi32(-cospi[50]);
    const __m256i  cospi58  = _mm256_set1_epi32(cospi[58]);
    const __m256i  cospi6   = _mm256_set1_epi32(cospi[6]);
    const __m256i  cospim58 = _mm256_set1_epi32(-cospi[58]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  zero     = _mm256_setzero_si256();

    __m256i u[16], v[16], x, y;
    int32_t col;

    for (col = 0; col < col_num; ++col) {
        // stage 0
        // stage 1
        u[0]  = in[0 * col_num + col];
        u[1]  = _mm256_sub_epi32(zero, in[15 * col_num + col]);
        u[2]  = _mm256_sub_epi32(zero, in[7 * col_num + col]);
        u[3]  = in[8 * col_num + col];
        u[4]  = _mm256_sub_epi32(zero, in[3 * col_num + col]);
        u[5]  = in[12 * col_num + col];
        u[6]  = in[4 * col_num + col];
        u[7]  = _mm256_sub_epi32(zero, in[11 * col_num + col]);
        u[8]  = _mm256_sub_epi32(zero, in[1 * col_num + col]);
        u[9]  = in[14 * col_num + col];
        u[10] = in[6 * col_num + col];
        u[11] = _mm256_sub_epi32(zero, in[9 * col_num + col]);
        u[12] = in[2 * col_num + col];
        u[13] = _mm256_sub_epi32(zero, in[13 * col_num + col]);
        u[14] = _mm256_sub_epi32(zero, in[5 * col_num + col]);
        u[15] = in[10 * col_num + col];

        // stage 2
        v[0] = u[0];
        v[1] = u[1];

        x    = _mm256_mullo_epi32(u[2], cospi32);
        y    = _mm256_mullo_epi32(u[3], cospi32);
        v[2] = _mm256_add_epi32(x, y);
        v[2] = _mm256_add_epi32(v[2], rnding);
        v[2] = _mm256_srai_epi32(v[2], bit);

        v[3] = _mm256_sub_epi32(x, y);
        v[3] = _mm256_add_epi32(v[3], rnding);
        v[3] = _mm256_srai_epi32(v[3], bit);

        v[4] = u[4];
        v[5] = u[5];

        x    = _mm256_mullo_epi32(u[6], cospi32);
        y    = _mm256_mullo_epi32(u[7], cospi32);
        v[6] = _mm256_add_epi32(x, y);
        v[6] = _mm256_add_epi32(v[6], rnding);
        v[6] = _mm256_srai_epi32(v[6], bit);

        v[7] = _mm256_sub_epi32(x, y);
        v[7] = _mm256_add_epi32(v[7], rnding);
        v[7] = _mm256_srai_epi32(v[7], bit);

        v[8] = u[8];
        v[9] = u[9];

        x     = _mm256_mullo_epi32(u[10], cospi32);
        y     = _mm256_mullo_epi32(u[11], cospi32);
        v[10] = _mm256_add_epi32(x, y);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[11] = _mm256_sub_epi32(x, y);
        v[11] = _mm256_add_epi32(v[11], rnding);
        v[11] = _mm256_srai_epi32(v[11], bit);

        v[12] = u[12];
        v[13] = u[13];

        x     = _mm256_mullo_epi32(u[14], cospi32);
        y     = _mm256_mullo_epi32(u[15], cospi32);
        v[14] = _mm256_add_epi32(x, y);
        v[14] = _mm256_add_epi32(v[14], rnding);
        v[14] = _mm256_srai_epi32(v[14], bit);

        v[15] = _mm256_sub_epi32(x, y);
        v[15] = _mm256_add_epi32(v[15], rnding);
        v[15] = _mm256_srai_epi32(v[15], bit);

        // stage 3
        u[0]  = _mm256_add_epi32(v[0], v[2]);
        u[1]  = _mm256_add_epi32(v[1], v[3]);
        u[2]  = _mm256_sub_epi32(v[0], v[2]);
        u[3]  = _mm256_sub_epi32(v[1], v[3]);
        u[4]  = _mm256_add_epi32(v[4], v[6]);
        u[5]  = _mm256_add_epi32(v[5], v[7]);
        u[6]  = _mm256_sub_epi32(v[4], v[6]);
        u[7]  = _mm256_sub_epi32(v[5], v[7]);
        u[8]  = _mm256_add_epi32(v[8], v[10]);
        u[9]  = _mm256_add_epi32(v[9], v[11]);
        u[10] = _mm256_sub_epi32(v[8], v[10]);
        u[11] = _mm256_sub_epi32(v[9], v[11]);
        u[12] = _mm256_add_epi32(v[12], v[14]);
        u[13] = _mm256_add_epi32(v[13], v[15]);
        u[14] = _mm256_sub_epi32(v[12], v[14]);
        u[15] = _mm256_sub_epi32(v[13], v[15]);

        // stage 4
        v[0]  = u[0];
        v[1]  = u[1];
        v[2]  = u[2];
        v[3]  = u[3];
        v[4]  = half_btf_avx2(&cospi16, &u[4], &cospi48, &u[5], &rnding, bit);
        v[5]  = half_btf_avx2(&cospi48, &u[4], &cospim16, &u[5], &rnding, bit);
        v[6]  = half_btf_avx2(&cospim48, &u[6], &cospi16, &u[7], &rnding, bit);
        v[7]  = half_btf_avx2(&cospi16, &u[6], &cospi48, &u[7], &rnding, bit);
        v[8]  = u[8];
        v[9]  = u[9];
        v[10] = u[10];
        v[11] = u[11];
        v[12] = half_btf_avx2(&cospi16, &u[12], &cospi48, &u[13], &rnding, bit);
        v[13] = half_btf_avx2(&cospi48, &u[12], &cospim16, &u[13], &rnding, bit);
        v[14] = half_btf_avx2(&cospim48, &u[14], &cospi16, &u[15], &rnding, bit);
        v[15] = half_btf_avx2(&cospi16, &u[14], &cospi48, &u[15], &rnding, bit);

        // stage 5
        u[0]  = _mm256_add_epi32(v[0], v[4]);
        u[1]  = _mm256_add_epi32(v[1], v[5]);
        u[2]  = _mm256_add_epi32(v[2], v[6]);
        u[3]  = _mm256_add_epi32(v[3], v[7]);
        u[4]  = _mm256_sub_epi32(v[0], v[4]);
        u[5]  = _mm256_sub_epi32(v[1], v[5]);
        u[6]  = _mm256_sub_epi32(v[2], v[6]);
        u[7]  = _mm256_sub_epi32(v[3], v[7]);
        u[8]  = _mm256_add_epi32(v[8], v[12]);
        u[9]  = _mm256_add_epi32(v[9], v[13]);
        u[10] = _mm256_add_epi32(v[10], v[14]);
        u[11] = _mm256_add_epi32(v[11], v[15]);
        u[12] = _mm256_sub_epi32(v[8], v[12]);
        u[13] = _mm256_sub_epi32(v[9], v[13]);
        u[14] = _mm256_sub_epi32(v[10], v[14]);
        u[15] = _mm256_sub_epi32(v[11], v[15]);

        // stage 6
        v[0]  = u[0];
        v[1]  = u[1];
        v[2]  = u[2];
        v[3]  = u[3];
        v[4]  = u[4];
        v[5]  = u[5];
        v[6]  = u[6];
        v[7]  = u[7];
        v[8]  = half_btf_avx2(&cospi8, &u[8], &cospi56, &u[9], &rnding, bit);
        v[9]  = half_btf_avx2(&cospi56, &u[8], &cospim8, &u[9], &rnding, bit);
        v[10] = half_btf_avx2(&cospi40, &u[10], &cospi24, &u[11], &rnding, bit);
        v[11] = half_btf_avx2(&cospi24, &u[10], &cospim40, &u[11], &rnding, bit);
        v[12] = half_btf_avx2(&cospim56, &u[12], &cospi8, &u[13], &rnding, bit);
        v[13] = half_btf_avx2(&cospi8, &u[12], &cospi56, &u[13], &rnding, bit);
        v[14] = half_btf_avx2(&cospim24, &u[14], &cospi40, &u[15], &rnding, bit);
        v[15] = half_btf_avx2(&cospi40, &u[14], &cospi24, &u[15], &rnding, bit);

        // stage 7
        u[0]  = _mm256_add_epi32(v[0], v[8]);
        u[1]  = _mm256_add_epi32(v[1], v[9]);
        u[2]  = _mm256_add_epi32(v[2], v[10]);
        u[3]  = _mm256_add_epi32(v[3], v[11]);
        u[4]  = _mm256_add_epi32(v[4], v[12]);
        u[5]  = _mm256_add_epi32(v[5], v[13]);
        u[6]  = _mm256_add_epi32(v[6], v[14]);
        u[7]  = _mm256_add_epi32(v[7], v[15]);
        u[8]  = _mm256_sub_epi32(v[0], v[8]);
        u[9]  = _mm256_sub_epi32(v[1], v[9]);
        u[10] = _mm256_sub_epi32(v[2], v[10]);
        u[11] = _mm256_sub_epi32(v[3], v[11]);
        u[12] = _mm256_sub_epi32(v[4], v[12]);
        u[13] = _mm256_sub_epi32(v[5], v[13]);
        u[14] = _mm256_sub_epi32(v[6], v[14]);
        u[15] = _mm256_sub_epi32(v[7], v[15]);

        // stage 8
        v[0]  = half_btf_avx2(&cospi2, &u[0], &cospi62, &u[1], &rnding, bit);
        v[1]  = half_btf_avx2(&cospi62, &u[0], &cospim2, &u[1], &rnding, bit);
        v[2]  = half_btf_avx2(&cospi10, &u[2], &cospi54, &u[3], &rnding, bit);
        v[3]  = half_btf_avx2(&cospi54, &u[2], &cospim10, &u[3], &rnding, bit);
        v[4]  = half_btf_avx2(&cospi18, &u[4], &cospi46, &u[5], &rnding, bit);
        v[5]  = half_btf_avx2(&cospi46, &u[4], &cospim18, &u[5], &rnding, bit);
        v[6]  = half_btf_avx2(&cospi26, &u[6], &cospi38, &u[7], &rnding, bit);
        v[7]  = half_btf_avx2(&cospi38, &u[6], &cospim26, &u[7], &rnding, bit);
        v[8]  = half_btf_avx2(&cospi34, &u[8], &cospi30, &u[9], &rnding, bit);
        v[9]  = half_btf_avx2(&cospi30, &u[8], &cospim34, &u[9], &rnding, bit);
        v[10] = half_btf_avx2(&cospi42, &u[10], &cospi22, &u[11], &rnding, bit);
        v[11] = half_btf_avx2(&cospi22, &u[10], &cospim42, &u[11], &rnding, bit);
        v[12] = half_btf_avx2(&cospi50, &u[12], &cospi14, &u[13], &rnding, bit);
        v[13] = half_btf_avx2(&cospi14, &u[12], &cospim50, &u[13], &rnding, bit);
        v[14] = half_btf_avx2(&cospi58, &u[14], &cospi6, &u[15], &rnding, bit);
        v[15] = half_btf_avx2(&cospi6, &u[14], &cospim58, &u[15], &rnding, bit);

        // stage 9
        out[0 * col_num + col]  = v[1];
        out[1 * col_num + col]  = v[14];
        out[2 * col_num + col]  = v[3];
        out[3 * col_num + col]  = v[12];
        out[4 * col_num + col]  = v[5];
        out[5 * col_num + col]  = v[10];
        out[6 * col_num + col]  = v[7];
        out[7 * col_num + col]  = v[8];
        out[8 * col_num + col]  = v[9];
        out[9 * col_num + col]  = v[6];
        out[10 * col_num + col] = v[11];
        out[11 * col_num + col] = v[4];
        out[12 * col_num + col] = v[13];
        out[13 * col_num + col] = v[2];
        out[14 * col_num + col] = v[15];
        out[15 * col_num + col] = v[0];
    }
}

void svt_av1_fwd_txfm2d_16x16_avx2(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[32], out[32];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_16X16];
    const int32_t txw_idx = get_txw_idx(TX_16X16);
    const int32_t txh_idx = get_txh_idx(TX_16X16);
    const int32_t col_num = 2;
    switch (tx_type) {
    case IDTX:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        fidtx16x16_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16(out, coeff);
        break;
    case DCT_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16_avx2(out, in);
        fdct16x16_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx2(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16_avx2(out, in);
        fdct16x16_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx2(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16_avx2(out, in);
        fadst16x16_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx2(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16_avx2(out, in);
        fadst16x16_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx2(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        fdct16x16_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16_avx2(out, in);
        fadst16x16_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx2(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16_avx2(out, in);
        fdct16x16_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx2(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x16(input, in, stride, 1, 1, shift[0]);
        fadst16x16_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16_avx2(out, in);
        fadst16x16_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx2(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        fadst16x16_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16_avx2(out, in);
        fadst16x16_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx2(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        transpose_16x16_avx2(out, in);
        fadst16x16_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx2(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case V_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        fidtx16x16_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16(out, coeff);
        break;
    case H_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(in, -shift[1]);
        transpose_16x16_avx2(in, out);
        fdct16x16_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx2(in, out);
        write_buffer_16x16(out, coeff);
        break;
    case V_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        fidtx16x16_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16(out, coeff);
        break;
    case H_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(in, -shift[1]);
        transpose_16x16_avx2(in, out);
        fadst16x16_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx2(in, out);
        write_buffer_16x16(out, coeff);
        break;
    case V_FLIPADST:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(out, -shift[1]);
        fidtx16x16_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16(out, coeff);
        break;
    case H_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        fidtx16x16_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding(in, -shift[1]);
        transpose_16x16_avx2(in, out);
        fadst16x16_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx2(in, out);
        write_buffer_16x16(out, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

static void av1_fdct32_new_avx2(const __m256i* input, __m256i* output, int8_t cos_bit, const int32_t col_num,
                                const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m256i  __rounding = _mm256_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 3;

    __m256i cospi_m32 = _mm256_set1_epi32(-cospi[32]);
    __m256i cospi_p32 = _mm256_set1_epi32(cospi[32]);
    __m256i cospi_m16 = _mm256_set1_epi32(-cospi[16]);
    __m256i cospi_p48 = _mm256_set1_epi32(cospi[48]);
    __m256i cospi_m48 = _mm256_set1_epi32(-cospi[48]);
    __m256i cospi_m08 = _mm256_set1_epi32(-cospi[8]);
    __m256i cospi_p56 = _mm256_set1_epi32(cospi[56]);
    __m256i cospi_m56 = _mm256_set1_epi32(-cospi[56]);
    __m256i cospi_p40 = _mm256_set1_epi32(cospi[40]);
    __m256i cospi_m40 = _mm256_set1_epi32(-cospi[40]);
    __m256i cospi_p24 = _mm256_set1_epi32(cospi[24]);
    __m256i cospi_m24 = _mm256_set1_epi32(-cospi[24]);
    __m256i cospi_p16 = _mm256_set1_epi32(cospi[16]);
    __m256i cospi_p08 = _mm256_set1_epi32(cospi[8]);
    __m256i cospi_p04 = _mm256_set1_epi32(cospi[4]);
    __m256i cospi_p60 = _mm256_set1_epi32(cospi[60]);
    __m256i cospi_p36 = _mm256_set1_epi32(cospi[36]);
    __m256i cospi_p28 = _mm256_set1_epi32(cospi[28]);
    __m256i cospi_p20 = _mm256_set1_epi32(cospi[20]);
    __m256i cospi_p44 = _mm256_set1_epi32(cospi[44]);
    __m256i cospi_p52 = _mm256_set1_epi32(cospi[52]);
    __m256i cospi_p12 = _mm256_set1_epi32(cospi[12]);
    __m256i cospi_p02 = _mm256_set1_epi32(cospi[2]);
    __m256i cospi_p06 = _mm256_set1_epi32(cospi[6]);
    __m256i cospi_p62 = _mm256_set1_epi32(cospi[62]);
    __m256i cospi_p34 = _mm256_set1_epi32(cospi[34]);
    __m256i cospi_p30 = _mm256_set1_epi32(cospi[30]);
    __m256i cospi_p18 = _mm256_set1_epi32(cospi[18]);
    __m256i cospi_p46 = _mm256_set1_epi32(cospi[46]);
    __m256i cospi_p50 = _mm256_set1_epi32(cospi[50]);
    __m256i cospi_p14 = _mm256_set1_epi32(cospi[14]);
    __m256i cospi_p10 = _mm256_set1_epi32(cospi[10]);
    __m256i cospi_p54 = _mm256_set1_epi32(cospi[54]);
    __m256i cospi_p42 = _mm256_set1_epi32(cospi[42]);
    __m256i cospi_p22 = _mm256_set1_epi32(cospi[22]);
    __m256i cospi_p26 = _mm256_set1_epi32(cospi[26]);
    __m256i cospi_p38 = _mm256_set1_epi32(cospi[38]);
    __m256i cospi_p58 = _mm256_set1_epi32(cospi[58]);

    __m256i buf0[32];
    __m256i buf1[32];

    for (int32_t col = 0; col < columns; col++) {
        const __m256i* in  = &input[col];
        __m256i*       out = &output[col];

        // stage 0
        // stage 1
        buf1[0]  = _mm256_add_epi32(in[0 * stride], in[31 * stride]);
        buf1[31] = _mm256_sub_epi32(in[0 * stride], in[31 * stride]);
        buf1[1]  = _mm256_add_epi32(in[1 * stride], in[30 * stride]);
        buf1[30] = _mm256_sub_epi32(in[1 * stride], in[30 * stride]);
        buf1[2]  = _mm256_add_epi32(in[2 * stride], in[29 * stride]);
        buf1[29] = _mm256_sub_epi32(in[2 * stride], in[29 * stride]);
        buf1[3]  = _mm256_add_epi32(in[3 * stride], in[28 * stride]);
        buf1[28] = _mm256_sub_epi32(in[3 * stride], in[28 * stride]);
        buf1[4]  = _mm256_add_epi32(in[4 * stride], in[27 * stride]);
        buf1[27] = _mm256_sub_epi32(in[4 * stride], in[27 * stride]);
        buf1[5]  = _mm256_add_epi32(in[5 * stride], in[26 * stride]);
        buf1[26] = _mm256_sub_epi32(in[5 * stride], in[26 * stride]);
        buf1[6]  = _mm256_add_epi32(in[6 * stride], in[25 * stride]);
        buf1[25] = _mm256_sub_epi32(in[6 * stride], in[25 * stride]);
        buf1[7]  = _mm256_add_epi32(in[7 * stride], in[24 * stride]);
        buf1[24] = _mm256_sub_epi32(in[7 * stride], in[24 * stride]);
        buf1[8]  = _mm256_add_epi32(in[8 * stride], in[23 * stride]);
        buf1[23] = _mm256_sub_epi32(in[8 * stride], in[23 * stride]);
        buf1[9]  = _mm256_add_epi32(in[9 * stride], in[22 * stride]);
        buf1[22] = _mm256_sub_epi32(in[9 * stride], in[22 * stride]);
        buf1[10] = _mm256_add_epi32(in[10 * stride], in[21 * stride]);
        buf1[21] = _mm256_sub_epi32(in[10 * stride], in[21 * stride]);
        buf1[11] = _mm256_add_epi32(in[11 * stride], in[20 * stride]);
        buf1[20] = _mm256_sub_epi32(in[11 * stride], in[20 * stride]);
        buf1[12] = _mm256_add_epi32(in[12 * stride], in[19 * stride]);
        buf1[19] = _mm256_sub_epi32(in[12 * stride], in[19 * stride]);
        buf1[13] = _mm256_add_epi32(in[13 * stride], in[18 * stride]);
        buf1[18] = _mm256_sub_epi32(in[13 * stride], in[18 * stride]);
        buf1[14] = _mm256_add_epi32(in[14 * stride], in[17 * stride]);
        buf1[17] = _mm256_sub_epi32(in[14 * stride], in[17 * stride]);
        buf1[15] = _mm256_add_epi32(in[15 * stride], in[16 * stride]);
        buf1[16] = _mm256_sub_epi32(in[15 * stride], in[16 * stride]);

        // stage 2
        buf0[0]  = _mm256_add_epi32(buf1[0], buf1[15]);
        buf0[15] = _mm256_sub_epi32(buf1[0], buf1[15]);
        buf0[1]  = _mm256_add_epi32(buf1[1], buf1[14]);
        buf0[14] = _mm256_sub_epi32(buf1[1], buf1[14]);
        buf0[2]  = _mm256_add_epi32(buf1[2], buf1[13]);
        buf0[13] = _mm256_sub_epi32(buf1[2], buf1[13]);
        buf0[3]  = _mm256_add_epi32(buf1[3], buf1[12]);
        buf0[12] = _mm256_sub_epi32(buf1[3], buf1[12]);
        buf0[4]  = _mm256_add_epi32(buf1[4], buf1[11]);
        buf0[11] = _mm256_sub_epi32(buf1[4], buf1[11]);
        buf0[5]  = _mm256_add_epi32(buf1[5], buf1[10]);
        buf0[10] = _mm256_sub_epi32(buf1[5], buf1[10]);
        buf0[6]  = _mm256_add_epi32(buf1[6], buf1[9]);
        buf0[9]  = _mm256_sub_epi32(buf1[6], buf1[9]);
        buf0[7]  = _mm256_add_epi32(buf1[7], buf1[8]);
        buf0[8]  = _mm256_sub_epi32(buf1[7], buf1[8]);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[23], buf1[24], buf0[23], buf0[24], __rounding, cos_bit);

        // stage 3
        buf1[0] = _mm256_add_epi32(buf0[0], buf0[7]);
        buf1[7] = _mm256_sub_epi32(buf0[0], buf0[7]);
        buf1[1] = _mm256_add_epi32(buf0[1], buf0[6]);
        buf1[6] = _mm256_sub_epi32(buf0[1], buf0[6]);
        buf1[2] = _mm256_add_epi32(buf0[2], buf0[5]);
        buf1[5] = _mm256_sub_epi32(buf0[2], buf0[5]);
        buf1[3] = _mm256_add_epi32(buf0[3], buf0[4]);
        buf1[4] = _mm256_sub_epi32(buf0[3], buf0[4]);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf0[11], buf0[12], buf1[11], buf1[12], __rounding, cos_bit);
        buf1[23] = _mm256_sub_epi32(buf1[16], buf0[23]);
        buf1[16] = _mm256_add_epi32(buf1[16], buf0[23]);
        buf1[22] = _mm256_sub_epi32(buf1[17], buf0[22]);
        buf1[17] = _mm256_add_epi32(buf1[17], buf0[22]);
        buf1[21] = _mm256_sub_epi32(buf1[18], buf0[21]);
        buf1[18] = _mm256_add_epi32(buf1[18], buf0[21]);
        buf1[20] = _mm256_sub_epi32(buf1[19], buf0[20]);
        buf1[19] = _mm256_add_epi32(buf1[19], buf0[20]);
        buf1[24] = _mm256_sub_epi32(buf1[31], buf0[24]);
        buf1[31] = _mm256_add_epi32(buf1[31], buf0[24]);
        buf1[25] = _mm256_sub_epi32(buf1[30], buf0[25]);
        buf1[30] = _mm256_add_epi32(buf1[30], buf0[25]);
        buf1[26] = _mm256_sub_epi32(buf1[29], buf0[26]);
        buf1[29] = _mm256_add_epi32(buf1[29], buf0[26]);
        buf1[27] = _mm256_sub_epi32(buf1[28], buf0[27]);
        buf1[28] = _mm256_add_epi32(buf1[28], buf0[27]);

        // stage 4
        buf0[0] = _mm256_add_epi32(buf1[0], buf1[3]);
        buf0[3] = _mm256_sub_epi32(buf1[0], buf1[3]);
        buf0[1] = _mm256_add_epi32(buf1[1], buf1[2]);
        buf0[2] = _mm256_sub_epi32(buf1[1], buf1[2]);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[5], buf1[6], buf0[5], buf0[6], __rounding, cos_bit);
        buf0[11] = _mm256_sub_epi32(buf0[8], buf1[11]);
        buf0[8]  = _mm256_add_epi32(buf0[8], buf1[11]);
        buf0[10] = _mm256_sub_epi32(buf0[9], buf1[10]);
        buf0[9]  = _mm256_add_epi32(buf0[9], buf1[10]);
        buf0[12] = _mm256_sub_epi32(buf0[15], buf1[12]);
        buf0[15] = _mm256_add_epi32(buf0[15], buf1[12]);
        buf0[13] = _mm256_sub_epi32(buf0[14], buf1[13]);
        buf0[14] = _mm256_add_epi32(buf0[14], buf1[13]);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, buf1[19], buf1[28], buf0[19], buf0[28], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);

        // stage 5
        btf_32_type0_avx2_new(
            cospi_p32, cospi_p32, buf0[0], buf0[1], out[0 * stride], out[16 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p48, cospi_p16, buf0[2], buf0[3], out[8 * stride], out[24 * stride], __rounding, cos_bit);
        buf1[5] = _mm256_sub_epi32(buf1[4], buf0[5]);
        buf1[4] = _mm256_add_epi32(buf1[4], buf0[5]);
        buf1[6] = _mm256_sub_epi32(buf1[7], buf0[6]);
        buf1[7] = _mm256_add_epi32(buf1[7], buf0[6]);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, buf0[9], buf0[14], buf1[9], buf1[14], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        buf1[19] = _mm256_sub_epi32(buf1[16], buf0[19]);
        buf1[16] = _mm256_add_epi32(buf1[16], buf0[19]);
        buf1[18] = _mm256_sub_epi32(buf1[17], buf0[18]);
        buf1[17] = _mm256_add_epi32(buf1[17], buf0[18]);
        buf1[20] = _mm256_sub_epi32(buf1[23], buf0[20]);
        buf1[23] = _mm256_add_epi32(buf1[23], buf0[20]);
        buf1[21] = _mm256_sub_epi32(buf1[22], buf0[21]);
        buf1[22] = _mm256_add_epi32(buf1[22], buf0[21]);
        buf1[27] = _mm256_sub_epi32(buf1[24], buf0[27]);
        buf1[24] = _mm256_add_epi32(buf1[24], buf0[27]);
        buf1[26] = _mm256_sub_epi32(buf1[25], buf0[26]);
        buf1[25] = _mm256_add_epi32(buf1[25], buf0[26]);
        buf1[28] = _mm256_sub_epi32(buf1[31], buf0[28]);
        buf1[31] = _mm256_add_epi32(buf1[31], buf0[28]);
        buf1[29] = _mm256_sub_epi32(buf1[30], buf0[29]);
        buf1[30] = _mm256_add_epi32(buf1[30], buf0[29]);

        // stage 6
        btf_32_type1_avx2_new(
            cospi_p56, cospi_p08, buf1[4], buf1[7], out[4 * stride], out[28 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p24, cospi_p40, buf1[5], buf1[6], out[20 * stride], out[12 * stride], __rounding, cos_bit);
        buf0[9]  = _mm256_sub_epi32(buf0[8], buf1[9]);
        buf0[8]  = _mm256_add_epi32(buf0[8], buf1[9]);
        buf0[10] = _mm256_sub_epi32(buf0[11], buf1[10]);
        buf0[11] = _mm256_add_epi32(buf0[11], buf1[10]);
        buf0[13] = _mm256_sub_epi32(buf0[12], buf1[13]);
        buf0[12] = _mm256_add_epi32(buf0[12], buf1[13]);
        buf0[14] = _mm256_sub_epi32(buf0[15], buf1[14]);
        buf0[15] = _mm256_add_epi32(buf0[15], buf1[14]);
        btf_32_type0_avx2_new(cospi_m08, cospi_p56, buf1[17], buf1[30], buf0[17], buf0[30], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m56, cospi_m08, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m40, cospi_p24, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m24, cospi_m40, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);

        // stage 7
        btf_32_type1_avx2_new(
            cospi_p60, cospi_p04, buf0[8], buf0[15], out[2 * stride], out[30 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p28, cospi_p36, buf0[9], buf0[14], out[18 * stride], out[14 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p44, cospi_p20, buf0[10], buf0[13], out[10 * stride], out[22 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p12, cospi_p52, buf0[11], buf0[12], out[26 * stride], out[6 * stride], __rounding, cos_bit);
        buf1[17] = _mm256_sub_epi32(buf1[16], buf0[17]);
        buf1[16] = _mm256_add_epi32(buf1[16], buf0[17]);
        buf1[18] = _mm256_sub_epi32(buf1[19], buf0[18]);
        buf1[19] = _mm256_add_epi32(buf1[19], buf0[18]);
        buf1[21] = _mm256_sub_epi32(buf1[20], buf0[21]);
        buf1[20] = _mm256_add_epi32(buf1[20], buf0[21]);
        buf1[22] = _mm256_sub_epi32(buf1[23], buf0[22]);
        buf1[23] = _mm256_add_epi32(buf1[23], buf0[22]);
        buf1[25] = _mm256_sub_epi32(buf1[24], buf0[25]);
        buf1[24] = _mm256_add_epi32(buf1[24], buf0[25]);
        buf1[26] = _mm256_sub_epi32(buf1[27], buf0[26]);
        buf1[27] = _mm256_add_epi32(buf1[27], buf0[26]);
        buf1[29] = _mm256_sub_epi32(buf1[28], buf0[29]);
        buf1[28] = _mm256_add_epi32(buf1[28], buf0[29]);
        buf1[30] = _mm256_sub_epi32(buf1[31], buf0[30]);
        buf1[31] = _mm256_add_epi32(buf1[31], buf0[30]);

        // stage 8
        btf_32_type1_avx2_new(
            cospi_p62, cospi_p02, buf1[16], buf1[31], out[1 * stride], out[31 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p30, cospi_p34, buf1[17], buf1[30], out[17 * stride], out[15 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p46, cospi_p18, buf1[18], buf1[29], out[9 * stride], out[23 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p14, cospi_p50, buf1[19], buf1[28], out[25 * stride], out[7 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p54, cospi_p10, buf1[20], buf1[27], out[5 * stride], out[27 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p22, cospi_p42, buf1[21], buf1[26], out[21 * stride], out[11 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p38, cospi_p26, buf1[22], buf1[25], out[13 * stride], out[19 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p06, cospi_p58, buf1[23], buf1[24], out[29 * stride], out[3 * stride], __rounding, cos_bit);
    }
}

static void av1_fdct32_new_line_wraper_avx2(const __m256i* input, __m256i* output, int8_t cos_bit,
                                            const int32_t stride) {
    av1_fdct32_new_avx2(input, output, cos_bit, 8, stride);
}

static void av1_fdct64_new_avx2(const __m256i* input, __m256i* output, int8_t cos_bit, const int32_t col_num,
                                const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m256i  __rounding = _mm256_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 3;

    __m256i cospi_m32 = _mm256_set1_epi32(-cospi[32]);
    __m256i cospi_p32 = _mm256_set1_epi32(cospi[32]);
    __m256i cospi_m16 = _mm256_set1_epi32(-cospi[16]);
    __m256i cospi_p48 = _mm256_set1_epi32(cospi[48]);
    __m256i cospi_m48 = _mm256_set1_epi32(-cospi[48]);
    __m256i cospi_p16 = _mm256_set1_epi32(cospi[16]);
    __m256i cospi_m08 = _mm256_set1_epi32(-cospi[8]);
    __m256i cospi_p56 = _mm256_set1_epi32(cospi[56]);
    __m256i cospi_m56 = _mm256_set1_epi32(-cospi[56]);
    __m256i cospi_m40 = _mm256_set1_epi32(-cospi[40]);
    __m256i cospi_p24 = _mm256_set1_epi32(cospi[24]);
    __m256i cospi_m24 = _mm256_set1_epi32(-cospi[24]);
    __m256i cospi_p08 = _mm256_set1_epi32(cospi[8]);
    __m256i cospi_p40 = _mm256_set1_epi32(cospi[40]);
    __m256i cospi_p60 = _mm256_set1_epi32(cospi[60]);
    __m256i cospi_p04 = _mm256_set1_epi32(cospi[4]);
    __m256i cospi_p28 = _mm256_set1_epi32(cospi[28]);
    __m256i cospi_p36 = _mm256_set1_epi32(cospi[36]);
    __m256i cospi_p44 = _mm256_set1_epi32(cospi[44]);
    __m256i cospi_p20 = _mm256_set1_epi32(cospi[20]);
    __m256i cospi_p12 = _mm256_set1_epi32(cospi[12]);
    __m256i cospi_p52 = _mm256_set1_epi32(cospi[52]);
    __m256i cospi_m04 = _mm256_set1_epi32(-cospi[4]);
    __m256i cospi_m60 = _mm256_set1_epi32(-cospi[60]);
    __m256i cospi_m36 = _mm256_set1_epi32(-cospi[36]);
    __m256i cospi_m28 = _mm256_set1_epi32(-cospi[28]);
    __m256i cospi_m20 = _mm256_set1_epi32(-cospi[20]);
    __m256i cospi_m44 = _mm256_set1_epi32(-cospi[44]);
    __m256i cospi_m52 = _mm256_set1_epi32(-cospi[52]);
    __m256i cospi_m12 = _mm256_set1_epi32(-cospi[12]);
    __m256i cospi_p62 = _mm256_set1_epi32(cospi[62]);
    __m256i cospi_p02 = _mm256_set1_epi32(cospi[2]);
    __m256i cospi_p30 = _mm256_set1_epi32(cospi[30]);
    __m256i cospi_p34 = _mm256_set1_epi32(cospi[34]);
    __m256i cospi_p46 = _mm256_set1_epi32(cospi[46]);
    __m256i cospi_p18 = _mm256_set1_epi32(cospi[18]);
    __m256i cospi_p14 = _mm256_set1_epi32(cospi[14]);
    __m256i cospi_p50 = _mm256_set1_epi32(cospi[50]);
    __m256i cospi_p54 = _mm256_set1_epi32(cospi[54]);
    __m256i cospi_p10 = _mm256_set1_epi32(cospi[10]);
    __m256i cospi_p22 = _mm256_set1_epi32(cospi[22]);
    __m256i cospi_p42 = _mm256_set1_epi32(cospi[42]);
    __m256i cospi_p38 = _mm256_set1_epi32(cospi[38]);
    __m256i cospi_p26 = _mm256_set1_epi32(cospi[26]);
    __m256i cospi_p06 = _mm256_set1_epi32(cospi[6]);
    __m256i cospi_p58 = _mm256_set1_epi32(cospi[58]);
    __m256i cospi_p63 = _mm256_set1_epi32(cospi[63]);
    __m256i cospi_p01 = _mm256_set1_epi32(cospi[1]);
    __m256i cospi_p31 = _mm256_set1_epi32(cospi[31]);
    __m256i cospi_p33 = _mm256_set1_epi32(cospi[33]);
    __m256i cospi_p47 = _mm256_set1_epi32(cospi[47]);
    __m256i cospi_p17 = _mm256_set1_epi32(cospi[17]);
    __m256i cospi_p15 = _mm256_set1_epi32(cospi[15]);
    __m256i cospi_p49 = _mm256_set1_epi32(cospi[49]);
    __m256i cospi_p55 = _mm256_set1_epi32(cospi[55]);
    __m256i cospi_p09 = _mm256_set1_epi32(cospi[9]);
    __m256i cospi_p23 = _mm256_set1_epi32(cospi[23]);
    __m256i cospi_p41 = _mm256_set1_epi32(cospi[41]);
    __m256i cospi_p39 = _mm256_set1_epi32(cospi[39]);
    __m256i cospi_p25 = _mm256_set1_epi32(cospi[25]);
    __m256i cospi_p07 = _mm256_set1_epi32(cospi[7]);
    __m256i cospi_p57 = _mm256_set1_epi32(cospi[57]);
    __m256i cospi_p59 = _mm256_set1_epi32(cospi[59]);
    __m256i cospi_p05 = _mm256_set1_epi32(cospi[5]);
    __m256i cospi_p27 = _mm256_set1_epi32(cospi[27]);
    __m256i cospi_p37 = _mm256_set1_epi32(cospi[37]);
    __m256i cospi_p43 = _mm256_set1_epi32(cospi[43]);
    __m256i cospi_p21 = _mm256_set1_epi32(cospi[21]);
    __m256i cospi_p11 = _mm256_set1_epi32(cospi[11]);
    __m256i cospi_p53 = _mm256_set1_epi32(cospi[53]);
    __m256i cospi_p51 = _mm256_set1_epi32(cospi[51]);
    __m256i cospi_p13 = _mm256_set1_epi32(cospi[13]);
    __m256i cospi_p19 = _mm256_set1_epi32(cospi[19]);
    __m256i cospi_p45 = _mm256_set1_epi32(cospi[45]);
    __m256i cospi_p35 = _mm256_set1_epi32(cospi[35]);
    __m256i cospi_p29 = _mm256_set1_epi32(cospi[29]);
    __m256i cospi_p03 = _mm256_set1_epi32(cospi[3]);
    __m256i cospi_p61 = _mm256_set1_epi32(cospi[61]);

    for (int32_t col = 0; col < columns; col++) {
        const __m256i* in  = &input[col];
        __m256i*       out = &output[col];

        // stage 1
        __m256i x1[64];
        x1[0]  = _mm256_add_epi32(in[0 * stride], in[63 * stride]);
        x1[63] = _mm256_sub_epi32(in[0 * stride], in[63 * stride]);
        x1[1]  = _mm256_add_epi32(in[1 * stride], in[62 * stride]);
        x1[62] = _mm256_sub_epi32(in[1 * stride], in[62 * stride]);
        x1[2]  = _mm256_add_epi32(in[2 * stride], in[61 * stride]);
        x1[61] = _mm256_sub_epi32(in[2 * stride], in[61 * stride]);
        x1[3]  = _mm256_add_epi32(in[3 * stride], in[60 * stride]);
        x1[60] = _mm256_sub_epi32(in[3 * stride], in[60 * stride]);
        x1[4]  = _mm256_add_epi32(in[4 * stride], in[59 * stride]);
        x1[59] = _mm256_sub_epi32(in[4 * stride], in[59 * stride]);
        x1[5]  = _mm256_add_epi32(in[5 * stride], in[58 * stride]);
        x1[58] = _mm256_sub_epi32(in[5 * stride], in[58 * stride]);
        x1[6]  = _mm256_add_epi32(in[6 * stride], in[57 * stride]);
        x1[57] = _mm256_sub_epi32(in[6 * stride], in[57 * stride]);
        x1[7]  = _mm256_add_epi32(in[7 * stride], in[56 * stride]);
        x1[56] = _mm256_sub_epi32(in[7 * stride], in[56 * stride]);
        x1[8]  = _mm256_add_epi32(in[8 * stride], in[55 * stride]);
        x1[55] = _mm256_sub_epi32(in[8 * stride], in[55 * stride]);
        x1[9]  = _mm256_add_epi32(in[9 * stride], in[54 * stride]);
        x1[54] = _mm256_sub_epi32(in[9 * stride], in[54 * stride]);
        x1[10] = _mm256_add_epi32(in[10 * stride], in[53 * stride]);
        x1[53] = _mm256_sub_epi32(in[10 * stride], in[53 * stride]);
        x1[11] = _mm256_add_epi32(in[11 * stride], in[52 * stride]);
        x1[52] = _mm256_sub_epi32(in[11 * stride], in[52 * stride]);
        x1[12] = _mm256_add_epi32(in[12 * stride], in[51 * stride]);
        x1[51] = _mm256_sub_epi32(in[12 * stride], in[51 * stride]);
        x1[13] = _mm256_add_epi32(in[13 * stride], in[50 * stride]);
        x1[50] = _mm256_sub_epi32(in[13 * stride], in[50 * stride]);
        x1[14] = _mm256_add_epi32(in[14 * stride], in[49 * stride]);
        x1[49] = _mm256_sub_epi32(in[14 * stride], in[49 * stride]);
        x1[15] = _mm256_add_epi32(in[15 * stride], in[48 * stride]);
        x1[48] = _mm256_sub_epi32(in[15 * stride], in[48 * stride]);
        x1[16] = _mm256_add_epi32(in[16 * stride], in[47 * stride]);
        x1[47] = _mm256_sub_epi32(in[16 * stride], in[47 * stride]);
        x1[17] = _mm256_add_epi32(in[17 * stride], in[46 * stride]);
        x1[46] = _mm256_sub_epi32(in[17 * stride], in[46 * stride]);
        x1[18] = _mm256_add_epi32(in[18 * stride], in[45 * stride]);
        x1[45] = _mm256_sub_epi32(in[18 * stride], in[45 * stride]);
        x1[19] = _mm256_add_epi32(in[19 * stride], in[44 * stride]);
        x1[44] = _mm256_sub_epi32(in[19 * stride], in[44 * stride]);
        x1[20] = _mm256_add_epi32(in[20 * stride], in[43 * stride]);
        x1[43] = _mm256_sub_epi32(in[20 * stride], in[43 * stride]);
        x1[21] = _mm256_add_epi32(in[21 * stride], in[42 * stride]);
        x1[42] = _mm256_sub_epi32(in[21 * stride], in[42 * stride]);
        x1[22] = _mm256_add_epi32(in[22 * stride], in[41 * stride]);
        x1[41] = _mm256_sub_epi32(in[22 * stride], in[41 * stride]);
        x1[23] = _mm256_add_epi32(in[23 * stride], in[40 * stride]);
        x1[40] = _mm256_sub_epi32(in[23 * stride], in[40 * stride]);
        x1[24] = _mm256_add_epi32(in[24 * stride], in[39 * stride]);
        x1[39] = _mm256_sub_epi32(in[24 * stride], in[39 * stride]);
        x1[25] = _mm256_add_epi32(in[25 * stride], in[38 * stride]);
        x1[38] = _mm256_sub_epi32(in[25 * stride], in[38 * stride]);
        x1[26] = _mm256_add_epi32(in[26 * stride], in[37 * stride]);
        x1[37] = _mm256_sub_epi32(in[26 * stride], in[37 * stride]);
        x1[27] = _mm256_add_epi32(in[27 * stride], in[36 * stride]);
        x1[36] = _mm256_sub_epi32(in[27 * stride], in[36 * stride]);
        x1[28] = _mm256_add_epi32(in[28 * stride], in[35 * stride]);
        x1[35] = _mm256_sub_epi32(in[28 * stride], in[35 * stride]);
        x1[29] = _mm256_add_epi32(in[29 * stride], in[34 * stride]);
        x1[34] = _mm256_sub_epi32(in[29 * stride], in[34 * stride]);
        x1[30] = _mm256_add_epi32(in[30 * stride], in[33 * stride]);
        x1[33] = _mm256_sub_epi32(in[30 * stride], in[33 * stride]);
        x1[31] = _mm256_add_epi32(in[31 * stride], in[32 * stride]);
        x1[32] = _mm256_sub_epi32(in[31 * stride], in[32 * stride]);

        // stage 2
        __m256i x2[54];
        x2[0]  = _mm256_add_epi32(x1[0], x1[31]);
        x2[31] = _mm256_sub_epi32(x1[0], x1[31]);
        x2[1]  = _mm256_add_epi32(x1[1], x1[30]);
        x2[30] = _mm256_sub_epi32(x1[1], x1[30]);
        x2[2]  = _mm256_add_epi32(x1[2], x1[29]);
        x2[29] = _mm256_sub_epi32(x1[2], x1[29]);
        x2[3]  = _mm256_add_epi32(x1[3], x1[28]);
        x2[28] = _mm256_sub_epi32(x1[3], x1[28]);
        x2[4]  = _mm256_add_epi32(x1[4], x1[27]);
        x2[27] = _mm256_sub_epi32(x1[4], x1[27]);
        x2[5]  = _mm256_add_epi32(x1[5], x1[26]);
        x2[26] = _mm256_sub_epi32(x1[5], x1[26]);
        x2[6]  = _mm256_add_epi32(x1[6], x1[25]);
        x2[25] = _mm256_sub_epi32(x1[6], x1[25]);
        x2[7]  = _mm256_add_epi32(x1[7], x1[24]);
        x2[24] = _mm256_sub_epi32(x1[7], x1[24]);
        x2[8]  = _mm256_add_epi32(x1[8], x1[23]);
        x2[23] = _mm256_sub_epi32(x1[8], x1[23]);
        x2[9]  = _mm256_add_epi32(x1[9], x1[22]);
        x2[22] = _mm256_sub_epi32(x1[9], x1[22]);
        x2[10] = _mm256_add_epi32(x1[10], x1[21]);
        x2[21] = _mm256_sub_epi32(x1[10], x1[21]);
        x2[11] = _mm256_add_epi32(x1[11], x1[20]);
        x2[20] = _mm256_sub_epi32(x1[11], x1[20]);
        x2[12] = _mm256_add_epi32(x1[12], x1[19]);
        x2[19] = _mm256_sub_epi32(x1[12], x1[19]);
        x2[13] = _mm256_add_epi32(x1[13], x1[18]);
        x2[18] = _mm256_sub_epi32(x1[13], x1[18]);
        x2[14] = _mm256_add_epi32(x1[14], x1[17]);
        x2[17] = _mm256_sub_epi32(x1[14], x1[17]);
        x2[15] = _mm256_add_epi32(x1[15], x1[16]);
        x2[16] = _mm256_sub_epi32(x1[15], x1[16]);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[40], x1[55], x2[32], x2[47], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[41], x1[54], x2[33], x2[46], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[42], x1[53], x2[34], x2[45], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[43], x1[52], x2[35], x2[44], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[44], x1[51], x2[36], x2[43], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[45], x1[50], x2[37], x2[42], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[46], x1[49], x2[38], x2[41], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[47], x1[48], x2[39], x2[40], __rounding, cos_bit);

        // stage 3
        __m256i x3[56];
        x3[0]  = _mm256_add_epi32(x2[0], x2[15]);
        x3[15] = _mm256_sub_epi32(x2[0], x2[15]);
        x3[1]  = _mm256_add_epi32(x2[1], x2[14]);
        x3[14] = _mm256_sub_epi32(x2[1], x2[14]);
        x3[2]  = _mm256_add_epi32(x2[2], x2[13]);
        x3[13] = _mm256_sub_epi32(x2[2], x2[13]);
        x3[3]  = _mm256_add_epi32(x2[3], x2[12]);
        x3[12] = _mm256_sub_epi32(x2[3], x2[12]);
        x3[4]  = _mm256_add_epi32(x2[4], x2[11]);
        x3[11] = _mm256_sub_epi32(x2[4], x2[11]);
        x3[5]  = _mm256_add_epi32(x2[5], x2[10]);
        x3[10] = _mm256_sub_epi32(x2[5], x2[10]);
        x3[6]  = _mm256_add_epi32(x2[6], x2[9]);
        x3[9]  = _mm256_sub_epi32(x2[6], x2[9]);
        x3[7]  = _mm256_add_epi32(x2[7], x2[8]);
        x3[8]  = _mm256_sub_epi32(x2[7], x2[8]);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x2[20], x2[27], x3[16], x3[23], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x2[21], x2[26], x3[17], x3[22], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x2[22], x2[25], x3[18], x3[21], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x2[23], x2[24], x3[19], x3[20], __rounding, cos_bit);
        x3[32] = _mm256_add_epi32(x1[32], x2[39]);
        x3[47] = _mm256_sub_epi32(x1[32], x2[39]);
        x3[33] = _mm256_add_epi32(x1[33], x2[38]);
        x3[46] = _mm256_sub_epi32(x1[33], x2[38]);
        x3[34] = _mm256_add_epi32(x1[34], x2[37]);
        x3[45] = _mm256_sub_epi32(x1[34], x2[37]);
        x3[35] = _mm256_add_epi32(x1[35], x2[36]);
        x3[44] = _mm256_sub_epi32(x1[35], x2[36]);
        x3[36] = _mm256_add_epi32(x1[36], x2[35]);
        x3[43] = _mm256_sub_epi32(x1[36], x2[35]);
        x3[37] = _mm256_add_epi32(x1[37], x2[34]);
        x3[42] = _mm256_sub_epi32(x1[37], x2[34]);
        x3[38] = _mm256_add_epi32(x1[38], x2[33]);
        x3[41] = _mm256_sub_epi32(x1[38], x2[33]);
        x3[39] = _mm256_add_epi32(x1[39], x2[32]);
        x3[40] = _mm256_sub_epi32(x1[39], x2[32]);
        x3[48] = _mm256_sub_epi32(x1[63], x2[40]);
        x3[24] = _mm256_add_epi32(x1[63], x2[40]);
        x3[49] = _mm256_sub_epi32(x1[62], x2[41]);
        x3[25] = _mm256_add_epi32(x1[62], x2[41]);
        x3[50] = _mm256_sub_epi32(x1[61], x2[42]);
        x3[26] = _mm256_add_epi32(x1[61], x2[42]);
        x3[51] = _mm256_sub_epi32(x1[60], x2[43]);
        x3[27] = _mm256_add_epi32(x1[60], x2[43]);
        x3[52] = _mm256_sub_epi32(x1[59], x2[44]);
        x3[28] = _mm256_add_epi32(x1[59], x2[44]);
        x3[53] = _mm256_sub_epi32(x1[58], x2[45]);
        x3[29] = _mm256_add_epi32(x1[58], x2[45]);
        x3[54] = _mm256_sub_epi32(x1[57], x2[46]);
        x3[30] = _mm256_add_epi32(x1[57], x2[46]);
        x3[55] = _mm256_sub_epi32(x1[56], x2[47]);
        x3[31] = _mm256_add_epi32(x1[56], x2[47]);

        // stage 4
        //__m256i x4[44]; replace with x1
        x1[0] = _mm256_add_epi32(x3[0], x3[7]);
        x1[7] = _mm256_sub_epi32(x3[0], x3[7]);
        x1[1] = _mm256_add_epi32(x3[1], x3[6]);
        x1[6] = _mm256_sub_epi32(x3[1], x3[6]);
        x1[2] = _mm256_add_epi32(x3[2], x3[5]);
        x1[5] = _mm256_sub_epi32(x3[2], x3[5]);
        x1[3] = _mm256_add_epi32(x3[3], x3[4]);
        x1[4] = _mm256_sub_epi32(x3[3], x3[4]);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x3[10], x3[13], x1[8], x1[11], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x3[11], x3[12], x1[9], x1[10], __rounding, cos_bit);
        x1[12] = _mm256_add_epi32(x2[16], x3[19]);
        x1[19] = _mm256_sub_epi32(x2[16], x3[19]);
        x1[13] = _mm256_add_epi32(x2[17], x3[18]);
        x1[18] = _mm256_sub_epi32(x2[17], x3[18]);
        x1[14] = _mm256_add_epi32(x2[18], x3[17]);
        x1[17] = _mm256_sub_epi32(x2[18], x3[17]);
        x1[15] = _mm256_add_epi32(x2[19], x3[16]);
        x1[16] = _mm256_sub_epi32(x2[19], x3[16]);
        x1[20] = _mm256_sub_epi32(x2[31], x3[20]);
        x1[27] = _mm256_add_epi32(x2[31], x3[20]);
        x1[21] = _mm256_sub_epi32(x2[30], x3[21]);
        x1[26] = _mm256_add_epi32(x2[30], x3[21]);
        x1[22] = _mm256_sub_epi32(x2[29], x3[22]);
        x1[25] = _mm256_add_epi32(x2[29], x3[22]);
        x1[23] = _mm256_sub_epi32(x2[28], x3[23]);
        x1[24] = _mm256_add_epi32(x2[28], x3[23]);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x3[36], x3[28], x1[28], x1[43], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x3[37], x3[29], x1[29], x1[42], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x3[38], x3[30], x1[30], x1[41], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x3[39], x3[31], x1[31], x1[40], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x3[40], x3[55], x1[32], x1[39], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x3[41], x3[54], x1[33], x1[38], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x3[42], x3[53], x1[34], x1[37], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x3[43], x3[52], x1[35], x1[36], __rounding, cos_bit);

        // stage 5
        //__m256i x5[54]; replace with x2
        x2[0] = _mm256_add_epi32(x1[0], x1[3]);
        x2[3] = _mm256_sub_epi32(x1[0], x1[3]);
        x2[1] = _mm256_add_epi32(x1[1], x1[2]);
        x2[2] = _mm256_sub_epi32(x1[1], x1[2]);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[5], x1[6], x2[4], x2[5], __rounding, cos_bit);
        x2[6]  = _mm256_add_epi32(x3[8], x1[9]);
        x2[9]  = _mm256_sub_epi32(x3[8], x1[9]);
        x2[7]  = _mm256_add_epi32(x3[9], x1[8]);
        x2[8]  = _mm256_sub_epi32(x3[9], x1[8]);
        x2[10] = _mm256_sub_epi32(x3[15], x1[10]);
        x2[13] = _mm256_add_epi32(x3[15], x1[10]);
        x2[11] = _mm256_sub_epi32(x3[14], x1[11]);
        x2[12] = _mm256_add_epi32(x3[14], x1[11]);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x1[14], x1[25], x2[14], x2[21], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x1[15], x1[24], x2[15], x2[20], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x1[16], x1[23], x2[16], x2[19], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x1[17], x1[22], x2[17], x2[18], __rounding, cos_bit);
        x2[22] = _mm256_add_epi32(x3[32], x1[31]);
        x2[29] = _mm256_sub_epi32(x3[32], x1[31]);
        x2[23] = _mm256_add_epi32(x3[33], x1[30]);
        x2[28] = _mm256_sub_epi32(x3[33], x1[30]);
        x2[24] = _mm256_add_epi32(x3[34], x1[29]);
        x2[27] = _mm256_sub_epi32(x3[34], x1[29]);
        x2[25] = _mm256_add_epi32(x3[35], x1[28]);
        x2[26] = _mm256_sub_epi32(x3[35], x1[28]);
        x2[30] = _mm256_sub_epi32(x3[47], x1[32]);
        x2[37] = _mm256_add_epi32(x3[47], x1[32]);
        x2[31] = _mm256_sub_epi32(x3[46], x1[33]);
        x2[36] = _mm256_add_epi32(x3[46], x1[33]);
        x2[32] = _mm256_sub_epi32(x3[45], x1[34]);
        x2[35] = _mm256_add_epi32(x3[45], x1[34]);
        x2[33] = _mm256_sub_epi32(x3[44], x1[35]);
        x2[34] = _mm256_add_epi32(x3[44], x1[35]);
        x2[38] = _mm256_add_epi32(x3[48], x1[39]);
        x2[45] = _mm256_sub_epi32(x3[48], x1[39]);
        x2[39] = _mm256_add_epi32(x3[49], x1[38]);
        x2[44] = _mm256_sub_epi32(x3[49], x1[38]);
        x2[40] = _mm256_add_epi32(x3[50], x1[37]);
        x2[43] = _mm256_sub_epi32(x3[50], x1[37]);
        x2[41] = _mm256_add_epi32(x3[51], x1[36]);
        x2[42] = _mm256_sub_epi32(x3[51], x1[36]);
        x2[46] = _mm256_sub_epi32(x3[24], x1[40]);
        x2[53] = _mm256_add_epi32(x3[24], x1[40]);
        x2[47] = _mm256_sub_epi32(x3[25], x1[41]);
        x2[52] = _mm256_add_epi32(x3[25], x1[41]);
        x2[48] = _mm256_sub_epi32(x3[26], x1[42]);
        x2[51] = _mm256_add_epi32(x3[26], x1[42]);
        x2[49] = _mm256_sub_epi32(x3[27], x1[43]);
        x2[50] = _mm256_add_epi32(x3[27], x1[43]);

        // stage 6
        //__m256i x6[40]; replace with x3
        btf_32_type0_avx2_new(
            cospi_p32, cospi_p32, x2[0], x2[1], out[0 * stride], out[32 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p48, cospi_p16, x2[2], x2[3], out[16 * stride], out[48 * stride], __rounding, cos_bit);
        x3[0] = _mm256_add_epi32(x1[4], x2[4]);
        x3[1] = _mm256_sub_epi32(x1[4], x2[4]);
        x3[2] = _mm256_sub_epi32(x1[7], x2[5]);
        x3[3] = _mm256_add_epi32(x1[7], x2[5]);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x2[7], x2[12], x3[4], x3[7], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x2[8], x2[11], x3[5], x3[6], __rounding, cos_bit);
        x3[8]  = _mm256_add_epi32(x1[12], x2[15]);
        x3[11] = _mm256_sub_epi32(x1[12], x2[15]);
        x3[9]  = _mm256_add_epi32(x1[13], x2[14]);
        x3[10] = _mm256_sub_epi32(x1[13], x2[14]);
        x3[12] = _mm256_sub_epi32(x1[19], x2[16]);
        x3[15] = _mm256_add_epi32(x1[19], x2[16]);
        x3[13] = _mm256_sub_epi32(x1[18], x2[17]);
        x3[14] = _mm256_add_epi32(x1[18], x2[17]);
        x3[16] = _mm256_add_epi32(x1[20], x2[19]);
        x3[19] = _mm256_sub_epi32(x1[20], x2[19]);
        x3[17] = _mm256_add_epi32(x1[21], x2[18]);
        x3[18] = _mm256_sub_epi32(x1[21], x2[18]);
        x3[20] = _mm256_sub_epi32(x1[27], x2[20]);
        x3[23] = _mm256_add_epi32(x1[27], x2[20]);
        x3[21] = _mm256_sub_epi32(x1[26], x2[21]);
        x3[22] = _mm256_add_epi32(x1[26], x2[21]);
        btf_32_type0_avx2_new(cospi_m08, cospi_p56, x2[24], x2[51], x3[24], x3[39], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m08, cospi_p56, x2[25], x2[50], x3[25], x3[38], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m56, cospi_m08, x2[26], x2[49], x3[26], x3[37], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m56, cospi_m08, x2[27], x2[48], x3[27], x3[36], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m40, cospi_p24, x2[32], x2[43], x3[28], x3[35], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m40, cospi_p24, x2[33], x2[42], x3[29], x3[34], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m24, cospi_m40, x2[34], x2[41], x3[30], x3[33], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m24, cospi_m40, x2[35], x2[40], x3[31], x3[32], __rounding, cos_bit);

        // stage 7
        //__m256i x7[48]; replace with x1
        btf_32_type1_avx2_new(
            cospi_p56, cospi_p08, x3[0], x3[3], out[8 * stride], out[56 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p24, cospi_p40, x3[1], x3[2], out[40 * stride], out[24 * stride], __rounding, cos_bit);
        x1[0] = _mm256_add_epi32(x2[6], x3[4]);
        x1[1] = _mm256_sub_epi32(x2[6], x3[4]);
        x1[2] = _mm256_sub_epi32(x2[9], x3[5]);
        x1[3] = _mm256_add_epi32(x2[9], x3[5]);
        x1[4] = _mm256_add_epi32(x2[10], x3[6]);
        x1[5] = _mm256_sub_epi32(x2[10], x3[6]);
        x1[6] = _mm256_sub_epi32(x2[13], x3[7]);
        x1[7] = _mm256_add_epi32(x2[13], x3[7]);
        btf_32_type0_avx2_new(cospi_m08, cospi_p56, x3[9], x3[22], x1[8], x1[15], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m56, cospi_m08, x3[10], x3[21], x1[9], x1[14], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m40, cospi_p24, x3[13], x3[18], x1[10], x1[13], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m24, cospi_m40, x3[14], x3[17], x1[11], x1[12], __rounding, cos_bit);
        x1[16] = _mm256_add_epi32(x2[22], x3[25]);
        x1[17] = _mm256_sub_epi32(x2[22], x3[25]);
        x1[19] = _mm256_add_epi32(x2[23], x3[24]);
        x1[20] = _mm256_sub_epi32(x2[23], x3[24]);
        x1[18] = _mm256_sub_epi32(x2[29], x3[26]);
        x1[21] = _mm256_add_epi32(x2[29], x3[26]);
        x1[22] = _mm256_sub_epi32(x2[28], x3[27]);
        x1[23] = _mm256_add_epi32(x2[28], x3[27]);
        x1[24] = _mm256_add_epi32(x2[30], x3[29]);
        x1[25] = _mm256_sub_epi32(x2[30], x3[29]);
        x1[26] = _mm256_add_epi32(x2[31], x3[28]);
        x1[27] = _mm256_sub_epi32(x2[31], x3[28]);
        x1[28] = _mm256_sub_epi32(x2[37], x3[30]);
        x1[29] = _mm256_add_epi32(x2[37], x3[30]);
        x1[30] = _mm256_sub_epi32(x2[36], x3[31]);
        x1[31] = _mm256_add_epi32(x2[36], x3[31]);
        x1[32] = _mm256_add_epi32(x2[38], x3[33]);
        x1[33] = _mm256_sub_epi32(x2[38], x3[33]);
        x1[34] = _mm256_add_epi32(x2[39], x3[32]);
        x1[35] = _mm256_sub_epi32(x2[39], x3[32]);
        x1[36] = _mm256_sub_epi32(x2[45], x3[34]);
        x1[37] = _mm256_add_epi32(x2[45], x3[34]);
        x1[38] = _mm256_sub_epi32(x2[44], x3[35]);
        x1[39] = _mm256_add_epi32(x2[44], x3[35]);
        x1[40] = _mm256_add_epi32(x2[46], x3[37]);
        x1[41] = _mm256_sub_epi32(x2[46], x3[37]);
        x1[42] = _mm256_add_epi32(x2[47], x3[36]);
        x1[43] = _mm256_sub_epi32(x2[47], x3[36]);
        x1[44] = _mm256_sub_epi32(x2[53], x3[38]);
        x1[45] = _mm256_add_epi32(x2[53], x3[38]);
        x1[46] = _mm256_sub_epi32(x2[52], x3[39]);
        x1[47] = _mm256_add_epi32(x2[52], x3[39]);

        // stage 8
        //__m256i x8[32]; replace with x2
        btf_32_type1_avx2_new(
            cospi_p60, cospi_p04, x1[0], x1[7], out[4 * stride], out[60 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p28, cospi_p36, x1[1], x1[6], out[36 * stride], out[28 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p44, cospi_p20, x1[2], x1[5], out[20 * stride], out[44 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p12, cospi_p52, x1[3], x1[4], out[52 * stride], out[12 * stride], __rounding, cos_bit);
        x2[0]  = _mm256_add_epi32(x3[8], x1[8]);
        x2[1]  = _mm256_sub_epi32(x3[8], x1[8]);
        x2[2]  = _mm256_sub_epi32(x3[11], x1[9]);
        x2[3]  = _mm256_add_epi32(x3[11], x1[9]);
        x2[4]  = _mm256_add_epi32(x3[12], x1[10]);
        x2[5]  = _mm256_sub_epi32(x3[12], x1[10]);
        x2[6]  = _mm256_sub_epi32(x3[15], x1[11]);
        x2[7]  = _mm256_add_epi32(x3[15], x1[11]);
        x2[8]  = _mm256_add_epi32(x3[16], x1[12]);
        x2[9]  = _mm256_sub_epi32(x3[16], x1[12]);
        x2[10] = _mm256_sub_epi32(x3[19], x1[13]);
        x2[11] = _mm256_add_epi32(x3[19], x1[13]);
        x2[12] = _mm256_add_epi32(x3[20], x1[14]);
        x2[13] = _mm256_sub_epi32(x3[20], x1[14]);
        x2[14] = _mm256_sub_epi32(x3[23], x1[15]);
        x2[15] = _mm256_add_epi32(x3[23], x1[15]);
        btf_32_type0_avx2_new(cospi_m04, cospi_p60, x1[19], x1[47], x2[16], x2[31], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m60, cospi_m04, x1[20], x1[46], x2[17], x2[30], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m36, cospi_p28, x1[22], x1[43], x2[18], x2[29], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m28, cospi_m36, x1[23], x1[42], x2[19], x2[28], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m20, cospi_p44, x1[26], x1[39], x2[20], x2[27], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m44, cospi_m20, x1[27], x1[38], x2[21], x2[26], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m52, cospi_p12, x1[30], x1[35], x2[22], x2[25], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m12, cospi_m52, x1[31], x1[34], x2[23], x2[24], __rounding, cos_bit);

        // stage 9
        //__m256i x9[32]; replace with x3
        btf_32_type1_avx2_new(
            cospi_p62, cospi_p02, x2[0], x2[15], out[2 * stride], out[62 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p30, cospi_p34, x2[1], x2[14], out[34 * stride], out[30 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p46, cospi_p18, x2[2], x2[13], out[18 * stride], out[46 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p14, cospi_p50, x2[3], x2[12], out[50 * stride], out[14 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p54, cospi_p10, x2[4], x2[11], out[10 * stride], out[54 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p22, cospi_p42, x2[5], x2[10], out[42 * stride], out[22 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p38, cospi_p26, x2[6], x2[9], out[26 * stride], out[38 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p06, cospi_p58, x2[7], x2[8], out[58 * stride], out[6 * stride], __rounding, cos_bit);
        x3[0]  = _mm256_add_epi32(x1[16], x2[16]);
        x3[1]  = _mm256_sub_epi32(x1[16], x2[16]);
        x3[2]  = _mm256_sub_epi32(x1[17], x2[17]);
        x3[3]  = _mm256_add_epi32(x1[17], x2[17]);
        x3[4]  = _mm256_add_epi32(x1[18], x2[18]);
        x3[5]  = _mm256_sub_epi32(x1[18], x2[18]);
        x3[6]  = _mm256_sub_epi32(x1[21], x2[19]);
        x3[7]  = _mm256_add_epi32(x1[21], x2[19]);
        x3[8]  = _mm256_add_epi32(x1[24], x2[20]);
        x3[9]  = _mm256_sub_epi32(x1[24], x2[20]);
        x3[10] = _mm256_sub_epi32(x1[25], x2[21]);
        x3[11] = _mm256_add_epi32(x1[25], x2[21]);
        x3[12] = _mm256_add_epi32(x1[28], x2[22]);
        x3[13] = _mm256_sub_epi32(x1[28], x2[22]);
        x3[14] = _mm256_sub_epi32(x1[29], x2[23]);
        x3[15] = _mm256_add_epi32(x1[29], x2[23]);
        x3[16] = _mm256_add_epi32(x1[32], x2[24]);
        x3[17] = _mm256_sub_epi32(x1[32], x2[24]);
        x3[18] = _mm256_sub_epi32(x1[33], x2[25]);
        x3[19] = _mm256_add_epi32(x1[33], x2[25]);
        x3[20] = _mm256_add_epi32(x1[36], x2[26]);
        x3[21] = _mm256_sub_epi32(x1[36], x2[26]);
        x3[22] = _mm256_sub_epi32(x1[37], x2[27]);
        x3[23] = _mm256_add_epi32(x1[37], x2[27]);
        x3[24] = _mm256_add_epi32(x1[40], x2[28]);
        x3[25] = _mm256_sub_epi32(x1[40], x2[28]);
        x3[26] = _mm256_sub_epi32(x1[41], x2[29]);
        x3[27] = _mm256_add_epi32(x1[41], x2[29]);
        x3[28] = _mm256_add_epi32(x1[44], x2[30]);
        x3[29] = _mm256_sub_epi32(x1[44], x2[30]);
        x3[30] = _mm256_sub_epi32(x1[45], x2[31]);
        x3[31] = _mm256_add_epi32(x1[45], x2[31]);

        // stage 10
        btf_32_type1_avx2_new(
            cospi_p63, cospi_p01, x3[0], x3[31], out[1 * stride], out[63 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p31, cospi_p33, x3[1], x3[30], out[33 * stride], out[31 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p47, cospi_p17, x3[2], x3[29], out[17 * stride], out[47 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p15, cospi_p49, x3[3], x3[28], out[49 * stride], out[15 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p55, cospi_p09, x3[4], x3[27], out[9 * stride], out[55 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p23, cospi_p41, x3[5], x3[26], out[41 * stride], out[23 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p39, cospi_p25, x3[6], x3[25], out[25 * stride], out[39 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p07, cospi_p57, x3[7], x3[24], out[57 * stride], out[7 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p59, cospi_p05, x3[8], x3[23], out[5 * stride], out[59 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p27, cospi_p37, x3[9], x3[22], out[37 * stride], out[27 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p43, cospi_p21, x3[10], x3[21], out[21 * stride], out[43 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p11, cospi_p53, x3[11], x3[20], out[53 * stride], out[11 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p51, cospi_p13, x3[12], x3[19], out[13 * stride], out[51 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p19, cospi_p45, x3[13], x3[18], out[45 * stride], out[19 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p35, cospi_p29, x3[14], x3[17], out[29 * stride], out[35 * stride], __rounding, cos_bit);
        btf_32_type1_avx2_new(
            cospi_p03, cospi_p61, x3[15], x3[16], out[61 * stride], out[3 * stride], __rounding, cos_bit);
    }
}

typedef void (*TxfmFuncAVX2)(const __m256i* input, __m256i* output, const int8_t cos_bit, const int8_t* stage_range);

static INLINE void fdct32x32_avx2(const __m256i* input, __m256i* output, const int8_t cos_bit,
                                  const int8_t* stage_range) {
    const int32_t txfm_size   = 32;
    const int32_t num_per_256 = 8;
    int32_t       col_num     = txfm_size / num_per_256;
    (void)stage_range;
    av1_fdct32_new_avx2(input, output, cos_bit, txfm_size, col_num);
}

static INLINE void fdct64x64_avx2(const __m256i* input, __m256i* output, const int8_t cos_bit) {
    const int32_t txfm_size   = 64;
    const int32_t num_per_256 = 8;
    int32_t       col_num     = txfm_size / num_per_256;
    av1_fdct64_new_avx2(input, output, cos_bit, txfm_size, col_num);
}

static INLINE void fidtx4x8_row_avx2(__m256i* input, __m256i* output, int32_t bit, int32_t col_num) {
    (void)bit;
    __m256i in[4];
    __m256i out[4];
    __m256i fact   = _mm256_set1_epi32(new_sqrt2);
    __m256i offset = _mm256_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m256i a_low;
    __m256i v[4];

    in[0] = yy_unpacklo_epi128(input[0], input[2]);
    in[1] = yy_unpackhi_epi128(input[0], input[2]);
    in[2] = yy_unpacklo_epi128(input[1], input[3]);
    in[3] = yy_unpackhi_epi128(input[1], input[3]);

    for (int32_t i = 0; i < 4; i++) {
        a_low  = _mm256_mullo_epi32(in[i * col_num], fact);
        a_low  = _mm256_add_epi32(a_low, offset);
        out[i] = _mm256_srai_epi32(a_low, new_sqrt2_bits);
    }

    // Transpose for 4x4
    v[0] = _mm256_unpacklo_epi32(out[0], out[1]);
    v[1] = _mm256_unpackhi_epi32(out[0], out[1]);
    v[2] = _mm256_unpacklo_epi32(out[2], out[3]);
    v[3] = _mm256_unpackhi_epi32(out[2], out[3]);

    out[0] = _mm256_unpacklo_epi64(v[0], v[2]);
    out[1] = _mm256_unpackhi_epi64(v[0], v[2]);
    out[2] = _mm256_unpacklo_epi64(v[1], v[3]);
    out[3] = _mm256_unpackhi_epi64(v[1], v[3]);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static INLINE void fidtx4x8_col_avx2(__m256i* in, __m256i* output, int32_t bit, int32_t col_num) {
    (void)bit;
    __m256i out[4];
    __m256i fact   = _mm256_set1_epi32(new_sqrt2);
    __m256i offset = _mm256_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m256i a_low;
    __m256i v[4];

    for (int32_t i = 0; i < 4; i++) {
        a_low  = _mm256_mullo_epi32(in[i * col_num], fact);
        a_low  = _mm256_add_epi32(a_low, offset);
        out[i] = _mm256_srai_epi32(a_low, new_sqrt2_bits);
    }

    // Transpose for 4x4
    v[0] = _mm256_unpacklo_epi32(out[0], out[1]);
    v[1] = _mm256_unpackhi_epi32(out[0], out[1]);
    v[2] = _mm256_unpacklo_epi32(out[2], out[3]);
    v[3] = _mm256_unpackhi_epi32(out[2], out[3]);

    out[0] = _mm256_unpacklo_epi64(v[0], v[2]);
    out[1] = _mm256_unpackhi_epi64(v[0], v[2]);
    out[2] = _mm256_unpacklo_epi64(v[1], v[3]);
    out[3] = _mm256_unpackhi_epi64(v[1], v[3]);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static INLINE void fidtx8x4_avx2(__m256i* in, __m256i* out, int32_t bit) {
    (void)bit;

    out[0] = _mm256_add_epi32(in[0], in[0]);
    out[1] = _mm256_add_epi32(in[1], in[1]);
    out[2] = _mm256_add_epi32(in[2], in[2]);
    out[3] = _mm256_add_epi32(in[3], in[3]);
}

static void av1_idtx32_new_avx2(const __m256i* input, __m256i* output, int8_t cos_bit, const int32_t col_num) {
    (void)cos_bit;
    for (int32_t i = 0; i < 32; i++) {
        output[i * col_num] = _mm256_slli_epi32(input[i * col_num], 2);
    }
}

static void fidtx32x32_avx2(const __m256i* input, __m256i* output, const int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;

    for (int32_t i = 0; i < 4; i++) {
        av1_idtx32_new_avx2(&input[i * 32], &output[i * 32], cos_bit, 1);
    }
}

static void fidtx32x8_avx2(const __m256i* in, __m256i* out, int8_t bit, int32_t col_num) {
    (void)bit;
    (void)col_num;
    out[4 * 0] = _mm256_slli_epi32(in[4 * 0], 1);
    out[4 * 1] = _mm256_slli_epi32(in[4 * 1], 1);
    out[4 * 2] = _mm256_slli_epi32(in[4 * 2], 1);
    out[4 * 3] = _mm256_slli_epi32(in[4 * 3], 1);
    out[4 * 4] = _mm256_slli_epi32(in[4 * 4], 1);
    out[4 * 5] = _mm256_slli_epi32(in[4 * 5], 1);
    out[4 * 6] = _mm256_slli_epi32(in[4 * 6], 1);
    out[4 * 7] = _mm256_slli_epi32(in[4 * 7], 1);
}

static void fidtx64x64_avx2(const __m256i* input, __m256i* output) {
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 4 * 5793; // 4 * new_sqrt2
    const int32_t col_num  = 8;
    const __m256i newsqrt  = _mm256_set1_epi32(sqrt);
    const __m256i rounding = _mm256_set1_epi32(1 << (bits - 1));

    __m256i temp;
    int32_t num_iters = 64 * col_num;
    for (int32_t i = 0; i < num_iters; i++) {
        temp      = _mm256_mullo_epi32(input[i], newsqrt);
        temp      = _mm256_add_epi32(temp, rounding);
        output[i] = _mm256_srai_epi32(temp, bits);
    }
}

static INLINE TxfmFuncAVX2 fwd_txfm_type_to_func_avx2(TxfmType txfmtype) {
    switch (txfmtype) {
    case TXFM_TYPE_DCT32:
        return fdct32x32_avx2;
        break;
    case TXFM_TYPE_IDENTITY32:
        return fidtx32x32_avx2;
        break;
    default:
        assert(0);
    }
    return NULL;
}

static INLINE void load_buffer_32x32_avx2(const int16_t* input, __m256i* output, int32_t stride) {
    __m128i temp[4];
    int32_t i;

    for (i = 0; i < 32; ++i) {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 0 * 8));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 1 * 8));
        temp[2] = _mm_loadu_si128((const __m128i*)(input + 2 * 8));
        temp[3] = _mm_loadu_si128((const __m128i*)(input + 3 * 8));

        output[0] = _mm256_cvtepi16_epi32(temp[0]);
        output[1] = _mm256_cvtepi16_epi32(temp[1]);
        output[2] = _mm256_cvtepi16_epi32(temp[2]);
        output[3] = _mm256_cvtepi16_epi32(temp[3]);
        input += stride;
        output += 4;
    }
}

static INLINE void load_buffer_32x16_avx2(const int16_t* input, __m256i* output, int32_t stride) {
    __m128i temp[4];
    int32_t i;

    for (i = 0; i < 16; ++i) {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 0 * 8));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 1 * 8));
        temp[2] = _mm_loadu_si128((const __m128i*)(input + 2 * 8));
        temp[3] = _mm_loadu_si128((const __m128i*)(input + 3 * 8));

        output[0] = _mm256_cvtepi16_epi32(temp[0]);
        output[1] = _mm256_cvtepi16_epi32(temp[1]);
        output[2] = _mm256_cvtepi16_epi32(temp[2]);
        output[3] = _mm256_cvtepi16_epi32(temp[3]);
        input += stride;
        output += 4;
    }
}

static INLINE void load_buffer_32x16_N2_avx2(const int16_t* input, __m256i* output, int32_t stride) {
    __m128i temp[4];
    int32_t i;

    for (i = 0; i < 16; ++i) {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 0 * 8));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 1 * 8));

        output[0] = _mm256_cvtepi16_epi32(temp[0]);
        output[1] = _mm256_cvtepi16_epi32(temp[1]);
        input += stride;
        output += 4;
    }
}

static INLINE void fwd_txfm2d_32x32_avx2(const int16_t* input, int32_t* output, const int32_t stride,
                                         const Txfm2dFlipCfg* cfg, int32_t* txfm_buf) {
    assert(cfg->tx_size < TX_SIZES);
    const int32_t      txfm_size       = tx_size_wide[cfg->tx_size];
    const int8_t*      shift           = cfg->shift;
    const int8_t*      stage_range_col = cfg->stage_range_col;
    const int8_t*      stage_range_row = cfg->stage_range_row;
    const int8_t       cos_bit_col     = cfg->cos_bit_col;
    const int8_t       cos_bit_row     = cfg->cos_bit_row;
    const TxfmFuncAVX2 txfm_func_col   = fwd_txfm_type_to_func_avx2(cfg->txfm_type_col);
    const TxfmFuncAVX2 txfm_func_row   = fwd_txfm_type_to_func_avx2(cfg->txfm_type_row);
    ASSERT(txfm_func_col);
    ASSERT(txfm_func_row);
    __m256i* buf_256         = (__m256i*)txfm_buf;
    __m256i* out_256         = (__m256i*)output;
    int32_t  num_per_256     = 8;
    int32_t  txfm2d_size_256 = txfm_size * txfm_size / num_per_256;

    load_buffer_32x32_avx2(input, buf_256, stride);
    av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256, -shift[0]);
    txfm_func_col(out_256, buf_256, cos_bit_col, stage_range_col);
    av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256, -shift[1]);
    transpose_32_avx2(txfm_size, out_256, buf_256);
    txfm_func_row(buf_256, out_256, cos_bit_row, stage_range_row);
    av1_round_shift_array_32_avx2(out_256, buf_256, txfm2d_size_256, -shift[2]);
    transpose_32_avx2(txfm_size, buf_256, out_256);
}

void svt_av1_fwd_txfm2d_32x32_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    DECLARE_ALIGNED(32, int32_t, txfm_buf[1024]);
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(tx_type, TX_32X32, &cfg);
    (void)bd;
    fwd_txfm2d_32x32_avx2(input, output, stride, &cfg, txfm_buf);
}

static INLINE void load_buffer_64x64_avx2(const int16_t* input, int32_t stride, __m256i* output) {
    __m128i x0, x1, x2, x3, x4, x5, x6, x7;
    __m256i v0, v1, v2, v3, v4, v5, v6, v7;
    int32_t i;

    for (i = 0; i < 64; ++i) {
        x0 = _mm_loadu_si128((const __m128i*)(input + 0 * 8));
        x1 = _mm_loadu_si128((const __m128i*)(input + 1 * 8));
        x2 = _mm_loadu_si128((const __m128i*)(input + 2 * 8));
        x3 = _mm_loadu_si128((const __m128i*)(input + 3 * 8));
        x4 = _mm_loadu_si128((const __m128i*)(input + 4 * 8));
        x5 = _mm_loadu_si128((const __m128i*)(input + 5 * 8));
        x6 = _mm_loadu_si128((const __m128i*)(input + 6 * 8));
        x7 = _mm_loadu_si128((const __m128i*)(input + 7 * 8));

        v0 = _mm256_cvtepi16_epi32(x0);
        v1 = _mm256_cvtepi16_epi32(x1);
        v2 = _mm256_cvtepi16_epi32(x2);
        v3 = _mm256_cvtepi16_epi32(x3);
        v4 = _mm256_cvtepi16_epi32(x4);
        v5 = _mm256_cvtepi16_epi32(x5);
        v6 = _mm256_cvtepi16_epi32(x6);
        v7 = _mm256_cvtepi16_epi32(x7);

        _mm256_storeu_si256(output + 0, v0);
        _mm256_storeu_si256(output + 1, v1);
        _mm256_storeu_si256(output + 2, v2);
        _mm256_storeu_si256(output + 3, v3);
        _mm256_storeu_si256(output + 4, v4);
        _mm256_storeu_si256(output + 5, v5);
        _mm256_storeu_si256(output + 6, v6);
        _mm256_storeu_si256(output + 7, v7);

        input += stride;
        output += 8;
    }
}

void svt_av1_fwd_txfm2d_64x64_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    __m256i       in[512];
    __m256i*      out     = (__m256i*)output;
    const int32_t txw_idx = tx_size_wide_log2[TX_64X64] - tx_size_wide_log2[0];
    const int32_t txh_idx = tx_size_high_log2[TX_64X64] - tx_size_high_log2[0];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_64X64];

    switch (tx_type) {
    case IDTX:
        load_buffer_64x64_avx2(input, stride, out);
        fidtx64x64_avx2(out, in);
        av1_round_shift_array_32_avx2(in, out, 512, -shift[1]);
        transpose_8nx8n(out, in, 64, 64);

        /*row wise transform*/
        fidtx64x64_avx2(in, out);
        av1_round_shift_array_32_avx2(out, in, 512, -shift[2]);
        transpose_8nx8n(in, out, 64, 64);
        break;
    case DCT_DCT:
        load_buffer_64x64_avx2(input, stride, out);
        fdct64x64_avx2(out, in, fwd_cos_bit_col[txw_idx][txh_idx]);
        av1_round_shift_array_32_avx2(in, out, 512, -shift[1]);
        transpose_8nx8n(out, in, 64, 64);

        /*row wise transform*/
        fdct64x64_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx]);
        av1_round_shift_array_32_avx2(out, in, 512, -shift[2]);
        transpose_8nx8n(in, out, 64, 64);
        break;
    default:
        assert(0);
    }
}

static INLINE void load_buffer_32_avx2(const int16_t* input, __m256i* in, int32_t stride, int32_t flipud,
                                       int32_t fliplr, int32_t shift) {
    __m128i temp[4];
    if (!flipud) {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 0 * stride));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 1 * stride));
        temp[2] = _mm_loadu_si128((const __m128i*)(input + 2 * stride));
        temp[3] = _mm_loadu_si128((const __m128i*)(input + 3 * stride));
    } else {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 3 * stride));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 2 * stride));
        temp[2] = _mm_loadu_si128((const __m128i*)(input + 1 * stride));
        temp[3] = _mm_loadu_si128((const __m128i*)(input + 0 * stride));
    }

    if (fliplr) {
        temp[0] = mm_reverse_epi16(temp[0]);
        temp[1] = mm_reverse_epi16(temp[1]);
        temp[2] = mm_reverse_epi16(temp[2]);
        temp[3] = mm_reverse_epi16(temp[3]);
    }

    in[0] = _mm256_cvtepi16_epi32(temp[0]);
    in[1] = _mm256_cvtepi16_epi32(temp[1]);
    in[2] = _mm256_cvtepi16_epi32(temp[2]);
    in[3] = _mm256_cvtepi16_epi32(temp[3]);

    in[0] = _mm256_slli_epi32(in[0], shift);
    in[1] = _mm256_slli_epi32(in[1], shift);
    in[2] = _mm256_slli_epi32(in[2], shift);
    in[3] = _mm256_slli_epi32(in[3], shift);
}

static INLINE void load_buffer_16_avx2(const int16_t* input, __m256i* in, int32_t stride, int32_t flipud,
                                       int32_t fliplr, int32_t shift) {
    __m128i temp[2];
    if (!flipud) {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 0 * stride));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 1 * stride));
    } else {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 1 * stride));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 0 * stride));
    }

    if (fliplr) {
        temp[0] = mm_reverse_epi16(temp[0]);
        temp[1] = mm_reverse_epi16(temp[1]);
    }

    in[0] = _mm256_cvtepi16_epi32(temp[0]);
    in[1] = _mm256_cvtepi16_epi32(temp[1]);

    in[0] = _mm256_slli_epi32(in[0], shift);
    in[1] = _mm256_slli_epi32(in[1], shift);
}

static INLINE void load_buffer_32x8n(const int16_t* input, __m256i* out, int32_t stride, int32_t flipud, int32_t fliplr,
                                     int32_t shift, const int32_t height) {
    for (int32_t col = 0; col < height; col++) {
        const int16_t* in     = input + col * stride;
        __m256i*       output = out + col * 4;
        load_buffer_32_avx2(in, output, 8, flipud, fliplr, shift);
    }
}

static AOM_FORCE_INLINE void load_buffer_16x8n(const int16_t* input, __m256i* out, int32_t stride, int32_t flipud,
                                               int32_t fliplr, int32_t shift, const int32_t height) {
    for (int32_t col = 0; col < height; col++) {
        const int16_t* in     = input + col * stride;
        __m256i*       output = out + col * 4;
        load_buffer_16_avx2(in, output, 8, flipud, fliplr, shift);
    }
}

static INLINE void load_buffer_8x16(const int16_t* input, __m256i* out, int32_t stride, int32_t flipud, int32_t fliplr,
                                    int32_t shift) {
    const int16_t* top_l = input;
    const int16_t* bot_l = input + 8 * stride;

    const int16_t* tmp;

    if (flipud) {
        tmp   = top_l;
        top_l = bot_l;
        bot_l = tmp;
    }

    load_buffer_8x8(top_l, out, stride, flipud, fliplr, shift);
    load_buffer_8x8(bot_l, out + 8, stride, flipud, fliplr, shift);
}

static INLINE void col_txfm_8x4_rounding(__m256i* in, int32_t shift) {
    const __m256i rounding = _mm256_set1_epi32(1 << (shift - 1));

    in[0] = _mm256_add_epi32(in[0], rounding);
    in[1] = _mm256_add_epi32(in[1], rounding);
    in[2] = _mm256_add_epi32(in[2], rounding);
    in[3] = _mm256_add_epi32(in[3], rounding);

    in[0] = _mm256_srai_epi32(in[0], shift);
    in[1] = _mm256_srai_epi32(in[1], shift);
    in[2] = _mm256_srai_epi32(in[2], shift);
    in[3] = _mm256_srai_epi32(in[3], shift);
}

static INLINE void col_txfm_8x16_rounding(__m256i* in, int32_t shift) {
    col_txfm_8x8_rounding(&in[0], shift);
    col_txfm_8x8_rounding(&in[8], shift);
}

static INLINE void write_buffer_16x8_avx2(const __m256i* res, int32_t* output, const int32_t stride) {
    _mm256_storeu_si256((__m256i*)(output), res[0]);
    _mm256_storeu_si256((__m256i*)(output + stride), res[1]);
    _mm256_storeu_si256((__m256i*)(output + (stride * 2)), res[2]);
    _mm256_storeu_si256((__m256i*)(output + (stride * 3)), res[3]);
    _mm256_storeu_si256((__m256i*)(output + (stride * 4)), res[4]);
    _mm256_storeu_si256((__m256i*)(output + (stride * 5)), res[5]);
    _mm256_storeu_si256((__m256i*)(output + (stride * 6)), res[6]);
    _mm256_storeu_si256((__m256i*)(output + (stride * 7)), res[7]);
}

void svt_av1_fwd_txfm2d_32x64_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m256i       in[256];
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_32X64];
    const int32_t txw_idx       = get_txw_idx(TX_32X64);
    const int32_t txh_idx       = get_txh_idx(TX_32X64);
    const int32_t txfm_size_col = tx_size_wide[TX_32X64];
    const int32_t txfm_size_row = tx_size_high[TX_32X64];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    // column transform
    load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
    av1_fdct64_new_avx2(in, in, bitcol, txfm_size_col, num_col);

    for (int32_t i = 0; i < num_row; i++) {
        col_txfm_16x16_rounding((in + i * txfm_size_col), -shift[1]);
    }
    transpose_8nx8n(in, outcoef256, txfm_size_col, txfm_size_row);

    // row transform
    av1_fdct32_new_avx2(outcoef256, in, bitrow, txfm_size_row, num_row);
    transpose_8nx8n(in, outcoef256, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_32_avx2(outcoef256, outcoef256, 256, -shift[2], new_sqrt2);
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x32_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m256i       in[256];
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X32];
    const int32_t txw_idx       = get_txw_idx(TX_64X32);
    const int32_t txh_idx       = get_txh_idx(TX_64X32);
    const int32_t txfm_size_col = tx_size_wide[TX_64X32];
    const int32_t txfm_size_row = tx_size_high[TX_64X32];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    // column transform
    for (int32_t i = 0; i < 32; i++) {
        load_buffer_32_avx2(input + 0 + i * stride, in + 0 + i * 8, 8, 0, 0, shift[0]);
        load_buffer_32_avx2(input + 32 + i * stride, in + 4 + i * 8, 8, 0, 0, shift[0]);
    }

    av1_fdct32_new_avx2(in, in, bitcol, txfm_size_col, num_col);

    for (int32_t i = 0; i < num_col; i++) {
        col_txfm_16x16_rounding((in + i * txfm_size_row), -shift[1]);
    }
    transpose_8nx8n(in, outcoef256, txfm_size_col, txfm_size_row);

    // row transform
    av1_fdct64_new_avx2(outcoef256, in, bitrow, txfm_size_row, num_row);
    transpose_8nx8n(in, outcoef256, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_32_avx2(outcoef256, outcoef256, 256, -shift[2], new_sqrt2);
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x64_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[128];
    __m256i*      outcoeff256   = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X64];
    const int32_t txw_idx       = get_txw_idx(TX_16X64);
    const int32_t txh_idx       = get_txh_idx(TX_16X64);
    const int32_t txfm_size_col = tx_size_wide[TX_16X64];
    const int32_t txfm_size_row = tx_size_high[TX_16X64];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    int32_t       ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    const int32_t num_row = txfm_size_row >> 3;
    const int32_t num_col = txfm_size_col >> 3;
    // col tranform
    for (int32_t i = 0; i < txfm_size_row; i += num_col) {
        load_buffer_16_avx2(input + (i + 0) * stride, in + (i + 0) * num_col, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_avx2(input + (i + 1) * stride, in + (i + 1) * num_col, 8, ud_flip, lr_flip, shift[0]);
    }

    av1_fdct64_new_avx2(in, outcoeff256, bitcol, txfm_size_col, num_col);

    col_txfm_16x16_rounding(outcoeff256, -shift[1]);
    col_txfm_16x16_rounding(outcoeff256 + 32, -shift[1]);
    col_txfm_16x16_rounding(outcoeff256 + 64, -shift[1]);
    col_txfm_16x16_rounding(outcoeff256 + 96, -shift[1]);
    transpose_8nx8n(outcoeff256, in, txfm_size_col, txfm_size_row);
    // row tranform
    fdct16x16_avx2(in, in, bitrow, num_row);
    transpose_8nx8n(in, outcoeff256, txfm_size_row, txfm_size_col);
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x16_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[128];
    __m256i*      outcoeff256   = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X16];
    const int32_t txw_idx       = get_txw_idx(TX_64X16);
    const int32_t txh_idx       = get_txh_idx(TX_64X16);
    const int32_t txfm_size_col = tx_size_wide[TX_64X16];
    const int32_t txfm_size_row = tx_size_high[TX_64X16];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    int32_t       ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    const int32_t num_row = txfm_size_row >> 3;
    const int32_t num_col = txfm_size_col >> 3;
    // col tranform
    for (int32_t i = 0; i < txfm_size_row; i++) {
        load_buffer_16_avx2(input + 0 + i * stride, in + 0 + i * 8, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_avx2(input + 16 + i * stride, in + 2 + i * 8, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_avx2(input + 32 + i * stride, in + 4 + i * 8, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_avx2(input + 48 + i * stride, in + 6 + i * 8, 8, ud_flip, lr_flip, shift[0]);
    }

    fdct16x16_avx2(in, outcoeff256, bitcol, num_col);
    col_txfm_16x16_rounding(outcoeff256, -shift[1]);
    col_txfm_16x16_rounding(outcoeff256 + 32, -shift[1]);
    col_txfm_16x16_rounding(outcoeff256 + 64, -shift[1]);
    col_txfm_16x16_rounding(outcoeff256 + 96, -shift[1]);
    transpose_8nx8n(outcoeff256, in, txfm_size_col, txfm_size_row);
    // row tranform
    av1_fdct64_new_avx2(in, in, bitrow, txfm_size_row, num_row);
    transpose_8nx8n(in, outcoeff256, txfm_size_row, txfm_size_col);
    (void)bd;
}

static const FwdTransform1dAvx2 col_fwdtxfm_8x32_arr[TX_TYPES] = {
    av1_fdct32_new_line_wraper_avx2, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    av1_idtx32_new_avx2, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST
};

static const FwdTransform1dAvx2 row_fwdtxfm_8x32_arr[TX_TYPES] = {
    fdct16x16_avx2, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    fidtx16x16_avx2, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST
};

static const FwdTransform1dAvx2 row_fwdtxfm_32x8_arr[TX_TYPES] = {
    fdct8x8_avx2, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST-ADST
    fidtx32x8_avx2, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL, // H_FLIPADST
};

static const FwdTransform1dAvx2 col_fwdtxfm_8x16_arr[TX_TYPES] = {
    fdct16x16_avx2, // DCT_DCT
    fadst16x16_avx2, // ADST_DCT
    fdct16x16_avx2, // DCT_ADST
    fadst16x16_avx2, // ADST_ADST
    fadst16x16_avx2, // FLIPADST_DCT
    fdct16x16_avx2, // DCT_FLIPADST
    fadst16x16_avx2, // FLIPADST_FLIPADST
    fadst16x16_avx2, // ADST_FLIPADST
    fadst16x16_avx2, // FLIPADST_ADST
    fidtx16x16_avx2, // IDTX
    fdct16x16_avx2, // V_DCT
    fidtx16x16_avx2, // H_DCT
    fadst16x16_avx2, // V_ADST
    fidtx16x16_avx2, // H_ADST
    fadst16x16_avx2, // V_FLIPADST
    fidtx16x16_avx2 // H_FLIPADST
};

static const FwdTransform1dAvx2 row_fwdtxfm_8x8_arr[TX_TYPES] = {
    fdct8x8_avx2, // DCT_DCT
    fdct8x8_avx2, // ADST_DCT
    fadst8x8_avx2, // DCT_ADST
    fadst8x8_avx2, // ADST_ADST
    fdct8x8_avx2, // FLIPADST_DCT
    fadst8x8_avx2, // DCT_FLIPADST
    fadst8x8_avx2, // FLIPADST_FLIPADST
    fadst8x8_avx2, // ADST_FLIPADST
    fadst8x8_avx2, // FLIPADST_ADST
    fidtx8x8_avx2, // IDTX
    fidtx8x8_avx2, // V_DCT
    fdct8x8_avx2, // H_DCT
    fidtx8x8_avx2, // V_ADST
    fadst8x8_avx2, // H_ADST
    fidtx8x8_avx2, // V_FLIPADST
    fadst8x8_avx2 // H_FLIPADST
};

static const FwdTransform1dAvx2 col_fwdtxfm_8x8_arr[TX_TYPES] = {
    fdct8x8_avx2, // DCT_DCT
    fadst8x8_avx2, // ADST_DCT
    fdct8x8_avx2, // DCT_ADST
    fadst8x8_avx2, // ADST_ADST
    fadst8x8_avx2, // FLIPADST_DCT
    fdct8x8_avx2, // DCT_FLIPADST
    fadst8x8_avx2, // FLIPADST_FLIPADST
    fadst8x8_avx2, // ADST_FLIPADST
    fadst8x8_avx2, // FLIPADST_ADST
    fidtx8x8_avx2, // IDTX
    fdct8x8_avx2, // V_DCT
    fidtx8x8_avx2, // H_DCT
    fadst8x8_avx2, // V_ADST
    fidtx8x8_avx2, // H_ADST
    fadst8x8_avx2, // V_FLIPADST
    fidtx8x8_avx2 // H_FLIPADST
};

static const FwdTransform1dAvx2 row_fwdtxfm_8x16_arr[TX_TYPES] = {
    fdct16x16_avx2, // DCT_DCT
    fdct16x16_avx2, // ADST_DCT
    fadst16x16_avx2, // DCT_ADST
    fadst16x16_avx2, // ADST_ADST
    fdct16x16_avx2, // FLIPADST_DCT
    fadst16x16_avx2, // DCT_FLIPADST
    fadst16x16_avx2, // FLIPADST_FLIPADST
    fadst16x16_avx2, // ADST_FLIPADST
    fadst16x16_avx2, // FLIPADST_ADST
    fidtx16x16_avx2, // IDTX
    fidtx16x16_avx2, // V_DCT
    fdct16x16_avx2, // H_DCT
    fidtx16x16_avx2, // V_ADST
    fadst16x16_avx2, // H_ADST
    fidtx16x16_avx2, // V_FLIPADST
    fadst16x16_avx2 // H_FLIPADST
};

/* call this function only for DCT_DCT, IDTX */
void svt_av1_fwd_txfm2d_16x32_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i                  in[64];
    __m256i*                 outcoef256    = (__m256i*)output;
    const int8_t*            shift         = fwd_txfm_shift_ls[TX_16X32];
    const int32_t            txw_idx       = get_txw_idx(TX_16X32);
    const int32_t            txh_idx       = get_txh_idx(TX_16X32);
    const FwdTransform1dAvx2 col_txfm      = col_fwdtxfm_8x32_arr[tx_type];
    const FwdTransform1dAvx2 row_txfm      = row_fwdtxfm_8x32_arr[tx_type];
    int8_t                   bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t                   bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t            txfm_size_col = tx_size_wide[TX_16X32];
    const int32_t            txfm_size_row = tx_size_high[TX_16X32];
    const int32_t            num_row       = txfm_size_row >> 3;
    const int32_t            num_col       = txfm_size_col >> 3;

    // column transform
    load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
    load_buffer_16x16(input + 16 * stride, in + 32, stride, 0, 0, shift[0]);

    for (int32_t i = 0; i < num_col; i++) {
        col_txfm((in + i), (in + i), bitcol, num_col);
    }
    col_txfm_16x16_rounding(&in[0], -shift[1]);
    col_txfm_16x16_rounding(&in[32], -shift[1]);
    transpose_8nx8n(in, outcoef256, txfm_size_col, txfm_size_row);

    // row transform
    row_txfm(outcoef256, in, bitrow, num_row);
    transpose_8nx8n(in, outcoef256, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_32_avx2(outcoef256, outcoef256, 64, -shift[2], new_sqrt2);
    (void)bd;
}

/* call this function only for IDTX */
void svt_av1_fwd_txfm2d_32x16_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i                  in[64];
    __m256i*                 outcoef256    = (__m256i*)output;
    const int8_t*            shift         = fwd_txfm_shift_ls[TX_32X16];
    const int32_t            txw_idx       = get_txw_idx(TX_32X16);
    const int32_t            txh_idx       = get_txh_idx(TX_32X16);
    const FwdTransform1dAvx2 col_txfm      = row_fwdtxfm_8x32_arr[tx_type];
    const FwdTransform1dAvx2 row_txfm      = col_fwdtxfm_8x32_arr[tx_type];
    int8_t                   bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t                   bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t            txfm_size_col = tx_size_wide[TX_32X16];
    const int32_t            txfm_size_row = tx_size_high[TX_32X16];
    const int32_t            num_row       = txfm_size_row >> 3;
    const int32_t            num_col       = txfm_size_col >> 3;

    // column transform
    load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
    col_txfm(in, in, bitcol, num_col);
    col_txfm_16x16_rounding(&in[0], -shift[1]);
    col_txfm_16x16_rounding(&in[32], -shift[1]);
    transpose_8nx8n(in, outcoef256, txfm_size_col, txfm_size_row);

    // row transform
    for (int32_t i = 0; i < num_row; i++) {
        row_txfm((outcoef256 + i), (in + i), bitrow, num_row);
    }
    transpose_8nx8n(in, outcoef256, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_32_avx2(outcoef256, outcoef256, 64, -shift[2], new_sqrt2);
    (void)bd;
}

/* call this function only for DCT_DCT, IDTX */
void svt_av1_fwd_txfm2d_8x32_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i                  in[32];
    __m256i*                 outcoef256 = (__m256i*)output;
    const int8_t*            shift      = fwd_txfm_shift_ls[TX_8X32];
    const int32_t            txw_idx    = get_txw_idx(TX_8X32);
    const int32_t            txh_idx    = get_txh_idx(TX_8X32);
    const FwdTransform1dAvx2 col_txfm   = col_fwdtxfm_8x32_arr[tx_type];
    const FwdTransform1dAvx2 row_txfm   = row_fwdtxfm_32x8_arr[tx_type];
    int8_t                   bitcol     = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t                   bitrow     = fwd_cos_bit_row[txw_idx][txh_idx];

    const int32_t txfm_size_col = tx_size_wide[TX_8X32];
    const int32_t txfm_size_row = tx_size_high[TX_8X32];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    // column transform
    load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
    load_buffer_8x16(input + (txfm_size_row >> 1) * stride, in + 16, stride, 0, 0, shift[0]);

    col_txfm(in, in, bitcol, num_col);
    col_txfm_16x16_rounding(in, -shift[1]);
    transpose_8nx8n(in, outcoef256, txfm_size_col, txfm_size_row);

    // row transform
    for (int32_t i = 0; i < num_row; i++) {
        row_txfm((outcoef256 + i), (in + i), bitrow, num_row);
    }
    transpose_8nx8n(in, outcoef256, txfm_size_row, txfm_size_col);
    (void)bd;
}

/* call this function only for DCT_DCT, IDTX */
void svt_av1_fwd_txfm2d_32x8_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i                  in[32];
    __m256i*                 outcoef256 = (__m256i*)output;
    const int8_t*            shift      = fwd_txfm_shift_ls[TX_32X8];
    const int32_t            txw_idx    = get_txw_idx(TX_32X8);
    const int32_t            txh_idx    = get_txh_idx(TX_32X8);
    const FwdTransform1dAvx2 col_txfm   = row_fwdtxfm_32x8_arr[tx_type];
    const FwdTransform1dAvx2 row_txfm   = col_fwdtxfm_8x32_arr[tx_type];
    int8_t                   bitcol     = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t                   bitrow     = fwd_cos_bit_row[txw_idx][txh_idx];

    const int32_t txfm_size_col = tx_size_wide[TX_32X8];
    const int32_t txfm_size_row = tx_size_high[TX_32X8];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    // column transform
    load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
    for (int32_t i = 0; i < num_col; i++) {
        col_txfm((in + i), (in + i), bitcol, num_col);
    }
    col_txfm_16x16_rounding(&in[0], -shift[1]);
    transpose_8nx8n(in, outcoef256, txfm_size_col, txfm_size_row);

    // row transform
    row_txfm(outcoef256, in, bitrow, num_row);

    transpose_8nx8n(in, outcoef256, txfm_size_row, txfm_size_col);
    (void)bd;
}

/* call this function for all 16 transform types */
void svt_av1_fwd_txfm2d_8x16_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i                  in[16], out[16];
    const int8_t*            shift    = fwd_txfm_shift_ls[TX_8X16];
    const int32_t            txw_idx  = get_txw_idx(TX_8X16);
    const int32_t            txh_idx  = get_txh_idx(TX_8X16);
    const FwdTransform1dAvx2 col_txfm = col_fwdtxfm_8x16_arr[tx_type];
    const FwdTransform1dAvx2 row_txfm = row_fwdtxfm_8x8_arr[tx_type];
    int8_t                   bitcol   = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t                   bitrow   = fwd_cos_bit_row[txw_idx][txh_idx];
    int32_t                  ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    const int32_t txfm_size_col = tx_size_wide[TX_8X16];
    const int32_t txfm_size_row = tx_size_high[TX_8X16];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    load_buffer_8x16(input, in, stride, ud_flip, lr_flip, shift[0]);
    // column transform
    col_txfm(in, in, bitcol, num_col);
    col_txfm_8x16_rounding(in, -shift[1]);
    transpose_8x8_avx2(in, out);
    transpose_8x8_avx2(in + 8, out + 8);

    // row transform
    for (int32_t i = 0; i < num_row; i++) {
        row_txfm(out + i * 8, out, bitrow, 1);
        transpose_8x8_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, in, 8, -shift[2], new_sqrt2);
        write_buffer_8x8(in, output + i * 64);
    }
    (void)bd;
}

/* call this function for all 16 transform types */
void svt_av1_fwd_txfm2d_16x8_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i                  in[16], out[16];
    const int8_t*            shift    = fwd_txfm_shift_ls[TX_16X8];
    const int32_t            txw_idx  = get_txw_idx(TX_16X8);
    const int32_t            txh_idx  = get_txh_idx(TX_16X8);
    const FwdTransform1dAvx2 col_txfm = col_fwdtxfm_8x8_arr[tx_type];
    const FwdTransform1dAvx2 row_txfm = row_fwdtxfm_8x16_arr[tx_type];
    int8_t                   bitcol   = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t                   bitrow   = fwd_cos_bit_row[txw_idx][txh_idx];
    int32_t                  ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    const int32_t txfm_size_col = tx_size_wide[TX_16X8];
    const int32_t txfm_size_row = tx_size_high[TX_16X8];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;
    assert(num_col > 0);
    // column transform
    for (int32_t i = 0; i < num_col; i++) {
        load_buffer_8x8(input + i * 8, in, stride, ud_flip, 0, shift[0]);
        col_txfm(in, in, bitcol, 1);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out + i * 8);
    }

    // row transform
    if (lr_flip) {
        for (int32_t i = 0; i < 16; i++) {
            in[16 - i - 1] = out[i];
        }
        row_txfm(in, out, bitrow, num_row);
    } else {
        row_txfm(out, out, bitrow, num_row);
    }

    for (int32_t i = 0; i < num_col; i++) {
        transpose_8x8_avx2(out + i * 8, in);
        av1_round_shift_rect_array_32_avx2(in, in, 8, -shift[2], new_sqrt2);
        write_buffer_16x8_avx2(in, output + i * 8, 16);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_4x8_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i in[4];
    __m256i outcoeff256[4];

    const int8_t* shift   = fwd_txfm_shift_ls[TX_4X8];
    const int32_t txw_idx = get_txw_idx(TX_4X8);
    const int32_t txh_idx = get_txh_idx(TX_4X8);
    int32_t       bitcol  = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow  = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_avx2(in, in, bitcol);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fdct4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case ADST_DCT:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fadst8x4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fdct4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case DCT_ADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_avx2(in, in, bitcol);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case ADST_ADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fadst8x4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case FLIPADST_DCT:
        load_buffer_4x8_avx2(input, in, stride, 1, 0, shift[0]);
        fadst8x4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fdct4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case DCT_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 1, shift[0]);
        fdct4x8_avx2(in, in, bitcol);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 1, 1, shift[0]);
        fadst8x4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case ADST_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 1, shift[0]);
        fadst8x4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case FLIPADST_ADST:
        load_buffer_4x8_avx2(input, in, stride, 1, 0, shift[0]);
        fadst8x4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case IDTX:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_avx2(in, in, bitcol);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fidtx4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case V_DCT:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_avx2(in, in, bitcol);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fidtx4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case H_DCT:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_avx2(in, in, bitcol);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fdct4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case V_ADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fadst8x4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fidtx4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case H_ADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_avx2(in, in, bitcol);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case V_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 1, 0, shift[0]);
        fadst8x4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fidtx4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    case H_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 1, shift[0]);
        fidtx8x4_avx2(in, in, bitcol);
        col_txfm_8x4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 4, -shift[2], new_sqrt2);
        write_buffer_4x8(outcoeff256, output);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_8x4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[4];
    __m256i*      outcoeff256 = (__m256i*)output;
    const int8_t* shift       = fwd_txfm_shift_ls[TX_8X4];
    const int32_t txw_idx     = get_txw_idx(TX_8X4);
    const int32_t txh_idx     = get_txh_idx(TX_8X4);
    int32_t       bitcol      = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow      = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fdct4x8_avx2(in, outcoeff256, bitrow);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case ADST_DCT:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fdct4x8_avx2(in, outcoeff256, bitrow);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case DCT_ADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fadst8x4_avx2(in, outcoeff256, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case ADST_ADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fadst8x4_avx2(in, outcoeff256, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case FLIPADST_DCT:
        load_buffer_8x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fdct4x8_avx2(in, outcoeff256, bitrow);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case DCT_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 1, shift[0]);
        fdct4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fadst8x4_avx2(in, outcoeff256, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 1, 1, shift[0]);
        fadst4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fadst8x4_avx2(in, outcoeff256, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case ADST_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 1, shift[0]);
        fadst4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fadst8x4_avx2(in, outcoeff256, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case FLIPADST_ADST:
        load_buffer_8x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fadst8x4_avx2(in, outcoeff256, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case IDTX:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fidtx8x4_avx2(in, outcoeff256, bitrow);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case V_DCT:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fidtx8x4_avx2(in, outcoeff256, bitrow);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case H_DCT:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fdct4x8_avx2(in, outcoeff256, bitrow);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case V_ADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fidtx8x4_avx2(in, outcoeff256, bitrow);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case H_ADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fadst8x4_avx2(in, outcoeff256, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case V_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fidtx8x4_avx2(in, outcoeff256, bitrow);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    case H_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 1, shift[0]);
        fidtx4x8_row_avx2(in, in, bitcol, 1);
        col_txfm_8x4_rounding(in, -shift[1]);
        fadst8x4_avx2(in, outcoeff256, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(outcoeff256, in, 4, -shift[2], new_sqrt2);
        transpose_4x8_avx2(in, outcoeff256);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_4x16_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[8];
    __m256i       outcoeff256[8];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_4X16];
    const int32_t txw_idx = get_txw_idx(TX_4X16);
    const int32_t txh_idx = get_txh_idx(TX_4X16);
    int32_t       bitcol  = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow  = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fdct16x4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fdct4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case ADST_DCT:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fadst16x4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fdct4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case DCT_ADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fdct16x4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case ADST_ADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fadst16x4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case FLIPADST_DCT:
        load_buffer_4x16_avx2(input, in, stride, 1, 0, shift[0]);
        fadst16x4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fdct4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case DCT_FLIPADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 1, shift[0]);
        fdct16x4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x16_avx2(input, in, stride, 1, 1, shift[0]);
        fadst16x4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case ADST_FLIPADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 1, shift[0]);
        fadst16x4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case FLIPADST_ADST:
        load_buffer_4x16_avx2(input, in, stride, 1, 0, shift[0]);
        fadst16x4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case IDTX:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx16x8_avx2(in, outcoeff256, bitcol, 1);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fidtx4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case V_DCT:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fdct16x4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fidtx4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case H_DCT:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx16x8_avx2(in, outcoeff256, bitcol, 1);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fdct4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case V_ADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fadst16x4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fidtx4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case H_ADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx16x8_avx2(in, outcoeff256, bitcol, 1);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case V_FLIPADST:
        load_buffer_4x16_avx2(input, in, stride, 1, 0, shift[0]);
        fadst16x4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fidtx4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    case H_FLIPADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 1, shift[0]);
        fidtx16x8_avx2(in, outcoeff256, bitcol, 1);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        transpose_4x16_avx2(outcoeff256, in);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_col_avx2(in + i, outcoeff256 + i * 4, bitrow, 2);
        }
        write_buffer_8x8(outcoeff256, output);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[8];
    __m256i*      outcoeff256 = (__m256i*)output;
    const int8_t* shift       = fwd_txfm_shift_ls[TX_16X4];
    const int32_t txw_idx     = get_txw_idx(TX_16X4);
    const int32_t txh_idx     = get_txh_idx(TX_16X4);
    int32_t       bitcol      = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow      = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fdct4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fdct16x4_avx2(outcoeff256, in, bitrow);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case ADST_DCT:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fdct16x4_avx2(outcoeff256, in, bitrow);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case DCT_ADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fdct4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fadst16x4_avx2(outcoeff256, in, bitrow);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case ADST_ADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fadst16x4_avx2(outcoeff256, in, bitrow);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case FLIPADST_DCT:
        load_buffer_16x4_avx2(input, in, stride, 1, 0, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fdct16x4_avx2(outcoeff256, in, bitrow);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case DCT_FLIPADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 1, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fdct4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fadst16x4_avx2(outcoeff256, in, bitrow);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x4_avx2(input, in, stride, 1, 1, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fadst16x4_avx2(outcoeff256, in, bitrow);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case ADST_FLIPADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 1, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fadst16x4_avx2(outcoeff256, in, bitrow);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case FLIPADST_ADST:
        load_buffer_16x4_avx2(input, in, stride, 1, 0, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fadst16x4_avx2(outcoeff256, in, bitrow);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case IDTX:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fidtx4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fidtx16x8_avx2(outcoeff256, in, bitrow, 1);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case V_DCT:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fdct4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fidtx16x8_avx2(outcoeff256, in, bitrow, 1);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case H_DCT:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fidtx4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fdct16x4_avx2(outcoeff256, in, bitrow);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case V_ADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fidtx16x8_avx2(outcoeff256, in, bitrow, 1);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case H_ADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fidtx4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fadst16x4_avx2(outcoeff256, in, bitrow);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case V_FLIPADST:
        load_buffer_16x4_avx2(input, in, stride, 1, 0, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fadst4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fidtx16x8_avx2(outcoeff256, in, bitrow, 1);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    case H_FLIPADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 1, shift[0]);
        for (int32_t i = 0; i < 2; i++) {
            fidtx4x8_row_avx2(in + i * 4, outcoeff256 + i * 4, bitcol, 1);
        }
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fadst16x4_avx2(outcoeff256, in, bitrow);
        transpose_4x16_avx2(in, outcoeff256);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

static INLINE void transpose_16x16_in_64x64_avx2(const __m256i* in, __m256i* out) {
    __m256i temp[32];
    TRANSPOSE_4X4_AVX2(in[0], in[8], in[16], in[24], temp[0], temp[2], temp[4], temp[6]);
    TRANSPOSE_4X4_AVX2(in[32], in[40], in[48], in[56], temp[17], temp[19], temp[21], temp[23]);
    TRANSPOSE_4X4_AVX2(in[1], in[9], in[17], in[25], temp[16], temp[18], temp[20], temp[22]);
    TRANSPOSE_4X4_AVX2(in[33], in[41], in[49], in[57], temp[25], temp[27], temp[29], temp[31]);

    TRANSPOSE_4X4_AVX2(in[64], in[72], in[80], in[88], temp[1], temp[3], temp[5], temp[7]);
    TRANSPOSE_4X4_AVX2(in[96], in[104], in[112], in[120], temp[9], temp[11], temp[13], temp[15]);
    TRANSPOSE_4X4_AVX2(in[65], in[73], in[81], in[89], temp[8], temp[10], temp[12], temp[14]);
    TRANSPOSE_4X4_AVX2(in[97], in[105], in[113], in[121], temp[24], temp[26], temp[28], temp[30]);

    out[0]   = yy_unpacklo_epi128(temp[0], temp[17]);
    out[1]   = yy_unpacklo_epi128(temp[1], temp[9]);
    out[8]   = yy_unpacklo_epi128(temp[2], temp[19]);
    out[9]   = yy_unpacklo_epi128(temp[3], temp[11]);
    out[16]  = yy_unpacklo_epi128(temp[4], temp[21]);
    out[17]  = yy_unpacklo_epi128(temp[5], temp[13]);
    out[24]  = yy_unpacklo_epi128(temp[6], temp[23]);
    out[25]  = yy_unpacklo_epi128(temp[7], temp[15]);
    out[32]  = yy_unpackhi_epi128(temp[0], temp[17]);
    out[33]  = yy_unpackhi_epi128(temp[1], temp[9]);
    out[40]  = yy_unpackhi_epi128(temp[2], temp[19]);
    out[41]  = yy_unpackhi_epi128(temp[3], temp[11]);
    out[48]  = yy_unpackhi_epi128(temp[4], temp[21]);
    out[49]  = yy_unpackhi_epi128(temp[5], temp[13]);
    out[56]  = yy_unpackhi_epi128(temp[6], temp[23]);
    out[57]  = yy_unpackhi_epi128(temp[7], temp[15]);
    out[64]  = yy_unpacklo_epi128(temp[16], temp[25]);
    out[65]  = yy_unpacklo_epi128(temp[8], temp[24]);
    out[72]  = yy_unpacklo_epi128(temp[18], temp[27]);
    out[73]  = yy_unpacklo_epi128(temp[10], temp[26]);
    out[80]  = yy_unpacklo_epi128(temp[20], temp[29]);
    out[81]  = yy_unpacklo_epi128(temp[12], temp[28]);
    out[88]  = yy_unpacklo_epi128(temp[22], temp[31]);
    out[89]  = yy_unpacklo_epi128(temp[14], temp[30]);
    out[96]  = yy_unpackhi_epi128(temp[16], temp[25]);
    out[97]  = yy_unpackhi_epi128(temp[8], temp[24]);
    out[104] = yy_unpackhi_epi128(temp[18], temp[27]);
    out[105] = yy_unpackhi_epi128(temp[10], temp[26]);
    out[112] = yy_unpackhi_epi128(temp[20], temp[29]);
    out[113] = yy_unpackhi_epi128(temp[12], temp[28]);
    out[120] = yy_unpackhi_epi128(temp[22], temp[31]);
    out[121] = yy_unpackhi_epi128(temp[14], temp[30]);
}

static AOM_FORCE_INLINE void transpose_32x32_in_64x64_avx2(const __m256i* in, __m256i* out) {
    transpose_16x16_in_64x64_avx2(in, out); //top-left
    transpose_16x16_in_64x64_avx2(in + 2, out + 128); //top-right
    transpose_16x16_in_64x64_avx2(in + 128, out + 2); //bottom-left
    transpose_16x16_in_64x64_avx2(in + 130, out + 130); //bottom-right
}

/*
*    Transpose top left block of size 16x16 in 32x32 block
*/
static INLINE void transpose_16x16_in_32x32_avx2(const __m256i* in, __m256i* out) {
    __m256i temp[32];
    TRANSPOSE_4X4_AVX2(in[0], in[4], in[8], in[12], temp[0], temp[2], temp[4], temp[6]);
    TRANSPOSE_4X4_AVX2(in[16], in[20], in[24], in[28], temp[17], temp[19], temp[21], temp[23]);
    TRANSPOSE_4X4_AVX2(in[1], in[5], in[9], in[13], temp[16], temp[18], temp[20], temp[22]);
    TRANSPOSE_4X4_AVX2(in[17], in[21], in[25], in[29], temp[25], temp[27], temp[29], temp[31]);

    TRANSPOSE_4X4_AVX2(in[32], in[36], in[40], in[44], temp[1], temp[3], temp[5], temp[7]);
    TRANSPOSE_4X4_AVX2(in[48], in[52], in[56], in[60], temp[9], temp[11], temp[13], temp[15]);
    TRANSPOSE_4X4_AVX2(in[33], in[37], in[41], in[45], temp[8], temp[10], temp[12], temp[14]);
    TRANSPOSE_4X4_AVX2(in[49], in[53], in[57], in[61], temp[24], temp[26], temp[28], temp[30]);

    out[0]  = yy_unpacklo_epi128(temp[0], temp[17]);
    out[1]  = yy_unpacklo_epi128(temp[1], temp[9]);
    out[4]  = yy_unpacklo_epi128(temp[2], temp[19]);
    out[5]  = yy_unpacklo_epi128(temp[3], temp[11]);
    out[8]  = yy_unpacklo_epi128(temp[4], temp[21]);
    out[9]  = yy_unpacklo_epi128(temp[5], temp[13]);
    out[12] = yy_unpacklo_epi128(temp[6], temp[23]);
    out[13] = yy_unpacklo_epi128(temp[7], temp[15]);
    out[16] = yy_unpackhi_epi128(temp[0], temp[17]);
    out[17] = yy_unpackhi_epi128(temp[1], temp[9]);
    out[20] = yy_unpackhi_epi128(temp[2], temp[19]);
    out[21] = yy_unpackhi_epi128(temp[3], temp[11]);
    out[24] = yy_unpackhi_epi128(temp[4], temp[21]);
    out[25] = yy_unpackhi_epi128(temp[5], temp[13]);
    out[28] = yy_unpackhi_epi128(temp[6], temp[23]);
    out[29] = yy_unpackhi_epi128(temp[7], temp[15]);
    out[32] = yy_unpacklo_epi128(temp[16], temp[25]);
    out[33] = yy_unpacklo_epi128(temp[8], temp[24]);
    out[36] = yy_unpacklo_epi128(temp[18], temp[27]);
    out[37] = yy_unpacklo_epi128(temp[10], temp[26]);
    out[40] = yy_unpacklo_epi128(temp[20], temp[29]);
    out[41] = yy_unpacklo_epi128(temp[12], temp[28]);
    out[44] = yy_unpacklo_epi128(temp[22], temp[31]);
    out[45] = yy_unpacklo_epi128(temp[14], temp[30]);
    out[48] = yy_unpackhi_epi128(temp[16], temp[25]);
    out[49] = yy_unpackhi_epi128(temp[8], temp[24]);
    out[52] = yy_unpackhi_epi128(temp[18], temp[27]);
    out[53] = yy_unpackhi_epi128(temp[10], temp[26]);
    out[56] = yy_unpackhi_epi128(temp[20], temp[29]);
    out[57] = yy_unpackhi_epi128(temp[12], temp[28]);
    out[60] = yy_unpackhi_epi128(temp[22], temp[31]);
    out[61] = yy_unpackhi_epi128(temp[14], temp[30]);
}

/*
*    Transpose top left block of size 8x8 in 16x16 block
*/
static INLINE void transpose_8x8_in_16x16_avx2(const __m256i* in, __m256i* out) {
    __m256i out1[8];
    TRANSPOSE_4X4_AVX2(in[0], in[2], in[4], in[6], out1[0], out1[1], out1[4], out1[5]);
    TRANSPOSE_4X4_AVX2(in[8], in[10], in[12], in[14], out1[2], out1[3], out1[6], out1[7]);

    out[0]  = yy_unpacklo_epi128(out1[0], out1[2]);
    out[2]  = yy_unpacklo_epi128(out1[1], out1[3]);
    out[4]  = yy_unpacklo_epi128(out1[4], out1[6]);
    out[6]  = yy_unpacklo_epi128(out1[5], out1[7]);
    out[8]  = yy_unpackhi_epi128(out1[0], out1[2]);
    out[10] = yy_unpackhi_epi128(out1[1], out1[3]);
    out[12] = yy_unpackhi_epi128(out1[4], out1[6]);
    out[14] = yy_unpackhi_epi128(out1[5], out1[7]);
}

static INLINE void transpose_8x8_half_avx2(const __m256i* in, __m256i* out) {
    const __m256i zero = _mm256_setzero_si256();
    TRANSPOSE_4X4_AVX2(in[0], in[1], in[2], in[3], out[0], out[1], out[2], out[3]);
    out[4] = yy_unpackhi_epi128(out[0], zero);
    out[5] = yy_unpackhi_epi128(out[1], zero);
    out[6] = yy_unpackhi_epi128(out[2], zero);
    out[7] = yy_unpackhi_epi128(out[3], zero);
}

static AOM_FORCE_INLINE void transpose_8x8_N2_avx2(const __m256i* in, __m256i* out) {
    TRANSPOSE_4X4_AVX2(in[0], in[1], in[2], in[3], out[0], out[1], out[2], out[3]);
}

static INLINE void transpose_4x8_in_4x16_avx2(const __m256i* in, __m256i* out) {
    __m256i perm = _mm256_set_epi32(7, 3, 6, 2, 5, 1, 4, 0);

    TRANSPOSE_4X4_AVX2(in[0], in[1], in[2], in[3], out[0], out[2], out[4], out[6]);
    out[0] = _mm256_permutevar8x32_epi32(out[0], perm);
    out[2] = _mm256_permutevar8x32_epi32(out[2], perm);
    out[4] = _mm256_permutevar8x32_epi32(out[4], perm);
    out[6] = _mm256_permutevar8x32_epi32(out[6], perm);
}

static AOM_FORCE_INLINE void transpose_4x8_in_4x16_half_avx2(const __m256i* in, __m256i* out) {
    __m256i perm = _mm256_set_epi32(7, 3, 6, 2, 5, 1, 4, 0);
    __m256i u0, u1;

    u0     = _mm256_unpacklo_epi32(in[0], in[1]);
    u1     = _mm256_unpacklo_epi32(in[2], in[3]);
    out[0] = _mm256_unpacklo_epi64(u0, u1);
    out[2] = _mm256_unpackhi_epi64(u0, u1);
    out[0] = _mm256_permutevar8x32_epi32(out[0], perm);
    out[2] = _mm256_permutevar8x32_epi32(out[2], perm);
}

static INLINE void load_buffer_4x8_in_8x8(const int16_t* input, __m256i* in, int32_t stride, int32_t flipud,
                                          int32_t fliplr, int32_t shift, int32_t step) {
    __m128i temp[4];
    if (!flipud) {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 0 * stride));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 1 * stride));
        temp[2] = _mm_loadu_si128((const __m128i*)(input + 2 * stride));
        temp[3] = _mm_loadu_si128((const __m128i*)(input + 3 * stride));
    } else {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 7 * stride));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 6 * stride));
        temp[2] = _mm_loadu_si128((const __m128i*)(input + 5 * stride));
        temp[3] = _mm_loadu_si128((const __m128i*)(input + 4 * stride));
    }

    if (fliplr) {
        temp[0] = mm_reverse_epi16(temp[0]);
        temp[1] = mm_reverse_epi16(temp[1]);
        temp[2] = mm_reverse_epi16(temp[2]);
        temp[3] = mm_reverse_epi16(temp[3]);
    }

    in[0 * step] = _mm256_cvtepi16_epi32(temp[0]);
    in[1 * step] = _mm256_cvtepi16_epi32(temp[1]);
    in[2 * step] = _mm256_cvtepi16_epi32(temp[2]);
    in[3 * step] = _mm256_cvtepi16_epi32(temp[3]);

    in[0 * step] = _mm256_slli_epi32(in[0 * step], shift);
    in[1 * step] = _mm256_slli_epi32(in[1 * step], shift);
    in[2 * step] = _mm256_slli_epi32(in[2 * step], shift);
    in[3 * step] = _mm256_slli_epi32(in[3 * step], shift);
}

static AOM_FORCE_INLINE void load_buffer_4x16_in_16x16(const int16_t* input, __m256i* out, int32_t stride,
                                                       int32_t flipud, int32_t fliplr, int32_t shift) {
    // Load 2 4x8 blocks
    const int16_t* top_l = input;
    const int16_t* top_r = input + 8;
    const int16_t* bot_l = input + 8 * stride;
    const int16_t* bot_r = input + 8 * stride + 8;

    const int16_t* tmp;

    if (flipud) {
        // Swap left columns
        top_l = bot_l;
        // Swap right columns
        top_r = bot_r;
    }

    if (fliplr) {
        // Swap top rows
        tmp   = top_l;
        top_l = top_r;
        top_r = tmp;
    }

    // load first 4 columns
    load_buffer_4x8_in_8x8(top_l, &out[0], stride, flipud, fliplr, shift, 2);

    // load second 4 columns
    load_buffer_4x8_in_8x8(top_r, &out[1], stride, flipud, fliplr, shift, 2);
}

static AOM_FORCE_INLINE void write_buffer_8x8_N2(const __m256i* res, int32_t* output) {
    const __m256i zero256 = _mm256_setzero_si256();
    const __m128i zero128 = _mm_setzero_si128();

    _mm_storeu_si128((__m128i*)(output + 0 * 8), _mm256_castsi256_si128(res[0]));
    _mm_storeu_si128((__m128i*)(output + 1 * 8), _mm256_castsi256_si128(res[1]));
    _mm_storeu_si128((__m128i*)(output + 2 * 8), _mm256_castsi256_si128(res[2]));
    _mm_storeu_si128((__m128i*)(output + 3 * 8), _mm256_castsi256_si128(res[3]));

    _mm_storeu_si128((__m128i*)(output + 0 * 8 + 4), zero128);
    _mm_storeu_si128((__m128i*)(output + 1 * 8 + 4), zero128);
    _mm_storeu_si128((__m128i*)(output + 2 * 8 + 4), zero128);
    _mm_storeu_si128((__m128i*)(output + 3 * 8 + 4), zero128);

    _mm256_storeu_si256((__m256i*)(output + 4 * 8), zero256);
    _mm256_storeu_si256((__m256i*)(output + 5 * 8), zero256);
    _mm256_storeu_si256((__m256i*)(output + 6 * 8), zero256);
    _mm256_storeu_si256((__m256i*)(output + 7 * 8), zero256);
}

static void fdct8x8_N2_avx2(const __m256i* in, __m256i* out, int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospi56  = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi8   = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi24  = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospi40  = _mm256_set1_epi32(cospi[40]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    __m256i        u[8], v[8];

    // stage 0
    // stage 1
    u[0] = _mm256_add_epi32(in[0 * col_num], in[7 * col_num]);
    v[7] = _mm256_sub_epi32(in[0 * col_num], in[7 * col_num]);
    u[1] = _mm256_add_epi32(in[1 * col_num], in[6 * col_num]);
    u[6] = _mm256_sub_epi32(in[1 * col_num], in[6 * col_num]);
    u[2] = _mm256_add_epi32(in[2 * col_num], in[5 * col_num]);
    u[5] = _mm256_sub_epi32(in[2 * col_num], in[5 * col_num]);
    u[3] = _mm256_add_epi32(in[3 * col_num], in[4 * col_num]);
    v[4] = _mm256_sub_epi32(in[3 * col_num], in[4 * col_num]);

    // stage 2
    v[0] = _mm256_add_epi32(u[0], u[3]);
    v[3] = _mm256_sub_epi32(u[0], u[3]);
    v[1] = _mm256_add_epi32(u[1], u[2]);
    v[2] = _mm256_sub_epi32(u[1], u[2]);

    v[5] = _mm256_mullo_epi32(u[5], cospim32);
    v[6] = _mm256_mullo_epi32(u[6], cospi32);
    v[5] = _mm256_add_epi32(v[5], v[6]);
    v[5] = _mm256_add_epi32(v[5], rnding);
    v[5] = _mm256_srai_epi32(v[5], bit);

    u[0] = _mm256_mullo_epi32(u[5], cospi32);
    v[6] = _mm256_mullo_epi32(u[6], cospim32);
    v[6] = _mm256_sub_epi32(u[0], v[6]);
    v[6] = _mm256_add_epi32(v[6], rnding);
    v[6] = _mm256_srai_epi32(v[6], bit);

    // stage 3
    // type 0
    v[0] = _mm256_mullo_epi32(v[0], cospi32);
    v[1] = _mm256_mullo_epi32(v[1], cospi32);
    u[0] = _mm256_add_epi32(v[0], v[1]);
    u[0] = _mm256_add_epi32(u[0], rnding);
    u[0] = _mm256_srai_epi32(u[0], bit);

    // type 1
    v[0] = _mm256_mullo_epi32(v[2], cospi48);
    v[1] = _mm256_mullo_epi32(v[3], cospi16);
    u[2] = _mm256_add_epi32(v[0], v[1]);
    u[2] = _mm256_add_epi32(u[2], rnding);
    u[2] = _mm256_srai_epi32(u[2], bit);

    u[4] = _mm256_add_epi32(v[4], v[5]);
    u[5] = _mm256_sub_epi32(v[4], v[5]);
    u[6] = _mm256_sub_epi32(v[7], v[6]);
    u[7] = _mm256_add_epi32(v[7], v[6]);

    // stage 4
    // stage 5
    v[0]             = _mm256_mullo_epi32(u[4], cospi56);
    v[1]             = _mm256_mullo_epi32(u[7], cospi8);
    v[0]             = _mm256_add_epi32(v[0], v[1]);
    v[0]             = _mm256_add_epi32(v[0], rnding);
    out[1 * col_num] = _mm256_srai_epi32(v[0], bit);

    v[0]             = _mm256_mullo_epi32(u[5], cospi40);
    v[1]             = _mm256_mullo_epi32(u[6], cospi24);
    v[0]             = _mm256_sub_epi32(v[1], v[0]);
    v[0]             = _mm256_add_epi32(v[0], rnding);
    out[3 * col_num] = _mm256_srai_epi32(v[0], bit);

    out[0 * col_num] = u[0];
    out[2 * col_num] = u[2];
}

static void fadst8x8_N2_avx2(const __m256i* in, __m256i* out, int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospim4  = _mm256_set1_epi32(-cospi[4]);
    const __m256i  cospi60  = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospim20 = _mm256_set1_epi32(-cospi[20]);
    const __m256i  cospi44  = _mm256_set1_epi32(cospi[44]);
    const __m256i  cospi28  = _mm256_set1_epi32(cospi[28]);
    const __m256i  cospi36  = _mm256_set1_epi32(cospi[36]);
    const __m256i  cospi52  = _mm256_set1_epi32(cospi[52]);
    const __m256i  cospi12  = _mm256_set1_epi32(cospi[12]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  zero     = _mm256_setzero_si256();
    __m256i        u0, u1, u2, u3, u4, u5, u6, u7;
    __m256i        v0, v1, v2, v3, v4, v5, v6, v7;
    __m256i        x, y;

    u0 = in[0 * col_num];
    u1 = _mm256_sub_epi32(zero, in[7 * col_num]);
    u2 = _mm256_sub_epi32(zero, in[3 * col_num]);
    u3 = in[4 * col_num];
    u4 = _mm256_sub_epi32(zero, in[1 * col_num]);
    u5 = in[6 * col_num];
    u6 = in[2 * col_num];
    u7 = _mm256_sub_epi32(zero, in[5 * col_num]);

    // stage 2
    v0 = u0;
    v1 = u1;

    x  = _mm256_mullo_epi32(u2, cospi32);
    y  = _mm256_mullo_epi32(u3, cospi32);
    v2 = _mm256_add_epi32(x, y);
    v2 = _mm256_add_epi32(v2, rnding);
    v2 = _mm256_srai_epi32(v2, bit);

    v3 = _mm256_sub_epi32(x, y);
    v3 = _mm256_add_epi32(v3, rnding);
    v3 = _mm256_srai_epi32(v3, bit);

    v4 = u4;
    v5 = u5;

    x  = _mm256_mullo_epi32(u6, cospi32);
    y  = _mm256_mullo_epi32(u7, cospi32);
    v6 = _mm256_add_epi32(x, y);
    v6 = _mm256_add_epi32(v6, rnding);
    v6 = _mm256_srai_epi32(v6, bit);

    v7 = _mm256_sub_epi32(x, y);
    v7 = _mm256_add_epi32(v7, rnding);
    v7 = _mm256_srai_epi32(v7, bit);

    // stage 3
    u0 = _mm256_add_epi32(v0, v2);
    u1 = _mm256_add_epi32(v1, v3);
    u2 = _mm256_sub_epi32(v0, v2);
    u3 = _mm256_sub_epi32(v1, v3);
    u4 = _mm256_add_epi32(v4, v6);
    u5 = _mm256_add_epi32(v5, v7);
    u6 = _mm256_sub_epi32(v4, v6);
    u7 = _mm256_sub_epi32(v5, v7);

    // stage 4
    v0 = u0;
    v1 = u1;
    v2 = u2;
    v3 = u3;

    x  = _mm256_mullo_epi32(u4, cospi16);
    y  = _mm256_mullo_epi32(u5, cospi48);
    v4 = _mm256_add_epi32(x, y);
    v4 = _mm256_add_epi32(v4, rnding);
    v4 = _mm256_srai_epi32(v4, bit);

    x  = _mm256_mullo_epi32(u4, cospi48);
    y  = _mm256_mullo_epi32(u5, cospim16);
    v5 = _mm256_add_epi32(x, y);
    v5 = _mm256_add_epi32(v5, rnding);
    v5 = _mm256_srai_epi32(v5, bit);

    x  = _mm256_mullo_epi32(u6, cospim48);
    y  = _mm256_mullo_epi32(u7, cospi16);
    v6 = _mm256_add_epi32(x, y);
    v6 = _mm256_add_epi32(v6, rnding);
    v6 = _mm256_srai_epi32(v6, bit);

    x  = _mm256_mullo_epi32(u6, cospi16);
    y  = _mm256_mullo_epi32(u7, cospi48);
    v7 = _mm256_add_epi32(x, y);
    v7 = _mm256_add_epi32(v7, rnding);
    v7 = _mm256_srai_epi32(v7, bit);

    // stage 5
    u0 = _mm256_add_epi32(v0, v4);
    u1 = _mm256_add_epi32(v1, v5);
    u2 = _mm256_add_epi32(v2, v6);
    u3 = _mm256_add_epi32(v3, v7);
    u4 = _mm256_sub_epi32(v0, v4);
    u5 = _mm256_sub_epi32(v1, v5);
    u6 = _mm256_sub_epi32(v2, v6);
    u7 = _mm256_sub_epi32(v3, v7);

    // stage 6
    x  = _mm256_mullo_epi32(u0, cospi60);
    y  = _mm256_mullo_epi32(u1, cospim4);
    v1 = _mm256_add_epi32(x, y);
    v1 = _mm256_add_epi32(v1, rnding);
    v1 = _mm256_srai_epi32(v1, bit);

    x  = _mm256_mullo_epi32(u2, cospi44);
    y  = _mm256_mullo_epi32(u3, cospim20);
    v3 = _mm256_add_epi32(x, y);
    v3 = _mm256_add_epi32(v3, rnding);
    v3 = _mm256_srai_epi32(v3, bit);

    x  = _mm256_mullo_epi32(u4, cospi36);
    y  = _mm256_mullo_epi32(u5, cospi28);
    v4 = _mm256_add_epi32(x, y);
    v4 = _mm256_add_epi32(v4, rnding);
    v4 = _mm256_srai_epi32(v4, bit);

    x  = _mm256_mullo_epi32(u6, cospi52);
    y  = _mm256_mullo_epi32(u7, cospi12);
    v6 = _mm256_add_epi32(x, y);
    v6 = _mm256_add_epi32(v6, rnding);
    v6 = _mm256_srai_epi32(v6, bit);

    // stage 7
    out[0 * col_num] = v1;
    out[1 * col_num] = v6;
    out[2 * col_num] = v3;
    out[3 * col_num] = v4;
}

static void fidtx8x8_N2_avx2(const __m256i* in, __m256i* out, int8_t bit, int32_t col_num) {
    (void)bit;
    out[0] = _mm256_slli_epi32(in[0 * col_num], 1);
    out[1] = _mm256_slli_epi32(in[1 * col_num], 1);
    out[2] = _mm256_slli_epi32(in[2 * col_num], 1);
    out[3] = _mm256_slli_epi32(in[3 * col_num], 1);
}

static AOM_FORCE_INLINE void col_txfm_8x8_N2_rounding(__m256i* in, int32_t shift) {
    const __m256i rounding = _mm256_set1_epi32(1 << (shift - 1));

    in[0] = _mm256_add_epi32(in[0], rounding);
    in[1] = _mm256_add_epi32(in[1], rounding);
    in[2] = _mm256_add_epi32(in[2], rounding);
    in[3] = _mm256_add_epi32(in[3], rounding);

    in[0] = _mm256_srai_epi32(in[0], shift);
    in[1] = _mm256_srai_epi32(in[1], shift);
    in[2] = _mm256_srai_epi32(in[2], shift);
    in[3] = _mm256_srai_epi32(in[3], shift);
}

static AOM_FORCE_INLINE void col_txfm_16x16_N2_rounding(__m256i* in, int32_t shift) {
    col_txfm_8x8_rounding(&in[0], shift);
    col_txfm_8x8_rounding(&in[8], shift);
}

static AOM_FORCE_INLINE void col_txfm_32x8_N2_half_rounding(__m256i* in, int32_t shift) {
    const __m256i rounding = _mm256_set1_epi32(1 << (shift - 1));
    in[0]                  = _mm256_add_epi32(in[0], rounding);
    in[1]                  = _mm256_add_epi32(in[1], rounding);
    in[4]                  = _mm256_add_epi32(in[4], rounding);
    in[5]                  = _mm256_add_epi32(in[5], rounding);
    in[0]                  = _mm256_srai_epi32(in[0], shift);
    in[1]                  = _mm256_srai_epi32(in[1], shift);
    in[4]                  = _mm256_srai_epi32(in[4], shift);
    in[5]                  = _mm256_srai_epi32(in[5], shift);
    in[8]                  = _mm256_add_epi32(in[8], rounding);
    in[9]                  = _mm256_add_epi32(in[9], rounding);
    in[12]                 = _mm256_add_epi32(in[12], rounding);
    in[13]                 = _mm256_add_epi32(in[13], rounding);
    in[8]                  = _mm256_srai_epi32(in[8], shift);
    in[9]                  = _mm256_srai_epi32(in[9], shift);
    in[12]                 = _mm256_srai_epi32(in[12], shift);
    in[13]                 = _mm256_srai_epi32(in[13], shift);
}

static AOM_FORCE_INLINE void col_txfm_16x16_N2_half_rounding(__m256i* in, int32_t shift) {
    const __m256i rounding = _mm256_set1_epi32(1 << (shift - 1));
    in[0]                  = _mm256_add_epi32(in[0], rounding);
    in[2]                  = _mm256_add_epi32(in[2], rounding);
    in[4]                  = _mm256_add_epi32(in[4], rounding);
    in[6]                  = _mm256_add_epi32(in[6], rounding);
    in[0]                  = _mm256_srai_epi32(in[0], shift);
    in[2]                  = _mm256_srai_epi32(in[2], shift);
    in[4]                  = _mm256_srai_epi32(in[4], shift);
    in[6]                  = _mm256_srai_epi32(in[6], shift);
    in[8]                  = _mm256_add_epi32(in[8], rounding);
    in[10]                 = _mm256_add_epi32(in[10], rounding);
    in[12]                 = _mm256_add_epi32(in[12], rounding);
    in[14]                 = _mm256_add_epi32(in[14], rounding);
    in[8]                  = _mm256_srai_epi32(in[8], shift);
    in[10]                 = _mm256_srai_epi32(in[10], shift);
    in[12]                 = _mm256_srai_epi32(in[12], shift);
    in[14]                 = _mm256_srai_epi32(in[14], shift);
}

static AOM_FORCE_INLINE void write_buffer_16x16_N2(const __m256i* res, int32_t* output) {
    const __m256i zero = _mm256_setzero_si256();
    int32_t       i;
    for (i = 0; i < 4; i++) {
        _mm256_storeu_si256((__m256i*)(output + i * 32), res[i * 4]);
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 8), zero);
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 16), res[i * 4 + 2]);
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 24), zero);
    }
    for (; i < 8; i++) {
        _mm256_storeu_si256((__m256i*)(output + i * 32), zero);
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 8), zero);
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 16), zero);
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 24), zero);
    }
}

static void fdct16x16_N2_avx2(const __m256i* in, __m256i* out, int8_t bit, const int32_t col_num, int32_t size) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospi56  = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi8   = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi24  = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospi40  = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi60  = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi4   = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospi28  = _mm256_set1_epi32(cospi[28]);
    const __m256i  cospi36  = _mm256_set1_epi32(cospi[36]);
    const __m256i  cospi44  = _mm256_set1_epi32(cospi[44]);
    const __m256i  cospi20  = _mm256_set1_epi32(cospi[20]);
    const __m256i  cospi12  = _mm256_set1_epi32(cospi[12]);
    const __m256i  cospi52  = _mm256_set1_epi32(cospi[52]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    __m256i        u[16], v[16], x;
    int32_t        col;

    for (col = 0; col < size; ++col) {
        // stage 0
        // stage 1
        u[0]  = _mm256_add_epi32(in[0 * col_num + col], in[15 * col_num + col]);
        u[15] = _mm256_sub_epi32(in[0 * col_num + col], in[15 * col_num + col]);
        u[1]  = _mm256_add_epi32(in[1 * col_num + col], in[14 * col_num + col]);
        u[14] = _mm256_sub_epi32(in[1 * col_num + col], in[14 * col_num + col]);
        u[2]  = _mm256_add_epi32(in[2 * col_num + col], in[13 * col_num + col]);
        u[13] = _mm256_sub_epi32(in[2 * col_num + col], in[13 * col_num + col]);
        u[3]  = _mm256_add_epi32(in[3 * col_num + col], in[12 * col_num + col]);
        u[12] = _mm256_sub_epi32(in[3 * col_num + col], in[12 * col_num + col]);
        u[4]  = _mm256_add_epi32(in[4 * col_num + col], in[11 * col_num + col]);
        u[11] = _mm256_sub_epi32(in[4 * col_num + col], in[11 * col_num + col]);
        u[5]  = _mm256_add_epi32(in[5 * col_num + col], in[10 * col_num + col]);
        u[10] = _mm256_sub_epi32(in[5 * col_num + col], in[10 * col_num + col]);
        u[6]  = _mm256_add_epi32(in[6 * col_num + col], in[9 * col_num + col]);
        u[9]  = _mm256_sub_epi32(in[6 * col_num + col], in[9 * col_num + col]);
        u[7]  = _mm256_add_epi32(in[7 * col_num + col], in[8 * col_num + col]);
        u[8]  = _mm256_sub_epi32(in[7 * col_num + col], in[8 * col_num + col]);

        // stage 2
        v[0] = _mm256_add_epi32(u[0], u[7]);
        v[7] = _mm256_sub_epi32(u[0], u[7]);
        v[1] = _mm256_add_epi32(u[1], u[6]);
        v[6] = _mm256_sub_epi32(u[1], u[6]);
        v[2] = _mm256_add_epi32(u[2], u[5]);
        v[5] = _mm256_sub_epi32(u[2], u[5]);
        v[3] = _mm256_add_epi32(u[3], u[4]);
        v[4] = _mm256_sub_epi32(u[3], u[4]);
        v[8] = u[8];
        v[9] = u[9];

        v[10] = _mm256_mullo_epi32(u[10], cospim32);
        x     = _mm256_mullo_epi32(u[13], cospi32);
        v[10] = _mm256_add_epi32(v[10], x);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[13] = _mm256_mullo_epi32(u[10], cospi32);
        x     = _mm256_mullo_epi32(u[13], cospim32);
        v[13] = _mm256_sub_epi32(v[13], x);
        v[13] = _mm256_add_epi32(v[13], rnding);
        v[13] = _mm256_srai_epi32(v[13], bit);

        v[11] = _mm256_mullo_epi32(u[11], cospim32);
        x     = _mm256_mullo_epi32(u[12], cospi32);
        v[11] = _mm256_add_epi32(v[11], x);
        v[11] = _mm256_add_epi32(v[11], rnding);
        v[11] = _mm256_srai_epi32(v[11], bit);

        v[12] = _mm256_mullo_epi32(u[11], cospi32);
        x     = _mm256_mullo_epi32(u[12], cospim32);
        v[12] = _mm256_sub_epi32(v[12], x);
        v[12] = _mm256_add_epi32(v[12], rnding);
        v[12] = _mm256_srai_epi32(v[12], bit);
        v[14] = u[14];
        v[15] = u[15];

        // stage 3
        u[0] = _mm256_add_epi32(v[0], v[3]);
        u[3] = _mm256_sub_epi32(v[0], v[3]);
        u[1] = _mm256_add_epi32(v[1], v[2]);
        u[2] = _mm256_sub_epi32(v[1], v[2]);
        u[4] = v[4];

        u[5] = _mm256_mullo_epi32(v[5], cospim32);
        x    = _mm256_mullo_epi32(v[6], cospi32);
        u[5] = _mm256_add_epi32(u[5], x);
        u[5] = _mm256_add_epi32(u[5], rnding);
        u[5] = _mm256_srai_epi32(u[5], bit);

        u[6] = _mm256_mullo_epi32(v[5], cospi32);
        x    = _mm256_mullo_epi32(v[6], cospim32);
        u[6] = _mm256_sub_epi32(u[6], x);
        u[6] = _mm256_add_epi32(u[6], rnding);
        u[6] = _mm256_srai_epi32(u[6], bit);

        u[7]  = v[7];
        u[8]  = _mm256_add_epi32(v[8], v[11]);
        u[11] = _mm256_sub_epi32(v[8], v[11]);
        u[9]  = _mm256_add_epi32(v[9], v[10]);
        u[10] = _mm256_sub_epi32(v[9], v[10]);
        u[12] = _mm256_sub_epi32(v[15], v[12]);
        u[15] = _mm256_add_epi32(v[15], v[12]);
        u[13] = _mm256_sub_epi32(v[14], v[13]);
        u[14] = _mm256_add_epi32(v[14], v[13]);

        // stage 4
        u[0] = _mm256_mullo_epi32(u[0], cospi32);
        u[1] = _mm256_mullo_epi32(u[1], cospi32);
        v[0] = _mm256_add_epi32(u[0], u[1]);
        v[0] = _mm256_add_epi32(v[0], rnding);
        v[0] = _mm256_srai_epi32(v[0], bit);

        v[2] = _mm256_mullo_epi32(u[2], cospi48);
        x    = _mm256_mullo_epi32(u[3], cospi16);
        v[2] = _mm256_add_epi32(v[2], x);
        v[2] = _mm256_add_epi32(v[2], rnding);
        v[2] = _mm256_srai_epi32(v[2], bit);

        v[4] = _mm256_add_epi32(u[4], u[5]);
        v[5] = _mm256_sub_epi32(u[4], u[5]);
        v[6] = _mm256_sub_epi32(u[7], u[6]);
        v[7] = _mm256_add_epi32(u[7], u[6]);
        v[8] = u[8];

        v[9] = _mm256_mullo_epi32(u[9], cospim16);
        x    = _mm256_mullo_epi32(u[14], cospi48);
        v[9] = _mm256_add_epi32(v[9], x);
        v[9] = _mm256_add_epi32(v[9], rnding);
        v[9] = _mm256_srai_epi32(v[9], bit);

        v[14] = _mm256_mullo_epi32(u[9], cospi48);
        x     = _mm256_mullo_epi32(u[14], cospim16);
        v[14] = _mm256_sub_epi32(v[14], x);
        v[14] = _mm256_add_epi32(v[14], rnding);
        v[14] = _mm256_srai_epi32(v[14], bit);

        v[10] = _mm256_mullo_epi32(u[10], cospim48);
        x     = _mm256_mullo_epi32(u[13], cospim16);
        v[10] = _mm256_add_epi32(v[10], x);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[13] = _mm256_mullo_epi32(u[10], cospim16);
        x     = _mm256_mullo_epi32(u[13], cospim48);
        v[13] = _mm256_sub_epi32(v[13], x);
        v[13] = _mm256_add_epi32(v[13], rnding);
        v[13] = _mm256_srai_epi32(v[13], bit);

        v[11] = u[11];
        v[12] = u[12];
        v[15] = u[15];

        // stage 5
        u[0] = v[0];
        u[2] = v[2];

        u[4] = _mm256_mullo_epi32(v[4], cospi56);
        x    = _mm256_mullo_epi32(v[7], cospi8);
        u[4] = _mm256_add_epi32(u[4], x);
        u[4] = _mm256_add_epi32(u[4], rnding);
        u[4] = _mm256_srai_epi32(u[4], bit);

        u[6] = _mm256_mullo_epi32(v[5], cospi40);
        x    = _mm256_mullo_epi32(v[6], cospi24);
        u[6] = _mm256_sub_epi32(x, u[6]);
        u[6] = _mm256_add_epi32(u[6], rnding);
        u[6] = _mm256_srai_epi32(u[6], bit);

        u[8]  = _mm256_add_epi32(v[8], v[9]);
        u[9]  = _mm256_sub_epi32(v[8], v[9]);
        u[10] = _mm256_sub_epi32(v[11], v[10]);
        u[11] = _mm256_add_epi32(v[11], v[10]);
        u[12] = _mm256_add_epi32(v[12], v[13]);
        u[13] = _mm256_sub_epi32(v[12], v[13]);
        u[14] = _mm256_sub_epi32(v[15], v[14]);
        u[15] = _mm256_add_epi32(v[15], v[14]);

        // stage 6
        v[0] = u[0];
        v[2] = u[2];
        v[4] = u[4];
        v[6] = u[6];

        v[8] = _mm256_mullo_epi32(u[8], cospi60);
        x    = _mm256_mullo_epi32(u[15], cospi4);
        v[8] = _mm256_add_epi32(v[8], x);
        v[8] = _mm256_add_epi32(v[8], rnding);
        v[8] = _mm256_srai_epi32(v[8], bit);

        v[14] = _mm256_mullo_epi32(u[9], cospi36);
        x     = _mm256_mullo_epi32(u[14], cospi28);
        v[14] = _mm256_sub_epi32(x, v[14]);
        v[14] = _mm256_add_epi32(v[14], rnding);
        v[14] = _mm256_srai_epi32(v[14], bit);

        v[10] = _mm256_mullo_epi32(u[10], cospi44);
        x     = _mm256_mullo_epi32(u[13], cospi20);
        v[10] = _mm256_add_epi32(v[10], x);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[12] = _mm256_mullo_epi32(u[11], cospi52);
        x     = _mm256_mullo_epi32(u[12], cospi12);
        v[12] = _mm256_sub_epi32(x, v[12]);
        v[12] = _mm256_add_epi32(v[12], rnding);
        v[12] = _mm256_srai_epi32(v[12], bit);

        out[0 * col_num + col] = v[0];
        out[1 * col_num + col] = v[8];
        out[2 * col_num + col] = v[4];
        out[3 * col_num + col] = v[12];
        out[4 * col_num + col] = v[2];
        out[5 * col_num + col] = v[10];
        out[6 * col_num + col] = v[6];
        out[7 * col_num + col] = v[14];
    }
}

static void fadst16x16_N2_avx2(const __m256i* in, __m256i* out, int8_t bit, const int32_t col_num, int32_t size) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospi8   = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi56  = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospim56 = _mm256_set1_epi32(-cospi[56]);
    const __m256i  cospim8  = _mm256_set1_epi32(-cospi[8]);
    const __m256i  cospi24  = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospim24 = _mm256_set1_epi32(-cospi[24]);
    const __m256i  cospim40 = _mm256_set1_epi32(-cospi[40]);
    const __m256i  cospi40  = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi62  = _mm256_set1_epi32(cospi[62]);
    const __m256i  cospim2  = _mm256_set1_epi32(-cospi[2]);
    const __m256i  cospi54  = _mm256_set1_epi32(cospi[54]);
    const __m256i  cospim10 = _mm256_set1_epi32(-cospi[10]);
    const __m256i  cospi46  = _mm256_set1_epi32(cospi[46]);
    const __m256i  cospim18 = _mm256_set1_epi32(-cospi[18]);
    const __m256i  cospi38  = _mm256_set1_epi32(cospi[38]);
    const __m256i  cospim26 = _mm256_set1_epi32(-cospi[26]);
    const __m256i  cospi34  = _mm256_set1_epi32(cospi[34]);
    const __m256i  cospi30  = _mm256_set1_epi32(cospi[30]);
    const __m256i  cospi42  = _mm256_set1_epi32(cospi[42]);
    const __m256i  cospi22  = _mm256_set1_epi32(cospi[22]);
    const __m256i  cospi50  = _mm256_set1_epi32(cospi[50]);
    const __m256i  cospi14  = _mm256_set1_epi32(cospi[14]);
    const __m256i  cospi58  = _mm256_set1_epi32(cospi[58]);
    const __m256i  cospi6   = _mm256_set1_epi32(cospi[6]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  zero     = _mm256_setzero_si256();

    __m256i u[16], v[16], x, y;
    int32_t col;

    for (col = 0; col < size; ++col) {
        // stage 0
        // stage 1
        u[0]  = in[0 * col_num + col];
        u[1]  = _mm256_sub_epi32(zero, in[15 * col_num + col]);
        u[2]  = _mm256_sub_epi32(zero, in[7 * col_num + col]);
        u[3]  = in[8 * col_num + col];
        u[4]  = _mm256_sub_epi32(zero, in[3 * col_num + col]);
        u[5]  = in[12 * col_num + col];
        u[6]  = in[4 * col_num + col];
        u[7]  = _mm256_sub_epi32(zero, in[11 * col_num + col]);
        u[8]  = _mm256_sub_epi32(zero, in[1 * col_num + col]);
        u[9]  = in[14 * col_num + col];
        u[10] = in[6 * col_num + col];
        u[11] = _mm256_sub_epi32(zero, in[9 * col_num + col]);
        u[12] = in[2 * col_num + col];
        u[13] = _mm256_sub_epi32(zero, in[13 * col_num + col]);
        u[14] = _mm256_sub_epi32(zero, in[5 * col_num + col]);
        u[15] = in[10 * col_num + col];

        // stage 2
        v[0] = u[0];
        v[1] = u[1];

        x    = _mm256_mullo_epi32(u[2], cospi32);
        y    = _mm256_mullo_epi32(u[3], cospi32);
        v[2] = _mm256_add_epi32(x, y);
        v[2] = _mm256_add_epi32(v[2], rnding);
        v[2] = _mm256_srai_epi32(v[2], bit);

        v[3] = _mm256_sub_epi32(x, y);
        v[3] = _mm256_add_epi32(v[3], rnding);
        v[3] = _mm256_srai_epi32(v[3], bit);

        v[4] = u[4];
        v[5] = u[5];

        x    = _mm256_mullo_epi32(u[6], cospi32);
        y    = _mm256_mullo_epi32(u[7], cospi32);
        v[6] = _mm256_add_epi32(x, y);
        v[6] = _mm256_add_epi32(v[6], rnding);
        v[6] = _mm256_srai_epi32(v[6], bit);

        v[7] = _mm256_sub_epi32(x, y);
        v[7] = _mm256_add_epi32(v[7], rnding);
        v[7] = _mm256_srai_epi32(v[7], bit);

        v[8] = u[8];
        v[9] = u[9];

        x     = _mm256_mullo_epi32(u[10], cospi32);
        y     = _mm256_mullo_epi32(u[11], cospi32);
        v[10] = _mm256_add_epi32(x, y);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[11] = _mm256_sub_epi32(x, y);
        v[11] = _mm256_add_epi32(v[11], rnding);
        v[11] = _mm256_srai_epi32(v[11], bit);

        v[12] = u[12];
        v[13] = u[13];

        x     = _mm256_mullo_epi32(u[14], cospi32);
        y     = _mm256_mullo_epi32(u[15], cospi32);
        v[14] = _mm256_add_epi32(x, y);
        v[14] = _mm256_add_epi32(v[14], rnding);
        v[14] = _mm256_srai_epi32(v[14], bit);

        v[15] = _mm256_sub_epi32(x, y);
        v[15] = _mm256_add_epi32(v[15], rnding);
        v[15] = _mm256_srai_epi32(v[15], bit);

        // stage 3
        u[0]  = _mm256_add_epi32(v[0], v[2]);
        u[1]  = _mm256_add_epi32(v[1], v[3]);
        u[2]  = _mm256_sub_epi32(v[0], v[2]);
        u[3]  = _mm256_sub_epi32(v[1], v[3]);
        u[4]  = _mm256_add_epi32(v[4], v[6]);
        u[5]  = _mm256_add_epi32(v[5], v[7]);
        u[6]  = _mm256_sub_epi32(v[4], v[6]);
        u[7]  = _mm256_sub_epi32(v[5], v[7]);
        u[8]  = _mm256_add_epi32(v[8], v[10]);
        u[9]  = _mm256_add_epi32(v[9], v[11]);
        u[10] = _mm256_sub_epi32(v[8], v[10]);
        u[11] = _mm256_sub_epi32(v[9], v[11]);
        u[12] = _mm256_add_epi32(v[12], v[14]);
        u[13] = _mm256_add_epi32(v[13], v[15]);
        u[14] = _mm256_sub_epi32(v[12], v[14]);
        u[15] = _mm256_sub_epi32(v[13], v[15]);

        // stage 4
        v[0]  = u[0];
        v[1]  = u[1];
        v[2]  = u[2];
        v[3]  = u[3];
        v[4]  = half_btf_avx2(&cospi16, &u[4], &cospi48, &u[5], &rnding, bit);
        v[5]  = half_btf_avx2(&cospi48, &u[4], &cospim16, &u[5], &rnding, bit);
        v[6]  = half_btf_avx2(&cospim48, &u[6], &cospi16, &u[7], &rnding, bit);
        v[7]  = half_btf_avx2(&cospi16, &u[6], &cospi48, &u[7], &rnding, bit);
        v[8]  = u[8];
        v[9]  = u[9];
        v[10] = u[10];
        v[11] = u[11];
        v[12] = half_btf_avx2(&cospi16, &u[12], &cospi48, &u[13], &rnding, bit);
        v[13] = half_btf_avx2(&cospi48, &u[12], &cospim16, &u[13], &rnding, bit);
        v[14] = half_btf_avx2(&cospim48, &u[14], &cospi16, &u[15], &rnding, bit);
        v[15] = half_btf_avx2(&cospi16, &u[14], &cospi48, &u[15], &rnding, bit);

        // stage 5
        u[0]  = _mm256_add_epi32(v[0], v[4]);
        u[1]  = _mm256_add_epi32(v[1], v[5]);
        u[2]  = _mm256_add_epi32(v[2], v[6]);
        u[3]  = _mm256_add_epi32(v[3], v[7]);
        u[4]  = _mm256_sub_epi32(v[0], v[4]);
        u[5]  = _mm256_sub_epi32(v[1], v[5]);
        u[6]  = _mm256_sub_epi32(v[2], v[6]);
        u[7]  = _mm256_sub_epi32(v[3], v[7]);
        u[8]  = _mm256_add_epi32(v[8], v[12]);
        u[9]  = _mm256_add_epi32(v[9], v[13]);
        u[10] = _mm256_add_epi32(v[10], v[14]);
        u[11] = _mm256_add_epi32(v[11], v[15]);
        u[12] = _mm256_sub_epi32(v[8], v[12]);
        u[13] = _mm256_sub_epi32(v[9], v[13]);
        u[14] = _mm256_sub_epi32(v[10], v[14]);
        u[15] = _mm256_sub_epi32(v[11], v[15]);

        // stage 6
        v[0]  = u[0];
        v[1]  = u[1];
        v[2]  = u[2];
        v[3]  = u[3];
        v[4]  = u[4];
        v[5]  = u[5];
        v[6]  = u[6];
        v[7]  = u[7];
        v[8]  = half_btf_avx2(&cospi8, &u[8], &cospi56, &u[9], &rnding, bit);
        v[9]  = half_btf_avx2(&cospi56, &u[8], &cospim8, &u[9], &rnding, bit);
        v[10] = half_btf_avx2(&cospi40, &u[10], &cospi24, &u[11], &rnding, bit);
        v[11] = half_btf_avx2(&cospi24, &u[10], &cospim40, &u[11], &rnding, bit);
        v[12] = half_btf_avx2(&cospim56, &u[12], &cospi8, &u[13], &rnding, bit);
        v[13] = half_btf_avx2(&cospi8, &u[12], &cospi56, &u[13], &rnding, bit);
        v[14] = half_btf_avx2(&cospim24, &u[14], &cospi40, &u[15], &rnding, bit);
        v[15] = half_btf_avx2(&cospi40, &u[14], &cospi24, &u[15], &rnding, bit);

        // stage 7
        u[0]  = _mm256_add_epi32(v[0], v[8]);
        u[1]  = _mm256_add_epi32(v[1], v[9]);
        u[2]  = _mm256_add_epi32(v[2], v[10]);
        u[3]  = _mm256_add_epi32(v[3], v[11]);
        u[4]  = _mm256_add_epi32(v[4], v[12]);
        u[5]  = _mm256_add_epi32(v[5], v[13]);
        u[6]  = _mm256_add_epi32(v[6], v[14]);
        u[7]  = _mm256_add_epi32(v[7], v[15]);
        u[8]  = _mm256_sub_epi32(v[0], v[8]);
        u[9]  = _mm256_sub_epi32(v[1], v[9]);
        u[10] = _mm256_sub_epi32(v[2], v[10]);
        u[11] = _mm256_sub_epi32(v[3], v[11]);
        u[12] = _mm256_sub_epi32(v[4], v[12]);
        u[13] = _mm256_sub_epi32(v[5], v[13]);
        u[14] = _mm256_sub_epi32(v[6], v[14]);
        u[15] = _mm256_sub_epi32(v[7], v[15]);

        // stage 8
        v[1]  = half_btf_avx2(&cospi62, &u[0], &cospim2, &u[1], &rnding, bit);
        v[3]  = half_btf_avx2(&cospi54, &u[2], &cospim10, &u[3], &rnding, bit);
        v[5]  = half_btf_avx2(&cospi46, &u[4], &cospim18, &u[5], &rnding, bit);
        v[7]  = half_btf_avx2(&cospi38, &u[6], &cospim26, &u[7], &rnding, bit);
        v[8]  = half_btf_avx2(&cospi34, &u[8], &cospi30, &u[9], &rnding, bit);
        v[10] = half_btf_avx2(&cospi42, &u[10], &cospi22, &u[11], &rnding, bit);
        v[12] = half_btf_avx2(&cospi50, &u[12], &cospi14, &u[13], &rnding, bit);
        v[14] = half_btf_avx2(&cospi58, &u[14], &cospi6, &u[15], &rnding, bit);

        // stage 9
        out[0 * col_num + col] = v[1];
        out[1 * col_num + col] = v[14];
        out[2 * col_num + col] = v[3];
        out[3 * col_num + col] = v[12];
        out[4 * col_num + col] = v[5];
        out[5 * col_num + col] = v[10];
        out[6 * col_num + col] = v[7];
        out[7 * col_num + col] = v[8];
    }
}

static void fidtx8xn_N2_col_avx2(const __m256i* in, __m256i* out, int8_t bit, int32_t col_num) {
    (void)bit;
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 2 * 5793; // 2 * new_sqrt2
    const __m256i newsqrt  = _mm256_set1_epi32(sqrt);
    const __m256i rounding = _mm256_set1_epi32(1 << (bits - 1));
    __m256i       temp;
    for (int32_t i = 0; i < col_num; i++) {
        temp   = _mm256_mullo_epi32(in[i], newsqrt);
        temp   = _mm256_add_epi32(temp, rounding);
        out[i] = _mm256_srai_epi32(temp, bits);
    }
}

static void fidtx16x16_N2_row_avx2(const __m256i* in, __m256i* out, int8_t bit, int32_t col_num) {
    (void)bit;
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 2 * 5793; // 2 * new_sqrt2
    const __m256i newsqrt  = _mm256_set1_epi32(sqrt);
    const __m256i rounding = _mm256_set1_epi32(1 << (bits - 1));
    __m256i       temp;
    int32_t       num_iters = 16 * col_num;
    for (int32_t i = 0; i < num_iters / 2; i += 2) {
        temp   = _mm256_mullo_epi32(in[i], newsqrt);
        temp   = _mm256_add_epi32(temp, rounding);
        out[i] = _mm256_srai_epi32(temp, bits);
    }
}

static void av1_idtx32_new_N2_avx2(const __m256i* input, __m256i* output, int8_t cos_bit, const int32_t col_num,
                                   int32_t size) {
    (void)cos_bit;

    for (int32_t i = 0; i < size; i += 4) {
        output[i * col_num]     = _mm256_slli_epi32(input[i * col_num], 2);
        output[i * col_num + 1] = _mm256_slli_epi32(input[i * col_num + 1], 2);
    }
}

static void av1_idtx16x32_N2_avx2(const __m256i* input, __m256i* output) {
    for (int32_t i = 0; i < 32; i += 2) {
        output[i] = _mm256_slli_epi32(input[i], 2);
    }
}

static AOM_FORCE_INLINE void clear_buffer_wxh_N2(__m256i* buff, int32_t num_col, int32_t num_row) {
    const __m256i zero    = _mm256_setzero_si256();
    const __m128i zero128 = _mm_setzero_si128();
    assert(num_col > 0);
    assert(num_row > 1);

    //clear top-right quarter
    if (num_col == 1) { //this means we have to use 128bit register to clear top-right quarter
        __m128i* ptr_128b = (__m128i*)buff;
        for (int i = 0; i < num_row / 2; i++) {
            ptr_128b[i * 2 + 1] = zero128;
        }

    } else {
        for (int i = 0; i < num_row / 2; i++) {
            for (int j = num_col / 2; j < num_col; j++) {
                buff[i * num_col + j] = zero;
            }
        }
    }
    //clear bottom half
    for (int i = num_row / 2; i < num_row; i++) {
        for (int j = 0; j < num_col; j++) {
            buff[i * num_col + j] = zero;
        }
    }
}

static void fidtx32x32_N2_col_avx2(const __m256i* input, __m256i* output, const int8_t cos_bit) {
    for (int32_t i = 0; i < 2; i++) {
        av1_idtx32_new_avx2(&input[i * 32], &output[i * 32], cos_bit, 1);
    }
}

static void fidtx32x32_N2_row_avx2(const __m256i* input, __m256i* output, const int8_t cos_bit) {
    int32_t i;
    for (i = 0; i < 2; i++) {
        av1_idtx32_new_N2_avx2(&input[i * 32], &output[i * 32], cos_bit, 1, 32);
    }
}

static void av1_fdct32_new_N2_avx2(const __m256i* input, __m256i* output, int8_t cos_bit, const int32_t col_num,
                                   const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m256i  __rounding = _mm256_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 3;

    __m256i cospi_m32 = _mm256_set1_epi32(-cospi[32]);
    __m256i cospi_p32 = _mm256_set1_epi32(cospi[32]);
    __m256i cospi_m16 = _mm256_set1_epi32(-cospi[16]);
    __m256i cospi_p48 = _mm256_set1_epi32(cospi[48]);
    __m256i cospi_m48 = _mm256_set1_epi32(-cospi[48]);
    __m256i cospi_m08 = _mm256_set1_epi32(-cospi[8]);
    __m256i cospi_p56 = _mm256_set1_epi32(cospi[56]);
    __m256i cospi_m56 = _mm256_set1_epi32(-cospi[56]);
    __m256i cospi_m40 = _mm256_set1_epi32(-cospi[40]);
    __m256i cospi_p24 = _mm256_set1_epi32(cospi[24]);
    __m256i cospi_m24 = _mm256_set1_epi32(-cospi[24]);
    __m256i cospi_p16 = _mm256_set1_epi32(cospi[16]);
    __m256i cospi_p08 = _mm256_set1_epi32(cospi[8]);
    __m256i cospi_p04 = _mm256_set1_epi32(cospi[4]);
    __m256i cospi_p60 = _mm256_set1_epi32(cospi[60]);
    __m256i cospi_m36 = _mm256_set1_epi32(-cospi[36]);
    __m256i cospi_p28 = _mm256_set1_epi32(cospi[28]);
    __m256i cospi_p20 = _mm256_set1_epi32(cospi[20]);
    __m256i cospi_p44 = _mm256_set1_epi32(cospi[44]);
    __m256i cospi_m52 = _mm256_set1_epi32(-cospi[52]);
    __m256i cospi_p12 = _mm256_set1_epi32(cospi[12]);
    __m256i cospi_p02 = _mm256_set1_epi32(cospi[2]);
    __m256i cospi_p06 = _mm256_set1_epi32(cospi[6]);
    __m256i cospi_p62 = _mm256_set1_epi32(cospi[62]);
    __m256i cospi_m34 = _mm256_set1_epi32(-cospi[34]);
    __m256i cospi_p30 = _mm256_set1_epi32(cospi[30]);
    __m256i cospi_p18 = _mm256_set1_epi32(cospi[18]);
    __m256i cospi_p46 = _mm256_set1_epi32(cospi[46]);
    __m256i cospi_m50 = _mm256_set1_epi32(-cospi[50]);
    __m256i cospi_p14 = _mm256_set1_epi32(cospi[14]);
    __m256i cospi_p10 = _mm256_set1_epi32(cospi[10]);
    __m256i cospi_p54 = _mm256_set1_epi32(cospi[54]);
    __m256i cospi_m42 = _mm256_set1_epi32(-cospi[42]);
    __m256i cospi_p22 = _mm256_set1_epi32(cospi[22]);
    __m256i cospi_p26 = _mm256_set1_epi32(cospi[26]);
    __m256i cospi_p38 = _mm256_set1_epi32(cospi[38]);
    __m256i cospi_m58 = _mm256_set1_epi32(-cospi[58]);

    __m256i buf0[32];
    __m256i buf1[32];

    for (int32_t col = 0; col < columns; col++) {
        const __m256i* in  = &input[col];
        __m256i*       out = &output[col];

        // stage 0
        // stage 1
        buf1[0]  = _mm256_add_epi32(in[0 * stride], in[31 * stride]);
        buf1[31] = _mm256_sub_epi32(in[0 * stride], in[31 * stride]);
        buf1[1]  = _mm256_add_epi32(in[1 * stride], in[30 * stride]);
        buf1[30] = _mm256_sub_epi32(in[1 * stride], in[30 * stride]);
        buf1[2]  = _mm256_add_epi32(in[2 * stride], in[29 * stride]);
        buf1[29] = _mm256_sub_epi32(in[2 * stride], in[29 * stride]);
        buf1[3]  = _mm256_add_epi32(in[3 * stride], in[28 * stride]);
        buf1[28] = _mm256_sub_epi32(in[3 * stride], in[28 * stride]);
        buf1[4]  = _mm256_add_epi32(in[4 * stride], in[27 * stride]);
        buf1[27] = _mm256_sub_epi32(in[4 * stride], in[27 * stride]);
        buf1[5]  = _mm256_add_epi32(in[5 * stride], in[26 * stride]);
        buf1[26] = _mm256_sub_epi32(in[5 * stride], in[26 * stride]);
        buf1[6]  = _mm256_add_epi32(in[6 * stride], in[25 * stride]);
        buf1[25] = _mm256_sub_epi32(in[6 * stride], in[25 * stride]);
        buf1[7]  = _mm256_add_epi32(in[7 * stride], in[24 * stride]);
        buf1[24] = _mm256_sub_epi32(in[7 * stride], in[24 * stride]);
        buf1[8]  = _mm256_add_epi32(in[8 * stride], in[23 * stride]);
        buf1[23] = _mm256_sub_epi32(in[8 * stride], in[23 * stride]);
        buf1[9]  = _mm256_add_epi32(in[9 * stride], in[22 * stride]);
        buf1[22] = _mm256_sub_epi32(in[9 * stride], in[22 * stride]);
        buf1[10] = _mm256_add_epi32(in[10 * stride], in[21 * stride]);
        buf1[21] = _mm256_sub_epi32(in[10 * stride], in[21 * stride]);
        buf1[11] = _mm256_add_epi32(in[11 * stride], in[20 * stride]);
        buf1[20] = _mm256_sub_epi32(in[11 * stride], in[20 * stride]);
        buf1[12] = _mm256_add_epi32(in[12 * stride], in[19 * stride]);
        buf1[19] = _mm256_sub_epi32(in[12 * stride], in[19 * stride]);
        buf1[13] = _mm256_add_epi32(in[13 * stride], in[18 * stride]);
        buf1[18] = _mm256_sub_epi32(in[13 * stride], in[18 * stride]);
        buf1[14] = _mm256_add_epi32(in[14 * stride], in[17 * stride]);
        buf1[17] = _mm256_sub_epi32(in[14 * stride], in[17 * stride]);
        buf1[15] = _mm256_add_epi32(in[15 * stride], in[16 * stride]);
        buf1[16] = _mm256_sub_epi32(in[15 * stride], in[16 * stride]);

        // stage 2
        buf0[0]  = _mm256_add_epi32(buf1[0], buf1[15]);
        buf0[15] = _mm256_sub_epi32(buf1[0], buf1[15]);
        buf0[1]  = _mm256_add_epi32(buf1[1], buf1[14]);
        buf0[14] = _mm256_sub_epi32(buf1[1], buf1[14]);
        buf0[2]  = _mm256_add_epi32(buf1[2], buf1[13]);
        buf0[13] = _mm256_sub_epi32(buf1[2], buf1[13]);
        buf0[3]  = _mm256_add_epi32(buf1[3], buf1[12]);
        buf0[12] = _mm256_sub_epi32(buf1[3], buf1[12]);
        buf0[4]  = _mm256_add_epi32(buf1[4], buf1[11]);
        buf0[11] = _mm256_sub_epi32(buf1[4], buf1[11]);
        buf0[5]  = _mm256_add_epi32(buf1[5], buf1[10]);
        buf0[10] = _mm256_sub_epi32(buf1[5], buf1[10]);
        buf0[6]  = _mm256_add_epi32(buf1[6], buf1[9]);
        buf0[9]  = _mm256_sub_epi32(buf1[6], buf1[9]);
        buf0[7]  = _mm256_add_epi32(buf1[7], buf1[8]);
        buf0[8]  = _mm256_sub_epi32(buf1[7], buf1[8]);
        buf0[16] = buf1[16];
        buf0[17] = buf1[17];
        buf0[18] = buf1[18];
        buf0[19] = buf1[19];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[23], buf1[24], buf0[23], buf0[24], __rounding, cos_bit);
        buf0[28] = buf1[28];
        buf0[29] = buf1[29];
        buf0[30] = buf1[30];
        buf0[31] = buf1[31];

        // stage 3
        buf1[0] = _mm256_add_epi32(buf0[0], buf0[7]);
        buf1[7] = _mm256_sub_epi32(buf0[0], buf0[7]);
        buf1[1] = _mm256_add_epi32(buf0[1], buf0[6]);
        buf1[6] = _mm256_sub_epi32(buf0[1], buf0[6]);
        buf1[2] = _mm256_add_epi32(buf0[2], buf0[5]);
        buf1[5] = _mm256_sub_epi32(buf0[2], buf0[5]);
        buf1[3] = _mm256_add_epi32(buf0[3], buf0[4]);
        buf1[4] = _mm256_sub_epi32(buf0[3], buf0[4]);
        buf1[8] = buf0[8];
        buf1[9] = buf0[9];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf0[11], buf0[12], buf1[11], buf1[12], __rounding, cos_bit);
        buf1[14] = buf0[14];
        buf1[15] = buf0[15];
        buf1[16] = _mm256_add_epi32(buf0[16], buf0[23]);
        buf1[23] = _mm256_sub_epi32(buf0[16], buf0[23]);
        buf1[17] = _mm256_add_epi32(buf0[17], buf0[22]);
        buf1[22] = _mm256_sub_epi32(buf0[17], buf0[22]);
        buf1[18] = _mm256_add_epi32(buf0[18], buf0[21]);
        buf1[21] = _mm256_sub_epi32(buf0[18], buf0[21]);
        buf1[19] = _mm256_add_epi32(buf0[19], buf0[20]);
        buf1[20] = _mm256_sub_epi32(buf0[19], buf0[20]);
        buf1[24] = _mm256_sub_epi32(buf0[31], buf0[24]);
        buf1[31] = _mm256_add_epi32(buf0[31], buf0[24]);
        buf1[25] = _mm256_sub_epi32(buf0[30], buf0[25]);
        buf1[30] = _mm256_add_epi32(buf0[30], buf0[25]);
        buf1[26] = _mm256_sub_epi32(buf0[29], buf0[26]);
        buf1[29] = _mm256_add_epi32(buf0[29], buf0[26]);
        buf1[27] = _mm256_sub_epi32(buf0[28], buf0[27]);
        buf1[28] = _mm256_add_epi32(buf0[28], buf0[27]);

        // stage 4
        buf0[0] = _mm256_add_epi32(buf1[0], buf1[3]);
        buf0[3] = _mm256_sub_epi32(buf1[0], buf1[3]);
        buf0[1] = _mm256_add_epi32(buf1[1], buf1[2]);
        buf0[2] = _mm256_sub_epi32(buf1[1], buf1[2]);
        buf0[4] = buf1[4];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[5], buf1[6], buf0[5], buf0[6], __rounding, cos_bit);
        buf0[7]  = buf1[7];
        buf0[8]  = _mm256_add_epi32(buf1[8], buf1[11]);
        buf0[11] = _mm256_sub_epi32(buf1[8], buf1[11]);
        buf0[9]  = _mm256_add_epi32(buf1[9], buf1[10]);
        buf0[10] = _mm256_sub_epi32(buf1[9], buf1[10]);
        buf0[12] = _mm256_sub_epi32(buf1[15], buf1[12]);
        buf0[15] = _mm256_add_epi32(buf1[15], buf1[12]);
        buf0[13] = _mm256_sub_epi32(buf1[14], buf1[13]);
        buf0[14] = _mm256_add_epi32(buf1[14], buf1[13]);
        buf0[16] = buf1[16];
        buf0[17] = buf1[17];
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, buf1[19], buf1[28], buf0[19], buf0[28], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        buf0[22] = buf1[22];
        buf0[23] = buf1[23];
        buf0[24] = buf1[24];
        buf0[25] = buf1[25];
        buf0[30] = buf1[30];
        buf0[31] = buf1[31];

        // stage 5
        buf1[0] = half_btf_avx2(&cospi_p32, &buf0[0], &cospi_p32, &buf0[1], &__rounding, cos_bit);
        buf1[2] = half_btf_avx2(&cospi_p48, &buf0[2], &cospi_p16, &buf0[3], &__rounding, cos_bit);
        buf1[4] = _mm256_add_epi32(buf0[4], buf0[5]);
        buf1[5] = _mm256_sub_epi32(buf0[4], buf0[5]);
        buf1[6] = _mm256_sub_epi32(buf0[7], buf0[6]);
        buf1[7] = _mm256_add_epi32(buf0[7], buf0[6]);
        buf1[8] = buf0[8];
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, buf0[9], buf0[14], buf1[9], buf1[14], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        buf1[11] = buf0[11];
        buf1[12] = buf0[12];
        buf1[15] = buf0[15];
        buf1[16] = _mm256_add_epi32(buf0[16], buf0[19]);
        buf1[19] = _mm256_sub_epi32(buf0[16], buf0[19]);
        buf1[17] = _mm256_add_epi32(buf0[17], buf0[18]);
        buf1[18] = _mm256_sub_epi32(buf0[17], buf0[18]);
        buf1[20] = _mm256_sub_epi32(buf0[23], buf0[20]);
        buf1[23] = _mm256_add_epi32(buf0[23], buf0[20]);
        buf1[21] = _mm256_sub_epi32(buf0[22], buf0[21]);
        buf1[22] = _mm256_add_epi32(buf0[22], buf0[21]);
        buf1[24] = _mm256_add_epi32(buf0[24], buf0[27]);
        buf1[27] = _mm256_sub_epi32(buf0[24], buf0[27]);
        buf1[25] = _mm256_add_epi32(buf0[25], buf0[26]);
        buf1[26] = _mm256_sub_epi32(buf0[25], buf0[26]);
        buf1[28] = _mm256_sub_epi32(buf0[31], buf0[28]);
        buf1[31] = _mm256_add_epi32(buf0[31], buf0[28]);
        buf1[29] = _mm256_sub_epi32(buf0[30], buf0[29]);
        buf1[30] = _mm256_add_epi32(buf0[30], buf0[29]);

        // stage 6
        buf0[0]  = buf1[0];
        buf0[2]  = buf1[2];
        buf0[4]  = half_btf_avx2(&cospi_p56, &buf1[4], &cospi_p08, &buf1[7], &__rounding, cos_bit);
        buf0[6]  = half_btf_avx2(&cospi_p24, &buf1[6], &cospi_m40, &buf1[5], &__rounding, cos_bit);
        buf0[8]  = _mm256_add_epi32(buf1[8], buf1[9]);
        buf0[9]  = _mm256_sub_epi32(buf1[8], buf1[9]);
        buf0[10] = _mm256_sub_epi32(buf1[11], buf1[10]);
        buf0[11] = _mm256_add_epi32(buf1[11], buf1[10]);
        buf0[12] = _mm256_add_epi32(buf1[12], buf1[13]);
        buf0[13] = _mm256_sub_epi32(buf1[12], buf1[13]);
        buf0[14] = _mm256_sub_epi32(buf1[15], buf1[14]);
        buf0[15] = _mm256_add_epi32(buf1[15], buf1[14]);
        buf0[16] = buf1[16];
        btf_32_type0_avx2_new(cospi_m08, cospi_p56, buf1[17], buf1[30], buf0[17], buf0[30], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m56, cospi_m08, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        buf0[19] = buf1[19];
        buf0[20] = buf1[20];
        btf_32_type0_avx2_new(cospi_m40, cospi_p24, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m24, cospi_m40, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);
        buf0[23] = buf1[23];
        buf0[24] = buf1[24];
        buf0[27] = buf1[27];
        buf0[28] = buf1[28];
        buf0[31] = buf1[31];

        // stage 7
        buf1[0]  = buf0[0];
        buf1[2]  = buf0[2];
        buf1[4]  = buf0[4];
        buf1[6]  = buf0[6];
        buf1[8]  = half_btf_avx2(&cospi_p60, &buf0[8], &cospi_p04, &buf0[15], &__rounding, cos_bit);
        buf1[14] = half_btf_avx2(&cospi_p28, &buf0[14], &cospi_m36, &buf0[9], &__rounding, cos_bit);
        buf1[10] = half_btf_avx2(&cospi_p44, &buf0[10], &cospi_p20, &buf0[13], &__rounding, cos_bit);
        buf1[12] = half_btf_avx2(&cospi_p12, &buf0[12], &cospi_m52, &buf0[11], &__rounding, cos_bit);

        buf1[16] = _mm256_add_epi32(buf0[16], buf0[17]);
        buf1[17] = _mm256_sub_epi32(buf0[16], buf0[17]);
        buf1[18] = _mm256_sub_epi32(buf0[19], buf0[18]);
        buf1[19] = _mm256_add_epi32(buf0[19], buf0[18]);
        buf1[20] = _mm256_add_epi32(buf0[20], buf0[21]);
        buf1[21] = _mm256_sub_epi32(buf0[20], buf0[21]);
        buf1[22] = _mm256_sub_epi32(buf0[23], buf0[22]);
        buf1[23] = _mm256_add_epi32(buf0[23], buf0[22]);
        buf1[24] = _mm256_add_epi32(buf0[24], buf0[25]);
        buf1[25] = _mm256_sub_epi32(buf0[24], buf0[25]);
        buf1[26] = _mm256_sub_epi32(buf0[27], buf0[26]);
        buf1[27] = _mm256_add_epi32(buf0[27], buf0[26]);
        buf1[28] = _mm256_add_epi32(buf0[28], buf0[29]);
        buf1[29] = _mm256_sub_epi32(buf0[28], buf0[29]);
        buf1[30] = _mm256_sub_epi32(buf0[31], buf0[30]);
        buf1[31] = _mm256_add_epi32(buf0[31], buf0[30]);

        // stage 8
        buf0[0]  = buf1[0];
        buf0[2]  = buf1[2];
        buf0[4]  = buf1[4];
        buf0[6]  = buf1[6];
        buf0[8]  = buf1[8];
        buf0[10] = buf1[10];
        buf0[12] = buf1[12];
        buf0[14] = buf1[14];
        buf0[16] = half_btf_avx2(&cospi_p62, &buf1[16], &cospi_p02, &buf1[31], &__rounding, cos_bit);
        buf0[30] = half_btf_avx2(&cospi_p30, &buf1[30], &cospi_m34, &buf1[17], &__rounding, cos_bit);
        buf0[18] = half_btf_avx2(&cospi_p46, &buf1[18], &cospi_p18, &buf1[29], &__rounding, cos_bit);
        buf0[28] = half_btf_avx2(&cospi_p14, &buf1[28], &cospi_m50, &buf1[19], &__rounding, cos_bit);
        buf0[20] = half_btf_avx2(&cospi_p54, &buf1[20], &cospi_p10, &buf1[27], &__rounding, cos_bit);
        buf0[26] = half_btf_avx2(&cospi_p22, &buf1[26], &cospi_m42, &buf1[21], &__rounding, cos_bit);
        buf0[22] = half_btf_avx2(&cospi_p38, &buf1[22], &cospi_p26, &buf1[25], &__rounding, cos_bit);
        buf0[24] = half_btf_avx2(&cospi_p06, &buf1[24], &cospi_m58, &buf1[23], &__rounding, cos_bit);

        // stage 9
        out[0 * stride]  = buf0[0];
        out[1 * stride]  = buf0[16];
        out[2 * stride]  = buf0[8];
        out[3 * stride]  = buf0[24];
        out[4 * stride]  = buf0[4];
        out[5 * stride]  = buf0[20];
        out[6 * stride]  = buf0[12];
        out[7 * stride]  = buf0[28];
        out[8 * stride]  = buf0[2];
        out[9 * stride]  = buf0[18];
        out[10 * stride] = buf0[10];
        out[11 * stride] = buf0[26];
        out[12 * stride] = buf0[6];
        out[13 * stride] = buf0[22];
        out[14 * stride] = buf0[14];
        out[15 * stride] = buf0[30];
    }
}

static AOM_FORCE_INLINE void fdct32x32_N2_row_avx2(const __m256i* input, __m256i* output, const int8_t cos_bit) {
    const int32_t txfm_size   = 32;
    const int32_t num_per_256 = 8;
    int32_t       col_num     = txfm_size / num_per_256;
    av1_fdct32_new_N2_avx2(input, output, cos_bit, txfm_size / 2, col_num);
}

static AOM_FORCE_INLINE void fdct32x32_N2_col_avx2(const __m256i* input, __m256i* output, const int8_t cos_bit) {
    const int32_t txfm_size   = 32;
    const int32_t num_per_256 = 8;
    int32_t       col_num     = txfm_size / num_per_256;
    av1_fdct32_new_N2_avx2(input, output, cos_bit, txfm_size, col_num);
}

static AOM_FORCE_INLINE void av1_round_shift_array_32_N2_avx2(__m256i* input, __m256i* output, const int32_t size,
                                                              const int32_t bit) {
    int32_t i;

    if (bit > 0) {
        const __m256i round = _mm256_set1_epi32(1 << (bit - 1));
        __m256i       r0;
        for (i = 0; i < size; i += 4) {
            r0            = _mm256_add_epi32(input[i], round);
            output[i]     = _mm256_srai_epi32(r0, bit);
            r0            = _mm256_add_epi32(input[i + 1], round);
            output[i + 1] = _mm256_srai_epi32(r0, bit);
        }
    } else {
        for (i = 0; i < size; i += 4) {
            output[i]     = _mm256_slli_epi32(input[i], -bit);
            output[i + 1] = _mm256_slli_epi32(input[i + 1], -bit);
        }
    }
}

static INLINE void av1_round_shift_rect_array_wxh_avx2(__m256i* input, __m256i* output, const int32_t bit,
                                                       const int32_t val, int32_t num_col, int32_t num_row) {
    const __m256i sqrt2  = _mm256_set1_epi32(val);
    const __m256i round2 = _mm256_set1_epi32(1 << (new_sqrt2_bits - 1));
    int32_t       i;
    if (bit > 0) {
        const __m256i round1 = _mm256_set1_epi32(1 << (bit - 1));
        __m256i       r0, r1, r2, r3;
        for (i = 0; i < num_row / 2; i++) {
            for (int j = 0; j < num_col / 2; j++) {
                r0                      = _mm256_add_epi32(input[i * num_col + j], round1);
                r1                      = _mm256_srai_epi32(r0, bit);
                r2                      = _mm256_mullo_epi32(sqrt2, r1);
                r3                      = _mm256_add_epi32(r2, round2);
                output[i * num_col + j] = _mm256_srai_epi32(r3, new_sqrt2_bits);
            }
        }
    } else {
        __m256i r0, r1, r2;
        for (i = 0; i < num_row / 2; i++) {
            for (int j = 0; j < num_col / 2; j++) {
                r0                      = _mm256_slli_epi32(input[i * num_col + j], -bit);
                r1                      = _mm256_mullo_epi32(sqrt2, r0);
                r2                      = _mm256_add_epi32(r1, round2);
                output[i * num_col + j] = _mm256_srai_epi32(r2, new_sqrt2_bits);
            }
        }
    }
}

static INLINE void load_buffer_32x32_in_64x64_avx2(const int16_t* input, int32_t stride, __m256i* output) {
    __m128i x0, x1, x2, x3;
    __m256i v0, v1, v2, v3;
    int32_t i;

    for (i = 0; i < 32; ++i) {
        x0 = _mm_loadu_si128((const __m128i*)(input + 0 * 8));
        x1 = _mm_loadu_si128((const __m128i*)(input + 1 * 8));
        x2 = _mm_loadu_si128((const __m128i*)(input + 2 * 8));
        x3 = _mm_loadu_si128((const __m128i*)(input + 3 * 8));

        v0 = _mm256_cvtepi16_epi32(x0);
        v1 = _mm256_cvtepi16_epi32(x1);
        v2 = _mm256_cvtepi16_epi32(x2);
        v3 = _mm256_cvtepi16_epi32(x3);

        _mm256_storeu_si256(output + 0, v0);
        _mm256_storeu_si256(output + 1, v1);
        _mm256_storeu_si256(output + 2, v2);
        _mm256_storeu_si256(output + 3, v3);

        input += stride;
        output += 8;
    }
}

static void av1_fdct64_new_N2_avx2(const __m256i* input, __m256i* output, int8_t cos_bit, const int32_t col_num,
                                   const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m256i  __rounding = _mm256_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 3;

    __m256i cospi_m32 = _mm256_set1_epi32(-cospi[32]);
    __m256i cospi_p32 = _mm256_set1_epi32(cospi[32]);
    __m256i cospi_m16 = _mm256_set1_epi32(-cospi[16]);
    __m256i cospi_p48 = _mm256_set1_epi32(cospi[48]);
    __m256i cospi_m48 = _mm256_set1_epi32(-cospi[48]);
    __m256i cospi_p16 = _mm256_set1_epi32(cospi[16]);
    __m256i cospi_m08 = _mm256_set1_epi32(-cospi[8]);
    __m256i cospi_p56 = _mm256_set1_epi32(cospi[56]);
    __m256i cospi_m56 = _mm256_set1_epi32(-cospi[56]);
    __m256i cospi_m40 = _mm256_set1_epi32(-cospi[40]);
    __m256i cospi_p24 = _mm256_set1_epi32(cospi[24]);
    __m256i cospi_m24 = _mm256_set1_epi32(-cospi[24]);
    __m256i cospi_p08 = _mm256_set1_epi32(cospi[8]);
    __m256i cospi_p60 = _mm256_set1_epi32(cospi[60]);
    __m256i cospi_p04 = _mm256_set1_epi32(cospi[4]);
    __m256i cospi_p28 = _mm256_set1_epi32(cospi[28]);
    __m256i cospi_p44 = _mm256_set1_epi32(cospi[44]);
    __m256i cospi_p20 = _mm256_set1_epi32(cospi[20]);
    __m256i cospi_p12 = _mm256_set1_epi32(cospi[12]);
    __m256i cospi_m04 = _mm256_set1_epi32(-cospi[4]);
    __m256i cospi_m60 = _mm256_set1_epi32(-cospi[60]);
    __m256i cospi_m36 = _mm256_set1_epi32(-cospi[36]);
    __m256i cospi_m28 = _mm256_set1_epi32(-cospi[28]);
    __m256i cospi_m20 = _mm256_set1_epi32(-cospi[20]);
    __m256i cospi_m44 = _mm256_set1_epi32(-cospi[44]);
    __m256i cospi_m52 = _mm256_set1_epi32(-cospi[52]);
    __m256i cospi_m12 = _mm256_set1_epi32(-cospi[12]);
    __m256i cospi_p62 = _mm256_set1_epi32(cospi[62]);
    __m256i cospi_p02 = _mm256_set1_epi32(cospi[2]);
    __m256i cospi_p30 = _mm256_set1_epi32(cospi[30]);
    __m256i cospi_m34 = _mm256_set1_epi32(-cospi[34]);
    __m256i cospi_p46 = _mm256_set1_epi32(cospi[46]);
    __m256i cospi_p18 = _mm256_set1_epi32(cospi[18]);
    __m256i cospi_p14 = _mm256_set1_epi32(cospi[14]);
    __m256i cospi_m50 = _mm256_set1_epi32(-cospi[50]);
    __m256i cospi_p54 = _mm256_set1_epi32(cospi[54]);
    __m256i cospi_p10 = _mm256_set1_epi32(cospi[10]);
    __m256i cospi_p22 = _mm256_set1_epi32(cospi[22]);
    __m256i cospi_m42 = _mm256_set1_epi32(-cospi[42]);
    __m256i cospi_p38 = _mm256_set1_epi32(cospi[38]);
    __m256i cospi_p26 = _mm256_set1_epi32(cospi[26]);
    __m256i cospi_p06 = _mm256_set1_epi32(cospi[6]);
    __m256i cospi_m58 = _mm256_set1_epi32(-cospi[58]);
    __m256i cospi_p63 = _mm256_set1_epi32(cospi[63]);
    __m256i cospi_p01 = _mm256_set1_epi32(cospi[1]);
    __m256i cospi_p31 = _mm256_set1_epi32(cospi[31]);
    __m256i cospi_m33 = _mm256_set1_epi32(-cospi[33]);
    __m256i cospi_p47 = _mm256_set1_epi32(cospi[47]);
    __m256i cospi_p17 = _mm256_set1_epi32(cospi[17]);
    __m256i cospi_p15 = _mm256_set1_epi32(cospi[15]);
    __m256i cospi_m49 = _mm256_set1_epi32(-cospi[49]);
    __m256i cospi_p55 = _mm256_set1_epi32(cospi[55]);
    __m256i cospi_p09 = _mm256_set1_epi32(cospi[9]);
    __m256i cospi_p23 = _mm256_set1_epi32(cospi[23]);
    __m256i cospi_m41 = _mm256_set1_epi32(-cospi[41]);
    __m256i cospi_p39 = _mm256_set1_epi32(cospi[39]);
    __m256i cospi_p25 = _mm256_set1_epi32(cospi[25]);
    __m256i cospi_p07 = _mm256_set1_epi32(cospi[7]);
    __m256i cospi_m57 = _mm256_set1_epi32(-cospi[57]);
    __m256i cospi_p59 = _mm256_set1_epi32(cospi[59]);
    __m256i cospi_p05 = _mm256_set1_epi32(cospi[5]);
    __m256i cospi_p27 = _mm256_set1_epi32(cospi[27]);
    __m256i cospi_m37 = _mm256_set1_epi32(-cospi[37]);
    __m256i cospi_p43 = _mm256_set1_epi32(cospi[43]);
    __m256i cospi_p21 = _mm256_set1_epi32(cospi[21]);
    __m256i cospi_p11 = _mm256_set1_epi32(cospi[11]);
    __m256i cospi_m53 = _mm256_set1_epi32(-cospi[53]);
    __m256i cospi_p51 = _mm256_set1_epi32(cospi[51]);
    __m256i cospi_p13 = _mm256_set1_epi32(cospi[13]);
    __m256i cospi_p19 = _mm256_set1_epi32(cospi[19]);
    __m256i cospi_m45 = _mm256_set1_epi32(-cospi[45]);
    __m256i cospi_p35 = _mm256_set1_epi32(cospi[35]);
    __m256i cospi_p29 = _mm256_set1_epi32(cospi[29]);
    __m256i cospi_p03 = _mm256_set1_epi32(cospi[3]);
    __m256i cospi_m61 = _mm256_set1_epi32(-cospi[61]);

    for (int32_t col = 0; col < columns; col++) {
        const __m256i* in  = &input[col];
        __m256i*       out = &output[col];

        // stage 1
        __m256i x1[64];
        x1[0]  = _mm256_add_epi32(in[0 * stride], in[63 * stride]);
        x1[63] = _mm256_sub_epi32(in[0 * stride], in[63 * stride]);
        x1[1]  = _mm256_add_epi32(in[1 * stride], in[62 * stride]);
        x1[62] = _mm256_sub_epi32(in[1 * stride], in[62 * stride]);
        x1[2]  = _mm256_add_epi32(in[2 * stride], in[61 * stride]);
        x1[61] = _mm256_sub_epi32(in[2 * stride], in[61 * stride]);
        x1[3]  = _mm256_add_epi32(in[3 * stride], in[60 * stride]);
        x1[60] = _mm256_sub_epi32(in[3 * stride], in[60 * stride]);
        x1[4]  = _mm256_add_epi32(in[4 * stride], in[59 * stride]);
        x1[59] = _mm256_sub_epi32(in[4 * stride], in[59 * stride]);
        x1[5]  = _mm256_add_epi32(in[5 * stride], in[58 * stride]);
        x1[58] = _mm256_sub_epi32(in[5 * stride], in[58 * stride]);
        x1[6]  = _mm256_add_epi32(in[6 * stride], in[57 * stride]);
        x1[57] = _mm256_sub_epi32(in[6 * stride], in[57 * stride]);
        x1[7]  = _mm256_add_epi32(in[7 * stride], in[56 * stride]);
        x1[56] = _mm256_sub_epi32(in[7 * stride], in[56 * stride]);
        x1[8]  = _mm256_add_epi32(in[8 * stride], in[55 * stride]);
        x1[55] = _mm256_sub_epi32(in[8 * stride], in[55 * stride]);
        x1[9]  = _mm256_add_epi32(in[9 * stride], in[54 * stride]);
        x1[54] = _mm256_sub_epi32(in[9 * stride], in[54 * stride]);
        x1[10] = _mm256_add_epi32(in[10 * stride], in[53 * stride]);
        x1[53] = _mm256_sub_epi32(in[10 * stride], in[53 * stride]);
        x1[11] = _mm256_add_epi32(in[11 * stride], in[52 * stride]);
        x1[52] = _mm256_sub_epi32(in[11 * stride], in[52 * stride]);
        x1[12] = _mm256_add_epi32(in[12 * stride], in[51 * stride]);
        x1[51] = _mm256_sub_epi32(in[12 * stride], in[51 * stride]);
        x1[13] = _mm256_add_epi32(in[13 * stride], in[50 * stride]);
        x1[50] = _mm256_sub_epi32(in[13 * stride], in[50 * stride]);
        x1[14] = _mm256_add_epi32(in[14 * stride], in[49 * stride]);
        x1[49] = _mm256_sub_epi32(in[14 * stride], in[49 * stride]);
        x1[15] = _mm256_add_epi32(in[15 * stride], in[48 * stride]);
        x1[48] = _mm256_sub_epi32(in[15 * stride], in[48 * stride]);
        x1[16] = _mm256_add_epi32(in[16 * stride], in[47 * stride]);
        x1[47] = _mm256_sub_epi32(in[16 * stride], in[47 * stride]);
        x1[17] = _mm256_add_epi32(in[17 * stride], in[46 * stride]);
        x1[46] = _mm256_sub_epi32(in[17 * stride], in[46 * stride]);
        x1[18] = _mm256_add_epi32(in[18 * stride], in[45 * stride]);
        x1[45] = _mm256_sub_epi32(in[18 * stride], in[45 * stride]);
        x1[19] = _mm256_add_epi32(in[19 * stride], in[44 * stride]);
        x1[44] = _mm256_sub_epi32(in[19 * stride], in[44 * stride]);
        x1[20] = _mm256_add_epi32(in[20 * stride], in[43 * stride]);
        x1[43] = _mm256_sub_epi32(in[20 * stride], in[43 * stride]);
        x1[21] = _mm256_add_epi32(in[21 * stride], in[42 * stride]);
        x1[42] = _mm256_sub_epi32(in[21 * stride], in[42 * stride]);
        x1[22] = _mm256_add_epi32(in[22 * stride], in[41 * stride]);
        x1[41] = _mm256_sub_epi32(in[22 * stride], in[41 * stride]);
        x1[23] = _mm256_add_epi32(in[23 * stride], in[40 * stride]);
        x1[40] = _mm256_sub_epi32(in[23 * stride], in[40 * stride]);
        x1[24] = _mm256_add_epi32(in[24 * stride], in[39 * stride]);
        x1[39] = _mm256_sub_epi32(in[24 * stride], in[39 * stride]);
        x1[25] = _mm256_add_epi32(in[25 * stride], in[38 * stride]);
        x1[38] = _mm256_sub_epi32(in[25 * stride], in[38 * stride]);
        x1[26] = _mm256_add_epi32(in[26 * stride], in[37 * stride]);
        x1[37] = _mm256_sub_epi32(in[26 * stride], in[37 * stride]);
        x1[27] = _mm256_add_epi32(in[27 * stride], in[36 * stride]);
        x1[36] = _mm256_sub_epi32(in[27 * stride], in[36 * stride]);
        x1[28] = _mm256_add_epi32(in[28 * stride], in[35 * stride]);
        x1[35] = _mm256_sub_epi32(in[28 * stride], in[35 * stride]);
        x1[29] = _mm256_add_epi32(in[29 * stride], in[34 * stride]);
        x1[34] = _mm256_sub_epi32(in[29 * stride], in[34 * stride]);
        x1[30] = _mm256_add_epi32(in[30 * stride], in[33 * stride]);
        x1[33] = _mm256_sub_epi32(in[30 * stride], in[33 * stride]);
        x1[31] = _mm256_add_epi32(in[31 * stride], in[32 * stride]);
        x1[32] = _mm256_sub_epi32(in[31 * stride], in[32 * stride]);

        // stage 2
        __m256i x2[64];
        x2[0]  = _mm256_add_epi32(x1[0], x1[31]);
        x2[31] = _mm256_sub_epi32(x1[0], x1[31]);
        x2[1]  = _mm256_add_epi32(x1[1], x1[30]);
        x2[30] = _mm256_sub_epi32(x1[1], x1[30]);
        x2[2]  = _mm256_add_epi32(x1[2], x1[29]);
        x2[29] = _mm256_sub_epi32(x1[2], x1[29]);
        x2[3]  = _mm256_add_epi32(x1[3], x1[28]);
        x2[28] = _mm256_sub_epi32(x1[3], x1[28]);
        x2[4]  = _mm256_add_epi32(x1[4], x1[27]);
        x2[27] = _mm256_sub_epi32(x1[4], x1[27]);
        x2[5]  = _mm256_add_epi32(x1[5], x1[26]);
        x2[26] = _mm256_sub_epi32(x1[5], x1[26]);
        x2[6]  = _mm256_add_epi32(x1[6], x1[25]);
        x2[25] = _mm256_sub_epi32(x1[6], x1[25]);
        x2[7]  = _mm256_add_epi32(x1[7], x1[24]);
        x2[24] = _mm256_sub_epi32(x1[7], x1[24]);
        x2[8]  = _mm256_add_epi32(x1[8], x1[23]);
        x2[23] = _mm256_sub_epi32(x1[8], x1[23]);
        x2[9]  = _mm256_add_epi32(x1[9], x1[22]);
        x2[22] = _mm256_sub_epi32(x1[9], x1[22]);
        x2[10] = _mm256_add_epi32(x1[10], x1[21]);
        x2[21] = _mm256_sub_epi32(x1[10], x1[21]);
        x2[11] = _mm256_add_epi32(x1[11], x1[20]);
        x2[20] = _mm256_sub_epi32(x1[11], x1[20]);
        x2[12] = _mm256_add_epi32(x1[12], x1[19]);
        x2[19] = _mm256_sub_epi32(x1[12], x1[19]);
        x2[13] = _mm256_add_epi32(x1[13], x1[18]);
        x2[18] = _mm256_sub_epi32(x1[13], x1[18]);
        x2[14] = _mm256_add_epi32(x1[14], x1[17]);
        x2[17] = _mm256_sub_epi32(x1[14], x1[17]);
        x2[15] = _mm256_add_epi32(x1[15], x1[16]);
        x2[16] = _mm256_sub_epi32(x1[15], x1[16]);
        x2[32] = x1[32];
        x2[33] = x1[33];
        x2[34] = x1[34];
        x2[35] = x1[35];
        x2[36] = x1[36];
        x2[37] = x1[37];
        x2[38] = x1[38];
        x2[39] = x1[39];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[40], x1[55], x2[40], x2[55], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[41], x1[54], x2[41], x2[54], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[42], x1[53], x2[42], x2[53], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[43], x1[52], x2[43], x2[52], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[44], x1[51], x2[44], x2[51], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[45], x1[50], x2[45], x2[50], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[46], x1[49], x2[46], x2[49], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[47], x1[48], x2[47], x2[48], __rounding, cos_bit);
        x2[56] = x1[56];
        x2[57] = x1[57];
        x2[58] = x1[58];
        x2[59] = x1[59];
        x2[60] = x1[60];
        x2[61] = x1[61];
        x2[62] = x1[62];
        x2[63] = x1[63];

        // stage 3
        __m256i x3[64];
        x3[0]  = _mm256_add_epi32(x2[0], x2[15]);
        x3[15] = _mm256_sub_epi32(x2[0], x2[15]);
        x3[1]  = _mm256_add_epi32(x2[1], x2[14]);
        x3[14] = _mm256_sub_epi32(x2[1], x2[14]);
        x3[2]  = _mm256_add_epi32(x2[2], x2[13]);
        x3[13] = _mm256_sub_epi32(x2[2], x2[13]);
        x3[3]  = _mm256_add_epi32(x2[3], x2[12]);
        x3[12] = _mm256_sub_epi32(x2[3], x2[12]);
        x3[4]  = _mm256_add_epi32(x2[4], x2[11]);
        x3[11] = _mm256_sub_epi32(x2[4], x2[11]);
        x3[5]  = _mm256_add_epi32(x2[5], x2[10]);
        x3[10] = _mm256_sub_epi32(x2[5], x2[10]);
        x3[6]  = _mm256_add_epi32(x2[6], x2[9]);
        x3[9]  = _mm256_sub_epi32(x2[6], x2[9]);
        x3[7]  = _mm256_add_epi32(x2[7], x2[8]);
        x3[8]  = _mm256_sub_epi32(x2[7], x2[8]);
        x3[16] = x2[16];
        x3[17] = x2[17];
        x3[18] = x2[18];
        x3[19] = x2[19];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x2[20], x2[27], x3[20], x3[27], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x2[21], x2[26], x3[21], x3[26], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x2[22], x2[25], x3[22], x3[25], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x2[23], x2[24], x3[23], x3[24], __rounding, cos_bit);
        x3[28] = x2[28];
        x3[29] = x2[29];
        x3[30] = x2[30];
        x3[31] = x2[31];
        x3[32] = _mm256_add_epi32(x2[32], x2[47]);
        x3[47] = _mm256_sub_epi32(x2[32], x2[47]);
        x3[33] = _mm256_add_epi32(x2[33], x2[46]);
        x3[46] = _mm256_sub_epi32(x2[33], x2[46]);
        x3[34] = _mm256_add_epi32(x2[34], x2[45]);
        x3[45] = _mm256_sub_epi32(x2[34], x2[45]);
        x3[35] = _mm256_add_epi32(x2[35], x2[44]);
        x3[44] = _mm256_sub_epi32(x2[35], x2[44]);
        x3[36] = _mm256_add_epi32(x2[36], x2[43]);
        x3[43] = _mm256_sub_epi32(x2[36], x2[43]);
        x3[37] = _mm256_add_epi32(x2[37], x2[42]);
        x3[42] = _mm256_sub_epi32(x2[37], x2[42]);
        x3[38] = _mm256_add_epi32(x2[38], x2[41]);
        x3[41] = _mm256_sub_epi32(x2[38], x2[41]);
        x3[39] = _mm256_add_epi32(x2[39], x2[40]);
        x3[40] = _mm256_sub_epi32(x2[39], x2[40]);
        x3[48] = _mm256_sub_epi32(x2[63], x2[48]);
        x3[63] = _mm256_add_epi32(x2[63], x2[48]);
        x3[49] = _mm256_sub_epi32(x2[62], x2[49]);
        x3[62] = _mm256_add_epi32(x2[62], x2[49]);
        x3[50] = _mm256_sub_epi32(x2[61], x2[50]);
        x3[61] = _mm256_add_epi32(x2[61], x2[50]);
        x3[51] = _mm256_sub_epi32(x2[60], x2[51]);
        x3[60] = _mm256_add_epi32(x2[60], x2[51]);
        x3[52] = _mm256_sub_epi32(x2[59], x2[52]);
        x3[59] = _mm256_add_epi32(x2[59], x2[52]);
        x3[53] = _mm256_sub_epi32(x2[58], x2[53]);
        x3[58] = _mm256_add_epi32(x2[58], x2[53]);
        x3[54] = _mm256_sub_epi32(x2[57], x2[54]);
        x3[57] = _mm256_add_epi32(x2[57], x2[54]);
        x3[55] = _mm256_sub_epi32(x2[56], x2[55]);
        x3[56] = _mm256_add_epi32(x2[56], x2[55]);

        // stage 4
        __m256i x4[64];
        x4[0] = _mm256_add_epi32(x3[0], x3[7]);
        x4[7] = _mm256_sub_epi32(x3[0], x3[7]);
        x4[1] = _mm256_add_epi32(x3[1], x3[6]);
        x4[6] = _mm256_sub_epi32(x3[1], x3[6]);
        x4[2] = _mm256_add_epi32(x3[2], x3[5]);
        x4[5] = _mm256_sub_epi32(x3[2], x3[5]);
        x4[3] = _mm256_add_epi32(x3[3], x3[4]);
        x4[4] = _mm256_sub_epi32(x3[3], x3[4]);
        x4[8] = x3[8];
        x4[9] = x3[9];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x3[10], x3[13], x4[10], x4[13], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x3[11], x3[12], x4[11], x4[12], __rounding, cos_bit);
        x4[14] = x3[14];
        x4[15] = x3[15];
        x4[16] = _mm256_add_epi32(x3[16], x3[23]);
        x4[23] = _mm256_sub_epi32(x3[16], x3[23]);
        x4[17] = _mm256_add_epi32(x3[17], x3[22]);
        x4[22] = _mm256_sub_epi32(x3[17], x3[22]);
        x4[18] = _mm256_add_epi32(x3[18], x3[21]);
        x4[21] = _mm256_sub_epi32(x3[18], x3[21]);
        x4[19] = _mm256_add_epi32(x3[19], x3[20]);
        x4[20] = _mm256_sub_epi32(x3[19], x3[20]);
        x4[24] = _mm256_sub_epi32(x3[31], x3[24]);
        x4[31] = _mm256_add_epi32(x3[31], x3[24]);
        x4[25] = _mm256_sub_epi32(x3[30], x3[25]);
        x4[30] = _mm256_add_epi32(x3[30], x3[25]);
        x4[26] = _mm256_sub_epi32(x3[29], x3[26]);
        x4[29] = _mm256_add_epi32(x3[29], x3[26]);
        x4[27] = _mm256_sub_epi32(x3[28], x3[27]);
        x4[28] = _mm256_add_epi32(x3[28], x3[27]);
        x4[32] = x3[32];
        x4[33] = x3[33];
        x4[34] = x3[34];
        x4[35] = x3[35];
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x3[36], x3[59], x4[36], x4[59], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x3[37], x3[58], x4[37], x4[58], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x3[38], x3[57], x4[38], x4[57], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x3[39], x3[56], x4[39], x4[56], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x3[40], x3[55], x4[40], x4[55], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x3[41], x3[54], x4[41], x4[54], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x3[42], x3[53], x4[42], x4[53], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x3[43], x3[52], x4[43], x4[52], __rounding, cos_bit);
        x4[44] = x3[44];
        x4[45] = x3[45];
        x4[46] = x3[46];
        x4[47] = x3[47];
        x4[48] = x3[48];
        x4[49] = x3[49];
        x4[50] = x3[50];
        x4[51] = x3[51];
        x4[60] = x3[60];
        x4[61] = x3[61];
        x4[62] = x3[62];
        x4[63] = x3[63];

        // stage 5
        __m256i x5[64];
        x5[0] = _mm256_add_epi32(x4[0], x4[3]);
        x5[3] = _mm256_sub_epi32(x4[0], x4[3]);
        x5[1] = _mm256_add_epi32(x4[1], x4[2]);
        x5[2] = _mm256_sub_epi32(x4[1], x4[2]);
        x5[4] = x4[4];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x4[5], x4[6], x5[5], x5[6], __rounding, cos_bit);
        x5[7]  = x4[7];
        x5[8]  = _mm256_add_epi32(x4[8], x4[11]);
        x5[11] = _mm256_sub_epi32(x4[8], x4[11]);
        x5[9]  = _mm256_add_epi32(x4[9], x4[10]);
        x5[10] = _mm256_sub_epi32(x4[9], x4[10]);
        x5[12] = _mm256_sub_epi32(x4[15], x4[12]);
        x5[15] = _mm256_add_epi32(x4[15], x4[12]);
        x5[13] = _mm256_sub_epi32(x4[14], x4[13]);
        x5[14] = _mm256_add_epi32(x4[14], x4[13]);
        x5[16] = x4[16];
        x5[17] = x4[17];
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x4[18], x4[29], x5[18], x5[29], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x4[19], x4[28], x5[19], x5[28], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x4[20], x4[27], x5[20], x5[27], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x4[21], x4[26], x5[21], x5[26], __rounding, cos_bit);
        x5[22] = x4[22];
        x5[23] = x4[23];
        x5[24] = x4[24];
        x5[25] = x4[25];
        x5[30] = x4[30];
        x5[31] = x4[31];
        x5[32] = _mm256_add_epi32(x4[32], x4[39]);
        x5[39] = _mm256_sub_epi32(x4[32], x4[39]);
        x5[33] = _mm256_add_epi32(x4[33], x4[38]);
        x5[38] = _mm256_sub_epi32(x4[33], x4[38]);
        x5[34] = _mm256_add_epi32(x4[34], x4[37]);
        x5[37] = _mm256_sub_epi32(x4[34], x4[37]);
        x5[35] = _mm256_add_epi32(x4[35], x4[36]);
        x5[36] = _mm256_sub_epi32(x4[35], x4[36]);
        x5[40] = _mm256_sub_epi32(x4[47], x4[40]);
        x5[47] = _mm256_add_epi32(x4[47], x4[40]);
        x5[41] = _mm256_sub_epi32(x4[46], x4[41]);
        x5[46] = _mm256_add_epi32(x4[46], x4[41]);
        x5[42] = _mm256_sub_epi32(x4[45], x4[42]);
        x5[45] = _mm256_add_epi32(x4[45], x4[42]);
        x5[43] = _mm256_sub_epi32(x4[44], x4[43]);
        x5[44] = _mm256_add_epi32(x4[44], x4[43]);
        x5[48] = _mm256_add_epi32(x4[48], x4[55]);
        x5[55] = _mm256_sub_epi32(x4[48], x4[55]);
        x5[49] = _mm256_add_epi32(x4[49], x4[54]);
        x5[54] = _mm256_sub_epi32(x4[49], x4[54]);
        x5[50] = _mm256_add_epi32(x4[50], x4[53]);
        x5[53] = _mm256_sub_epi32(x4[50], x4[53]);
        x5[51] = _mm256_add_epi32(x4[51], x4[52]);
        x5[52] = _mm256_sub_epi32(x4[51], x4[52]);
        x5[56] = _mm256_sub_epi32(x4[63], x4[56]);
        x5[63] = _mm256_add_epi32(x4[63], x4[56]);
        x5[57] = _mm256_sub_epi32(x4[62], x4[57]);
        x5[62] = _mm256_add_epi32(x4[62], x4[57]);
        x5[58] = _mm256_sub_epi32(x4[61], x4[58]);
        x5[61] = _mm256_add_epi32(x4[61], x4[58]);
        x5[59] = _mm256_sub_epi32(x4[60], x4[59]);
        x5[60] = _mm256_add_epi32(x4[60], x4[59]);

        // stage 6
        __m256i x6[64];
        x6[0] = half_btf_avx2(&cospi_p32, &x5[0], &cospi_p32, &x5[1], &__rounding, cos_bit);
        x6[2] = half_btf_avx2(&cospi_p48, &x5[2], &cospi_p16, &x5[3], &__rounding, cos_bit);
        x6[4] = _mm256_add_epi32(x5[4], x5[5]);
        x6[5] = _mm256_sub_epi32(x5[4], x5[5]);
        x6[6] = _mm256_sub_epi32(x5[7], x5[6]);
        x6[7] = _mm256_add_epi32(x5[7], x5[6]);
        x6[8] = x5[8];
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x5[9], x5[14], x6[9], x6[14], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x5[10], x5[13], x6[10], x6[13], __rounding, cos_bit);
        x6[11] = x5[11];
        x6[12] = x5[12];
        x6[15] = x5[15];
        x6[16] = _mm256_add_epi32(x5[16], x5[19]);
        x6[19] = _mm256_sub_epi32(x5[16], x5[19]);
        x6[17] = _mm256_add_epi32(x5[17], x5[18]);
        x6[18] = _mm256_sub_epi32(x5[17], x5[18]);
        x6[20] = _mm256_sub_epi32(x5[23], x5[20]);
        x6[23] = _mm256_add_epi32(x5[23], x5[20]);
        x6[21] = _mm256_sub_epi32(x5[22], x5[21]);
        x6[22] = _mm256_add_epi32(x5[22], x5[21]);
        x6[24] = _mm256_add_epi32(x5[24], x5[27]);
        x6[27] = _mm256_sub_epi32(x5[24], x5[27]);
        x6[25] = _mm256_add_epi32(x5[25], x5[26]);
        x6[26] = _mm256_sub_epi32(x5[25], x5[26]);
        x6[28] = _mm256_sub_epi32(x5[31], x5[28]);
        x6[31] = _mm256_add_epi32(x5[31], x5[28]);
        x6[29] = _mm256_sub_epi32(x5[30], x5[29]);
        x6[30] = _mm256_add_epi32(x5[30], x5[29]);
        x6[32] = x5[32];
        x6[33] = x5[33];
        btf_32_type0_avx2_new(cospi_m08, cospi_p56, x5[34], x5[61], x6[34], x6[61], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m08, cospi_p56, x5[35], x5[60], x6[35], x6[60], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m56, cospi_m08, x5[36], x5[59], x6[36], x6[59], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m56, cospi_m08, x5[37], x5[58], x6[37], x6[58], __rounding, cos_bit);
        x6[38] = x5[38];
        x6[39] = x5[39];
        x6[40] = x5[40];
        x6[41] = x5[41];
        btf_32_type0_avx2_new(cospi_m40, cospi_p24, x5[42], x5[53], x6[42], x6[53], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m40, cospi_p24, x5[43], x5[52], x6[43], x6[52], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m24, cospi_m40, x5[44], x5[51], x6[44], x6[51], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m24, cospi_m40, x5[45], x5[50], x6[45], x6[50], __rounding, cos_bit);
        x6[46] = x5[46];
        x6[47] = x5[47];
        x6[48] = x5[48];
        x6[49] = x5[49];
        x6[54] = x5[54];
        x6[55] = x5[55];
        x6[56] = x5[56];
        x6[57] = x5[57];
        x6[62] = x5[62];
        x6[63] = x5[63];

        // stage 7
        __m256i x7[64];
        x7[0] = x6[0];
        //x7[1] = x6[1];
        x7[2] = x6[2];
        //x7[3] = x6[3];
        x7[4]  = half_btf_avx2(&cospi_p56, &x6[4], &cospi_p08, &x6[7], &__rounding, cos_bit);
        x7[6]  = half_btf_avx2(&cospi_p24, &x6[6], &cospi_m40, &x6[5], &__rounding, cos_bit);
        x7[8]  = _mm256_add_epi32(x6[8], x6[9]);
        x7[9]  = _mm256_sub_epi32(x6[8], x6[9]);
        x7[10] = _mm256_sub_epi32(x6[11], x6[10]);
        x7[11] = _mm256_add_epi32(x6[11], x6[10]);
        x7[12] = _mm256_add_epi32(x6[12], x6[13]);
        x7[13] = _mm256_sub_epi32(x6[12], x6[13]);
        x7[14] = _mm256_sub_epi32(x6[15], x6[14]);
        x7[15] = _mm256_add_epi32(x6[15], x6[14]);
        x7[16] = x6[16];
        btf_32_type0_avx2_new(cospi_m08, cospi_p56, x6[17], x6[30], x7[17], x7[30], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m56, cospi_m08, x6[18], x6[29], x7[18], x7[29], __rounding, cos_bit);
        x7[19] = x6[19];
        x7[20] = x6[20];
        btf_32_type0_avx2_new(cospi_m40, cospi_p24, x6[21], x6[26], x7[21], x7[26], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m24, cospi_m40, x6[22], x6[25], x7[22], x7[25], __rounding, cos_bit);
        x7[23] = x6[23];
        x7[24] = x6[24];
        x7[27] = x6[27];
        x7[28] = x6[28];
        x7[31] = x6[31];
        x7[32] = _mm256_add_epi32(x6[32], x6[35]);
        x7[35] = _mm256_sub_epi32(x6[32], x6[35]);
        x7[33] = _mm256_add_epi32(x6[33], x6[34]);
        x7[34] = _mm256_sub_epi32(x6[33], x6[34]);
        x7[36] = _mm256_sub_epi32(x6[39], x6[36]);
        x7[39] = _mm256_add_epi32(x6[39], x6[36]);
        x7[37] = _mm256_sub_epi32(x6[38], x6[37]);
        x7[38] = _mm256_add_epi32(x6[38], x6[37]);
        x7[40] = _mm256_add_epi32(x6[40], x6[43]);
        x7[43] = _mm256_sub_epi32(x6[40], x6[43]);
        x7[41] = _mm256_add_epi32(x6[41], x6[42]);
        x7[42] = _mm256_sub_epi32(x6[41], x6[42]);
        x7[44] = _mm256_sub_epi32(x6[47], x6[44]);
        x7[47] = _mm256_add_epi32(x6[47], x6[44]);
        x7[45] = _mm256_sub_epi32(x6[46], x6[45]);
        x7[46] = _mm256_add_epi32(x6[46], x6[45]);
        x7[48] = _mm256_add_epi32(x6[48], x6[51]);
        x7[51] = _mm256_sub_epi32(x6[48], x6[51]);
        x7[49] = _mm256_add_epi32(x6[49], x6[50]);
        x7[50] = _mm256_sub_epi32(x6[49], x6[50]);
        x7[52] = _mm256_sub_epi32(x6[55], x6[52]);
        x7[55] = _mm256_add_epi32(x6[55], x6[52]);
        x7[53] = _mm256_sub_epi32(x6[54], x6[53]);
        x7[54] = _mm256_add_epi32(x6[54], x6[53]);
        x7[56] = _mm256_add_epi32(x6[56], x6[59]);
        x7[59] = _mm256_sub_epi32(x6[56], x6[59]);
        x7[57] = _mm256_add_epi32(x6[57], x6[58]);
        x7[58] = _mm256_sub_epi32(x6[57], x6[58]);
        x7[60] = _mm256_sub_epi32(x6[63], x6[60]);
        x7[63] = _mm256_add_epi32(x6[63], x6[60]);
        x7[61] = _mm256_sub_epi32(x6[62], x6[61]);
        x7[62] = _mm256_add_epi32(x6[62], x6[61]);

        // stage 8
        __m256i x8[64];
        x8[0] = x7[0];
        x8[2] = x7[2];
        x8[4] = x7[4];
        x8[6] = x7[6];

        x8[8]  = half_btf_avx2(&cospi_p60, &x7[8], &cospi_p04, &x7[15], &__rounding, cos_bit);
        x8[14] = half_btf_avx2(&cospi_p28, &x7[14], &cospi_m36, &x7[9], &__rounding, cos_bit);
        x8[10] = half_btf_avx2(&cospi_p44, &x7[10], &cospi_p20, &x7[13], &__rounding, cos_bit);
        x8[12] = half_btf_avx2(&cospi_p12, &x7[12], &cospi_m52, &x7[11], &__rounding, cos_bit);
        x8[16] = _mm256_add_epi32(x7[16], x7[17]);
        x8[17] = _mm256_sub_epi32(x7[16], x7[17]);
        x8[18] = _mm256_sub_epi32(x7[19], x7[18]);
        x8[19] = _mm256_add_epi32(x7[19], x7[18]);
        x8[20] = _mm256_add_epi32(x7[20], x7[21]);
        x8[21] = _mm256_sub_epi32(x7[20], x7[21]);
        x8[22] = _mm256_sub_epi32(x7[23], x7[22]);
        x8[23] = _mm256_add_epi32(x7[23], x7[22]);
        x8[24] = _mm256_add_epi32(x7[24], x7[25]);
        x8[25] = _mm256_sub_epi32(x7[24], x7[25]);
        x8[26] = _mm256_sub_epi32(x7[27], x7[26]);
        x8[27] = _mm256_add_epi32(x7[27], x7[26]);
        x8[28] = _mm256_add_epi32(x7[28], x7[29]);
        x8[29] = _mm256_sub_epi32(x7[28], x7[29]);
        x8[30] = _mm256_sub_epi32(x7[31], x7[30]);
        x8[31] = _mm256_add_epi32(x7[31], x7[30]);
        x8[32] = x7[32];
        btf_32_type0_avx2_new(cospi_m04, cospi_p60, x7[33], x7[62], x8[33], x8[62], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m60, cospi_m04, x7[34], x7[61], x8[34], x8[61], __rounding, cos_bit);
        x8[35] = x7[35];
        x8[36] = x7[36];
        btf_32_type0_avx2_new(cospi_m36, cospi_p28, x7[37], x7[58], x8[37], x8[58], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m28, cospi_m36, x7[38], x7[57], x8[38], x8[57], __rounding, cos_bit);
        x8[39] = x7[39];
        x8[40] = x7[40];
        btf_32_type0_avx2_new(cospi_m20, cospi_p44, x7[41], x7[54], x8[41], x8[54], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m44, cospi_m20, x7[42], x7[53], x8[42], x8[53], __rounding, cos_bit);
        x8[43] = x7[43];
        x8[44] = x7[44];
        btf_32_type0_avx2_new(cospi_m52, cospi_p12, x7[45], x7[50], x8[45], x8[50], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m12, cospi_m52, x7[46], x7[49], x8[46], x8[49], __rounding, cos_bit);
        x8[47] = x7[47];
        x8[48] = x7[48];
        x8[51] = x7[51];
        x8[52] = x7[52];
        x8[55] = x7[55];
        x8[56] = x7[56];
        x8[59] = x7[59];
        x8[60] = x7[60];
        x8[63] = x7[63];

        // stage 9
        __m256i x9[64];
        x9[0]  = x8[0];
        x9[2]  = x8[2];
        x9[4]  = x8[4];
        x9[6]  = x8[6];
        x9[8]  = x8[8];
        x9[10] = x8[10];
        x9[12] = x8[12];
        x9[14] = x8[14];
        x9[16] = half_btf_avx2(&cospi_p62, &x8[16], &cospi_p02, &x8[31], &__rounding, cos_bit);
        x9[30] = half_btf_avx2(&cospi_p30, &x8[30], &cospi_m34, &x8[17], &__rounding, cos_bit);
        x9[18] = half_btf_avx2(&cospi_p46, &x8[18], &cospi_p18, &x8[29], &__rounding, cos_bit);
        x9[28] = half_btf_avx2(&cospi_p14, &x8[28], &cospi_m50, &x8[19], &__rounding, cos_bit);
        x9[20] = half_btf_avx2(&cospi_p54, &x8[20], &cospi_p10, &x8[27], &__rounding, cos_bit);
        x9[26] = half_btf_avx2(&cospi_p22, &x8[26], &cospi_m42, &x8[21], &__rounding, cos_bit);
        x9[22] = half_btf_avx2(&cospi_p38, &x8[22], &cospi_p26, &x8[25], &__rounding, cos_bit);
        x9[24] = half_btf_avx2(&cospi_p06, &x8[24], &cospi_m58, &x8[23], &__rounding, cos_bit);
        x9[32] = _mm256_add_epi32(x8[32], x8[33]);
        x9[33] = _mm256_sub_epi32(x8[32], x8[33]);
        x9[34] = _mm256_sub_epi32(x8[35], x8[34]);
        x9[35] = _mm256_add_epi32(x8[35], x8[34]);
        x9[36] = _mm256_add_epi32(x8[36], x8[37]);
        x9[37] = _mm256_sub_epi32(x8[36], x8[37]);
        x9[38] = _mm256_sub_epi32(x8[39], x8[38]);
        x9[39] = _mm256_add_epi32(x8[39], x8[38]);
        x9[40] = _mm256_add_epi32(x8[40], x8[41]);
        x9[41] = _mm256_sub_epi32(x8[40], x8[41]);
        x9[42] = _mm256_sub_epi32(x8[43], x8[42]);
        x9[43] = _mm256_add_epi32(x8[43], x8[42]);
        x9[44] = _mm256_add_epi32(x8[44], x8[45]);
        x9[45] = _mm256_sub_epi32(x8[44], x8[45]);
        x9[46] = _mm256_sub_epi32(x8[47], x8[46]);
        x9[47] = _mm256_add_epi32(x8[47], x8[46]);
        x9[48] = _mm256_add_epi32(x8[48], x8[49]);
        x9[49] = _mm256_sub_epi32(x8[48], x8[49]);
        x9[50] = _mm256_sub_epi32(x8[51], x8[50]);
        x9[51] = _mm256_add_epi32(x8[51], x8[50]);
        x9[52] = _mm256_add_epi32(x8[52], x8[53]);
        x9[53] = _mm256_sub_epi32(x8[52], x8[53]);
        x9[54] = _mm256_sub_epi32(x8[55], x8[54]);
        x9[55] = _mm256_add_epi32(x8[55], x8[54]);
        x9[56] = _mm256_add_epi32(x8[56], x8[57]);
        x9[57] = _mm256_sub_epi32(x8[56], x8[57]);
        x9[58] = _mm256_sub_epi32(x8[59], x8[58]);
        x9[59] = _mm256_add_epi32(x8[59], x8[58]);
        x9[60] = _mm256_add_epi32(x8[60], x8[61]);
        x9[61] = _mm256_sub_epi32(x8[60], x8[61]);
        x9[62] = _mm256_sub_epi32(x8[63], x8[62]);
        x9[63] = _mm256_add_epi32(x8[63], x8[62]);

        // stage 10
        __m256i x10[64];
        out[0 * stride]  = x9[0];
        out[16 * stride] = x9[2];
        out[8 * stride]  = x9[4];
        out[24 * stride] = x9[6];
        out[4 * stride]  = x9[8];
        out[20 * stride] = x9[10];
        out[12 * stride] = x9[12];
        out[28 * stride] = x9[14];
        out[2 * stride]  = x9[16];
        out[18 * stride] = x9[18];
        out[10 * stride] = x9[20];
        out[26 * stride] = x9[22];
        out[6 * stride]  = x9[24];
        out[22 * stride] = x9[26];
        out[14 * stride] = x9[28];
        out[30 * stride] = x9[30];
        x10[32]          = half_btf_avx2(&cospi_p63, &x9[32], &cospi_p01, &x9[63], &__rounding, cos_bit);
        x10[62]          = half_btf_avx2(&cospi_p31, &x9[62], &cospi_m33, &x9[33], &__rounding, cos_bit);
        x10[34]          = half_btf_avx2(&cospi_p47, &x9[34], &cospi_p17, &x9[61], &__rounding, cos_bit);
        x10[60]          = half_btf_avx2(&cospi_p15, &x9[60], &cospi_m49, &x9[35], &__rounding, cos_bit);
        x10[36]          = half_btf_avx2(&cospi_p55, &x9[36], &cospi_p09, &x9[59], &__rounding, cos_bit);
        x10[58]          = half_btf_avx2(&cospi_p23, &x9[58], &cospi_m41, &x9[37], &__rounding, cos_bit);
        x10[38]          = half_btf_avx2(&cospi_p39, &x9[38], &cospi_p25, &x9[57], &__rounding, cos_bit);
        x10[56]          = half_btf_avx2(&cospi_p07, &x9[56], &cospi_m57, &x9[39], &__rounding, cos_bit);
        x10[40]          = half_btf_avx2(&cospi_p59, &x9[40], &cospi_p05, &x9[55], &__rounding, cos_bit);
        x10[54]          = half_btf_avx2(&cospi_p27, &x9[54], &cospi_m37, &x9[41], &__rounding, cos_bit);
        x10[42]          = half_btf_avx2(&cospi_p43, &x9[42], &cospi_p21, &x9[53], &__rounding, cos_bit);
        x10[52]          = half_btf_avx2(&cospi_p11, &x9[52], &cospi_m53, &x9[43], &__rounding, cos_bit);
        x10[44]          = half_btf_avx2(&cospi_p51, &x9[44], &cospi_p13, &x9[51], &__rounding, cos_bit);
        x10[50]          = half_btf_avx2(&cospi_p19, &x9[50], &cospi_m45, &x9[45], &__rounding, cos_bit);
        x10[46]          = half_btf_avx2(&cospi_p35, &x9[46], &cospi_p29, &x9[49], &__rounding, cos_bit);
        x10[48]          = half_btf_avx2(&cospi_p03, &x9[48], &cospi_m61, &x9[47], &__rounding, cos_bit);

        // stage 11
        out[1 * stride]  = x10[32];
        out[3 * stride]  = x10[48];
        out[5 * stride]  = x10[40];
        out[7 * stride]  = x10[56];
        out[9 * stride]  = x10[36];
        out[11 * stride] = x10[52];
        out[13 * stride] = x10[44];
        out[15 * stride] = x10[60];
        out[17 * stride] = x10[34];
        out[19 * stride] = x10[50];
        out[21 * stride] = x10[42];
        out[23 * stride] = x10[58];
        out[25 * stride] = x10[38];
        out[27 * stride] = x10[54];
        out[29 * stride] = x10[46];
        out[31 * stride] = x10[62];
    }
}

static void fidtx64x64_N2_avx2(const __m256i* input, __m256i* output) {
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 4 * 5793; // 4 * new_sqrt2
    const __m256i newsqrt  = _mm256_set1_epi32(sqrt);
    const __m256i rounding = _mm256_set1_epi32(1 << (bits - 1));

    __m256i temp;
    for (int32_t i = 0; i < 256; i += 8) {
        temp          = _mm256_mullo_epi32(input[i], newsqrt);
        temp          = _mm256_add_epi32(temp, rounding);
        output[i]     = _mm256_srai_epi32(temp, bits);
        temp          = _mm256_mullo_epi32(input[i + 1], newsqrt);
        temp          = _mm256_add_epi32(temp, rounding);
        output[i + 1] = _mm256_srai_epi32(temp, bits);
        temp          = _mm256_mullo_epi32(input[i + 2], newsqrt);
        temp          = _mm256_add_epi32(temp, rounding);
        output[i + 2] = _mm256_srai_epi32(temp, bits);
        temp          = _mm256_mullo_epi32(input[i + 3], newsqrt);
        temp          = _mm256_add_epi32(temp, rounding);
        output[i + 3] = _mm256_srai_epi32(temp, bits);
    }
}

static INLINE void av1_round_shift_array_64_N2_avx2(__m256i* input, __m256i* output, const int32_t size,
                                                    const int32_t bit) {
    int32_t i;
    if (bit > 0) {
        const __m256i round = _mm256_set1_epi32(1 << (bit - 1));
        __m256i       r0;
        for (i = 0; i < size; i += 8) {
            r0            = _mm256_add_epi32(input[i], round);
            output[i]     = _mm256_srai_epi32(r0, bit);
            r0            = _mm256_add_epi32(input[i + 1], round);
            output[i + 1] = _mm256_srai_epi32(r0, bit);
            r0            = _mm256_add_epi32(input[i + 2], round);
            output[i + 2] = _mm256_srai_epi32(r0, bit);
            r0            = _mm256_add_epi32(input[i + 3], round);
            output[i + 3] = _mm256_srai_epi32(r0, bit);
        }
    } else {
        for (i = 0; i < size; i += 8) {
            output[i]     = _mm256_slli_epi32(input[i], -bit);
            output[i + 1] = _mm256_slli_epi32(input[i + 1], -bit);
            output[i + 2] = _mm256_slli_epi32(input[i + 2], -bit);
            output[i + 3] = _mm256_slli_epi32(input[i + 3], -bit);
        }
    }
}

static void av1_idtx16_new_avx2(const __m256i* input, __m256i* output, int8_t cos_bit, const int32_t col_num) {
    (void)cos_bit;
    for (int32_t i = 0; i < 16; i++) {
        output[i * col_num] = _mm256_slli_epi32(input[i * col_num], 2);
    }
}

static void fidtx32x8_N2_avx2(const __m256i* input, __m256i* output, int8_t cos_bit, const int32_t col_num,
                              int32_t row_num) {
    (void)cos_bit;
    for (int32_t i = 0; i < row_num; i++) {
        output[i * col_num] = _mm256_slli_epi32(input[i * col_num], 1);
    }
}

static void fidtx32x16_N2_avx2(const __m256i* in, __m256i* out, int8_t bit, int32_t rows) {
    (void)bit;
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 2 * 5793; // 2 * new_sqrt2
    const __m256i newsqrt  = _mm256_set1_epi32(sqrt);
    const __m256i rounding = _mm256_set1_epi32(1 << (bits - 1));
    __m256i       temp;

    for (int32_t i = 0; i < rows; i++) {
        temp           = _mm256_mullo_epi32(in[i * 4], newsqrt);
        temp           = _mm256_add_epi32(temp, rounding);
        out[i * 4]     = _mm256_srai_epi32(temp, bits);
        temp           = _mm256_mullo_epi32(in[i * 4 + 1], newsqrt);
        temp           = _mm256_add_epi32(temp, rounding);
        out[i * 4 + 1] = _mm256_srai_epi32(temp, bits);
    }
}

static void av1_idtx32x16_N2_avx2(const __m256i* input, __m256i* output, const int32_t rows) {
    for (int32_t i = 0; i < rows; i++) {
        output[i * 4]     = _mm256_slli_epi32(input[i * 4], 2);
        output[i * 4 + 1] = _mm256_slli_epi32(input[i * 4 + 1], 2);
    }
}

static AOM_FORCE_INLINE void col_txfm_32x16_N2_rounding(__m256i* in, int32_t shift) {
    const __m256i rounding = _mm256_set1_epi32(1 << (shift - 1));
    in[0]                  = _mm256_add_epi32(in[0], rounding);
    in[4]                  = _mm256_add_epi32(in[4], rounding);
    in[8]                  = _mm256_add_epi32(in[8], rounding);
    in[12]                 = _mm256_add_epi32(in[12], rounding);
    in[0]                  = _mm256_srai_epi32(in[0], shift);
    in[4]                  = _mm256_srai_epi32(in[4], shift);
    in[8]                  = _mm256_srai_epi32(in[8], shift);
    in[12]                 = _mm256_srai_epi32(in[12], shift);
    in[16]                 = _mm256_add_epi32(in[16], rounding);
    in[20]                 = _mm256_add_epi32(in[20], rounding);
    in[24]                 = _mm256_add_epi32(in[24], rounding);
    in[28]                 = _mm256_add_epi32(in[28], rounding);
    in[16]                 = _mm256_srai_epi32(in[16], shift);
    in[20]                 = _mm256_srai_epi32(in[20], shift);
    in[24]                 = _mm256_srai_epi32(in[24], shift);
    in[28]                 = _mm256_srai_epi32(in[28], shift);
}

static void fdct16x4_N2_avx2(__m256i* input, __m256i* output, int32_t bit) {
    __m128i* in  = (__m128i*)input;
    __m128i* out = (__m128i*)output;

    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospi4   = _mm_set1_epi32(cospi[4]);
    const __m128i  cospi28  = _mm_set1_epi32(cospi[28]);
    const __m128i  cospi36  = _mm_set1_epi32(cospi[36]);
    const __m128i  cospi44  = _mm_set1_epi32(cospi[44]);
    const __m128i  cospi20  = _mm_set1_epi32(cospi[20]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[16], v[16], x;

    // stage 0
    // stage 1
    u[0]  = _mm_add_epi32(in[0], in[15]);
    v[15] = _mm_sub_epi32(in[0], in[15]);
    u[1]  = _mm_add_epi32(in[1], in[14]);
    v[14] = _mm_sub_epi32(in[1], in[14]);
    u[2]  = _mm_add_epi32(in[2], in[13]);
    u[13] = _mm_sub_epi32(in[2], in[13]);
    u[3]  = _mm_add_epi32(in[3], in[12]);
    u[12] = _mm_sub_epi32(in[3], in[12]);
    u[4]  = _mm_add_epi32(in[4], in[11]);
    u[11] = _mm_sub_epi32(in[4], in[11]);
    u[5]  = _mm_add_epi32(in[5], in[10]);
    u[10] = _mm_sub_epi32(in[5], in[10]);
    u[6]  = _mm_add_epi32(in[6], in[9]);
    v[9]  = _mm_sub_epi32(in[6], in[9]);
    u[7]  = _mm_add_epi32(in[7], in[8]);
    v[8]  = _mm_sub_epi32(in[7], in[8]);

    // stage 2
    v[0] = _mm_add_epi32(u[0], u[7]);
    u[7] = _mm_sub_epi32(u[0], u[7]);
    v[1] = _mm_add_epi32(u[1], u[6]);
    v[6] = _mm_sub_epi32(u[1], u[6]);
    v[2] = _mm_add_epi32(u[2], u[5]);
    v[5] = _mm_sub_epi32(u[2], u[5]);
    v[3] = _mm_add_epi32(u[3], u[4]);
    u[4] = _mm_sub_epi32(u[3], u[4]);

    v[10] = _mm_mullo_epi32(u[10], cospim32);
    x     = _mm_mullo_epi32(u[13], cospi32);
    v[10] = _mm_add_epi32(v[10], x);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[13] = _mm_mullo_epi32(u[10], cospi32);
    x     = _mm_mullo_epi32(u[13], cospim32);
    v[13] = _mm_sub_epi32(v[13], x);
    v[13] = _mm_add_epi32(v[13], rnding);
    v[13] = _mm_srai_epi32(v[13], bit);

    v[11] = _mm_mullo_epi32(u[11], cospim32);
    x     = _mm_mullo_epi32(u[12], cospi32);
    v[11] = _mm_add_epi32(v[11], x);
    v[11] = _mm_add_epi32(v[11], rnding);
    v[11] = _mm_srai_epi32(v[11], bit);

    v[12] = _mm_mullo_epi32(u[11], cospi32);
    x     = _mm_mullo_epi32(u[12], cospim32);
    v[12] = _mm_sub_epi32(v[12], x);
    v[12] = _mm_add_epi32(v[12], rnding);
    v[12] = _mm_srai_epi32(v[12], bit);

    // stage 3
    u[0] = _mm_add_epi32(v[0], v[3]);
    u[3] = _mm_sub_epi32(v[0], v[3]);
    u[1] = _mm_add_epi32(v[1], v[2]);
    u[2] = _mm_sub_epi32(v[1], v[2]);

    u[5] = _mm_mullo_epi32(v[5], cospim32);
    x    = _mm_mullo_epi32(v[6], cospi32);
    u[5] = _mm_add_epi32(u[5], x);
    u[5] = _mm_add_epi32(u[5], rnding);
    u[5] = _mm_srai_epi32(u[5], bit);

    u[6] = _mm_mullo_epi32(v[5], cospi32);
    x    = _mm_mullo_epi32(v[6], cospim32);
    u[6] = _mm_sub_epi32(u[6], x);
    u[6] = _mm_add_epi32(u[6], rnding);
    u[6] = _mm_srai_epi32(u[6], bit);

    u[8]  = _mm_add_epi32(v[8], v[11]);
    v[11] = _mm_sub_epi32(v[8], v[11]);
    u[9]  = _mm_add_epi32(v[9], v[10]);
    u[10] = _mm_sub_epi32(v[9], v[10]);
    u[12] = _mm_sub_epi32(v[15], v[12]);
    v[15] = _mm_add_epi32(v[15], v[12]);
    u[13] = _mm_sub_epi32(v[14], v[13]);
    u[14] = _mm_add_epi32(v[14], v[13]);

    // stage 4
    u[0]   = _mm_mullo_epi32(u[0], cospi32);
    u[1]   = _mm_mullo_epi32(u[1], cospi32);
    v[0]   = _mm_add_epi32(u[0], u[1]);
    v[0]   = _mm_add_epi32(v[0], rnding);
    out[0] = _mm_srai_epi32(v[0], bit);

    v[2]   = _mm_mullo_epi32(u[2], cospi48);
    x      = _mm_mullo_epi32(u[3], cospi16);
    v[2]   = _mm_add_epi32(v[2], x);
    v[2]   = _mm_add_epi32(v[2], rnding);
    out[4] = _mm_srai_epi32(v[2], bit);

    v[4] = _mm_add_epi32(u[4], u[5]);
    v[5] = _mm_sub_epi32(u[4], u[5]);
    v[6] = _mm_sub_epi32(u[7], u[6]);
    v[7] = _mm_add_epi32(u[7], u[6]);
    v[8] = u[8];

    v[9] = _mm_mullo_epi32(u[9], cospim16);
    x    = _mm_mullo_epi32(u[14], cospi48);
    v[9] = _mm_add_epi32(v[9], x);
    v[9] = _mm_add_epi32(v[9], rnding);
    v[9] = _mm_srai_epi32(v[9], bit);

    v[14] = _mm_mullo_epi32(u[9], cospi48);
    x     = _mm_mullo_epi32(u[14], cospim16);
    v[14] = _mm_sub_epi32(v[14], x);
    v[14] = _mm_add_epi32(v[14], rnding);
    v[14] = _mm_srai_epi32(v[14], bit);

    v[10] = _mm_mullo_epi32(u[10], cospim48);
    x     = _mm_mullo_epi32(u[13], cospim16);
    v[10] = _mm_add_epi32(v[10], x);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[13] = _mm_mullo_epi32(u[10], cospim16);
    x     = _mm_mullo_epi32(u[13], cospim48);
    v[13] = _mm_sub_epi32(v[13], x);
    v[13] = _mm_add_epi32(v[13], rnding);
    v[13] = _mm_srai_epi32(v[13], bit);

    v[12] = u[12];

    // stage 5
    u[4]   = _mm_mullo_epi32(v[4], cospi56);
    x      = _mm_mullo_epi32(v[7], cospi8);
    u[4]   = _mm_add_epi32(u[4], x);
    u[4]   = _mm_add_epi32(u[4], rnding);
    out[2] = _mm_srai_epi32(u[4], bit);

    u[6]   = _mm_mullo_epi32(v[5], cospi40);
    x      = _mm_mullo_epi32(v[6], cospi24);
    u[6]   = _mm_sub_epi32(x, u[6]);
    u[6]   = _mm_add_epi32(u[6], rnding);
    out[6] = _mm_srai_epi32(u[6], bit);

    u[8]  = _mm_add_epi32(v[8], v[9]);
    u[9]  = _mm_sub_epi32(v[8], v[9]);
    u[10] = _mm_sub_epi32(v[11], v[10]);
    u[11] = _mm_add_epi32(v[11], v[10]);
    u[12] = _mm_add_epi32(v[12], v[13]);
    u[13] = _mm_sub_epi32(v[12], v[13]);
    u[14] = _mm_sub_epi32(v[15], v[14]);
    u[15] = _mm_add_epi32(v[15], v[14]);

    // stage 6
    v[8]   = _mm_mullo_epi32(u[8], cospi60);
    x      = _mm_mullo_epi32(u[15], cospi4);
    v[8]   = _mm_add_epi32(v[8], x);
    v[8]   = _mm_add_epi32(v[8], rnding);
    out[1] = _mm_srai_epi32(v[8], bit);

    v[14]  = _mm_mullo_epi32(u[9], cospi36);
    x      = _mm_mullo_epi32(u[14], cospi28);
    v[14]  = _mm_sub_epi32(x, v[14]);
    v[14]  = _mm_add_epi32(v[14], rnding);
    out[7] = _mm_srai_epi32(v[14], bit);

    v[10]  = _mm_mullo_epi32(u[10], cospi44);
    x      = _mm_mullo_epi32(u[13], cospi20);
    v[10]  = _mm_add_epi32(v[10], x);
    v[10]  = _mm_add_epi32(v[10], rnding);
    out[5] = _mm_srai_epi32(v[10], bit);

    v[12]  = _mm_mullo_epi32(u[11], cospi52);
    x      = _mm_mullo_epi32(u[12], cospi12);
    v[12]  = _mm_sub_epi32(x, v[12]);
    v[12]  = _mm_add_epi32(v[12], rnding);
    out[3] = _mm_srai_epi32(v[12], bit);
}

static void fadst16x4_N2_avx2(__m256i* input, __m256i* output, int32_t bit) {
    __m128i* in  = (__m128i*)input;
    __m128i* out = (__m128i*)output;

    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospim56 = _mm_set1_epi32(-cospi[56]);
    const __m128i  cospim8  = _mm_set1_epi32(-cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospim24 = _mm_set1_epi32(-cospi[24]);
    const __m128i  cospim40 = _mm_set1_epi32(-cospi[40]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  cospi62  = _mm_set1_epi32(cospi[62]);
    const __m128i  cospim2  = _mm_set1_epi32(-cospi[2]);
    const __m128i  cospi54  = _mm_set1_epi32(cospi[54]);
    const __m128i  cospim10 = _mm_set1_epi32(-cospi[10]);
    const __m128i  cospi46  = _mm_set1_epi32(cospi[46]);
    const __m128i  cospim18 = _mm_set1_epi32(-cospi[18]);
    const __m128i  cospi38  = _mm_set1_epi32(cospi[38]);
    const __m128i  cospim26 = _mm_set1_epi32(-cospi[26]);
    const __m128i  cospi34  = _mm_set1_epi32(cospi[34]);
    const __m128i  cospi30  = _mm_set1_epi32(cospi[30]);
    const __m128i  cospi42  = _mm_set1_epi32(cospi[42]);
    const __m128i  cospi22  = _mm_set1_epi32(cospi[22]);
    const __m128i  cospi50  = _mm_set1_epi32(cospi[50]);
    const __m128i  cospi14  = _mm_set1_epi32(cospi[14]);
    const __m128i  cospi58  = _mm_set1_epi32(cospi[58]);
    const __m128i  cospi6   = _mm_set1_epi32(cospi[6]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();

    __m128i u[16], v[16], x, y;
    __m128i tmp[13];

    tmp[0] = _mm_sub_epi32(zero, in[15]);
    u[2]   = _mm_sub_epi32(zero, in[7]);
    tmp[1] = _mm_sub_epi32(zero, in[3]);
    u[7]   = _mm_sub_epi32(zero, in[11]);
    tmp[2] = _mm_sub_epi32(zero, in[1]);
    u[11]  = _mm_sub_epi32(zero, in[9]);
    tmp[3] = _mm_sub_epi32(zero, in[13]);
    u[14]  = _mm_sub_epi32(zero, in[5]);

    // stage 2

    x    = _mm_mullo_epi32(u[2], cospi32);
    y    = _mm_mullo_epi32(in[8], cospi32);
    v[2] = _mm_add_epi32(x, y);
    v[2] = _mm_add_epi32(v[2], rnding);
    v[2] = _mm_srai_epi32(v[2], bit);

    v[3] = _mm_sub_epi32(x, y);
    v[3] = _mm_add_epi32(v[3], rnding);
    v[3] = _mm_srai_epi32(v[3], bit);

    x    = _mm_mullo_epi32(in[4], cospi32);
    y    = _mm_mullo_epi32(u[7], cospi32);
    v[6] = _mm_add_epi32(x, y);
    v[6] = _mm_add_epi32(v[6], rnding);
    v[6] = _mm_srai_epi32(v[6], bit);

    v[7] = _mm_sub_epi32(x, y);
    v[7] = _mm_add_epi32(v[7], rnding);
    v[7] = _mm_srai_epi32(v[7], bit);

    x     = _mm_mullo_epi32(in[6], cospi32);
    y     = _mm_mullo_epi32(u[11], cospi32);
    v[10] = _mm_add_epi32(x, y);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[11] = _mm_sub_epi32(x, y);
    v[11] = _mm_add_epi32(v[11], rnding);
    v[11] = _mm_srai_epi32(v[11], bit);

    x     = _mm_mullo_epi32(u[14], cospi32);
    y     = _mm_mullo_epi32(in[10], cospi32);
    v[14] = _mm_add_epi32(x, y);
    v[14] = _mm_add_epi32(v[14], rnding);
    v[14] = _mm_srai_epi32(v[14], bit);

    v[15] = _mm_sub_epi32(x, y);
    v[15] = _mm_add_epi32(v[15], rnding);
    v[15] = _mm_srai_epi32(v[15], bit);

    // stage 3
    tmp[4] = _mm_add_epi32(in[0], v[2]);
    tmp[5] = _mm_add_epi32(tmp[0], v[3]);
    tmp[6] = _mm_sub_epi32(in[0], v[2]);
    tmp[0] = _mm_sub_epi32(tmp[0], v[3]);
    u[4]   = _mm_add_epi32(tmp[1], v[6]);
    u[5]   = _mm_add_epi32(in[12], v[7]);
    u[6]   = _mm_sub_epi32(tmp[1], v[6]);
    u[7]   = _mm_sub_epi32(in[12], v[7]);
    tmp[1] = _mm_add_epi32(tmp[2], v[10]);
    tmp[7] = _mm_add_epi32(in[14], v[11]);
    tmp[2] = _mm_sub_epi32(tmp[2], v[10]);
    tmp[8] = _mm_sub_epi32(in[14], v[11]);
    u[12]  = _mm_add_epi32(in[2], v[14]);
    u[13]  = _mm_add_epi32(tmp[3], v[15]);
    u[14]  = _mm_sub_epi32(in[2], v[14]);
    u[15]  = _mm_sub_epi32(tmp[3], v[15]);

    // stage 4
    v[4]  = half_btf_small(&cospi16, &u[4], &cospi48, &u[5], &rnding, bit);
    v[5]  = half_btf_small(&cospi48, &u[4], &cospim16, &u[5], &rnding, bit);
    v[6]  = half_btf_small(&cospim48, &u[6], &cospi16, &u[7], &rnding, bit);
    v[7]  = half_btf_small(&cospi16, &u[6], &cospi48, &u[7], &rnding, bit);
    v[12] = half_btf_small(&cospi16, &u[12], &cospi48, &u[13], &rnding, bit);
    v[13] = half_btf_small(&cospi48, &u[12], &cospim16, &u[13], &rnding, bit);
    v[14] = half_btf_small(&cospim48, &u[14], &cospi16, &u[15], &rnding, bit);
    v[15] = half_btf_small(&cospi16, &u[14], &cospi48, &u[15], &rnding, bit);

    // stage 5
    tmp[9]  = _mm_add_epi32(tmp[4], v[4]);
    tmp[10] = _mm_add_epi32(tmp[5], v[5]);
    tmp[11] = _mm_add_epi32(tmp[6], v[6]);
    tmp[12] = _mm_add_epi32(tmp[0], v[7]);
    tmp[4]  = _mm_sub_epi32(tmp[4], v[4]);
    tmp[5]  = _mm_sub_epi32(tmp[5], v[5]);
    tmp[6]  = _mm_sub_epi32(tmp[6], v[6]);
    tmp[0]  = _mm_sub_epi32(tmp[0], v[7]);
    u[8]    = _mm_add_epi32(tmp[1], v[12]);
    u[9]    = _mm_add_epi32(tmp[7], v[13]);
    u[10]   = _mm_add_epi32(tmp[2], v[14]);
    u[11]   = _mm_add_epi32(tmp[8], v[15]);
    u[12]   = _mm_sub_epi32(tmp[1], v[12]);
    u[13]   = _mm_sub_epi32(tmp[7], v[13]);
    u[14]   = _mm_sub_epi32(tmp[2], v[14]);
    u[15]   = _mm_sub_epi32(tmp[8], v[15]);

    // stage 6
    v[8]  = half_btf_small(&cospi8, &u[8], &cospi56, &u[9], &rnding, bit);
    v[9]  = half_btf_small(&cospi56, &u[8], &cospim8, &u[9], &rnding, bit);
    v[10] = half_btf_small(&cospi40, &u[10], &cospi24, &u[11], &rnding, bit);
    v[11] = half_btf_small(&cospi24, &u[10], &cospim40, &u[11], &rnding, bit);
    v[12] = half_btf_small(&cospim56, &u[12], &cospi8, &u[13], &rnding, bit);
    v[13] = half_btf_small(&cospi8, &u[12], &cospi56, &u[13], &rnding, bit);
    v[14] = half_btf_small(&cospim24, &u[14], &cospi40, &u[15], &rnding, bit);
    v[15] = half_btf_small(&cospi40, &u[14], &cospi24, &u[15], &rnding, bit);

    // stage 7
    u[0]  = _mm_add_epi32(tmp[9], v[8]);
    u[1]  = _mm_add_epi32(tmp[10], v[9]);
    u[2]  = _mm_add_epi32(tmp[11], v[10]);
    u[3]  = _mm_add_epi32(tmp[12], v[11]);
    u[4]  = _mm_add_epi32(tmp[4], v[12]);
    u[5]  = _mm_add_epi32(tmp[5], v[13]);
    u[6]  = _mm_add_epi32(tmp[6], v[14]);
    u[7]  = _mm_add_epi32(tmp[0], v[15]);
    u[8]  = _mm_sub_epi32(tmp[9], v[8]);
    u[9]  = _mm_sub_epi32(tmp[10], v[9]);
    u[10] = _mm_sub_epi32(tmp[11], v[10]);
    u[11] = _mm_sub_epi32(tmp[12], v[11]);
    u[12] = _mm_sub_epi32(tmp[4], v[12]);
    u[13] = _mm_sub_epi32(tmp[5], v[13]);
    u[14] = _mm_sub_epi32(tmp[6], v[14]);
    u[15] = _mm_sub_epi32(tmp[0], v[15]);

    // stage 8
    out[0] = half_btf_small(&cospi62, &u[0], &cospim2, &u[1], &rnding, bit);
    out[2] = half_btf_small(&cospi54, &u[2], &cospim10, &u[3], &rnding, bit);
    out[4] = half_btf_small(&cospi46, &u[4], &cospim18, &u[5], &rnding, bit);
    out[6] = half_btf_small(&cospi38, &u[6], &cospim26, &u[7], &rnding, bit);
    out[7] = half_btf_small(&cospi34, &u[8], &cospi30, &u[9], &rnding, bit);
    out[5] = half_btf_small(&cospi42, &u[10], &cospi22, &u[11], &rnding, bit);
    out[3] = half_btf_small(&cospi50, &u[12], &cospi14, &u[13], &rnding, bit);
    out[1] = half_btf_small(&cospi58, &u[14], &cospi6, &u[15], &rnding, bit);
}

static void fdct4x8_col_N2_avx2(__m256i* in, __m256i* output, int32_t bit, const int32_t num_col) {
    const int32_t* cospi   = cospi_arr(bit);
    const __m256i  zero    = _mm256_setzero_si256();
    const __m256i  cospi32 = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi48 = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16 = _mm256_set1_epi32(cospi[16]);
    const __m256i  rnding  = _mm256_set1_epi32(1 << (bit - 1));
    __m256i        s0, s1, s2, s3;
    __m256i        u0, u1, u2, u3;
    __m256i        v0, v1, v2, v3;
    __m256i        out[4];

    int32_t endidx = 3 * num_col;
    s0             = _mm256_add_epi32(in[0], in[endidx]);
    s3             = _mm256_sub_epi32(in[0], in[endidx]);
    endidx -= num_col;
    s1 = _mm256_add_epi32(in[num_col], in[endidx]);
    s2 = _mm256_sub_epi32(in[num_col], in[endidx]);

    // btf_32_sse4_1_type0(cospi32, cospi32, s[01], u[02], bit);
    u0 = _mm256_mullo_epi32(s0, cospi32);
    u1 = _mm256_mullo_epi32(s1, cospi32);
    u2 = _mm256_add_epi32(u0, u1);

    u3 = _mm256_add_epi32(u2, rnding);

    u0 = _mm256_srai_epi32(u3, bit);

    // btf_32_sse4_1_type1(cospi48, cospi16, s[23], u[13], bit);
    v0 = _mm256_mullo_epi32(s2, cospi48);
    v1 = _mm256_mullo_epi32(s3, cospi16);
    v2 = _mm256_add_epi32(v0, v1);

    v3 = _mm256_add_epi32(v2, rnding);
    u1 = _mm256_srai_epi32(v3, bit);

    // Transpose 4x4 32-bit
    v0 = _mm256_unpacklo_epi32(u0, u1);
    v1 = _mm256_unpackhi_epi32(u0, u1);

    out[0] = _mm256_unpacklo_epi64(v0, zero);
    out[1] = _mm256_unpackhi_epi64(v0, zero);
    out[2] = _mm256_unpacklo_epi64(v1, zero);
    out[3] = _mm256_unpackhi_epi64(v1, zero);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static void fadst4x8_col_N2_avx2(__m256i* in, __m256i* output, int32_t bit, const int32_t num_col) {
    const int32_t* sinpi  = sinpi_arr(bit);
    const __m256i  zero   = _mm256_setzero_si256();
    const __m256i  rnding = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  sinpi1 = _mm256_set1_epi32((int32_t)sinpi[1]);
    const __m256i  sinpi2 = _mm256_set1_epi32((int32_t)sinpi[2]);
    const __m256i  sinpi3 = _mm256_set1_epi32((int32_t)sinpi[3]);
    const __m256i  sinpi4 = _mm256_set1_epi32((int32_t)sinpi[4]);
    __m256i        s0, s1, s2, s3, s4;
    __m256i        x0, x1;
    __m256i        u0, u1;
    __m256i        v0, v1;
    __m256i        out[4];

    int32_t idx = 0 * num_col;
    s0          = _mm256_mullo_epi32(in[idx], sinpi1);
    u0          = _mm256_add_epi32(in[idx], in[idx + num_col]);
    idx += num_col;
    s2 = _mm256_mullo_epi32(in[idx], sinpi2);
    idx += num_col;
    s4 = _mm256_mullo_epi32(in[idx], sinpi3);
    idx += num_col;
    s3 = _mm256_mullo_epi32(in[idx], sinpi4);
    s1 = _mm256_sub_epi32(u0, in[idx]);

    u0 = _mm256_add_epi32(s0, s2);
    x0 = _mm256_add_epi32(u0, s3);
    x1 = _mm256_mullo_epi32(s1, sinpi3);

    s0 = _mm256_add_epi32(x0, s4);

    u0 = _mm256_add_epi32(s0, rnding);
    u0 = _mm256_srai_epi32(u0, bit);

    u1 = _mm256_add_epi32(x1, rnding);
    u1 = _mm256_srai_epi32(u1, bit);

    v0 = _mm256_unpacklo_epi32(u0, u1);
    v1 = _mm256_unpackhi_epi32(u0, u1);

    out[0] = _mm256_unpacklo_epi64(v0, zero);
    out[1] = _mm256_unpackhi_epi64(v0, zero);
    out[2] = _mm256_unpacklo_epi64(v1, zero);
    out[3] = _mm256_unpackhi_epi64(v1, zero);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static AOM_FORCE_INLINE void clear_buffer_4x16_N2(__m256i* buff) {
    const __m256i zero = _mm256_setzero_si256();
    buff[4]            = zero;
    buff[5]            = zero;
    buff[6]            = zero;
    buff[7]            = zero;
}

static INLINE void fidtx4x8_col_N2_avx2(__m256i* in, __m256i* output, int32_t bit, int32_t col_num) {
    (void)bit;
    const __m256i zero   = _mm256_setzero_si256();
    __m256i       fact   = _mm256_set1_epi32(new_sqrt2);
    __m256i       offset = _mm256_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m256i       a_low;

    for (int32_t i = 0; i < col_num; i++) {
        a_low     = _mm256_mullo_epi32(in[i], fact);
        a_low     = _mm256_add_epi32(a_low, offset);
        a_low     = _mm256_srai_epi32(a_low, new_sqrt2_bits);
        output[i] = _mm256_unpacklo_epi64(a_low, zero);
    }
}

static AOM_FORCE_INLINE void fidtx4x8_N2_perm_avx2(__m256i* in, __m256i* output, int32_t bit) {
    (void)bit;
    __m256i fact   = _mm256_set1_epi32(2 * new_sqrt2);
    __m256i offset = _mm256_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m256i a_low;
    __m256i out[2];

    a_low  = _mm256_mullo_epi32(in[0], fact);
    a_low  = _mm256_add_epi32(a_low, offset);
    out[0] = _mm256_srai_epi32(a_low, new_sqrt2_bits);
    a_low  = _mm256_mullo_epi32(in[2], fact);
    a_low  = _mm256_add_epi32(a_low, offset);
    out[1] = _mm256_srai_epi32(a_low, new_sqrt2_bits);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
}

static INLINE void fidtx4x8_row_N2_avx2(__m256i* in, __m256i* output, int32_t bit) {
    (void)bit;
    __m256i fact   = _mm256_set1_epi32(new_sqrt2);
    __m256i offset = _mm256_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m256i a_low;

    for (int32_t i = 0; i < 4; i += 2) {
        a_low     = _mm256_mullo_epi32(in[i], fact);
        a_low     = _mm256_add_epi32(a_low, offset);
        output[i] = _mm256_srai_epi32(a_low, new_sqrt2_bits);
    }
}

static void fdct4x8_row_N2_with_round_avx2(__m256i* input, __m256i* output, int32_t bit, const int32_t num_col,
                                           int32_t shift) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  zero     = _mm256_setzero_si256();
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  rounding = _mm256_set1_epi32(1 << (shift - 1));
    __m256i        in[4];
    __m256i        out[4];
    __m256i        s0, s1, s2, s3;
    __m256i        u0, u1, u2, u3;
    __m256i        v0, v1, v2, v3;
    int32_t        endidx = 3 * num_col;

    in[0] = yy_unpacklo_epi128(input[0], input[2]);
    in[1] = yy_unpackhi_epi128(input[0], input[2]);
    in[2] = yy_unpacklo_epi128(input[1], input[3]);
    in[3] = yy_unpackhi_epi128(input[1], input[3]);

    s0 = _mm256_add_epi32(in[0], in[endidx]);
    s3 = _mm256_sub_epi32(in[0], in[endidx]);
    endidx -= num_col;
    s1 = _mm256_add_epi32(in[num_col], in[endidx]);
    s2 = _mm256_sub_epi32(in[num_col], in[endidx]);

    // btf_32_sse4_1_type0(cospi32, cospi32, s[01], u[02], bit);
    u0 = _mm256_mullo_epi32(s0, cospi32);
    u1 = _mm256_mullo_epi32(s1, cospi32);
    u2 = _mm256_add_epi32(u0, u1);

    u3 = _mm256_add_epi32(u2, rnding);

    u0 = _mm256_srai_epi32(u3, bit);

    // btf_32_sse4_1_type1(cospi48, cospi16, s[23], u[13], bit);
    v0 = _mm256_mullo_epi32(s2, cospi48);
    v1 = _mm256_mullo_epi32(s3, cospi16);
    v2 = _mm256_add_epi32(v0, v1);

    v3 = _mm256_add_epi32(v2, rnding);
    u1 = _mm256_srai_epi32(v3, bit);

    //round
    u0 = _mm256_add_epi32(u0, rounding);
    u0 = _mm256_srai_epi32(u0, shift);
    u1 = _mm256_add_epi32(u1, rounding);
    u1 = _mm256_srai_epi32(u1, shift);

    // Transpose 4x4 32-bit
    v0 = _mm256_unpacklo_epi32(u0, u1);
    v1 = _mm256_unpackhi_epi32(u0, u1);

    out[0] = _mm256_unpacklo_epi64(v0, zero);
    out[1] = _mm256_unpackhi_epi64(v0, zero);
    out[2] = _mm256_unpacklo_epi64(v1, zero);
    out[3] = _mm256_unpackhi_epi64(v1, zero);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static void fadst4x8_row_N2_with_round_avx2(__m256i* input, __m256i* output, int32_t bit, const int32_t num_col,
                                            int32_t shift) {
    const int32_t* sinpi    = sinpi_arr(bit);
    const __m256i  zero     = _mm256_setzero_si256();
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  rounding = _mm256_set1_epi32(1 << (shift - 1));
    const __m256i  sinpi1   = _mm256_set1_epi32((int32_t)sinpi[1]);
    const __m256i  sinpi2   = _mm256_set1_epi32((int32_t)sinpi[2]);
    const __m256i  sinpi3   = _mm256_set1_epi32((int32_t)sinpi[3]);
    const __m256i  sinpi4   = _mm256_set1_epi32((int32_t)sinpi[4]);
    __m256i        s0, s1, s2, s3, s4;
    __m256i        x0, x1;
    __m256i        u0, u1;
    __m256i        v0, v1;
    __m256i        out[4];
    __m256i        in[4];

    int32_t idx = 0 * num_col;

    in[0] = yy_unpacklo_epi128(input[0], input[2]);
    in[1] = yy_unpackhi_epi128(input[0], input[2]);
    in[2] = yy_unpacklo_epi128(input[1], input[3]);
    in[3] = yy_unpackhi_epi128(input[1], input[3]);

    s0 = _mm256_mullo_epi32(in[idx], sinpi1);
    u0 = _mm256_add_epi32(in[idx], in[idx + num_col]);
    idx += num_col;
    s2 = _mm256_mullo_epi32(in[idx], sinpi2);
    idx += num_col;
    s4 = _mm256_mullo_epi32(in[idx], sinpi3);
    idx += num_col;
    s3 = _mm256_mullo_epi32(in[idx], sinpi4);
    s1 = _mm256_sub_epi32(u0, in[idx]);

    u0 = _mm256_add_epi32(s0, s2);
    x0 = _mm256_add_epi32(u0, s3);
    x1 = _mm256_mullo_epi32(s1, sinpi3);

    s0 = _mm256_add_epi32(x0, s4);

    u0 = _mm256_add_epi32(s0, rnding);
    u0 = _mm256_srai_epi32(u0, bit);

    u1 = _mm256_add_epi32(x1, rnding);
    u1 = _mm256_srai_epi32(u1, bit);

    //round
    u0 = _mm256_add_epi32(u0, rounding);
    u0 = _mm256_srai_epi32(u0, shift);
    u1 = _mm256_add_epi32(u1, rounding);
    u1 = _mm256_srai_epi32(u1, shift);

    v0 = _mm256_unpacklo_epi32(u0, u1);
    v1 = _mm256_unpackhi_epi32(u0, u1);

    out[0] = _mm256_unpacklo_epi64(v0, zero);
    out[1] = _mm256_unpackhi_epi64(v0, zero);
    out[2] = _mm256_unpacklo_epi64(v1, zero);
    out[3] = _mm256_unpackhi_epi64(v1, zero);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static INLINE void fidtx4x8_row_N2_with_round_avx2(__m256i* input, __m256i* output, int32_t bit, int32_t shift) {
    (void)bit;
    __m256i       in[2];
    __m256i       out[4];
    __m256i       fact     = _mm256_set1_epi32(new_sqrt2);
    __m256i       offset   = _mm256_set1_epi32(1 << (new_sqrt2_bits - 1));
    const __m256i rounding = _mm256_set1_epi32(1 << (shift - 1));
    const __m256i zero     = _mm256_setzero_si256();
    __m256i       a_low;
    __m256i       v[4];

    in[0] = yy_unpacklo_epi128(input[0], input[2]);
    in[1] = yy_unpackhi_epi128(input[0], input[2]);

    for (int32_t i = 0; i < 2; i++) {
        a_low  = _mm256_mullo_epi32(in[i], fact);
        a_low  = _mm256_add_epi32(a_low, offset);
        a_low  = _mm256_srai_epi32(a_low, new_sqrt2_bits);
        a_low  = _mm256_add_epi32(a_low, rounding);
        out[i] = _mm256_srai_epi32(a_low, shift);
    }

    // Transpose for 4x4
    v[0] = _mm256_unpacklo_epi32(out[0], out[1]);
    v[1] = _mm256_unpackhi_epi32(out[0], out[1]);

    out[0] = _mm256_unpacklo_epi64(v[0], zero);
    out[1] = _mm256_unpackhi_epi64(v[0], zero);
    out[2] = _mm256_unpacklo_epi64(v[1], zero);
    out[3] = _mm256_unpackhi_epi64(v[1], zero);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static void fdct4x8_N2_avx2(__m256i* input, __m256i* output, int32_t bit) {
    __m128i*       in       = (__m128i*)input;
    __m128i*       out      = (__m128i*)output;
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[8], v[8];

    // Even 8 points 0, 2, ..., 14
    // stage 0
    // stage 1
    u[0] = _mm_add_epi32(in[0], in[7]);
    v[7] = _mm_sub_epi32(in[0], in[7]); // v[7]
    u[1] = _mm_add_epi32(in[1], in[6]);
    u[6] = _mm_sub_epi32(in[1], in[6]);
    u[2] = _mm_add_epi32(in[2], in[5]);
    u[5] = _mm_sub_epi32(in[2], in[5]);
    u[3] = _mm_add_epi32(in[3], in[4]);
    v[4] = _mm_sub_epi32(in[3], in[4]); // v[4]

    // stage 2
    v[0] = _mm_add_epi32(u[0], u[3]);
    v[3] = _mm_sub_epi32(u[0], u[3]);
    v[1] = _mm_add_epi32(u[1], u[2]);
    v[2] = _mm_sub_epi32(u[1], u[2]);

    v[5] = _mm_mullo_epi32(u[5], cospim32);
    v[6] = _mm_mullo_epi32(u[6], cospi32);
    v[5] = _mm_add_epi32(v[5], v[6]);
    v[5] = _mm_add_epi32(v[5], rnding);
    v[5] = _mm_srai_epi32(v[5], bit);

    u[0] = _mm_mullo_epi32(u[5], cospi32);
    v[6] = _mm_mullo_epi32(u[6], cospim32);
    v[6] = _mm_sub_epi32(u[0], v[6]);
    v[6] = _mm_add_epi32(v[6], rnding);
    v[6] = _mm_srai_epi32(v[6], bit);

    // stage 3
    // type 0
    v[0]   = _mm_mullo_epi32(v[0], cospi32);
    v[1]   = _mm_mullo_epi32(v[1], cospi32);
    u[0]   = _mm_add_epi32(v[0], v[1]);
    u[0]   = _mm_add_epi32(u[0], rnding);
    out[0] = _mm_srai_epi32(u[0], bit);

    // type 1
    v[0]   = _mm_mullo_epi32(v[2], cospi48);
    v[1]   = _mm_mullo_epi32(v[3], cospi16);
    u[2]   = _mm_add_epi32(v[0], v[1]);
    u[2]   = _mm_add_epi32(u[2], rnding);
    out[2] = _mm_srai_epi32(u[2], bit);

    u[4] = _mm_add_epi32(v[4], v[5]);
    u[5] = _mm_sub_epi32(v[4], v[5]);
    u[6] = _mm_sub_epi32(v[7], v[6]);
    u[7] = _mm_add_epi32(v[7], v[6]);

    // stage 4
    // stage 5
    v[0]   = _mm_mullo_epi32(u[4], cospi56);
    v[1]   = _mm_mullo_epi32(u[7], cospi8);
    v[0]   = _mm_add_epi32(v[0], v[1]);
    v[0]   = _mm_add_epi32(v[0], rnding);
    out[1] = _mm_srai_epi32(v[0], bit); // buf0[4]

    v[0]   = _mm_mullo_epi32(u[5], cospi40);
    v[1]   = _mm_mullo_epi32(u[6], cospi24);
    v[0]   = _mm_sub_epi32(v[1], v[0]);
    v[0]   = _mm_add_epi32(v[0], rnding);
    out[3] = _mm_srai_epi32(v[0], bit); // buf0[6]
}

static AOM_FORCE_INLINE void col_txfm_8x4_N2_rounding(__m256i* in, int32_t shift) {
    const __m256i rounding = _mm256_set1_epi32(1 << (shift - 1));

    in[0] = _mm256_add_epi32(in[0], rounding);
    in[1] = _mm256_add_epi32(in[1], rounding);

    in[0] = _mm256_srai_epi32(in[0], shift);
    in[1] = _mm256_srai_epi32(in[1], shift);
}

static AOM_FORCE_INLINE void write_buffer_4x8_N2(const __m256i* res, int32_t* output) {
    const __m256i zero = _mm256_setzero_si256();
    _mm256_storeu_si256((__m256i*)(output + 0 * 8), res[0]);
    _mm256_storeu_si256((__m256i*)(output + 1 * 8), res[1]);
    _mm256_storeu_si256((__m256i*)(output + 2 * 8), zero);
    _mm256_storeu_si256((__m256i*)(output + 3 * 8), zero);
}

static void fadst8x4_N2_avx2(__m256i* input, __m256i* output, int32_t bit, const int32_t col_num) {
    __m128i*       in       = (__m128i*)input;
    __m128i*       out      = (__m128i*)output;
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim4  = _mm_set1_epi32(-cospi[4]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospim20 = _mm_set1_epi32(-cospi[20]);
    const __m128i  cospi44  = _mm_set1_epi32(cospi[44]);
    const __m128i  cospi28  = _mm_set1_epi32(cospi[28]);
    const __m128i  cospi36  = _mm_set1_epi32(cospi[36]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();
    __m128i        u0, u1, u2, u3, u4, u5, u6, u7;
    __m128i        v0, v1, v2, v3, v4, v5, v6, v7;
    __m128i        x, y;
    int32_t        col;

    // Note:
    //  Even column: 0, 2, ..., 14
    //  Odd column: 1, 3, ..., 15
    //  one even column plus one odd column constructs one row (8 coeffs)
    //  total we have 8 rows (8x8).
    for (col = 0; col < col_num; ++col) {
        // stage 0
        // stage 1
        u0 = in[col_num * 0 + col];
        u1 = _mm_sub_epi32(zero, in[col_num * 7 + col]);
        u2 = _mm_sub_epi32(zero, in[col_num * 3 + col]);
        u3 = in[col_num * 4 + col];
        u4 = _mm_sub_epi32(zero, in[col_num * 1 + col]);
        u5 = in[col_num * 6 + col];
        u6 = in[col_num * 2 + col];
        u7 = _mm_sub_epi32(zero, in[col_num * 5 + col]);

        // stage 2
        v0 = u0;
        v1 = u1;

        x  = _mm_mullo_epi32(u2, cospi32);
        y  = _mm_mullo_epi32(u3, cospi32);
        v2 = _mm_add_epi32(x, y);
        v2 = _mm_add_epi32(v2, rnding);
        v2 = _mm_srai_epi32(v2, bit);

        v3 = _mm_sub_epi32(x, y);
        v3 = _mm_add_epi32(v3, rnding);
        v3 = _mm_srai_epi32(v3, bit);

        v4 = u4;
        v5 = u5;

        x  = _mm_mullo_epi32(u6, cospi32);
        y  = _mm_mullo_epi32(u7, cospi32);
        v6 = _mm_add_epi32(x, y);
        v6 = _mm_add_epi32(v6, rnding);
        v6 = _mm_srai_epi32(v6, bit);

        v7 = _mm_sub_epi32(x, y);
        v7 = _mm_add_epi32(v7, rnding);
        v7 = _mm_srai_epi32(v7, bit);

        // stage 3
        u0 = _mm_add_epi32(v0, v2);
        u1 = _mm_add_epi32(v1, v3);
        u2 = _mm_sub_epi32(v0, v2);
        u3 = _mm_sub_epi32(v1, v3);
        u4 = _mm_add_epi32(v4, v6);
        u5 = _mm_add_epi32(v5, v7);
        u6 = _mm_sub_epi32(v4, v6);
        u7 = _mm_sub_epi32(v5, v7);

        // stage 4
        v0 = u0;
        v1 = u1;
        v2 = u2;
        v3 = u3;

        x  = _mm_mullo_epi32(u4, cospi16);
        y  = _mm_mullo_epi32(u5, cospi48);
        v4 = _mm_add_epi32(x, y);
        v4 = _mm_add_epi32(v4, rnding);
        v4 = _mm_srai_epi32(v4, bit);

        x  = _mm_mullo_epi32(u4, cospi48);
        y  = _mm_mullo_epi32(u5, cospim16);
        v5 = _mm_add_epi32(x, y);
        v5 = _mm_add_epi32(v5, rnding);
        v5 = _mm_srai_epi32(v5, bit);

        x  = _mm_mullo_epi32(u6, cospim48);
        y  = _mm_mullo_epi32(u7, cospi16);
        v6 = _mm_add_epi32(x, y);
        v6 = _mm_add_epi32(v6, rnding);
        v6 = _mm_srai_epi32(v6, bit);

        x  = _mm_mullo_epi32(u6, cospi16);
        y  = _mm_mullo_epi32(u7, cospi48);
        v7 = _mm_add_epi32(x, y);
        v7 = _mm_add_epi32(v7, rnding);
        v7 = _mm_srai_epi32(v7, bit);

        // stage 5
        u0 = _mm_add_epi32(v0, v4);
        u1 = _mm_add_epi32(v1, v5);
        u2 = _mm_add_epi32(v2, v6);
        u3 = _mm_add_epi32(v3, v7);
        u4 = _mm_sub_epi32(v0, v4);
        u5 = _mm_sub_epi32(v1, v5);
        u6 = _mm_sub_epi32(v2, v6);
        u7 = _mm_sub_epi32(v3, v7);

        // stage 6
        x                      = _mm_mullo_epi32(u0, cospi60);
        y                      = _mm_mullo_epi32(u1, cospim4);
        v1                     = _mm_add_epi32(x, y);
        v1                     = _mm_add_epi32(v1, rnding);
        out[col_num * 0 + col] = _mm_srai_epi32(v1, bit);

        x                      = _mm_mullo_epi32(u2, cospi44);
        y                      = _mm_mullo_epi32(u3, cospim20);
        v3                     = _mm_add_epi32(x, y);
        v3                     = _mm_add_epi32(v3, rnding);
        out[col_num * 2 + col] = _mm_srai_epi32(v3, bit);

        x                      = _mm_mullo_epi32(u4, cospi36);
        y                      = _mm_mullo_epi32(u5, cospi28);
        v4                     = _mm_add_epi32(x, y);
        v4                     = _mm_add_epi32(v4, rnding);
        out[col_num * 3 + col] = _mm_srai_epi32(v4, bit);

        x                      = _mm_mullo_epi32(u6, cospi52);
        y                      = _mm_mullo_epi32(u7, cospi12);
        v6                     = _mm_add_epi32(x, y);
        v6                     = _mm_add_epi32(v6, rnding);
        out[col_num * 1 + col] = _mm_srai_epi32(v6, bit);
    }
}

static AOM_FORCE_INLINE void fidtx8x4_N2_avx2(__m256i* in, __m256i* out, int32_t bit) {
    (void)bit;

    out[0] = _mm256_add_epi32(in[0], in[0]);
    out[1] = _mm256_add_epi32(in[1], in[1]);
}

static AOM_FORCE_INLINE void write_buffer_16x8_N2_avx2(const __m256i* res, __m256i* out) {
    out[0] = res[0];
    out[2] = res[1];
    out[4] = res[2];
    out[6] = res[3];
}

void svt_av1_fwd_txfm2d_8x8_N2_avx2(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[8], out[8];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_8X8];
    const int32_t txw_idx = get_txw_idx(TX_8X8);
    const int32_t txh_idx = get_txh_idx(TX_8X8);

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_avx2(out, in);
        fdct8x8_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_avx2(out, in);
        fdct8x8_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_avx2(out, in);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_avx2(out, in);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_avx2(out, in);
        fdct8x8_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fdct8x8_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_avx2(out, in);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 1, shift[0]);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_avx2(out, in);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_avx2(out, in);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        transpose_8x8_half_avx2(out, in);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N2(in, coeff);
        break;
    case IDTX:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 0, shift[0], 1);
        fidtx8x8_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8x8_N2_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N2(out, coeff);
        break;
    case V_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8x8_N2_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N2(out, coeff);
        break;
    case H_DCT:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 0, shift[0], 1);
        fidtx8x8_N2_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fdct8x8_N2_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(in, out);
        write_buffer_8x8_N2(out, coeff);
        break;
    case V_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8x8_N2_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N2(out, coeff);
        break;
    case H_ADST:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 0, shift[0], 1);
        fidtx8x8_N2_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fadst8x8_N2_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(in, out);
        write_buffer_8x8_N2(out, coeff);
        break;
    case V_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8x8_N2_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N2(out, coeff);
        break;
    case H_FLIPADST:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 1, shift[0], 1);
        fidtx8x8_N2_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fadst8x8_N2_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(in, out);
        write_buffer_8x8_N2(out, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x16_N2_avx2(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[32], out[32];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_16X16];
    const int32_t txw_idx = get_txw_idx(TX_16X16);
    const int32_t txh_idx = get_txh_idx(TX_16X16);
    const int32_t col_num = 2;
    switch (tx_type) {
    case IDTX:
        load_buffer_16x16_N2_half(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_N2_row_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_N2_half_rounding(out, -shift[1]);
        fidtx16x16_N2_row_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16_N2(out, coeff);
        break;
    case DCT_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fdct16x16_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_avx2(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fdct16x16_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_avx2(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num);
        transpose_8x8_in_16x16_avx2(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_avx2(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        fdct16x16_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_avx2(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fdct16x16_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_avx2(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x16(input, in, stride, 1, 1, shift[0]);
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_avx2(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_avx2(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N2_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_avx2(out, in);
        write_buffer_16x16_N2(in, coeff);
        break;
    case V_DCT:
        load_buffer_16x16_N2(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num / 2);
        col_txfm_16x16_N2_half_rounding(out, -shift[1]);
        fidtx16x16_N2_row_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16_N2(out, coeff);
        break;
    case H_DCT:
        load_buffer_16x16_N2_H(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 16);
        col_txfm_16x16_N2_rounding(in, -shift[1]);
        transpose_8x8_in_16x16_avx2(in, out); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(in + 1, out + 16); //top-right ->bottom-left
        fdct16x16_N2_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_avx2(in, out);
        write_buffer_16x16_N2(out, coeff);
        break;
    case V_ADST:
        load_buffer_16x16_N2(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num / 2);
        col_txfm_16x16_N2_half_rounding(out, -shift[1]);
        fidtx16x16_N2_row_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16_N2(out, coeff);
        break;
    case H_ADST:
        load_buffer_16x16_N2_H(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 16);
        col_txfm_16x16_N2_rounding(in, -shift[1]);
        transpose_8x8_in_16x16_avx2(in, out); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(in + 1, out + 16); //top-right ->bottom-left
        fadst16x16_N2_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_avx2(in, out);
        write_buffer_16x16_N2(out, coeff);
        break;
    case V_FLIPADST:
        load_buffer_16x16_N2(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N2_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num / 2);
        col_txfm_16x16_N2_half_rounding(out, -shift[1]);
        fidtx16x16_N2_row_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16_N2(out, coeff);
        break;
    case H_FLIPADST:
        load_buffer_16x16_N2_H(input, in, stride, 0, 1, shift[0]);
        fidtx8xn_N2_col_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 16);
        col_txfm_16x16_N2_rounding(in, -shift[1]);
        transpose_8x8_in_16x16_avx2(in, out); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(in + 1, out + 16); //top-right ->bottom-left
        fadst16x16_N2_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num, col_num / 2);
        transpose_8x8_in_16x16_avx2(in, out);
        write_buffer_16x16_N2(out, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x64_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    __m256i       in[512];
    __m256i*      out     = (__m256i*)output;
    const int32_t txw_idx = tx_size_wide_log2[TX_64X64] - tx_size_wide_log2[0];
    const int32_t txh_idx = tx_size_high_log2[TX_64X64] - tx_size_high_log2[0];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_64X64];

    switch (tx_type) {
    case IDTX:
        load_buffer_32x32_in_64x64_avx2(input, stride, out);
        fidtx64x64_N2_avx2(out, in);
        av1_round_shift_array_64_N2_avx2(in, in, 512 / 2, -shift[1]);
        /*row wise transform*/
        fidtx64x64_N2_avx2(in, out);
        av1_round_shift_array_64_N2_avx2(out, out, 512 / 2, -shift[2]);
        clear_buffer_wxh_N2(out, 8, 64);
        break;
    case DCT_DCT:
        load_buffer_64x64_avx2(input, stride, out);
        av1_fdct64_new_N2_avx2(out, in, fwd_cos_bit_col[txw_idx][txh_idx], 64, 8);
        av1_round_shift_array_32_avx2(in, out, 512 / 2, -shift[1]);
        transpose_32x32_in_64x64_avx2(out, in); //top-left
        transpose_32x32_in_64x64_avx2(out + 4, in + 256); //top-right -> bottom left
        /*row wise transform*/
        av1_fdct64_new_N2_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 32, 8);
        av1_round_shift_array_64_N2_avx2(out, in, 512 / 2, -shift[2]);
        transpose_32x32_in_64x64_avx2(in, out); //top-left
        clear_buffer_wxh_N2(out, 8, 64);
        break;
    default:
        assert(0);
    }
}

void svt_av1_fwd_txfm2d_32x32_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    const int8_t* shift           = fwd_txfm_shift_ls[TX_32X32];
    const int32_t txw_idx         = tx_size_wide_log2[TX_32X32] - tx_size_wide_log2[0];
    const int32_t txh_idx         = tx_size_high_log2[TX_32X32] - tx_size_high_log2[0];
    const int8_t  cos_bit_col     = fwd_cos_bit_col[txw_idx][txh_idx];
    const int8_t  cos_bit_row     = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm2d_size_256 = 32 * 32 / 8;
    __m256i       buf_256[128];
    __m256i*      out_256 = (__m256i*)output;
    (void)bd;

    switch (tx_type) {
    case IDTX:
        load_buffer_32x16_N2_avx2(input, buf_256, stride);
        av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256 / 2, -shift[0]);
        fidtx32x32_N2_row_avx2(out_256, buf_256, cos_bit_col);
        av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256 / 2, -shift[1]);
        fidtx32x32_N2_row_avx2(out_256, buf_256, cos_bit_row);
        av1_round_shift_array_32_N2_avx2(buf_256, out_256, txfm2d_size_256 / 2, -shift[2]);
        clear_buffer_wxh_N2(out_256, 4, 32);
        break;
    case DCT_DCT:
        load_buffer_32x32_avx2(input, buf_256, stride);
        av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256, -shift[0]);
        fdct32x32_N2_col_avx2(out_256, buf_256, cos_bit_col);
        av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256 / 2, -shift[1]);
        transpose_16x16_in_32x32_avx2(out_256, buf_256); //top-left -> top-left
        transpose_16x16_in_32x32_avx2(out_256 + 2, buf_256 + 64); //top-right ->bottom-left
        fdct32x32_N2_row_avx2(buf_256, out_256, cos_bit_row);
        av1_round_shift_array_32_N2_avx2(out_256, out_256, txfm2d_size_256 / 2, -shift[2]);
        transpose_16x16_in_32x32_avx2(out_256, out_256); //transpose nonzer output
        clear_buffer_wxh_N2(out_256, 4, 32);
        break;
    case V_DCT:
        load_buffer_32x32_avx2(input, buf_256, stride);
        av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256, -shift[0]);
        fdct32x32_N2_row_avx2(out_256, buf_256, cos_bit_col); //col
        av1_round_shift_array_32_N2_avx2(buf_256, out_256, txfm2d_size_256 / 2, -shift[1]);
        fidtx32x32_N2_row_avx2(out_256, buf_256, cos_bit_row);
        av1_round_shift_array_32_N2_avx2(buf_256, out_256, txfm2d_size_256 / 2, -shift[2]);
        clear_buffer_wxh_N2(out_256, 4, 32);
        break;
    case H_DCT:
        load_buffer_32x16_avx2(input, buf_256, stride);
        av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256 / 2, -shift[0]);
        fidtx32x32_N2_col_avx2(out_256, buf_256, cos_bit_col);
        av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256 / 2, -shift[1]);
        transpose_16x16_in_32x32_avx2(out_256, buf_256); //top-left -> top-left
        transpose_16x16_in_32x32_avx2(out_256 + 2, buf_256 + 64); //top-right ->bottom-left
        fdct32x32_N2_row_avx2(buf_256, out_256, cos_bit_row);
        av1_round_shift_array_32_N2_avx2(out_256, out_256, txfm2d_size_256 / 2, -shift[2]);
        transpose_16x16_in_32x32_avx2(out_256, out_256); //transpose nonzer output
        clear_buffer_wxh_N2(out_256, 4, 32);
        break;
    default:
        assert(0);
    }
}

void svt_av1_fwd_txfm2d_16x32_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[64];
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X32];
    const int32_t txw_idx       = get_txw_idx(TX_16X32);
    const int32_t txh_idx       = get_txh_idx(TX_16X32);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_16X32];
    const int32_t txfm_size_row = tx_size_high[TX_16X32];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_16x16_N2(input, in, stride, 0, 0, shift[0]);
        av1_idtx16x32_N2_avx2(in, in);
        col_txfm_16x16_N2_half_rounding(&in[0], -shift[1]);
        col_txfm_16x16_N2_half_rounding(&in[16], -shift[1]);
        fidtx16x16_N2_row_avx2(in, outcoef256, bitrow, num_row);
        av1_round_shift_rect_array_wxh_avx2(outcoef256, outcoef256, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        load_buffer_16x16(input + 16 * stride, in + 32, stride, 0, 0, shift[0]);
        for (int32_t i = 0; i < num_col; i++) {
            av1_fdct32_new_N2_avx2((in + i), (in + i), bitcol, 8, num_col);
        }
        col_txfm_16x16_rounding(&in[0], -shift[1]);
        transpose_8nx8n_N2_half(in, outcoef256, txfm_size_col, txfm_size_row);
        fdct16x16_N2_avx2(outcoef256, in, bitrow, num_row, num_row / 2);
        transpose_8nx8n_N2_quad(in, outcoef256, txfm_size_row, txfm_size_col);
        av1_round_shift_rect_array_wxh_avx2(outcoef256, outcoef256, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }

    (void)bd;
}

/* call this function only for IDTX */
void svt_av1_fwd_txfm2d_32x16_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[64];
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_32X16];
    const int32_t txw_idx       = get_txw_idx(TX_32X16);
    const int32_t txh_idx       = get_txh_idx(TX_32X16);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_32X16];
    const int32_t txfm_size_row = tx_size_high[TX_32X16];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_16x8n(input, in, stride, 0, 0, shift[0], txfm_size_row / 2);
        fidtx32x16_N2_avx2(in, in, bitcol, txfm_size_row / 2);
        col_txfm_32x16_N2_rounding(&in[0], -shift[1]);
        col_txfm_32x16_N2_rounding(&in[1], -shift[1]);
        av1_idtx32x16_N2_avx2(in, outcoef256, txfm_size_row / 2);
        av1_round_shift_rect_array_wxh_avx2(outcoef256, outcoef256, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
        fdct16x16_N2_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_16x16_rounding(&in[0], -shift[1]);
        transpose_8nx8n_N2_half(in, outcoef256, txfm_size_col, txfm_size_row);
        av1_fdct32_new_N2_avx2(outcoef256, in, bitrow, 8, num_row);
        transpose_8nx8n_N2_quad(in, outcoef256, txfm_size_row, txfm_size_col);
        av1_round_shift_rect_array_wxh_avx2(outcoef256, outcoef256, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }

    (void)bd;
}

/* call this function only for DCT_DCT, IDTX */
void svt_av1_fwd_txfm2d_8x32_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[32];
    __m256i*      outcoef256 = (__m256i*)output;
    const int8_t* shift      = fwd_txfm_shift_ls[TX_8X32];
    const int32_t txw_idx    = get_txw_idx(TX_8X32);
    const int32_t txh_idx    = get_txh_idx(TX_8X32);
    int8_t        bitcol     = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow     = fwd_cos_bit_row[txw_idx][txh_idx];

    const int32_t txfm_size_col = tx_size_wide[TX_8X32];
    const int32_t txfm_size_row = tx_size_high[TX_8X32];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        av1_idtx16_new_avx2(in, in, bitcol, num_col);
        col_txfm_16x16_N2_rounding(in, -shift[1]);
        // row transform
        fidtx32x8_N2_avx2(in, outcoef256, bitrow, num_col, 16);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        load_buffer_8x16(input + (txfm_size_row >> 1) * stride, in + 16, stride, 0, 0, shift[0]);
        av1_fdct32_new_N2_avx2(in, in, bitcol, 8, num_col);
        col_txfm_16x16_N2_rounding(in, -shift[1]);
        transpose_8nx8n_N2_half(in, outcoef256, txfm_size_col, txfm_size_row);
        // row transform
        fdct8x8_N2_avx2(outcoef256, in, bitrow, num_row);
        fdct8x8_N2_avx2(outcoef256 + 1, in + 1, bitrow, num_row);
        transpose_8nx8n_N2_quad(in, outcoef256, txfm_size_row, txfm_size_col);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }

    (void)bd;
}

/* call this function only for DCT_DCT, IDTX */
void svt_av1_fwd_txfm2d_32x8_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[32];
    __m256i*      outcoef256 = (__m256i*)output;
    const int8_t* shift      = fwd_txfm_shift_ls[TX_32X8];
    const int32_t txw_idx    = get_txw_idx(TX_32X8);
    const int32_t txh_idx    = get_txh_idx(TX_32X8);
    int8_t        bitcol     = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow     = fwd_cos_bit_row[txw_idx][txh_idx];

    const int32_t txfm_size_col = tx_size_wide[TX_32X8];
    const int32_t txfm_size_row = tx_size_high[TX_32X8];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    (void)bd;

    switch (tx_type) {
    case IDTX:
        load_buffer_16x8n(input, in, stride, 0, 0, shift[0], txfm_size_row / 2);
        fidtx32x8_N2_avx2(in, in, bitcol, num_col, 4);
        fidtx32x8_N2_avx2(in + 1, in + 1, bitcol, num_col, 4);
        col_txfm_32x8_N2_half_rounding(&in[0], -shift[1]);
        // row transform
        av1_idtx32_new_N2_avx2(in, outcoef256, bitrow, num_row, 16);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
        for (int32_t i = 0; i < num_col; i++) {
            fdct8x8_N2_avx2((in + i), (in + i), bitcol, num_col);
        }
        col_txfm_16x16_N2_rounding(&in[0], -shift[1]);
        transpose_8nx8n_N2_half(in, outcoef256, txfm_size_col, txfm_size_row);
        // row transform
        av1_fdct32_new_N2_avx2(outcoef256, in, bitrow, 8, num_row);
        transpose_8nx8n_N2_quad(in, outcoef256, txfm_size_row, txfm_size_col);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
}

/* call this function for all 16 transform types */
void svt_av1_fwd_txfm2d_8x16_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[16], out[16];
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_8X16];
    const int32_t txw_idx       = get_txw_idx(TX_8X16);
    const int32_t txh_idx       = get_txh_idx(TX_8X16);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_8X16];
    const int32_t txfm_size_row = tx_size_high[TX_8X16];
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N2_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fdct8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case ADST_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N2_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fdct8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_ADST:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N2_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fadst8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case ADST_ADST:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N2_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fadst8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case FLIPADST_DCT:
        load_buffer_8x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N2_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fdct8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_FLIPADST:
        load_buffer_8x16(input, in, stride, 0, 1, shift[0]);
        fdct16x16_N2_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fadst8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x16(input, in, stride, 1, 1, shift[0]);
        fadst16x16_N2_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fadst8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case ADST_FLIPADST:
        load_buffer_8x16(input, in, stride, 0, 1, shift[0]);
        fadst16x16_N2_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fadst8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case FLIPADST_ADST:
        load_buffer_8x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N2_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fadst8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case IDTX:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, in, bitcol, 8);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fidtx8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case V_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N2_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fidtx8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case H_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, in, bitcol, 8);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fdct8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case V_ADST:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N2_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fidtx8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case H_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, in, bitcol, 8);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fadst8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case V_FLIPADST:
        load_buffer_8x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N2_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fidtx8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case H_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fidtx8xn_N2_col_avx2(in, in, bitcol, 8);
        col_txfm_8x8_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fadst8x8_N2_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        transpose_8x8_half_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }

    (void)bd;
}

/* call this function for all 16 transform types */
void svt_av1_fwd_txfm2d_16x8_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[16], out[16] = {_mm256_setzero_si256()};
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X8];
    const int32_t txw_idx       = get_txw_idx(TX_16X8);
    const int32_t txh_idx       = get_txh_idx(TX_16X8);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_16X8];
    const int32_t txfm_size_row = tx_size_high[TX_16X8];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;
    assert(num_col > 0);

    switch (tx_type) {
    case DCT_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fdct8x8_N2_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fdct16x16_N2_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case ADST_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fadst8x8_N2_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fdct16x16_N2_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fdct8x8_N2_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fadst16x16_N2_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case ADST_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fadst8x8_N2_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fadst16x16_N2_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case FLIPADST_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 1, 0, shift[0]);
            fadst8x8_N2_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fdct16x16_N2_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fdct8x8_N2_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[16 - i - 1] = out[i];
        }
        fadst16x16_N2_avx2(in, out, bitrow, num_row, 1);
        transpose_8x8_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case FLIPADST_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 1, 0, shift[0]);
            fadst8x8_N2_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[16 - i - 1] = out[i];
        }
        fadst16x16_N2_avx2(in, out, bitrow, num_row, 1);
        transpose_8x8_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case ADST_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fadst8x8_N2_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[16 - i - 1] = out[i];
        }
        fadst16x16_N2_avx2(in, out, bitrow, num_row, 1);
        transpose_8x8_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case FLIPADST_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 1, 0, shift[0]);
            fadst8x8_N2_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fadst16x16_N2_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case IDTX:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8x8_N2_avx2(in, out, bitcol, 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8xn_N2_col_avx2(out, out, bitrow, 4);
        av1_round_shift_rect_array_32_avx2(out, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case V_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N2_avx2(in, out, bitcol, 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8xn_N2_col_avx2(out, out, bitrow, 4);
        av1_round_shift_rect_array_32_avx2(out, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case H_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fidtx8x8_N2_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fdct16x16_N2_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case V_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N2_avx2(in, out, bitcol, 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8xn_N2_col_avx2(out, out, bitrow, 4);
        av1_round_shift_rect_array_32_avx2(out, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case H_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fidtx8x8_N2_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fadst16x16_N2_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case V_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N2_avx2(in, out, bitcol, 1);
        col_txfm_8x8_N2_rounding(out, -shift[1]);
        fidtx8xn_N2_col_avx2(out, out, bitrow, 4);
        av1_round_shift_rect_array_32_avx2(out, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    case H_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fidtx8x8_N2_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N2_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[16 - i - 1] = out[i];
        }
        fadst16x16_avx2(in, out, bitrow, num_row);
        transpose_8x8_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 4, -shift[2], new_sqrt2);
        write_buffer_16x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }

    (void)bd;
}

void svt_av1_fwd_txfm2d_4x8_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i in[4];
    __m256i outcoeff256[4];

    const int8_t* shift   = fwd_txfm_shift_ls[TX_4X8];
    const int32_t txw_idx = get_txw_idx(TX_4X8);
    const int32_t txh_idx = get_txh_idx(TX_4X8);
    int32_t       bitcol  = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow  = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_N2_avx2(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fdct4x8_col_N2_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case ADST_DCT:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fadst8x4_N2_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fdct4x8_col_N2_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case DCT_ADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_N2_avx2(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N2_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case ADST_ADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fadst8x4_N2_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N2_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case FLIPADST_DCT:
        load_buffer_4x8_avx2(input, in, stride, 1, 0, shift[0]);
        fadst8x4_N2_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fdct4x8_col_N2_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case DCT_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 1, shift[0]);
        fdct4x8_N2_avx2(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N2_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 1, 1, shift[0]);
        fadst8x4_N2_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N2_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case ADST_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 1, shift[0]);
        fadst8x4_N2_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N2_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case FLIPADST_ADST:
        load_buffer_4x8_avx2(input, in, stride, 1, 0, shift[0]);
        fadst8x4_N2_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N2_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case IDTX:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_N2_avx2(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        fidtx4x8_col_N2_avx2(in, in, bitrow, 2);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case V_DCT:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_N2_avx2(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        fidtx4x8_col_N2_avx2(in, in, bitrow, 2);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case H_DCT:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_N2_avx2(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fdct4x8_col_N2_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case V_ADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fadst8x4_N2_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        fidtx4x8_col_N2_avx2(in, in, bitrow, 2);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case H_ADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_N2_avx2(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N2_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case V_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 1, 0, shift[0]);
        fadst8x4_N2_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        fidtx4x8_col_N2_avx2(in, in, bitrow, 2);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    case H_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 1, shift[0]);
        fidtx8x4_N2_avx2(in, in, bitcol);
        col_txfm_8x4_N2_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N2_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        write_buffer_4x8_N2(outcoeff256, output);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_8x4_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[4]       = {_mm256_setzero_si256()};
    __m256i*      outcoeff256 = (__m256i*)output;
    const int8_t* shift       = fwd_txfm_shift_ls[TX_8X4];
    const int32_t txw_idx     = get_txw_idx(TX_8X4);
    const int32_t txh_idx     = get_txh_idx(TX_8X4);
    int32_t       bitcol      = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow      = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N2_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fdct4x8_N2_avx2(in, outcoeff256, bitrow);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case ADST_DCT:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fdct4x8_N2_avx2(in, outcoeff256, bitrow);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case DCT_ADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N2_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N2_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case ADST_ADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N2_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case FLIPADST_DCT:
        load_buffer_8x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fdct4x8_N2_avx2(in, outcoeff256, bitrow);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case DCT_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 1, shift[0]);
        fdct4x8_row_N2_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N2_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 1, 1, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N2_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case ADST_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 1, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N2_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case FLIPADST_ADST:
        load_buffer_8x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N2_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case IDTX:
        load_buffer_4x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N2_with_round_avx2(in, in, bitcol, -shift[1]);
        fidtx8x4_N2_avx2(in, outcoeff256, bitrow);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case V_DCT:
        load_buffer_4x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N2_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fidtx8x4_N2_avx2(in, outcoeff256, bitrow);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case H_DCT:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N2_with_round_avx2(in, in, bitcol, -shift[1]);
        fdct4x8_N2_avx2(in, outcoeff256, bitrow);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case V_ADST:
        load_buffer_4x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fidtx8x4_N2_avx2(in, outcoeff256, bitrow);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case H_ADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N2_with_round_avx2(in, in, bitcol, -shift[1]);
        fadst8x4_N2_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case V_FLIPADST:
        load_buffer_4x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fidtx8x4_N2_avx2(in, outcoeff256, bitrow);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    case H_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 1, shift[0]);
        fidtx4x8_row_N2_with_round_avx2(in, in, bitcol, -shift[1]);
        fadst8x4_N2_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 2, -shift[2], new_sqrt2);
        clear_buffer_wxh_N2(outcoeff256, 1, 4);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_4x16_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[8];
    __m256i*      outcoeff256 = (__m256i*)output;
    const int8_t* shift       = fwd_txfm_shift_ls[TX_4X16];
    const int32_t txw_idx     = get_txw_idx(TX_4X16);
    const int32_t txh_idx     = get_txh_idx(TX_4X16);
    int32_t       bitcol      = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow      = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fdct16x4_N2_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fdct4x8_col_N2_avx2(in, outcoeff256, bitrow, 2); //dct + transpose + clear right half of buffer
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case ADST_DCT:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fadst16x4_N2_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fdct4x8_col_N2_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case DCT_ADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fdct16x4_N2_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N2_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case ADST_ADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fadst16x4_N2_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N2_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case FLIPADST_DCT:
        load_buffer_4x16_avx2(input, in, stride, 1, 0, shift[0]);
        fadst16x4_N2_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fdct4x8_col_N2_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case DCT_FLIPADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 1, shift[0]);
        fdct16x4_N2_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N2_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x16_avx2(input, in, stride, 1, 1, shift[0]);
        fadst16x4_N2_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N2_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case ADST_FLIPADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 1, shift[0]);
        fadst16x4_N2_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N2_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case FLIPADST_ADST:
        load_buffer_4x16_avx2(input, in, stride, 1, 0, shift[0]);
        fadst16x4_N2_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N2_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case IDTX:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, outcoeff256, bitcol, 4);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        fidtx4x8_col_N2_avx2(outcoeff256, outcoeff256, bitrow, 4);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case V_DCT:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fdct16x4_N2_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        fidtx4x8_col_N2_avx2(outcoeff256, outcoeff256, bitrow, 4);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case H_DCT:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, outcoeff256, bitcol, 4);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fdct4x8_col_N2_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case V_ADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fadst16x4_N2_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        fidtx4x8_col_N2_avx2(outcoeff256, outcoeff256, bitrow, 4);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case H_ADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, outcoeff256, bitcol, 4);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N2_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case V_FLIPADST:
        load_buffer_4x16_avx2(input, in, stride, 1, 0, shift[0]);
        fadst16x4_N2_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        fidtx4x8_col_N2_avx2(outcoeff256, outcoeff256, bitrow, 4);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    case H_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 1, shift[0]);
        fidtx8xn_N2_col_avx2(in, outcoeff256, bitcol, 4);
        col_txfm_8x8_N2_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N2_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N2(outcoeff256);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x4_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[8];
    __m256i*      outcoeff256 = (__m256i*)output;
    const int8_t* shift       = fwd_txfm_shift_ls[TX_16X4];
    const int32_t txw_idx     = get_txw_idx(TX_16X4);
    const int32_t txh_idx     = get_txh_idx(TX_16X4);
    int32_t       bitcol      = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow      = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fdct4x8_row_N2_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fdct16x4_N2_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case ADST_DCT:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fadst4x8_row_N2_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fdct16x4_N2_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case DCT_ADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fdct4x8_row_N2_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fadst16x4_N2_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case ADST_ADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fadst4x8_row_N2_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fadst16x4_N2_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case FLIPADST_DCT:
        load_buffer_16x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fadst4x8_row_N2_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fdct16x4_N2_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case DCT_FLIPADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 1, shift[0]);
        fdct4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fdct4x8_row_N2_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fadst16x4_N2_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x4_avx2(input, in, stride, 1, 1, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fadst4x8_row_N2_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fadst16x4_N2_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case ADST_FLIPADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 1, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fadst4x8_row_N2_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fadst16x4_N2_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case FLIPADST_ADST:
        load_buffer_16x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fadst4x8_row_N2_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fadst16x4_N2_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case IDTX:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N2_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_rounding(outcoeff256, -shift[1]);
        fidtx4x8_N2_perm_avx2(outcoeff256, outcoeff256, bitrow);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case V_DCT:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fidtx8xn_N2_col_avx2(outcoeff256, in, bitrow, 4);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case H_DCT:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, -shift[1]);
        fidtx4x8_row_N2_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, -shift[1]);
        fdct16x4_N2_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case V_ADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fidtx8xn_N2_col_avx2(outcoeff256, in, bitrow, 4);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case H_ADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, -shift[1]);
        fidtx4x8_row_N2_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, -shift[1]);
        fadst16x4_N2_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case V_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fidtx8xn_N2_col_avx2(outcoeff256, in, bitrow, 4);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    case H_FLIPADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 1, shift[0]);
        fidtx4x8_row_N2_with_round_avx2(in, outcoeff256, bitcol, -shift[1]);
        fidtx4x8_row_N2_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, -shift[1]);
        fadst16x4_N2_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_half_avx2(in, outcoeff256);
        clear_buffer_wxh_N2(outcoeff256, 2, 4);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_32x64_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m256i       in[256];
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_32X64];
    const int32_t txw_idx       = get_txw_idx(TX_32X64);
    const int32_t txh_idx       = get_txh_idx(TX_32X64);
    const int32_t txfm_size_col = tx_size_wide[TX_32X64];
    const int32_t txfm_size_row = tx_size_high[TX_32X64];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    // column transform
    load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
    av1_fdct64_new_N2_avx2(in, in, bitcol, txfm_size_col, num_col);

    for (int32_t i = 0; i < num_row / 2; i++) {
        col_txfm_16x16_rounding((in + i * txfm_size_col), -shift[1]);
    }
    transpose_8nx8n_N2_half(in, outcoef256, txfm_size_col, txfm_size_row);

    // row transform
    av1_fdct32_new_N2_avx2(outcoef256, in, bitrow, txfm_size_row / 2, num_row);
    transpose_8nx8n_N2_quad(in, outcoef256, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_wxh_avx2(outcoef256, outcoef256, -shift[2], new_sqrt2, num_col, txfm_size_row);
    clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x32_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m256i       in[256];
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X32];
    const int32_t txw_idx       = get_txw_idx(TX_64X32);
    const int32_t txh_idx       = get_txh_idx(TX_64X32);
    const int32_t txfm_size_col = tx_size_wide[TX_64X32];
    const int32_t txfm_size_row = tx_size_high[TX_64X32];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    // column transform
    for (int32_t i = 0; i < 32; i++) {
        load_buffer_32_avx2(input + 0 + i * stride, in + 0 + i * 8, 8, 0, 0, shift[0]);
        load_buffer_32_avx2(input + 32 + i * stride, in + 4 + i * 8, 8, 0, 0, shift[0]);
    }

    av1_fdct32_new_N2_avx2(in, in, bitcol, txfm_size_col, num_col);

    for (int32_t i = 0; i < num_col / 2; i++) {
        col_txfm_16x16_rounding((in + i * txfm_size_row), -shift[1]);
    }
    transpose_8nx8n_N2_half(in, outcoef256, txfm_size_col, txfm_size_row);

    // row transform
    av1_fdct64_new_N2_avx2(outcoef256, in, bitrow, txfm_size_row / 2, num_row);
    transpose_8nx8n_N2_quad(in, outcoef256, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_wxh_avx2(outcoef256, outcoef256, -shift[2], new_sqrt2, num_col, txfm_size_row);
    clear_buffer_wxh_N2(outcoef256, num_col, txfm_size_row);
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x64_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[128];
    __m256i*      outcoeff256   = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X64];
    const int32_t txw_idx       = get_txw_idx(TX_16X64);
    const int32_t txh_idx       = get_txh_idx(TX_16X64);
    const int32_t txfm_size_col = tx_size_wide[TX_16X64];
    const int32_t txfm_size_row = tx_size_high[TX_16X64];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    int32_t       ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    const int32_t num_row = txfm_size_row >> 3;
    const int32_t num_col = txfm_size_col >> 3;
    // col tranform
    for (int32_t i = 0; i < txfm_size_row; i += num_col) {
        load_buffer_16_avx2(input + (i + 0) * stride, in + (i + 0) * num_col, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_avx2(input + (i + 1) * stride, in + (i + 1) * num_col, 8, ud_flip, lr_flip, shift[0]);
    }

    av1_fdct64_new_N2_avx2(in, outcoeff256, bitcol, txfm_size_col, num_col);

    col_txfm_16x16_rounding(outcoeff256, -shift[1]);
    col_txfm_16x16_rounding(outcoeff256 + 32, -shift[1]);
    transpose_8nx8n_N2_half(outcoeff256, in, txfm_size_col, txfm_size_row);
    // row tranform
    fdct16x16_N2_avx2(in, in, bitrow, num_row, num_row / 2);
    transpose_8nx8n_N2_quad(in, outcoeff256, txfm_size_row, txfm_size_col);
    clear_buffer_wxh_N2(outcoeff256, num_col, txfm_size_row);
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x16_N2_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[128];
    __m256i*      outcoeff256   = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X16];
    const int32_t txw_idx       = get_txw_idx(TX_64X16);
    const int32_t txh_idx       = get_txh_idx(TX_64X16);
    const int32_t txfm_size_col = tx_size_wide[TX_64X16];
    const int32_t txfm_size_row = tx_size_high[TX_64X16];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    int32_t       ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    const int32_t num_row = txfm_size_row >> 3;
    const int32_t num_col = txfm_size_col >> 3;
    // col tranform
    for (int32_t i = 0; i < txfm_size_row; i++) {
        load_buffer_16_avx2(input + 0 + i * stride, in + 0 + i * 8, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_avx2(input + 16 + i * stride, in + 2 + i * 8, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_avx2(input + 32 + i * stride, in + 4 + i * 8, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_avx2(input + 48 + i * stride, in + 6 + i * 8, 8, ud_flip, lr_flip, shift[0]);
    }

    fdct16x16_N2_avx2(in, outcoeff256, bitcol, num_col, num_col);
    col_txfm_16x16_rounding(outcoeff256, -shift[1]);
    col_txfm_16x16_rounding(outcoeff256 + 32, -shift[1]);
    transpose_8nx8n_N2_half(outcoeff256, in, txfm_size_col, txfm_size_row);
    // row tranform
    av1_fdct64_new_N2_avx2(in, in, bitrow, txfm_size_row / 2, num_row);
    transpose_8nx8n_N2_quad(in, outcoeff256, txfm_size_row, txfm_size_col);
    clear_buffer_wxh_N2(outcoeff256, num_col, txfm_size_row);
    (void)bd;
}

static INLINE void transpose_4x4_in_16x16_avx2(const __m256i* in, __m256i* out) {
    __m256i out1[4];
    __m256i zero = _mm256_setzero_si256();
    TRANSPOSE_4X4_AVX2(in[0], in[2], in[4], in[6], out1[0], out1[1], out1[2], out1[3]);
    out[0] = yy_unpacklo_epi128(out1[0], zero);
    out[2] = yy_unpacklo_epi128(out1[1], zero);
    out[4] = yy_unpacklo_epi128(out1[2], zero);
    out[6] = yy_unpacklo_epi128(out1[3], zero);
}

static INLINE void transpose_8x8_in_32x32_avx2(const __m256i* in, __m256i* out) {
    __m256i temp[8];
    TRANSPOSE_4X4_AVX2(in[0], in[4], in[8], in[12], temp[0], temp[1], temp[2], temp[3]);
    TRANSPOSE_4X4_AVX2(in[16], in[20], in[24], in[28], temp[4], temp[5], temp[6], temp[7]);

    out[0]  = yy_unpacklo_epi128(temp[0], temp[4]);
    out[4]  = yy_unpacklo_epi128(temp[1], temp[5]);
    out[8]  = yy_unpacklo_epi128(temp[2], temp[6]);
    out[12] = yy_unpacklo_epi128(temp[3], temp[7]);
    out[16] = yy_unpackhi_epi128(temp[0], temp[4]);
    out[20] = yy_unpackhi_epi128(temp[1], temp[5]);
    out[24] = yy_unpackhi_epi128(temp[2], temp[6]);
    out[28] = yy_unpackhi_epi128(temp[3], temp[7]);
}

static INLINE void transpose_8nx8n_N4_half(const __m256i* input, __m256i* output, const int32_t width,
                                           const int32_t height) {
    const int32_t numcol      = height >> 3;
    const int32_t numrow      = width >> 3;
    int32_t       calc_numcol = numcol >> 2;
    if (!calc_numcol) {
        calc_numcol = 1;
    }

    __m256i out1[8];
    for (int32_t j = 0; j < numrow; j++) {
        for (int32_t i = 0; i < calc_numcol; i++) {
            TRANSPOSE_4X4_AVX2(input[i * width + j + (numrow * 0)],
                               input[i * width + j + (numrow * 1)],
                               input[i * width + j + (numrow * 2)],
                               input[i * width + j + (numrow * 3)],
                               out1[0],
                               out1[1],
                               out1[4],
                               out1[5]);
            TRANSPOSE_4X4_AVX2(input[i * width + j + (numrow * 4)],
                               input[i * width + j + (numrow * 5)],
                               input[i * width + j + (numrow * 6)],
                               input[i * width + j + (numrow * 7)],
                               out1[2],
                               out1[3],
                               out1[6],
                               out1[7]);
            output[j * height + i + (numcol * 0)] = yy_unpacklo_epi128(out1[0], out1[2]);
            output[j * height + i + (numcol * 1)] = yy_unpacklo_epi128(out1[1], out1[3]);
            output[j * height + i + (numcol * 2)] = yy_unpacklo_epi128(out1[4], out1[6]);
            output[j * height + i + (numcol * 3)] = yy_unpacklo_epi128(out1[5], out1[7]);
            output[j * height + i + (numcol * 4)] = yy_unpackhi_epi128(out1[0], out1[2]);
            output[j * height + i + (numcol * 5)] = yy_unpackhi_epi128(out1[1], out1[3]);
            output[j * height + i + (numcol * 6)] = yy_unpackhi_epi128(out1[4], out1[6]);
            output[j * height + i + (numcol * 7)] = yy_unpackhi_epi128(out1[5], out1[7]);
        }
    }
}

static INLINE void transpose_8nx8n_N4_quad(const __m256i* input, __m256i* output, const int32_t width,
                                           const int32_t height) {
    const int32_t numcol = height >> 3;
    const int32_t numrow = width >> 3;

    int32_t calc_numcol = numcol >> 2;
    int32_t calc_numrow = numrow >> 2;
    if (!calc_numcol) {
        calc_numcol = 1;
    }
    if (!calc_numrow) {
        calc_numrow = 1;
    }

    __m256i out1[8];
    for (int32_t j = 0; j < calc_numrow; j++) {
        for (int32_t i = 0; i < calc_numcol; i++) {
            TRANSPOSE_4X4_AVX2(input[i * width + j + (numrow * 0)],
                               input[i * width + j + (numrow * 1)],
                               input[i * width + j + (numrow * 2)],
                               input[i * width + j + (numrow * 3)],
                               out1[0],
                               out1[1],
                               out1[4],
                               out1[5]);
            TRANSPOSE_4X4_AVX2(input[i * width + j + (numrow * 4)],
                               input[i * width + j + (numrow * 5)],
                               input[i * width + j + (numrow * 6)],
                               input[i * width + j + (numrow * 7)],
                               out1[2],
                               out1[3],
                               out1[6],
                               out1[7]);
            output[j * height + i + (numcol * 0)] = yy_unpacklo_epi128(out1[0], out1[2]);
            output[j * height + i + (numcol * 1)] = yy_unpacklo_epi128(out1[1], out1[3]);
            output[j * height + i + (numcol * 2)] = yy_unpacklo_epi128(out1[4], out1[6]);
            output[j * height + i + (numcol * 3)] = yy_unpacklo_epi128(out1[5], out1[7]);
            output[j * height + i + (numcol * 4)] = yy_unpackhi_epi128(out1[0], out1[2]);
            output[j * height + i + (numcol * 5)] = yy_unpackhi_epi128(out1[1], out1[3]);
            output[j * height + i + (numcol * 6)] = yy_unpackhi_epi128(out1[4], out1[6]);
            output[j * height + i + (numcol * 7)] = yy_unpackhi_epi128(out1[5], out1[7]);
        }
    }
}

static INLINE void transpose_4x8_in_4x16_quad_avx2(const __m256i* in, __m256i* out) {
    __m256i perm = _mm256_set_epi32(7, 3, 6, 2, 5, 1, 4, 0);
    __m256i u0, u1;

    u0     = _mm256_unpacklo_epi32(in[0], in[1]);
    u1     = _mm256_unpacklo_epi32(in[2], in[3]);
    out[0] = _mm256_unpacklo_epi64(u0, u1);
    out[0] = _mm256_permutevar8x32_epi32(out[0], perm);
}

static AOM_FORCE_INLINE void write_buffer_8x8_N4(const __m256i* res, int32_t* output) {
    const __m256i zero256 = _mm256_setzero_si256();

    _mm256_storeu_si256((__m256i*)(output + 0 * 8), zero256);
    _mm256_storeu_si256((__m256i*)(output + 1 * 8), zero256);

    _mm_storel_epi64((__m128i*)(output + 0 * 8), _mm256_castsi256_si128(res[0]));
    _mm_storel_epi64((__m128i*)(output + 1 * 8), _mm256_castsi256_si128(res[1]));

    _mm256_storeu_si256((__m256i*)(output + 2 * 8), zero256);
    _mm256_storeu_si256((__m256i*)(output + 3 * 8), zero256);
    _mm256_storeu_si256((__m256i*)(output + 4 * 8), zero256);
    _mm256_storeu_si256((__m256i*)(output + 5 * 8), zero256);
    _mm256_storeu_si256((__m256i*)(output + 6 * 8), zero256);
    _mm256_storeu_si256((__m256i*)(output + 7 * 8), zero256);
}

static INLINE void write_buffer_16x16_N4(const __m256i* res, int32_t* output) {
    int32_t       fact = -1, index = -1;
    const __m256i zero    = _mm256_setzero_si256();
    const __m128i zero128 = _mm_setzero_si128();
    int32_t       i;
    for (i = 0; i < 2; i++) {
        _mm_storeu_si128((__m128i*)(output + (++fact) * 16), _mm256_castsi256_si128(res[++index]));
        _mm_storeu_si128((__m128i*)(output + fact * 16 + 4), zero128);
        _mm256_storeu_si256((__m256i*)(output + fact * 16 + 8), zero);
        ++index;
        _mm_storeu_si128((__m128i*)(output + (++fact) * 16), _mm256_castsi256_si128(res[++index]));
        _mm_storeu_si128((__m128i*)(output + fact * 16 + 4), zero128);
        _mm256_storeu_si256((__m256i*)(output + fact * 16 + 8), zero);
        ++index;
    }
    for (; i < 8; i++) {
        _mm256_storeu_si256((__m256i*)(output + (++fact) * 16), zero);
        _mm256_storeu_si256((__m256i*)(output + (fact) * 16 + 8), zero);
        _mm256_storeu_si256((__m256i*)(output + (++fact) * 16), zero);
        _mm256_storeu_si256((__m256i*)(output + (fact) * 16 + 8), zero);
    }
}

static INLINE void write_buffer_32x32_N4(const __m256i* res, int32_t* output) {
    const __m256i zero = _mm256_setzero_si256();
    uint32_t      i;

    for (i = 0; i < 8; i++) {
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 0), res[i * 4]);
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 8), zero);
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 16), zero);
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 24), zero);
    }

    for (; i < 32; i++) {
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 0), zero);
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 8), zero);
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 16), zero);
        _mm256_storeu_si256((__m256i*)(output + i * 32 + 24), zero);
    }
}

static AOM_FORCE_INLINE void write_buffer_16x8_N4_avx2(const __m256i* res, __m256i* out) {
    out[0] = res[0];
    out[2] = res[1];
}

static AOM_FORCE_INLINE void col_txfm_8x8_N4_rounding(__m256i* in, int32_t shift) {
    const __m256i rounding = _mm256_set1_epi32(1 << (shift - 1));

    in[0] = _mm256_add_epi32(in[0], rounding);
    in[1] = _mm256_add_epi32(in[1], rounding);

    in[0] = _mm256_srai_epi32(in[0], shift);
    in[1] = _mm256_srai_epi32(in[1], shift);
}

static AOM_FORCE_INLINE void col_txfm_32x8_N4_rounding(__m256i* in, int32_t shift) {
    const __m256i rounding = _mm256_set1_epi32(1 << (shift - 1));

    in[0] = _mm256_add_epi32(in[0], rounding);
    in[4] = _mm256_add_epi32(in[4], rounding);

    in[0] = _mm256_srai_epi32(in[0], shift);
    in[4] = _mm256_srai_epi32(in[4], shift);
}

static AOM_FORCE_INLINE void col_txfm_16x16_N4_rounding(__m256i* in, int32_t shift) {
    col_txfm_8x8_rounding(&in[0], shift);
}

static INLINE void av1_round_shift_array_32_N4_avx2(__m256i* input, __m256i* output, const int32_t size,
                                                    const int32_t bit) {
    int32_t i;
    if (bit > 0) {
        const __m256i round = _mm256_set1_epi32(1 << (bit - 1));
        __m256i       r0;
        for (i = 0; i < size; i += 4) {
            r0        = _mm256_add_epi32(input[i], round);
            output[i] = _mm256_srai_epi32(r0, bit);
        }
    } else {
        for (i = 0; i < size; i += 4) {
            output[i] = _mm256_slli_epi32(input[i], -bit);
        }
    }
}

static INLINE void av1_round_shift_rect_wxh_N4(__m256i* input, __m256i* output, const int32_t bit, const int32_t val,
                                               int32_t num_col, int32_t num_row) {
    const __m256i sqrt2       = _mm256_set1_epi32(val);
    const __m256i round2      = _mm256_set1_epi32(1 << (new_sqrt2_bits - 1));
    int32_t       calc_numcol = num_col >> 2;
    if (!calc_numcol) {
        calc_numcol = 1;
    }
    int32_t i;
    if (bit > 0) {
        const __m256i round1 = _mm256_set1_epi32(1 << (bit - 1));
        __m256i       r0, r1, r2, r3;
        for (i = 0; i < num_row / 4; i++) {
            for (int j = 0; j < calc_numcol; j++) {
                r0                      = _mm256_add_epi32(input[i * num_col + j], round1);
                r1                      = _mm256_srai_epi32(r0, bit);
                r2                      = _mm256_mullo_epi32(sqrt2, r1);
                r3                      = _mm256_add_epi32(r2, round2);
                output[i * num_col + j] = _mm256_srai_epi32(r3, new_sqrt2_bits);
            }
        }
    } else {
        __m256i r0, r1, r2;
        for (i = 0; i < num_row / 4; i++) {
            for (int j = 0; j < calc_numcol; j++) {
                r0                      = _mm256_slli_epi32(input[i * num_col + j], -bit);
                r1                      = _mm256_mullo_epi32(sqrt2, r0);
                r2                      = _mm256_add_epi32(r1, round2);
                output[i * num_col + j] = _mm256_srai_epi32(r2, new_sqrt2_bits);
            }
        }
    }
}

static AOM_FORCE_INLINE void col_txfm_32x16_N4_rounding(__m256i* in, int32_t shift) {
    const __m256i rounding = _mm256_set1_epi32(1 << (shift - 1));
    in[0]                  = _mm256_add_epi32(in[0], rounding);
    in[4]                  = _mm256_add_epi32(in[4], rounding);
    in[8]                  = _mm256_add_epi32(in[8], rounding);
    in[12]                 = _mm256_add_epi32(in[12], rounding);
    in[0]                  = _mm256_srai_epi32(in[0], shift);
    in[4]                  = _mm256_srai_epi32(in[4], shift);
    in[8]                  = _mm256_srai_epi32(in[8], shift);
    in[12]                 = _mm256_srai_epi32(in[12], shift);
}

static INLINE void load_buffer_8x8_in_32x32_avx2(const int16_t* input, __m256i* output, int32_t stride) {
    __m128i temp;
    int32_t i;

    for (i = 0; i < 8; ++i) {
        temp      = _mm_loadu_si128((const __m128i*)(input));
        output[0] = _mm256_cvtepi16_epi32(temp);

        input += stride;
        output += 4;
    }
}

static INLINE void load_buffer_8x32_in_32x32_avx2(const int16_t* input, __m256i* output, int32_t stride) {
    __m128i temp;
    int32_t i;

    for (i = 0; i < 32; ++i) {
        temp      = _mm_loadu_si128((const __m128i*)(input));
        output[0] = _mm256_cvtepi16_epi32(temp);

        input += stride;
        output += 4;
    }
}

static INLINE void load_buffer_32x8_in_32x32_avx2(const int16_t* input, __m256i* output, int32_t stride) {
    __m128i temp[4];
    int32_t i;

    for (i = 0; i < 8; ++i) {
        temp[0] = _mm_loadu_si128((const __m128i*)(input + 0 * 8));
        temp[1] = _mm_loadu_si128((const __m128i*)(input + 1 * 8));
        temp[2] = _mm_loadu_si128((const __m128i*)(input + 2 * 8));
        temp[3] = _mm_loadu_si128((const __m128i*)(input + 3 * 8));

        output[0] = _mm256_cvtepi16_epi32(temp[0]);
        output[1] = _mm256_cvtepi16_epi32(temp[1]);
        output[2] = _mm256_cvtepi16_epi32(temp[2]);
        output[3] = _mm256_cvtepi16_epi32(temp[3]);
        input += stride;
        output += 4;
    }
}

static INLINE void load_buffer_16x16_in_64x64_avx2(const int16_t* input, int32_t stride, __m256i* output) {
    __m128i x0, x1;
    __m256i v0, v1;
    int32_t i;

    for (i = 0; i < 16; ++i) {
        x0 = _mm_loadu_si128((const __m128i*)(input + 0 * 8));
        x1 = _mm_loadu_si128((const __m128i*)(input + 1 * 8));

        v0 = _mm256_cvtepi16_epi32(x0);
        v1 = _mm256_cvtepi16_epi32(x1);

        _mm256_storeu_si256(output + 0, v0);
        _mm256_storeu_si256(output + 1, v1);

        input += stride;
        output += 8;
    }
}

static AOM_FORCE_INLINE void load_buffer_8x16_N4(const int16_t* input, __m256i* out, int32_t stride, int32_t flipud,
                                                 int32_t fliplr, int32_t shift) {
    if (flipud) {
        load_buffer_8x8(input + 8 * stride, out, stride, flipud, fliplr, shift);
    } else {
        load_buffer_8x8(input, out, stride, flipud, fliplr, shift);
    }
}

static INLINE void clear_buffer_wxh_N4(__m256i* buff, int32_t num_col, int32_t num_row) {
    const __m256i zero    = _mm256_setzero_si256();
    const __m128i zero128 = _mm_setzero_si128();
    assert(num_col > 0);
    assert(num_row > 1);

    if (num_col == 1) {
        __m128i* ptr_128b = (__m128i*)buff;
        for (int i = 0; i < num_row / 4; i++) {
            ptr_128b[i * 2]     = _mm_unpacklo_epi64(ptr_128b[i * 2], zero128);
            ptr_128b[i * 2 + 1] = zero128;
        }
    } else if (num_col == 2) {
        __m128i* ptr_128b = (__m128i*)buff;
        for (int i = 0; i < num_row / 4; i++) {
            ptr_128b[i * 4 + 1] = zero128;
            buff[i * 2 + 1]     = zero;
        }
    } else {
        for (int i = 0; i < num_row / 4; i++) {
            for (int j = num_col / 4; j < num_col; j++) {
                buff[i * num_col + j] = zero;
            }
        }
    }
    //clear bottom
    for (int i = num_row / 4; i < num_row; i++) {
        for (int j = 0; j < num_col; j++) {
            buff[i * num_col + j] = zero;
        }
    }
}

static AOM_FORCE_INLINE void clear_buffer_4x16_N4(__m256i* buff) {
    const __m256i zero = _mm256_setzero_si256();
    buff[2]            = zero;
    buff[3]            = zero;
    buff[4]            = zero;
    buff[5]            = zero;
    buff[6]            = zero;
    buff[7]            = zero;
}

static INLINE void av1_round_shift_array_64_N4_avx2(__m256i* input, __m256i* output, const int32_t size,
                                                    const int32_t bit) {
    int32_t i;
    if (bit > 0) {
        const __m256i round = _mm256_set1_epi32(1 << (bit - 1));
        __m256i       r0;
        for (i = 0; i < size; i += 8) {
            r0            = _mm256_add_epi32(input[i], round);
            output[i]     = _mm256_srai_epi32(r0, bit);
            r0            = _mm256_add_epi32(input[i + 1], round);
            output[i + 1] = _mm256_srai_epi32(r0, bit);
        }
    } else {
        for (i = 0; i < size; i += 8) {
            output[i]     = _mm256_slli_epi32(input[i], -bit);
            output[i + 1] = _mm256_slli_epi32(input[i + 1], -bit);
        }
    }
}

static void fdct8x8_N4_avx2(const __m256i* in, __m256i* out, int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i  cospi56  = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi8   = _mm256_set1_epi32(cospi[8]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    __m256i        u[8], v[8];

    // stage 0
    // stage 1
    u[0] = _mm256_add_epi32(in[0 * col_num], in[7 * col_num]);
    v[7] = _mm256_sub_epi32(in[0 * col_num], in[7 * col_num]);
    u[1] = _mm256_add_epi32(in[1 * col_num], in[6 * col_num]);
    u[6] = _mm256_sub_epi32(in[1 * col_num], in[6 * col_num]);
    u[2] = _mm256_add_epi32(in[2 * col_num], in[5 * col_num]);
    u[5] = _mm256_sub_epi32(in[2 * col_num], in[5 * col_num]);
    u[3] = _mm256_add_epi32(in[3 * col_num], in[4 * col_num]);
    v[4] = _mm256_sub_epi32(in[3 * col_num], in[4 * col_num]);

    // stage 2
    v[0] = _mm256_add_epi32(u[0], u[3]);
    v[1] = _mm256_add_epi32(u[1], u[2]);

    v[5] = _mm256_mullo_epi32(u[5], cospim32);
    v[6] = _mm256_mullo_epi32(u[6], cospi32);
    v[5] = _mm256_add_epi32(v[5], v[6]);
    v[5] = _mm256_add_epi32(v[5], rnding);
    v[5] = _mm256_srai_epi32(v[5], bit);

    u[0] = _mm256_mullo_epi32(u[5], cospi32);
    v[6] = _mm256_mullo_epi32(u[6], cospim32);
    v[6] = _mm256_sub_epi32(u[0], v[6]);
    v[6] = _mm256_add_epi32(v[6], rnding);
    v[6] = _mm256_srai_epi32(v[6], bit);

    // stage 3
    // type 0
    v[0] = _mm256_mullo_epi32(v[0], cospi32);
    v[1] = _mm256_mullo_epi32(v[1], cospi32);
    u[0] = _mm256_add_epi32(v[0], v[1]);
    u[0] = _mm256_add_epi32(u[0], rnding);
    u[0] = _mm256_srai_epi32(u[0], bit);

    u[4] = _mm256_add_epi32(v[4], v[5]);
    u[7] = _mm256_add_epi32(v[7], v[6]);

    // stage 4
    // stage 5
    v[0]             = _mm256_mullo_epi32(u[4], cospi56);
    v[1]             = _mm256_mullo_epi32(u[7], cospi8);
    v[0]             = _mm256_add_epi32(v[0], v[1]);
    v[0]             = _mm256_add_epi32(v[0], rnding);
    out[1 * col_num] = _mm256_srai_epi32(v[0], bit);

    out[0 * col_num] = u[0];
}

static void fadst8x8_N4_avx2(const __m256i* in, __m256i* out, int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospim4  = _mm256_set1_epi32(-cospi[4]);
    const __m256i  cospi60  = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi52  = _mm256_set1_epi32(cospi[52]);
    const __m256i  cospi12  = _mm256_set1_epi32(cospi[12]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  zero     = _mm256_setzero_si256();
    __m256i        u0, u1, u2, u3, u4, u5, u6, u7;
    __m256i        v0, v1, v2, v3, v4, v5, v6, v7;
    __m256i        x, y;

    u0 = in[0 * col_num];
    u1 = _mm256_sub_epi32(zero, in[7 * col_num]);
    u2 = _mm256_sub_epi32(zero, in[3 * col_num]);
    u3 = in[4 * col_num];
    u4 = _mm256_sub_epi32(zero, in[1 * col_num]);
    u5 = in[6 * col_num];
    u6 = in[2 * col_num];
    u7 = _mm256_sub_epi32(zero, in[5 * col_num]);

    // stage 2
    v0 = u0;
    v1 = u1;

    x  = _mm256_mullo_epi32(u2, cospi32);
    y  = _mm256_mullo_epi32(u3, cospi32);
    v2 = _mm256_add_epi32(x, y);
    v2 = _mm256_add_epi32(v2, rnding);
    v2 = _mm256_srai_epi32(v2, bit);

    v3 = _mm256_sub_epi32(x, y);
    v3 = _mm256_add_epi32(v3, rnding);
    v3 = _mm256_srai_epi32(v3, bit);

    v4 = u4;
    v5 = u5;

    x  = _mm256_mullo_epi32(u6, cospi32);
    y  = _mm256_mullo_epi32(u7, cospi32);
    v6 = _mm256_add_epi32(x, y);
    v6 = _mm256_add_epi32(v6, rnding);
    v6 = _mm256_srai_epi32(v6, bit);

    v7 = _mm256_sub_epi32(x, y);
    v7 = _mm256_add_epi32(v7, rnding);
    v7 = _mm256_srai_epi32(v7, bit);

    // stage 3
    u0 = _mm256_add_epi32(v0, v2);
    u1 = _mm256_add_epi32(v1, v3);
    u2 = _mm256_sub_epi32(v0, v2);
    u3 = _mm256_sub_epi32(v1, v3);
    u4 = _mm256_add_epi32(v4, v6);
    u5 = _mm256_add_epi32(v5, v7);
    u6 = _mm256_sub_epi32(v4, v6);
    u7 = _mm256_sub_epi32(v5, v7);

    // stage 4
    v0 = u0;
    v1 = u1;
    v2 = u2;
    v3 = u3;

    x  = _mm256_mullo_epi32(u4, cospi16);
    y  = _mm256_mullo_epi32(u5, cospi48);
    v4 = _mm256_add_epi32(x, y);
    v4 = _mm256_add_epi32(v4, rnding);
    v4 = _mm256_srai_epi32(v4, bit);

    x  = _mm256_mullo_epi32(u4, cospi48);
    y  = _mm256_mullo_epi32(u5, cospim16);
    v5 = _mm256_add_epi32(x, y);
    v5 = _mm256_add_epi32(v5, rnding);
    v5 = _mm256_srai_epi32(v5, bit);

    x  = _mm256_mullo_epi32(u6, cospim48);
    y  = _mm256_mullo_epi32(u7, cospi16);
    v6 = _mm256_add_epi32(x, y);
    v6 = _mm256_add_epi32(v6, rnding);
    v6 = _mm256_srai_epi32(v6, bit);

    x  = _mm256_mullo_epi32(u6, cospi16);
    y  = _mm256_mullo_epi32(u7, cospi48);
    v7 = _mm256_add_epi32(x, y);
    v7 = _mm256_add_epi32(v7, rnding);
    v7 = _mm256_srai_epi32(v7, bit);

    // stage 5
    u0 = _mm256_add_epi32(v0, v4);
    u1 = _mm256_add_epi32(v1, v5);
    u6 = _mm256_sub_epi32(v2, v6);
    u7 = _mm256_sub_epi32(v3, v7);

    // stage 6
    x  = _mm256_mullo_epi32(u0, cospi60);
    y  = _mm256_mullo_epi32(u1, cospim4);
    v1 = _mm256_add_epi32(x, y);
    v1 = _mm256_add_epi32(v1, rnding);
    v1 = _mm256_srai_epi32(v1, bit);

    x  = _mm256_mullo_epi32(u6, cospi52);
    y  = _mm256_mullo_epi32(u7, cospi12);
    v6 = _mm256_add_epi32(x, y);
    v6 = _mm256_add_epi32(v6, rnding);
    v6 = _mm256_srai_epi32(v6, bit);

    // stage 7
    out[0 * col_num] = v1;
    out[1 * col_num] = v6;
}

static void fidtx8x8_N4_avx2(const __m256i* in, __m256i* out, int8_t bit, int32_t col_num) {
    (void)bit;
    out[0] = _mm256_slli_epi32(in[0 * col_num], 1);
    out[1] = _mm256_slli_epi32(in[1 * col_num], 1);
}

static void fdct16x16_N4_avx2(const __m256i* in, __m256i* out, int8_t bit, const int32_t col_num, int32_t size) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospim32 = _mm256_set1_epi32(-cospi[32]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospi56  = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospi8   = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi60  = _mm256_set1_epi32(cospi[60]);
    const __m256i  cospi4   = _mm256_set1_epi32(cospi[4]);
    const __m256i  cospi12  = _mm256_set1_epi32(cospi[12]);
    const __m256i  cospi52  = _mm256_set1_epi32(cospi[52]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    __m256i        u[16], v[16], x;
    int32_t        col;

    for (col = 0; col < size; ++col) {
        // stage 0
        // stage 1
        u[0]  = _mm256_add_epi32(in[0 * col_num + col], in[15 * col_num + col]);
        u[15] = _mm256_sub_epi32(in[0 * col_num + col], in[15 * col_num + col]);
        u[1]  = _mm256_add_epi32(in[1 * col_num + col], in[14 * col_num + col]);
        u[14] = _mm256_sub_epi32(in[1 * col_num + col], in[14 * col_num + col]);
        u[2]  = _mm256_add_epi32(in[2 * col_num + col], in[13 * col_num + col]);
        u[13] = _mm256_sub_epi32(in[2 * col_num + col], in[13 * col_num + col]);
        u[3]  = _mm256_add_epi32(in[3 * col_num + col], in[12 * col_num + col]);
        u[12] = _mm256_sub_epi32(in[3 * col_num + col], in[12 * col_num + col]);
        u[4]  = _mm256_add_epi32(in[4 * col_num + col], in[11 * col_num + col]);
        u[11] = _mm256_sub_epi32(in[4 * col_num + col], in[11 * col_num + col]);
        u[5]  = _mm256_add_epi32(in[5 * col_num + col], in[10 * col_num + col]);
        u[10] = _mm256_sub_epi32(in[5 * col_num + col], in[10 * col_num + col]);
        u[6]  = _mm256_add_epi32(in[6 * col_num + col], in[9 * col_num + col]);
        u[9]  = _mm256_sub_epi32(in[6 * col_num + col], in[9 * col_num + col]);
        u[7]  = _mm256_add_epi32(in[7 * col_num + col], in[8 * col_num + col]);
        u[8]  = _mm256_sub_epi32(in[7 * col_num + col], in[8 * col_num + col]);

        // stage 2
        v[0] = _mm256_add_epi32(u[0], u[7]);
        v[7] = _mm256_sub_epi32(u[0], u[7]);
        v[1] = _mm256_add_epi32(u[1], u[6]);
        v[6] = _mm256_sub_epi32(u[1], u[6]);
        v[2] = _mm256_add_epi32(u[2], u[5]);
        v[5] = _mm256_sub_epi32(u[2], u[5]);
        v[3] = _mm256_add_epi32(u[3], u[4]);
        v[4] = _mm256_sub_epi32(u[3], u[4]);
        v[8] = u[8];
        v[9] = u[9];

        v[10] = _mm256_mullo_epi32(u[10], cospim32);
        x     = _mm256_mullo_epi32(u[13], cospi32);
        v[10] = _mm256_add_epi32(v[10], x);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[13] = _mm256_mullo_epi32(u[10], cospi32);
        x     = _mm256_mullo_epi32(u[13], cospim32);
        v[13] = _mm256_sub_epi32(v[13], x);
        v[13] = _mm256_add_epi32(v[13], rnding);
        v[13] = _mm256_srai_epi32(v[13], bit);

        v[11] = _mm256_mullo_epi32(u[11], cospim32);
        x     = _mm256_mullo_epi32(u[12], cospi32);
        v[11] = _mm256_add_epi32(v[11], x);
        v[11] = _mm256_add_epi32(v[11], rnding);
        v[11] = _mm256_srai_epi32(v[11], bit);

        v[12] = _mm256_mullo_epi32(u[11], cospi32);
        x     = _mm256_mullo_epi32(u[12], cospim32);
        v[12] = _mm256_sub_epi32(v[12], x);
        v[12] = _mm256_add_epi32(v[12], rnding);
        v[12] = _mm256_srai_epi32(v[12], bit);
        v[14] = u[14];
        v[15] = u[15];

        // stage 3
        u[0] = _mm256_add_epi32(v[0], v[3]);
        u[1] = _mm256_add_epi32(v[1], v[2]);
        u[4] = v[4];

        u[5] = _mm256_mullo_epi32(v[5], cospim32);
        x    = _mm256_mullo_epi32(v[6], cospi32);
        u[5] = _mm256_add_epi32(u[5], x);
        u[5] = _mm256_add_epi32(u[5], rnding);
        u[5] = _mm256_srai_epi32(u[5], bit);

        u[6] = _mm256_mullo_epi32(v[5], cospi32);
        x    = _mm256_mullo_epi32(v[6], cospim32);
        u[6] = _mm256_sub_epi32(u[6], x);
        u[6] = _mm256_add_epi32(u[6], rnding);
        u[6] = _mm256_srai_epi32(u[6], bit);

        u[7]  = v[7];
        u[8]  = _mm256_add_epi32(v[8], v[11]);
        u[11] = _mm256_sub_epi32(v[8], v[11]);
        u[9]  = _mm256_add_epi32(v[9], v[10]);
        u[10] = _mm256_sub_epi32(v[9], v[10]);
        u[12] = _mm256_sub_epi32(v[15], v[12]);
        u[15] = _mm256_add_epi32(v[15], v[12]);
        u[13] = _mm256_sub_epi32(v[14], v[13]);
        u[14] = _mm256_add_epi32(v[14], v[13]);

        // stage 4
        u[0] = _mm256_mullo_epi32(u[0], cospi32);
        u[1] = _mm256_mullo_epi32(u[1], cospi32);
        v[0] = _mm256_add_epi32(u[0], u[1]);
        v[0] = _mm256_add_epi32(v[0], rnding);
        v[0] = _mm256_srai_epi32(v[0], bit);

        v[4] = _mm256_add_epi32(u[4], u[5]);
        v[7] = _mm256_add_epi32(u[7], u[6]);
        v[8] = u[8];

        v[9] = _mm256_mullo_epi32(u[9], cospim16);
        x    = _mm256_mullo_epi32(u[14], cospi48);
        v[9] = _mm256_add_epi32(v[9], x);
        v[9] = _mm256_add_epi32(v[9], rnding);
        v[9] = _mm256_srai_epi32(v[9], bit);

        v[14] = _mm256_mullo_epi32(u[9], cospi48);
        x     = _mm256_mullo_epi32(u[14], cospim16);
        v[14] = _mm256_sub_epi32(v[14], x);
        v[14] = _mm256_add_epi32(v[14], rnding);
        v[14] = _mm256_srai_epi32(v[14], bit);

        v[10] = _mm256_mullo_epi32(u[10], cospim48);
        x     = _mm256_mullo_epi32(u[13], cospim16);
        v[10] = _mm256_add_epi32(v[10], x);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[13] = _mm256_mullo_epi32(u[10], cospim16);
        x     = _mm256_mullo_epi32(u[13], cospim48);
        v[13] = _mm256_sub_epi32(v[13], x);
        v[13] = _mm256_add_epi32(v[13], rnding);
        v[13] = _mm256_srai_epi32(v[13], bit);

        v[11] = u[11];
        v[12] = u[12];
        v[15] = u[15];

        // stage 5
        u[0] = v[0];

        u[4] = _mm256_mullo_epi32(v[4], cospi56);
        x    = _mm256_mullo_epi32(v[7], cospi8);
        u[4] = _mm256_add_epi32(u[4], x);
        u[4] = _mm256_add_epi32(u[4], rnding);
        u[4] = _mm256_srai_epi32(u[4], bit);

        u[8]  = _mm256_add_epi32(v[8], v[9]);
        u[11] = _mm256_add_epi32(v[11], v[10]);
        u[12] = _mm256_add_epi32(v[12], v[13]);
        u[15] = _mm256_add_epi32(v[15], v[14]);

        // stage 6
        v[0] = u[0];
        v[4] = u[4];

        v[8] = _mm256_mullo_epi32(u[8], cospi60);
        x    = _mm256_mullo_epi32(u[15], cospi4);
        v[8] = _mm256_add_epi32(v[8], x);
        v[8] = _mm256_add_epi32(v[8], rnding);
        v[8] = _mm256_srai_epi32(v[8], bit);

        v[12] = _mm256_mullo_epi32(u[11], cospi52);
        x     = _mm256_mullo_epi32(u[12], cospi12);
        v[12] = _mm256_sub_epi32(x, v[12]);
        v[12] = _mm256_add_epi32(v[12], rnding);
        v[12] = _mm256_srai_epi32(v[12], bit);

        out[0 * col_num + col] = v[0];
        out[1 * col_num + col] = v[8];
        out[2 * col_num + col] = v[4];
        out[3 * col_num + col] = v[12];
    }
}

static void fadst16x16_N4_avx2(const __m256i* in, __m256i* out, int8_t bit, const int32_t col_num, int32_t size) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  cospi48  = _mm256_set1_epi32(cospi[48]);
    const __m256i  cospi16  = _mm256_set1_epi32(cospi[16]);
    const __m256i  cospim16 = _mm256_set1_epi32(-cospi[16]);
    const __m256i  cospim48 = _mm256_set1_epi32(-cospi[48]);
    const __m256i  cospi8   = _mm256_set1_epi32(cospi[8]);
    const __m256i  cospi56  = _mm256_set1_epi32(cospi[56]);
    const __m256i  cospim56 = _mm256_set1_epi32(-cospi[56]);
    const __m256i  cospim8  = _mm256_set1_epi32(-cospi[8]);
    const __m256i  cospi24  = _mm256_set1_epi32(cospi[24]);
    const __m256i  cospim24 = _mm256_set1_epi32(-cospi[24]);
    const __m256i  cospim40 = _mm256_set1_epi32(-cospi[40]);
    const __m256i  cospi40  = _mm256_set1_epi32(cospi[40]);
    const __m256i  cospi62  = _mm256_set1_epi32(cospi[62]);
    const __m256i  cospim2  = _mm256_set1_epi32(-cospi[2]);
    const __m256i  cospi54  = _mm256_set1_epi32(cospi[54]);
    const __m256i  cospim10 = _mm256_set1_epi32(-cospi[10]);
    const __m256i  cospi50  = _mm256_set1_epi32(cospi[50]);
    const __m256i  cospi14  = _mm256_set1_epi32(cospi[14]);
    const __m256i  cospi58  = _mm256_set1_epi32(cospi[58]);
    const __m256i  cospi6   = _mm256_set1_epi32(cospi[6]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  zero     = _mm256_setzero_si256();

    __m256i u[16], v[16], x, y;
    int32_t col;

    for (col = 0; col < size; ++col) {
        // stage 0
        // stage 1
        u[0]  = in[0 * col_num + col];
        u[1]  = _mm256_sub_epi32(zero, in[15 * col_num + col]);
        u[2]  = _mm256_sub_epi32(zero, in[7 * col_num + col]);
        u[3]  = in[8 * col_num + col];
        u[4]  = _mm256_sub_epi32(zero, in[3 * col_num + col]);
        u[5]  = in[12 * col_num + col];
        u[6]  = in[4 * col_num + col];
        u[7]  = _mm256_sub_epi32(zero, in[11 * col_num + col]);
        u[8]  = _mm256_sub_epi32(zero, in[1 * col_num + col]);
        u[9]  = in[14 * col_num + col];
        u[10] = in[6 * col_num + col];
        u[11] = _mm256_sub_epi32(zero, in[9 * col_num + col]);
        u[12] = in[2 * col_num + col];
        u[13] = _mm256_sub_epi32(zero, in[13 * col_num + col]);
        u[14] = _mm256_sub_epi32(zero, in[5 * col_num + col]);
        u[15] = in[10 * col_num + col];

        // stage 2
        v[0] = u[0];
        v[1] = u[1];

        x    = _mm256_mullo_epi32(u[2], cospi32);
        y    = _mm256_mullo_epi32(u[3], cospi32);
        v[2] = _mm256_add_epi32(x, y);
        v[2] = _mm256_add_epi32(v[2], rnding);
        v[2] = _mm256_srai_epi32(v[2], bit);

        v[3] = _mm256_sub_epi32(x, y);
        v[3] = _mm256_add_epi32(v[3], rnding);
        v[3] = _mm256_srai_epi32(v[3], bit);

        v[4] = u[4];
        v[5] = u[5];

        x    = _mm256_mullo_epi32(u[6], cospi32);
        y    = _mm256_mullo_epi32(u[7], cospi32);
        v[6] = _mm256_add_epi32(x, y);
        v[6] = _mm256_add_epi32(v[6], rnding);
        v[6] = _mm256_srai_epi32(v[6], bit);

        v[7] = _mm256_sub_epi32(x, y);
        v[7] = _mm256_add_epi32(v[7], rnding);
        v[7] = _mm256_srai_epi32(v[7], bit);

        v[8] = u[8];
        v[9] = u[9];

        x     = _mm256_mullo_epi32(u[10], cospi32);
        y     = _mm256_mullo_epi32(u[11], cospi32);
        v[10] = _mm256_add_epi32(x, y);
        v[10] = _mm256_add_epi32(v[10], rnding);
        v[10] = _mm256_srai_epi32(v[10], bit);

        v[11] = _mm256_sub_epi32(x, y);
        v[11] = _mm256_add_epi32(v[11], rnding);
        v[11] = _mm256_srai_epi32(v[11], bit);

        v[12] = u[12];
        v[13] = u[13];

        x     = _mm256_mullo_epi32(u[14], cospi32);
        y     = _mm256_mullo_epi32(u[15], cospi32);
        v[14] = _mm256_add_epi32(x, y);
        v[14] = _mm256_add_epi32(v[14], rnding);
        v[14] = _mm256_srai_epi32(v[14], bit);

        v[15] = _mm256_sub_epi32(x, y);
        v[15] = _mm256_add_epi32(v[15], rnding);
        v[15] = _mm256_srai_epi32(v[15], bit);

        // stage 3
        u[0]  = _mm256_add_epi32(v[0], v[2]);
        u[1]  = _mm256_add_epi32(v[1], v[3]);
        u[2]  = _mm256_sub_epi32(v[0], v[2]);
        u[3]  = _mm256_sub_epi32(v[1], v[3]);
        u[4]  = _mm256_add_epi32(v[4], v[6]);
        u[5]  = _mm256_add_epi32(v[5], v[7]);
        u[6]  = _mm256_sub_epi32(v[4], v[6]);
        u[7]  = _mm256_sub_epi32(v[5], v[7]);
        u[8]  = _mm256_add_epi32(v[8], v[10]);
        u[9]  = _mm256_add_epi32(v[9], v[11]);
        u[10] = _mm256_sub_epi32(v[8], v[10]);
        u[11] = _mm256_sub_epi32(v[9], v[11]);
        u[12] = _mm256_add_epi32(v[12], v[14]);
        u[13] = _mm256_add_epi32(v[13], v[15]);
        u[14] = _mm256_sub_epi32(v[12], v[14]);
        u[15] = _mm256_sub_epi32(v[13], v[15]);

        // stage 4
        v[0]  = u[0];
        v[1]  = u[1];
        v[2]  = u[2];
        v[3]  = u[3];
        v[4]  = half_btf_avx2(&cospi16, &u[4], &cospi48, &u[5], &rnding, bit);
        v[5]  = half_btf_avx2(&cospi48, &u[4], &cospim16, &u[5], &rnding, bit);
        v[6]  = half_btf_avx2(&cospim48, &u[6], &cospi16, &u[7], &rnding, bit);
        v[7]  = half_btf_avx2(&cospi16, &u[6], &cospi48, &u[7], &rnding, bit);
        v[8]  = u[8];
        v[9]  = u[9];
        v[10] = u[10];
        v[11] = u[11];
        v[12] = half_btf_avx2(&cospi16, &u[12], &cospi48, &u[13], &rnding, bit);
        v[13] = half_btf_avx2(&cospi48, &u[12], &cospim16, &u[13], &rnding, bit);
        v[14] = half_btf_avx2(&cospim48, &u[14], &cospi16, &u[15], &rnding, bit);
        v[15] = half_btf_avx2(&cospi16, &u[14], &cospi48, &u[15], &rnding, bit);

        // stage 5
        u[0]  = _mm256_add_epi32(v[0], v[4]);
        u[1]  = _mm256_add_epi32(v[1], v[5]);
        u[2]  = _mm256_add_epi32(v[2], v[6]);
        u[3]  = _mm256_add_epi32(v[3], v[7]);
        u[4]  = _mm256_sub_epi32(v[0], v[4]);
        u[5]  = _mm256_sub_epi32(v[1], v[5]);
        u[6]  = _mm256_sub_epi32(v[2], v[6]);
        u[7]  = _mm256_sub_epi32(v[3], v[7]);
        u[8]  = _mm256_add_epi32(v[8], v[12]);
        u[9]  = _mm256_add_epi32(v[9], v[13]);
        u[10] = _mm256_add_epi32(v[10], v[14]);
        u[11] = _mm256_add_epi32(v[11], v[15]);
        u[12] = _mm256_sub_epi32(v[8], v[12]);
        u[13] = _mm256_sub_epi32(v[9], v[13]);
        u[14] = _mm256_sub_epi32(v[10], v[14]);
        u[15] = _mm256_sub_epi32(v[11], v[15]);

        // stage 6
        v[0]  = u[0];
        v[1]  = u[1];
        v[2]  = u[2];
        v[3]  = u[3];
        v[4]  = u[4];
        v[5]  = u[5];
        v[6]  = u[6];
        v[7]  = u[7];
        v[8]  = half_btf_avx2(&cospi8, &u[8], &cospi56, &u[9], &rnding, bit);
        v[9]  = half_btf_avx2(&cospi56, &u[8], &cospim8, &u[9], &rnding, bit);
        v[10] = half_btf_avx2(&cospi40, &u[10], &cospi24, &u[11], &rnding, bit);
        v[11] = half_btf_avx2(&cospi24, &u[10], &cospim40, &u[11], &rnding, bit);
        v[12] = half_btf_avx2(&cospim56, &u[12], &cospi8, &u[13], &rnding, bit);
        v[13] = half_btf_avx2(&cospi8, &u[12], &cospi56, &u[13], &rnding, bit);
        v[14] = half_btf_avx2(&cospim24, &u[14], &cospi40, &u[15], &rnding, bit);
        v[15] = half_btf_avx2(&cospi40, &u[14], &cospi24, &u[15], &rnding, bit);

        // stage 7
        u[0]  = _mm256_add_epi32(v[0], v[8]);
        u[1]  = _mm256_add_epi32(v[1], v[9]);
        u[2]  = _mm256_add_epi32(v[2], v[10]);
        u[3]  = _mm256_add_epi32(v[3], v[11]);
        u[12] = _mm256_sub_epi32(v[4], v[12]);
        u[13] = _mm256_sub_epi32(v[5], v[13]);
        u[14] = _mm256_sub_epi32(v[6], v[14]);
        u[15] = _mm256_sub_epi32(v[7], v[15]);

        // stage 8
        v[1]  = half_btf_avx2(&cospi62, &u[0], &cospim2, &u[1], &rnding, bit);
        v[3]  = half_btf_avx2(&cospi54, &u[2], &cospim10, &u[3], &rnding, bit);
        v[12] = half_btf_avx2(&cospi50, &u[12], &cospi14, &u[13], &rnding, bit);
        v[14] = half_btf_avx2(&cospi58, &u[14], &cospi6, &u[15], &rnding, bit);

        // stage 9
        out[0 * col_num + col] = v[1];
        out[1 * col_num + col] = v[14];
        out[2 * col_num + col] = v[3];
        out[3 * col_num + col] = v[12];
    }
}

static void fidtx16x16_N4_avx2(const __m256i* in, __m256i* out, int8_t bit, int32_t col_num, int32_t step) {
    (void)bit;
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 2 * 5793; // 2 * new_sqrt2
    const __m256i newsqrt  = _mm256_set1_epi32(sqrt);
    const __m256i rounding = _mm256_set1_epi32(1 << (bits - 1));
    __m256i       temp;
    int32_t       num_iters = 16 * col_num;
    for (int32_t i = 0; i < num_iters / 4; i += step) {
        temp   = _mm256_mullo_epi32(in[i], newsqrt);
        temp   = _mm256_add_epi32(temp, rounding);
        out[i] = _mm256_srai_epi32(temp, bits);
    }
}

static void av1_fdct32_new_N4_avx2(const __m256i* input, __m256i* output, int8_t cos_bit, const int32_t col_num,
                                   const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m256i  __rounding = _mm256_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 3;

    __m256i cospi_m32 = _mm256_set1_epi32(-cospi[32]);
    __m256i cospi_p32 = _mm256_set1_epi32(cospi[32]);
    __m256i cospi_m16 = _mm256_set1_epi32(-cospi[16]);
    __m256i cospi_p48 = _mm256_set1_epi32(cospi[48]);
    __m256i cospi_m48 = _mm256_set1_epi32(-cospi[48]);
    __m256i cospi_m08 = _mm256_set1_epi32(-cospi[8]);
    __m256i cospi_p56 = _mm256_set1_epi32(cospi[56]);
    __m256i cospi_m56 = _mm256_set1_epi32(-cospi[56]);
    __m256i cospi_m40 = _mm256_set1_epi32(-cospi[40]);
    __m256i cospi_p24 = _mm256_set1_epi32(cospi[24]);
    __m256i cospi_m24 = _mm256_set1_epi32(-cospi[24]);
    __m256i cospi_p08 = _mm256_set1_epi32(cospi[8]);
    __m256i cospi_p04 = _mm256_set1_epi32(cospi[4]);
    __m256i cospi_p60 = _mm256_set1_epi32(cospi[60]);
    __m256i cospi_m52 = _mm256_set1_epi32(-cospi[52]);
    __m256i cospi_p12 = _mm256_set1_epi32(cospi[12]);
    __m256i cospi_p02 = _mm256_set1_epi32(cospi[2]);
    __m256i cospi_p06 = _mm256_set1_epi32(cospi[6]);
    __m256i cospi_p62 = _mm256_set1_epi32(cospi[62]);
    __m256i cospi_m50 = _mm256_set1_epi32(-cospi[50]);
    __m256i cospi_p14 = _mm256_set1_epi32(cospi[14]);
    __m256i cospi_p10 = _mm256_set1_epi32(cospi[10]);
    __m256i cospi_p54 = _mm256_set1_epi32(cospi[54]);
    __m256i cospi_m58 = _mm256_set1_epi32(-cospi[58]);

    __m256i buf0[32];
    __m256i buf1[32];

    for (int32_t col = 0; col < columns; col++) {
        const __m256i* in  = &input[col];
        __m256i*       out = &output[col];

        // stage 0
        // stage 1
        buf1[0]  = _mm256_add_epi32(in[0 * stride], in[31 * stride]);
        buf1[31] = _mm256_sub_epi32(in[0 * stride], in[31 * stride]);
        buf1[1]  = _mm256_add_epi32(in[1 * stride], in[30 * stride]);
        buf1[30] = _mm256_sub_epi32(in[1 * stride], in[30 * stride]);
        buf1[2]  = _mm256_add_epi32(in[2 * stride], in[29 * stride]);
        buf1[29] = _mm256_sub_epi32(in[2 * stride], in[29 * stride]);
        buf1[3]  = _mm256_add_epi32(in[3 * stride], in[28 * stride]);
        buf1[28] = _mm256_sub_epi32(in[3 * stride], in[28 * stride]);
        buf1[4]  = _mm256_add_epi32(in[4 * stride], in[27 * stride]);
        buf1[27] = _mm256_sub_epi32(in[4 * stride], in[27 * stride]);
        buf1[5]  = _mm256_add_epi32(in[5 * stride], in[26 * stride]);
        buf1[26] = _mm256_sub_epi32(in[5 * stride], in[26 * stride]);
        buf1[6]  = _mm256_add_epi32(in[6 * stride], in[25 * stride]);
        buf1[25] = _mm256_sub_epi32(in[6 * stride], in[25 * stride]);
        buf1[7]  = _mm256_add_epi32(in[7 * stride], in[24 * stride]);
        buf1[24] = _mm256_sub_epi32(in[7 * stride], in[24 * stride]);
        buf1[8]  = _mm256_add_epi32(in[8 * stride], in[23 * stride]);
        buf1[23] = _mm256_sub_epi32(in[8 * stride], in[23 * stride]);
        buf1[9]  = _mm256_add_epi32(in[9 * stride], in[22 * stride]);
        buf1[22] = _mm256_sub_epi32(in[9 * stride], in[22 * stride]);
        buf1[10] = _mm256_add_epi32(in[10 * stride], in[21 * stride]);
        buf1[21] = _mm256_sub_epi32(in[10 * stride], in[21 * stride]);
        buf1[11] = _mm256_add_epi32(in[11 * stride], in[20 * stride]);
        buf1[20] = _mm256_sub_epi32(in[11 * stride], in[20 * stride]);
        buf1[12] = _mm256_add_epi32(in[12 * stride], in[19 * stride]);
        buf1[19] = _mm256_sub_epi32(in[12 * stride], in[19 * stride]);
        buf1[13] = _mm256_add_epi32(in[13 * stride], in[18 * stride]);
        buf1[18] = _mm256_sub_epi32(in[13 * stride], in[18 * stride]);
        buf1[14] = _mm256_add_epi32(in[14 * stride], in[17 * stride]);
        buf1[17] = _mm256_sub_epi32(in[14 * stride], in[17 * stride]);
        buf1[15] = _mm256_add_epi32(in[15 * stride], in[16 * stride]);
        buf1[16] = _mm256_sub_epi32(in[15 * stride], in[16 * stride]);

        // stage 2
        buf0[0]  = _mm256_add_epi32(buf1[0], buf1[15]);
        buf0[15] = _mm256_sub_epi32(buf1[0], buf1[15]);
        buf0[1]  = _mm256_add_epi32(buf1[1], buf1[14]);
        buf0[14] = _mm256_sub_epi32(buf1[1], buf1[14]);
        buf0[2]  = _mm256_add_epi32(buf1[2], buf1[13]);
        buf0[13] = _mm256_sub_epi32(buf1[2], buf1[13]);
        buf0[3]  = _mm256_add_epi32(buf1[3], buf1[12]);
        buf0[12] = _mm256_sub_epi32(buf1[3], buf1[12]);
        buf0[4]  = _mm256_add_epi32(buf1[4], buf1[11]);
        buf0[11] = _mm256_sub_epi32(buf1[4], buf1[11]);
        buf0[5]  = _mm256_add_epi32(buf1[5], buf1[10]);
        buf0[10] = _mm256_sub_epi32(buf1[5], buf1[10]);
        buf0[6]  = _mm256_add_epi32(buf1[6], buf1[9]);
        buf0[9]  = _mm256_sub_epi32(buf1[6], buf1[9]);
        buf0[7]  = _mm256_add_epi32(buf1[7], buf1[8]);
        buf0[8]  = _mm256_sub_epi32(buf1[7], buf1[8]);
        buf0[16] = buf1[16];
        buf0[17] = buf1[17];
        buf0[18] = buf1[18];
        buf0[19] = buf1[19];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[23], buf1[24], buf0[23], buf0[24], __rounding, cos_bit);
        buf0[28] = buf1[28];
        buf0[29] = buf1[29];
        buf0[30] = buf1[30];
        buf0[31] = buf1[31];

        // stage 3
        buf1[0] = _mm256_add_epi32(buf0[0], buf0[7]);
        buf1[7] = _mm256_sub_epi32(buf0[0], buf0[7]);
        buf1[1] = _mm256_add_epi32(buf0[1], buf0[6]);
        buf1[6] = _mm256_sub_epi32(buf0[1], buf0[6]);
        buf1[2] = _mm256_add_epi32(buf0[2], buf0[5]);
        buf1[5] = _mm256_sub_epi32(buf0[2], buf0[5]);
        buf1[3] = _mm256_add_epi32(buf0[3], buf0[4]);
        buf1[4] = _mm256_sub_epi32(buf0[3], buf0[4]);
        buf1[8] = buf0[8];
        buf1[9] = buf0[9];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf0[11], buf0[12], buf1[11], buf1[12], __rounding, cos_bit);
        buf1[14] = buf0[14];
        buf1[15] = buf0[15];
        buf1[16] = _mm256_add_epi32(buf0[16], buf0[23]);
        buf1[23] = _mm256_sub_epi32(buf0[16], buf0[23]);
        buf1[17] = _mm256_add_epi32(buf0[17], buf0[22]);
        buf1[22] = _mm256_sub_epi32(buf0[17], buf0[22]);
        buf1[18] = _mm256_add_epi32(buf0[18], buf0[21]);
        buf1[21] = _mm256_sub_epi32(buf0[18], buf0[21]);
        buf1[19] = _mm256_add_epi32(buf0[19], buf0[20]);
        buf1[20] = _mm256_sub_epi32(buf0[19], buf0[20]);
        buf1[24] = _mm256_sub_epi32(buf0[31], buf0[24]);
        buf1[31] = _mm256_add_epi32(buf0[31], buf0[24]);
        buf1[25] = _mm256_sub_epi32(buf0[30], buf0[25]);
        buf1[30] = _mm256_add_epi32(buf0[30], buf0[25]);
        buf1[26] = _mm256_sub_epi32(buf0[29], buf0[26]);
        buf1[29] = _mm256_add_epi32(buf0[29], buf0[26]);
        buf1[27] = _mm256_sub_epi32(buf0[28], buf0[27]);
        buf1[28] = _mm256_add_epi32(buf0[28], buf0[27]);

        // stage 4
        buf0[0] = _mm256_add_epi32(buf1[0], buf1[3]);
        buf0[1] = _mm256_add_epi32(buf1[1], buf1[2]);
        buf0[4] = buf1[4];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, buf1[5], buf1[6], buf0[5], buf0[6], __rounding, cos_bit);
        buf0[7]  = buf1[7];
        buf0[8]  = _mm256_add_epi32(buf1[8], buf1[11]);
        buf0[11] = _mm256_sub_epi32(buf1[8], buf1[11]);
        buf0[9]  = _mm256_add_epi32(buf1[9], buf1[10]);
        buf0[10] = _mm256_sub_epi32(buf1[9], buf1[10]);
        buf0[12] = _mm256_sub_epi32(buf1[15], buf1[12]);
        buf0[15] = _mm256_add_epi32(buf1[15], buf1[12]);
        buf0[13] = _mm256_sub_epi32(buf1[14], buf1[13]);
        buf0[14] = _mm256_add_epi32(buf1[14], buf1[13]);
        buf0[16] = buf1[16];
        buf0[17] = buf1[17];
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, buf1[19], buf1[28], buf0[19], buf0[28], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        buf0[22] = buf1[22];
        buf0[23] = buf1[23];
        buf0[24] = buf1[24];
        buf0[25] = buf1[25];
        buf0[30] = buf1[30];
        buf0[31] = buf1[31];

        // stage 5
        buf1[0] = half_btf_avx2(&cospi_p32, &buf0[0], &cospi_p32, &buf0[1], &__rounding, cos_bit);
        buf1[4] = _mm256_add_epi32(buf0[4], buf0[5]);
        buf1[7] = _mm256_add_epi32(buf0[7], buf0[6]);
        buf1[8] = buf0[8];
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, buf0[9], buf0[14], buf1[9], buf1[14], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        buf1[11] = buf0[11];
        buf1[12] = buf0[12];
        buf1[15] = buf0[15];
        buf1[16] = _mm256_add_epi32(buf0[16], buf0[19]);
        buf1[19] = _mm256_sub_epi32(buf0[16], buf0[19]);
        buf1[17] = _mm256_add_epi32(buf0[17], buf0[18]);
        buf1[18] = _mm256_sub_epi32(buf0[17], buf0[18]);
        buf1[20] = _mm256_sub_epi32(buf0[23], buf0[20]);
        buf1[23] = _mm256_add_epi32(buf0[23], buf0[20]);
        buf1[21] = _mm256_sub_epi32(buf0[22], buf0[21]);
        buf1[22] = _mm256_add_epi32(buf0[22], buf0[21]);
        buf1[24] = _mm256_add_epi32(buf0[24], buf0[27]);
        buf1[27] = _mm256_sub_epi32(buf0[24], buf0[27]);
        buf1[25] = _mm256_add_epi32(buf0[25], buf0[26]);
        buf1[26] = _mm256_sub_epi32(buf0[25], buf0[26]);
        buf1[28] = _mm256_sub_epi32(buf0[31], buf0[28]);
        buf1[31] = _mm256_add_epi32(buf0[31], buf0[28]);
        buf1[29] = _mm256_sub_epi32(buf0[30], buf0[29]);
        buf1[30] = _mm256_add_epi32(buf0[30], buf0[29]);

        // stage 6
        buf0[0]  = buf1[0];
        buf0[4]  = half_btf_avx2(&cospi_p56, &buf1[4], &cospi_p08, &buf1[7], &__rounding, cos_bit);
        buf0[8]  = _mm256_add_epi32(buf1[8], buf1[9]);
        buf0[11] = _mm256_add_epi32(buf1[11], buf1[10]);
        buf0[12] = _mm256_add_epi32(buf1[12], buf1[13]);
        buf0[15] = _mm256_add_epi32(buf1[15], buf1[14]);
        buf0[16] = buf1[16];
        btf_32_type0_avx2_new(cospi_m08, cospi_p56, buf1[17], buf1[30], buf0[17], buf0[30], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m56, cospi_m08, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        buf0[19] = buf1[19];
        buf0[20] = buf1[20];
        btf_32_type0_avx2_new(cospi_m40, cospi_p24, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m24, cospi_m40, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);
        buf0[23] = buf1[23];
        buf0[24] = buf1[24];
        buf0[27] = buf1[27];
        buf0[28] = buf1[28];
        buf0[31] = buf1[31];

        // stage 7
        buf1[0]  = buf0[0];
        buf1[4]  = buf0[4];
        buf1[8]  = half_btf_avx2(&cospi_p60, &buf0[8], &cospi_p04, &buf0[15], &__rounding, cos_bit);
        buf1[12] = half_btf_avx2(&cospi_p12, &buf0[12], &cospi_m52, &buf0[11], &__rounding, cos_bit);

        buf1[16] = _mm256_add_epi32(buf0[16], buf0[17]);
        buf1[19] = _mm256_add_epi32(buf0[19], buf0[18]);
        buf1[20] = _mm256_add_epi32(buf0[20], buf0[21]);
        buf1[23] = _mm256_add_epi32(buf0[23], buf0[22]);
        buf1[24] = _mm256_add_epi32(buf0[24], buf0[25]);
        buf1[27] = _mm256_add_epi32(buf0[27], buf0[26]);
        buf1[28] = _mm256_add_epi32(buf0[28], buf0[29]);
        buf1[31] = _mm256_add_epi32(buf0[31], buf0[30]);

        // stage 8
        buf0[0]  = buf1[0];
        buf0[4]  = buf1[4];
        buf0[8]  = buf1[8];
        buf0[12] = buf1[12];
        buf0[16] = half_btf_avx2(&cospi_p62, &buf1[16], &cospi_p02, &buf1[31], &__rounding, cos_bit);
        buf0[28] = half_btf_avx2(&cospi_p14, &buf1[28], &cospi_m50, &buf1[19], &__rounding, cos_bit);
        buf0[20] = half_btf_avx2(&cospi_p54, &buf1[20], &cospi_p10, &buf1[27], &__rounding, cos_bit);
        buf0[24] = half_btf_avx2(&cospi_p06, &buf1[24], &cospi_m58, &buf1[23], &__rounding, cos_bit);

        // stage 9
        out[0 * stride] = buf0[0];
        out[1 * stride] = buf0[16];
        out[2 * stride] = buf0[8];
        out[3 * stride] = buf0[24];
        out[4 * stride] = buf0[4];
        out[5 * stride] = buf0[20];
        out[6 * stride] = buf0[12];
        out[7 * stride] = buf0[28];
    }
}

static AOM_FORCE_INLINE void fdct32x32_N4_col_avx2(const __m256i* input, __m256i* output, const int8_t cos_bit) {
    const int32_t txfm_size   = 32;
    const int32_t num_per_256 = 8;
    int32_t       col_num     = txfm_size / num_per_256;
    av1_fdct32_new_N4_avx2(input, output, cos_bit, txfm_size, col_num);
}

static AOM_FORCE_INLINE void fdct32x32_N4_row_avx2(const __m256i* input, __m256i* output, const int8_t cos_bit) {
    const int32_t txfm_size   = 32;
    const int32_t num_per_256 = 8;
    int32_t       col_num     = txfm_size / num_per_256;
    av1_fdct32_new_N4_avx2(input, output, cos_bit, txfm_size / 4, col_num);
}

static void fidtx_wxh_N4_avx2(const __m256i* input, __m256i* output, int32_t size, int32_t step) {
    for (int32_t i = 0; i < size; i += step) {
        output[i] = _mm256_slli_epi32(input[i], 2);
    }
}

static void av1_fdct64_new_N4_avx2(const __m256i* input, __m256i* output, int8_t cos_bit, const int32_t col_num,
                                   const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m256i  __rounding = _mm256_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 3;

    __m256i cospi_m32 = _mm256_set1_epi32(-cospi[32]);
    __m256i cospi_p32 = _mm256_set1_epi32(cospi[32]);
    __m256i cospi_m16 = _mm256_set1_epi32(-cospi[16]);
    __m256i cospi_p48 = _mm256_set1_epi32(cospi[48]);
    __m256i cospi_m48 = _mm256_set1_epi32(-cospi[48]);
    __m256i cospi_m08 = _mm256_set1_epi32(-cospi[8]);
    __m256i cospi_p56 = _mm256_set1_epi32(cospi[56]);
    __m256i cospi_m56 = _mm256_set1_epi32(-cospi[56]);
    __m256i cospi_m40 = _mm256_set1_epi32(-cospi[40]);
    __m256i cospi_p24 = _mm256_set1_epi32(cospi[24]);
    __m256i cospi_m24 = _mm256_set1_epi32(-cospi[24]);
    __m256i cospi_p08 = _mm256_set1_epi32(cospi[8]);
    __m256i cospi_p60 = _mm256_set1_epi32(cospi[60]);
    __m256i cospi_p04 = _mm256_set1_epi32(cospi[4]);
    __m256i cospi_p28 = _mm256_set1_epi32(cospi[28]);
    __m256i cospi_p44 = _mm256_set1_epi32(cospi[44]);
    __m256i cospi_p12 = _mm256_set1_epi32(cospi[12]);
    __m256i cospi_m04 = _mm256_set1_epi32(-cospi[4]);
    __m256i cospi_m60 = _mm256_set1_epi32(-cospi[60]);
    __m256i cospi_m36 = _mm256_set1_epi32(-cospi[36]);
    __m256i cospi_m28 = _mm256_set1_epi32(-cospi[28]);
    __m256i cospi_m20 = _mm256_set1_epi32(-cospi[20]);
    __m256i cospi_m44 = _mm256_set1_epi32(-cospi[44]);
    __m256i cospi_m52 = _mm256_set1_epi32(-cospi[52]);
    __m256i cospi_m12 = _mm256_set1_epi32(-cospi[12]);
    __m256i cospi_p62 = _mm256_set1_epi32(cospi[62]);
    __m256i cospi_p02 = _mm256_set1_epi32(cospi[2]);
    __m256i cospi_p14 = _mm256_set1_epi32(cospi[14]);
    __m256i cospi_m50 = _mm256_set1_epi32(-cospi[50]);
    __m256i cospi_p54 = _mm256_set1_epi32(cospi[54]);
    __m256i cospi_p10 = _mm256_set1_epi32(cospi[10]);
    __m256i cospi_p06 = _mm256_set1_epi32(cospi[6]);
    __m256i cospi_m58 = _mm256_set1_epi32(-cospi[58]);
    __m256i cospi_p63 = _mm256_set1_epi32(cospi[63]);
    __m256i cospi_p01 = _mm256_set1_epi32(cospi[1]);
    __m256i cospi_p15 = _mm256_set1_epi32(cospi[15]);
    __m256i cospi_m49 = _mm256_set1_epi32(-cospi[49]);
    __m256i cospi_p55 = _mm256_set1_epi32(cospi[55]);
    __m256i cospi_p09 = _mm256_set1_epi32(cospi[9]);
    __m256i cospi_p07 = _mm256_set1_epi32(cospi[7]);
    __m256i cospi_m57 = _mm256_set1_epi32(-cospi[57]);
    __m256i cospi_p59 = _mm256_set1_epi32(cospi[59]);
    __m256i cospi_p05 = _mm256_set1_epi32(cospi[5]);
    __m256i cospi_p11 = _mm256_set1_epi32(cospi[11]);
    __m256i cospi_m53 = _mm256_set1_epi32(-cospi[53]);
    __m256i cospi_p51 = _mm256_set1_epi32(cospi[51]);
    __m256i cospi_p13 = _mm256_set1_epi32(cospi[13]);
    __m256i cospi_p03 = _mm256_set1_epi32(cospi[3]);
    __m256i cospi_m61 = _mm256_set1_epi32(-cospi[61]);

    for (int32_t col = 0; col < columns; col++) {
        const __m256i* in  = &input[col];
        __m256i*       out = &output[col];

        // stage 1
        __m256i x1[64];
        x1[0]  = _mm256_add_epi32(in[0 * stride], in[63 * stride]);
        x1[63] = _mm256_sub_epi32(in[0 * stride], in[63 * stride]);
        x1[1]  = _mm256_add_epi32(in[1 * stride], in[62 * stride]);
        x1[62] = _mm256_sub_epi32(in[1 * stride], in[62 * stride]);
        x1[2]  = _mm256_add_epi32(in[2 * stride], in[61 * stride]);
        x1[61] = _mm256_sub_epi32(in[2 * stride], in[61 * stride]);
        x1[3]  = _mm256_add_epi32(in[3 * stride], in[60 * stride]);
        x1[60] = _mm256_sub_epi32(in[3 * stride], in[60 * stride]);
        x1[4]  = _mm256_add_epi32(in[4 * stride], in[59 * stride]);
        x1[59] = _mm256_sub_epi32(in[4 * stride], in[59 * stride]);
        x1[5]  = _mm256_add_epi32(in[5 * stride], in[58 * stride]);
        x1[58] = _mm256_sub_epi32(in[5 * stride], in[58 * stride]);
        x1[6]  = _mm256_add_epi32(in[6 * stride], in[57 * stride]);
        x1[57] = _mm256_sub_epi32(in[6 * stride], in[57 * stride]);
        x1[7]  = _mm256_add_epi32(in[7 * stride], in[56 * stride]);
        x1[56] = _mm256_sub_epi32(in[7 * stride], in[56 * stride]);
        x1[8]  = _mm256_add_epi32(in[8 * stride], in[55 * stride]);
        x1[55] = _mm256_sub_epi32(in[8 * stride], in[55 * stride]);
        x1[9]  = _mm256_add_epi32(in[9 * stride], in[54 * stride]);
        x1[54] = _mm256_sub_epi32(in[9 * stride], in[54 * stride]);
        x1[10] = _mm256_add_epi32(in[10 * stride], in[53 * stride]);
        x1[53] = _mm256_sub_epi32(in[10 * stride], in[53 * stride]);
        x1[11] = _mm256_add_epi32(in[11 * stride], in[52 * stride]);
        x1[52] = _mm256_sub_epi32(in[11 * stride], in[52 * stride]);
        x1[12] = _mm256_add_epi32(in[12 * stride], in[51 * stride]);
        x1[51] = _mm256_sub_epi32(in[12 * stride], in[51 * stride]);
        x1[13] = _mm256_add_epi32(in[13 * stride], in[50 * stride]);
        x1[50] = _mm256_sub_epi32(in[13 * stride], in[50 * stride]);
        x1[14] = _mm256_add_epi32(in[14 * stride], in[49 * stride]);
        x1[49] = _mm256_sub_epi32(in[14 * stride], in[49 * stride]);
        x1[15] = _mm256_add_epi32(in[15 * stride], in[48 * stride]);
        x1[48] = _mm256_sub_epi32(in[15 * stride], in[48 * stride]);
        x1[16] = _mm256_add_epi32(in[16 * stride], in[47 * stride]);
        x1[47] = _mm256_sub_epi32(in[16 * stride], in[47 * stride]);
        x1[17] = _mm256_add_epi32(in[17 * stride], in[46 * stride]);
        x1[46] = _mm256_sub_epi32(in[17 * stride], in[46 * stride]);
        x1[18] = _mm256_add_epi32(in[18 * stride], in[45 * stride]);
        x1[45] = _mm256_sub_epi32(in[18 * stride], in[45 * stride]);
        x1[19] = _mm256_add_epi32(in[19 * stride], in[44 * stride]);
        x1[44] = _mm256_sub_epi32(in[19 * stride], in[44 * stride]);
        x1[20] = _mm256_add_epi32(in[20 * stride], in[43 * stride]);
        x1[43] = _mm256_sub_epi32(in[20 * stride], in[43 * stride]);
        x1[21] = _mm256_add_epi32(in[21 * stride], in[42 * stride]);
        x1[42] = _mm256_sub_epi32(in[21 * stride], in[42 * stride]);
        x1[22] = _mm256_add_epi32(in[22 * stride], in[41 * stride]);
        x1[41] = _mm256_sub_epi32(in[22 * stride], in[41 * stride]);
        x1[23] = _mm256_add_epi32(in[23 * stride], in[40 * stride]);
        x1[40] = _mm256_sub_epi32(in[23 * stride], in[40 * stride]);
        x1[24] = _mm256_add_epi32(in[24 * stride], in[39 * stride]);
        x1[39] = _mm256_sub_epi32(in[24 * stride], in[39 * stride]);
        x1[25] = _mm256_add_epi32(in[25 * stride], in[38 * stride]);
        x1[38] = _mm256_sub_epi32(in[25 * stride], in[38 * stride]);
        x1[26] = _mm256_add_epi32(in[26 * stride], in[37 * stride]);
        x1[37] = _mm256_sub_epi32(in[26 * stride], in[37 * stride]);
        x1[27] = _mm256_add_epi32(in[27 * stride], in[36 * stride]);
        x1[36] = _mm256_sub_epi32(in[27 * stride], in[36 * stride]);
        x1[28] = _mm256_add_epi32(in[28 * stride], in[35 * stride]);
        x1[35] = _mm256_sub_epi32(in[28 * stride], in[35 * stride]);
        x1[29] = _mm256_add_epi32(in[29 * stride], in[34 * stride]);
        x1[34] = _mm256_sub_epi32(in[29 * stride], in[34 * stride]);
        x1[30] = _mm256_add_epi32(in[30 * stride], in[33 * stride]);
        x1[33] = _mm256_sub_epi32(in[30 * stride], in[33 * stride]);
        x1[31] = _mm256_add_epi32(in[31 * stride], in[32 * stride]);
        x1[32] = _mm256_sub_epi32(in[31 * stride], in[32 * stride]);

        // stage 2
        __m256i x2[64];
        x2[0]  = _mm256_add_epi32(x1[0], x1[31]);
        x2[31] = _mm256_sub_epi32(x1[0], x1[31]);
        x2[1]  = _mm256_add_epi32(x1[1], x1[30]);
        x2[30] = _mm256_sub_epi32(x1[1], x1[30]);
        x2[2]  = _mm256_add_epi32(x1[2], x1[29]);
        x2[29] = _mm256_sub_epi32(x1[2], x1[29]);
        x2[3]  = _mm256_add_epi32(x1[3], x1[28]);
        x2[28] = _mm256_sub_epi32(x1[3], x1[28]);
        x2[4]  = _mm256_add_epi32(x1[4], x1[27]);
        x2[27] = _mm256_sub_epi32(x1[4], x1[27]);
        x2[5]  = _mm256_add_epi32(x1[5], x1[26]);
        x2[26] = _mm256_sub_epi32(x1[5], x1[26]);
        x2[6]  = _mm256_add_epi32(x1[6], x1[25]);
        x2[25] = _mm256_sub_epi32(x1[6], x1[25]);
        x2[7]  = _mm256_add_epi32(x1[7], x1[24]);
        x2[24] = _mm256_sub_epi32(x1[7], x1[24]);
        x2[8]  = _mm256_add_epi32(x1[8], x1[23]);
        x2[23] = _mm256_sub_epi32(x1[8], x1[23]);
        x2[9]  = _mm256_add_epi32(x1[9], x1[22]);
        x2[22] = _mm256_sub_epi32(x1[9], x1[22]);
        x2[10] = _mm256_add_epi32(x1[10], x1[21]);
        x2[21] = _mm256_sub_epi32(x1[10], x1[21]);
        x2[11] = _mm256_add_epi32(x1[11], x1[20]);
        x2[20] = _mm256_sub_epi32(x1[11], x1[20]);
        x2[12] = _mm256_add_epi32(x1[12], x1[19]);
        x2[19] = _mm256_sub_epi32(x1[12], x1[19]);
        x2[13] = _mm256_add_epi32(x1[13], x1[18]);
        x2[18] = _mm256_sub_epi32(x1[13], x1[18]);
        x2[14] = _mm256_add_epi32(x1[14], x1[17]);
        x2[17] = _mm256_sub_epi32(x1[14], x1[17]);
        x2[15] = _mm256_add_epi32(x1[15], x1[16]);
        x2[16] = _mm256_sub_epi32(x1[15], x1[16]);
        x2[32] = x1[32];
        x2[33] = x1[33];
        x2[34] = x1[34];
        x2[35] = x1[35];
        x2[36] = x1[36];
        x2[37] = x1[37];
        x2[38] = x1[38];
        x2[39] = x1[39];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[40], x1[55], x2[40], x2[55], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[41], x1[54], x2[41], x2[54], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[42], x1[53], x2[42], x2[53], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[43], x1[52], x2[43], x2[52], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[44], x1[51], x2[44], x2[51], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[45], x1[50], x2[45], x2[50], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[46], x1[49], x2[46], x2[49], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x1[47], x1[48], x2[47], x2[48], __rounding, cos_bit);
        x2[56] = x1[56];
        x2[57] = x1[57];
        x2[58] = x1[58];
        x2[59] = x1[59];
        x2[60] = x1[60];
        x2[61] = x1[61];
        x2[62] = x1[62];
        x2[63] = x1[63];

        // stage 3
        __m256i x3[64];
        x3[0]  = _mm256_add_epi32(x2[0], x2[15]);
        x3[15] = _mm256_sub_epi32(x2[0], x2[15]);
        x3[1]  = _mm256_add_epi32(x2[1], x2[14]);
        x3[14] = _mm256_sub_epi32(x2[1], x2[14]);
        x3[2]  = _mm256_add_epi32(x2[2], x2[13]);
        x3[13] = _mm256_sub_epi32(x2[2], x2[13]);
        x3[3]  = _mm256_add_epi32(x2[3], x2[12]);
        x3[12] = _mm256_sub_epi32(x2[3], x2[12]);
        x3[4]  = _mm256_add_epi32(x2[4], x2[11]);
        x3[11] = _mm256_sub_epi32(x2[4], x2[11]);
        x3[5]  = _mm256_add_epi32(x2[5], x2[10]);
        x3[10] = _mm256_sub_epi32(x2[5], x2[10]);
        x3[6]  = _mm256_add_epi32(x2[6], x2[9]);
        x3[9]  = _mm256_sub_epi32(x2[6], x2[9]);
        x3[7]  = _mm256_add_epi32(x2[7], x2[8]);
        x3[8]  = _mm256_sub_epi32(x2[7], x2[8]);
        x3[16] = x2[16];
        x3[17] = x2[17];
        x3[18] = x2[18];
        x3[19] = x2[19];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x2[20], x2[27], x3[20], x3[27], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x2[21], x2[26], x3[21], x3[26], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x2[22], x2[25], x3[22], x3[25], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x2[23], x2[24], x3[23], x3[24], __rounding, cos_bit);
        x3[28] = x2[28];
        x3[29] = x2[29];
        x3[30] = x2[30];
        x3[31] = x2[31];
        x3[32] = _mm256_add_epi32(x2[32], x2[47]);
        x3[47] = _mm256_sub_epi32(x2[32], x2[47]);
        x3[33] = _mm256_add_epi32(x2[33], x2[46]);
        x3[46] = _mm256_sub_epi32(x2[33], x2[46]);
        x3[34] = _mm256_add_epi32(x2[34], x2[45]);
        x3[45] = _mm256_sub_epi32(x2[34], x2[45]);
        x3[35] = _mm256_add_epi32(x2[35], x2[44]);
        x3[44] = _mm256_sub_epi32(x2[35], x2[44]);
        x3[36] = _mm256_add_epi32(x2[36], x2[43]);
        x3[43] = _mm256_sub_epi32(x2[36], x2[43]);
        x3[37] = _mm256_add_epi32(x2[37], x2[42]);
        x3[42] = _mm256_sub_epi32(x2[37], x2[42]);
        x3[38] = _mm256_add_epi32(x2[38], x2[41]);
        x3[41] = _mm256_sub_epi32(x2[38], x2[41]);
        x3[39] = _mm256_add_epi32(x2[39], x2[40]);
        x3[40] = _mm256_sub_epi32(x2[39], x2[40]);
        x3[48] = _mm256_sub_epi32(x2[63], x2[48]);
        x3[63] = _mm256_add_epi32(x2[63], x2[48]);
        x3[49] = _mm256_sub_epi32(x2[62], x2[49]);
        x3[62] = _mm256_add_epi32(x2[62], x2[49]);
        x3[50] = _mm256_sub_epi32(x2[61], x2[50]);
        x3[61] = _mm256_add_epi32(x2[61], x2[50]);
        x3[51] = _mm256_sub_epi32(x2[60], x2[51]);
        x3[60] = _mm256_add_epi32(x2[60], x2[51]);
        x3[52] = _mm256_sub_epi32(x2[59], x2[52]);
        x3[59] = _mm256_add_epi32(x2[59], x2[52]);
        x3[53] = _mm256_sub_epi32(x2[58], x2[53]);
        x3[58] = _mm256_add_epi32(x2[58], x2[53]);
        x3[54] = _mm256_sub_epi32(x2[57], x2[54]);
        x3[57] = _mm256_add_epi32(x2[57], x2[54]);
        x3[55] = _mm256_sub_epi32(x2[56], x2[55]);
        x3[56] = _mm256_add_epi32(x2[56], x2[55]);

        // stage 4
        __m256i x4[64];
        x4[0] = _mm256_add_epi32(x3[0], x3[7]);
        x4[7] = _mm256_sub_epi32(x3[0], x3[7]);
        x4[1] = _mm256_add_epi32(x3[1], x3[6]);
        x4[6] = _mm256_sub_epi32(x3[1], x3[6]);
        x4[2] = _mm256_add_epi32(x3[2], x3[5]);
        x4[5] = _mm256_sub_epi32(x3[2], x3[5]);
        x4[3] = _mm256_add_epi32(x3[3], x3[4]);
        x4[4] = _mm256_sub_epi32(x3[3], x3[4]);
        x4[8] = x3[8];
        x4[9] = x3[9];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x3[10], x3[13], x4[10], x4[13], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x3[11], x3[12], x4[11], x4[12], __rounding, cos_bit);
        x4[14] = x3[14];
        x4[15] = x3[15];
        x4[16] = _mm256_add_epi32(x3[16], x3[23]);
        x4[23] = _mm256_sub_epi32(x3[16], x3[23]);
        x4[17] = _mm256_add_epi32(x3[17], x3[22]);
        x4[22] = _mm256_sub_epi32(x3[17], x3[22]);
        x4[18] = _mm256_add_epi32(x3[18], x3[21]);
        x4[21] = _mm256_sub_epi32(x3[18], x3[21]);
        x4[19] = _mm256_add_epi32(x3[19], x3[20]);
        x4[20] = _mm256_sub_epi32(x3[19], x3[20]);
        x4[24] = _mm256_sub_epi32(x3[31], x3[24]);
        x4[31] = _mm256_add_epi32(x3[31], x3[24]);
        x4[25] = _mm256_sub_epi32(x3[30], x3[25]);
        x4[30] = _mm256_add_epi32(x3[30], x3[25]);
        x4[26] = _mm256_sub_epi32(x3[29], x3[26]);
        x4[29] = _mm256_add_epi32(x3[29], x3[26]);
        x4[27] = _mm256_sub_epi32(x3[28], x3[27]);
        x4[28] = _mm256_add_epi32(x3[28], x3[27]);
        x4[32] = x3[32];
        x4[33] = x3[33];
        x4[34] = x3[34];
        x4[35] = x3[35];
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x3[36], x3[59], x4[36], x4[59], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x3[37], x3[58], x4[37], x4[58], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x3[38], x3[57], x4[38], x4[57], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x3[39], x3[56], x4[39], x4[56], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x3[40], x3[55], x4[40], x4[55], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x3[41], x3[54], x4[41], x4[54], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x3[42], x3[53], x4[42], x4[53], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x3[43], x3[52], x4[43], x4[52], __rounding, cos_bit);
        x4[44] = x3[44];
        x4[45] = x3[45];
        x4[46] = x3[46];
        x4[47] = x3[47];
        x4[48] = x3[48];
        x4[49] = x3[49];
        x4[50] = x3[50];
        x4[51] = x3[51];
        x4[60] = x3[60];
        x4[61] = x3[61];
        x4[62] = x3[62];
        x4[63] = x3[63];

        // stage 5
        __m256i x5[64];
        x5[0] = _mm256_add_epi32(x4[0], x4[3]);
        x5[1] = _mm256_add_epi32(x4[1], x4[2]);
        x5[4] = x4[4];
        btf_32_type0_avx2_new(cospi_m32, cospi_p32, x4[5], x4[6], x5[5], x5[6], __rounding, cos_bit);
        x5[7]  = x4[7];
        x5[8]  = _mm256_add_epi32(x4[8], x4[11]);
        x5[11] = _mm256_sub_epi32(x4[8], x4[11]);
        x5[9]  = _mm256_add_epi32(x4[9], x4[10]);
        x5[10] = _mm256_sub_epi32(x4[9], x4[10]);
        x5[12] = _mm256_sub_epi32(x4[15], x4[12]);
        x5[15] = _mm256_add_epi32(x4[15], x4[12]);
        x5[13] = _mm256_sub_epi32(x4[14], x4[13]);
        x5[14] = _mm256_add_epi32(x4[14], x4[13]);
        x5[16] = x4[16];
        x5[17] = x4[17];
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x4[18], x4[29], x5[18], x5[29], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x4[19], x4[28], x5[19], x5[28], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x4[20], x4[27], x5[20], x5[27], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x4[21], x4[26], x5[21], x5[26], __rounding, cos_bit);
        x5[22] = x4[22];
        x5[23] = x4[23];
        x5[24] = x4[24];
        x5[25] = x4[25];
        x5[30] = x4[30];
        x5[31] = x4[31];
        x5[32] = _mm256_add_epi32(x4[32], x4[39]);
        x5[39] = _mm256_sub_epi32(x4[32], x4[39]);
        x5[33] = _mm256_add_epi32(x4[33], x4[38]);
        x5[38] = _mm256_sub_epi32(x4[33], x4[38]);
        x5[34] = _mm256_add_epi32(x4[34], x4[37]);
        x5[37] = _mm256_sub_epi32(x4[34], x4[37]);
        x5[35] = _mm256_add_epi32(x4[35], x4[36]);
        x5[36] = _mm256_sub_epi32(x4[35], x4[36]);
        x5[40] = _mm256_sub_epi32(x4[47], x4[40]);
        x5[47] = _mm256_add_epi32(x4[47], x4[40]);
        x5[41] = _mm256_sub_epi32(x4[46], x4[41]);
        x5[46] = _mm256_add_epi32(x4[46], x4[41]);
        x5[42] = _mm256_sub_epi32(x4[45], x4[42]);
        x5[45] = _mm256_add_epi32(x4[45], x4[42]);
        x5[43] = _mm256_sub_epi32(x4[44], x4[43]);
        x5[44] = _mm256_add_epi32(x4[44], x4[43]);
        x5[48] = _mm256_add_epi32(x4[48], x4[55]);
        x5[55] = _mm256_sub_epi32(x4[48], x4[55]);
        x5[49] = _mm256_add_epi32(x4[49], x4[54]);
        x5[54] = _mm256_sub_epi32(x4[49], x4[54]);
        x5[50] = _mm256_add_epi32(x4[50], x4[53]);
        x5[53] = _mm256_sub_epi32(x4[50], x4[53]);
        x5[51] = _mm256_add_epi32(x4[51], x4[52]);
        x5[52] = _mm256_sub_epi32(x4[51], x4[52]);
        x5[56] = _mm256_sub_epi32(x4[63], x4[56]);
        x5[63] = _mm256_add_epi32(x4[63], x4[56]);
        x5[57] = _mm256_sub_epi32(x4[62], x4[57]);
        x5[62] = _mm256_add_epi32(x4[62], x4[57]);
        x5[58] = _mm256_sub_epi32(x4[61], x4[58]);
        x5[61] = _mm256_add_epi32(x4[61], x4[58]);
        x5[59] = _mm256_sub_epi32(x4[60], x4[59]);
        x5[60] = _mm256_add_epi32(x4[60], x4[59]);

        // stage 6
        __m256i x6[64];
        out[0 * stride] = half_btf_avx2(&cospi_p32, &x5[0], &cospi_p32, &x5[1], &__rounding, cos_bit);
        x6[4]           = _mm256_add_epi32(x5[4], x5[5]);
        x6[7]           = _mm256_add_epi32(x5[7], x5[6]);
        x6[8]           = x5[8];
        btf_32_type0_avx2_new(cospi_m16, cospi_p48, x5[9], x5[14], x6[9], x6[14], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m48, cospi_m16, x5[10], x5[13], x6[10], x6[13], __rounding, cos_bit);
        x6[11] = x5[11];
        x6[12] = x5[12];
        x6[15] = x5[15];
        x6[16] = _mm256_add_epi32(x5[16], x5[19]);
        x6[19] = _mm256_sub_epi32(x5[16], x5[19]);
        x6[17] = _mm256_add_epi32(x5[17], x5[18]);
        x6[18] = _mm256_sub_epi32(x5[17], x5[18]);
        x6[20] = _mm256_sub_epi32(x5[23], x5[20]);
        x6[23] = _mm256_add_epi32(x5[23], x5[20]);
        x6[21] = _mm256_sub_epi32(x5[22], x5[21]);
        x6[22] = _mm256_add_epi32(x5[22], x5[21]);
        x6[24] = _mm256_add_epi32(x5[24], x5[27]);
        x6[27] = _mm256_sub_epi32(x5[24], x5[27]);
        x6[25] = _mm256_add_epi32(x5[25], x5[26]);
        x6[26] = _mm256_sub_epi32(x5[25], x5[26]);
        x6[28] = _mm256_sub_epi32(x5[31], x5[28]);
        x6[31] = _mm256_add_epi32(x5[31], x5[28]);
        x6[29] = _mm256_sub_epi32(x5[30], x5[29]);
        x6[30] = _mm256_add_epi32(x5[30], x5[29]);
        x6[32] = x5[32];
        x6[33] = x5[33];
        btf_32_type0_avx2_new(cospi_m08, cospi_p56, x5[34], x5[61], x6[34], x6[61], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m08, cospi_p56, x5[35], x5[60], x6[35], x6[60], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m56, cospi_m08, x5[36], x5[59], x6[36], x6[59], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m56, cospi_m08, x5[37], x5[58], x6[37], x6[58], __rounding, cos_bit);
        x6[38] = x5[38];
        x6[39] = x5[39];
        x6[40] = x5[40];
        x6[41] = x5[41];
        btf_32_type0_avx2_new(cospi_m40, cospi_p24, x5[42], x5[53], x6[42], x6[53], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m40, cospi_p24, x5[43], x5[52], x6[43], x6[52], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m24, cospi_m40, x5[44], x5[51], x6[44], x6[51], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m24, cospi_m40, x5[45], x5[50], x6[45], x6[50], __rounding, cos_bit);
        x6[46] = x5[46];
        x6[47] = x5[47];
        x6[48] = x5[48];
        x6[49] = x5[49];
        x6[54] = x5[54];
        x6[55] = x5[55];
        x6[56] = x5[56];
        x6[57] = x5[57];
        x6[62] = x5[62];
        x6[63] = x5[63];

        // stage 7
        __m256i x7[64];
        out[8 * stride] = half_btf_avx2(&cospi_p56, &x6[4], &cospi_p08, &x6[7], &__rounding, cos_bit);
        x7[8]           = _mm256_add_epi32(x6[8], x6[9]);
        x7[11]          = _mm256_add_epi32(x6[11], x6[10]);
        x7[12]          = _mm256_add_epi32(x6[12], x6[13]);
        x7[15]          = _mm256_add_epi32(x6[15], x6[14]);
        x7[16]          = x6[16];
        btf_32_type0_avx2_new(cospi_m08, cospi_p56, x6[17], x6[30], x7[17], x7[30], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m56, cospi_m08, x6[18], x6[29], x7[18], x7[29], __rounding, cos_bit);
        x7[19] = x6[19];
        x7[20] = x6[20];
        btf_32_type0_avx2_new(cospi_m40, cospi_p24, x6[21], x6[26], x7[21], x7[26], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m24, cospi_m40, x6[22], x6[25], x7[22], x7[25], __rounding, cos_bit);
        x7[23] = x6[23];
        x7[24] = x6[24];
        x7[27] = x6[27];
        x7[28] = x6[28];
        x7[31] = x6[31];
        x7[32] = _mm256_add_epi32(x6[32], x6[35]);
        x7[35] = _mm256_sub_epi32(x6[32], x6[35]);
        x7[33] = _mm256_add_epi32(x6[33], x6[34]);
        x7[34] = _mm256_sub_epi32(x6[33], x6[34]);
        x7[36] = _mm256_sub_epi32(x6[39], x6[36]);
        x7[39] = _mm256_add_epi32(x6[39], x6[36]);
        x7[37] = _mm256_sub_epi32(x6[38], x6[37]);
        x7[38] = _mm256_add_epi32(x6[38], x6[37]);
        x7[40] = _mm256_add_epi32(x6[40], x6[43]);
        x7[43] = _mm256_sub_epi32(x6[40], x6[43]);
        x7[41] = _mm256_add_epi32(x6[41], x6[42]);
        x7[42] = _mm256_sub_epi32(x6[41], x6[42]);
        x7[44] = _mm256_sub_epi32(x6[47], x6[44]);
        x7[47] = _mm256_add_epi32(x6[47], x6[44]);
        x7[45] = _mm256_sub_epi32(x6[46], x6[45]);
        x7[46] = _mm256_add_epi32(x6[46], x6[45]);
        x7[48] = _mm256_add_epi32(x6[48], x6[51]);
        x7[51] = _mm256_sub_epi32(x6[48], x6[51]);
        x7[49] = _mm256_add_epi32(x6[49], x6[50]);
        x7[50] = _mm256_sub_epi32(x6[49], x6[50]);
        x7[52] = _mm256_sub_epi32(x6[55], x6[52]);
        x7[55] = _mm256_add_epi32(x6[55], x6[52]);
        x7[53] = _mm256_sub_epi32(x6[54], x6[53]);
        x7[54] = _mm256_add_epi32(x6[54], x6[53]);
        x7[56] = _mm256_add_epi32(x6[56], x6[59]);
        x7[59] = _mm256_sub_epi32(x6[56], x6[59]);
        x7[57] = _mm256_add_epi32(x6[57], x6[58]);
        x7[58] = _mm256_sub_epi32(x6[57], x6[58]);
        x7[60] = _mm256_sub_epi32(x6[63], x6[60]);
        x7[63] = _mm256_add_epi32(x6[63], x6[60]);
        x7[61] = _mm256_sub_epi32(x6[62], x6[61]);
        x7[62] = _mm256_add_epi32(x6[62], x6[61]);

        // stage 8
        __m256i x8[40];
        out[4 * stride]  = half_btf_avx2(&cospi_p60, &x7[8], &cospi_p04, &x7[15], &__rounding, cos_bit);
        out[12 * stride] = half_btf_avx2(&cospi_p12, &x7[12], &cospi_m52, &x7[11], &__rounding, cos_bit);
        x8[0]            = _mm256_add_epi32(x7[16], x7[17]);
        x8[1]            = _mm256_add_epi32(x7[19], x7[18]);
        x8[2]            = _mm256_add_epi32(x7[20], x7[21]);
        x8[3]            = _mm256_add_epi32(x7[23], x7[22]);
        x8[4]            = _mm256_add_epi32(x7[24], x7[25]);
        x8[5]            = _mm256_add_epi32(x7[27], x7[26]);
        x8[6]            = _mm256_add_epi32(x7[28], x7[29]);
        x8[7]            = _mm256_add_epi32(x7[31], x7[30]);
        x8[8]            = x7[32];
        btf_32_type0_avx2_new(cospi_m04, cospi_p60, x7[33], x7[62], x8[9], x8[32], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m60, cospi_m04, x7[34], x7[61], x8[10], x8[33], __rounding, cos_bit);
        x8[11] = x7[35];
        x8[12] = x7[36];
        btf_32_type0_avx2_new(cospi_m36, cospi_p28, x7[37], x7[58], x8[13], x8[34], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m28, cospi_m36, x7[38], x7[57], x8[14], x8[35], __rounding, cos_bit);
        x8[15] = x7[39];
        x8[16] = x7[40];
        btf_32_type0_avx2_new(cospi_m20, cospi_p44, x7[41], x7[54], x8[17], x8[36], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m44, cospi_m20, x7[42], x7[53], x8[18], x8[37], __rounding, cos_bit);
        x8[19] = x7[43];
        x8[20] = x7[44];
        btf_32_type0_avx2_new(cospi_m52, cospi_p12, x7[45], x7[50], x8[21], x8[38], __rounding, cos_bit);
        btf_32_type0_avx2_new(cospi_m12, cospi_m52, x7[46], x7[49], x8[22], x8[39], __rounding, cos_bit);
        x8[23] = x7[47];
        x8[24] = x7[48];
        x8[25] = x7[51];
        x8[26] = x7[52];
        x8[27] = x7[55];
        x8[28] = x7[56];
        x8[29] = x7[59];
        x8[30] = x7[60];
        x8[31] = x7[63];

        // stage 9
        __m256i x9[16];
        out[2 * stride]  = half_btf_avx2(&cospi_p62, &x8[0], &cospi_p02, &x8[7], &__rounding, cos_bit);
        out[14 * stride] = half_btf_avx2(&cospi_p14, &x8[6], &cospi_m50, &x8[1], &__rounding, cos_bit);
        out[10 * stride] = half_btf_avx2(&cospi_p54, &x8[2], &cospi_p10, &x8[5], &__rounding, cos_bit);
        out[6 * stride]  = half_btf_avx2(&cospi_p06, &x8[4], &cospi_m58, &x8[3], &__rounding, cos_bit);
        x9[0]            = _mm256_add_epi32(x8[8], x8[9]);
        x9[1]            = _mm256_add_epi32(x8[11], x8[10]);
        x9[2]            = _mm256_add_epi32(x8[12], x8[13]);
        x9[3]            = _mm256_add_epi32(x8[15], x8[14]);
        x9[4]            = _mm256_add_epi32(x8[16], x8[17]);
        x9[5]            = _mm256_add_epi32(x8[19], x8[18]);
        x9[6]            = _mm256_add_epi32(x8[20], x8[21]);
        x9[7]            = _mm256_add_epi32(x8[23], x8[22]);
        x9[8]            = _mm256_add_epi32(x8[24], x8[39]);
        x9[9]            = _mm256_add_epi32(x8[25], x8[38]);
        x9[10]           = _mm256_add_epi32(x8[26], x8[37]);
        x9[11]           = _mm256_add_epi32(x8[27], x8[36]);
        x9[12]           = _mm256_add_epi32(x8[28], x8[35]);
        x9[13]           = _mm256_add_epi32(x8[29], x8[34]);
        x9[14]           = _mm256_add_epi32(x8[30], x8[33]);
        x9[15]           = _mm256_add_epi32(x8[31], x8[32]);

        // stage 10
        out[1 * stride]  = half_btf_avx2(&cospi_p63, &x9[0], &cospi_p01, &x9[15], &__rounding, cos_bit);
        out[15 * stride] = half_btf_avx2(&cospi_p15, &x9[14], &cospi_m49, &x9[1], &__rounding, cos_bit);
        out[9 * stride]  = half_btf_avx2(&cospi_p55, &x9[2], &cospi_p09, &x9[13], &__rounding, cos_bit);
        out[7 * stride]  = half_btf_avx2(&cospi_p07, &x9[12], &cospi_m57, &x9[3], &__rounding, cos_bit);
        out[5 * stride]  = half_btf_avx2(&cospi_p59, &x9[4], &cospi_p05, &x9[11], &__rounding, cos_bit);
        out[11 * stride] = half_btf_avx2(&cospi_p11, &x9[10], &cospi_m53, &x9[5], &__rounding, cos_bit);
        out[13 * stride] = half_btf_avx2(&cospi_p51, &x9[6], &cospi_p13, &x9[9], &__rounding, cos_bit);
        out[3 * stride]  = half_btf_avx2(&cospi_p03, &x9[8], &cospi_m61, &x9[7], &__rounding, cos_bit);
    }
}

static void fidtx64x64_N4_avx2(const __m256i* input, __m256i* output) {
    const int32_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 4 * 5793; // 4 * new_sqrt2
    const __m256i newsqrt  = _mm256_set1_epi32(sqrt);
    const __m256i rounding = _mm256_set1_epi32(1 << (bits - 1));

    __m256i temp;
    for (int32_t i = 0; i < 128; i += 8) {
        temp          = _mm256_mullo_epi32(input[i], newsqrt);
        temp          = _mm256_add_epi32(temp, rounding);
        output[i]     = _mm256_srai_epi32(temp, bits);
        temp          = _mm256_mullo_epi32(input[i + 1], newsqrt);
        temp          = _mm256_add_epi32(temp, rounding);
        output[i + 1] = _mm256_srai_epi32(temp, bits);
    }
}

static void fdct16x4_N4_avx2(__m256i* input, __m256i* output, int32_t bit) {
    __m128i* in  = (__m128i*)input;
    __m128i* out = (__m128i*)output;

    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospi4   = _mm_set1_epi32(cospi[4]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[16], v[16], x;

    // stage 0
    // stage 1
    u[0]  = _mm_add_epi32(in[0], in[15]);
    v[15] = _mm_sub_epi32(in[0], in[15]);
    u[1]  = _mm_add_epi32(in[1], in[14]);
    v[14] = _mm_sub_epi32(in[1], in[14]);
    u[2]  = _mm_add_epi32(in[2], in[13]);
    u[13] = _mm_sub_epi32(in[2], in[13]);
    u[3]  = _mm_add_epi32(in[3], in[12]);
    u[12] = _mm_sub_epi32(in[3], in[12]);
    u[4]  = _mm_add_epi32(in[4], in[11]);
    u[11] = _mm_sub_epi32(in[4], in[11]);
    u[5]  = _mm_add_epi32(in[5], in[10]);
    u[10] = _mm_sub_epi32(in[5], in[10]);
    u[6]  = _mm_add_epi32(in[6], in[9]);
    v[9]  = _mm_sub_epi32(in[6], in[9]);
    u[7]  = _mm_add_epi32(in[7], in[8]);
    v[8]  = _mm_sub_epi32(in[7], in[8]);

    // stage 2
    v[0] = _mm_add_epi32(u[0], u[7]);
    u[7] = _mm_sub_epi32(u[0], u[7]);
    v[1] = _mm_add_epi32(u[1], u[6]);
    v[6] = _mm_sub_epi32(u[1], u[6]);
    v[2] = _mm_add_epi32(u[2], u[5]);
    v[5] = _mm_sub_epi32(u[2], u[5]);
    v[3] = _mm_add_epi32(u[3], u[4]);
    u[4] = _mm_sub_epi32(u[3], u[4]);

    v[10] = _mm_mullo_epi32(u[10], cospim32);
    x     = _mm_mullo_epi32(u[13], cospi32);
    v[10] = _mm_add_epi32(v[10], x);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[13] = _mm_mullo_epi32(u[10], cospi32);
    x     = _mm_mullo_epi32(u[13], cospim32);
    v[13] = _mm_sub_epi32(v[13], x);
    v[13] = _mm_add_epi32(v[13], rnding);
    v[13] = _mm_srai_epi32(v[13], bit);

    v[11] = _mm_mullo_epi32(u[11], cospim32);
    x     = _mm_mullo_epi32(u[12], cospi32);
    v[11] = _mm_add_epi32(v[11], x);
    v[11] = _mm_add_epi32(v[11], rnding);
    v[11] = _mm_srai_epi32(v[11], bit);

    v[12] = _mm_mullo_epi32(u[11], cospi32);
    x     = _mm_mullo_epi32(u[12], cospim32);
    v[12] = _mm_sub_epi32(v[12], x);
    v[12] = _mm_add_epi32(v[12], rnding);
    v[12] = _mm_srai_epi32(v[12], bit);

    // stage 3
    u[0] = _mm_add_epi32(v[0], v[3]);
    u[1] = _mm_add_epi32(v[1], v[2]);

    u[5] = _mm_mullo_epi32(v[5], cospim32);
    x    = _mm_mullo_epi32(v[6], cospi32);
    u[5] = _mm_add_epi32(u[5], x);
    u[5] = _mm_add_epi32(u[5], rnding);
    u[5] = _mm_srai_epi32(u[5], bit);

    u[6] = _mm_mullo_epi32(v[5], cospi32);
    x    = _mm_mullo_epi32(v[6], cospim32);
    u[6] = _mm_sub_epi32(u[6], x);
    u[6] = _mm_add_epi32(u[6], rnding);
    u[6] = _mm_srai_epi32(u[6], bit);

    u[8]  = _mm_add_epi32(v[8], v[11]);
    v[11] = _mm_sub_epi32(v[8], v[11]);
    u[9]  = _mm_add_epi32(v[9], v[10]);
    u[10] = _mm_sub_epi32(v[9], v[10]);
    u[12] = _mm_sub_epi32(v[15], v[12]);
    v[15] = _mm_add_epi32(v[15], v[12]);
    u[13] = _mm_sub_epi32(v[14], v[13]);
    u[14] = _mm_add_epi32(v[14], v[13]);

    // stage 4
    u[0]   = _mm_mullo_epi32(u[0], cospi32);
    u[1]   = _mm_mullo_epi32(u[1], cospi32);
    v[0]   = _mm_add_epi32(u[0], u[1]);
    v[0]   = _mm_add_epi32(v[0], rnding);
    out[0] = _mm_srai_epi32(v[0], bit);

    v[4] = _mm_add_epi32(u[4], u[5]);
    v[7] = _mm_add_epi32(u[7], u[6]);
    v[8] = u[8];

    v[9] = _mm_mullo_epi32(u[9], cospim16);
    x    = _mm_mullo_epi32(u[14], cospi48);
    v[9] = _mm_add_epi32(v[9], x);
    v[9] = _mm_add_epi32(v[9], rnding);
    v[9] = _mm_srai_epi32(v[9], bit);

    v[14] = _mm_mullo_epi32(u[9], cospi48);
    x     = _mm_mullo_epi32(u[14], cospim16);
    v[14] = _mm_sub_epi32(v[14], x);
    v[14] = _mm_add_epi32(v[14], rnding);
    v[14] = _mm_srai_epi32(v[14], bit);

    v[10] = _mm_mullo_epi32(u[10], cospim48);
    x     = _mm_mullo_epi32(u[13], cospim16);
    v[10] = _mm_add_epi32(v[10], x);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[13] = _mm_mullo_epi32(u[10], cospim16);
    x     = _mm_mullo_epi32(u[13], cospim48);
    v[13] = _mm_sub_epi32(v[13], x);
    v[13] = _mm_add_epi32(v[13], rnding);
    v[13] = _mm_srai_epi32(v[13], bit);

    v[12] = u[12];

    // stage 5
    u[4]   = _mm_mullo_epi32(v[4], cospi56);
    x      = _mm_mullo_epi32(v[7], cospi8);
    u[4]   = _mm_add_epi32(u[4], x);
    u[4]   = _mm_add_epi32(u[4], rnding);
    out[2] = _mm_srai_epi32(u[4], bit);

    u[8]  = _mm_add_epi32(v[8], v[9]);
    u[11] = _mm_add_epi32(v[11], v[10]);
    u[12] = _mm_add_epi32(v[12], v[13]);
    u[15] = _mm_add_epi32(v[15], v[14]);

    // stage 6
    v[8]   = _mm_mullo_epi32(u[8], cospi60);
    x      = _mm_mullo_epi32(u[15], cospi4);
    v[8]   = _mm_add_epi32(v[8], x);
    v[8]   = _mm_add_epi32(v[8], rnding);
    out[1] = _mm_srai_epi32(v[8], bit);

    v[12]  = _mm_mullo_epi32(u[11], cospi52);
    x      = _mm_mullo_epi32(u[12], cospi12);
    v[12]  = _mm_sub_epi32(x, v[12]);
    v[12]  = _mm_add_epi32(v[12], rnding);
    out[3] = _mm_srai_epi32(v[12], bit);
}

static void fdct4x8_col_N4_avx2(__m256i* in, __m256i* output, int32_t bit, const int32_t num_col) {
    const int32_t* cospi   = cospi_arr(bit);
    const __m256i  zero    = _mm256_setzero_si256();
    const __m256i  cospi32 = _mm256_set1_epi32(cospi[32]);
    const __m256i  rnding  = _mm256_set1_epi32(1 << (bit - 1));
    __m256i        s0, s1;
    __m256i        u0, u1, u2, u3;
    __m256i        v0, v1;
    __m256i        out[4];

    int32_t endidx = 3 * num_col;
    s0             = _mm256_add_epi32(in[0], in[endidx]);
    endidx -= num_col;
    s1 = _mm256_add_epi32(in[num_col], in[endidx]);

    // btf_32_sse4_1_type0(cospi32, cospi32, s[01], u[02], bit);
    u0 = _mm256_mullo_epi32(s0, cospi32);
    u1 = _mm256_mullo_epi32(s1, cospi32);
    u2 = _mm256_add_epi32(u0, u1);

    u3 = _mm256_add_epi32(u2, rnding);

    u0 = _mm256_srai_epi32(u3, bit);

    // Transpose 4x4 32-bit
    v0 = _mm256_unpacklo_epi32(u0, zero);
    v1 = _mm256_unpackhi_epi32(u0, zero);

    out[0] = _mm256_unpacklo_epi64(v0, zero);
    out[1] = _mm256_unpackhi_epi64(v0, zero);
    out[2] = _mm256_unpacklo_epi64(v1, zero);
    out[3] = _mm256_unpackhi_epi64(v1, zero);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
}

static void fdct4x8_row_N4_with_round_avx2(__m256i* input, __m256i* output, int32_t bit, const int32_t num_col,
                                           int32_t shift) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m256i  zero     = _mm256_setzero_si256();
    const __m256i  cospi32  = _mm256_set1_epi32(cospi[32]);
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  rounding = _mm256_set1_epi32(1 << (shift - 1));
    __m256i        s0, s1;
    __m256i        u0, u1, u2, u3;
    __m256i        v0, v1;
    __m256i        out[4];
    __m256i        in[4];

    in[0] = yy_unpacklo_epi128(input[0], input[2]);
    in[1] = yy_unpackhi_epi128(input[0], input[2]);
    in[2] = yy_unpacklo_epi128(input[1], input[3]);
    in[3] = yy_unpackhi_epi128(input[1], input[3]);

    int32_t endidx = 3 * num_col;
    s0             = _mm256_add_epi32(in[0], in[endidx]);
    endidx -= num_col;
    s1 = _mm256_add_epi32(in[num_col], in[endidx]);

    // btf_32_sse4_1_type0(cospi32, cospi32, s[01], u[02], bit);
    u0 = _mm256_mullo_epi32(s0, cospi32);
    u1 = _mm256_mullo_epi32(s1, cospi32);
    u2 = _mm256_add_epi32(u0, u1);

    u3 = _mm256_add_epi32(u2, rnding);

    u0 = _mm256_srai_epi32(u3, bit);

    u0 = _mm256_add_epi32(u0, rounding);
    u0 = _mm256_srai_epi32(u0, shift);

    // Transpose 4x4 32-bit
    v0 = _mm256_unpacklo_epi32(u0, zero);
    v1 = _mm256_unpackhi_epi32(u0, zero);

    out[0] = _mm256_unpacklo_epi64(v0, zero);
    out[1] = _mm256_unpackhi_epi64(v0, zero);
    out[2] = _mm256_unpacklo_epi64(v1, zero);
    out[3] = _mm256_unpackhi_epi64(v1, zero);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static INLINE void fadst4x8_col_N4_avx2(__m256i* in, __m256i* output, int32_t bit, const int32_t num_col) {
    const int32_t* sinpi  = sinpi_arr(bit);
    const __m256i  zero   = _mm256_setzero_si256();
    const __m256i  rnding = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  sinpi1 = _mm256_set1_epi32((int32_t)sinpi[1]);
    const __m256i  sinpi2 = _mm256_set1_epi32((int32_t)sinpi[2]);
    const __m256i  sinpi3 = _mm256_set1_epi32((int32_t)sinpi[3]);
    const __m256i  sinpi4 = _mm256_set1_epi32((int32_t)sinpi[4]);
    __m256i        s0, s2, s3, s4;
    __m256i        u0, u1;
    __m256i        v0, v1;
    __m256i        out[4];

    int32_t idx = 0 * num_col;
    s0          = _mm256_mullo_epi32(in[idx], sinpi1);
    idx += num_col;
    s2 = _mm256_mullo_epi32(in[idx], sinpi2);
    idx += num_col;
    s4 = _mm256_mullo_epi32(in[idx], sinpi3);
    idx += num_col;
    s3 = _mm256_mullo_epi32(in[idx], sinpi4);

    u0 = _mm256_add_epi32(s0, s2);
    u1 = _mm256_add_epi32(u0, s3);

    s0 = _mm256_add_epi32(u1, s4);

    u0 = _mm256_add_epi32(s0, rnding);
    u0 = _mm256_srai_epi32(u0, bit);

    v0 = _mm256_unpacklo_epi32(u0, zero);
    v1 = _mm256_unpackhi_epi32(u0, zero);

    out[0] = _mm256_unpacklo_epi64(v0, zero);
    out[1] = _mm256_unpackhi_epi64(v0, zero);
    out[2] = _mm256_unpacklo_epi64(v1, zero);
    out[3] = _mm256_unpackhi_epi64(v1, zero);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
}

static void fadst4x8_row_N4_with_round_avx2(__m256i* input, __m256i* output, int32_t bit, const int32_t num_col,
                                            int32_t shift) {
    const int32_t* sinpi    = sinpi_arr(bit);
    const __m256i  zero     = _mm256_setzero_si256();
    const __m256i  rnding   = _mm256_set1_epi32(1 << (bit - 1));
    const __m256i  rounding = _mm256_set1_epi32(1 << (shift - 1));
    const __m256i  sinpi1   = _mm256_set1_epi32((int32_t)sinpi[1]);
    const __m256i  sinpi2   = _mm256_set1_epi32((int32_t)sinpi[2]);
    const __m256i  sinpi3   = _mm256_set1_epi32((int32_t)sinpi[3]);
    const __m256i  sinpi4   = _mm256_set1_epi32((int32_t)sinpi[4]);
    __m256i        s0, s2, s3, s4;
    __m256i        u0, u1;
    __m256i        v0, v1;
    __m256i        out[4];
    __m256i        in[4];

    in[0] = yy_unpacklo_epi128(input[0], input[2]);
    in[1] = yy_unpackhi_epi128(input[0], input[2]);
    in[2] = yy_unpacklo_epi128(input[1], input[3]);
    in[3] = yy_unpackhi_epi128(input[1], input[3]);

    int32_t idx = 0 * num_col;
    s0          = _mm256_mullo_epi32(in[idx], sinpi1);
    idx += num_col;
    s2 = _mm256_mullo_epi32(in[idx], sinpi2);
    idx += num_col;
    s4 = _mm256_mullo_epi32(in[idx], sinpi3);
    idx += num_col;
    s3 = _mm256_mullo_epi32(in[idx], sinpi4);

    u0 = _mm256_add_epi32(s0, s2);
    u1 = _mm256_add_epi32(u0, s3);

    s0 = _mm256_add_epi32(u1, s4);

    u0 = _mm256_add_epi32(s0, rnding);
    u0 = _mm256_srai_epi32(u0, bit);

    u0 = _mm256_add_epi32(u0, rounding);
    u0 = _mm256_srai_epi32(u0, shift);

    v0 = _mm256_unpacklo_epi32(u0, zero);
    v1 = _mm256_unpackhi_epi32(u0, zero);

    out[0] = _mm256_unpacklo_epi64(v0, zero);
    out[1] = _mm256_unpackhi_epi64(v0, zero);
    out[2] = _mm256_unpacklo_epi64(v1, zero);
    out[3] = _mm256_unpackhi_epi64(v1, zero);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static void fadst16x4_N4_avx2(__m256i* input, __m256i* output, int32_t bit) {
    __m128i* in  = (__m128i*)input;
    __m128i* out = (__m128i*)output;

    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospim56 = _mm_set1_epi32(-cospi[56]);
    const __m128i  cospim8  = _mm_set1_epi32(-cospi[8]);
    const __m128i  cospi24  = _mm_set1_epi32(cospi[24]);
    const __m128i  cospim24 = _mm_set1_epi32(-cospi[24]);
    const __m128i  cospim40 = _mm_set1_epi32(-cospi[40]);
    const __m128i  cospi40  = _mm_set1_epi32(cospi[40]);
    const __m128i  cospi62  = _mm_set1_epi32(cospi[62]);
    const __m128i  cospim2  = _mm_set1_epi32(-cospi[2]);
    const __m128i  cospi54  = _mm_set1_epi32(cospi[54]);
    const __m128i  cospim10 = _mm_set1_epi32(-cospi[10]);
    const __m128i  cospi50  = _mm_set1_epi32(cospi[50]);
    const __m128i  cospi14  = _mm_set1_epi32(cospi[14]);
    const __m128i  cospi58  = _mm_set1_epi32(cospi[58]);
    const __m128i  cospi6   = _mm_set1_epi32(cospi[6]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();

    __m128i u[16], v[16], x, y;
    __m128i tmp[13];

    tmp[0] = _mm_sub_epi32(zero, in[15]);
    u[2]   = _mm_sub_epi32(zero, in[7]);
    tmp[1] = _mm_sub_epi32(zero, in[3]);
    u[7]   = _mm_sub_epi32(zero, in[11]);
    tmp[2] = _mm_sub_epi32(zero, in[1]);
    u[11]  = _mm_sub_epi32(zero, in[9]);
    tmp[3] = _mm_sub_epi32(zero, in[13]);
    u[14]  = _mm_sub_epi32(zero, in[5]);

    // stage 2

    x    = _mm_mullo_epi32(u[2], cospi32);
    y    = _mm_mullo_epi32(in[8], cospi32);
    v[2] = _mm_add_epi32(x, y);
    v[2] = _mm_add_epi32(v[2], rnding);
    v[2] = _mm_srai_epi32(v[2], bit);

    v[3] = _mm_sub_epi32(x, y);
    v[3] = _mm_add_epi32(v[3], rnding);
    v[3] = _mm_srai_epi32(v[3], bit);

    x    = _mm_mullo_epi32(in[4], cospi32);
    y    = _mm_mullo_epi32(u[7], cospi32);
    v[6] = _mm_add_epi32(x, y);
    v[6] = _mm_add_epi32(v[6], rnding);
    v[6] = _mm_srai_epi32(v[6], bit);

    v[7] = _mm_sub_epi32(x, y);
    v[7] = _mm_add_epi32(v[7], rnding);
    v[7] = _mm_srai_epi32(v[7], bit);

    x     = _mm_mullo_epi32(in[6], cospi32);
    y     = _mm_mullo_epi32(u[11], cospi32);
    v[10] = _mm_add_epi32(x, y);
    v[10] = _mm_add_epi32(v[10], rnding);
    v[10] = _mm_srai_epi32(v[10], bit);

    v[11] = _mm_sub_epi32(x, y);
    v[11] = _mm_add_epi32(v[11], rnding);
    v[11] = _mm_srai_epi32(v[11], bit);

    x     = _mm_mullo_epi32(u[14], cospi32);
    y     = _mm_mullo_epi32(in[10], cospi32);
    v[14] = _mm_add_epi32(x, y);
    v[14] = _mm_add_epi32(v[14], rnding);
    v[14] = _mm_srai_epi32(v[14], bit);

    v[15] = _mm_sub_epi32(x, y);
    v[15] = _mm_add_epi32(v[15], rnding);
    v[15] = _mm_srai_epi32(v[15], bit);

    // stage 3
    tmp[4] = _mm_add_epi32(in[0], v[2]);
    tmp[5] = _mm_add_epi32(tmp[0], v[3]);
    tmp[6] = _mm_sub_epi32(in[0], v[2]);
    tmp[0] = _mm_sub_epi32(tmp[0], v[3]);
    u[4]   = _mm_add_epi32(tmp[1], v[6]);
    u[5]   = _mm_add_epi32(in[12], v[7]);
    u[6]   = _mm_sub_epi32(tmp[1], v[6]);
    u[7]   = _mm_sub_epi32(in[12], v[7]);
    tmp[1] = _mm_add_epi32(tmp[2], v[10]);
    tmp[7] = _mm_add_epi32(in[14], v[11]);
    tmp[2] = _mm_sub_epi32(tmp[2], v[10]);
    tmp[8] = _mm_sub_epi32(in[14], v[11]);
    u[12]  = _mm_add_epi32(in[2], v[14]);
    u[13]  = _mm_add_epi32(tmp[3], v[15]);
    u[14]  = _mm_sub_epi32(in[2], v[14]);
    u[15]  = _mm_sub_epi32(tmp[3], v[15]);

    // stage 4
    v[4]  = half_btf_small(&cospi16, &u[4], &cospi48, &u[5], &rnding, bit);
    v[5]  = half_btf_small(&cospi48, &u[4], &cospim16, &u[5], &rnding, bit);
    v[6]  = half_btf_small(&cospim48, &u[6], &cospi16, &u[7], &rnding, bit);
    v[7]  = half_btf_small(&cospi16, &u[6], &cospi48, &u[7], &rnding, bit);
    v[12] = half_btf_small(&cospi16, &u[12], &cospi48, &u[13], &rnding, bit);
    v[13] = half_btf_small(&cospi48, &u[12], &cospim16, &u[13], &rnding, bit);
    v[14] = half_btf_small(&cospim48, &u[14], &cospi16, &u[15], &rnding, bit);
    v[15] = half_btf_small(&cospi16, &u[14], &cospi48, &u[15], &rnding, bit);

    // stage 5
    tmp[9]  = _mm_add_epi32(tmp[4], v[4]);
    tmp[10] = _mm_add_epi32(tmp[5], v[5]);
    tmp[11] = _mm_add_epi32(tmp[6], v[6]);
    tmp[12] = _mm_add_epi32(tmp[0], v[7]);
    tmp[4]  = _mm_sub_epi32(tmp[4], v[4]);
    tmp[5]  = _mm_sub_epi32(tmp[5], v[5]);
    tmp[6]  = _mm_sub_epi32(tmp[6], v[6]);
    tmp[0]  = _mm_sub_epi32(tmp[0], v[7]);
    u[8]    = _mm_add_epi32(tmp[1], v[12]);
    u[9]    = _mm_add_epi32(tmp[7], v[13]);
    u[10]   = _mm_add_epi32(tmp[2], v[14]);
    u[11]   = _mm_add_epi32(tmp[8], v[15]);
    u[12]   = _mm_sub_epi32(tmp[1], v[12]);
    u[13]   = _mm_sub_epi32(tmp[7], v[13]);
    u[14]   = _mm_sub_epi32(tmp[2], v[14]);
    u[15]   = _mm_sub_epi32(tmp[8], v[15]);

    // stage 6
    v[8]  = half_btf_small(&cospi8, &u[8], &cospi56, &u[9], &rnding, bit);
    v[9]  = half_btf_small(&cospi56, &u[8], &cospim8, &u[9], &rnding, bit);
    v[10] = half_btf_small(&cospi40, &u[10], &cospi24, &u[11], &rnding, bit);
    v[11] = half_btf_small(&cospi24, &u[10], &cospim40, &u[11], &rnding, bit);
    v[12] = half_btf_small(&cospim56, &u[12], &cospi8, &u[13], &rnding, bit);
    v[13] = half_btf_small(&cospi8, &u[12], &cospi56, &u[13], &rnding, bit);
    v[14] = half_btf_small(&cospim24, &u[14], &cospi40, &u[15], &rnding, bit);
    v[15] = half_btf_small(&cospi40, &u[14], &cospi24, &u[15], &rnding, bit);

    // stage 7
    u[0]  = _mm_add_epi32(tmp[9], v[8]);
    u[1]  = _mm_add_epi32(tmp[10], v[9]);
    u[2]  = _mm_add_epi32(tmp[11], v[10]);
    u[3]  = _mm_add_epi32(tmp[12], v[11]);
    u[12] = _mm_sub_epi32(tmp[4], v[12]);
    u[13] = _mm_sub_epi32(tmp[5], v[13]);
    u[14] = _mm_sub_epi32(tmp[6], v[14]);
    u[15] = _mm_sub_epi32(tmp[0], v[15]);

    // stage 8
    out[0] = half_btf_small(&cospi62, &u[0], &cospim2, &u[1], &rnding, bit);
    out[2] = half_btf_small(&cospi54, &u[2], &cospim10, &u[3], &rnding, bit);
    out[3] = half_btf_small(&cospi50, &u[12], &cospi14, &u[13], &rnding, bit);
    out[1] = half_btf_small(&cospi58, &u[14], &cospi6, &u[15], &rnding, bit);
}

static INLINE void fidtx4x8_col_N4_avx2(__m256i* in, __m256i* output, int32_t bit, int32_t col_num) {
    (void)bit;
    const __m256i zero   = _mm256_setzero_si256();
    __m256i       fact   = _mm256_set1_epi32(new_sqrt2);
    __m256i       offset = _mm256_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m256i       a_low;

    for (int32_t i = 0; i < col_num; i++) {
        a_low     = _mm256_mullo_epi32(in[i], fact);
        a_low     = _mm256_add_epi32(a_low, offset);
        a_low     = _mm256_srai_epi32(a_low, new_sqrt2_bits);
        output[i] = _mm256_blend_epi32(zero, a_low, 17);
    }
}

static INLINE void fidtx4x8_row_N4_with_round_avx2(__m256i* input, __m256i* output, int32_t bit, int32_t shift) {
    (void)bit;
    __m256i out[4];
    __m256i v[2];
    __m256i a_low;

    const __m256i fact     = _mm256_set1_epi32(new_sqrt2);
    const __m256i offset   = _mm256_set1_epi32(1 << (new_sqrt2_bits - 1));
    const __m256i rounding = _mm256_set1_epi32(1 << (shift - 1));
    const __m256i zero     = _mm256_setzero_si256();

    a_low = yy_unpacklo_epi128(input[0], input[2]);
    a_low = _mm256_mullo_epi32(a_low, fact);
    a_low = _mm256_add_epi32(a_low, offset);
    a_low = _mm256_srai_epi32(a_low, new_sqrt2_bits);
    a_low = _mm256_add_epi32(a_low, rounding);
    a_low = _mm256_srai_epi32(a_low, shift);

    // Transpose for 4x4
    v[0] = _mm256_unpacklo_epi32(a_low, zero);
    v[1] = _mm256_unpackhi_epi32(a_low, zero);

    out[0] = _mm256_unpacklo_epi64(v[0], zero);
    out[1] = _mm256_unpackhi_epi64(v[0], zero);
    out[2] = _mm256_unpacklo_epi64(v[1], zero);
    out[3] = _mm256_unpackhi_epi64(v[1], zero);

    output[0] = yy_unpacklo_epi128(out[0], out[1]);
    output[1] = yy_unpacklo_epi128(out[2], out[3]);
    output[2] = yy_unpackhi_epi128(out[0], out[1]);
    output[3] = yy_unpackhi_epi128(out[2], out[3]);
}

static INLINE void fidtx4x8_row_N4_avx2(__m256i* in, __m256i* output, int32_t bit) {
    (void)bit;
    __m256i fact   = _mm256_set1_epi32(new_sqrt2);
    __m256i offset = _mm256_set1_epi32(1 << (new_sqrt2_bits - 1));
    __m256i a_low;

    a_low     = _mm256_mullo_epi32(in[0], fact);
    a_low     = _mm256_add_epi32(a_low, offset);
    output[0] = _mm256_srai_epi32(a_low, new_sqrt2_bits);
}

static AOM_FORCE_INLINE void fidtx8x4_N4_avx2(__m256i* in, __m256i* out, int32_t bit) {
    (void)bit;
    out[0] = _mm256_add_epi32(in[0], in[0]);
}

static void fdct4x8_N4_avx2(__m256i* input, __m256i* output, int32_t bit) {
    __m128i*       in       = (__m128i*)input;
    __m128i*       out      = (__m128i*)output;
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospim32 = _mm_set1_epi32(-cospi[32]);
    const __m128i  cospi56  = _mm_set1_epi32(cospi[56]);
    const __m128i  cospi8   = _mm_set1_epi32(cospi[8]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    __m128i        u[8], v[8];

    // Even 8 points 0, 2, ..., 14
    // stage 0
    // stage 1
    u[0] = _mm_add_epi32(in[0], in[7]);
    v[7] = _mm_sub_epi32(in[0], in[7]); // v[7]
    u[1] = _mm_add_epi32(in[1], in[6]);
    u[6] = _mm_sub_epi32(in[1], in[6]);
    u[2] = _mm_add_epi32(in[2], in[5]);
    u[5] = _mm_sub_epi32(in[2], in[5]);
    u[3] = _mm_add_epi32(in[3], in[4]);
    v[4] = _mm_sub_epi32(in[3], in[4]); // v[4]

    // stage 2
    v[0] = _mm_add_epi32(u[0], u[3]);
    v[1] = _mm_add_epi32(u[1], u[2]);

    v[5] = _mm_mullo_epi32(u[5], cospim32);
    v[6] = _mm_mullo_epi32(u[6], cospi32);
    v[5] = _mm_add_epi32(v[5], v[6]);
    v[5] = _mm_add_epi32(v[5], rnding);
    v[5] = _mm_srai_epi32(v[5], bit);

    u[0] = _mm_mullo_epi32(u[5], cospi32);
    v[6] = _mm_mullo_epi32(u[6], cospim32);
    v[6] = _mm_sub_epi32(u[0], v[6]);
    v[6] = _mm_add_epi32(v[6], rnding);
    v[6] = _mm_srai_epi32(v[6], bit);

    // stage 3
    // type 0
    v[0]   = _mm_mullo_epi32(v[0], cospi32);
    v[1]   = _mm_mullo_epi32(v[1], cospi32);
    u[0]   = _mm_add_epi32(v[0], v[1]);
    u[0]   = _mm_add_epi32(u[0], rnding);
    out[0] = _mm_srai_epi32(u[0], bit);

    u[4] = _mm_add_epi32(v[4], v[5]);
    u[7] = _mm_add_epi32(v[7], v[6]);

    // stage 4
    // stage 5
    v[0]   = _mm_mullo_epi32(u[4], cospi56);
    v[1]   = _mm_mullo_epi32(u[7], cospi8);
    v[0]   = _mm_add_epi32(v[0], v[1]);
    v[0]   = _mm_add_epi32(v[0], rnding);
    out[1] = _mm_srai_epi32(v[0], bit); // buf0[4]
}

static void fadst8x4_N4_avx2(__m256i* input, __m256i* output, int32_t bit, const int32_t col_num) {
    __m128i*       in       = (__m128i*)input;
    __m128i*       out      = (__m128i*)output;
    const int32_t* cospi    = cospi_arr(bit);
    const __m128i  cospi32  = _mm_set1_epi32(cospi[32]);
    const __m128i  cospi16  = _mm_set1_epi32(cospi[16]);
    const __m128i  cospim16 = _mm_set1_epi32(-cospi[16]);
    const __m128i  cospi48  = _mm_set1_epi32(cospi[48]);
    const __m128i  cospim48 = _mm_set1_epi32(-cospi[48]);
    const __m128i  cospim4  = _mm_set1_epi32(-cospi[4]);
    const __m128i  cospi60  = _mm_set1_epi32(cospi[60]);
    const __m128i  cospi52  = _mm_set1_epi32(cospi[52]);
    const __m128i  cospi12  = _mm_set1_epi32(cospi[12]);
    const __m128i  rnding   = _mm_set1_epi32(1 << (bit - 1));
    const __m128i  zero     = _mm_setzero_si128();
    __m128i        u0, u1, u2, u3, u4, u5, u6, u7;
    __m128i        v0, v1, v2, v3, v4, v5, v6, v7;
    __m128i        x, y;
    int32_t        col;

    // Note:
    //  Even column: 0, 2, ..., 14
    //  Odd column: 1, 3, ..., 15
    //  one even column plus one odd column constructs one row (8 coeffs)
    //  total we have 8 rows (8x8).
    for (col = 0; col < col_num; ++col) {
        // stage 0
        // stage 1
        u0 = in[col_num * 0 + col];
        u1 = _mm_sub_epi32(zero, in[col_num * 7 + col]);
        u2 = _mm_sub_epi32(zero, in[col_num * 3 + col]);
        u3 = in[col_num * 4 + col];
        u4 = _mm_sub_epi32(zero, in[col_num * 1 + col]);
        u5 = in[col_num * 6 + col];
        u6 = in[col_num * 2 + col];
        u7 = _mm_sub_epi32(zero, in[col_num * 5 + col]);

        // stage 2
        v0 = u0;
        v1 = u1;

        x  = _mm_mullo_epi32(u2, cospi32);
        y  = _mm_mullo_epi32(u3, cospi32);
        v2 = _mm_add_epi32(x, y);
        v2 = _mm_add_epi32(v2, rnding);
        v2 = _mm_srai_epi32(v2, bit);

        v3 = _mm_sub_epi32(x, y);
        v3 = _mm_add_epi32(v3, rnding);
        v3 = _mm_srai_epi32(v3, bit);

        v4 = u4;
        v5 = u5;

        x  = _mm_mullo_epi32(u6, cospi32);
        y  = _mm_mullo_epi32(u7, cospi32);
        v6 = _mm_add_epi32(x, y);
        v6 = _mm_add_epi32(v6, rnding);
        v6 = _mm_srai_epi32(v6, bit);

        v7 = _mm_sub_epi32(x, y);
        v7 = _mm_add_epi32(v7, rnding);
        v7 = _mm_srai_epi32(v7, bit);

        // stage 3
        u0 = _mm_add_epi32(v0, v2);
        u1 = _mm_add_epi32(v1, v3);
        u2 = _mm_sub_epi32(v0, v2);
        u3 = _mm_sub_epi32(v1, v3);
        u4 = _mm_add_epi32(v4, v6);
        u5 = _mm_add_epi32(v5, v7);
        u6 = _mm_sub_epi32(v4, v6);
        u7 = _mm_sub_epi32(v5, v7);

        // stage 4
        v0 = u0;
        v1 = u1;
        v2 = u2;
        v3 = u3;

        x  = _mm_mullo_epi32(u4, cospi16);
        y  = _mm_mullo_epi32(u5, cospi48);
        v4 = _mm_add_epi32(x, y);
        v4 = _mm_add_epi32(v4, rnding);
        v4 = _mm_srai_epi32(v4, bit);

        x  = _mm_mullo_epi32(u4, cospi48);
        y  = _mm_mullo_epi32(u5, cospim16);
        v5 = _mm_add_epi32(x, y);
        v5 = _mm_add_epi32(v5, rnding);
        v5 = _mm_srai_epi32(v5, bit);

        x  = _mm_mullo_epi32(u6, cospim48);
        y  = _mm_mullo_epi32(u7, cospi16);
        v6 = _mm_add_epi32(x, y);
        v6 = _mm_add_epi32(v6, rnding);
        v6 = _mm_srai_epi32(v6, bit);

        x  = _mm_mullo_epi32(u6, cospi16);
        y  = _mm_mullo_epi32(u7, cospi48);
        v7 = _mm_add_epi32(x, y);
        v7 = _mm_add_epi32(v7, rnding);
        v7 = _mm_srai_epi32(v7, bit);

        // stage 5
        u0 = _mm_add_epi32(v0, v4);
        u1 = _mm_add_epi32(v1, v5);
        u6 = _mm_sub_epi32(v2, v6);
        u7 = _mm_sub_epi32(v3, v7);

        // stage 6
        x                      = _mm_mullo_epi32(u0, cospi60);
        y                      = _mm_mullo_epi32(u1, cospim4);
        v1                     = _mm_add_epi32(x, y);
        v1                     = _mm_add_epi32(v1, rnding);
        out[col_num * 0 + col] = _mm_srai_epi32(v1, bit);

        x                      = _mm_mullo_epi32(u6, cospi52);
        y                      = _mm_mullo_epi32(u7, cospi12);
        v6                     = _mm_add_epi32(x, y);
        v6                     = _mm_add_epi32(v6, rnding);
        out[col_num * 1 + col] = _mm_srai_epi32(v6, bit);
    }
}

static AOM_FORCE_INLINE void write_buffer_4x8_N4(const __m256i* res, int32_t* output) {
    const __m256i zero = _mm256_setzero_si256();
    _mm256_storeu_si256((__m256i*)(output + 0 * 8), res[0]);
    _mm256_storeu_si256((__m256i*)(output + 1 * 8), zero);
    _mm256_storeu_si256((__m256i*)(output + 2 * 8), zero);
    _mm256_storeu_si256((__m256i*)(output + 3 * 8), zero);
}

static AOM_FORCE_INLINE void col_txfm_8x4_N4_rounding(__m256i* in, int32_t shift) {
    const __m256i rounding = _mm256_set1_epi32(1 << (shift - 1));

    in[0] = _mm256_add_epi32(in[0], rounding);
    in[0] = _mm256_srai_epi32(in[0], shift);
}

void svt_av1_fwd_txfm2d_8x8_N4_avx2(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[8], out[8] = {_mm256_setzero_si256()};
    const int8_t* shift   = fwd_txfm_shift_ls[TX_8X8];
    const int32_t txw_idx = get_txw_idx(TX_8X8);
    const int32_t txh_idx = get_txh_idx(TX_8X8);

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fdct8x8_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fdct8x8_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fdct8x8_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fdct8x8_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 1, shift[0]);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        transpose_8x8_avx2(out, in);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(out, in);
        write_buffer_8x8_N4(in, coeff);
        break;
    case IDTX:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 0, shift[0], 1);
        clear_buffer_4x16_N2(in);
        fidtx8x8_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8x8_N4_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N4(out, coeff);
        break;
    case V_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8x8_N4_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N4(out, coeff);
        break;
    case H_DCT:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 0, shift[0], 1);
        clear_buffer_4x16_N2(in);
        fidtx8x8_N4_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fdct8x8_N4_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(in, out);
        write_buffer_8x8_N4(out, coeff);
        break;
    case V_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8x8_N4_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N4(out, coeff);
        break;
    case H_ADST:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 0, shift[0], 1);
        clear_buffer_4x16_N2(in);
        fidtx8x8_N4_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fadst8x8_N4_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(in, out);
        write_buffer_8x8_N4(out, coeff);
        break;
    case V_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8x8_N4_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        write_buffer_8x8_N4(out, coeff);
        break;
    case H_FLIPADST:
        load_buffer_4x8_in_8x8(input, in, stride, 0, 1, shift[0], 1);
        clear_buffer_4x16_N2(in);
        fidtx8x8_N4_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], 1);
        col_txfm_8x8_N4_rounding(in, -shift[1]);
        transpose_8x8_avx2(in, out);
        fadst8x8_N4_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], 1);
        transpose_8x8_N2_avx2(in, out);
        write_buffer_8x8_N4(out, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x16_N4_avx2(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[32] = {_mm256_setzero_si256()}, out[32] = {_mm256_setzero_si256()};
    const int8_t* shift   = fwd_txfm_shift_ls[TX_16X16];
    const int32_t txw_idx = get_txw_idx(TX_16X16);
    const int32_t txh_idx = get_txh_idx(TX_16X16);
    const int32_t col_num = 2;
    switch (tx_type) {
    case IDTX:
        load_buffer_4x16_in_16x16(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, 2);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        fidtx16x16_N4_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 2);
        write_buffer_16x16_N4(out, coeff);
        break;
    case DCT_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fdct16x16_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 1);
        transpose_4x4_in_16x16_avx2(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fdct16x16_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 1);
        transpose_4x4_in_16x16_avx2(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 1);
        transpose_4x4_in_16x16_avx2(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 1);
        transpose_4x4_in_16x16_avx2(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        fdct16x16_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 1);
        transpose_4x4_in_16x16_avx2(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fdct16x16_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 1);
        transpose_4x4_in_16x16_avx2(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x16(input, in, stride, 1, 1, shift[0]);
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 1);
        transpose_4x4_in_16x16_avx2(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_16x16(input, in, stride, 0, 1, shift[0]);
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 1);
        transpose_4x4_in_16x16_avx2(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        transpose_8x8_in_16x16_avx2(out, in); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(out + 1, in + 16); //top-right ->bottom-left
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 1);
        transpose_4x4_in_16x16_avx2(out, in);
        write_buffer_16x16_N4(in, coeff);
        break;
    case V_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, 1);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        fidtx16x16_N4_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 2);
        write_buffer_16x16_N4(out, coeff);
        break;
    case H_DCT:
        load_buffer_4x16_in_16x16(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_N4_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], col_num, 1);
        col_txfm_16x16_N4_rounding(in, -shift[1]);
        transpose_8x8_in_16x16_avx2(in, out); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(in + 1, out + 16); //top-right ->bottom-left
        fdct16x16_N4_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 1);
        transpose_4x4_in_16x16_avx2(in, out);
        write_buffer_16x16_N4(out, coeff);
        break;
    case V_ADST:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, 1);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        fidtx16x16_N4_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 2);
        write_buffer_16x16_N4(out, coeff);
        break;
    case H_ADST:
        load_buffer_4x16_in_16x16(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_N4_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], col_num, 1);
        col_txfm_16x16_N4_rounding(in, -shift[1]);
        transpose_8x8_in_16x16_avx2(in, out); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(in + 1, out + 16); //top-right ->bottom-left
        fadst16x16_N4_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 1);
        transpose_4x4_in_16x16_avx2(in, out);
        write_buffer_16x16_N4(out, coeff);
        break;
    case V_FLIPADST:
        load_buffer_16x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N4_avx2(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num, col_num);
        col_txfm_16x16_N4_rounding(out, -shift[1]);
        fidtx16x16_N4_avx2(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 2);
        write_buffer_16x16_N4(out, coeff);
        break;
    case H_FLIPADST:
        load_buffer_4x16_in_16x16(input, in, stride, 0, 1, shift[0]);
        fidtx16x16_N4_avx2(in, in, fwd_cos_bit_col[txw_idx][txh_idx], col_num, 1);
        col_txfm_16x16_N4_rounding(in, -shift[1]);
        transpose_8x8_in_16x16_avx2(in, out); //top-left -> top-left
        transpose_8x8_in_16x16_avx2(in + 1, out + 16); //top-right ->bottom-left
        fadst16x16_N4_avx2(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num, 1);
        transpose_4x4_in_16x16_avx2(in, out);
        write_buffer_16x16_N4(out, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x64_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    __m256i       in[512];
    __m256i*      out     = (__m256i*)output;
    const int32_t txw_idx = tx_size_wide_log2[TX_64X64] - tx_size_wide_log2[0];
    const int32_t txh_idx = tx_size_high_log2[TX_64X64] - tx_size_high_log2[0];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_64X64];

    switch (tx_type) {
    case IDTX:
        load_buffer_16x16_in_64x64_avx2(input, stride, out);
        fidtx64x64_N4_avx2(out, in);
        av1_round_shift_array_64_N4_avx2(in, in, 512 / 4, -shift[1]);
        /*row wise transform*/
        fidtx64x64_N4_avx2(in, out);
        av1_round_shift_array_64_N4_avx2(out, out, 512 / 4, -shift[2]);
        clear_buffer_wxh_N4(out, 8, 64);
        break;
    case DCT_DCT:
        load_buffer_64x64_avx2(input, stride, out);
        av1_fdct64_new_N4_avx2(out, in, fwd_cos_bit_col[txw_idx][txh_idx], 64, 8);
        av1_round_shift_array_32_avx2(in, out, 512 / 4, -shift[1]);
        transpose_16x16_in_64x64_avx2(out, in);
        transpose_16x16_in_64x64_avx2(out + 2, in + 128);
        transpose_16x16_in_64x64_avx2(out + 4, in + 256);
        transpose_16x16_in_64x64_avx2(out + 6, in + 384);
        /*row wise transform*/
        av1_fdct64_new_N4_avx2(in, out, fwd_cos_bit_row[txw_idx][txh_idx], 16, 8);
        av1_round_shift_array_64_N4_avx2(out, in, 512 / 4, -shift[2]);
        transpose_16x16_in_64x64_avx2(in, out); //top-left
        clear_buffer_wxh_N4(out, 8, 64);
        break;
    default:
        assert(0);
    }
}

void svt_av1_fwd_txfm2d_32x32_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    const int8_t* shift           = fwd_txfm_shift_ls[TX_32X32];
    const int32_t txw_idx         = tx_size_wide_log2[TX_32X32] - tx_size_wide_log2[0];
    const int32_t txh_idx         = tx_size_high_log2[TX_32X32] - tx_size_high_log2[0];
    const int8_t  cos_bit_col     = fwd_cos_bit_col[txw_idx][txh_idx];
    const int8_t  cos_bit_row     = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm2d_size_256 = 32 * 32 / 8;
    __m256i       buf_256[128];
    __m256i*      out_256 = (__m256i*)output;
    (void)bd;

    switch (tx_type) {
    case IDTX:
        load_buffer_8x8_in_32x32_avx2(input, buf_256, stride);
        av1_round_shift_array_32_N4_avx2(buf_256, out_256, 32, -shift[0]);
        fidtx_wxh_N4_avx2(out_256, buf_256, 32, 4);
        av1_round_shift_array_32_N4_avx2(buf_256, out_256, 32, -shift[1]);
        fidtx_wxh_N4_avx2(out_256, buf_256, 32, 4);
        av1_round_shift_array_32_N4_avx2(buf_256, buf_256, 32, -shift[2]);
        write_buffer_32x32_N4(buf_256, output);
        break;
    case DCT_DCT:
        load_buffer_32x32_avx2(input, buf_256, stride);
        av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256, -shift[0]);
        fdct32x32_N4_col_avx2(out_256, buf_256, cos_bit_col);
        av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256 / 4, -shift[1]);
        //transpose 32x8 to 8x32
        transpose_8x8_in_32x32_avx2(out_256, buf_256);
        transpose_8x8_in_32x32_avx2(out_256 + 1, buf_256 + 32);
        transpose_8x8_in_32x32_avx2(out_256 + 2, buf_256 + 64);
        transpose_8x8_in_32x32_avx2(out_256 + 3, buf_256 + 96);
        fdct32x32_N4_row_avx2(buf_256, out_256, cos_bit_row);
        av1_round_shift_array_32_N4_avx2(out_256, out_256, 32, -shift[2]);
        transpose_8x8_in_32x32_avx2(out_256, buf_256);
        write_buffer_32x32_N4(buf_256, output);
        break;
    case V_DCT:
        load_buffer_8x32_in_32x32_avx2(input, buf_256, stride);
        av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256, -shift[0]);
        fdct32x32_N4_row_avx2(out_256, buf_256, cos_bit_col);
        av1_round_shift_array_32_N4_avx2(buf_256, out_256, 32, -shift[1]);
        fidtx_wxh_N4_avx2(out_256, buf_256, 32, 4);
        av1_round_shift_array_32_N4_avx2(buf_256, buf_256, 32, -shift[2]);
        write_buffer_32x32_N4(buf_256, output);
        break;
    case H_DCT:
        load_buffer_32x8_in_32x32_avx2(input, buf_256, stride);
        av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256 / 4, -shift[0]);
        fidtx_wxh_N4_avx2(out_256, buf_256, 32, 1);
        av1_round_shift_array_32_avx2(buf_256, out_256, txfm2d_size_256 / 4, -shift[1]);
        //transpose 32x8 to 8x32
        transpose_8x8_in_32x32_avx2(out_256, buf_256);
        transpose_8x8_in_32x32_avx2(out_256 + 1, buf_256 + 32);
        transpose_8x8_in_32x32_avx2(out_256 + 2, buf_256 + 64);
        transpose_8x8_in_32x32_avx2(out_256 + 3, buf_256 + 96);
        fdct32x32_N4_row_avx2(buf_256, out_256, cos_bit_row);
        av1_round_shift_array_32_N4_avx2(out_256, out_256, 32, -shift[2]);
        transpose_8x8_in_32x32_avx2(out_256, buf_256);
        write_buffer_32x32_N4(buf_256, output);
        break;
    default:
        assert(0);
    }
}

void svt_av1_fwd_txfm2d_16x32_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[64];
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X32];
    const int32_t txw_idx       = get_txw_idx(TX_16X32);
    const int32_t txh_idx       = get_txh_idx(TX_16X32);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_16X32];
    const int32_t txfm_size_row = tx_size_high[TX_16X32];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_16x16_N2_half(input, in, stride, 0, 0, shift[0]);
        fidtx_wxh_N4_avx2(in, in, 16, 2);
        col_txfm_16x16_N2_half_rounding(&in[0], -shift[1]);
        fidtx16x16_N4_avx2(in, outcoef256, bitrow, num_row, 2);
        av1_round_shift_rect_wxh_N4(outcoef256, outcoef256, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_16x16(input, in, stride, 0, 0, shift[0]);
        load_buffer_16x16(input + 16 * stride, in + 32, stride, 0, 0, shift[0]);
        for (int32_t i = 0; i < num_col; i++) {
            av1_fdct32_new_N4_avx2((in + i), (in + i), bitcol, 8, num_col);
        }
        col_txfm_16x16_N2_rounding(&in[0], -shift[1]);
        transpose_8nx8n_N4_half(in, outcoef256, txfm_size_col, txfm_size_row);
        fdct16x16_N4_avx2(outcoef256, in, bitrow, num_row, num_row / 2);
        transpose_8nx8n_N4_quad(in, outcoef256, txfm_size_row, txfm_size_col);
        av1_round_shift_rect_wxh_N4(outcoef256, outcoef256, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

/* call this function only for IDTX */
void svt_av1_fwd_txfm2d_32x16_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[64];
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_32X16];
    const int32_t txw_idx       = get_txw_idx(TX_32X16);
    const int32_t txh_idx       = get_txh_idx(TX_32X16);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_32X16];
    const int32_t txfm_size_row = tx_size_high[TX_32X16];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_16x8n(input, in, stride, 0, 0, shift[0], txfm_size_row / 4);
        fidtx16x16_N4_avx2(in, in, bitcol, 4, 4);
        col_txfm_32x16_N4_rounding(&in[0], -shift[1]);
        fidtx_wxh_N4_avx2(in, outcoef256, 16, 4);
        av1_round_shift_rect_wxh_N4(outcoef256, outcoef256, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
        fdct16x16_N4_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_16x16_N4_rounding(&in[0], -shift[1]);
        col_txfm_16x16_N4_rounding(&in[8], -shift[1]);
        transpose_8nx8n_N4_half(in, outcoef256, txfm_size_col, txfm_size_row);
        av1_fdct32_new_N4_avx2(outcoef256, in, bitrow, 8, num_row);
        transpose_8nx8n_N4_quad(in, outcoef256, txfm_size_row, txfm_size_col);
        av1_round_shift_rect_wxh_N4(outcoef256, outcoef256, -shift[2], new_sqrt2, num_col, txfm_size_row);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

/* call this function only for DCT_DCT, IDTX */
void svt_av1_fwd_txfm2d_8x32_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[32];
    __m256i*      outcoef256 = (__m256i*)output;
    const int8_t* shift      = fwd_txfm_shift_ls[TX_8X32];
    const int32_t txw_idx    = get_txw_idx(TX_8X32);
    const int32_t txh_idx    = get_txh_idx(TX_8X32);
    int8_t        bitcol     = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow     = fwd_cos_bit_row[txw_idx][txh_idx];

    const int32_t txfm_size_col = tx_size_wide[TX_8X32];
    const int32_t txfm_size_row = tx_size_high[TX_8X32];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_8x16_N4(input, in, stride, 0, 0, shift[0]);
        fidtx_wxh_N4_avx2(in, in, 8, 1);
        col_txfm_16x16_N4_rounding(in, -shift[1]);
        // row transform
        fidtx32x8_N2_avx2(in, outcoef256, bitrow, num_col, 8);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        load_buffer_8x16(input + (txfm_size_row >> 1) * stride, in + 16, stride, 0, 0, shift[0]);
        av1_fdct32_new_N4_avx2(in, in, bitcol, 8, num_col);
        col_txfm_16x16_N4_rounding(in, -shift[1]);
        transpose_8nx8n_N4_half(in, outcoef256, txfm_size_col, txfm_size_row);
        // row transform
        fdct8x8_N4_avx2(outcoef256, in, bitrow, num_row);
        transpose_8nx8n_N4_quad(in, outcoef256, txfm_size_row, txfm_size_col);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

/* call this function only for DCT_DCT, IDTX */
void svt_av1_fwd_txfm2d_32x8_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[32];
    __m256i*      outcoef256 = (__m256i*)output;
    const int8_t* shift      = fwd_txfm_shift_ls[TX_32X8];
    const int32_t txw_idx    = get_txw_idx(TX_32X8);
    const int32_t txh_idx    = get_txh_idx(TX_32X8);
    int8_t        bitcol     = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow     = fwd_cos_bit_row[txw_idx][txh_idx];

    const int32_t txfm_size_col = tx_size_wide[TX_32X8];
    const int32_t txfm_size_row = tx_size_high[TX_32X8];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case IDTX:
        load_buffer_16x8n(input, in, stride, 0, 0, shift[0], txfm_size_row / 4);
        fidtx32x8_N2_avx2(in, in, bitcol, num_col, 2);
        col_txfm_32x8_N4_rounding(&in[0], -shift[1]);
        // row transform
        fidtx_wxh_N4_avx2(in, outcoef256, 8, 4);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_DCT:
        load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
        for (int32_t i = 0; i < num_col; i++) {
            fdct8x8_N4_avx2((in + i), (in + i), bitcol, num_col);
        }
        col_txfm_16x16_N4_rounding(&in[0], -shift[1]);
        transpose_8nx8n_N4_half(in, outcoef256, txfm_size_col, txfm_size_row);
        // row transform
        av1_fdct32_new_N4_avx2(outcoef256, in, bitrow, 8, num_row);
        transpose_8nx8n_N4_quad(in, outcoef256, txfm_size_row, txfm_size_col);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

/* call this function for all 16 transform types */
void svt_av1_fwd_txfm2d_8x16_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[16], out[16];
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_8X16];
    const int32_t txw_idx       = get_txw_idx(TX_8X16);
    const int32_t txh_idx       = get_txh_idx(TX_8X16);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_8X16];
    const int32_t txfm_size_row = tx_size_high[TX_8X16];
    const int32_t num_col       = txfm_size_col >> 3;

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N4_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fdct8x8_N4_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        transpose_8x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case ADST_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N4_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fdct8x8_N4_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        transpose_8x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_ADST:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N4_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fadst8x8_N4_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        transpose_8x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case ADST_ADST:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N4_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fadst8x8_N4_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        transpose_8x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case FLIPADST_DCT:
        load_buffer_8x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N4_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fdct8x8_N4_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        transpose_8x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_FLIPADST:
        load_buffer_8x16(input, in, stride, 0, 1, shift[0]);
        fdct16x16_N4_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fadst8x8_N4_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        transpose_8x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x16(input, in, stride, 1, 1, shift[0]);
        fadst16x16_N4_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fadst8x8_N4_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        transpose_8x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case ADST_FLIPADST:
        load_buffer_8x16(input, in, stride, 0, 1, shift[0]);
        fadst16x16_N4_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fadst8x8_N4_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        transpose_8x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case FLIPADST_ADST:
        load_buffer_8x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N4_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fadst8x8_N4_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        transpose_8x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case IDTX:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, in, bitcol, 4);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        fidtx8x8_N2_avx2(in, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoef256, 4, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case V_DCT:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fdct16x16_N4_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        fidtx8x8_N2_avx2(in, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoef256, 4, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case H_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, in, bitcol, 4);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fdct8x8_N4_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        transpose_8x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case V_ADST:
        load_buffer_8x16(input, in, stride, 0, 0, shift[0]);
        fadst16x16_N4_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        fidtx8x8_N2_avx2(in, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoef256, 4, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case H_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, in, bitcol, 4);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fadst8x8_N4_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        transpose_8x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case V_FLIPADST:
        load_buffer_8x16(input, in, stride, 1, 0, shift[0]);
        fadst16x16_N4_avx2(in, in, bitcol, num_col, num_col);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        fidtx8x8_N2_avx2(in, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoef256, 4, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case H_FLIPADST:
        load_buffer_8x8(input, in, stride, 0, 1, shift[0]);
        fidtx8xn_N2_col_avx2(in, in, bitcol, 4);
        col_txfm_8x8_N2_rounding(in, -shift[1]);
        transpose_8x8_half_avx2(in, out);
        fadst8x8_N4_avx2(out, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        transpose_8x8_N2_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

/* call this function for all 16 transform types */
void svt_av1_fwd_txfm2d_16x8_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[16], out[16] = {_mm256_setzero_si256()};
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X8];
    const int32_t txw_idx       = get_txw_idx(TX_16X8);
    const int32_t txh_idx       = get_txh_idx(TX_16X8);
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t txfm_size_col = tx_size_wide[TX_16X8];
    const int32_t txfm_size_row = tx_size_high[TX_16X8];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;
    assert(num_col > 0);
    // column transform
    switch (tx_type) {
    case DCT_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fdct8x8_N4_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fdct16x16_N4_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_N2_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case ADST_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fadst8x8_N4_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fdct16x16_N4_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_N2_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fdct8x8_N4_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fadst16x16_N4_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_N2_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case ADST_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fadst8x8_N4_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fadst16x16_N4_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_N2_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case FLIPADST_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 1, 0, shift[0]);
            fadst8x8_N4_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fdct16x16_N4_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_N2_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case DCT_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fdct8x8_N4_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[16 - i - 1] = out[i];
        }
        fadst16x16_N4_avx2(in, out, bitrow, num_row, 1);
        transpose_8x8_N2_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case FLIPADST_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 1, 0, shift[0]);
            fadst8x8_N4_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[16 - i - 1] = out[i];
        }
        fadst16x16_N4_avx2(in, out, bitrow, num_row, 1);
        transpose_8x8_N2_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case ADST_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fadst8x8_N4_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[16 - i - 1] = out[i];
        }
        fadst16x16_N4_avx2(in, out, bitrow, num_row, 1);
        transpose_8x8_N2_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case FLIPADST_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 1, 0, shift[0]);
            fadst8x8_N4_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fadst16x16_N4_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_N2_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case IDTX:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fidtx8x8_N4_avx2(in, out, bitcol, 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8xn_N2_col_avx2(out, out, bitrow, 2);
        av1_round_shift_rect_array_32_avx2(out, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case V_DCT:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fdct8x8_N4_avx2(in, out, bitcol, 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8xn_N2_col_avx2(out, out, bitrow, 2);
        av1_round_shift_rect_array_32_avx2(out, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case H_DCT:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fidtx8x8_N4_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fdct16x16_N4_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_N2_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case V_ADST:
        load_buffer_8x8(input, in, stride, 0, 0, shift[0]);
        fadst8x8_N4_avx2(in, out, bitcol, 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8xn_N2_col_avx2(out, out, bitrow, 2);
        av1_round_shift_rect_array_32_avx2(out, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case H_ADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fidtx8x8_N4_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        fadst16x16_N4_avx2(out, out, bitrow, num_row, 1);
        transpose_8x8_N2_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case V_FLIPADST:
        load_buffer_8x8(input, in, stride, 1, 0, shift[0]);
        fadst8x8_N4_avx2(in, out, bitcol, 1);
        col_txfm_8x8_N4_rounding(out, -shift[1]);
        fidtx8xn_N2_col_avx2(out, out, bitrow, 2);
        av1_round_shift_rect_array_32_avx2(out, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    case H_FLIPADST:
        for (int32_t i = 0; i < num_col; i++) {
            load_buffer_8x8(input + i * 8, in, stride, 0, 0, shift[0]);
            fidtx8x8_N4_avx2(in, in, bitcol, 1);
            col_txfm_8x8_N4_rounding(in, -shift[1]);
            transpose_8x8_half_avx2(in, out + i * 8);
        }
        for (int32_t i = 0; i < 16; i++) {
            in[16 - i - 1] = out[i];
        }
        fadst16x16_avx2(in, out, bitrow, num_row);
        transpose_8x8_N2_avx2(out, in);
        av1_round_shift_rect_array_32_avx2(in, out, 2, -shift[2], new_sqrt2);
        write_buffer_16x8_N4_avx2(out, outcoef256);
        clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_4x8_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i in[4];
    __m256i outcoeff256[4];

    const int8_t* shift   = fwd_txfm_shift_ls[TX_4X8];
    const int32_t txw_idx = get_txw_idx(TX_4X8);
    const int32_t txh_idx = get_txh_idx(TX_4X8);
    int32_t       bitcol  = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow  = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_N4_avx2(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fdct4x8_col_N4_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case ADST_DCT:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fadst8x4_N4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fdct4x8_col_N4_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case DCT_ADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_N4_avx2(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N4_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case ADST_ADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fadst8x4_N4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N4_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case FLIPADST_DCT:
        load_buffer_4x8_avx2(input, in, stride, 1, 0, shift[0]);
        fadst8x4_N4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fdct4x8_col_N4_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case DCT_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 1, shift[0]);
        fdct4x8_N4_avx2(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N4_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 1, 1, shift[0]);
        fadst8x4_N4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N4_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case ADST_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 1, shift[0]);
        fadst8x4_N4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N4_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case FLIPADST_ADST:
        load_buffer_4x8_avx2(input, in, stride, 1, 0, shift[0]);
        fadst8x4_N4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N4_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case IDTX:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_N4_avx2(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        fidtx4x8_col_N4_avx2(in, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case V_DCT:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_N4_avx2(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        fidtx4x8_col_N4_avx2(in, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case H_DCT:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_N4_avx2(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fdct4x8_col_N4_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case V_ADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fadst8x4_N4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        fidtx4x8_col_N4_avx2(in, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case H_ADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8x4_N4_avx2(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N4_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case V_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 1, 0, shift[0]);
        fadst8x4_N4_avx2(in, in, bitcol, 1);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        fidtx4x8_col_N4_avx2(in, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    case H_FLIPADST:
        load_buffer_4x8_avx2(input, in, stride, 0, 1, shift[0]);
        fidtx8x4_N4_avx2(in, in, bitcol);
        col_txfm_8x4_N4_rounding(in, -shift[1]);
        transpose_4x8_avx2(in, outcoeff256);
        fadst4x8_col_N4_avx2(outcoeff256, in, bitrow, 1);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        write_buffer_4x8_N4(outcoeff256, output);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_8x4_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[4];
    __m256i*      outcoeff256 = (__m256i*)output;
    const int8_t* shift       = fwd_txfm_shift_ls[TX_8X4];
    const int32_t txw_idx     = get_txw_idx(TX_8X4);
    const int32_t txh_idx     = get_txh_idx(TX_8X4);
    int32_t       bitcol      = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow      = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N4_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fdct4x8_N4_avx2(in, outcoeff256, bitrow);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case ADST_DCT:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fdct4x8_N4_avx2(in, outcoeff256, bitrow);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case DCT_ADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N4_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N4_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case ADST_ADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N4_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case FLIPADST_DCT:
        load_buffer_8x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fdct4x8_N4_avx2(in, outcoeff256, bitrow);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case DCT_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 1, shift[0]);
        fdct4x8_row_N4_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N4_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 1, 1, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N4_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case ADST_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 1, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N4_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case FLIPADST_ADST:
        load_buffer_8x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fadst8x4_N4_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case IDTX:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N4_with_round_avx2(in, in, bitcol, -shift[1]);
        fidtx8x4_N2_avx2(in, outcoeff256, bitrow);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case V_DCT:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N4_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fidtx8x4_N2_avx2(in, outcoeff256, bitrow);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case H_DCT:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N4_with_round_avx2(in, in, bitcol, -shift[1]);
        fdct4x8_N4_avx2(in, outcoeff256, bitrow);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case V_ADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fidtx8x4_N2_avx2(in, outcoeff256, bitrow);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case H_ADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N4_with_round_avx2(in, in, bitcol, -shift[1]);
        fadst8x4_N4_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case V_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, in, bitcol, 1, -shift[1]);
        fidtx8x4_N2_avx2(in, outcoeff256, bitrow);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    case H_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 1, shift[0]);
        fidtx4x8_row_N4_with_round_avx2(in, in, bitcol, -shift[1]);
        fadst8x4_N4_avx2(in, outcoeff256, bitrow, 1);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        av1_round_shift_rect_array_32_avx2(in, outcoeff256, 1, -shift[2], new_sqrt2);
        clear_buffer_wxh_N4(outcoeff256, 1, 4);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_4x16_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[8];
    __m256i*      outcoeff256 = (__m256i*)output;
    const int8_t* shift       = fwd_txfm_shift_ls[TX_4X16];
    const int32_t txw_idx     = get_txw_idx(TX_4X16);
    const int32_t txh_idx     = get_txh_idx(TX_4X16);
    int32_t       bitcol      = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow      = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fdct16x4_N4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fdct4x8_col_N4_avx2(in, outcoeff256, bitrow, 2); //dct + transpose + clear right half of buffer
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case ADST_DCT:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fadst16x4_N4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fdct4x8_col_N4_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case DCT_ADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fdct16x4_N4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N4_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case ADST_ADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fadst16x4_N4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N4_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case FLIPADST_DCT:
        load_buffer_4x16_avx2(input, in, stride, 1, 0, shift[0]);
        fadst16x4_N4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fdct4x8_col_N4_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case DCT_FLIPADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 1, shift[0]);
        fdct16x4_N4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N4_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_4x16_avx2(input, in, stride, 1, 1, shift[0]);
        fadst16x4_N4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N4_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case ADST_FLIPADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 1, shift[0]);
        fadst16x4_N4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N4_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case FLIPADST_ADST:
        load_buffer_4x16_avx2(input, in, stride, 1, 0, shift[0]);
        fadst16x4_N4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N4_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case IDTX:
        load_buffer_4x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, outcoeff256, bitcol, 2);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        fidtx4x8_col_N4_avx2(outcoeff256, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case V_DCT:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fdct16x4_N4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        fidtx4x8_col_N4_avx2(outcoeff256, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case H_DCT:
        load_buffer_4x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, outcoeff256, bitcol, 2);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fdct4x8_col_N4_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case V_ADST:
        load_buffer_4x16_avx2(input, in, stride, 0, 0, shift[0]);
        fadst16x4_N4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        fidtx4x8_col_N4_avx2(outcoeff256, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case H_ADST:
        load_buffer_4x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx8xn_N2_col_avx2(in, outcoeff256, bitcol, 2);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N4_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case V_FLIPADST:
        load_buffer_4x16_avx2(input, in, stride, 1, 0, shift[0]);
        fadst16x4_N4_avx2(in, outcoeff256, bitcol);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        fidtx4x8_col_N4_avx2(outcoeff256, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    case H_FLIPADST:
        load_buffer_4x4_avx2(input, in, stride, 0, 1, shift[0]);
        fidtx8xn_N2_col_avx2(in, outcoeff256, bitcol, 2);
        col_txfm_8x8_N4_rounding(outcoeff256, -shift[1]);
        transpose_4x8_in_4x16_avx2(outcoeff256, in);
        fadst4x8_col_N4_avx2(in, outcoeff256, bitrow, 2);
        clear_buffer_4x16_N4(outcoeff256);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x4_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[8];
    __m256i*      outcoeff256 = (__m256i*)output;
    const int8_t* shift       = fwd_txfm_shift_ls[TX_16X4];
    const int32_t txw_idx     = get_txw_idx(TX_16X4);
    const int32_t txh_idx     = get_txh_idx(TX_16X4);
    int32_t       bitcol      = fwd_cos_bit_col[txw_idx][txh_idx];
    int32_t       bitrow      = fwd_cos_bit_row[txw_idx][txh_idx];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fdct4x8_row_N4_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fdct16x4_N4_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_quad_avx2(in, outcoeff256);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case ADST_DCT:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fadst4x8_row_N4_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fdct16x4_N4_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_quad_avx2(in, outcoeff256);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case DCT_ADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fdct4x8_row_N4_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fadst16x4_N4_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_quad_avx2(in, outcoeff256);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case ADST_ADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fadst4x8_row_N4_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fadst16x4_N4_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_quad_avx2(in, outcoeff256);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case FLIPADST_DCT:
        load_buffer_16x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fadst4x8_row_N4_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fdct16x4_N4_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_quad_avx2(in, outcoeff256);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case DCT_FLIPADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 1, shift[0]);
        fdct4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fdct4x8_row_N4_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fadst16x4_N4_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_quad_avx2(in, outcoeff256);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x4_avx2(input, in, stride, 1, 1, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fadst4x8_row_N4_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fadst16x4_N4_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_quad_avx2(in, outcoeff256);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case ADST_FLIPADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 1, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fadst4x8_row_N4_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fadst16x4_N4_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_quad_avx2(in, outcoeff256);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case FLIPADST_ADST:
        load_buffer_16x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        fadst4x8_row_N4_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, 1, -shift[1]);
        fadst16x4_N4_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_quad_avx2(in, outcoeff256);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case IDTX:
        load_buffer_4x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N4_avx2(in, outcoeff256, bitcol);
        __m256i rounding = _mm256_set1_epi32(1 << (-shift[1] - 1));
        outcoeff256[0]   = _mm256_add_epi32(outcoeff256[0], rounding);
        outcoeff256[0]   = _mm256_srai_epi32(outcoeff256[0], -shift[1]);
        fidtx4x8_N2_perm_avx2(outcoeff256, outcoeff256, bitrow);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case V_DCT:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fdct4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        fidtx8xn_N2_col_avx2(in, outcoeff256, bitrow, 1);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case H_DCT:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, -shift[1]);
        fidtx4x8_row_N4_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, -shift[1]);
        fdct16x4_N4_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_quad_avx2(in, outcoeff256);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case V_ADST:
        load_buffer_8x4_avx2(input, in, stride, 0, 0, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        fidtx8xn_N2_col_avx2(in, outcoeff256, bitrow, 1);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case H_ADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 0, shift[0]);
        fidtx4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, -shift[1]);
        fidtx4x8_row_N4_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, -shift[1]);
        fadst16x4_N4_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_quad_avx2(in, outcoeff256);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case V_FLIPADST:
        load_buffer_8x4_avx2(input, in, stride, 1, 0, shift[0]);
        fadst4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, 1, -shift[1]);
        transpose_4x8_in_4x16_quad_avx2(outcoeff256, in);
        fidtx8xn_N2_col_avx2(in, outcoeff256, bitrow, 1);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    case H_FLIPADST:
        load_buffer_16x4_avx2(input, in, stride, 0, 1, shift[0]);
        fidtx4x8_row_N4_with_round_avx2(in, outcoeff256, bitcol, -shift[1]);
        fidtx4x8_row_N4_with_round_avx2(in + 4, outcoeff256 + 4, bitcol, -shift[1]);
        fadst16x4_N4_avx2(outcoeff256, in, bitrow);
        transpose_4x8_in_4x16_quad_avx2(in, outcoeff256);
        clear_buffer_wxh_N4(outcoeff256, 2, 4);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

void svt_av1_fwd_txfm2d_32x64_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m256i       in[256];
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_32X64];
    const int32_t txw_idx       = get_txw_idx(TX_32X64);
    const int32_t txh_idx       = get_txh_idx(TX_32X64);
    const int32_t txfm_size_col = tx_size_wide[TX_32X64];
    const int32_t txfm_size_row = tx_size_high[TX_32X64];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    load_buffer_32x8n(input, in, stride, 0, 0, shift[0], txfm_size_row);
    av1_fdct64_new_N4_avx2(in, in, bitcol, txfm_size_col, num_col);
    col_txfm_16x16_rounding(in, -shift[1]);
    col_txfm_16x16_rounding(in + txfm_size_col, -shift[1]);
    transpose_8nx8n_N4_half(in, outcoef256, txfm_size_col, txfm_size_row);
    av1_fdct32_new_N4_avx2(outcoef256, in, bitrow, txfm_size_row / 2, num_row);
    transpose_8nx8n_N4_quad(in, outcoef256, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_wxh_N4(outcoef256, outcoef256, -shift[2], new_sqrt2, num_col, txfm_size_row);
    clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x32_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m256i       in[256];
    __m256i*      outcoef256    = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X32];
    const int32_t txw_idx       = get_txw_idx(TX_64X32);
    const int32_t txh_idx       = get_txh_idx(TX_64X32);
    const int32_t txfm_size_col = tx_size_wide[TX_64X32];
    const int32_t txfm_size_row = tx_size_high[TX_64X32];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 3;
    const int32_t num_col       = txfm_size_col >> 3;

    for (int32_t i = 0; i < 32; i++) {
        load_buffer_32_avx2(input + 0 + i * stride, in + 0 + i * 8, 8, 0, 0, shift[0]);
        load_buffer_32_avx2(input + 32 + i * stride, in + 4 + i * 8, 8, 0, 0, shift[0]);
    }
    av1_fdct32_new_N4_avx2(in, in, bitcol, txfm_size_col, num_col);
    col_txfm_16x16_rounding(in, -shift[1]);
    col_txfm_16x16_rounding(in + txfm_size_row, -shift[1]);
    transpose_8nx8n_N4_half(in, outcoef256, txfm_size_col, txfm_size_row);
    av1_fdct64_new_N4_avx2(outcoef256, in, bitrow, txfm_size_row / 2, num_row);
    transpose_8nx8n_N4_quad(in, outcoef256, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_wxh_N4(outcoef256, outcoef256, -shift[2], new_sqrt2, num_col, txfm_size_row);
    clear_buffer_wxh_N4(outcoef256, num_col, txfm_size_row);
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x64_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[128];
    __m256i*      outcoeff256   = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X64];
    const int32_t txw_idx       = get_txw_idx(TX_16X64);
    const int32_t txh_idx       = get_txh_idx(TX_16X64);
    const int32_t txfm_size_col = tx_size_wide[TX_16X64];
    const int32_t txfm_size_row = tx_size_high[TX_16X64];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    int32_t       ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    const int32_t num_row = txfm_size_row >> 3;
    const int32_t num_col = txfm_size_col >> 3;
    // col tranform
    for (int32_t i = 0; i < txfm_size_row; i += num_col) {
        load_buffer_16_avx2(input + (i + 0) * stride, in + (i + 0) * num_col, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_avx2(input + (i + 1) * stride, in + (i + 1) * num_col, 8, ud_flip, lr_flip, shift[0]);
    }
    av1_fdct64_new_N4_avx2(in, outcoeff256, bitcol, txfm_size_col, num_col);
    col_txfm_16x16_rounding(outcoeff256, -shift[1]);
    transpose_8nx8n_N4_half(outcoeff256, in, txfm_size_col, txfm_size_row);
    // row tranform
    fdct16x16_N4_avx2(in, in, bitrow, num_row, num_row / 4);
    transpose_8nx8n_N4_quad(in, outcoeff256, txfm_size_row, txfm_size_col);
    clear_buffer_wxh_N4(outcoeff256, num_col, txfm_size_row);
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x16_N4_avx2(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m256i       in[128];
    __m256i*      outcoeff256   = (__m256i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X16];
    const int32_t txw_idx       = get_txw_idx(TX_64X16);
    const int32_t txh_idx       = get_txh_idx(TX_64X16);
    const int32_t txfm_size_col = tx_size_wide[TX_64X16];
    const int32_t txfm_size_row = tx_size_high[TX_64X16];
    int8_t        bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    int8_t        bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    int32_t       ud_flip, lr_flip;
    get_flip_cfg(tx_type, &ud_flip, &lr_flip);
    const int32_t num_row = txfm_size_row >> 3;
    const int32_t num_col = txfm_size_col >> 3;
    // col tranform
    for (int32_t i = 0; i < txfm_size_row; i++) {
        load_buffer_16_avx2(input + 0 + i * stride, in + 0 + i * 8, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_avx2(input + 16 + i * stride, in + 2 + i * 8, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_avx2(input + 32 + i * stride, in + 4 + i * 8, 8, ud_flip, lr_flip, shift[0]);
        load_buffer_16_avx2(input + 48 + i * stride, in + 6 + i * 8, 8, ud_flip, lr_flip, shift[0]);
    }

    fdct16x16_N4_avx2(in, outcoeff256, bitcol, num_col, num_col);
    col_txfm_16x16_rounding(outcoeff256, -shift[1]);
    transpose_8nx8n_N4_half(outcoeff256, in, txfm_size_col, txfm_size_row);
    // row tranform
    av1_fdct64_new_N4_avx2(in, in, bitrow, txfm_size_row / 2, num_row);
    transpose_8nx8n_N4_quad(in, outcoeff256, txfm_size_row, txfm_size_col);
    clear_buffer_wxh_N4(outcoeff256, num_col, txfm_size_row);
    (void)bd;
}
