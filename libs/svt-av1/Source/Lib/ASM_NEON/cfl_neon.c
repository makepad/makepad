/*
 * Copyright (c) 2024, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */
#include <arm_neon.h>

#include "definitions.h"
#include "intra_prediction.h"
#include "mem_neon.h"

/* Store half of a vector. */
static inline void vsth_u8(uint8_t* ptr, uint8x8_t val) {
    vst1_lane_u32((uint32_t*)ptr, vreinterpret_u32_u8(val), 0);
}

/* Saturating negate 16-bit integers in a when the corresponding signed 16-bit
integer in b is negative.
Notes:
  * Negating INT16_MIN results in INT16_MIN. However, this cannot occur in
  practice, as scaled_luma is the multiplication of two absolute values.
  * In the Intel equivalent, elements in a are zeroed out when the
  corresponding elements in b are zero. Because vsign is used twice in a
  row, with b in the first call becoming a in the second call, there's no
  impact from not zeroing out. */
static inline int16x4_t vsign_s16(int16x4_t a, int16x4_t b) {
    const int16x4_t mask = vshr_n_s16(b, 15);
    return veor_s16(vadd_s16(a, mask), mask);
}

/* Saturating negate 16-bit integers in a when the corresponding signed 16-bit
integer in b is negative.
Notes:
  * Negating INT16_MIN results in INT16_MIN. However, this cannot occur in
  practice, as scaled_luma is the multiplication of two absolute values.
  * In the Intel equivalent, elements in a are zeroed out when the
  corresponding elements in b are zero. Because vsignq is used twice in a
  row, with b in the first call becoming a in the second call, there's no
  impact from not zeroing out. */
static inline int16x8_t vsignq_s16(int16x8_t a, int16x8_t b) {
    const int16x8_t mask = vshrq_n_s16(b, 15);
    return veorq_s16(vaddq_s16(a, mask), mask);
}

static inline int16x4_t predict_w4(const int16_t* pred_buf_q3, int16x4_t alpha_sign, int abs_alpha_q12, int16x4_t dc) {
    const int16x4_t ac_q3       = vld1_s16(pred_buf_q3);
    const int16x4_t ac_sign     = veor_s16(alpha_sign, ac_q3);
    int16x4_t       scaled_luma = vqrdmulh_n_s16(vabs_s16(ac_q3), abs_alpha_q12);
    return vadd_s16(vsign_s16(scaled_luma, ac_sign), dc);
}

static inline int16x8_t predict_w8(const int16_t* pred_buf_q3, int16x8_t alpha_sign, int abs_alpha_q12, int16x8_t dc) {
    const int16x8_t ac_q3       = vld1q_s16(pred_buf_q3);
    const int16x8_t ac_sign     = veorq_s16(alpha_sign, ac_q3);
    int16x8_t       scaled_luma = vqrdmulhq_n_s16(vabsq_s16(ac_q3), abs_alpha_q12);
    return vaddq_s16(vsignq_s16(scaled_luma, ac_sign), dc);
}

static inline int16x8x2_t predict_w16(const int16_t* pred_buf_q3, int16x8_t alpha_sign, int abs_alpha_q12,
                                      int16x8_t dc) {
    const int16x8x2_t ac_q3         = vld1q_s16_x2(pred_buf_q3);
    const int16x8_t   ac_sign_0     = veorq_s16(alpha_sign, ac_q3.val[0]);
    const int16x8_t   ac_sign_1     = veorq_s16(alpha_sign, ac_q3.val[1]);
    const int16x8_t   scaled_luma_0 = vqrdmulhq_n_s16(vabsq_s16(ac_q3.val[0]), abs_alpha_q12);
    const int16x8_t   scaled_luma_1 = vqrdmulhq_n_s16(vabsq_s16(ac_q3.val[1]), abs_alpha_q12);
    int16x8x2_t       result;
    result.val[0] = vaddq_s16(vsignq_s16(scaled_luma_0, ac_sign_0), dc);
    result.val[1] = vaddq_s16(vsignq_s16(scaled_luma_1, ac_sign_1), dc);
    return result;
}

