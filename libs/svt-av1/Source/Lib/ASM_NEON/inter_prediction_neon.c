/*
 * Copyright (c) 2018, Alliance for Open Media. All rights reserved.
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
#include <stdbool.h>

#include "common_dsp_rtcd.h"
#include "definitions.h"
#include "enc_inter_prediction.h"
#include "mem_neon.h"
#include "utility.h"

static inline void diffwtd_mask_d16_neon(uint8_t* mask, const bool inverse, const CONV_BUF_TYPE* src0, int src0_stride,
                                         const CONV_BUF_TYPE* src1, int src1_stride, int h, int w, int bd) {
    const int       round     = 2 * FILTER_BITS - ROUND0_BITS - COMPOUND_ROUND1_BITS + (bd - 8);
    const int16x8_t round_vec = vdupq_n_s16((int16_t)(-round));

    if (w >= 16) {
        int i = 0;
        do {
            int j = 0;
            do {
                uint16x8_t s0_lo = vld1q_u16(src0 + j);
                uint16x8_t s1_lo = vld1q_u16(src1 + j);
                uint16x8_t s0_hi = vld1q_u16(src0 + j + 8);
                uint16x8_t s1_hi = vld1q_u16(src1 + j + 8);

                uint16x8_t diff_lo_u16 = vrshlq_u16(vabdq_u16(s0_lo, s1_lo), round_vec);
                uint16x8_t diff_hi_u16 = vrshlq_u16(vabdq_u16(s0_hi, s1_hi), round_vec);
                uint8x8_t  diff_lo_u8  = vshrn_n_u16(diff_lo_u16, DIFF_FACTOR_LOG2);
                uint8x8_t  diff_hi_u8  = vshrn_n_u16(diff_hi_u16, DIFF_FACTOR_LOG2);
                uint8x16_t diff        = vcombine_u8(diff_lo_u8, diff_hi_u8);

                uint8x16_t m;
                if (inverse) {
                    m = vqsubq_u8(vdupq_n_u8(64 - 38), diff); // Saturating to 0
                } else {
                    m = vminq_u8(vaddq_u8(diff, vdupq_n_u8(38)), vdupq_n_u8(64));
                }

                vst1q_u8(mask, m);

                mask += 16;
                j += 16;
            } while (j < w);
            src0 += src0_stride;
            src1 += src1_stride;
        } while (++i < h);
    } else if (w == 8) {
        int i = 0;
        do {
            uint16x8_t s0 = vld1q_u16(src0);
            uint16x8_t s1 = vld1q_u16(src1);

            uint16x8_t diff_u16 = vrshlq_u16(vabdq_u16(s0, s1), round_vec);
            uint8x8_t  diff_u8  = vshrn_n_u16(diff_u16, DIFF_FACTOR_LOG2);
            uint8x8_t  m;
            if (inverse) {
                m = vqsub_u8(vdup_n_u8(64 - 38), diff_u8); // Saturating to 0
            } else {
                m = vmin_u8(vadd_u8(diff_u8, vdup_n_u8(38)), vdup_n_u8(64));
            }

            vst1_u8(mask, m);

            mask += 8;
            src0 += src0_stride;
            src1 += src1_stride;
        } while (++i < h);
    } else if (w == 4) {
        int i = 0;
        do {
            uint16x8_t s0 = vcombine_u16(vld1_u16(src0), vld1_u16(src0 + src0_stride));
            uint16x8_t s1 = vcombine_u16(vld1_u16(src1), vld1_u16(src1 + src1_stride));

            uint16x8_t diff_u16 = vrshlq_u16(vabdq_u16(s0, s1), round_vec);
            uint8x8_t  diff_u8  = vshrn_n_u16(diff_u16, DIFF_FACTOR_LOG2);
            uint8x8_t  m;
            if (inverse) {
                m = vqsub_u8(vdup_n_u8(64 - 38), diff_u8); // Saturating to 0
            } else {
                m = vmin_u8(vadd_u8(diff_u8, vdup_n_u8(38)), vdup_n_u8(64));
            }

            vst1_u8(mask, m);

            mask += 8;
            src0 += 2 * src0_stride;
            src1 += 2 * src1_stride;
            i += 2;
        } while (i < h);
    }
}

void svt_av1_build_compound_diffwtd_mask_d16_neon(uint8_t* mask, DIFFWTD_MASK_TYPE mask_type, const CONV_BUF_TYPE* src0,
                                                  int src0_stride, const CONV_BUF_TYPE* src1, int src1_stride, int h,
                                                  int w, ConvolveParams* conv_params, int bd) {
    (void)conv_params;
    assert(h >= 4);
    assert(w >= 4);
    assert(mask_type == DIFFWTD_38_INV || mask_type == DIFFWTD_38);

    if (mask_type == DIFFWTD_38) {
        diffwtd_mask_d16_neon(mask, /*inverse=*/false, src0, src0_stride, src1, src1_stride, h, w, bd);
    } else { // mask_type == DIFFWTD_38_INV
        diffwtd_mask_d16_neon(mask, /*inverse=*/true, src0, src0_stride, src1, src1_stride, h, w, bd);
    }
}

