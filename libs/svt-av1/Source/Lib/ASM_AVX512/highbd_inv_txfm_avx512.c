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

#include "common_utils.h"

#if EN_AVX512_SUPPORT

#include <assert.h>
#include <immintrin.h>
#include "common_dsp_rtcd.h"
#include "inv_transforms.h"
#include "synonyms_avx512.h"
#include "transpose_avx512.h"

extern const int8_t* svt_aom_inv_txfm_shift_ls[];
const int32_t*       cospi_arr(int32_t n);
const int32_t*       sinpi_arr(int32_t n);

#define ONE (uint8_t)1U

typedef void (*inv_transform_1d_avx512)(__m512i* in, __m512i* out, const int8_t bit, int32_t num_cols);

static void load_buffer_64x64_lower_32x32_avx512(const int32_t* coeff, __m512i* in) {
    int32_t i, j;

    __m512i zeroes = _mm512_setzero_si512();

    for (i = 0; i < 32; ++i) {
        for (j = 0; j < 2; ++j) {
            in[4 * i + j]     = _mm512_loadu_si512((const __m512i*)(coeff + 32 * i + 16 * j));
            in[4 * i + j + 2] = zeroes;
        }
    }

    for (i = 0; i < 128; ++i) {
        in[128 + i] = zeroes;
    }
}

static void round_shift_64x64_avx512(__m512i* in, const int8_t shift) {
    uint8_t ushift = (uint8_t)shift;
    __m512i rnding = _mm512_set1_epi32(1 << (ushift - 1));

    for (int32_t i = 0; i < 256; i += 4) {
        in[i]     = _mm512_add_epi32(in[i], rnding);
        in[i + 1] = _mm512_add_epi32(in[i + 1], rnding);
        in[i + 2] = _mm512_add_epi32(in[i + 2], rnding);
        in[i + 3] = _mm512_add_epi32(in[i + 3], rnding);

        in[i]     = _mm512_srai_epi32(in[i], ushift);
        in[i + 1] = _mm512_srai_epi32(in[i + 1], ushift);
        in[i + 2] = _mm512_srai_epi32(in[i + 2], ushift);
        in[i + 3] = _mm512_srai_epi32(in[i + 3], ushift);
    }
}

// Note:
//  rounding = 1 << (bit - 1)
static INLINE __m512i half_btf_avx512(const __m512i* w0, const __m512i* n0, const __m512i* w1, const __m512i* n1,
                                      const __m512i* rounding, const int8_t bit) {
    __m512i x, y;

    x = _mm512_mullo_epi32(*w0, *n0);
    y = _mm512_mullo_epi32(*w1, *n1);
    x = _mm512_add_epi32(x, y);
    x = _mm512_add_epi32(x, *rounding);
    x = _mm512_srai_epi32(x, (uint8_t)bit);
    return x;
}

static INLINE __m512i half_btf_0_avx512(const __m512i* w0, const __m512i* n0, const __m512i* rounding,
                                        const int8_t bit) {
    __m512i x;

    x = _mm512_mullo_epi32(*w0, *n0);
    x = _mm512_add_epi32(x, *rounding);
    x = _mm512_srai_epi32(x, (uint8_t)bit);
    return x;
}

static void addsub_avx512(const __m512i in0, const __m512i in1, __m512i* out0, __m512i* out1, const __m512i* clamp_lo,
                          const __m512i* clamp_hi) {
    __m512i a0 = _mm512_add_epi32(in0, in1);
    __m512i a1 = _mm512_sub_epi32(in0, in1);

    a0 = _mm512_max_epi32(a0, *clamp_lo);
    a0 = _mm512_min_epi32(a0, *clamp_hi);
    a1 = _mm512_max_epi32(a1, *clamp_lo);
    a1 = _mm512_min_epi32(a1, *clamp_hi);

    *out0 = a0;
    *out1 = a1;
}

static void addsub_shift_avx512(const __m512i in0, const __m512i in1, __m512i* out0, __m512i* out1,
                                const __m512i* clamp_lo, const __m512i* clamp_hi, int32_t shift) {
    __m512i offset       = _mm512_set1_epi32((1 << shift) >> 1);
    __m512i in0_w_offset = _mm512_add_epi32(in0, offset);

    __m512i a0 = _mm512_add_epi32(in0_w_offset, in1);
    __m512i a1 = _mm512_sub_epi32(in0_w_offset, in1);

    a0 = _mm512_srai_epi32(a0, shift);
    a1 = _mm512_srai_epi32(a1, shift);

    a0 = _mm512_max_epi32(a0, *clamp_lo);
    a0 = _mm512_min_epi32(a0, *clamp_hi);
    a1 = _mm512_max_epi32(a1, *clamp_lo);
    a1 = _mm512_min_epi32(a1, *clamp_hi);

    *out0 = a0;
    *out1 = a1;
}

static void idct64x64_avx512(__m512i* in, __m512i* out, const int8_t bit, int32_t do_cols, int32_t bd) {
    int32_t        i, j;
    const int32_t* cospi     = cospi_arr(bit);
    const __m512i  rnding    = _mm512_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + (do_cols ? 6 : 8));
    const __m512i  clamp_lo  = _mm512_set1_epi32(-(1 << (log_range - 1)));
    const __m512i  clamp_hi  = _mm512_set1_epi32((1 << (log_range - 1)) - 1);
    int32_t        col;

    const __m512i cospi1  = _mm512_set1_epi32(cospi[1]);
    const __m512i cospi2  = _mm512_set1_epi32(cospi[2]);
    const __m512i cospi3  = _mm512_set1_epi32(cospi[3]);
    const __m512i cospi4  = _mm512_set1_epi32(cospi[4]);
    const __m512i cospi5  = _mm512_set1_epi32(cospi[5]);
    const __m512i cospi6  = _mm512_set1_epi32(cospi[6]);
    const __m512i cospi7  = _mm512_set1_epi32(cospi[7]);
    const __m512i cospi8  = _mm512_set1_epi32(cospi[8]);
    const __m512i cospi9  = _mm512_set1_epi32(cospi[9]);
    const __m512i cospi10 = _mm512_set1_epi32(cospi[10]);
    const __m512i cospi11 = _mm512_set1_epi32(cospi[11]);
    const __m512i cospi12 = _mm512_set1_epi32(cospi[12]);
    const __m512i cospi13 = _mm512_set1_epi32(cospi[13]);
    const __m512i cospi14 = _mm512_set1_epi32(cospi[14]);
    const __m512i cospi15 = _mm512_set1_epi32(cospi[15]);
    const __m512i cospi16 = _mm512_set1_epi32(cospi[16]);
    const __m512i cospi17 = _mm512_set1_epi32(cospi[17]);
    const __m512i cospi18 = _mm512_set1_epi32(cospi[18]);
    const __m512i cospi19 = _mm512_set1_epi32(cospi[19]);
    const __m512i cospi20 = _mm512_set1_epi32(cospi[20]);
    const __m512i cospi21 = _mm512_set1_epi32(cospi[21]);
    const __m512i cospi22 = _mm512_set1_epi32(cospi[22]);
    const __m512i cospi23 = _mm512_set1_epi32(cospi[23]);
    const __m512i cospi24 = _mm512_set1_epi32(cospi[24]);
    const __m512i cospi25 = _mm512_set1_epi32(cospi[25]);
    const __m512i cospi26 = _mm512_set1_epi32(cospi[26]);
    const __m512i cospi27 = _mm512_set1_epi32(cospi[27]);
    const __m512i cospi28 = _mm512_set1_epi32(cospi[28]);
    const __m512i cospi29 = _mm512_set1_epi32(cospi[29]);
    const __m512i cospi30 = _mm512_set1_epi32(cospi[30]);
    const __m512i cospi31 = _mm512_set1_epi32(cospi[31]);
    const __m512i cospi32 = _mm512_set1_epi32(cospi[32]);
    const __m512i cospi35 = _mm512_set1_epi32(cospi[35]);
    const __m512i cospi36 = _mm512_set1_epi32(cospi[36]);
    const __m512i cospi38 = _mm512_set1_epi32(cospi[38]);
    const __m512i cospi39 = _mm512_set1_epi32(cospi[39]);
    const __m512i cospi40 = _mm512_set1_epi32(cospi[40]);
    const __m512i cospi43 = _mm512_set1_epi32(cospi[43]);
    const __m512i cospi44 = _mm512_set1_epi32(cospi[44]);
    const __m512i cospi46 = _mm512_set1_epi32(cospi[46]);
    const __m512i cospi47 = _mm512_set1_epi32(cospi[47]);
    const __m512i cospi48 = _mm512_set1_epi32(cospi[48]);
    const __m512i cospi51 = _mm512_set1_epi32(cospi[51]);
    const __m512i cospi52 = _mm512_set1_epi32(cospi[52]);
    const __m512i cospi54 = _mm512_set1_epi32(cospi[54]);
    const __m512i cospi55 = _mm512_set1_epi32(cospi[55]);
    const __m512i cospi56 = _mm512_set1_epi32(cospi[56]);
    const __m512i cospi59 = _mm512_set1_epi32(cospi[59]);
    const __m512i cospi60 = _mm512_set1_epi32(cospi[60]);
    const __m512i cospi62 = _mm512_set1_epi32(cospi[62]);
    const __m512i cospi63 = _mm512_set1_epi32(cospi[63]);

    const __m512i cospim4  = _mm512_set1_epi32(-cospi[4]);
    const __m512i cospim8  = _mm512_set1_epi32(-cospi[8]);
    const __m512i cospim12 = _mm512_set1_epi32(-cospi[12]);
    const __m512i cospim16 = _mm512_set1_epi32(-cospi[16]);
    const __m512i cospim20 = _mm512_set1_epi32(-cospi[20]);
    const __m512i cospim24 = _mm512_set1_epi32(-cospi[24]);
    const __m512i cospim28 = _mm512_set1_epi32(-cospi[28]);
    const __m512i cospim32 = _mm512_set1_epi32(-cospi[32]);
    const __m512i cospim33 = _mm512_set1_epi32(-cospi[33]);
    const __m512i cospim34 = _mm512_set1_epi32(-cospi[34]);
    const __m512i cospim36 = _mm512_set1_epi32(-cospi[36]);
    const __m512i cospim37 = _mm512_set1_epi32(-cospi[37]);
    const __m512i cospim40 = _mm512_set1_epi32(-cospi[40]);
    const __m512i cospim41 = _mm512_set1_epi32(-cospi[41]);
    const __m512i cospim42 = _mm512_set1_epi32(-cospi[42]);
    const __m512i cospim44 = _mm512_set1_epi32(-cospi[44]);
    const __m512i cospim45 = _mm512_set1_epi32(-cospi[45]);
    const __m512i cospim48 = _mm512_set1_epi32(-cospi[48]);
    const __m512i cospim49 = _mm512_set1_epi32(-cospi[49]);
    const __m512i cospim50 = _mm512_set1_epi32(-cospi[50]);
    const __m512i cospim52 = _mm512_set1_epi32(-cospi[52]);
    const __m512i cospim53 = _mm512_set1_epi32(-cospi[53]);
    const __m512i cospim56 = _mm512_set1_epi32(-cospi[56]);
    const __m512i cospim57 = _mm512_set1_epi32(-cospi[57]);
    const __m512i cospim58 = _mm512_set1_epi32(-cospi[58]);
    const __m512i cospim60 = _mm512_set1_epi32(-cospi[60]);
    const __m512i cospim61 = _mm512_set1_epi32(-cospi[61]);

    for (col = 0; col < (do_cols ? 64 / 16 : 32 / 16); ++col) {
        __m512i u[64], v[64];

        // stage 1
        u[32] = in[1 * 4 + col];
        u[34] = in[17 * 4 + col];
        u[36] = in[9 * 4 + col];
        u[38] = in[25 * 4 + col];
        u[40] = in[5 * 4 + col];
        u[42] = in[21 * 4 + col];
        u[44] = in[13 * 4 + col];
        u[46] = in[29 * 4 + col];
        u[48] = in[3 * 4 + col];
        u[50] = in[19 * 4 + col];
        u[52] = in[11 * 4 + col];
        u[54] = in[27 * 4 + col];
        u[56] = in[7 * 4 + col];
        u[58] = in[23 * 4 + col];
        u[60] = in[15 * 4 + col];
        u[62] = in[31 * 4 + col];

        v[16] = in[2 * 4 + col];
        v[18] = in[18 * 4 + col];
        v[20] = in[10 * 4 + col];
        v[22] = in[26 * 4 + col];
        v[24] = in[6 * 4 + col];
        v[26] = in[22 * 4 + col];
        v[28] = in[14 * 4 + col];
        v[30] = in[30 * 4 + col];

        u[8]  = in[4 * 4 + col];
        u[10] = in[20 * 4 + col];
        u[12] = in[12 * 4 + col];
        u[14] = in[28 * 4 + col];

        v[4] = in[8 * 4 + col];
        v[6] = in[24 * 4 + col];

        u[0] = in[0 * 4 + col];
        u[2] = in[16 * 4 + col];

        // stage 2
        v[32] = half_btf_0_avx512(&cospi63, &u[32], &rnding, bit);
        v[33] = half_btf_0_avx512(&cospim33, &u[62], &rnding, bit);
        v[34] = half_btf_0_avx512(&cospi47, &u[34], &rnding, bit);
        v[35] = half_btf_0_avx512(&cospim49, &u[60], &rnding, bit);
        v[36] = half_btf_0_avx512(&cospi55, &u[36], &rnding, bit);
        v[37] = half_btf_0_avx512(&cospim41, &u[58], &rnding, bit);
        v[38] = half_btf_0_avx512(&cospi39, &u[38], &rnding, bit);
        v[39] = half_btf_0_avx512(&cospim57, &u[56], &rnding, bit);
        v[40] = half_btf_0_avx512(&cospi59, &u[40], &rnding, bit);
        v[41] = half_btf_0_avx512(&cospim37, &u[54], &rnding, bit);
        v[42] = half_btf_0_avx512(&cospi43, &u[42], &rnding, bit);
        v[43] = half_btf_0_avx512(&cospim53, &u[52], &rnding, bit);
        v[44] = half_btf_0_avx512(&cospi51, &u[44], &rnding, bit);
        v[45] = half_btf_0_avx512(&cospim45, &u[50], &rnding, bit);
        v[46] = half_btf_0_avx512(&cospi35, &u[46], &rnding, bit);
        v[47] = half_btf_0_avx512(&cospim61, &u[48], &rnding, bit);
        v[48] = half_btf_0_avx512(&cospi3, &u[48], &rnding, bit);
        v[49] = half_btf_0_avx512(&cospi29, &u[46], &rnding, bit);
        v[50] = half_btf_0_avx512(&cospi19, &u[50], &rnding, bit);
        v[51] = half_btf_0_avx512(&cospi13, &u[44], &rnding, bit);
        v[52] = half_btf_0_avx512(&cospi11, &u[52], &rnding, bit);
        v[53] = half_btf_0_avx512(&cospi21, &u[42], &rnding, bit);
        v[54] = half_btf_0_avx512(&cospi27, &u[54], &rnding, bit);
        v[55] = half_btf_0_avx512(&cospi5, &u[40], &rnding, bit);
        v[56] = half_btf_0_avx512(&cospi7, &u[56], &rnding, bit);
        v[57] = half_btf_0_avx512(&cospi25, &u[38], &rnding, bit);
        v[58] = half_btf_0_avx512(&cospi23, &u[58], &rnding, bit);
        v[59] = half_btf_0_avx512(&cospi9, &u[36], &rnding, bit);
        v[60] = half_btf_0_avx512(&cospi15, &u[60], &rnding, bit);
        v[61] = half_btf_0_avx512(&cospi17, &u[34], &rnding, bit);
        v[62] = half_btf_0_avx512(&cospi31, &u[62], &rnding, bit);
        v[63] = half_btf_0_avx512(&cospi1, &u[32], &rnding, bit);

        // stage 3
        u[16] = half_btf_0_avx512(&cospi62, &v[16], &rnding, bit);
        u[17] = half_btf_0_avx512(&cospim34, &v[30], &rnding, bit);
        u[18] = half_btf_0_avx512(&cospi46, &v[18], &rnding, bit);
        u[19] = half_btf_0_avx512(&cospim50, &v[28], &rnding, bit);
        u[20] = half_btf_0_avx512(&cospi54, &v[20], &rnding, bit);
        u[21] = half_btf_0_avx512(&cospim42, &v[26], &rnding, bit);
        u[22] = half_btf_0_avx512(&cospi38, &v[22], &rnding, bit);
        u[23] = half_btf_0_avx512(&cospim58, &v[24], &rnding, bit);
        u[24] = half_btf_0_avx512(&cospi6, &v[24], &rnding, bit);
        u[25] = half_btf_0_avx512(&cospi26, &v[22], &rnding, bit);
        u[26] = half_btf_0_avx512(&cospi22, &v[26], &rnding, bit);
        u[27] = half_btf_0_avx512(&cospi10, &v[20], &rnding, bit);
        u[28] = half_btf_0_avx512(&cospi14, &v[28], &rnding, bit);
        u[29] = half_btf_0_avx512(&cospi18, &v[18], &rnding, bit);
        u[30] = half_btf_0_avx512(&cospi30, &v[30], &rnding, bit);
        u[31] = half_btf_0_avx512(&cospi2, &v[16], &rnding, bit);

        for (i = 32; i < 64; i += 4) {
            addsub_avx512(v[i + 0], v[i + 1], &u[i + 0], &u[i + 1], &clamp_lo, &clamp_hi);
            addsub_avx512(v[i + 3], v[i + 2], &u[i + 3], &u[i + 2], &clamp_lo, &clamp_hi);
        }

        // stage 4
        v[8]  = half_btf_0_avx512(&cospi60, &u[8], &rnding, bit);
        v[9]  = half_btf_0_avx512(&cospim36, &u[14], &rnding, bit);
        v[10] = half_btf_0_avx512(&cospi44, &u[10], &rnding, bit);
        v[11] = half_btf_0_avx512(&cospim52, &u[12], &rnding, bit);
        v[12] = half_btf_0_avx512(&cospi12, &u[12], &rnding, bit);
        v[13] = half_btf_0_avx512(&cospi20, &u[10], &rnding, bit);
        v[14] = half_btf_0_avx512(&cospi28, &u[14], &rnding, bit);
        v[15] = half_btf_0_avx512(&cospi4, &u[8], &rnding, bit);

        for (i = 16; i < 32; i += 4) {
            addsub_avx512(u[i + 0], u[i + 1], &v[i + 0], &v[i + 1], &clamp_lo, &clamp_hi);
            addsub_avx512(u[i + 3], u[i + 2], &v[i + 3], &v[i + 2], &clamp_lo, &clamp_hi);
        }

        for (i = 32; i < 64; i += 4) {
            v[i + 0] = u[i + 0];
            v[i + 3] = u[i + 3];
        }

        v[33] = half_btf_avx512(&cospim4, &u[33], &cospi60, &u[62], &rnding, bit);
        v[34] = half_btf_avx512(&cospim60, &u[34], &cospim4, &u[61], &rnding, bit);
        v[37] = half_btf_avx512(&cospim36, &u[37], &cospi28, &u[58], &rnding, bit);
        v[38] = half_btf_avx512(&cospim28, &u[38], &cospim36, &u[57], &rnding, bit);
        v[41] = half_btf_avx512(&cospim20, &u[41], &cospi44, &u[54], &rnding, bit);
        v[42] = half_btf_avx512(&cospim44, &u[42], &cospim20, &u[53], &rnding, bit);
        v[45] = half_btf_avx512(&cospim52, &u[45], &cospi12, &u[50], &rnding, bit);
        v[46] = half_btf_avx512(&cospim12, &u[46], &cospim52, &u[49], &rnding, bit);
        v[49] = half_btf_avx512(&cospim52, &u[46], &cospi12, &u[49], &rnding, bit);
        v[50] = half_btf_avx512(&cospi12, &u[45], &cospi52, &u[50], &rnding, bit);
        v[53] = half_btf_avx512(&cospim20, &u[42], &cospi44, &u[53], &rnding, bit);
        v[54] = half_btf_avx512(&cospi44, &u[41], &cospi20, &u[54], &rnding, bit);
        v[57] = half_btf_avx512(&cospim36, &u[38], &cospi28, &u[57], &rnding, bit);
        v[58] = half_btf_avx512(&cospi28, &u[37], &cospi36, &u[58], &rnding, bit);
        v[61] = half_btf_avx512(&cospim4, &u[34], &cospi60, &u[61], &rnding, bit);
        v[62] = half_btf_avx512(&cospi60, &u[33], &cospi4, &u[62], &rnding, bit);

        // stage 5
        u[4] = half_btf_0_avx512(&cospi56, &v[4], &rnding, bit);
        u[5] = half_btf_0_avx512(&cospim40, &v[6], &rnding, bit);
        u[6] = half_btf_0_avx512(&cospi24, &v[6], &rnding, bit);
        u[7] = half_btf_0_avx512(&cospi8, &v[4], &rnding, bit);

        for (i = 8; i < 16; i += 4) {
            addsub_avx512(v[i + 0], v[i + 1], &u[i + 0], &u[i + 1], &clamp_lo, &clamp_hi);
            addsub_avx512(v[i + 3], v[i + 2], &u[i + 3], &u[i + 2], &clamp_lo, &clamp_hi);
        }

        for (i = 16; i < 32; i += 4) {
            u[i + 0] = v[i + 0];
            u[i + 3] = v[i + 3];
        }

        u[17] = half_btf_avx512(&cospim8, &v[17], &cospi56, &v[30], &rnding, bit);
        u[18] = half_btf_avx512(&cospim56, &v[18], &cospim8, &v[29], &rnding, bit);
        u[21] = half_btf_avx512(&cospim40, &v[21], &cospi24, &v[26], &rnding, bit);
        u[22] = half_btf_avx512(&cospim24, &v[22], &cospim40, &v[25], &rnding, bit);
        u[25] = half_btf_avx512(&cospim40, &v[22], &cospi24, &v[25], &rnding, bit);
        u[26] = half_btf_avx512(&cospi24, &v[21], &cospi40, &v[26], &rnding, bit);
        u[29] = half_btf_avx512(&cospim8, &v[18], &cospi56, &v[29], &rnding, bit);
        u[30] = half_btf_avx512(&cospi56, &v[17], &cospi8, &v[30], &rnding, bit);

        for (i = 32; i < 64; i += 8) {
            addsub_avx512(v[i + 0], v[i + 3], &u[i + 0], &u[i + 3], &clamp_lo, &clamp_hi);
            addsub_avx512(v[i + 1], v[i + 2], &u[i + 1], &u[i + 2], &clamp_lo, &clamp_hi);

            addsub_avx512(v[i + 7], v[i + 4], &u[i + 7], &u[i + 4], &clamp_lo, &clamp_hi);
            addsub_avx512(v[i + 6], v[i + 5], &u[i + 6], &u[i + 5], &clamp_lo, &clamp_hi);
        }

        // stage 6
        v[0] = half_btf_0_avx512(&cospi32, &u[0], &rnding, bit);
        v[1] = half_btf_0_avx512(&cospi32, &u[0], &rnding, bit);
        v[2] = half_btf_0_avx512(&cospi48, &u[2], &rnding, bit);
        v[3] = half_btf_0_avx512(&cospi16, &u[2], &rnding, bit);

        addsub_avx512(u[4], u[5], &v[4], &v[5], &clamp_lo, &clamp_hi);
        addsub_avx512(u[7], u[6], &v[7], &v[6], &clamp_lo, &clamp_hi);

        for (i = 8; i < 16; i += 4) {
            v[i + 0] = u[i + 0];
            v[i + 3] = u[i + 3];
        }

        v[9]  = half_btf_avx512(&cospim16, &u[9], &cospi48, &u[14], &rnding, bit);
        v[10] = half_btf_avx512(&cospim48, &u[10], &cospim16, &u[13], &rnding, bit);
        v[13] = half_btf_avx512(&cospim16, &u[10], &cospi48, &u[13], &rnding, bit);
        v[14] = half_btf_avx512(&cospi48, &u[9], &cospi16, &u[14], &rnding, bit);

        for (i = 16; i < 32; i += 8) {
            addsub_avx512(u[i + 0], u[i + 3], &v[i + 0], &v[i + 3], &clamp_lo, &clamp_hi);
            addsub_avx512(u[i + 1], u[i + 2], &v[i + 1], &v[i + 2], &clamp_lo, &clamp_hi);

            addsub_avx512(u[i + 7], u[i + 4], &v[i + 7], &v[i + 4], &clamp_lo, &clamp_hi);
            addsub_avx512(u[i + 6], u[i + 5], &v[i + 6], &v[i + 5], &clamp_lo, &clamp_hi);
        }

        for (i = 32; i < 64; i += 8) {
            v[i + 0] = u[i + 0];
            v[i + 1] = u[i + 1];
            v[i + 6] = u[i + 6];
            v[i + 7] = u[i + 7];
        }

        v[34] = half_btf_avx512(&cospim8, &u[34], &cospi56, &u[61], &rnding, bit);
        v[35] = half_btf_avx512(&cospim8, &u[35], &cospi56, &u[60], &rnding, bit);
        v[36] = half_btf_avx512(&cospim56, &u[36], &cospim8, &u[59], &rnding, bit);
        v[37] = half_btf_avx512(&cospim56, &u[37], &cospim8, &u[58], &rnding, bit);
        v[42] = half_btf_avx512(&cospim40, &u[42], &cospi24, &u[53], &rnding, bit);
        v[43] = half_btf_avx512(&cospim40, &u[43], &cospi24, &u[52], &rnding, bit);
        v[44] = half_btf_avx512(&cospim24, &u[44], &cospim40, &u[51], &rnding, bit);
        v[45] = half_btf_avx512(&cospim24, &u[45], &cospim40, &u[50], &rnding, bit);
        v[50] = half_btf_avx512(&cospim40, &u[45], &cospi24, &u[50], &rnding, bit);
        v[51] = half_btf_avx512(&cospim40, &u[44], &cospi24, &u[51], &rnding, bit);
        v[52] = half_btf_avx512(&cospi24, &u[43], &cospi40, &u[52], &rnding, bit);
        v[53] = half_btf_avx512(&cospi24, &u[42], &cospi40, &u[53], &rnding, bit);
        v[58] = half_btf_avx512(&cospim8, &u[37], &cospi56, &u[58], &rnding, bit);
        v[59] = half_btf_avx512(&cospim8, &u[36], &cospi56, &u[59], &rnding, bit);
        v[60] = half_btf_avx512(&cospi56, &u[35], &cospi8, &u[60], &rnding, bit);
        v[61] = half_btf_avx512(&cospi56, &u[34], &cospi8, &u[61], &rnding, bit);

        // stage 7
        addsub_avx512(v[0], v[3], &u[0], &u[3], &clamp_lo, &clamp_hi);
        addsub_avx512(v[1], v[2], &u[1], &u[2], &clamp_lo, &clamp_hi);

        u[4] = v[4];
        u[7] = v[7];
        u[5] = half_btf_avx512(&cospim32, &v[5], &cospi32, &v[6], &rnding, bit);
        u[6] = half_btf_avx512(&cospi32, &v[5], &cospi32, &v[6], &rnding, bit);

        addsub_avx512(v[8], v[11], &u[8], &u[11], &clamp_lo, &clamp_hi);
        addsub_avx512(v[9], v[10], &u[9], &u[10], &clamp_lo, &clamp_hi);
        addsub_avx512(v[15], v[12], &u[15], &u[12], &clamp_lo, &clamp_hi);
        addsub_avx512(v[14], v[13], &u[14], &u[13], &clamp_lo, &clamp_hi);

        for (i = 16; i < 32; i += 8) {
            u[i + 0] = v[i + 0];
            u[i + 1] = v[i + 1];
            u[i + 6] = v[i + 6];
            u[i + 7] = v[i + 7];
        }

        u[18] = half_btf_avx512(&cospim16, &v[18], &cospi48, &v[29], &rnding, bit);
        u[19] = half_btf_avx512(&cospim16, &v[19], &cospi48, &v[28], &rnding, bit);
        u[20] = half_btf_avx512(&cospim48, &v[20], &cospim16, &v[27], &rnding, bit);
        u[21] = half_btf_avx512(&cospim48, &v[21], &cospim16, &v[26], &rnding, bit);
        u[26] = half_btf_avx512(&cospim16, &v[21], &cospi48, &v[26], &rnding, bit);
        u[27] = half_btf_avx512(&cospim16, &v[20], &cospi48, &v[27], &rnding, bit);
        u[28] = half_btf_avx512(&cospi48, &v[19], &cospi16, &v[28], &rnding, bit);
        u[29] = half_btf_avx512(&cospi48, &v[18], &cospi16, &v[29], &rnding, bit);

        for (i = 32; i < 64; i += 16) {
            for (j = i; j < i + 4; j++) {
                addsub_avx512(v[j], v[j ^ 7], &u[j], &u[j ^ 7], &clamp_lo, &clamp_hi);
                addsub_avx512(v[j ^ 15], v[j ^ 8], &u[j ^ 15], &u[j ^ 8], &clamp_lo, &clamp_hi);
            }
        }

        // stage 8
        for (i = 0; i < 4; ++i) {
            addsub_avx512(u[i], u[7 - i], &v[i], &v[7 - i], &clamp_lo, &clamp_hi);
        }

        v[8]  = u[8];
        v[9]  = u[9];
        v[14] = u[14];
        v[15] = u[15];

        v[10] = half_btf_avx512(&cospim32, &u[10], &cospi32, &u[13], &rnding, bit);
        v[11] = half_btf_avx512(&cospim32, &u[11], &cospi32, &u[12], &rnding, bit);
        v[12] = half_btf_avx512(&cospi32, &u[11], &cospi32, &u[12], &rnding, bit);
        v[13] = half_btf_avx512(&cospi32, &u[10], &cospi32, &u[13], &rnding, bit);

        for (i = 16; i < 20; ++i) {
            addsub_avx512(u[i], u[i ^ 7], &v[i], &v[i ^ 7], &clamp_lo, &clamp_hi);
            addsub_avx512(u[i ^ 15], u[i ^ 8], &v[i ^ 15], &v[i ^ 8], &clamp_lo, &clamp_hi);
        }

        for (i = 32; i < 36; ++i) {
            v[i]      = u[i];
            v[i + 12] = u[i + 12];
            v[i + 16] = u[i + 16];
            v[i + 28] = u[i + 28];
        }

        v[36] = half_btf_avx512(&cospim16, &u[36], &cospi48, &u[59], &rnding, bit);
        v[37] = half_btf_avx512(&cospim16, &u[37], &cospi48, &u[58], &rnding, bit);
        v[38] = half_btf_avx512(&cospim16, &u[38], &cospi48, &u[57], &rnding, bit);
        v[39] = half_btf_avx512(&cospim16, &u[39], &cospi48, &u[56], &rnding, bit);
        v[40] = half_btf_avx512(&cospim48, &u[40], &cospim16, &u[55], &rnding, bit);
        v[41] = half_btf_avx512(&cospim48, &u[41], &cospim16, &u[54], &rnding, bit);
        v[42] = half_btf_avx512(&cospim48, &u[42], &cospim16, &u[53], &rnding, bit);
        v[43] = half_btf_avx512(&cospim48, &u[43], &cospim16, &u[52], &rnding, bit);
        v[52] = half_btf_avx512(&cospim16, &u[43], &cospi48, &u[52], &rnding, bit);
        v[53] = half_btf_avx512(&cospim16, &u[42], &cospi48, &u[53], &rnding, bit);
        v[54] = half_btf_avx512(&cospim16, &u[41], &cospi48, &u[54], &rnding, bit);
        v[55] = half_btf_avx512(&cospim16, &u[40], &cospi48, &u[55], &rnding, bit);
        v[56] = half_btf_avx512(&cospi48, &u[39], &cospi16, &u[56], &rnding, bit);
        v[57] = half_btf_avx512(&cospi48, &u[38], &cospi16, &u[57], &rnding, bit);
        v[58] = half_btf_avx512(&cospi48, &u[37], &cospi16, &u[58], &rnding, bit);
        v[59] = half_btf_avx512(&cospi48, &u[36], &cospi16, &u[59], &rnding, bit);

        // stage 9
        for (i = 0; i < 8; ++i) {
            addsub_avx512(v[i], v[15 - i], &u[i], &u[15 - i], &clamp_lo, &clamp_hi);
        }

        for (i = 16; i < 20; ++i) {
            u[i]      = v[i];
            u[i + 12] = v[i + 12];
        }

        u[20] = half_btf_avx512(&cospim32, &v[20], &cospi32, &v[27], &rnding, bit);
        u[21] = half_btf_avx512(&cospim32, &v[21], &cospi32, &v[26], &rnding, bit);
        u[22] = half_btf_avx512(&cospim32, &v[22], &cospi32, &v[25], &rnding, bit);
        u[23] = half_btf_avx512(&cospim32, &v[23], &cospi32, &v[24], &rnding, bit);
        u[24] = half_btf_avx512(&cospi32, &v[23], &cospi32, &v[24], &rnding, bit);
        u[25] = half_btf_avx512(&cospi32, &v[22], &cospi32, &v[25], &rnding, bit);
        u[26] = half_btf_avx512(&cospi32, &v[21], &cospi32, &v[26], &rnding, bit);
        u[27] = half_btf_avx512(&cospi32, &v[20], &cospi32, &v[27], &rnding, bit);

        for (i = 32; i < 40; i++) {
            addsub_avx512(v[i], v[i ^ 15], &u[i], &u[i ^ 15], &clamp_lo, &clamp_hi);
        }

        for (i = 48; i < 56; i++) {
            addsub_avx512(v[i ^ 15], v[i], &u[i ^ 15], &u[i], &clamp_lo, &clamp_hi);
        }

        // stage 10
        for (i = 0; i < 16; i++) {
            addsub_avx512(u[i], u[31 - i], &v[i], &v[31 - i], &clamp_lo, &clamp_hi);
        }

        for (i = 32; i < 40; i++) {
            v[i] = u[i];
        }

        v[40] = half_btf_avx512(&cospim32, &u[40], &cospi32, &u[55], &rnding, bit);
        v[41] = half_btf_avx512(&cospim32, &u[41], &cospi32, &u[54], &rnding, bit);
        v[42] = half_btf_avx512(&cospim32, &u[42], &cospi32, &u[53], &rnding, bit);
        v[43] = half_btf_avx512(&cospim32, &u[43], &cospi32, &u[52], &rnding, bit);
        v[44] = half_btf_avx512(&cospim32, &u[44], &cospi32, &u[51], &rnding, bit);
        v[45] = half_btf_avx512(&cospim32, &u[45], &cospi32, &u[50], &rnding, bit);
        v[46] = half_btf_avx512(&cospim32, &u[46], &cospi32, &u[49], &rnding, bit);
        v[47] = half_btf_avx512(&cospim32, &u[47], &cospi32, &u[48], &rnding, bit);
        v[48] = half_btf_avx512(&cospi32, &u[47], &cospi32, &u[48], &rnding, bit);
        v[49] = half_btf_avx512(&cospi32, &u[46], &cospi32, &u[49], &rnding, bit);
        v[50] = half_btf_avx512(&cospi32, &u[45], &cospi32, &u[50], &rnding, bit);
        v[51] = half_btf_avx512(&cospi32, &u[44], &cospi32, &u[51], &rnding, bit);
        v[52] = half_btf_avx512(&cospi32, &u[43], &cospi32, &u[52], &rnding, bit);
        v[53] = half_btf_avx512(&cospi32, &u[42], &cospi32, &u[53], &rnding, bit);
        v[54] = half_btf_avx512(&cospi32, &u[41], &cospi32, &u[54], &rnding, bit);
        v[55] = half_btf_avx512(&cospi32, &u[40], &cospi32, &u[55], &rnding, bit);

        for (i = 56; i < 64; i++) {
            v[i] = u[i];
        }

        // stage 11
        for (i = 0; i < 32; i++) {
            addsub_avx512(v[i], v[63 - i], &out[4 * (i) + col], &out[4 * (63 - i) + col], &clamp_lo, &clamp_hi);
        }
    }
}

