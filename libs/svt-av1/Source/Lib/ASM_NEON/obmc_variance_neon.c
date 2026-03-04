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
#include "mem_neon.h"
#include "sum_neon.h"
#include "aom_dsp_rtcd.h"
#include "var_filter_neon.h"
#include "obmc_constants_neon.h"

static inline void obmc_variance_8x1_s16_neon(int16x8_t pre_s16, const int32_t* wsrc, const int32_t* mask,
                                              int32x4_t* ssev, int32x4_t* sumv) {
    // For 4xh and 8xh we observe it is faster to avoid the double-widening of
    // pre. Instead we do a single widening step and narrow the mask to 16-bits
    // to allow us to perform a widening multiply. Widening multiply
    // instructions have better throughput on some micro-architectures but for
    // the larger block sizes this benefit is outweighed by the additional
    // instruction needed to first narrow the mask vectors.

    int32x4_t wsrc_s32_lo = vld1q_s32(&wsrc[0]);
    int32x4_t wsrc_s32_hi = vld1q_s32(&wsrc[4]);
    int16x8_t mask_s16 =
        vuzpq_s16(vreinterpretq_s16_s32(vld1q_s32(&mask[0])), vreinterpretq_s16_s32(vld1q_s32(&mask[4]))).val[0];

    int32x4_t diff_s32_lo = vmlsl_s16(wsrc_s32_lo, vget_low_s16(pre_s16), vget_low_s16(mask_s16));
    int32x4_t diff_s32_hi = vmlsl_s16(wsrc_s32_hi, vget_high_s16(pre_s16), vget_high_s16(mask_s16));

    // ROUND_POWER_OF_TWO_SIGNED(value, 12) rounds to nearest with ties away
    // from zero, however vrshrq_n_s32 rounds to nearest with ties rounded up.
    // This difference only affects the bit patterns at the rounding breakpoints
    // exactly, so we can add -1 to all negative numbers to move the breakpoint
    // one value across and into the correct rounding region.
    diff_s32_lo            = vsraq_n_s32(diff_s32_lo, diff_s32_lo, 31);
    diff_s32_hi            = vsraq_n_s32(diff_s32_hi, diff_s32_hi, 31);
    int32x4_t round_s32_lo = vrshrq_n_s32(diff_s32_lo, 12);
    int32x4_t round_s32_hi = vrshrq_n_s32(diff_s32_hi, 12);

    *sumv = vrsraq_n_s32(*sumv, diff_s32_lo, 12);
    *sumv = vrsraq_n_s32(*sumv, diff_s32_hi, 12);
    *ssev = vmlaq_s32(*ssev, round_s32_lo, round_s32_lo);
    *ssev = vmlaq_s32(*ssev, round_s32_hi, round_s32_hi);
}

// Use tbl for doing a double-width zero extension from 8->32 bits since we can
// do this in one instruction rather than two (indices out of range (255 here)
// are set to zero by tbl).
DECLARE_ALIGNED(16, const uint8_t, obmc_variance_permute_idx[]) = {
    0,   255, 255, 255, 1,   255, 255, 255, 2,   255, 255, 255, 3,   255, 255, 255, 4,   255, 255, 255, 5,   255,
    255, 255, 6,   255, 255, 255, 7,   255, 255, 255, 8,   255, 255, 255, 9,   255, 255, 255, 10,  255, 255, 255,
    11,  255, 255, 255, 12,  255, 255, 255, 13,  255, 255, 255, 14,  255, 255, 255, 15,  255, 255, 255};

