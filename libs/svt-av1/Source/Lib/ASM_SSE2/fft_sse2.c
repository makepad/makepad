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

#include <xmmintrin.h>

#include "definitions.h"
#include "fft_common.h"

static INLINE void transpose4x4(const float* A, float* b, const int32_t lda, const int32_t ldb) {
    __m128 row1 = _mm_loadu_ps(&A[0 * lda]);
    __m128 row2 = _mm_loadu_ps(&A[1 * lda]);
    __m128 row3 = _mm_loadu_ps(&A[2 * lda]);
    __m128 row4 = _mm_loadu_ps(&A[3 * lda]);
    _MM_TRANSPOSE4_PS(row1, row2, row3, row4);
    _mm_storeu_ps(&b[0 * ldb], row1);
    _mm_storeu_ps(&b[1 * ldb], row2);
    _mm_storeu_ps(&b[2 * ldb], row3);
    _mm_storeu_ps(&b[3 * ldb], row4);
}

void svt_aom_transpose_float_sse2(const float* A, float* b, int32_t n) {
    for (int32_t y = 0; y < n; y += 4) {
        for (int32_t x = 0; x < n; x += 4) {
            transpose4x4(A + y * n + x, b + x * n + y, n, n);
        }
    }
}

void svt_aom_fft_unpack_2d_output_sse2(const float* packed, float* output, int32_t n) {
    const int32_t n2         = n / 2;
    output[0]                = packed[0];
    output[1]                = 0;
    output[2 * (n2 * n)]     = packed[n2 * n];
    output[2 * (n2 * n) + 1] = 0;

    output[2 * n2]                = packed[n2];
    output[2 * n2 + 1]            = 0;
    output[2 * (n2 * n + n2)]     = packed[n2 * n + n2];
    output[2 * (n2 * n + n2) + 1] = 0;

    for (int32_t c = 1; c < n2; ++c) {
        output[2 * (0 * n + c)]      = packed[c];
        output[2 * (0 * n + c) + 1]  = packed[c + n2];
        output[2 * (n2 * n + c) + 0] = packed[n2 * n + c];
        output[2 * (n2 * n + c) + 1] = packed[n2 * n + c + n2];
    }
    for (int32_t r = 1; r < n2; ++r) {
        output[2 * (r * n + 0)]      = packed[r * n];
        output[2 * (r * n + 0) + 1]  = packed[(r + n2) * n];
        output[2 * (r * n + n2) + 0] = packed[r * n + n2];
        output[2 * (r * n + n2) + 1] = packed[(r + n2) * n + n2];

        for (int32_t c = 1; c < AOMMIN(n2, 4); ++c) {
            output[2 * (r * n + c)]     = packed[r * n + c] - packed[(r + n2) * n + c + n2];
            output[2 * (r * n + c) + 1] = packed[(r + n2) * n + c] + packed[r * n + c + n2];
        }

        for (int32_t c = 4; c < n2; c += 4) {
            __m128 real1 = _mm_loadu_ps(packed + r * n + c);
            __m128 real2 = _mm_loadu_ps(packed + (r + n2) * n + c + n2);
            __m128 imag1 = _mm_loadu_ps(packed + (r + n2) * n + c);
            __m128 imag2 = _mm_loadu_ps(packed + r * n + c + n2);
            real1        = _mm_sub_ps(real1, real2);
            imag1        = _mm_add_ps(imag1, imag2);
            _mm_storeu_ps(output + 2 * (r * n + c), _mm_unpacklo_ps(real1, imag1));
            _mm_storeu_ps(output + 2 * (r * n + c + 2), _mm_unpackhi_ps(real1, imag1));
        }

        int32_t r2                    = r + n2;
        int32_t r3                    = n - r2;
        output[2 * (r2 * n + 0)]      = packed[r3 * n];
        output[2 * (r2 * n + 0) + 1]  = -packed[(r3 + n2) * n];
        output[2 * (r2 * n + n2)]     = packed[r3 * n + n2];
        output[2 * (r2 * n + n2) + 1] = -packed[(r3 + n2) * n + n2];
        for (int32_t c = 1; c < AOMMIN(4, n2); ++c) {
            output[2 * (r2 * n + c)]     = packed[r3 * n + c] + packed[(r3 + n2) * n + c + n2];
            output[2 * (r2 * n + c) + 1] = -packed[(r3 + n2) * n + c] + packed[r3 * n + c + n2];
        }
        for (int32_t c = 4; c < n2; c += 4) {
            __m128 real1 = _mm_loadu_ps(packed + r3 * n + c);
            __m128 real2 = _mm_loadu_ps(packed + (r3 + n2) * n + c + n2);
            __m128 imag1 = _mm_loadu_ps(packed + (r3 + n2) * n + c);
            __m128 imag2 = _mm_loadu_ps(packed + r3 * n + c + n2);
            real1        = _mm_add_ps(real1, real2);
            imag1        = _mm_sub_ps(imag2, imag1);
            _mm_storeu_ps(output + 2 * (r2 * n + c), _mm_unpacklo_ps(real1, imag1));
            _mm_storeu_ps(output + 2 * (r2 * n + c + 2), _mm_unpackhi_ps(real1, imag1));
        }
    }
}

// Generate definitions for 1d transforms using float and __mm128
GEN_FFT_4(static INLINE void, sse2, float, __m128, _mm_loadu_ps, _mm_storeu_ps, _mm_set1_ps, _mm_add_ps, _mm_sub_ps);

void svt_aom_fft4x4_float_sse2(const float* input, float* temp, float* output) {
    svt_aom_fft_2d_gen(input,
                       temp,
                       output,
                       4,
                       svt_aom_fft1d_4_sse2,
                       svt_aom_transpose_float_sse2,
                       svt_aom_fft_unpack_2d_output_sse2,
                       4);
}

// Generate definitions for 1d inverse transforms using float and mm128
GEN_IFFT_4(static INLINE void, sse2, float, __m128, _mm_loadu_ps, _mm_storeu_ps, _mm_set1_ps, _mm_add_ps, _mm_sub_ps);

void svt_aom_ifft4x4_float_sse2(const float* input, float* temp, float* output) {
    svt_aom_ifft_2d_gen(input,
                        temp,
                        output,
                        4,
                        svt_aom_fft1d_4_float,
                        svt_aom_fft1d_4_sse2,
                        svt_aom_ifft1d_4_sse2,
                        svt_aom_transpose_float_sse2,
                        4);
}
