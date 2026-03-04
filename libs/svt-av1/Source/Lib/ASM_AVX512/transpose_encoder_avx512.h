/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2019, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef AOM_DSP_X86_TRANSPOSE_ENCODER_AVX512_H_
#define AOM_DSP_X86_TRANSPOSE_ENCODER_AVX512_H_

#include "transpose_avx512.h"

static INLINE void transpose_16nx16m_avx512(const __m512i* in, __m512i* out, const int32_t width,
                                            const int32_t height) {
    const int32_t numcol = height >> 4;
    const int32_t numrow = width >> 4;
    __m512i       out1[16];
    for (int32_t j = 0; j < numrow; j++) {
        for (int32_t i = 0; i < numcol; i++) {
            TRANSPOSE_4X4_AVX512(in[i * width + j + (numrow * 0)],
                                 in[i * width + j + (numrow * 1)],
                                 in[i * width + j + (numrow * 2)],
                                 in[i * width + j + (numrow * 3)],
                                 out1[0],
                                 out1[1],
                                 out1[2],
                                 out1[3]);
            TRANSPOSE_4X4_AVX512(in[i * width + j + (numrow * 4)],
                                 in[i * width + j + (numrow * 5)],
                                 in[i * width + j + (numrow * 6)],
                                 in[i * width + j + (numrow * 7)],
                                 out1[4],
                                 out1[5],
                                 out1[6],
                                 out1[7]);
            TRANSPOSE_4X4_AVX512(in[i * width + j + (numrow * 8)],
                                 in[i * width + j + (numrow * 9)],
                                 in[i * width + j + (numrow * 10)],
                                 in[i * width + j + (numrow * 11)],
                                 out1[8],
                                 out1[9],
                                 out1[10],
                                 out1[11]);
            TRANSPOSE_4X4_AVX512(in[i * width + j + (numrow * 12)],
                                 in[i * width + j + (numrow * 13)],
                                 in[i * width + j + (numrow * 14)],
                                 in[i * width + j + (numrow * 15)],
                                 out1[12],
                                 out1[13],
                                 out1[14],
                                 out1[15]);

            __m128i* outptr = (__m128i*)(out + (j * height + i + (numcol * 0)));

            //will get first row of transpose matrix from corresponding 4 vectors in out1
            outptr[0] = _mm512_castsi512_si128(out1[0]);
            outptr[1] = _mm512_castsi512_si128(out1[4]);
            outptr[2] = _mm512_castsi512_si128(out1[8]);
            outptr[3] = _mm512_castsi512_si128(out1[12]);

            //will get second row of transpose matrix from corresponding 4 vectors in out1
            outptr    = (__m128i*)(out + (j * height + i + (numcol * 1)));
            outptr[0] = _mm512_castsi512_si128(out1[1]);
            outptr[1] = _mm512_castsi512_si128(out1[5]);
            outptr[2] = _mm512_castsi512_si128(out1[9]);
            outptr[3] = _mm512_castsi512_si128(out1[13]);

            //will get 3rd row of transpose matrix from corresponding 4 vectors in out1
            outptr    = (__m128i*)(out + (j * height + i + (numcol * 2)));
            outptr[0] = _mm512_castsi512_si128(out1[2]);
            outptr[1] = _mm512_castsi512_si128(out1[6]);
            outptr[2] = _mm512_castsi512_si128(out1[10]);
            outptr[3] = _mm512_castsi512_si128(out1[14]);

            //will get 4th row of transpose matrix from corresponding 4 vectors in out1
            outptr    = (__m128i*)(out + (j * height + i + (numcol * 3)));
            outptr[0] = _mm512_castsi512_si128(out1[3]);
            outptr[1] = _mm512_castsi512_si128(out1[7]);
            outptr[2] = _mm512_castsi512_si128(out1[11]);
            outptr[3] = _mm512_castsi512_si128(out1[15]);

            const __m512i idx_1 = _mm512_setr_epi64(2, 3, 10, 11, 0, 0, 0, 0);
            const __m512i idx_2 = _mm512_setr_epi64(4, 5, 12, 13, 0, 0, 0, 0);
            const __m512i idx_3 = _mm512_setr_epi64(6, 7, 14, 15, 0, 0, 0, 0);

            //will get 5th row of transpose matrix from corresponding 4 vectors in out1
            __m256i* outptr256 = (__m256i*)(out + (j * height + i + (numcol * 4)));
            outptr256[0]       = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_1, out1[4]));
            outptr256[1]       = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_1, out1[12]));

            //will get 6th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 5)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_1, out1[5]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_1, out1[13]));

            //will get 7th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 6)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_1, out1[6]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_1, out1[14]));

            //will get 8th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 7)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_1, out1[7]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_1, out1[15]));

            //will get 9th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 8)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_2, out1[4]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_2, out1[12]));

            //will get 10th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 9)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_2, out1[5]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_2, out1[13]));

            //will get 11th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 10)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_2, out1[6]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_2, out1[14]));

            //will get 12th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 11)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_2, out1[7]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_2, out1[15]));

            //will get 13th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 12)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_3, out1[4]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_3, out1[12]));

            //will get 14th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 13)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_3, out1[5]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_3, out1[13]));

            //will get 15th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 14)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_3, out1[6]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_3, out1[14]));

            //will get 16th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 15)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_3, out1[7]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_3, out1[15]));
        }
    }
}

