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

#ifndef AOM_DSP_X86_TRANSPOSE_AVX2_H_
#define AOM_DSP_X86_TRANSPOSE_AVX2_H_

#include <immintrin.h> // AVX2
#include "definitions.h"
#include "synonyms_avx2.h"

static INLINE void transpose_32bit_8x8_avx2(const __m256i* const in, __m256i* const out) {
    // Unpack 32 bit elements. Goes from:
    // in[0]: 00 01 02 03  04 05 06 07
    // in[1]: 10 11 12 13  14 15 16 17
    // in[2]: 20 21 22 23  24 25 26 27
    // in[3]: 30 31 32 33  34 35 36 37
    // in[4]: 40 41 42 43  44 45 46 47
    // in[5]: 50 51 52 53  54 55 56 57
    // in[6]: 60 61 62 63  64 65 66 67
    // in[7]: 70 71 72 73  74 75 76 77
    // to:
    // a0:    00 10 01 11  04 14 05 15
    // a1:    20 30 21 31  24 34 25 35
    // a2:    40 50 41 51  44 54 45 55
    // a3:    60 70 61 71  64 74 65 75
    // a4:    02 12 03 13  06 16 07 17
    // a5:    22 32 23 33  26 36 27 37
    // a6:    42 52 43 53  46 56 47 57
    // a7:    62 72 63 73  66 76 67 77
    const __m256i a0 = _mm256_unpacklo_epi32(in[0], in[1]);
    const __m256i a1 = _mm256_unpacklo_epi32(in[2], in[3]);
    const __m256i a2 = _mm256_unpacklo_epi32(in[4], in[5]);
    const __m256i a3 = _mm256_unpacklo_epi32(in[6], in[7]);
    const __m256i a4 = _mm256_unpackhi_epi32(in[0], in[1]);
    const __m256i a5 = _mm256_unpackhi_epi32(in[2], in[3]);
    const __m256i a6 = _mm256_unpackhi_epi32(in[4], in[5]);
    const __m256i a7 = _mm256_unpackhi_epi32(in[6], in[7]);

    // Unpack 64 bit elements resulting in:
    // b0: 00 10 20 30  04 14 24 34
    // b1: 40 50 60 70  44 54 64 74
    // b2: 01 11 21 31  05 15 25 35
    // b3: 41 51 61 71  45 55 65 75
    // b4: 02 12 22 32  06 16 26 36
    // b5: 42 52 62 72  46 56 66 76
    // b6: 03 13 23 33  07 17 27 37
    // b7: 43 53 63 73  47 57 67 77
    const __m256i b0 = _mm256_unpacklo_epi64(a0, a1);
    const __m256i b1 = _mm256_unpacklo_epi64(a2, a3);
    const __m256i b2 = _mm256_unpackhi_epi64(a0, a1);
    const __m256i b3 = _mm256_unpackhi_epi64(a2, a3);
    const __m256i b4 = _mm256_unpacklo_epi64(a4, a5);
    const __m256i b5 = _mm256_unpacklo_epi64(a6, a7);
    const __m256i b6 = _mm256_unpackhi_epi64(a4, a5);
    const __m256i b7 = _mm256_unpackhi_epi64(a6, a7);

    // Unpack 128 bit elements resulting in:
    // out[0]: 00 10 20 30  40 50 60 70
    // out[1]: 01 11 21 31  41 51 61 71
    // out[2]: 02 12 22 32  42 52 62 72
    // out[3]: 03 13 23 33  43 53 63 73
    // out[4]: 04 14 24 34  44 54 64 74
    // out[5]: 05 15 25 35  45 55 65 75
    // out[6]: 06 16 26 36  46 56 66 76
    // out[7]: 07 17 27 37  47 57 67 77
    out[0] = yy_unpacklo_epi128(b0, b1);
    out[1] = yy_unpacklo_epi128(b2, b3);
    out[2] = yy_unpacklo_epi128(b4, b5);
    out[3] = yy_unpacklo_epi128(b6, b7);
    out[4] = yy_unpackhi_epi128(b0, b1);
    out[5] = yy_unpackhi_epi128(b2, b3);
    out[6] = yy_unpackhi_epi128(b4, b5);
    out[7] = yy_unpackhi_epi128(b6, b7);
}

static INLINE void transpose_64bit_4x4_avx2(const __m256i* const in, __m256i* const out) {
    // Unpack 32 bit elements. Goes from:
    // in[0]: 00 01 02 03
    // in[1]: 10 11 12 13
    // in[2]: 20 21 22 23
    // in[3]: 30 31 32 33
    // to:
    // a0:    00 10 02 12
    // a1:    20 30 22 32
    // a2:    01 11 03 13
    // a3:    21 31 23 33
    const __m256i a0 = _mm256_unpacklo_epi64(in[0], in[1]);
    const __m256i a1 = _mm256_unpacklo_epi64(in[2], in[3]);
    const __m256i a2 = _mm256_unpackhi_epi64(in[0], in[1]);
    const __m256i a3 = _mm256_unpackhi_epi64(in[2], in[3]);

    // Unpack 64 bit elements resulting in:
    // out[0]: 00 10 20 30
    // out[1]: 01 11 21 31
    // out[2]: 02 12 22 32
    // out[3]: 03 13 23 33
    out[0] = yy_unpacklo_epi128(a0, a1);
    out[1] = yy_unpacklo_epi128(a2, a3);
    out[2] = yy_unpackhi_epi128(a0, a1);
    out[3] = yy_unpackhi_epi128(a2, a3);
}

