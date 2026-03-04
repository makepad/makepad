/*
 * Copyright (c) 2025, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#include <arm_neon.h>

#include "aom_dsp_rtcd.h"
#include "mem_neon.h"
#include "sum_neon.h"

static AOM_FORCE_INLINE uint32x4_t sad_8x4_dual_neon(const uint8_t* restrict src_ptr, uint32_t src_stride,
                                                     const uint8_t* restrict ref_ptr, uint32_t ref_stride, int mult) {
    uint32x4_t sum = vdupq_n_u32(0);

    int h = 4;
    do {
        uint8x16_t s0 = vld1q_u8(src_ptr);
        uint8x16_t s1 = vld1q_u8(src_ptr + src_stride);
        uint8x16_t r0 = vld1q_u8(ref_ptr);
        uint8x16_t r1 = vld1q_u8(ref_ptr + ref_stride);

        uint8x16_t abs_diff0 = vabdq_u8(s0, r0);
        uint8x16_t abs_diff1 = vabdq_u8(s1, r1);

        sum = vdotq_u32(sum, abs_diff0, vdupq_n_u8(mult));
        sum = vdotq_u32(sum, abs_diff1, vdupq_n_u8(mult));

        src_ptr += 2 * src_stride;
        ref_ptr += 2 * ref_stride;
        h -= 2;
    } while (h != 0);

    return sum;
}

static AOM_FORCE_INLINE uint32x4_t sad_8x8_dual_neon(const uint8_t* restrict src_ptr, uint32_t src_stride,
                                                     const uint8_t* restrict ref_ptr, uint32_t ref_stride, int mult) {
    uint32x4_t sum = vdupq_n_u32(0);

    int h = 8;
    do {
        uint8x16_t s = vld1q_u8(src_ptr);
        uint8x16_t r = vld1q_u8(ref_ptr);

        uint8x16_t abs_diff = vabdq_u8(s, r);

        sum = vdotq_u32(sum, abs_diff, vdupq_n_u8(mult));

        src_ptr += src_stride;
        ref_ptr += ref_stride;
    } while (--h != 0);

    return sum;
}
