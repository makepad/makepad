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

#include <arm_neon.h>

#include "aom_dsp_rtcd.h"
#include "mem_neon.h"

uint32_t svt_aom_highbd_mse16x16_neon(const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr, int ref_stride) {
    uint16_t* src = CONVERT_TO_SHORTPTR(src_ptr);
    uint16_t* ref = CONVERT_TO_SHORTPTR(ref_ptr);

    uint32x4_t sse_u32[2] = {vdupq_n_u32(0), vdupq_n_u32(0)};

    int i = 16;
    do {
        int j = 0;
        do {
            uint16x8_t s = vld1q_u16(src + j);
            uint16x8_t r = vld1q_u16(ref + j);

            uint16x8_t diff = vabdq_u16(s, r);

            sse_u32[0] = vmlal_u16(sse_u32[0], vget_low_u16(diff), vget_low_u16(diff));
            sse_u32[1] = vmlal_u16(sse_u32[1], vget_high_u16(diff), vget_high_u16(diff));

            j += 8;
        } while (j < 16);

        src += src_stride;
        ref += ref_stride;
    } while (--i != 0);

    return vaddvq_u32(vaddq_u32(sse_u32[0], sse_u32[1]));
}

static inline void highbd_variance_4xh_neon(const uint16_t* src_ptr, int src_stride, const uint16_t* ref_ptr,
                                            int ref_stride, int h, uint64_t* sse, int64_t* sum) {
    int16x8_t sum_s16 = vdupq_n_s16(0);
    int32x4_t sse_s32 = vdupq_n_s32(0);

    int i = h;
    do {
        const uint16x8_t s = load_u16_4x2(src_ptr, src_stride);
        const uint16x8_t r = load_u16_4x2(ref_ptr, ref_stride);

        int16x8_t diff = vreinterpretq_s16_u16(vsubq_u16(s, r));
        sum_s16        = vaddq_s16(sum_s16, diff);

        sse_s32 = vmlal_s16(sse_s32, vget_low_s16(diff), vget_low_s16(diff));
        sse_s32 = vmlal_s16(sse_s32, vget_high_s16(diff), vget_high_s16(diff));

        src_ptr += 2 * src_stride;
        ref_ptr += 2 * ref_stride;
        i -= 2;
    } while (i != 0);

    *sum = vaddlvq_s16(sum_s16);
    *sse = vaddvq_s32(sse_s32);
}

static inline void highbd_variance_large_neon(const uint16_t* src_ptr, int src_stride, const uint16_t* ref_ptr,
                                              int ref_stride, int w, int h, uint64_t* sse, int64_t* sum) {
    int32x4_t sum_s32    = vdupq_n_s32(0);
    int32x4_t sse_s32[2] = {vdupq_n_s32(0), vdupq_n_s32(0)};

    int i = h;
    do {
        int j = 0;
        do {
            const uint16x8_t s = vld1q_u16(src_ptr + j);
            const uint16x8_t r = vld1q_u16(ref_ptr + j);

            const int16x8_t diff = vreinterpretq_s16_u16(vsubq_u16(s, r));
            sum_s32              = vpadalq_s16(sum_s32, diff);

            sse_s32[0] = vmlal_s16(sse_s32[0], vget_low_s16(diff), vget_low_s16(diff));
            sse_s32[1] = vmlal_s16(sse_s32[1], vget_high_s16(diff), vget_high_s16(diff));

            j += 8;
        } while (j < w);

        src_ptr += src_stride;
        ref_ptr += ref_stride;
    } while (--i != 0);

    *sum = vaddvq_s32(sum_s32);
    *sse = vaddlvq_u32(vreinterpretq_u32_s32(vaddq_s32(sse_s32[0], sse_s32[1])));
}

static inline void highbd_variance_8xh_neon(const uint16_t* src, int src_stride, const uint16_t* ref, int ref_stride,
                                            int h, uint64_t* sse, int64_t* sum) {
    highbd_variance_large_neon(src, src_stride, ref, ref_stride, 8, h, sse, sum);
}