static inline int16x8x4_t predict_w32(const int16_t* pred_buf_q3, int16x8_t alpha_sign, int abs_alpha_q12,
                                      int16x8_t dc) {
    const int16x8x4_t ac_q3         = vld1q_s16_x4(pred_buf_q3);
    const int16x8_t   ac_sign_0     = veorq_s16(alpha_sign, ac_q3.val[0]);
    const int16x8_t   ac_sign_1     = veorq_s16(alpha_sign, ac_q3.val[1]);
    const int16x8_t   ac_sign_2     = veorq_s16(alpha_sign, ac_q3.val[2]);
    const int16x8_t   ac_sign_3     = veorq_s16(alpha_sign, ac_q3.val[3]);
    const int16x8_t   scaled_luma_0 = vqrdmulhq_n_s16(vabsq_s16(ac_q3.val[0]), abs_alpha_q12);
    const int16x8_t   scaled_luma_1 = vqrdmulhq_n_s16(vabsq_s16(ac_q3.val[1]), abs_alpha_q12);
    const int16x8_t   scaled_luma_2 = vqrdmulhq_n_s16(vabsq_s16(ac_q3.val[2]), abs_alpha_q12);
    const int16x8_t   scaled_luma_3 = vqrdmulhq_n_s16(vabsq_s16(ac_q3.val[3]), abs_alpha_q12);
    int16x8x4_t       result;
    result.val[0] = vaddq_s16(vsignq_s16(scaled_luma_0, ac_sign_0), dc);
    result.val[1] = vaddq_s16(vsignq_s16(scaled_luma_1, ac_sign_1), dc);
    result.val[2] = vaddq_s16(vsignq_s16(scaled_luma_2, ac_sign_2), dc);
    result.val[3] = vaddq_s16(vsignq_s16(scaled_luma_3, ac_sign_3), dc);
    return result;
}

void svt_aom_cfl_predict_lbd_neon(const int16_t* pred_buf_q3, uint8_t* pred, int32_t pred_stride, uint8_t* dst,
                                  int32_t dst_stride, int32_t alpha_q3, int32_t bit_depth, int32_t width,
                                  int32_t height) {
    (void)bit_depth;
    (void)pred_stride;
    const int16_t        abs_alpha_q12 = abs(alpha_q3) << 9;
    const int16_t* const end           = pred_buf_q3 + height * CFL_BUF_LINE;
    if (width == 4) {
        const int16x4_t alpha_sign = vdup_n_s16(alpha_q3);
        const int16x4_t dc         = vdup_n_s16(*pred);
        do {
            const int16x4_t pred_vector = predict_w4(pred_buf_q3, alpha_sign, abs_alpha_q12, dc);
            vsth_u8(dst, vqmovun_s16(vcombine_s16(pred_vector, pred_vector)));
            dst += dst_stride;
        } while ((pred_buf_q3 += CFL_BUF_LINE) < end);
    } else {
        const int16x8_t alpha_sign = vdupq_n_s16(alpha_q3);
        const int16x8_t dc         = vdupq_n_s16(*pred);
        do {
            if (width == 8) {
                vst1_u8(dst, vqmovun_s16(predict_w8(pred_buf_q3, alpha_sign, abs_alpha_q12, dc)));
            } else if (width == 16) {
                const int16x8x2_t pred_vector = predict_w16(pred_buf_q3, alpha_sign, abs_alpha_q12, dc);
                const uint8x8x2_t predun      = {{vqmovun_s16(pred_vector.val[0]), vqmovun_s16(pred_vector.val[1])}};
                vst1_u8_x2(dst, predun);
            } else {
                const int16x8x4_t pred_vector = predict_w32(pred_buf_q3, alpha_sign, abs_alpha_q12, dc);
                const uint8x8x4_t predun      = {{vqmovun_s16(pred_vector.val[0]),
                                                  vqmovun_s16(pred_vector.val[1]),
                                                  vqmovun_s16(pred_vector.val[2]),
                                                  vqmovun_s16(pred_vector.val[3])}};
                vst1_u8_x4(dst, predun);
            }
            dst += dst_stride;
        } while ((pred_buf_q3 += CFL_BUF_LINE) < end);
    }
}

static inline uint16x4_t clamp_s16(int16x4_t a, int16x4_t max) {
    return vreinterpret_u16_s16(vmax_s16(vmin_s16(a, max), vdup_n_s16(0)));
}