static inline void obmc_variance_8x1_s32_neon(int32x4_t pre_lo, int32x4_t pre_hi, const int32_t* wsrc,
                                              const int32_t* mask, int32x4_t* ssev, int32x4_t* sumv) {
    int32x4_t wsrc_lo = vld1q_s32(&wsrc[0]);
    int32x4_t wsrc_hi = vld1q_s32(&wsrc[4]);
    int32x4_t mask_lo = vld1q_s32(&mask[0]);
    int32x4_t mask_hi = vld1q_s32(&mask[4]);

    int32x4_t diff_lo = vmlsq_s32(wsrc_lo, pre_lo, mask_lo);
    int32x4_t diff_hi = vmlsq_s32(wsrc_hi, pre_hi, mask_hi);

    // ROUND_POWER_OF_TWO_SIGNED(value, 12) rounds to nearest with ties away from
    // zero, however vrshrq_n_s32 rounds to nearest with ties rounded up. This
    // difference only affects the bit patterns at the rounding breakpoints
    // exactly, so we can add -1 to all negative numbers to move the breakpoint
    // one value across and into the correct rounding region.
    diff_lo            = vsraq_n_s32(diff_lo, diff_lo, 31);
    diff_hi            = vsraq_n_s32(diff_hi, diff_hi, 31);
    int32x4_t round_lo = vrshrq_n_s32(diff_lo, 12);
    int32x4_t round_hi = vrshrq_n_s32(diff_hi, 12);

    *sumv = vrsraq_n_s32(*sumv, diff_lo, 12);
    *sumv = vrsraq_n_s32(*sumv, diff_hi, 12);
    *ssev = vmlaq_s32(*ssev, round_lo, round_lo);
    *ssev = vmlaq_s32(*ssev, round_hi, round_hi);
}

static inline void obmc_variance_large_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc,
                                            const int32_t* mask, int width, int height, unsigned* sse, int* sum) {
    assert(width % 16 == 0);

    // Use tbl for doing a double-width zero extension from 8->32 bits since we
    // can do this in one instruction rather than two.
    uint8x16_t pre_idx0 = vld1q_u8(&obmc_variance_permute_idx[0]);
    uint8x16_t pre_idx1 = vld1q_u8(&obmc_variance_permute_idx[16]);
    uint8x16_t pre_idx2 = vld1q_u8(&obmc_variance_permute_idx[32]);
    uint8x16_t pre_idx3 = vld1q_u8(&obmc_variance_permute_idx[48]);

    int32x4_t ssev = vdupq_n_s32(0);
    int32x4_t sumv = vdupq_n_s32(0);

    int h = height;
    do {
        int w = width;
        do {
            uint8x16_t pre_u8 = vld1q_u8(pre);

            int32x4_t pre_s32_lo = vreinterpretq_s32_u8(vqtbl1q_u8(pre_u8, pre_idx0));
            int32x4_t pre_s32_hi = vreinterpretq_s32_u8(vqtbl1q_u8(pre_u8, pre_idx1));
            obmc_variance_8x1_s32_neon(pre_s32_lo, pre_s32_hi, &wsrc[0], &mask[0], &ssev, &sumv);

            pre_s32_lo = vreinterpretq_s32_u8(vqtbl1q_u8(pre_u8, pre_idx2));
            pre_s32_hi = vreinterpretq_s32_u8(vqtbl1q_u8(pre_u8, pre_idx3));
            obmc_variance_8x1_s32_neon(pre_s32_lo, pre_s32_hi, &wsrc[8], &mask[8], &ssev, &sumv);

            wsrc += 16;
            mask += 16;
            pre += 16;
            w -= 16;
        } while (w != 0);

        pre += pre_stride - width;
    } while (--h != 0);

    *sse = vaddvq_s32(ssev);
    *sum = vaddvq_s32(sumv);
}

static inline void obmc_variance_neon_128xh(const uint8_t* pre, int pre_stride, const int32_t* wsrc,
                                            const int32_t* mask, int h, unsigned* sse, int* sum) {
    obmc_variance_large_neon(pre, pre_stride, wsrc, mask, 128, h, sse, sum);
}

static inline void obmc_variance_neon_64xh(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                           int h, unsigned* sse, int* sum) {
    obmc_variance_large_neon(pre, pre_stride, wsrc, mask, 64, h, sse, sum);
}

static inline void obmc_variance_neon_32xh(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                           int h, unsigned* sse, int* sum) {
    obmc_variance_large_neon(pre, pre_stride, wsrc, mask, 32, h, sse, sum);
}

static inline void obmc_variance_neon_16xh(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                           int h, unsigned* sse, int* sum) {
    obmc_variance_large_neon(pre, pre_stride, wsrc, mask, 16, h, sse, sum);
}

