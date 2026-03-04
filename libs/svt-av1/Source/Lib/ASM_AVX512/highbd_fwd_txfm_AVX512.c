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
#include <assert.h>
#include "aom_dsp_rtcd.h"
#include "transforms.h"
#include <immintrin.h>
#include "transpose_encoder_avx512.h"

const int32_t* cospi_arr(int32_t n);
const int32_t* sinpi_arr(int32_t n);

void svt_aom_transform_config(TxType tx_type, TxSize tx_size, Txfm2dFlipCfg* cfg);

typedef void (*fwd_transform_1d_avx512)(const __m512i* in, __m512i* out, const int8_t bit, const int32_t num_cols);

#define btf_32_type0_avx512_new(ww0, ww1, in0, in1, out0, out1, r, bit) \
    do {                                                                \
        const __m512i in0_w0 = _mm512_mullo_epi32(in0, ww0);            \
        const __m512i in1_w1 = _mm512_mullo_epi32(in1, ww1);            \
        out0                 = _mm512_add_epi32(in0_w0, in1_w1);        \
        out0                 = _mm512_add_epi32(out0, r);               \
        out0                 = _mm512_srai_epi32(out0, (uint8_t)bit);   \
        const __m512i in0_w1 = _mm512_mullo_epi32(in0, ww1);            \
        const __m512i in1_w0 = _mm512_mullo_epi32(in1, ww0);            \
        out1                 = _mm512_sub_epi32(in0_w1, in1_w0);        \
        out1                 = _mm512_add_epi32(out1, r);               \
        out1                 = _mm512_srai_epi32(out1, (uint8_t)bit);   \
    } while (0)

// out0 = in0*w0 + in1*w1
// out1 = in1*w0 - in0*w1
#define btf_32_type1_avx512_new(ww0, ww1, in0, in1, out0, out1, r, bit)  \
    do {                                                                 \
        btf_32_type0_avx512_new(ww1, ww0, in1, in0, out0, out1, r, bit); \
    } while (0)

static INLINE void load_buffer_16x16_avx512(const int16_t* input, __m512i* out, int32_t stride, int32_t flipud,
                                            int32_t fliplr, const int8_t shift) {
    __m256i temp[16];
    uint8_t ushift = (uint8_t)shift;
    if (flipud) {
        /* load rows upside down (bottom to top) */
        for (int32_t i = 0; i < 16; i++) {
            int idx   = 15 - i;
            temp[idx] = _mm256_loadu_si256((const __m256i*)(input + i * stride));
            out[idx]  = _mm512_cvtepi16_epi32(temp[idx]);
            out[idx]  = _mm512_slli_epi32(out[idx], ushift);
        }
    } else {
        /* load rows normally */
        for (int32_t i = 0; i < 16; i++) {
            temp[i] = _mm256_loadu_si256((const __m256i*)(input + i * stride));
            out[i]  = _mm512_cvtepi16_epi32(temp[i]);
            out[i]  = _mm512_slli_epi32(out[i], ushift);
        }
    }

    if (fliplr) {
        /*flip columns left to right*/
        uint32_t idx[] = {15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0};
        __m512i  index = _mm512_loadu_si512(idx);

        for (int32_t i = 0; i < 16; i++) {
            out[i] = _mm512_permutexvar_epi32(index, out[i]);
        }
    }
}

static void fidtx16x16_avx512(const __m512i* in, __m512i* out, const int8_t bit, const int32_t col_num) {
    (void)bit;
    const uint8_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 2 * 5793; // 2 * new_sqrt2
    const __m512i newsqrt  = _mm512_set1_epi32(sqrt);
    const __m512i rounding = _mm512_set1_epi32(1 << (bits - 1));
    __m512i       temp;
    int32_t       num_iters = 16 * col_num;
    for (int32_t i = 0; i < num_iters; i++) {
        temp   = _mm512_mullo_epi32(in[i], newsqrt);
        temp   = _mm512_add_epi32(temp, rounding);
        out[i] = _mm512_srai_epi32(temp, bits);
    }
}

static INLINE void col_txfm_16x16_rounding_avx512(__m512i* in, const int8_t shift) {
    uint8_t       ushift   = (uint8_t)shift;
    const __m512i rounding = _mm512_set1_epi32(1 << (ushift - 1));
    for (int32_t i = 0; i < 16; i++) {
        in[i] = _mm512_add_epi32(in[i], rounding);
        in[i] = _mm512_srai_epi32(in[i], ushift);
    }
}

static INLINE void write_buffer_16x16(const __m512i* res, int32_t* output) {
    int32_t fact = -1, index = -1;
    for (int32_t i = 0; i < 8; i++) {
        _mm512_storeu_si512((__m512i*)(output + (++fact) * 32), res[++index]);
        _mm512_storeu_si512((__m512i*)(output + (fact) * 32 + 16), res[++index]);
    }
}

static INLINE __m512i half_btf_avx512(const __m512i* w0, const __m512i* n0, const __m512i* w1, const __m512i* n1,
                                      const __m512i* rounding, uint8_t bit) {
    __m512i x, y;

    x = _mm512_mullo_epi32(*w0, *n0);
    y = _mm512_mullo_epi32(*w1, *n1);
    x = _mm512_add_epi32(x, y);
    x = _mm512_add_epi32(x, *rounding);
    x = _mm512_srai_epi32(x, bit);
    return x;
}

static void fadst16x16_avx512(const __m512i* in, __m512i* out, const int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m512i  cospi32  = _mm512_set1_epi32(cospi[32]);
    const __m512i  cospi48  = _mm512_set1_epi32(cospi[48]);
    const __m512i  cospi16  = _mm512_set1_epi32(cospi[16]);
    const __m512i  cospim16 = _mm512_set1_epi32(-cospi[16]);
    const __m512i  cospim48 = _mm512_set1_epi32(-cospi[48]);
    const __m512i  cospi8   = _mm512_set1_epi32(cospi[8]);
    const __m512i  cospi56  = _mm512_set1_epi32(cospi[56]);
    const __m512i  cospim56 = _mm512_set1_epi32(-cospi[56]);
    const __m512i  cospim8  = _mm512_set1_epi32(-cospi[8]);
    const __m512i  cospi24  = _mm512_set1_epi32(cospi[24]);
    const __m512i  cospim24 = _mm512_set1_epi32(-cospi[24]);
    const __m512i  cospim40 = _mm512_set1_epi32(-cospi[40]);
    const __m512i  cospi40  = _mm512_set1_epi32(cospi[40]);
    const __m512i  cospi2   = _mm512_set1_epi32(cospi[2]);
    const __m512i  cospi62  = _mm512_set1_epi32(cospi[62]);
    const __m512i  cospim2  = _mm512_set1_epi32(-cospi[2]);
    const __m512i  cospi10  = _mm512_set1_epi32(cospi[10]);
    const __m512i  cospi54  = _mm512_set1_epi32(cospi[54]);
    const __m512i  cospim10 = _mm512_set1_epi32(-cospi[10]);
    const __m512i  cospi18  = _mm512_set1_epi32(cospi[18]);
    const __m512i  cospi46  = _mm512_set1_epi32(cospi[46]);
    const __m512i  cospim18 = _mm512_set1_epi32(-cospi[18]);
    const __m512i  cospi26  = _mm512_set1_epi32(cospi[26]);
    const __m512i  cospi38  = _mm512_set1_epi32(cospi[38]);
    const __m512i  cospim26 = _mm512_set1_epi32(-cospi[26]);
    const __m512i  cospi34  = _mm512_set1_epi32(cospi[34]);
    const __m512i  cospi30  = _mm512_set1_epi32(cospi[30]);
    const __m512i  cospim34 = _mm512_set1_epi32(-cospi[34]);
    const __m512i  cospi42  = _mm512_set1_epi32(cospi[42]);
    const __m512i  cospi22  = _mm512_set1_epi32(cospi[22]);
    const __m512i  cospim42 = _mm512_set1_epi32(-cospi[42]);
    const __m512i  cospi50  = _mm512_set1_epi32(cospi[50]);
    const __m512i  cospi14  = _mm512_set1_epi32(cospi[14]);
    const __m512i  cospim50 = _mm512_set1_epi32(-cospi[50]);
    const __m512i  cospi58  = _mm512_set1_epi32(cospi[58]);
    const __m512i  cospi6   = _mm512_set1_epi32(cospi[6]);
    const __m512i  cospim58 = _mm512_set1_epi32(-cospi[58]);
    const __m512i  rnding   = _mm512_set1_epi32(1 << (bit - 1));
    const __m512i  zeroes   = _mm512_setzero_si512();

    __m512i u[16], v[16], x, y;
    int32_t col;

    for (col = 0; col < col_num; ++col) {
        // stage 1
        u[1]  = _mm512_sub_epi32(zeroes, in[15 * col_num + col]);
        u[2]  = _mm512_sub_epi32(zeroes, in[7 * col_num + col]);
        u[4]  = _mm512_sub_epi32(zeroes, in[3 * col_num + col]);
        u[7]  = _mm512_sub_epi32(zeroes, in[11 * col_num + col]);
        u[8]  = _mm512_sub_epi32(zeroes, in[1 * col_num + col]);
        u[11] = _mm512_sub_epi32(zeroes, in[9 * col_num + col]);
        u[13] = _mm512_sub_epi32(zeroes, in[13 * col_num + col]);
        u[14] = _mm512_sub_epi32(zeroes, in[5 * col_num + col]);

        // stage 2
        x    = _mm512_mullo_epi32(u[2], cospi32);
        y    = _mm512_mullo_epi32(in[8 * col_num + col], cospi32);
        v[2] = _mm512_add_epi32(x, y);
        v[2] = _mm512_add_epi32(v[2], rnding);
        v[2] = _mm512_srai_epi32(v[2], (uint8_t)bit);

        v[3] = _mm512_sub_epi32(x, y);
        v[3] = _mm512_add_epi32(v[3], rnding);
        v[3] = _mm512_srai_epi32(v[3], (uint8_t)bit);

        x    = _mm512_mullo_epi32(in[4 * col_num + col], cospi32);
        y    = _mm512_mullo_epi32(u[7], cospi32);
        v[6] = _mm512_add_epi32(x, y);
        v[6] = _mm512_add_epi32(v[6], rnding);
        v[6] = _mm512_srai_epi32(v[6], (uint8_t)bit);

        v[7] = _mm512_sub_epi32(x, y);
        v[7] = _mm512_add_epi32(v[7], rnding);
        v[7] = _mm512_srai_epi32(v[7], (uint8_t)bit);

        x     = _mm512_mullo_epi32(in[6 * col_num + col], cospi32);
        y     = _mm512_mullo_epi32(u[11], cospi32);
        v[10] = _mm512_add_epi32(x, y);
        v[10] = _mm512_add_epi32(v[10], rnding);
        v[10] = _mm512_srai_epi32(v[10], (uint8_t)bit);

        v[11] = _mm512_sub_epi32(x, y);
        v[11] = _mm512_add_epi32(v[11], rnding);
        v[11] = _mm512_srai_epi32(v[11], (uint8_t)bit);

        x     = _mm512_mullo_epi32(u[14], cospi32);
        y     = _mm512_mullo_epi32(in[10 * col_num + col], cospi32);
        v[14] = _mm512_add_epi32(x, y);
        v[14] = _mm512_add_epi32(v[14], rnding);
        v[14] = _mm512_srai_epi32(v[14], (uint8_t)bit);

        v[15] = _mm512_sub_epi32(x, y);
        v[15] = _mm512_add_epi32(v[15], rnding);
        v[15] = _mm512_srai_epi32(v[15], (uint8_t)bit);

        // stage 3
        u[0]  = _mm512_add_epi32(in[0 * col_num + col], v[2]);
        u[3]  = _mm512_sub_epi32(u[1], v[3]);
        u[1]  = _mm512_add_epi32(u[1], v[3]);
        u[2]  = _mm512_sub_epi32(in[0 * col_num + col], v[2]);
        u[6]  = _mm512_sub_epi32(u[4], v[6]);
        u[4]  = _mm512_add_epi32(u[4], v[6]);
        u[5]  = _mm512_add_epi32(in[12 * col_num + col], v[7]);
        u[7]  = _mm512_sub_epi32(in[12 * col_num + col], v[7]);
        u[10] = _mm512_sub_epi32(u[8], v[10]);
        u[8]  = _mm512_add_epi32(u[8], v[10]);
        u[9]  = _mm512_add_epi32(in[14 * col_num + col], v[11]);
        u[11] = _mm512_sub_epi32(in[14 * col_num + col], v[11]);
        u[12] = _mm512_add_epi32(in[2 * col_num + col], v[14]);
        u[15] = _mm512_sub_epi32(u[13], v[15]);
        u[13] = _mm512_add_epi32(u[13], v[15]);
        u[14] = _mm512_sub_epi32(in[2 * col_num + col], v[14]);

        // stage 4
        v[4]  = half_btf_avx512(&cospi16, &u[4], &cospi48, &u[5], &rnding, bit);
        v[5]  = half_btf_avx512(&cospi48, &u[4], &cospim16, &u[5], &rnding, bit);
        v[6]  = half_btf_avx512(&cospim48, &u[6], &cospi16, &u[7], &rnding, bit);
        v[7]  = half_btf_avx512(&cospi16, &u[6], &cospi48, &u[7], &rnding, bit);
        v[12] = half_btf_avx512(&cospi16, &u[12], &cospi48, &u[13], &rnding, bit);
        v[13] = half_btf_avx512(&cospi48, &u[12], &cospim16, &u[13], &rnding, bit);
        v[14] = half_btf_avx512(&cospim48, &u[14], &cospi16, &u[15], &rnding, bit);
        v[15] = half_btf_avx512(&cospi16, &u[14], &cospi48, &u[15], &rnding, bit);

        // stage 5
        u[4]  = _mm512_sub_epi32(u[0], v[4]);
        u[0]  = _mm512_add_epi32(u[0], v[4]);
        u[5]  = _mm512_sub_epi32(u[1], v[5]);
        u[1]  = _mm512_add_epi32(u[1], v[5]);
        u[6]  = _mm512_sub_epi32(u[2], v[6]);
        u[2]  = _mm512_add_epi32(u[2], v[6]);
        u[7]  = _mm512_sub_epi32(u[3], v[7]);
        u[3]  = _mm512_add_epi32(u[3], v[7]);
        u[12] = _mm512_sub_epi32(u[8], v[12]);
        u[8]  = _mm512_add_epi32(u[8], v[12]);
        u[13] = _mm512_sub_epi32(u[9], v[13]);
        u[9]  = _mm512_add_epi32(u[9], v[13]);
        u[14] = _mm512_sub_epi32(u[10], v[14]);
        u[10] = _mm512_add_epi32(u[10], v[14]);
        u[15] = _mm512_sub_epi32(u[11], v[15]);
        u[11] = _mm512_add_epi32(u[11], v[15]);

        // stage 6
        v[8]  = half_btf_avx512(&cospi8, &u[8], &cospi56, &u[9], &rnding, bit);
        v[9]  = half_btf_avx512(&cospi56, &u[8], &cospim8, &u[9], &rnding, bit);
        v[10] = half_btf_avx512(&cospi40, &u[10], &cospi24, &u[11], &rnding, bit);
        v[11] = half_btf_avx512(&cospi24, &u[10], &cospim40, &u[11], &rnding, bit);
        v[12] = half_btf_avx512(&cospim56, &u[12], &cospi8, &u[13], &rnding, bit);
        v[13] = half_btf_avx512(&cospi8, &u[12], &cospi56, &u[13], &rnding, bit);
        v[14] = half_btf_avx512(&cospim24, &u[14], &cospi40, &u[15], &rnding, bit);
        v[15] = half_btf_avx512(&cospi40, &u[14], &cospi24, &u[15], &rnding, bit);

        // stage 7
        u[8]  = _mm512_sub_epi32(u[0], v[8]);
        u[0]  = _mm512_add_epi32(u[0], v[8]);
        u[9]  = _mm512_sub_epi32(u[1], v[9]);
        u[1]  = _mm512_add_epi32(u[1], v[9]);
        u[10] = _mm512_sub_epi32(u[2], v[10]);
        u[2]  = _mm512_add_epi32(u[2], v[10]);
        u[11] = _mm512_sub_epi32(u[3], v[11]);
        u[3]  = _mm512_add_epi32(u[3], v[11]);
        u[12] = _mm512_sub_epi32(u[4], v[12]);
        u[4]  = _mm512_add_epi32(u[4], v[12]);
        u[13] = _mm512_sub_epi32(u[5], v[13]);
        u[5]  = _mm512_add_epi32(u[5], v[13]);
        u[14] = _mm512_sub_epi32(u[6], v[14]);
        u[6]  = _mm512_add_epi32(u[6], v[14]);
        u[15] = _mm512_sub_epi32(u[7], v[15]);
        u[7]  = _mm512_add_epi32(u[7], v[15]);

        // stage 8
        out[15 * col_num + col] = half_btf_avx512(&cospi2, &u[0], &cospi62, &u[1], &rnding, bit);
        out[0 * col_num + col]  = half_btf_avx512(&cospi62, &u[0], &cospim2, &u[1], &rnding, bit);
        out[13 * col_num + col] = half_btf_avx512(&cospi10, &u[2], &cospi54, &u[3], &rnding, bit);
        out[2 * col_num + col]  = half_btf_avx512(&cospi54, &u[2], &cospim10, &u[3], &rnding, bit);
        out[11 * col_num + col] = half_btf_avx512(&cospi18, &u[4], &cospi46, &u[5], &rnding, bit);
        out[4 * col_num + col]  = half_btf_avx512(&cospi46, &u[4], &cospim18, &u[5], &rnding, bit);
        out[9 * col_num + col]  = half_btf_avx512(&cospi26, &u[6], &cospi38, &u[7], &rnding, bit);
        out[6 * col_num + col]  = half_btf_avx512(&cospi38, &u[6], &cospim26, &u[7], &rnding, bit);
        out[7 * col_num + col]  = half_btf_avx512(&cospi34, &u[8], &cospi30, &u[9], &rnding, bit);
        out[8 * col_num + col]  = half_btf_avx512(&cospi30, &u[8], &cospim34, &u[9], &rnding, bit);
        out[5 * col_num + col]  = half_btf_avx512(&cospi42, &u[10], &cospi22, &u[11], &rnding, bit);
        out[10 * col_num + col] = half_btf_avx512(&cospi22, &u[10], &cospim42, &u[11], &rnding, bit);
        out[3 * col_num + col]  = half_btf_avx512(&cospi50, &u[12], &cospi14, &u[13], &rnding, bit);
        out[12 * col_num + col] = half_btf_avx512(&cospi14, &u[12], &cospim50, &u[13], &rnding, bit);
        out[1 * col_num + col]  = half_btf_avx512(&cospi58, &u[14], &cospi6, &u[15], &rnding, bit);
        out[14 * col_num + col] = half_btf_avx512(&cospi6, &u[14], &cospim58, &u[15], &rnding, bit);
    }
}

static void fdct16x16_avx512(const __m512i* in, __m512i* out, const int8_t bit, const int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m512i  cospi32  = _mm512_set1_epi32(cospi[32]);
    const __m512i  cospim32 = _mm512_set1_epi32(-cospi[32]);
    const __m512i  cospi48  = _mm512_set1_epi32(cospi[48]);
    const __m512i  cospi16  = _mm512_set1_epi32(cospi[16]);
    const __m512i  cospim48 = _mm512_set1_epi32(-cospi[48]);
    const __m512i  cospim16 = _mm512_set1_epi32(-cospi[16]);
    const __m512i  cospi56  = _mm512_set1_epi32(cospi[56]);
    const __m512i  cospi8   = _mm512_set1_epi32(cospi[8]);
    const __m512i  cospi24  = _mm512_set1_epi32(cospi[24]);
    const __m512i  cospi40  = _mm512_set1_epi32(cospi[40]);
    const __m512i  cospi60  = _mm512_set1_epi32(cospi[60]);
    const __m512i  cospi4   = _mm512_set1_epi32(cospi[4]);
    const __m512i  cospi28  = _mm512_set1_epi32(cospi[28]);
    const __m512i  cospi36  = _mm512_set1_epi32(cospi[36]);
    const __m512i  cospi44  = _mm512_set1_epi32(cospi[44]);
    const __m512i  cospi20  = _mm512_set1_epi32(cospi[20]);
    const __m512i  cospi12  = _mm512_set1_epi32(cospi[12]);
    const __m512i  cospi52  = _mm512_set1_epi32(cospi[52]);
    const __m512i  rnding   = _mm512_set1_epi32(1 << (bit - 1));
    __m512i        u[16], v[16], x;
    int32_t        col;

    for (col = 0; col < col_num; ++col) {
        // stage 0
        // stage 1
        u[0]  = _mm512_add_epi32(in[0 * col_num + col], in[15 * col_num + col]);
        u[15] = _mm512_sub_epi32(in[0 * col_num + col], in[15 * col_num + col]);
        u[1]  = _mm512_add_epi32(in[1 * col_num + col], in[14 * col_num + col]);
        u[14] = _mm512_sub_epi32(in[1 * col_num + col], in[14 * col_num + col]);
        u[2]  = _mm512_add_epi32(in[2 * col_num + col], in[13 * col_num + col]);
        u[13] = _mm512_sub_epi32(in[2 * col_num + col], in[13 * col_num + col]);
        u[3]  = _mm512_add_epi32(in[3 * col_num + col], in[12 * col_num + col]);
        u[12] = _mm512_sub_epi32(in[3 * col_num + col], in[12 * col_num + col]);
        u[4]  = _mm512_add_epi32(in[4 * col_num + col], in[11 * col_num + col]);
        u[11] = _mm512_sub_epi32(in[4 * col_num + col], in[11 * col_num + col]);
        u[5]  = _mm512_add_epi32(in[5 * col_num + col], in[10 * col_num + col]);
        u[10] = _mm512_sub_epi32(in[5 * col_num + col], in[10 * col_num + col]);
        u[6]  = _mm512_add_epi32(in[6 * col_num + col], in[9 * col_num + col]);
        u[9]  = _mm512_sub_epi32(in[6 * col_num + col], in[9 * col_num + col]);
        u[7]  = _mm512_add_epi32(in[7 * col_num + col], in[8 * col_num + col]);
        u[8]  = _mm512_sub_epi32(in[7 * col_num + col], in[8 * col_num + col]);

        // stage 2
        v[0] = _mm512_add_epi32(u[0], u[7]);
        v[7] = _mm512_sub_epi32(u[0], u[7]);
        v[1] = _mm512_add_epi32(u[1], u[6]);
        v[6] = _mm512_sub_epi32(u[1], u[6]);
        v[2] = _mm512_add_epi32(u[2], u[5]);
        v[5] = _mm512_sub_epi32(u[2], u[5]);
        v[3] = _mm512_add_epi32(u[3], u[4]);
        v[4] = _mm512_sub_epi32(u[3], u[4]);

        v[10] = _mm512_mullo_epi32(u[10], cospim32);
        x     = _mm512_mullo_epi32(u[13], cospi32);
        v[10] = _mm512_add_epi32(v[10], x);
        v[10] = _mm512_add_epi32(v[10], rnding);
        v[10] = _mm512_srai_epi32(v[10], (uint8_t)bit);

        v[13] = _mm512_mullo_epi32(u[10], cospi32);
        x     = _mm512_mullo_epi32(u[13], cospim32);
        v[13] = _mm512_sub_epi32(v[13], x);
        v[13] = _mm512_add_epi32(v[13], rnding);
        v[13] = _mm512_srai_epi32(v[13], (uint8_t)bit);

        v[11] = _mm512_mullo_epi32(u[11], cospim32);
        x     = _mm512_mullo_epi32(u[12], cospi32);
        v[11] = _mm512_add_epi32(v[11], x);
        v[11] = _mm512_add_epi32(v[11], rnding);
        v[11] = _mm512_srai_epi32(v[11], (uint8_t)bit);

        v[12] = _mm512_mullo_epi32(u[11], cospi32);
        x     = _mm512_mullo_epi32(u[12], cospim32);
        v[12] = _mm512_sub_epi32(v[12], x);
        v[12] = _mm512_add_epi32(v[12], rnding);
        v[12] = _mm512_srai_epi32(v[12], (uint8_t)bit);

        // stage 3
        u[0] = _mm512_add_epi32(v[0], v[3]);
        u[3] = _mm512_sub_epi32(v[0], v[3]);
        u[1] = _mm512_add_epi32(v[1], v[2]);
        u[2] = _mm512_sub_epi32(v[1], v[2]);

        u[5] = _mm512_mullo_epi32(v[5], cospim32);
        x    = _mm512_mullo_epi32(v[6], cospi32);
        u[5] = _mm512_add_epi32(u[5], x);
        u[5] = _mm512_add_epi32(u[5], rnding);
        u[5] = _mm512_srai_epi32(u[5], (uint8_t)bit);

        u[6] = _mm512_mullo_epi32(v[5], cospi32);
        x    = _mm512_mullo_epi32(v[6], cospim32);
        u[6] = _mm512_sub_epi32(u[6], x);
        u[6] = _mm512_add_epi32(u[6], rnding);
        u[6] = _mm512_srai_epi32(u[6], (uint8_t)bit);

        u[11] = _mm512_sub_epi32(u[8], v[11]);
        u[8]  = _mm512_add_epi32(u[8], v[11]);
        u[10] = _mm512_sub_epi32(u[9], v[10]);
        u[9]  = _mm512_add_epi32(u[9], v[10]);
        u[12] = _mm512_sub_epi32(u[15], v[12]);
        u[15] = _mm512_add_epi32(u[15], v[12]);
        u[13] = _mm512_sub_epi32(u[14], v[13]);
        u[14] = _mm512_add_epi32(u[14], v[13]);

        // stage 4
        u[0]                   = _mm512_mullo_epi32(u[0], cospi32);
        u[1]                   = _mm512_mullo_epi32(u[1], cospi32);
        v[0]                   = _mm512_add_epi32(u[0], u[1]);
        v[0]                   = _mm512_add_epi32(v[0], rnding);
        out[0 * col_num + col] = _mm512_srai_epi32(v[0], (uint8_t)bit);

        v[1]                   = _mm512_sub_epi32(u[0], u[1]);
        v[1]                   = _mm512_add_epi32(v[1], rnding);
        out[8 * col_num + col] = _mm512_srai_epi32(v[1], (uint8_t)bit);

        v[2]                   = _mm512_mullo_epi32(u[2], cospi48);
        x                      = _mm512_mullo_epi32(u[3], cospi16);
        v[2]                   = _mm512_add_epi32(v[2], x);
        v[2]                   = _mm512_add_epi32(v[2], rnding);
        out[4 * col_num + col] = _mm512_srai_epi32(v[2], (uint8_t)bit);

        v[3]                    = _mm512_mullo_epi32(u[2], cospi16);
        x                       = _mm512_mullo_epi32(u[3], cospi48);
        v[3]                    = _mm512_sub_epi32(x, v[3]);
        v[3]                    = _mm512_add_epi32(v[3], rnding);
        out[12 * col_num + col] = _mm512_srai_epi32(v[3], (uint8_t)bit);

        v[5] = _mm512_sub_epi32(v[4], u[5]);
        v[4] = _mm512_add_epi32(v[4], u[5]);
        v[6] = _mm512_sub_epi32(v[7], u[6]);
        v[7] = _mm512_add_epi32(v[7], u[6]);

        v[9] = _mm512_mullo_epi32(u[9], cospim16);
        x    = _mm512_mullo_epi32(u[14], cospi48);
        v[9] = _mm512_add_epi32(v[9], x);
        v[9] = _mm512_add_epi32(v[9], rnding);
        v[9] = _mm512_srai_epi32(v[9], (uint8_t)bit);

        v[14] = _mm512_mullo_epi32(u[9], cospi48);
        x     = _mm512_mullo_epi32(u[14], cospim16);
        v[14] = _mm512_sub_epi32(v[14], x);
        v[14] = _mm512_add_epi32(v[14], rnding);
        v[14] = _mm512_srai_epi32(v[14], (uint8_t)bit);

        v[10] = _mm512_mullo_epi32(u[10], cospim48);
        x     = _mm512_mullo_epi32(u[13], cospim16);
        v[10] = _mm512_add_epi32(v[10], x);
        v[10] = _mm512_add_epi32(v[10], rnding);
        v[10] = _mm512_srai_epi32(v[10], (uint8_t)bit);

        v[13] = _mm512_mullo_epi32(u[10], cospim16);
        x     = _mm512_mullo_epi32(u[13], cospim48);
        v[13] = _mm512_sub_epi32(v[13], x);
        v[13] = _mm512_add_epi32(v[13], rnding);
        v[13] = _mm512_srai_epi32(v[13], (uint8_t)bit);

        // stage 5
        u[4]                   = _mm512_mullo_epi32(v[4], cospi56);
        x                      = _mm512_mullo_epi32(v[7], cospi8);
        u[4]                   = _mm512_add_epi32(u[4], x);
        u[4]                   = _mm512_add_epi32(u[4], rnding);
        out[2 * col_num + col] = _mm512_srai_epi32(u[4], (uint8_t)bit);

        u[7]                    = _mm512_mullo_epi32(v[4], cospi8);
        x                       = _mm512_mullo_epi32(v[7], cospi56);
        u[7]                    = _mm512_sub_epi32(x, u[7]);
        u[7]                    = _mm512_add_epi32(u[7], rnding);
        out[14 * col_num + col] = _mm512_srai_epi32(u[7], (uint8_t)bit);

        u[5]                    = _mm512_mullo_epi32(v[5], cospi24);
        x                       = _mm512_mullo_epi32(v[6], cospi40);
        u[5]                    = _mm512_add_epi32(u[5], x);
        u[5]                    = _mm512_add_epi32(u[5], rnding);
        out[10 * col_num + col] = _mm512_srai_epi32(u[5], (uint8_t)bit);

        u[6]                   = _mm512_mullo_epi32(v[5], cospi40);
        x                      = _mm512_mullo_epi32(v[6], cospi24);
        u[6]                   = _mm512_sub_epi32(x, u[6]);
        u[6]                   = _mm512_add_epi32(u[6], rnding);
        out[6 * col_num + col] = _mm512_srai_epi32(u[6], (uint8_t)bit);

        u[9]  = _mm512_sub_epi32(u[8], v[9]);
        u[8]  = _mm512_add_epi32(u[8], v[9]);
        u[10] = _mm512_sub_epi32(u[11], v[10]);
        u[11] = _mm512_add_epi32(u[11], v[10]);
        u[13] = _mm512_sub_epi32(u[12], v[13]);
        u[12] = _mm512_add_epi32(u[12], v[13]);
        u[14] = _mm512_sub_epi32(u[15], v[14]);
        u[15] = _mm512_add_epi32(u[15], v[14]);

        // stage 6
        v[8]                   = _mm512_mullo_epi32(u[8], cospi60);
        x                      = _mm512_mullo_epi32(u[15], cospi4);
        v[8]                   = _mm512_add_epi32(v[8], x);
        v[8]                   = _mm512_add_epi32(v[8], rnding);
        out[1 * col_num + col] = _mm512_srai_epi32(v[8], (uint8_t)bit);

        v[15]                   = _mm512_mullo_epi32(u[8], cospi4);
        x                       = _mm512_mullo_epi32(u[15], cospi60);
        v[15]                   = _mm512_sub_epi32(x, v[15]);
        v[15]                   = _mm512_add_epi32(v[15], rnding);
        out[15 * col_num + col] = _mm512_srai_epi32(v[15], (uint8_t)bit);

        v[9]                   = _mm512_mullo_epi32(u[9], cospi28);
        x                      = _mm512_mullo_epi32(u[14], cospi36);
        v[9]                   = _mm512_add_epi32(v[9], x);
        v[9]                   = _mm512_add_epi32(v[9], rnding);
        out[9 * col_num + col] = _mm512_srai_epi32(v[9], (uint8_t)bit);

        v[14]                  = _mm512_mullo_epi32(u[9], cospi36);
        x                      = _mm512_mullo_epi32(u[14], cospi28);
        v[14]                  = _mm512_sub_epi32(x, v[14]);
        v[14]                  = _mm512_add_epi32(v[14], rnding);
        out[7 * col_num + col] = _mm512_srai_epi32(v[14], (uint8_t)bit);

        v[10]                  = _mm512_mullo_epi32(u[10], cospi44);
        x                      = _mm512_mullo_epi32(u[13], cospi20);
        v[10]                  = _mm512_add_epi32(v[10], x);
        v[10]                  = _mm512_add_epi32(v[10], rnding);
        out[5 * col_num + col] = _mm512_srai_epi32(v[10], (uint8_t)bit);

        v[13]                   = _mm512_mullo_epi32(u[10], cospi20);
        x                       = _mm512_mullo_epi32(u[13], cospi44);
        v[13]                   = _mm512_sub_epi32(x, v[13]);
        v[13]                   = _mm512_add_epi32(v[13], rnding);
        out[11 * col_num + col] = _mm512_srai_epi32(v[13], (uint8_t)bit);

        v[11]                   = _mm512_mullo_epi32(u[11], cospi12);
        x                       = _mm512_mullo_epi32(u[12], cospi52);
        v[11]                   = _mm512_add_epi32(v[11], x);
        v[11]                   = _mm512_add_epi32(v[11], rnding);
        out[13 * col_num + col] = _mm512_srai_epi32(v[11], (uint8_t)bit);

        v[12]                  = _mm512_mullo_epi32(u[11], cospi52);
        x                      = _mm512_mullo_epi32(u[12], cospi12);
        v[12]                  = _mm512_sub_epi32(x, v[12]);
        v[12]                  = _mm512_add_epi32(v[12], rnding);
        out[3 * col_num + col] = _mm512_srai_epi32(v[12], (uint8_t)bit);
    }
}