static AOM_FORCE_INLINE void transpose_16nx16n_N2_half_avx512(int32_t txfm_size, const __m512i* input,
                                                              __m512i* output) {
    const int32_t num_per_512 = 16;
    const int32_t row_size    = txfm_size;
    const int32_t col_size    = txfm_size / num_per_512;
    int32_t       r, c;

    int32_t calc_numrow = row_size >> 1;
    if (!calc_numrow) {
        calc_numrow = 1;
    }

    // transpose each 16x16 block internally
    for (r = 0; r < calc_numrow; r += 16) {
        for (c = 0; c < col_size; c++) {
            transpose_16x16_stride_avx512(col_size, &input[r * col_size + c], &output[c * 16 * col_size + r / 16]);
        }
    }
}

static AOM_FORCE_INLINE void transpose_16nx16n_N2_quad_avx512(int32_t txfm_size, const __m512i* input,
                                                              __m512i* output) {
    const int32_t num_per_512 = 16;
    const int32_t row_size    = txfm_size;
    const int32_t col_size    = txfm_size / num_per_512;
    int32_t       r, c;

    int32_t calc_numrow = row_size >> 1;
    if (!calc_numrow) {
        calc_numrow = 1;
    }
    int32_t calc_numcol = col_size >> 1;
    if (!calc_numcol) {
        calc_numcol = 1;
    }

    // transpose each 16x16 block internally
    for (r = 0; r < calc_numrow; r += 16) {
        for (c = 0; c < calc_numcol; c++) {
            transpose_16x16_stride_avx512(col_size, &input[r * col_size + c], &output[c * 16 * col_size + r / 16]);
        }
    }
}