static inline void diffwtd_mask_neon(uint8_t* mask, const bool inverse, const uint8_t* src0, int src0_stride,
                                     const uint8_t* src1, int src1_stride, int h, int w) {
    if (w >= 16) {
        int i = 0;
        do {
            int j = 0;
            do {
                uint8x16_t s0 = vld1q_u8(src0 + j);
                uint8x16_t s1 = vld1q_u8(src1 + j);

                uint8x16_t diff = vshrq_n_u8(vabdq_u8(s0, s1), DIFF_FACTOR_LOG2);
                uint8x16_t m;
                if (inverse) {
                    m = vqsubq_u8(vdupq_n_u8(64 - 38), diff); // Saturating to 0
                } else {
                    m = vminq_u8(vaddq_u8(diff, vdupq_n_u8(38)), vdupq_n_u8(64));
                }

                vst1q_u8(mask, m);

                mask += 16;
                j += 16;
            } while (j < w);
            src0 += src0_stride;
            src1 += src1_stride;
        } while (++i < h);
    } else if (w == 8) {
        int i = 0;
        do {
            uint8x16_t s0 = vcombine_u8(vld1_u8(src0), vld1_u8(src0 + src0_stride));
            uint8x16_t s1 = vcombine_u8(vld1_u8(src1), vld1_u8(src1 + src0_stride));

            uint8x16_t diff = vshrq_n_u8(vabdq_u8(s0, s1), DIFF_FACTOR_LOG2);
            uint8x16_t m;
            if (inverse) {
                m = vqsubq_u8(vdupq_n_u8(64 - 38), diff); // Saturating to 0
            } else {
                m = vminq_u8(vaddq_u8(diff, vdupq_n_u8(38)), vdupq_n_u8(64));
            }

            vst1q_u8(mask, m);

            mask += 16;
            src0 += 2 * src0_stride;
            src1 += 2 * src1_stride;
            i += 2;
        } while (i < h);
    } else if (w == 4) {
        int i = 0;
        do {
            uint8x16_t s0 = load_u8_4x4(src0, src0_stride);
            uint8x16_t s1 = load_u8_4x4(src1, src1_stride);

            uint8x16_t diff = vshrq_n_u8(vabdq_u8(s0, s1), DIFF_FACTOR_LOG2);
            uint8x16_t m;
            if (inverse) {
                m = vqsubq_u8(vdupq_n_u8(64 - 38), diff); // Saturating to 0
            } else {
                m = vminq_u8(vaddq_u8(diff, vdupq_n_u8(38)), vdupq_n_u8(64));
            }

            vst1q_u8(mask, m);

            mask += 16;
            src0 += 4 * src0_stride;
            src1 += 4 * src1_stride;
            i += 4;
        } while (i < h);
    }
}