static inline uint16x8_t clampq_s16(int16x8_t a, int16x8_t max) {
    return vreinterpretq_u16_s16(vmaxq_s16(vminq_s16(a, max), vdupq_n_s16(0)));
}

static inline uint16x8x2_t clamp2q_s16(int16x8x2_t a, int16x8_t max) {
    uint16x8x2_t result;
    result.val[0] = vreinterpretq_u16_s16(vmaxq_s16(vminq_s16(a.val[0], max), vdupq_n_s16(0)));
    result.val[1] = vreinterpretq_u16_s16(vmaxq_s16(vminq_s16(a.val[1], max), vdupq_n_s16(0)));
    return result;
}

static inline uint16x8x4_t clamp4q_s16(int16x8x4_t a, int16x8_t max) {
    uint16x8x4_t result;
    result.val[0] = vreinterpretq_u16_s16(vmaxq_s16(vminq_s16(a.val[0], max), vdupq_n_s16(0)));
    result.val[1] = vreinterpretq_u16_s16(vmaxq_s16(vminq_s16(a.val[1], max), vdupq_n_s16(0)));
    result.val[2] = vreinterpretq_u16_s16(vmaxq_s16(vminq_s16(a.val[2], max), vdupq_n_s16(0)));
    result.val[3] = vreinterpretq_u16_s16(vmaxq_s16(vminq_s16(a.val[3], max), vdupq_n_s16(0)));
    return result;
}

void svt_cfl_predict_hbd_neon(const int16_t* pred_buf_q3, uint16_t* pred, int pred_stride, uint16_t* dst,
                              int dst_stride, int alpha_q3, int bd, int width, int height) {
    (void)pred_stride;
    (void)bd; // bd is assumed to be 10.
    const int            max           = (1 << 10) - 1;
    const int16_t        abs_alpha_q12 = abs(alpha_q3) << 9;
    const int16_t* const end           = pred_buf_q3 + height * CFL_BUF_LINE;
    if (width == 4) {
        const int16x4_t alpha_sign = vdup_n_s16(alpha_q3);
        const int16x4_t dc         = vdup_n_s16(*pred);
        const int16x4_t max_16x4   = vdup_n_s16(max);
        do {
            const int16x4_t scaled_luma = predict_w4(pred_buf_q3, alpha_sign, abs_alpha_q12, dc);
            vst1_u16(dst, clamp_s16(scaled_luma, max_16x4));
            dst += dst_stride;
            pred_buf_q3 += CFL_BUF_LINE;
        } while (pred_buf_q3 < end);
    } else {
        const int16x8_t alpha_sign = vdupq_n_s16(alpha_q3);
        const int16x8_t dc         = vdupq_n_s16(*pred);
        const int16x8_t max_16x8   = vdupq_n_s16(max);
        do {
            if (width == 8) {
                const int16x8_t pred_v = predict_w8(pred_buf_q3, alpha_sign, abs_alpha_q12, dc);
                vst1q_u16(dst, clampq_s16(pred_v, max_16x8));
            } else if (width == 16) {
                const int16x8x2_t pred_v = predict_w16(pred_buf_q3, alpha_sign, abs_alpha_q12, dc);
                vst1q_u16_x2(dst, clamp2q_s16(pred_v, max_16x8));
            } else {
                const int16x8x4_t pred_v = predict_w32(pred_buf_q3, alpha_sign, abs_alpha_q12, dc);
                vst1q_u16_x4(dst, clamp4q_s16(pred_v, max_16x8));
            }
            dst += dst_stride;
            pred_buf_q3 += CFL_BUF_LINE;
        } while (pred_buf_q3 < end);
    }
}

