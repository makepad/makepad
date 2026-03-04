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

#ifndef COMPUTE_SAD_SVE_H
#define COMPUTE_SAD_SVE_H

#include <arm_neon.h>

#include "aom_dsp_rtcd.h"
#include "common_dsp_rtcd.h"
#include "compute_sad_c.h"
#include "neon_sve_bridge.h"
#include "utility.h"

static inline uint32_t sad_anywxh_sve(const uint8_t* src, uint32_t src_stride, const uint8_t* ref, uint32_t ref_stride,
                                      uint32_t width, uint32_t height) {
    uint32x4_t sum_u32 = vdupq_n_u32(0);

    do {
        int w = width;

        const uint8_t* src_ptr = src;
        const uint8_t* ref_ptr = ref;

        while (w >= 16) {
            const uint8x16_t s = vld1q_u8(src_ptr);
            sad16_neon_dotprod(s, vld1q_u8(ref_ptr), &sum_u32);

            src_ptr += 16;
            ref_ptr += 16;
            w -= 16;
        }

        const svbool_t   p  = svwhilelt_b8_s32(0, width & 15);
        const uint8x16_t s  = svget_neonq_u8(svld1_u8(p, src_ptr));
        const uint8x16_t r0 = svget_neonq_u8(svld1_u8(p, ref_ptr + 0));

        sad16_neon_dotprod(s, r0, &sum_u32);

        src += src_stride;
        ref += ref_stride;
    } while (--height != 0);

    return vaddvq_u32(sum_u32);
}

#endif // COMPUTE_SAD_SVE_H
