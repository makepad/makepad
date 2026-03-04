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
#include <assert.h>
#include "definitions.h"
#include "mem_neon.h"
#include "sum_neon.h"

#include "intra_prediction.h"

/* ---------------------P R E D I C T I O N   Z 1--------------------------- */
static inline void dr_prediction_z1_WxH_internal_neon_small(int W, int H, uint8x8_t* dst, const uint8_t* above,
                                                            int upsample_above, int dx) {
    const int frac_bits  = 6 - upsample_above;
    const int max_base_x = ((W + H) - 1) << upsample_above;

    assert(dx > 0);
    //   above0 * (32 - shift) + above1 * shift
    // = (above1 - above0) * shift + above0 * 32

    const uint8x8_t a_mbase_x = vdup_n_u8(above[max_base_x]);

    int x = dx;
    for (int r = 0; r < H; r++) {
        int base          = x >> frac_bits;
        int base_max_diff = (max_base_x - base) >> upsample_above;
        if (base_max_diff <= 0) {
            for (int i = r; i < H; ++i) {
                dst[i] = a_mbase_x; // save 4 values
            }
            return;
        }

        if (base_max_diff > W) {
            base_max_diff = W;
        }

        uint8x8x2_t a01_128;
        uint16x8_t  shift;
        if (upsample_above) {
            a01_128 = vld2_u8(above + base);
            shift   = vdupq_n_u16(x & 0x1f); // ((x << upsample_above) & 0x3f) >> 1
        } else {
            a01_128.val[0] = vld1_u8(above + base);
            a01_128.val[1] = vld1_u8(above + base + 1);
            shift          = vdupq_n_u16((x & 0x3f) >> 1);
        }

        uint16x8_t diff = vsubl_u8(a01_128.val[1], a01_128.val[0]);
        uint16x8_t a32  = vshll_n_u8(a01_128.val[0], 5);
        uint16x8_t res  = vmlaq_u16(a32, diff, shift);

        uint8x8_t mask = vld1_u8(base_mask[base_max_diff]);
        dst[r]         = vbsl_u8(mask, vrshrn_n_u16(res, 5), a_mbase_x);

        x += dx;
    }
}

static inline void dr_prediction_z1_4xH_neon(int H, uint8_t* dst, ptrdiff_t stride, const uint8_t* above,
                                             int upsample_above, int dx) {
    uint8x8_t dstvec[16];

    dr_prediction_z1_WxH_internal_neon_small(4, H, dstvec, above, upsample_above, dx);
    for (int i = 0; i < H; i++) {
        vst1_lane_u32((uint32_t*)(dst + stride * i), vreinterpret_u32_u8(dstvec[i]), 0);
    }
}

static inline void dr_prediction_z1_8xH_neon(int H, uint8_t* dst, ptrdiff_t stride, const uint8_t* above,
                                             int upsample_above, int dx) {
    uint8x8_t dstvec[32];

    dr_prediction_z1_WxH_internal_neon_small(8, H, dstvec, above, upsample_above, dx);
    for (int i = 0; i < H; i++) {
        vst1_u8(dst + stride * i, dstvec[i]);
    }
}

static inline void dr_prediction_z1_WxH_internal_neon_large(int W, int H, uint8x16_t* dst, const uint8_t* above,
                                                            int dx) {
    // Here upsample_above is 0 by design of svt_aom_use_intra_edge_upsample
    // defined in Source/Lib/Codec/intra_prediction.c.
    const int frac_bits  = 6;
    const int max_base_x = (W + H) - 1;

    assert(dx > 0);
    //   above0 * (32 - shift) + above1 * shift
    // = (above1 - above0) * shift + above0 * 32

    const uint8x16_t a_mbase_x = vdupq_n_u8(above[max_base_x]);

    int x = dx;
    for (int r = 0; r < H; r++) {
        int base          = x >> frac_bits;
        int base_max_diff = max_base_x - base;
        if (base_max_diff <= 0) {
            for (int i = r; i < H; ++i) {
                dst[i] = a_mbase_x; // save 4 values
            }
            return;
        }

        if (base_max_diff > W) {
            base_max_diff = W;
        }

        uint8x16_t a0_128 = vld1q_u8(above + base);
        uint8x16_t a1_128 = vld1q_u8(above + base + 1);
        uint16x8_t shift  = vdupq_n_u16((x & 0x3f) >> 1);

        uint16x8x2_t diff, a32, res;
        diff.val[0]       = vsubl_u8(vget_low_u8(a1_128), vget_low_u8(a0_128));
        diff.val[1]       = vsubl_u8(vget_high_u8(a1_128), vget_high_u8(a0_128));
        a32.val[0]        = vshll_n_u8(vget_low_u8(a0_128), 5);
        a32.val[1]        = vshll_n_u8(vget_high_u8(a0_128), 5);
        res.val[0]        = vmlaq_u16(a32.val[0], diff.val[0], shift);
        res.val[1]        = vmlaq_u16(a32.val[1], diff.val[1], shift);
        uint8x16_t v_temp = vcombine_u8(vrshrn_n_u16(res.val[0], 5), vrshrn_n_u16(res.val[1], 5));

        uint8x16_t mask = vld1q_u8(base_mask[base_max_diff]);
        dst[r]          = vbslq_u8(mask, v_temp, a_mbase_x);

        x += dx;
    }
}

static inline void dr_prediction_z1_16xH_neon(int H, uint8_t* dst, ptrdiff_t stride, const uint8_t* above, int dx) {
    uint8x16_t dstvec[64];

    dr_prediction_z1_WxH_internal_neon_large(16, H, dstvec, above, dx);
    for (int i = 0; i < H; i++) {
        vst1q_u8(dst + stride * i, dstvec[i]);
    }
}

static inline void dr_prediction_z1_32xH_internal_neon(int H, uint8x16x2_t* dstvec, const uint8_t* above, int dx) {
    // Here upsample_above is 0 by design of svt_aom_use_intra_edge_upsample
    // defined in Source/Lib/Codec/intra_prediction.c.
    const int frac_bits  = 6;
    const int max_base_x = ((32 + H) - 1);

    //   above0 * (32 - shift) + above1 * shift
    // = (above1 - above0) * shift + above0 * 32

    const uint8x16_t a_mbase_x = vdupq_n_u8(above[max_base_x]);

    int x = dx;
    for (int r = 0; r < H; r++) {
        uint8x16_t res16[2];

        int base          = x >> frac_bits;
        int base_max_diff = (max_base_x - base);
        if (base_max_diff <= 0) {
            for (int i = r; i < H; ++i) {
                dstvec[i].val[0] = a_mbase_x; // save 32 values
                dstvec[i].val[1] = a_mbase_x;
            }
            return;
        }
        if (base_max_diff > 32) {
            base_max_diff = 32;
        }

        uint16x8_t shift = vdupq_n_u16((x & 0x3f) >> 1);

        for (int j = 0, jj = 0; j < 32; j += 16, jj++) {
            int mdiff = base_max_diff - j;
            if (mdiff <= 0) {
                res16[jj] = a_mbase_x;
            } else {
                uint8x16_t a0_128 = vld1q_u8(above + base + j);
                uint8x16_t a1_128 = vld1q_u8(above + base + j + 1);

                uint16x8x2_t a32, diff, res;
                diff.val[0] = vsubl_u8(vget_low_u8(a1_128), vget_low_u8(a0_128));
                diff.val[1] = vsubl_u8(vget_high_u8(a1_128), vget_high_u8(a0_128));
                a32.val[0]  = vshll_n_u8(vget_low_u8(a0_128), 5);
                a32.val[1]  = vshll_n_u8(vget_high_u8(a0_128), 5);

                res.val[0] = vmlaq_u16(a32.val[0], diff.val[0], shift);
                res.val[1] = vmlaq_u16(a32.val[1], diff.val[1], shift);

                res16[jj] = vcombine_u8(vrshrn_n_u16(res.val[0], 5), vrshrn_n_u16(res.val[1], 5));
            }
        }

        uint8x16x2_t mask;

        mask.val[0]      = vld1q_u8(base_mask[base_max_diff]);
        mask.val[1]      = vld1q_u8(base_mask[base_max_diff] + 16);
        dstvec[r].val[0] = vbslq_u8(mask.val[0], res16[0], a_mbase_x);
        dstvec[r].val[1] = vbslq_u8(mask.val[1], res16[1], a_mbase_x);
        x += dx;
    }
}

static inline void dr_prediction_z1_32xH_neon(int H, uint8_t* dst, ptrdiff_t stride, const uint8_t* above, int dx) {
    uint8x16x2_t dstvec[64];
    dr_prediction_z1_32xH_internal_neon(H, dstvec, above, dx);
    for (int i = 0; i < H; i++) {
        vst1q_u8(dst + stride * i, dstvec[i].val[0]);
        vst1q_u8(dst + stride * i + 16, dstvec[i].val[1]);
    }
}

static inline void dr_prediction_z1_64xH_neon(int H, uint8_t* dst, ptrdiff_t stride, const uint8_t* above, int dx) {
    // Here upsample_above is 0 by design of svt_aom_use_intra_edge_upsample
    // defined in Source/Lib/Codec/intra_prediction.c.
    const int frac_bits  = 6;
    const int max_base_x = (64 + H) - 1;

    //   above0 * (32 - shift) + above1 * shift
    // = (above1 - above0) * shift + above0 * 32

    const uint8x16_t a_mbase_x = vdupq_n_u8(above[max_base_x]);

    int x = dx;
    for (int r = 0; r < H; r++, dst += stride, x += dx) {
        int base = x >> frac_bits;
        if (base >= max_base_x) {
            for (int i = r; i < H; ++i) {
                vst1q_u8(dst, a_mbase_x);
                vst1q_u8(dst + 16, a_mbase_x);
                vst1q_u8(dst + 32, a_mbase_x);
                vst1q_u8(dst + 48, a_mbase_x);
                dst += stride;
            }
            return;
        }

        uint16x8_t shift = vdupq_n_u16((x & 0x3f) >> 1);

        for (int j = 0; j < 64; j += 16) {
            int mdif = max_base_x - (base + j);
            if (mdif <= 0) {
                vst1q_u8(dst + j, a_mbase_x);
            } else {
                uint8x16_t a0_128 = vld1q_u8(above + base + j);
                uint8x16_t a1_128 = vld1q_u8(above + base + 1 + j);

                uint16x8x2_t a32, diff, res;
                diff.val[0]       = vsubl_u8(vget_low_u8(a1_128), vget_low_u8(a0_128));
                diff.val[1]       = vsubl_u8(vget_high_u8(a1_128), vget_high_u8(a0_128));
                a32.val[0]        = vshll_n_u8(vget_low_u8(a0_128), 5);
                a32.val[1]        = vshll_n_u8(vget_high_u8(a0_128), 5);
                res.val[0]        = vmlaq_u16(a32.val[0], diff.val[0], shift);
                res.val[1]        = vmlaq_u16(a32.val[1], diff.val[1], shift);
                uint8x16_t v_temp = vcombine_u8(vrshrn_n_u16(res.val[0], 5), vrshrn_n_u16(res.val[1], 5));

                mdif = mdif > 16 ? 16 : mdif;

                uint8x16_t mask   = vld1q_u8(base_mask[mdif]);
                uint8x16_t res128 = vbslq_u8(mask, v_temp, a_mbase_x);

                vst1q_u8(dst + j, res128);
            }
        }
    }
}

// Directional prediction, zone 1: 0 < angle < 90
void svt_av1_dr_prediction_z1_neon(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                                   const uint8_t* left, int32_t upsample_above, int32_t dx, int32_t dy) {
    (void)left;
    (void)dy;

    switch (bw) {
    case 4: {
        dr_prediction_z1_4xH_neon(bh, dst, stride, above, upsample_above, dx);
        break;
    }
    case 8: {
        dr_prediction_z1_8xH_neon(bh, dst, stride, above, upsample_above, dx);
        break;
    }
    case 16: {
        dr_prediction_z1_16xH_neon(bh, dst, stride, above, dx);
        break;
    }
    case 32: {
        dr_prediction_z1_32xH_neon(bh, dst, stride, above, dx);
        break;
    }
    case 64: {
        dr_prediction_z1_64xH_neon(bh, dst, stride, above, dx);
        break;
    }
    }
}

/* ---------------------P R E D I C T I O N   Z 2--------------------------- */
static inline void dr_prediction_z2_4xH_neon(int H, uint8_t* dst, ptrdiff_t stride, const uint8_t* above,
                                             const uint8_t* left, int upsample_above, int upsample_left, int dx,
                                             int dy) {
    const int min_base_x  = -(1 << upsample_above);
    const int frac_bits_x = 6 - upsample_above;
    const int frac_bits_y = 6 - upsample_left;

    assert(dx > 0);

    //   above0 * (32 - shift) + above1 * shift
    // = (above1 - above0) * shift + above0 * 32
    uint8x8_t  v_zero_u8     = vdup_n_u8(0);
    uint16x4_t v_c1f         = vdup_n_u16(0x1f);
    int16x4_t  v_1234        = vcreate_s16(0x0004000300020001);
    int16x4_t  dy64          = vdup_n_s16(dy);
    int16x4_t  v_frac_bits_y = vdup_n_s16(-frac_bits_y);

    // Use ext rather than loading left + 14 directly to avoid over-read.
    const uint8x16_t   left_m2   = vld1q_u8(left - 2);
    const uint8x16_t   left_0    = vld1q_u8(left);
    const uint8x16_t   left_14   = vextq_u8(left_0, left_0, 14);
    const uint8x16x2_t left_vals = {{left_m2, left_14}};

    for (int r = 0; r < H; r++) {
        uint16x4x2_t v_shift    = {{vdup_n_u16(0), vdup_n_u16(0)}};
        int          y          = r + 1;
        int          x          = -y * dx;
        int          base_x     = x >> frac_bits_x;
        int          base_shift = 0;
        if (base_x < (min_base_x - 1)) {
            base_shift = (min_base_x - base_x - 1) >> upsample_above;
        }
        int base_min_diff = (min_base_x - base_x + upsample_above) >> upsample_above;
        if (base_min_diff > 4) {
            base_min_diff = 4;
        } else {
            if (base_min_diff < 0) {
                base_min_diff = 0;
            }
        }

        uint16x8_t a0_x, a1_x;

        if (base_shift <= 4) {
            if (upsample_above) {
                uint8x8x2_t a01_128 = vld2_u8(above + base_x);

                a0_x           = vmovl_u8(a01_128.val[0]);
                a1_x           = vmovl_u8(a01_128.val[1]);
                v_shift.val[0] = vdup_n_u16(x & 0x1f); // ((x << upsample_above) & 0x3f) >> 1
            } else {
                a0_x           = vmovl_u8(vld1_u8(above + base_x));
                a1_x           = vmovl_u8(vld1_u8(above + base_x + 1));
                v_shift.val[0] = vdup_n_u16((x & 0x3f) >> 1);
            }
        } else {
            a0_x = vdupq_n_u16(0);
            a1_x = vdupq_n_u16(0);
        }

        // y calc
        if (base_x < min_base_x) {
            int16x4_t v_r6       = vdup_n_s16(r << 6);
            int16x4_t y_c64      = vmls_s16(v_r6, v_1234, dy64);
            int16x4_t base_y_c64 = vshl_s16(y_c64, v_frac_bits_y);

            uint8x8_t left_idx0 = vreinterpret_u8_s16(vadd_s16(base_y_c64, vdup_n_s16(2))); // [0, 16]
            uint8x8_t left_idx1 = vreinterpret_u8_s16(vadd_s16(base_y_c64, vdup_n_s16(3))); // [1, 17]

            uint8x8_t a0_y = vtrn1_u8(vqtbl2_u8(left_vals, left_idx0), v_zero_u8);
            uint8x8_t a1_y = vtrn1_u8(vqtbl2_u8(left_vals, left_idx1), v_zero_u8);

            if (upsample_left) {
                v_shift.val[1] = vand_u16(vreinterpret_u16_s16(y_c64), v_c1f); // ((y << upsample_left) & 0x3f) >> 1
            } else {
                v_shift.val[1] = vand_u16(vshr_n_u16(vreinterpret_u16_s16(y_c64), 1), v_c1f); // (y & 0x3f) >> 1
            }

            a0_x = vcombine_u16(vget_low_u16(a0_x), vreinterpret_u16_u8(a0_y));
            a1_x = vcombine_u16(vget_low_u16(a1_x), vreinterpret_u16_u8(a1_y));
        }

        uint16x8_t shift = vcombine_u16(v_shift.val[0], v_shift.val[1]);
        uint16x8_t diff  = vsubq_u16(a1_x, a0_x); // a[x+1] - a[x]
        uint16x8_t a32   = vshlq_n_u16(a0_x, 5); // a[x] * 32

        uint16x8_t res  = vmlaq_u16(a32, diff, shift);
        uint8x8_t  resx = vrshrn_n_u16(res, 5);
        uint8x8_t  resy = vext_u8(resx, v_zero_u8, 4);

        uint8x8_t mask    = vld1_u8(base_mask[base_min_diff]);
        uint8x8_t v_resxy = vbsl_u8(mask, resy, resx);
        vst1_lane_u32((uint32_t*)dst, vreinterpret_u32_u8(v_resxy), 0);

        dst += stride;
    }
}

static inline void dr_prediction_z2_8xH_neon(int H, uint8_t* dst, ptrdiff_t stride, const uint8_t* above,
                                             const uint8_t* left, int upsample_above, int upsample_left, int dx,
                                             int dy) {
    const int min_base_x  = -(1 << upsample_above);
    const int frac_bits_x = 6 - upsample_above;
    const int frac_bits_y = 6 - upsample_left;

    //   above0 * (32 - shift) + above1 * shift
    // = (above1 - above0) * shift + above0 * 32

    uint16x8x2_t diff, a32;
    int16x8_t    v_frac_bits_y = vdupq_n_s16(-frac_bits_y);

    uint16x8_t c1f   = vdupq_n_u16(0x1f);
    int16x8_t  dy128 = vdupq_n_s16(dy);
    uint16x8_t c1234 = vcombine_u16(vcreate_u16(0x0004000300020001), vcreate_u16(0x0008000700060005));

    // Use ext rather than loading left + 30 directly to avoid over-read.
    const uint8x16_t   left_m2   = vld1q_u8(left - 2);
    const uint8x16_t   left_0    = vld1q_u8(left + 0);
    const uint8x16_t   left_16   = vld1q_u8(left + 16);
    const uint8x16_t   left_14   = vextq_u8(left_0, left_16, 14);
    const uint8x16_t   left_30   = vextq_u8(left_16, left_16, 14);
    const uint8x16x3_t left_vals = {{left_m2, left_14, left_30}};

    for (int r = 0; r < H; r++) {
        uint8x8_t    resx, resy, resxy;
        uint16x8x2_t res, shift;
        shift.val[1] = vdupq_n_u16(0);

        int y          = r + 1;
        int x          = -y * dx;
        int base_x     = x >> frac_bits_x;
        int base_shift = 0;
        if (base_x < (min_base_x - 1)) {
            base_shift = (min_base_x - base_x - 1) >> upsample_above;
        }
        int base_min_diff = (min_base_x - base_x + upsample_above) >> upsample_above;
        if (base_min_diff > 8) {
            base_min_diff = 8;
        } else {
            if (base_min_diff < 0) {
                base_min_diff = 0;
            }
        }

        if (base_shift <= 8) {
            uint8x8_t a0_x0, a1_x0;
            if (upsample_above) {
                uint8x8x2_t a01_128 = vld2_u8(above + base_x);

                a0_x0        = a01_128.val[0];
                a1_x0        = a01_128.val[1];
                shift.val[0] = vdupq_n_u16(x & 0x1f); // ((x << upsample_above) & 0x3f) >> 1
            } else {
                a0_x0        = vld1_u8(above + base_x);
                a1_x0        = vld1_u8(above + base_x + 1);
                shift.val[0] = vdupq_n_u16((x & 0x3f) >> 1);
            }

            diff.val[0] = vsubl_u8(a1_x0, a0_x0); // a[x+1] - a[x]
            a32.val[0]  = vshll_n_u8(a0_x0, 5);
            res.val[0]  = vmlaq_u16(a32.val[0], diff.val[0], shift.val[0]);
            resx        = vrshrn_n_u16(res.val[0], 5);
        } else {
            resx = vdup_n_u8(0);
        }

        // y calc
        if (base_x < min_base_x) {
            int16x8_t v_r6 = vdupq_n_s16(r << 6);

            int16x8_t y_c128      = vmlsq_s16(v_r6, vreinterpretq_s16_u16(c1234), dy128);
            int16x8_t base_y_c128 = vshlq_s16(y_c128, v_frac_bits_y);

            uint8x16_t left_idx0  = vreinterpretq_u8_s16(vaddq_s16(base_y_c128, vdupq_n_s16(2))); // [0, 33]
            uint8x16_t left_idx1  = vreinterpretq_u8_s16(vaddq_s16(base_y_c128, vdupq_n_s16(3))); // [1, 34]
            uint8x16_t left_idx01 = vuzp1q_u8(left_idx0, left_idx1);

            uint8x16_t a01_x = vqtbl3q_u8(left_vals, left_idx01);
            uint8x8_t  a0_x1 = vget_low_u8(a01_x);
            uint8x8_t  a1_x1 = vget_high_u8(a01_x);

            if (upsample_left) {
                shift.val[1] = vandq_u16(vreinterpretq_u16_s16(y_c128), c1f); // ((y << upsample_left) & 0x3f) >> 1
            } else {
                shift.val[1] = vandq_u16(vshrq_n_u16(vreinterpretq_u16_s16(y_c128), 1), c1f); // (y & 0x3f) >> 1
            }

            diff.val[1] = vsubl_u8(a1_x1, a0_x1);
            a32.val[1]  = vshll_n_u8(a0_x1, 5);
            res.val[1]  = vmlaq_u16(a32.val[1], diff.val[1], shift.val[1]);
            resy        = vrshrn_n_u16(res.val[1], 5);

            uint8x8_t mask = vld1_u8(base_mask[base_min_diff]);
            resxy          = vbsl_u8(mask, resy, resx);
            vst1_u8(dst, resxy);
        } else {
            vst1_u8(dst, resx);
        }

        dst += stride;
    }
}

