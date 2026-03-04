/*
* Copyright (c) 2023, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at www.aomedia.org/license/software. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at www.aomedia.org/license/patent.
*/

#ifndef AOM_AOM_DSP_ARM_SUM_NEON_H_
#define AOM_AOM_DSP_ARM_SUM_NEON_H_

#include <arm_neon.h>

static inline uint32x4_t horizontal_add_4d_u32x4(const uint32x4_t sum[4]) {
    uint32x4_t res01 = vpaddq_u32(sum[0], sum[1]);
    uint32x4_t res23 = vpaddq_u32(sum[2], sum[3]);
    return vpaddq_u32(res01, res23);
}

static inline int32x4_t horizontal_add_4d_s32x4(const int32x4_t sum[4]) {
    int32x4_t res01 = vpaddq_s32(sum[0], sum[1]);
    int32x4_t res23 = vpaddq_s32(sum[2], sum[3]);
    return vpaddq_s32(res01, res23);
}

static inline uint32_t horizontal_long_add_u16x8(const uint16x8_t vec_lo, const uint16x8_t vec_hi) {
    return vaddlvq_u16(vec_lo) + vaddlvq_u16(vec_hi);
}

static inline uint32x4_t horizontal_long_add_4d_u16x8(const uint16x8_t sum_lo[4], const uint16x8_t sum_hi[4]) {
    const uint32x4_t a0 = vpaddlq_u16(sum_lo[0]);
    const uint32x4_t a1 = vpaddlq_u16(sum_lo[1]);
    const uint32x4_t a2 = vpaddlq_u16(sum_lo[2]);
    const uint32x4_t a3 = vpaddlq_u16(sum_lo[3]);
    const uint32x4_t b0 = vpadalq_u16(a0, sum_hi[0]);
    const uint32x4_t b1 = vpadalq_u16(a1, sum_hi[1]);
    const uint32x4_t b2 = vpadalq_u16(a2, sum_hi[2]);
    const uint32x4_t b3 = vpadalq_u16(a3, sum_hi[3]);
    const uint32x4_t c0 = vpaddq_u32(b0, b1);
    const uint32x4_t c1 = vpaddq_u32(b2, b3);
    return vpaddq_u32(c0, c1);
}

static inline uint32x4_t horizontal_add_4d_u16x8(const uint16x8_t sum[4]) {
    const uint16x8_t a0 = vpaddq_u16(sum[0], sum[1]);
    const uint16x8_t a1 = vpaddq_u16(sum[2], sum[3]);
    const uint16x8_t b0 = vpaddq_u16(a0, a1);
    return vpaddlq_u16(b0);
}

static inline int32x4_t horizontal_add_4d_s16x8(const int16x8_t sum[4]) {
    const int16x8_t a0 = vpaddq_s16(sum[0], sum[1]);
    const int16x8_t a1 = vpaddq_s16(sum[2], sum[3]);
    const int16x8_t b0 = vpaddq_s16(a0, a1);
    return vpaddlq_s16(b0);
}

static inline uint64_t horizontal_long_add_u32x4_x2(const uint32x4_t a[2]) {
    return vaddlvq_u32(a[0]) + vaddlvq_u32(a[1]);
}

static inline uint64_t horizontal_long_add_u32x4_x4(const uint32x4_t a[4]) {
    uint64x2_t sum = vpaddlq_u32(a[0]);
    sum            = vpadalq_u32(sum, a[1]);
    sum            = vpadalq_u32(sum, a[2]);
    sum            = vpadalq_u32(sum, a[3]);

    return vaddvq_u64(sum);
}

static inline uint64_t horizontal_long_add_u32x4_x8(const uint32x4_t a[8]) {
    uint64x2_t sum[2];
    sum[0] = vpaddlq_u32(a[0]);
    sum[1] = vpaddlq_u32(a[1]);
    sum[0] = vpadalq_u32(sum[0], a[2]);
    sum[1] = vpadalq_u32(sum[1], a[3]);
    sum[0] = vpadalq_u32(sum[0], a[4]);
    sum[1] = vpadalq_u32(sum[1], a[5]);
    sum[0] = vpadalq_u32(sum[0], a[6]);
    sum[1] = vpadalq_u32(sum[1], a[7]);

    return vaddvq_u64(vaddq_u64(sum[0], sum[1]));
}

#endif // AOM_AOM_DSP_ARM_SUM_NEON_H_