void svt_cfl_luma_subsampling_420_lbd_neon(const uint8_t* input, int input_stride, int16_t* pred_buf_q3, int width,
                                           int height) {
    const int16_t* end         = pred_buf_q3 + (height >> 1) * CFL_BUF_LINE;
    const int      luma_stride = input_stride << 1;
    if (width == 4) {
        do {
            const uint8x8_t top = load_u8_4x2(input, luma_stride);
            const uint8x8_t bot = load_u8_4x2(input + input_stride, luma_stride);
            uint16x4_t      sum = vpaddl_u8(top);
            sum                 = vpadal_u8(sum, bot);
            sum                 = vadd_u16(sum, sum);

            store_s16x2_strided_x2(pred_buf_q3, CFL_BUF_LINE, vreinterpret_s16_u16(sum));

            input += 2 * luma_stride;
            pred_buf_q3 += 2 * CFL_BUF_LINE;
        } while (pred_buf_q3 < end);
    } else if (width == 8) {
        do {
            uint8x16_t top = load_u8_8x2(input, luma_stride);
            uint8x16_t bot = load_u8_8x2(input + input_stride, luma_stride);

            uint16x8_t sum = vpaddlq_u8(top);
            sum            = vpadalq_u8(sum, bot);
            sum            = vaddq_u16(sum, sum);

            store_s16x4_strided_x2(pred_buf_q3, CFL_BUF_LINE, vreinterpretq_s16_u16(sum));

            input += 2 * luma_stride;
            pred_buf_q3 += 2 * CFL_BUF_LINE;
        } while (pred_buf_q3 < end);
    } else if (width == 16) {
        do {
            const uint8x16_t top = vld1q_u8(input);
            const uint8x16_t bot = vld1q_u8(input + input_stride);

            uint16x8_t sum = vpaddlq_u8(top);
            sum            = vpadalq_u8(sum, bot);
            sum            = vaddq_u16(sum, sum);

            vst1q_s16(pred_buf_q3, vreinterpretq_s16_u16(sum));

            input += luma_stride;
            pred_buf_q3 += CFL_BUF_LINE;
        } while (pred_buf_q3 < end);
    } else {
        do {
            const uint8x16x2_t top = vld1q_u8_x2(input);
            const uint8x16x2_t bot = vld1q_u8_x2(input + input_stride);

            uint16x8_t sum0 = vpaddlq_u8(top.val[0]);
            uint16x8_t sum1 = vpaddlq_u8(top.val[1]);
            sum0            = vpadalq_u8(sum0, bot.val[0]);
            sum1            = vpadalq_u8(sum1, bot.val[1]);

            sum0 = vaddq_u16(sum0, sum0);
            sum1 = vaddq_u16(sum1, sum1);

            vst1q_s16(pred_buf_q3 + 0, vreinterpretq_s16_u16(sum0));
            vst1q_s16(pred_buf_q3 + 8, vreinterpretq_s16_u16(sum1));

            input += luma_stride;
            pred_buf_q3 += CFL_BUF_LINE;
        } while (pred_buf_q3 < end);
    }
}