static inline void dr_prediction_z2_WxH_neon(int W, int H, uint8_t* dst, ptrdiff_t stride, const uint8_t* above,
                                             const uint8_t* left, int dx, int dy) {
    // here upsample_above and upsample_left are 0 by design of
    // av1_use_intra_edge_upsample
    const int min_base_x  = -1;
    const int frac_bits_x = 6;
    const int frac_bits_y = 6;

    uint16x8x2_t a32, c1234, diff, shifty;
    uint8x16_t   v_zero        = vdupq_n_u8(0);
    int16x8_t    v_frac_bits_y = vdupq_n_s16(-frac_bits_y);

    uint16x8_t c3f   = vdupq_n_u16(0x3f);
    int16x8_t  dy256 = vdupq_n_s16(dy);
    c1234.val[0]     = vcombine_u16(vcreate_u16(0x0004000300020001), vcreate_u16(0x0008000700060005));
    c1234.val[1]     = vcombine_u16(vcreate_u16(0x000C000B000A0009), vcreate_u16(0x0010000F000E000D));

    const uint8x16_t   left_m1    = vld1q_u8(left - 1);
    const uint8x16_t   left_0     = vld1q_u8(left + 0);
    const uint8x16_t   left_16    = vld1q_u8(left + 16);
    const uint8x16_t   left_32    = vld1q_u8(left + 32);
    const uint8x16_t   left_48    = vld1q_u8(left + 48);
    const uint8x16_t   left_15    = vextq_u8(left_0, left_16, 15);
    const uint8x16_t   left_31    = vextq_u8(left_16, left_32, 15);
    const uint8x16_t   left_47    = vextq_u8(left_32, left_48, 15);
    const uint8x16x4_t left_vals0 = {{left_m1, left_15, left_31, left_47}};
    const uint8x16x4_t left_vals1 = {{left_0, left_16, left_32, left_48}};

    for (int r = 0; r < H; r++) {
        uint16x8x2_t res, shift;
        uint8x16_t   resx, resy, resxy;
        int          y      = r + 1;
        int          x      = -y * dx;
        int          base_x = x >> frac_bits_x;

        for (int j = 0; j < W; j += 16) {
            uint16x8_t j256 = vdupq_n_u16(j);

            int base_shift = 0;
            if ((base_x + j) < (min_base_x - 1)) {
                base_shift = (min_base_x - (base_x + j) - 1);
            }
            int base_min_diff = (min_base_x - base_x - j);
            if (base_min_diff > 16) {
                base_min_diff = 16;
            } else {
                if (base_min_diff < 0) {
                    base_min_diff = 0;
                }
            }

            if (base_shift < 16) {
                uint8x16_t a0_x128 = vld1q_u8(above + base_x + j);
                uint8x16_t a1_x128 = vld1q_u8(above + base_x + 1 + j);

                shift.val[0] = vdupq_n_u16((x & 0x3f) >> 1);
                shift.val[1] = vdupq_n_u16((x & 0x3f) >> 1);

                diff.val[0] = vsubl_u8(vget_low_u8(a1_x128), vget_low_u8(a0_x128)); // a[x+1] - a[x]
                diff.val[1] = vsubl_u8(vget_high_u8(a1_x128), vget_high_u8(a0_x128)); // a[x+1] - a[x]
                a32.val[0]  = vshll_n_u8(vget_low_u8(a0_x128), 5); // a[x] * 32
                a32.val[1]  = vshll_n_u8(vget_high_u8(a0_x128), 5); // a[x] * 32

                res.val[0] = vmlaq_u16(a32.val[0], diff.val[0], shift.val[0]);
                res.val[1] = vmlaq_u16(a32.val[1], diff.val[1], shift.val[1]);

                resx = vcombine_u8(vrshrn_n_u16(res.val[0], 5), vrshrn_n_u16(res.val[1], 5));
            } else {
                resx = v_zero;
            }

            // y calc
            if (base_x < min_base_x) {
                int16x8x2_t c256, y_c256, base_y_c256;
                int16x8_t   v_r6 = vdupq_n_s16(r << 6);

                c256.val[0] = vaddq_s16(vreinterpretq_s16_u16(j256), vreinterpretq_s16_u16(c1234.val[0]));
                c256.val[1] = vaddq_s16(vreinterpretq_s16_u16(j256), vreinterpretq_s16_u16(c1234.val[1]));

                y_c256.val[0] = vmlsq_s16(v_r6, c256.val[0], dy256);
                y_c256.val[1] = vmlsq_s16(v_r6, c256.val[1], dy256);

                base_y_c256.val[0] = vshlq_s16(y_c256.val[0], v_frac_bits_y);
                base_y_c256.val[1] = vshlq_s16(y_c256.val[1], v_frac_bits_y);

                // Values in left_idx{0,1} range from 0 through 63 inclusive.
                uint8x16_t left_idx0  = vreinterpretq_u8_s16(vaddq_s16(base_y_c256.val[0], vdupq_n_s16(1)));
                uint8x16_t left_idx1  = vreinterpretq_u8_s16(vaddq_s16(base_y_c256.val[1], vdupq_n_s16(1)));
                uint8x16_t left_idx01 = vuzp1q_u8(left_idx0, left_idx1);

                uint8x16_t a0_y01 = vqtbl4q_u8(left_vals0, left_idx01);
                uint8x16_t a1_y01 = vqtbl4q_u8(left_vals1, left_idx01);

                uint8x8_t a0_y0 = vget_low_u8(a0_y01);
                uint8x8_t a0_y1 = vget_high_u8(a0_y01);
                uint8x8_t a1_y0 = vget_low_u8(a1_y01);
                uint8x8_t a1_y1 = vget_high_u8(a1_y01);

                shifty.val[0] = vshrq_n_u16(vandq_u16(vreinterpretq_u16_s16(y_c256.val[0]), c3f), 1);
                shifty.val[1] = vshrq_n_u16(vandq_u16(vreinterpretq_u16_s16(y_c256.val[1]), c3f), 1);

                diff.val[0] = vsubl_u8(a1_y0, a0_y0); // a[x+1] - a[x]
                diff.val[1] = vsubl_u8(a1_y1, a0_y1); // a[x+1] - a[x]
                a32.val[0]  = vshll_n_u8(a0_y0, 5);
                a32.val[1]  = vshll_n_u8(a0_y1, 5);
                res.val[0]  = vmlaq_u16(a32.val[0], diff.val[0], shifty.val[0]);
                res.val[1]  = vmlaq_u16(a32.val[1], diff.val[1], shifty.val[1]);

                resy = vcombine_u8(vrshrn_n_u16(res.val[0], 5), vrshrn_n_u16(res.val[1], 5));

                uint8x16_t mask = vld1q_u8(base_mask[base_min_diff]);
                resxy           = vbslq_u8(mask, resy, resx);
                vst1q_u8(dst + j, resxy);
            } else {
                vst1q_u8(dst + j, resx);
            }
        } // for j
        dst += stride;
    }
}

// Directional prediction, zone 2: 90 < angle < 180
void svt_av1_dr_prediction_z2_neon(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                                   const uint8_t* left, int32_t upsample_above, int32_t upsample_left, int32_t dx,
                                   int32_t dy) {
    assert(dx > 0);
    assert(dy > 0);

    switch (bw) {
    case 4: {
        dr_prediction_z2_4xH_neon(bh, dst, stride, above, left, upsample_above, upsample_left, dx, dy);
        break;
    }
    case 8: {
        dr_prediction_z2_8xH_neon(bh, dst, stride, above, left, upsample_above, upsample_left, dx, dy);
        break;
    }
    default: {
        dr_prediction_z2_WxH_neon(bw, bh, dst, stride, above, left, dx, dy);
        break;
    }
    }
}

/* ---------------------P R E D I C T I O N   Z 3--------------------------- */
static inline void transpose4x16_neon(uint8x16_t* x, uint16x8x2_t* d) {
    uint8x16x2_t w0, w1;

    w0 = vzipq_u8(x[0], x[1]);
    w1 = vzipq_u8(x[2], x[3]);

    d[0] = vzipq_u16(vreinterpretq_u16_u8(w0.val[0]), vreinterpretq_u16_u8(w1.val[0]));
    d[1] = vzipq_u16(vreinterpretq_u16_u8(w0.val[1]), vreinterpretq_u16_u8(w1.val[1]));
}

static inline void transpose4x8_8x4_low_neon(uint8x8_t* x, uint16x4x2_t* d) {
    uint8x8x2_t w0, w1;

    w0 = vzip_u8(x[0], x[1]);
    w1 = vzip_u8(x[2], x[3]);

    *d = vzip_u16(vreinterpret_u16_u8(w0.val[0]), vreinterpret_u16_u8(w1.val[0]));
}

static inline void transpose4x8_8x4_neon(uint8x8_t* x, uint16x4x2_t* d) {
    uint8x8x2_t w0, w1;

    w0 = vzip_u8(x[0], x[1]);
    w1 = vzip_u8(x[2], x[3]);

    d[0] = vzip_u16(vreinterpret_u16_u8(w0.val[0]), vreinterpret_u16_u8(w1.val[0]));
    d[1] = vzip_u16(vreinterpret_u16_u8(w0.val[1]), vreinterpret_u16_u8(w1.val[1]));
}

static inline void transpose8x8_low_neon(uint8x8_t* x, uint32x2x2_t* d) {
    uint8x8x2_t  w0, w1, w2, w3;
    uint16x4x2_t w4, w5;

    w0 = vzip_u8(x[0], x[1]);
    w1 = vzip_u8(x[2], x[3]);
    w2 = vzip_u8(x[4], x[5]);
    w3 = vzip_u8(x[6], x[7]);

    w4 = vzip_u16(vreinterpret_u16_u8(w0.val[0]), vreinterpret_u16_u8(w1.val[0]));
    w5 = vzip_u16(vreinterpret_u16_u8(w2.val[0]), vreinterpret_u16_u8(w3.val[0]));

    d[0] = vzip_u32(vreinterpret_u32_u16(w4.val[0]), vreinterpret_u32_u16(w5.val[0]));
    d[1] = vzip_u32(vreinterpret_u32_u16(w4.val[1]), vreinterpret_u32_u16(w5.val[1]));
}

static inline void transpose8x8_neon(uint8x8_t* x, uint32x2x2_t* d) {
    uint8x8x2_t  w0, w1, w2, w3;
    uint16x4x2_t w4, w5, w6, w7;

    w0 = vzip_u8(x[0], x[1]);
    w1 = vzip_u8(x[2], x[3]);
    w2 = vzip_u8(x[4], x[5]);
    w3 = vzip_u8(x[6], x[7]);

    w4 = vzip_u16(vreinterpret_u16_u8(w0.val[0]), vreinterpret_u16_u8(w1.val[0]));
    w5 = vzip_u16(vreinterpret_u16_u8(w2.val[0]), vreinterpret_u16_u8(w3.val[0]));

    d[0] = vzip_u32(vreinterpret_u32_u16(w4.val[0]), vreinterpret_u32_u16(w5.val[0]));
    d[1] = vzip_u32(vreinterpret_u32_u16(w4.val[1]), vreinterpret_u32_u16(w5.val[1]));

    w6 = vzip_u16(vreinterpret_u16_u8(w0.val[1]), vreinterpret_u16_u8(w1.val[1]));
    w7 = vzip_u16(vreinterpret_u16_u8(w2.val[1]), vreinterpret_u16_u8(w3.val[1]));

    d[2] = vzip_u32(vreinterpret_u32_u16(w6.val[0]), vreinterpret_u32_u16(w7.val[0]));
    d[3] = vzip_u32(vreinterpret_u32_u16(w6.val[1]), vreinterpret_u32_u16(w7.val[1]));
}

static inline void transpose16x8_8x16_neon(uint8x8_t* x, uint64x2_t* d) {
    uint8x8x2_t  w0, w1, w2, w3, w8, w9, w10, w11;
    uint16x4x2_t w4, w5, w12, w13;
    uint32x2x2_t w6, w7, w14, w15;

    w0 = vzip_u8(x[0], x[1]);
    w1 = vzip_u8(x[2], x[3]);
    w2 = vzip_u8(x[4], x[5]);
    w3 = vzip_u8(x[6], x[7]);

    w8  = vzip_u8(x[8], x[9]);
    w9  = vzip_u8(x[10], x[11]);
    w10 = vzip_u8(x[12], x[13]);
    w11 = vzip_u8(x[14], x[15]);

    w4  = vzip_u16(vreinterpret_u16_u8(w0.val[0]), vreinterpret_u16_u8(w1.val[0]));
    w5  = vzip_u16(vreinterpret_u16_u8(w2.val[0]), vreinterpret_u16_u8(w3.val[0]));
    w12 = vzip_u16(vreinterpret_u16_u8(w8.val[0]), vreinterpret_u16_u8(w9.val[0]));
    w13 = vzip_u16(vreinterpret_u16_u8(w10.val[0]), vreinterpret_u16_u8(w11.val[0]));

    w6  = vzip_u32(vreinterpret_u32_u16(w4.val[0]), vreinterpret_u32_u16(w5.val[0]));
    w7  = vzip_u32(vreinterpret_u32_u16(w4.val[1]), vreinterpret_u32_u16(w5.val[1]));
    w14 = vzip_u32(vreinterpret_u32_u16(w12.val[0]), vreinterpret_u32_u16(w13.val[0]));
    w15 = vzip_u32(vreinterpret_u32_u16(w12.val[1]), vreinterpret_u32_u16(w13.val[1]));

    // Store first 4-line result
    d[0] = vcombine_u64(vreinterpret_u64_u32(w6.val[0]), vreinterpret_u64_u32(w14.val[0]));
    d[1] = vcombine_u64(vreinterpret_u64_u32(w6.val[1]), vreinterpret_u64_u32(w14.val[1]));
    d[2] = vcombine_u64(vreinterpret_u64_u32(w7.val[0]), vreinterpret_u64_u32(w15.val[0]));
    d[3] = vcombine_u64(vreinterpret_u64_u32(w7.val[1]), vreinterpret_u64_u32(w15.val[1]));

    w4  = vzip_u16(vreinterpret_u16_u8(w0.val[1]), vreinterpret_u16_u8(w1.val[1]));
    w5  = vzip_u16(vreinterpret_u16_u8(w2.val[1]), vreinterpret_u16_u8(w3.val[1]));
    w12 = vzip_u16(vreinterpret_u16_u8(w8.val[1]), vreinterpret_u16_u8(w9.val[1]));
    w13 = vzip_u16(vreinterpret_u16_u8(w10.val[1]), vreinterpret_u16_u8(w11.val[1]));

    w6  = vzip_u32(vreinterpret_u32_u16(w4.val[0]), vreinterpret_u32_u16(w5.val[0]));
    w7  = vzip_u32(vreinterpret_u32_u16(w4.val[1]), vreinterpret_u32_u16(w5.val[1]));
    w14 = vzip_u32(vreinterpret_u32_u16(w12.val[0]), vreinterpret_u32_u16(w13.val[0]));
    w15 = vzip_u32(vreinterpret_u32_u16(w12.val[1]), vreinterpret_u32_u16(w13.val[1]));

    // Store second 4-line result
    d[4] = vcombine_u64(vreinterpret_u64_u32(w6.val[0]), vreinterpret_u64_u32(w14.val[0]));
    d[5] = vcombine_u64(vreinterpret_u64_u32(w6.val[1]), vreinterpret_u64_u32(w14.val[1]));
    d[6] = vcombine_u64(vreinterpret_u64_u32(w7.val[0]), vreinterpret_u64_u32(w15.val[0]));
    d[7] = vcombine_u64(vreinterpret_u64_u32(w7.val[1]), vreinterpret_u64_u32(w15.val[1]));
}

static inline void transpose8x16_16x8_neon(uint8x16_t* x, uint64x2_t* d) {
    uint8x16x2_t w0, w1, w2, w3;
    uint16x8x2_t w4, w5, w6, w7;
    uint32x4x2_t w8, w9, w10, w11;

    w0 = vzipq_u8(x[0], x[1]);
    w1 = vzipq_u8(x[2], x[3]);
    w2 = vzipq_u8(x[4], x[5]);
    w3 = vzipq_u8(x[6], x[7]);

    w4 = vzipq_u16(vreinterpretq_u16_u8(w0.val[0]), vreinterpretq_u16_u8(w1.val[0]));
    w5 = vzipq_u16(vreinterpretq_u16_u8(w2.val[0]), vreinterpretq_u16_u8(w3.val[0]));
    w6 = vzipq_u16(vreinterpretq_u16_u8(w0.val[1]), vreinterpretq_u16_u8(w1.val[1]));
    w7 = vzipq_u16(vreinterpretq_u16_u8(w2.val[1]), vreinterpretq_u16_u8(w3.val[1]));

    w8  = vzipq_u32(vreinterpretq_u32_u16(w4.val[0]), vreinterpretq_u32_u16(w5.val[0]));
    w9  = vzipq_u32(vreinterpretq_u32_u16(w6.val[0]), vreinterpretq_u32_u16(w7.val[0]));
    w10 = vzipq_u32(vreinterpretq_u32_u16(w4.val[1]), vreinterpretq_u32_u16(w5.val[1]));
    w11 = vzipq_u32(vreinterpretq_u32_u16(w6.val[1]), vreinterpretq_u32_u16(w7.val[1]));

    d[0] = vzip1q_u64(vreinterpretq_u64_u32(w8.val[0]), vreinterpretq_u64_u32(w9.val[0]));
    d[1] = vzip2q_u64(vreinterpretq_u64_u32(w8.val[0]), vreinterpretq_u64_u32(w9.val[0]));
    d[2] = vzip1q_u64(vreinterpretq_u64_u32(w8.val[1]), vreinterpretq_u64_u32(w9.val[1]));
    d[3] = vzip2q_u64(vreinterpretq_u64_u32(w8.val[1]), vreinterpretq_u64_u32(w9.val[1]));
    d[4] = vzip1q_u64(vreinterpretq_u64_u32(w10.val[0]), vreinterpretq_u64_u32(w11.val[0]));
    d[5] = vzip2q_u64(vreinterpretq_u64_u32(w10.val[0]), vreinterpretq_u64_u32(w11.val[0]));
    d[6] = vzip1q_u64(vreinterpretq_u64_u32(w10.val[1]), vreinterpretq_u64_u32(w11.val[1]));
    d[7] = vzip2q_u64(vreinterpretq_u64_u32(w10.val[1]), vreinterpretq_u64_u32(w11.val[1]));
}