static INLINE void transpose_64bit_4x6_avx2(const __m256i* const in, __m256i* const out) {
    // Unpack 64 bit elements. Goes from:
    // in[0]: 00 01  02 03
    // in[1]: 10 11  12 13
    // in[2]: 20 21  22 23
    // in[3]: 30 31  32 33
    // in[4]: 40 41  42 43
    // in[5]: 50 51  52 53
    // to:
    // a0:    00 10  02 12
    // a1:    20 30  22 32
    // a2:    40 50  42 52
    // a4:    01 11  03 13
    // a5:    21 31  23 33
    // a6:    41 51  43 53
    const __m256i a0 = _mm256_unpacklo_epi64(in[0], in[1]);
    const __m256i a1 = _mm256_unpacklo_epi64(in[2], in[3]);
    const __m256i a2 = _mm256_unpacklo_epi64(in[4], in[5]);
    const __m256i a4 = _mm256_unpackhi_epi64(in[0], in[1]);
    const __m256i a5 = _mm256_unpackhi_epi64(in[2], in[3]);
    const __m256i a6 = _mm256_unpackhi_epi64(in[4], in[5]);

    // Unpack 128 bit elements resulting in:
    // b0: 00 10  20 30
    // b1: 40 50  40 50
    // b2: 01 11  21 31
    // b3: 41 51  41 51
    // b4: 02 12  22 32
    // b5: 42 52  42 52
    // b6: 03 13  23 33
    // b7: 43 53  43 53
    out[0] = yy_unpacklo_epi128(a0, a1);
    out[1] = yy_unpacklo_epi128(a2, a2);
    out[2] = yy_unpacklo_epi128(a4, a5);
    out[3] = yy_unpacklo_epi128(a6, a6);
    out[4] = yy_unpackhi_epi128(a0, a1);
    out[5] = yy_unpackhi_epi128(a2, a2);
    out[6] = yy_unpackhi_epi128(a4, a5);
    out[7] = yy_unpackhi_epi128(a6, a6);
}

static INLINE void transpose_64bit_4x8_avx2(const __m256i* const in, __m256i* const out) {
    // Unpack 64 bit elements. Goes from:
    // in[0]: 00 01  02 03
    // in[1]: 10 11  12 13
    // in[2]: 20 21  22 23
    // in[3]: 30 31  32 33
    // in[4]: 40 41  42 43
    // in[5]: 50 51  52 53
    // in[6]: 60 61  62 63
    // in[7]: 70 71  72 73
    // to:
    // a0:    00 10  02 12
    // a1:    20 30  22 32
    // a2:    40 50  42 52
    // a3:    60 70  62 72
    // a4:    01 11  03 13
    // a5:    21 31  23 33
    // a6:    41 51  43 53
    // a7:    61 71  63 73
    const __m256i a0 = _mm256_unpacklo_epi64(in[0], in[1]);
    const __m256i a1 = _mm256_unpacklo_epi64(in[2], in[3]);
    const __m256i a2 = _mm256_unpacklo_epi64(in[4], in[5]);
    const __m256i a3 = _mm256_unpacklo_epi64(in[6], in[7]);
    const __m256i a4 = _mm256_unpackhi_epi64(in[0], in[1]);
    const __m256i a5 = _mm256_unpackhi_epi64(in[2], in[3]);
    const __m256i a6 = _mm256_unpackhi_epi64(in[4], in[5]);
    const __m256i a7 = _mm256_unpackhi_epi64(in[6], in[7]);

    // Unpack 128 bit elements resulting in:
    // b0: 00 10  20 30
    // b1: 40 50  60 70
    // b2: 01 11  21 31
    // b3: 41 51  61 71
    // b4: 02 12  22 32
    // b5: 42 52  62 72
    // b6: 03 13  23 33
    // b7: 43 53  63 73
    out[0] = yy_unpacklo_epi128(a0, a1);
    out[1] = yy_unpacklo_epi128(a2, a3);
    out[2] = yy_unpacklo_epi128(a4, a5);
    out[3] = yy_unpacklo_epi128(a6, a7);
    out[4] = yy_unpackhi_epi128(a0, a1);
    out[5] = yy_unpackhi_epi128(a2, a3);
    out[6] = yy_unpackhi_epi128(a4, a5);
    out[7] = yy_unpackhi_epi128(a6, a7);
}

#endif // AOM_DSP_X86_TRANSPOSE_AVX2_H_
