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
#include "var_filter_neon.h"

static inline void variance_4xh_neon(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, int h,
                                     uint32_t* sse, int* sum) {
    int16x8_t sum_s16 = vdupq_n_s16(0);
    int32x4_t sse_s32 = vdupq_n_s32(0);

    // Number of rows we can process before 'sum_s16' overflows:
    // 32767 / 255 ~= 128, but we use an 8-wide accumulator; so 256 4-wide rows.
    assert(h <= 256);

    int i = h;
    do {
        uint8x8_t s    = load_u8_4x2(src, src_stride);
        uint8x8_t r    = load_u8_4x2(ref, ref_stride);
        int16x8_t diff = vreinterpretq_s16_u16(vsubl_u8(s, r));

        sum_s16 = vaddq_s16(sum_s16, diff);

        sse_s32 = vmlal_s16(sse_s32, vget_low_s16(diff), vget_low_s16(diff));
        sse_s32 = vmlal_s16(sse_s32, vget_high_s16(diff), vget_high_s16(diff));

        src += 2 * src_stride;
        ref += 2 * ref_stride;
        i -= 2;
    } while (i != 0);

    *sum = vaddlvq_s16(sum_s16);
    *sse = (uint32_t)vaddvq_s32(sse_s32);
}

static inline void variance_8xh_neon(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, int h,
                                     uint32_t* sse, int* sum) {
    int16x8_t sum_s16    = vdupq_n_s16(0);
    int32x4_t sse_s32[2] = {vdupq_n_s32(0), vdupq_n_s32(0)};

    // Number of rows we can process before 'sum_s16' overflows:
    // 32767 / 255 ~= 128
    assert(h <= 128);

    int i = h;
    do {
        uint8x8_t s    = vld1_u8(src);
        uint8x8_t r    = vld1_u8(ref);
        int16x8_t diff = vreinterpretq_s16_u16(vsubl_u8(s, r));

        sum_s16 = vaddq_s16(sum_s16, diff);

        sse_s32[0] = vmlal_s16(sse_s32[0], vget_low_s16(diff), vget_low_s16(diff));
        sse_s32[1] = vmlal_s16(sse_s32[1], vget_high_s16(diff), vget_high_s16(diff));

        src += src_stride;
        ref += ref_stride;
    } while (--i != 0);

    *sum = vaddlvq_s16(sum_s16);
    *sse = (uint32_t)vaddvq_s32(vaddq_s32(sse_s32[0], sse_s32[1]));
}

static inline void variance_16xh_neon(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, int h,
                                      uint32_t* sse, int* sum) {
    int16x8_t sum_s16[2] = {vdupq_n_s16(0), vdupq_n_s16(0)};
    int32x4_t sse_s32[2] = {vdupq_n_s32(0), vdupq_n_s32(0)};

    // Number of rows we can process before 'sum_s16' accumulators overflow:
    // 32767 / 255 ~= 128, so 128 16-wide rows.
    assert(h <= 128);

    int i = h;
    do {
        uint8x16_t s = vld1q_u8(src);
        uint8x16_t r = vld1q_u8(ref);

        int16x8_t diff_l = vreinterpretq_s16_u16(vsubl_u8(vget_low_u8(s), vget_low_u8(r)));
        int16x8_t diff_h = vreinterpretq_s16_u16(vsubl_u8(vget_high_u8(s), vget_high_u8(r)));

        sum_s16[0] = vaddq_s16(sum_s16[0], diff_l);
        sum_s16[1] = vaddq_s16(sum_s16[1], diff_h);

        sse_s32[0] = vmlal_s16(sse_s32[0], vget_low_s16(diff_l), vget_low_s16(diff_l));
        sse_s32[1] = vmlal_s16(sse_s32[1], vget_high_s16(diff_l), vget_high_s16(diff_l));
        sse_s32[0] = vmlal_s16(sse_s32[0], vget_low_s16(diff_h), vget_low_s16(diff_h));
        sse_s32[1] = vmlal_s16(sse_s32[1], vget_high_s16(diff_h), vget_high_s16(diff_h));

        src += src_stride;
        ref += ref_stride;
    } while (--i != 0);

    *sum = vaddlvq_s16(vaddq_s16(sum_s16[0], sum_s16[1]));
    *sse = (uint32_t)vaddvq_s32(vaddq_s32(sse_s32[0], sse_s32[1]));
}