static inline void transpose16x16_neon(uint8x16_t* x, uint64x2_t* d) {
    uint8x16x2_t w0, w1, w2, w3, w4, w5, w6, w7;
    uint16x8x2_t w8, w9, w10, w11;
    uint32x4x2_t w12, w13, w14, w15;

    w0 = vzipq_u8(x[0], x[1]);
    w1 = vzipq_u8(x[2], x[3]);
    w2 = vzipq_u8(x[4], x[5]);
    w3 = vzipq_u8(x[6], x[7]);

    w4 = vzipq_u8(x[8], x[9]);
    w5 = vzipq_u8(x[10], x[11]);
    w6 = vzipq_u8(x[12], x[13]);
    w7 = vzipq_u8(x[14], x[15]);

    w8  = vzipq_u16(vreinterpretq_u16_u8(w0.val[0]), vreinterpretq_u16_u8(w1.val[0]));
    w9  = vzipq_u16(vreinterpretq_u16_u8(w2.val[0]), vreinterpretq_u16_u8(w3.val[0]));
    w10 = vzipq_u16(vreinterpretq_u16_u8(w4.val[0]), vreinterpretq_u16_u8(w5.val[0]));
    w11 = vzipq_u16(vreinterpretq_u16_u8(w6.val[0]), vreinterpretq_u16_u8(w7.val[0]));

    w12 = vzipq_u32(vreinterpretq_u32_u16(w8.val[0]), vreinterpretq_u32_u16(w9.val[0]));
    w13 = vzipq_u32(vreinterpretq_u32_u16(w10.val[0]), vreinterpretq_u32_u16(w11.val[0]));
    w14 = vzipq_u32(vreinterpretq_u32_u16(w8.val[1]), vreinterpretq_u32_u16(w9.val[1]));
    w15 = vzipq_u32(vreinterpretq_u32_u16(w10.val[1]), vreinterpretq_u32_u16(w11.val[1]));

    d[0] = vzip1q_u64(vreinterpretq_u64_u32(w12.val[0]), vreinterpretq_u64_u32(w13.val[0]));
    d[1] = vzip2q_u64(vreinterpretq_u64_u32(w12.val[0]), vreinterpretq_u64_u32(w13.val[0]));
    d[2] = vzip1q_u64(vreinterpretq_u64_u32(w12.val[1]), vreinterpretq_u64_u32(w13.val[1]));
    d[3] = vzip2q_u64(vreinterpretq_u64_u32(w12.val[1]), vreinterpretq_u64_u32(w13.val[1]));
    d[4] = vzip1q_u64(vreinterpretq_u64_u32(w14.val[0]), vreinterpretq_u64_u32(w15.val[0]));
    d[5] = vzip2q_u64(vreinterpretq_u64_u32(w14.val[0]), vreinterpretq_u64_u32(w15.val[0]));
    d[6] = vzip1q_u64(vreinterpretq_u64_u32(w14.val[1]), vreinterpretq_u64_u32(w15.val[1]));
    d[7] = vzip2q_u64(vreinterpretq_u64_u32(w14.val[1]), vreinterpretq_u64_u32(w15.val[1]));

    // upper half
    w8  = vzipq_u16(vreinterpretq_u16_u8(w0.val[1]), vreinterpretq_u16_u8(w1.val[1]));
    w9  = vzipq_u16(vreinterpretq_u16_u8(w2.val[1]), vreinterpretq_u16_u8(w3.val[1]));
    w10 = vzipq_u16(vreinterpretq_u16_u8(w4.val[1]), vreinterpretq_u16_u8(w5.val[1]));
    w11 = vzipq_u16(vreinterpretq_u16_u8(w6.val[1]), vreinterpretq_u16_u8(w7.val[1]));

    w12 = vzipq_u32(vreinterpretq_u32_u16(w8.val[0]), vreinterpretq_u32_u16(w9.val[0]));
    w13 = vzipq_u32(vreinterpretq_u32_u16(w10.val[0]), vreinterpretq_u32_u16(w11.val[0]));
    w14 = vzipq_u32(vreinterpretq_u32_u16(w8.val[1]), vreinterpretq_u32_u16(w9.val[1]));
    w15 = vzipq_u32(vreinterpretq_u32_u16(w10.val[1]), vreinterpretq_u32_u16(w11.val[1]));

    d[8]  = vzip1q_u64(vreinterpretq_u64_u32(w12.val[0]), vreinterpretq_u64_u32(w13.val[0]));
    d[9]  = vzip2q_u64(vreinterpretq_u64_u32(w12.val[0]), vreinterpretq_u64_u32(w13.val[0]));
    d[10] = vzip1q_u64(vreinterpretq_u64_u32(w12.val[1]), vreinterpretq_u64_u32(w13.val[1]));
    d[11] = vzip2q_u64(vreinterpretq_u64_u32(w12.val[1]), vreinterpretq_u64_u32(w13.val[1]));
    d[12] = vzip1q_u64(vreinterpretq_u64_u32(w14.val[0]), vreinterpretq_u64_u32(w15.val[0]));
    d[13] = vzip2q_u64(vreinterpretq_u64_u32(w14.val[0]), vreinterpretq_u64_u32(w15.val[0]));
    d[14] = vzip1q_u64(vreinterpretq_u64_u32(w14.val[1]), vreinterpretq_u64_u32(w15.val[1]));
    d[15] = vzip2q_u64(vreinterpretq_u64_u32(w14.val[1]), vreinterpretq_u64_u32(w15.val[1]));
}

static inline void transpose16x32_neon(uint8x16x2_t* x, uint64x2x2_t* d) {
    uint8x16x2_t w0, w1, w2, w3, w8, w9, w10, w11;
    uint16x8x2_t w4, w5, w12, w13;
    uint32x4x2_t w6, w7, w14, w15;

    w0 = vzipq_u8(x[0].val[0], x[1].val[0]);
    w1 = vzipq_u8(x[2].val[0], x[3].val[0]);
    w2 = vzipq_u8(x[4].val[0], x[5].val[0]);
    w3 = vzipq_u8(x[6].val[0], x[7].val[0]);

    w8  = vzipq_u8(x[8].val[0], x[9].val[0]);
    w9  = vzipq_u8(x[10].val[0], x[11].val[0]);
    w10 = vzipq_u8(x[12].val[0], x[13].val[0]);
    w11 = vzipq_u8(x[14].val[0], x[15].val[0]);

    w4  = vzipq_u16(vreinterpretq_u16_u8(w0.val[0]), vreinterpretq_u16_u8(w1.val[0]));
    w5  = vzipq_u16(vreinterpretq_u16_u8(w2.val[0]), vreinterpretq_u16_u8(w3.val[0]));
    w12 = vzipq_u16(vreinterpretq_u16_u8(w8.val[0]), vreinterpretq_u16_u8(w9.val[0]));
    w13 = vzipq_u16(vreinterpretq_u16_u8(w10.val[0]), vreinterpretq_u16_u8(w11.val[0]));

    w6  = vzipq_u32(vreinterpretq_u32_u16(w4.val[0]), vreinterpretq_u32_u16(w5.val[0]));
    w7  = vzipq_u32(vreinterpretq_u32_u16(w4.val[1]), vreinterpretq_u32_u16(w5.val[1]));
    w14 = vzipq_u32(vreinterpretq_u32_u16(w12.val[0]), vreinterpretq_u32_u16(w13.val[0]));
    w15 = vzipq_u32(vreinterpretq_u32_u16(w12.val[1]), vreinterpretq_u32_u16(w13.val[1]));

    // Store first 4-line result

    d[0].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w6.val[0]), vreinterpretq_u64_u32(w14.val[0]));
    d[0].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w6.val[0]), vreinterpretq_u64_u32(w14.val[0]));
    d[1].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w6.val[1]), vreinterpretq_u64_u32(w14.val[1]));
    d[1].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w6.val[1]), vreinterpretq_u64_u32(w14.val[1]));
    d[2].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w7.val[0]), vreinterpretq_u64_u32(w15.val[0]));
    d[2].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w7.val[0]), vreinterpretq_u64_u32(w15.val[0]));
    d[3].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w7.val[1]), vreinterpretq_u64_u32(w15.val[1]));
    d[3].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w7.val[1]), vreinterpretq_u64_u32(w15.val[1]));

    w4  = vzipq_u16(vreinterpretq_u16_u8(w0.val[1]), vreinterpretq_u16_u8(w1.val[1]));
    w5  = vzipq_u16(vreinterpretq_u16_u8(w2.val[1]), vreinterpretq_u16_u8(w3.val[1]));
    w12 = vzipq_u16(vreinterpretq_u16_u8(w8.val[1]), vreinterpretq_u16_u8(w9.val[1]));
    w13 = vzipq_u16(vreinterpretq_u16_u8(w10.val[1]), vreinterpretq_u16_u8(w11.val[1]));

    w6  = vzipq_u32(vreinterpretq_u32_u16(w4.val[0]), vreinterpretq_u32_u16(w5.val[0]));
    w7  = vzipq_u32(vreinterpretq_u32_u16(w4.val[1]), vreinterpretq_u32_u16(w5.val[1]));
    w14 = vzipq_u32(vreinterpretq_u32_u16(w12.val[0]), vreinterpretq_u32_u16(w13.val[0]));
    w15 = vzipq_u32(vreinterpretq_u32_u16(w12.val[1]), vreinterpretq_u32_u16(w13.val[1]));

    // Store second 4-line result

    d[4].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w6.val[0]), vreinterpretq_u64_u32(w14.val[0]));
    d[4].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w6.val[0]), vreinterpretq_u64_u32(w14.val[0]));
    d[5].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w6.val[1]), vreinterpretq_u64_u32(w14.val[1]));
    d[5].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w6.val[1]), vreinterpretq_u64_u32(w14.val[1]));
    d[6].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w7.val[0]), vreinterpretq_u64_u32(w15.val[0]));
    d[6].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w7.val[0]), vreinterpretq_u64_u32(w15.val[0]));
    d[7].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w7.val[1]), vreinterpretq_u64_u32(w15.val[1]));
    d[7].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w7.val[1]), vreinterpretq_u64_u32(w15.val[1]));

    // upper half
    w0 = vzipq_u8(x[0].val[1], x[1].val[1]);
    w1 = vzipq_u8(x[2].val[1], x[3].val[1]);
    w2 = vzipq_u8(x[4].val[1], x[5].val[1]);
    w3 = vzipq_u8(x[6].val[1], x[7].val[1]);

    w8  = vzipq_u8(x[8].val[1], x[9].val[1]);
    w9  = vzipq_u8(x[10].val[1], x[11].val[1]);
    w10 = vzipq_u8(x[12].val[1], x[13].val[1]);
    w11 = vzipq_u8(x[14].val[1], x[15].val[1]);

    w4  = vzipq_u16(vreinterpretq_u16_u8(w0.val[0]), vreinterpretq_u16_u8(w1.val[0]));
    w5  = vzipq_u16(vreinterpretq_u16_u8(w2.val[0]), vreinterpretq_u16_u8(w3.val[0]));
    w12 = vzipq_u16(vreinterpretq_u16_u8(w8.val[0]), vreinterpretq_u16_u8(w9.val[0]));
    w13 = vzipq_u16(vreinterpretq_u16_u8(w10.val[0]), vreinterpretq_u16_u8(w11.val[0]));

    w6  = vzipq_u32(vreinterpretq_u32_u16(w4.val[0]), vreinterpretq_u32_u16(w5.val[0]));
    w7  = vzipq_u32(vreinterpretq_u32_u16(w4.val[1]), vreinterpretq_u32_u16(w5.val[1]));
    w14 = vzipq_u32(vreinterpretq_u32_u16(w12.val[0]), vreinterpretq_u32_u16(w13.val[0]));
    w15 = vzipq_u32(vreinterpretq_u32_u16(w12.val[1]), vreinterpretq_u32_u16(w13.val[1]));

    // Store first 4-line result

    d[8].val[0]  = vzip1q_u64(vreinterpretq_u64_u32(w6.val[0]), vreinterpretq_u64_u32(w14.val[0]));
    d[8].val[1]  = vzip2q_u64(vreinterpretq_u64_u32(w6.val[0]), vreinterpretq_u64_u32(w14.val[0]));
    d[9].val[0]  = vzip1q_u64(vreinterpretq_u64_u32(w6.val[1]), vreinterpretq_u64_u32(w14.val[1]));
    d[9].val[1]  = vzip2q_u64(vreinterpretq_u64_u32(w6.val[1]), vreinterpretq_u64_u32(w14.val[1]));
    d[10].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w7.val[0]), vreinterpretq_u64_u32(w15.val[0]));
    d[10].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w7.val[0]), vreinterpretq_u64_u32(w15.val[0]));
    d[11].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w7.val[1]), vreinterpretq_u64_u32(w15.val[1]));
    d[11].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w7.val[1]), vreinterpretq_u64_u32(w15.val[1]));

    w4  = vzipq_u16(vreinterpretq_u16_u8(w0.val[1]), vreinterpretq_u16_u8(w1.val[1]));
    w5  = vzipq_u16(vreinterpretq_u16_u8(w2.val[1]), vreinterpretq_u16_u8(w3.val[1]));
    w12 = vzipq_u16(vreinterpretq_u16_u8(w8.val[1]), vreinterpretq_u16_u8(w9.val[1]));
    w13 = vzipq_u16(vreinterpretq_u16_u8(w10.val[1]), vreinterpretq_u16_u8(w11.val[1]));

    w6  = vzipq_u32(vreinterpretq_u32_u16(w4.val[0]), vreinterpretq_u32_u16(w5.val[0]));
    w7  = vzipq_u32(vreinterpretq_u32_u16(w4.val[1]), vreinterpretq_u32_u16(w5.val[1]));
    w14 = vzipq_u32(vreinterpretq_u32_u16(w12.val[0]), vreinterpretq_u32_u16(w13.val[0]));
    w15 = vzipq_u32(vreinterpretq_u32_u16(w12.val[1]), vreinterpretq_u32_u16(w13.val[1]));

    // Store second 4-line result

    d[12].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w6.val[0]), vreinterpretq_u64_u32(w14.val[0]));
    d[12].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w6.val[0]), vreinterpretq_u64_u32(w14.val[0]));
    d[13].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w6.val[1]), vreinterpretq_u64_u32(w14.val[1]));
    d[13].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w6.val[1]), vreinterpretq_u64_u32(w14.val[1]));
    d[14].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w7.val[0]), vreinterpretq_u64_u32(w15.val[0]));
    d[14].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w7.val[0]), vreinterpretq_u64_u32(w15.val[0]));
    d[15].val[0] = vzip1q_u64(vreinterpretq_u64_u32(w7.val[1]), vreinterpretq_u64_u32(w15.val[1]));
    d[15].val[1] = vzip2q_u64(vreinterpretq_u64_u32(w7.val[1]), vreinterpretq_u64_u32(w15.val[1]));
}

static inline void transpose_TX_16X16(const uint8_t* src, ptrdiff_t pitchSrc, uint8_t* dst, ptrdiff_t pitchDst) {
    uint8x16_t r[16];
    uint64x2_t d[16];
    for (int i = 0; i < 16; i++) {
        r[i] = vld1q_u8(src + i * pitchSrc);
    }
    transpose16x16_neon(r, d);
    for (int i = 0; i < 16; i++) {
        vst1q_u8(dst + i * pitchDst, vreinterpretq_u8_u64(d[i]));
    }
}

static inline void transpose(const uint8_t* src, ptrdiff_t pitchSrc, uint8_t* dst, ptrdiff_t pitchDst, int width,
                             int height) {
    for (int j = 0; j < height; j += 16) {
        for (int i = 0; i < width; i += 16) {
            transpose_TX_16X16(src + i * pitchSrc + j, pitchSrc, dst + j * pitchDst + i, pitchDst);
        }
    }
}

static inline void dr_prediction_z3_4x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int upsample_left,
                                             int dy) {
    uint8x8_t    dstvec[4];
    uint16x4x2_t dest;

    dr_prediction_z1_WxH_internal_neon_small(4, 4, dstvec, left, upsample_left, dy);
    transpose4x8_8x4_low_neon(dstvec, &dest);
    vst1_lane_u32((uint32_t*)(dst + stride * 0), vreinterpret_u32_u16(dest.val[0]), 0);
    vst1_lane_u32((uint32_t*)(dst + stride * 1), vreinterpret_u32_u16(dest.val[0]), 1);
    vst1_lane_u32((uint32_t*)(dst + stride * 2), vreinterpret_u32_u16(dest.val[1]), 0);
    vst1_lane_u32((uint32_t*)(dst + stride * 3), vreinterpret_u32_u16(dest.val[1]), 1);
}

static inline void dr_prediction_z3_8x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int upsample_left,
                                             int dy) {
    uint8x8_t    dstvec[8];
    uint32x2x2_t d[4];

    dr_prediction_z1_WxH_internal_neon_small(8, 8, dstvec, left, upsample_left, dy);
    transpose8x8_neon(dstvec, d);
    vst1_u32((uint32_t*)(dst + 0 * stride), d[0].val[0]);
    vst1_u32((uint32_t*)(dst + 1 * stride), d[0].val[1]);
    vst1_u32((uint32_t*)(dst + 2 * stride), d[1].val[0]);
    vst1_u32((uint32_t*)(dst + 3 * stride), d[1].val[1]);
    vst1_u32((uint32_t*)(dst + 4 * stride), d[2].val[0]);
    vst1_u32((uint32_t*)(dst + 5 * stride), d[2].val[1]);
    vst1_u32((uint32_t*)(dst + 6 * stride), d[3].val[0]);
    vst1_u32((uint32_t*)(dst + 7 * stride), d[3].val[1]);
}

static inline void dr_prediction_z3_4x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int upsample_left,
                                             int dy) {
    uint8x8_t    dstvec[4];
    uint16x4x2_t d[2];

    dr_prediction_z1_WxH_internal_neon_small(8, 4, dstvec, left, upsample_left, dy);
    transpose4x8_8x4_neon(dstvec, d);
    vst1_lane_u32((uint32_t*)(dst + stride * 0), vreinterpret_u32_u16(d[0].val[0]), 0);
    vst1_lane_u32((uint32_t*)(dst + stride * 1), vreinterpret_u32_u16(d[0].val[0]), 1);
    vst1_lane_u32((uint32_t*)(dst + stride * 2), vreinterpret_u32_u16(d[0].val[1]), 0);
    vst1_lane_u32((uint32_t*)(dst + stride * 3), vreinterpret_u32_u16(d[0].val[1]), 1);
    vst1_lane_u32((uint32_t*)(dst + stride * 4), vreinterpret_u32_u16(d[1].val[0]), 0);
    vst1_lane_u32((uint32_t*)(dst + stride * 5), vreinterpret_u32_u16(d[1].val[0]), 1);
    vst1_lane_u32((uint32_t*)(dst + stride * 6), vreinterpret_u32_u16(d[1].val[1]), 0);
    vst1_lane_u32((uint32_t*)(dst + stride * 7), vreinterpret_u32_u16(d[1].val[1]), 1);
}

static inline void dr_prediction_z3_8x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int upsample_left,
                                             int dy) {
    uint8x8_t    dstvec[8];
    uint32x2x2_t d[2];

    dr_prediction_z1_WxH_internal_neon_small(4, 8, dstvec, left, upsample_left, dy);
    transpose8x8_low_neon(dstvec, d);
    vst1_u32((uint32_t*)(dst + 0 * stride), d[0].val[0]);
    vst1_u32((uint32_t*)(dst + 1 * stride), d[0].val[1]);
    vst1_u32((uint32_t*)(dst + 2 * stride), d[1].val[0]);
    vst1_u32((uint32_t*)(dst + 3 * stride), d[1].val[1]);
}

static inline void dr_prediction_z3_8x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int dy) {
    uint8x16_t dstvec[8];
    uint64x2_t d[8];

    dr_prediction_z1_WxH_internal_neon_large(16, 8, dstvec, left, dy);
    transpose8x16_16x8_neon(dstvec, d);
    for (int i = 0; i < 8; i++) {
        vst1_u8(dst + i * stride, vreinterpret_u8_u64(vget_low_u64(d[i])));
        vst1_u8(dst + (i + 8) * stride, vreinterpret_u8_u64(vget_high_u64(d[i])));
    }
}

static inline void dr_prediction_z3_16x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int upsample_left,
                                              int dy) {
    uint8x8_t  dstvec[16];
    uint64x2_t d[8];

    dr_prediction_z1_WxH_internal_neon_small(8, 16, dstvec, left, upsample_left, dy);
    transpose16x8_8x16_neon(dstvec, d);
    for (int i = 0; i < 8; i++) {
        vst1q_u8(dst + i * stride, vreinterpretq_u8_u64(d[i]));
    }
}

