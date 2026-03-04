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

#ifndef AOM_DSP_X86_TRANSPOSE_AVX512_H_
#define AOM_DSP_X86_TRANSPOSE_AVX512_H_

#include "definitions.h"
#include <immintrin.h> // AVX2

#define TRANSPOSE_4X4_AVX512(x0, x1, x2, x3, y0, y1, y2, y3) \
    do {                                                     \
        __m512i u0, u1, u2, u3;                              \
        u0 = _mm512_unpacklo_epi32(x0, x1);                  \
        u1 = _mm512_unpackhi_epi32(x0, x1);                  \
        u2 = _mm512_unpacklo_epi32(x2, x3);                  \
        u3 = _mm512_unpackhi_epi32(x2, x3);                  \
        y0 = _mm512_unpacklo_epi64(u0, u2);                  \
        y1 = _mm512_unpackhi_epi64(u0, u2);                  \
        y2 = _mm512_unpacklo_epi64(u1, u3);                  \
        y3 = _mm512_unpackhi_epi64(u1, u3);                  \
    } while (0)

static INLINE void transpose_16x16_avx512(const __m512i* in, __m512i* out) {
    __m512i out1[16];
    TRANSPOSE_4X4_AVX512(in[0], in[1], in[2], in[3], out1[0], out1[1], out1[2], out1[3]);
    TRANSPOSE_4X4_AVX512(in[4], in[5], in[6], in[7], out1[4], out1[5], out1[6], out1[7]);
    TRANSPOSE_4X4_AVX512(in[8], in[9], in[10], in[11], out1[8], out1[9], out1[10], out1[11]);
    TRANSPOSE_4X4_AVX512(in[12], in[13], in[14], in[15], out1[12], out1[13], out1[14], out1[15]);

    __m128i* outptr = (__m128i*)(out + 0);

    //will get first row of transpose matrix from corresponding 4 vectors in out1
    outptr[0] = _mm512_castsi512_si128(out1[0]);
    outptr[1] = _mm512_castsi512_si128(out1[4]);
    outptr[2] = _mm512_castsi512_si128(out1[8]);
    outptr[3] = _mm512_castsi512_si128(out1[12]);

    //will get second row of transpose matrix from corresponding 4 vectors in out1
    outptr    = (__m128i*)(out + 1);
    outptr[0] = _mm512_castsi512_si128(out1[1]);
    outptr[1] = _mm512_castsi512_si128(out1[5]);
    outptr[2] = _mm512_castsi512_si128(out1[9]);
    outptr[3] = _mm512_castsi512_si128(out1[13]);

    //will get 3rd row of transpose matrix from corresponding 4 vectors in out1
    outptr    = (__m128i*)(out + 2);
    outptr[0] = _mm512_castsi512_si128(out1[2]);
    outptr[1] = _mm512_castsi512_si128(out1[6]);
    outptr[2] = _mm512_castsi512_si128(out1[10]);
    outptr[3] = _mm512_castsi512_si128(out1[14]);

    //will get 4th row of transpose matrix from corresponding 4 vectors in out1
    outptr    = (__m128i*)(out + 3);
    outptr[0] = _mm512_castsi512_si128(out1[3]);
    outptr[1] = _mm512_castsi512_si128(out1[7]);
    outptr[2] = _mm512_castsi512_si128(out1[11]);
    outptr[3] = _mm512_castsi512_si128(out1[15]);

    const __m512i idx_1 = _mm512_setr_epi64(2, 3, 10, 11, 0, 0, 0, 0);
    const __m512i idx_2 = _mm512_setr_epi64(4, 5, 12, 13, 0, 0, 0, 0);
    const __m512i idx_3 = _mm512_setr_epi64(6, 7, 14, 15, 0, 0, 0, 0);

    //will get 5th row of transpose matrix from corresponding 4 vectors in out1
    __m256i* outptr256 = (__m256i*)(out + 4);
    outptr256[0]       = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_1, out1[4]));
    outptr256[1]       = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_1, out1[12]));

    //will get 6th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 5);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_1, out1[5]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_1, out1[13]));

    //will get 7th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 6);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_1, out1[6]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_1, out1[14]));

    //will get 8th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 7);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_1, out1[7]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_1, out1[15]));

    //will get 9th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 8);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_2, out1[4]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_2, out1[12]));

    //will get 10th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 9);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_2, out1[5]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_2, out1[13]));

    //will get 11th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 10);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_2, out1[6]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_2, out1[14]));

    //will get 12th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 11);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_2, out1[7]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_2, out1[15]));

    //will get 13th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 12);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_3, out1[4]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_3, out1[12]));

    //will get 14th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 13);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_3, out1[5]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_3, out1[13]));

    //will get 15th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 14);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_3, out1[6]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_3, out1[14]));

    //will get 16th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 15);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_3, out1[7]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_3, out1[15]));
}