static inline void highbd_variance_16xh_neon(const uint16_t* src, int src_stride, const uint16_t* ref, int ref_stride,
                                             int h, uint64_t* sse, int64_t* sum) {
    highbd_variance_large_neon(src, src_stride, ref, ref_stride, 16, h, sse, sum);
}

static inline void highbd_variance_32xh_neon(const uint16_t* src, int src_stride, const uint16_t* ref, int ref_stride,
                                             int h, uint64_t* sse, int64_t* sum) {
    highbd_variance_large_neon(src, src_stride, ref, ref_stride, 32, h, sse, sum);
}

static inline void highbd_variance_64xh_neon(const uint16_t* src, int src_stride, const uint16_t* ref, int ref_stride,
                                             int h, uint64_t* sse, int64_t* sum) {
    highbd_variance_large_neon(src, src_stride, ref, ref_stride, 64, h, sse, sum);
}

static inline void highbd_variance_128xh_neon(const uint16_t* src, int src_stride, const uint16_t* ref, int ref_stride,
                                              int h, uint64_t* sse, int64_t* sum) {
    highbd_variance_large_neon(src, src_stride, ref, ref_stride, 128, h, sse, sum);
}

#define HBD_VARIANCE_WXH_10_NEON(w, h)                                                                   \
    uint32_t svt_aom_highbd_10_variance##w##x##h##_neon(                                                 \
        const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr, int ref_stride, uint32_t* sse) { \
        uint64_t  sse_long = 0;                                                                          \
        int64_t   sum_long = 0;                                                                          \
        uint16_t* src      = CONVERT_TO_SHORTPTR(src_ptr);                                               \
        uint16_t* ref      = CONVERT_TO_SHORTPTR(ref_ptr);                                               \
        highbd_variance_##w##xh_neon(src, src_stride, ref, ref_stride, h, &sse_long, &sum_long);         \
        *sse        = (uint32_t)ROUND_POWER_OF_TWO(sse_long, 4);                                         \
        int     sum = (int)ROUND_POWER_OF_TWO(sum_long, 2);                                              \
        int64_t var = (int64_t)*sse - (((int64_t)sum * sum) / (w * h));                                  \
        return var >= 0 ? (uint32_t)var : 0;                                                             \
    }

HBD_VARIANCE_WXH_10_NEON(4, 4)
HBD_VARIANCE_WXH_10_NEON(4, 8)
HBD_VARIANCE_WXH_10_NEON(4, 16)

HBD_VARIANCE_WXH_10_NEON(8, 4)
HBD_VARIANCE_WXH_10_NEON(8, 8)
HBD_VARIANCE_WXH_10_NEON(8, 16)
HBD_VARIANCE_WXH_10_NEON(8, 32)

HBD_VARIANCE_WXH_10_NEON(16, 4)
HBD_VARIANCE_WXH_10_NEON(16, 8)
HBD_VARIANCE_WXH_10_NEON(16, 16)
HBD_VARIANCE_WXH_10_NEON(16, 32)
HBD_VARIANCE_WXH_10_NEON(16, 64)

HBD_VARIANCE_WXH_10_NEON(32, 8)
HBD_VARIANCE_WXH_10_NEON(32, 16)
HBD_VARIANCE_WXH_10_NEON(32, 32)
HBD_VARIANCE_WXH_10_NEON(32, 64)

HBD_VARIANCE_WXH_10_NEON(64, 16)
HBD_VARIANCE_WXH_10_NEON(64, 32)
HBD_VARIANCE_WXH_10_NEON(64, 64)
HBD_VARIANCE_WXH_10_NEON(64, 128)

HBD_VARIANCE_WXH_10_NEON(128, 64)
HBD_VARIANCE_WXH_10_NEON(128, 128)