static AOM_FORCE_INLINE void transpose_16nx16m_N2_half_avx512(const __m512i* in, __m512i* out, const int32_t width,
                                                              const int32_t height) {
    const int32_t numcol = height >> 4;
    const int32_t numrow = width >> 4;
    __m512i       out1[16];

    int32_t calc_numcol = numcol >> 1;
    if (!calc_numcol) {
        calc_numcol = 1;
    }

    for (int32_t j = 0; j < numrow; j++) {
        for (int32_t i = 0; i < calc_numcol; i++) {
            TRANSPOSE_4X4_AVX512(in[i * width + j + (numrow * 0)],
                                 in[i * width + j + (numrow * 1)],
                                 in[i * width + j + (numrow * 2)],
                                 in[i * width + j + (numrow * 3)],
                                 out1[0],
                                 out1[1],
                                 out1[2],
                                 out1[3]);
            TRANSPOSE_4X4_AVX512(in[i * width + j + (numrow * 4)],
                                 in[i * width + j + (numrow * 5)],
                                 in[i * width + j + (numrow * 6)],
                                 in[i * width + j + (numrow * 7)],
                                 out1[4],
                                 out1[5],
                                 out1[6],
                                 out1[7]);
            TRANSPOSE_4X4_AVX512(in[i * width + j + (numrow * 8)],
                                 in[i * width + j + (numrow * 9)],
                                 in[i * width + j + (numrow * 10)],
                                 in[i * width + j + (numrow * 11)],
                                 out1[8],
                                 out1[9],
                                 out1[10],
                                 out1[11]);
            TRANSPOSE_4X4_AVX512(in[i * width + j + (numrow * 12)],
                                 in[i * width + j + (numrow * 13)],
                                 in[i * width + j + (numrow * 14)],
                                 in[i * width + j + (numrow * 15)],
                                 out1[12],
                                 out1[13],
                                 out1[14],
                                 out1[15]);

            __m128i* outptr = (__m128i*)(out + (j * height + i + (numcol * 0)));

            //will get first row of transpose matrix from corresponding 4 vectors in out1
            outptr[0] = _mm512_castsi512_si128(out1[0]);
            outptr[1] = _mm512_castsi512_si128(out1[4]);
            outptr[2] = _mm512_castsi512_si128(out1[8]);
            outptr[3] = _mm512_castsi512_si128(out1[12]);

            //will get second row of transpose matrix from corresponding 4 vectors in out1
            outptr    = (__m128i*)(out + (j * height + i + (numcol * 1)));
            outptr[0] = _mm512_castsi512_si128(out1[1]);
            outptr[1] = _mm512_castsi512_si128(out1[5]);
            outptr[2] = _mm512_castsi512_si128(out1[9]);
            outptr[3] = _mm512_castsi512_si128(out1[13]);

            //will get 3rd row of transpose matrix from corresponding 4 vectors in out1
            outptr    = (__m128i*)(out + (j * height + i + (numcol * 2)));
            outptr[0] = _mm512_castsi512_si128(out1[2]);
            outptr[1] = _mm512_castsi512_si128(out1[6]);
            outptr[2] = _mm512_castsi512_si128(out1[10]);
            outptr[3] = _mm512_castsi512_si128(out1[14]);

            //will get 4th row of transpose matrix from corresponding 4 vectors in out1
            outptr    = (__m128i*)(out + (j * height + i + (numcol * 3)));
            outptr[0] = _mm512_castsi512_si128(out1[3]);
            outptr[1] = _mm512_castsi512_si128(out1[7]);
            outptr[2] = _mm512_castsi512_si128(out1[11]);
            outptr[3] = _mm512_castsi512_si128(out1[15]);

            const __m512i idx_1 = _mm512_setr_epi64(2, 3, 10, 11, 0, 0, 0, 0);
            const __m512i idx_2 = _mm512_setr_epi64(4, 5, 12, 13, 0, 0, 0, 0);
            const __m512i idx_3 = _mm512_setr_epi64(6, 7, 14, 15, 0, 0, 0, 0);

            //will get 5th row of transpose matrix from corresponding 4 vectors in out1
            __m256i* outptr256 = (__m256i*)(out + (j * height + i + (numcol * 4)));
            outptr256[0]       = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_1, out1[4]));
            outptr256[1]       = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_1, out1[12]));

            //will get 6th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 5)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_1, out1[5]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_1, out1[13]));

            //will get 7th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 6)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_1, out1[6]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_1, out1[14]));

            //will get 8th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 7)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_1, out1[7]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_1, out1[15]));

            //will get 9th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 8)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_2, out1[4]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_2, out1[12]));

            //will get 10th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 9)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_2, out1[5]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_2, out1[13]));

            //will get 11th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 10)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_2, out1[6]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_2, out1[14]));

            //will get 12th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 11)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_2, out1[7]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_2, out1[15]));

            //will get 13th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 12)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_3, out1[4]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_3, out1[12]));

            //will get 14th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 13)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_3, out1[5]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_3, out1[13]));

            //will get 15th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 14)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_3, out1[6]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_3, out1[14]));

            //will get 16th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 15)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_3, out1[7]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_3, out1[15]));
        }
    }
}