static inline void dr_prediction_z3_4x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int dy) {
    uint8x16_t   dstvec[4];
    uint16x8x2_t d[2];

    dr_prediction_z1_WxH_internal_neon_large(16, 4, dstvec, left, dy);
    transpose4x16_neon(dstvec, d);
    vst1q_lane_u32((uint32_t*)(dst + stride * 0), vreinterpretq_u32_u16(d[0].val[0]), 0);
    vst1q_lane_u32((uint32_t*)(dst + stride * 1), vreinterpretq_u32_u16(d[0].val[0]), 1);
    vst1q_lane_u32((uint32_t*)(dst + stride * 2), vreinterpretq_u32_u16(d[0].val[0]), 2);
    vst1q_lane_u32((uint32_t*)(dst + stride * 3), vreinterpretq_u32_u16(d[0].val[0]), 3);

    vst1q_lane_u32((uint32_t*)(dst + stride * 4), vreinterpretq_u32_u16(d[0].val[1]), 0);
    vst1q_lane_u32((uint32_t*)(dst + stride * 5), vreinterpretq_u32_u16(d[0].val[1]), 1);
    vst1q_lane_u32((uint32_t*)(dst + stride * 6), vreinterpretq_u32_u16(d[0].val[1]), 2);
    vst1q_lane_u32((uint32_t*)(dst + stride * 7), vreinterpretq_u32_u16(d[0].val[1]), 3);

    vst1q_lane_u32((uint32_t*)(dst + stride * 8), vreinterpretq_u32_u16(d[1].val[0]), 0);
    vst1q_lane_u32((uint32_t*)(dst + stride * 9), vreinterpretq_u32_u16(d[1].val[0]), 1);
    vst1q_lane_u32((uint32_t*)(dst + stride * 10), vreinterpretq_u32_u16(d[1].val[0]), 2);
    vst1q_lane_u32((uint32_t*)(dst + stride * 11), vreinterpretq_u32_u16(d[1].val[0]), 3);

    vst1q_lane_u32((uint32_t*)(dst + stride * 12), vreinterpretq_u32_u16(d[1].val[1]), 0);
    vst1q_lane_u32((uint32_t*)(dst + stride * 13), vreinterpretq_u32_u16(d[1].val[1]), 1);
    vst1q_lane_u32((uint32_t*)(dst + stride * 14), vreinterpretq_u32_u16(d[1].val[1]), 2);
    vst1q_lane_u32((uint32_t*)(dst + stride * 15), vreinterpretq_u32_u16(d[1].val[1]), 3);
}

static inline void dr_prediction_z3_16x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int upsample_left,
                                              int dy) {
    uint8x8_t  dstvec[16];
    uint64x2_t d[8];

    dr_prediction_z1_WxH_internal_neon_small(4, 16, dstvec, left, upsample_left, dy);
    transpose16x8_8x16_neon(dstvec, d);
    for (int i = 0; i < 4; i++) {
        vst1q_u8(dst + i * stride, vreinterpretq_u8_u64(d[i]));
    }
}

static inline void dr_prediction_z3_8x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int dy) {
    uint8x16x2_t dstvec[16];
    uint64x2x2_t d[16];
    uint8x16_t   v_zero = vdupq_n_u8(0);
    dr_prediction_z1_32xH_internal_neon(8, dstvec, left, dy);
    for (int i = 8; i < 16; i++) {
        dstvec[i].val[0] = v_zero;
        dstvec[i].val[1] = v_zero;
    }
    transpose16x32_neon(dstvec, d);
    for (int i = 0; i < 16; i++) {
        vst1_u8(dst + 2 * i * stride, vreinterpret_u8_u64(vget_low_u64(d[i].val[0])));
        vst1_u8(dst + (2 * i + 1) * stride, vreinterpret_u8_u64(vget_low_u64(d[i].val[1])));
    }
}

static inline void dr_prediction_z3_32x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int upsample_left,
                                              int dy) {
    uint8x8_t  dstvec[32];
    uint64x2_t d[16];

    dr_prediction_z1_WxH_internal_neon_small(8, 32, dstvec, left, upsample_left, dy);
    transpose16x8_8x16_neon(dstvec, d);
    transpose16x8_8x16_neon(dstvec + 16, d + 8);
    for (int i = 0; i < 8; i++) {
        vst1q_u8(dst + i * stride, vreinterpretq_u8_u64(d[i]));
        vst1q_u8(dst + i * stride + 16, vreinterpretq_u8_u64(d[i + 8]));
    }
}

static inline void dr_prediction_z3_16x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int dy) {
    uint8x16_t dstvec[16];
    uint64x2_t d[16];

    dr_prediction_z1_WxH_internal_neon_large(16, 16, dstvec, left, dy);
    transpose16x16_neon(dstvec, d);
    for (int i = 0; i < 16; i++) {
        vst1q_u8(dst + i * stride, vreinterpretq_u8_u64(d[i]));
    }
}

static inline void dr_prediction_z3_32x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int dy) {
    uint8x16x2_t dstvec[32];
    uint64x2x2_t d[32];
    dr_prediction_z1_32xH_internal_neon(32, dstvec, left, dy);
    transpose16x32_neon(dstvec, d);
    transpose16x32_neon(dstvec + 16, d + 16);
    for (int i = 0; i < 16; i++) {
        vst1q_u8(dst + 2 * i * stride, vreinterpretq_u8_u64(d[i].val[0]));
        vst1q_u8(dst + 2 * i * stride + 16, vreinterpretq_u8_u64(d[i + 16].val[0]));
        vst1q_u8(dst + (2 * i + 1) * stride, vreinterpretq_u8_u64(d[i].val[1]));
        vst1q_u8(dst + (2 * i + 1) * stride + 16, vreinterpretq_u8_u64(d[i + 16].val[1]));
    }
}

static inline void dr_prediction_z3_64x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int dy) {
    DECLARE_ALIGNED(16, uint8_t, dstT[64 * 64]);

    dr_prediction_z1_64xH_neon(64, dstT, 64, left, dy);
    transpose(dstT, 64, dst, stride, 64, 64);
}

static inline void dr_prediction_z3_16x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int dy) {
    uint8x16x2_t dstvec[16];
    uint64x2x2_t d[16];
    dr_prediction_z1_32xH_internal_neon(16, dstvec, left, dy);
    transpose16x32_neon(dstvec, d);
    for (int i = 0; i < 16; i++) {
        vst1q_u8(dst + 2 * i * stride, vreinterpretq_u8_u64(d[i].val[0]));
        vst1q_u8(dst + (2 * i + 1) * stride, vreinterpretq_u8_u64(d[i].val[1]));
    }
}

static inline void dr_prediction_z3_32x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int dy) {
    uint8x16_t dstvec[32];
    uint64x2_t d[16];

    dr_prediction_z1_WxH_internal_neon_large(16, 32, dstvec, left, dy);
    for (int i = 0; i < 32; i += 16) {
        transpose16x16_neon((dstvec + i), d);
        for (int j = 0; j < 16; j++) {
            vst1q_u8(dst + j * stride + i, vreinterpretq_u8_u64(d[j]));
        }
    }
}

static inline void dr_prediction_z3_32x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int dy) {
    uint8_t dstT[64 * 32];

    dr_prediction_z1_64xH_neon(32, dstT, 64, left, dy);
    transpose(dstT, 64, dst, stride, 32, 64);
}

static inline void dr_prediction_z3_64x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int dy) {
    uint8_t dstT[32 * 64];

    dr_prediction_z1_32xH_neon(64, dstT, 32, left, dy);
    transpose(dstT, 32, dst, stride, 64, 32);
}

static inline void dr_prediction_z3_16x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int dy) {
    uint8_t dstT[64 * 16];

    dr_prediction_z1_64xH_neon(16, dstT, 64, left, dy);
    transpose(dstT, 64, dst, stride, 16, 64);
}

static inline void dr_prediction_z3_64x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* left, int dy) {
    uint8x16_t dstvec[64];
    uint64x2_t d[16];

    dr_prediction_z1_WxH_internal_neon_large(16, 64, dstvec, left, dy);
    for (int i = 0; i < 64; i += 16) {
        transpose16x16_neon((dstvec + i), d);
        for (int j = 0; j < 16; j++) {
            vst1q_u8(dst + j * stride + i, vreinterpretq_u8_u64(d[j]));
        }
    }
}

void svt_av1_dr_prediction_z3_neon(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                                   const uint8_t* left, int32_t upsample_left, int32_t dx, int32_t dy) {
    (void)above;
    (void)dx;
    assert(dx == 1);
    assert(dy > 0);

    if (bw == bh) {
        switch (bw) {
        case 4: {
            dr_prediction_z3_4x4_neon(dst, stride, left, upsample_left, dy);
            break;
        }
        case 8: {
            dr_prediction_z3_8x8_neon(dst, stride, left, upsample_left, dy);
            break;
        }
        case 16: {
            dr_prediction_z3_16x16_neon(dst, stride, left, dy);
            break;
        }
        case 32: {
            dr_prediction_z3_32x32_neon(dst, stride, left, dy);
            break;
        }
        case 64: {
            dr_prediction_z3_64x64_neon(dst, stride, left, dy);
            break;
        }
        }
    } else {
        if (bw < bh) {
            if (bw + bw == bh) {
                switch (bw) {
                case 4: {
                    dr_prediction_z3_4x8_neon(dst, stride, left, upsample_left, dy);
                    break;
                }
                case 8: {
                    dr_prediction_z3_8x16_neon(dst, stride, left, dy);
                    break;
                }
                case 16: {
                    dr_prediction_z3_16x32_neon(dst, stride, left, dy);
                    break;
                }
                case 32: {
                    dr_prediction_z3_32x64_neon(dst, stride, left, dy);
                    break;
                }
                }
            } else {
                switch (bw) {
                case 4: {
                    dr_prediction_z3_4x16_neon(dst, stride, left, dy);
                    break;
                }
                case 8: {
                    dr_prediction_z3_8x32_neon(dst, stride, left, dy);
                    break;
                }
                case 16: {
                    dr_prediction_z3_16x64_neon(dst, stride, left, dy);
                    break;
                }
                }
            }
        } else {
            if (bh + bh == bw) {
                switch (bh) {
                case 4: {
                    dr_prediction_z3_8x4_neon(dst, stride, left, upsample_left, dy);
                    break;
                }
                case 8: {
                    dr_prediction_z3_16x8_neon(dst, stride, left, upsample_left, dy);
                    break;
                }
                case 16: {
                    dr_prediction_z3_32x16_neon(dst, stride, left, dy);
                    break;
                }
                case 32: {
                    dr_prediction_z3_64x32_neon(dst, stride, left, dy);
                    break;
                }
                }
            } else {
                switch (bh) {
                case 4: {
                    dr_prediction_z3_16x4_neon(dst, stride, left, upsample_left, dy);
                    break;
                }
                case 8: {
                    dr_prediction_z3_32x8_neon(dst, stride, left, upsample_left, dy);
                    break;
                }
                case 16: {
                    dr_prediction_z3_64x16_neon(dst, stride, left, dy);
                    break;
                }
                }
            }
        }
    }
}

// clang-format off
// These kernels are a transposed version of those defined in filterintra_c.c,
// with the absolute value of the negatives taken in the top row.
DECLARE_ALIGNED(16, static const uint8_t, av1_filter_intra_taps_neon[FILTER_INTRA_MODES][7][8]) = {
    {
        {  6,  5,  3,  3,  4,  3,  3,  3 },
        { 10,  2,  1,  1,  6,  2,  2,  1 },
        {  0, 10,  1,  1,  0,  6,  2,  2 },
        {  0,  0, 10,  2,  0,  0,  6,  2 },
        {  0,  0,  0, 10,  0,  0,  0,  6 },
        { 12,  9,  7,  5,  2,  2,  2,  3 },
        {  0,  0,  0,  0, 12,  9,  7,  5 }
    },
    {
        { 10,  6,  4,  2, 10,  6,  4,  2 },
        { 16,  0,  0,  0, 16,  0,  0,  0 },
        {  0, 16,  0,  0,  0, 16,  0,  0 },
        {  0,  0, 16,  0,  0,  0, 16,  0 },
        {  0,  0,  0, 16,  0,  0,  0, 16 },
        { 10,  6,  4,  2,  0,  0,  0,  0 },
        {  0,  0,  0,  0, 10,  6,  4,  2 }
    },
    {
        {  8,  8,  8,  8,  4,  4,  4,  4 },
        {  8,  0,  0,  0,  4,  0,  0,  0 },
        {  0,  8,  0,  0,  0,  4,  0,  0 },
        {  0,  0,  8,  0,  0,  0,  4,  0 },
        {  0,  0,  0,  8,  0,  0,  0,  4 },
        { 16, 16, 16, 16,  0,  0,  0,  0 },
        {  0,  0,  0,  0, 16, 16, 16, 16 }
    },
    {
        {  2,  1,  1,  0,  1,  1,  1,  1 },
        {  8,  3,  2,  1,  4,  3,  2,  2 },
        {  0,  8,  3,  2,  0,  4,  3,  2 },
        {  0,  0,  8,  3,  0,  0,  4,  3 },
        {  0,  0,  0,  8,  0,  0,  0,  4 },
        { 10,  6,  4,  2,  3,  4,  4,  3 },
        {  0,  0,  0,  0, 10,  6,  4,  3 }
    },
    {
        { 12, 10,  9,  8, 10,  9,  8,  7 },
        { 14,  0,  0,  0, 12,  1,  0,  0 },
        {  0, 14,  0,  0,  0, 12,  0,  0 },
        {  0,  0, 14,  0,  0,  0, 12,  1 },
        {  0,  0,  0, 14,  0,  0,  0, 12 },
        { 14, 12, 11, 10,  0,  0,  1,  1 },
        {  0,  0,  0,  0, 14, 12, 11,  9 }
    }
};
// clang-format on

static inline uint8x8_t filter_intra_predictor(uint8x8_t s0, uint8x8_t s1, uint8x8_t s2, uint8x8_t s3, uint8x8_t s4,
                                               uint8x8_t s5, uint8x8_t s6, const uint8x8_t f0, const uint8x8_t f1,
                                               const uint8x8_t f2, const uint8x8_t f3, const uint8x8_t f4,
                                               const uint8x8_t f5, const uint8x8_t f6) {
    uint16x8_t acc = vmull_u8(s1, f1);
    // First row of each filter has all negative values so subtract.
    acc = vmlsl_u8(acc, s0, f0);
    acc = vmlal_u8(acc, s2, f2);
    acc = vmlal_u8(acc, s3, f3);
    acc = vmlal_u8(acc, s4, f4);
    acc = vmlal_u8(acc, s5, f5);
    acc = vmlal_u8(acc, s6, f6);

    return vqrshrun_n_s16(vreinterpretq_s16_u16(acc), FILTER_INTRA_SCALE_BITS);
}

void svt_av1_filter_intra_predictor_neon(uint8_t* dst, ptrdiff_t stride, TxSize tx_size, const uint8_t* above,
                                         const uint8_t* left, int mode) {
    const int width  = tx_size_wide[tx_size];
    const int height = tx_size_high[tx_size];
    assert(width <= 32 && height <= 32);

    const uint8x8_t f0 = vld1_u8(av1_filter_intra_taps_neon[mode][0]);
    const uint8x8_t f1 = vld1_u8(av1_filter_intra_taps_neon[mode][1]);
    const uint8x8_t f2 = vld1_u8(av1_filter_intra_taps_neon[mode][2]);
    const uint8x8_t f3 = vld1_u8(av1_filter_intra_taps_neon[mode][3]);
    const uint8x8_t f4 = vld1_u8(av1_filter_intra_taps_neon[mode][4]);
    const uint8x8_t f5 = vld1_u8(av1_filter_intra_taps_neon[mode][5]);
    const uint8x8_t f6 = vld1_u8(av1_filter_intra_taps_neon[mode][6]);

    // Computing 4 cols per iteration (instead of 8) for 8x<h> blocks is faster.
    if (width <= 8) {
        uint8x8_t s0 = vdup_n_u8(above[-1]);
        uint8x8_t s5 = vdup_n_u8(left[0]);
        uint8x8_t s6 = vdup_n_u8(left[1]);

        int c = 0;
        do {
            uint8x8_t s1234 = load_unaligned_u8_4x1(above + c);
            uint8x8_t s1    = vdup_lane_u8(s1234, 0);
            uint8x8_t s2    = vdup_lane_u8(s1234, 1);
            uint8x8_t s3    = vdup_lane_u8(s1234, 2);
            uint8x8_t s4    = vdup_lane_u8(s1234, 3);

            uint8x8_t res = filter_intra_predictor(s0, s1, s2, s3, s4, s5, s6, f0, f1, f2, f3, f4, f5, f6);

            store_u8x4_strided_x2(dst + c, stride, res);

            s0 = s4;
            s5 = vdup_lane_u8(res, 3);
            s6 = vdup_lane_u8(res, 7);

            c += 4;
        } while (c < width);

        int r = 2;
        do {
            s0 = vdup_n_u8(left[r - 1]);
            s5 = vdup_n_u8(left[r + 0]);
            s6 = vdup_n_u8(left[r + 1]);

            c = 0;
            do {
                uint8x8_t s1234 = load_u8_4x1(dst + (r - 1) * stride + c);
                uint8x8_t s1    = vdup_lane_u8(s1234, 0);
                uint8x8_t s2    = vdup_lane_u8(s1234, 1);
                uint8x8_t s3    = vdup_lane_u8(s1234, 2);
                uint8x8_t s4    = vdup_lane_u8(s1234, 3);

                uint8x8_t res = filter_intra_predictor(s0, s1, s2, s3, s4, s5, s6, f0, f1, f2, f3, f4, f5, f6);

                store_u8x4_strided_x2(dst + r * stride + c, stride, res);

                s0 = s4;
                s5 = vdup_lane_u8(res, 3);
                s6 = vdup_lane_u8(res, 7);

                c += 4;
            } while (c < width);

            r += 2;
        } while (r < height);
    } else {
        uint8x8_t s0_lo = vdup_n_u8(above[-1]);
        uint8x8_t s5_lo = vdup_n_u8(left[0]);
        uint8x8_t s6_lo = vdup_n_u8(left[1]);

        int c = 0;
        do {
            uint8x8_t s1234 = vld1_u8(above + c);
            uint8x8_t s1_lo = vdup_lane_u8(s1234, 0);
            uint8x8_t s2_lo = vdup_lane_u8(s1234, 1);
            uint8x8_t s3_lo = vdup_lane_u8(s1234, 2);
            uint8x8_t s4_lo = vdup_lane_u8(s1234, 3);

            uint8x8_t res_lo = filter_intra_predictor(
                s0_lo, s1_lo, s2_lo, s3_lo, s4_lo, s5_lo, s6_lo, f0, f1, f2, f3, f4, f5, f6);

            uint8x8_t s0_hi = s4_lo;
            uint8x8_t s1_hi = vdup_lane_u8(s1234, 4);
            uint8x8_t s2_hi = vdup_lane_u8(s1234, 5);
            uint8x8_t s3_hi = vdup_lane_u8(s1234, 6);
            uint8x8_t s4_hi = vdup_lane_u8(s1234, 7);
            uint8x8_t s5_hi = vdup_lane_u8(res_lo, 3);
            uint8x8_t s6_hi = vdup_lane_u8(res_lo, 7);

            uint8x8_t res_hi = filter_intra_predictor(
                s0_hi, s1_hi, s2_hi, s3_hi, s4_hi, s5_hi, s6_hi, f0, f1, f2, f3, f4, f5, f6);

            uint32x2x2_t res = vzip_u32(vreinterpret_u32_u8(res_lo), vreinterpret_u32_u8(res_hi));

            vst1_u8(dst + 0 * stride + c, vreinterpret_u8_u32(res.val[0]));
            vst1_u8(dst + 1 * stride + c, vreinterpret_u8_u32(res.val[1]));

            s0_lo = s4_hi;
            s5_lo = vdup_lane_u8(res_hi, 3);
            s6_lo = vdup_lane_u8(res_hi, 7);
            c += 8;
        } while (c < width);

        int r = 2;
        do {
            s0_lo = vdup_n_u8(left[r - 1]);
            s5_lo = vdup_n_u8(left[r + 0]);
            s6_lo = vdup_n_u8(left[r + 1]);

            c = 0;
            do {
                uint8x8_t s1234 = vld1_u8(dst + (r - 1) * stride + c);
                uint8x8_t s1_lo = vdup_lane_u8(s1234, 0);
                uint8x8_t s2_lo = vdup_lane_u8(s1234, 1);
                uint8x8_t s3_lo = vdup_lane_u8(s1234, 2);
                uint8x8_t s4_lo = vdup_lane_u8(s1234, 3);

                uint8x8_t res_lo = filter_intra_predictor(
                    s0_lo, s1_lo, s2_lo, s3_lo, s4_lo, s5_lo, s6_lo, f0, f1, f2, f3, f4, f5, f6);

                uint8x8_t s0_hi = s4_lo;
                uint8x8_t s1_hi = vdup_lane_u8(s1234, 4);
                uint8x8_t s2_hi = vdup_lane_u8(s1234, 5);
                uint8x8_t s3_hi = vdup_lane_u8(s1234, 6);
                uint8x8_t s4_hi = vdup_lane_u8(s1234, 7);
                uint8x8_t s5_hi = vdup_lane_u8(res_lo, 3);
                uint8x8_t s6_hi = vdup_lane_u8(res_lo, 7);

                uint8x8_t res_hi = filter_intra_predictor(
                    s0_hi, s1_hi, s2_hi, s3_hi, s4_hi, s5_hi, s6_hi, f0, f1, f2, f3, f4, f5, f6);

                uint32x2x2_t res = vzip_u32(vreinterpret_u32_u8(res_lo), vreinterpret_u32_u8(res_hi));

                vst1_u8(dst + (r + 0) * stride + c, vreinterpret_u8_u32(res.val[0]));
                vst1_u8(dst + (r + 1) * stride + c, vreinterpret_u8_u32(res.val[1]));

                s0_lo = s4_hi;
                s5_lo = vdup_lane_u8(res_hi, 3);
                s6_lo = vdup_lane_u8(res_hi, 7);
                c += 8;
            } while (c < width);

            r += 2;
        } while (r < height);
    }
}

