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

#include "picture_operators_sse2.h"
#include <emmintrin.h>
#include "definitions.h"

/******************************************************************************************************
                                       svt_residual_kernel16bit_sse2_intrin
******************************************************************************************************/
void svt_residual_kernel16bit_sse2_intrin(uint16_t* input, uint32_t input_stride, uint16_t* pred, uint32_t pred_stride,
                                          int16_t* residual, uint32_t residual_stride, uint32_t area_width,
                                          uint32_t area_height) {
    uint32_t x, y;
    __m128i  residual0, residual1;

    if (area_width == 4) {
        for (y = 0; y < area_height; y += 2) {
            residual0 = _mm_sub_epi16(_mm_loadl_epi64((__m128i*)input), _mm_loadl_epi64((__m128i*)pred));
            residual1 = _mm_sub_epi16(_mm_loadl_epi64((__m128i*)(input + input_stride)),
                                      _mm_loadl_epi64((__m128i*)(pred + pred_stride)));

            _mm_storel_epi64((__m128i*)residual, residual0);
            _mm_storel_epi64((__m128i*)(residual + residual_stride), residual1);

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
    } else if (area_width == 8) {
        for (y = 0; y < area_height; y += 2) {
            residual0 = _mm_sub_epi16(_mm_loadu_si128((__m128i*)input), _mm_loadu_si128((__m128i*)pred));
            residual1 = _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride)),
                                      _mm_loadu_si128((__m128i*)(pred + pred_stride)));

            _mm_storeu_si128((__m128i*)residual, residual0);
            _mm_storeu_si128((__m128i*)(residual + residual_stride), residual1);

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
    } else if (area_width == 16) {
        __m128i residual2, residual3;

        for (y = 0; y < area_height; y += 2) {
            residual0 = _mm_sub_epi16(_mm_loadu_si128((__m128i*)input), _mm_loadu_si128((__m128i*)pred));
            residual1 = _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + 8)), _mm_loadu_si128((__m128i*)(pred + 8)));
            residual2 = _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride)),
                                      _mm_loadu_si128((__m128i*)(pred + pred_stride)));
            residual3 = _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride + 8)),
                                      _mm_loadu_si128((__m128i*)(pred + pred_stride + 8)));

            _mm_storeu_si128((__m128i*)residual, residual0);
            _mm_storeu_si128((__m128i*)(residual + 8), residual1);
            _mm_storeu_si128((__m128i*)(residual + residual_stride), residual2);
            _mm_storeu_si128((__m128i*)(residual + residual_stride + 8), residual3);

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
    } else if (area_width == 32) {
        for (y = 0; y < area_height; y += 2) {
            //residual[column_index] = ((int16_t)input[column_index]) - ((int16_t)pred[column_index]);
            _mm_storeu_si128((__m128i*)residual,
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)input), _mm_loadu_si128((__m128i*)pred)));
            _mm_storeu_si128(
                (__m128i*)(residual + 8),
                _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + 8)), _mm_loadu_si128((__m128i*)(pred + 8))));
            _mm_storeu_si128(
                (__m128i*)(residual + 16),
                _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + 16)), _mm_loadu_si128((__m128i*)(pred + 16))));
            _mm_storeu_si128(
                (__m128i*)(residual + 24),
                _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + 24)), _mm_loadu_si128((__m128i*)(pred + 24))));

            _mm_storeu_si128((__m128i*)(residual + residual_stride),
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride)),
                                           _mm_loadu_si128((__m128i*)(pred + pred_stride))));
            _mm_storeu_si128((__m128i*)(residual + residual_stride + 8),
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride + 8)),
                                           _mm_loadu_si128((__m128i*)(pred + pred_stride + 8))));
            _mm_storeu_si128((__m128i*)(residual + residual_stride + 16),
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride + 16)),
                                           _mm_loadu_si128((__m128i*)(pred + pred_stride + 16))));
            _mm_storeu_si128((__m128i*)(residual + residual_stride + 24),
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride + 24)),
                                           _mm_loadu_si128((__m128i*)(pred + pred_stride + 24))));

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
    } else if (area_width == 64) { // Branch was not tested because the encoder had max txb_size of 32

        for (y = 0; y < area_height; y += 2) {
            //residual[column_index] = ((int16_t)input[column_index]) - ((int16_t)pred[column_index]) 8 indices per _mm_sub_epi16
            _mm_storeu_si128((__m128i*)residual,
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)input), _mm_loadu_si128((__m128i*)pred)));
            _mm_storeu_si128(
                (__m128i*)(residual + 8),
                _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + 8)), _mm_loadu_si128((__m128i*)(pred + 8))));
            _mm_storeu_si128(
                (__m128i*)(residual + 16),
                _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + 16)), _mm_loadu_si128((__m128i*)(pred + 16))));
            _mm_storeu_si128(
                (__m128i*)(residual + 24),
                _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + 24)), _mm_loadu_si128((__m128i*)(pred + 24))));
            _mm_storeu_si128(
                (__m128i*)(residual + 32),
                _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + 32)), _mm_loadu_si128((__m128i*)(pred + 32))));
            _mm_storeu_si128(
                (__m128i*)(residual + 40),
                _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + 40)), _mm_loadu_si128((__m128i*)(pred + 40))));
            _mm_storeu_si128(
                (__m128i*)(residual + 48),
                _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + 48)), _mm_loadu_si128((__m128i*)(pred + 48))));
            _mm_storeu_si128(
                (__m128i*)(residual + 56),
                _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + 56)), _mm_loadu_si128((__m128i*)(pred + 56))));

            _mm_storeu_si128((__m128i*)(residual + residual_stride),
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride)),
                                           _mm_loadu_si128((__m128i*)(pred + pred_stride))));
            _mm_storeu_si128((__m128i*)(residual + residual_stride + 8),
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride + 8)),
                                           _mm_loadu_si128((__m128i*)(pred + pred_stride + 8))));
            _mm_storeu_si128((__m128i*)(residual + residual_stride + 16),
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride + 16)),
                                           _mm_loadu_si128((__m128i*)(pred + pred_stride + 16))));
            _mm_storeu_si128((__m128i*)(residual + residual_stride + 24),
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride + 24)),
                                           _mm_loadu_si128((__m128i*)(pred + pred_stride + 24))));
            _mm_storeu_si128((__m128i*)(residual + residual_stride + 32),
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride + 32)),
                                           _mm_loadu_si128((__m128i*)(pred + pred_stride + 32))));
            _mm_storeu_si128((__m128i*)(residual + residual_stride + 40),
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride + 40)),
                                           _mm_loadu_si128((__m128i*)(pred + pred_stride + 40))));
            _mm_storeu_si128((__m128i*)(residual + residual_stride + 48),
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride + 48)),
                                           _mm_loadu_si128((__m128i*)(pred + pred_stride + 48))));
            _mm_storeu_si128((__m128i*)(residual + residual_stride + 56),
                             _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride + 56)),
                                           _mm_loadu_si128((__m128i*)(pred + pred_stride + 56))));

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
    } else {
        uint32_t input_stride_diff    = 2 * input_stride;
        uint32_t pred_stride_diff     = 2 * pred_stride;
        uint32_t residual_stride_diff = 2 * residual_stride;
        input_stride_diff -= area_width;
        pred_stride_diff -= area_width;
        residual_stride_diff -= area_width;

        if (!(area_width & 7)) {
            for (x = 0; x < area_height; x += 2) {
                for (y = 0; y < area_width; y += 8) {
                    _mm_storeu_si128((__m128i*)residual,
                                     _mm_sub_epi16(_mm_loadu_si128((__m128i*)input), _mm_loadu_si128((__m128i*)pred)));
                    _mm_storeu_si128((__m128i*)(residual + residual_stride),
                                     _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride)),
                                                   _mm_loadu_si128((__m128i*)(pred + pred_stride))));

                    input += 8;
                    pred += 8;
                    residual += 8;
                }
                input    = input + input_stride_diff;
                pred     = pred + pred_stride_diff;
                residual = residual + residual_stride_diff;
            }
        } else {
            for (x = 0; x < area_height; x += 2) {
                for (y = 0; y < area_width; y += 4) {
                    _mm_storel_epi64((__m128i*)residual,
                                     _mm_sub_epi16(_mm_loadu_si128((__m128i*)input), _mm_loadu_si128((__m128i*)pred)));
                    _mm_storel_epi64((__m128i*)(residual + residual_stride),
                                     _mm_sub_epi16(_mm_loadu_si128((__m128i*)(input + input_stride)),
                                                   _mm_loadu_si128((__m128i*)(pred + pred_stride))));

                    input += 4;
                    pred += 4;
                    residual += 4;
                }
                input    = input + input_stride_diff;
                pred     = pred + pred_stride_diff;
                residual = residual + residual_stride_diff;
            }
        }
    }
    return;
}