static INLINE void highbd_clamp_epi32_avx512(__m512i* x, int32_t bd) {
    const __m512i zeroes = _mm512_setzero_si512();
    const __m512i max    = _mm512_set1_epi32((1 << bd) - 1);

    *x = _mm512_min_epi32(*x, max);
    *x = _mm512_max_epi32(*x, zeroes);
}

static void load_buffer_16x16_avx512(const int32_t* coeff, __m512i* in) {
    int32_t i;
    for (i = 0; i < 16; ++i) {
        in[i] = zz_load_512(coeff);
        coeff += 16;
    }
}

static INLINE __m256i highbd_clamp_epi16_avx512(__m256i u, int32_t bd) {
    const __m256i zeroes = _mm256_setzero_si256();
    const __m256i ONEs   = _mm256_set1_epi16(1);
    const __m256i max    = _mm256_sub_epi16(_mm256_slli_epi16(ONEs, (uint8_t)bd), ONEs);
    __m256i       clamped, mask;

    mask    = _mm256_cmpgt_epi16(u, max);
    clamped = _mm256_andnot_si256(mask, u);
    mask    = _mm256_and_si256(mask, max);
    clamped = _mm256_or_si256(mask, clamped);
    mask    = _mm256_cmpgt_epi16(clamped, zeroes);
    clamped = _mm256_and_si256(clamped, mask);

    return clamped;
}

static INLINE void write_buffer_16x16_avx512_new(__m512i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                                 int32_t stride_w, int32_t fliplr, int32_t flipud, int32_t bd) {
    __m512i  u1, v0, index;
    __m256i  u0, a, b;
    __m128i  p, q, r, s;
    int32_t  i     = 0;
    uint32_t idx[] = {15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0};
    index          = _mm512_loadu_si512((const __m256i*)idx);

    if (flipud) {
        output_r += stride_r * 15;
        stride_r = -stride_r;
        output_w += stride_w * 15;
        stride_w = -stride_w;
    }

    while (i < 16) {
        u0 = _mm256_loadu_si256((const __m256i*)output_r);

        u1 = _mm512_cvtepu16_epi32(u0);
        v0 = in[i];
        if (fliplr) {
            v0 = _mm512_permutexvar_epi32(index, v0);
        }

        v0 = _mm512_add_epi32(v0, u1);

        a  = _mm512_castsi512_si256(v0);
        b  = _mm512_extracti64x4_epi64(v0, ONE);
        p  = _mm256_castsi256_si128(a);
        q  = _mm256_extracti128_si256(a, ONE);
        r  = _mm256_castsi256_si128(b);
        s  = _mm256_extracti128_si256(b, ONE);
        p  = _mm_packus_epi32(p, q);
        r  = _mm_packus_epi32(r, s);
        u0 = _mm256_castsi128_si256(p);
        u0 = _mm256_insertf128_si256(u0, r, ONE);
        u0 = highbd_clamp_epi16_avx512(u0, bd);

        _mm256_storeu_si256((__m256i*)output_w, u0);

        output_r += stride_r;
        output_w += stride_w;
        i += 1;
    }
}

static INLINE void write_buffer_16x16_avx512(__m512i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                             int32_t stride_w, int32_t fliplr, int32_t flipud, int32_t bd) {
    __m512i  u1, v0, index;
    __m256i  u0;
    int32_t  i     = 0;
    uint32_t idx[] = {15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0};
    index          = _mm512_loadu_si512((const __m256i*)idx);

    if (flipud) {
        output_r += stride_r * 15;
        stride_r = -stride_r;
        output_w += stride_w * 15;
        stride_w = -stride_w;
    }

    while (i < 16) {
        u0 = _mm256_loadu_si256((const __m256i*)output_r);

        u1 = _mm512_cvtepu16_epi32(u0);
        v0 = in[i];
        if (fliplr) {
            v0 = _mm512_permutexvar_epi32(index, v0);
        }

        v0 = _mm512_add_epi32(v0, u1);
        highbd_clamp_epi32_avx512(&v0, bd);

        u0 = _mm512_cvtepi32_epi16(v0);

        _mm256_storeu_si256((__m256i*)output_w, u0);

        output_r += stride_r;
        output_w += stride_w;
        i += 1;
    }
}

static INLINE void round_shift_16x16_avx512(__m512i* in, const int8_t shift) {
    uint8_t ushift = (uint8_t)shift;
    __m512i rnding = _mm512_set1_epi32(1 << (ushift - 1));
    int32_t i      = 0;

    while (i < 16) {
        in[i] = _mm512_add_epi32(in[i], rnding);
        in[i] = _mm512_srai_epi32(in[i], ushift);
        i++;
    }
}

static INLINE void iidentity16_and_round_shift_avx512(__m512i* input, int32_t shift) {
    const __m512i scalar = _mm512_set1_epi32(new_sqrt2);
    const __m512i rnding = _mm512_set1_epi32((1 << (new_sqrt2_bits - 2)) + (!!(shift) << (shift + new_sqrt2_bits - 2)));

    for (int32_t i = 0; i < 16; i++) {
        input[i] = _mm512_mullo_epi32(input[i], scalar);
        input[i] = _mm512_add_epi32(input[i], rnding);
        input[i] = _mm512_srai_epi32(input[i], (uint8_t)(new_sqrt2_bits - 1 + shift));
    }
}