/* ---------------------DC PREDICTOR--------------------------- */
/* DC 4x4 */
static inline uint16x8_t dc_load_sum_4(const uint8_t* in) {
    const uint8x8_t  a  = load_u8_4x1(in);
    const uint16x4_t p0 = vpaddl_u8(a);
    const uint16x4_t p1 = vpadd_u16(p0, p0);
    return vcombine_u16(p1, vdup_n_u16(0));
}

static inline void dc_store_4xh(uint8_t* dst, ptrdiff_t stride, int h, uint8x8_t dc) {
    for (int i = 0; i < h; ++i) {
        store_u8_4x1(dst + i * stride, dc);
    }
}

void svt_aom_dc_predictor_4x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_top  = dc_load_sum_4(above);
    const uint16x8_t sum_left = dc_load_sum_4(left);
    const uint16x8_t sum      = vaddq_u16(sum_left, sum_top);
    const uint8x8_t  dc0      = vrshrn_n_u16(sum, 3);
    dc_store_4xh(dst, stride, 4, vdup_lane_u8(dc0, 0));
}

void svt_aom_dc_left_predictor_4x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_left = dc_load_sum_4(left);
    const uint8x8_t  dc0      = vrshrn_n_u16(sum_left, 2);
    (void)above;
    dc_store_4xh(dst, stride, 4, vdup_lane_u8(dc0, 0));
}

void svt_aom_dc_top_predictor_4x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_top = dc_load_sum_4(above);
    const uint8x8_t  dc0     = vrshrn_n_u16(sum_top, 2);
    (void)left;
    dc_store_4xh(dst, stride, 4, vdup_lane_u8(dc0, 0));
}

void svt_aom_dc_128_predictor_4x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x8_t dc0 = vdup_n_u8(0x80);
    (void)above;
    (void)left;
    dc_store_4xh(dst, stride, 4, dc0);
}

/* DC 8x8 */
static inline uint16x8_t dc_load_sum_8(const uint8_t* in) {
    /* This isn't used in the case where we want to load both above and left
   vectors, since we want to avoid performing the reduction twice.*/
    const uint8x8_t  a  = vld1_u8(in);
    const uint16x4_t p0 = vpaddl_u8(a);
    const uint16x4_t p1 = vpadd_u16(p0, p0);
    const uint16x4_t p2 = vpadd_u16(p1, p1);
    return vcombine_u16(p2, vdup_n_u16(0));
}

static inline uint16x8_t horizontal_add_and_broadcast_u16x8(uint16x8_t a) {
    const uint16x8_t b = vpaddq_u16(a, a);
    const uint16x8_t c = vpaddq_u16(b, b);
    return vpaddq_u16(c, c);
}

static inline void dc_store_8xh(uint8_t* dst, ptrdiff_t stride, int h, uint8x8_t dc) {
    for (int i = 0; i < h; ++i) {
        vst1_u8(dst + i * stride, dc);
    }
}

void svt_aom_dc_predictor_8x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x8_t sum_top  = vld1_u8(above);
    const uint8x8_t sum_left = vld1_u8(left);
    uint16x8_t      sum      = vaddl_u8(sum_left, sum_top);
    sum                      = horizontal_add_and_broadcast_u16x8(sum);
    const uint8x8_t dc0      = vrshrn_n_u16(sum, 4);
    dc_store_8xh(dst, stride, 8, vdup_lane_u8(dc0, 0));
}

void svt_aom_dc_left_predictor_8x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_left = dc_load_sum_8(left);
    const uint8x8_t  dc0      = vrshrn_n_u16(sum_left, 3);
    (void)above;
    dc_store_8xh(dst, stride, 8, vdup_lane_u8(dc0, 0));
}

void svt_aom_dc_top_predictor_8x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_top = dc_load_sum_8(above);
    const uint8x8_t  dc0     = vrshrn_n_u16(sum_top, 3);
    (void)left;
    dc_store_8xh(dst, stride, 8, vdup_lane_u8(dc0, 0));
}

void svt_aom_dc_128_predictor_8x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x8_t dc0 = vdup_n_u8(0x80);
    (void)above;
    (void)left;
    dc_store_8xh(dst, stride, 8, dc0);
}

/* DC 16x16 */
static inline uint16x8_t dc_load_partial_sum_16(const uint8_t* in) {
    const uint8x16_t a = vld1q_u8(in);
    return vpaddlq_u8(a);
}

static inline uint16x8_t dc_load_sum_16(const uint8_t* in) {
    return horizontal_add_and_broadcast_u16x8(dc_load_partial_sum_16(in));
}

static inline void dc_store_16xh(uint8_t* dst, ptrdiff_t stride, int h, uint8x16_t dc) {
    for (int i = 0; i < h; ++i) {
        vst1q_u8(dst + i * stride, dc);
    }
}

void svt_aom_dc_predictor_16x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_top  = dc_load_partial_sum_16(above);
    const uint16x8_t sum_left = dc_load_partial_sum_16(left);
    uint16x8_t       sum      = vaddq_u16(sum_left, sum_top);
    sum                       = horizontal_add_and_broadcast_u16x8(sum);
    const uint8x8_t dc0       = vrshrn_n_u16(sum, 5);
    dc_store_16xh(dst, stride, 16, vdupq_lane_u8(dc0, 0));
}

void svt_aom_dc_left_predictor_16x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_left = dc_load_sum_16(left);
    const uint8x8_t  dc0      = vrshrn_n_u16(sum_left, 4);
    (void)above;
    dc_store_16xh(dst, stride, 16, vdupq_lane_u8(dc0, 0));
}

void svt_aom_dc_top_predictor_16x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_top = dc_load_sum_16(above);
    const uint8x8_t  dc0     = vrshrn_n_u16(sum_top, 4);
    (void)left;
    dc_store_16xh(dst, stride, 16, vdupq_lane_u8(dc0, 0));
}

void svt_aom_dc_128_predictor_16x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t dc0 = vdupq_n_u8(0x80);
    (void)above;
    (void)left;
    dc_store_16xh(dst, stride, 16, dc0);
}

/* DC 32x32 */
static inline uint16x8_t dc_load_partial_sum_32(const uint8_t* in) {
    const uint8x16_t a0 = vld1q_u8(in);
    const uint8x16_t a1 = vld1q_u8(in + 16);
    return vpadalq_u8(vpaddlq_u8(a0), a1);
}

static inline uint16x8_t dc_load_sum_32(const uint8_t* in) {
    return horizontal_add_and_broadcast_u16x8(dc_load_partial_sum_32(in));
}

static inline void dc_store_32xh(uint8_t* dst, ptrdiff_t stride, int h, uint8x16_t dc) {
    for (int i = 0; i < h; ++i) {
        vst1q_u8(dst + i * stride, dc);
        vst1q_u8(dst + i * stride + 16, dc);
    }
}

void svt_aom_dc_predictor_32x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_top  = dc_load_partial_sum_32(above);
    const uint16x8_t sum_left = dc_load_partial_sum_32(left);
    uint16x8_t       sum      = vaddq_u16(sum_left, sum_top);
    sum                       = horizontal_add_and_broadcast_u16x8(sum);
    const uint8x8_t dc0       = vrshrn_n_u16(sum, 6);
    dc_store_32xh(dst, stride, 32, vdupq_lane_u8(dc0, 0));
}

void svt_aom_dc_left_predictor_32x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_left = dc_load_sum_32(left);
    const uint8x8_t  dc0      = vrshrn_n_u16(sum_left, 5);
    (void)above;
    dc_store_32xh(dst, stride, 32, vdupq_lane_u8(dc0, 0));
}

void svt_aom_dc_top_predictor_32x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_top = dc_load_sum_32(above);
    const uint8x8_t  dc0     = vrshrn_n_u16(sum_top, 5);
    (void)left;
    dc_store_32xh(dst, stride, 32, vdupq_lane_u8(dc0, 0));
}

void svt_aom_dc_128_predictor_32x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t dc0 = vdupq_n_u8(0x80);
    (void)above;
    (void)left;
    dc_store_32xh(dst, stride, 32, dc0);
}

/* DC 64x64 */
static inline uint16x8_t dc_load_partial_sum_64(const uint8_t* in) {
    const uint8x16_t a0  = vld1q_u8(in);
    const uint8x16_t a1  = vld1q_u8(in + 16);
    const uint8x16_t a2  = vld1q_u8(in + 32);
    const uint8x16_t a3  = vld1q_u8(in + 48);
    const uint16x8_t p01 = vpadalq_u8(vpaddlq_u8(a0), a1);
    const uint16x8_t p23 = vpadalq_u8(vpaddlq_u8(a2), a3);
    return vaddq_u16(p01, p23);
}

static inline uint16x8_t dc_load_sum_64(const uint8_t* in) {
    return horizontal_add_and_broadcast_u16x8(dc_load_partial_sum_64(in));
}

static inline void dc_store_64xh(uint8_t* dst, ptrdiff_t stride, int h, uint8x16_t dc) {
    for (int i = 0; i < h; ++i) {
        vst1q_u8(dst + i * stride, dc);
        vst1q_u8(dst + i * stride + 16, dc);
        vst1q_u8(dst + i * stride + 32, dc);
        vst1q_u8(dst + i * stride + 48, dc);
    }
}

void svt_aom_dc_predictor_64x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_top  = dc_load_partial_sum_64(above);
    const uint16x8_t sum_left = dc_load_partial_sum_64(left);
    uint16x8_t       sum      = vaddq_u16(sum_left, sum_top);
    sum                       = horizontal_add_and_broadcast_u16x8(sum);
    const uint8x8_t dc0       = vrshrn_n_u16(sum, 7);
    dc_store_64xh(dst, stride, 64, vdupq_lane_u8(dc0, 0));
}

void svt_aom_dc_left_predictor_64x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_left = dc_load_sum_64(left);
    const uint8x8_t  dc0      = vrshrn_n_u16(sum_left, 6);
    (void)above;
    dc_store_64xh(dst, stride, 64, vdupq_lane_u8(dc0, 0));
}

void svt_aom_dc_top_predictor_64x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint16x8_t sum_top = dc_load_sum_64(above);
    const uint8x8_t  dc0     = vrshrn_n_u16(sum_top, 6);
    (void)left;
    dc_store_64xh(dst, stride, 64, vdupq_lane_u8(dc0, 0));
}

void svt_aom_dc_128_predictor_64x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t dc0 = vdupq_n_u8(0x80);
    (void)above;
    (void)left;
    dc_store_64xh(dst, stride, 64, dc0);
}

/* DC rectangular cases */
static inline int divide_using_multiply_shift(int num, int shift1, int multiplier, int shift2) {
    const int interm = num >> shift1;
    return interm * multiplier >> shift2;
}

static inline int calculate_dc_from_sum(int bw, int bh, uint32_t sum, int shift1, int multiplier) {
    const int expected_dc = divide_using_multiply_shift(sum + ((bw + bh) >> 1), shift1, multiplier, DC_SHIFT2);
    assert(expected_dc < (1 << 8));
    return expected_dc;
}

void svt_aom_dc_predictor_4x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint8x8_t a   = load_u8_4x1(above);
    uint8x8_t l   = vld1_u8(left);
    uint32_t  sum = vaddlvq_u16(vaddl_u8(a, l));
    uint32_t  dc  = calculate_dc_from_sum(4, 8, sum, 2, DC_MULTIPLIER_1X2);
    dc_store_4xh(dst, stride, 8, vdup_n_u8(dc));
}

void svt_aom_dc_predictor_8x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint8x8_t a   = vld1_u8(above);
    uint8x8_t l   = load_u8_4x1(left);
    uint32_t  sum = vaddlvq_u16(vaddl_u8(a, l));
    uint32_t  dc  = calculate_dc_from_sum(8, 4, sum, 2, DC_MULTIPLIER_1X2);
    dc_store_8xh(dst, stride, 4, vdup_n_u8(dc));
}

void svt_aom_dc_predictor_4x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint8x8_t  a      = load_u8_4x1(above);
    uint8x16_t l      = vld1q_u8(left);
    uint16x8_t sum_al = vaddw_u8(vpaddlq_u8(l), a);
    uint32_t   sum    = vaddlvq_u16(sum_al);
    uint32_t   dc     = calculate_dc_from_sum(4, 16, sum, 2, DC_MULTIPLIER_1X4);
    dc_store_4xh(dst, stride, 16, vdup_n_u8(dc));
}

void svt_aom_dc_predictor_16x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint8x16_t a      = vld1q_u8(above);
    uint8x8_t  l      = load_u8_4x1(left);
    uint16x8_t sum_al = vaddw_u8(vpaddlq_u8(a), l);
    uint32_t   sum    = vaddlvq_u16(sum_al);
    uint32_t   dc     = calculate_dc_from_sum(16, 4, sum, 2, DC_MULTIPLIER_1X4);
    dc_store_16xh(dst, stride, 4, vdupq_n_u8(dc));
}

void svt_aom_dc_predictor_8x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint8x8_t  a      = vld1_u8(above);
    uint8x16_t l      = vld1q_u8(left);
    uint16x8_t sum_al = vaddw_u8(vpaddlq_u8(l), a);
    uint32_t   sum    = vaddlvq_u16(sum_al);
    uint32_t   dc     = calculate_dc_from_sum(8, 16, sum, 3, DC_MULTIPLIER_1X2);
    dc_store_8xh(dst, stride, 16, vdup_n_u8(dc));
}

void svt_aom_dc_predictor_16x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint8x16_t a      = vld1q_u8(above);
    uint8x8_t  l      = vld1_u8(left);
    uint16x8_t sum_al = vaddw_u8(vpaddlq_u8(a), l);
    uint32_t   sum    = vaddlvq_u16(sum_al);
    uint32_t   dc     = calculate_dc_from_sum(16, 8, sum, 3, DC_MULTIPLIER_1X2);
    dc_store_16xh(dst, stride, 8, vdupq_n_u8(dc));
}

void svt_aom_dc_predictor_8x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint8x8_t  a        = vld1_u8(above);
    uint16x8_t sum_left = dc_load_partial_sum_32(left);
    uint16x8_t sum_al   = vaddw_u8(sum_left, a);
    uint32_t   sum      = vaddlvq_u16(sum_al);
    uint32_t   dc       = calculate_dc_from_sum(8, 32, sum, 3, DC_MULTIPLIER_1X4);
    dc_store_8xh(dst, stride, 32, vdup_n_u8(dc));
}

void svt_aom_dc_predictor_32x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint16x8_t sum_top = dc_load_partial_sum_32(above);
    uint8x8_t  l       = vld1_u8(left);
    uint16x8_t sum_al  = vaddw_u8(sum_top, l);
    uint32_t   sum     = vaddlvq_u16(sum_al);
    uint32_t   dc      = calculate_dc_from_sum(32, 8, sum, 3, DC_MULTIPLIER_1X4);
    dc_store_32xh(dst, stride, 8, vdupq_n_u8(dc));
}

void svt_aom_dc_predictor_16x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint16x8_t sum_above = dc_load_partial_sum_16(above);
    uint16x8_t sum_left  = dc_load_partial_sum_32(left);
    uint16x8_t sum_al    = vaddq_u16(sum_left, sum_above);
    uint32_t   sum       = vaddlvq_u16(sum_al);
    uint32_t   dc        = calculate_dc_from_sum(16, 32, sum, 4, DC_MULTIPLIER_1X2);
    dc_store_16xh(dst, stride, 32, vdupq_n_u8(dc));
}

void svt_aom_dc_predictor_32x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint16x8_t sum_above = dc_load_partial_sum_32(above);
    uint16x8_t sum_left  = dc_load_partial_sum_16(left);
    uint16x8_t sum_al    = vaddq_u16(sum_left, sum_above);
    uint32_t   sum       = vaddlvq_u16(sum_al);
    uint32_t   dc        = calculate_dc_from_sum(32, 16, sum, 4, DC_MULTIPLIER_1X2);
    dc_store_32xh(dst, stride, 16, vdupq_n_u8(dc));
}

void svt_aom_dc_predictor_16x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint16x8_t sum_above = dc_load_partial_sum_16(above);
    uint16x8_t sum_left  = dc_load_partial_sum_64(left);
    uint16x8_t sum_al    = vaddq_u16(sum_left, sum_above);
    uint32_t   sum       = vaddlvq_u16(sum_al);
    uint32_t   dc        = calculate_dc_from_sum(16, 64, sum, 4, DC_MULTIPLIER_1X4);
    dc_store_16xh(dst, stride, 64, vdupq_n_u8(dc));
}

void svt_aom_dc_predictor_64x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint16x8_t sum_above = dc_load_partial_sum_64(above);
    uint16x8_t sum_left  = dc_load_partial_sum_16(left);
    uint16x8_t sum_al    = vaddq_u16(sum_above, sum_left);
    uint32_t   sum       = vaddlvq_u16(sum_al);
    uint32_t   dc        = calculate_dc_from_sum(64, 16, sum, 4, DC_MULTIPLIER_1X4);
    dc_store_64xh(dst, stride, 16, vdupq_n_u8(dc));
}

void svt_aom_dc_predictor_32x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint16x8_t sum_above = dc_load_partial_sum_32(above);
    uint16x8_t sum_left  = dc_load_partial_sum_64(left);
    uint16x8_t sum_al    = vaddq_u16(sum_above, sum_left);
    uint32_t   sum       = vaddlvq_u16(sum_al);
    uint32_t   dc        = calculate_dc_from_sum(32, 64, sum, 5, DC_MULTIPLIER_1X2);
    dc_store_32xh(dst, stride, 64, vdupq_n_u8(dc));
}

void svt_aom_dc_predictor_64x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    uint16x8_t sum_above = dc_load_partial_sum_64(above);
    uint16x8_t sum_left  = dc_load_partial_sum_32(left);
    uint16x8_t sum_al    = vaddq_u16(sum_above, sum_left);
    uint32_t   sum       = vaddlvq_u16(sum_al);
    uint32_t   dc        = calculate_dc_from_sum(64, 32, sum, 5, DC_MULTIPLIER_1X2);
    dc_store_64xh(dst, stride, 32, vdupq_n_u8(dc));
}

#define DC_PREDICTOR_128(w, h, q)                                                    \
    void svt_aom_dc_128_predictor_##w##x##h##_neon(                                  \
        uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) { \
        (void)above;                                                                 \
        (void)left;                                                                  \
        dc_store_##w##xh(dst, stride, (h), vdup##q##_n_u8(0x80));                    \
    }

DC_PREDICTOR_128(4, 8, )
DC_PREDICTOR_128(4, 16, )
DC_PREDICTOR_128(8, 4, )
DC_PREDICTOR_128(8, 16, )
DC_PREDICTOR_128(8, 32, )
DC_PREDICTOR_128(16, 4, q)
DC_PREDICTOR_128(16, 8, q)
DC_PREDICTOR_128(16, 32, q)
DC_PREDICTOR_128(16, 64, q)
DC_PREDICTOR_128(32, 8, q)
DC_PREDICTOR_128(32, 16, q)
DC_PREDICTOR_128(32, 64, q)
DC_PREDICTOR_128(64, 32, q)
DC_PREDICTOR_128(64, 16, q)

#define DC_PREDICTOR_LEFT(w, h, shift, q)                                            \
    void svt_aom_dc_left_predictor_##w##x##h##_neon(                                 \
        uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) { \
        (void)above;                                                                 \
        const uint16x8_t sum = dc_load_sum_##h(left);                                \
        const uint8x8_t  dc0 = vrshrn_n_u16(sum, (shift));                           \
        dc_store_##w##xh(dst, stride, (h), vdup##q##_lane_u8(dc0, 0));               \
    }