static AOM_FORCE_INLINE void transpose_16nx16m_N2_quad_avx512(const __m512i* in, __m512i* out, const int32_t width,
                                                              const int32_t height) {
    const int32_t numcol = height >> 4;
    const int32_t numrow = width >> 4;

    int32_t calc_numcol = numcol >> 1;
    if (!calc_numcol) {
        calc_numcol = 1;
    }
    int32_t calc_numrow = numrow >> 1;
    if (!calc_numrow) {
        calc_numrow = 1;
    }

    __m512i out1[16];
    for (int32_t j = 0; j < calc_numrow; j++) {
        for (int32_t i = 0; i < calc_numcol; i++) {
            TRANSPOSE_4X4_AVX512(in[i * width + j + (numrow * 0)],
                                 in[i * width + j + (numrow * 1)],
                                 in[i * width + j + (numrow * 2)],
                                 in[i * width + j + (numrow * 3)],
                                 out1[0],
                                 out1[1],
                                 out1[2],
                                 out1[3]);
            TRANSPOSE_4X4_AVX512(in[i * width + j + (numrow * 4)],
                                 in[i * width + j + (numrow * 5)],
                                 in[i * width + j + (numrow * 6)],
                                 in[i * width + j + (numrow * 7)],
                                 out1[4],
                                 out1[5],
                                 out1[6],
                                 out1[7]);
            TRANSPOSE_4X4_AVX512(in[i * width + j + (numrow * 8)],
                                 in[i * width + j + (numrow * 9)],
                                 in[i * width + j + (numrow * 10)],
                                 in[i * width + j + (numrow * 11)],
                                 out1[8],
                                 out1[9],
                                 out1[10],
                                 out1[11]);
            TRANSPOSE_4X4_AVX512(in[i * width + j + (numrow * 12)],
                                 in[i * width + j + (numrow * 13)],
                                 in[i * width + j + (numrow * 14)],
                                 in[i * width + j + (numrow * 15)],
                                 out1[12],
                                 out1[13],
                                 out1[14],
                                 out1[15]);

            __m128i* outptr = (__m128i*)(out + (j * height + i + (numcol * 0)));

            //will get first row of transpose matrix from corresponding 4 vectors in out1
            outptr[0] = _mm512_castsi512_si128(out1[0]);
            outptr[1] = _mm512_castsi512_si128(out1[4]);
            outptr[2] = _mm512_castsi512_si128(out1[8]);
            outptr[3] = _mm512_castsi512_si128(out1[12]);

            //will get second row of transpose matrix from corresponding 4 vectors in out1
            outptr    = (__m128i*)(out + (j * height + i + (numcol * 1)));
            outptr[0] = _mm512_castsi512_si128(out1[1]);
            outptr[1] = _mm512_castsi512_si128(out1[5]);
            outptr[2] = _mm512_castsi512_si128(out1[9]);
            outptr[3] = _mm512_castsi512_si128(out1[13]);

            //will get 3rd row of transpose matrix from corresponding 4 vectors in out1
            outptr    = (__m128i*)(out + (j * height + i + (numcol * 2)));
            outptr[0] = _mm512_castsi512_si128(out1[2]);
            outptr[1] = _mm512_castsi512_si128(out1[6]);
            outptr[2] = _mm512_castsi512_si128(out1[10]);
            outptr[3] = _mm512_castsi512_si128(out1[14]);

            //will get 4th row of transpose matrix from corresponding 4 vectors in out1
            outptr    = (__m128i*)(out + (j * height + i + (numcol * 3)));
            outptr[0] = _mm512_castsi512_si128(out1[3]);
            outptr[1] = _mm512_castsi512_si128(out1[7]);
            outptr[2] = _mm512_castsi512_si128(out1[11]);
            outptr[3] = _mm512_castsi512_si128(out1[15]);

            const __m512i idx_1 = _mm512_setr_epi64(2, 3, 10, 11, 0, 0, 0, 0);
            const __m512i idx_2 = _mm512_setr_epi64(4, 5, 12, 13, 0, 0, 0, 0);
            const __m512i idx_3 = _mm512_setr_epi64(6, 7, 14, 15, 0, 0, 0, 0);

            //will get 5th row of transpose matrix from corresponding 4 vectors in out1
            __m256i* outptr256 = (__m256i*)(out + (j * height + i + (numcol * 4)));
            outptr256[0]       = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_1, out1[4]));
            outptr256[1]       = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_1, out1[12]));

            //will get 6th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 5)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_1, out1[5]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_1, out1[13]));

            //will get 7th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 6)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_1, out1[6]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_1, out1[14]));

            //will get 8th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 7)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_1, out1[7]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_1, out1[15]));

            //will get 9th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 8)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_2, out1[4]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_2, out1[12]));

            //will get 10th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 9)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_2, out1[5]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_2, out1[13]));

            //will get 11th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 10)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_2, out1[6]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_2, out1[14]));

            //will get 12th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 11)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_2, out1[7]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_2, out1[15]));

            //will get 13th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 12)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_3, out1[4]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_3, out1[12]));

            //will get 14th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 13)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_3, out1[5]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_3, out1[13]));

            //will get 15th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 14)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_3, out1[6]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_3, out1[14]));

            //will get 16th row of transpose matrix from corresponding 4 vectors in out1
            outptr256    = (__m256i*)(out + (j * height + i + (numcol * 15)));
            outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_3, out1[7]));
            outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_3, out1[15]));
        }
    }
}