static INLINE void idct16_col_avx512(__m512i* in, __m512i* out, const int8_t bit) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m512i  cospi60  = _mm512_set1_epi32(cospi[60]);
    const __m512i  cospi28  = _mm512_set1_epi32(cospi[28]);
    const __m512i  cospi44  = _mm512_set1_epi32(cospi[44]);
    const __m512i  cospi12  = _mm512_set1_epi32(cospi[12]);
    const __m512i  cospi52  = _mm512_set1_epi32(cospi[52]);
    const __m512i  cospi20  = _mm512_set1_epi32(cospi[20]);
    const __m512i  cospi36  = _mm512_set1_epi32(cospi[36]);
    const __m512i  cospi4   = _mm512_set1_epi32(cospi[4]);
    const __m512i  cospi56  = _mm512_set1_epi32(cospi[56]);
    const __m512i  cospi24  = _mm512_set1_epi32(cospi[24]);
    const __m512i  cospi40  = _mm512_set1_epi32(cospi[40]);
    const __m512i  cospi8   = _mm512_set1_epi32(cospi[8]);
    const __m512i  cospi32  = _mm512_set1_epi32(cospi[32]);
    const __m512i  cospi48  = _mm512_set1_epi32(cospi[48]);
    const __m512i  cospi16  = _mm512_set1_epi32(cospi[16]);
    const __m512i  cospim4  = _mm512_set1_epi32(-cospi[4]);
    const __m512i  cospim36 = _mm512_set1_epi32(-cospi[36]);
    const __m512i  cospim20 = _mm512_set1_epi32(-cospi[20]);
    const __m512i  cospim52 = _mm512_set1_epi32(-cospi[52]);
    const __m512i  cospim8  = _mm512_set1_epi32(-cospi[8]);
    const __m512i  cospim40 = _mm512_set1_epi32(-cospi[40]);
    const __m512i  cospim32 = _mm512_set1_epi32(-cospi[32]);
    const __m512i  cospim48 = _mm512_set1_epi32(-cospi[48]);
    const __m512i  cospim16 = _mm512_set1_epi32(-cospi[16]);
    const __m512i  rounding = _mm512_set1_epi32(1 << (bit - 1));
    __m512i        tmp[16], tmp2[16];
    //stage 1

    //stage 2
    tmp[8]  = half_btf_avx512(&cospi60, &in[1], &cospim4, &in[15], &rounding, bit);
    tmp[9]  = half_btf_avx512(&cospi28, &in[9], &cospim36, &in[7], &rounding, bit);
    tmp[10] = half_btf_avx512(&cospi44, &in[5], &cospim20, &in[11], &rounding, bit);
    tmp[11] = half_btf_avx512(&cospi12, &in[13], &cospim52, &in[3], &rounding, bit);
    tmp[12] = half_btf_avx512(&cospi52, &in[13], &cospi12, &in[3], &rounding, bit);
    tmp[13] = half_btf_avx512(&cospi20, &in[5], &cospi44, &in[11], &rounding, bit);
    tmp[14] = half_btf_avx512(&cospi36, &in[9], &cospi28, &in[7], &rounding, bit);
    tmp[15] = half_btf_avx512(&cospi4, &in[1], &cospi60, &in[15], &rounding, bit);

    //stage 3
    tmp2[0]  = half_btf_avx512(&cospi56, &in[2], &cospim8, &in[14], &rounding, bit);
    tmp2[1]  = half_btf_avx512(&cospi24, &in[10], &cospim40, &in[6], &rounding, bit);
    tmp2[2]  = half_btf_avx512(&cospi40, &in[10], &cospi24, &in[6], &rounding, bit);
    tmp2[3]  = half_btf_avx512(&cospi8, &in[2], &cospi56, &in[14], &rounding, bit);
    tmp2[4]  = _mm512_add_epi32(tmp[8], tmp[9]);
    tmp2[5]  = _mm512_sub_epi32(tmp[8], tmp[9]);
    tmp2[6]  = _mm512_sub_epi32(tmp[11], tmp[10]);
    tmp2[7]  = _mm512_add_epi32(tmp[10], tmp[11]);
    tmp2[8]  = _mm512_add_epi32(tmp[12], tmp[13]);
    tmp2[9]  = _mm512_sub_epi32(tmp[12], tmp[13]);
    tmp2[10] = _mm512_sub_epi32(tmp[15], tmp[14]);
    tmp2[11] = _mm512_add_epi32(tmp[14], tmp[15]);

    //stage 4
    tmp[0]  = half_btf_avx512(&cospi32, &in[0], &cospi32, &in[8], &rounding, bit);
    tmp[1]  = half_btf_avx512(&cospi32, &in[0], &cospim32, &in[8], &rounding, bit);
    tmp[2]  = half_btf_avx512(&cospi48, &in[4], &cospim16, &in[12], &rounding, bit);
    tmp[3]  = half_btf_avx512(&cospi16, &in[4], &cospi48, &in[12], &rounding, bit);
    tmp[4]  = _mm512_add_epi32(tmp2[0], tmp2[1]);
    tmp[5]  = _mm512_sub_epi32(tmp2[0], tmp2[1]);
    tmp[6]  = _mm512_sub_epi32(tmp2[3], tmp2[2]);
    tmp[7]  = _mm512_add_epi32(tmp2[2], tmp2[3]);
    tmp[9]  = half_btf_avx512(&cospim16, &tmp2[5], &cospi48, &tmp2[10], &rounding, bit);
    tmp[10] = half_btf_avx512(&cospim48, &tmp2[6], &cospim16, &tmp2[9], &rounding, bit);
    tmp[13] = half_btf_avx512(&cospim16, &tmp2[6], &cospi48, &tmp2[9], &rounding, bit);
    tmp[14] = half_btf_avx512(&cospi48, &tmp2[5], &cospi16, &tmp2[10], &rounding, bit);

    //stage 5
    tmp2[12] = _mm512_sub_epi32(tmp2[11], tmp2[8]);
    tmp2[15] = _mm512_add_epi32(tmp2[8], tmp2[11]);
    tmp2[8]  = _mm512_add_epi32(tmp2[4], tmp2[7]);
    tmp2[11] = _mm512_sub_epi32(tmp2[4], tmp2[7]);
    tmp2[0]  = _mm512_add_epi32(tmp[0], tmp[3]);
    tmp2[1]  = _mm512_add_epi32(tmp[1], tmp[2]);
    tmp2[2]  = _mm512_sub_epi32(tmp[1], tmp[2]);
    tmp2[3]  = _mm512_sub_epi32(tmp[0], tmp[3]);
    tmp2[5]  = half_btf_avx512(&cospim32, &tmp[5], &cospi32, &tmp[6], &rounding, bit);
    tmp2[6]  = half_btf_avx512(&cospi32, &tmp[5], &cospi32, &tmp[6], &rounding, bit);
    tmp2[9]  = _mm512_add_epi32(tmp[9], tmp[10]);
    tmp2[10] = _mm512_sub_epi32(tmp[9], tmp[10]);
    tmp2[13] = _mm512_sub_epi32(tmp[14], tmp[13]);
    tmp2[14] = _mm512_add_epi32(tmp[13], tmp[14]);

    //stage 6
    tmp[0]  = _mm512_add_epi32(tmp2[0], tmp[7]);
    tmp[1]  = _mm512_add_epi32(tmp2[1], tmp2[6]);
    tmp[2]  = _mm512_add_epi32(tmp2[2], tmp2[5]);
    tmp[3]  = _mm512_add_epi32(tmp2[3], tmp[4]);
    tmp[4]  = _mm512_sub_epi32(tmp2[3], tmp[4]);
    tmp[5]  = _mm512_sub_epi32(tmp2[2], tmp2[5]);
    tmp[6]  = _mm512_sub_epi32(tmp2[1], tmp2[6]);
    tmp[7]  = _mm512_sub_epi32(tmp2[0], tmp[7]);
    tmp[10] = half_btf_avx512(&cospim32, &tmp2[10], &cospi32, &tmp2[13], &rounding, bit);
    tmp[11] = half_btf_avx512(&cospim32, &tmp2[11], &cospi32, &tmp2[12], &rounding, bit);
    tmp[12] = half_btf_avx512(&cospi32, &tmp2[11], &cospi32, &tmp2[12], &rounding, bit);
    tmp[13] = half_btf_avx512(&cospi32, &tmp2[10], &cospi32, &tmp2[13], &rounding, bit);

    //stage 7
    out[0]  = _mm512_add_epi32(tmp[0], tmp2[15]);
    out[1]  = _mm512_add_epi32(tmp[1], tmp2[14]);
    out[2]  = _mm512_add_epi32(tmp[2], tmp[13]);
    out[3]  = _mm512_add_epi32(tmp[3], tmp[12]);
    out[4]  = _mm512_add_epi32(tmp[4], tmp[11]);
    out[5]  = _mm512_add_epi32(tmp[5], tmp[10]);
    out[6]  = _mm512_add_epi32(tmp[6], tmp2[9]);
    out[7]  = _mm512_add_epi32(tmp[7], tmp2[8]);
    out[8]  = _mm512_sub_epi32(tmp[7], tmp2[8]);
    out[9]  = _mm512_sub_epi32(tmp[6], tmp2[9]);
    out[10] = _mm512_sub_epi32(tmp[5], tmp[10]);
    out[11] = _mm512_sub_epi32(tmp[4], tmp[11]);
    out[12] = _mm512_sub_epi32(tmp[3], tmp[12]);
    out[13] = _mm512_sub_epi32(tmp[2], tmp[13]);
    out[14] = _mm512_sub_epi32(tmp[1], tmp2[14]);
    out[15] = _mm512_sub_epi32(tmp[0], tmp2[15]);
}

static INLINE void iadst16_col_avx512(__m512i* in, __m512i* out, const int8_t cos_bit) {
    const int32_t* cospi   = cospi_arr(cos_bit);
    const __m512i  cospi2  = _mm512_set1_epi32(cospi[2]);
    const __m512i  cospi62 = _mm512_set1_epi32(cospi[62]);
    const __m512i  cospi10 = _mm512_set1_epi32(cospi[10]);
    const __m512i  cospi54 = _mm512_set1_epi32(cospi[54]);
    const __m512i  cospi18 = _mm512_set1_epi32(cospi[18]);
    const __m512i  cospi46 = _mm512_set1_epi32(cospi[46]);
    const __m512i  cospi26 = _mm512_set1_epi32(cospi[26]);
    const __m512i  cospi38 = _mm512_set1_epi32(cospi[38]);
    const __m512i  cospi34 = _mm512_set1_epi32(cospi[34]);
    const __m512i  cospi30 = _mm512_set1_epi32(cospi[30]);
    const __m512i  cospi42 = _mm512_set1_epi32(cospi[42]);
    const __m512i  cospi22 = _mm512_set1_epi32(cospi[22]);
    const __m512i  cospi50 = _mm512_set1_epi32(cospi[50]);
    const __m512i  cospi14 = _mm512_set1_epi32(cospi[14]);
    const __m512i  cospi58 = _mm512_set1_epi32(cospi[58]);
    const __m512i  cospi6  = _mm512_set1_epi32(cospi[6]);

    const __m512i cospi8  = _mm512_set1_epi32(cospi[8]);
    const __m512i cospi56 = _mm512_set1_epi32(cospi[56]);
    const __m512i cospi40 = _mm512_set1_epi32(cospi[40]);
    const __m512i cospi24 = _mm512_set1_epi32(cospi[24]);

    const __m512i cospi16 = _mm512_set1_epi32(cospi[16]);
    const __m512i cospi48 = _mm512_set1_epi32(cospi[48]);
    const __m512i cospi32 = _mm512_set1_epi32(cospi[32]);

    const __m512i cospim2  = _mm512_set1_epi32(-cospi[2]);
    const __m512i cospim10 = _mm512_set1_epi32(-cospi[10]);
    const __m512i cospim18 = _mm512_set1_epi32(-cospi[18]);
    const __m512i cospim26 = _mm512_set1_epi32(-cospi[26]);
    const __m512i cospim34 = _mm512_set1_epi32(-cospi[34]);
    const __m512i cospim42 = _mm512_set1_epi32(-cospi[42]);
    const __m512i cospim50 = _mm512_set1_epi32(-cospi[50]);
    const __m512i cospim58 = _mm512_set1_epi32(-cospi[58]);
    const __m512i cospim56 = _mm512_set1_epi32(-cospi[56]);
    const __m512i cospim24 = _mm512_set1_epi32(-cospi[24]);
    const __m512i cospim8  = _mm512_set1_epi32(-cospi[8]);
    const __m512i cospim40 = _mm512_set1_epi32(-cospi[40]);
    const __m512i cospim48 = _mm512_set1_epi32(-cospi[48]);
    const __m512i cospim16 = _mm512_set1_epi32(-cospi[16]);
    const __m512i cospim32 = _mm512_set1_epi32(-cospi[32]);

    const __m256i negative = _mm256_set1_epi32(-1);
    const __m512i rounding = _mm512_set1_epi32(1 << (cos_bit - 1));

    __m512i tmp[16], tmp2[16], tmp3[16];
    __m256i temp1, temp2;
    //stage 1

    //stage 2
    tmp[0]  = half_btf_avx512(&cospi2, &in[15], &cospi62, &in[0], &rounding, cos_bit);
    tmp[1]  = half_btf_avx512(&cospi62, &in[15], &cospim2, &in[0], &rounding, cos_bit);
    tmp[2]  = half_btf_avx512(&cospi10, &in[13], &cospi54, &in[2], &rounding, cos_bit);
    tmp[3]  = half_btf_avx512(&cospi54, &in[13], &cospim10, &in[2], &rounding, cos_bit);
    tmp[4]  = half_btf_avx512(&cospi18, &in[11], &cospi46, &in[4], &rounding, cos_bit);
    tmp[5]  = half_btf_avx512(&cospi46, &in[11], &cospim18, &in[4], &rounding, cos_bit);
    tmp[6]  = half_btf_avx512(&cospi26, &in[9], &cospi38, &in[6], &rounding, cos_bit);
    tmp[7]  = half_btf_avx512(&cospi38, &in[9], &cospim26, &in[6], &rounding, cos_bit);
    tmp[8]  = half_btf_avx512(&cospi34, &in[7], &cospi30, &in[8], &rounding, cos_bit);
    tmp[9]  = half_btf_avx512(&cospi30, &in[7], &cospim34, &in[8], &rounding, cos_bit);
    tmp[10] = half_btf_avx512(&cospi42, &in[5], &cospi22, &in[10], &rounding, cos_bit);
    tmp[11] = half_btf_avx512(&cospi22, &in[5], &cospim42, &in[10], &rounding, cos_bit);
    tmp[12] = half_btf_avx512(&cospi50, &in[3], &cospi14, &in[12], &rounding, cos_bit);
    tmp[13] = half_btf_avx512(&cospi14, &in[3], &cospim50, &in[12], &rounding, cos_bit);
    tmp[14] = half_btf_avx512(&cospi58, &in[1], &cospi6, &in[14], &rounding, cos_bit);
    tmp[15] = half_btf_avx512(&cospi6, &in[1], &cospim58, &in[14], &rounding, cos_bit);

    //stage 3
    tmp3[0]  = _mm512_add_epi32(tmp[0], tmp[8]);
    tmp3[1]  = _mm512_add_epi32(tmp[1], tmp[9]);
    tmp3[2]  = _mm512_add_epi32(tmp[2], tmp[10]);
    tmp3[3]  = _mm512_add_epi32(tmp[3], tmp[11]);
    tmp3[4]  = _mm512_add_epi32(tmp[4], tmp[12]);
    tmp3[5]  = _mm512_add_epi32(tmp[5], tmp[13]);
    tmp3[6]  = _mm512_add_epi32(tmp[6], tmp[14]);
    tmp3[7]  = _mm512_add_epi32(tmp[7], tmp[15]);
    tmp2[8]  = _mm512_sub_epi32(tmp[0], tmp[8]);
    tmp2[9]  = _mm512_sub_epi32(tmp[1], tmp[9]);
    tmp2[10] = _mm512_sub_epi32(tmp[2], tmp[10]);
    tmp2[11] = _mm512_sub_epi32(tmp[3], tmp[11]);
    tmp2[12] = _mm512_sub_epi32(tmp[4], tmp[12]);
    tmp2[13] = _mm512_sub_epi32(tmp[5], tmp[13]);
    tmp2[14] = _mm512_sub_epi32(tmp[6], tmp[14]);
    tmp2[15] = _mm512_sub_epi32(tmp[7], tmp[15]);

    //stage 4
    tmp[8]  = half_btf_avx512(&cospi8, &tmp2[8], &cospi56, &tmp2[9], &rounding, cos_bit);
    tmp[9]  = half_btf_avx512(&cospi56, &tmp2[8], &cospim8, &tmp2[9], &rounding, cos_bit);
    tmp[10] = half_btf_avx512(&cospi40, &tmp2[10], &cospi24, &tmp2[11], &rounding, cos_bit);
    tmp[11] = half_btf_avx512(&cospi24, &tmp2[10], &cospim40, &tmp2[11], &rounding, cos_bit);
    tmp[12] = half_btf_avx512(&cospim56, &tmp2[12], &cospi8, &tmp2[13], &rounding, cos_bit);
    tmp[13] = half_btf_avx512(&cospi8, &tmp2[12], &cospi56, &tmp2[13], &rounding, cos_bit);
    tmp[14] = half_btf_avx512(&cospim24, &tmp2[14], &cospi40, &tmp2[15], &rounding, cos_bit);
    tmp[15] = half_btf_avx512(&cospi40, &tmp2[14], &cospi24, &tmp2[15], &rounding, cos_bit);

    //stage 5
    tmp3[8]  = _mm512_add_epi32(tmp3[0], tmp3[4]);
    tmp3[9]  = _mm512_add_epi32(tmp3[1], tmp3[5]);
    tmp3[10] = _mm512_add_epi32(tmp3[2], tmp3[6]);
    tmp3[11] = _mm512_add_epi32(tmp3[3], tmp3[7]);
    tmp2[4]  = _mm512_sub_epi32(tmp3[0], tmp3[4]);
    tmp2[5]  = _mm512_sub_epi32(tmp3[1], tmp3[5]);
    tmp2[6]  = _mm512_sub_epi32(tmp3[2], tmp3[6]);
    tmp2[7]  = _mm512_sub_epi32(tmp3[3], tmp3[7]);
    tmp3[12] = _mm512_add_epi32(tmp[8], tmp[12]);
    tmp3[13] = _mm512_add_epi32(tmp[9], tmp[13]);
    tmp3[14] = _mm512_add_epi32(tmp[10], tmp[14]);
    tmp3[15] = _mm512_add_epi32(tmp[11], tmp[15]);
    tmp2[12] = _mm512_sub_epi32(tmp[8], tmp[12]);
    tmp2[13] = _mm512_sub_epi32(tmp[9], tmp[13]);
    tmp2[14] = _mm512_sub_epi32(tmp[10], tmp[14]);
    tmp2[15] = _mm512_sub_epi32(tmp[11], tmp[15]);

    //stage 6
    tmp[4]  = half_btf_avx512(&cospi16, &tmp2[4], &cospi48, &tmp2[5], &rounding, cos_bit);
    tmp[5]  = half_btf_avx512(&cospi48, &tmp2[4], &cospim16, &tmp2[5], &rounding, cos_bit);
    tmp[6]  = half_btf_avx512(&cospim48, &tmp2[6], &cospi16, &tmp2[7], &rounding, cos_bit);
    tmp[7]  = half_btf_avx512(&cospi16, &tmp2[6], &cospi48, &tmp2[7], &rounding, cos_bit);
    tmp[12] = half_btf_avx512(&cospi16, &tmp2[12], &cospi48, &tmp2[13], &rounding, cos_bit);
    tmp[13] = half_btf_avx512(&cospi48, &tmp2[12], &cospim16, &tmp2[13], &rounding, cos_bit);
    tmp[14] = half_btf_avx512(&cospim48, &tmp2[14], &cospi16, &tmp2[15], &rounding, cos_bit);
    tmp[15] = half_btf_avx512(&cospi16, &tmp2[14], &cospi48, &tmp2[15], &rounding, cos_bit);

    //stage 7
    out[0]   = _mm512_add_epi32(tmp3[8], tmp3[10]);
    out[2]   = _mm512_add_epi32(tmp[12], tmp[14]);
    out[12]  = _mm512_add_epi32(tmp[5], tmp[7]);
    out[14]  = _mm512_add_epi32(tmp3[13], tmp3[15]);
    tmp2[1]  = _mm512_add_epi32(tmp3[9], tmp3[11]);
    tmp2[2]  = _mm512_sub_epi32(tmp3[8], tmp3[10]);
    tmp2[3]  = _mm512_sub_epi32(tmp3[9], tmp3[11]);
    tmp2[4]  = _mm512_add_epi32(tmp[4], tmp[6]);
    tmp2[6]  = _mm512_sub_epi32(tmp[4], tmp[6]);
    tmp2[7]  = _mm512_sub_epi32(tmp[5], tmp[7]);
    tmp2[8]  = _mm512_add_epi32(tmp3[12], tmp3[14]);
    tmp2[10] = _mm512_sub_epi32(tmp3[12], tmp3[14]);
    tmp2[11] = _mm512_sub_epi32(tmp3[13], tmp3[15]);
    tmp2[13] = _mm512_add_epi32(tmp[13], tmp[15]);
    tmp2[14] = _mm512_sub_epi32(tmp[12], tmp[14]);
    tmp2[15] = _mm512_sub_epi32(tmp[13], tmp[15]);

    //stage 8
    out[4]  = half_btf_avx512(&cospi32, &tmp2[6], &cospi32, &tmp2[7], &rounding, cos_bit);
    out[6]  = half_btf_avx512(&cospi32, &tmp2[10], &cospi32, &tmp2[11], &rounding, cos_bit);
    out[8]  = half_btf_avx512(&cospi32, &tmp2[2], &cospim32, &tmp2[3], &rounding, cos_bit);
    out[10] = half_btf_avx512(&cospi32, &tmp2[14], &cospim32, &tmp2[15], &rounding, cos_bit);
    tmp[2]  = half_btf_avx512(&cospi32, &tmp2[2], &cospi32, &tmp2[3], &rounding, cos_bit);
    tmp[7]  = half_btf_avx512(&cospi32, &tmp2[6], &cospim32, &tmp2[7], &rounding, cos_bit);
    tmp[11] = half_btf_avx512(&cospi32, &tmp2[10], &cospim32, &tmp2[11], &rounding, cos_bit);
    tmp[14] = half_btf_avx512(&cospi32, &tmp2[14], &cospi32, &tmp2[15], &rounding, cos_bit);

    //stage 9
    temp1  = _mm512_castsi512_si256(tmp2[8]);
    temp2  = _mm512_extracti64x4_epi64(tmp2[8], ONE);
    temp1  = _mm256_sign_epi32(temp1, negative);
    temp2  = _mm256_sign_epi32(temp2, negative);
    out[1] = _mm512_castsi256_si512(temp1);
    out[1] = _mm512_inserti64x4(out[1], temp2, ONE);

    temp1  = _mm512_castsi512_si256(tmp2[4]);
    temp2  = _mm512_extracti64x4_epi64(tmp2[4], ONE);
    temp1  = _mm256_sign_epi32(temp1, negative);
    temp2  = _mm256_sign_epi32(temp2, negative);
    out[3] = _mm512_castsi256_si512(temp1);
    out[3] = _mm512_inserti64x4(out[3], temp2, ONE);

    temp1  = _mm512_castsi512_si256(tmp[14]);
    temp2  = _mm512_extracti64x4_epi64(tmp[14], ONE);
    temp1  = _mm256_sign_epi32(temp1, negative);
    temp2  = _mm256_sign_epi32(temp2, negative);
    out[5] = _mm512_castsi256_si512(temp1);
    out[5] = _mm512_inserti64x4(out[5], temp2, ONE);

    temp1  = _mm512_castsi512_si256(tmp[2]);
    temp2  = _mm512_extracti64x4_epi64(tmp[2], ONE);
    temp1  = _mm256_sign_epi32(temp1, negative);
    temp2  = _mm256_sign_epi32(temp2, negative);
    out[7] = _mm512_castsi256_si512(temp1);
    out[7] = _mm512_inserti64x4(out[7], temp2, ONE);

    temp1  = _mm512_castsi512_si256(tmp[11]);
    temp2  = _mm512_extracti64x4_epi64(tmp[11], ONE);
    temp1  = _mm256_sign_epi32(temp1, negative);
    temp2  = _mm256_sign_epi32(temp2, negative);
    out[9] = _mm512_castsi256_si512(temp1);
    out[9] = _mm512_inserti64x4(out[9], temp2, ONE);

    temp1   = _mm512_castsi512_si256(tmp[7]);
    temp2   = _mm512_extracti64x4_epi64(tmp[7], ONE);
    temp1   = _mm256_sign_epi32(temp1, negative);
    temp2   = _mm256_sign_epi32(temp2, negative);
    out[11] = _mm512_castsi256_si512(temp1);
    out[11] = _mm512_inserti64x4(out[11], temp2, ONE);

    temp1   = _mm512_castsi512_si256(tmp2[13]);
    temp2   = _mm512_extracti64x4_epi64(tmp2[13], ONE);
    temp1   = _mm256_sign_epi32(temp1, negative);
    temp2   = _mm256_sign_epi32(temp2, negative);
    out[13] = _mm512_castsi256_si512(temp1);
    out[13] = _mm512_inserti64x4(out[13], temp2, ONE);

    temp1   = _mm512_castsi512_si256(tmp2[1]);
    temp2   = _mm512_extracti64x4_epi64(tmp2[1], ONE);
    temp1   = _mm256_sign_epi32(temp1, negative);
    temp2   = _mm256_sign_epi32(temp2, negative);
    out[15] = _mm512_castsi256_si512(temp1);
    out[15] = _mm512_inserti64x4(out[15], temp2, ONE);
}