DC_PREDICTOR_LEFT(4, 8, 3, )
DC_PREDICTOR_LEFT(8, 4, 2, )
DC_PREDICTOR_LEFT(8, 16, 4, )
DC_PREDICTOR_LEFT(16, 8, 3, q)
DC_PREDICTOR_LEFT(16, 32, 5, q)
DC_PREDICTOR_LEFT(32, 16, 4, q)
DC_PREDICTOR_LEFT(32, 64, 6, q)
DC_PREDICTOR_LEFT(64, 32, 5, q)
DC_PREDICTOR_LEFT(4, 16, 4, )
DC_PREDICTOR_LEFT(16, 4, 2, q)
DC_PREDICTOR_LEFT(8, 32, 5, )
DC_PREDICTOR_LEFT(32, 8, 3, q)
DC_PREDICTOR_LEFT(16, 64, 6, q)
DC_PREDICTOR_LEFT(64, 16, 4, q)

#define DC_PREDICTOR_TOP(w, h, shift, q)                                             \
    void svt_aom_dc_top_predictor_##w##x##h##_neon(                                  \
        uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) { \
        (void)left;                                                                  \
        const uint16x8_t sum = dc_load_sum_##w(above);                               \
        const uint8x8_t  dc0 = vrshrn_n_u16(sum, (shift));                           \
        dc_store_##w##xh(dst, stride, (h), vdup##q##_lane_u8(dc0, 0));               \
    }

DC_PREDICTOR_TOP(4, 8, 2, )
DC_PREDICTOR_TOP(4, 16, 2, )
DC_PREDICTOR_TOP(8, 4, 3, )
DC_PREDICTOR_TOP(8, 16, 3, )
DC_PREDICTOR_TOP(8, 32, 3, )
DC_PREDICTOR_TOP(16, 4, 4, q)
DC_PREDICTOR_TOP(16, 8, 4, q)
DC_PREDICTOR_TOP(16, 32, 4, q)
DC_PREDICTOR_TOP(16, 64, 4, q)
DC_PREDICTOR_TOP(32, 8, 5, q)
DC_PREDICTOR_TOP(32, 16, 5, q)
DC_PREDICTOR_TOP(32, 64, 5, q)
DC_PREDICTOR_TOP(64, 16, 6, q)
DC_PREDICTOR_TOP(64, 32, 6, q)

/* ---------------------SMOOTH PREDICTOR--------------------------- */
/* 256 - v = vneg_s8(v) */
static inline uint8x8_t negate_s8(const uint8x8_t v) {
    return vreinterpret_u8_s8(vneg_s8(vreinterpret_s8_u8(v)));
}

static inline void smooth_4xh_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* const top_row,
                                   const uint8_t* const left_column, const int height) {
    const uint8_t        top_right   = top_row[3];
    const uint8_t        bottom_left = left_column[height - 1];
    const uint8_t* const weights_y   = sm_weight_arrays + height;

    uint8x8_t        top_v            = load_u8_4x1(top_row);
    const uint8x8_t  top_right_v      = vdup_n_u8(top_right);
    const uint8x8_t  bottom_left_v    = vdup_n_u8(bottom_left);
    uint8x8_t        weights_x_v      = load_u8_4x1(sm_weight_arrays + 4);
    const uint8x8_t  scaled_weights_x = negate_s8(weights_x_v);
    const uint16x8_t weighted_tr      = vmull_u8(scaled_weights_x, top_right_v);

    assert(height > 0);
    int y = 0;
    do {
        const uint8x8_t  left_v           = vdup_n_u8(left_column[y]);
        const uint8x8_t  weights_y_v      = vdup_n_u8(weights_y[y]);
        const uint8x8_t  scaled_weights_y = negate_s8(weights_y_v);
        const uint16x8_t weighted_bl      = vmull_u8(scaled_weights_y, bottom_left_v);
        const uint16x8_t weighted_top_bl  = vmlal_u8(weighted_bl, weights_y_v, top_v);
        const uint16x8_t weighted_left_tr = vmlal_u8(weighted_tr, weights_x_v, left_v);
        /* Maximum value of each parameter: 0xFF00 */
        const uint16x8_t avg    = vhaddq_u16(weighted_top_bl, weighted_left_tr);
        const uint8x8_t  result = vmovn_u16(vrshlq_u16(avg, vdupq_n_s16(-sm_weight_log2_scale)));

        vst1_lane_u32((uint32_t*)dst, vreinterpret_u32_u8(result), 0);
        dst += stride;
    } while (++y != height);
}

static inline uint8x8_t calculate_pred(const uint16x8_t weighted_top_bl, const uint16x8_t weighted_left_tr) {
    /* Maximum value of each parameter: 0xFF00 */
    const uint16x8_t avg = vhaddq_u16(weighted_top_bl, weighted_left_tr);
    return vmovn_u16(vrshlq_u16(avg, vdupq_n_s16(-sm_weight_log2_scale)));
}

static inline uint8x8_t calculate_weights_and_pred(const uint8x8_t top, const uint8x8_t left,
                                                   const uint16x8_t weighted_tr, const uint8x8_t bottom_left,
                                                   const uint8x8_t weights_x, const uint8x8_t scaled_weights_y,
                                                   const uint8x8_t weights_y) {
    const uint16x8_t weighted_top     = vmull_u8(weights_y, top);
    const uint16x8_t weighted_top_bl  = vmlal_u8(weighted_top, scaled_weights_y, bottom_left);
    const uint16x8_t weighted_left_tr = vmlal_u8(weighted_tr, weights_x, left);
    return calculate_pred(weighted_top_bl, weighted_left_tr);
}

static inline void smooth_8xh_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* const top_row,
                                   const uint8_t* const left_column, const int height) {
    const uint8_t        top_right   = top_row[7];
    const uint8_t        bottom_left = left_column[height - 1];
    const uint8_t* const weights_y   = sm_weight_arrays + height;

    const uint8x8_t  top_v            = vld1_u8(top_row);
    const uint8x8_t  top_right_v      = vdup_n_u8(top_right);
    const uint8x8_t  bottom_left_v    = vdup_n_u8(bottom_left);
    const uint8x8_t  weights_x_v      = vld1_u8(sm_weight_arrays + 8);
    const uint8x8_t  scaled_weights_x = negate_s8(weights_x_v);
    const uint16x8_t weighted_tr      = vmull_u8(scaled_weights_x, top_right_v);

    assert(height > 0);
    int y = 0;
    do {
        const uint8x8_t left_v           = vdup_n_u8(left_column[y]);
        const uint8x8_t weights_y_v      = vdup_n_u8(weights_y[y]);
        const uint8x8_t scaled_weights_y = negate_s8(weights_y_v);
        const uint8x8_t result           = calculate_weights_and_pred(
            top_v, left_v, weighted_tr, bottom_left_v, weights_x_v, scaled_weights_y, weights_y_v);

        vst1_u8(dst, result);
        dst += stride;
    } while (++y != height);
}

#define SMOOTH_NXM(W, H)                                                               \
    void svt_aom_smooth_predictor_##W##x##H##_neon(                                    \
        uint8_t* dst, ptrdiff_t y_stride, const uint8_t* above, const uint8_t* left) { \
        smooth_##W##xh_neon(dst, y_stride, above, left, H);                            \
    }

SMOOTH_NXM(4, 4)
SMOOTH_NXM(4, 8)
SMOOTH_NXM(8, 4)
SMOOTH_NXM(8, 8)
SMOOTH_NXM(4, 16)
SMOOTH_NXM(8, 16)
SMOOTH_NXM(8, 32)

static inline uint8x16_t calculate_weights_and_predq(const uint8x16_t top, const uint8x8_t left,
                                                     const uint8x8_t top_right, const uint8x8_t weights_y,
                                                     const uint8x16_t weights_x, const uint8x16_t scaled_weights_x,
                                                     const uint16x8_t weighted_bl) {
    const uint16x8_t weighted_top_bl_low  = vmlal_u8(weighted_bl, weights_y, vget_low_u8(top));
    const uint16x8_t weighted_left_low    = vmull_u8(vget_low_u8(weights_x), left);
    const uint16x8_t weighted_left_tr_low = vmlal_u8(weighted_left_low, vget_low_u8(scaled_weights_x), top_right);
    const uint8x8_t  result_low           = calculate_pred(weighted_top_bl_low, weighted_left_tr_low);

    const uint16x8_t weighted_top_bl_high  = vmlal_u8(weighted_bl, weights_y, vget_high_u8(top));
    const uint16x8_t weighted_left_high    = vmull_u8(vget_high_u8(weights_x), left);
    const uint16x8_t weighted_left_tr_high = vmlal_u8(weighted_left_high, vget_high_u8(scaled_weights_x), top_right);
    const uint8x8_t  result_high           = calculate_pred(weighted_top_bl_high, weighted_left_tr_high);

    return vcombine_u8(result_low, result_high);
}

/* 256 - v = vneg_s8(v) */
static inline uint8x16_t negate_s8q(const uint8x16_t v) {
    return vreinterpretq_u8_s8(vnegq_s8(vreinterpretq_s8_u8(v)));
}

/* For width 16 and above. */
#define SMOOTH_PREDICTOR_WIDE(W)                                                                                    \
    static inline void smooth_##W##xh_wide_neon(uint8_t*             dst,                                           \
                                                ptrdiff_t            stride,                                        \
                                                const uint8_t* const top_row,                                       \
                                                const uint8_t* const left_column,                                   \
                                                const int            height) {                                                 \
        const uint8_t        top_right   = top_row[(W) - 1];                                                        \
        const uint8_t        bottom_left = left_column[height - 1];                                                 \
        const uint8_t* const weights_y   = sm_weight_arrays + height;                                               \
                                                                                                                    \
        uint8x16_t top_v[4];                                                                                        \
        top_v[0] = vld1q_u8(top_row);                                                                               \
        if ((W) > 16) {                                                                                             \
            top_v[1] = vld1q_u8(top_row + 16);                                                                      \
            if ((W) == 64) {                                                                                        \
                top_v[2] = vld1q_u8(top_row + 32);                                                                  \
                top_v[3] = vld1q_u8(top_row + 48);                                                                  \
            }                                                                                                       \
        }                                                                                                           \
                                                                                                                    \
        const uint8x8_t top_right_v   = vdup_n_u8(top_right);                                                       \
        const uint8x8_t bottom_left_v = vdup_n_u8(bottom_left);                                                     \
                                                                                                                    \
        uint8x16_t weights_x_v[4];                                                                                  \
        weights_x_v[0] = vld1q_u8(sm_weight_arrays + (W));                                                          \
        if ((W) > 16) {                                                                                             \
            weights_x_v[1] = vld1q_u8(sm_weight_arrays + (W) + 16);                                                 \
            if ((W) == 64) {                                                                                        \
                weights_x_v[2] = vld1q_u8(sm_weight_arrays + (W) + 32);                                             \
                weights_x_v[3] = vld1q_u8(sm_weight_arrays + (W) + 48);                                             \
            }                                                                                                       \
        }                                                                                                           \
                                                                                                                    \
        uint8x16_t scaled_weights_x[4];                                                                             \
        scaled_weights_x[0] = negate_s8q(weights_x_v[0]);                                                           \
        if ((W) > 16) {                                                                                             \
            scaled_weights_x[1] = negate_s8q(weights_x_v[1]);                                                       \
            if ((W) == 64) {                                                                                        \
                scaled_weights_x[2] = negate_s8q(weights_x_v[2]);                                                   \
                scaled_weights_x[3] = negate_s8q(weights_x_v[3]);                                                   \
            }                                                                                                       \
        }                                                                                                           \
                                                                                                                    \
        for (int y = 0; y < height; ++y) {                                                                          \
            const uint8x8_t  left_v           = vdup_n_u8(left_column[y]);                                          \
            const uint8x8_t  weights_y_v      = vdup_n_u8(weights_y[y]);                                            \
            const uint8x8_t  scaled_weights_y = negate_s8(weights_y_v);                                             \
            const uint16x8_t weighted_bl      = vmull_u8(scaled_weights_y, bottom_left_v);                          \
                                                                                                                    \
            vst1q_u8(                                                                                               \
                dst,                                                                                                \
                calculate_weights_and_predq(                                                                        \
                    top_v[0], left_v, top_right_v, weights_y_v, weights_x_v[0], scaled_weights_x[0], weighted_bl)); \
                                                                                                                    \
            if ((W) > 16) {                                                                                         \
                vst1q_u8(dst + 16,                                                                                  \
                         calculate_weights_and_predq(top_v[1],                                                      \
                                                     left_v,                                                        \
                                                     top_right_v,                                                   \
                                                     weights_y_v,                                                   \
                                                     weights_x_v[1],                                                \
                                                     scaled_weights_x[1],                                           \
                                                     weighted_bl));                                                 \
                if ((W) == 64) {                                                                                    \
                    vst1q_u8(dst + 32,                                                                              \
                             calculate_weights_and_predq(top_v[2],                                                  \
                                                         left_v,                                                    \
                                                         top_right_v,                                               \
                                                         weights_y_v,                                               \
                                                         weights_x_v[2],                                            \
                                                         scaled_weights_x[2],                                       \
                                                         weighted_bl));                                             \
                    vst1q_u8(dst + 48,                                                                              \
                             calculate_weights_and_predq(top_v[3],                                                  \
                                                         left_v,                                                    \
                                                         top_right_v,                                               \
                                                         weights_y_v,                                               \
                                                         weights_x_v[3],                                            \
                                                         scaled_weights_x[3],                                       \
                                                         weighted_bl));                                             \
                }                                                                                                   \
            }                                                                                                       \
                                                                                                                    \
            dst += stride;                                                                                          \
        }                                                                                                           \
    }

SMOOTH_PREDICTOR_WIDE(16)
SMOOTH_PREDICTOR_WIDE(32)
SMOOTH_PREDICTOR_WIDE(64)

#define SMOOTH_NXM_WIDE(W, H)                                                          \
    void svt_aom_smooth_predictor_##W##x##H##_neon(                                    \
        uint8_t* dst, ptrdiff_t y_stride, const uint8_t* above, const uint8_t* left) { \
        smooth_##W##xh_wide_neon(dst, y_stride, above, left, H);                       \
    }

SMOOTH_NXM_WIDE(16, 4)
SMOOTH_NXM_WIDE(16, 8)
SMOOTH_NXM_WIDE(16, 16)
SMOOTH_NXM_WIDE(16, 32)
SMOOTH_NXM_WIDE(16, 64)
SMOOTH_NXM_WIDE(32, 8)
SMOOTH_NXM_WIDE(32, 16)
SMOOTH_NXM_WIDE(32, 32)
SMOOTH_NXM_WIDE(32, 64)
SMOOTH_NXM_WIDE(64, 16)
SMOOTH_NXM_WIDE(64, 32)
SMOOTH_NXM_WIDE(64, 64)

/* For widths 4 and 8. */
#define SMOOTH_H_PREDICTOR(W)                                                                                    \
    static inline void smooth_h_##W##xh_neon(uint8_t*             dst,                                           \
                                             ptrdiff_t            stride,                                        \
                                             const uint8_t* const top_row,                                       \
                                             const uint8_t* const left_column,                                   \
                                             const int            height) {                                                 \
        const uint8_t top_right = top_row[(W) - 1];                                                              \
                                                                                                                 \
        const uint8x8_t top_right_v = vdup_n_u8(top_right);                                                      \
        /* Over-reads for 4xN but still within the array. */                                                     \
        const uint8x8_t  weights_x        = vld1_u8(sm_weight_arrays + (W));                                     \
        const uint8x8_t  scaled_weights_x = negate_s8(weights_x);                                                \
        const uint16x8_t weighted_tr      = vmull_u8(scaled_weights_x, top_right_v);                             \
                                                                                                                 \
        assert(height > 0);                                                                                      \
        int y = 0;                                                                                               \
        do {                                                                                                     \
            const uint8x8_t  left_v           = vdup_n_u8(left_column[y]);                                       \
            const uint16x8_t weighted_left_tr = vmlal_u8(weighted_tr, weights_x, left_v);                        \
            const uint8x8_t  pred = vmovn_u16(vrshlq_u16(weighted_left_tr, vdupq_n_s16(-sm_weight_log2_scale))); \
                                                                                                                 \
            if ((W) == 4) {                                                                                      \
                vst1_lane_u32((uint32_t*)dst, vreinterpret_u32_u8(pred), 0);                                     \
            } else { /* width == 8 */                                                                            \
                vst1_u8(dst, pred);                                                                              \
            }                                                                                                    \
            dst += stride;                                                                                       \
        } while (++y != height);                                                                                 \
    }

SMOOTH_H_PREDICTOR(4)
SMOOTH_H_PREDICTOR(8)

#define SMOOTH_H_NXM(W, H)                                                             \
    void svt_aom_smooth_h_predictor_##W##x##H##_neon(                                  \
        uint8_t* dst, ptrdiff_t y_stride, const uint8_t* above, const uint8_t* left) { \
        smooth_h_##W##xh_neon(dst, y_stride, above, left, H);                          \
    }

SMOOTH_H_NXM(4, 4)
SMOOTH_H_NXM(4, 8)
SMOOTH_H_NXM(4, 16)
SMOOTH_H_NXM(8, 4)
SMOOTH_H_NXM(8, 8)
SMOOTH_H_NXM(8, 16)
SMOOTH_H_NXM(8, 32)

static inline uint8x16_t calculate_horizontal_weights_and_pred(const uint8x8_t left, const uint8x8_t top_right,
                                                               const uint8x16_t weights_x,
                                                               const uint8x16_t scaled_weights_x) {
    const uint16x8_t weighted_left_low    = vmull_u8(vget_low_u8(weights_x), left);
    const uint16x8_t weighted_left_tr_low = vmlal_u8(weighted_left_low, vget_low_u8(scaled_weights_x), top_right);
    const uint8x8_t  pred_scaled_low = vmovn_u16(vrshlq_u16(weighted_left_tr_low, vdupq_n_s16(-sm_weight_log2_scale)));

    const uint16x8_t weighted_left_high    = vmull_u8(vget_high_u8(weights_x), left);
    const uint16x8_t weighted_left_tr_high = vmlal_u8(weighted_left_high, vget_high_u8(scaled_weights_x), top_right);
    const uint8x8_t pred_scaled_high = vmovn_u16(vrshlq_u16(weighted_left_tr_high, vdupq_n_s16(-sm_weight_log2_scale)));

    return vcombine_u8(pred_scaled_low, pred_scaled_high);
}

/* For width 16 and above. */
#define SMOOTH_H_PREDICTOR_WIDE(W)                                                   \
    static inline void smooth_h_##W##xh_wide_neon(uint8_t*             dst,          \
                                                  ptrdiff_t            stride,       \
                                                  const uint8_t* const top_row,      \
                                                  const uint8_t* const left_column,  \
                                                  const int            height) {                \
        const uint8_t top_right = top_row[(W) - 1];                                  \
                                                                                     \
        const uint8x8_t top_right_v = vdup_n_u8(top_right);                          \
                                                                                     \
        uint8x16_t weights_x[4];                                                     \
        weights_x[0] = vld1q_u8(sm_weight_arrays + (W));                             \
        if ((W) > 16) {                                                              \
            weights_x[1] = vld1q_u8(sm_weight_arrays + (W) + 16);                    \
            if ((W) == 64) {                                                         \
                weights_x[2] = vld1q_u8(sm_weight_arrays + (W) + 32);                \
                weights_x[3] = vld1q_u8(sm_weight_arrays + (W) + 48);                \
            }                                                                        \
        }                                                                            \
                                                                                     \
        uint8x16_t scaled_weights_x[4];                                              \
        scaled_weights_x[0] = negate_s8q(weights_x[0]);                              \
        if ((W) > 16) {                                                              \
            scaled_weights_x[1] = negate_s8q(weights_x[1]);                          \
            if ((W) == 64) {                                                         \
                scaled_weights_x[2] = negate_s8q(weights_x[2]);                      \
                scaled_weights_x[3] = negate_s8q(weights_x[3]);                      \
            }                                                                        \
        }                                                                            \
                                                                                     \
        assert(height > 0);                                                          \
        int y = 0;                                                                   \
        do {                                                                         \
            const uint8x8_t left_v = vdup_n_u8(left_column[y]);                      \
                                                                                     \
            const uint8x16_t pred_0 = calculate_horizontal_weights_and_pred(         \
                left_v, top_right_v, weights_x[0], scaled_weights_x[0]);             \
            vst1q_u8(dst, pred_0);                                                   \
                                                                                     \
            if ((W) > 16) {                                                          \
                const uint8x16_t pred_1 = calculate_horizontal_weights_and_pred(     \
                    left_v, top_right_v, weights_x[1], scaled_weights_x[1]);         \
                vst1q_u8(dst + 16, pred_1);                                          \
                                                                                     \
                if ((W) == 64) {                                                     \
                    const uint8x16_t pred_2 = calculate_horizontal_weights_and_pred( \
                        left_v, top_right_v, weights_x[2], scaled_weights_x[2]);     \
                    vst1q_u8(dst + 32, pred_2);                                      \
                                                                                     \
                    const uint8x16_t pred_3 = calculate_horizontal_weights_and_pred( \
                        left_v, top_right_v, weights_x[3], scaled_weights_x[3]);     \
                    vst1q_u8(dst + 48, pred_3);                                      \
                }                                                                    \
            }                                                                        \
            dst += stride;                                                           \
        } while (++y != height);                                                     \
    }

