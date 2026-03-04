/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#ifndef _HIGHBD_TXFM_UTILITY_NEON_H
#define _HIGHBD_TXFM_UTILITY_NEON_H

#include <arm_neon.h>

static inline int32x4_t half_btf_neon(const int cospi0, const int32x4_t n0, const int cospi1, const int32x4_t n1,
                                      int32_t bit) {
    int32x4_t res = vmulq_n_s32(n0, cospi0);
    res           = vmlaq_n_s32(res, n1, cospi1);
    return vrshlq_s32(res, vdupq_n_s32(-bit));
}

static inline int32x4_t half_btf_0_neon(const int cospi0, const int32x4_t n0, int32_t bit) {
    int32x4_t x = vmulq_n_s32(n0, cospi0);
    return vrshlq_s32(x, vdupq_n_s32(-bit));
}

#endif // _HIGHBD_TXFM_UTILITY_NEON_H