/********************************************************************************************
* faster memcopy for <= 64B blocks, great w/ inlining and size known at compile time (or w/ PGO)
* THIS NEEDS TO STAY IN A HEADER FOR BEST PERFORMANCE
********************************************************************************************/
#ifdef ARCH_X86_64
#include <immintrin.h>
#if defined(__GNUC__) && !defined(__clang__) && !defined(__ICC__)
__attribute__((optimize("unroll-loops")))
#endif
void svt_memcpy_intrin_sse(void* dst_ptr, void const* src_ptr, size_t size) {
    const unsigned char* src = src_ptr;
    unsigned char*       dst = dst_ptr;
    size_t               i   = 0;

#ifdef _INTEL_COMPILER
#pragma unroll
#endif
    while ((i + 16) <= size) {
        _mm_storeu_ps((float*)(void*)(dst + i), _mm_loadu_ps((const float*)(const void*)(src + i)));
        i += 16;
    }

    if ((i + 8) <= size) {
        _mm_store_sd((double*)(void*)(dst + i), _mm_load_sd((const double*)(const void*)(src + i)));
        i += 8;
    }

    for (; i < size; ++i) {
        dst[i] = src[i];
    }
}

// Store 8 16 bit values. If the destination is 32 bits then sign extend the
// values by multiplying by 1.
static INLINE void store_tran_low(__m128i a, int32_t* b) {
    const __m128i one  = _mm_set1_epi16(1);
    const __m128i a_hi = _mm_mulhi_epi16(a, one);
    const __m128i a_lo = _mm_mullo_epi16(a, one);
    const __m128i a_1  = _mm_unpacklo_epi16(a_lo, a_hi);
    const __m128i a_2  = _mm_unpackhi_epi16(a_lo, a_hi);
    _mm_store_si128((__m128i*)(b), a_1);
    _mm_store_si128((__m128i*)(b + 4), a_2);
}

