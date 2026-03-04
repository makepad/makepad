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
#include "sum_neon.h"
#include "obmc_constants_neon.h"

static inline void obmc_sad_8x1_s16_neon(int16x8_t ref_s16, const int32_t* mask, const int32_t* wsrc, uint32x4_t* sum) {
    int32x4_t wsrc_lo = vld1q_s32(wsrc);
    int32x4_t wsrc_hi = vld1q_s32(wsrc + 4);

    int32x4_t mask_lo = vld1q_s32(mask);
    int32x4_t mask_hi = vld1q_s32(mask + 4);

    int16x8_t mask_s16 = vuzpq_s16(vreinterpretq_s16_s32(mask_lo), vreinterpretq_s16_s32(mask_hi)).val[0];

    int32x4_t pre_lo = vmull_s16(vget_low_s16(ref_s16), vget_low_s16(mask_s16));
    int32x4_t pre_hi = vmull_s16(vget_high_s16(ref_s16), vget_high_s16(mask_s16));

    uint32x4_t abs_lo = vreinterpretq_u32_s32(vabdq_s32(wsrc_lo, pre_lo));
    uint32x4_t abs_hi = vreinterpretq_u32_s32(vabdq_s32(wsrc_hi, pre_hi));

    *sum = vrsraq_n_u32(*sum, abs_lo, 12);
    *sum = vrsraq_n_u32(*sum, abs_hi, 12);
}

static inline void obmc_sad_8x1_s32_neon(uint32x4_t ref_u32_lo, uint32x4_t ref_u32_hi, const int32_t* mask,
                                         const int32_t* wsrc, uint32x4_t sum[2]) {
    int32x4_t wsrc_lo = vld1q_s32(wsrc);
    int32x4_t wsrc_hi = vld1q_s32(wsrc + 4);
    int32x4_t mask_lo = vld1q_s32(mask);
    int32x4_t mask_hi = vld1q_s32(mask + 4);

    int32x4_t pre_lo = vmulq_s32(vreinterpretq_s32_u32(ref_u32_lo), mask_lo);
    int32x4_t pre_hi = vmulq_s32(vreinterpretq_s32_u32(ref_u32_hi), mask_hi);

    uint32x4_t abs_lo = vreinterpretq_u32_s32(vabdq_s32(wsrc_lo, pre_lo));
    uint32x4_t abs_hi = vreinterpretq_u32_s32(vabdq_s32(wsrc_hi, pre_hi));

    sum[0] = vrsraq_n_u32(sum[0], abs_lo, 12);
    sum[1] = vrsraq_n_u32(sum[1], abs_hi, 12);
}

static inline unsigned int obmc_sad_large_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc,
                                               const int32_t* mask, int width, int height) {
    uint32x4_t sum[2] = {vdupq_n_u32(0), vdupq_n_u32(0)};

    // Use tbl for doing a double-width zero extension from 8->32 bits since we
    // can do this in one instruction rather than two.
    uint8x16_t pre_idx0 = vld1q_u8(&obmc_variance_permute_idx[0]);
    uint8x16_t pre_idx1 = vld1q_u8(&obmc_variance_permute_idx[16]);
    uint8x16_t pre_idx2 = vld1q_u8(&obmc_variance_permute_idx[32]);
    uint8x16_t pre_idx3 = vld1q_u8(&obmc_variance_permute_idx[48]);

    int h = height;
    do {
        int            w       = width;
        const uint8_t* ref_ptr = ref;
        do {
            uint8x16_t r = vld1q_u8(ref_ptr);

            uint32x4_t ref_u32_lo = vreinterpretq_u32_u8(vqtbl1q_u8(r, pre_idx0));
            uint32x4_t ref_u32_hi = vreinterpretq_u32_u8(vqtbl1q_u8(r, pre_idx1));
            obmc_sad_8x1_s32_neon(ref_u32_lo, ref_u32_hi, mask, wsrc, sum);

            ref_u32_lo = vreinterpretq_u32_u8(vqtbl1q_u8(r, pre_idx2));
            ref_u32_hi = vreinterpretq_u32_u8(vqtbl1q_u8(r, pre_idx3));
            obmc_sad_8x1_s32_neon(ref_u32_lo, ref_u32_hi, mask + 8, wsrc + 8, sum);

            ref_ptr += 16;
            wsrc += 16;
            mask += 16;
            w -= 16;
        } while (w != 0);

        ref += ref_stride;
    } while (--h != 0);

    return vaddvq_u32(vaddq_u32(sum[0], sum[1]));
}

static inline unsigned int obmc_sad_128xh_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc,
                                               const int32_t* mask, int h) {
    return obmc_sad_large_neon(ref, ref_stride, wsrc, mask, 128, h);
}