SMOOTH_H_PREDICTOR_WIDE(16)
SMOOTH_H_PREDICTOR_WIDE(32)
SMOOTH_H_PREDICTOR_WIDE(64)

#define SMOOTH_H_NXM_WIDE(W, H)                                                        \
    void svt_aom_smooth_h_predictor_##W##x##H##_neon(                                  \
        uint8_t* dst, ptrdiff_t y_stride, const uint8_t* above, const uint8_t* left) { \
        smooth_h_##W##xh_wide_neon(dst, y_stride, above, left, H);                     \
    }

SMOOTH_H_NXM_WIDE(16, 4)
SMOOTH_H_NXM_WIDE(16, 8)
SMOOTH_H_NXM_WIDE(16, 16)
SMOOTH_H_NXM_WIDE(16, 32)
SMOOTH_H_NXM_WIDE(16, 64)
SMOOTH_H_NXM_WIDE(32, 8)
SMOOTH_H_NXM_WIDE(32, 16)
SMOOTH_H_NXM_WIDE(32, 32)
SMOOTH_H_NXM_WIDE(32, 64)
SMOOTH_H_NXM_WIDE(64, 16)
SMOOTH_H_NXM_WIDE(64, 32)
SMOOTH_H_NXM_WIDE(64, 64)

/* For widths 4 and 8. */
#define SMOOTH_V_PREDICTOR(W)                                                                                   \
    static inline void smooth_v_##W##xh_neon(uint8_t*             dst,                                          \
                                             ptrdiff_t            stride,                                       \
                                             const uint8_t* const top_row,                                      \
                                             const uint8_t* const left_column,                                  \
                                             const int            height) {                                                \
        uint8x8_t            top_v;                                                                             \
        const uint8_t        bottom_left = left_column[height - 1];                                             \
        const uint8_t* const weights_y   = sm_weight_arrays + height;                                           \
                                                                                                                \
        if ((W) == 4) {                                                                                         \
            top_v = load_u8_4x1(top_row);                                                                       \
        } else { /* width == 8 */                                                                               \
            top_v = vld1_u8(top_row);                                                                           \
        }                                                                                                       \
                                                                                                                \
        const uint8x8_t bottom_left_v = vdup_n_u8(bottom_left);                                                 \
                                                                                                                \
        assert(height > 0);                                                                                     \
        int y = 0;                                                                                              \
        do {                                                                                                    \
            const uint8x8_t weights_y_v      = vdup_n_u8(weights_y[y]);                                         \
            const uint8x8_t scaled_weights_y = negate_s8(weights_y_v);                                          \
                                                                                                                \
            const uint16x8_t weighted_top    = vmull_u8(weights_y_v, top_v);                                    \
            const uint16x8_t weighted_top_bl = vmlal_u8(weighted_top, scaled_weights_y, bottom_left_v);         \
            const uint8x8_t  pred = vmovn_u16(vrshlq_u16(weighted_top_bl, vdupq_n_s16(-sm_weight_log2_scale))); \
                                                                                                                \
            if ((W) == 4) {                                                                                     \
                vst1_lane_u32((uint32_t*)dst, vreinterpret_u32_u8(pred), 0);                                    \
            } else { /* width == 8 */                                                                           \
                vst1_u8(dst, pred);                                                                             \
            }                                                                                                   \
            dst += stride;                                                                                      \
        } while (++y != height);                                                                                \
    }

SMOOTH_V_PREDICTOR(4)
SMOOTH_V_PREDICTOR(8)

#define SMOOTH_V_NXM(W, H)                                                             \
    void svt_aom_smooth_v_predictor_##W##x##H##_neon(                                  \
        uint8_t* dst, ptrdiff_t y_stride, const uint8_t* above, const uint8_t* left) { \
        smooth_v_##W##xh_neon(dst, y_stride, above, left, H);                          \
    }

SMOOTH_V_NXM(4, 4)
SMOOTH_V_NXM(4, 8)
SMOOTH_V_NXM(4, 16)
SMOOTH_V_NXM(8, 4)
SMOOTH_V_NXM(8, 8)
SMOOTH_V_NXM(8, 16)
SMOOTH_V_NXM(8, 32)

static inline uint8x16_t calculate_vertical_weights_and_pred(const uint8x16_t top, const uint8x8_t weights_y,
                                                             const uint16x8_t weighted_bl) {
    const uint16x8_t pred_low         = vmlal_u8(weighted_bl, weights_y, vget_low_u8(top));
    const uint16x8_t pred_high        = vmlal_u8(weighted_bl, weights_y, vget_high_u8(top));
    const uint8x8_t  pred_scaled_low  = vmovn_u16(vrshlq_u16(pred_low, vdupq_n_s16(-sm_weight_log2_scale)));
    const uint8x8_t  pred_scaled_high = vmovn_u16(vrshlq_u16(pred_high, vdupq_n_s16(-sm_weight_log2_scale)));
    return vcombine_u8(pred_scaled_low, pred_scaled_high);
}

/* For width 16 and above. */
#define SMOOTH_V_PREDICTOR_WIDE(W)                                                                                     \
    static inline void smooth_v_##W##xh_wide_neon(uint8_t*             dst,                                            \
                                                  ptrdiff_t            stride,                                         \
                                                  const uint8_t* const top_row,                                        \
                                                  const uint8_t* const left_column,                                    \
                                                  const int            height) {                                                  \
        const uint8_t        bottom_left = left_column[height - 1];                                                    \
        const uint8_t* const weights_y   = sm_weight_arrays + height;                                                  \
                                                                                                                       \
        uint8x16_t top_v[4];                                                                                           \
        top_v[0] = vld1q_u8(top_row);                                                                                  \
        if ((W) > 16) {                                                                                                \
            top_v[1] = vld1q_u8(top_row + 16);                                                                         \
            if ((W) == 64) {                                                                                           \
                top_v[2] = vld1q_u8(top_row + 32);                                                                     \
                top_v[3] = vld1q_u8(top_row + 48);                                                                     \
            }                                                                                                          \
        }                                                                                                              \
                                                                                                                       \
        const uint8x8_t bottom_left_v = vdup_n_u8(bottom_left);                                                        \
                                                                                                                       \
        assert(height > 0);                                                                                            \
        int y = 0;                                                                                                     \
        do {                                                                                                           \
            const uint8x8_t  weights_y_v      = vdup_n_u8(weights_y[y]);                                               \
            const uint8x8_t  scaled_weights_y = negate_s8(weights_y_v);                                                \
            const uint16x8_t weighted_bl      = vmull_u8(scaled_weights_y, bottom_left_v);                             \
                                                                                                                       \
            const uint8x16_t pred_0 = calculate_vertical_weights_and_pred(top_v[0], weights_y_v, weighted_bl);         \
            vst1q_u8(dst, pred_0);                                                                                     \
                                                                                                                       \
            if ((W) > 16) {                                                                                            \
                const uint8x16_t pred_1 = calculate_vertical_weights_and_pred(top_v[1], weights_y_v, weighted_bl);     \
                vst1q_u8(dst + 16, pred_1);                                                                            \
                                                                                                                       \
                if ((W) == 64) {                                                                                       \
                    const uint8x16_t pred_2 = calculate_vertical_weights_and_pred(top_v[2], weights_y_v, weighted_bl); \
                    vst1q_u8(dst + 32, pred_2);                                                                        \
                                                                                                                       \
                    const uint8x16_t pred_3 = calculate_vertical_weights_and_pred(top_v[3], weights_y_v, weighted_bl); \
                    vst1q_u8(dst + 48, pred_3);                                                                        \
                }                                                                                                      \
            }                                                                                                          \
                                                                                                                       \
            dst += stride;                                                                                             \
        } while (++y != height);                                                                                       \
    }

SMOOTH_V_PREDICTOR_WIDE(16)
SMOOTH_V_PREDICTOR_WIDE(32)
SMOOTH_V_PREDICTOR_WIDE(64)

#define SMOOTH_V_NXM_WIDE(W, H)                                                        \
    void svt_aom_smooth_v_predictor_##W##x##H##_neon(                                  \
        uint8_t* dst, ptrdiff_t y_stride, const uint8_t* above, const uint8_t* left) { \
        smooth_v_##W##xh_wide_neon(dst, y_stride, above, left, H);                     \
    }

SMOOTH_V_NXM_WIDE(16, 4)
SMOOTH_V_NXM_WIDE(16, 8)
SMOOTH_V_NXM_WIDE(16, 16)
SMOOTH_V_NXM_WIDE(16, 32)
SMOOTH_V_NXM_WIDE(16, 64)
SMOOTH_V_NXM_WIDE(32, 8)
SMOOTH_V_NXM_WIDE(32, 16)
SMOOTH_V_NXM_WIDE(32, 32)
SMOOTH_V_NXM_WIDE(32, 64)
SMOOTH_V_NXM_WIDE(64, 16)
SMOOTH_V_NXM_WIDE(64, 32)
SMOOTH_V_NXM_WIDE(64, 64)

/* ---------------------V PREDICTOR--------------------------- */
static inline void v_store_4xh(uint8_t* dst, ptrdiff_t stride, int h, uint8x8_t d0) {
    for (int i = 0; i < h; ++i) {
        store_u8_4x1(dst + i * stride, d0);
    }
}

static inline void v_store_8xh(uint8_t* dst, ptrdiff_t stride, int h, uint8x8_t d0) {
    for (int i = 0; i < h; ++i) {
        vst1_u8(dst + i * stride, d0);
    }
}

static inline void v_store_16xh(uint8_t* dst, ptrdiff_t stride, int h, uint8x16_t d0) {
    for (int i = 0; i < h; ++i) {
        vst1q_u8(dst + i * stride, d0);
    }
}

static inline void v_store_32xh(uint8_t* dst, ptrdiff_t stride, int h, uint8x16_t d0, uint8x16_t d1) {
    for (int i = 0; i < h; ++i) {
        vst1q_u8(dst + 0, d0);
        vst1q_u8(dst + 16, d1);
        dst += stride;
    }
}

static inline void v_store_64xh(uint8_t* dst, ptrdiff_t stride, int h, uint8x16_t d0, uint8x16_t d1, uint8x16_t d2,
                                uint8x16_t d3) {
    for (int i = 0; i < h; ++i) {
        vst1q_u8(dst + 0, d0);
        vst1q_u8(dst + 16, d1);
        vst1q_u8(dst + 32, d2);
        vst1q_u8(dst + 48, d3);
        dst += stride;
    }
}

void svt_aom_v_predictor_4x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)left;
    v_store_4xh(dst, stride, 4, load_u8_4x1(above));
}

void svt_aom_v_predictor_8x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)left;
    v_store_8xh(dst, stride, 8, vld1_u8(above));
}

void svt_aom_v_predictor_16x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)left;
    v_store_16xh(dst, stride, 16, vld1q_u8(above));
}

void svt_aom_v_predictor_32x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(above);
    const uint8x16_t d1 = vld1q_u8(above + 16);
    (void)left;
    v_store_32xh(dst, stride, 32, d0, d1);
}

void svt_aom_v_predictor_4x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)left;
    v_store_4xh(dst, stride, 8, load_u8_4x1(above));
}

void svt_aom_v_predictor_4x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)left;
    v_store_4xh(dst, stride, 16, load_u8_4x1(above));
}

void svt_aom_v_predictor_8x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)left;
    v_store_8xh(dst, stride, 4, vld1_u8(above));
}

void svt_aom_v_predictor_8x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)left;
    v_store_8xh(dst, stride, 16, vld1_u8(above));
}

void svt_aom_v_predictor_8x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)left;
    v_store_8xh(dst, stride, 32, vld1_u8(above));
}

void svt_aom_v_predictor_16x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)left;
    v_store_16xh(dst, stride, 4, vld1q_u8(above));
}

void svt_aom_v_predictor_16x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)left;
    v_store_16xh(dst, stride, 8, vld1q_u8(above));
}

void svt_aom_v_predictor_16x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)left;
    v_store_16xh(dst, stride, 32, vld1q_u8(above));
}

void svt_aom_v_predictor_16x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)left;
    v_store_16xh(dst, stride, 64, vld1q_u8(above));
}

void svt_aom_v_predictor_32x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(above);
    const uint8x16_t d1 = vld1q_u8(above + 16);
    (void)left;
    v_store_32xh(dst, stride, 8, d0, d1);
}

void svt_aom_v_predictor_32x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(above);
    const uint8x16_t d1 = vld1q_u8(above + 16);
    (void)left;
    v_store_32xh(dst, stride, 16, d0, d1);
}

void svt_aom_v_predictor_32x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(above);
    const uint8x16_t d1 = vld1q_u8(above + 16);
    (void)left;
    v_store_32xh(dst, stride, 64, d0, d1);
}

void svt_aom_v_predictor_64x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(above);
    const uint8x16_t d1 = vld1q_u8(above + 16);
    const uint8x16_t d2 = vld1q_u8(above + 32);
    const uint8x16_t d3 = vld1q_u8(above + 48);
    (void)left;
    v_store_64xh(dst, stride, 16, d0, d1, d2, d3);
}

void svt_aom_v_predictor_64x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(above);
    const uint8x16_t d1 = vld1q_u8(above + 16);
    const uint8x16_t d2 = vld1q_u8(above + 32);
    const uint8x16_t d3 = vld1q_u8(above + 48);
    (void)left;
    v_store_64xh(dst, stride, 32, d0, d1, d2, d3);
}

void svt_aom_v_predictor_64x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(above);
    const uint8x16_t d1 = vld1q_u8(above + 16);
    const uint8x16_t d2 = vld1q_u8(above + 32);
    const uint8x16_t d3 = vld1q_u8(above + 48);
    (void)left;
    v_store_64xh(dst, stride, 64, d0, d1, d2, d3);
}

/* ---------------------H PREDICTOR--------------------------- */
static inline void h_store_4x8(uint8_t* dst, ptrdiff_t stride, uint8x8_t d0) {
    store_u8_4x1(dst + 0 * stride, vdup_lane_u8(d0, 0));
    store_u8_4x1(dst + 1 * stride, vdup_lane_u8(d0, 1));
    store_u8_4x1(dst + 2 * stride, vdup_lane_u8(d0, 2));
    store_u8_4x1(dst + 3 * stride, vdup_lane_u8(d0, 3));
    store_u8_4x1(dst + 4 * stride, vdup_lane_u8(d0, 4));
    store_u8_4x1(dst + 5 * stride, vdup_lane_u8(d0, 5));
    store_u8_4x1(dst + 6 * stride, vdup_lane_u8(d0, 6));
    store_u8_4x1(dst + 7 * stride, vdup_lane_u8(d0, 7));
}

static inline void h_store_8x8(uint8_t* dst, ptrdiff_t stride, uint8x8_t d0) {
    vst1_u8(dst + 0 * stride, vdup_lane_u8(d0, 0));
    vst1_u8(dst + 1 * stride, vdup_lane_u8(d0, 1));
    vst1_u8(dst + 2 * stride, vdup_lane_u8(d0, 2));
    vst1_u8(dst + 3 * stride, vdup_lane_u8(d0, 3));
    vst1_u8(dst + 4 * stride, vdup_lane_u8(d0, 4));
    vst1_u8(dst + 5 * stride, vdup_lane_u8(d0, 5));
    vst1_u8(dst + 6 * stride, vdup_lane_u8(d0, 6));
    vst1_u8(dst + 7 * stride, vdup_lane_u8(d0, 7));
}

static inline void h_store_16x8(uint8_t* dst, ptrdiff_t stride, uint8x8_t d0) {
    vst1q_u8(dst + 0 * stride, vdupq_lane_u8(d0, 0));
    vst1q_u8(dst + 1 * stride, vdupq_lane_u8(d0, 1));
    vst1q_u8(dst + 2 * stride, vdupq_lane_u8(d0, 2));
    vst1q_u8(dst + 3 * stride, vdupq_lane_u8(d0, 3));
    vst1q_u8(dst + 4 * stride, vdupq_lane_u8(d0, 4));
    vst1q_u8(dst + 5 * stride, vdupq_lane_u8(d0, 5));
    vst1q_u8(dst + 6 * stride, vdupq_lane_u8(d0, 6));
    vst1q_u8(dst + 7 * stride, vdupq_lane_u8(d0, 7));
}

static inline void h_store_32x8(uint8_t* dst, ptrdiff_t stride, uint8x8_t d0) {
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 0));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 0));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 1));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 1));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 2));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 2));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 3));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 3));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 4));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 4));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 5));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 5));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 6));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 6));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 7));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 7));
}

static inline void h_store_64x8(uint8_t* dst, ptrdiff_t stride, uint8x8_t d0) {
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 0));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 0));
    vst1q_u8(dst + 32, vdupq_lane_u8(d0, 0));
    vst1q_u8(dst + 48, vdupq_lane_u8(d0, 0));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 1));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 1));
    vst1q_u8(dst + 32, vdupq_lane_u8(d0, 1));
    vst1q_u8(dst + 48, vdupq_lane_u8(d0, 1));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 2));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 2));
    vst1q_u8(dst + 32, vdupq_lane_u8(d0, 2));
    vst1q_u8(dst + 48, vdupq_lane_u8(d0, 2));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 3));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 3));
    vst1q_u8(dst + 32, vdupq_lane_u8(d0, 3));
    vst1q_u8(dst + 48, vdupq_lane_u8(d0, 3));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 4));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 4));
    vst1q_u8(dst + 32, vdupq_lane_u8(d0, 4));
    vst1q_u8(dst + 48, vdupq_lane_u8(d0, 4));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 5));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 5));
    vst1q_u8(dst + 32, vdupq_lane_u8(d0, 5));
    vst1q_u8(dst + 48, vdupq_lane_u8(d0, 5));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 6));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 6));
    vst1q_u8(dst + 32, vdupq_lane_u8(d0, 6));
    vst1q_u8(dst + 48, vdupq_lane_u8(d0, 6));
    dst += stride;
    vst1q_u8(dst + 0, vdupq_lane_u8(d0, 7));
    vst1q_u8(dst + 16, vdupq_lane_u8(d0, 7));
    vst1q_u8(dst + 32, vdupq_lane_u8(d0, 7));
    vst1q_u8(dst + 48, vdupq_lane_u8(d0, 7));
}

void svt_aom_h_predictor_4x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x8_t d0 = load_u8_4x1(left);
    (void)above;
    store_u8_4x1(dst + 0 * stride, vdup_lane_u8(d0, 0));
    store_u8_4x1(dst + 1 * stride, vdup_lane_u8(d0, 1));
    store_u8_4x1(dst + 2 * stride, vdup_lane_u8(d0, 2));
    store_u8_4x1(dst + 3 * stride, vdup_lane_u8(d0, 3));
}

void svt_aom_h_predictor_8x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x8_t d0 = vld1_u8(left);
    (void)above;
    h_store_8x8(dst, stride, d0);
}

void svt_aom_h_predictor_16x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(left);
    (void)above;
    h_store_16x8(dst, stride, vget_low_u8(d0));
    h_store_16x8(dst + 8 * stride, stride, vget_high_u8(d0));
}

void svt_aom_h_predictor_32x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(left);
    const uint8x16_t d1 = vld1q_u8(left + 16);
    (void)above;
    h_store_32x8(dst + 0 * stride, stride, vget_low_u8(d0));
    h_store_32x8(dst + 8 * stride, stride, vget_high_u8(d0));
    h_store_32x8(dst + 16 * stride, stride, vget_low_u8(d1));
    h_store_32x8(dst + 24 * stride, stride, vget_high_u8(d1));
}

void svt_aom_h_predictor_4x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x8_t d0 = vld1_u8(left);
    (void)above;
    h_store_4x8(dst, stride, d0);
}

void svt_aom_h_predictor_4x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(left);
    (void)above;
    h_store_4x8(dst + 0 * stride, stride, vget_low_u8(d0));
    h_store_4x8(dst + 8 * stride, stride, vget_high_u8(d0));
}

void svt_aom_h_predictor_8x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x8_t d0 = load_u8_4x1(left);
    (void)above;
    vst1_u8(dst + 0 * stride, vdup_lane_u8(d0, 0));
    vst1_u8(dst + 1 * stride, vdup_lane_u8(d0, 1));
    vst1_u8(dst + 2 * stride, vdup_lane_u8(d0, 2));
    vst1_u8(dst + 3 * stride, vdup_lane_u8(d0, 3));
}