static inline void hadamard_col4_sse2(__m128i* in, int iter) {
    const __m128i a0 = in[0];
    const __m128i a1 = in[1];
    const __m128i a2 = in[2];
    const __m128i a3 = in[3];
    const __m128i b0 = _mm_srai_epi16(_mm_add_epi16(a0, a1), 1);
    const __m128i b1 = _mm_srai_epi16(_mm_sub_epi16(a0, a1), 1);
    const __m128i b2 = _mm_srai_epi16(_mm_add_epi16(a2, a3), 1);
    const __m128i b3 = _mm_srai_epi16(_mm_sub_epi16(a2, a3), 1);
    in[0]            = _mm_add_epi16(b0, b2);
    in[1]            = _mm_add_epi16(b1, b3);
    in[2]            = _mm_sub_epi16(b0, b2);
    in[3]            = _mm_sub_epi16(b1, b3);

    if (iter == 0) {
        const __m128i ba      = _mm_unpacklo_epi16(in[0], in[1]);
        const __m128i dc      = _mm_unpacklo_epi16(in[2], in[3]);
        const __m128i dcba_lo = _mm_unpacklo_epi32(ba, dc);
        const __m128i dcba_hi = _mm_unpackhi_epi32(ba, dc);
        in[0]                 = dcba_lo;
        in[1]                 = _mm_srli_si128(dcba_lo, 8);
        in[2]                 = dcba_hi;
        in[3]                 = _mm_srli_si128(dcba_hi, 8);
    }
}