void svt_av1_build_compound_diffwtd_mask_neon(uint8_t* mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t* src0,
                                              int src0_stride, const uint8_t* src1, int src1_stride, int h, int w) {
    assert(h % 4 == 0);
    assert(w % 4 == 0);
    assert(mask_type == DIFFWTD_38_INV || mask_type == DIFFWTD_38);

    if (mask_type == DIFFWTD_38) {
        diffwtd_mask_neon(mask, /*inverse=*/false, src0, src0_stride, src1, src1_stride, h, w);
    } else { // mask_type == DIFFWTD_38_INV
        diffwtd_mask_neon(mask, /*inverse=*/true, src0, src0_stride, src1, src1_stride, h, w);
    }
}

// Use tbl for doing a double-width zero extension from 8->32 bits since we can
// do this in one instruction rather than two. Indices out of range (255 here)
// are set to zero by tbl.
// clang-format off
DECLARE_ALIGNED(16, static const uint8_t, obmc_variance_permute_idx[]) = {
    0,  255, 255, 255, 1,  255, 255, 255, 2,  255, 255, 255, 3,  255, 255, 255,
    4,  255, 255, 255, 5,  255, 255, 255, 6,  255, 255, 255, 7,  255, 255, 255,
    8,  255, 255, 255, 9,  255, 255, 255, 10, 255, 255, 255, 11, 255, 255, 255,
    12, 255, 255, 255, 13, 255, 255, 255, 14, 255, 255, 255, 15, 255, 255, 255
};
// clang-format on

static inline void weighted_pred_left_neon(int32_t* wsrc_ptr, int32_t* mask_ptr, int32x4_t tmp_lo, int32x4_t tmp_hi,
                                           int32x4_t m0_lo, int32x4_t m0_hi, int32x4_t m1_lo, int32x4_t m1_hi,
                                           int stride) {
    int32x4_t wsrc_lo_s32 = vld1q_s32(wsrc_ptr);
    int32x4_t wsrc_hi_s32 = vld1q_s32(wsrc_ptr + stride);
    int32x4_t mask_lo_s32 = vld1q_s32(mask_ptr);
    int32x4_t mask_hi_s32 = vld1q_s32(mask_ptr + stride);

    wsrc_lo_s32 = vshrq_n_s32(wsrc_lo_s32, AOM_BLEND_A64_ROUND_BITS);
    wsrc_hi_s32 = vshrq_n_s32(wsrc_hi_s32, AOM_BLEND_A64_ROUND_BITS);
    mask_lo_s32 = vshrq_n_s32(mask_lo_s32, AOM_BLEND_A64_ROUND_BITS);
    mask_hi_s32 = vshrq_n_s32(mask_hi_s32, AOM_BLEND_A64_ROUND_BITS);

    wsrc_lo_s32 = vmulq_s32(wsrc_lo_s32, m0_lo);
    wsrc_hi_s32 = vmulq_s32(wsrc_hi_s32, m0_hi);
    wsrc_lo_s32 = vmlaq_s32(wsrc_lo_s32, tmp_lo, m1_lo);
    wsrc_hi_s32 = vmlaq_s32(wsrc_hi_s32, tmp_hi, m1_hi);

    vst1q_s32(wsrc_ptr, wsrc_lo_s32);
    vst1q_s32(wsrc_ptr + stride, wsrc_hi_s32);

    mask_lo_s32 = vmulq_s32(mask_lo_s32, m0_lo);
    mask_hi_s32 = vmulq_s32(mask_hi_s32, m0_hi);

    vst1q_s32(mask_ptr, mask_lo_s32);
    vst1q_s32(mask_ptr + stride, mask_hi_s32);
}