void svt_av1_inv_txfm2d_add_16x16_avx512(const int32_t* coeff, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                         int32_t stride_w, TxType tx_type, int32_t bd) {
    __m512i       in[16], out[16];
    const int8_t* shift   = svt_aom_inv_txfm_shift_ls[TX_16X16];
    const int32_t txw_idx = get_txw_idx(TX_16X16);
    const int32_t txh_idx = get_txh_idx(TX_16X16);

    switch (tx_type) {
    case IDTX:
        load_buffer_16x16_avx512(coeff, in);
        iidentity16_and_round_shift_avx512(in, -shift[0]);
        iidentity16_and_round_shift_avx512(in, -shift[1]);
        write_buffer_16x16_avx512(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;

    case V_DCT:
        load_buffer_16x16_avx512(coeff, in);
        iidentity16_and_round_shift_avx512(in, -shift[0]);
        idct16_col_avx512(in, out, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16_avx512(out, -shift[1]);
        write_buffer_16x16_avx512(out, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;

    case H_DCT:
        load_buffer_16x16_avx512(coeff, in);
        transpose_16x16_avx512(in, out);
        idct16_col_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        transpose_16x16_avx512(in, out);
        round_shift_16x16_avx512(out, -shift[0]);
        iidentity16_and_round_shift_avx512(out, -shift[1]);
        write_buffer_16x16_avx512(out, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;

    case V_ADST:
        load_buffer_16x16_avx512(coeff, in);
        iidentity16_and_round_shift_avx512(in, -shift[0]);
        iadst16_col_avx512(in, out, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16_avx512(out, -shift[1]);
        write_buffer_16x16_avx512(out, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;

    case H_ADST:
        load_buffer_16x16_avx512(coeff, in);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        transpose_16x16_avx512(in, out);
        round_shift_16x16_avx512(out, -shift[0]);
        iidentity16_and_round_shift_avx512(out, -shift[1]);
        write_buffer_16x16_avx512(out, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;

    case V_FLIPADST:
        load_buffer_16x16_avx512(coeff, in);
        iidentity16_and_round_shift_avx512(in, -shift[0]);
        iadst16_col_avx512(in, out, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16_avx512(out, -shift[1]);
        write_buffer_16x16_avx512(out, output_r, stride_r, output_w, stride_w, 0, 1, bd);
        break;

    case H_FLIPADST:
        load_buffer_16x16_avx512(coeff, in);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        transpose_16x16_avx512(in, out);
        round_shift_16x16_avx512(out, -shift[0]);
        iidentity16_and_round_shift_avx512(out, -shift[1]);
        write_buffer_16x16_avx512(out, output_r, stride_r, output_w, stride_w, 1, 0, bd);
        break;

    case DCT_DCT:
        load_buffer_16x16_avx512(coeff, in);
        transpose_16x16_avx512(in, out);
        idct16_col_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[0]);
        transpose_16x16_avx512(in, out);
        idct16_col_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[1]);
        write_buffer_16x16_avx512_new(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;

    case DCT_ADST:
        load_buffer_16x16_avx512(coeff, in);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[0]);
        transpose_16x16_avx512(in, out);
        idct16_col_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[1]);
        write_buffer_16x16_avx512_new(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;

    case ADST_DCT:
        load_buffer_16x16_avx512(coeff, in);
        transpose_16x16_avx512(in, out);
        idct16_col_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[0]);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[1]);
        write_buffer_16x16_avx512_new(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;

    case ADST_ADST:
        load_buffer_16x16_avx512(coeff, in);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[0]);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[1]);
        write_buffer_16x16_avx512_new(in, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;

    case FLIPADST_DCT:
        load_buffer_16x16_avx512(coeff, in);
        transpose_16x16_avx512(in, out);
        idct16_col_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[0]);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[1]);
        write_buffer_16x16_avx512_new(in, output_r, stride_r, output_w, stride_w, 0, 1, bd);
        break;

    case DCT_FLIPADST:
        load_buffer_16x16_avx512(coeff, in);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[0]);
        transpose_16x16_avx512(in, out);
        idct16_col_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[1]);
        write_buffer_16x16_avx512_new(in, output_r, stride_r, output_w, stride_w, 1, 0, bd);
        break;

    case ADST_FLIPADST:
        load_buffer_16x16_avx512(coeff, in);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[0]);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[1]);
        write_buffer_16x16_avx512_new(in, output_r, stride_r, output_w, stride_w, 1, 0, bd);
        break;

    case FLIPADST_FLIPADST:
        load_buffer_16x16_avx512(coeff, in);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[0]);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[1]);
        write_buffer_16x16_avx512_new(in, output_r, stride_r, output_w, stride_w, 1, 1, bd);
        break;

    case FLIPADST_ADST:
        load_buffer_16x16_avx512(coeff, in);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[0]);
        transpose_16x16_avx512(in, out);
        iadst16_col_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx]);
        round_shift_16x16_avx512(in, -shift[1]);
        write_buffer_16x16_avx512_new(in, output_r, stride_r, output_w, stride_w, 0, 1, bd);
        break;

    default:
        assert(0);
    }
}

static void load_buffer_32x32(const int32_t* coeff, __m512i* in) {
    int32_t i;
    for (i = 0; i < 64; ++i) {
        in[i] = _mm512_loadu_si512((const __m512i*)coeff);
        coeff += 16;
    }
}