static inline void variance_large_neon(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, int w,
                                       int h, int h_limit, uint32_t* sse, int* sum) {
    int32x4_t sum_s32 = vdupq_n_s32(0);

    int32x4_t sse_s32_0 = vdupq_n_s32(0);
    int32x4_t sse_s32_1 = vdupq_n_s32(0);
    int32x4_t sse_s32_2 = vdupq_n_s32(0);
    int32x4_t sse_s32_3 = vdupq_n_s32(0);

    // 'h_limit' is the number of 'w'-width rows we can process before our 16-bit
    // accumulator overflows. After hitting this limit we accumulate into 32-bit
    // elements.
    int h_tmp = h > h_limit ? h_limit : h;

    int i = 0;
    do {
        int16x8_t sum_s16_0 = vdupq_n_s16(0);
        int16x8_t sum_s16_1 = vdupq_n_s16(0);

        do {
            int j = 0;
            do {
                uint8x16_t s = vld1q_u8(src + j);
                uint8x16_t r = vld1q_u8(ref + j);

                const int16x8_t diff_l = vreinterpretq_s16_u16(vsubl_u8(vget_low_u8(s), vget_low_u8(r)));
                const int16x8_t diff_h = vreinterpretq_s16_u16(vsubl_u8(vget_high_u8(s), vget_high_u8(r)));

                sum_s16_0 = vaddq_s16(sum_s16_0, diff_l);
                sum_s16_1 = vaddq_s16(sum_s16_1, diff_h);

                sse_s32_0 = vmlal_s16(sse_s32_0, vget_low_s16(diff_l), vget_low_s16(diff_l));
                sse_s32_1 = vmlal_s16(sse_s32_1, vget_high_s16(diff_l), vget_high_s16(diff_l));
                sse_s32_2 = vmlal_s16(sse_s32_2, vget_low_s16(diff_h), vget_low_s16(diff_h));
                sse_s32_3 = vmlal_s16(sse_s32_3, vget_high_s16(diff_h), vget_high_s16(diff_h));

                j += 16;
            } while (j < w);

            src += src_stride;
            ref += ref_stride;
            i++;
        } while (i < h_tmp);

        sum_s32 = vpadalq_s16(sum_s32, sum_s16_0);
        sum_s32 = vpadalq_s16(sum_s32, sum_s16_1);

        h_tmp += h_limit;
    } while (i < h);

    *sum = vaddvq_s32(sum_s32);
    *sse = (uint32_t)vaddvq_s32(vaddq_s32(vaddq_s32(sse_s32_0, sse_s32_1), vaddq_s32(sse_s32_2, sse_s32_3)));
}

static inline void variance_32xh_neon(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, int h,
                                      uint32_t* sse, int* sum) {
    variance_large_neon(src, src_stride, ref, ref_stride, 32, h, 64, sse, sum);
}

static inline void variance_64xh_neon(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, int h,
                                      uint32_t* sse, int* sum) {
    variance_large_neon(src, src_stride, ref, ref_stride, 64, h, 32, sse, sum);
}

static inline void variance_128xh_neon(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, int h,
                                       uint32_t* sse, int* sum) {
    variance_large_neon(src, src_stride, ref, ref_stride, 128, h, 16, sse, sum);
}

#define VARIANCE_WXH_NEON(w, h, shift)                                                               \
    unsigned int svt_aom_variance##w##x##h##_neon(                                                   \
        const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride, unsigned int* sse) { \
        int sum;                                                                                     \
        variance_##w##xh_neon(src, src_stride, ref, ref_stride, h, sse, &sum);                       \
        return *sse - (uint32_t)(((int64_t)sum * sum) >> shift);                                     \
    }

VARIANCE_WXH_NEON(4, 4, 4)
VARIANCE_WXH_NEON(4, 8, 5)
VARIANCE_WXH_NEON(4, 16, 6)