void svt_aom_h_predictor_8x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(left);
    (void)above;
    h_store_8x8(dst + 0 * stride, stride, vget_low_u8(d0));
    h_store_8x8(dst + 8 * stride, stride, vget_high_u8(d0));
}

void svt_aom_h_predictor_8x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(left);
    const uint8x16_t d1 = vld1q_u8(left + 16);
    (void)above;
    h_store_8x8(dst + 0 * stride, stride, vget_low_u8(d0));
    h_store_8x8(dst + 8 * stride, stride, vget_high_u8(d0));
    h_store_8x8(dst + 16 * stride, stride, vget_low_u8(d1));
    h_store_8x8(dst + 24 * stride, stride, vget_high_u8(d1));
}

void svt_aom_h_predictor_16x4_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x8_t d0 = load_u8_4x1(left);
    (void)above;
    vst1q_u8(dst + 0 * stride, vdupq_lane_u8(d0, 0));
    vst1q_u8(dst + 1 * stride, vdupq_lane_u8(d0, 1));
    vst1q_u8(dst + 2 * stride, vdupq_lane_u8(d0, 2));
    vst1q_u8(dst + 3 * stride, vdupq_lane_u8(d0, 3));
}

void svt_aom_h_predictor_16x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x8_t d0 = vld1_u8(left);
    (void)above;
    h_store_16x8(dst, stride, d0);
}

void svt_aom_h_predictor_16x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(left);
    const uint8x16_t d1 = vld1q_u8(left + 16);
    (void)above;
    h_store_16x8(dst + 0 * stride, stride, vget_low_u8(d0));
    h_store_16x8(dst + 8 * stride, stride, vget_high_u8(d0));
    h_store_16x8(dst + 16 * stride, stride, vget_low_u8(d1));
    h_store_16x8(dst + 24 * stride, stride, vget_high_u8(d1));
}

void svt_aom_h_predictor_16x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(left);
    const uint8x16_t d1 = vld1q_u8(left + 16);
    const uint8x16_t d2 = vld1q_u8(left + 32);
    const uint8x16_t d3 = vld1q_u8(left + 48);
    (void)above;
    h_store_16x8(dst + 0 * stride, stride, vget_low_u8(d0));
    h_store_16x8(dst + 8 * stride, stride, vget_high_u8(d0));
    h_store_16x8(dst + 16 * stride, stride, vget_low_u8(d1));
    h_store_16x8(dst + 24 * stride, stride, vget_high_u8(d1));
    h_store_16x8(dst + 32 * stride, stride, vget_low_u8(d2));
    h_store_16x8(dst + 40 * stride, stride, vget_high_u8(d2));
    h_store_16x8(dst + 48 * stride, stride, vget_low_u8(d3));
    h_store_16x8(dst + 56 * stride, stride, vget_high_u8(d3));
}

void svt_aom_h_predictor_32x8_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x8_t d0 = vld1_u8(left);
    (void)above;
    h_store_32x8(dst, stride, d0);
}

void svt_aom_h_predictor_32x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(left);
    (void)above;
    h_store_32x8(dst + 0 * stride, stride, vget_low_u8(d0));
    h_store_32x8(dst + 8 * stride, stride, vget_high_u8(d0));
}

void svt_aom_h_predictor_32x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(left + 0);
    const uint8x16_t d1 = vld1q_u8(left + 16);
    const uint8x16_t d2 = vld1q_u8(left + 32);
    const uint8x16_t d3 = vld1q_u8(left + 48);
    (void)above;
    h_store_32x8(dst + 0 * stride, stride, vget_low_u8(d0));
    h_store_32x8(dst + 8 * stride, stride, vget_high_u8(d0));
    h_store_32x8(dst + 16 * stride, stride, vget_low_u8(d1));
    h_store_32x8(dst + 24 * stride, stride, vget_high_u8(d1));
    h_store_32x8(dst + 32 * stride, stride, vget_low_u8(d2));
    h_store_32x8(dst + 40 * stride, stride, vget_high_u8(d2));
    h_store_32x8(dst + 48 * stride, stride, vget_low_u8(d3));
    h_store_32x8(dst + 56 * stride, stride, vget_high_u8(d3));
}

void svt_aom_h_predictor_64x16_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    const uint8x16_t d0 = vld1q_u8(left);
    (void)above;
    h_store_64x8(dst + 0 * stride, stride, vget_low_u8(d0));
    h_store_64x8(dst + 8 * stride, stride, vget_high_u8(d0));
}

void svt_aom_h_predictor_64x32_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)above;
    for (int i = 0; i < 2; ++i) {
        const uint8x16_t d0 = vld1q_u8(left);
        h_store_64x8(dst + 0 * stride, stride, vget_low_u8(d0));
        h_store_64x8(dst + 8 * stride, stride, vget_high_u8(d0));
        left += 16;
        dst += 16 * stride;
    }
}

void svt_aom_h_predictor_64x64_neon(uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) {
    (void)above;
    for (int i = 0; i < 4; ++i) {
        const uint8x16_t d0 = vld1q_u8(left);
        h_store_64x8(dst + 0 * stride, stride, vget_low_u8(d0));
        h_store_64x8(dst + 8 * stride, stride, vget_high_u8(d0));
        left += 16;
        dst += 16 * stride;
    }
}

/* ---------------------PAETH PREDICTOR--------------------------- */
static inline void paeth_4or8_x_h_neon(uint8_t* dest, ptrdiff_t stride, const uint8_t* const top_row,
                                       const uint8_t* const left_column, int width, int height) {
    const uint8x8_t  top         = width == 4 ? load_u8_4x1(top_row) : vld1_u8(top_row);
    const uint8x8_t  top_left    = vdup_n_u8(top_row[-1]);
    const uint16x8_t top_left_x2 = vdupq_n_u16(top_row[-1] + top_row[-1]);

    assert(height > 0);
    int y = 0;
    do {
        const uint8x8_t left = vdup_n_u8(left_column[y]);

        const uint8x8_t  left_dist     = vabd_u8(top, top_left);
        const uint8x8_t  top_dist      = vabd_u8(left, top_left);
        const uint16x8_t top_left_dist = vabdq_u16(vaddl_u8(top, left), top_left_x2);

        const uint8x8_t left_le_top      = vcle_u8(left_dist, top_dist);
        const uint8x8_t left_le_top_left = vmovn_u16(vcleq_u16(vmovl_u8(left_dist), top_left_dist));
        const uint8x8_t top_le_top_left  = vmovn_u16(vcleq_u16(vmovl_u8(top_dist), top_left_dist));

        const uint8x8_t left_mask        = vand_u8(left_le_top, left_le_top_left);
        uint8x8_t       result           = vbsl_u8(left_mask, left, top);
        const uint8x8_t left_or_top_mask = vorr_u8(left_mask, top_le_top_left);
        result                           = vbsl_u8(left_or_top_mask, result, top_left);

        if (width == 4) {
            store_u8_4x1(dest, result);
        } else {
            vst1_u8(dest, result);
        }
        dest += stride;
    } while (++y != height);
}

#define PAETH_NXM(W, H)                                                              \
    void svt_aom_paeth_predictor_##W##x##H##_neon(                                   \
        uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) { \
        paeth_4or8_x_h_neon(dst, stride, above, left, W, H);                         \
    }

PAETH_NXM(4, 4)
PAETH_NXM(4, 8)
PAETH_NXM(8, 4)
PAETH_NXM(8, 8)
PAETH_NXM(8, 16)
PAETH_NXM(4, 16)
PAETH_NXM(8, 32)

/* Calculate X distance <= TopLeft distance and pack the resulting mask into uint8x8_t */
static inline uint8x16_t x_le_top_left(const uint8x16_t x_dist, const uint16x8_t top_left_dist_low,
                                       const uint16x8_t top_left_dist_high) {
    const uint8x16_t top_left_dist = vcombine_u8(vqmovn_u16(top_left_dist_low), vqmovn_u16(top_left_dist_high));
    return vcleq_u8(x_dist, top_left_dist);
}

/* Select the closest values and collect them. */
static inline uint8x16_t select_paeth(const uint8x16_t top, const uint8x16_t left, const uint8x16_t top_left,
                                      const uint8x16_t left_le_top, const uint8x16_t left_le_top_left,
                                      const uint8x16_t top_le_top_left) {
    const uint8x16_t left_mask        = vandq_u8(left_le_top, left_le_top_left);
    uint8x16_t       result           = vbslq_u8(left_mask, left, top);
    const uint8x16_t left_or_top_mask = vorrq_u8(left_mask, top_le_top_left);
    return vbslq_u8(left_or_top_mask, result, top_left);
}

/* Generate numbered and high/low versions of top_left_dist.*/
#define TOP_LEFT_DIST(num)                                                                                       \
    const uint16x8_t top_left_##num##_dist_low  = vabdq_u16(vaddl_u8(vget_low_u8(top[num]), vget_low_u8(left)),  \
                                                           top_left_x2);                                        \
    const uint16x8_t top_left_##num##_dist_high = vabdq_u16(vaddl_u8(vget_high_u8(top[num]), vget_low_u8(left)), \
                                                            top_left_x2)

/* Generate numbered versions of XLeTopLeft with x = left. */
#define LEFT_LE_TOP_LEFT(num)                                \
    const uint8x16_t left_le_top_left_##num = x_le_top_left( \
        left_##num##_dist, top_left_##num##_dist_low, top_left_##num##_dist_high)

/* Generate numbered versions of XLeTopLeft with x = top. */
#define TOP_LE_TOP_LEFT(num)                                \
    const uint8x16_t top_le_top_left_##num = x_le_top_left( \
        top_dist, top_left_##num##_dist_low, top_left_##num##_dist_high)

static inline void paeth16_plus_x_h_neon(uint8_t* dest, ptrdiff_t stride, const uint8_t* const top_row,
                                         const uint8_t* const left_column, int width, int height) {
    const uint8x16_t top_left    = vdupq_n_u8(top_row[-1]);
    const uint16x8_t top_left_x2 = vdupq_n_u16(top_row[-1] + top_row[-1]);
    uint8x16_t       top[4];
    top[0] = vld1q_u8(top_row);
    if (width > 16) {
        top[1] = vld1q_u8(top_row + 16);
        if (width == 64) {
            top[2] = vld1q_u8(top_row + 32);
            top[3] = vld1q_u8(top_row + 48);
        }
    }

    assert(height > 0);
    int y = 0;
    do {
        const uint8x16_t left = vdupq_n_u8(left_column[y]);

        const uint8x16_t top_dist = vabdq_u8(left, top_left);

        const uint8x16_t left_0_dist = vabdq_u8(top[0], top_left);
        TOP_LEFT_DIST(0);
        const uint8x16_t left_0_le_top = vcleq_u8(left_0_dist, top_dist);
        LEFT_LE_TOP_LEFT(0);
        TOP_LE_TOP_LEFT(0);

        const uint8x16_t result_0 = select_paeth(
            top[0], left, top_left, left_0_le_top, left_le_top_left_0, top_le_top_left_0);
        vst1q_u8(dest, result_0);

        if (width > 16) {
            const uint8x16_t left_1_dist = vabdq_u8(top[1], top_left);
            TOP_LEFT_DIST(1);
            const uint8x16_t left_1_le_top = vcleq_u8(left_1_dist, top_dist);
            LEFT_LE_TOP_LEFT(1);
            TOP_LE_TOP_LEFT(1);

            const uint8x16_t result_1 = select_paeth(
                top[1], left, top_left, left_1_le_top, left_le_top_left_1, top_le_top_left_1);
            vst1q_u8(dest + 16, result_1);

            if (width == 64) {
                const uint8x16_t left_2_dist = vabdq_u8(top[2], top_left);
                TOP_LEFT_DIST(2);
                const uint8x16_t left_2_le_top = vcleq_u8(left_2_dist, top_dist);
                LEFT_LE_TOP_LEFT(2);
                TOP_LE_TOP_LEFT(2);

                const uint8x16_t result_2 = select_paeth(
                    top[2], left, top_left, left_2_le_top, left_le_top_left_2, top_le_top_left_2);
                vst1q_u8(dest + 32, result_2);

                const uint8x16_t left_3_dist = vabdq_u8(top[3], top_left);
                TOP_LEFT_DIST(3);
                const uint8x16_t left_3_le_top = vcleq_u8(left_3_dist, top_dist);
                LEFT_LE_TOP_LEFT(3);
                TOP_LE_TOP_LEFT(3);

                const uint8x16_t result_3 = select_paeth(
                    top[3], left, top_left, left_3_le_top, left_le_top_left_3, top_le_top_left_3);
                vst1q_u8(dest + 48, result_3);
            }
        }

        dest += stride;
    } while (++y != height);
}

#define PAETH_NXM_WIDE(W, H)                                                         \
    void svt_aom_paeth_predictor_##W##x##H##_neon(                                   \
        uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) { \
        paeth16_plus_x_h_neon(dst, stride, above, left, W, H);                       \
    }

PAETH_NXM_WIDE(16, 8)
PAETH_NXM_WIDE(16, 16)
PAETH_NXM_WIDE(16, 32)
PAETH_NXM_WIDE(32, 16)
PAETH_NXM_WIDE(32, 32)
PAETH_NXM_WIDE(32, 64)
PAETH_NXM_WIDE(64, 32)
PAETH_NXM_WIDE(64, 64)
PAETH_NXM_WIDE(16, 4)
PAETH_NXM_WIDE(16, 64)
PAETH_NXM_WIDE(32, 8)
PAETH_NXM_WIDE(64, 16)

void svt_av1_filter_intra_edge_neon(uint8_t* p, int sz, int strength) {
    if (!strength) {
        return;
    }
    assert(sz >= 0 && sz <= 129);

    uint8_t edge[160]; // Max value of sz + enough padding for vector accesses.
    memcpy(edge + 1, p, sz * sizeof(*p));

    // Populate extra space appropriately.
    edge[0]      = edge[1];
    edge[sz + 1] = edge[sz];
    edge[sz + 2] = edge[sz];

    // Don't overwrite first pixel.
    uint8_t* dst = p + 1;
    sz--;

    if (strength == 1) { // Filter: {4, 8, 4}.
        const uint8_t* src = edge + 1;

        while (sz >= 8) {
            uint8x8_t s0 = vld1_u8(src);
            uint8x8_t s1 = vld1_u8(src + 1);
            uint8x8_t s2 = vld1_u8(src + 2);

            // Make use of the identity:
            // (4*a + 8*b + 4*c) >> 4 == (a + (b << 1) + c) >> 2
            uint16x8_t t0  = vaddl_u8(s0, s2);
            uint16x8_t t1  = vaddl_u8(s1, s1);
            uint16x8_t sum = vaddq_u16(t0, t1);
            uint8x8_t  res = vrshrn_n_u16(sum, 2);

            vst1_u8(dst, res);

            src += 8;
            dst += 8;
            sz -= 8;
        }

        if (sz > 0) { // Handle sz < 8 to avoid modifying out-of-bounds values.
            uint8x8_t s0 = vld1_u8(src);
            uint8x8_t s1 = vld1_u8(src + 1);
            uint8x8_t s2 = vld1_u8(src + 2);

            uint16x8_t t0  = vaddl_u8(s0, s2);
            uint16x8_t t1  = vaddl_u8(s1, s1);
            uint16x8_t sum = vaddq_u16(t0, t1);
            uint8x8_t  res = vrshrn_n_u16(sum, 2);

            // Mask off out-of-bounds indices.
            uint8x8_t current_dst = vld1_u8(dst);
            uint8x8_t mask        = vcgt_u8(vdup_n_u8(sz), vcreate_u8(0x0706050403020100));
            res                   = vbsl_u8(mask, res, current_dst);

            vst1_u8(dst, res);
        }
    } else if (strength == 2) { // Filter: {5, 6, 5}.
        const uint8_t* src = edge + 1;

        const uint8x8x3_t filter = {{vdup_n_u8(5), vdup_n_u8(6), vdup_n_u8(5)}};

        while (sz >= 8) {
            uint8x8_t s0 = vld1_u8(src);
            uint8x8_t s1 = vld1_u8(src + 1);
            uint8x8_t s2 = vld1_u8(src + 2);

            uint16x8_t accum = vmull_u8(s0, filter.val[0]);
            accum            = vmlal_u8(accum, s1, filter.val[1]);
            accum            = vmlal_u8(accum, s2, filter.val[2]);
            uint8x8_t res    = vrshrn_n_u16(accum, 4);

            vst1_u8(dst, res);

            src += 8;
            dst += 8;
            sz -= 8;
        }

        if (sz > 0) { // Handle sz < 8 to avoid modifying out-of-bounds values.
            uint8x8_t s0 = vld1_u8(src);
            uint8x8_t s1 = vld1_u8(src + 1);
            uint8x8_t s2 = vld1_u8(src + 2);

            uint16x8_t accum = vmull_u8(s0, filter.val[0]);
            accum            = vmlal_u8(accum, s1, filter.val[1]);
            accum            = vmlal_u8(accum, s2, filter.val[2]);
            uint8x8_t res    = vrshrn_n_u16(accum, 4);

            // Mask off out-of-bounds indices.
            uint8x8_t current_dst = vld1_u8(dst);
            uint8x8_t mask        = vcgt_u8(vdup_n_u8(sz), vcreate_u8(0x0706050403020100));
            res                   = vbsl_u8(mask, res, current_dst);

            vst1_u8(dst, res);
        }
    } else { // Filter {2, 4, 4, 4, 2}.
        const uint8_t* src = edge;

        while (sz >= 8) {
            uint8x8_t s0 = vld1_u8(src);
            uint8x8_t s1 = vld1_u8(src + 1);
            uint8x8_t s2 = vld1_u8(src + 2);
            uint8x8_t s3 = vld1_u8(src + 3);
            uint8x8_t s4 = vld1_u8(src + 4);

            // Make use of the identity:
            // (2*a + 4*b + 4*c + 4*d + 2*e) >> 4 == (a + ((b + c + d) << 1) + e) >> 3
            uint16x8_t t0  = vaddl_u8(s0, s4);
            uint16x8_t t1  = vaddl_u8(s1, s2);
            t1             = vaddw_u8(t1, s3);
            t1             = vaddq_u16(t1, t1);
            uint16x8_t sum = vaddq_u16(t0, t1);
            uint8x8_t  res = vrshrn_n_u16(sum, 3);

            vst1_u8(dst, res);

            src += 8;
            dst += 8;
            sz -= 8;
        }

        if (sz > 0) { // Handle sz < 8 to avoid modifying out-of-bounds values.
            uint8x8_t s0 = vld1_u8(src);
            uint8x8_t s1 = vld1_u8(src + 1);
            uint8x8_t s2 = vld1_u8(src + 2);
            uint8x8_t s3 = vld1_u8(src + 3);
            uint8x8_t s4 = vld1_u8(src + 4);

            uint16x8_t t0  = vaddl_u8(s0, s4);
            uint16x8_t t1  = vaddl_u8(s1, s2);
            t1             = vaddw_u8(t1, s3);
            t1             = vaddq_u16(t1, t1);
            uint16x8_t sum = vaddq_u16(t0, t1);
            uint8x8_t  res = vrshrn_n_u16(sum, 3);

            // Mask off out-of-bounds indices.
            uint8x8_t current_dst = vld1_u8(dst);
            uint8x8_t mask        = vcgt_u8(vdup_n_u8(sz), vcreate_u8(0x0706050403020100));
            res                   = vbsl_u8(mask, res, current_dst);

            vst1_u8(dst, res);
        }
    }
}

void svt_av1_upsample_intra_edge_neon(uint8_t* p, int sz) {
    if (!sz) {
        return;
    }

    assert(sz <= MAX_UPSAMPLE_SZ);

    uint8_t        edge[MAX_UPSAMPLE_SZ + 3];
    const uint8_t* src = edge;

    // Copy p[-1..(sz-1)] and pad out both ends.
    edge[0] = p[-1];
    edge[1] = p[-1];
    memcpy(edge + 2, p, sz);
    edge[sz + 2] = p[sz - 1];
    p[-2]        = p[-1];

    uint8_t* dst = p - 1;

    do {
        uint8x8_t s0 = vld1_u8(src);
        uint8x8_t s1 = vld1_u8(src + 1);
        uint8x8_t s2 = vld1_u8(src + 2);
        uint8x8_t s3 = vld1_u8(src + 3);

        int16x8_t t0 = vreinterpretq_s16_u16(vaddl_u8(s0, s3));
        int16x8_t t1 = vreinterpretq_s16_u16(vaddl_u8(s1, s2));
        t1           = vmulq_n_s16(t1, 9);
        t1           = vsubq_s16(t1, t0);

        uint8x8x2_t res = {{vqrshrun_n_s16(t1, 4), s2}};

        vst2_u8(dst, res);

        src += 8;
        dst += 16;
        sz -= 8;
    } while (sz > 0);
}