static void idct32_avx512(__m512i* in, __m512i* out, const int8_t bit, int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m512i  cospi62  = _mm512_set1_epi32(cospi[62]);
    const __m512i  cospi30  = _mm512_set1_epi32(cospi[30]);
    const __m512i  cospi46  = _mm512_set1_epi32(cospi[46]);
    const __m512i  cospi14  = _mm512_set1_epi32(cospi[14]);
    const __m512i  cospi54  = _mm512_set1_epi32(cospi[54]);
    const __m512i  cospi22  = _mm512_set1_epi32(cospi[22]);
    const __m512i  cospi38  = _mm512_set1_epi32(cospi[38]);
    const __m512i  cospi6   = _mm512_set1_epi32(cospi[6]);
    const __m512i  cospi58  = _mm512_set1_epi32(cospi[58]);
    const __m512i  cospi26  = _mm512_set1_epi32(cospi[26]);
    const __m512i  cospi42  = _mm512_set1_epi32(cospi[42]);
    const __m512i  cospi10  = _mm512_set1_epi32(cospi[10]);
    const __m512i  cospi50  = _mm512_set1_epi32(cospi[50]);
    const __m512i  cospi18  = _mm512_set1_epi32(cospi[18]);
    const __m512i  cospi34  = _mm512_set1_epi32(cospi[34]);
    const __m512i  cospi2   = _mm512_set1_epi32(cospi[2]);
    const __m512i  cospim58 = _mm512_set1_epi32(-cospi[58]);
    const __m512i  cospim26 = _mm512_set1_epi32(-cospi[26]);
    const __m512i  cospim42 = _mm512_set1_epi32(-cospi[42]);
    const __m512i  cospim10 = _mm512_set1_epi32(-cospi[10]);
    const __m512i  cospim50 = _mm512_set1_epi32(-cospi[50]);
    const __m512i  cospim18 = _mm512_set1_epi32(-cospi[18]);
    const __m512i  cospim34 = _mm512_set1_epi32(-cospi[34]);
    const __m512i  cospim2  = _mm512_set1_epi32(-cospi[2]);
    const __m512i  cospi60  = _mm512_set1_epi32(cospi[60]);
    const __m512i  cospi28  = _mm512_set1_epi32(cospi[28]);
    const __m512i  cospi44  = _mm512_set1_epi32(cospi[44]);
    const __m512i  cospi12  = _mm512_set1_epi32(cospi[12]);
    const __m512i  cospi52  = _mm512_set1_epi32(cospi[52]);
    const __m512i  cospi20  = _mm512_set1_epi32(cospi[20]);
    const __m512i  cospi36  = _mm512_set1_epi32(cospi[36]);
    const __m512i  cospi4   = _mm512_set1_epi32(cospi[4]);
    const __m512i  cospim52 = _mm512_set1_epi32(-cospi[52]);
    const __m512i  cospim20 = _mm512_set1_epi32(-cospi[20]);
    const __m512i  cospim36 = _mm512_set1_epi32(-cospi[36]);
    const __m512i  cospim4  = _mm512_set1_epi32(-cospi[4]);
    const __m512i  cospi56  = _mm512_set1_epi32(cospi[56]);
    const __m512i  cospi24  = _mm512_set1_epi32(cospi[24]);
    const __m512i  cospi40  = _mm512_set1_epi32(cospi[40]);
    const __m512i  cospi8   = _mm512_set1_epi32(cospi[8]);
    const __m512i  cospim40 = _mm512_set1_epi32(-cospi[40]);
    const __m512i  cospim8  = _mm512_set1_epi32(-cospi[8]);
    const __m512i  cospim56 = _mm512_set1_epi32(-cospi[56]);
    const __m512i  cospim24 = _mm512_set1_epi32(-cospi[24]);
    const __m512i  cospi32  = _mm512_set1_epi32(cospi[32]);
    const __m512i  cospim32 = _mm512_set1_epi32(-cospi[32]);
    const __m512i  cospi48  = _mm512_set1_epi32(cospi[48]);
    const __m512i  cospim48 = _mm512_set1_epi32(-cospi[48]);
    const __m512i  cospi16  = _mm512_set1_epi32(cospi[16]);
    const __m512i  cospim16 = _mm512_set1_epi32(-cospi[16]);
    const __m512i  rounding = _mm512_set1_epi32(1 << (bit - 1));
    __m512i        bf1[32], bf0[32];
    int32_t        col;

    for (col = 0; col < col_num; ++col) {
        // stage 0
        // stage 1
        bf1[0]  = in[0 * col_num + col];
        bf1[1]  = in[16 * col_num + col];
        bf1[2]  = in[8 * col_num + col];
        bf1[3]  = in[24 * col_num + col];
        bf1[4]  = in[4 * col_num + col];
        bf1[5]  = in[20 * col_num + col];
        bf1[6]  = in[12 * col_num + col];
        bf1[7]  = in[28 * col_num + col];
        bf1[8]  = in[2 * col_num + col];
        bf1[9]  = in[18 * col_num + col];
        bf1[10] = in[10 * col_num + col];
        bf1[11] = in[26 * col_num + col];
        bf1[12] = in[6 * col_num + col];
        bf1[13] = in[22 * col_num + col];
        bf1[14] = in[14 * col_num + col];
        bf1[15] = in[30 * col_num + col];
        bf1[16] = in[1 * col_num + col];
        bf1[17] = in[17 * col_num + col];
        bf1[18] = in[9 * col_num + col];
        bf1[19] = in[25 * col_num + col];
        bf1[20] = in[5 * col_num + col];
        bf1[21] = in[21 * col_num + col];
        bf1[22] = in[13 * col_num + col];
        bf1[23] = in[29 * col_num + col];
        bf1[24] = in[3 * col_num + col];
        bf1[25] = in[19 * col_num + col];
        bf1[26] = in[11 * col_num + col];
        bf1[27] = in[27 * col_num + col];
        bf1[28] = in[7 * col_num + col];
        bf1[29] = in[23 * col_num + col];
        bf1[30] = in[15 * col_num + col];
        bf1[31] = in[31 * col_num + col];

        // stage 2
        bf0[0]  = bf1[0];
        bf0[1]  = bf1[1];
        bf0[2]  = bf1[2];
        bf0[3]  = bf1[3];
        bf0[4]  = bf1[4];
        bf0[5]  = bf1[5];
        bf0[6]  = bf1[6];
        bf0[7]  = bf1[7];
        bf0[8]  = bf1[8];
        bf0[9]  = bf1[9];
        bf0[10] = bf1[10];
        bf0[11] = bf1[11];
        bf0[12] = bf1[12];
        bf0[13] = bf1[13];
        bf0[14] = bf1[14];
        bf0[15] = bf1[15];
        bf0[16] = half_btf_avx512(&cospi62, &bf1[16], &cospim2, &bf1[31], &rounding, bit);
        bf0[17] = half_btf_avx512(&cospi30, &bf1[17], &cospim34, &bf1[30], &rounding, bit);
        bf0[18] = half_btf_avx512(&cospi46, &bf1[18], &cospim18, &bf1[29], &rounding, bit);
        bf0[19] = half_btf_avx512(&cospi14, &bf1[19], &cospim50, &bf1[28], &rounding, bit);
        bf0[20] = half_btf_avx512(&cospi54, &bf1[20], &cospim10, &bf1[27], &rounding, bit);
        bf0[21] = half_btf_avx512(&cospi22, &bf1[21], &cospim42, &bf1[26], &rounding, bit);
        bf0[22] = half_btf_avx512(&cospi38, &bf1[22], &cospim26, &bf1[25], &rounding, bit);
        bf0[23] = half_btf_avx512(&cospi6, &bf1[23], &cospim58, &bf1[24], &rounding, bit);
        bf0[24] = half_btf_avx512(&cospi58, &bf1[23], &cospi6, &bf1[24], &rounding, bit);
        bf0[25] = half_btf_avx512(&cospi26, &bf1[22], &cospi38, &bf1[25], &rounding, bit);
        bf0[26] = half_btf_avx512(&cospi42, &bf1[21], &cospi22, &bf1[26], &rounding, bit);
        bf0[27] = half_btf_avx512(&cospi10, &bf1[20], &cospi54, &bf1[27], &rounding, bit);
        bf0[28] = half_btf_avx512(&cospi50, &bf1[19], &cospi14, &bf1[28], &rounding, bit);
        bf0[29] = half_btf_avx512(&cospi18, &bf1[18], &cospi46, &bf1[29], &rounding, bit);
        bf0[30] = half_btf_avx512(&cospi34, &bf1[17], &cospi30, &bf1[30], &rounding, bit);
        bf0[31] = half_btf_avx512(&cospi2, &bf1[16], &cospi62, &bf1[31], &rounding, bit);

        // stage 3
        bf1[0]  = bf0[0];
        bf1[1]  = bf0[1];
        bf1[2]  = bf0[2];
        bf1[3]  = bf0[3];
        bf1[4]  = bf0[4];
        bf1[5]  = bf0[5];
        bf1[6]  = bf0[6];
        bf1[7]  = bf0[7];
        bf1[8]  = half_btf_avx512(&cospi60, &bf0[8], &cospim4, &bf0[15], &rounding, bit);
        bf1[9]  = half_btf_avx512(&cospi28, &bf0[9], &cospim36, &bf0[14], &rounding, bit);
        bf1[10] = half_btf_avx512(&cospi44, &bf0[10], &cospim20, &bf0[13], &rounding, bit);
        bf1[11] = half_btf_avx512(&cospi12, &bf0[11], &cospim52, &bf0[12], &rounding, bit);
        bf1[12] = half_btf_avx512(&cospi52, &bf0[11], &cospi12, &bf0[12], &rounding, bit);
        bf1[13] = half_btf_avx512(&cospi20, &bf0[10], &cospi44, &bf0[13], &rounding, bit);
        bf1[14] = half_btf_avx512(&cospi36, &bf0[9], &cospi28, &bf0[14], &rounding, bit);
        bf1[15] = half_btf_avx512(&cospi4, &bf0[8], &cospi60, &bf0[15], &rounding, bit);
        bf1[16] = _mm512_add_epi32(bf0[16], bf0[17]);
        bf1[17] = _mm512_sub_epi32(bf0[16], bf0[17]);
        bf1[18] = _mm512_sub_epi32(bf0[19], bf0[18]);
        bf1[19] = _mm512_add_epi32(bf0[18], bf0[19]);
        bf1[20] = _mm512_add_epi32(bf0[20], bf0[21]);
        bf1[21] = _mm512_sub_epi32(bf0[20], bf0[21]);
        bf1[22] = _mm512_sub_epi32(bf0[23], bf0[22]);
        bf1[23] = _mm512_add_epi32(bf0[22], bf0[23]);
        bf1[24] = _mm512_add_epi32(bf0[24], bf0[25]);
        bf1[25] = _mm512_sub_epi32(bf0[24], bf0[25]);
        bf1[26] = _mm512_sub_epi32(bf0[27], bf0[26]);
        bf1[27] = _mm512_add_epi32(bf0[26], bf0[27]);
        bf1[28] = _mm512_add_epi32(bf0[28], bf0[29]);
        bf1[29] = _mm512_sub_epi32(bf0[28], bf0[29]);
        bf1[30] = _mm512_sub_epi32(bf0[31], bf0[30]);
        bf1[31] = _mm512_add_epi32(bf0[30], bf0[31]);

        // stage 4
        bf0[0]  = bf1[0];
        bf0[1]  = bf1[1];
        bf0[2]  = bf1[2];
        bf0[3]  = bf1[3];
        bf0[4]  = half_btf_avx512(&cospi56, &bf1[4], &cospim8, &bf1[7], &rounding, bit);
        bf0[5]  = half_btf_avx512(&cospi24, &bf1[5], &cospim40, &bf1[6], &rounding, bit);
        bf0[6]  = half_btf_avx512(&cospi40, &bf1[5], &cospi24, &bf1[6], &rounding, bit);
        bf0[7]  = half_btf_avx512(&cospi8, &bf1[4], &cospi56, &bf1[7], &rounding, bit);
        bf0[8]  = _mm512_add_epi32(bf1[8], bf1[9]);
        bf0[9]  = _mm512_sub_epi32(bf1[8], bf1[9]);
        bf0[10] = _mm512_sub_epi32(bf1[11], bf1[10]);
        bf0[11] = _mm512_add_epi32(bf1[10], bf1[11]);
        bf0[12] = _mm512_add_epi32(bf1[12], bf1[13]);
        bf0[13] = _mm512_sub_epi32(bf1[12], bf1[13]);
        bf0[14] = _mm512_sub_epi32(bf1[15], bf1[14]);
        bf0[15] = _mm512_add_epi32(bf1[14], bf1[15]);
        bf0[16] = bf1[16];
        bf0[17] = half_btf_avx512(&cospim8, &bf1[17], &cospi56, &bf1[30], &rounding, bit);
        bf0[18] = half_btf_avx512(&cospim56, &bf1[18], &cospim8, &bf1[29], &rounding, bit);
        bf0[19] = bf1[19];
        bf0[20] = bf1[20];
        bf0[21] = half_btf_avx512(&cospim40, &bf1[21], &cospi24, &bf1[26], &rounding, bit);
        bf0[22] = half_btf_avx512(&cospim24, &bf1[22], &cospim40, &bf1[25], &rounding, bit);
        bf0[23] = bf1[23];
        bf0[24] = bf1[24];
        bf0[25] = half_btf_avx512(&cospim40, &bf1[22], &cospi24, &bf1[25], &rounding, bit);
        bf0[26] = half_btf_avx512(&cospi24, &bf1[21], &cospi40, &bf1[26], &rounding, bit);
        bf0[27] = bf1[27];
        bf0[28] = bf1[28];
        bf0[29] = half_btf_avx512(&cospim8, &bf1[18], &cospi56, &bf1[29], &rounding, bit);
        bf0[30] = half_btf_avx512(&cospi56, &bf1[17], &cospi8, &bf1[30], &rounding, bit);
        bf0[31] = bf1[31];

        // stage 5
        bf1[0]  = half_btf_avx512(&cospi32, &bf0[0], &cospi32, &bf0[1], &rounding, bit);
        bf1[1]  = half_btf_avx512(&cospi32, &bf0[0], &cospim32, &bf0[1], &rounding, bit);
        bf1[2]  = half_btf_avx512(&cospi48, &bf0[2], &cospim16, &bf0[3], &rounding, bit);
        bf1[3]  = half_btf_avx512(&cospi16, &bf0[2], &cospi48, &bf0[3], &rounding, bit);
        bf1[4]  = _mm512_add_epi32(bf0[4], bf0[5]);
        bf1[5]  = _mm512_sub_epi32(bf0[4], bf0[5]);
        bf1[6]  = _mm512_sub_epi32(bf0[7], bf0[6]);
        bf1[7]  = _mm512_add_epi32(bf0[6], bf0[7]);
        bf1[8]  = bf0[8];
        bf1[9]  = half_btf_avx512(&cospim16, &bf0[9], &cospi48, &bf0[14], &rounding, bit);
        bf1[10] = half_btf_avx512(&cospim48, &bf0[10], &cospim16, &bf0[13], &rounding, bit);
        bf1[11] = bf0[11];
        bf1[12] = bf0[12];
        bf1[13] = half_btf_avx512(&cospim16, &bf0[10], &cospi48, &bf0[13], &rounding, bit);
        bf1[14] = half_btf_avx512(&cospi48, &bf0[9], &cospi16, &bf0[14], &rounding, bit);
        bf1[15] = bf0[15];
        bf1[16] = _mm512_add_epi32(bf0[16], bf0[19]);
        bf1[17] = _mm512_add_epi32(bf0[17], bf0[18]);
        bf1[18] = _mm512_sub_epi32(bf0[17], bf0[18]);
        bf1[19] = _mm512_sub_epi32(bf0[16], bf0[19]);
        bf1[20] = _mm512_sub_epi32(bf0[23], bf0[20]);
        bf1[21] = _mm512_sub_epi32(bf0[22], bf0[21]);
        bf1[22] = _mm512_add_epi32(bf0[21], bf0[22]);
        bf1[23] = _mm512_add_epi32(bf0[20], bf0[23]);
        bf1[24] = _mm512_add_epi32(bf0[24], bf0[27]);
        bf1[25] = _mm512_add_epi32(bf0[25], bf0[26]);
        bf1[26] = _mm512_sub_epi32(bf0[25], bf0[26]);
        bf1[27] = _mm512_sub_epi32(bf0[24], bf0[27]);
        bf1[28] = _mm512_sub_epi32(bf0[31], bf0[28]);
        bf1[29] = _mm512_sub_epi32(bf0[30], bf0[29]);
        bf1[30] = _mm512_add_epi32(bf0[29], bf0[30]);
        bf1[31] = _mm512_add_epi32(bf0[28], bf0[31]);

        // stage 6
        bf0[0]  = _mm512_add_epi32(bf1[0], bf1[3]);
        bf0[1]  = _mm512_add_epi32(bf1[1], bf1[2]);
        bf0[2]  = _mm512_sub_epi32(bf1[1], bf1[2]);
        bf0[3]  = _mm512_sub_epi32(bf1[0], bf1[3]);
        bf0[4]  = bf1[4];
        bf0[5]  = half_btf_avx512(&cospim32, &bf1[5], &cospi32, &bf1[6], &rounding, bit);
        bf0[6]  = half_btf_avx512(&cospi32, &bf1[5], &cospi32, &bf1[6], &rounding, bit);
        bf0[7]  = bf1[7];
        bf0[8]  = _mm512_add_epi32(bf1[8], bf1[11]);
        bf0[9]  = _mm512_add_epi32(bf1[9], bf1[10]);
        bf0[10] = _mm512_sub_epi32(bf1[9], bf1[10]);
        bf0[11] = _mm512_sub_epi32(bf1[8], bf1[11]);
        bf0[12] = _mm512_sub_epi32(bf1[15], bf1[12]);
        bf0[13] = _mm512_sub_epi32(bf1[14], bf1[13]);
        bf0[14] = _mm512_add_epi32(bf1[13], bf1[14]);
        bf0[15] = _mm512_add_epi32(bf1[12], bf1[15]);
        bf0[16] = bf1[16];
        bf0[17] = bf1[17];
        bf0[18] = half_btf_avx512(&cospim16, &bf1[18], &cospi48, &bf1[29], &rounding, bit);
        bf0[19] = half_btf_avx512(&cospim16, &bf1[19], &cospi48, &bf1[28], &rounding, bit);
        bf0[20] = half_btf_avx512(&cospim48, &bf1[20], &cospim16, &bf1[27], &rounding, bit);
        bf0[21] = half_btf_avx512(&cospim48, &bf1[21], &cospim16, &bf1[26], &rounding, bit);
        bf0[22] = bf1[22];
        bf0[23] = bf1[23];
        bf0[24] = bf1[24];
        bf0[25] = bf1[25];
        bf0[26] = half_btf_avx512(&cospim16, &bf1[21], &cospi48, &bf1[26], &rounding, bit);
        bf0[27] = half_btf_avx512(&cospim16, &bf1[20], &cospi48, &bf1[27], &rounding, bit);
        bf0[28] = half_btf_avx512(&cospi48, &bf1[19], &cospi16, &bf1[28], &rounding, bit);
        bf0[29] = half_btf_avx512(&cospi48, &bf1[18], &cospi16, &bf1[29], &rounding, bit);
        bf0[30] = bf1[30];
        bf0[31] = bf1[31];

        // stage 7
        bf1[0]  = _mm512_add_epi32(bf0[0], bf0[7]);
        bf1[1]  = _mm512_add_epi32(bf0[1], bf0[6]);
        bf1[2]  = _mm512_add_epi32(bf0[2], bf0[5]);
        bf1[3]  = _mm512_add_epi32(bf0[3], bf0[4]);
        bf1[4]  = _mm512_sub_epi32(bf0[3], bf0[4]);
        bf1[5]  = _mm512_sub_epi32(bf0[2], bf0[5]);
        bf1[6]  = _mm512_sub_epi32(bf0[1], bf0[6]);
        bf1[7]  = _mm512_sub_epi32(bf0[0], bf0[7]);
        bf1[8]  = bf0[8];
        bf1[9]  = bf0[9];
        bf1[10] = half_btf_avx512(&cospim32, &bf0[10], &cospi32, &bf0[13], &rounding, bit);
        bf1[11] = half_btf_avx512(&cospim32, &bf0[11], &cospi32, &bf0[12], &rounding, bit);
        bf1[12] = half_btf_avx512(&cospi32, &bf0[11], &cospi32, &bf0[12], &rounding, bit);
        bf1[13] = half_btf_avx512(&cospi32, &bf0[10], &cospi32, &bf0[13], &rounding, bit);
        bf1[14] = bf0[14];
        bf1[15] = bf0[15];
        bf1[16] = _mm512_add_epi32(bf0[16], bf0[23]);
        bf1[17] = _mm512_add_epi32(bf0[17], bf0[22]);
        bf1[18] = _mm512_add_epi32(bf0[18], bf0[21]);
        bf1[19] = _mm512_add_epi32(bf0[19], bf0[20]);
        bf1[20] = _mm512_sub_epi32(bf0[19], bf0[20]);
        bf1[21] = _mm512_sub_epi32(bf0[18], bf0[21]);
        bf1[22] = _mm512_sub_epi32(bf0[17], bf0[22]);
        bf1[23] = _mm512_sub_epi32(bf0[16], bf0[23]);
        bf1[24] = _mm512_sub_epi32(bf0[31], bf0[24]);
        bf1[25] = _mm512_sub_epi32(bf0[30], bf0[25]);
        bf1[26] = _mm512_sub_epi32(bf0[29], bf0[26]);
        bf1[27] = _mm512_sub_epi32(bf0[28], bf0[27]);
        bf1[28] = _mm512_add_epi32(bf0[27], bf0[28]);
        bf1[29] = _mm512_add_epi32(bf0[26], bf0[29]);
        bf1[30] = _mm512_add_epi32(bf0[25], bf0[30]);
        bf1[31] = _mm512_add_epi32(bf0[24], bf0[31]);

        // stage 8
        bf0[0]  = _mm512_add_epi32(bf1[0], bf1[15]);
        bf0[1]  = _mm512_add_epi32(bf1[1], bf1[14]);
        bf0[2]  = _mm512_add_epi32(bf1[2], bf1[13]);
        bf0[3]  = _mm512_add_epi32(bf1[3], bf1[12]);
        bf0[4]  = _mm512_add_epi32(bf1[4], bf1[11]);
        bf0[5]  = _mm512_add_epi32(bf1[5], bf1[10]);
        bf0[6]  = _mm512_add_epi32(bf1[6], bf1[9]);
        bf0[7]  = _mm512_add_epi32(bf1[7], bf1[8]);
        bf0[8]  = _mm512_sub_epi32(bf1[7], bf1[8]);
        bf0[9]  = _mm512_sub_epi32(bf1[6], bf1[9]);
        bf0[10] = _mm512_sub_epi32(bf1[5], bf1[10]);
        bf0[11] = _mm512_sub_epi32(bf1[4], bf1[11]);
        bf0[12] = _mm512_sub_epi32(bf1[3], bf1[12]);
        bf0[13] = _mm512_sub_epi32(bf1[2], bf1[13]);
        bf0[14] = _mm512_sub_epi32(bf1[1], bf1[14]);
        bf0[15] = _mm512_sub_epi32(bf1[0], bf1[15]);
        bf0[16] = bf1[16];
        bf0[17] = bf1[17];
        bf0[18] = bf1[18];
        bf0[19] = bf1[19];
        bf0[20] = half_btf_avx512(&cospim32, &bf1[20], &cospi32, &bf1[27], &rounding, bit);
        bf0[21] = half_btf_avx512(&cospim32, &bf1[21], &cospi32, &bf1[26], &rounding, bit);
        bf0[22] = half_btf_avx512(&cospim32, &bf1[22], &cospi32, &bf1[25], &rounding, bit);
        bf0[23] = half_btf_avx512(&cospim32, &bf1[23], &cospi32, &bf1[24], &rounding, bit);
        bf0[24] = half_btf_avx512(&cospi32, &bf1[23], &cospi32, &bf1[24], &rounding, bit);
        bf0[25] = half_btf_avx512(&cospi32, &bf1[22], &cospi32, &bf1[25], &rounding, bit);
        bf0[26] = half_btf_avx512(&cospi32, &bf1[21], &cospi32, &bf1[26], &rounding, bit);
        bf0[27] = half_btf_avx512(&cospi32, &bf1[20], &cospi32, &bf1[27], &rounding, bit);
        bf0[28] = bf1[28];
        bf0[29] = bf1[29];
        bf0[30] = bf1[30];
        bf0[31] = bf1[31];

        // stage 9
        out[0 * col_num + col]  = _mm512_add_epi32(bf0[0], bf0[31]);
        out[1 * col_num + col]  = _mm512_add_epi32(bf0[1], bf0[30]);
        out[2 * col_num + col]  = _mm512_add_epi32(bf0[2], bf0[29]);
        out[3 * col_num + col]  = _mm512_add_epi32(bf0[3], bf0[28]);
        out[4 * col_num + col]  = _mm512_add_epi32(bf0[4], bf0[27]);
        out[5 * col_num + col]  = _mm512_add_epi32(bf0[5], bf0[26]);
        out[6 * col_num + col]  = _mm512_add_epi32(bf0[6], bf0[25]);
        out[7 * col_num + col]  = _mm512_add_epi32(bf0[7], bf0[24]);
        out[8 * col_num + col]  = _mm512_add_epi32(bf0[8], bf0[23]);
        out[9 * col_num + col]  = _mm512_add_epi32(bf0[9], bf0[22]);
        out[10 * col_num + col] = _mm512_add_epi32(bf0[10], bf0[21]);
        out[11 * col_num + col] = _mm512_add_epi32(bf0[11], bf0[20]);
        out[12 * col_num + col] = _mm512_add_epi32(bf0[12], bf0[19]);
        out[13 * col_num + col] = _mm512_add_epi32(bf0[13], bf0[18]);
        out[14 * col_num + col] = _mm512_add_epi32(bf0[14], bf0[17]);
        out[15 * col_num + col] = _mm512_add_epi32(bf0[15], bf0[16]);
        out[16 * col_num + col] = _mm512_sub_epi32(bf0[15], bf0[16]);
        out[17 * col_num + col] = _mm512_sub_epi32(bf0[14], bf0[17]);
        out[18 * col_num + col] = _mm512_sub_epi32(bf0[13], bf0[18]);
        out[19 * col_num + col] = _mm512_sub_epi32(bf0[12], bf0[19]);
        out[20 * col_num + col] = _mm512_sub_epi32(bf0[11], bf0[20]);
        out[21 * col_num + col] = _mm512_sub_epi32(bf0[10], bf0[21]);
        out[22 * col_num + col] = _mm512_sub_epi32(bf0[9], bf0[22]);
        out[23 * col_num + col] = _mm512_sub_epi32(bf0[8], bf0[23]);
        out[24 * col_num + col] = _mm512_sub_epi32(bf0[7], bf0[24]);
        out[25 * col_num + col] = _mm512_sub_epi32(bf0[6], bf0[25]);
        out[26 * col_num + col] = _mm512_sub_epi32(bf0[5], bf0[26]);
        out[27 * col_num + col] = _mm512_sub_epi32(bf0[4], bf0[27]);
        out[28 * col_num + col] = _mm512_sub_epi32(bf0[3], bf0[28]);
        out[29 * col_num + col] = _mm512_sub_epi32(bf0[2], bf0[29]);
        out[30 * col_num + col] = _mm512_sub_epi32(bf0[1], bf0[30]);
        out[31 * col_num + col] = _mm512_sub_epi32(bf0[0], bf0[31]);
    }
}

static INLINE void round_shift_32x32_avx512(__m512i* in, int32_t shift) {
    uint8_t ushift = (uint8_t)shift;
    __m512i rnding = _mm512_set1_epi32(1 << (ushift - 1));
    int32_t i      = 0;

    while (i < 64) {
        in[i] = _mm512_add_epi32(in[i], rnding);
        in[i] = _mm512_srai_epi32(in[i], ushift);
        i++;
    }
}

static void swap_addr(uint16_t** output1, uint16_t** output2) {
    uint16_t* tmp;
    tmp      = *output1;
    *output1 = *output2;
    *output2 = tmp;
}

static void assign_16x16_input_from_32x32_avx512(const __m512i* in, __m512i* in16x16, int32_t col) {
    int32_t i;
    for (i = 0; i < 16; i += 1) {
        in16x16[i] = in[col];
        col += 2;
    }
}

static void write_buffer_32x32_avx512_new(__m512i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                          int32_t stride_w, int32_t fliplr, int32_t flipud, int32_t bd) {
    __m512i   in16x16[16]; /*load 16x16 blocks*/
    uint16_t* left_up_r    = &output_r[0];
    uint16_t* right_up_r   = &output_r[16];
    uint16_t* left_down_r  = &output_r[16 * stride_r];
    uint16_t* right_down_r = &output_r[16 * stride_r + 16];
    uint16_t* left_up_w    = &output_w[0];
    uint16_t* right_up_w   = &output_w[16];
    uint16_t* left_down_w  = &output_w[16 * stride_w];
    uint16_t* right_down_w = &output_w[16 * stride_w + 16];

    if (fliplr) {
        swap_addr(&left_up_r, &right_up_r);
        swap_addr(&left_down_r, &right_down_r);
        swap_addr(&left_up_w, &right_up_w);
        swap_addr(&left_down_w, &right_down_w);
    }

    if (flipud) {
        swap_addr(&left_up_r, &left_down_r);
        swap_addr(&right_up_r, &right_down_r);
        swap_addr(&left_up_w, &left_down_w);
        swap_addr(&right_up_w, &right_down_w);
    }

    // Left-up quarter
    assign_16x16_input_from_32x32_avx512(in, in16x16, 0);
    write_buffer_16x16_avx512_new(in16x16, left_up_r, stride_r, left_up_w, stride_w, fliplr, flipud, bd);

    // Right-up quarter
    assign_16x16_input_from_32x32_avx512(in, in16x16, 32 / 2 / 16);
    write_buffer_16x16_avx512_new(in16x16, right_up_r, stride_r, right_up_w, stride_w, fliplr, flipud, bd);

    // Left-down quarter
    assign_16x16_input_from_32x32_avx512(in, in16x16, 32 * 32 / 2 / 16);
    write_buffer_16x16_avx512_new(in16x16, left_down_r, stride_r, left_down_w, stride_w, fliplr, flipud, bd);

    // Right-down quarter
    assign_16x16_input_from_32x32_avx512(in, in16x16, 32 * 32 / 2 / 16 + 32 / 2 / 16);
    write_buffer_16x16_avx512_new(in16x16, right_down_r, stride_r, right_down_w, stride_w, fliplr, flipud, bd);
}

static void write_buffer_32x32_avx512(__m512i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                      int32_t stride_w, int32_t fliplr, int32_t flipud, int32_t bd) {
    __m512i   in16x16[16]; /*load 16x16 blocks*/
    uint16_t* left_up_r    = &output_r[0];
    uint16_t* right_up_r   = &output_r[16];
    uint16_t* left_down_r  = &output_r[16 * stride_r];
    uint16_t* right_down_r = &output_r[16 * stride_r + 16];
    uint16_t* left_up_w    = &output_w[0];
    uint16_t* right_up_w   = &output_w[16];
    uint16_t* left_down_w  = &output_w[16 * stride_w];
    uint16_t* right_down_w = &output_w[16 * stride_w + 16];

    if (fliplr) {
        swap_addr(&left_up_r, &right_up_r);
        swap_addr(&left_down_r, &right_down_r);
        swap_addr(&left_up_w, &right_up_w);
        swap_addr(&left_down_w, &right_down_w);
    }

    if (flipud) {
        swap_addr(&left_up_r, &left_down_r);
        swap_addr(&right_up_r, &right_down_r);
        swap_addr(&left_up_w, &left_down_w);
        swap_addr(&right_up_w, &right_down_w);
    }

    // Left-up quarter
    assign_16x16_input_from_32x32_avx512(in, in16x16, 0);
    write_buffer_16x16_avx512(in16x16, left_up_r, stride_r, left_up_w, stride_w, fliplr, flipud, bd);

    // Right-up quarter
    assign_16x16_input_from_32x32_avx512(in, in16x16, 32 / 2 / 16);
    write_buffer_16x16_avx512(in16x16, right_up_r, stride_r, right_up_w, stride_w, fliplr, flipud, bd);

    // Left-down quarter
    assign_16x16_input_from_32x32_avx512(in, in16x16, 32 * 32 / 2 / 16);
    write_buffer_16x16_avx512(in16x16, left_down_r, stride_r, left_down_w, stride_w, fliplr, flipud, bd);

    // Right-down quarter
    assign_16x16_input_from_32x32_avx512(in, in16x16, 32 * 32 / 2 / 16 + 32 / 2 / 16);
    write_buffer_16x16_avx512(in16x16, right_down_r, stride_r, right_down_w, stride_w, fliplr, flipud, bd);
}

static void assign_32x32_input_from_64x64_avx512(const __m512i* in, __m512i* in32x32, int32_t col) {
    int32_t i;
    for (i = 0; i < 32 * 32 / 16; i += 2) {
        in32x32[i]     = in[col];
        in32x32[i + 1] = in[col + 1];
        col += 4;
    }
}