void svt_aom_hadamard_4x4_sse2(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff) {
    __m128i src[4];
    src[0] = _mm_loadl_epi64((const __m128i*)src_diff);
    src[1] = _mm_loadl_epi64((const __m128i*)(src_diff += src_stride));
    src[2] = _mm_loadl_epi64((const __m128i*)(src_diff += src_stride));
    src[3] = _mm_loadl_epi64((const __m128i*)(src_diff + src_stride));

    hadamard_col4_sse2(src, 0);
    hadamard_col4_sse2(src, 1);

    store_tran_low(_mm_unpacklo_epi64(src[0], src[1]), coeff);
    coeff += 8;
    store_tran_low(_mm_unpacklo_epi64(src[2], src[3]), coeff);
}

static INLINE void hadamard_col8_sse2(__m128i* in, int iter) {
    __m128i a0 = in[0];
    __m128i a1 = in[1];
    __m128i a2 = in[2];
    __m128i a3 = in[3];
    __m128i a4 = in[4];
    __m128i a5 = in[5];
    __m128i a6 = in[6];
    __m128i a7 = in[7];

    __m128i b0 = _mm_add_epi16(a0, a1);
    __m128i b1 = _mm_sub_epi16(a0, a1);
    __m128i b2 = _mm_add_epi16(a2, a3);
    __m128i b3 = _mm_sub_epi16(a2, a3);
    __m128i b4 = _mm_add_epi16(a4, a5);
    __m128i b5 = _mm_sub_epi16(a4, a5);
    __m128i b6 = _mm_add_epi16(a6, a7);
    __m128i b7 = _mm_sub_epi16(a6, a7);

    a0 = _mm_add_epi16(b0, b2);
    a1 = _mm_add_epi16(b1, b3);
    a2 = _mm_sub_epi16(b0, b2);
    a3 = _mm_sub_epi16(b1, b3);
    a4 = _mm_add_epi16(b4, b6);
    a5 = _mm_add_epi16(b5, b7);
    a6 = _mm_sub_epi16(b4, b6);
    a7 = _mm_sub_epi16(b5, b7);

    if (iter == 0) {
        b0 = _mm_add_epi16(a0, a4);
        b7 = _mm_add_epi16(a1, a5);
        b3 = _mm_add_epi16(a2, a6);
        b4 = _mm_add_epi16(a3, a7);
        b2 = _mm_sub_epi16(a0, a4);
        b6 = _mm_sub_epi16(a1, a5);
        b1 = _mm_sub_epi16(a2, a6);
        b5 = _mm_sub_epi16(a3, a7);

        a0 = _mm_unpacklo_epi16(b0, b1);
        a1 = _mm_unpacklo_epi16(b2, b3);
        a2 = _mm_unpackhi_epi16(b0, b1);
        a3 = _mm_unpackhi_epi16(b2, b3);
        a4 = _mm_unpacklo_epi16(b4, b5);
        a5 = _mm_unpacklo_epi16(b6, b7);
        a6 = _mm_unpackhi_epi16(b4, b5);
        a7 = _mm_unpackhi_epi16(b6, b7);

        b0 = _mm_unpacklo_epi32(a0, a1);
        b1 = _mm_unpacklo_epi32(a4, a5);
        b2 = _mm_unpackhi_epi32(a0, a1);
        b3 = _mm_unpackhi_epi32(a4, a5);
        b4 = _mm_unpacklo_epi32(a2, a3);
        b5 = _mm_unpacklo_epi32(a6, a7);
        b6 = _mm_unpackhi_epi32(a2, a3);
        b7 = _mm_unpackhi_epi32(a6, a7);

        in[0] = _mm_unpacklo_epi64(b0, b1);
        in[1] = _mm_unpackhi_epi64(b0, b1);
        in[2] = _mm_unpacklo_epi64(b2, b3);
        in[3] = _mm_unpackhi_epi64(b2, b3);
        in[4] = _mm_unpacklo_epi64(b4, b5);
        in[5] = _mm_unpackhi_epi64(b4, b5);
        in[6] = _mm_unpacklo_epi64(b6, b7);
        in[7] = _mm_unpackhi_epi64(b6, b7);
    } else {
        in[0] = _mm_add_epi16(a0, a4);
        in[7] = _mm_add_epi16(a1, a5);
        in[3] = _mm_add_epi16(a2, a6);
        in[4] = _mm_add_epi16(a3, a7);
        in[2] = _mm_sub_epi16(a0, a4);
        in[6] = _mm_sub_epi16(a1, a5);
        in[1] = _mm_sub_epi16(a2, a6);
        in[5] = _mm_sub_epi16(a3, a7);
    }
}

