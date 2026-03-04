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

#include <immintrin.h>

#include "definitions.h"
#include "fft_common.h"

extern void svt_aom_transpose_float_sse2(const float* A, float* b, int32_t n);
extern void svt_aom_fft_unpack_2d_output_sse2(const float* col_fft, float* output, int32_t n);

// Generate the 1d forward transforms for float using _mm256
GEN_FFT_8(static INLINE void, avx2, float, __m256, _mm256_loadu_ps, _mm256_storeu_ps, _mm256_set1_ps, _mm256_add_ps,
          _mm256_sub_ps, _mm256_mul_ps);
GEN_FFT_16(static INLINE void, avx2, float, __m256, _mm256_loadu_ps, _mm256_storeu_ps, _mm256_set1_ps, _mm256_add_ps,
           _mm256_sub_ps, _mm256_mul_ps);
GEN_FFT_32(static INLINE void, avx2, float, __m256, _mm256_loadu_ps, _mm256_storeu_ps, _mm256_set1_ps, _mm256_add_ps,
           _mm256_sub_ps, _mm256_mul_ps);

static INLINE void transpose8x8_float(const float* A, float* b, const int32_t lda, const int32_t ldb) {
    __m256 in0 = _mm256_loadu_ps(&A[0 * lda]);
    __m256 in1 = _mm256_loadu_ps(&A[1 * lda]);
    __m256 in2 = _mm256_loadu_ps(&A[2 * lda]);
    __m256 in3 = _mm256_loadu_ps(&A[3 * lda]);
    __m256 in4 = _mm256_loadu_ps(&A[4 * lda]);
    __m256 in5 = _mm256_loadu_ps(&A[5 * lda]);
    __m256 in6 = _mm256_loadu_ps(&A[6 * lda]);
    __m256 in7 = _mm256_loadu_ps(&A[7 * lda]);

    __m256 temp0 = _mm256_unpacklo_ps(in0, in2);
    __m256 temp1 = _mm256_unpackhi_ps(in0, in2);
    __m256 temp2 = _mm256_unpacklo_ps(in1, in3);
    __m256 temp3 = _mm256_unpackhi_ps(in1, in3);
    __m256 temp4 = _mm256_unpacklo_ps(in4, in6);
    __m256 temp5 = _mm256_unpackhi_ps(in4, in6);
    __m256 temp6 = _mm256_unpacklo_ps(in5, in7);
    __m256 temp7 = _mm256_unpackhi_ps(in5, in7);

    in0 = _mm256_unpacklo_ps(temp0, temp2);
    in1 = _mm256_unpackhi_ps(temp0, temp2);
    in4 = _mm256_unpacklo_ps(temp1, temp3);
    in5 = _mm256_unpackhi_ps(temp1, temp3);
    in2 = _mm256_unpacklo_ps(temp4, temp6);
    in3 = _mm256_unpackhi_ps(temp4, temp6);
    in6 = _mm256_unpacklo_ps(temp5, temp7);
    in7 = _mm256_unpackhi_ps(temp5, temp7);

    temp0 = _mm256_permute2f128_ps(in0, in2, 0x20);
    temp1 = _mm256_permute2f128_ps(in1, in3, 0x20);
    temp2 = _mm256_permute2f128_ps(in4, in6, 0x20);
    temp3 = _mm256_permute2f128_ps(in5, in7, 0x20);
    temp4 = _mm256_permute2f128_ps(in0, in2, 0x31);
    temp5 = _mm256_permute2f128_ps(in1, in3, 0x31);
    temp6 = _mm256_permute2f128_ps(in4, in6, 0x31);
    temp7 = _mm256_permute2f128_ps(in5, in7, 0x31);

    _mm256_storeu_ps(&b[0 * ldb], temp0);
    _mm256_storeu_ps(&b[1 * ldb], temp1);
    _mm256_storeu_ps(&b[2 * ldb], temp2);
    _mm256_storeu_ps(&b[3 * ldb], temp3);
    _mm256_storeu_ps(&b[4 * ldb], temp4);
    _mm256_storeu_ps(&b[5 * ldb], temp5);
    _mm256_storeu_ps(&b[6 * ldb], temp6);
    _mm256_storeu_ps(&b[7 * ldb], temp7);
}

void svt_aom_transpose_float_avx2(const float* A, float* b, int32_t n) {
    for (int32_t y = 0; y < n; y += 8) {
        for (int32_t x = 0; x < n; x += 8) {
            transpose8x8_float(A + y * n + x, b + x * n + y, n, n);
        }
    }
}

void svt_aom_fft8x8_float_avx2(const float* input, float* temp, float* output) {
    svt_aom_fft_2d_gen(input,
                       temp,
                       output,
                       8,
                       svt_aom_fft1d_8_avx2,
                       svt_aom_transpose_float_avx2,
                       svt_aom_fft_unpack_2d_output_sse2,
                       8);
}

void svt_aom_fft16x16_float_avx2(const float* input, float* temp, float* output) {
    svt_aom_fft_2d_gen(input,
                       temp,
                       output,
                       16,
                       svt_aom_fft1d_16_avx2,
                       svt_aom_transpose_float_avx2,
                       svt_aom_fft_unpack_2d_output_sse2,
                       8);
}

void svt_aom_fft32x32_float_avx2(const float* input, float* temp, float* output) {
    svt_aom_fft_2d_gen(input,
                       temp,
                       output,
                       32,
                       svt_aom_fft1d_32_avx2,
                       svt_aom_transpose_float_avx2,
                       svt_aom_fft_unpack_2d_output_sse2,
                       8);
}

// Generate the 1d inverse transforms for float using _mm256
GEN_IFFT_8(static INLINE void, avx2, float, __m256, _mm256_loadu_ps, _mm256_storeu_ps, _mm256_set1_ps, _mm256_add_ps,
           _mm256_sub_ps, _mm256_mul_ps);
GEN_IFFT_16(static INLINE void, avx2, float, __m256, _mm256_loadu_ps, _mm256_storeu_ps, _mm256_set1_ps, _mm256_add_ps,
            _mm256_sub_ps, _mm256_mul_ps);
GEN_IFFT_32(static INLINE void, avx2, float, __m256, _mm256_loadu_ps, _mm256_storeu_ps, _mm256_set1_ps, _mm256_add_ps,
            _mm256_sub_ps, _mm256_mul_ps);

void svt_aom_ifft8x8_float_avx2(const float* input, float* temp, float* output) {
    svt_aom_ifft_2d_gen(input,
                        temp,
                        output,
                        8,
                        svt_aom_fft1d_8_float,
                        svt_aom_fft1d_8_avx2,
                        svt_aom_ifft1d_8_avx2,
                        svt_aom_transpose_float_avx2,
                        8);
}

void svt_aom_ifft16x16_float_avx2(const float* input, float* temp, float* output) {
    svt_aom_ifft_2d_gen(input,
                        temp,
                        output,
                        16,
                        svt_aom_fft1d_16_float,
                        svt_aom_fft1d_16_avx2,
                        svt_aom_ifft1d_16_avx2,
                        svt_aom_transpose_float_avx2,
                        8);
}

void svt_aom_ifft32x32_float_avx2(const float* input, float* temp, float* output) {
    svt_aom_ifft_2d_gen(input,
                        temp,
                        output,
                        32,
                        svt_aom_fft1d_32_float,
                        svt_aom_fft1d_32_avx2,
                        svt_aom_ifft1d_32_avx2,
                        svt_aom_transpose_float_avx2,
                        8);
}