static inline void obmc_variance_neon_8xh(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                          int h, unsigned* sse, int* sum) {
    int32x4_t ssev = vdupq_n_s32(0);
    int32x4_t sumv = vdupq_n_s32(0);

    do {
        uint8x8_t pre_u8  = vld1_u8(pre);
        int16x8_t pre_s16 = vreinterpretq_s16_u16(vmovl_u8(pre_u8));

        obmc_variance_8x1_s16_neon(pre_s16, wsrc, mask, &ssev, &sumv);

        pre += pre_stride;
        wsrc += 8;
        mask += 8;
    } while (--h != 0);

    *sse = vaddvq_s32(ssev);
    *sum = vaddvq_s32(sumv);
}

static inline void obmc_variance_neon_4xh(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                          int h, unsigned* sse, int* sum) {
    assert(h % 2 == 0);

    int32x4_t ssev = vdupq_n_s32(0);
    int32x4_t sumv = vdupq_n_s32(0);

    do {
        uint8x8_t pre_u8  = load_u8_4x2(pre, pre_stride);
        int16x8_t pre_s16 = vreinterpretq_s16_u16(vmovl_u8(pre_u8));

        obmc_variance_8x1_s16_neon(pre_s16, wsrc, mask, &ssev, &sumv);

        pre += 2 * pre_stride;
        wsrc += 8;
        mask += 8;
        h -= 2;
    } while (h != 0);

    *sse = vaddvq_s32(ssev);
    *sum = vaddvq_s32(sumv);
}

unsigned svt_aom_obmc_variance4x4_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                       unsigned* sse) {
    int sum;
    obmc_variance_neon_4xh(pre, pre_stride, wsrc, mask, 4, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (4 * 4));
}

unsigned svt_aom_obmc_variance4x8_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                       unsigned* sse) {
    int sum;
    obmc_variance_neon_4xh(pre, pre_stride, wsrc, mask, 8, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (4 * 8));
}

unsigned svt_aom_obmc_variance8x4_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                       unsigned* sse) {
    int sum;
    obmc_variance_neon_8xh(pre, pre_stride, wsrc, mask, 4, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (8 * 4));
}

unsigned svt_aom_obmc_variance8x8_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                       unsigned* sse) {
    int sum;
    obmc_variance_neon_8xh(pre, pre_stride, wsrc, mask, 8, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (8 * 8));
}

unsigned svt_aom_obmc_variance8x16_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                        unsigned* sse) {
    int sum;
    obmc_variance_neon_8xh(pre, pre_stride, wsrc, mask, 16, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (8 * 16));
}

unsigned svt_aom_obmc_variance16x8_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                        unsigned* sse) {
    int sum;
    obmc_variance_neon_16xh(pre, pre_stride, wsrc, mask, 8, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (16 * 8));
}

unsigned svt_aom_obmc_variance16x16_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                         unsigned* sse) {
    int sum;
    obmc_variance_neon_16xh(pre, pre_stride, wsrc, mask, 16, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (16 * 16));
}

unsigned svt_aom_obmc_variance16x32_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                         unsigned* sse) {
    int sum;
    obmc_variance_neon_16xh(pre, pre_stride, wsrc, mask, 32, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (16 * 32));
}

unsigned svt_aom_obmc_variance32x16_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                         unsigned* sse) {
    int sum;
    obmc_variance_neon_32xh(pre, pre_stride, wsrc, mask, 16, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (32 * 16));
}

unsigned svt_aom_obmc_variance32x32_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                         unsigned* sse) {
    int sum;
    obmc_variance_neon_32xh(pre, pre_stride, wsrc, mask, 32, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (32 * 32));
}

unsigned svt_aom_obmc_variance32x64_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                         unsigned* sse) {
    int sum;
    obmc_variance_neon_32xh(pre, pre_stride, wsrc, mask, 64, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (32 * 64));
}

unsigned svt_aom_obmc_variance64x32_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                         unsigned* sse) {
    int sum;
    obmc_variance_neon_64xh(pre, pre_stride, wsrc, mask, 32, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (64 * 32));
}