void svt_av1_inv_txfm2d_add_32x32_avx512(const int32_t* coeff, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                         int32_t stride_w, TxType tx_type, int32_t bd) {
    __m512i       in_avx512[64], out_avx512[128];
    const int8_t* shift     = svt_aom_inv_txfm_shift_ls[TX_32X32];
    const int32_t txw_idx   = get_txw_idx(TX_32X32);
    const int32_t txh_idx   = get_txh_idx(TX_32X32);
    const int32_t txfm_size = 32;
    switch (tx_type) {
    case DCT_DCT:
        load_buffer_32x32(coeff, in_avx512);
        transpose_16nx16n_avx512(txfm_size, in_avx512, out_avx512);
        idct32_avx512(out_avx512, in_avx512, inv_cos_bit_row[txw_idx][txh_idx], 2);
        round_shift_32x32_avx512(in_avx512, -shift[0]);
        transpose_16nx16n_avx512(txfm_size, in_avx512, out_avx512);
        idct32_avx512(out_avx512, in_avx512, inv_cos_bit_col[txw_idx][txh_idx], 2);
        round_shift_32x32_avx512(in_avx512, -shift[1]);
        write_buffer_32x32_avx512(in_avx512, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    case IDTX:
        load_buffer_32x32(coeff, in_avx512);
        round_shift_32x32_avx512(in_avx512, -shift[0] - shift[1] - 4);
        write_buffer_32x32_avx512(in_avx512, output_r, stride_r, output_w, stride_w, 0, 0, bd);
        break;
    default:
        assert(0);
    }
}

static void write_buffer_64x64_avx512(__m512i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                      int32_t stride_w, int32_t fliplr, int32_t flipud, int32_t shift, int32_t bd) {
    __m512i   in32x32[32 * 32 / 16];
    uint16_t* left_up_r    = &output_r[0];
    uint16_t* right_up_r   = &output_r[32];
    uint16_t* left_down_r  = &output_r[32 * stride_r];
    uint16_t* right_down_r = &output_r[32 * stride_r + 32];
    uint16_t* left_up_w    = &output_w[0];
    uint16_t* right_up_w   = &output_w[32];
    uint16_t* left_down_w  = &output_w[32 * stride_w];
    uint16_t* right_down_w = &output_w[32 * stride_w + 32];

    if (fliplr) {
        swap_addr(&left_up_r, &right_up_r);
        swap_addr(&left_down_r, &right_down_r);
        swap_addr(&left_up_w, &right_up_w);
        swap_addr(&left_down_w, &right_down_w);
    }

    if (flipud) {
        swap_addr(&left_up_r, &left_down_r);
        swap_addr(&right_up_r, &right_down_r);
        swap_addr(&left_up_w, &left_down_w);
        swap_addr(&right_up_w, &right_down_w);
    }

    // Left-up quarter
    assign_32x32_input_from_64x64_avx512(in, in32x32, 0);
    round_shift_32x32_avx512(in32x32, shift);
    write_buffer_32x32_avx512_new(in32x32, left_up_r, stride_r, left_up_w, stride_w, fliplr, flipud, bd);

    // Right-up quarter
    assign_32x32_input_from_64x64_avx512(in, in32x32, 64 / 2 / 16);
    round_shift_32x32_avx512(in32x32, shift);
    write_buffer_32x32_avx512_new(in32x32, right_up_r, stride_r, right_up_w, stride_w, fliplr, flipud, bd);

    // Left-down quarter
    assign_32x32_input_from_64x64_avx512(in, in32x32, 64 * 64 / 2 / 16);
    round_shift_32x32_avx512(in32x32, shift);
    write_buffer_32x32_avx512_new(in32x32, left_down_r, stride_r, left_down_w, stride_w, fliplr, flipud, bd);

    // Right-down quarter
    assign_32x32_input_from_64x64_avx512(in, in32x32, 64 * 64 / 2 / 16 + 64 / 2 / 16);
    round_shift_32x32_avx512(in32x32, shift);
    write_buffer_32x32_avx512_new(in32x32, right_down_r, stride_r, right_down_w, stride_w, fliplr, flipud, bd);
}

void svt_av1_inv_txfm2d_add_64x64_avx512(const int32_t* coeff, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                         int32_t stride_w, TxType tx_type, int32_t bd) {
    __m512i       in[64 * 64 / 16], out[64 * 64 / 16];
    const int8_t* shift   = svt_aom_inv_txfm_shift_ls[TX_64X64];
    const int32_t txw_idx = tx_size_wide_log2[TX_64X64] - tx_size_wide_log2[0];
    const int32_t txh_idx = tx_size_high_log2[TX_64X64] - tx_size_high_log2[0];

    switch (tx_type) {
    case DCT_DCT:
        load_buffer_64x64_lower_32x32_avx512(coeff, in);
        transpose_16nx16n_avx512(64, in, out);
        idct64x64_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx], 0, bd);
        // transpose before shift, so shift can apply to 128 contiguous values
        transpose_16nx16n_avx512(64, in, out);
        round_shift_64x64_avx512(out, -shift[0]);
        idct64x64_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx], 1, bd);
        write_buffer_64x64_avx512(in, output_r, stride_r, output_w, stride_w, 0, 0, -shift[1], bd);
        break;

    default:
        svt_av1_inv_txfm2d_add_64x64_c(coeff, output_r, stride_r, output_w, stride_w, tx_type, bd);
        break;
    }
}

static void load_buffer_16_avx512(const int32_t* coeff, __m512i* in) {
    in[0] = _mm512_loadu_si512((const __m512i*)coeff);
}

static void load_buffer_64_avx512(const int32_t* coeff, __m512i* in) {
    int32_t i;
    for (i = 0; i < 4; i++) {
        in[i] = _mm512_loadu_si512((const __m512i*)coeff);
        coeff += 16;
    }
}

static INLINE void idct16_avx512(__m512i* in, __m512i* out, const int8_t bit, int32_t col_num) {
    const int32_t* cospi    = cospi_arr(bit);
    const __m512i  cospi60  = _mm512_set1_epi32(cospi[60]);
    const __m512i  cospi28  = _mm512_set1_epi32(cospi[28]);
    const __m512i  cospi44  = _mm512_set1_epi32(cospi[44]);
    const __m512i  cospi12  = _mm512_set1_epi32(cospi[12]);
    const __m512i  cospi52  = _mm512_set1_epi32(cospi[52]);
    const __m512i  cospi20  = _mm512_set1_epi32(cospi[20]);
    const __m512i  cospi36  = _mm512_set1_epi32(cospi[36]);
    const __m512i  cospi4   = _mm512_set1_epi32(cospi[4]);
    const __m512i  cospi56  = _mm512_set1_epi32(cospi[56]);
    const __m512i  cospi24  = _mm512_set1_epi32(cospi[24]);
    const __m512i  cospi40  = _mm512_set1_epi32(cospi[40]);
    const __m512i  cospi8   = _mm512_set1_epi32(cospi[8]);
    const __m512i  cospi32  = _mm512_set1_epi32(cospi[32]);
    const __m512i  cospi48  = _mm512_set1_epi32(cospi[48]);
    const __m512i  cospi16  = _mm512_set1_epi32(cospi[16]);
    const __m512i  cospim4  = _mm512_set1_epi32(-cospi[4]);
    const __m512i  cospim36 = _mm512_set1_epi32(-cospi[36]);
    const __m512i  cospim20 = _mm512_set1_epi32(-cospi[20]);
    const __m512i  cospim52 = _mm512_set1_epi32(-cospi[52]);
    const __m512i  cospim8  = _mm512_set1_epi32(-cospi[8]);
    const __m512i  cospim40 = _mm512_set1_epi32(-cospi[40]);
    const __m512i  cospim32 = _mm512_set1_epi32(-cospi[32]);
    const __m512i  cospim48 = _mm512_set1_epi32(-cospi[48]);
    const __m512i  cospim16 = _mm512_set1_epi32(-cospi[16]);
    const __m512i  rounding = _mm512_set1_epi32(1 << (bit - 1));
    __m512i        tmp[16], tmp2[16];
    int32_t        col;

    for (col = 0; col < col_num; ++col) {
        //stage 1

        //stage 2
        tmp[8]  = half_btf_avx512(&cospi60, &in[1 * col_num + col], &cospim4, &in[15 * col_num + col], &rounding, bit);
        tmp[9]  = half_btf_avx512(&cospi28, &in[9 * col_num + col], &cospim36, &in[7 * col_num + col], &rounding, bit);
        tmp[10] = half_btf_avx512(&cospi44, &in[5 * col_num + col], &cospim20, &in[11 * col_num + col], &rounding, bit);
        tmp[11] = half_btf_avx512(&cospi12, &in[13 * col_num + col], &cospim52, &in[3 * col_num + col], &rounding, bit);
        tmp[12] = half_btf_avx512(&cospi52, &in[13 * col_num + col], &cospi12, &in[3 * col_num + col], &rounding, bit);
        tmp[13] = half_btf_avx512(&cospi20, &in[5 * col_num + col], &cospi44, &in[11 * col_num + col], &rounding, bit);
        tmp[14] = half_btf_avx512(&cospi36, &in[9 * col_num + col], &cospi28, &in[7 * col_num + col], &rounding, bit);
        tmp[15] = half_btf_avx512(&cospi4, &in[1 * col_num + col], &cospi60, &in[15 * col_num + col], &rounding, bit);

        //stage 3
        tmp2[0] = half_btf_avx512(&cospi56, &in[2 * col_num + col], &cospim8, &in[14 * col_num + col], &rounding, bit);
        tmp2[1] = half_btf_avx512(&cospi24, &in[10 * col_num + col], &cospim40, &in[6 * col_num + col], &rounding, bit);
        tmp2[2] = half_btf_avx512(&cospi40, &in[10 * col_num + col], &cospi24, &in[6 * col_num + col], &rounding, bit);
        tmp2[3] = half_btf_avx512(&cospi8, &in[2 * col_num + col], &cospi56, &in[14 * col_num + col], &rounding, bit);
        tmp2[4] = _mm512_add_epi32(tmp[8], tmp[9]);
        tmp2[5] = _mm512_sub_epi32(tmp[8], tmp[9]);
        tmp2[6] = _mm512_sub_epi32(tmp[11], tmp[10]);
        tmp2[7] = _mm512_add_epi32(tmp[10], tmp[11]);
        tmp2[8] = _mm512_add_epi32(tmp[12], tmp[13]);
        tmp2[9] = _mm512_sub_epi32(tmp[12], tmp[13]);
        tmp2[10] = _mm512_sub_epi32(tmp[15], tmp[14]);
        tmp2[11] = _mm512_add_epi32(tmp[14], tmp[15]);

        //stage 4
        tmp[0]  = half_btf_avx512(&cospi32, &in[0 * col_num + col], &cospi32, &in[8 * col_num + col], &rounding, bit);
        tmp[1]  = half_btf_avx512(&cospi32, &in[0 * col_num + col], &cospim32, &in[8 * col_num + col], &rounding, bit);
        tmp[2]  = half_btf_avx512(&cospi48, &in[4 * col_num + col], &cospim16, &in[12 * col_num + col], &rounding, bit);
        tmp[3]  = half_btf_avx512(&cospi16, &in[4 * col_num + col], &cospi48, &in[12 * col_num + col], &rounding, bit);
        tmp[4]  = _mm512_add_epi32(tmp2[0], tmp2[1]);
        tmp[5]  = _mm512_sub_epi32(tmp2[0], tmp2[1]);
        tmp[6]  = _mm512_sub_epi32(tmp2[3], tmp2[2]);
        tmp[7]  = _mm512_add_epi32(tmp2[2], tmp2[3]);
        tmp[9]  = half_btf_avx512(&cospim16, &tmp2[5], &cospi48, &tmp2[10], &rounding, bit);
        tmp[10] = half_btf_avx512(&cospim48, &tmp2[6], &cospim16, &tmp2[9], &rounding, bit);
        tmp[13] = half_btf_avx512(&cospim16, &tmp2[6], &cospi48, &tmp2[9], &rounding, bit);
        tmp[14] = half_btf_avx512(&cospi48, &tmp2[5], &cospi16, &tmp2[10], &rounding, bit);

        //stage 5
        tmp2[12] = _mm512_sub_epi32(tmp2[11], tmp2[8]);
        tmp2[15] = _mm512_add_epi32(tmp2[8], tmp2[11]);
        tmp2[8]  = _mm512_add_epi32(tmp2[4], tmp2[7]);
        tmp2[11] = _mm512_sub_epi32(tmp2[4], tmp2[7]);
        tmp2[0]  = _mm512_add_epi32(tmp[0], tmp[3]);
        tmp2[1]  = _mm512_add_epi32(tmp[1], tmp[2]);
        tmp2[2]  = _mm512_sub_epi32(tmp[1], tmp[2]);
        tmp2[3]  = _mm512_sub_epi32(tmp[0], tmp[3]);
        tmp2[5]  = half_btf_avx512(&cospim32, &tmp[5], &cospi32, &tmp[6], &rounding, bit);
        tmp2[6]  = half_btf_avx512(&cospi32, &tmp[5], &cospi32, &tmp[6], &rounding, bit);
        tmp2[9]  = _mm512_add_epi32(tmp[9], tmp[10]);
        tmp2[10] = _mm512_sub_epi32(tmp[9], tmp[10]);
        tmp2[13] = _mm512_sub_epi32(tmp[14], tmp[13]);
        tmp2[14] = _mm512_add_epi32(tmp[13], tmp[14]);

        //stage 6
        tmp[0]  = _mm512_add_epi32(tmp2[0], tmp[7]);
        tmp[1]  = _mm512_add_epi32(tmp2[1], tmp2[6]);
        tmp[2]  = _mm512_add_epi32(tmp2[2], tmp2[5]);
        tmp[3]  = _mm512_add_epi32(tmp2[3], tmp[4]);
        tmp[4]  = _mm512_sub_epi32(tmp2[3], tmp[4]);
        tmp[5]  = _mm512_sub_epi32(tmp2[2], tmp2[5]);
        tmp[6]  = _mm512_sub_epi32(tmp2[1], tmp2[6]);
        tmp[7]  = _mm512_sub_epi32(tmp2[0], tmp[7]);
        tmp[10] = half_btf_avx512(&cospim32, &tmp2[10], &cospi32, &tmp2[13], &rounding, bit);
        tmp[11] = half_btf_avx512(&cospim32, &tmp2[11], &cospi32, &tmp2[12], &rounding, bit);
        tmp[12] = half_btf_avx512(&cospi32, &tmp2[11], &cospi32, &tmp2[12], &rounding, bit);
        tmp[13] = half_btf_avx512(&cospi32, &tmp2[10], &cospi32, &tmp2[13], &rounding, bit);

        //stage 7
        out[0 * col_num + col]  = _mm512_add_epi32(tmp[0], tmp2[15]);
        out[1 * col_num + col]  = _mm512_add_epi32(tmp[1], tmp2[14]);
        out[2 * col_num + col]  = _mm512_add_epi32(tmp[2], tmp[13]);
        out[3 * col_num + col]  = _mm512_add_epi32(tmp[3], tmp[12]);
        out[4 * col_num + col]  = _mm512_add_epi32(tmp[4], tmp[11]);
        out[5 * col_num + col]  = _mm512_add_epi32(tmp[5], tmp[10]);
        out[6 * col_num + col]  = _mm512_add_epi32(tmp[6], tmp2[9]);
        out[7 * col_num + col]  = _mm512_add_epi32(tmp[7], tmp2[8]);
        out[8 * col_num + col]  = _mm512_sub_epi32(tmp[7], tmp2[8]);
        out[9 * col_num + col]  = _mm512_sub_epi32(tmp[6], tmp2[9]);
        out[10 * col_num + col] = _mm512_sub_epi32(tmp[5], tmp[10]);
        out[11 * col_num + col] = _mm512_sub_epi32(tmp[4], tmp[11]);
        out[12 * col_num + col] = _mm512_sub_epi32(tmp[3], tmp[12]);
        out[13 * col_num + col] = _mm512_sub_epi32(tmp[2], tmp[13]);
        out[14 * col_num + col] = _mm512_sub_epi32(tmp[1], tmp2[14]);
        out[15 * col_num + col] = _mm512_sub_epi32(tmp[0], tmp2[15]);
    }
}

