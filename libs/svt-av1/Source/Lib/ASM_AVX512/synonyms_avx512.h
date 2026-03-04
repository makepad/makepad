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

#ifndef AOM_DSP_X86_SYNONYMS_AVX512_H_
#define AOM_DSP_X86_SYNONYMS_AVX512_H_

#include <immintrin.h>

#if EN_AVX512_SUPPORT

/**
  * Various reusable shorthands for x86 SIMD intrinsics.
  *
  * Intrinsics prefixed with zz_ operate on or return 512bit ZMM registers.
  */

// Loads and stores to do away with the tedium of casting the address
// to the right type.
static INLINE __m512i zz_load_512(const void* const a) {
#ifdef EB_TEST_SIMD_ALIGN
    if ((intptr_t)a % 64) {
        SVT_LOG("\n zz_load_512() NOT 64-byte aligned!!!\n");
    }
#endif
    return _mm512_load_si512((const __m512i*)a);
}

static INLINE __m512i zz_loadu_512(const void* const a) {
    return _mm512_loadu_si512((const __m512i*)a);
}

static INLINE void zz_store_512(void* const a, const __m512i v) {
#ifdef EB_TEST_SIMD_ALIGN
    if ((intptr_t)a % 64) {
        SVT_LOG("\n zz_store_512() NOT 64-byte aligned!!!\n");
    }
#endif
    _mm512_store_si512((__m512i*)a, v);
}

static INLINE void zz_storeu_512(void* const a, const __m512i v) {
    _mm512_storeu_si512((__m512i*)a, v);
}

#endif // EN_AVX512_SUPPORT

#endif // AOM_DSP_X86_SYNONYMS_AVX512_H_