VARIANCE_WXH_NEON(8, 4, 5)
VARIANCE_WXH_NEON(8, 8, 6)
VARIANCE_WXH_NEON(8, 16, 7)
VARIANCE_WXH_NEON(8, 32, 8)

VARIANCE_WXH_NEON(16, 4, 6)
VARIANCE_WXH_NEON(16, 8, 7)
VARIANCE_WXH_NEON(16, 16, 8)
VARIANCE_WXH_NEON(16, 32, 9)
VARIANCE_WXH_NEON(16, 64, 10)

VARIANCE_WXH_NEON(32, 8, 8)
VARIANCE_WXH_NEON(32, 16, 9)
VARIANCE_WXH_NEON(32, 32, 10)
VARIANCE_WXH_NEON(32, 64, 11)

VARIANCE_WXH_NEON(64, 16, 10)
VARIANCE_WXH_NEON(64, 32, 11)
VARIANCE_WXH_NEON(64, 64, 12)
VARIANCE_WXH_NEON(64, 128, 13)

VARIANCE_WXH_NEON(128, 64, 13)
VARIANCE_WXH_NEON(128, 128, 14)

#undef VARIANCE_WXH_NEON

#define SUBPEL_VARIANCE_WXH_NEON(w, h, padding)                                        \
    unsigned int svt_aom_sub_pixel_variance##w##x##h##_neon(const uint8_t* src,        \
                                                            int            src_stride, \
                                                            int            xoffset,    \
                                                            int            yoffset,    \
                                                            const uint8_t* ref,        \
                                                            int            ref_stride, \
                                                            uint32_t*      sse) {           \
        uint8_t tmp0[w * (h + padding)];                                               \
        uint8_t tmp1[w * h];                                                           \
        var_filter_block2d_bil_w##w(src, tmp0, src_stride, 1, (h + padding), xoffset); \
        var_filter_block2d_bil_w##w(tmp0, tmp1, w, w, h, yoffset);                     \
        return svt_aom_variance##w##x##h##_neon(tmp1, w, ref, ref_stride, sse);        \
    }

#define SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(w, h, padding)                                     \
    unsigned int svt_aom_sub_pixel_variance##w##x##h##_neon(const uint8_t* src,                 \
                                                            int            src_stride,          \
                                                            int            xoffset,             \
                                                            int            yoffset,             \
                                                            const uint8_t* ref,                 \
                                                            int            ref_stride,          \
                                                            unsigned int*  sse) {                \
        if (xoffset == 0) {                                                                     \
            if (yoffset == 0) {                                                                 \
                return svt_aom_variance##w##x##h##_neon(src, src_stride, ref, ref_stride, sse); \
            } else if (yoffset == 4) {                                                          \
                uint8_t tmp[w * h];                                                             \
                var_filter_block2d_avg(src, tmp, src_stride, src_stride, w, h);                 \
                return svt_aom_variance##w##x##h##_neon(tmp, w, ref, ref_stride, sse);          \
            } else {                                                                            \
                uint8_t tmp[w * h];                                                             \
                var_filter_block2d_bil_w##w(src, tmp, src_stride, src_stride, h, yoffset);      \
                return svt_aom_variance##w##x##h##_neon(tmp, w, ref, ref_stride, sse);          \
            }                                                                                   \
        } else if (xoffset == 4) {                                                              \
            uint8_t tmp0[w * (h + padding)];                                                    \
            if (yoffset == 0) {                                                                 \
                var_filter_block2d_avg(src, tmp0, src_stride, 1, w, h);                         \
                return svt_aom_variance##w##x##h##_neon(tmp0, w, ref, ref_stride, sse);         \
            } else if (yoffset == 4) {                                                          \
                uint8_t tmp1[w * (h + padding)];                                                \
                var_filter_block2d_avg(src, tmp0, src_stride, 1, w, (h + padding));             \
                var_filter_block2d_avg(tmp0, tmp1, w, w, w, h);                                 \
                return svt_aom_variance##w##x##h##_neon(tmp1, w, ref, ref_stride, sse);         \
            } else {                                                                            \
                uint8_t tmp1[w * (h + padding)];                                                \
                var_filter_block2d_avg(src, tmp0, src_stride, 1, w, (h + padding));             \
                var_filter_block2d_bil_w##w(tmp0, tmp1, w, w, h, yoffset);                      \
                return svt_aom_variance##w##x##h##_neon(tmp1, w, ref, ref_stride, sse);         \
            }                                                                                   \
        } else {                                                                                \
            uint8_t tmp0[w * (h + padding)];                                                    \
            if (yoffset == 0) {                                                                 \
                var_filter_block2d_bil_w##w(src, tmp0, src_stride, 1, h, xoffset);              \
                return svt_aom_variance##w##x##h##_neon(tmp0, w, ref, ref_stride, sse);         \
            } else if (yoffset == 4) {                                                          \
                uint8_t tmp1[w * h];                                                            \
                var_filter_block2d_bil_w##w(src, tmp0, src_stride, 1, (h + padding), xoffset);  \
                var_filter_block2d_avg(tmp0, tmp1, w, w, w, h);                                 \
                return svt_aom_variance##w##x##h##_neon(tmp1, w, ref, ref_stride, sse);         \
            } else {                                                                            \
                uint8_t tmp1[w * h];                                                            \
                var_filter_block2d_bil_w##w(src, tmp0, src_stride, 1, (h + padding), xoffset);  \
                var_filter_block2d_bil_w##w(tmp0, tmp1, w, w, h, yoffset);                      \
                return svt_aom_variance##w##x##h##_neon(tmp1, w, ref, ref_stride, sse);         \
            }                                                                                   \
        }                                                                                       \
    }