static INLINE void hadamard_8x8_sse2(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff, int is_final) {
    __m128i src[8];
    src[0] = _mm_load_si128((const __m128i*)src_diff);
    src[1] = _mm_load_si128((const __m128i*)(src_diff += src_stride));
    src[2] = _mm_load_si128((const __m128i*)(src_diff += src_stride));
    src[3] = _mm_load_si128((const __m128i*)(src_diff += src_stride));
    src[4] = _mm_load_si128((const __m128i*)(src_diff += src_stride));
    src[5] = _mm_load_si128((const __m128i*)(src_diff += src_stride));
    src[6] = _mm_load_si128((const __m128i*)(src_diff += src_stride));
    src[7] = _mm_load_si128((const __m128i*)(src_diff += src_stride));

    hadamard_col8_sse2(src, 0);
    hadamard_col8_sse2(src, 1);

    if (is_final) {
        store_tran_low(src[0], coeff);
        coeff += 8;
        store_tran_low(src[1], coeff);
        coeff += 8;
        store_tran_low(src[2], coeff);
        coeff += 8;
        store_tran_low(src[3], coeff);
        coeff += 8;
        store_tran_low(src[4], coeff);
        coeff += 8;
        store_tran_low(src[5], coeff);
        coeff += 8;
        store_tran_low(src[6], coeff);
        coeff += 8;
        store_tran_low(src[7], coeff);
    } else {
        int16_t* coeff16 = (int16_t*)coeff;
        _mm_store_si128((__m128i*)coeff16, src[0]);
        coeff16 += 8;
        _mm_store_si128((__m128i*)coeff16, src[1]);
        coeff16 += 8;
        _mm_store_si128((__m128i*)coeff16, src[2]);
        coeff16 += 8;
        _mm_store_si128((__m128i*)coeff16, src[3]);
        coeff16 += 8;
        _mm_store_si128((__m128i*)coeff16, src[4]);
        coeff16 += 8;
        _mm_store_si128((__m128i*)coeff16, src[5]);
        coeff16 += 8;
        _mm_store_si128((__m128i*)coeff16, src[6]);
        coeff16 += 8;
        _mm_store_si128((__m128i*)coeff16, src[7]);
    }
}

void svt_aom_hadamard_8x8_sse2(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff) {
    hadamard_8x8_sse2(src_diff, src_stride, coeff, 1);
}
#endif