static inline unsigned int obmc_sad_64xh_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc,
                                              const int32_t* mask, int h) {
    return obmc_sad_large_neon(ref, ref_stride, wsrc, mask, 64, h);
}

static inline unsigned int obmc_sad_32xh_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc,
                                              const int32_t* mask, int h) {
    return obmc_sad_large_neon(ref, ref_stride, wsrc, mask, 32, h);
}

static inline unsigned int obmc_sad_16xh_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc,
                                              const int32_t* mask, int h) {
    return obmc_sad_large_neon(ref, ref_stride, wsrc, mask, 16, h);
}

static inline unsigned int obmc_sad_8xh_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc,
                                             const int32_t* mask, int height) {
    uint32x4_t sum = vdupq_n_u32(0);

    int h = height;
    do {
        uint8x8_t r = vld1_u8(ref);

        int16x8_t ref_s16 = vreinterpretq_s16_u16(vmovl_u8(r));
        obmc_sad_8x1_s16_neon(ref_s16, mask, wsrc, &sum);

        ref += ref_stride;
        wsrc += 8;
        mask += 8;
    } while (--h != 0);

    return vaddvq_u32(sum);
}

static inline unsigned int obmc_sad_4xh_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc,
                                             const int32_t* mask, int height) {
    uint32x4_t sum = vdupq_n_u32(0);

    int h = height / 2;
    do {
        uint8x8_t r = load_u8_4x2(ref, ref_stride);

        int16x8_t ref_s16 = vreinterpretq_s16_u16(vmovl_u8(r));
        obmc_sad_8x1_s16_neon(ref_s16, mask, wsrc, &sum);

        ref += 2 * ref_stride;
        wsrc += 8;
        mask += 8;
    } while (--h != 0);

    return vaddvq_u32(sum);
}

unsigned int svt_aom_obmc_sad4x4_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_4xh_neon(ref, ref_stride, wsrc, mask, 4);
}

unsigned int svt_aom_obmc_sad4x8_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_4xh_neon(ref, ref_stride, wsrc, mask, 8);
}

unsigned int svt_aom_obmc_sad4x16_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_4xh_neon(ref, ref_stride, wsrc, mask, 16);
}

unsigned int svt_aom_obmc_sad8x4_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_8xh_neon(ref, ref_stride, wsrc, mask, 4);
}

unsigned int svt_aom_obmc_sad8x8_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_8xh_neon(ref, ref_stride, wsrc, mask, 8);
}

unsigned int svt_aom_obmc_sad8x16_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_8xh_neon(ref, ref_stride, wsrc, mask, 16);
}

unsigned int svt_aom_obmc_sad8x32_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_8xh_neon(ref, ref_stride, wsrc, mask, 32);
}

unsigned int svt_aom_obmc_sad16x4_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_16xh_neon(ref, ref_stride, wsrc, mask, 4);
}

unsigned int svt_aom_obmc_sad16x8_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_16xh_neon(ref, ref_stride, wsrc, mask, 8);
}

unsigned int svt_aom_obmc_sad16x16_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_16xh_neon(ref, ref_stride, wsrc, mask, 16);
}

unsigned int svt_aom_obmc_sad16x32_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_16xh_neon(ref, ref_stride, wsrc, mask, 32);
}

unsigned int svt_aom_obmc_sad16x64_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_16xh_neon(ref, ref_stride, wsrc, mask, 64);
}

unsigned int svt_aom_obmc_sad32x8_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_32xh_neon(ref, ref_stride, wsrc, mask, 8);
}

unsigned int svt_aom_obmc_sad32x16_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_32xh_neon(ref, ref_stride, wsrc, mask, 16);
}

unsigned int svt_aom_obmc_sad32x32_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_32xh_neon(ref, ref_stride, wsrc, mask, 32);
}

unsigned int svt_aom_obmc_sad32x64_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_32xh_neon(ref, ref_stride, wsrc, mask, 64);
}

unsigned int svt_aom_obmc_sad64x16_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_64xh_neon(ref, ref_stride, wsrc, mask, 16);
}

unsigned int svt_aom_obmc_sad64x32_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_64xh_neon(ref, ref_stride, wsrc, mask, 32);
}

unsigned int svt_aom_obmc_sad64x64_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_64xh_neon(ref, ref_stride, wsrc, mask, 64);
}

unsigned int svt_aom_obmc_sad64x128_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_64xh_neon(ref, ref_stride, wsrc, mask, 128);
}

unsigned int svt_aom_obmc_sad128x64_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) {
    return obmc_sad_128xh_neon(ref, ref_stride, wsrc, mask, 64);
}

unsigned int svt_aom_obmc_sad128x128_neon(const uint8_t* ref, int ref_stride, const int32_t* wsrc,
                                          const int32_t* mask) {
    return obmc_sad_128xh_neon(ref, ref_stride, wsrc, mask, 128);
}
