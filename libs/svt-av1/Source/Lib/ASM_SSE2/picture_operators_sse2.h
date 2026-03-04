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

#ifndef EbPictureOperators_SSE2_h
#define EbPictureOperators_SSE2_h

#include <emmintrin.h>
#include "definitions.h"

#ifdef __cplusplus
extern "C" {
#endif

static INLINE int32_t hadd32_sse2_intrin(const __m128i src) {
    const __m128i dst0 = _mm_add_epi32(src, _mm_srli_si128(src, 8));
    const __m128i dst1 = _mm_add_epi32(dst0, _mm_srli_si128(dst0, 4));

    return _mm_cvtsi128_si32(dst1);
}

#ifdef __cplusplus
}
#endif
#endif // EbPictureOperators_SSE2_h