static AOM_FORCE_INLINE void transpose_16nx16n_N4_half_avx512(int32_t txfm_size, const __m512i* input,
                                                              __m512i* output) {
    const int32_t num_per_512 = 16;
    const int32_t row_size    = txfm_size;
    const int32_t col_size    = txfm_size / num_per_512;
    int32_t       r, c;

    int32_t calc_numrow = row_size >> 2;
    if (!calc_numrow) {
        calc_numrow = 1;
    }

    // transpose each 16x16 block internally
    for (r = 0; r < calc_numrow; r += 16) {
        for (c = 0; c < col_size; c++) {
            transpose_16x16_stride_avx512(col_size, &input[r * col_size + c], &output[c * 16 * col_size + r / 16]);
        }
    }
}

static AOM_FORCE_INLINE void transpose_16nx16n_N4_quad_avx512(int32_t txfm_size, const __m512i* input,
                                                              __m512i* output) {
    const int32_t num_per_512 = 16;
    const int32_t row_size    = txfm_size;
    const int32_t col_size    = txfm_size / num_per_512;
    int32_t       r, c;

    int32_t calc_numrow = row_size >> 2;
    if (!calc_numrow) {
        calc_numrow = 1;
    }
    int32_t calc_numcol = col_size >> 2;
    if (!calc_numcol) {
        calc_numcol = 1;
    }

    // transpose each 16x16 block internally
    for (r = 0; r < calc_numrow; r += 16) {
        for (c = 0; c < calc_numcol; c++) {
            transpose_16x16_stride_avx512(col_size, &input[r * col_size + c], &output[c * 16 * col_size + r / 16]);
        }
    }
}
#endif // AOM_DSP_X86_TRANSPOSE_ENCODER_AVX512_H_