unsigned svt_aom_obmc_variance64x64_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                         unsigned* sse) {
    int sum;
    obmc_variance_neon_64xh(pre, pre_stride, wsrc, mask, 64, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (64 * 64));
}

unsigned svt_aom_obmc_variance64x128_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                          unsigned* sse) {
    int sum;
    obmc_variance_neon_64xh(pre, pre_stride, wsrc, mask, 128, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (64 * 128));
}

unsigned svt_aom_obmc_variance128x64_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                          unsigned* sse) {
    int sum;
    obmc_variance_neon_128xh(pre, pre_stride, wsrc, mask, 64, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (128 * 64));
}

unsigned svt_aom_obmc_variance128x128_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                           unsigned* sse) {
    int sum;
    obmc_variance_neon_128xh(pre, pre_stride, wsrc, mask, 128, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (128 * 128));
}

unsigned svt_aom_obmc_variance4x16_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                        unsigned* sse) {
    int sum;
    obmc_variance_neon_4xh(pre, pre_stride, wsrc, mask, 16, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (4 * 16));
}

unsigned svt_aom_obmc_variance16x4_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                        unsigned* sse) {
    int sum;
    obmc_variance_neon_16xh(pre, pre_stride, wsrc, mask, 4, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (16 * 4));
}

unsigned svt_aom_obmc_variance8x32_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                        unsigned* sse) {
    int sum;
    obmc_variance_neon_8xh(pre, pre_stride, wsrc, mask, 32, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (8 * 32));
}

unsigned svt_aom_obmc_variance32x8_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                        unsigned* sse) {
    int sum;
    obmc_variance_neon_32xh(pre, pre_stride, wsrc, mask, 8, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (32 * 8));
}

unsigned svt_aom_obmc_variance16x64_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                         unsigned* sse) {
    int sum;
    obmc_variance_neon_16xh(pre, pre_stride, wsrc, mask, 64, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (16 * 64));
}

unsigned svt_aom_obmc_variance64x16_neon(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                         unsigned* sse) {
    int sum;
    obmc_variance_neon_64xh(pre, pre_stride, wsrc, mask, 16, sse, &sum);
    return *sse - (unsigned)(((int64_t)sum * sum) / (64 * 16));
}

#define OBMC_SUBPEL_VARIANCE_WXH_NEON(w, h, padding)                                        \
    unsigned int svt_aom_obmc_sub_pixel_variance##w##x##h##_neon(const uint8_t* pre,        \
                                                                 int            pre_stride, \
                                                                 int            xoffset,    \
                                                                 int            yoffset,    \
                                                                 const int32_t* wsrc,       \
                                                                 const int32_t* mask,       \
                                                                 unsigned int*  sse) {       \
        uint8_t tmp0[w * (h + padding)];                                                    \
        uint8_t tmp1[w * h];                                                                \
        var_filter_block2d_bil_w##w(pre, tmp0, pre_stride, 1, h + padding, xoffset);        \
        var_filter_block2d_bil_w##w(tmp0, tmp1, w, w, h, yoffset);                          \
        return svt_aom_obmc_variance##w##x##h##_neon(tmp1, w, wsrc, mask, sse);             \
    }