void svt_av1_calc_target_weighted_pred_left_neon(uint8_t is16bit, MacroBlockD* xd, int rel_mi_row, uint8_t nb_mi_height,
                                                 MbModeInfo* nb_mi, void* fun_ctxt) {
    (void)nb_mi;
    (void)is16bit;

    // Use tbl for doing a double-width zero extension from 8->32 bits since we
    // can do this in one instruction rather than two.
    uint8x16_t pre_idx0 = vld1q_u8(&obmc_variance_permute_idx[0]);
    uint8x16_t pre_idx1 = vld1q_u8(&obmc_variance_permute_idx[16]);
    uint8x16_t pre_idx2 = vld1q_u8(&obmc_variance_permute_idx[32]);
    uint8x16_t pre_idx3 = vld1q_u8(&obmc_variance_permute_idx[48]);

    struct calc_target_weighted_pred_ctxt* ctxt = (struct calc_target_weighted_pred_ctxt*)fun_ctxt;
    assert(ctxt->overlap >= 2);
    assert(svt_av1_get_obmc_mask(ctxt->overlap) != NULL);

    const int bw = xd->n4_w << MI_SIZE_LOG2;

    int32_t*       wsrc = ctxt->wsrc_buf + (rel_mi_row * MI_SIZE * bw);
    int32_t*       mask = ctxt->mask_buf + (rel_mi_row * MI_SIZE * bw);
    const uint8_t* tmp  = ctxt->tmp + (rel_mi_row * MI_SIZE * ctxt->tmp_stride);

    if (ctxt->overlap == 2) {
        const int32_t mask1d0[2] = {45, 64};
        // We can compute m1 ahead of time and void the separate shift left by AOM_BLEND_A64_ROUND_BITS.
        // We will then have m1[i] = 64 * (64 - m0[i]).
        const int32_t mask1d1[2] = {1216, 0};

        int32x4_t m0 = vreinterpretq_s32_s64(vld1q_dup_s64((int64_t*)mask1d0));
        int32x4_t m1 = vreinterpretq_s32_s64(vld1q_dup_s64((int64_t*)mask1d1));
        // MI_SIZE = 4 so it's fine to do 4 rows at a time.
        int row = nb_mi_height * MI_SIZE;
        do {
            uint8x16_t tmp_u8 = vcombine_u8(load_u8_2x4(tmp, ctxt->tmp_stride), vdup_n_u8(0));

            int32x4_t tmp_lo = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx0));
            int32x4_t tmp_hi = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx1));

            int32x4_t wsrc_lo_s32 = load_s32_2x2(wsrc + 0 * bw, bw);
            int32x4_t wsrc_hi_s32 = load_s32_2x2(wsrc + 2 * bw, bw);
            int32x4_t mask_lo_s32 = load_s32_2x2(mask + 0 * bw, bw);
            int32x4_t mask_hi_s32 = load_s32_2x2(mask + 2 * bw, bw);

            wsrc_lo_s32 = vshrq_n_s32(wsrc_lo_s32, AOM_BLEND_A64_ROUND_BITS);
            wsrc_hi_s32 = vshrq_n_s32(wsrc_hi_s32, AOM_BLEND_A64_ROUND_BITS);
            mask_lo_s32 = vshrq_n_s32(mask_lo_s32, AOM_BLEND_A64_ROUND_BITS);
            mask_hi_s32 = vshrq_n_s32(mask_hi_s32, AOM_BLEND_A64_ROUND_BITS);

            wsrc_lo_s32 = vmulq_s32(wsrc_lo_s32, m0);
            wsrc_hi_s32 = vmulq_s32(wsrc_hi_s32, m0);
            wsrc_lo_s32 = vmlaq_s32(wsrc_lo_s32, tmp_lo, m1);
            wsrc_hi_s32 = vmlaq_s32(wsrc_hi_s32, tmp_hi, m1);

            vst1_s32(wsrc + 0 * bw, vget_low_s32(wsrc_lo_s32));
            vst1_s32(wsrc + 1 * bw, vget_high_s32(wsrc_lo_s32));
            vst1_s32(wsrc + 2 * bw, vget_low_s32(wsrc_hi_s32));
            vst1_s32(wsrc + 3 * bw, vget_high_s32(wsrc_hi_s32));

            mask_lo_s32 = vmulq_s32(mask_lo_s32, m0);
            mask_hi_s32 = vmulq_s32(mask_hi_s32, m0);

            vst1_s32(mask + 0 * bw, vget_low_s32(mask_lo_s32));
            vst1_s32(mask + 1 * bw, vget_high_s32(mask_lo_s32));
            vst1_s32(mask + 2 * bw, vget_low_s32(mask_hi_s32));
            vst1_s32(mask + 3 * bw, vget_high_s32(mask_hi_s32));

            wsrc += 4 * bw;
            mask += 4 * bw;
            tmp += 4 * ctxt->tmp_stride;
            row -= 4;
        } while (row != 0);
    } else if (ctxt->overlap == 4) {
        const int32_t mask1d0[4] = {39, 50, 59, 64};
        // We can compute m1 ahead of time and void the separate shift left by AOM_BLEND_A64_ROUND_BITS.
        // We will then have m1[i] = 64 * (64 - m0[i]).
        const int32_t mask1d1[4] = {1600, 896, 320, 0};

        int32x4_t m0  = vld1q_s32(mask1d0);
        int32x4_t m1  = vld1q_s32(mask1d1);
        int       row = nb_mi_height * MI_SIZE;
        do {
            uint8x16_t tmp_u8 = load_u8_4x4(tmp, ctxt->tmp_stride);

            int32x4_t tmp_s32[2];
            tmp_s32[0] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx0));
            tmp_s32[1] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx1));

            weighted_pred_left_neon(wsrc, mask, tmp_s32[0], tmp_s32[1], m0, m0, m1, m1, bw);

            wsrc += 2 * bw;
            mask += 2 * bw;
            tmp += 2 * ctxt->tmp_stride;
            row -= 2;
        } while (row != 0);
    } else if (ctxt->overlap == 8) {
        const int32_t mask1d0[8] = {36, 42, 48, 53, 57, 61, 64, 64};
        // We can compute m1 ahead of time and void the separate shift left by AOM_BLEND_A64_ROUND_BITS.
        // We will then have m1[i] = 64 * (64 - m0[i]).
        const int32_t mask1d1[8] = {1792, 1408, 1024, 704, 448, 192, 0, 0};
        int32x4_t     m0[2], m1[2];

        load_s32_4x2((int32_t*)mask1d0, 4, &m0[0], &m0[1]);
        load_s32_4x2((int32_t*)mask1d1, 4, &m1[0], &m1[1]);

        int row = nb_mi_height * MI_SIZE;
        do {
            uint8x16_t tmp_u8 = vcombine_u8(vld1_u8(tmp), vdup_n_u8(0));

            int32x4_t tmp_s32[2];
            tmp_s32[0] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx0));
            tmp_s32[1] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx1));

            weighted_pred_left_neon(wsrc, mask, tmp_s32[0], tmp_s32[1], m0[0], m0[1], m1[0], m1[1], 4);

            wsrc += bw;
            mask += bw;
            tmp += ctxt->tmp_stride;
        } while (--row != 0);
    } else if (ctxt->overlap == 16) {
        const int32_t mask1d0[16] = {34, 37, 40, 43, 46, 49, 52, 54, 56, 58, 60, 61, 64, 64, 64, 64};
        // We can compute m1 ahead of time and void the separate shift left by AOM_BLEND_A64_ROUND_BITS.
        // We will then have m1[i] = 64 * (64 - m0[i]).
        const int32_t mask1d1[16] = {1920, 1728, 1536, 1344, 1152, 960, 768, 640, 512, 384, 256, 192, 0, 0, 0, 0};

        int32x4_t m0[4], m1[4];
        load_s32_4x4((int32_t*)mask1d0, 4, &m0[0], &m0[1], &m0[2], &m0[3]);
        load_s32_4x4((int32_t*)mask1d1, 4, &m1[0], &m1[1], &m1[2], &m1[3]);

        int row = nb_mi_height * MI_SIZE;
        do {
            uint8x16_t tmp_u8 = vld1q_u8(tmp);
            int32x4_t  tmp_s32[4];
            tmp_s32[0] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx0));
            tmp_s32[1] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx1));
            tmp_s32[2] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx2));
            tmp_s32[3] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx3));

            weighted_pred_left_neon(wsrc + 0, mask + 0, tmp_s32[0], tmp_s32[1], m0[0], m0[1], m1[0], m1[1], 4);
            weighted_pred_left_neon(wsrc + 8, mask + 8, tmp_s32[2], tmp_s32[3], m0[2], m0[3], m1[2], m1[3], 4);

            wsrc += bw;
            mask += bw;
            tmp += ctxt->tmp_stride;
        } while (--row != 0);
    } else {
        const int32_t mask1d0[32] = {33, 35, 36, 38, 40, 41, 43, 44, 45, 47, 48, 50, 51, 52, 53, 55,
                                     56, 57, 58, 59, 60, 60, 61, 62, 64, 64, 64, 64, 64, 64, 64, 64};
        // We can compute m1 ahead of time and void the separate shift left by AOM_BLEND_A64_ROUND_BITS.
        // We will then have m1[i] = 64 * (64 - m0[i]).
        const int32_t mask1d1[32] = {1984, 1856, 1792, 1664, 1536, 1472, 1344, 1280, 1216, 1088, 1024,
                                     896,  832,  768,  704,  576,  512,  448,  384,  320,  256,  256,
                                     192,  128,  0,    0,    0,    0,    0,    0,    0,    0};
        int32x4_t     m0[8], m1[8];
        load_s32_4x8((int32_t*)mask1d0, 4, &m0[0], &m0[1], &m0[2], &m0[3], &m0[4], &m0[5], &m0[6], &m0[7]);
        load_s32_4x8((int32_t*)mask1d1, 4, &m1[0], &m1[1], &m1[2], &m1[3], &m1[4], &m1[5], &m1[6], &m1[7]);

        int row = nb_mi_height * MI_SIZE;
        do {
            uint8x16_t tmp_u8 = vld1q_u8(tmp);
            int32x4_t  tmp_s32[8];
            tmp_s32[0] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx0));
            tmp_s32[1] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx1));
            tmp_s32[2] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx2));
            tmp_s32[3] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx3));

            weighted_pred_left_neon(wsrc + 0, mask + 0, tmp_s32[0], tmp_s32[1], m0[0], m0[1], m1[0], m1[1], 4);
            weighted_pred_left_neon(wsrc + 8, mask + 8, tmp_s32[2], tmp_s32[3], m0[2], m0[3], m1[2], m1[3], 4);

            tmp_u8     = vld1q_u8(tmp + 16);
            tmp_s32[4] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx0));
            tmp_s32[5] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx1));
            tmp_s32[6] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx2));
            tmp_s32[7] = vreinterpretq_s32_u8(vqtbl1q_u8(tmp_u8, pre_idx3));

            weighted_pred_left_neon(wsrc + 16, mask + 16, tmp_s32[4], tmp_s32[5], m0[4], m0[5], m1[4], m1[5], 4);
            weighted_pred_left_neon(wsrc + 24, mask + 24, tmp_s32[6], tmp_s32[7], m0[6], m0[7], m1[6], m1[7], 4);

            wsrc += bw;
            mask += bw;
            tmp += ctxt->tmp_stride;
        } while (--row != 0);
    }
}