void svt_cfl_luma_subsampling_420_hbd_neon(const uint16_t* input, int input_stride, int16_t* pred_buf_q3, int width,
                                           int height) {
    const int16_t* end         = pred_buf_q3 + (height >> 1) * CFL_BUF_LINE;
    const int      luma_stride = input_stride << 1;
    if (width == 4) {
        do {
            const uint16x8_t top = load_u16_4x2(input, luma_stride);
            const uint16x8_t bot = load_u16_4x2(input + input_stride, luma_stride);

            uint16x8_t sum = vaddq_u16(top, bot);
            sum            = vpaddq_u16(sum, sum);
            sum            = vaddq_u16(sum, sum);

            store_s16x2_strided_x2(pred_buf_q3, CFL_BUF_LINE, vget_low_s16(vreinterpretq_s16_u16(sum)));

            input += 2 * luma_stride;
            pred_buf_q3 += 2 * CFL_BUF_LINE;
        } while (pred_buf_q3 < end);
    } else if (width == 8) {
        do {
            uint16x8_t top0, top1, bot0, bot1;
            load_u16_8x2(input, luma_stride, &top0, &top1);
            load_u16_8x2(input + input_stride, luma_stride, &bot0, &bot1);

            uint16x8_t sum0  = vaddq_u16(top0, bot0);
            uint16x8_t sum1  = vaddq_u16(top1, bot1);
            uint16x8_t sum01 = vpaddq_u16(sum0, sum1);
            sum01            = vaddq_u16(sum01, sum01);

            store_s16x4_strided_x2(pred_buf_q3, CFL_BUF_LINE, vreinterpretq_s16_u16(sum01));

            input += 2 * luma_stride;
            pred_buf_q3 += 2 * CFL_BUF_LINE;
        } while (pred_buf_q3 < end);
    } else if (width == 16) {
        do {
            uint16x8_t top0, top1, bot0, bot1;
            load_u16_8x2(input + 0, input_stride, &top0, &bot0);
            load_u16_8x2(input + 8, input_stride, &top1, &bot1);

            uint16x8_t sum0  = vaddq_u16(top0, bot0);
            uint16x8_t sum1  = vaddq_u16(top1, bot1);
            uint16x8_t sum01 = vpaddq_u16(sum0, sum1);
            sum01            = vaddq_u16(sum01, sum01);

            vst1q_s16(pred_buf_q3, vreinterpretq_s16_u16(sum01));

            input += luma_stride;
            pred_buf_q3 += CFL_BUF_LINE;
        } while (pred_buf_q3 < end);
    } else if (width == 32) {
        do {
            uint16x8_t top[4], bot[4];
            load_u16_8x4(input, 8, &top[0], &top[1], &top[2], &top[3]);
            load_u16_8x4(input + input_stride, 8, &bot[0], &bot[1], &bot[2], &bot[3]);

            uint16x8_t sum0 = vaddq_u16(top[0], bot[0]);
            uint16x8_t sum1 = vaddq_u16(top[1], bot[1]);
            uint16x8_t sum2 = vaddq_u16(top[2], bot[2]);
            uint16x8_t sum3 = vaddq_u16(top[3], bot[3]);

            uint16x8_t sum01 = vpaddq_u16(sum0, sum1);
            uint16x8_t sum23 = vpaddq_u16(sum2, sum3);
            sum01            = vaddq_u16(sum01, sum01);
            sum23            = vaddq_u16(sum23, sum23);

            store_s16_8x2(pred_buf_q3, 8, vreinterpretq_s16_u16(sum01), vreinterpretq_s16_u16(sum23));

            input += luma_stride;
            pred_buf_q3 += CFL_BUF_LINE;
        } while (pred_buf_q3 < end);
    }
}

