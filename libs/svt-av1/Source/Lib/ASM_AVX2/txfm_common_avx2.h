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
#ifndef AV1_COMMON_X86_AV1_TXFM_COMMON_AVX2_H_
#define AV1_COMMON_X86_AV1_TXFM_COMMON_AVX2_H_

#include <immintrin.h>
#include <stdint.h>
#include "inv_transforms.h"
#include "synonyms_avx2.h"

#ifdef __cplusplus
extern "C" {
#endif

#define pair_set_epi16(a, b) _mm_set1_epi32((int32_t)(((uint16_t)(a)) | (((uint32_t)(b)) << 16)))

// Reverse the 8 16 bit words in __m128i
static INLINE __m128i mm_reverse_epi16(const __m128i x) {
    const __m128i a = _mm_shufflelo_epi16(x, 0x1b);
    const __m128i b = _mm_shufflehi_epi16(a, 0x1b);
    return _mm_shuffle_epi32(b, 0x4e);
}

static INLINE __m256i pair_set_w16_epi16(int16_t a, int16_t b) {
    return _mm256_set1_epi32((int32_t)(((uint16_t)(a)) | (((uint32_t)(b)) << 16)));
}

static INLINE void btf_16_w16_avx2(const __m256i w0, const __m256i w1, __m256i* in0, __m256i* in1, const __m256i _r,
                                   const int32_t cos_bit) {
    __m256i t0 = _mm256_unpacklo_epi16(*in0, *in1);
    __m256i t1 = _mm256_unpackhi_epi16(*in0, *in1);
    __m256i u0 = _mm256_madd_epi16(t0, w0);
    __m256i u1 = _mm256_madd_epi16(t1, w0);
    __m256i v0 = _mm256_madd_epi16(t0, w1);
    __m256i v1 = _mm256_madd_epi16(t1, w1);

    __m256i a0 = _mm256_add_epi32(u0, _r);
    __m256i a1 = _mm256_add_epi32(u1, _r);
    __m256i b0 = _mm256_add_epi32(v0, _r);
    __m256i b1 = _mm256_add_epi32(v1, _r);

    __m256i c0 = _mm256_srai_epi32(a0, cos_bit);
    __m256i c1 = _mm256_srai_epi32(a1, cos_bit);
    __m256i d0 = _mm256_srai_epi32(b0, cos_bit);
    __m256i d1 = _mm256_srai_epi32(b1, cos_bit);

    *in0 = _mm256_packs_epi32(c0, c1);
    *in1 = _mm256_packs_epi32(d0, d1);
}

static INLINE void btf_16_adds_subs_avx2(__m256i* in0, __m256i* in1) {
    const __m256i _in0 = *in0;
    const __m256i _in1 = *in1;
    *in0               = _mm256_adds_epi16(_in0, _in1);
    *in1               = _mm256_subs_epi16(_in0, _in1);
}

static INLINE void btf_16_adds_subs_out_avx2(__m256i* out0, __m256i* out1, __m256i in0, __m256i in1) {
    const __m256i _in0 = in0;
    const __m256i _in1 = in1;
    *out0              = _mm256_adds_epi16(_in0, _in1);
    *out1              = _mm256_subs_epi16(_in0, _in1);
}

static INLINE __m256i load_32bit_to_16bit_w16_avx2(const int32_t* a) {
    const __m256i a_low  = _mm256_lddqu_si256((const __m256i*)a);
    const __m256i a_high = _mm256_lddqu_si256((const __m256i*)(a + 8));
    const __m256i b      = _mm256_packs_epi32(a_low, a_high);
    return _mm256_permute4x64_epi64(b, 0xD8);
}

static INLINE void load_buffer_32bit_to_16bit_w16_avx2(const int32_t* in, int stride, __m256i* out, int out_size) {
    for (int i = 0; i < out_size; ++i) {
        out[i] = load_32bit_to_16bit_w16_avx2(in + i * stride);
    }
}

static INLINE void transpose_16bit_16x16_avx2(const __m256i* const in, __m256i* const out) {
    // Unpack 16 bit elements. Goes from:
    // in[0]: 00 01 02 03  08 09 0a 0b  04 05 06 07  0c 0d 0e 0f
    // in[1]: 10 11 12 13  18 19 1a 1b  14 15 16 17  1c 1d 1e 1f
    // in[2]: 20 21 22 23  28 29 2a 2b  24 25 26 27  2c 2d 2e 2f
    // in[3]: 30 31 32 33  38 39 3a 3b  34 35 36 37  3c 3d 3e 3f
    // in[4]: 40 41 42 43  48 49 4a 4b  44 45 46 47  4c 4d 4e 4f
    // in[5]: 50 51 52 53  58 59 5a 5b  54 55 56 57  5c 5d 5e 5f
    // in[6]: 60 61 62 63  68 69 6a 6b  64 65 66 67  6c 6d 6e 6f
    // in[7]: 70 71 72 73  78 79 7a 7b  74 75 76 77  7c 7d 7e 7f
    // in[8]: 80 81 82 83  88 89 8a 8b  84 85 86 87  8c 8d 8e 8f
    // to:
    // a0:    00 10 01 11  02 12 03 13  04 14 05 15  06 16 07 17
    // a1:    20 30 21 31  22 32 23 33  24 34 25 35  26 36 27 37
    // a2:    40 50 41 51  42 52 43 53  44 54 45 55  46 56 47 57
    // a3:    60 70 61 71  62 72 63 73  64 74 65 75  66 76 67 77
    // ...
    __m256i a[16];
    for (int i = 0; i < 16; i += 2) {
        a[i / 2 + 0] = _mm256_unpacklo_epi16(in[i], in[i + 1]);
        a[i / 2 + 8] = _mm256_unpackhi_epi16(in[i], in[i + 1]);
    }
    __m256i b[16];
    for (int i = 0; i < 16; i += 2) {
        b[i / 2 + 0] = _mm256_unpacklo_epi32(a[i], a[i + 1]);
        b[i / 2 + 8] = _mm256_unpackhi_epi32(a[i], a[i + 1]);
    }
    __m256i c[16];
    for (int i = 0; i < 16; i += 2) {
        c[i / 2 + 0] = _mm256_unpacklo_epi64(b[i], b[i + 1]);
        c[i / 2 + 8] = _mm256_unpackhi_epi64(b[i], b[i + 1]);
    }
    out[0 + 0] = yy_unpacklo_epi128(c[0], c[1]);
    out[1 + 0] = yy_unpacklo_epi128(c[8], c[9]);
    out[2 + 0] = yy_unpacklo_epi128(c[4], c[5]);
    out[3 + 0] = yy_unpacklo_epi128(c[12], c[13]);

    out[0 + 8] = yy_unpackhi_epi128(c[0], c[1]);
    out[1 + 8] = yy_unpackhi_epi128(c[8], c[9]);
    out[2 + 8] = yy_unpackhi_epi128(c[4], c[5]);
    out[3 + 8] = yy_unpackhi_epi128(c[12], c[13]);

    out[4 + 0] = yy_unpacklo_epi128(c[0 + 2], c[1 + 2]);
    out[5 + 0] = yy_unpacklo_epi128(c[8 + 2], c[9 + 2]);
    out[6 + 0] = yy_unpacklo_epi128(c[4 + 2], c[5 + 2]);
    out[7 + 0] = yy_unpacklo_epi128(c[12 + 2], c[13 + 2]);

    out[4 + 8] = yy_unpackhi_epi128(c[0 + 2], c[1 + 2]);
    out[5 + 8] = yy_unpackhi_epi128(c[8 + 2], c[9 + 2]);
    out[6 + 8] = yy_unpackhi_epi128(c[4 + 2], c[5 + 2]);
    out[7 + 8] = yy_unpackhi_epi128(c[12 + 2], c[13 + 2]);
}

static INLINE void flip_buf_avx2(__m256i* in, __m256i* out, int size) {
    for (int i = 0; i < size; ++i) {
        out[size - i - 1] = in[i];
    }
}

static INLINE void round_shift_16bit_w16_avx2(__m256i* in, int size, int bit) {
    if (bit < 0) {
        bit           = -bit;
        __m256i round = _mm256_set1_epi16(1 << (bit - 1));
        for (int i = 0; i < size; ++i) {
            in[i] = _mm256_adds_epi16(in[i], round);
            in[i] = _mm256_srai_epi16(in[i], bit);
        }
    } else if (bit > 0) {
        for (int i = 0; i < size; ++i) {
            in[i] = _mm256_slli_epi16(in[i], bit);
        }
    }
}

static INLINE void av1_round_shift_array_32_avx2(__m256i* input, __m256i* output, const int32_t size,
                                                 const int32_t bit) {
    int32_t i;
    if (bit > 0) {
        const __m256i round = _mm256_set1_epi32(1 << (bit - 1));
        __m256i       r0;
        for (i = 0; i < size; i++) {
            r0        = _mm256_add_epi32(input[i], round);
            output[i] = _mm256_srai_epi32(r0, bit);
        }
    } else {
        for (i = 0; i < size; i++) {
            output[i] = _mm256_slli_epi32(input[i], -bit);
        }
    }
}

static INLINE void av1_round_shift_rect_array_32_avx2(__m256i* input, __m256i* output, const int32_t size,
                                                      const int32_t bit, const int32_t val) {
    const __m256i sqrt2  = _mm256_set1_epi32(val);
    const __m256i round2 = _mm256_set1_epi32(1 << (new_sqrt2_bits - 1));
    int32_t       i;
    if (bit > 0) {
        const __m256i round1 = _mm256_set1_epi32(1 << (bit - 1));
        __m256i       r0, r1, r2, r3;
        for (i = 0; i < size; i++) {
            r0        = _mm256_add_epi32(input[i], round1);
            r1        = _mm256_srai_epi32(r0, bit);
            r2        = _mm256_mullo_epi32(sqrt2, r1);
            r3        = _mm256_add_epi32(r2, round2);
            output[i] = _mm256_srai_epi32(r3, new_sqrt2_bits);
        }
    } else {
        __m256i r0, r1, r2;
        for (i = 0; i < size; i++) {
            r0        = _mm256_slli_epi32(input[i], -bit);
            r1        = _mm256_mullo_epi32(sqrt2, r0);
            r2        = _mm256_add_epi32(r1, round2);
            output[i] = _mm256_srai_epi32(r2, new_sqrt2_bits);
        }
    }
}

#ifdef __cplusplus
}
#endif

#endif // AV1_COMMON_X86_AV1_TXFM_COMMON_AVX2_H_
