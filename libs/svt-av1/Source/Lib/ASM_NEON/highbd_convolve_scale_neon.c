/*
 * Copyright (c) 2024, Alliance for Open Media. All rights reserved.
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
#include <stdint.h>

#include "common_dsp_rtcd.h"
#include "compound_convolve_neon.h"
#include "convolve_scale_neon.h"
#include "filter.h"
#include "highbd_convolve_scale_neon.h"
#include "mem_neon.h"
#include "transpose_neon.h"
#include "utility.h"
#include "svt_malloc.h"

static inline void highbd_dist_wtd_comp_avg_neon(const uint16_t* src_ptr, int src_stride, uint16_t* dst_ptr,
                                                 int dst_stride, int w, int h, ConvolveParams* conv_params,
                                                 const int round_bits, const int offset, const int bd) {
    CONV_BUF_TYPE*   ref_ptr     = conv_params->dst;
    const int        ref_stride  = conv_params->dst_stride;
    const int32x4_t  round_shift = vdupq_n_s32(-round_bits);
    const uint32x4_t offset_vec  = vdupq_n_u32(offset);
    const uint16x8_t max         = vdupq_n_u16((1 << bd) - 1);
    uint16x4_t       fwd_offset  = vdup_n_u16(conv_params->fwd_offset);
    uint16x4_t       bck_offset  = vdup_n_u16(conv_params->bck_offset);

    // Weighted averaging
    if (w <= 4) {
        do {
            const uint16x4_t src = vld1_u16(src_ptr);
            const uint16x4_t ref = vld1_u16(ref_ptr);

            uint32x4_t wtd_avg = vmull_u16(ref, fwd_offset);
            wtd_avg            = vmlal_u16(wtd_avg, src, bck_offset);
            wtd_avg            = vshrq_n_u32(wtd_avg, DIST_PRECISION_BITS);
            int32x4_t d0       = vreinterpretq_s32_u32(vsubq_u32(wtd_avg, offset_vec));
            d0                 = vqrshlq_s32(d0, round_shift);

            uint16x4_t d0_u16 = vqmovun_s32(d0);
            d0_u16            = vmin_u16(d0_u16, vget_low_u16(max));

            if (w == 2) {
                store_u16_2x1(dst_ptr, d0_u16);
            } else {
                vst1_u16(dst_ptr, d0_u16);
            }

            src_ptr += src_stride;
            dst_ptr += dst_stride;
            ref_ptr += ref_stride;
        } while (--h != 0);
    } else {
        do {
            int             width = w;
            const uint16_t* src   = src_ptr;
            const uint16_t* ref   = ref_ptr;
            uint16_t*       dst   = dst_ptr;
            do {
                const uint16x8_t s = vld1q_u16(src);
                const uint16x8_t r = vld1q_u16(ref);

                uint32x4_t wtd_avg0 = vmull_u16(vget_low_u16(r), fwd_offset);
                wtd_avg0            = vmlal_u16(wtd_avg0, vget_low_u16(s), bck_offset);
                wtd_avg0            = vshrq_n_u32(wtd_avg0, DIST_PRECISION_BITS);
                int32x4_t d0        = vreinterpretq_s32_u32(vsubq_u32(wtd_avg0, offset_vec));
                d0                  = vqrshlq_s32(d0, round_shift);

                uint32x4_t wtd_avg1 = vmull_u16(vget_high_u16(r), fwd_offset);
                wtd_avg1            = vmlal_u16(wtd_avg1, vget_high_u16(s), bck_offset);
                wtd_avg1            = vshrq_n_u32(wtd_avg1, DIST_PRECISION_BITS);
                int32x4_t d1        = vreinterpretq_s32_u32(vsubq_u32(wtd_avg1, offset_vec));
                d1                  = vqrshlq_s32(d1, round_shift);

                uint16x8_t d01 = vcombine_u16(vqmovun_s32(d0), vqmovun_s32(d1));
                d01            = vminq_u16(d01, max);
                vst1q_u16(dst, d01);

                src += 8;
                ref += 8;
                dst += 8;
                width -= 8;
            } while (width != 0);
            src_ptr += src_stride;
            dst_ptr += dst_stride;
            ref_ptr += ref_stride;
        } while (--h != 0);
    }
}

static inline void highbd_comp_avg_neon(const uint16_t* src_ptr, int src_stride, uint16_t* dst_ptr, int dst_stride,
                                        int w, int h, ConvolveParams* conv_params, const int round_bits,
                                        const int offset, const int bd) {
    CONV_BUF_TYPE*   ref_ptr     = conv_params->dst;
    const int        ref_stride  = conv_params->dst_stride;
    const int32x4_t  round_shift = vdupq_n_s32(-round_bits);
    const uint16x4_t offset_vec  = vdup_n_u16(offset);
    const uint16x8_t max         = vdupq_n_u16((1 << bd) - 1);

    if (w <= 4) {
        do {
            const uint16x4_t src = vld1_u16(src_ptr);
            const uint16x4_t ref = vld1_u16(ref_ptr);

            uint16x4_t avg = vhadd_u16(src, ref);
            int32x4_t  d0  = vreinterpretq_s32_u32(vsubl_u16(avg, offset_vec));
            d0             = vqrshlq_s32(d0, round_shift);

            uint16x4_t d0_u16 = vqmovun_s32(d0);
            d0_u16            = vmin_u16(d0_u16, vget_low_u16(max));

            if (w == 2) {
                store_u16_2x1(dst_ptr, d0_u16);
            } else {
                vst1_u16(dst_ptr, d0_u16);
            }

            src_ptr += src_stride;
            ref_ptr += ref_stride;
            dst_ptr += dst_stride;
        } while (--h != 0);
    } else {
        do {
            int             width = w;
            const uint16_t* src   = src_ptr;
            const uint16_t* ref   = ref_ptr;
            uint16_t*       dst   = dst_ptr;
            do {
                const uint16x8_t s = vld1q_u16(src);
                const uint16x8_t r = vld1q_u16(ref);

                uint16x8_t avg   = vhaddq_u16(s, r);
                int32x4_t  d0_lo = vreinterpretq_s32_u32(vsubl_u16(vget_low_u16(avg), offset_vec));
                int32x4_t  d0_hi = vreinterpretq_s32_u32(vsubl_u16(vget_high_u16(avg), offset_vec));
                d0_lo            = vqrshlq_s32(d0_lo, round_shift);
                d0_hi            = vqrshlq_s32(d0_hi, round_shift);

                uint16x8_t d0 = vcombine_u16(vqmovun_s32(d0_lo), vqmovun_s32(d0_hi));
                d0            = vminq_u16(d0, max);
                vst1q_u16(dst, d0);

                src += 8;
                ref += 8;
                dst += 8;
                width -= 8;
            } while (width != 0);

            src_ptr += src_stride;
            ref_ptr += ref_stride;
            dst_ptr += dst_stride;
        } while (--h != 0);
    }
}

static inline void highbd_convolve_2d_x_scale_8tap_neon(const uint16_t* src_ptr, int src_stride, uint16_t* dst_ptr,
                                                        int dst_stride, int w, int h, const int subpel_x_qn,
                                                        const int x_step_qn, const InterpFilterParams* filter_params,
                                                        ConvolveParams* conv_params, const int offset) {
    const int32x4_t shift_s32  = vdupq_n_s32(-conv_params->round_0);
    const int32x4_t offset_s32 = vdupq_n_s32(offset);

    if (w <= 4) {
        int       height = h;
        uint16_t* d      = dst_ptr;

        do {
            int x_qn = subpel_x_qn;

            const int16_t* src4_ptr[4];
            src4_ptr[0] = (int16_t*)&src_ptr[((x_qn + 0 * x_step_qn) >> SCALE_SUBPEL_BITS)];
            src4_ptr[1] = (int16_t*)&src_ptr[((x_qn + 1 * x_step_qn) >> SCALE_SUBPEL_BITS)];
            src4_ptr[2] = (int16_t*)&src_ptr[((x_qn + 2 * x_step_qn) >> SCALE_SUBPEL_BITS)];
            src4_ptr[3] = (int16_t*)&src_ptr[((x_qn + 3 * x_step_qn) >> SCALE_SUBPEL_BITS)];

            // Load source
            int16x8_t s0 = vld1q_s16(src4_ptr[0]);
            int16x8_t s1 = vld1q_s16(src4_ptr[1]);
            int16x8_t s2 = vld1q_s16(src4_ptr[2]);
            int16x8_t s3 = vld1q_s16(src4_ptr[3]);

            const int16_t filter_offset0 = SUBPEL_TAPS *
                (((x_qn + 0 * x_step_qn) & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS);
            const int16_t filter_offset1 = SUBPEL_TAPS *
                (((x_qn + 1 * x_step_qn) & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS);
            const int16_t filter_offset2 = SUBPEL_TAPS *
                (((x_qn + 2 * x_step_qn) & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS);
            const int16_t filter_offset3 = SUBPEL_TAPS *
                (((x_qn + 3 * x_step_qn) & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS);

            // Load the filters
            const int16x8_t x_filter0 = vld1q_s16(filter_params->filter_ptr + filter_offset0);
            const int16x8_t x_filter1 = vld1q_s16(filter_params->filter_ptr + filter_offset1);
            const int16x8_t x_filter2 = vld1q_s16(filter_params->filter_ptr + filter_offset2);
            const int16x8_t x_filter3 = vld1q_s16(filter_params->filter_ptr + filter_offset3);

            int16x8_t filters[4] = {x_filter0, x_filter1, x_filter2, x_filter3};
            transpose_array_inplace_u16_4x8((uint16x8_t*)filters);

            int16x4_t filters_lo[] = {
                vget_low_s16(filters[0]), vget_low_s16(filters[1]), vget_low_s16(filters[2]), vget_low_s16(filters[3])};
            int16x4_t filters_hi[] = {vget_high_s16(filters[0]),
                                      vget_high_s16(filters[1]),
                                      vget_high_s16(filters[2]),
                                      vget_high_s16(filters[3])};

            // Run the 2D Scale convolution
            uint16x4_t d0 = highbd_convolve8_2d_scale_horiz4x8_s32_s16(
                s0, s1, s2, s3, filters_lo, filters_hi, shift_s32, offset_s32);

            if (w == 2) {
                store_u16_2x1(d, d0);
            } else {
                vst1_u16(d, d0);
            }

            src_ptr += src_stride;
            d += dst_stride;
            height--;
        } while (height > 0);
    } else {
        int height = h;

        do {
            int             width = w;
            int             x_qn  = subpel_x_qn;
            uint16_t*       d     = dst_ptr;
            const uint16_t* s     = src_ptr;

            do {
                int16_t* src4_ptr[4];
                src4_ptr[0] = (int16_t*)&s[((x_qn + 0 * x_step_qn) >> SCALE_SUBPEL_BITS)];
                src4_ptr[1] = (int16_t*)&s[((x_qn + 1 * x_step_qn) >> SCALE_SUBPEL_BITS)];
                src4_ptr[2] = (int16_t*)&s[((x_qn + 2 * x_step_qn) >> SCALE_SUBPEL_BITS)];
                src4_ptr[3] = (int16_t*)&s[((x_qn + 3 * x_step_qn) >> SCALE_SUBPEL_BITS)];

                // Load source
                int16x8_t s0 = vld1q_s16(src4_ptr[0]);
                int16x8_t s1 = vld1q_s16(src4_ptr[1]);
                int16x8_t s2 = vld1q_s16(src4_ptr[2]);
                int16x8_t s3 = vld1q_s16(src4_ptr[3]);

                const int16_t filter_offset0 = SUBPEL_TAPS *
                    (((x_qn + 0 * x_step_qn) & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS);
                const int16_t filter_offset1 = SUBPEL_TAPS *
                    (((x_qn + 1 * x_step_qn) & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS);
                const int16_t filter_offset2 = SUBPEL_TAPS *
                    (((x_qn + 2 * x_step_qn) & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS);
                const int16_t filter_offset3 = SUBPEL_TAPS *
                    (((x_qn + 3 * x_step_qn) & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS);

                // Load the filters
                const int16x8_t x_filter0 = vld1q_s16(filter_params->filter_ptr + filter_offset0);
                const int16x8_t x_filter1 = vld1q_s16(filter_params->filter_ptr + filter_offset1);
                const int16x8_t x_filter2 = vld1q_s16(filter_params->filter_ptr + filter_offset2);
                const int16x8_t x_filter3 = vld1q_s16(filter_params->filter_ptr + filter_offset3);

                int16x8_t filters[4] = {x_filter0, x_filter1, x_filter2, x_filter3};
                transpose_array_inplace_u16_4x8((uint16x8_t*)filters);

                int16x4_t filters_lo[] = {vget_low_s16(filters[0]),
                                          vget_low_s16(filters[1]),
                                          vget_low_s16(filters[2]),
                                          vget_low_s16(filters[3])};
                int16x4_t filters_hi[] = {vget_high_s16(filters[0]),
                                          vget_high_s16(filters[1]),
                                          vget_high_s16(filters[2]),
                                          vget_high_s16(filters[3])};

                // Run the 2D Scale X convolution
                uint16x4_t d0 = highbd_convolve8_2d_scale_horiz4x8_s32_s16(
                    s0, s1, s2, s3, filters_lo, filters_hi, shift_s32, offset_s32);

                vst1_u16(d, d0);

                x_qn += 4 * x_step_qn;
                d += 4;
                width -= 4;
            } while (width > 0);

            src_ptr += src_stride;
            dst_ptr += dst_stride;
            height--;
        } while (height > 0);
    }
}

static inline void highbd_convolve_2d_y_scale_8tap_neon(const uint16_t* src_ptr, int src_stride, uint16_t* dst_ptr,
                                                        int dst_stride, int w, int h, const int subpel_y_qn,
                                                        const int y_step_qn, const InterpFilterParams* filter_params,
                                                        const int round1_bits, const int offset) {
    const int32x4_t offset_s32 = vdupq_n_s32(1 << offset);

    const int32x4_t round1_shift_s32 = vdupq_n_s32(-round1_bits);
    if (w <= 4) {
        int       height = h;
        uint16_t* d      = dst_ptr;
        int       y_qn   = subpel_y_qn;

        do {
            const int16_t* s = (const int16_t*)&src_ptr[(y_qn >> SCALE_SUBPEL_BITS) * src_stride];

            int16x4_t s0, s1, s2, s3, s4, s5, s6, s7;
            load_s16_4x8(s, src_stride, &s0, &s1, &s2, &s3, &s4, &s5, &s6, &s7);

            const int       y_filter_idx = (y_qn & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS;
            const int16_t*  y_filter_ptr = av1_get_interp_filter_subpel_kernel(*filter_params, y_filter_idx);
            const int16x8_t y_filter     = vld1q_s16(y_filter_ptr);

            uint16x4_t d0 = highbd_convolve8_4_srsub_s32_s16(
                s0, s1, s2, s3, s4, s5, s6, s7, y_filter, round1_shift_s32, offset_s32);

            if (w == 2) {
                store_u16_2x1(d, d0);
            } else {
                vst1_u16(d, d0);
            }

            y_qn += y_step_qn;
            d += dst_stride;
            height--;
        } while (height > 0);
    } else {
        int width = w;

        do {
            int height = h;
            int y_qn   = subpel_y_qn;

            uint16_t* d = dst_ptr;

            do {
                const int16_t* s = (const int16_t*)&src_ptr[(y_qn >> SCALE_SUBPEL_BITS) * src_stride];
                int16x8_t      s0, s1, s2, s3, s4, s5, s6, s7;
                load_s16_8x8(s, src_stride, &s0, &s1, &s2, &s3, &s4, &s5, &s6, &s7);

                const int       y_filter_idx = (y_qn & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS;
                const int16_t*  y_filter_ptr = av1_get_interp_filter_subpel_kernel(*filter_params, y_filter_idx);
                const int16x8_t y_filter     = vld1q_s16(y_filter_ptr);

                uint16x8_t d0 = highbd_convolve8_8_srsub_s32_s16(
                    s0, s1, s2, s3, s4, s5, s6, s7, y_filter, round1_shift_s32, offset_s32);
                vst1q_u16(d, d0);

                y_qn += y_step_qn;
                d += dst_stride;
                height--;
            } while (height > 0);
            src_ptr += 8;
            dst_ptr += 8;
            width -= 8;
        } while (width > 0);
    }
}

static inline void highbd_convolve_correct_offset_neon(const uint16_t* src_ptr, int src_stride, uint16_t* dst_ptr,
                                                       int dst_stride, int w, int h, const int round_bits,
                                                       const int offset, const int bd) {
    const int32x4_t  round_shift_s32 = vdupq_n_s32(-round_bits);
    const int16x4_t  offset_s16      = vdup_n_s16(offset);
    const uint16x8_t max             = vdupq_n_u16((1 << bd) - 1);

    if (w <= 4) {
        for (int y = 0; y < h; ++y) {
            const int16x4_t s  = vld1_s16((const int16_t*)src_ptr + y * src_stride);
            const int32x4_t d0 = vqrshlq_s32(vsubl_s16(s, offset_s16), round_shift_s32);
            uint16x4_t      d  = vqmovun_s32(d0);
            d                  = vmin_u16(d, vget_low_u16(max));
            if (w == 2) {
                store_u16_2x1(dst_ptr + y * dst_stride, d);
            } else {
                vst1_u16(dst_ptr + y * dst_stride, d);
            }
        }
    } else {
        for (int y = 0; y < h; ++y) {
            for (int x = 0; x < w; x += 8) {
                // Subtract round offset and convolve round
                const int16x8_t s   = vld1q_s16((const int16_t*)src_ptr + y * src_stride + x);
                const int32x4_t d0  = vqrshlq_s32(vsubl_s16(vget_low_s16(s), offset_s16), round_shift_s32);
                const int32x4_t d1  = vqrshlq_s32(vsubl_s16(vget_high_s16(s), offset_s16), round_shift_s32);
                uint16x8_t      d01 = vcombine_u16(vqmovun_s32(d0), vqmovun_s32(d1));
                d01                 = vminq_u16(d01, max);
                vst1q_u16(dst_ptr + y * dst_stride + x, d01);
            }
        }
    }
}

void svt_av1_highbd_convolve_2d_scale_neon(const uint16_t* src, int src_stride, uint16_t* dst, int dst_stride, int w,
                                           int h, const InterpFilterParams* filter_params_x,
                                           const InterpFilterParams* filter_params_y, const int subpel_x_qn,
                                           const int x_step_qn, const int subpel_y_qn, const int y_step_qn,
                                           ConvolveParams* conv_params, int bd) {
    uint16_t* im_block = (uint16_t*)svt_aom_memalign(
        16, 2 * sizeof(uint16_t) * MAX_SB_SIZE * (MAX_SB_SIZE + MAX_FILTER_TAP));
    if (!im_block) {
        return;
    }
    uint16_t* im_block2 = (uint16_t*)svt_aom_memalign(
        16, 2 * sizeof(uint16_t) * MAX_SB_SIZE * (MAX_SB_SIZE + MAX_FILTER_TAP));
    if (!im_block2) {
        svt_aom_free(im_block); // free the first block and return.
        return;
    }

    int       im_h      = (((h - 1) * y_step_qn + subpel_y_qn) >> SCALE_SUBPEL_BITS) + filter_params_y->taps;
    const int im_stride = MAX_SB_SIZE;
    const int bits      = FILTER_BITS * 2 - conv_params->round_0 - conv_params->round_1;
    assert(bits >= 0);

    const int vert_offset         = filter_params_y->taps / 2 - 1;
    const int horiz_offset        = filter_params_x->taps / 2 - 1;
    const int x_offset_bits       = (1 << (bd + FILTER_BITS - 1));
    const int y_offset_bits       = bd + 2 * FILTER_BITS - conv_params->round_0;
    const int y_offset_correction = ((1 << (y_offset_bits - conv_params->round_1)) +
                                     (1 << (y_offset_bits - conv_params->round_1 - 1)));

    CONV_BUF_TYPE* dst16        = conv_params->dst;
    const int      dst16_stride = conv_params->dst_stride;

    const uint16_t* src_ptr = src - vert_offset * src_stride - horiz_offset;

    highbd_convolve_2d_x_scale_8tap_neon(src_ptr,
                                         src_stride,
                                         im_block,
                                         im_stride,
                                         w,
                                         im_h,
                                         subpel_x_qn,
                                         x_step_qn,
                                         filter_params_x,
                                         conv_params,
                                         x_offset_bits);
    if (conv_params->is_compound && !conv_params->do_average) {
        highbd_convolve_2d_y_scale_8tap_neon(im_block,
                                             im_stride,
                                             dst16,
                                             dst16_stride,
                                             w,
                                             h,
                                             subpel_y_qn,
                                             y_step_qn,
                                             filter_params_y,
                                             conv_params->round_1,
                                             y_offset_bits);
    } else {
        highbd_convolve_2d_y_scale_8tap_neon(im_block,
                                             im_stride,
                                             im_block2,
                                             im_stride,
                                             w,
                                             h,
                                             subpel_y_qn,
                                             y_step_qn,
                                             filter_params_y,
                                             conv_params->round_1,
                                             y_offset_bits);
    }

    // Do the compound averaging outside the loop, avoids branching within the
    // main loop
    if (!conv_params->is_compound || !conv_params->do_average) {
        highbd_convolve_correct_offset_neon(im_block2, im_stride, dst, dst_stride, w, h, bits, y_offset_correction, bd);
    } else if (conv_params->use_dist_wtd_comp_avg) {
        highbd_dist_wtd_comp_avg_neon(
            im_block2, im_stride, dst, dst_stride, w, h, conv_params, bits, y_offset_correction, bd);
    } else {
        highbd_comp_avg_neon(im_block2, im_stride, dst, dst_stride, w, h, conv_params, bits, y_offset_correction, bd);
    }
    svt_aom_free(im_block);
    svt_aom_free(im_block2);
}