static INLINE void transpose_16x16_stride_avx512(int32_t stride, const __m512i* in, __m512i* out) {
    __m512i out1[16];
    TRANSPOSE_4X4_AVX512(
        in[0 * stride], in[1 * stride], in[2 * stride], in[3 * stride], out1[0], out1[1], out1[2], out1[3]);
    TRANSPOSE_4X4_AVX512(
        in[4 * stride], in[5 * stride], in[6 * stride], in[7 * stride], out1[4], out1[5], out1[6], out1[7]);
    TRANSPOSE_4X4_AVX512(
        in[8 * stride], in[9 * stride], in[10 * stride], in[11 * stride], out1[8], out1[9], out1[10], out1[11]);
    TRANSPOSE_4X4_AVX512(
        in[12 * stride], in[13 * stride], in[14 * stride], in[15 * stride], out1[12], out1[13], out1[14], out1[15]);

    __m128i* outptr = (__m128i*)(out + 0 * stride);

    //will get first row of transpose matrix from corresponding 4 vectors in out1
    outptr[0] = _mm512_castsi512_si128(out1[0]);
    outptr[1] = _mm512_castsi512_si128(out1[4]);
    outptr[2] = _mm512_castsi512_si128(out1[8]);
    outptr[3] = _mm512_castsi512_si128(out1[12]);

    //will get second row of transpose matrix from corresponding 4 vectors in out1
    outptr    = (__m128i*)(out + 1 * stride);
    outptr[0] = _mm512_castsi512_si128(out1[1]);
    outptr[1] = _mm512_castsi512_si128(out1[5]);
    outptr[2] = _mm512_castsi512_si128(out1[9]);
    outptr[3] = _mm512_castsi512_si128(out1[13]);

    //will get 3rd row of transpose matrix from corresponding 4 vectors in out1
    outptr    = (__m128i*)(out + 2 * stride);
    outptr[0] = _mm512_castsi512_si128(out1[2]);
    outptr[1] = _mm512_castsi512_si128(out1[6]);
    outptr[2] = _mm512_castsi512_si128(out1[10]);
    outptr[3] = _mm512_castsi512_si128(out1[14]);

    //will get 4th row of transpose matrix from corresponding 4 vectors in out1
    outptr    = (__m128i*)(out + 3 * stride);
    outptr[0] = _mm512_castsi512_si128(out1[3]);
    outptr[1] = _mm512_castsi512_si128(out1[7]);
    outptr[2] = _mm512_castsi512_si128(out1[11]);
    outptr[3] = _mm512_castsi512_si128(out1[15]);

    const __m512i idx_1 = _mm512_setr_epi64(2, 3, 10, 11, 0, 0, 0, 0);
    const __m512i idx_2 = _mm512_setr_epi64(4, 5, 12, 13, 0, 0, 0, 0);
    const __m512i idx_3 = _mm512_setr_epi64(6, 7, 14, 15, 0, 0, 0, 0);

    //will get 5th row of transpose matrix from corresponding 4 vectors in out1
    __m256i* outptr256 = (__m256i*)(out + 4 * stride);
    outptr256[0]       = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_1, out1[4]));
    outptr256[1]       = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_1, out1[12]));

    //will get 6th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 5 * stride);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_1, out1[5]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_1, out1[13]));

    //will get 7th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 6 * stride);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_1, out1[6]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_1, out1[14]));

    //will get 8th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 7 * stride);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_1, out1[7]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_1, out1[15]));

    //will get 9th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 8 * stride);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_2, out1[4]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_2, out1[12]));

    //will get 10th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 9 * stride);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_2, out1[5]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_2, out1[13]));

    //will get 11th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 10 * stride);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_2, out1[6]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_2, out1[14]));

    //will get 12th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 11 * stride);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_2, out1[7]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_2, out1[15]));

    //will get 13th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 12 * stride);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[0], idx_3, out1[4]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[8], idx_3, out1[12]));

    //will get 14th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 13 * stride);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[1], idx_3, out1[5]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[9], idx_3, out1[13]));

    //will get 15th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 14 * stride);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[2], idx_3, out1[6]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[10], idx_3, out1[14]));

    //will get 16th row of transpose matrix from corresponding 4 vectors in out1
    outptr256    = (__m256i*)(out + 15 * stride);
    outptr256[0] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[3], idx_3, out1[7]));
    outptr256[1] = _mm512_castsi512_si256(_mm512_permutex2var_epi64(out1[11], idx_3, out1[15]));
}

static INLINE void transpose_16nx16n_avx512(int32_t txfm_size, const __m512i* input, __m512i* output) {
    const int32_t num_per_512 = 16;
    const int32_t row_size    = txfm_size;
    const int32_t col_size    = txfm_size / num_per_512;
    int32_t       r, c;

    // transpose each 16x16 block internally
    for (r = 0; r < row_size; r += 16) {
        for (c = 0; c < col_size; c++) {
            transpose_16x16_stride_avx512(col_size, &input[r * col_size + c], &output[c * 16 * col_size + r / 16]);
        }
    }
}

static INLINE void transpose_16nx16m_inv_avx512(const __m512i* in, __m512i* out, const int32_t width,
                                                const int32_t height) {
    const int32_t numcol = height >> 4;
    const int32_t numrow = width >> 4;

    __m512i out1[16];

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

#endif // AOM_DSP_X86_TRANSPOSE_AVX512_H_