static void idct64_avx512(__m512i* in, __m512i* out, const int8_t bit, int32_t do_cols, int32_t bd, int32_t out_shift) {
    int32_t        i, j;
    const int32_t* cospi     = cospi_arr(bit);
    const __m512i  rnding    = _mm512_set1_epi32(1 << (bit - 1));
    const int32_t  log_range = AOMMAX(16, bd + ((do_cols - 1) ? 6 : 8));
    const __m512i  clamp_lo  = _mm512_set1_epi32(-(1 << (log_range - 1)));
    const __m512i  clamp_hi  = _mm512_set1_epi32((1 << (log_range - 1)) - 1);
    int32_t        col;

    const __m512i cospi1  = _mm512_set1_epi32(cospi[1]);
    const __m512i cospi2  = _mm512_set1_epi32(cospi[2]);
    const __m512i cospi3  = _mm512_set1_epi32(cospi[3]);
    const __m512i cospi4  = _mm512_set1_epi32(cospi[4]);
    const __m512i cospi5  = _mm512_set1_epi32(cospi[5]);
    const __m512i cospi6  = _mm512_set1_epi32(cospi[6]);
    const __m512i cospi7  = _mm512_set1_epi32(cospi[7]);
    const __m512i cospi8  = _mm512_set1_epi32(cospi[8]);
    const __m512i cospi9  = _mm512_set1_epi32(cospi[9]);
    const __m512i cospi10 = _mm512_set1_epi32(cospi[10]);
    const __m512i cospi11 = _mm512_set1_epi32(cospi[11]);
    const __m512i cospi12 = _mm512_set1_epi32(cospi[12]);
    const __m512i cospi13 = _mm512_set1_epi32(cospi[13]);
    const __m512i cospi14 = _mm512_set1_epi32(cospi[14]);
    const __m512i cospi15 = _mm512_set1_epi32(cospi[15]);
    const __m512i cospi16 = _mm512_set1_epi32(cospi[16]);
    const __m512i cospi17 = _mm512_set1_epi32(cospi[17]);
    const __m512i cospi18 = _mm512_set1_epi32(cospi[18]);
    const __m512i cospi19 = _mm512_set1_epi32(cospi[19]);
    const __m512i cospi20 = _mm512_set1_epi32(cospi[20]);
    const __m512i cospi21 = _mm512_set1_epi32(cospi[21]);
    const __m512i cospi22 = _mm512_set1_epi32(cospi[22]);
    const __m512i cospi23 = _mm512_set1_epi32(cospi[23]);
    const __m512i cospi24 = _mm512_set1_epi32(cospi[24]);
    const __m512i cospi25 = _mm512_set1_epi32(cospi[25]);
    const __m512i cospi26 = _mm512_set1_epi32(cospi[26]);
    const __m512i cospi27 = _mm512_set1_epi32(cospi[27]);
    const __m512i cospi28 = _mm512_set1_epi32(cospi[28]);
    const __m512i cospi29 = _mm512_set1_epi32(cospi[29]);
    const __m512i cospi30 = _mm512_set1_epi32(cospi[30]);
    const __m512i cospi31 = _mm512_set1_epi32(cospi[31]);
    const __m512i cospi32 = _mm512_set1_epi32(cospi[32]);
    const __m512i cospi35 = _mm512_set1_epi32(cospi[35]);
    const __m512i cospi36 = _mm512_set1_epi32(cospi[36]);
    const __m512i cospi38 = _mm512_set1_epi32(cospi[38]);
    const __m512i cospi39 = _mm512_set1_epi32(cospi[39]);
    const __m512i cospi40 = _mm512_set1_epi32(cospi[40]);
    const __m512i cospi43 = _mm512_set1_epi32(cospi[43]);
    const __m512i cospi44 = _mm512_set1_epi32(cospi[44]);
    const __m512i cospi46 = _mm512_set1_epi32(cospi[46]);
    const __m512i cospi47 = _mm512_set1_epi32(cospi[47]);
    const __m512i cospi48 = _mm512_set1_epi32(cospi[48]);
    const __m512i cospi51 = _mm512_set1_epi32(cospi[51]);
    const __m512i cospi52 = _mm512_set1_epi32(cospi[52]);
    const __m512i cospi54 = _mm512_set1_epi32(cospi[54]);
    const __m512i cospi55 = _mm512_set1_epi32(cospi[55]);
    const __m512i cospi56 = _mm512_set1_epi32(cospi[56]);
    const __m512i cospi59 = _mm512_set1_epi32(cospi[59]);
    const __m512i cospi60 = _mm512_set1_epi32(cospi[60]);
    const __m512i cospi62 = _mm512_set1_epi32(cospi[62]);
    const __m512i cospi63 = _mm512_set1_epi32(cospi[63]);

    const __m512i cospim4  = _mm512_set1_epi32(-cospi[4]);
    const __m512i cospim8  = _mm512_set1_epi32(-cospi[8]);
    const __m512i cospim12 = _mm512_set1_epi32(-cospi[12]);
    const __m512i cospim16 = _mm512_set1_epi32(-cospi[16]);
    const __m512i cospim20 = _mm512_set1_epi32(-cospi[20]);
    const __m512i cospim24 = _mm512_set1_epi32(-cospi[24]);
    const __m512i cospim28 = _mm512_set1_epi32(-cospi[28]);
    const __m512i cospim32 = _mm512_set1_epi32(-cospi[32]);
    const __m512i cospim33 = _mm512_set1_epi32(-cospi[33]);
    const __m512i cospim34 = _mm512_set1_epi32(-cospi[34]);
    const __m512i cospim36 = _mm512_set1_epi32(-cospi[36]);
    const __m512i cospim37 = _mm512_set1_epi32(-cospi[37]);
    const __m512i cospim40 = _mm512_set1_epi32(-cospi[40]);
    const __m512i cospim41 = _mm512_set1_epi32(-cospi[41]);
    const __m512i cospim42 = _mm512_set1_epi32(-cospi[42]);
    const __m512i cospim44 = _mm512_set1_epi32(-cospi[44]);
    const __m512i cospim45 = _mm512_set1_epi32(-cospi[45]);
    const __m512i cospim48 = _mm512_set1_epi32(-cospi[48]);
    const __m512i cospim49 = _mm512_set1_epi32(-cospi[49]);
    const __m512i cospim50 = _mm512_set1_epi32(-cospi[50]);
    const __m512i cospim52 = _mm512_set1_epi32(-cospi[52]);
    const __m512i cospim53 = _mm512_set1_epi32(-cospi[53]);
    const __m512i cospim56 = _mm512_set1_epi32(-cospi[56]);
    const __m512i cospim57 = _mm512_set1_epi32(-cospi[57]);
    const __m512i cospim58 = _mm512_set1_epi32(-cospi[58]);
    const __m512i cospim60 = _mm512_set1_epi32(-cospi[60]);
    const __m512i cospim61 = _mm512_set1_epi32(-cospi[61]);

    for (col = 0; col < do_cols; ++col) {
        __m512i u[64], v[64];

        // stage 1
        u[32] = in[1 * do_cols + col];
        u[34] = in[17 * do_cols + col];
        u[36] = in[9 * do_cols + col];
        u[38] = in[25 * do_cols + col];
        u[40] = in[5 * do_cols + col];
        u[42] = in[21 * do_cols + col];
        u[44] = in[13 * do_cols + col];
        u[46] = in[29 * do_cols + col];
        u[48] = in[3 * do_cols + col];
        u[50] = in[19 * do_cols + col];
        u[52] = in[11 * do_cols + col];
        u[54] = in[27 * do_cols + col];
        u[56] = in[7 * do_cols + col];
        u[58] = in[23 * do_cols + col];
        u[60] = in[15 * do_cols + col];
        u[62] = in[31 * do_cols + col];

        v[16] = in[2 * do_cols + col];
        v[18] = in[18 * do_cols + col];
        v[20] = in[10 * do_cols + col];
        v[22] = in[26 * do_cols + col];
        v[24] = in[6 * do_cols + col];
        v[26] = in[22 * do_cols + col];
        v[28] = in[14 * do_cols + col];
        v[30] = in[30 * do_cols + col];

        u[8]  = in[4 * do_cols + col];
        u[10] = in[20 * do_cols + col];
        u[12] = in[12 * do_cols + col];
        u[14] = in[28 * do_cols + col];

        v[4] = in[8 * do_cols + col];
        v[6] = in[24 * do_cols + col];

        u[0] = in[0 * do_cols + col];
        u[2] = in[16 * do_cols + col];

        // stage 2
        v[32] = half_btf_0_avx512(&cospi63, &u[32], &rnding, bit);
        v[33] = half_btf_0_avx512(&cospim33, &u[62], &rnding, bit);
        v[34] = half_btf_0_avx512(&cospi47, &u[34], &rnding, bit);
        v[35] = half_btf_0_avx512(&cospim49, &u[60], &rnding, bit);
        v[36] = half_btf_0_avx512(&cospi55, &u[36], &rnding, bit);
        v[37] = half_btf_0_avx512(&cospim41, &u[58], &rnding, bit);
        v[38] = half_btf_0_avx512(&cospi39, &u[38], &rnding, bit);
        v[39] = half_btf_0_avx512(&cospim57, &u[56], &rnding, bit);
        v[40] = half_btf_0_avx512(&cospi59, &u[40], &rnding, bit);
        v[41] = half_btf_0_avx512(&cospim37, &u[54], &rnding, bit);
        v[42] = half_btf_0_avx512(&cospi43, &u[42], &rnding, bit);
        v[43] = half_btf_0_avx512(&cospim53, &u[52], &rnding, bit);
        v[44] = half_btf_0_avx512(&cospi51, &u[44], &rnding, bit);
        v[45] = half_btf_0_avx512(&cospim45, &u[50], &rnding, bit);
        v[46] = half_btf_0_avx512(&cospi35, &u[46], &rnding, bit);
        v[47] = half_btf_0_avx512(&cospim61, &u[48], &rnding, bit);
        v[48] = half_btf_0_avx512(&cospi3, &u[48], &rnding, bit);
        v[49] = half_btf_0_avx512(&cospi29, &u[46], &rnding, bit);
        v[50] = half_btf_0_avx512(&cospi19, &u[50], &rnding, bit);
        v[51] = half_btf_0_avx512(&cospi13, &u[44], &rnding, bit);
        v[52] = half_btf_0_avx512(&cospi11, &u[52], &rnding, bit);
        v[53] = half_btf_0_avx512(&cospi21, &u[42], &rnding, bit);
        v[54] = half_btf_0_avx512(&cospi27, &u[54], &rnding, bit);
        v[55] = half_btf_0_avx512(&cospi5, &u[40], &rnding, bit);
        v[56] = half_btf_0_avx512(&cospi7, &u[56], &rnding, bit);
        v[57] = half_btf_0_avx512(&cospi25, &u[38], &rnding, bit);
        v[58] = half_btf_0_avx512(&cospi23, &u[58], &rnding, bit);
        v[59] = half_btf_0_avx512(&cospi9, &u[36], &rnding, bit);
        v[60] = half_btf_0_avx512(&cospi15, &u[60], &rnding, bit);
        v[61] = half_btf_0_avx512(&cospi17, &u[34], &rnding, bit);
        v[62] = half_btf_0_avx512(&cospi31, &u[62], &rnding, bit);
        v[63] = half_btf_0_avx512(&cospi1, &u[32], &rnding, bit);

        // stage 3
        u[16] = half_btf_0_avx512(&cospi62, &v[16], &rnding, bit);
        u[17] = half_btf_0_avx512(&cospim34, &v[30], &rnding, bit);
        u[18] = half_btf_0_avx512(&cospi46, &v[18], &rnding, bit);
        u[19] = half_btf_0_avx512(&cospim50, &v[28], &rnding, bit);
        u[20] = half_btf_0_avx512(&cospi54, &v[20], &rnding, bit);
        u[21] = half_btf_0_avx512(&cospim42, &v[26], &rnding, bit);
        u[22] = half_btf_0_avx512(&cospi38, &v[22], &rnding, bit);
        u[23] = half_btf_0_avx512(&cospim58, &v[24], &rnding, bit);
        u[24] = half_btf_0_avx512(&cospi6, &v[24], &rnding, bit);
        u[25] = half_btf_0_avx512(&cospi26, &v[22], &rnding, bit);
        u[26] = half_btf_0_avx512(&cospi22, &v[26], &rnding, bit);
        u[27] = half_btf_0_avx512(&cospi10, &v[20], &rnding, bit);
        u[28] = half_btf_0_avx512(&cospi14, &v[28], &rnding, bit);
        u[29] = half_btf_0_avx512(&cospi18, &v[18], &rnding, bit);
        u[30] = half_btf_0_avx512(&cospi30, &v[30], &rnding, bit);
        u[31] = half_btf_0_avx512(&cospi2, &v[16], &rnding, bit);

        for (i = 32; i < 64; i += 4) {
            addsub_avx512(v[i + 0], v[i + 1], &u[i + 0], &u[i + 1], &clamp_lo, &clamp_hi);
            addsub_avx512(v[i + 3], v[i + 2], &u[i + 3], &u[i + 2], &clamp_lo, &clamp_hi);
        }

        // stage 4
        v[8]  = half_btf_0_avx512(&cospi60, &u[8], &rnding, bit);
        v[9]  = half_btf_0_avx512(&cospim36, &u[14], &rnding, bit);
        v[10] = half_btf_0_avx512(&cospi44, &u[10], &rnding, bit);
        v[11] = half_btf_0_avx512(&cospim52, &u[12], &rnding, bit);
        v[12] = half_btf_0_avx512(&cospi12, &u[12], &rnding, bit);
        v[13] = half_btf_0_avx512(&cospi20, &u[10], &rnding, bit);
        v[14] = half_btf_0_avx512(&cospi28, &u[14], &rnding, bit);
        v[15] = half_btf_0_avx512(&cospi4, &u[8], &rnding, bit);

        for (i = 16; i < 32; i += 4) {
            addsub_avx512(u[i + 0], u[i + 1], &v[i + 0], &v[i + 1], &clamp_lo, &clamp_hi);
            addsub_avx512(u[i + 3], u[i + 2], &v[i + 3], &v[i + 2], &clamp_lo, &clamp_hi);
        }

        for (i = 32; i < 64; i += 4) {
            v[i + 0] = u[i + 0];
            v[i + 3] = u[i + 3];
        }

        v[33] = half_btf_avx512(&cospim4, &u[33], &cospi60, &u[62], &rnding, bit);
        v[34] = half_btf_avx512(&cospim60, &u[34], &cospim4, &u[61], &rnding, bit);
        v[37] = half_btf_avx512(&cospim36, &u[37], &cospi28, &u[58], &rnding, bit);
        v[38] = half_btf_avx512(&cospim28, &u[38], &cospim36, &u[57], &rnding, bit);
        v[41] = half_btf_avx512(&cospim20, &u[41], &cospi44, &u[54], &rnding, bit);
        v[42] = half_btf_avx512(&cospim44, &u[42], &cospim20, &u[53], &rnding, bit);
        v[45] = half_btf_avx512(&cospim52, &u[45], &cospi12, &u[50], &rnding, bit);
        v[46] = half_btf_avx512(&cospim12, &u[46], &cospim52, &u[49], &rnding, bit);
        v[49] = half_btf_avx512(&cospim52, &u[46], &cospi12, &u[49], &rnding, bit);
        v[50] = half_btf_avx512(&cospi12, &u[45], &cospi52, &u[50], &rnding, bit);
        v[53] = half_btf_avx512(&cospim20, &u[42], &cospi44, &u[53], &rnding, bit);
        v[54] = half_btf_avx512(&cospi44, &u[41], &cospi20, &u[54], &rnding, bit);
        v[57] = half_btf_avx512(&cospim36, &u[38], &cospi28, &u[57], &rnding, bit);
        v[58] = half_btf_avx512(&cospi28, &u[37], &cospi36, &u[58], &rnding, bit);
        v[61] = half_btf_avx512(&cospim4, &u[34], &cospi60, &u[61], &rnding, bit);
        v[62] = half_btf_avx512(&cospi60, &u[33], &cospi4, &u[62], &rnding, bit);

        // stage 5
        u[4] = half_btf_0_avx512(&cospi56, &v[4], &rnding, bit);
        u[5] = half_btf_0_avx512(&cospim40, &v[6], &rnding, bit);
        u[6] = half_btf_0_avx512(&cospi24, &v[6], &rnding, bit);
        u[7] = half_btf_0_avx512(&cospi8, &v[4], &rnding, bit);

        for (i = 8; i < 16; i += 4) {
            addsub_avx512(v[i + 0], v[i + 1], &u[i + 0], &u[i + 1], &clamp_lo, &clamp_hi);
            addsub_avx512(v[i + 3], v[i + 2], &u[i + 3], &u[i + 2], &clamp_lo, &clamp_hi);
        }

        for (i = 16; i < 32; i += 4) {
            u[i + 0] = v[i + 0];
            u[i + 3] = v[i + 3];
        }

        u[17] = half_btf_avx512(&cospim8, &v[17], &cospi56, &v[30], &rnding, bit);
        u[18] = half_btf_avx512(&cospim56, &v[18], &cospim8, &v[29], &rnding, bit);
        u[21] = half_btf_avx512(&cospim40, &v[21], &cospi24, &v[26], &rnding, bit);
        u[22] = half_btf_avx512(&cospim24, &v[22], &cospim40, &v[25], &rnding, bit);
        u[25] = half_btf_avx512(&cospim40, &v[22], &cospi24, &v[25], &rnding, bit);
        u[26] = half_btf_avx512(&cospi24, &v[21], &cospi40, &v[26], &rnding, bit);
        u[29] = half_btf_avx512(&cospim8, &v[18], &cospi56, &v[29], &rnding, bit);
        u[30] = half_btf_avx512(&cospi56, &v[17], &cospi8, &v[30], &rnding, bit);

        for (i = 32; i < 64; i += 8) {
            addsub_avx512(v[i + 0], v[i + 3], &u[i + 0], &u[i + 3], &clamp_lo, &clamp_hi);
            addsub_avx512(v[i + 1], v[i + 2], &u[i + 1], &u[i + 2], &clamp_lo, &clamp_hi);

            addsub_avx512(v[i + 7], v[i + 4], &u[i + 7], &u[i + 4], &clamp_lo, &clamp_hi);
            addsub_avx512(v[i + 6], v[i + 5], &u[i + 6], &u[i + 5], &clamp_lo, &clamp_hi);
        }

        // stage 6
        v[0] = half_btf_0_avx512(&cospi32, &u[0], &rnding, bit);
        v[1] = half_btf_0_avx512(&cospi32, &u[0], &rnding, bit);
        v[2] = half_btf_0_avx512(&cospi48, &u[2], &rnding, bit);
        v[3] = half_btf_0_avx512(&cospi16, &u[2], &rnding, bit);

        addsub_avx512(u[4], u[5], &v[4], &v[5], &clamp_lo, &clamp_hi);
        addsub_avx512(u[7], u[6], &v[7], &v[6], &clamp_lo, &clamp_hi);

        for (i = 8; i < 16; i += 4) {
            v[i + 0] = u[i + 0];
            v[i + 3] = u[i + 3];
        }

        v[9]  = half_btf_avx512(&cospim16, &u[9], &cospi48, &u[14], &rnding, bit);
        v[10] = half_btf_avx512(&cospim48, &u[10], &cospim16, &u[13], &rnding, bit);
        v[13] = half_btf_avx512(&cospim16, &u[10], &cospi48, &u[13], &rnding, bit);
        v[14] = half_btf_avx512(&cospi48, &u[9], &cospi16, &u[14], &rnding, bit);

        for (i = 16; i < 32; i += 8) {
            addsub_avx512(u[i + 0], u[i + 3], &v[i + 0], &v[i + 3], &clamp_lo, &clamp_hi);
            addsub_avx512(u[i + 1], u[i + 2], &v[i + 1], &v[i + 2], &clamp_lo, &clamp_hi);

            addsub_avx512(u[i + 7], u[i + 4], &v[i + 7], &v[i + 4], &clamp_lo, &clamp_hi);
            addsub_avx512(u[i + 6], u[i + 5], &v[i + 6], &v[i + 5], &clamp_lo, &clamp_hi);
        }

        for (i = 32; i < 64; i += 8) {
            v[i + 0] = u[i + 0];
            v[i + 1] = u[i + 1];
            v[i + 6] = u[i + 6];
            v[i + 7] = u[i + 7];
        }

        v[34] = half_btf_avx512(&cospim8, &u[34], &cospi56, &u[61], &rnding, bit);
        v[35] = half_btf_avx512(&cospim8, &u[35], &cospi56, &u[60], &rnding, bit);
        v[36] = half_btf_avx512(&cospim56, &u[36], &cospim8, &u[59], &rnding, bit);
        v[37] = half_btf_avx512(&cospim56, &u[37], &cospim8, &u[58], &rnding, bit);
        v[42] = half_btf_avx512(&cospim40, &u[42], &cospi24, &u[53], &rnding, bit);
        v[43] = half_btf_avx512(&cospim40, &u[43], &cospi24, &u[52], &rnding, bit);
        v[44] = half_btf_avx512(&cospim24, &u[44], &cospim40, &u[51], &rnding, bit);
        v[45] = half_btf_avx512(&cospim24, &u[45], &cospim40, &u[50], &rnding, bit);
        v[50] = half_btf_avx512(&cospim40, &u[45], &cospi24, &u[50], &rnding, bit);
        v[51] = half_btf_avx512(&cospim40, &u[44], &cospi24, &u[51], &rnding, bit);
        v[52] = half_btf_avx512(&cospi24, &u[43], &cospi40, &u[52], &rnding, bit);
        v[53] = half_btf_avx512(&cospi24, &u[42], &cospi40, &u[53], &rnding, bit);
        v[58] = half_btf_avx512(&cospim8, &u[37], &cospi56, &u[58], &rnding, bit);
        v[59] = half_btf_avx512(&cospim8, &u[36], &cospi56, &u[59], &rnding, bit);
        v[60] = half_btf_avx512(&cospi56, &u[35], &cospi8, &u[60], &rnding, bit);
        v[61] = half_btf_avx512(&cospi56, &u[34], &cospi8, &u[61], &rnding, bit);

        // stage 7
        addsub_avx512(v[0], v[3], &u[0], &u[3], &clamp_lo, &clamp_hi);
        addsub_avx512(v[1], v[2], &u[1], &u[2], &clamp_lo, &clamp_hi);

        u[4] = v[4];
        u[7] = v[7];
        u[5] = half_btf_avx512(&cospim32, &v[5], &cospi32, &v[6], &rnding, bit);
        u[6] = half_btf_avx512(&cospi32, &v[5], &cospi32, &v[6], &rnding, bit);

        addsub_avx512(v[8], v[11], &u[8], &u[11], &clamp_lo, &clamp_hi);
        addsub_avx512(v[9], v[10], &u[9], &u[10], &clamp_lo, &clamp_hi);
        addsub_avx512(v[15], v[12], &u[15], &u[12], &clamp_lo, &clamp_hi);
        addsub_avx512(v[14], v[13], &u[14], &u[13], &clamp_lo, &clamp_hi);

        for (i = 16; i < 32; i += 8) {
            u[i + 0] = v[i + 0];
            u[i + 1] = v[i + 1];
            u[i + 6] = v[i + 6];
            u[i + 7] = v[i + 7];
        }

        u[18] = half_btf_avx512(&cospim16, &v[18], &cospi48, &v[29], &rnding, bit);
        u[19] = half_btf_avx512(&cospim16, &v[19], &cospi48, &v[28], &rnding, bit);
        u[20] = half_btf_avx512(&cospim48, &v[20], &cospim16, &v[27], &rnding, bit);
        u[21] = half_btf_avx512(&cospim48, &v[21], &cospim16, &v[26], &rnding, bit);
        u[26] = half_btf_avx512(&cospim16, &v[21], &cospi48, &v[26], &rnding, bit);
        u[27] = half_btf_avx512(&cospim16, &v[20], &cospi48, &v[27], &rnding, bit);
        u[28] = half_btf_avx512(&cospi48, &v[19], &cospi16, &v[28], &rnding, bit);
        u[29] = half_btf_avx512(&cospi48, &v[18], &cospi16, &v[29], &rnding, bit);

        for (i = 32; i < 64; i += 16) {
            for (j = i; j < i + 4; j++) {
                addsub_avx512(v[j], v[j ^ 7], &u[j], &u[j ^ 7], &clamp_lo, &clamp_hi);
                addsub_avx512(v[j ^ 15], v[j ^ 8], &u[j ^ 15], &u[j ^ 8], &clamp_lo, &clamp_hi);
            }
        }

        // stage 8
        for (i = 0; i < 4; ++i) {
            addsub_avx512(u[i], u[7 - i], &v[i], &v[7 - i], &clamp_lo, &clamp_hi);
        }

        v[8]  = u[8];
        v[9]  = u[9];
        v[14] = u[14];
        v[15] = u[15];

        v[10] = half_btf_avx512(&cospim32, &u[10], &cospi32, &u[13], &rnding, bit);
        v[11] = half_btf_avx512(&cospim32, &u[11], &cospi32, &u[12], &rnding, bit);
        v[12] = half_btf_avx512(&cospi32, &u[11], &cospi32, &u[12], &rnding, bit);
        v[13] = half_btf_avx512(&cospi32, &u[10], &cospi32, &u[13], &rnding, bit);

        for (i = 16; i < 20; ++i) {
            addsub_avx512(u[i], u[i ^ 7], &v[i], &v[i ^ 7], &clamp_lo, &clamp_hi);
            addsub_avx512(u[i ^ 15], u[i ^ 8], &v[i ^ 15], &v[i ^ 8], &clamp_lo, &clamp_hi);
        }

        for (i = 32; i < 36; ++i) {
            v[i]      = u[i];
            v[i + 12] = u[i + 12];
            v[i + 16] = u[i + 16];
            v[i + 28] = u[i + 28];
        }

        v[36] = half_btf_avx512(&cospim16, &u[36], &cospi48, &u[59], &rnding, bit);
        v[37] = half_btf_avx512(&cospim16, &u[37], &cospi48, &u[58], &rnding, bit);
        v[38] = half_btf_avx512(&cospim16, &u[38], &cospi48, &u[57], &rnding, bit);
        v[39] = half_btf_avx512(&cospim16, &u[39], &cospi48, &u[56], &rnding, bit);
        v[40] = half_btf_avx512(&cospim48, &u[40], &cospim16, &u[55], &rnding, bit);
        v[41] = half_btf_avx512(&cospim48, &u[41], &cospim16, &u[54], &rnding, bit);
        v[42] = half_btf_avx512(&cospim48, &u[42], &cospim16, &u[53], &rnding, bit);
        v[43] = half_btf_avx512(&cospim48, &u[43], &cospim16, &u[52], &rnding, bit);
        v[52] = half_btf_avx512(&cospim16, &u[43], &cospi48, &u[52], &rnding, bit);
        v[53] = half_btf_avx512(&cospim16, &u[42], &cospi48, &u[53], &rnding, bit);
        v[54] = half_btf_avx512(&cospim16, &u[41], &cospi48, &u[54], &rnding, bit);
        v[55] = half_btf_avx512(&cospim16, &u[40], &cospi48, &u[55], &rnding, bit);
        v[56] = half_btf_avx512(&cospi48, &u[39], &cospi16, &u[56], &rnding, bit);
        v[57] = half_btf_avx512(&cospi48, &u[38], &cospi16, &u[57], &rnding, bit);
        v[58] = half_btf_avx512(&cospi48, &u[37], &cospi16, &u[58], &rnding, bit);
        v[59] = half_btf_avx512(&cospi48, &u[36], &cospi16, &u[59], &rnding, bit);

        // stage 9
        for (i = 0; i < 8; ++i) {
            addsub_avx512(v[i], v[15 - i], &u[i], &u[15 - i], &clamp_lo, &clamp_hi);
        }

        for (i = 16; i < 20; ++i) {
            u[i]      = v[i];
            u[i + 12] = v[i + 12];
        }

        u[20] = half_btf_avx512(&cospim32, &v[20], &cospi32, &v[27], &rnding, bit);
        u[21] = half_btf_avx512(&cospim32, &v[21], &cospi32, &v[26], &rnding, bit);
        u[22] = half_btf_avx512(&cospim32, &v[22], &cospi32, &v[25], &rnding, bit);
        u[23] = half_btf_avx512(&cospim32, &v[23], &cospi32, &v[24], &rnding, bit);
        u[24] = half_btf_avx512(&cospi32, &v[23], &cospi32, &v[24], &rnding, bit);
        u[25] = half_btf_avx512(&cospi32, &v[22], &cospi32, &v[25], &rnding, bit);
        u[26] = half_btf_avx512(&cospi32, &v[21], &cospi32, &v[26], &rnding, bit);
        u[27] = half_btf_avx512(&cospi32, &v[20], &cospi32, &v[27], &rnding, bit);

        for (i = 32; i < 40; i++) {
            addsub_avx512(v[i], v[i ^ 15], &u[i], &u[i ^ 15], &clamp_lo, &clamp_hi);
        }

        for (i = 48; i < 56; i++) {
            addsub_avx512(v[i ^ 15], v[i], &u[i ^ 15], &u[i], &clamp_lo, &clamp_hi);
        }

        // stage 10
        for (i = 0; i < 16; i++) {
            addsub_avx512(u[i], u[31 - i], &v[i], &v[31 - i], &clamp_lo, &clamp_hi);
        }

        for (i = 32; i < 40; i++) {
            v[i] = u[i];
        }

        v[40] = half_btf_avx512(&cospim32, &u[40], &cospi32, &u[55], &rnding, bit);
        v[41] = half_btf_avx512(&cospim32, &u[41], &cospi32, &u[54], &rnding, bit);
        v[42] = half_btf_avx512(&cospim32, &u[42], &cospi32, &u[53], &rnding, bit);
        v[43] = half_btf_avx512(&cospim32, &u[43], &cospi32, &u[52], &rnding, bit);
        v[44] = half_btf_avx512(&cospim32, &u[44], &cospi32, &u[51], &rnding, bit);
        v[45] = half_btf_avx512(&cospim32, &u[45], &cospi32, &u[50], &rnding, bit);
        v[46] = half_btf_avx512(&cospim32, &u[46], &cospi32, &u[49], &rnding, bit);
        v[47] = half_btf_avx512(&cospim32, &u[47], &cospi32, &u[48], &rnding, bit);
        v[48] = half_btf_avx512(&cospi32, &u[47], &cospi32, &u[48], &rnding, bit);
        v[49] = half_btf_avx512(&cospi32, &u[46], &cospi32, &u[49], &rnding, bit);
        v[50] = half_btf_avx512(&cospi32, &u[45], &cospi32, &u[50], &rnding, bit);
        v[51] = half_btf_avx512(&cospi32, &u[44], &cospi32, &u[51], &rnding, bit);
        v[52] = half_btf_avx512(&cospi32, &u[43], &cospi32, &u[52], &rnding, bit);
        v[53] = half_btf_avx512(&cospi32, &u[42], &cospi32, &u[53], &rnding, bit);
        v[54] = half_btf_avx512(&cospi32, &u[41], &cospi32, &u[54], &rnding, bit);
        v[55] = half_btf_avx512(&cospi32, &u[40], &cospi32, &u[55], &rnding, bit);

        for (i = 56; i < 64; i++) {
            v[i] = u[i];
        }

        // stage 11
        for (i = 0; i < 32; i++) {
            addsub_shift_avx512(v[i],
                                v[63 - i],
                                &out[do_cols * (i) + col],
                                &out[do_cols * (63 - i) + col],
                                &clamp_lo,
                                &clamp_hi,
                                out_shift);
        }
    }
}