#define SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(w, h, padding)                                \
    unsigned int svt_aom_obmc_sub_pixel_variance##w##x##h##_neon(const uint8_t* pre,            \
                                                                 int            pre_stride,     \
                                                                 int            xoffset,        \
                                                                 int            yoffset,        \
                                                                 const int32_t* wsrc,           \
                                                                 const int32_t* mask,           \
                                                                 unsigned int*  sse) {           \
        if (xoffset == 0) {                                                                     \
            if (yoffset == 0) {                                                                 \
                return svt_aom_obmc_variance##w##x##h##_neon(pre, pre_stride, wsrc, mask, sse); \
            } else if (yoffset == 4) {                                                          \
                uint8_t tmp[w * h];                                                             \
                var_filter_block2d_avg(pre, tmp, pre_stride, pre_stride, w, h);                 \
                return svt_aom_obmc_variance##w##x##h##_neon(tmp, w, wsrc, mask, sse);          \
            } else {                                                                            \
                uint8_t tmp[w * h];                                                             \
                var_filter_block2d_bil_w##w(pre, tmp, pre_stride, pre_stride, h, yoffset);      \
                return svt_aom_obmc_variance##w##x##h##_neon(tmp, w, wsrc, mask, sse);          \
            }                                                                                   \
        } else if (xoffset == 4) {                                                              \
            uint8_t tmp0[w * (h + padding)];                                                    \
            if (yoffset == 0) {                                                                 \
                var_filter_block2d_avg(pre, tmp0, pre_stride, 1, w, h);                         \
                return svt_aom_obmc_variance##w##x##h##_neon(tmp0, w, wsrc, mask, sse);         \
            } else if (yoffset == 4) {                                                          \
                uint8_t tmp1[w * (h + padding)];                                                \
                var_filter_block2d_avg(pre, tmp0, pre_stride, 1, w, h + padding);               \
                var_filter_block2d_avg(tmp0, tmp1, w, w, w, h);                                 \
                return svt_aom_obmc_variance##w##x##h##_neon(tmp1, w, wsrc, mask, sse);         \
            } else {                                                                            \
                uint8_t tmp1[w * (h + padding)];                                                \
                var_filter_block2d_avg(pre, tmp0, pre_stride, 1, w, h + padding);               \
                var_filter_block2d_bil_w##w(tmp0, tmp1, w, w, h, yoffset);                      \
                return svt_aom_obmc_variance##w##x##h##_neon(tmp1, w, wsrc, mask, sse);         \
            }                                                                                   \
        } else {                                                                                \
            uint8_t tmp0[w * (h + padding)];                                                    \
            if (yoffset == 0) {                                                                 \
                var_filter_block2d_bil_w##w(pre, tmp0, pre_stride, 1, h, xoffset);              \
                return svt_aom_obmc_variance##w##x##h##_neon(tmp0, w, wsrc, mask, sse);         \
            } else if (yoffset == 4) {                                                          \
                uint8_t tmp1[w * h];                                                            \
                var_filter_block2d_bil_w##w(pre, tmp0, pre_stride, 1, h + padding, xoffset);    \
                var_filter_block2d_avg(tmp0, tmp1, w, w, w, h);                                 \
                return svt_aom_obmc_variance##w##x##h##_neon(tmp1, w, wsrc, mask, sse);         \
            } else {                                                                            \
                uint8_t tmp1[w * h];                                                            \
                var_filter_block2d_bil_w##w(pre, tmp0, pre_stride, 1, h + padding, xoffset);    \
                var_filter_block2d_bil_w##w(tmp0, tmp1, w, w, h, yoffset);                      \
                return svt_aom_obmc_variance##w##x##h##_neon(tmp1, w, wsrc, mask, sse);         \
            }                                                                                   \
        }                                                                                       \
    }

OBMC_SUBPEL_VARIANCE_WXH_NEON(4, 4, 2)
OBMC_SUBPEL_VARIANCE_WXH_NEON(4, 8, 2)
OBMC_SUBPEL_VARIANCE_WXH_NEON(4, 16, 2)

OBMC_SUBPEL_VARIANCE_WXH_NEON(8, 4, 1)
OBMC_SUBPEL_VARIANCE_WXH_NEON(8, 8, 1)
OBMC_SUBPEL_VARIANCE_WXH_NEON(8, 16, 1)
OBMC_SUBPEL_VARIANCE_WXH_NEON(8, 32, 1)

OBMC_SUBPEL_VARIANCE_WXH_NEON(16, 4, 1)
OBMC_SUBPEL_VARIANCE_WXH_NEON(16, 8, 1)
SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(16, 16, 1)
SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(16, 32, 1)
SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(16, 64, 1)

SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(32, 8, 1)
SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(32, 16, 1)
SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(32, 32, 1)
SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(32, 64, 1)

SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(64, 16, 1)
SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(64, 32, 1)
SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(64, 64, 1)
SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(64, 128, 1)

SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(128, 64, 1)
SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON(128, 128, 1)

#undef OBMC_SUBPEL_VARIANCE_WXH_NEON
#undef SPECIALIZED_OBMC_SUBPEL_VARIANCE_WXH_NEON