SUBPEL_VARIANCE_WXH_NEON(4, 4, 2)
SUBPEL_VARIANCE_WXH_NEON(4, 8, 2)
SUBPEL_VARIANCE_WXH_NEON(4, 16, 2)

SUBPEL_VARIANCE_WXH_NEON(8, 4, 1)
SUBPEL_VARIANCE_WXH_NEON(8, 8, 1)
SUBPEL_VARIANCE_WXH_NEON(8, 16, 1)
SUBPEL_VARIANCE_WXH_NEON(8, 32, 1)

SUBPEL_VARIANCE_WXH_NEON(16, 4, 1)
SUBPEL_VARIANCE_WXH_NEON(16, 8, 1)
SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(16, 16, 1)
SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(16, 32, 1)
SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(16, 64, 1)

SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(32, 8, 1)
SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(32, 16, 1)
SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(32, 32, 1)
SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(32, 64, 1)

SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(64, 16, 1)
SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(64, 32, 1)
SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(64, 64, 1)
SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(64, 128, 1)

SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(128, 64, 1)
SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON(128, 128, 1)

#undef SUBPEL_VARIANCE_WXH_NEON
#undef SPECIALIZED_SUBPEL_VARIANCE_WXH_NEON

unsigned int svt_aom_mse16x16_neon(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride) {
    int32x4_t sse_s32[4] = {vdupq_n_s32(0), vdupq_n_s32(0), vdupq_n_s32(0), vdupq_n_s32(0)};

    int i = 16;
    do {
        uint8x16_t s0 = vld1q_u8(src);
        uint8x16_t r0 = vld1q_u8(ref);

        int16x8_t diff0 = vreinterpretq_s16_u16(vsubl_u8(vget_low_u8(s0), vget_low_u8(r0)));
        int16x8_t diff1 = vreinterpretq_s16_u16(vsubl_u8(vget_high_u8(s0), vget_high_u8(r0)));

        sse_s32[0] = vmlal_s16(sse_s32[0], vget_low_s16(diff1), vget_low_s16(diff1));
        sse_s32[1] = vmlal_s16(sse_s32[1], vget_low_s16(diff0), vget_low_s16(diff0));

        sse_s32[2] = vmlal_s16(sse_s32[2], vget_high_s16(diff0), vget_high_s16(diff0));
        sse_s32[3] = vmlal_s16(sse_s32[3], vget_high_s16(diff1), vget_high_s16(diff1));

        src += src_stride;
        ref += ref_stride;
    } while (--i != 0);

    unsigned int sse = vaddvq_u32(vreinterpretq_u32_s32(horizontal_add_4d_s32x4(sse_s32)));
    return sse;
}