void svt_subtract_average_neon(int16_t* pred_buf_q3, int width, int height, int round_offset, const int num_pel_log2) {
    const uint16_t* end = (uint16_t*)pred_buf_q3 + height * CFL_BUF_LINE;

    // Round offset is not needed, because Neon will handle the rounding.
    (void)round_offset;

    // To optimize the use of the CPU pipeline, we process 4 rows per iteration
    const int step = 4 * CFL_BUF_LINE;

    // At this stage, the prediction buffer contains scaled reconstructed luma
    // pixels, which are positive integer and only require 15 bits. By using
    // unsigned integer for the sum, we can do one addition operation inside 16
    // bits (8 lanes) before having to convert to 32 bits (4 lanes).
    const uint16_t* sum_buf = (uint16_t*)pred_buf_q3;
    uint32x4_t      sum     = vdupq_n_u32(0);

    if (width == 4) {
        do {
            uint16x4_t a0, a1, a2, a3;
            load_u16_4x4(sum_buf, CFL_BUF_LINE, &a0, &a1, &a2, &a3);

            uint16x4_t a01 = vadd_u16(a0, a1);
            uint16x4_t a23 = vadd_u16(a2, a3);

            sum = vaddq_u32(sum, vaddl_u16(a01, a23));
            sum_buf += step;
        } while (sum_buf < end);
    } else if (width == 8) {
        do {
            uint16x8_t a0, a1, a2, a3;
            load_u16_8x4(sum_buf, CFL_BUF_LINE, &a0, &a1, &a2, &a3);

            uint16x8_t a01 = vaddq_u16(a0, a1);
            uint16x8_t a23 = vaddq_u16(a2, a3);

            sum = vpadalq_u16(sum, a01);
            sum = vpadalq_u16(sum, a23);
            sum_buf += step;
        } while (sum_buf < end);
    } else if (width == 16) {
        do {
            uint16x8_t a0, a1, a2, a3, a4, a5, a6, a7;
            load_u16_8x4(sum_buf + 0, CFL_BUF_LINE, &a0, &a1, &a2, &a3);
            load_u16_8x4(sum_buf + 8, CFL_BUF_LINE, &a4, &a5, &a6, &a7);

            uint16x8_t a01 = vaddq_u16(a0, a1);
            uint16x8_t a23 = vaddq_u16(a2, a3);
            uint16x8_t a45 = vaddq_u16(a4, a5);
            uint16x8_t a67 = vaddq_u16(a6, a7);

            sum = vpadalq_u16(sum, a01);
            sum = vpadalq_u16(sum, a23);
            sum = vpadalq_u16(sum, a45);
            sum = vpadalq_u16(sum, a67);

            sum_buf += step;
        } while (sum_buf < end);
    } else {
        uint32x4_t sum0 = vdupq_n_u32(0);
        uint32x4_t sum1 = vdupq_n_u32(0);
        do {
            uint16x8_t a[4], b[4], c[4], d[4];
            load_u16_8x4(sum_buf + 0, CFL_BUF_LINE, &a[0], &a[1], &a[2], &a[3]);
            load_u16_8x4(sum_buf + 8, CFL_BUF_LINE, &b[0], &b[1], &b[2], &b[3]);
            load_u16_8x4(sum_buf + 16, CFL_BUF_LINE, &c[0], &c[1], &c[2], &c[3]);
            load_u16_8x4(sum_buf + 24, CFL_BUF_LINE, &d[0], &d[1], &d[2], &d[3]);

            uint16x8_t a01 = vaddq_u16(a[0], a[1]);
            uint16x8_t a23 = vaddq_u16(a[2], a[3]);
            uint16x8_t b01 = vaddq_u16(b[0], b[1]);
            uint16x8_t b23 = vaddq_u16(b[2], b[3]);
            uint16x8_t c01 = vaddq_u16(c[0], c[1]);
            uint16x8_t c23 = vaddq_u16(c[2], c[3]);
            uint16x8_t d01 = vaddq_u16(d[0], d[1]);
            uint16x8_t d23 = vaddq_u16(d[2], d[3]);

            sum0 = vpadalq_u16(sum0, a01);
            sum0 = vpadalq_u16(sum0, b01);
            sum0 = vpadalq_u16(sum0, c01);
            sum0 = vpadalq_u16(sum0, d01);

            sum1 = vpadalq_u16(sum1, a23);
            sum1 = vpadalq_u16(sum1, b23);
            sum1 = vpadalq_u16(sum1, c23);
            sum1 = vpadalq_u16(sum1, d23);

            sum_buf += step;
        } while (sum_buf < end);
        sum = vaddq_u32(sum0, sum1);
    }

    // Permute and add in such a way that each lane contains the block sum.
    // [A+C+B+D, B+D+A+C, C+A+D+B, D+B+C+A]
    sum = vpaddq_u32(sum, sum);
    sum = vpaddq_u32(sum, sum);

    // Computing the average could be done using scalars, but getting off the Neon
    // engine introduces latency, so we use vrshlq_u32.
    uint32x4_t avg_32   = vrshlq_u32(sum, vdupq_n_s32(-num_pel_log2));
    int16x4_t  avg_16x4 = vreinterpret_s16_u16(vmovn_u32(avg_32));

    if (width == 4) {
        do {
            int16x4_t a0 = vld1_s16(pred_buf_q3);
            vst1_s16(pred_buf_q3, vsub_s16(a0, avg_16x4));

            pred_buf_q3 += CFL_BUF_LINE;
        } while ((uint16_t*)pred_buf_q3 < end);
    } else if (width == 8) {
        const int16x8_t avg_16x8 = vcombine_s16(avg_16x4, avg_16x4);
        do {
            int16x8_t a0, a1, a2, a3;
            load_s16_8x4(pred_buf_q3, CFL_BUF_LINE, &a0, &a1, &a2, &a3);

            int16x8_t a0_avg = vsubq_s16(a0, avg_16x8);
            int16x8_t a1_avg = vsubq_s16(a1, avg_16x8);
            int16x8_t a2_avg = vsubq_s16(a2, avg_16x8);
            int16x8_t a3_avg = vsubq_s16(a3, avg_16x8);

            store_s16_8x4(pred_buf_q3, CFL_BUF_LINE, a0_avg, a1_avg, a2_avg, a3_avg);

            pred_buf_q3 += step;
        } while ((uint16_t*)pred_buf_q3 < end);
    } else if (width == 16) {
        const int16x8_t avg_16x8 = vcombine_s16(avg_16x4, avg_16x4);
        do {
            int16x8_t a0, a1, a2, a3;
            load_s16_8x4(pred_buf_q3, CFL_BUF_LINE, &a0, &a1, &a2, &a3);

            int16x8_t a0_avg = vsubq_s16(a0, avg_16x8);
            int16x8_t a1_avg = vsubq_s16(a1, avg_16x8);
            int16x8_t a2_avg = vsubq_s16(a2, avg_16x8);
            int16x8_t a3_avg = vsubq_s16(a3, avg_16x8);

            store_s16_8x4(pred_buf_q3, CFL_BUF_LINE, a0_avg, a1_avg, a2_avg, a3_avg);

            int16x8_t a4, a5, a6, a7;
            load_s16_8x4(pred_buf_q3 + 8, CFL_BUF_LINE, &a4, &a5, &a6, &a7);

            int16x8_t a4_avg = vsubq_s16(a4, avg_16x8);
            int16x8_t a5_avg = vsubq_s16(a5, avg_16x8);
            int16x8_t a6_avg = vsubq_s16(a6, avg_16x8);
            int16x8_t a7_avg = vsubq_s16(a7, avg_16x8);

            store_s16_8x4(pred_buf_q3 + 8, CFL_BUF_LINE, a4_avg, a5_avg, a6_avg, a7_avg);
            pred_buf_q3 += step;
        } while ((uint16_t*)pred_buf_q3 < end);
    } else if (width == 32) {
        const int16x8_t avg_16x8 = vcombine_s16(avg_16x4, avg_16x4);
        do {
            int16x8_t a0, a1, a2, a3;
            load_s16_8x4(pred_buf_q3, CFL_BUF_LINE, &a0, &a1, &a2, &a3);

            int16x8_t a0_avg = vsubq_s16(a0, avg_16x8);
            int16x8_t a1_avg = vsubq_s16(a1, avg_16x8);
            int16x8_t a2_avg = vsubq_s16(a2, avg_16x8);
            int16x8_t a3_avg = vsubq_s16(a3, avg_16x8);

            store_s16_8x4(pred_buf_q3, CFL_BUF_LINE, a0_avg, a1_avg, a2_avg, a3_avg);

            int16x8_t a4, a5, a6, a7;
            load_s16_8x4(pred_buf_q3 + 8, CFL_BUF_LINE, &a4, &a5, &a6, &a7);

            int16x8_t a4_avg = vsubq_s16(a4, avg_16x8);
            int16x8_t a5_avg = vsubq_s16(a5, avg_16x8);
            int16x8_t a6_avg = vsubq_s16(a6, avg_16x8);
            int16x8_t a7_avg = vsubq_s16(a7, avg_16x8);

            store_s16_8x4(pred_buf_q3 + 8, CFL_BUF_LINE, a4_avg, a5_avg, a6_avg, a7_avg);

            int16x8_t a8, a9, a10, a11;
            load_s16_8x4(pred_buf_q3 + 16, CFL_BUF_LINE, &a8, &a9, &a10, &a11);

            int16x8_t a8_avg  = vsubq_s16(a8, avg_16x8);
            int16x8_t a9_avg  = vsubq_s16(a9, avg_16x8);
            int16x8_t a10_avg = vsubq_s16(a10, avg_16x8);
            int16x8_t a11_avg = vsubq_s16(a11, avg_16x8);

            store_s16_8x4(pred_buf_q3 + 16, CFL_BUF_LINE, a8_avg, a9_avg, a10_avg, a11_avg);

            int16x8_t a12, a13, a14, a15;
            load_s16_8x4(pred_buf_q3 + 24, CFL_BUF_LINE, &a12, &a13, &a14, &a15);

            int16x8_t a12_avg = vsubq_s16(a12, avg_16x8);
            int16x8_t a13_avg = vsubq_s16(a13, avg_16x8);
            int16x8_t a14_avg = vsubq_s16(a14, avg_16x8);
            int16x8_t a15_avg = vsubq_s16(a15, avg_16x8);

            store_s16_8x4(pred_buf_q3 + 24, CFL_BUF_LINE, a12_avg, a13_avg, a14_avg, a15_avg);
            pred_buf_q3 += step;
        } while ((uint16_t*)pred_buf_q3 < end);
    }
}
