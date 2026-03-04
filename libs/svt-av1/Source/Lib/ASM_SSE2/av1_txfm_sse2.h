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
#ifndef AV1_COMMON_X86_AV1_TXFM_SSE2_H_
#define AV1_COMMON_X86_AV1_TXFM_SSE2_H_

#include <emmintrin.h> // SSE2

#ifdef __cplusplus
extern "C" {
#endif

#define pair_set_epi16(a, b) _mm_set1_epi32((int32_t)(((uint16_t)(a)) | (((uint32_t)(b)) << 16)))

#define btf_16_4p_sse2(w0, w1, in0, in1, out0, out1, __rounding) \
    {                                                            \
        __m128i t0 = _mm_unpacklo_epi16(in0, in1);               \
        __m128i u0 = _mm_madd_epi16(t0, w0);                     \
        __m128i v0 = _mm_madd_epi16(t0, w1);                     \
                                                                 \
        __m128i a0 = _mm_add_epi32(u0, __rounding);              \
        __m128i b0 = _mm_add_epi32(v0, __rounding);              \
                                                                 \
        __m128i c0 = _mm_srai_epi32(a0, cos_bit);                \
        __m128i d0 = _mm_srai_epi32(b0, cos_bit);                \
                                                                 \
        out0 = _mm_packs_epi32(c0, c0);                          \
        out1 = _mm_packs_epi32(d0, d0);                          \
    }

#define btf_16_sse2(w0, w1, in0, in1, out0, out1, __rounding) \
    {                                                         \
        __m128i t0 = _mm_unpacklo_epi16(in0, in1);            \
        __m128i t1 = _mm_unpackhi_epi16(in0, in1);            \
        __m128i u0 = _mm_madd_epi16(t0, w0);                  \
        __m128i u1 = _mm_madd_epi16(t1, w0);                  \
        __m128i v0 = _mm_madd_epi16(t0, w1);                  \
        __m128i v1 = _mm_madd_epi16(t1, w1);                  \
                                                              \
        __m128i a0 = _mm_add_epi32(u0, __rounding);           \
        __m128i a1 = _mm_add_epi32(u1, __rounding);           \
        __m128i b0 = _mm_add_epi32(v0, __rounding);           \
        __m128i b1 = _mm_add_epi32(v1, __rounding);           \
                                                              \
        __m128i c0 = _mm_srai_epi32(a0, cos_bit);             \
        __m128i c1 = _mm_srai_epi32(a1, cos_bit);             \
        __m128i d0 = _mm_srai_epi32(b0, cos_bit);             \
        __m128i d1 = _mm_srai_epi32(b1, cos_bit);             \
                                                              \
        out0 = _mm_packs_epi32(c0, c1);                       \
        out1 = _mm_packs_epi32(d0, d1);                       \
    }

static INLINE __m128i load_32bit_to_16bit(const int32_t* a) {
    const __m128i a_low = _mm_loadu_si128((const __m128i*)a);
    return _mm_packs_epi32(a_low, *(const __m128i*)(a + 4));
}

static INLINE __m128i load_32bit_to_16bit_w4(const int32_t* a) {
    const __m128i a_low = _mm_loadu_si128((const __m128i*)a);
    return _mm_packs_epi32(a_low, a_low);
}

static INLINE void load_buffer_32bit_to_16bit(const int32_t* in, int32_t stride, __m128i* out, int32_t out_size) {
    for (int32_t i = 0; i < out_size; ++i) {
        out[i] = load_32bit_to_16bit(in + i * stride);
    }
}

static INLINE void load_buffer_32bit_to_16bit_w4(const int32_t* in, int32_t stride, __m128i* out, int32_t out_size) {
    for (int32_t i = 0; i < out_size; ++i) {
        out[i] = load_32bit_to_16bit_w4(in + i * stride);
    }
}

static INLINE void flip_buf_sse2(__m128i* in, __m128i* out, int32_t size) {
    for (int32_t i = 0; i < size; ++i) {
        out[size - i - 1] = in[i];
    }
}

#ifdef __cplusplus
}
#endif // __cplusplus
#endif // AV1_COMMON_X86_AV1_TXFM_SSE2_H_