static void iidtx16_avx512(__m512i* in, __m512i* out, const int8_t bit, int32_t col_num) {
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

void iidtx32_avx512(__m512i* input, __m512i* output, const int8_t cos_bit, int32_t col_num) {
    (void)cos_bit;
    for (int32_t i = 0; i < 32; i++) {
        output[i * col_num] = _mm512_slli_epi32(input[i * col_num], (uint8_t)2);
    }
}

static void write_buffer_16x16n_avx512(__m512i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                       int32_t stride_w, int32_t fliplr, int32_t flipud, int32_t bd, int32_t size) {
    __m512i u1, v0;
    __m256i u0;
    int32_t i = 0;
    (void)fliplr;
    (void)flipud;
    while (i < size) {
        u0 = _mm256_loadu_si256((const __m256i*)output_r);
        u1 = _mm512_cvtepu16_epi32(u0);
        v0 = in[i];
        v0 = _mm512_add_epi32(v0, u1);
        highbd_clamp_epi32_avx512(&v0, bd);
        u0 = _mm512_cvtepi32_epi16(v0);
        _mm256_storeu_si256((__m256i*)output_w, u0);
        output_r += stride_r;
        output_w += stride_w;
        i += 1;
    }
}

static void write_buffer_64x16n_avx512(__m512i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                       int32_t stride_w, int32_t fliplr, int32_t flipud, int32_t bd, int32_t size) {
    __m512i v0, v1, v2, v3, x0, x1, x2, x3;
    __m256i u0, u1, u2, u3;
    int32_t i = 0;
    (void)fliplr;
    (void)flipud;
    while (i < size) {
        u0 = _mm256_loadu_si256((const __m256i*)output_r);
        u1 = _mm256_loadu_si256((const __m256i*)(output_r + 16));
        u2 = _mm256_loadu_si256((const __m256i*)(output_r + 32));
        u3 = _mm256_loadu_si256((const __m256i*)(output_r + 48));
        x0 = _mm512_cvtepu16_epi32(u0);
        x1 = _mm512_cvtepu16_epi32(u1);
        x2 = _mm512_cvtepu16_epi32(u2);
        x3 = _mm512_cvtepu16_epi32(u3);

        v0 = in[i];
        v1 = in[i + 1];
        v2 = in[i + 2];
        v3 = in[i + 3];

        v0 = _mm512_add_epi32(v0, x0);
        v1 = _mm512_add_epi32(v1, x1);
        v2 = _mm512_add_epi32(v2, x2);
        v3 = _mm512_add_epi32(v3, x3);

        highbd_clamp_epi32_avx512(&v0, bd);
        highbd_clamp_epi32_avx512(&v1, bd);
        highbd_clamp_epi32_avx512(&v2, bd);
        highbd_clamp_epi32_avx512(&v3, bd);

        u0 = _mm512_cvtepi32_epi16(v0);
        u1 = _mm512_cvtepi32_epi16(v1);
        u2 = _mm512_cvtepi32_epi16(v2);
        u3 = _mm512_cvtepi32_epi16(v3);

        _mm256_storeu_si256((__m256i*)output_w, u0);
        _mm256_storeu_si256((__m256i*)(output_w + 16), u1);
        _mm256_storeu_si256((__m256i*)(output_w + 32), u2);
        _mm256_storeu_si256((__m256i*)(output_w + 48), u3);
        output_r += stride_r;
        output_w += stride_w;
        i += 4;
    }
}

static void write_buffer_32x16n_avx512(__m512i* in, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                       int32_t stride_w, int32_t fliplr, int32_t flipud, int32_t bd, int32_t size) {
    __m512i v0, v1, x0, x1;
    __m256i u0, u1;
    int32_t i = 0;
    (void)fliplr;
    (void)flipud;
    while (i < size) {
        u0 = _mm256_loadu_si256((const __m256i*)output_r);
        u1 = _mm256_loadu_si256((const __m256i*)(output_r + 16));

        x0 = _mm512_cvtepu16_epi32(u0);
        x1 = _mm512_cvtepu16_epi32(u1);

        v0 = in[i];
        v1 = in[i + 1];

        v0 = _mm512_add_epi32(v0, x0);
        v1 = _mm512_add_epi32(v1, x1);

        highbd_clamp_epi32_avx512(&v0, bd);
        highbd_clamp_epi32_avx512(&v1, bd);

        u0 = _mm512_cvtepi32_epi16(v0);
        u1 = _mm512_cvtepi32_epi16(v1);

        _mm256_storeu_si256((__m256i*)output_w, u0);
        _mm256_storeu_si256((__m256i*)(output_w + 16), u1);

        output_r += stride_r;
        output_w += stride_w;
        i += 2;
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

void svt_av1_inv_txfm2d_add_16x64_avx512(const int32_t* coeff, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                         int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    (void)tx_type;
    (void)eob;
    __m512i       in[64], out[64];
    const int8_t* shift         = svt_aom_inv_txfm_shift_ls[tx_size];
    const int32_t txw_idx       = tx_size_wide_log2[tx_size] - tx_size_wide_log2[0];
    const int32_t txh_idx       = tx_size_high_log2[tx_size] - tx_size_high_log2[0];
    const int32_t txfm_size_col = tx_size_wide[tx_size];
    const int32_t txfm_size_row = tx_size_high[tx_size];
    const int32_t num_row       = txfm_size_row >> 4;
    const int32_t num_col       = txfm_size_col >> 4;

    // row tranform
    for (int32_t i = 0; i < txfm_size_row; i++) {
        load_buffer_16_avx512(coeff + i * txfm_size_col, in + i);
    }
    transpose_16nx16m_inv_avx512(in, out, txfm_size_col, txfm_size_row);
    idct16_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx], num_row);
    round_shift_16x16_avx512(in, -shift[0]);
    round_shift_16x16_avx512(in + 16, -shift[0]);
    round_shift_16x16_avx512(in + 32, -shift[0]);
    round_shift_16x16_avx512(in + 48, -shift[0]);
    transpose_16nx16m_inv_avx512(in, out, txfm_size_row, txfm_size_col);

    // column transform
    idct64_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx], num_col, bd, -shift[1]);
    write_buffer_16x16n_avx512(in, output_r, stride_r, output_w, stride_w, 0, 0, bd, 64);
}

void svt_av1_inv_txfm2d_add_64x16_avx512(const int32_t* coeff, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                         int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    (void)tx_type;
    (void)eob;
    __m512i       in[64], out[64];
    const int8_t* shift         = svt_aom_inv_txfm_shift_ls[tx_size];
    const int32_t txw_idx       = tx_size_wide_log2[tx_size] - tx_size_wide_log2[0];
    const int32_t txh_idx       = tx_size_high_log2[tx_size] - tx_size_high_log2[0];
    const int32_t txfm_size_col = tx_size_wide[tx_size];
    const int32_t txfm_size_row = tx_size_high[tx_size];
    const int32_t num_row       = txfm_size_row >> 4;
    const int32_t num_col       = txfm_size_col >> 4;

    // row tranform
    for (int32_t i = 0; i < txfm_size_row; i++) {
        load_buffer_64_avx512(coeff + i * txfm_size_col, in + (i * 4));
    }
    transpose_16nx16m_inv_avx512(in, out, 32, 16);
    idct64_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx], num_row, bd, -shift[0]);
    transpose_16nx16m_inv_avx512(in, out, txfm_size_row, txfm_size_col);

    //column transform
    idct16_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx], num_col);
    av1_round_shift_array_avx512(in, out, 64, -shift[1]);
    write_buffer_64x16n_avx512(out, output_r, stride_r, output_w, stride_w, 0, 0, bd, 64);
}

void svt_av1_inv_txfm2d_add_32x64_avx512(const int32_t* coeff, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                         int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    (void)tx_type;
    (void)eob;
    __m512i       in[128], out[128];
    const int8_t* shift         = svt_aom_inv_txfm_shift_ls[tx_size];
    const int32_t txw_idx       = tx_size_wide_log2[tx_size] - tx_size_wide_log2[0];
    const int32_t txh_idx       = tx_size_high_log2[tx_size] - tx_size_high_log2[0];
    const int32_t txfm_size_col = tx_size_wide[tx_size];
    const int32_t txfm_size_row = tx_size_high[tx_size];
    const int32_t num_row       = txfm_size_row >> 4;
    const int32_t num_col       = txfm_size_col >> 4;

    // row tranform
    for (int32_t i = 0; i < txfm_size_row; i++) {
        load_buffer_16_avx512(coeff + 0 + i * txfm_size_col, in + 0 + i * 2);
        load_buffer_16_avx512(coeff + 16 + i * txfm_size_col, in + 1 + i * 2);
    }
    transpose_16nx16m_inv_avx512(in, out, txfm_size_col, txfm_size_row);
    av1_round_shift_rect_array_32_avx512(out, out, 128, (int8_t)0, 2896);
    idct32_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx], num_row);
    for (int32_t i = 0; i < 8; i++) {
        round_shift_16x16_avx512((in + i * 16), -shift[0]);
    }
    transpose_16nx16m_inv_avx512(in, out, txfm_size_row, txfm_size_col);

    // column transform
    idct64_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx], num_col, bd, -shift[1]);
    write_buffer_32x16n_avx512(in, output_r, stride_r, output_w, stride_w, 0, 0, bd, 128);
}

void svt_av1_inv_txfm2d_add_64x32_avx512(const int32_t* coeff, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                         int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    (void)tx_type;
    (void)eob;
    __m512i       in[128], out[128];
    const int8_t* shift         = svt_aom_inv_txfm_shift_ls[tx_size];
    const int32_t txw_idx       = tx_size_wide_log2[tx_size] - tx_size_wide_log2[0];
    const int32_t txh_idx       = tx_size_high_log2[tx_size] - tx_size_high_log2[0];
    const int32_t txfm_size_col = tx_size_wide[tx_size];
    const int32_t txfm_size_row = tx_size_high[tx_size];
    const int32_t num_row       = txfm_size_row >> 4;
    const int32_t num_col       = txfm_size_col >> 4;

    // row tranform
    for (int32_t i = 0; i < txfm_size_row; i++) {
        load_buffer_16_avx512(coeff + 0 + i * txfm_size_col, in + 0 + i * 4);
        load_buffer_16_avx512(coeff + 16 + i * txfm_size_col, in + 1 + i * 4);
        load_buffer_16_avx512(coeff + 32 + i * txfm_size_col, in + 2 + i * 4);
        load_buffer_16_avx512(coeff + 48 + i * txfm_size_col, in + 3 + i * 4);
    }
    transpose_16nx16m_inv_avx512(in, out, 32, 32);
    av1_round_shift_rect_array_32_avx512(out, out, 128, (int8_t)0, 2896);
    idct64_avx512(out, in, inv_cos_bit_row[txw_idx][txh_idx], num_row, bd, -shift[0]);
    transpose_16nx16m_inv_avx512(in, out, txfm_size_row, txfm_size_col);

    // column transform
    idct32_avx512(out, in, inv_cos_bit_col[txw_idx][txh_idx], num_col);
    av1_round_shift_array_avx512(in, out, 128, -shift[1]);
    write_buffer_64x16n_avx512(out, output_r, stride_r, output_w, stride_w, 0, 0, bd, 128);
}

static const inv_transform_1d_avx512 col_invtxfm_16x32_arr[TX_TYPES] = {
    idct32_avx512, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    iidtx32_avx512, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST
};

static const inv_transform_1d_avx512 row_invtxfm_16x32_arr[TX_TYPES] = {
    idct16_avx512, // DCT_DCT
    NULL, // ADST_DCT
    NULL, // DCT_ADST
    NULL, // ADST_ADST
    NULL, // FLIPADST_DCT
    NULL, // DCT_FLIPADST
    NULL, // FLIPADST_FLIPADST
    NULL, // ADST_FLIPADST
    NULL, // FLIPADST_ADST
    iidtx16_avx512, // IDTX
    NULL, // V_DCT
    NULL, // H_DCT
    NULL, // V_ADST
    NULL, // H_ADST
    NULL, // V_FLIPADST
    NULL // H_FLIPADST
};

void svt_av1_inv_txfm2d_add_16x32_avx512(const int32_t* coeff, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                         int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    (void)eob;
    __m512i                       in[32], out[32];
    const int8_t*                 shift         = svt_aom_inv_txfm_shift_ls[tx_size];
    const int32_t                 txw_idx       = tx_size_wide_log2[tx_size] - tx_size_wide_log2[0];
    const int32_t                 txh_idx       = tx_size_high_log2[tx_size] - tx_size_high_log2[0];
    const int32_t                 txfm_size_col = tx_size_wide[tx_size];
    const int32_t                 txfm_size_row = tx_size_high[tx_size];
    int32_t                       num_row       = txfm_size_row >> 4;
    int32_t                       num_col       = txfm_size_col >> 4;
    const inv_transform_1d_avx512 col_txfm      = col_invtxfm_16x32_arr[tx_type];
    const inv_transform_1d_avx512 row_txfm      = row_invtxfm_16x32_arr[tx_type];

    // row tranform
    for (int32_t i = 0; i < txfm_size_row; i++) {
        load_buffer_16_avx512(coeff + i * txfm_size_col, in + i);
    }
    transpose_16nx16m_inv_avx512(in, out, txfm_size_col, txfm_size_row);
    av1_round_shift_rect_array_32_avx512(out, out, 32, (int8_t)0, 2896);
    row_txfm(out, in, inv_cos_bit_row[txw_idx][txh_idx], num_row);
    for (int32_t i = 0; i < 2; i++) {
        round_shift_16x16_avx512((in + i * 16), -shift[0]);
    }
    transpose_16nx16m_inv_avx512(in, out, txfm_size_row, txfm_size_col);

    // column transform
    col_txfm(out, in, inv_cos_bit_col[txw_idx][txh_idx], num_col);
    av1_round_shift_array_avx512(in, out, 32, -shift[1]);
    write_buffer_16x16n_avx512(out, output_r, stride_r, output_w, stride_w, 0, 0, bd, 32);
}

void svt_av1_inv_txfm2d_add_32x16_avx512(const int32_t* coeff, uint16_t* output_r, int32_t stride_r, uint16_t* output_w,
                                         int32_t stride_w, TxType tx_type, TxSize tx_size, int32_t eob, int32_t bd) {
    (void)eob;
    __m512i                       in[32], out[32];
    const int8_t*                 shift         = svt_aom_inv_txfm_shift_ls[tx_size];
    const int32_t                 txw_idx       = tx_size_wide_log2[tx_size] - tx_size_wide_log2[0];
    const int32_t                 txh_idx       = tx_size_high_log2[tx_size] - tx_size_high_log2[0];
    const int32_t                 txfm_size_col = tx_size_wide[tx_size];
    const int32_t                 txfm_size_row = tx_size_high[tx_size];
    int32_t                       num_row       = txfm_size_row >> 4;
    int32_t                       num_col       = txfm_size_col >> 4;
    const inv_transform_1d_avx512 col_txfm      = row_invtxfm_16x32_arr[tx_type];
    const inv_transform_1d_avx512 row_txfm      = col_invtxfm_16x32_arr[tx_type];

    // row tranform
    for (int32_t i = 0; i < txfm_size_row; i++) {
        load_buffer_16_avx512(coeff + 0 + i * txfm_size_col, in + 0 + i * 2);
        load_buffer_16_avx512(coeff + 16 + i * txfm_size_col, in + 1 + i * 2);
    }
    transpose_16nx16m_inv_avx512(in, out, txfm_size_col, txfm_size_row);
    av1_round_shift_rect_array_32_avx512(out, out, 32, (int8_t)0, 2896);
    row_txfm(out, in, inv_cos_bit_row[txw_idx][txh_idx], num_row);
    for (int32_t i = 0; i < 2; i++) {
        round_shift_16x16_avx512((in + i * 16), -shift[0]);
    }
    transpose_16nx16m_inv_avx512(in, out, txfm_size_row, txfm_size_col);

    // column transform
    col_txfm(out, in, inv_cos_bit_col[txw_idx][txh_idx], num_col);
    av1_round_shift_array_avx512(in, out, 32, -shift[1]);
    write_buffer_32x16n_avx512(out, output_r, stride_r, output_w, stride_w, 0, 0, bd, 32);
}
#endif
