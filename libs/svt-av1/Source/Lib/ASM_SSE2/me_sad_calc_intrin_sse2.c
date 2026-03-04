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
#include <emmintrin.h>
#include <stdint.h>

void svt_initialize_buffer_32bits_sse2_intrin(uint32_t* pointer, uint32_t count128, uint32_t count32, uint32_t value) {
    __m128i  xmm1, xmm2;
    uint32_t index128;
    xmm2 = _mm_cvtsi32_si128(value);
    xmm1 = _mm_or_si128(_mm_slli_si128(xmm2, 4), xmm2);
    xmm2 = _mm_or_si128(_mm_slli_si128(xmm1, 8), xmm1);

    for (index128 = 0; index128 < count128; ++index128) {
        _mm_storeu_si128((__m128i*)pointer, xmm2);
        pointer += 4;
    }
    if (count32 == 3) { //Initialize 96 bits
        _mm_storel_epi64((__m128i*)(pointer), xmm2);
        *(pointer + 2) = _mm_cvtsi128_si32(xmm2);
    } else if (count32 == 2) { // Initialize 64 bits
        _mm_storel_epi64((__m128i*)pointer, xmm2);
    } else if (count32 == 1) { // Initialize 32 bits
        *(pointer) = _mm_cvtsi128_si32(xmm2);
    }
}