void svt_av1_fwd_txfm2d_16x16_avx512(int16_t* input, int32_t* coeff, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m512i       in[16], out[16];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_16X16];
    const int32_t txw_idx = get_txw_idx(TX_16X16);
    const int32_t txh_idx = get_txh_idx(TX_16X16);
    const int32_t col_num = 1;
    switch (tx_type) {
    case IDTX:
        load_buffer_16x16_avx512(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_avx512(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(out, -shift[1]);
        fidtx16x16_avx512(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16(out, coeff);
        break;
    case DCT_DCT:
        load_buffer_16x16_avx512(input, in, stride, 0, 0, shift[0]);
        fdct16x16_avx512(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(out, -shift[1]);
        transpose_16x16_avx512(out, in);
        fdct16x16_avx512(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx512(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case ADST_DCT:
        load_buffer_16x16_avx512(input, in, stride, 0, 0, shift[0]);
        fadst16x16_avx512(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(out, -shift[1]);
        transpose_16x16_avx512(out, in);
        fdct16x16_avx512(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx512(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case DCT_ADST:
        load_buffer_16x16_avx512(input, in, stride, 0, 0, shift[0]);
        fdct16x16_avx512(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(out, -shift[1]);
        transpose_16x16_avx512(out, in);
        fadst16x16_avx512(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx512(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case ADST_ADST:
        load_buffer_16x16_avx512(input, in, stride, 0, 0, shift[0]);
        fadst16x16_avx512(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(out, -shift[1]);
        transpose_16x16_avx512(out, in);
        fadst16x16_avx512(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx512(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case DCT_FLIPADST:
        load_buffer_16x16_avx512(input, in, stride, 0, 1, shift[0]);
        fdct16x16_avx512(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(out, -shift[1]);
        transpose_16x16_avx512(out, in);
        fadst16x16_avx512(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx512(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case FLIPADST_DCT:
        load_buffer_16x16_avx512(input, in, stride, 1, 0, shift[0]);
        fadst16x16_avx512(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(out, -shift[1]);
        transpose_16x16_avx512(out, in);
        fdct16x16_avx512(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx512(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case FLIPADST_FLIPADST:
        load_buffer_16x16_avx512(input, in, stride, 1, 1, shift[0]);
        fadst16x16_avx512(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(out, -shift[1]);
        transpose_16x16_avx512(out, in);
        fadst16x16_avx512(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx512(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case ADST_FLIPADST:
        load_buffer_16x16_avx512(input, in, stride, 0, 1, shift[0]);
        fadst16x16_avx512(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(out, -shift[1]);
        transpose_16x16_avx512(out, in);
        fadst16x16_avx512(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx512(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case FLIPADST_ADST:
        load_buffer_16x16_avx512(input, in, stride, 1, 0, shift[0]);
        fadst16x16_avx512(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(out, -shift[1]);
        transpose_16x16_avx512(out, in);
        fadst16x16_avx512(in, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx512(out, in);
        write_buffer_16x16(in, coeff);
        break;
    case V_DCT:
        load_buffer_16x16_avx512(input, in, stride, 0, 0, shift[0]);
        fdct16x16_avx512(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(out, -shift[1]);
        fidtx16x16_avx512(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16(out, coeff);
        break;
    case H_DCT:
        load_buffer_16x16_avx512(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_avx512(in, in, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(in, -shift[1]);
        transpose_16x16_avx512(in, out);
        fdct16x16_avx512(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx512(in, out);
        write_buffer_16x16(out, coeff);
        break;
    case V_ADST:
        load_buffer_16x16_avx512(input, in, stride, 0, 0, shift[0]);
        fadst16x16_avx512(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(out, -shift[1]);
        fidtx16x16_avx512(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16(out, coeff);
        break;
    case H_ADST:
        load_buffer_16x16_avx512(input, in, stride, 0, 0, shift[0]);
        fidtx16x16_avx512(in, in, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(in, -shift[1]);
        transpose_16x16_avx512(in, out);
        fadst16x16_avx512(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx512(in, out);
        write_buffer_16x16(out, coeff);
        break;
    case V_FLIPADST:
        load_buffer_16x16_avx512(input, in, stride, 1, 0, shift[0]);
        fadst16x16_avx512(in, out, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(out, -shift[1]);
        fidtx16x16_avx512(out, out, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        write_buffer_16x16(out, coeff);
        break;
    case H_FLIPADST:
        load_buffer_16x16_avx512(input, in, stride, 0, 1, shift[0]);
        fidtx16x16_avx512(in, in, fwd_cos_bit_col[txw_idx][txh_idx], col_num);
        col_txfm_16x16_rounding_avx512(in, -shift[1]);
        transpose_16x16_avx512(in, out);
        fadst16x16_avx512(out, in, fwd_cos_bit_row[txw_idx][txh_idx], col_num);
        transpose_16x16_avx512(in, out);
        write_buffer_16x16(out, coeff);
        break;
    default:
        assert(0);
    }
    (void)bd;
}

static void av1_fdct32_new_avx512(const __m512i* input, __m512i* output, const int8_t cos_bit, const int32_t col_num,
                                  const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m512i  __rounding = _mm512_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 4;

    __m512i cospi_m32 = _mm512_set1_epi32(-cospi[32]);
    __m512i cospi_p32 = _mm512_set1_epi32(cospi[32]);
    __m512i cospi_m16 = _mm512_set1_epi32(-cospi[16]);
    __m512i cospi_p48 = _mm512_set1_epi32(cospi[48]);
    __m512i cospi_m48 = _mm512_set1_epi32(-cospi[48]);
    __m512i cospi_m08 = _mm512_set1_epi32(-cospi[8]);
    __m512i cospi_p56 = _mm512_set1_epi32(cospi[56]);
    __m512i cospi_m56 = _mm512_set1_epi32(-cospi[56]);
    __m512i cospi_p40 = _mm512_set1_epi32(cospi[40]);
    __m512i cospi_m40 = _mm512_set1_epi32(-cospi[40]);
    __m512i cospi_p24 = _mm512_set1_epi32(cospi[24]);
    __m512i cospi_m24 = _mm512_set1_epi32(-cospi[24]);
    __m512i cospi_p16 = _mm512_set1_epi32(cospi[16]);
    __m512i cospi_p08 = _mm512_set1_epi32(cospi[8]);
    __m512i cospi_p04 = _mm512_set1_epi32(cospi[4]);
    __m512i cospi_p60 = _mm512_set1_epi32(cospi[60]);
    __m512i cospi_p36 = _mm512_set1_epi32(cospi[36]);
    __m512i cospi_p28 = _mm512_set1_epi32(cospi[28]);
    __m512i cospi_p20 = _mm512_set1_epi32(cospi[20]);
    __m512i cospi_p44 = _mm512_set1_epi32(cospi[44]);
    __m512i cospi_p52 = _mm512_set1_epi32(cospi[52]);
    __m512i cospi_p12 = _mm512_set1_epi32(cospi[12]);
    __m512i cospi_p02 = _mm512_set1_epi32(cospi[2]);
    __m512i cospi_p06 = _mm512_set1_epi32(cospi[6]);
    __m512i cospi_p62 = _mm512_set1_epi32(cospi[62]);
    __m512i cospi_p34 = _mm512_set1_epi32(cospi[34]);
    __m512i cospi_p30 = _mm512_set1_epi32(cospi[30]);
    __m512i cospi_p18 = _mm512_set1_epi32(cospi[18]);
    __m512i cospi_p46 = _mm512_set1_epi32(cospi[46]);
    __m512i cospi_p50 = _mm512_set1_epi32(cospi[50]);
    __m512i cospi_p14 = _mm512_set1_epi32(cospi[14]);
    __m512i cospi_p10 = _mm512_set1_epi32(cospi[10]);
    __m512i cospi_p54 = _mm512_set1_epi32(cospi[54]);
    __m512i cospi_p42 = _mm512_set1_epi32(cospi[42]);
    __m512i cospi_p22 = _mm512_set1_epi32(cospi[22]);
    __m512i cospi_p26 = _mm512_set1_epi32(cospi[26]);
    __m512i cospi_p38 = _mm512_set1_epi32(cospi[38]);
    __m512i cospi_p58 = _mm512_set1_epi32(cospi[58]);

    __m512i buf0[32];
    __m512i buf1[32];

    for (int32_t col = 0; col < columns; col++) {
        const __m512i* in  = &input[col];
        __m512i*       out = &output[col];

        // stage 0
        // stage 1
        buf1[0]  = _mm512_add_epi32(in[0 * stride], in[31 * stride]);
        buf1[31] = _mm512_sub_epi32(in[0 * stride], in[31 * stride]);
        buf1[1]  = _mm512_add_epi32(in[1 * stride], in[30 * stride]);
        buf1[30] = _mm512_sub_epi32(in[1 * stride], in[30 * stride]);
        buf1[2]  = _mm512_add_epi32(in[2 * stride], in[29 * stride]);
        buf1[29] = _mm512_sub_epi32(in[2 * stride], in[29 * stride]);
        buf1[3]  = _mm512_add_epi32(in[3 * stride], in[28 * stride]);
        buf1[28] = _mm512_sub_epi32(in[3 * stride], in[28 * stride]);
        buf1[4]  = _mm512_add_epi32(in[4 * stride], in[27 * stride]);
        buf1[27] = _mm512_sub_epi32(in[4 * stride], in[27 * stride]);
        buf1[5]  = _mm512_add_epi32(in[5 * stride], in[26 * stride]);
        buf1[26] = _mm512_sub_epi32(in[5 * stride], in[26 * stride]);
        buf1[6]  = _mm512_add_epi32(in[6 * stride], in[25 * stride]);
        buf1[25] = _mm512_sub_epi32(in[6 * stride], in[25 * stride]);
        buf1[7]  = _mm512_add_epi32(in[7 * stride], in[24 * stride]);
        buf1[24] = _mm512_sub_epi32(in[7 * stride], in[24 * stride]);
        buf1[8]  = _mm512_add_epi32(in[8 * stride], in[23 * stride]);
        buf1[23] = _mm512_sub_epi32(in[8 * stride], in[23 * stride]);
        buf1[9]  = _mm512_add_epi32(in[9 * stride], in[22 * stride]);
        buf1[22] = _mm512_sub_epi32(in[9 * stride], in[22 * stride]);
        buf1[10] = _mm512_add_epi32(in[10 * stride], in[21 * stride]);
        buf1[21] = _mm512_sub_epi32(in[10 * stride], in[21 * stride]);
        buf1[11] = _mm512_add_epi32(in[11 * stride], in[20 * stride]);
        buf1[20] = _mm512_sub_epi32(in[11 * stride], in[20 * stride]);
        buf1[12] = _mm512_add_epi32(in[12 * stride], in[19 * stride]);
        buf1[19] = _mm512_sub_epi32(in[12 * stride], in[19 * stride]);
        buf1[13] = _mm512_add_epi32(in[13 * stride], in[18 * stride]);
        buf1[18] = _mm512_sub_epi32(in[13 * stride], in[18 * stride]);
        buf1[14] = _mm512_add_epi32(in[14 * stride], in[17 * stride]);
        buf1[17] = _mm512_sub_epi32(in[14 * stride], in[17 * stride]);
        buf1[15] = _mm512_add_epi32(in[15 * stride], in[16 * stride]);
        buf1[16] = _mm512_sub_epi32(in[15 * stride], in[16 * stride]);

        // stage 2
        buf0[0]  = _mm512_add_epi32(buf1[0], buf1[15]);
        buf0[15] = _mm512_sub_epi32(buf1[0], buf1[15]);
        buf0[1]  = _mm512_add_epi32(buf1[1], buf1[14]);
        buf0[14] = _mm512_sub_epi32(buf1[1], buf1[14]);
        buf0[2]  = _mm512_add_epi32(buf1[2], buf1[13]);
        buf0[13] = _mm512_sub_epi32(buf1[2], buf1[13]);
        buf0[3]  = _mm512_add_epi32(buf1[3], buf1[12]);
        buf0[12] = _mm512_sub_epi32(buf1[3], buf1[12]);
        buf0[4]  = _mm512_add_epi32(buf1[4], buf1[11]);
        buf0[11] = _mm512_sub_epi32(buf1[4], buf1[11]);
        buf0[5]  = _mm512_add_epi32(buf1[5], buf1[10]);
        buf0[10] = _mm512_sub_epi32(buf1[5], buf1[10]);
        buf0[6]  = _mm512_add_epi32(buf1[6], buf1[9]);
        buf0[9]  = _mm512_sub_epi32(buf1[6], buf1[9]);
        buf0[7]  = _mm512_add_epi32(buf1[7], buf1[8]);
        buf0[8]  = _mm512_sub_epi32(buf1[7], buf1[8]);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf1[23], buf1[24], buf0[23], buf0[24], __rounding, cos_bit);

        // stage 3
        buf1[0] = _mm512_add_epi32(buf0[0], buf0[7]);
        buf1[7] = _mm512_sub_epi32(buf0[0], buf0[7]);
        buf1[1] = _mm512_add_epi32(buf0[1], buf0[6]);
        buf1[6] = _mm512_sub_epi32(buf0[1], buf0[6]);
        buf1[2] = _mm512_add_epi32(buf0[2], buf0[5]);
        buf1[5] = _mm512_sub_epi32(buf0[2], buf0[5]);
        buf1[3] = _mm512_add_epi32(buf0[3], buf0[4]);
        buf1[4] = _mm512_sub_epi32(buf0[3], buf0[4]);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf0[11], buf0[12], buf1[11], buf1[12], __rounding, cos_bit);
        buf1[23] = _mm512_sub_epi32(buf1[16], buf0[23]);
        buf1[16] = _mm512_add_epi32(buf1[16], buf0[23]);
        buf1[22] = _mm512_sub_epi32(buf1[17], buf0[22]);
        buf1[17] = _mm512_add_epi32(buf1[17], buf0[22]);
        buf1[21] = _mm512_sub_epi32(buf1[18], buf0[21]);
        buf1[18] = _mm512_add_epi32(buf1[18], buf0[21]);
        buf1[20] = _mm512_sub_epi32(buf1[19], buf0[20]);
        buf1[19] = _mm512_add_epi32(buf1[19], buf0[20]);
        buf1[24] = _mm512_sub_epi32(buf1[31], buf0[24]);
        buf1[31] = _mm512_add_epi32(buf1[31], buf0[24]);
        buf1[25] = _mm512_sub_epi32(buf1[30], buf0[25]);
        buf1[30] = _mm512_add_epi32(buf1[30], buf0[25]);
        buf1[26] = _mm512_sub_epi32(buf1[29], buf0[26]);
        buf1[29] = _mm512_add_epi32(buf1[29], buf0[26]);
        buf1[27] = _mm512_sub_epi32(buf1[28], buf0[27]);
        buf1[28] = _mm512_add_epi32(buf1[28], buf0[27]);

        // stage 4
        buf0[0] = _mm512_add_epi32(buf1[0], buf1[3]);
        buf0[3] = _mm512_sub_epi32(buf1[0], buf1[3]);
        buf0[1] = _mm512_add_epi32(buf1[1], buf1[2]);
        buf0[2] = _mm512_sub_epi32(buf1[1], buf1[2]);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf1[5], buf1[6], buf0[5], buf0[6], __rounding, cos_bit);
        buf0[11] = _mm512_sub_epi32(buf0[8], buf1[11]);
        buf0[8]  = _mm512_add_epi32(buf0[8], buf1[11]);
        buf0[10] = _mm512_sub_epi32(buf0[9], buf1[10]);
        buf0[9]  = _mm512_add_epi32(buf0[9], buf1[10]);
        buf0[12] = _mm512_sub_epi32(buf0[15], buf1[12]);
        buf0[15] = _mm512_add_epi32(buf0[15], buf1[12]);
        buf0[13] = _mm512_sub_epi32(buf0[14], buf1[13]);
        buf0[14] = _mm512_add_epi32(buf0[14], buf1[13]);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, buf1[19], buf1[28], buf0[19], buf0[28], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);

        // stage 5
        btf_32_type0_avx512_new(
            cospi_p32, cospi_p32, buf0[0], buf0[1], out[0 * stride], out[16 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p48, cospi_p16, buf0[2], buf0[3], out[8 * stride], out[24 * stride], __rounding, cos_bit);
        buf1[5] = _mm512_sub_epi32(buf1[4], buf0[5]);
        buf1[4] = _mm512_add_epi32(buf1[4], buf0[5]);
        buf1[6] = _mm512_sub_epi32(buf1[7], buf0[6]);
        buf1[7] = _mm512_add_epi32(buf1[7], buf0[6]);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, buf0[9], buf0[14], buf1[9], buf1[14], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        buf1[19] = _mm512_sub_epi32(buf1[16], buf0[19]);
        buf1[16] = _mm512_add_epi32(buf1[16], buf0[19]);
        buf1[18] = _mm512_sub_epi32(buf1[17], buf0[18]);
        buf1[17] = _mm512_add_epi32(buf1[17], buf0[18]);
        buf1[20] = _mm512_sub_epi32(buf1[23], buf0[20]);
        buf1[23] = _mm512_add_epi32(buf1[23], buf0[20]);
        buf1[21] = _mm512_sub_epi32(buf1[22], buf0[21]);
        buf1[22] = _mm512_add_epi32(buf1[22], buf0[21]);
        buf1[27] = _mm512_sub_epi32(buf1[24], buf0[27]);
        buf1[24] = _mm512_add_epi32(buf1[24], buf0[27]);
        buf1[26] = _mm512_sub_epi32(buf1[25], buf0[26]);
        buf1[25] = _mm512_add_epi32(buf1[25], buf0[26]);
        buf1[28] = _mm512_sub_epi32(buf1[31], buf0[28]);
        buf1[31] = _mm512_add_epi32(buf1[31], buf0[28]);
        buf1[29] = _mm512_sub_epi32(buf1[30], buf0[29]);
        buf1[30] = _mm512_add_epi32(buf1[30], buf0[29]);

        // stage 6
        btf_32_type1_avx512_new(
            cospi_p56, cospi_p08, buf1[4], buf1[7], out[4 * stride], out[28 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p24, cospi_p40, buf1[5], buf1[6], out[20 * stride], out[12 * stride], __rounding, cos_bit);
        buf0[9]  = _mm512_sub_epi32(buf0[8], buf1[9]);
        buf0[8]  = _mm512_add_epi32(buf0[8], buf1[9]);
        buf0[10] = _mm512_sub_epi32(buf0[11], buf1[10]);
        buf0[11] = _mm512_add_epi32(buf0[11], buf1[10]);
        buf0[13] = _mm512_sub_epi32(buf0[12], buf1[13]);
        buf0[12] = _mm512_add_epi32(buf0[12], buf1[13]);
        buf0[14] = _mm512_sub_epi32(buf0[15], buf1[14]);
        buf0[15] = _mm512_add_epi32(buf0[15], buf1[14]);
        btf_32_type0_avx512_new(cospi_m08, cospi_p56, buf1[17], buf1[30], buf0[17], buf0[30], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m56, cospi_m08, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m40, cospi_p24, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m24, cospi_m40, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);

        // stage 7
        btf_32_type1_avx512_new(
            cospi_p60, cospi_p04, buf0[8], buf0[15], out[2 * stride], out[30 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p28, cospi_p36, buf0[9], buf0[14], out[18 * stride], out[14 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p44, cospi_p20, buf0[10], buf0[13], out[10 * stride], out[22 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p12, cospi_p52, buf0[11], buf0[12], out[26 * stride], out[6 * stride], __rounding, cos_bit);
        buf1[17] = _mm512_sub_epi32(buf1[16], buf0[17]);
        buf1[16] = _mm512_add_epi32(buf1[16], buf0[17]);
        buf1[18] = _mm512_sub_epi32(buf1[19], buf0[18]);
        buf1[19] = _mm512_add_epi32(buf1[19], buf0[18]);
        buf1[21] = _mm512_sub_epi32(buf1[20], buf0[21]);
        buf1[20] = _mm512_add_epi32(buf1[20], buf0[21]);
        buf1[22] = _mm512_sub_epi32(buf1[23], buf0[22]);
        buf1[23] = _mm512_add_epi32(buf1[23], buf0[22]);
        buf1[25] = _mm512_sub_epi32(buf1[24], buf0[25]);
        buf1[24] = _mm512_add_epi32(buf1[24], buf0[25]);
        buf1[26] = _mm512_sub_epi32(buf1[27], buf0[26]);
        buf1[27] = _mm512_add_epi32(buf1[27], buf0[26]);
        buf1[29] = _mm512_sub_epi32(buf1[28], buf0[29]);
        buf1[28] = _mm512_add_epi32(buf1[28], buf0[29]);
        buf1[30] = _mm512_sub_epi32(buf1[31], buf0[30]);
        buf1[31] = _mm512_add_epi32(buf1[31], buf0[30]);

        // stage 8
        btf_32_type1_avx512_new(
            cospi_p62, cospi_p02, buf1[16], buf1[31], out[1 * stride], out[31 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p30, cospi_p34, buf1[17], buf1[30], out[17 * stride], out[15 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p46, cospi_p18, buf1[18], buf1[29], out[9 * stride], out[23 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p14, cospi_p50, buf1[19], buf1[28], out[25 * stride], out[7 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p54, cospi_p10, buf1[20], buf1[27], out[5 * stride], out[27 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p22, cospi_p42, buf1[21], buf1[26], out[21 * stride], out[11 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p38, cospi_p26, buf1[22], buf1[25], out[13 * stride], out[19 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p06, cospi_p58, buf1[23], buf1[24], out[29 * stride], out[3 * stride], __rounding, cos_bit);
    }
}

static INLINE void fdct32x32_avx512(const __m512i* input, __m512i* output, const int8_t cos_bit,
                                    const int8_t* stage_range) {
    const int32_t txfm_size   = 32;
    const int32_t num_per_512 = 16;
    int32_t       col_num     = txfm_size / num_per_512;
    (void)stage_range;
    av1_fdct32_new_avx512(input, output, cos_bit, txfm_size, col_num);
}

void av1_idtx32_new_avx512(const __m512i* input, __m512i* output, const int8_t cos_bit, const int32_t col_num) {
    (void)cos_bit;
    for (int32_t i = 0; i < 32; i++) {
        output[i * col_num] = _mm512_slli_epi32(input[i * col_num], (uint8_t)2);
    }
}

static void fidtx32x32_avx512(const __m512i* input, __m512i* output, const int8_t cos_bit, const int8_t* stage_range) {
    (void)stage_range;

    for (int32_t i = 0; i < 2; i++) {
        av1_idtx32_new_avx512(&input[i * 32], &output[i * 32], cos_bit, 1);
    }
}

typedef void (*TxfmFuncAVX512)(const __m512i* input, __m512i* output, const int8_t cos_bit, const int8_t* stage_range);

static INLINE TxfmFuncAVX512 fwd_txfm_type_to_func_avx512(TxfmType txfmtype) {
    switch (txfmtype) {
    case TXFM_TYPE_DCT32:
        return fdct32x32_avx512;
        break;
    case TXFM_TYPE_IDENTITY32:
        return fidtx32x32_avx512;
        break;
    default:
        assert(0);
    }
    return NULL;
}

static INLINE void load_buffer_32x32_avx512(const int16_t* input, __m512i* output, int32_t stride) {
    __m256i temp[2];
    int32_t i;

    for (i = 0; i < 32; ++i) {
        temp[0] = _mm256_loadu_si256((const __m256i*)(input + 0 * 16));
        temp[1] = _mm256_loadu_si256((const __m256i*)(input + 1 * 16));

        output[0] = _mm512_cvtepi16_epi32(temp[0]);
        output[1] = _mm512_cvtepi16_epi32(temp[1]);

        input += stride;
        output += 2;
    }
}

static INLINE void av1_round_shift_array_avx512(__m512i* input, __m512i* output, const int32_t size, const int8_t bit) {
    if (bit > 0) {
        __m512i round = _mm512_set1_epi32(1 << (bit - 1));
        int32_t i;
        for (i = 0; i < size; i++) {
            output[i] = _mm512_srai_epi32(_mm512_add_epi32(input[i], round), (uint8_t)bit);
        }
    } else {
        int32_t i;
        for (i = 0; i < size; i++) {
            output[i] = _mm512_slli_epi32(input[i], (uint8_t)(-bit));
        }
    }
}

static INLINE void fwd_txfm2d_32x32_avx512(const int16_t* input, int32_t* output, const int32_t stride,
                                           const Txfm2dFlipCfg* cfg, int32_t* txfm_buf) {
    assert(cfg->tx_size < TX_SIZES);
    const int32_t        txfm_size       = tx_size_wide[cfg->tx_size];
    const int8_t*        shift           = cfg->shift;
    const int8_t*        stage_range_col = cfg->stage_range_col;
    const int8_t*        stage_range_row = cfg->stage_range_row;
    const int8_t         cos_bit_col     = cfg->cos_bit_col;
    const int8_t         cos_bit_row     = cfg->cos_bit_row;
    const TxfmFuncAVX512 txfm_func_col   = fwd_txfm_type_to_func_avx512(cfg->txfm_type_col);
    const TxfmFuncAVX512 txfm_func_row   = fwd_txfm_type_to_func_avx512(cfg->txfm_type_row);
    ASSERT(txfm_func_col);
    ASSERT(txfm_func_row);
    __m512i* buf_512         = (__m512i*)txfm_buf;
    __m512i* out_512         = (__m512i*)output;
    int32_t  num_per_512     = 16;
    int32_t  txfm2d_size_512 = txfm_size * txfm_size / num_per_512;

    load_buffer_32x32_avx512(input, buf_512, stride);
    av1_round_shift_array_avx512(buf_512, out_512, txfm2d_size_512, -shift[0]);
    txfm_func_col(out_512, buf_512, cos_bit_col, stage_range_col);
    av1_round_shift_array_avx512(buf_512, out_512, txfm2d_size_512, -shift[1]);
    transpose_16nx16n_avx512(txfm_size, out_512, buf_512);
    txfm_func_row(buf_512, out_512, cos_bit_row, stage_range_row);
    av1_round_shift_array_avx512(out_512, buf_512, txfm2d_size_512, -shift[2]);
    transpose_16nx16n_avx512(txfm_size, buf_512, out_512);
}

void svt_av1_fwd_txfm2d_32x32_avx512(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    DECLARE_ALIGNED(64, int32_t, txfm_buf[1024]);
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(tx_type, TX_32X32, &cfg);
    (void)bd;
    fwd_txfm2d_32x32_avx512(input, output, stride, &cfg, txfm_buf);
}

static void fidtx64x64_avx512(const __m512i* input, __m512i* output) {
    const uint8_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 4 * 5793; // 4 * new_sqrt2
    const int32_t col_num  = 4;
    const __m512i newsqrt  = _mm512_set1_epi32(sqrt);
    const __m512i rounding = _mm512_set1_epi32(1 << (bits - 1));

    __m512i temp;
    int32_t num_iters = 64 * col_num;
    for (int32_t i = 0; i < num_iters; i++) {
        temp      = _mm512_mullo_epi32(input[i], newsqrt);
        temp      = _mm512_add_epi32(temp, rounding);
        output[i] = _mm512_srai_epi32(temp, bits);
    }
}

static INLINE void load_buffer_64x64_avx512(const int16_t* input, int32_t stride, __m512i* output) {
    __m256i x0, x1, x2, x3;
    __m512i v0, v1, v2, v3;
    int32_t i;

    for (i = 0; i < 64; ++i) {
        x0 = _mm256_loadu_si256((const __m256i*)(input + 0 * 16));
        x1 = _mm256_loadu_si256((const __m256i*)(input + 1 * 16));
        x2 = _mm256_loadu_si256((const __m256i*)(input + 2 * 16));
        x3 = _mm256_loadu_si256((const __m256i*)(input + 3 * 16));

        v0 = _mm512_cvtepi16_epi32(x0);
        v1 = _mm512_cvtepi16_epi32(x1);
        v2 = _mm512_cvtepi16_epi32(x2);
        v3 = _mm512_cvtepi16_epi32(x3);

        _mm512_storeu_si512(output + 0, v0);
        _mm512_storeu_si512(output + 1, v1);
        _mm512_storeu_si512(output + 2, v2);
        _mm512_storeu_si512(output + 3, v3);

        input += stride;
        output += 4;
    }
}

static void av1_fdct64_new_avx512(const __m512i* input, __m512i* output, const int8_t cos_bit, const int32_t col_num,
                                  const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m512i  __rounding = _mm512_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 4;

    __m512i cospi_m32 = _mm512_set1_epi32(-cospi[32]);
    __m512i cospi_p32 = _mm512_set1_epi32(cospi[32]);
    __m512i cospi_m16 = _mm512_set1_epi32(-cospi[16]);
    __m512i cospi_p48 = _mm512_set1_epi32(cospi[48]);
    __m512i cospi_m48 = _mm512_set1_epi32(-cospi[48]);
    __m512i cospi_p16 = _mm512_set1_epi32(cospi[16]);
    __m512i cospi_m08 = _mm512_set1_epi32(-cospi[8]);
    __m512i cospi_p56 = _mm512_set1_epi32(cospi[56]);
    __m512i cospi_m56 = _mm512_set1_epi32(-cospi[56]);
    __m512i cospi_m40 = _mm512_set1_epi32(-cospi[40]);
    __m512i cospi_p24 = _mm512_set1_epi32(cospi[24]);
    __m512i cospi_m24 = _mm512_set1_epi32(-cospi[24]);
    __m512i cospi_p08 = _mm512_set1_epi32(cospi[8]);
    __m512i cospi_p40 = _mm512_set1_epi32(cospi[40]);
    __m512i cospi_p60 = _mm512_set1_epi32(cospi[60]);
    __m512i cospi_p04 = _mm512_set1_epi32(cospi[4]);
    __m512i cospi_p28 = _mm512_set1_epi32(cospi[28]);
    __m512i cospi_p36 = _mm512_set1_epi32(cospi[36]);
    __m512i cospi_p44 = _mm512_set1_epi32(cospi[44]);
    __m512i cospi_p20 = _mm512_set1_epi32(cospi[20]);
    __m512i cospi_p12 = _mm512_set1_epi32(cospi[12]);
    __m512i cospi_p52 = _mm512_set1_epi32(cospi[52]);
    __m512i cospi_m04 = _mm512_set1_epi32(-cospi[4]);
    __m512i cospi_m60 = _mm512_set1_epi32(-cospi[60]);
    __m512i cospi_m36 = _mm512_set1_epi32(-cospi[36]);
    __m512i cospi_m28 = _mm512_set1_epi32(-cospi[28]);
    __m512i cospi_m20 = _mm512_set1_epi32(-cospi[20]);
    __m512i cospi_m44 = _mm512_set1_epi32(-cospi[44]);
    __m512i cospi_m52 = _mm512_set1_epi32(-cospi[52]);
    __m512i cospi_m12 = _mm512_set1_epi32(-cospi[12]);
    __m512i cospi_p62 = _mm512_set1_epi32(cospi[62]);
    __m512i cospi_p02 = _mm512_set1_epi32(cospi[2]);
    __m512i cospi_p30 = _mm512_set1_epi32(cospi[30]);
    __m512i cospi_p34 = _mm512_set1_epi32(cospi[34]);
    __m512i cospi_p46 = _mm512_set1_epi32(cospi[46]);
    __m512i cospi_p18 = _mm512_set1_epi32(cospi[18]);
    __m512i cospi_p14 = _mm512_set1_epi32(cospi[14]);
    __m512i cospi_p50 = _mm512_set1_epi32(cospi[50]);
    __m512i cospi_p54 = _mm512_set1_epi32(cospi[54]);
    __m512i cospi_p10 = _mm512_set1_epi32(cospi[10]);
    __m512i cospi_p22 = _mm512_set1_epi32(cospi[22]);
    __m512i cospi_p42 = _mm512_set1_epi32(cospi[42]);
    __m512i cospi_p38 = _mm512_set1_epi32(cospi[38]);
    __m512i cospi_p26 = _mm512_set1_epi32(cospi[26]);
    __m512i cospi_p06 = _mm512_set1_epi32(cospi[6]);
    __m512i cospi_p58 = _mm512_set1_epi32(cospi[58]);
    __m512i cospi_p63 = _mm512_set1_epi32(cospi[63]);
    __m512i cospi_p01 = _mm512_set1_epi32(cospi[1]);
    __m512i cospi_p31 = _mm512_set1_epi32(cospi[31]);
    __m512i cospi_p33 = _mm512_set1_epi32(cospi[33]);
    __m512i cospi_p47 = _mm512_set1_epi32(cospi[47]);
    __m512i cospi_p17 = _mm512_set1_epi32(cospi[17]);
    __m512i cospi_p15 = _mm512_set1_epi32(cospi[15]);
    __m512i cospi_p49 = _mm512_set1_epi32(cospi[49]);
    __m512i cospi_p55 = _mm512_set1_epi32(cospi[55]);
    __m512i cospi_p09 = _mm512_set1_epi32(cospi[9]);
    __m512i cospi_p23 = _mm512_set1_epi32(cospi[23]);
    __m512i cospi_p41 = _mm512_set1_epi32(cospi[41]);
    __m512i cospi_p39 = _mm512_set1_epi32(cospi[39]);
    __m512i cospi_p25 = _mm512_set1_epi32(cospi[25]);
    __m512i cospi_p07 = _mm512_set1_epi32(cospi[7]);
    __m512i cospi_p57 = _mm512_set1_epi32(cospi[57]);
    __m512i cospi_p59 = _mm512_set1_epi32(cospi[59]);
    __m512i cospi_p05 = _mm512_set1_epi32(cospi[5]);
    __m512i cospi_p27 = _mm512_set1_epi32(cospi[27]);
    __m512i cospi_p37 = _mm512_set1_epi32(cospi[37]);
    __m512i cospi_p43 = _mm512_set1_epi32(cospi[43]);
    __m512i cospi_p21 = _mm512_set1_epi32(cospi[21]);
    __m512i cospi_p11 = _mm512_set1_epi32(cospi[11]);
    __m512i cospi_p53 = _mm512_set1_epi32(cospi[53]);
    __m512i cospi_p51 = _mm512_set1_epi32(cospi[51]);
    __m512i cospi_p13 = _mm512_set1_epi32(cospi[13]);
    __m512i cospi_p19 = _mm512_set1_epi32(cospi[19]);
    __m512i cospi_p45 = _mm512_set1_epi32(cospi[45]);
    __m512i cospi_p35 = _mm512_set1_epi32(cospi[35]);
    __m512i cospi_p29 = _mm512_set1_epi32(cospi[29]);
    __m512i cospi_p03 = _mm512_set1_epi32(cospi[3]);
    __m512i cospi_p61 = _mm512_set1_epi32(cospi[61]);

    for (int32_t col = 0; col < columns; col++) {
        const __m512i* in  = &input[col];
        __m512i*       out = &output[col];

        // stage 1
        __m512i x1[64];
        x1[0]  = _mm512_add_epi32(in[0 * stride], in[63 * stride]);
        x1[63] = _mm512_sub_epi32(in[0 * stride], in[63 * stride]);
        x1[1]  = _mm512_add_epi32(in[1 * stride], in[62 * stride]);
        x1[62] = _mm512_sub_epi32(in[1 * stride], in[62 * stride]);
        x1[2]  = _mm512_add_epi32(in[2 * stride], in[61 * stride]);
        x1[61] = _mm512_sub_epi32(in[2 * stride], in[61 * stride]);
        x1[3]  = _mm512_add_epi32(in[3 * stride], in[60 * stride]);
        x1[60] = _mm512_sub_epi32(in[3 * stride], in[60 * stride]);
        x1[4]  = _mm512_add_epi32(in[4 * stride], in[59 * stride]);
        x1[59] = _mm512_sub_epi32(in[4 * stride], in[59 * stride]);
        x1[5]  = _mm512_add_epi32(in[5 * stride], in[58 * stride]);
        x1[58] = _mm512_sub_epi32(in[5 * stride], in[58 * stride]);
        x1[6]  = _mm512_add_epi32(in[6 * stride], in[57 * stride]);
        x1[57] = _mm512_sub_epi32(in[6 * stride], in[57 * stride]);
        x1[7]  = _mm512_add_epi32(in[7 * stride], in[56 * stride]);
        x1[56] = _mm512_sub_epi32(in[7 * stride], in[56 * stride]);
        x1[8]  = _mm512_add_epi32(in[8 * stride], in[55 * stride]);
        x1[55] = _mm512_sub_epi32(in[8 * stride], in[55 * stride]);
        x1[9]  = _mm512_add_epi32(in[9 * stride], in[54 * stride]);
        x1[54] = _mm512_sub_epi32(in[9 * stride], in[54 * stride]);
        x1[10] = _mm512_add_epi32(in[10 * stride], in[53 * stride]);
        x1[53] = _mm512_sub_epi32(in[10 * stride], in[53 * stride]);
        x1[11] = _mm512_add_epi32(in[11 * stride], in[52 * stride]);
        x1[52] = _mm512_sub_epi32(in[11 * stride], in[52 * stride]);
        x1[12] = _mm512_add_epi32(in[12 * stride], in[51 * stride]);
        x1[51] = _mm512_sub_epi32(in[12 * stride], in[51 * stride]);
        x1[13] = _mm512_add_epi32(in[13 * stride], in[50 * stride]);
        x1[50] = _mm512_sub_epi32(in[13 * stride], in[50 * stride]);
        x1[14] = _mm512_add_epi32(in[14 * stride], in[49 * stride]);
        x1[49] = _mm512_sub_epi32(in[14 * stride], in[49 * stride]);
        x1[15] = _mm512_add_epi32(in[15 * stride], in[48 * stride]);
        x1[48] = _mm512_sub_epi32(in[15 * stride], in[48 * stride]);
        x1[16] = _mm512_add_epi32(in[16 * stride], in[47 * stride]);
        x1[47] = _mm512_sub_epi32(in[16 * stride], in[47 * stride]);
        x1[17] = _mm512_add_epi32(in[17 * stride], in[46 * stride]);
        x1[46] = _mm512_sub_epi32(in[17 * stride], in[46 * stride]);
        x1[18] = _mm512_add_epi32(in[18 * stride], in[45 * stride]);
        x1[45] = _mm512_sub_epi32(in[18 * stride], in[45 * stride]);
        x1[19] = _mm512_add_epi32(in[19 * stride], in[44 * stride]);
        x1[44] = _mm512_sub_epi32(in[19 * stride], in[44 * stride]);
        x1[20] = _mm512_add_epi32(in[20 * stride], in[43 * stride]);
        x1[43] = _mm512_sub_epi32(in[20 * stride], in[43 * stride]);
        x1[21] = _mm512_add_epi32(in[21 * stride], in[42 * stride]);
        x1[42] = _mm512_sub_epi32(in[21 * stride], in[42 * stride]);
        x1[22] = _mm512_add_epi32(in[22 * stride], in[41 * stride]);
        x1[41] = _mm512_sub_epi32(in[22 * stride], in[41 * stride]);
        x1[23] = _mm512_add_epi32(in[23 * stride], in[40 * stride]);
        x1[40] = _mm512_sub_epi32(in[23 * stride], in[40 * stride]);
        x1[24] = _mm512_add_epi32(in[24 * stride], in[39 * stride]);
        x1[39] = _mm512_sub_epi32(in[24 * stride], in[39 * stride]);
        x1[25] = _mm512_add_epi32(in[25 * stride], in[38 * stride]);
        x1[38] = _mm512_sub_epi32(in[25 * stride], in[38 * stride]);
        x1[26] = _mm512_add_epi32(in[26 * stride], in[37 * stride]);
        x1[37] = _mm512_sub_epi32(in[26 * stride], in[37 * stride]);
        x1[27] = _mm512_add_epi32(in[27 * stride], in[36 * stride]);
        x1[36] = _mm512_sub_epi32(in[27 * stride], in[36 * stride]);
        x1[28] = _mm512_add_epi32(in[28 * stride], in[35 * stride]);
        x1[35] = _mm512_sub_epi32(in[28 * stride], in[35 * stride]);
        x1[29] = _mm512_add_epi32(in[29 * stride], in[34 * stride]);
        x1[34] = _mm512_sub_epi32(in[29 * stride], in[34 * stride]);
        x1[30] = _mm512_add_epi32(in[30 * stride], in[33 * stride]);
        x1[33] = _mm512_sub_epi32(in[30 * stride], in[33 * stride]);
        x1[31] = _mm512_add_epi32(in[31 * stride], in[32 * stride]);
        x1[32] = _mm512_sub_epi32(in[31 * stride], in[32 * stride]);

        // stage 2
        __m512i x2[54];
        x2[0]  = _mm512_add_epi32(x1[0], x1[31]);
        x2[31] = _mm512_sub_epi32(x1[0], x1[31]);
        x2[1]  = _mm512_add_epi32(x1[1], x1[30]);
        x2[30] = _mm512_sub_epi32(x1[1], x1[30]);
        x2[2]  = _mm512_add_epi32(x1[2], x1[29]);
        x2[29] = _mm512_sub_epi32(x1[2], x1[29]);
        x2[3]  = _mm512_add_epi32(x1[3], x1[28]);
        x2[28] = _mm512_sub_epi32(x1[3], x1[28]);
        x2[4]  = _mm512_add_epi32(x1[4], x1[27]);
        x2[27] = _mm512_sub_epi32(x1[4], x1[27]);
        x2[5]  = _mm512_add_epi32(x1[5], x1[26]);
        x2[26] = _mm512_sub_epi32(x1[5], x1[26]);
        x2[6]  = _mm512_add_epi32(x1[6], x1[25]);
        x2[25] = _mm512_sub_epi32(x1[6], x1[25]);
        x2[7]  = _mm512_add_epi32(x1[7], x1[24]);
        x2[24] = _mm512_sub_epi32(x1[7], x1[24]);
        x2[8]  = _mm512_add_epi32(x1[8], x1[23]);
        x2[23] = _mm512_sub_epi32(x1[8], x1[23]);
        x2[9]  = _mm512_add_epi32(x1[9], x1[22]);
        x2[22] = _mm512_sub_epi32(x1[9], x1[22]);
        x2[10] = _mm512_add_epi32(x1[10], x1[21]);
        x2[21] = _mm512_sub_epi32(x1[10], x1[21]);
        x2[11] = _mm512_add_epi32(x1[11], x1[20]);
        x2[20] = _mm512_sub_epi32(x1[11], x1[20]);
        x2[12] = _mm512_add_epi32(x1[12], x1[19]);
        x2[19] = _mm512_sub_epi32(x1[12], x1[19]);
        x2[13] = _mm512_add_epi32(x1[13], x1[18]);
        x2[18] = _mm512_sub_epi32(x1[13], x1[18]);
        x2[14] = _mm512_add_epi32(x1[14], x1[17]);
        x2[17] = _mm512_sub_epi32(x1[14], x1[17]);
        x2[15] = _mm512_add_epi32(x1[15], x1[16]);
        x2[16] = _mm512_sub_epi32(x1[15], x1[16]);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[40], x1[55], x2[32], x2[47], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[41], x1[54], x2[33], x2[46], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[42], x1[53], x2[34], x2[45], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[43], x1[52], x2[35], x2[44], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[44], x1[51], x2[36], x2[43], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[45], x1[50], x2[37], x2[42], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[46], x1[49], x2[38], x2[41], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[47], x1[48], x2[39], x2[40], __rounding, cos_bit);

        // stage 3
        __m512i x3[56];
        x3[0]  = _mm512_add_epi32(x2[0], x2[15]);
        x3[15] = _mm512_sub_epi32(x2[0], x2[15]);
        x3[1]  = _mm512_add_epi32(x2[1], x2[14]);
        x3[14] = _mm512_sub_epi32(x2[1], x2[14]);
        x3[2]  = _mm512_add_epi32(x2[2], x2[13]);
        x3[13] = _mm512_sub_epi32(x2[2], x2[13]);
        x3[3]  = _mm512_add_epi32(x2[3], x2[12]);
        x3[12] = _mm512_sub_epi32(x2[3], x2[12]);
        x3[4]  = _mm512_add_epi32(x2[4], x2[11]);
        x3[11] = _mm512_sub_epi32(x2[4], x2[11]);
        x3[5]  = _mm512_add_epi32(x2[5], x2[10]);
        x3[10] = _mm512_sub_epi32(x2[5], x2[10]);
        x3[6]  = _mm512_add_epi32(x2[6], x2[9]);
        x3[9]  = _mm512_sub_epi32(x2[6], x2[9]);
        x3[7]  = _mm512_add_epi32(x2[7], x2[8]);
        x3[8]  = _mm512_sub_epi32(x2[7], x2[8]);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x2[20], x2[27], x3[16], x3[23], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x2[21], x2[26], x3[17], x3[22], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x2[22], x2[25], x3[18], x3[21], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x2[23], x2[24], x3[19], x3[20], __rounding, cos_bit);
        x3[32] = _mm512_add_epi32(x1[32], x2[39]);
        x3[47] = _mm512_sub_epi32(x1[32], x2[39]);
        x3[33] = _mm512_add_epi32(x1[33], x2[38]);
        x3[46] = _mm512_sub_epi32(x1[33], x2[38]);
        x3[34] = _mm512_add_epi32(x1[34], x2[37]);
        x3[45] = _mm512_sub_epi32(x1[34], x2[37]);
        x3[35] = _mm512_add_epi32(x1[35], x2[36]);
        x3[44] = _mm512_sub_epi32(x1[35], x2[36]);
        x3[36] = _mm512_add_epi32(x1[36], x2[35]);
        x3[43] = _mm512_sub_epi32(x1[36], x2[35]);
        x3[37] = _mm512_add_epi32(x1[37], x2[34]);
        x3[42] = _mm512_sub_epi32(x1[37], x2[34]);
        x3[38] = _mm512_add_epi32(x1[38], x2[33]);
        x3[41] = _mm512_sub_epi32(x1[38], x2[33]);
        x3[39] = _mm512_add_epi32(x1[39], x2[32]);
        x3[40] = _mm512_sub_epi32(x1[39], x2[32]);
        x3[48] = _mm512_sub_epi32(x1[63], x2[40]);
        x3[24] = _mm512_add_epi32(x1[63], x2[40]);
        x3[49] = _mm512_sub_epi32(x1[62], x2[41]);
        x3[25] = _mm512_add_epi32(x1[62], x2[41]);
        x3[50] = _mm512_sub_epi32(x1[61], x2[42]);
        x3[26] = _mm512_add_epi32(x1[61], x2[42]);
        x3[51] = _mm512_sub_epi32(x1[60], x2[43]);
        x3[27] = _mm512_add_epi32(x1[60], x2[43]);
        x3[52] = _mm512_sub_epi32(x1[59], x2[44]);
        x3[28] = _mm512_add_epi32(x1[59], x2[44]);
        x3[53] = _mm512_sub_epi32(x1[58], x2[45]);
        x3[29] = _mm512_add_epi32(x1[58], x2[45]);
        x3[54] = _mm512_sub_epi32(x1[57], x2[46]);
        x3[30] = _mm512_add_epi32(x1[57], x2[46]);
        x3[55] = _mm512_sub_epi32(x1[56], x2[47]);
        x3[31] = _mm512_add_epi32(x1[56], x2[47]);

        // stage 4
        //__m512i x4[44]; replace with x1
        x1[0] = _mm512_add_epi32(x3[0], x3[7]);
        x1[7] = _mm512_sub_epi32(x3[0], x3[7]);
        x1[1] = _mm512_add_epi32(x3[1], x3[6]);
        x1[6] = _mm512_sub_epi32(x3[1], x3[6]);
        x1[2] = _mm512_add_epi32(x3[2], x3[5]);
        x1[5] = _mm512_sub_epi32(x3[2], x3[5]);
        x1[3] = _mm512_add_epi32(x3[3], x3[4]);
        x1[4] = _mm512_sub_epi32(x3[3], x3[4]);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x3[10], x3[13], x1[8], x1[11], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x3[11], x3[12], x1[9], x1[10], __rounding, cos_bit);
        x1[12] = _mm512_add_epi32(x2[16], x3[19]);
        x1[19] = _mm512_sub_epi32(x2[16], x3[19]);
        x1[13] = _mm512_add_epi32(x2[17], x3[18]);
        x1[18] = _mm512_sub_epi32(x2[17], x3[18]);
        x1[14] = _mm512_add_epi32(x2[18], x3[17]);
        x1[17] = _mm512_sub_epi32(x2[18], x3[17]);
        x1[15] = _mm512_add_epi32(x2[19], x3[16]);
        x1[16] = _mm512_sub_epi32(x2[19], x3[16]);
        x1[20] = _mm512_sub_epi32(x2[31], x3[20]);
        x1[27] = _mm512_add_epi32(x2[31], x3[20]);
        x1[21] = _mm512_sub_epi32(x2[30], x3[21]);
        x1[26] = _mm512_add_epi32(x2[30], x3[21]);
        x1[22] = _mm512_sub_epi32(x2[29], x3[22]);
        x1[25] = _mm512_add_epi32(x2[29], x3[22]);
        x1[23] = _mm512_sub_epi32(x2[28], x3[23]);
        x1[24] = _mm512_add_epi32(x2[28], x3[23]);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x3[36], x3[28], x1[28], x1[43], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x3[37], x3[29], x1[29], x1[42], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x3[38], x3[30], x1[30], x1[41], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x3[39], x3[31], x1[31], x1[40], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x3[40], x3[55], x1[32], x1[39], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x3[41], x3[54], x1[33], x1[38], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x3[42], x3[53], x1[34], x1[37], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x3[43], x3[52], x1[35], x1[36], __rounding, cos_bit);

        // stage 5
        //__m512i x5[54]; replace with x2
        x2[0] = _mm512_add_epi32(x1[0], x1[3]);
        x2[3] = _mm512_sub_epi32(x1[0], x1[3]);
        x2[1] = _mm512_add_epi32(x1[1], x1[2]);
        x2[2] = _mm512_sub_epi32(x1[1], x1[2]);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[5], x1[6], x2[4], x2[5], __rounding, cos_bit);
        x2[6]  = _mm512_add_epi32(x3[8], x1[9]);
        x2[9]  = _mm512_sub_epi32(x3[8], x1[9]);
        x2[7]  = _mm512_add_epi32(x3[9], x1[8]);
        x2[8]  = _mm512_sub_epi32(x3[9], x1[8]);
        x2[10] = _mm512_sub_epi32(x3[15], x1[10]);
        x2[13] = _mm512_add_epi32(x3[15], x1[10]);
        x2[11] = _mm512_sub_epi32(x3[14], x1[11]);
        x2[12] = _mm512_add_epi32(x3[14], x1[11]);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x1[14], x1[25], x2[14], x2[21], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x1[15], x1[24], x2[15], x2[20], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x1[16], x1[23], x2[16], x2[19], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x1[17], x1[22], x2[17], x2[18], __rounding, cos_bit);
        x2[22] = _mm512_add_epi32(x3[32], x1[31]);
        x2[29] = _mm512_sub_epi32(x3[32], x1[31]);
        x2[23] = _mm512_add_epi32(x3[33], x1[30]);
        x2[28] = _mm512_sub_epi32(x3[33], x1[30]);
        x2[24] = _mm512_add_epi32(x3[34], x1[29]);
        x2[27] = _mm512_sub_epi32(x3[34], x1[29]);
        x2[25] = _mm512_add_epi32(x3[35], x1[28]);
        x2[26] = _mm512_sub_epi32(x3[35], x1[28]);
        x2[30] = _mm512_sub_epi32(x3[47], x1[32]);
        x2[37] = _mm512_add_epi32(x3[47], x1[32]);
        x2[31] = _mm512_sub_epi32(x3[46], x1[33]);
        x2[36] = _mm512_add_epi32(x3[46], x1[33]);
        x2[32] = _mm512_sub_epi32(x3[45], x1[34]);
        x2[35] = _mm512_add_epi32(x3[45], x1[34]);
        x2[33] = _mm512_sub_epi32(x3[44], x1[35]);
        x2[34] = _mm512_add_epi32(x3[44], x1[35]);
        x2[38] = _mm512_add_epi32(x3[48], x1[39]);
        x2[45] = _mm512_sub_epi32(x3[48], x1[39]);
        x2[39] = _mm512_add_epi32(x3[49], x1[38]);
        x2[44] = _mm512_sub_epi32(x3[49], x1[38]);
        x2[40] = _mm512_add_epi32(x3[50], x1[37]);
        x2[43] = _mm512_sub_epi32(x3[50], x1[37]);
        x2[41] = _mm512_add_epi32(x3[51], x1[36]);
        x2[42] = _mm512_sub_epi32(x3[51], x1[36]);
        x2[46] = _mm512_sub_epi32(x3[24], x1[40]);
        x2[53] = _mm512_add_epi32(x3[24], x1[40]);
        x2[47] = _mm512_sub_epi32(x3[25], x1[41]);
        x2[52] = _mm512_add_epi32(x3[25], x1[41]);
        x2[48] = _mm512_sub_epi32(x3[26], x1[42]);
        x2[51] = _mm512_add_epi32(x3[26], x1[42]);
        x2[49] = _mm512_sub_epi32(x3[27], x1[43]);
        x2[50] = _mm512_add_epi32(x3[27], x1[43]);

        // stage 6
        //__m512i x6[40]; replace with x3
        btf_32_type0_avx512_new(
            cospi_p32, cospi_p32, x2[0], x2[1], out[0 * stride], out[32 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p48, cospi_p16, x2[2], x2[3], out[16 * stride], out[48 * stride], __rounding, cos_bit);
        x3[0] = _mm512_add_epi32(x1[4], x2[4]);
        x3[1] = _mm512_sub_epi32(x1[4], x2[4]);
        x3[2] = _mm512_sub_epi32(x1[7], x2[5]);
        x3[3] = _mm512_add_epi32(x1[7], x2[5]);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x2[7], x2[12], x3[4], x3[7], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x2[8], x2[11], x3[5], x3[6], __rounding, cos_bit);
        x3[8]  = _mm512_add_epi32(x1[12], x2[15]);
        x3[11] = _mm512_sub_epi32(x1[12], x2[15]);
        x3[9]  = _mm512_add_epi32(x1[13], x2[14]);
        x3[10] = _mm512_sub_epi32(x1[13], x2[14]);
        x3[12] = _mm512_sub_epi32(x1[19], x2[16]);
        x3[15] = _mm512_add_epi32(x1[19], x2[16]);
        x3[13] = _mm512_sub_epi32(x1[18], x2[17]);
        x3[14] = _mm512_add_epi32(x1[18], x2[17]);
        x3[16] = _mm512_add_epi32(x1[20], x2[19]);
        x3[19] = _mm512_sub_epi32(x1[20], x2[19]);
        x3[17] = _mm512_add_epi32(x1[21], x2[18]);
        x3[18] = _mm512_sub_epi32(x1[21], x2[18]);
        x3[20] = _mm512_sub_epi32(x1[27], x2[20]);
        x3[23] = _mm512_add_epi32(x1[27], x2[20]);
        x3[21] = _mm512_sub_epi32(x1[26], x2[21]);
        x3[22] = _mm512_add_epi32(x1[26], x2[21]);
        btf_32_type0_avx512_new(cospi_m08, cospi_p56, x2[24], x2[51], x3[24], x3[39], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m08, cospi_p56, x2[25], x2[50], x3[25], x3[38], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m56, cospi_m08, x2[26], x2[49], x3[26], x3[37], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m56, cospi_m08, x2[27], x2[48], x3[27], x3[36], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m40, cospi_p24, x2[32], x2[43], x3[28], x3[35], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m40, cospi_p24, x2[33], x2[42], x3[29], x3[34], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m24, cospi_m40, x2[34], x2[41], x3[30], x3[33], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m24, cospi_m40, x2[35], x2[40], x3[31], x3[32], __rounding, cos_bit);

        // stage 7
        //__m512i x7[48]; replace with x1
        btf_32_type1_avx512_new(
            cospi_p56, cospi_p08, x3[0], x3[3], out[8 * stride], out[56 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p24, cospi_p40, x3[1], x3[2], out[40 * stride], out[24 * stride], __rounding, cos_bit);
        x1[0] = _mm512_add_epi32(x2[6], x3[4]);
        x1[1] = _mm512_sub_epi32(x2[6], x3[4]);
        x1[2] = _mm512_sub_epi32(x2[9], x3[5]);
        x1[3] = _mm512_add_epi32(x2[9], x3[5]);
        x1[4] = _mm512_add_epi32(x2[10], x3[6]);
        x1[5] = _mm512_sub_epi32(x2[10], x3[6]);
        x1[6] = _mm512_sub_epi32(x2[13], x3[7]);
        x1[7] = _mm512_add_epi32(x2[13], x3[7]);
        btf_32_type0_avx512_new(cospi_m08, cospi_p56, x3[9], x3[22], x1[8], x1[15], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m56, cospi_m08, x3[10], x3[21], x1[9], x1[14], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m40, cospi_p24, x3[13], x3[18], x1[10], x1[13], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m24, cospi_m40, x3[14], x3[17], x1[11], x1[12], __rounding, cos_bit);
        x1[16] = _mm512_add_epi32(x2[22], x3[25]);
        x1[17] = _mm512_sub_epi32(x2[22], x3[25]);
        x1[19] = _mm512_add_epi32(x2[23], x3[24]);
        x1[20] = _mm512_sub_epi32(x2[23], x3[24]);
        x1[18] = _mm512_sub_epi32(x2[29], x3[26]);
        x1[21] = _mm512_add_epi32(x2[29], x3[26]);
        x1[22] = _mm512_sub_epi32(x2[28], x3[27]);
        x1[23] = _mm512_add_epi32(x2[28], x3[27]);
        x1[24] = _mm512_add_epi32(x2[30], x3[29]);
        x1[25] = _mm512_sub_epi32(x2[30], x3[29]);
        x1[26] = _mm512_add_epi32(x2[31], x3[28]);
        x1[27] = _mm512_sub_epi32(x2[31], x3[28]);
        x1[28] = _mm512_sub_epi32(x2[37], x3[30]);
        x1[29] = _mm512_add_epi32(x2[37], x3[30]);
        x1[30] = _mm512_sub_epi32(x2[36], x3[31]);
        x1[31] = _mm512_add_epi32(x2[36], x3[31]);
        x1[32] = _mm512_add_epi32(x2[38], x3[33]);
        x1[33] = _mm512_sub_epi32(x2[38], x3[33]);
        x1[34] = _mm512_add_epi32(x2[39], x3[32]);
        x1[35] = _mm512_sub_epi32(x2[39], x3[32]);
        x1[36] = _mm512_sub_epi32(x2[45], x3[34]);
        x1[37] = _mm512_add_epi32(x2[45], x3[34]);
        x1[38] = _mm512_sub_epi32(x2[44], x3[35]);
        x1[39] = _mm512_add_epi32(x2[44], x3[35]);
        x1[40] = _mm512_add_epi32(x2[46], x3[37]);
        x1[41] = _mm512_sub_epi32(x2[46], x3[37]);
        x1[42] = _mm512_add_epi32(x2[47], x3[36]);
        x1[43] = _mm512_sub_epi32(x2[47], x3[36]);
        x1[44] = _mm512_sub_epi32(x2[53], x3[38]);
        x1[45] = _mm512_add_epi32(x2[53], x3[38]);
        x1[46] = _mm512_sub_epi32(x2[52], x3[39]);
        x1[47] = _mm512_add_epi32(x2[52], x3[39]);

        // stage 8
        //__m512i x8[32]; replace with x2
        btf_32_type1_avx512_new(
            cospi_p60, cospi_p04, x1[0], x1[7], out[4 * stride], out[60 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p28, cospi_p36, x1[1], x1[6], out[36 * stride], out[28 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p44, cospi_p20, x1[2], x1[5], out[20 * stride], out[44 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p12, cospi_p52, x1[3], x1[4], out[52 * stride], out[12 * stride], __rounding, cos_bit);
        x2[0]  = _mm512_add_epi32(x3[8], x1[8]);
        x2[1]  = _mm512_sub_epi32(x3[8], x1[8]);
        x2[2]  = _mm512_sub_epi32(x3[11], x1[9]);
        x2[3]  = _mm512_add_epi32(x3[11], x1[9]);
        x2[4]  = _mm512_add_epi32(x3[12], x1[10]);
        x2[5]  = _mm512_sub_epi32(x3[12], x1[10]);
        x2[6]  = _mm512_sub_epi32(x3[15], x1[11]);
        x2[7]  = _mm512_add_epi32(x3[15], x1[11]);
        x2[8]  = _mm512_add_epi32(x3[16], x1[12]);
        x2[9]  = _mm512_sub_epi32(x3[16], x1[12]);
        x2[10] = _mm512_sub_epi32(x3[19], x1[13]);
        x2[11] = _mm512_add_epi32(x3[19], x1[13]);
        x2[12] = _mm512_add_epi32(x3[20], x1[14]);
        x2[13] = _mm512_sub_epi32(x3[20], x1[14]);
        x2[14] = _mm512_sub_epi32(x3[23], x1[15]);
        x2[15] = _mm512_add_epi32(x3[23], x1[15]);
        btf_32_type0_avx512_new(cospi_m04, cospi_p60, x1[19], x1[47], x2[16], x2[31], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m60, cospi_m04, x1[20], x1[46], x2[17], x2[30], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m36, cospi_p28, x1[22], x1[43], x2[18], x2[29], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m28, cospi_m36, x1[23], x1[42], x2[19], x2[28], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m20, cospi_p44, x1[26], x1[39], x2[20], x2[27], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m44, cospi_m20, x1[27], x1[38], x2[21], x2[26], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m52, cospi_p12, x1[30], x1[35], x2[22], x2[25], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m12, cospi_m52, x1[31], x1[34], x2[23], x2[24], __rounding, cos_bit);

        // stage 9
        //__m512i x9[32]; replace with x3
        btf_32_type1_avx512_new(
            cospi_p62, cospi_p02, x2[0], x2[15], out[2 * stride], out[62 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p30, cospi_p34, x2[1], x2[14], out[34 * stride], out[30 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p46, cospi_p18, x2[2], x2[13], out[18 * stride], out[46 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p14, cospi_p50, x2[3], x2[12], out[50 * stride], out[14 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p54, cospi_p10, x2[4], x2[11], out[10 * stride], out[54 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p22, cospi_p42, x2[5], x2[10], out[42 * stride], out[22 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p38, cospi_p26, x2[6], x2[9], out[26 * stride], out[38 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p06, cospi_p58, x2[7], x2[8], out[58 * stride], out[6 * stride], __rounding, cos_bit);
        x3[0]  = _mm512_add_epi32(x1[16], x2[16]);
        x3[1]  = _mm512_sub_epi32(x1[16], x2[16]);
        x3[2]  = _mm512_sub_epi32(x1[17], x2[17]);
        x3[3]  = _mm512_add_epi32(x1[17], x2[17]);
        x3[4]  = _mm512_add_epi32(x1[18], x2[18]);
        x3[5]  = _mm512_sub_epi32(x1[18], x2[18]);
        x3[6]  = _mm512_sub_epi32(x1[21], x2[19]);
        x3[7]  = _mm512_add_epi32(x1[21], x2[19]);
        x3[8]  = _mm512_add_epi32(x1[24], x2[20]);
        x3[9]  = _mm512_sub_epi32(x1[24], x2[20]);
        x3[10] = _mm512_sub_epi32(x1[25], x2[21]);
        x3[11] = _mm512_add_epi32(x1[25], x2[21]);
        x3[12] = _mm512_add_epi32(x1[28], x2[22]);
        x3[13] = _mm512_sub_epi32(x1[28], x2[22]);
        x3[14] = _mm512_sub_epi32(x1[29], x2[23]);
        x3[15] = _mm512_add_epi32(x1[29], x2[23]);
        x3[16] = _mm512_add_epi32(x1[32], x2[24]);
        x3[17] = _mm512_sub_epi32(x1[32], x2[24]);
        x3[18] = _mm512_sub_epi32(x1[33], x2[25]);
        x3[19] = _mm512_add_epi32(x1[33], x2[25]);
        x3[20] = _mm512_add_epi32(x1[36], x2[26]);
        x3[21] = _mm512_sub_epi32(x1[36], x2[26]);
        x3[22] = _mm512_sub_epi32(x1[37], x2[27]);
        x3[23] = _mm512_add_epi32(x1[37], x2[27]);
        x3[24] = _mm512_add_epi32(x1[40], x2[28]);
        x3[25] = _mm512_sub_epi32(x1[40], x2[28]);
        x3[26] = _mm512_sub_epi32(x1[41], x2[29]);
        x3[27] = _mm512_add_epi32(x1[41], x2[29]);
        x3[28] = _mm512_add_epi32(x1[44], x2[30]);
        x3[29] = _mm512_sub_epi32(x1[44], x2[30]);
        x3[30] = _mm512_sub_epi32(x1[45], x2[31]);
        x3[31] = _mm512_add_epi32(x1[45], x2[31]);

        // stage 10
        btf_32_type1_avx512_new(
            cospi_p63, cospi_p01, x3[0], x3[31], out[1 * stride], out[63 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p31, cospi_p33, x3[1], x3[30], out[33 * stride], out[31 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p47, cospi_p17, x3[2], x3[29], out[17 * stride], out[47 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p15, cospi_p49, x3[3], x3[28], out[49 * stride], out[15 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p55, cospi_p09, x3[4], x3[27], out[9 * stride], out[55 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p23, cospi_p41, x3[5], x3[26], out[41 * stride], out[23 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p39, cospi_p25, x3[6], x3[25], out[25 * stride], out[39 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p07, cospi_p57, x3[7], x3[24], out[57 * stride], out[7 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p59, cospi_p05, x3[8], x3[23], out[5 * stride], out[59 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p27, cospi_p37, x3[9], x3[22], out[37 * stride], out[27 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p43, cospi_p21, x3[10], x3[21], out[21 * stride], out[43 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p11, cospi_p53, x3[11], x3[20], out[53 * stride], out[11 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p51, cospi_p13, x3[12], x3[19], out[13 * stride], out[51 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p19, cospi_p45, x3[13], x3[18], out[45 * stride], out[19 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p35, cospi_p29, x3[14], x3[17], out[29 * stride], out[35 * stride], __rounding, cos_bit);
        btf_32_type1_avx512_new(
            cospi_p03, cospi_p61, x3[15], x3[16], out[61 * stride], out[3 * stride], __rounding, cos_bit);
    }
}

static INLINE void fdct64x64_avx512(const __m512i* input, __m512i* output, const int8_t cos_bit) {
    const int32_t txfm_size   = 64;
    const int32_t num_per_512 = 16;
    int32_t       col_num     = txfm_size / num_per_512;
    av1_fdct64_new_avx512(input, output, cos_bit, txfm_size, col_num);
}

void svt_av1_fwd_txfm2d_64x64_avx512(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    __m512i       in[256];
    __m512i*      out     = (__m512i*)output;
    const int32_t txw_idx = tx_size_wide_log2[TX_64X64] - tx_size_wide_log2[0];
    const int32_t txh_idx = tx_size_high_log2[TX_64X64] - tx_size_high_log2[0];
    const int8_t* shift   = fwd_txfm_shift_ls[TX_64X64];

    switch (tx_type) {
    case IDTX:
        load_buffer_64x64_avx512(input, stride, out);
        fidtx64x64_avx512(out, in);
        av1_round_shift_array_avx512(in, out, 256, -shift[1]);
        transpose_16nx16n_avx512(64, out, in);

        /*row wise transform*/
        fidtx64x64_avx512(in, out);
        av1_round_shift_array_avx512(out, in, 256, -shift[2]);
        transpose_16nx16n_avx512(64, in, out);
        break;
    case DCT_DCT:
        load_buffer_64x64_avx512(input, stride, out);
        fdct64x64_avx512(out, in, fwd_cos_bit_col[txw_idx][txh_idx]);
        av1_round_shift_array_avx512(in, out, 256, -shift[1]);
        transpose_16nx16n_avx512(64, out, in);

        /*row wise transform*/
        fdct64x64_avx512(in, out, fwd_cos_bit_row[txw_idx][txh_idx]);
        av1_round_shift_array_avx512(out, in, 256, -shift[2]);
        transpose_16nx16n_avx512(64, in, out);
        break;
    default:
        assert(0);
    }
}

static INLINE void load_buffer_16_avx512(const int16_t* input, __m512i* in, int32_t stride, int32_t flipud,
                                         int32_t fliplr, const int8_t shift) {
    (void)flipud;
    (void)fliplr;
    __m256i temp;
    uint8_t ushift = (uint8_t)shift;
    temp           = _mm256_loadu_si256((const __m256i*)(input + 0 * stride));

    in[0] = _mm512_cvtepi16_epi32(temp);
    in[0] = _mm512_slli_epi32(in[0], ushift);
}

static INLINE void load_buffer_32_avx512(const int16_t* input, __m512i* in, int32_t stride, int32_t flipud,
                                         int32_t fliplr, const int8_t shift) {
    (void)flipud;
    (void)fliplr;
    __m256i temp[2];
    uint8_t ushift = (uint8_t)shift;
    temp[0]        = _mm256_loadu_si256((const __m256i*)(input + 0 * stride));
    temp[1]        = _mm256_loadu_si256((const __m256i*)(input + 1 * stride));

    in[0] = _mm512_cvtepi16_epi32(temp[0]);
    in[1] = _mm512_cvtepi16_epi32(temp[1]);

    in[0] = _mm512_slli_epi32(in[0], ushift);
    in[1] = _mm512_slli_epi32(in[1], ushift);
}

static INLINE void load_buffer_32x16n(const int16_t* input, __m512i* out, int32_t stride, int32_t flipud,
                                      int32_t fliplr, const int8_t shift, const int32_t height) {
    const int16_t* in     = input;
    __m512i*       output = out;
    for (int32_t col = 0; col < height; col++) {
        in     = input + col * stride;
        output = out + col * 2;
        load_buffer_32_avx512(in, output, 16, flipud, fliplr, shift);
    }
}

static INLINE void av1_round_shift_rect_array_32_avx512(__m512i* input, __m512i* output, const int32_t size,
                                                        const int8_t bit, const int32_t val) {
    const __m512i sqrt2  = _mm512_set1_epi32(val);
    const __m512i round2 = _mm512_set1_epi32(1 << (12 - 1));
    int32_t       i;
    if (bit > 0) {
        const __m512i round1 = _mm512_set1_epi32(1 << (bit - 1));
        __m512i       r0, r1, r2, r3;
        for (i = 0; i < size; i++) {
            r0        = _mm512_add_epi32(input[i], round1);
            r1        = _mm512_srai_epi32(r0, (uint8_t)bit);
            r2        = _mm512_mullo_epi32(sqrt2, r1);
            r3        = _mm512_add_epi32(r2, round2);
            output[i] = _mm512_srai_epi32(r3, (uint8_t)12);
        }
    } else {
        __m512i r0, r1, r2;
        for (i = 0; i < size; i++) {
            r0        = _mm512_slli_epi32(input[i], (uint8_t)-bit);
            r1        = _mm512_mullo_epi32(sqrt2, r0);
            r2        = _mm512_add_epi32(r1, round2);
            output[i] = _mm512_srai_epi32(r2, (uint8_t)12);
        }
    }
}

void svt_av1_fwd_txfm2d_32x64_avx512(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m512i       in[128];
    __m512i*      outcoef512    = (__m512i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_32X64];
    const int32_t txw_idx       = get_txw_idx(TX_32X64);
    const int32_t txh_idx       = get_txh_idx(TX_32X64);
    const int32_t txfm_size_col = tx_size_wide[TX_32X64];
    const int32_t txfm_size_row = tx_size_high[TX_32X64];
    const int8_t  bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    const int8_t  bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 4;
    const int32_t num_col       = txfm_size_col >> 4;

    // column transform
    load_buffer_32x16n(input, in, stride, 0, 0, shift[0], txfm_size_row);
    av1_fdct64_new_avx512(in, in, bitcol, txfm_size_col, num_col);

    for (int32_t i = 0; i < 8; i++) {
        col_txfm_16x16_rounding_avx512((in + i * 16), -shift[1]);
    }
    transpose_16nx16m_avx512(in, outcoef512, txfm_size_col, txfm_size_row);

    // row transform
    av1_fdct32_new_avx512(outcoef512, in, bitrow, txfm_size_row, num_row);
    transpose_16nx16m_avx512(in, outcoef512, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_32_avx512(outcoef512, outcoef512, 128, -shift[2], 5793);
    (void)bd;
}

void svt_av1_fwd_txfm2d_64x32_avx512(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m512i       in[128];
    __m512i*      outcoef512    = (__m512i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X32];
    const int32_t txw_idx       = get_txw_idx(TX_64X32);
    const int32_t txh_idx       = get_txh_idx(TX_64X32);
    const int32_t txfm_size_col = tx_size_wide[TX_64X32];
    const int32_t txfm_size_row = tx_size_high[TX_64X32];
    const int8_t  bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    const int8_t  bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 4;
    const int32_t num_col       = txfm_size_col >> 4;

    // column transform
    for (int32_t i = 0; i < 32; i++) {
        load_buffer_32_avx512(input + 0 + i * stride, in + 0 + i * 4, 16, 0, 0, shift[0]);
        load_buffer_32_avx512(input + 32 + i * stride, in + 2 + i * 4, 16, 0, 0, shift[0]);
    }

    av1_fdct32_new_avx512(in, in, bitcol, txfm_size_col, num_col);

    for (int32_t i = 0; i < 8; i++) {
        col_txfm_16x16_rounding_avx512((in + i * 16), -shift[1]);
    }
    transpose_16nx16m_avx512(in, outcoef512, txfm_size_col, txfm_size_row);

    // row transform
    av1_fdct64_new_avx512(outcoef512, in, bitrow, txfm_size_row, num_row);
    transpose_16nx16m_avx512(in, outcoef512, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_32_avx512(outcoef512, outcoef512, 128, -shift[2], 5793);
    (void)bd;
}

void svt_av1_fwd_txfm2d_16x64_avx512(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m512i       in[64];
    __m512i*      outcoeff512   = (__m512i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_16X64];
    const int32_t txw_idx       = get_txw_idx(TX_16X64);
    const int32_t txh_idx       = get_txh_idx(TX_16X64);
    const int32_t txfm_size_col = tx_size_wide[TX_16X64];
    const int32_t txfm_size_row = tx_size_high[TX_16X64];
    const int8_t  bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    const int8_t  bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 4;
    const int32_t num_col       = txfm_size_col >> 4;

    // column tranform
    for (int32_t i = 0; i < txfm_size_row; i++) {
        load_buffer_16_avx512(input + i * stride, in + i, 16, 0, 0, shift[0]);
    }

    av1_fdct64_new_avx512(in, outcoeff512, bitcol, txfm_size_col, num_col);

    col_txfm_16x16_rounding_avx512(outcoeff512, -shift[1]);
    col_txfm_16x16_rounding_avx512(outcoeff512 + 16, -shift[1]);
    col_txfm_16x16_rounding_avx512(outcoeff512 + 32, -shift[1]);
    col_txfm_16x16_rounding_avx512(outcoeff512 + 48, -shift[1]);
    transpose_16nx16m_avx512(outcoeff512, in, txfm_size_col, txfm_size_row);
    // row tranform
    fdct16x16_avx512(in, in, bitrow, num_row);
    transpose_16nx16m_avx512(in, outcoeff512, txfm_size_row, txfm_size_col);
    (void)bd;
    (void)tx_type;
}

void svt_av1_fwd_txfm2d_64x16_avx512(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m512i       in[64];
    __m512i*      outcoeff512   = (__m512i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X16];
    const int32_t txw_idx       = get_txw_idx(TX_64X16);
    const int32_t txh_idx       = get_txh_idx(TX_64X16);
    const int32_t txfm_size_col = tx_size_wide[TX_64X16];
    const int32_t txfm_size_row = tx_size_high[TX_64X16];
    const int8_t  bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    const int8_t  bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 4;
    const int32_t num_col       = txfm_size_col >> 4;
    // column tranform
    for (int32_t i = 0; i < txfm_size_row; i++) {
        load_buffer_16_avx512(input + 0 + i * stride, in + 0 + i * 4, 16, 0, 0, shift[0]);
        load_buffer_16_avx512(input + 16 + i * stride, in + 1 + i * 4, 16, 0, 0, shift[0]);
        load_buffer_16_avx512(input + 32 + i * stride, in + 2 + i * 4, 16, 0, 0, shift[0]);
        load_buffer_16_avx512(input + 48 + i * stride, in + 3 + i * 4, 16, 0, 0, shift[0]);
    }

    fdct16x16_avx512(in, outcoeff512, bitcol, num_col);
    col_txfm_16x16_rounding_avx512(outcoeff512, -shift[1]);
    col_txfm_16x16_rounding_avx512(outcoeff512 + 16, -shift[1]);
    col_txfm_16x16_rounding_avx512(outcoeff512 + 32, -shift[1]);
    col_txfm_16x16_rounding_avx512(outcoeff512 + 48, -shift[1]);
    transpose_16nx16m_avx512(outcoeff512, in, txfm_size_col, txfm_size_row);
    // row tranform
    av1_fdct64_new_avx512(in, in, bitrow, txfm_size_row, num_row);
    transpose_16nx16m_avx512(in, outcoeff512, txfm_size_row, txfm_size_col);
    (void)bd;
    (void)tx_type;
}

static void av1_fdct32_new_line_wraper_avx512(const __m512i* input, __m512i* output, const int8_t cos_bit,
                                              const int32_t stride) {
    av1_fdct32_new_avx512(input, output, cos_bit, 16, stride);
}

static const fwd_transform_1d_avx512 col_fwdtxfm_16x32_arr[TX_TYPES] = {
    av1_fdct32_new_line_wraper_avx512, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    av1_idtx32_new_avx512, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST
};

static const fwd_transform_1d_avx512 row_fwdtxfm_16x32_arr[TX_TYPES] = {
    fdct16x16_avx512, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    fidtx16x16_avx512, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST
};

/* call this function only for DCT_DCT, IDTX */
void svt_av1_fwd_txfm2d_16x32_avx512(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m512i                       in[32];
    __m512i*                      outcoef512    = (__m512i*)output;
    const int8_t*                 shift         = fwd_txfm_shift_ls[TX_16X32];
    const int32_t                 txw_idx       = get_txw_idx(TX_16X32);
    const int32_t                 txh_idx       = get_txh_idx(TX_16X32);
    const fwd_transform_1d_avx512 col_txfm      = col_fwdtxfm_16x32_arr[tx_type];
    const fwd_transform_1d_avx512 row_txfm      = row_fwdtxfm_16x32_arr[tx_type];
    const int8_t                  bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    const int8_t                  bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t                 txfm_size_col = tx_size_wide[TX_16X32];
    const int32_t                 txfm_size_row = tx_size_high[TX_16X32];
    const int32_t                 num_row       = txfm_size_row >> 4;
    const int32_t                 num_col       = txfm_size_col >> 4;

    // column transform
    load_buffer_16x16_avx512(input, in, stride, 0, 0, shift[0]);
    load_buffer_16x16_avx512(input + 16 * stride, in + 16, stride, 0, 0, shift[0]);

    col_txfm(in, in, bitcol, num_col);
    col_txfm_16x16_rounding_avx512(&in[0], -shift[1]);
    col_txfm_16x16_rounding_avx512(&in[16], -shift[1]);
    transpose_16nx16m_avx512(in, outcoef512, txfm_size_col, txfm_size_row);

    // row transform
    row_txfm(outcoef512, in, bitrow, num_row);
    transpose_16nx16m_avx512(in, outcoef512, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_32_avx512(outcoef512, outcoef512, 32, -shift[2], 5793);
    (void)bd;
}

/* call this function only for DCT_DCT, IDTX */
void svt_av1_fwd_txfm2d_32x16_avx512(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    __m512i                       in[32];
    __m512i*                      outcoef512    = (__m512i*)output;
    const int8_t*                 shift         = fwd_txfm_shift_ls[TX_32X16];
    const int32_t                 txw_idx       = get_txw_idx(TX_32X16);
    const int32_t                 txh_idx       = get_txh_idx(TX_32X16);
    const fwd_transform_1d_avx512 col_txfm      = row_fwdtxfm_16x32_arr[tx_type];
    const fwd_transform_1d_avx512 row_txfm      = col_fwdtxfm_16x32_arr[tx_type];
    const int8_t                  bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    const int8_t                  bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t                 txfm_size_col = tx_size_wide[TX_32X16];
    const int32_t                 txfm_size_row = tx_size_high[TX_32X16];
    const int32_t                 num_row       = txfm_size_row >> 4;
    const int32_t                 num_col       = txfm_size_col >> 4;

    // column transform
    load_buffer_32x16n(input, in, stride, 0, 0, shift[0], txfm_size_row);
    col_txfm(in, in, bitcol, num_col);
    col_txfm_16x16_rounding_avx512(&in[0], -shift[1]);
    col_txfm_16x16_rounding_avx512(&in[16], -shift[1]);
    transpose_16nx16m_avx512(in, outcoef512, txfm_size_col, txfm_size_row);

    // row transform
    row_txfm(outcoef512, in, bitrow, num_row);

    transpose_16nx16m_avx512(in, outcoef512, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_32_avx512(outcoef512, outcoef512, 32, -shift[2], 5793);
    (void)bd;
}

static AOM_FORCE_INLINE void av1_round_shift_array_wxh_N2_avx512(__m512i* input, __m512i* output, const int32_t col_num,
                                                                 const int32_t height, const int8_t bit) {
    if (bit > 0) {
        __m512i round = _mm512_set1_epi32(1 << (bit - 1));
        int32_t i, j;
        for (i = 0; i < height; i++) {
            for (j = 0; j < col_num / 2; j++) {
                output[i * col_num + j] = _mm512_srai_epi32(_mm512_add_epi32(input[i * col_num + j], round),
                                                            (uint8_t)bit);
            }
        }
    } else {
        int32_t i, j;
        for (i = 0; i < height; i++) {
            for (j = 0; j < col_num / 2; j++) {
                output[i * col_num + j] = _mm512_slli_epi32(input[i * col_num + j], (uint8_t)(-bit));
            }
        }
    }
}

static AOM_FORCE_INLINE void av1_round_shift_rect_array_wxh_N2_avx512(__m512i* input, __m512i* output,
                                                                      const int32_t col_num, const int32_t height,
                                                                      const int8_t bit, const int32_t val) {
    const __m512i sqrt2  = _mm512_set1_epi32(val);
    const __m512i round2 = _mm512_set1_epi32(1 << (12 - 1));
    int32_t       i, j;
    if (bit > 0) {
        const __m512i round1 = _mm512_set1_epi32(1 << (bit - 1));
        __m512i       r0, r1, r2, r3;
        for (i = 0; i < height; i++) {
            for (j = 0; j < col_num / 2; j++) {
                r0                      = _mm512_add_epi32(input[i * col_num + j], round1);
                r1                      = _mm512_srai_epi32(r0, (uint8_t)bit);
                r2                      = _mm512_mullo_epi32(sqrt2, r1);
                r3                      = _mm512_add_epi32(r2, round2);
                output[i * col_num + j] = _mm512_srai_epi32(r3, (uint8_t)12);
            }
        }
    } else {
        __m512i r0, r1, r2;
        for (i = 0; i < height; i++) {
            for (j = 0; j < col_num / 2; j++) {
                r0                      = _mm512_slli_epi32(input[i * col_num + j], (uint8_t)-bit);
                r1                      = _mm512_mullo_epi32(sqrt2, r0);
                r2                      = _mm512_add_epi32(r1, round2);
                output[i * col_num + j] = _mm512_srai_epi32(r2, (uint8_t)12);
            }
        }
    }
}

static AOM_FORCE_INLINE void clear_buffer_wxh_N2_avx512(__m512i* buff, int32_t width, int32_t height) {
    const __m512i zero512 = _mm512_setzero_si512();
    int32_t       i, j;
    assert(width > 16);

    int32_t num_col = width >> 4;

    //top-right quarter
    for (i = 0; i < height / 2; i++) {
        for (j = num_col / 2; j < num_col; j++) {
            buff[i * num_col + j] = zero512;
        }
    }

    //bottom half
    for (i = height / 2; i < height; i++) {
        for (j = 0; j < num_col; j++) {
            buff[i * num_col + j] = zero512;
        }
    }
}

static AOM_FORCE_INLINE void load_buffer_32x32_in_64x64_avx512(const int16_t* input, int32_t stride, __m512i* output) {
    __m256i x0, x1;
    __m512i v0, v1;
    int32_t i;

    for (i = 0; i < 32; ++i) {
        x0 = _mm256_loadu_si256((const __m256i*)(input + 0 * 16));
        x1 = _mm256_loadu_si256((const __m256i*)(input + 1 * 16));

        v0 = _mm512_cvtepi16_epi32(x0);
        v1 = _mm512_cvtepi16_epi32(x1);

        _mm512_storeu_si512(output + 0, v0);
        _mm512_storeu_si512(output + 1, v1);

        input += stride;
        output += 4;
    }
}

static AOM_FORCE_INLINE void load_buffer_16xh_in_32x32_avx512(const int16_t* input, __m512i* output, int32_t stride,
                                                              int32_t height) {
    __m256i temp;
    int32_t i;

    for (i = 0; i < height; ++i) {
        temp      = _mm256_loadu_si256((const __m256i*)(input + 0 * 16));
        output[0] = _mm512_cvtepi16_epi32(temp);

        input += stride;
        output += 2;
    }
}

static AOM_FORCE_INLINE void load_buffer_32xh_in_32x32_avx512(const int16_t* input, __m512i* output, int32_t stride,
                                                              int32_t height) {
    __m256i temp[2];
    int32_t i;

    for (i = 0; i < height; ++i) {
        temp[0] = _mm256_loadu_si256((const __m256i*)(input + 0 * 16));
        temp[1] = _mm256_loadu_si256((const __m256i*)(input + 1 * 16));

        output[0] = _mm512_cvtepi16_epi32(temp[0]);
        output[1] = _mm512_cvtepi16_epi32(temp[1]);

        input += stride;
        output += 2;
    }
}

static void av1_fdct64_new_N2_avx512(const __m512i* input, __m512i* output, const int8_t cos_bit, const int32_t col_num,
                                     const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m512i  __rounding = _mm512_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 4;

    __m512i cospi_m32 = _mm512_set1_epi32(-cospi[32]);
    __m512i cospi_p32 = _mm512_set1_epi32(cospi[32]);
    __m512i cospi_m16 = _mm512_set1_epi32(-cospi[16]);
    __m512i cospi_p48 = _mm512_set1_epi32(cospi[48]);
    __m512i cospi_m48 = _mm512_set1_epi32(-cospi[48]);
    __m512i cospi_p16 = _mm512_set1_epi32(cospi[16]);
    __m512i cospi_m08 = _mm512_set1_epi32(-cospi[8]);
    __m512i cospi_p56 = _mm512_set1_epi32(cospi[56]);
    __m512i cospi_m56 = _mm512_set1_epi32(-cospi[56]);
    __m512i cospi_m40 = _mm512_set1_epi32(-cospi[40]);
    __m512i cospi_p24 = _mm512_set1_epi32(cospi[24]);
    __m512i cospi_m24 = _mm512_set1_epi32(-cospi[24]);
    __m512i cospi_p08 = _mm512_set1_epi32(cospi[8]);
    __m512i cospi_p60 = _mm512_set1_epi32(cospi[60]);
    __m512i cospi_p04 = _mm512_set1_epi32(cospi[4]);
    __m512i cospi_p28 = _mm512_set1_epi32(cospi[28]);
    __m512i cospi_p44 = _mm512_set1_epi32(cospi[44]);
    __m512i cospi_p20 = _mm512_set1_epi32(cospi[20]);
    __m512i cospi_p12 = _mm512_set1_epi32(cospi[12]);
    __m512i cospi_m04 = _mm512_set1_epi32(-cospi[4]);
    __m512i cospi_m60 = _mm512_set1_epi32(-cospi[60]);
    __m512i cospi_m36 = _mm512_set1_epi32(-cospi[36]);
    __m512i cospi_m28 = _mm512_set1_epi32(-cospi[28]);
    __m512i cospi_m20 = _mm512_set1_epi32(-cospi[20]);
    __m512i cospi_m44 = _mm512_set1_epi32(-cospi[44]);
    __m512i cospi_m52 = _mm512_set1_epi32(-cospi[52]);
    __m512i cospi_m12 = _mm512_set1_epi32(-cospi[12]);
    __m512i cospi_p62 = _mm512_set1_epi32(cospi[62]);
    __m512i cospi_p02 = _mm512_set1_epi32(cospi[2]);
    __m512i cospi_p30 = _mm512_set1_epi32(cospi[30]);
    __m512i cospi_m34 = _mm512_set1_epi32(-cospi[34]);
    __m512i cospi_p46 = _mm512_set1_epi32(cospi[46]);
    __m512i cospi_p18 = _mm512_set1_epi32(cospi[18]);
    __m512i cospi_p14 = _mm512_set1_epi32(cospi[14]);
    __m512i cospi_m50 = _mm512_set1_epi32(-cospi[50]);
    __m512i cospi_p54 = _mm512_set1_epi32(cospi[54]);
    __m512i cospi_p10 = _mm512_set1_epi32(cospi[10]);
    __m512i cospi_p22 = _mm512_set1_epi32(cospi[22]);
    __m512i cospi_m42 = _mm512_set1_epi32(-cospi[42]);
    __m512i cospi_p38 = _mm512_set1_epi32(cospi[38]);
    __m512i cospi_p26 = _mm512_set1_epi32(cospi[26]);
    __m512i cospi_p06 = _mm512_set1_epi32(cospi[6]);
    __m512i cospi_m58 = _mm512_set1_epi32(-cospi[58]);
    __m512i cospi_p63 = _mm512_set1_epi32(cospi[63]);
    __m512i cospi_p01 = _mm512_set1_epi32(cospi[1]);
    __m512i cospi_p31 = _mm512_set1_epi32(cospi[31]);
    __m512i cospi_m33 = _mm512_set1_epi32(-cospi[33]);
    __m512i cospi_p47 = _mm512_set1_epi32(cospi[47]);
    __m512i cospi_p17 = _mm512_set1_epi32(cospi[17]);
    __m512i cospi_p15 = _mm512_set1_epi32(cospi[15]);
    __m512i cospi_m49 = _mm512_set1_epi32(-cospi[49]);
    __m512i cospi_p55 = _mm512_set1_epi32(cospi[55]);
    __m512i cospi_p09 = _mm512_set1_epi32(cospi[9]);
    __m512i cospi_p23 = _mm512_set1_epi32(cospi[23]);
    __m512i cospi_m41 = _mm512_set1_epi32(-cospi[41]);
    __m512i cospi_p39 = _mm512_set1_epi32(cospi[39]);
    __m512i cospi_p25 = _mm512_set1_epi32(cospi[25]);
    __m512i cospi_p07 = _mm512_set1_epi32(cospi[7]);
    __m512i cospi_m57 = _mm512_set1_epi32(-cospi[57]);
    __m512i cospi_p59 = _mm512_set1_epi32(cospi[59]);
    __m512i cospi_p05 = _mm512_set1_epi32(cospi[5]);
    __m512i cospi_p27 = _mm512_set1_epi32(cospi[27]);
    __m512i cospi_m37 = _mm512_set1_epi32(-cospi[37]);
    __m512i cospi_p43 = _mm512_set1_epi32(cospi[43]);
    __m512i cospi_p21 = _mm512_set1_epi32(cospi[21]);
    __m512i cospi_p11 = _mm512_set1_epi32(cospi[11]);
    __m512i cospi_m53 = _mm512_set1_epi32(-cospi[53]);
    __m512i cospi_p51 = _mm512_set1_epi32(cospi[51]);
    __m512i cospi_p13 = _mm512_set1_epi32(cospi[13]);
    __m512i cospi_p19 = _mm512_set1_epi32(cospi[19]);
    __m512i cospi_m45 = _mm512_set1_epi32(-cospi[45]);
    __m512i cospi_p35 = _mm512_set1_epi32(cospi[35]);
    __m512i cospi_p29 = _mm512_set1_epi32(cospi[29]);
    __m512i cospi_p03 = _mm512_set1_epi32(cospi[3]);
    __m512i cospi_m61 = _mm512_set1_epi32(-cospi[61]);

    for (int32_t col = 0; col < columns; col++) {
        const __m512i* in  = &input[col];
        __m512i*       out = &output[col];

        // stage 1
        __m512i x1[64];
        x1[0]  = _mm512_add_epi32(in[0 * stride], in[63 * stride]);
        x1[63] = _mm512_sub_epi32(in[0 * stride], in[63 * stride]);
        x1[1]  = _mm512_add_epi32(in[1 * stride], in[62 * stride]);
        x1[62] = _mm512_sub_epi32(in[1 * stride], in[62 * stride]);
        x1[2]  = _mm512_add_epi32(in[2 * stride], in[61 * stride]);
        x1[61] = _mm512_sub_epi32(in[2 * stride], in[61 * stride]);
        x1[3]  = _mm512_add_epi32(in[3 * stride], in[60 * stride]);
        x1[60] = _mm512_sub_epi32(in[3 * stride], in[60 * stride]);
        x1[4]  = _mm512_add_epi32(in[4 * stride], in[59 * stride]);
        x1[59] = _mm512_sub_epi32(in[4 * stride], in[59 * stride]);
        x1[5]  = _mm512_add_epi32(in[5 * stride], in[58 * stride]);
        x1[58] = _mm512_sub_epi32(in[5 * stride], in[58 * stride]);
        x1[6]  = _mm512_add_epi32(in[6 * stride], in[57 * stride]);
        x1[57] = _mm512_sub_epi32(in[6 * stride], in[57 * stride]);
        x1[7]  = _mm512_add_epi32(in[7 * stride], in[56 * stride]);
        x1[56] = _mm512_sub_epi32(in[7 * stride], in[56 * stride]);
        x1[8]  = _mm512_add_epi32(in[8 * stride], in[55 * stride]);
        x1[55] = _mm512_sub_epi32(in[8 * stride], in[55 * stride]);
        x1[9]  = _mm512_add_epi32(in[9 * stride], in[54 * stride]);
        x1[54] = _mm512_sub_epi32(in[9 * stride], in[54 * stride]);
        x1[10] = _mm512_add_epi32(in[10 * stride], in[53 * stride]);
        x1[53] = _mm512_sub_epi32(in[10 * stride], in[53 * stride]);
        x1[11] = _mm512_add_epi32(in[11 * stride], in[52 * stride]);
        x1[52] = _mm512_sub_epi32(in[11 * stride], in[52 * stride]);
        x1[12] = _mm512_add_epi32(in[12 * stride], in[51 * stride]);
        x1[51] = _mm512_sub_epi32(in[12 * stride], in[51 * stride]);
        x1[13] = _mm512_add_epi32(in[13 * stride], in[50 * stride]);
        x1[50] = _mm512_sub_epi32(in[13 * stride], in[50 * stride]);
        x1[14] = _mm512_add_epi32(in[14 * stride], in[49 * stride]);
        x1[49] = _mm512_sub_epi32(in[14 * stride], in[49 * stride]);
        x1[15] = _mm512_add_epi32(in[15 * stride], in[48 * stride]);
        x1[48] = _mm512_sub_epi32(in[15 * stride], in[48 * stride]);
        x1[16] = _mm512_add_epi32(in[16 * stride], in[47 * stride]);
        x1[47] = _mm512_sub_epi32(in[16 * stride], in[47 * stride]);
        x1[17] = _mm512_add_epi32(in[17 * stride], in[46 * stride]);
        x1[46] = _mm512_sub_epi32(in[17 * stride], in[46 * stride]);
        x1[18] = _mm512_add_epi32(in[18 * stride], in[45 * stride]);
        x1[45] = _mm512_sub_epi32(in[18 * stride], in[45 * stride]);
        x1[19] = _mm512_add_epi32(in[19 * stride], in[44 * stride]);
        x1[44] = _mm512_sub_epi32(in[19 * stride], in[44 * stride]);
        x1[20] = _mm512_add_epi32(in[20 * stride], in[43 * stride]);
        x1[43] = _mm512_sub_epi32(in[20 * stride], in[43 * stride]);
        x1[21] = _mm512_add_epi32(in[21 * stride], in[42 * stride]);
        x1[42] = _mm512_sub_epi32(in[21 * stride], in[42 * stride]);
        x1[22] = _mm512_add_epi32(in[22 * stride], in[41 * stride]);
        x1[41] = _mm512_sub_epi32(in[22 * stride], in[41 * stride]);
        x1[23] = _mm512_add_epi32(in[23 * stride], in[40 * stride]);
        x1[40] = _mm512_sub_epi32(in[23 * stride], in[40 * stride]);
        x1[24] = _mm512_add_epi32(in[24 * stride], in[39 * stride]);
        x1[39] = _mm512_sub_epi32(in[24 * stride], in[39 * stride]);
        x1[25] = _mm512_add_epi32(in[25 * stride], in[38 * stride]);
        x1[38] = _mm512_sub_epi32(in[25 * stride], in[38 * stride]);
        x1[26] = _mm512_add_epi32(in[26 * stride], in[37 * stride]);
        x1[37] = _mm512_sub_epi32(in[26 * stride], in[37 * stride]);
        x1[27] = _mm512_add_epi32(in[27 * stride], in[36 * stride]);
        x1[36] = _mm512_sub_epi32(in[27 * stride], in[36 * stride]);
        x1[28] = _mm512_add_epi32(in[28 * stride], in[35 * stride]);
        x1[35] = _mm512_sub_epi32(in[28 * stride], in[35 * stride]);
        x1[29] = _mm512_add_epi32(in[29 * stride], in[34 * stride]);
        x1[34] = _mm512_sub_epi32(in[29 * stride], in[34 * stride]);
        x1[30] = _mm512_add_epi32(in[30 * stride], in[33 * stride]);
        x1[33] = _mm512_sub_epi32(in[30 * stride], in[33 * stride]);
        x1[31] = _mm512_add_epi32(in[31 * stride], in[32 * stride]);
        x1[32] = _mm512_sub_epi32(in[31 * stride], in[32 * stride]);

        // stage 2
        __m512i x2[64];
        x2[0]  = _mm512_add_epi32(x1[0], x1[31]);
        x2[31] = _mm512_sub_epi32(x1[0], x1[31]);
        x2[1]  = _mm512_add_epi32(x1[1], x1[30]);
        x2[30] = _mm512_sub_epi32(x1[1], x1[30]);
        x2[2]  = _mm512_add_epi32(x1[2], x1[29]);
        x2[29] = _mm512_sub_epi32(x1[2], x1[29]);
        x2[3]  = _mm512_add_epi32(x1[3], x1[28]);
        x2[28] = _mm512_sub_epi32(x1[3], x1[28]);
        x2[4]  = _mm512_add_epi32(x1[4], x1[27]);
        x2[27] = _mm512_sub_epi32(x1[4], x1[27]);
        x2[5]  = _mm512_add_epi32(x1[5], x1[26]);
        x2[26] = _mm512_sub_epi32(x1[5], x1[26]);
        x2[6]  = _mm512_add_epi32(x1[6], x1[25]);
        x2[25] = _mm512_sub_epi32(x1[6], x1[25]);
        x2[7]  = _mm512_add_epi32(x1[7], x1[24]);
        x2[24] = _mm512_sub_epi32(x1[7], x1[24]);
        x2[8]  = _mm512_add_epi32(x1[8], x1[23]);
        x2[23] = _mm512_sub_epi32(x1[8], x1[23]);
        x2[9]  = _mm512_add_epi32(x1[9], x1[22]);
        x2[22] = _mm512_sub_epi32(x1[9], x1[22]);
        x2[10] = _mm512_add_epi32(x1[10], x1[21]);
        x2[21] = _mm512_sub_epi32(x1[10], x1[21]);
        x2[11] = _mm512_add_epi32(x1[11], x1[20]);
        x2[20] = _mm512_sub_epi32(x1[11], x1[20]);
        x2[12] = _mm512_add_epi32(x1[12], x1[19]);
        x2[19] = _mm512_sub_epi32(x1[12], x1[19]);
        x2[13] = _mm512_add_epi32(x1[13], x1[18]);
        x2[18] = _mm512_sub_epi32(x1[13], x1[18]);
        x2[14] = _mm512_add_epi32(x1[14], x1[17]);
        x2[17] = _mm512_sub_epi32(x1[14], x1[17]);
        x2[15] = _mm512_add_epi32(x1[15], x1[16]);
        x2[16] = _mm512_sub_epi32(x1[15], x1[16]);
        x2[32] = x1[32];
        x2[33] = x1[33];
        x2[34] = x1[34];
        x2[35] = x1[35];
        x2[36] = x1[36];
        x2[37] = x1[37];
        x2[38] = x1[38];
        x2[39] = x1[39];
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[40], x1[55], x2[40], x2[55], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[41], x1[54], x2[41], x2[54], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[42], x1[53], x2[42], x2[53], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[43], x1[52], x2[43], x2[52], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[44], x1[51], x2[44], x2[51], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[45], x1[50], x2[45], x2[50], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[46], x1[49], x2[46], x2[49], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[47], x1[48], x2[47], x2[48], __rounding, cos_bit);
        x2[56] = x1[56];
        x2[57] = x1[57];
        x2[58] = x1[58];
        x2[59] = x1[59];
        x2[60] = x1[60];
        x2[61] = x1[61];
        x2[62] = x1[62];
        x2[63] = x1[63];

        // stage 3
        __m512i x3[64];
        x3[0]  = _mm512_add_epi32(x2[0], x2[15]);
        x3[15] = _mm512_sub_epi32(x2[0], x2[15]);
        x3[1]  = _mm512_add_epi32(x2[1], x2[14]);
        x3[14] = _mm512_sub_epi32(x2[1], x2[14]);
        x3[2]  = _mm512_add_epi32(x2[2], x2[13]);
        x3[13] = _mm512_sub_epi32(x2[2], x2[13]);
        x3[3]  = _mm512_add_epi32(x2[3], x2[12]);
        x3[12] = _mm512_sub_epi32(x2[3], x2[12]);
        x3[4]  = _mm512_add_epi32(x2[4], x2[11]);
        x3[11] = _mm512_sub_epi32(x2[4], x2[11]);
        x3[5]  = _mm512_add_epi32(x2[5], x2[10]);
        x3[10] = _mm512_sub_epi32(x2[5], x2[10]);
        x3[6]  = _mm512_add_epi32(x2[6], x2[9]);
        x3[9]  = _mm512_sub_epi32(x2[6], x2[9]);
        x3[7]  = _mm512_add_epi32(x2[7], x2[8]);
        x3[8]  = _mm512_sub_epi32(x2[7], x2[8]);
        x3[16] = x2[16];
        x3[17] = x2[17];
        x3[18] = x2[18];
        x3[19] = x2[19];
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x2[20], x2[27], x3[20], x3[27], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x2[21], x2[26], x3[21], x3[26], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x2[22], x2[25], x3[22], x3[25], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x2[23], x2[24], x3[23], x3[24], __rounding, cos_bit);
        x3[28] = x2[28];
        x3[29] = x2[29];
        x3[30] = x2[30];
        x3[31] = x2[31];
        x3[32] = _mm512_add_epi32(x2[32], x2[47]);
        x3[47] = _mm512_sub_epi32(x2[32], x2[47]);
        x3[33] = _mm512_add_epi32(x2[33], x2[46]);
        x3[46] = _mm512_sub_epi32(x2[33], x2[46]);
        x3[34] = _mm512_add_epi32(x2[34], x2[45]);
        x3[45] = _mm512_sub_epi32(x2[34], x2[45]);
        x3[35] = _mm512_add_epi32(x2[35], x2[44]);
        x3[44] = _mm512_sub_epi32(x2[35], x2[44]);
        x3[36] = _mm512_add_epi32(x2[36], x2[43]);
        x3[43] = _mm512_sub_epi32(x2[36], x2[43]);
        x3[37] = _mm512_add_epi32(x2[37], x2[42]);
        x3[42] = _mm512_sub_epi32(x2[37], x2[42]);
        x3[38] = _mm512_add_epi32(x2[38], x2[41]);
        x3[41] = _mm512_sub_epi32(x2[38], x2[41]);
        x3[39] = _mm512_add_epi32(x2[39], x2[40]);
        x3[40] = _mm512_sub_epi32(x2[39], x2[40]);
        x3[48] = _mm512_sub_epi32(x2[63], x2[48]);
        x3[63] = _mm512_add_epi32(x2[63], x2[48]);
        x3[49] = _mm512_sub_epi32(x2[62], x2[49]);
        x3[62] = _mm512_add_epi32(x2[62], x2[49]);
        x3[50] = _mm512_sub_epi32(x2[61], x2[50]);
        x3[61] = _mm512_add_epi32(x2[61], x2[50]);
        x3[51] = _mm512_sub_epi32(x2[60], x2[51]);
        x3[60] = _mm512_add_epi32(x2[60], x2[51]);
        x3[52] = _mm512_sub_epi32(x2[59], x2[52]);
        x3[59] = _mm512_add_epi32(x2[59], x2[52]);
        x3[53] = _mm512_sub_epi32(x2[58], x2[53]);
        x3[58] = _mm512_add_epi32(x2[58], x2[53]);
        x3[54] = _mm512_sub_epi32(x2[57], x2[54]);
        x3[57] = _mm512_add_epi32(x2[57], x2[54]);
        x3[55] = _mm512_sub_epi32(x2[56], x2[55]);
        x3[56] = _mm512_add_epi32(x2[56], x2[55]);

        // stage 4
        __m512i x4[64];
        x4[0] = _mm512_add_epi32(x3[0], x3[7]);
        x4[7] = _mm512_sub_epi32(x3[0], x3[7]);
        x4[1] = _mm512_add_epi32(x3[1], x3[6]);
        x4[6] = _mm512_sub_epi32(x3[1], x3[6]);
        x4[2] = _mm512_add_epi32(x3[2], x3[5]);
        x4[5] = _mm512_sub_epi32(x3[2], x3[5]);
        x4[3] = _mm512_add_epi32(x3[3], x3[4]);
        x4[4] = _mm512_sub_epi32(x3[3], x3[4]);
        x4[8] = x3[8];
        x4[9] = x3[9];
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x3[10], x3[13], x4[10], x4[13], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x3[11], x3[12], x4[11], x4[12], __rounding, cos_bit);
        x4[14] = x3[14];
        x4[15] = x3[15];
        x4[16] = _mm512_add_epi32(x3[16], x3[23]);
        x4[23] = _mm512_sub_epi32(x3[16], x3[23]);
        x4[17] = _mm512_add_epi32(x3[17], x3[22]);
        x4[22] = _mm512_sub_epi32(x3[17], x3[22]);
        x4[18] = _mm512_add_epi32(x3[18], x3[21]);
        x4[21] = _mm512_sub_epi32(x3[18], x3[21]);
        x4[19] = _mm512_add_epi32(x3[19], x3[20]);
        x4[20] = _mm512_sub_epi32(x3[19], x3[20]);
        x4[24] = _mm512_sub_epi32(x3[31], x3[24]);
        x4[31] = _mm512_add_epi32(x3[31], x3[24]);
        x4[25] = _mm512_sub_epi32(x3[30], x3[25]);
        x4[30] = _mm512_add_epi32(x3[30], x3[25]);
        x4[26] = _mm512_sub_epi32(x3[29], x3[26]);
        x4[29] = _mm512_add_epi32(x3[29], x3[26]);
        x4[27] = _mm512_sub_epi32(x3[28], x3[27]);
        x4[28] = _mm512_add_epi32(x3[28], x3[27]);
        x4[32] = x3[32];
        x4[33] = x3[33];
        x4[34] = x3[34];
        x4[35] = x3[35];
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x3[36], x3[59], x4[36], x4[59], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x3[37], x3[58], x4[37], x4[58], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x3[38], x3[57], x4[38], x4[57], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x3[39], x3[56], x4[39], x4[56], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x3[40], x3[55], x4[40], x4[55], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x3[41], x3[54], x4[41], x4[54], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x3[42], x3[53], x4[42], x4[53], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x3[43], x3[52], x4[43], x4[52], __rounding, cos_bit);
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
        __m512i x5[64];
        x5[0] = _mm512_add_epi32(x4[0], x4[3]);
        x5[3] = _mm512_sub_epi32(x4[0], x4[3]);
        x5[1] = _mm512_add_epi32(x4[1], x4[2]);
        x5[2] = _mm512_sub_epi32(x4[1], x4[2]);
        x5[4] = x4[4];
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x4[5], x4[6], x5[5], x5[6], __rounding, cos_bit);
        x5[7]  = x4[7];
        x5[8]  = _mm512_add_epi32(x4[8], x4[11]);
        x5[11] = _mm512_sub_epi32(x4[8], x4[11]);
        x5[9]  = _mm512_add_epi32(x4[9], x4[10]);
        x5[10] = _mm512_sub_epi32(x4[9], x4[10]);
        x5[12] = _mm512_sub_epi32(x4[15], x4[12]);
        x5[15] = _mm512_add_epi32(x4[15], x4[12]);
        x5[13] = _mm512_sub_epi32(x4[14], x4[13]);
        x5[14] = _mm512_add_epi32(x4[14], x4[13]);
        x5[16] = x4[16];
        x5[17] = x4[17];
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x4[18], x4[29], x5[18], x5[29], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x4[19], x4[28], x5[19], x5[28], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x4[20], x4[27], x5[20], x5[27], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x4[21], x4[26], x5[21], x5[26], __rounding, cos_bit);
        x5[22] = x4[22];
        x5[23] = x4[23];
        x5[24] = x4[24];
        x5[25] = x4[25];
        x5[30] = x4[30];
        x5[31] = x4[31];
        x5[32] = _mm512_add_epi32(x4[32], x4[39]);
        x5[39] = _mm512_sub_epi32(x4[32], x4[39]);
        x5[33] = _mm512_add_epi32(x4[33], x4[38]);
        x5[38] = _mm512_sub_epi32(x4[33], x4[38]);
        x5[34] = _mm512_add_epi32(x4[34], x4[37]);
        x5[37] = _mm512_sub_epi32(x4[34], x4[37]);
        x5[35] = _mm512_add_epi32(x4[35], x4[36]);
        x5[36] = _mm512_sub_epi32(x4[35], x4[36]);
        x5[40] = _mm512_sub_epi32(x4[47], x4[40]);
        x5[47] = _mm512_add_epi32(x4[47], x4[40]);
        x5[41] = _mm512_sub_epi32(x4[46], x4[41]);
        x5[46] = _mm512_add_epi32(x4[46], x4[41]);
        x5[42] = _mm512_sub_epi32(x4[45], x4[42]);
        x5[45] = _mm512_add_epi32(x4[45], x4[42]);
        x5[43] = _mm512_sub_epi32(x4[44], x4[43]);
        x5[44] = _mm512_add_epi32(x4[44], x4[43]);
        x5[48] = _mm512_add_epi32(x4[48], x4[55]);
        x5[55] = _mm512_sub_epi32(x4[48], x4[55]);
        x5[49] = _mm512_add_epi32(x4[49], x4[54]);
        x5[54] = _mm512_sub_epi32(x4[49], x4[54]);
        x5[50] = _mm512_add_epi32(x4[50], x4[53]);
        x5[53] = _mm512_sub_epi32(x4[50], x4[53]);
        x5[51] = _mm512_add_epi32(x4[51], x4[52]);
        x5[52] = _mm512_sub_epi32(x4[51], x4[52]);
        x5[56] = _mm512_sub_epi32(x4[63], x4[56]);
        x5[63] = _mm512_add_epi32(x4[63], x4[56]);
        x5[57] = _mm512_sub_epi32(x4[62], x4[57]);
        x5[62] = _mm512_add_epi32(x4[62], x4[57]);
        x5[58] = _mm512_sub_epi32(x4[61], x4[58]);
        x5[61] = _mm512_add_epi32(x4[61], x4[58]);
        x5[59] = _mm512_sub_epi32(x4[60], x4[59]);
        x5[60] = _mm512_add_epi32(x4[60], x4[59]);

        // stage 6
        __m512i x6[64];
        x6[0] = half_btf_avx512(&cospi_p32, &x5[0], &cospi_p32, &x5[1], &__rounding, cos_bit);
        x6[2] = half_btf_avx512(&cospi_p48, &x5[2], &cospi_p16, &x5[3], &__rounding, cos_bit);
        x6[4] = _mm512_add_epi32(x5[4], x5[5]);
        x6[5] = _mm512_sub_epi32(x5[4], x5[5]);
        x6[6] = _mm512_sub_epi32(x5[7], x5[6]);
        x6[7] = _mm512_add_epi32(x5[7], x5[6]);
        x6[8] = x5[8];
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x5[9], x5[14], x6[9], x6[14], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x5[10], x5[13], x6[10], x6[13], __rounding, cos_bit);
        x6[11] = x5[11];
        x6[12] = x5[12];
        x6[15] = x5[15];
        x6[16] = _mm512_add_epi32(x5[16], x5[19]);
        x6[19] = _mm512_sub_epi32(x5[16], x5[19]);
        x6[17] = _mm512_add_epi32(x5[17], x5[18]);
        x6[18] = _mm512_sub_epi32(x5[17], x5[18]);
        x6[20] = _mm512_sub_epi32(x5[23], x5[20]);
        x6[23] = _mm512_add_epi32(x5[23], x5[20]);
        x6[21] = _mm512_sub_epi32(x5[22], x5[21]);
        x6[22] = _mm512_add_epi32(x5[22], x5[21]);
        x6[24] = _mm512_add_epi32(x5[24], x5[27]);
        x6[27] = _mm512_sub_epi32(x5[24], x5[27]);
        x6[25] = _mm512_add_epi32(x5[25], x5[26]);
        x6[26] = _mm512_sub_epi32(x5[25], x5[26]);
        x6[28] = _mm512_sub_epi32(x5[31], x5[28]);
        x6[31] = _mm512_add_epi32(x5[31], x5[28]);
        x6[29] = _mm512_sub_epi32(x5[30], x5[29]);
        x6[30] = _mm512_add_epi32(x5[30], x5[29]);
        x6[32] = x5[32];
        x6[33] = x5[33];
        btf_32_type0_avx512_new(cospi_m08, cospi_p56, x5[34], x5[61], x6[34], x6[61], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m08, cospi_p56, x5[35], x5[60], x6[35], x6[60], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m56, cospi_m08, x5[36], x5[59], x6[36], x6[59], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m56, cospi_m08, x5[37], x5[58], x6[37], x6[58], __rounding, cos_bit);
        x6[38] = x5[38];
        x6[39] = x5[39];
        x6[40] = x5[40];
        x6[41] = x5[41];
        btf_32_type0_avx512_new(cospi_m40, cospi_p24, x5[42], x5[53], x6[42], x6[53], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m40, cospi_p24, x5[43], x5[52], x6[43], x6[52], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m24, cospi_m40, x5[44], x5[51], x6[44], x6[51], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m24, cospi_m40, x5[45], x5[50], x6[45], x6[50], __rounding, cos_bit);
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
        __m512i x7[64];
        x7[0]  = x6[0];
        x7[2]  = x6[2];
        x7[4]  = half_btf_avx512(&cospi_p56, &x6[4], &cospi_p08, &x6[7], &__rounding, cos_bit);
        x7[6]  = half_btf_avx512(&cospi_p24, &x6[6], &cospi_m40, &x6[5], &__rounding, cos_bit);
        x7[8]  = _mm512_add_epi32(x6[8], x6[9]);
        x7[9]  = _mm512_sub_epi32(x6[8], x6[9]);
        x7[10] = _mm512_sub_epi32(x6[11], x6[10]);
        x7[11] = _mm512_add_epi32(x6[11], x6[10]);
        x7[12] = _mm512_add_epi32(x6[12], x6[13]);
        x7[13] = _mm512_sub_epi32(x6[12], x6[13]);
        x7[14] = _mm512_sub_epi32(x6[15], x6[14]);
        x7[15] = _mm512_add_epi32(x6[15], x6[14]);
        x7[16] = x6[16];
        btf_32_type0_avx512_new(cospi_m08, cospi_p56, x6[17], x6[30], x7[17], x7[30], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m56, cospi_m08, x6[18], x6[29], x7[18], x7[29], __rounding, cos_bit);
        x7[19] = x6[19];
        x7[20] = x6[20];
        btf_32_type0_avx512_new(cospi_m40, cospi_p24, x6[21], x6[26], x7[21], x7[26], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m24, cospi_m40, x6[22], x6[25], x7[22], x7[25], __rounding, cos_bit);
        x7[23] = x6[23];
        x7[24] = x6[24];
        x7[27] = x6[27];
        x7[28] = x6[28];
        x7[31] = x6[31];
        x7[32] = _mm512_add_epi32(x6[32], x6[35]);
        x7[35] = _mm512_sub_epi32(x6[32], x6[35]);
        x7[33] = _mm512_add_epi32(x6[33], x6[34]);
        x7[34] = _mm512_sub_epi32(x6[33], x6[34]);
        x7[36] = _mm512_sub_epi32(x6[39], x6[36]);
        x7[39] = _mm512_add_epi32(x6[39], x6[36]);
        x7[37] = _mm512_sub_epi32(x6[38], x6[37]);
        x7[38] = _mm512_add_epi32(x6[38], x6[37]);
        x7[40] = _mm512_add_epi32(x6[40], x6[43]);
        x7[43] = _mm512_sub_epi32(x6[40], x6[43]);
        x7[41] = _mm512_add_epi32(x6[41], x6[42]);
        x7[42] = _mm512_sub_epi32(x6[41], x6[42]);
        x7[44] = _mm512_sub_epi32(x6[47], x6[44]);
        x7[47] = _mm512_add_epi32(x6[47], x6[44]);
        x7[45] = _mm512_sub_epi32(x6[46], x6[45]);
        x7[46] = _mm512_add_epi32(x6[46], x6[45]);
        x7[48] = _mm512_add_epi32(x6[48], x6[51]);
        x7[51] = _mm512_sub_epi32(x6[48], x6[51]);
        x7[49] = _mm512_add_epi32(x6[49], x6[50]);
        x7[50] = _mm512_sub_epi32(x6[49], x6[50]);
        x7[52] = _mm512_sub_epi32(x6[55], x6[52]);
        x7[55] = _mm512_add_epi32(x6[55], x6[52]);
        x7[53] = _mm512_sub_epi32(x6[54], x6[53]);
        x7[54] = _mm512_add_epi32(x6[54], x6[53]);
        x7[56] = _mm512_add_epi32(x6[56], x6[59]);
        x7[59] = _mm512_sub_epi32(x6[56], x6[59]);
        x7[57] = _mm512_add_epi32(x6[57], x6[58]);
        x7[58] = _mm512_sub_epi32(x6[57], x6[58]);
        x7[60] = _mm512_sub_epi32(x6[63], x6[60]);
        x7[63] = _mm512_add_epi32(x6[63], x6[60]);
        x7[61] = _mm512_sub_epi32(x6[62], x6[61]);
        x7[62] = _mm512_add_epi32(x6[62], x6[61]);

        // stage 8
        __m512i x8[64];
        x8[0] = x7[0];
        x8[2] = x7[2];
        x8[4] = x7[4];
        x8[6] = x7[6];

        x8[8]  = half_btf_avx512(&cospi_p60, &x7[8], &cospi_p04, &x7[15], &__rounding, cos_bit);
        x8[14] = half_btf_avx512(&cospi_p28, &x7[14], &cospi_m36, &x7[9], &__rounding, cos_bit);
        x8[10] = half_btf_avx512(&cospi_p44, &x7[10], &cospi_p20, &x7[13], &__rounding, cos_bit);
        x8[12] = half_btf_avx512(&cospi_p12, &x7[12], &cospi_m52, &x7[11], &__rounding, cos_bit);
        x8[16] = _mm512_add_epi32(x7[16], x7[17]);
        x8[17] = _mm512_sub_epi32(x7[16], x7[17]);
        x8[18] = _mm512_sub_epi32(x7[19], x7[18]);
        x8[19] = _mm512_add_epi32(x7[19], x7[18]);
        x8[20] = _mm512_add_epi32(x7[20], x7[21]);
        x8[21] = _mm512_sub_epi32(x7[20], x7[21]);
        x8[22] = _mm512_sub_epi32(x7[23], x7[22]);
        x8[23] = _mm512_add_epi32(x7[23], x7[22]);
        x8[24] = _mm512_add_epi32(x7[24], x7[25]);
        x8[25] = _mm512_sub_epi32(x7[24], x7[25]);
        x8[26] = _mm512_sub_epi32(x7[27], x7[26]);
        x8[27] = _mm512_add_epi32(x7[27], x7[26]);
        x8[28] = _mm512_add_epi32(x7[28], x7[29]);
        x8[29] = _mm512_sub_epi32(x7[28], x7[29]);
        x8[30] = _mm512_sub_epi32(x7[31], x7[30]);
        x8[31] = _mm512_add_epi32(x7[31], x7[30]);
        x8[32] = x7[32];
        btf_32_type0_avx512_new(cospi_m04, cospi_p60, x7[33], x7[62], x8[33], x8[62], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m60, cospi_m04, x7[34], x7[61], x8[34], x8[61], __rounding, cos_bit);
        x8[35] = x7[35];
        x8[36] = x7[36];
        btf_32_type0_avx512_new(cospi_m36, cospi_p28, x7[37], x7[58], x8[37], x8[58], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m28, cospi_m36, x7[38], x7[57], x8[38], x8[57], __rounding, cos_bit);
        x8[39] = x7[39];
        x8[40] = x7[40];
        btf_32_type0_avx512_new(cospi_m20, cospi_p44, x7[41], x7[54], x8[41], x8[54], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m44, cospi_m20, x7[42], x7[53], x8[42], x8[53], __rounding, cos_bit);
        x8[43] = x7[43];
        x8[44] = x7[44];
        btf_32_type0_avx512_new(cospi_m52, cospi_p12, x7[45], x7[50], x8[45], x8[50], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m12, cospi_m52, x7[46], x7[49], x8[46], x8[49], __rounding, cos_bit);
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
        __m512i x9[64];
        x9[0]  = x8[0];
        x9[2]  = x8[2];
        x9[4]  = x8[4];
        x9[6]  = x8[6];
        x9[8]  = x8[8];
        x9[10] = x8[10];
        x9[12] = x8[12];
        x9[14] = x8[14];
        x9[16] = half_btf_avx512(&cospi_p62, &x8[16], &cospi_p02, &x8[31], &__rounding, cos_bit);
        x9[30] = half_btf_avx512(&cospi_p30, &x8[30], &cospi_m34, &x8[17], &__rounding, cos_bit);
        x9[18] = half_btf_avx512(&cospi_p46, &x8[18], &cospi_p18, &x8[29], &__rounding, cos_bit);
        x9[28] = half_btf_avx512(&cospi_p14, &x8[28], &cospi_m50, &x8[19], &__rounding, cos_bit);
        x9[20] = half_btf_avx512(&cospi_p54, &x8[20], &cospi_p10, &x8[27], &__rounding, cos_bit);
        x9[26] = half_btf_avx512(&cospi_p22, &x8[26], &cospi_m42, &x8[21], &__rounding, cos_bit);
        x9[22] = half_btf_avx512(&cospi_p38, &x8[22], &cospi_p26, &x8[25], &__rounding, cos_bit);
        x9[24] = half_btf_avx512(&cospi_p06, &x8[24], &cospi_m58, &x8[23], &__rounding, cos_bit);
        x9[32] = _mm512_add_epi32(x8[32], x8[33]);
        x9[33] = _mm512_sub_epi32(x8[32], x8[33]);
        x9[34] = _mm512_sub_epi32(x8[35], x8[34]);
        x9[35] = _mm512_add_epi32(x8[35], x8[34]);
        x9[36] = _mm512_add_epi32(x8[36], x8[37]);
        x9[37] = _mm512_sub_epi32(x8[36], x8[37]);
        x9[38] = _mm512_sub_epi32(x8[39], x8[38]);
        x9[39] = _mm512_add_epi32(x8[39], x8[38]);
        x9[40] = _mm512_add_epi32(x8[40], x8[41]);
        x9[41] = _mm512_sub_epi32(x8[40], x8[41]);
        x9[42] = _mm512_sub_epi32(x8[43], x8[42]);
        x9[43] = _mm512_add_epi32(x8[43], x8[42]);
        x9[44] = _mm512_add_epi32(x8[44], x8[45]);
        x9[45] = _mm512_sub_epi32(x8[44], x8[45]);
        x9[46] = _mm512_sub_epi32(x8[47], x8[46]);
        x9[47] = _mm512_add_epi32(x8[47], x8[46]);
        x9[48] = _mm512_add_epi32(x8[48], x8[49]);
        x9[49] = _mm512_sub_epi32(x8[48], x8[49]);
        x9[50] = _mm512_sub_epi32(x8[51], x8[50]);
        x9[51] = _mm512_add_epi32(x8[51], x8[50]);
        x9[52] = _mm512_add_epi32(x8[52], x8[53]);
        x9[53] = _mm512_sub_epi32(x8[52], x8[53]);
        x9[54] = _mm512_sub_epi32(x8[55], x8[54]);
        x9[55] = _mm512_add_epi32(x8[55], x8[54]);
        x9[56] = _mm512_add_epi32(x8[56], x8[57]);
        x9[57] = _mm512_sub_epi32(x8[56], x8[57]);
        x9[58] = _mm512_sub_epi32(x8[59], x8[58]);
        x9[59] = _mm512_add_epi32(x8[59], x8[58]);
        x9[60] = _mm512_add_epi32(x8[60], x8[61]);
        x9[61] = _mm512_sub_epi32(x8[60], x8[61]);
        x9[62] = _mm512_sub_epi32(x8[63], x8[62]);
        x9[63] = _mm512_add_epi32(x8[63], x8[62]);

        // stage 10
        __m512i x10[64];
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
        x10[32]          = half_btf_avx512(&cospi_p63, &x9[32], &cospi_p01, &x9[63], &__rounding, cos_bit);
        x10[62]          = half_btf_avx512(&cospi_p31, &x9[62], &cospi_m33, &x9[33], &__rounding, cos_bit);
        x10[34]          = half_btf_avx512(&cospi_p47, &x9[34], &cospi_p17, &x9[61], &__rounding, cos_bit);
        x10[60]          = half_btf_avx512(&cospi_p15, &x9[60], &cospi_m49, &x9[35], &__rounding, cos_bit);
        x10[36]          = half_btf_avx512(&cospi_p55, &x9[36], &cospi_p09, &x9[59], &__rounding, cos_bit);
        x10[58]          = half_btf_avx512(&cospi_p23, &x9[58], &cospi_m41, &x9[37], &__rounding, cos_bit);
        x10[38]          = half_btf_avx512(&cospi_p39, &x9[38], &cospi_p25, &x9[57], &__rounding, cos_bit);
        x10[56]          = half_btf_avx512(&cospi_p07, &x9[56], &cospi_m57, &x9[39], &__rounding, cos_bit);
        x10[40]          = half_btf_avx512(&cospi_p59, &x9[40], &cospi_p05, &x9[55], &__rounding, cos_bit);
        x10[54]          = half_btf_avx512(&cospi_p27, &x9[54], &cospi_m37, &x9[41], &__rounding, cos_bit);
        x10[42]          = half_btf_avx512(&cospi_p43, &x9[42], &cospi_p21, &x9[53], &__rounding, cos_bit);
        x10[52]          = half_btf_avx512(&cospi_p11, &x9[52], &cospi_m53, &x9[43], &__rounding, cos_bit);
        x10[44]          = half_btf_avx512(&cospi_p51, &x9[44], &cospi_p13, &x9[51], &__rounding, cos_bit);
        x10[50]          = half_btf_avx512(&cospi_p19, &x9[50], &cospi_m45, &x9[45], &__rounding, cos_bit);
        x10[46]          = half_btf_avx512(&cospi_p35, &x9[46], &cospi_p29, &x9[49], &__rounding, cos_bit);
        x10[48]          = half_btf_avx512(&cospi_p03, &x9[48], &cospi_m61, &x9[47], &__rounding, cos_bit);

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

static void fidtx64x64_N2_avx512(const __m512i* input, __m512i* output) {
    const uint8_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 4 * 5793; // 4 * new_sqrt2
    const int32_t col_num  = 4;
    const __m512i newsqrt  = _mm512_set1_epi32(sqrt);
    const __m512i rounding = _mm512_set1_epi32(1 << (bits - 1));

    __m512i temp;
    int32_t num_iters = 64 * col_num;
    for (int32_t i = 0; i < num_iters / 2; i += 4) {
        temp          = _mm512_mullo_epi32(input[i], newsqrt);
        temp          = _mm512_add_epi32(temp, rounding);
        output[i]     = _mm512_srai_epi32(temp, bits);
        temp          = _mm512_mullo_epi32(input[i + 1], newsqrt);
        temp          = _mm512_add_epi32(temp, rounding);
        output[i + 1] = _mm512_srai_epi32(temp, bits);
    }
}

static void av1_fdct32_new_N2_avx512(const __m512i* input, __m512i* output, const int8_t cos_bit, const int32_t col_num,
                                     const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m512i  __rounding = _mm512_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 4;

    __m512i cospi_m32 = _mm512_set1_epi32(-cospi[32]);
    __m512i cospi_p32 = _mm512_set1_epi32(cospi[32]);
    __m512i cospi_m16 = _mm512_set1_epi32(-cospi[16]);
    __m512i cospi_p48 = _mm512_set1_epi32(cospi[48]);
    __m512i cospi_m48 = _mm512_set1_epi32(-cospi[48]);
    __m512i cospi_m08 = _mm512_set1_epi32(-cospi[8]);
    __m512i cospi_p56 = _mm512_set1_epi32(cospi[56]);
    __m512i cospi_m56 = _mm512_set1_epi32(-cospi[56]);
    __m512i cospi_m40 = _mm512_set1_epi32(-cospi[40]);
    __m512i cospi_p24 = _mm512_set1_epi32(cospi[24]);
    __m512i cospi_m24 = _mm512_set1_epi32(-cospi[24]);
    __m512i cospi_p16 = _mm512_set1_epi32(cospi[16]);
    __m512i cospi_p08 = _mm512_set1_epi32(cospi[8]);
    __m512i cospi_p04 = _mm512_set1_epi32(cospi[4]);
    __m512i cospi_p60 = _mm512_set1_epi32(cospi[60]);
    __m512i cospi_m36 = _mm512_set1_epi32(-cospi[36]);
    __m512i cospi_p28 = _mm512_set1_epi32(cospi[28]);
    __m512i cospi_p20 = _mm512_set1_epi32(cospi[20]);
    __m512i cospi_p44 = _mm512_set1_epi32(cospi[44]);
    __m512i cospi_m52 = _mm512_set1_epi32(-cospi[52]);
    __m512i cospi_p12 = _mm512_set1_epi32(cospi[12]);
    __m512i cospi_p02 = _mm512_set1_epi32(cospi[2]);
    __m512i cospi_p06 = _mm512_set1_epi32(cospi[6]);
    __m512i cospi_p62 = _mm512_set1_epi32(cospi[62]);
    __m512i cospi_m34 = _mm512_set1_epi32(-cospi[34]);
    __m512i cospi_p30 = _mm512_set1_epi32(cospi[30]);
    __m512i cospi_p18 = _mm512_set1_epi32(cospi[18]);
    __m512i cospi_p46 = _mm512_set1_epi32(cospi[46]);
    __m512i cospi_m50 = _mm512_set1_epi32(-cospi[50]);
    __m512i cospi_p14 = _mm512_set1_epi32(cospi[14]);
    __m512i cospi_p10 = _mm512_set1_epi32(cospi[10]);
    __m512i cospi_p54 = _mm512_set1_epi32(cospi[54]);
    __m512i cospi_m42 = _mm512_set1_epi32(-cospi[42]);
    __m512i cospi_p22 = _mm512_set1_epi32(cospi[22]);
    __m512i cospi_p26 = _mm512_set1_epi32(cospi[26]);
    __m512i cospi_p38 = _mm512_set1_epi32(cospi[38]);
    __m512i cospi_m58 = _mm512_set1_epi32(-cospi[58]);

    __m512i buf0[32];
    __m512i buf1[32];

    for (int32_t col = 0; col < columns; col++) {
        const __m512i* in  = &input[col];
        __m512i*       out = &output[col];

        // stage 0
        // stage 1
        buf1[0]  = _mm512_add_epi32(in[0 * stride], in[31 * stride]);
        buf1[31] = _mm512_sub_epi32(in[0 * stride], in[31 * stride]);
        buf1[1]  = _mm512_add_epi32(in[1 * stride], in[30 * stride]);
        buf1[30] = _mm512_sub_epi32(in[1 * stride], in[30 * stride]);
        buf1[2]  = _mm512_add_epi32(in[2 * stride], in[29 * stride]);
        buf1[29] = _mm512_sub_epi32(in[2 * stride], in[29 * stride]);
        buf1[3]  = _mm512_add_epi32(in[3 * stride], in[28 * stride]);
        buf1[28] = _mm512_sub_epi32(in[3 * stride], in[28 * stride]);
        buf1[4]  = _mm512_add_epi32(in[4 * stride], in[27 * stride]);
        buf1[27] = _mm512_sub_epi32(in[4 * stride], in[27 * stride]);
        buf1[5]  = _mm512_add_epi32(in[5 * stride], in[26 * stride]);
        buf1[26] = _mm512_sub_epi32(in[5 * stride], in[26 * stride]);
        buf1[6]  = _mm512_add_epi32(in[6 * stride], in[25 * stride]);
        buf1[25] = _mm512_sub_epi32(in[6 * stride], in[25 * stride]);
        buf1[7]  = _mm512_add_epi32(in[7 * stride], in[24 * stride]);
        buf1[24] = _mm512_sub_epi32(in[7 * stride], in[24 * stride]);
        buf1[8]  = _mm512_add_epi32(in[8 * stride], in[23 * stride]);
        buf1[23] = _mm512_sub_epi32(in[8 * stride], in[23 * stride]);
        buf1[9]  = _mm512_add_epi32(in[9 * stride], in[22 * stride]);
        buf1[22] = _mm512_sub_epi32(in[9 * stride], in[22 * stride]);
        buf1[10] = _mm512_add_epi32(in[10 * stride], in[21 * stride]);
        buf1[21] = _mm512_sub_epi32(in[10 * stride], in[21 * stride]);
        buf1[11] = _mm512_add_epi32(in[11 * stride], in[20 * stride]);
        buf1[20] = _mm512_sub_epi32(in[11 * stride], in[20 * stride]);
        buf1[12] = _mm512_add_epi32(in[12 * stride], in[19 * stride]);
        buf1[19] = _mm512_sub_epi32(in[12 * stride], in[19 * stride]);
        buf1[13] = _mm512_add_epi32(in[13 * stride], in[18 * stride]);
        buf1[18] = _mm512_sub_epi32(in[13 * stride], in[18 * stride]);
        buf1[14] = _mm512_add_epi32(in[14 * stride], in[17 * stride]);
        buf1[17] = _mm512_sub_epi32(in[14 * stride], in[17 * stride]);
        buf1[15] = _mm512_add_epi32(in[15 * stride], in[16 * stride]);
        buf1[16] = _mm512_sub_epi32(in[15 * stride], in[16 * stride]);

        // stage 2
        buf0[0]  = _mm512_add_epi32(buf1[0], buf1[15]);
        buf0[15] = _mm512_sub_epi32(buf1[0], buf1[15]);
        buf0[1]  = _mm512_add_epi32(buf1[1], buf1[14]);
        buf0[14] = _mm512_sub_epi32(buf1[1], buf1[14]);
        buf0[2]  = _mm512_add_epi32(buf1[2], buf1[13]);
        buf0[13] = _mm512_sub_epi32(buf1[2], buf1[13]);
        buf0[3]  = _mm512_add_epi32(buf1[3], buf1[12]);
        buf0[12] = _mm512_sub_epi32(buf1[3], buf1[12]);
        buf0[4]  = _mm512_add_epi32(buf1[4], buf1[11]);
        buf0[11] = _mm512_sub_epi32(buf1[4], buf1[11]);
        buf0[5]  = _mm512_add_epi32(buf1[5], buf1[10]);
        buf0[10] = _mm512_sub_epi32(buf1[5], buf1[10]);
        buf0[6]  = _mm512_add_epi32(buf1[6], buf1[9]);
        buf0[9]  = _mm512_sub_epi32(buf1[6], buf1[9]);
        buf0[7]  = _mm512_add_epi32(buf1[7], buf1[8]);
        buf0[8]  = _mm512_sub_epi32(buf1[7], buf1[8]);
        buf0[16] = buf1[16];
        buf0[17] = buf1[17];
        buf0[18] = buf1[18];
        buf0[19] = buf1[19];
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf1[23], buf1[24], buf0[23], buf0[24], __rounding, cos_bit);
        buf0[28] = buf1[28];
        buf0[29] = buf1[29];
        buf0[30] = buf1[30];
        buf0[31] = buf1[31];

        // stage 3
        buf1[0] = _mm512_add_epi32(buf0[0], buf0[7]);
        buf1[7] = _mm512_sub_epi32(buf0[0], buf0[7]);
        buf1[1] = _mm512_add_epi32(buf0[1], buf0[6]);
        buf1[6] = _mm512_sub_epi32(buf0[1], buf0[6]);
        buf1[2] = _mm512_add_epi32(buf0[2], buf0[5]);
        buf1[5] = _mm512_sub_epi32(buf0[2], buf0[5]);
        buf1[3] = _mm512_add_epi32(buf0[3], buf0[4]);
        buf1[4] = _mm512_sub_epi32(buf0[3], buf0[4]);
        buf1[8] = buf0[8];
        buf1[9] = buf0[9];
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf0[11], buf0[12], buf1[11], buf1[12], __rounding, cos_bit);
        buf1[14] = buf0[14];
        buf1[15] = buf0[15];
        buf1[16] = _mm512_add_epi32(buf0[16], buf0[23]);
        buf1[23] = _mm512_sub_epi32(buf0[16], buf0[23]);
        buf1[17] = _mm512_add_epi32(buf0[17], buf0[22]);
        buf1[22] = _mm512_sub_epi32(buf0[17], buf0[22]);
        buf1[18] = _mm512_add_epi32(buf0[18], buf0[21]);
        buf1[21] = _mm512_sub_epi32(buf0[18], buf0[21]);
        buf1[19] = _mm512_add_epi32(buf0[19], buf0[20]);
        buf1[20] = _mm512_sub_epi32(buf0[19], buf0[20]);
        buf1[24] = _mm512_sub_epi32(buf0[31], buf0[24]);
        buf1[31] = _mm512_add_epi32(buf0[31], buf0[24]);
        buf1[25] = _mm512_sub_epi32(buf0[30], buf0[25]);
        buf1[30] = _mm512_add_epi32(buf0[30], buf0[25]);
        buf1[26] = _mm512_sub_epi32(buf0[29], buf0[26]);
        buf1[29] = _mm512_add_epi32(buf0[29], buf0[26]);
        buf1[27] = _mm512_sub_epi32(buf0[28], buf0[27]);
        buf1[28] = _mm512_add_epi32(buf0[28], buf0[27]);

        // stage 4
        buf0[0] = _mm512_add_epi32(buf1[0], buf1[3]);
        buf0[3] = _mm512_sub_epi32(buf1[0], buf1[3]);
        buf0[1] = _mm512_add_epi32(buf1[1], buf1[2]);
        buf0[2] = _mm512_sub_epi32(buf1[1], buf1[2]);
        buf0[4] = buf1[4];
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, buf1[5], buf1[6], buf0[5], buf0[6], __rounding, cos_bit);
        buf0[7]  = buf1[7];
        buf0[8]  = _mm512_add_epi32(buf1[8], buf1[11]);
        buf0[11] = _mm512_sub_epi32(buf1[8], buf1[11]);
        buf0[9]  = _mm512_add_epi32(buf1[9], buf1[10]);
        buf0[10] = _mm512_sub_epi32(buf1[9], buf1[10]);
        buf0[12] = _mm512_sub_epi32(buf1[15], buf1[12]);
        buf0[15] = _mm512_add_epi32(buf1[15], buf1[12]);
        buf0[13] = _mm512_sub_epi32(buf1[14], buf1[13]);
        buf0[14] = _mm512_add_epi32(buf1[14], buf1[13]);
        buf0[16] = buf1[16];
        buf0[17] = buf1[17];
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, buf1[19], buf1[28], buf0[19], buf0[28], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, buf1[20], buf1[27], buf0[20], buf0[27], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        buf0[22] = buf1[22];
        buf0[23] = buf1[23];
        buf0[24] = buf1[24];
        buf0[25] = buf1[25];
        buf0[30] = buf1[30];
        buf0[31] = buf1[31];

        // stage 5
        buf1[0] = half_btf_avx512(&cospi_p32, &buf0[0], &cospi_p32, &buf0[1], &__rounding, cos_bit);
        buf1[2] = half_btf_avx512(&cospi_p48, &buf0[2], &cospi_p16, &buf0[3], &__rounding, cos_bit);
        buf1[4] = _mm512_add_epi32(buf0[4], buf0[5]);
        buf1[5] = _mm512_sub_epi32(buf0[4], buf0[5]);
        buf1[6] = _mm512_sub_epi32(buf0[7], buf0[6]);
        buf1[7] = _mm512_add_epi32(buf0[7], buf0[6]);
        buf1[8] = buf0[8];
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, buf0[9], buf0[14], buf1[9], buf1[14], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, buf0[10], buf0[13], buf1[10], buf1[13], __rounding, cos_bit);
        buf1[11] = buf0[11];
        buf1[12] = buf0[12];
        buf1[15] = buf0[15];
        buf1[16] = _mm512_add_epi32(buf0[16], buf0[19]);
        buf1[19] = _mm512_sub_epi32(buf0[16], buf0[19]);
        buf1[17] = _mm512_add_epi32(buf0[17], buf0[18]);
        buf1[18] = _mm512_sub_epi32(buf0[17], buf0[18]);
        buf1[20] = _mm512_sub_epi32(buf0[23], buf0[20]);
        buf1[23] = _mm512_add_epi32(buf0[23], buf0[20]);
        buf1[21] = _mm512_sub_epi32(buf0[22], buf0[21]);
        buf1[22] = _mm512_add_epi32(buf0[22], buf0[21]);
        buf1[24] = _mm512_add_epi32(buf0[24], buf0[27]);
        buf1[27] = _mm512_sub_epi32(buf0[24], buf0[27]);
        buf1[25] = _mm512_add_epi32(buf0[25], buf0[26]);
        buf1[26] = _mm512_sub_epi32(buf0[25], buf0[26]);
        buf1[28] = _mm512_sub_epi32(buf0[31], buf0[28]);
        buf1[31] = _mm512_add_epi32(buf0[31], buf0[28]);
        buf1[29] = _mm512_sub_epi32(buf0[30], buf0[29]);
        buf1[30] = _mm512_add_epi32(buf0[30], buf0[29]);

        // stage 6
        buf0[0]  = buf1[0];
        buf0[2]  = buf1[2];
        buf0[4]  = half_btf_avx512(&cospi_p56, &buf1[4], &cospi_p08, &buf1[7], &__rounding, cos_bit);
        buf0[6]  = half_btf_avx512(&cospi_p24, &buf1[6], &cospi_m40, &buf1[5], &__rounding, cos_bit);
        buf0[8]  = _mm512_add_epi32(buf1[8], buf1[9]);
        buf0[9]  = _mm512_sub_epi32(buf1[8], buf1[9]);
        buf0[10] = _mm512_sub_epi32(buf1[11], buf1[10]);
        buf0[11] = _mm512_add_epi32(buf1[11], buf1[10]);
        buf0[12] = _mm512_add_epi32(buf1[12], buf1[13]);
        buf0[13] = _mm512_sub_epi32(buf1[12], buf1[13]);
        buf0[14] = _mm512_sub_epi32(buf1[15], buf1[14]);
        buf0[15] = _mm512_add_epi32(buf1[15], buf1[14]);
        buf0[16] = buf1[16];
        btf_32_type0_avx512_new(cospi_m08, cospi_p56, buf1[17], buf1[30], buf0[17], buf0[30], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m56, cospi_m08, buf1[18], buf1[29], buf0[18], buf0[29], __rounding, cos_bit);
        buf0[19] = buf1[19];
        buf0[20] = buf1[20];
        btf_32_type0_avx512_new(cospi_m40, cospi_p24, buf1[21], buf1[26], buf0[21], buf0[26], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m24, cospi_m40, buf1[22], buf1[25], buf0[22], buf0[25], __rounding, cos_bit);
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
        buf1[8]  = half_btf_avx512(&cospi_p60, &buf0[8], &cospi_p04, &buf0[15], &__rounding, cos_bit);
        buf1[14] = half_btf_avx512(&cospi_p28, &buf0[14], &cospi_m36, &buf0[9], &__rounding, cos_bit);
        buf1[10] = half_btf_avx512(&cospi_p44, &buf0[10], &cospi_p20, &buf0[13], &__rounding, cos_bit);
        buf1[12] = half_btf_avx512(&cospi_p12, &buf0[12], &cospi_m52, &buf0[11], &__rounding, cos_bit);
        buf1[16] = _mm512_add_epi32(buf0[16], buf0[17]);
        buf1[17] = _mm512_sub_epi32(buf0[16], buf0[17]);
        buf1[18] = _mm512_sub_epi32(buf0[19], buf0[18]);
        buf1[19] = _mm512_add_epi32(buf0[19], buf0[18]);
        buf1[20] = _mm512_add_epi32(buf0[20], buf0[21]);
        buf1[21] = _mm512_sub_epi32(buf0[20], buf0[21]);
        buf1[22] = _mm512_sub_epi32(buf0[23], buf0[22]);
        buf1[23] = _mm512_add_epi32(buf0[23], buf0[22]);
        buf1[24] = _mm512_add_epi32(buf0[24], buf0[25]);
        buf1[25] = _mm512_sub_epi32(buf0[24], buf0[25]);
        buf1[26] = _mm512_sub_epi32(buf0[27], buf0[26]);
        buf1[27] = _mm512_add_epi32(buf0[27], buf0[26]);
        buf1[28] = _mm512_add_epi32(buf0[28], buf0[29]);
        buf1[29] = _mm512_sub_epi32(buf0[28], buf0[29]);
        buf1[30] = _mm512_sub_epi32(buf0[31], buf0[30]);
        buf1[31] = _mm512_add_epi32(buf0[31], buf0[30]);

        // stage 8
        buf0[0]  = buf1[0];
        buf0[2]  = buf1[2];
        buf0[4]  = buf1[4];
        buf0[6]  = buf1[6];
        buf0[8]  = buf1[8];
        buf0[10] = buf1[10];
        buf0[12] = buf1[12];
        buf0[14] = buf1[14];
        buf0[16] = half_btf_avx512(&cospi_p62, &buf1[16], &cospi_p02, &buf1[31], &__rounding, cos_bit);
        buf0[30] = half_btf_avx512(&cospi_p30, &buf1[30], &cospi_m34, &buf1[17], &__rounding, cos_bit);
        buf0[18] = half_btf_avx512(&cospi_p46, &buf1[18], &cospi_p18, &buf1[29], &__rounding, cos_bit);
        buf0[28] = half_btf_avx512(&cospi_p14, &buf1[28], &cospi_m50, &buf1[19], &__rounding, cos_bit);
        buf0[20] = half_btf_avx512(&cospi_p54, &buf1[20], &cospi_p10, &buf1[27], &__rounding, cos_bit);
        buf0[26] = half_btf_avx512(&cospi_p22, &buf1[26], &cospi_m42, &buf1[21], &__rounding, cos_bit);
        buf0[22] = half_btf_avx512(&cospi_p38, &buf1[22], &cospi_p26, &buf1[25], &__rounding, cos_bit);
        buf0[24] = half_btf_avx512(&cospi_p06, &buf1[24], &cospi_m58, &buf1[23], &__rounding, cos_bit);

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

static AOM_FORCE_INLINE void av1_idtx32_wxh_N2_avx512(const __m512i* input, __m512i* output, int32_t col_num,
                                                      int32_t row_num) {
    for (int32_t i = 0; i < row_num; i++) {
        for (int32_t j = 0; j < col_num / 2; j++) {
            output[i * col_num + j] = _mm512_slli_epi32(input[i * col_num + j], (uint8_t)2);
        }
    }
}

void av1_fwd_txfm2d_32x32_N2_avx512(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    DECLARE_ALIGNED(64, int32_t, txfm_buf[1024]);
    Txfm2dFlipCfg cfg;
    svt_aom_transform_config(tx_type, TX_32X32, &cfg);
    (void)bd;
    const int32_t txfm_size       = 32;
    const int8_t* shift           = cfg.shift;
    const int8_t  cos_bit_col     = cfg.cos_bit_col;
    const int8_t  cos_bit_row     = cfg.cos_bit_row;
    __m512i*      buf_512         = (__m512i*)txfm_buf;
    __m512i*      out_512         = (__m512i*)output;
    int32_t       num_per_512     = 16;
    int32_t       txfm2d_size_512 = txfm_size * txfm_size / num_per_512;
    int32_t       col_num         = txfm_size / num_per_512;

    switch (tx_type) {
    case IDTX:
        load_buffer_16xh_in_32x32_avx512(input, buf_512, stride, txfm_size / 2);
        av1_round_shift_array_wxh_N2_avx512(buf_512, out_512, col_num, txfm_size / 2, -shift[0]);
        av1_idtx32_wxh_N2_avx512(out_512, buf_512, col_num, txfm_size / 2);
        av1_round_shift_array_wxh_N2_avx512(buf_512, out_512, col_num, txfm_size / 2, -shift[1]);
        av1_idtx32_wxh_N2_avx512(out_512, buf_512, col_num, txfm_size / 2);
        av1_round_shift_array_wxh_N2_avx512(buf_512, out_512, col_num, txfm_size / 2, -shift[2]);
        clear_buffer_wxh_N2_avx512(out_512, 32, 32);
        break;
    case DCT_DCT:
        load_buffer_32x32_avx512(input, buf_512, stride);
        av1_round_shift_array_avx512(buf_512, out_512, txfm2d_size_512, -shift[0]);
        av1_fdct32_new_N2_avx512(out_512, buf_512, cos_bit_col, txfm_size, col_num);
        av1_round_shift_array_avx512(buf_512, out_512, txfm2d_size_512 / 2, -shift[1]);
        transpose_16nx16n_N2_half_avx512(txfm_size, out_512, buf_512);
        av1_fdct32_new_N2_avx512(buf_512, out_512, cos_bit_row, txfm_size / 2, col_num);
        av1_round_shift_array_wxh_N2_avx512(out_512, buf_512, col_num, txfm_size / 2, -shift[2]);
        transpose_16nx16n_N2_quad_avx512(txfm_size, buf_512, out_512);
        clear_buffer_wxh_N2_avx512(out_512, 32, 32);
        break;
    case V_DCT:
        load_buffer_16xh_in_32x32_avx512(input, buf_512, stride, txfm_size);
        av1_round_shift_array_avx512(buf_512, out_512, txfm2d_size_512, -shift[0]);
        av1_fdct32_new_N2_avx512(out_512, buf_512, cos_bit_col, txfm_size / 2, col_num);
        av1_round_shift_array_wxh_N2_avx512(buf_512, out_512, col_num, txfm_size / 2, -shift[1]);
        av1_idtx32_wxh_N2_avx512(out_512, buf_512, col_num, txfm_size / 2);
        av1_round_shift_array_wxh_N2_avx512(buf_512, out_512, col_num, txfm_size / 2, -shift[2]);
        clear_buffer_wxh_N2_avx512(out_512, 32, 32);
        break;
    case H_DCT:
        load_buffer_32xh_in_32x32_avx512(input, buf_512, stride, txfm_size / 2);
        av1_round_shift_array_avx512(buf_512, out_512, txfm2d_size_512 / 2, -shift[0]);
        av1_idtx32_new_avx512(out_512, buf_512, cos_bit_col, 1);
        av1_round_shift_array_avx512(buf_512, out_512, txfm2d_size_512 / 2, -shift[1]);
        transpose_16nx16n_N2_half_avx512(txfm_size, out_512, buf_512);
        av1_fdct32_new_N2_avx512(buf_512, out_512, cos_bit_row, txfm_size / 2, col_num);
        av1_round_shift_array_wxh_N2_avx512(out_512, buf_512, col_num, txfm_size / 2, -shift[2]);
        transpose_16nx16n_N2_quad_avx512(txfm_size, buf_512, out_512);
        clear_buffer_wxh_N2_avx512(out_512, 32, 32);
        break;
    default:
        assert(0);
    }
}

void av1_fwd_txfm2d_64x64_N2_avx512(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    __m512i       in[256];
    __m512i*      out         = (__m512i*)output;
    const int32_t txw_idx     = tx_size_wide_log2[TX_64X64] - tx_size_wide_log2[0];
    const int32_t txh_idx     = tx_size_high_log2[TX_64X64] - tx_size_high_log2[0];
    const int8_t* shift       = fwd_txfm_shift_ls[TX_64X64];
    const int32_t txfm_size   = 64;
    const int32_t num_per_512 = 16;
    int32_t       col_num     = txfm_size / num_per_512;

    switch (tx_type) {
    case IDTX:
        load_buffer_32x32_in_64x64_avx512(input, stride, out);
        fidtx64x64_N2_avx512(out, in);
        av1_round_shift_array_wxh_N2_avx512(in, out, col_num, txfm_size / 2, -shift[1]);
        /*row wise transform*/
        fidtx64x64_N2_avx512(out, in);
        av1_round_shift_array_wxh_N2_avx512(in, out, col_num, txfm_size / 2, -shift[2]);
        clear_buffer_wxh_N2_avx512(out, 64, 64);
        break;
    case DCT_DCT:
        load_buffer_64x64_avx512(input, stride, out);
        av1_fdct64_new_N2_avx512(out, in, fwd_cos_bit_col[txw_idx][txh_idx], txfm_size, col_num);
        av1_round_shift_array_avx512(in, out, 256 / 2, -shift[1]);
        transpose_16nx16n_N2_half_avx512(64, out, in);

        /*row wise transform*/
        av1_fdct64_new_N2_avx512(in, out, fwd_cos_bit_row[txw_idx][txh_idx], txfm_size / 2, col_num);
        av1_round_shift_array_wxh_N2_avx512(out, in, col_num, txfm_size / 2, -shift[2]);
        transpose_16nx16n_N2_quad_avx512(64, in, out);
        clear_buffer_wxh_N2_avx512(out, 64, 64);
        break;
    default:
        assert(0);
    }
}

void av1_fwd_txfm2d_32x64_N2_avx512(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m512i       in[128];
    __m512i*      outcoef512    = (__m512i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_32X64];
    const int32_t txw_idx       = get_txw_idx(TX_32X64);
    const int32_t txh_idx       = get_txh_idx(TX_32X64);
    const int32_t txfm_size_col = tx_size_wide[TX_32X64];
    const int32_t txfm_size_row = tx_size_high[TX_32X64];
    const int8_t  bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    const int8_t  bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 4;
    const int32_t num_col       = txfm_size_col >> 4;

    // column transform
    load_buffer_32x16n(input, in, stride, 0, 0, shift[0], txfm_size_row);
    av1_fdct64_new_N2_avx512(in, in, bitcol, txfm_size_col, num_col);
    for (int32_t i = 0; i < 4; i++) {
        col_txfm_16x16_rounding_avx512((in + i * 16), -shift[1]);
    }
    transpose_16nx16m_N2_half_avx512(in, outcoef512, txfm_size_col, txfm_size_row);

    // row transform
    av1_fdct32_new_N2_avx512(outcoef512, in, bitrow, txfm_size_row / 2, num_row);
    transpose_16nx16m_N2_quad_avx512(in, outcoef512, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_wxh_N2_avx512(outcoef512, outcoef512, num_col, txfm_size_row / 2, -shift[2], 5793);
    clear_buffer_wxh_N2_avx512(outcoef512, 32, 64);
    (void)bd;
}

void av1_fwd_txfm2d_64x32_N2_avx512(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)tx_type;
    __m512i       in[128];
    __m512i*      outcoef512    = (__m512i*)output;
    const int8_t* shift         = fwd_txfm_shift_ls[TX_64X32];
    const int32_t txw_idx       = get_txw_idx(TX_64X32);
    const int32_t txh_idx       = get_txh_idx(TX_64X32);
    const int32_t txfm_size_col = tx_size_wide[TX_64X32];
    const int32_t txfm_size_row = tx_size_high[TX_64X32];
    const int8_t  bitcol        = fwd_cos_bit_col[txw_idx][txh_idx];
    const int8_t  bitrow        = fwd_cos_bit_row[txw_idx][txh_idx];
    const int32_t num_row       = txfm_size_row >> 4;
    const int32_t num_col       = txfm_size_col >> 4;

    // column transform
    for (int32_t i = 0; i < 32; i++) {
        load_buffer_32_avx512(input + 0 + i * stride, in + 0 + i * 4, 16, 0, 0, shift[0]);
        load_buffer_32_avx512(input + 32 + i * stride, in + 2 + i * 4, 16, 0, 0, shift[0]);
    }
    av1_fdct32_new_N2_avx512(in, in, bitcol, txfm_size_col, num_col);
    for (int32_t i = 0; i < 4; i++) {
        col_txfm_16x16_rounding_avx512((in + i * 16), -shift[1]);
    }
    transpose_16nx16m_N2_half_avx512(in, outcoef512, txfm_size_col, txfm_size_row);

    // row transform
    av1_fdct64_new_N2_avx512(outcoef512, in, bitrow, txfm_size_row / 2, num_row);
    transpose_16nx16m_N2_quad_avx512(in, outcoef512, txfm_size_row, txfm_size_col);
    av1_round_shift_rect_array_wxh_N2_avx512(outcoef512, outcoef512, num_col, txfm_size_row / 2, -shift[2], 5793);
    clear_buffer_wxh_N2_avx512(outcoef512, 64, 32);
    (void)bd;
}

/******************************END*N2***************************************/

/********************************N4****************************************/

static AOM_FORCE_INLINE void clear_buffer_64x64_N4_avx512(__m512i* buff) {
    const __m512i zero512 = _mm512_setzero_si512();
    int32_t       i;

    for (i = 0; i < 16; i++) {
        buff[i * 4 + 1] = zero512;
        buff[i * 4 + 2] = zero512;
        buff[i * 4 + 3] = zero512;
    }

    for (i = 16; i < 64; i++) {
        buff[i * 4 + 0] = zero512;
        buff[i * 4 + 1] = zero512;
        buff[i * 4 + 2] = zero512;
        buff[i * 4 + 3] = zero512;
    }
}

static AOM_FORCE_INLINE void av1_round_shift_array_64x64_N4_avx512(__m512i* input, __m512i* output, const int8_t bit) {
    if (bit > 0) {
        __m512i round = _mm512_set1_epi32(1 << (bit - 1));
        int32_t i;
        for (i = 0; i < 16; i++) {
            output[i * 4] = _mm512_srai_epi32(_mm512_add_epi32(input[i * 4], round), (uint8_t)bit);
        }
    } else {
        int32_t i;
        for (i = 0; i < 16; i++) {
            output[i * 4] = _mm512_slli_epi32(input[i * 4], (uint8_t)(-bit));
        }
    }
}

static AOM_FORCE_INLINE void load_buffer_16x16_in_64x64_avx512(const int16_t* input, int32_t stride, __m512i* output) {
    __m256i x0;
    __m512i v0;
    int32_t i;

    for (i = 0; i < 16; ++i) {
        x0 = _mm256_loadu_si256((const __m256i*)(input));
        v0 = _mm512_cvtepi16_epi32(x0);
        _mm512_storeu_si512(output, v0);

        input += stride;
        output += 4;
    }
}

static void av1_fdct64_new_N4_avx512(const __m512i* input, __m512i* output, const int8_t cos_bit, const int32_t col_num,
                                     const int32_t stride) {
    const int32_t* cospi      = cospi_arr(cos_bit);
    const __m512i  __rounding = _mm512_set1_epi32(1 << (cos_bit - 1));
    const int32_t  columns    = col_num >> 4;

    __m512i cospi_m32 = _mm512_set1_epi32(-cospi[32]);
    __m512i cospi_p32 = _mm512_set1_epi32(cospi[32]);
    __m512i cospi_m16 = _mm512_set1_epi32(-cospi[16]);
    __m512i cospi_p48 = _mm512_set1_epi32(cospi[48]);
    __m512i cospi_m48 = _mm512_set1_epi32(-cospi[48]);
    __m512i cospi_m08 = _mm512_set1_epi32(-cospi[8]);
    __m512i cospi_p56 = _mm512_set1_epi32(cospi[56]);
    __m512i cospi_m56 = _mm512_set1_epi32(-cospi[56]);
    __m512i cospi_m40 = _mm512_set1_epi32(-cospi[40]);
    __m512i cospi_p24 = _mm512_set1_epi32(cospi[24]);
    __m512i cospi_m24 = _mm512_set1_epi32(-cospi[24]);
    __m512i cospi_p08 = _mm512_set1_epi32(cospi[8]);
    __m512i cospi_p60 = _mm512_set1_epi32(cospi[60]);
    __m512i cospi_p04 = _mm512_set1_epi32(cospi[4]);
    __m512i cospi_p28 = _mm512_set1_epi32(cospi[28]);
    __m512i cospi_p44 = _mm512_set1_epi32(cospi[44]);
    __m512i cospi_p12 = _mm512_set1_epi32(cospi[12]);
    __m512i cospi_m04 = _mm512_set1_epi32(-cospi[4]);
    __m512i cospi_m60 = _mm512_set1_epi32(-cospi[60]);
    __m512i cospi_m36 = _mm512_set1_epi32(-cospi[36]);
    __m512i cospi_m28 = _mm512_set1_epi32(-cospi[28]);
    __m512i cospi_m20 = _mm512_set1_epi32(-cospi[20]);
    __m512i cospi_m44 = _mm512_set1_epi32(-cospi[44]);
    __m512i cospi_m52 = _mm512_set1_epi32(-cospi[52]);
    __m512i cospi_m12 = _mm512_set1_epi32(-cospi[12]);
    __m512i cospi_p62 = _mm512_set1_epi32(cospi[62]);
    __m512i cospi_p02 = _mm512_set1_epi32(cospi[2]);
    __m512i cospi_p14 = _mm512_set1_epi32(cospi[14]);
    __m512i cospi_m50 = _mm512_set1_epi32(-cospi[50]);
    __m512i cospi_p54 = _mm512_set1_epi32(cospi[54]);
    __m512i cospi_p10 = _mm512_set1_epi32(cospi[10]);
    __m512i cospi_p06 = _mm512_set1_epi32(cospi[6]);
    __m512i cospi_m58 = _mm512_set1_epi32(-cospi[58]);
    __m512i cospi_p63 = _mm512_set1_epi32(cospi[63]);
    __m512i cospi_p01 = _mm512_set1_epi32(cospi[1]);
    __m512i cospi_p15 = _mm512_set1_epi32(cospi[15]);
    __m512i cospi_m49 = _mm512_set1_epi32(-cospi[49]);
    __m512i cospi_p55 = _mm512_set1_epi32(cospi[55]);
    __m512i cospi_p09 = _mm512_set1_epi32(cospi[9]);
    __m512i cospi_p07 = _mm512_set1_epi32(cospi[7]);
    __m512i cospi_m57 = _mm512_set1_epi32(-cospi[57]);
    __m512i cospi_p59 = _mm512_set1_epi32(cospi[59]);
    __m512i cospi_p05 = _mm512_set1_epi32(cospi[5]);
    __m512i cospi_p11 = _mm512_set1_epi32(cospi[11]);
    __m512i cospi_m53 = _mm512_set1_epi32(-cospi[53]);
    __m512i cospi_p51 = _mm512_set1_epi32(cospi[51]);
    __m512i cospi_p13 = _mm512_set1_epi32(cospi[13]);
    __m512i cospi_p03 = _mm512_set1_epi32(cospi[3]);
    __m512i cospi_m61 = _mm512_set1_epi32(-cospi[61]);

    for (int32_t col = 0; col < columns; col++) {
        const __m512i* in  = &input[col];
        __m512i*       out = &output[col];

        // stage 1
        __m512i x1[64];
        x1[0]  = _mm512_add_epi32(in[0 * stride], in[63 * stride]);
        x1[63] = _mm512_sub_epi32(in[0 * stride], in[63 * stride]);
        x1[1]  = _mm512_add_epi32(in[1 * stride], in[62 * stride]);
        x1[62] = _mm512_sub_epi32(in[1 * stride], in[62 * stride]);
        x1[2]  = _mm512_add_epi32(in[2 * stride], in[61 * stride]);
        x1[61] = _mm512_sub_epi32(in[2 * stride], in[61 * stride]);
        x1[3]  = _mm512_add_epi32(in[3 * stride], in[60 * stride]);
        x1[60] = _mm512_sub_epi32(in[3 * stride], in[60 * stride]);
        x1[4]  = _mm512_add_epi32(in[4 * stride], in[59 * stride]);
        x1[59] = _mm512_sub_epi32(in[4 * stride], in[59 * stride]);
        x1[5]  = _mm512_add_epi32(in[5 * stride], in[58 * stride]);
        x1[58] = _mm512_sub_epi32(in[5 * stride], in[58 * stride]);
        x1[6]  = _mm512_add_epi32(in[6 * stride], in[57 * stride]);
        x1[57] = _mm512_sub_epi32(in[6 * stride], in[57 * stride]);
        x1[7]  = _mm512_add_epi32(in[7 * stride], in[56 * stride]);
        x1[56] = _mm512_sub_epi32(in[7 * stride], in[56 * stride]);
        x1[8]  = _mm512_add_epi32(in[8 * stride], in[55 * stride]);
        x1[55] = _mm512_sub_epi32(in[8 * stride], in[55 * stride]);
        x1[9]  = _mm512_add_epi32(in[9 * stride], in[54 * stride]);
        x1[54] = _mm512_sub_epi32(in[9 * stride], in[54 * stride]);
        x1[10] = _mm512_add_epi32(in[10 * stride], in[53 * stride]);
        x1[53] = _mm512_sub_epi32(in[10 * stride], in[53 * stride]);
        x1[11] = _mm512_add_epi32(in[11 * stride], in[52 * stride]);
        x1[52] = _mm512_sub_epi32(in[11 * stride], in[52 * stride]);
        x1[12] = _mm512_add_epi32(in[12 * stride], in[51 * stride]);
        x1[51] = _mm512_sub_epi32(in[12 * stride], in[51 * stride]);
        x1[13] = _mm512_add_epi32(in[13 * stride], in[50 * stride]);
        x1[50] = _mm512_sub_epi32(in[13 * stride], in[50 * stride]);
        x1[14] = _mm512_add_epi32(in[14 * stride], in[49 * stride]);
        x1[49] = _mm512_sub_epi32(in[14 * stride], in[49 * stride]);
        x1[15] = _mm512_add_epi32(in[15 * stride], in[48 * stride]);
        x1[48] = _mm512_sub_epi32(in[15 * stride], in[48 * stride]);
        x1[16] = _mm512_add_epi32(in[16 * stride], in[47 * stride]);
        x1[47] = _mm512_sub_epi32(in[16 * stride], in[47 * stride]);
        x1[17] = _mm512_add_epi32(in[17 * stride], in[46 * stride]);
        x1[46] = _mm512_sub_epi32(in[17 * stride], in[46 * stride]);
        x1[18] = _mm512_add_epi32(in[18 * stride], in[45 * stride]);
        x1[45] = _mm512_sub_epi32(in[18 * stride], in[45 * stride]);
        x1[19] = _mm512_add_epi32(in[19 * stride], in[44 * stride]);
        x1[44] = _mm512_sub_epi32(in[19 * stride], in[44 * stride]);
        x1[20] = _mm512_add_epi32(in[20 * stride], in[43 * stride]);
        x1[43] = _mm512_sub_epi32(in[20 * stride], in[43 * stride]);
        x1[21] = _mm512_add_epi32(in[21 * stride], in[42 * stride]);
        x1[42] = _mm512_sub_epi32(in[21 * stride], in[42 * stride]);
        x1[22] = _mm512_add_epi32(in[22 * stride], in[41 * stride]);
        x1[41] = _mm512_sub_epi32(in[22 * stride], in[41 * stride]);
        x1[23] = _mm512_add_epi32(in[23 * stride], in[40 * stride]);
        x1[40] = _mm512_sub_epi32(in[23 * stride], in[40 * stride]);
        x1[24] = _mm512_add_epi32(in[24 * stride], in[39 * stride]);
        x1[39] = _mm512_sub_epi32(in[24 * stride], in[39 * stride]);
        x1[25] = _mm512_add_epi32(in[25 * stride], in[38 * stride]);
        x1[38] = _mm512_sub_epi32(in[25 * stride], in[38 * stride]);
        x1[26] = _mm512_add_epi32(in[26 * stride], in[37 * stride]);
        x1[37] = _mm512_sub_epi32(in[26 * stride], in[37 * stride]);
        x1[27] = _mm512_add_epi32(in[27 * stride], in[36 * stride]);
        x1[36] = _mm512_sub_epi32(in[27 * stride], in[36 * stride]);
        x1[28] = _mm512_add_epi32(in[28 * stride], in[35 * stride]);
        x1[35] = _mm512_sub_epi32(in[28 * stride], in[35 * stride]);
        x1[29] = _mm512_add_epi32(in[29 * stride], in[34 * stride]);
        x1[34] = _mm512_sub_epi32(in[29 * stride], in[34 * stride]);
        x1[30] = _mm512_add_epi32(in[30 * stride], in[33 * stride]);
        x1[33] = _mm512_sub_epi32(in[30 * stride], in[33 * stride]);
        x1[31] = _mm512_add_epi32(in[31 * stride], in[32 * stride]);
        x1[32] = _mm512_sub_epi32(in[31 * stride], in[32 * stride]);

        // stage 2
        __m512i x2[64];
        x2[0]  = _mm512_add_epi32(x1[0], x1[31]);
        x2[31] = _mm512_sub_epi32(x1[0], x1[31]);
        x2[1]  = _mm512_add_epi32(x1[1], x1[30]);
        x2[30] = _mm512_sub_epi32(x1[1], x1[30]);
        x2[2]  = _mm512_add_epi32(x1[2], x1[29]);
        x2[29] = _mm512_sub_epi32(x1[2], x1[29]);
        x2[3]  = _mm512_add_epi32(x1[3], x1[28]);
        x2[28] = _mm512_sub_epi32(x1[3], x1[28]);
        x2[4]  = _mm512_add_epi32(x1[4], x1[27]);
        x2[27] = _mm512_sub_epi32(x1[4], x1[27]);
        x2[5]  = _mm512_add_epi32(x1[5], x1[26]);
        x2[26] = _mm512_sub_epi32(x1[5], x1[26]);
        x2[6]  = _mm512_add_epi32(x1[6], x1[25]);
        x2[25] = _mm512_sub_epi32(x1[6], x1[25]);
        x2[7]  = _mm512_add_epi32(x1[7], x1[24]);
        x2[24] = _mm512_sub_epi32(x1[7], x1[24]);
        x2[8]  = _mm512_add_epi32(x1[8], x1[23]);
        x2[23] = _mm512_sub_epi32(x1[8], x1[23]);
        x2[9]  = _mm512_add_epi32(x1[9], x1[22]);
        x2[22] = _mm512_sub_epi32(x1[9], x1[22]);
        x2[10] = _mm512_add_epi32(x1[10], x1[21]);
        x2[21] = _mm512_sub_epi32(x1[10], x1[21]);
        x2[11] = _mm512_add_epi32(x1[11], x1[20]);
        x2[20] = _mm512_sub_epi32(x1[11], x1[20]);
        x2[12] = _mm512_add_epi32(x1[12], x1[19]);
        x2[19] = _mm512_sub_epi32(x1[12], x1[19]);
        x2[13] = _mm512_add_epi32(x1[13], x1[18]);
        x2[18] = _mm512_sub_epi32(x1[13], x1[18]);
        x2[14] = _mm512_add_epi32(x1[14], x1[17]);
        x2[17] = _mm512_sub_epi32(x1[14], x1[17]);
        x2[15] = _mm512_add_epi32(x1[15], x1[16]);
        x2[16] = _mm512_sub_epi32(x1[15], x1[16]);
        x2[32] = x1[32];
        x2[33] = x1[33];
        x2[34] = x1[34];
        x2[35] = x1[35];
        x2[36] = x1[36];
        x2[37] = x1[37];
        x2[38] = x1[38];
        x2[39] = x1[39];
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[40], x1[55], x2[40], x2[55], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[41], x1[54], x2[41], x2[54], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[42], x1[53], x2[42], x2[53], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[43], x1[52], x2[43], x2[52], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[44], x1[51], x2[44], x2[51], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[45], x1[50], x2[45], x2[50], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[46], x1[49], x2[46], x2[49], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x1[47], x1[48], x2[47], x2[48], __rounding, cos_bit);
        x2[56] = x1[56];
        x2[57] = x1[57];
        x2[58] = x1[58];
        x2[59] = x1[59];
        x2[60] = x1[60];
        x2[61] = x1[61];
        x2[62] = x1[62];
        x2[63] = x1[63];

        // stage 3
        __m512i x3[64];
        x3[0]  = _mm512_add_epi32(x2[0], x2[15]);
        x3[15] = _mm512_sub_epi32(x2[0], x2[15]);
        x3[1]  = _mm512_add_epi32(x2[1], x2[14]);
        x3[14] = _mm512_sub_epi32(x2[1], x2[14]);
        x3[2]  = _mm512_add_epi32(x2[2], x2[13]);
        x3[13] = _mm512_sub_epi32(x2[2], x2[13]);
        x3[3]  = _mm512_add_epi32(x2[3], x2[12]);
        x3[12] = _mm512_sub_epi32(x2[3], x2[12]);
        x3[4]  = _mm512_add_epi32(x2[4], x2[11]);
        x3[11] = _mm512_sub_epi32(x2[4], x2[11]);
        x3[5]  = _mm512_add_epi32(x2[5], x2[10]);
        x3[10] = _mm512_sub_epi32(x2[5], x2[10]);
        x3[6]  = _mm512_add_epi32(x2[6], x2[9]);
        x3[9]  = _mm512_sub_epi32(x2[6], x2[9]);
        x3[7]  = _mm512_add_epi32(x2[7], x2[8]);
        x3[8]  = _mm512_sub_epi32(x2[7], x2[8]);
        x3[16] = x2[16];
        x3[17] = x2[17];
        x3[18] = x2[18];
        x3[19] = x2[19];
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x2[20], x2[27], x3[20], x3[27], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x2[21], x2[26], x3[21], x3[26], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x2[22], x2[25], x3[22], x3[25], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x2[23], x2[24], x3[23], x3[24], __rounding, cos_bit);
        x3[28] = x2[28];
        x3[29] = x2[29];
        x3[30] = x2[30];
        x3[31] = x2[31];
        x3[32] = _mm512_add_epi32(x2[32], x2[47]);
        x3[47] = _mm512_sub_epi32(x2[32], x2[47]);
        x3[33] = _mm512_add_epi32(x2[33], x2[46]);
        x3[46] = _mm512_sub_epi32(x2[33], x2[46]);
        x3[34] = _mm512_add_epi32(x2[34], x2[45]);
        x3[45] = _mm512_sub_epi32(x2[34], x2[45]);
        x3[35] = _mm512_add_epi32(x2[35], x2[44]);
        x3[44] = _mm512_sub_epi32(x2[35], x2[44]);
        x3[36] = _mm512_add_epi32(x2[36], x2[43]);
        x3[43] = _mm512_sub_epi32(x2[36], x2[43]);
        x3[37] = _mm512_add_epi32(x2[37], x2[42]);
        x3[42] = _mm512_sub_epi32(x2[37], x2[42]);
        x3[38] = _mm512_add_epi32(x2[38], x2[41]);
        x3[41] = _mm512_sub_epi32(x2[38], x2[41]);
        x3[39] = _mm512_add_epi32(x2[39], x2[40]);
        x3[40] = _mm512_sub_epi32(x2[39], x2[40]);
        x3[48] = _mm512_sub_epi32(x2[63], x2[48]);
        x3[63] = _mm512_add_epi32(x2[63], x2[48]);
        x3[49] = _mm512_sub_epi32(x2[62], x2[49]);
        x3[62] = _mm512_add_epi32(x2[62], x2[49]);
        x3[50] = _mm512_sub_epi32(x2[61], x2[50]);
        x3[61] = _mm512_add_epi32(x2[61], x2[50]);
        x3[51] = _mm512_sub_epi32(x2[60], x2[51]);
        x3[60] = _mm512_add_epi32(x2[60], x2[51]);
        x3[52] = _mm512_sub_epi32(x2[59], x2[52]);
        x3[59] = _mm512_add_epi32(x2[59], x2[52]);
        x3[53] = _mm512_sub_epi32(x2[58], x2[53]);
        x3[58] = _mm512_add_epi32(x2[58], x2[53]);
        x3[54] = _mm512_sub_epi32(x2[57], x2[54]);
        x3[57] = _mm512_add_epi32(x2[57], x2[54]);
        x3[55] = _mm512_sub_epi32(x2[56], x2[55]);
        x3[56] = _mm512_add_epi32(x2[56], x2[55]);

        // stage 4
        __m512i x4[64];
        x4[0] = _mm512_add_epi32(x3[0], x3[7]);
        x4[7] = _mm512_sub_epi32(x3[0], x3[7]);
        x4[1] = _mm512_add_epi32(x3[1], x3[6]);
        x4[6] = _mm512_sub_epi32(x3[1], x3[6]);
        x4[2] = _mm512_add_epi32(x3[2], x3[5]);
        x4[5] = _mm512_sub_epi32(x3[2], x3[5]);
        x4[3] = _mm512_add_epi32(x3[3], x3[4]);
        x4[4] = _mm512_sub_epi32(x3[3], x3[4]);
        x4[8] = x3[8];
        x4[9] = x3[9];
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x3[10], x3[13], x4[10], x4[13], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x3[11], x3[12], x4[11], x4[12], __rounding, cos_bit);
        x4[14] = x3[14];
        x4[15] = x3[15];
        x4[16] = _mm512_add_epi32(x3[16], x3[23]);
        x4[23] = _mm512_sub_epi32(x3[16], x3[23]);
        x4[17] = _mm512_add_epi32(x3[17], x3[22]);
        x4[22] = _mm512_sub_epi32(x3[17], x3[22]);
        x4[18] = _mm512_add_epi32(x3[18], x3[21]);
        x4[21] = _mm512_sub_epi32(x3[18], x3[21]);
        x4[19] = _mm512_add_epi32(x3[19], x3[20]);
        x4[20] = _mm512_sub_epi32(x3[19], x3[20]);
        x4[24] = _mm512_sub_epi32(x3[31], x3[24]);
        x4[31] = _mm512_add_epi32(x3[31], x3[24]);
        x4[25] = _mm512_sub_epi32(x3[30], x3[25]);
        x4[30] = _mm512_add_epi32(x3[30], x3[25]);
        x4[26] = _mm512_sub_epi32(x3[29], x3[26]);
        x4[29] = _mm512_add_epi32(x3[29], x3[26]);
        x4[27] = _mm512_sub_epi32(x3[28], x3[27]);
        x4[28] = _mm512_add_epi32(x3[28], x3[27]);
        x4[32] = x3[32];
        x4[33] = x3[33];
        x4[34] = x3[34];
        x4[35] = x3[35];
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x3[36], x3[59], x4[36], x4[59], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x3[37], x3[58], x4[37], x4[58], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x3[38], x3[57], x4[38], x4[57], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x3[39], x3[56], x4[39], x4[56], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x3[40], x3[55], x4[40], x4[55], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x3[41], x3[54], x4[41], x4[54], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x3[42], x3[53], x4[42], x4[53], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x3[43], x3[52], x4[43], x4[52], __rounding, cos_bit);
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
        __m512i x5[64];
        x5[0] = _mm512_add_epi32(x4[0], x4[3]);
        x5[1] = _mm512_add_epi32(x4[1], x4[2]);
        x5[4] = x4[4];
        btf_32_type0_avx512_new(cospi_m32, cospi_p32, x4[5], x4[6], x5[5], x5[6], __rounding, cos_bit);
        x5[7]  = x4[7];
        x5[8]  = _mm512_add_epi32(x4[8], x4[11]);
        x5[11] = _mm512_sub_epi32(x4[8], x4[11]);
        x5[9]  = _mm512_add_epi32(x4[9], x4[10]);
        x5[10] = _mm512_sub_epi32(x4[9], x4[10]);
        x5[12] = _mm512_sub_epi32(x4[15], x4[12]);
        x5[15] = _mm512_add_epi32(x4[15], x4[12]);
        x5[13] = _mm512_sub_epi32(x4[14], x4[13]);
        x5[14] = _mm512_add_epi32(x4[14], x4[13]);
        x5[16] = x4[16];
        x5[17] = x4[17];
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x4[18], x4[29], x5[18], x5[29], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x4[19], x4[28], x5[19], x5[28], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x4[20], x4[27], x5[20], x5[27], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x4[21], x4[26], x5[21], x5[26], __rounding, cos_bit);
        x5[22] = x4[22];
        x5[23] = x4[23];
        x5[24] = x4[24];
        x5[25] = x4[25];
        x5[30] = x4[30];
        x5[31] = x4[31];
        x5[32] = _mm512_add_epi32(x4[32], x4[39]);
        x5[39] = _mm512_sub_epi32(x4[32], x4[39]);
        x5[33] = _mm512_add_epi32(x4[33], x4[38]);
        x5[38] = _mm512_sub_epi32(x4[33], x4[38]);
        x5[34] = _mm512_add_epi32(x4[34], x4[37]);
        x5[37] = _mm512_sub_epi32(x4[34], x4[37]);
        x5[35] = _mm512_add_epi32(x4[35], x4[36]);
        x5[36] = _mm512_sub_epi32(x4[35], x4[36]);
        x5[40] = _mm512_sub_epi32(x4[47], x4[40]);
        x5[47] = _mm512_add_epi32(x4[47], x4[40]);
        x5[41] = _mm512_sub_epi32(x4[46], x4[41]);
        x5[46] = _mm512_add_epi32(x4[46], x4[41]);
        x5[42] = _mm512_sub_epi32(x4[45], x4[42]);
        x5[45] = _mm512_add_epi32(x4[45], x4[42]);
        x5[43] = _mm512_sub_epi32(x4[44], x4[43]);
        x5[44] = _mm512_add_epi32(x4[44], x4[43]);
        x5[48] = _mm512_add_epi32(x4[48], x4[55]);
        x5[55] = _mm512_sub_epi32(x4[48], x4[55]);
        x5[49] = _mm512_add_epi32(x4[49], x4[54]);
        x5[54] = _mm512_sub_epi32(x4[49], x4[54]);
        x5[50] = _mm512_add_epi32(x4[50], x4[53]);
        x5[53] = _mm512_sub_epi32(x4[50], x4[53]);
        x5[51] = _mm512_add_epi32(x4[51], x4[52]);
        x5[52] = _mm512_sub_epi32(x4[51], x4[52]);
        x5[56] = _mm512_sub_epi32(x4[63], x4[56]);
        x5[63] = _mm512_add_epi32(x4[63], x4[56]);
        x5[57] = _mm512_sub_epi32(x4[62], x4[57]);
        x5[62] = _mm512_add_epi32(x4[62], x4[57]);
        x5[58] = _mm512_sub_epi32(x4[61], x4[58]);
        x5[61] = _mm512_add_epi32(x4[61], x4[58]);
        x5[59] = _mm512_sub_epi32(x4[60], x4[59]);
        x5[60] = _mm512_add_epi32(x4[60], x4[59]);

        // stage 6
        __m512i x6[64];
        x6[0] = half_btf_avx512(&cospi_p32, &x5[0], &cospi_p32, &x5[1], &__rounding, cos_bit);
        x6[4] = _mm512_add_epi32(x5[4], x5[5]);
        x6[7] = _mm512_add_epi32(x5[7], x5[6]);
        x6[8] = x5[8];
        btf_32_type0_avx512_new(cospi_m16, cospi_p48, x5[9], x5[14], x6[9], x6[14], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m48, cospi_m16, x5[10], x5[13], x6[10], x6[13], __rounding, cos_bit);
        x6[11] = x5[11];
        x6[12] = x5[12];
        x6[15] = x5[15];
        x6[16] = _mm512_add_epi32(x5[16], x5[19]);
        x6[19] = _mm512_sub_epi32(x5[16], x5[19]);
        x6[17] = _mm512_add_epi32(x5[17], x5[18]);
        x6[18] = _mm512_sub_epi32(x5[17], x5[18]);
        x6[20] = _mm512_sub_epi32(x5[23], x5[20]);
        x6[23] = _mm512_add_epi32(x5[23], x5[20]);
        x6[21] = _mm512_sub_epi32(x5[22], x5[21]);
        x6[22] = _mm512_add_epi32(x5[22], x5[21]);
        x6[24] = _mm512_add_epi32(x5[24], x5[27]);
        x6[27] = _mm512_sub_epi32(x5[24], x5[27]);
        x6[25] = _mm512_add_epi32(x5[25], x5[26]);
        x6[26] = _mm512_sub_epi32(x5[25], x5[26]);
        x6[28] = _mm512_sub_epi32(x5[31], x5[28]);
        x6[31] = _mm512_add_epi32(x5[31], x5[28]);
        x6[29] = _mm512_sub_epi32(x5[30], x5[29]);
        x6[30] = _mm512_add_epi32(x5[30], x5[29]);
        x6[32] = x5[32];
        x6[33] = x5[33];
        btf_32_type0_avx512_new(cospi_m08, cospi_p56, x5[34], x5[61], x6[34], x6[61], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m08, cospi_p56, x5[35], x5[60], x6[35], x6[60], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m56, cospi_m08, x5[36], x5[59], x6[36], x6[59], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m56, cospi_m08, x5[37], x5[58], x6[37], x6[58], __rounding, cos_bit);
        x6[38] = x5[38];
        x6[39] = x5[39];
        x6[40] = x5[40];
        x6[41] = x5[41];
        btf_32_type0_avx512_new(cospi_m40, cospi_p24, x5[42], x5[53], x6[42], x6[53], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m40, cospi_p24, x5[43], x5[52], x6[43], x6[52], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m24, cospi_m40, x5[44], x5[51], x6[44], x6[51], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m24, cospi_m40, x5[45], x5[50], x6[45], x6[50], __rounding, cos_bit);
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
        __m512i x7[64];
        x7[0]  = x6[0];
        x7[4]  = half_btf_avx512(&cospi_p56, &x6[4], &cospi_p08, &x6[7], &__rounding, cos_bit);
        x7[8]  = _mm512_add_epi32(x6[8], x6[9]);
        x7[11] = _mm512_add_epi32(x6[11], x6[10]);
        x7[12] = _mm512_add_epi32(x6[12], x6[13]);
        x7[15] = _mm512_add_epi32(x6[15], x6[14]);
        x7[16] = x6[16];
        btf_32_type0_avx512_new(cospi_m08, cospi_p56, x6[17], x6[30], x7[17], x7[30], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m56, cospi_m08, x6[18], x6[29], x7[18], x7[29], __rounding, cos_bit);
        x7[19] = x6[19];
        x7[20] = x6[20];
        btf_32_type0_avx512_new(cospi_m40, cospi_p24, x6[21], x6[26], x7[21], x7[26], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m24, cospi_m40, x6[22], x6[25], x7[22], x7[25], __rounding, cos_bit);
        x7[23] = x6[23];
        x7[24] = x6[24];
        x7[27] = x6[27];
        x7[28] = x6[28];
        x7[31] = x6[31];
        x7[32] = _mm512_add_epi32(x6[32], x6[35]);
        x7[35] = _mm512_sub_epi32(x6[32], x6[35]);
        x7[33] = _mm512_add_epi32(x6[33], x6[34]);
        x7[34] = _mm512_sub_epi32(x6[33], x6[34]);
        x7[36] = _mm512_sub_epi32(x6[39], x6[36]);
        x7[39] = _mm512_add_epi32(x6[39], x6[36]);
        x7[37] = _mm512_sub_epi32(x6[38], x6[37]);
        x7[38] = _mm512_add_epi32(x6[38], x6[37]);
        x7[40] = _mm512_add_epi32(x6[40], x6[43]);
        x7[43] = _mm512_sub_epi32(x6[40], x6[43]);
        x7[41] = _mm512_add_epi32(x6[41], x6[42]);
        x7[42] = _mm512_sub_epi32(x6[41], x6[42]);
        x7[44] = _mm512_sub_epi32(x6[47], x6[44]);
        x7[47] = _mm512_add_epi32(x6[47], x6[44]);
        x7[45] = _mm512_sub_epi32(x6[46], x6[45]);
        x7[46] = _mm512_add_epi32(x6[46], x6[45]);
        x7[48] = _mm512_add_epi32(x6[48], x6[51]);
        x7[51] = _mm512_sub_epi32(x6[48], x6[51]);
        x7[49] = _mm512_add_epi32(x6[49], x6[50]);
        x7[50] = _mm512_sub_epi32(x6[49], x6[50]);
        x7[52] = _mm512_sub_epi32(x6[55], x6[52]);
        x7[55] = _mm512_add_epi32(x6[55], x6[52]);
        x7[53] = _mm512_sub_epi32(x6[54], x6[53]);
        x7[54] = _mm512_add_epi32(x6[54], x6[53]);
        x7[56] = _mm512_add_epi32(x6[56], x6[59]);
        x7[59] = _mm512_sub_epi32(x6[56], x6[59]);
        x7[57] = _mm512_add_epi32(x6[57], x6[58]);
        x7[58] = _mm512_sub_epi32(x6[57], x6[58]);
        x7[60] = _mm512_sub_epi32(x6[63], x6[60]);
        x7[63] = _mm512_add_epi32(x6[63], x6[60]);
        x7[61] = _mm512_sub_epi32(x6[62], x6[61]);
        x7[62] = _mm512_add_epi32(x6[62], x6[61]);

        // stage 8
        __m512i x8[64];
        out[0 * stride]  = x7[0];
        out[8 * stride]  = x7[4];
        out[4 * stride]  = half_btf_avx512(&cospi_p60, &x7[8], &cospi_p04, &x7[15], &__rounding, cos_bit);
        out[12 * stride] = half_btf_avx512(&cospi_p12, &x7[12], &cospi_m52, &x7[11], &__rounding, cos_bit);
        x8[16]           = _mm512_add_epi32(x7[16], x7[17]);
        x8[19]           = _mm512_add_epi32(x7[19], x7[18]);
        x8[20]           = _mm512_add_epi32(x7[20], x7[21]);
        x8[23]           = _mm512_add_epi32(x7[23], x7[22]);
        x8[24]           = _mm512_add_epi32(x7[24], x7[25]);
        x8[27]           = _mm512_add_epi32(x7[27], x7[26]);
        x8[28]           = _mm512_add_epi32(x7[28], x7[29]);
        x8[31]           = _mm512_add_epi32(x7[31], x7[30]);
        x8[32]           = x7[32];
        btf_32_type0_avx512_new(cospi_m04, cospi_p60, x7[33], x7[62], x8[33], x8[62], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m60, cospi_m04, x7[34], x7[61], x8[34], x8[61], __rounding, cos_bit);
        x8[35] = x7[35];
        x8[36] = x7[36];
        btf_32_type0_avx512_new(cospi_m36, cospi_p28, x7[37], x7[58], x8[37], x8[58], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m28, cospi_m36, x7[38], x7[57], x8[38], x8[57], __rounding, cos_bit);
        x8[39] = x7[39];
        x8[40] = x7[40];
        btf_32_type0_avx512_new(cospi_m20, cospi_p44, x7[41], x7[54], x8[41], x8[54], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m44, cospi_m20, x7[42], x7[53], x8[42], x8[53], __rounding, cos_bit);
        x8[43] = x7[43];
        x8[44] = x7[44];
        btf_32_type0_avx512_new(cospi_m52, cospi_p12, x7[45], x7[50], x8[45], x8[50], __rounding, cos_bit);
        btf_32_type0_avx512_new(cospi_m12, cospi_m52, x7[46], x7[49], x8[46], x8[49], __rounding, cos_bit);
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
        __m512i x9[16];
        out[2 * stride]  = half_btf_avx512(&cospi_p62, &x8[16], &cospi_p02, &x8[31], &__rounding, cos_bit);
        out[14 * stride] = half_btf_avx512(&cospi_p14, &x8[28], &cospi_m50, &x8[19], &__rounding, cos_bit);
        out[10 * stride] = half_btf_avx512(&cospi_p54, &x8[20], &cospi_p10, &x8[27], &__rounding, cos_bit);
        out[6 * stride]  = half_btf_avx512(&cospi_p06, &x8[24], &cospi_m58, &x8[23], &__rounding, cos_bit);
        x9[0]            = _mm512_add_epi32(x8[32], x8[33]);
        x9[1]            = _mm512_add_epi32(x8[35], x8[34]);
        x9[2]            = _mm512_add_epi32(x8[36], x8[37]);
        x9[3]            = _mm512_add_epi32(x8[39], x8[38]);
        x9[4]            = _mm512_add_epi32(x8[40], x8[41]);
        x9[5]            = _mm512_add_epi32(x8[43], x8[42]);
        x9[6]            = _mm512_add_epi32(x8[44], x8[45]);
        x9[7]            = _mm512_add_epi32(x8[47], x8[46]);
        x9[8]            = _mm512_add_epi32(x8[48], x8[49]);
        x9[9]            = _mm512_add_epi32(x8[51], x8[50]);
        x9[10]           = _mm512_add_epi32(x8[63], x8[62]);
        x9[11]           = _mm512_add_epi32(x8[52], x8[53]);
        x9[12]           = _mm512_add_epi32(x8[55], x8[54]);
        x9[13]           = _mm512_add_epi32(x8[56], x8[57]);
        x9[14]           = _mm512_add_epi32(x8[59], x8[58]);
        x9[15]           = _mm512_add_epi32(x8[60], x8[61]);

        // stage 10
        out[1 * stride]  = half_btf_avx512(&cospi_p63, &x9[0], &cospi_p01, &x9[10], &__rounding, cos_bit);
        out[15 * stride] = half_btf_avx512(&cospi_p15, &x9[15], &cospi_m49, &x9[1], &__rounding, cos_bit);
        out[9 * stride]  = half_btf_avx512(&cospi_p55, &x9[2], &cospi_p09, &x9[14], &__rounding, cos_bit);
        out[7 * stride]  = half_btf_avx512(&cospi_p07, &x9[13], &cospi_m57, &x9[3], &__rounding, cos_bit);
        out[5 * stride]  = half_btf_avx512(&cospi_p59, &x9[4], &cospi_p05, &x9[12], &__rounding, cos_bit);
        out[11 * stride] = half_btf_avx512(&cospi_p11, &x9[11], &cospi_m53, &x9[5], &__rounding, cos_bit);
        out[13 * stride] = half_btf_avx512(&cospi_p51, &x9[6], &cospi_p13, &x9[9], &__rounding, cos_bit);
        out[3 * stride]  = half_btf_avx512(&cospi_p03, &x9[8], &cospi_m61, &x9[7], &__rounding, cos_bit);
    }
}

static void fidtx64x64_N4_avx512(const __m512i* input, __m512i* output) {
    const uint8_t bits     = 12; // new_sqrt2_bits = 12
    const int32_t sqrt     = 4 * 5793; // 4 * new_sqrt2
    const __m512i newsqrt  = _mm512_set1_epi32(sqrt);
    const __m512i rounding = _mm512_set1_epi32(1 << (bits - 1));

    __m512i temp;
    for (int32_t i = 0; i < 64; i += 4) {
        temp      = _mm512_mullo_epi32(input[i], newsqrt);
        temp      = _mm512_add_epi32(temp, rounding);
        output[i] = _mm512_srai_epi32(temp, bits);
    }
}

void av1_fwd_txfm2d_64x64_N4_avx512(int16_t* input, int32_t* output, uint32_t stride, TxType tx_type, uint8_t bd) {
    (void)bd;
    __m512i       in[256];
    __m512i*      out         = (__m512i*)output;
    const int32_t txw_idx     = tx_size_wide_log2[TX_64X64] - tx_size_wide_log2[0];
    const int32_t txh_idx     = tx_size_high_log2[TX_64X64] - tx_size_high_log2[0];
    const int8_t* shift       = fwd_txfm_shift_ls[TX_64X64];
    const int32_t txfm_size   = 64;
    const int32_t num_per_512 = 16;
    int32_t       col_num     = txfm_size / num_per_512;

    switch (tx_type) {
    case IDTX:
        load_buffer_16x16_in_64x64_avx512(input, stride, out);
        fidtx64x64_N4_avx512(out, in);
        av1_round_shift_array_64x64_N4_avx512(in, out, -shift[1]);
        /*row wise transform*/
        fidtx64x64_N4_avx512(out, in);
        av1_round_shift_array_64x64_N4_avx512(in, out, -shift[2]);
        clear_buffer_64x64_N4_avx512(out);
        break;
    case DCT_DCT:
        load_buffer_64x64_avx512(input, stride, out);
        av1_fdct64_new_N4_avx512(out, in, fwd_cos_bit_col[txw_idx][txh_idx], txfm_size, col_num);
        av1_round_shift_array_avx512(in, out, 256 / 4, -shift[1]);
        transpose_16nx16n_N4_half_avx512(64, out, in);
        /*row wise transform*/
        av1_fdct64_new_N4_avx512(in, out, fwd_cos_bit_row[txw_idx][txh_idx], txfm_size / 4, col_num);
        av1_round_shift_array_64x64_N4_avx512(out, in, -shift[2]);
        transpose_16nx16n_N4_quad_avx512(64, in, out);
        clear_buffer_64x64_N4_avx512(out);
        break;
    default:
        assert(0);
    }
}

#endif
