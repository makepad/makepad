/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 3-Clause Clear License and
* the Alliance for Open Media Patent License 1.0. If the BSD 3-Clause Clear License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <stdlib.h>

#include "inter_prediction.h"
#include "convolve.h"
#include "common_dsp_rtcd.h"
#include "utility.h"
#include "pic_operators.h"

#define SCALE_SUBPEL_BITS 10
#define SCALE_SUBPEL_SHIFTS (1 << SCALE_SUBPEL_BITS)
#define SCALE_SUBPEL_MASK (SCALE_SUBPEL_SHIFTS - 1)
#define SCALE_EXTRA_BITS (SCALE_SUBPEL_BITS - SUBPEL_BITS)

void svt_aom_pack_block(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer, uint32_t inn_stride,
                        uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t width, uint32_t height) {
    svt_aom_pack2d_src(
        in8_bit_buffer, in8_stride, inn_bit_buffer, inn_stride, out16_bit_buffer, out_stride, width, height);
}

static WedgeMasksType wedge_masks[BLOCK_SIZES_ALL][2];

int svt_aom_is_masked_compound_type(COMPOUND_TYPE type) {
    return (type == COMPOUND_WEDGE || type == COMPOUND_DIFFWTD);
}

void svt_aom_highbd_subtract_block_c(int rows, int cols, int16_t* diff, ptrdiff_t diff_stride, const uint8_t* src8,
                                     ptrdiff_t src_stride, const uint8_t* pred8, ptrdiff_t pred_stride, int bd) {
    uint16_t* src  = (uint16_t*)(src8);
    uint16_t* pred = (uint16_t*)(pred8);
    (void)bd;

    for (int r = 0; r < rows; r++) {
        for (int c = 0; c < cols; c++) {
            diff[c] = src[c] - pred[c];
        }

        diff += diff_stride;
        pred += pred_stride;
        src += src_stride;
    }
}

void svt_aom_subtract_block_c(int rows, int cols, int16_t* diff, ptrdiff_t diff_stride, const uint8_t* src,
                              ptrdiff_t src_stride, const uint8_t* pred, ptrdiff_t pred_stride) {
    for (int r = 0; r < rows; r++) {
        for (int c = 0; c < cols; c++) {
            diff[c] = src[c] - pred[c];
        }

        diff += diff_stride;
        pred += pred_stride;
        src += src_stride;
    }
}

static void diffwtd_mask(uint8_t* mask, int which_inverse, int mask_base, const uint8_t* src0, int src0_stride,
                         const uint8_t* src1, int src1_stride, int h, int w) {
    for (int i = 0; i < h; ++i) {
        for (int j = 0; j < w; ++j) {
            int diff        = abs((int)src0[i * src0_stride + j] - (int)src1[i * src1_stride + j]);
            int m           = clamp(mask_base + (diff / DIFF_FACTOR), 0, AOM_BLEND_A64_MAX_ALPHA);
            mask[i * w + j] = which_inverse ? AOM_BLEND_A64_MAX_ALPHA - m : m;
        }
    }
}

static AOM_FORCE_INLINE void diffwtd_mask_highbd(uint8_t* mask, int which_inverse, int mask_base, const uint16_t* src0,
                                                 int src0_stride, const uint16_t* src1, int src1_stride, int h, int w,
                                                 const unsigned int bd) {
    assert(bd >= 8);
    if (bd == 8) {
        if (which_inverse) {
            for (int i = 0; i < h; ++i) {
                for (int j = 0; j < w; ++j) {
                    int          diff = abs((int)src0[j] - (int)src1[j]) / DIFF_FACTOR;
                    unsigned int m    = negative_to_zero(mask_base + diff);
                    m                 = AOMMIN(m, AOM_BLEND_A64_MAX_ALPHA);
                    mask[j]           = AOM_BLEND_A64_MAX_ALPHA - m;
                }
                src0 += src0_stride;
                src1 += src1_stride;
                mask += w;
            }
        } else {
            for (int i = 0; i < h; ++i) {
                for (int j = 0; j < w; ++j) {
                    int          diff = abs((int)src0[j] - (int)src1[j]) / DIFF_FACTOR;
                    unsigned int m    = negative_to_zero(mask_base + diff);
                    m                 = AOMMIN(m, AOM_BLEND_A64_MAX_ALPHA);
                    mask[j]           = m;
                }
                src0 += src0_stride;
                src1 += src1_stride;
                mask += w;
            }
        }
    } else {
        const unsigned int bd_shift = bd - 8;
        if (which_inverse) {
            for (int i = 0; i < h; ++i) {
                for (int j = 0; j < w; ++j) {
                    int          diff = (abs((int)src0[j] - (int)src1[j]) >> bd_shift) / DIFF_FACTOR;
                    unsigned int m    = negative_to_zero(mask_base + diff);
                    m                 = AOMMIN(m, AOM_BLEND_A64_MAX_ALPHA);
                    mask[j]           = AOM_BLEND_A64_MAX_ALPHA - m;
                }
                src0 += src0_stride;
                src1 += src1_stride;
                mask += w;
            }
        } else {
            for (int i = 0; i < h; ++i) {
                for (int j = 0; j < w; ++j) {
                    int          diff = (abs((int)src0[j] - (int)src1[j]) >> bd_shift) / DIFF_FACTOR;
                    unsigned int m    = negative_to_zero(mask_base + diff);
                    m                 = AOMMIN(m, AOM_BLEND_A64_MAX_ALPHA);
                    mask[j]           = m;
                }
                src0 += src0_stride;
                src1 += src1_stride;
                mask += w;
            }
        }
    }
}

void svt_av1_build_compound_diffwtd_mask_highbd_c(uint8_t* mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t* src0,
                                                  int src0_stride, const uint8_t* src1, int src1_stride, int h, int w,
                                                  int bd) {
    switch (mask_type) {
    case DIFFWTD_38:
        diffwtd_mask_highbd(mask, 0, 38, (uint16_t*)src0, src0_stride, (uint16_t*)src1, src1_stride, h, w, bd);
        break;
    case DIFFWTD_38_INV:
        diffwtd_mask_highbd(mask, 1, 38, (uint16_t*)src0, src0_stride, (uint16_t*)src1, src1_stride, h, w, bd);
        break;
    default:
        assert(0);
    }
}

void svt_av1_build_compound_diffwtd_mask_c(uint8_t* mask, DIFFWTD_MASK_TYPE mask_type, const uint8_t* src0,
                                           int src0_stride, const uint8_t* src1, int src1_stride, int h, int w) {
    switch (mask_type) {
    case DIFFWTD_38:
        diffwtd_mask(mask, 0, 38, src0, src0_stride, src1, src1_stride, h, w);
        break;
    case DIFFWTD_38_INV:
        diffwtd_mask(mask, 1, 38, src0, src0_stride, src1, src1_stride, h, w);
        break;
    default:
        assert(0);
    }
}

// Note: Expect val to be in q4 precision
static INLINE int32_t scaled_x(int32_t val, const ScaleFactors* sf) {
    const int     off  = (sf->x_scale_fp - (1 << REF_SCALE_SHIFT)) * (1 << (SUBPEL_BITS - 1));
    const int64_t tval = (int64_t)val * sf->x_scale_fp + off;
    return (int)ROUND_POWER_OF_TWO_SIGNED_64(tval, REF_SCALE_SHIFT - SCALE_EXTRA_BITS);
}

// Note: Expect val to be in q4 precision
static INLINE int32_t scaled_y(int32_t val, const ScaleFactors* sf) {
    const int32_t off  = (sf->y_scale_fp - (1 << REF_SCALE_SHIFT)) * (1 << (SUBPEL_BITS - 1));
    const int64_t tval = (int64_t)val * sf->y_scale_fp + off;
    return (int32_t)ROUND_POWER_OF_TWO_SIGNED_64(tval, REF_SCALE_SHIFT - SCALE_EXTRA_BITS);
}

// Note: Expect val to be in q4 precision
static int32_t unscaled_value(int32_t val, const ScaleFactors* sf) {
    (void)sf;
    return val << SCALE_EXTRA_BITS;
}

static int32_t get_fixed_point_scale_factor(int32_t other_size, int32_t this_size) {
    // Calculate scaling factor once for each reference frame
    // and use fixed point scaling factors in decoding and encoding routines.
    // Hardware implementations can calculate scale factor in device driver
    // and use multiplication and shifting on hardware instead of division.
    return ((other_size << REF_SCALE_SHIFT) + this_size / 2) / this_size;
}

// Given the fixed point scale, calculate coarse point scale.
static int32_t fixed_point_scale_to_coarse_point_scale(int32_t scale_fp) {
    return ROUND_POWER_OF_TWO(scale_fp, REF_SCALE_SHIFT - SCALE_SUBPEL_BITS);
}

void svt_av1_setup_scale_factors_for_frame(ScaleFactors* sf, int other_w, int other_h, int this_w, int this_h) {
    if (!valid_ref_frame_size(other_w, other_h, this_w, this_h)) {
        sf->x_scale_fp = REF_INVALID_SCALE;
        sf->y_scale_fp = REF_INVALID_SCALE;
        return;
    }

    sf->x_scale_fp = get_fixed_point_scale_factor(other_w, this_w);
    sf->y_scale_fp = get_fixed_point_scale_factor(other_h, this_h);

    sf->x_step_q4 = fixed_point_scale_to_coarse_point_scale(sf->x_scale_fp);
    sf->y_step_q4 = fixed_point_scale_to_coarse_point_scale(sf->y_scale_fp);

    if (av1_is_scaled(sf)) {
        sf->scale_value_x = scaled_x;
        sf->scale_value_y = scaled_y;
    } else {
        sf->scale_value_x = unscaled_value;
        sf->scale_value_y = unscaled_value;
    }
}

static INLINE int32_t has_scale(int32_t xs, int32_t ys) {
    return xs != SCALE_SUBPEL_SHIFTS || ys != SCALE_SUBPEL_SHIFTS;
}

static INLINE void revert_scale_extra_bits(SubpelParams* sp) {
    sp->subpel_x >>= SCALE_EXTRA_BITS;
    sp->subpel_y >>= SCALE_EXTRA_BITS;
    sp->xs >>= SCALE_EXTRA_BITS;
    sp->ys >>= SCALE_EXTRA_BITS;
    assert(sp->subpel_x < SUBPEL_SHIFTS);
    assert(sp->subpel_y < SUBPEL_SHIFTS);
    assert(sp->xs <= SUBPEL_SHIFTS);
    assert(sp->ys <= SUBPEL_SHIFTS);
}

DECLARE_ALIGNED(256, const InterpKernel, sub_pel_filters_8[SUBPEL_SHIFTS]) = {{0, 0, 0, 128, 0, 0, 0, 0},
                                                                              {0, 2, -6, 126, 8, -2, 0, 0},
                                                                              {0, 2, -10, 122, 18, -4, 0, 0},
                                                                              {0, 2, -12, 116, 28, -8, 2, 0},
                                                                              {0, 2, -14, 110, 38, -10, 2, 0},
                                                                              {0, 2, -14, 102, 48, -12, 2, 0},
                                                                              {0, 2, -16, 94, 58, -12, 2, 0},
                                                                              {0, 2, -14, 84, 66, -12, 2, 0},
                                                                              {0, 2, -14, 76, 76, -14, 2, 0},
                                                                              {0, 2, -12, 66, 84, -14, 2, 0},
                                                                              {0, 2, -12, 58, 94, -16, 2, 0},
                                                                              {0, 2, -12, 48, 102, -14, 2, 0},
                                                                              {0, 2, -10, 38, 110, -14, 2, 0},
                                                                              {0, 2, -8, 28, 116, -12, 2, 0},
                                                                              {0, 0, -4, 18, 122, -10, 2, 0},
                                                                              {0, 0, -2, 8, 126, -6, 2, 0}};
DECLARE_ALIGNED(256, const InterpKernel, sub_pel_filters_4[SUBPEL_SHIFTS]) = {{0, 0, 0, 128, 0, 0, 0, 0},
                                                                              {0, 0, -4, 126, 8, -2, 0, 0},
                                                                              {0, 0, -8, 122, 18, -4, 0, 0},
                                                                              {0, 0, -10, 116, 28, -6, 0, 0},
                                                                              {0, 0, -12, 110, 38, -8, 0, 0},
                                                                              {0, 0, -12, 102, 48, -10, 0, 0},
                                                                              {0, 0, -14, 94, 58, -10, 0, 0},
                                                                              {0, 0, -12, 84, 66, -10, 0, 0},
                                                                              {0, 0, -12, 76, 76, -12, 0, 0},
                                                                              {0, 0, -10, 66, 84, -12, 0, 0},
                                                                              {0, 0, -10, 58, 94, -14, 0, 0},
                                                                              {0, 0, -10, 48, 102, -12, 0, 0},
                                                                              {0, 0, -8, 38, 110, -12, 0, 0},
                                                                              {0, 0, -6, 28, 116, -10, 0, 0},
                                                                              {0, 0, -4, 18, 122, -8, 0, 0},
                                                                              {0, 0, -2, 8, 126, -4, 0, 0}};

#define MAX_FILTER_TAP 8

int svt_aom_get_relative_dist_enc(SeqHeader* seq_header, int ref_hint, int order_hint) {
    int diff, m;
    if (!seq_header->order_hint_info.enable_order_hint) {
        return 0;
    }
    diff = ref_hint - order_hint;
    m    = 1 << (seq_header->order_hint_info.order_hint_bits - 1);
    diff = (diff & (m - 1)) - (diff & m);
    return diff;
}

static const int quant_dist_weight[4][2]          = {{2, 3}, {2, 5}, {2, 7}, {1, MAX_FRAME_DISTANCE}};
static const int quant_dist_lookup_table[2][4][2] = {
    {{9, 7}, {11, 5}, {12, 4}, {13, 3}},
    {{7, 9}, {5, 11}, {4, 12}, {3, 13}},
};

void svt_av1_dist_wtd_comp_weight_assign(SeqHeader* seq_header, int cur_frame_index, int bck_frame_index,
                                         int fwd_frame_index, int compound_idx, int order_idx, int* fwd_offset,
                                         int* bck_offset, int* use_dist_wtd_comp_avg, int is_compound) {
    assert(fwd_offset != NULL && bck_offset != NULL);
    if (!is_compound || compound_idx) {
        *use_dist_wtd_comp_avg = 0;
        return;
    }

    *use_dist_wtd_comp_avg = 1;

    int d0 = clamp(
        abs(svt_aom_get_relative_dist_enc(seq_header, fwd_frame_index, cur_frame_index)), 0, MAX_FRAME_DISTANCE);
    int d1 = clamp(
        abs(svt_aom_get_relative_dist_enc(seq_header, cur_frame_index, bck_frame_index)), 0, MAX_FRAME_DISTANCE);

    const int order = d0 <= d1;

    if (d0 == 0 || d1 == 0) {
        *fwd_offset = quant_dist_lookup_table[order_idx][3][order];
        *bck_offset = quant_dist_lookup_table[order_idx][3][1 - order];
        return;
    }

    int i;
    for (i = 0; i < 3; ++i) {
        int c0    = quant_dist_weight[i][order];
        int c1    = quant_dist_weight[i][!order];
        int d0_c0 = d0 * c0;
        int d1_c1 = d1 * c1;
        if ((d0 > d1 && d0_c0 < d1_c1) || (d0 <= d1 && d0_c0 > d1_c1)) {
            break;
        }
    }

    *fwd_offset = quant_dist_lookup_table[order_idx][i][order];
    *bck_offset = quant_dist_lookup_table[order_idx][i][1 - order];
}

void svt_av1_convolve_2d_sr_c(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                              int32_t h, const InterpFilterParams* filter_params_x,
                              const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                              const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    int16_t       im_block[(MAX_SB_SIZE + MAX_FILTER_TAP - 1) * MAX_SB_SIZE];
    int32_t       im_h      = h + filter_params_y->taps - 1;
    int32_t       im_stride = w;
    const int32_t fo_vert   = filter_params_y->taps / 2 - 1;
    const int32_t fo_horiz  = filter_params_x->taps / 2 - 1;
    const int32_t bd        = 8;
    const int32_t bits      = FILTER_BITS * 2 - conv_params->round_0 - conv_params->round_1;

    // horizontal filter
    const uint8_t* src_horiz = src - fo_vert * src_stride;
    const int16_t* x_filter  = av1_get_interp_filter_subpel_kernel(*filter_params_x, subpel_x_q4 & SUBPEL_MASK);
    for (int32_t y = 0; y < im_h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t sum = (1 << (bd + FILTER_BITS - 1));
            for (int32_t k = 0; k < filter_params_x->taps; ++k) {
                sum += x_filter[k] * src_horiz[y * src_stride + x - fo_horiz + k];
            }
            assert(0 <= sum && sum < (1 << (bd + FILTER_BITS + 1)));
            im_block[y * im_stride + x] = (int16_t)ROUND_POWER_OF_TWO(sum, conv_params->round_0);
        }
    }

    // vertical filter
    int16_t*       src_vert    = im_block + fo_vert * im_stride;
    const int16_t* y_filter    = av1_get_interp_filter_subpel_kernel(*filter_params_y, subpel_y_q4 & SUBPEL_MASK);
    const int32_t  offset_bits = bd + 2 * FILTER_BITS - conv_params->round_0;
    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t sum = 1 << offset_bits;
            for (int32_t k = 0; k < filter_params_y->taps; ++k) {
                sum += y_filter[k] * src_vert[(y - fo_vert + k) * im_stride + x];
            }
            assert(0 <= sum && sum < (1 << (offset_bits + 2)));
            int16_t res             = (ConvBufType)(ROUND_POWER_OF_TWO(sum, conv_params->round_1) -
                                        ((1 << (offset_bits - conv_params->round_1)) +
                                         (1 << (offset_bits - conv_params->round_1 - 1))));
            dst[y * dst_stride + x] = (uint8_t)clip_pixel_highbd(ROUND_POWER_OF_TWO(res, bits), 8);
        }
    }
}

void svt_av1_convolve_y_sr_c(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                             int32_t h, const InterpFilterParams* filter_params_x,
                             const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                             const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    assert(filter_params_y != NULL);
    const int32_t fo_vert = filter_params_y->taps / 2 - 1;
    (void)filter_params_x;
    (void)subpel_x_q4;
    (void)conv_params;

    assert(conv_params->round_0 <= FILTER_BITS);
    assert(((conv_params->round_0 + conv_params->round_1) <= (FILTER_BITS + 1)) ||
           ((conv_params->round_0 + conv_params->round_1) == (2 * FILTER_BITS)));

    // vertical filter
    const int16_t* y_filter = av1_get_interp_filter_subpel_kernel(*filter_params_y, subpel_y_q4 & SUBPEL_MASK);

    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t res = 0;
            for (int32_t k = 0; k < filter_params_y->taps; ++k) {
                res += y_filter[k] * src[(y - fo_vert + k) * src_stride + x];
            }
            dst[y * dst_stride + x] = (uint8_t)clip_pixel_highbd(ROUND_POWER_OF_TWO(res, FILTER_BITS), 8);
        }
    }
}

void svt_av1_convolve_x_sr_c(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                             int32_t h, const InterpFilterParams* filter_params_x,
                             const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                             const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    const int32_t fo_horiz = filter_params_x->taps / 2 - 1;
    const int32_t bits     = FILTER_BITS - conv_params->round_0;
    (void)filter_params_y;
    (void)subpel_y_q4;
    (void)conv_params;

    assert(bits >= 0);
    assert((FILTER_BITS - conv_params->round_1) >= 0 ||
           ((conv_params->round_0 + conv_params->round_1) == 2 * FILTER_BITS));

    // horizontal filter
    const int16_t* x_filter = av1_get_interp_filter_subpel_kernel(*filter_params_x, subpel_x_q4 & SUBPEL_MASK);

    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t res = 0;
            for (int32_t k = 0; k < filter_params_x->taps; ++k) {
                res += x_filter[k] * src[y * src_stride + x - fo_horiz + k];
            }
            res                     = ROUND_POWER_OF_TWO(res, conv_params->round_0);
            dst[y * dst_stride + x] = (uint8_t)clip_pixel_highbd(ROUND_POWER_OF_TWO(res, bits), 8);
        }
    }
}

void svt_av1_convolve_2d_copy_sr_c(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                                   int32_t h, const InterpFilterParams* filter_params_x,
                                   const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                   const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    (void)filter_params_x;
    (void)filter_params_y;
    (void)subpel_x_q4;
    (void)subpel_y_q4;
    (void)conv_params;

    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            dst[y * dst_stride + x] = src[y * src_stride + x];
        }
    }
}

void svt_av1_convolve_2d_scale_c(const uint8_t* src, int src_stride, uint8_t* dst8, int dst8_stride, int w, int h,
                                 const InterpFilterParams* filter_params_x, const InterpFilterParams* filter_params_y,
                                 const int subpel_x_qn, const int x_step_qn, const int subpel_y_qn, const int y_step_qn,
                                 ConvolveParams* conv_params) {
    int16_t        im_block[(2 * MAX_SB_SIZE + MAX_FILTER_TAP) * MAX_SB_SIZE];
    int            im_h         = (((h - 1) * y_step_qn + subpel_y_qn) >> SCALE_SUBPEL_BITS) + filter_params_y->taps;
    CONV_BUF_TYPE* dst16        = conv_params->dst;
    const int      dst16_stride = conv_params->dst_stride;
    const int      bits         = FILTER_BITS * 2 - conv_params->round_0 - conv_params->round_1;
    assert(bits >= 0);
    int       im_stride = w;
    const int fo_vert   = filter_params_y->taps / 2 - 1;
    const int fo_horiz  = filter_params_x->taps / 2 - 1;
    const int bd        = 8;

    // horizontal filter
    const uint8_t* src_horiz = src - fo_vert * src_stride;
    for (int y = 0; y < im_h; ++y) {
        int x_qn = subpel_x_qn;
        for (int x = 0; x < w; ++x, x_qn += x_step_qn) {
            const uint8_t* const src_x        = &src_horiz[(x_qn >> SCALE_SUBPEL_BITS)];
            const int            x_filter_idx = (x_qn & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS;
            assert(x_filter_idx < SUBPEL_SHIFTS);
            const int16_t* x_filter = av1_get_interp_filter_subpel_kernel(*filter_params_x, x_filter_idx);
            int32_t        sum      = (1 << (bd + FILTER_BITS - 1));
            for (int k = 0; k < filter_params_x->taps; ++k) {
                sum += x_filter[k] * src_x[k - fo_horiz];
            }
            assert(0 <= sum && sum < (1 << (bd + FILTER_BITS + 1)));
            im_block[y * im_stride + x] = (int16_t)ROUND_POWER_OF_TWO(sum, conv_params->round_0);
        }
        src_horiz += src_stride;
    }

    // vertical filter
    int16_t*  src_vert    = im_block + fo_vert * im_stride;
    const int offset_bits = bd + 2 * FILTER_BITS - conv_params->round_0;
    for (int x = 0; x < w; ++x) {
        int y_qn = subpel_y_qn;
        for (int y = 0; y < h; ++y, y_qn += y_step_qn) {
            const int16_t* src_y        = &src_vert[(y_qn >> SCALE_SUBPEL_BITS) * im_stride];
            const int      y_filter_idx = (y_qn & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS;
            assert(y_filter_idx < SUBPEL_SHIFTS);
            const int16_t* y_filter = av1_get_interp_filter_subpel_kernel(*filter_params_y, y_filter_idx);
            int32_t        sum      = 1 << offset_bits;
            for (int k = 0; k < filter_params_y->taps; ++k) {
                sum += y_filter[k] * src_y[(k - fo_vert) * im_stride];
            }
            assert(0 <= sum && sum < (1 << (offset_bits + 2)));
            CONV_BUF_TYPE res = ROUND_POWER_OF_TWO(sum, conv_params->round_1);
            if (conv_params->is_compound) {
                if (conv_params->do_average) {
                    int32_t tmp = dst16[y * dst16_stride + x];
                    if (conv_params->use_dist_wtd_comp_avg) {
                        tmp = tmp * conv_params->fwd_offset + res * conv_params->bck_offset;
                        tmp = tmp >> DIST_PRECISION_BITS;
                    } else {
                        tmp += res;
                        tmp = tmp >> 1;
                    }
                    /* Subtract round offset and convolve round */
                    tmp = tmp -
                        ((1 << (offset_bits - conv_params->round_1)) + (1 << (offset_bits - conv_params->round_1 - 1)));
                    dst8[y * dst8_stride + x] = clip_pixel(ROUND_POWER_OF_TWO(tmp, bits));
                } else {
                    dst16[y * dst16_stride + x] = res;
                }
            } else {
                /* Subtract round offset and convolve round */
                int32_t tmp = res -
                    ((1 << (offset_bits - conv_params->round_1)) + (1 << (offset_bits - conv_params->round_1 - 1)));
                dst8[y * dst8_stride + x] = clip_pixel(ROUND_POWER_OF_TWO(tmp, bits));
            }
        }
        src_vert++;
    }
}

void svt_av1_jnt_convolve_2d_c(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride, int32_t w,
                               int32_t h, const InterpFilterParams* filter_params_x,
                               const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                               const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    ConvBufType*  dst        = conv_params->dst;
    int32_t       dst_stride = conv_params->dst_stride;
    int16_t       im_block[(MAX_SB_SIZE + MAX_FILTER_TAP - 1) * MAX_SB_SIZE];
    int32_t       im_h       = h + filter_params_y->taps - 1;
    int32_t       im_stride  = w;
    const int32_t fo_vert    = filter_params_y->taps / 2 - 1;
    const int32_t fo_horiz   = filter_params_x->taps / 2 - 1;
    const int32_t bd         = 8;
    const int32_t round_bits = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;

    // horizontal filter
    const uint8_t* src_horiz = src - fo_vert * src_stride;
    const int16_t* x_filter  = av1_get_interp_filter_subpel_kernel(*filter_params_x, subpel_x_q4 & SUBPEL_MASK);
    for (int32_t y = 0; y < im_h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t sum = (1 << (bd + FILTER_BITS - 1));
            for (int32_t k = 0; k < filter_params_x->taps; ++k) {
                sum += x_filter[k] * src_horiz[y * src_stride + x - fo_horiz + k];
            }
            assert(0 <= sum && sum < (1 << (bd + FILTER_BITS + 1)));
            im_block[y * im_stride + x] = (int16_t)ROUND_POWER_OF_TWO(sum, conv_params->round_0);
        }
    }

    // vertical filter
    int16_t*       src_vert    = im_block + fo_vert * im_stride;
    const int16_t* y_filter    = av1_get_interp_filter_subpel_kernel(*filter_params_y, subpel_y_q4 & SUBPEL_MASK);
    const int32_t  offset_bits = bd + 2 * FILTER_BITS - conv_params->round_0;
    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t sum = 1 << offset_bits;
            for (int32_t k = 0; k < filter_params_y->taps; ++k) {
                sum += y_filter[k] * src_vert[(y - fo_vert + k) * im_stride + x];
            }
            assert(0 <= sum && sum < (1 << (offset_bits + 2)));
            ConvBufType res = (ConvBufType)ROUND_POWER_OF_TWO(sum, conv_params->round_1);
            if (conv_params->do_average) {
                int32_t tmp = dst[y * dst_stride + x];
                if (conv_params->use_jnt_comp_avg) {
                    tmp = tmp * conv_params->fwd_offset + res * conv_params->bck_offset;
                    tmp = tmp >> DIST_PRECISION_BITS;
                } else {
                    tmp += res;
                    tmp = tmp >> 1;
                }
                tmp -= (1 << (offset_bits - conv_params->round_1)) + (1 << (offset_bits - conv_params->round_1 - 1));
                dst8[y * dst8_stride + x] = (uint8_t)clip_pixel_highbd(ROUND_POWER_OF_TWO(tmp, round_bits), 8);
            } else {
                dst[y * dst_stride + x] = res;
            }
        }
    }
}

void svt_av1_jnt_convolve_y_c(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride, int32_t w,
                              int32_t h, const InterpFilterParams* filter_params_x,
                              const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                              const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    ConvBufType*  dst          = conv_params->dst;
    int32_t       dst_stride   = conv_params->dst_stride;
    const int32_t fo_vert      = filter_params_y->taps / 2 - 1;
    const int32_t bits         = FILTER_BITS - conv_params->round_0;
    const int32_t bd           = 8;
    const int32_t offset_bits  = bd + 2 * FILTER_BITS - conv_params->round_0;
    const int32_t round_offset = (1 << (offset_bits - conv_params->round_1)) +
        (1 << (offset_bits - conv_params->round_1 - 1));
    const int32_t round_bits = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    (void)filter_params_x;
    (void)subpel_x_q4;

    // vertical filter
    const int16_t* y_filter = av1_get_interp_filter_subpel_kernel(*filter_params_y, subpel_y_q4 & SUBPEL_MASK);
    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t res = 0;
            for (int32_t k = 0; k < filter_params_y->taps; ++k) {
                res += y_filter[k] * src[(y - fo_vert + k) * src_stride + x];
            }
            res *= (1 << bits);
            res = ROUND_POWER_OF_TWO(res, conv_params->round_1) + round_offset;

            if (conv_params->do_average) {
                int32_t tmp = dst[y * dst_stride + x];
                if (conv_params->use_jnt_comp_avg) {
                    tmp = tmp * conv_params->fwd_offset + res * conv_params->bck_offset;
                    tmp = tmp >> DIST_PRECISION_BITS;
                } else {
                    tmp += res;
                    tmp = tmp >> 1;
                }
                tmp -= round_offset;
                dst8[y * dst8_stride + x] = (uint8_t)clip_pixel_highbd(ROUND_POWER_OF_TWO(tmp, round_bits), 8);
            } else {
                dst[y * dst_stride + x] = (ConvBufType)res;
            }
        }
    }
}

void svt_av1_jnt_convolve_x_c(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride, int32_t w,
                              int32_t h, const InterpFilterParams* filter_params_x,
                              const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                              const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    ConvBufType*  dst          = conv_params->dst;
    int32_t       dst_stride   = conv_params->dst_stride;
    const int32_t fo_horiz     = filter_params_x->taps / 2 - 1;
    const int32_t bits         = FILTER_BITS - conv_params->round_1;
    const int32_t bd           = 8;
    const int32_t offset_bits  = bd + 2 * FILTER_BITS - conv_params->round_0;
    const int32_t round_offset = (1 << (offset_bits - conv_params->round_1)) +
        (1 << (offset_bits - conv_params->round_1 - 1));
    const int32_t round_bits = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    (void)filter_params_y;
    (void)subpel_y_q4;

    // horizontal filter
    const int16_t* x_filter = av1_get_interp_filter_subpel_kernel(*filter_params_x, subpel_x_q4 & SUBPEL_MASK);
    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t res = 0;
            for (int32_t k = 0; k < filter_params_x->taps; ++k) {
                res += x_filter[k] * src[y * src_stride + x - fo_horiz + k];
            }
            res = (1 << bits) * ROUND_POWER_OF_TWO(res, conv_params->round_0);
            res += round_offset;

            if (conv_params->do_average) {
                int32_t tmp = dst[y * dst_stride + x];
                if (conv_params->use_jnt_comp_avg) {
                    tmp = tmp * conv_params->fwd_offset + res * conv_params->bck_offset;
                    tmp = tmp >> DIST_PRECISION_BITS;
                } else {
                    tmp += res;
                    tmp = tmp >> 1;
                }
                tmp -= round_offset;
                dst8[y * dst8_stride + x] = (uint8_t)clip_pixel_highbd(ROUND_POWER_OF_TWO(tmp, round_bits), 8);
            } else {
                dst[y * dst_stride + x] = (ConvBufType)res;
            }
        }
    }
}

void svt_av1_jnt_convolve_2d_copy_c(const uint8_t* src, int32_t src_stride, uint8_t* dst8, int32_t dst8_stride,
                                    int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                    const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                    const int32_t subpel_y_q4, ConvolveParams* conv_params) {
    ConvBufType*  dst          = conv_params->dst;
    int32_t       dst_stride   = conv_params->dst_stride;
    const int32_t bits         = FILTER_BITS * 2 - conv_params->round_1 - conv_params->round_0;
    const int32_t bd           = 8;
    const int32_t offset_bits  = bd + 2 * FILTER_BITS - conv_params->round_0;
    const int32_t round_offset = (1 << (offset_bits - conv_params->round_1)) +
        (1 << (offset_bits - conv_params->round_1 - 1));
    (void)filter_params_x;
    (void)filter_params_y;
    (void)subpel_x_q4;
    (void)subpel_y_q4;

    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            ConvBufType res = src[y * src_stride + x] << bits;
            res += (ConvBufType)round_offset;

            if (conv_params->do_average) {
                int32_t tmp = dst[y * dst_stride + x];
                if (conv_params->use_jnt_comp_avg) {
                    tmp = tmp * conv_params->fwd_offset + res * conv_params->bck_offset;
                    tmp = tmp >> DIST_PRECISION_BITS;
                } else {
                    tmp += res;
                    tmp = tmp >> 1;
                }
                tmp -= round_offset;
                dst8[y * dst8_stride + x] = (uint8_t)clip_pixel_highbd(ROUND_POWER_OF_TWO(tmp, bits), 8);
            } else {
                dst[y * dst_stride + x] = res;
            }
        }
    }
}

void svt_av1_highbd_convolve_2d_copy_sr_c(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                                          int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                          const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                          const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    (void)filter_params_x;
    (void)filter_params_y;
    (void)subpel_x_q4;
    (void)subpel_y_q4;
    (void)conv_params;
    (void)bd;

    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            dst[y * dst_stride + x] = src[y * src_stride + x];
        }
    }
}

void svt_av1_highbd_convolve_x_sr_c(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                                    int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                    const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                    const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    const int32_t fo_horiz = filter_params_x->taps / 2 - 1;
    const int32_t bits     = FILTER_BITS - conv_params->round_0;
    (void)filter_params_y;
    (void)subpel_y_q4;

    assert(bits >= 0);
    assert((FILTER_BITS - conv_params->round_1) >= 0 ||
           ((conv_params->round_0 + conv_params->round_1) == 2 * FILTER_BITS));

    // horizontal filter
    const int16_t* x_filter = av1_get_interp_filter_subpel_kernel(*filter_params_x, subpel_x_q4 & SUBPEL_MASK);
    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t res = 0;
            for (int32_t k = 0; k < filter_params_x->taps; ++k) {
                res += x_filter[k] * src[y * src_stride + x - fo_horiz + k];
            }
            res                     = ROUND_POWER_OF_TWO(res, conv_params->round_0);
            dst[y * dst_stride + x] = clip_pixel_highbd(ROUND_POWER_OF_TWO(res, bits), bd);
        }
    }
}

void svt_av1_highbd_convolve_y_sr_c(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                                    int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                    const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                    const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    assert(filter_params_y != NULL);
    const int32_t fo_vert = filter_params_y->taps / 2 - 1;
    (void)filter_params_x;
    (void)subpel_x_q4;
    (void)conv_params;

    assert(conv_params->round_0 <= FILTER_BITS);
    assert(((conv_params->round_0 + conv_params->round_1) <= (FILTER_BITS + 1)) ||
           ((conv_params->round_0 + conv_params->round_1) == (2 * FILTER_BITS)));
    // vertical filter
    const int16_t* y_filter = av1_get_interp_filter_subpel_kernel(*filter_params_y, subpel_y_q4 & SUBPEL_MASK);
    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t res = 0;
            for (int32_t k = 0; k < filter_params_y->taps; ++k) {
                res += y_filter[k] * src[(y - fo_vert + k) * src_stride + x];
            }
            dst[y * dst_stride + x] = clip_pixel_highbd(ROUND_POWER_OF_TWO(res, FILTER_BITS), bd);
        }
    }
}

void svt_av1_highbd_convolve_2d_sr_c(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                                     int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                     const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                     const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    int16_t       im_block[(MAX_SB_SIZE + MAX_FILTER_TAP - 1) * MAX_SB_SIZE];
    int32_t       im_h      = h + filter_params_y->taps - 1;
    int32_t       im_stride = w;
    const int32_t fo_vert   = filter_params_y->taps / 2 - 1;
    const int32_t fo_horiz  = filter_params_x->taps / 2 - 1;
    const int32_t bits      = FILTER_BITS * 2 - conv_params->round_0 - conv_params->round_1;
    assert(bits >= 0);

    // horizontal filter
    const uint16_t* src_horiz = src - fo_vert * src_stride;
    const int16_t*  x_filter  = av1_get_interp_filter_subpel_kernel(*filter_params_x, subpel_x_q4 & SUBPEL_MASK);
    for (int32_t y = 0; y < im_h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t sum = (1 << (bd + FILTER_BITS - 1));
            for (int32_t k = 0; k < filter_params_x->taps; ++k) {
                sum += x_filter[k] * src_horiz[y * src_stride + x - fo_horiz + k];
            }
            assert(0 <= sum && sum < (1 << (bd + FILTER_BITS + 1)));
            im_block[y * im_stride + x] = (ConvBufType)ROUND_POWER_OF_TWO(sum, conv_params->round_0);
        }
    }

    // vertical filter
    int16_t*       src_vert    = im_block + fo_vert * im_stride;
    const int16_t* y_filter    = av1_get_interp_filter_subpel_kernel(*filter_params_y, subpel_y_q4 & SUBPEL_MASK);
    const int32_t  offset_bits = bd + 2 * FILTER_BITS - conv_params->round_0;
    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t sum = 1 << offset_bits;
            for (int32_t k = 0; k < filter_params_y->taps; ++k) {
                sum += y_filter[k] * src_vert[(y - fo_vert + k) * im_stride + x];
            }
            assert(0 <= sum && sum < (1 << (offset_bits + 2)));
            int32_t res = ROUND_POWER_OF_TWO(sum, conv_params->round_1) -
                ((1 << (offset_bits - conv_params->round_1)) + (1 << (offset_bits - conv_params->round_1 - 1)));
            dst[y * dst_stride + x] = clip_pixel_highbd(ROUND_POWER_OF_TWO(res, bits), bd);
        }
    }
}

void svt_av1_highbd_convolve_2d_scale_c(const uint16_t* src, int src_stride, uint16_t* dst, int dst_stride, int w,
                                        int h, const InterpFilterParams* filter_params_x,
                                        const InterpFilterParams* filter_params_y, const int subpel_x_qn,
                                        const int x_step_qn, const int subpel_y_qn, const int y_step_qn,
                                        ConvolveParams* conv_params, int bd) {
    int16_t        im_block[(2 * MAX_SB_SIZE + MAX_FILTER_TAP) * MAX_SB_SIZE];
    int            im_h         = (((h - 1) * y_step_qn + subpel_y_qn) >> SCALE_SUBPEL_BITS) + filter_params_y->taps;
    int            im_stride    = w;
    const int      fo_vert      = filter_params_y->taps / 2 - 1;
    const int      fo_horiz     = filter_params_x->taps / 2 - 1;
    CONV_BUF_TYPE* dst16        = conv_params->dst;
    const int      dst16_stride = conv_params->dst_stride;
    const int      bits         = FILTER_BITS * 2 - conv_params->round_0 - conv_params->round_1;
    assert(bits >= 0);
    // horizontal filter
    const uint16_t* src_horiz = src - fo_vert * src_stride;
    for (int y = 0; y < im_h; ++y) {
        int x_qn = subpel_x_qn;
        for (int x = 0; x < w; ++x, x_qn += x_step_qn) {
            const uint16_t* const src_x        = &src_horiz[(x_qn >> SCALE_SUBPEL_BITS)];
            const int             x_filter_idx = (x_qn & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS;
            assert(x_filter_idx < SUBPEL_SHIFTS);
            const int16_t* x_filter = av1_get_interp_filter_subpel_kernel(*filter_params_x, x_filter_idx);
            int32_t        sum      = (1 << (bd + FILTER_BITS - 1));
            for (int k = 0; k < filter_params_x->taps; ++k) {
                sum += x_filter[k] * src_x[k - fo_horiz];
            }
            assert(0 <= sum && sum < (1 << (bd + FILTER_BITS + 1)));
            im_block[y * im_stride + x] = (int16_t)ROUND_POWER_OF_TWO(sum, conv_params->round_0);
        }
        src_horiz += src_stride;
    }

    // vertical filter
    int16_t*  src_vert    = im_block + fo_vert * im_stride;
    const int offset_bits = bd + 2 * FILTER_BITS - conv_params->round_0;
    for (int x = 0; x < w; ++x) {
        int y_qn = subpel_y_qn;
        for (int y = 0; y < h; ++y, y_qn += y_step_qn) {
            const int16_t* src_y        = &src_vert[(y_qn >> SCALE_SUBPEL_BITS) * im_stride];
            const int      y_filter_idx = (y_qn & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS;
            assert(y_filter_idx < SUBPEL_SHIFTS);
            const int16_t* y_filter = av1_get_interp_filter_subpel_kernel(*filter_params_y, y_filter_idx);
            int32_t        sum      = 1 << offset_bits;
            for (int k = 0; k < filter_params_y->taps; ++k) {
                sum += y_filter[k] * src_y[(k - fo_vert) * im_stride];
            }
            assert(0 <= sum && sum < (1 << (offset_bits + 2)));
            CONV_BUF_TYPE res = ROUND_POWER_OF_TWO(sum, conv_params->round_1);
            if (conv_params->is_compound) {
                if (conv_params->do_average) {
                    int32_t tmp = dst16[y * dst16_stride + x];
                    if (conv_params->use_dist_wtd_comp_avg) {
                        tmp = tmp * conv_params->fwd_offset + res * conv_params->bck_offset;
                        tmp = tmp >> DIST_PRECISION_BITS;
                    } else {
                        tmp += res;
                        tmp = tmp >> 1;
                    }
                    /* Subtract round offset and convolve round */
                    tmp = tmp -
                        ((1 << (offset_bits - conv_params->round_1)) + (1 << (offset_bits - conv_params->round_1 - 1)));
                    dst[y * dst_stride + x] = clip_pixel_highbd(ROUND_POWER_OF_TWO(tmp, bits), bd);
                } else {
                    dst16[y * dst16_stride + x] = res;
                }
            } else {
                /* Subtract round offset and convolve round */
                int32_t tmp = res -
                    ((1 << (offset_bits - conv_params->round_1)) + (1 << (offset_bits - conv_params->round_1 - 1)));
                dst[y * dst_stride + x] = clip_pixel_highbd(ROUND_POWER_OF_TWO(tmp, bits), bd);
            }
        }
        src_vert++;
    }
}

void svt_av1_highbd_jnt_convolve_x_c(const uint16_t* src, int32_t src_stride, uint16_t* dst16, int32_t dst16_stride,
                                     int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                     const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                     const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    ConvBufType*  dst          = conv_params->dst;
    int32_t       dst_stride   = conv_params->dst_stride;
    const int32_t fo_horiz     = filter_params_x->taps / 2 - 1;
    const int32_t bits         = FILTER_BITS - conv_params->round_1;
    const int32_t offset_bits  = bd + 2 * FILTER_BITS - conv_params->round_0;
    const int32_t round_offset = (1 << (offset_bits - conv_params->round_1)) +
        (1 << (offset_bits - conv_params->round_1 - 1));
    const int32_t round_bits = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    assert(round_bits >= 0);
    (void)filter_params_y;
    (void)subpel_y_q4;
    assert(bits >= 0);
    // horizontal filter
    const int16_t* x_filter = av1_get_interp_filter_subpel_kernel(*filter_params_x, subpel_x_q4 & SUBPEL_MASK);
    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t res = 0;
            for (int32_t k = 0; k < filter_params_x->taps; ++k) {
                res += x_filter[k] * src[y * src_stride + x - fo_horiz + k];
            }
            res = (1 << bits) * ROUND_POWER_OF_TWO(res, conv_params->round_0);
            res += round_offset;

            if (conv_params->do_average) {
                int32_t tmp = dst[y * dst_stride + x];
                if (conv_params->use_jnt_comp_avg) {
                    tmp = tmp * conv_params->fwd_offset + res * conv_params->bck_offset;
                    tmp = tmp >> DIST_PRECISION_BITS;
                } else {
                    tmp += res;
                    tmp = tmp >> 1;
                }
                tmp -= round_offset;
                dst16[y * dst16_stride + x] = clip_pixel_highbd(ROUND_POWER_OF_TWO(tmp, round_bits), bd);
            } else {
                dst[y * dst_stride + x] = (ConvBufType)res;
            }
        }
    }
}

void svt_av1_highbd_jnt_convolve_y_c(const uint16_t* src, int32_t src_stride, uint16_t* dst16, int32_t dst16_stride,
                                     int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                     const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                     const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    ConvBufType*  dst          = conv_params->dst;
    int32_t       dst_stride   = conv_params->dst_stride;
    const int32_t fo_vert      = filter_params_y->taps / 2 - 1;
    const int32_t bits         = FILTER_BITS - conv_params->round_0;
    const int32_t offset_bits  = bd + 2 * FILTER_BITS - conv_params->round_0;
    const int32_t round_offset = (1 << (offset_bits - conv_params->round_1)) +
        (1 << (offset_bits - conv_params->round_1 - 1));
    const int32_t round_bits = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    assert(round_bits >= 0);
    (void)filter_params_x;
    (void)subpel_x_q4;
    assert(bits >= 0);
    // vertical filter
    const int16_t* y_filter = av1_get_interp_filter_subpel_kernel(*filter_params_y, subpel_y_q4 & SUBPEL_MASK);
    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            int32_t res = 0;
            for (int32_t k = 0; k < filter_params_y->taps; ++k) {
                res += y_filter[k] * src[(y - fo_vert + k) * src_stride + x];
            }
            res *= (1 << bits);
            res = ROUND_POWER_OF_TWO(res, conv_params->round_1) + round_offset;

            if (conv_params->do_average) {
                int32_t tmp = dst[y * dst_stride + x];
                if (conv_params->use_jnt_comp_avg) {
                    tmp = tmp * conv_params->fwd_offset + res * conv_params->bck_offset;
                    tmp = tmp >> DIST_PRECISION_BITS;
                } else {
                    tmp += res;
                    tmp = tmp >> 1;
                }
                tmp -= round_offset;
                dst16[y * dst16_stride + x] = clip_pixel_highbd(ROUND_POWER_OF_TWO(tmp, round_bits), bd);
            } else {
                dst[y * dst_stride + x] = (ConvBufType)res;
            }
        }
    }
}

void svt_av1_highbd_jnt_convolve_2d_copy_c(const uint16_t* src, int32_t src_stride, uint16_t* dst16,
                                           int32_t dst16_stride, int32_t w, int32_t h,
                                           const InterpFilterParams* filter_params_x,
                                           const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                           const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd) {
    ConvBufType*  dst          = conv_params->dst;
    int32_t       dst_stride   = conv_params->dst_stride;
    const int32_t bits         = FILTER_BITS * 2 - conv_params->round_1 - conv_params->round_0;
    const int32_t offset_bits  = bd + 2 * FILTER_BITS - conv_params->round_0;
    const int32_t round_offset = (1 << (offset_bits - conv_params->round_1)) +
        (1 << (offset_bits - conv_params->round_1 - 1));
    assert(bits >= 0);
    (void)filter_params_x;
    (void)filter_params_y;
    (void)subpel_x_q4;
    (void)subpel_y_q4;

    for (int32_t y = 0; y < h; ++y) {
        for (int32_t x = 0; x < w; ++x) {
            ConvBufType res = src[y * src_stride + x] << bits;
            res += (ConvBufType)round_offset;
            if (conv_params->do_average) {
                int32_t tmp = dst[y * dst_stride + x];
                if (conv_params->use_jnt_comp_avg) {
                    tmp = tmp * conv_params->fwd_offset + res * conv_params->bck_offset;
                    tmp = tmp >> DIST_PRECISION_BITS;
                } else {
                    tmp += res;
                    tmp = tmp >> 1;
                }
                tmp -= round_offset;
                dst16[y * dst16_stride + x] = clip_pixel_highbd(ROUND_POWER_OF_TWO(tmp, bits), bd);
            } else {
                dst[y * dst_stride + x] = res;
            }
        }
    }
}

void svt_av1_highbd_jnt_convolve_2d_c(const uint16_t* src, int32_t src_stride, uint16_t* dst16, int32_t dst16_stride,
                                      int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                      const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                      const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd)

{
    int16_t       im_block[(MAX_SB_SIZE + MAX_FILTER_TAP - 1) * MAX_SB_SIZE];
    ConvBufType*  dst        = conv_params->dst;
    int32_t       dst_stride = conv_params->dst_stride;
    int32_t       im_h       = h + filter_params_y->taps - 1;
    int32_t       im_stride  = w;
    const int32_t fo_vert    = filter_params_y->taps / 2 - 1;
    const int32_t fo_horiz   = filter_params_x->taps / 2 - 1;

    const int32_t round_bits = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1;
    assert(round_bits >= 0);

    // horizontal filter
    const uint16_t* src_horiz = src - fo_vert * src_stride;
    const int16_t*  x_filter  = av1_get_interp_filter_subpel_kernel(*filter_params_x, subpel_x_q4 & SUBPEL_MASK);
    for (int y = 0; y < im_h; ++y) {
        for (int x = 0; x < w; ++x) {
            int32_t sum = (1 << (bd + FILTER_BITS - 1));
            for (int k = 0; k < filter_params_x->taps; ++k) {
                sum += x_filter[k] * src_horiz[y * src_stride + x - fo_horiz + k];
            }
            assert(0 <= sum && sum < (1 << (bd + FILTER_BITS + 1)));
            (void)bd;
            im_block[y * im_stride + x] = (int16_t)ROUND_POWER_OF_TWO(sum, conv_params->round_0);
        }
    }

    // vertical filter
    int16_t*       src_vert    = im_block + fo_vert * im_stride;
    const int32_t  offset_bits = bd + 2 * FILTER_BITS - conv_params->round_0;
    const int16_t* y_filter    = av1_get_interp_filter_subpel_kernel(*filter_params_y, subpel_y_q4 & SUBPEL_MASK);
    for (int y = 0; y < h; ++y) {
        for (int x = 0; x < w; ++x) {
            int32_t sum = 1 << offset_bits;
            for (int k = 0; k < filter_params_y->taps; ++k) {
                sum += y_filter[k] * src_vert[(y - fo_vert + k) * im_stride + x];
            }
            assert(0 <= sum && sum < (1 << (offset_bits + 2)));
            ConvBufType res = (ConvBufType)ROUND_POWER_OF_TWO(sum, conv_params->round_1);
            if (conv_params->do_average) {
                int32_t tmp = dst[y * dst_stride + x];
                if (conv_params->use_jnt_comp_avg) {
                    tmp = tmp * conv_params->fwd_offset + res * conv_params->bck_offset;
                    tmp = tmp >> DIST_PRECISION_BITS;
                } else {
                    tmp += res;
                    tmp = tmp >> 1;
                }
                tmp -= (1 << (offset_bits - conv_params->round_1)) + (1 << (offset_bits - conv_params->round_1 - 1));
                dst16[y * dst16_stride + x] = clip_pixel_highbd(ROUND_POWER_OF_TWO(tmp, round_bits), bd);
            } else {
                dst[y * dst_stride + x] = res;
            }
        }
    }
}

aom_highbd_convolve_fn_t svt_aom_convolveHbd[/*subX*/ 2][/*subY*/ 2][/*bi*/ 2];

void svt_aom_asm_set_convolve_hbd_asm_table(void) {
    svt_aom_convolveHbd[0][0][0] = svt_av1_highbd_convolve_2d_copy_sr;
    svt_aom_convolveHbd[0][0][1] = svt_av1_highbd_jnt_convolve_2d_copy;

    svt_aom_convolveHbd[0][1][0] = svt_av1_highbd_convolve_y_sr;
    svt_aom_convolveHbd[0][1][1] = svt_av1_highbd_jnt_convolve_y;

    svt_aom_convolveHbd[1][0][0] = svt_av1_highbd_convolve_x_sr;
    svt_aom_convolveHbd[1][0][1] = svt_av1_highbd_jnt_convolve_x;

    svt_aom_convolveHbd[1][1][0] = svt_av1_highbd_convolve_2d_sr;
    svt_aom_convolveHbd[1][1][1] = svt_av1_highbd_jnt_convolve_2d;
}

AomConvolveFn svt_aom_convolve[/*subX*/ 2][/*subY*/ 2][/*bi*/ 2];

void svt_aom_asm_set_convolve_asm_table(void) {
    svt_aom_convolve[0][0][0] = svt_av1_convolve_2d_copy_sr;
    svt_aom_convolve[0][0][1] = svt_av1_jnt_convolve_2d_copy;

    svt_aom_convolve[0][1][0] = svt_av1_convolve_y_sr;
    svt_aom_convolve[0][1][1] = svt_av1_jnt_convolve_y;

    svt_aom_convolve[1][0][0] = svt_av1_convolve_x_sr;
    svt_aom_convolve[1][0][1] = svt_av1_jnt_convolve_x;

    svt_aom_convolve[1][1][0] = svt_av1_convolve_2d_sr;
    svt_aom_convolve[1][1][1] = svt_av1_jnt_convolve_2d;
}

DECLARE_ALIGNED(256, const InterpKernel, sub_pel_filters_8sharp[SUBPEL_SHIFTS]) = {{0, 0, 0, 128, 0, 0, 0, 0},
                                                                                   {-2, 2, -6, 126, 8, -2, 2, 0},
                                                                                   {-2, 6, -12, 124, 16, -6, 4, -2},
                                                                                   {-2, 8, -18, 120, 26, -10, 6, -2},
                                                                                   {-4, 10, -22, 116, 38, -14, 6, -2},
                                                                                   {-4, 10, -22, 108, 48, -18, 8, -2},
                                                                                   {-4, 10, -24, 100, 60, -20, 8, -2},
                                                                                   {-4, 10, -24, 90, 70, -22, 10, -2},
                                                                                   {-4, 12, -24, 80, 80, -24, 12, -4},
                                                                                   {-2, 10, -22, 70, 90, -24, 10, -4},
                                                                                   {-2, 8, -20, 60, 100, -24, 10, -4},
                                                                                   {-2, 8, -18, 48, 108, -22, 10, -4},
                                                                                   {-2, 6, -14, 38, 116, -22, 10, -4},
                                                                                   {-2, 6, -10, 26, 120, -18, 8, -2},
                                                                                   {-2, 4, -6, 16, 124, -12, 6, -2},
                                                                                   {0, 2, -2, 8, 126, -6, 2, -2}};

DECLARE_ALIGNED(256, const InterpKernel, sub_pel_filters_8smooth[SUBPEL_SHIFTS]) = {{0, 0, 0, 128, 0, 0, 0, 0},
                                                                                    {0, 2, 28, 62, 34, 2, 0, 0},
                                                                                    {0, 0, 26, 62, 36, 4, 0, 0},
                                                                                    {0, 0, 22, 62, 40, 4, 0, 0},
                                                                                    {0, 0, 20, 60, 42, 6, 0, 0},
                                                                                    {0, 0, 18, 58, 44, 8, 0, 0},
                                                                                    {0, 0, 16, 56, 46, 10, 0, 0},
                                                                                    {0, -2, 16, 54, 48, 12, 0, 0},
                                                                                    {0, -2, 14, 52, 52, 14, -2, 0},
                                                                                    {0, 0, 12, 48, 54, 16, -2, 0},
                                                                                    {0, 0, 10, 46, 56, 16, 0, 0},
                                                                                    {0, 0, 8, 44, 58, 18, 0, 0},
                                                                                    {0, 0, 6, 42, 60, 20, 0, 0},
                                                                                    {0, 0, 4, 40, 62, 22, 0, 0},
                                                                                    {0, 0, 4, 36, 62, 26, 0, 0},
                                                                                    {0, 0, 2, 34, 62, 28, 2, 0}};
DECLARE_ALIGNED(256, const InterpKernel, bilinear_filters[SUBPEL_SHIFTS])        = {{0, 0, 0, 128, 0, 0, 0, 0},
                                                                                    {0, 0, 0, 120, 8, 0, 0, 0},
                                                                                    {0, 0, 0, 112, 16, 0, 0, 0},
                                                                                    {0, 0, 0, 104, 24, 0, 0, 0},
                                                                                    {0, 0, 0, 96, 32, 0, 0, 0},
                                                                                    {0, 0, 0, 88, 40, 0, 0, 0},
                                                                                    {0, 0, 0, 80, 48, 0, 0, 0},
                                                                                    {0, 0, 0, 72, 56, 0, 0, 0},
                                                                                    {0, 0, 0, 64, 64, 0, 0, 0},
                                                                                    {0, 0, 0, 56, 72, 0, 0, 0},
                                                                                    {0, 0, 0, 48, 80, 0, 0, 0},
                                                                                    {0, 0, 0, 40, 88, 0, 0, 0},
                                                                                    {0, 0, 0, 32, 96, 0, 0, 0},
                                                                                    {0, 0, 0, 24, 104, 0, 0, 0},
                                                                                    {0, 0, 0, 16, 112, 0, 0, 0},
                                                                                    {0, 0, 0, 8, 120, 0, 0, 0}};
DECLARE_ALIGNED(256, const InterpKernel, sub_pel_filters_4smooth[SUBPEL_SHIFTS]) = {{0, 0, 0, 128, 0, 0, 0, 0},
                                                                                    {0, 0, 30, 62, 34, 2, 0, 0},
                                                                                    {0, 0, 26, 62, 36, 4, 0, 0},
                                                                                    {0, 0, 22, 62, 40, 4, 0, 0},
                                                                                    {0, 0, 20, 60, 42, 6, 0, 0},
                                                                                    {0, 0, 18, 58, 44, 8, 0, 0},
                                                                                    {0, 0, 16, 56, 46, 10, 0, 0},
                                                                                    {0, 0, 14, 54, 48, 12, 0, 0},
                                                                                    {0, 0, 12, 52, 52, 12, 0, 0},
                                                                                    {0, 0, 12, 48, 54, 14, 0, 0},
                                                                                    {0, 0, 10, 46, 56, 16, 0, 0},
                                                                                    {0, 0, 8, 44, 58, 18, 0, 0},
                                                                                    {0, 0, 6, 42, 60, 20, 0, 0},
                                                                                    {0, 0, 4, 40, 62, 22, 0, 0},
                                                                                    {0, 0, 4, 36, 62, 26, 0, 0},
                                                                                    {0, 0, 2, 34, 62, 30, 0, 0}};
BlockSize svt_aom_scale_chroma_bsize(BlockSize bsize, int32_t subsampling_x, int32_t subsampling_y);

void convolve_2d_for_intrabc(const uint8_t* src, int src_stride, uint8_t* dst, int dst_stride, int w, int h,
                             int subpel_x_q4, int subpel_y_q4, ConvolveParams* conv_params) {
    const InterpFilterParams* filter_params_x = subpel_x_q4 ? &av1_interp_filter_params_list[BILINEAR] : NULL;
    const InterpFilterParams* filter_params_y = subpel_y_q4 ? &av1_interp_filter_params_list[BILINEAR] : NULL;
    if (subpel_x_q4 != 0 && subpel_y_q4 != 0) {
        svt_av1_convolve_2d_sr(src,
                               src_stride,
                               dst,
                               dst_stride,
                               w,
                               h,
                               (InterpFilterParams*)filter_params_x,
                               (InterpFilterParams*)filter_params_y,
                               8,
                               8,
                               conv_params);
    } else if (subpel_x_q4 != 0) {
        svt_av1_convolve_x_sr(src,
                              src_stride,
                              dst,
                              dst_stride,
                              w,
                              h,
                              (InterpFilterParams*)filter_params_x,
                              (InterpFilterParams*)filter_params_y,
                              8,
                              0,
                              conv_params);
    } else {
        svt_av1_convolve_y_sr(src,
                              src_stride,
                              dst,
                              dst_stride,
                              w,
                              h,
                              (InterpFilterParams*)filter_params_x,
                              (InterpFilterParams*)filter_params_y,
                              0,
                              8,
                              conv_params);
    }
}

void highbd_convolve_2d_for_intrabc(const uint16_t* src, int src_stride, uint16_t* dst, int dst_stride, int w, int h,
                                    int subpel_x_q4, int subpel_y_q4, ConvolveParams* conv_params, int bd) {
    const InterpFilterParams* filter_params_x = subpel_x_q4 ? &av1_interp_filter_params_list[BILINEAR] : NULL;
    const InterpFilterParams* filter_params_y = subpel_y_q4 ? &av1_interp_filter_params_list[BILINEAR] : NULL;
    if (subpel_x_q4 != 0 && subpel_y_q4 != 0) {
        svt_av1_highbd_convolve_2d_sr(
            src, src_stride, dst, dst_stride, w, h, filter_params_x, filter_params_y, 8, 8, conv_params, bd);
    } else if (subpel_x_q4 != 0) {
        svt_av1_highbd_convolve_x_sr(
            src, src_stride, dst, dst_stride, w, h, filter_params_x, filter_params_y, 8, 0, conv_params, bd);
    } else {
        svt_av1_highbd_convolve_y_sr(
            src, src_stride, dst, dst_stride, w, h, filter_params_x, filter_params_y, 0, 8, conv_params, bd);
    }
}

/*
*/
void svt_inter_predictor_light_pd0(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                                   int32_t h, SubpelParams* subpel_params, ConvolveParams* conv_params) {
    const int32_t is_scaled = has_scale(subpel_params->xs, subpel_params->ys);
    if (is_scaled) {
        InterpFilterParams filter_params_x, filter_params_y;
        av1_get_convolve_filter_params(
            av1_make_interp_filters(EIGHTTAP_REGULAR, EIGHTTAP_REGULAR), &filter_params_x, &filter_params_y, w, h);
        svt_av1_convolve_2d_scale(src,
                                  src_stride,
                                  dst,
                                  dst_stride,
                                  w,
                                  h,
                                  &filter_params_x,
                                  &filter_params_y,
                                  subpel_params->subpel_x,
                                  subpel_params->xs,
                                  subpel_params->subpel_y,
                                  subpel_params->ys,
                                  conv_params);
    } else {
        UNUSED(subpel_params);
        svt_aom_convolve[0][0][conv_params->is_compound](
            src, src_stride, dst, dst_stride, w, h, 0, 0, 0, 0, conv_params);
    }
}

void svt_inter_predictor_light_pd1(uint8_t* src, uint8_t* src_2b, int32_t src_stride, uint8_t* dst, int32_t dst_stride,
                                   int32_t w, int32_t h, InterpFilters interp_filters, SubpelParams* subpel_params,
                                   ConvolveParams* conv_params, int32_t bd) {
    InterpFilterParams filter_params_x, filter_params_y;
    av1_get_convolve_filter_params(interp_filters, &filter_params_x, &filter_params_y, w, h);
    const int32_t is_scaled = has_scale(subpel_params->xs, subpel_params->ys);

    if (bd > EB_EIGHT_BIT) {
        // for super-res, the reference frame block might be 2x than predictor in maximum
        // for reference scaling, it might be 4x since both width and height is scaled 2x
        // should pack enough buffer for scaled reference
        DECLARE_ALIGNED(16, uint16_t, src16[PACKED_BUFFER_SIZE * 4]);
        int32_t src_stride16;
        // pack the reference into temp 16bit buffer
        uint8_t  offset       = INTERPOLATION_OFFSET;
        uint32_t width_scale  = 1;
        uint32_t height_scale = 1;
        if (is_scaled) {
            width_scale  = subpel_params->xs != SCALE_SUBPEL_SHIFTS ? 2 : 1;
            height_scale = subpel_params->ys != SCALE_SUBPEL_SHIFTS ? 2 : 1;
        }
        // optimize stride from MAX_SB_SIZE to bwidth to minimum the block buffer size
        src_stride16 = w * width_scale + (offset << 1);
        // 16-byte align of src16
        if (src_stride16 % 8) {
            src_stride16 = ALIGN_POWER_OF_TWO(src_stride16, 3);
        }

        svt_aom_pack_block(src - offset - (offset * src_stride),
                           src_stride,
                           src_2b - offset - (offset * src_stride),
                           src_stride,
                           src16,
                           src_stride16,
                           w * width_scale + (offset << 1),
                           h * height_scale + (offset << 1));
        uint16_t* src_10b = src16 + offset + (offset * src_stride16);
        uint16_t* dst16   = (uint16_t*)dst;

        if (is_scaled) {
            svt_av1_highbd_convolve_2d_scale(src_10b,
                                             src_stride16,
                                             dst16,
                                             dst_stride,
                                             w,
                                             h,
                                             &filter_params_x,
                                             &filter_params_y,
                                             subpel_params->subpel_x,
                                             subpel_params->xs,
                                             subpel_params->subpel_y,
                                             subpel_params->ys,
                                             conv_params,
                                             bd);
        } else {
            SubpelParams sp = *subpel_params;
            revert_scale_extra_bits(&sp);
            svt_aom_convolveHbd[sp.subpel_x != 0][sp.subpel_y != 0][conv_params->is_compound](src_10b,
                                                                                              src_stride16,
                                                                                              dst16,
                                                                                              dst_stride,
                                                                                              w,
                                                                                              h,
                                                                                              &filter_params_x,
                                                                                              &filter_params_y,
                                                                                              sp.subpel_x,
                                                                                              sp.subpel_y,
                                                                                              conv_params,
                                                                                              bd);
        }
    } else {
        if (is_scaled) {
            svt_av1_convolve_2d_scale(src,
                                      src_stride,
                                      dst,
                                      dst_stride,
                                      w,
                                      h,
                                      &filter_params_x,
                                      &filter_params_y,
                                      subpel_params->subpel_x,
                                      subpel_params->xs,
                                      subpel_params->subpel_y,
                                      subpel_params->ys,
                                      conv_params);
        } else {
            SubpelParams sp = *subpel_params;
            revert_scale_extra_bits(&sp);
            svt_aom_convolve[sp.subpel_x != 0][sp.subpel_y != 0][conv_params->is_compound](src,
                                                                                           src_stride,
                                                                                           dst,
                                                                                           dst_stride,
                                                                                           w,
                                                                                           h,
                                                                                           &filter_params_x,
                                                                                           &filter_params_y,
                                                                                           sp.subpel_x,
                                                                                           sp.subpel_y,
                                                                                           conv_params);
        }
    }
}

void svt_inter_predictor(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride,
                         const SubpelParams* subpel_params, const ScaleFactors* sf, int32_t w, int32_t h,
                         ConvolveParams* conv_params, InterpFilters interp_filters, int32_t is_intrabc) {
    InterpFilterParams filter_params_x, filter_params_y;
    const int32_t      is_scaled = has_scale(subpel_params->xs, subpel_params->ys);

    av1_get_convolve_filter_params(interp_filters, &filter_params_x, &filter_params_y, w, h);

    assert(conv_params->do_average == 0 || conv_params->do_average == 1);
    assert(sf);
    UNUSED(sf);
    assert(IMPLIES(is_intrabc, !is_scaled));

    if (is_scaled) {
        if (is_intrabc && (subpel_params->subpel_x != 0 || subpel_params->subpel_y != 0)) {
            convolve_2d_for_intrabc(
                src, src_stride, dst, dst_stride, w, h, subpel_params->subpel_x, subpel_params->subpel_y, conv_params);
            return;
        }
        if (conv_params->is_compound) {
            assert(conv_params->dst != NULL);
        }
        svt_av1_convolve_2d_scale(src,
                                  src_stride,
                                  dst,
                                  dst_stride,
                                  w,
                                  h,
                                  &filter_params_x,
                                  &filter_params_y,
                                  subpel_params->subpel_x,
                                  subpel_params->xs,
                                  subpel_params->subpel_y,
                                  subpel_params->ys,
                                  conv_params);
    } else {
        SubpelParams sp = *subpel_params;
        revert_scale_extra_bits(&sp);

        if (is_intrabc && (sp.subpel_x != 0 || sp.subpel_y != 0)) {
            convolve_2d_for_intrabc(src, src_stride, dst, dst_stride, w, h, sp.subpel_x, sp.subpel_y, conv_params);
            return;
        }

        svt_aom_convolve[sp.subpel_x != 0][sp.subpel_y != 0][conv_params->is_compound](src,
                                                                                       src_stride,
                                                                                       dst,
                                                                                       dst_stride,
                                                                                       w,
                                                                                       h,
                                                                                       &filter_params_x,
                                                                                       &filter_params_y,
                                                                                       sp.subpel_x,
                                                                                       sp.subpel_y,
                                                                                       conv_params);
    }
}

void svt_highbd_inter_predictor(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                                const SubpelParams* subpel_params, const ScaleFactors* sf, int32_t w, int32_t h,
                                ConvolveParams* conv_params, InterpFilters interp_filters, int32_t is_intrabc,
                                int32_t bd) {
    InterpFilterParams filter_params_x, filter_params_y;
    const int32_t      is_scaled = has_scale(subpel_params->xs, subpel_params->ys);

    av1_get_convolve_filter_params(interp_filters, &filter_params_x, &filter_params_y, w, h);

    assert(conv_params->do_average == 0 || conv_params->do_average == 1);
    assert(sf);
    UNUSED(sf);
    assert(IMPLIES(is_intrabc, !is_scaled));

    if (is_scaled) {
        if (is_intrabc && (subpel_params->subpel_x != 0 || subpel_params->subpel_y != 0)) {
            highbd_convolve_2d_for_intrabc(src,
                                           src_stride,
                                           dst,
                                           dst_stride,
                                           w,
                                           h,
                                           subpel_params->subpel_x,
                                           subpel_params->subpel_y,
                                           conv_params,
                                           bd);
            return;
        }
        if (conv_params->is_compound) {
            assert(conv_params->dst != NULL);
        }
        svt_av1_highbd_convolve_2d_scale(src,
                                         src_stride,
                                         dst,
                                         dst_stride,
                                         w,
                                         h,
                                         &filter_params_x,
                                         &filter_params_y,
                                         subpel_params->subpel_x,
                                         subpel_params->xs,
                                         subpel_params->subpel_y,
                                         subpel_params->ys,
                                         conv_params,
                                         bd);
    } else {
        SubpelParams sp = *subpel_params;
        revert_scale_extra_bits(&sp);

        if (is_intrabc && (sp.subpel_x != 0 || sp.subpel_y != 0)) {
            highbd_convolve_2d_for_intrabc(
                src, src_stride, dst, dst_stride, w, h, sp.subpel_x, sp.subpel_y, conv_params, bd);
            return;
        }

        svt_aom_convolveHbd[sp.subpel_x != 0][sp.subpel_y != 0][conv_params->is_compound](src,
                                                                                          src_stride,
                                                                                          dst,
                                                                                          dst_stride,
                                                                                          w,
                                                                                          h,
                                                                                          &filter_params_x,
                                                                                          &filter_params_y,
                                                                                          sp.subpel_x,
                                                                                          sp.subpel_y,
                                                                                          conv_params,
                                                                                          bd);
    }
}

#define USE_PRECOMPUTED_WEDGE_SIGN 1
#define USE_PRECOMPUTED_WEDGE_MASK 1

#if USE_PRECOMPUTED_WEDGE_MASK
static const uint8_t wedge_primary_oblique_odd[MASK_PRIMARY_SIZE] = {
    0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,
    0,  0,  0,  0,  0,  0,  1,  2,  6,  18, 37, 53, 60, 63, 64, 64, 64, 64, 64, 64, 64, 64,
    64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
};
static const uint8_t wedge_primary_oblique_even[MASK_PRIMARY_SIZE] = {
    0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,
    0,  0,  0,  0,  0,  0,  1,  4,  11, 27, 46, 58, 62, 63, 64, 64, 64, 64, 64, 64, 64, 64,
    64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
};
static const uint8_t wedge_primary_vertical[MASK_PRIMARY_SIZE] = {
    0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,  0,
    0,  0,  0,  0,  0,  0,  0,  2,  7,  21, 43, 57, 62, 64, 64, 64, 64, 64, 64, 64, 64, 64,
    64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
};

DECLARE_ALIGNED(16, static uint8_t, wedge_signflip_lookup[BLOCK_SIZES_ALL][MAX_WEDGE_TYPES]) = {
    {
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    }, // not used
    {
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    }, // not used
    {
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    }, // not used
    {
        1,
        1,
        1,
        1,
        1,
        1,
        1,
        1,
        1,
        1,
        0,
        1,
        1,
        1,
        0,
        1,
    },
    {
        1,
        1,
        1,
        1,
        0,
        1,
        1,
        1,
        1,
        1,
        0,
        1,
        1,
        1,
        0,
        1,
    },
    {
        1,
        1,
        1,
        1,
        0,
        1,
        1,
        1,
        1,
        1,
        0,
        1,
        1,
        1,
        0,
        1,
    },
    {
        1,
        1,
        1,
        1,
        1,
        1,
        1,
        1,
        1,
        1,
        0,
        1,
        1,
        1,
        0,
        1,
    },
    {
        1,
        1,
        1,
        1,
        0,
        1,
        1,
        1,
        1,
        1,
        0,
        1,
        1,
        1,
        0,
        1,
    },
    {
        1,
        1,
        1,
        1,
        0,
        1,
        1,
        1,
        1,
        1,
        0,
        1,
        1,
        1,
        0,
        1,
    },
    {
        1,
        1,
        1,
        1,
        1,
        1,
        1,
        1,
        1,
        1,
        0,
        1,
        1,
        1,
        0,
        1,
    },
    {
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    }, // not used
    {
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    }, // not used
    {
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    }, // not used
    {
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    }, // not used
    {
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    }, // not used
    {
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    }, // not used
    {
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    }, // not used
    {
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    }, // not used
    {
        1,
        1,
        1,
        1,
        0,
        1,
        1,
        1,
        0,
        1,
        0,
        1,
        1,
        1,
        0,
        1,
    },
    {
        1,
        1,
        1,
        1,
        0,
        1,
        1,
        1,
        1,
        1,
        0,
        1,
        0,
        1,
        0,
        1,
    },
    {
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    }, // not used
    {
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    }, // not used
};

static const WedgeCodeType wedge_codebook_16_hgtw[16] = {
    {WEDGE_OBLIQUE27, 4, 4},
    {WEDGE_OBLIQUE63, 4, 4},
    {WEDGE_OBLIQUE117, 4, 4},
    {WEDGE_OBLIQUE153, 4, 4},
    {WEDGE_HORIZONTAL, 4, 2},
    {WEDGE_HORIZONTAL, 4, 4},
    {WEDGE_HORIZONTAL, 4, 6},
    {WEDGE_VERTICAL, 4, 4},
    {WEDGE_OBLIQUE27, 4, 2},
    {WEDGE_OBLIQUE27, 4, 6},
    {WEDGE_OBLIQUE153, 4, 2},
    {WEDGE_OBLIQUE153, 4, 6},
    {WEDGE_OBLIQUE63, 2, 4},
    {WEDGE_OBLIQUE63, 6, 4},
    {WEDGE_OBLIQUE117, 2, 4},
    {WEDGE_OBLIQUE117, 6, 4},
};

static const WedgeCodeType wedge_codebook_16_hltw[16] = {
    {WEDGE_OBLIQUE27, 4, 4},
    {WEDGE_OBLIQUE63, 4, 4},
    {WEDGE_OBLIQUE117, 4, 4},
    {WEDGE_OBLIQUE153, 4, 4},
    {WEDGE_VERTICAL, 2, 4},
    {WEDGE_VERTICAL, 4, 4},
    {WEDGE_VERTICAL, 6, 4},
    {WEDGE_HORIZONTAL, 4, 4},
    {WEDGE_OBLIQUE27, 4, 2},
    {WEDGE_OBLIQUE27, 4, 6},
    {WEDGE_OBLIQUE153, 4, 2},
    {WEDGE_OBLIQUE153, 4, 6},
    {WEDGE_OBLIQUE63, 2, 4},
    {WEDGE_OBLIQUE63, 6, 4},
    {WEDGE_OBLIQUE117, 2, 4},
    {WEDGE_OBLIQUE117, 6, 4},
};

static const WedgeCodeType wedge_codebook_16_heqw[16] = {
    {WEDGE_OBLIQUE27, 4, 4},
    {WEDGE_OBLIQUE63, 4, 4},
    {WEDGE_OBLIQUE117, 4, 4},
    {WEDGE_OBLIQUE153, 4, 4},
    {WEDGE_HORIZONTAL, 4, 2},
    {WEDGE_HORIZONTAL, 4, 6},
    {WEDGE_VERTICAL, 2, 4},
    {WEDGE_VERTICAL, 6, 4},
    {WEDGE_OBLIQUE27, 4, 2},
    {WEDGE_OBLIQUE27, 4, 6},
    {WEDGE_OBLIQUE153, 4, 2},
    {WEDGE_OBLIQUE153, 4, 6},
    {WEDGE_OBLIQUE63, 2, 4},
    {WEDGE_OBLIQUE63, 6, 4},
    {WEDGE_OBLIQUE117, 2, 4},
    {WEDGE_OBLIQUE117, 6, 4},
};

static const WedgeParamsType wedge_params_lookup[BLOCK_SIZES_ALL] = {
    {0, NULL, NULL, NULL},
    {0, NULL, NULL, NULL},
    {0, NULL, NULL, NULL},
    {4, wedge_codebook_16_heqw, wedge_signflip_lookup[BLOCK_8X8], wedge_masks[BLOCK_8X8]},
    {4, wedge_codebook_16_hgtw, wedge_signflip_lookup[BLOCK_8X16], wedge_masks[BLOCK_8X16]},
    {4, wedge_codebook_16_hltw, wedge_signflip_lookup[BLOCK_16X8], wedge_masks[BLOCK_16X8]},
    {4, wedge_codebook_16_heqw, wedge_signflip_lookup[BLOCK_16X16], wedge_masks[BLOCK_16X16]},
    {4, wedge_codebook_16_hgtw, wedge_signflip_lookup[BLOCK_16X32], wedge_masks[BLOCK_16X32]},
    {4, wedge_codebook_16_hltw, wedge_signflip_lookup[BLOCK_32X16], wedge_masks[BLOCK_32X16]},
    {4, wedge_codebook_16_heqw, wedge_signflip_lookup[BLOCK_32X32], wedge_masks[BLOCK_32X32]},
    {0, NULL, NULL, NULL},
    {0, NULL, NULL, NULL},
    {0, NULL, NULL, NULL},
    {0, NULL, NULL, NULL},
    {0, NULL, NULL, NULL},
    {0, NULL, NULL, NULL},
    {0, NULL, NULL, NULL},
    {0, NULL, NULL, NULL},
    {4, wedge_codebook_16_hgtw, wedge_signflip_lookup[BLOCK_8X32], wedge_masks[BLOCK_8X32]},
    {4, wedge_codebook_16_hltw, wedge_signflip_lookup[BLOCK_32X8], wedge_masks[BLOCK_32X8]},
    {0, NULL, NULL, NULL},
    {0, NULL, NULL, NULL},
};

int svt_aom_is_interintra_wedge_used(BlockSize bsize) {
    return wedge_params_lookup[bsize].bits > 0;
}

int32_t svt_aom_get_wedge_bits_lookup(BlockSize bsize) {
    return wedge_params_lookup[bsize].bits;
}

const uint8_t* svt_aom_get_contiguous_soft_mask(int wedge_index, int wedge_sign, BlockSize bsize) {
    return wedge_params_lookup[bsize].masks[wedge_sign][wedge_index];
}

static void aom_convolve_copy_c(const uint8_t* src, ptrdiff_t src_stride, uint8_t* dst, ptrdiff_t dst_stride,
                                const int16_t* filter_x, int filter_x_stride, const int16_t* filter_y,
                                int filter_y_stride, int w, int h) {
    (void)filter_x;
    (void)filter_x_stride;
    (void)filter_y;
    (void)filter_y_stride;

    for (int r = h; r > 0; --r) {
        svt_memcpy(dst, src, w);
        src += src_stride;
        dst += dst_stride;
    }
}

static void shift_copy(const uint8_t* src, uint8_t* dst, int shift, int width) {
    if (shift >= 0) {
        svt_memcpy(dst + shift, src, width - shift);
        memset(dst, src[0], shift);
    } else {
        shift = -shift;
        svt_memcpy(dst, src + shift, width - shift);
        memset(dst + width - shift, src[width - 1], shift);
    }
}

int svt_aom_get_wedge_params_bits(BlockSize bsize) {
    return wedge_params_lookup[bsize].bits;
}

#endif // USE_PRECOMPUTED_WEDGE_MASK

// [negative][direction]
DECLARE_ALIGNED(16, static uint8_t, wedge_mask_obl[2][WEDGE_DIRECTIONS][MASK_PRIMARY_SIZE * MASK_PRIMARY_SIZE]);

// 4 * MAX_WEDGE_SQUARE is an easy to compute and fairly tight upper bound
// on the sum of all mask sizes up to an including MAX_WEDGE_SQUARE.
DECLARE_ALIGNED(16, static uint8_t, wedge_mask_buf[2 * MAX_WEDGE_TYPES * 4 * MAX_WEDGE_SQUARE]);

static void init_wedge_primary_masks() {
    const int w      = MASK_PRIMARY_SIZE;
    const int h      = MASK_PRIMARY_SIZE;
    const int stride = MASK_PRIMARY_STRIDE;
    // Note: index [0] stores the primary, and [1] its complement.
#if USE_PRECOMPUTED_WEDGE_MASK
    // Generate prototype by shifting the primary
    int shift = h / 4;
    for (int i = 0; i < h; i += 2) {
        shift_copy(
            wedge_primary_oblique_even, &wedge_mask_obl[0][WEDGE_OBLIQUE63][i * stride], shift, MASK_PRIMARY_SIZE);
        shift--;
        shift_copy(
            wedge_primary_oblique_odd, &wedge_mask_obl[0][WEDGE_OBLIQUE63][(i + 1) * stride], shift, MASK_PRIMARY_SIZE);
        svt_memcpy(&wedge_mask_obl[0][WEDGE_VERTICAL][i * stride],
                   wedge_primary_vertical,
                   MASK_PRIMARY_SIZE * sizeof(wedge_primary_vertical[0]));
        svt_memcpy(&wedge_mask_obl[0][WEDGE_VERTICAL][(i + 1) * stride],
                   wedge_primary_vertical,
                   MASK_PRIMARY_SIZE * sizeof(wedge_primary_vertical[0]));
    }
#else
    static const double smoother_param = 2.85;
    const int           a[2]           = {2, 1};
    const double        asqrt          = sqrt(a[0] * a[0] + a[1] * a[1]);
    for (int i = 0; i < h; i++) {
        for (int j = 0; j < w; ++j) {
            int       x                                        = (2 * j + 1 - w);
            int       y                                        = (2 * i + 1 - h);
            double    d                                        = (a[0] * x + a[1] * y) / asqrt;
            const int msk                                      = (int)rint((1.0 + tanh(d / smoother_param)) * 32);
            wedge_mask_obl[0][WEDGE_OBLIQUE63][i * stride + j] = msk;
            const int mskx                                     = (int)rint((1.0 + tanh(x / smoother_param)) * 32);
            wedge_mask_obl[0][WEDGE_VERTICAL][i * stride + j]  = mskx;
        }
    }
#endif // USE_PRECOMPUTED_WEDGE_MASK
    for (int i = 0; i < h; ++i) {
        for (int j = 0; j < w; ++j) {
            const int msk                                      = wedge_mask_obl[0][WEDGE_OBLIQUE63][i * stride + j];
            wedge_mask_obl[0][WEDGE_OBLIQUE27][j * stride + i] = msk;
            wedge_mask_obl[0][WEDGE_OBLIQUE117][i * stride + w - 1 - j] =
                wedge_mask_obl[0][WEDGE_OBLIQUE153][(w - 1 - j) * stride + i] = (1 << WEDGE_WEIGHT_BITS) - msk;
            wedge_mask_obl[1][WEDGE_OBLIQUE63][i * stride + j] = wedge_mask_obl[1][WEDGE_OBLIQUE27][j * stride + i] =
                (1 << WEDGE_WEIGHT_BITS) - msk;
            wedge_mask_obl[1][WEDGE_OBLIQUE117][i * stride + w - 1 - j] =
                wedge_mask_obl[1][WEDGE_OBLIQUE153][(w - 1 - j) * stride + i] = msk;
            const int mskx                                      = wedge_mask_obl[0][WEDGE_VERTICAL][i * stride + j];
            wedge_mask_obl[0][WEDGE_HORIZONTAL][j * stride + i] = mskx;
            wedge_mask_obl[1][WEDGE_VERTICAL][i * stride + j]   = wedge_mask_obl[1][WEDGE_HORIZONTAL][j * stride + i] =
                (1 << WEDGE_WEIGHT_BITS) - mskx;
        }
    }
}

#if !USE_PRECOMPUTED_WEDGE_SIGN
// If the signs for the wedges for various BLOCK_SIZES are
// inconsistent flip the sign flag. Do it only once for every
// wedge codebook.
static void init_wedge_signs() {
    memset(wedge_signflip_lookup, 0, sizeof(wedge_signflip_lookup));
    for (BLOCK_SIZE bsize = BLOCK_4X4; bsize < BLOCK_SIZES_ALL; ++bsize) {
        const int               bw           = block_size_wide[bsize];
        const int               bh           = block_size_high[bsize];
        const wedge_params_type wedge_params = wedge_params_lookup[bsize];
        const int               wbits        = wedge_params.bits;
        const int               wtypes       = 1 << wbits;

        if (wbits) {
            for (int w = 0; w < wtypes; ++w) {
                // Get the mask primary, i.e. index [0]
                const uint8_t* mask = get_wedge_mask_inplace(w, 0, bsize);
                int            avg  = 0;
                for (int i = 0; i < bw; ++i) {
                    avg += mask[i];
                }
                for (int i = 1; i < bh; ++i) {
                    avg += mask[i * MASK_PRIMARY_STRIDE];
                }
                avg = (avg + (bw + bh - 1) / 2) / (bw + bh - 1);
                // Default sign of this wedge is 1 if the average < 32, 0 otherwise.
                // If default sign is 1:
                //   If sign requested is 0, we need to flip the sign and return
                //   the complement i.e. index [1] instead. If sign requested is 1
                //   we need to flip the sign and return index [0] instead.
                // If default sign is 0:
                //   If sign requested is 0, we need to return index [0] the primary
                //   if sign requested is 1, we need to return the complement index [1]
                //   instead.
                wedge_params.signflip[w] = (avg < 32);
            }
        }
    }
}
#endif // !USE_PRECOMPUTED_WEDGE_SIGN

static const uint8_t* get_wedge_mask_inplace(int wedge_index, int neg, BlockSize bsize) {
    const int bh = block_size_high[bsize];
    const int bw = block_size_wide[bsize];

    assert(wedge_index >= 0 && wedge_index < (1 << svt_aom_get_wedge_bits_lookup(bsize)));
    const WedgeCodeType* a = wedge_params_lookup[bsize].codebook + wedge_index;
    int                  woff, hoff;
    const uint8_t        wsignflip = wedge_params_lookup[bsize].signflip[wedge_index];

    woff = (a->x_offset * bw) >> 3;
    hoff = (a->y_offset * bh) >> 3;
    return wedge_mask_obl[neg ^ wsignflip][a->direction] + MASK_PRIMARY_STRIDE * (MASK_PRIMARY_SIZE / 2 - hoff) +
        MASK_PRIMARY_SIZE / 2 - woff;
}

static void init_wedge_masks() {
    uint8_t* dst = wedge_mask_buf;
    memset(wedge_masks, 0, sizeof(wedge_masks));
    for (BlockSize bsize = BLOCK_4X4; bsize < BLOCK_SIZES_ALL; ++bsize) {
        const int              bw           = block_size_wide[bsize];
        const int              bh           = block_size_high[bsize];
        const WedgeParamsType* wedge_params = &wedge_params_lookup[bsize];
        const int              wbits        = wedge_params->bits;
        const int              wtypes       = 1 << wbits;
        if (wbits == 0) {
            continue;
        }
        for (int w = 0; w < wtypes; ++w) {
            const uint8_t* mask;
            mask = get_wedge_mask_inplace(w, 0, bsize);
            aom_convolve_copy_c(mask, MASK_PRIMARY_STRIDE, dst, bw, NULL, 0, NULL, 0, bw, bh);
            wedge_params->masks[0][w] = dst;
            dst += bw * bh;

            mask = get_wedge_mask_inplace(w, 1, bsize);
            aom_convolve_copy_c(mask, MASK_PRIMARY_STRIDE, dst, bw, NULL, 0, NULL, 0, bw, bh);
            wedge_params->masks[1][w] = dst;
            dst += bw * bh;
        }
        assert(sizeof(wedge_mask_buf) >= (size_t)(dst - wedge_mask_buf));
    }
}

// Equation of line: f(x, y) = a[0]*(x - a[2]*w/8) + a[1]*(y - a[3]*h/8) = 0
void svt_av1_init_wedge_masks(void) {
    init_wedge_primary_masks();
#if !USE_PRECOMPUTED_WEDGE_SIGN
    init_wedge_signs();
#endif // !USE_PRECOMPUTED_WEDGE_SIGN
    init_wedge_masks();
}

int svt_aom_is_masked_compound_type(COMPOUND_TYPE type);

/* clang-format off */
static const uint8_t ii_weights1d[MAX_SB_SIZE] = {
    60, 58, 56, 54, 52, 50, 48, 47, 45, 44, 42, 41, 39, 38, 37, 35, 34, 33, 32,
    31, 30, 29, 28, 27, 26, 25, 24, 23, 22, 22, 21, 20, 19, 19, 18, 18, 17, 16,
    16, 15, 15, 14, 14, 13, 13, 12, 12, 12, 11, 11, 10, 10, 10,  9,  9,  9,  8,
    8,  8,  8,  7,  7,  7,  7,  6,  6,  6,  6,  6,  5,  5,  5,  5,  5,  4,  4,
    4,  4,  4,  4,  4,  4,  3,  3,  3,  3,  3,  3,  3,  3,  3,  2,  2,  2,  2,
    2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  2,  1,  1,  1,  1,  1,  1,  1,  1,
    1,  1,  1,  1,  1,  1,  1,  1,  1,  1,  1,  1,  1,  1
};
static const uint8_t ii_size_scales[BLOCK_SIZES_ALL] = {
    32, 16, 16, 16, 8, 8, 8, 4,
    4,  4,  2,  2,  2, 1, 1, 1,
    8,  8,  4,  4,  2, 2
};
/* clang-format on */

static void build_smooth_interintra_mask(uint8_t* mask, int stride, BlockSize plane_bsize, InterIntraMode mode) {
    const int bw         = block_size_wide[plane_bsize];
    const int bh         = block_size_high[plane_bsize];
    const int size_scale = ii_size_scales[plane_bsize];

    switch (mode) {
    case II_V_PRED:
        for (int i = 0; i < bh; ++i) {
            memset(mask, ii_weights1d[i * size_scale], bw * sizeof(mask[0]));
            mask += stride;
        }
        break;

    case II_H_PRED:
        for (int i = 0; i < bh; ++i) {
            for (int j = 0; j < bw; ++j) {
                mask[j] = ii_weights1d[j * size_scale];
            }
            mask += stride;
        }
        break;

    case II_SMOOTH_PRED:
        for (int i = 0; i < bh; ++i) {
            for (int j = 0; j < bw; ++j) {
                mask[j] = ii_weights1d[(i < j ? i : j) * size_scale];
            }
            mask += stride;
        }
        break;

    case II_DC_PRED:
    default:
        for (int i = 0; i < bh; ++i) {
            memset(mask, 32, bw * sizeof(mask[0]));
            mask += stride;
        }
        break;
    }
}

// ii_masks stores the actual masks. We use smooth_ii_masks to access ii_masks so that we can index the array
// directly with the bsize (BlockSize that would be passed when doing the prediction) without using the extra memory
// to store empty, unused masks for the BLOCK_SIZES that don't allow inter-intra
static uint8_t  ii_masks[BLOCK_32X32 - BLOCK_4X4 + 1][INTERINTRA_MODES][MAX_INTERINTRA_SB_SQUARE];
static uint8_t* smooth_ii_masks[BLOCK_SIZES_ALL][INTERINTRA_MODES];

// Initialize the masks used for inter-intra compound blending. Inter-intra is allowed for 8x8-32x32 blocks, but
// masks must be generated down to 4x4 because of chroma. The stride of each mask is the block width.
void init_ii_masks(void) {
    memset(smooth_ii_masks, 0 /*NULL*/, sizeof(smooth_ii_masks));
    for (BlockSize bsize = BLOCK_4X4; bsize <= BLOCK_32X32; ++bsize) {
        const int bw = block_size_wide[bsize];
        for (InterIntraMode ii_mode = II_DC_PRED; ii_mode < INTERINTRA_MODES; ii_mode++) {
            build_smooth_interintra_mask(ii_masks[bsize - BLOCK_4X4][ii_mode], bw, bsize, ii_mode);
            smooth_ii_masks[bsize][ii_mode] = ii_masks[bsize - BLOCK_4X4][ii_mode];
        }
    }
}

// mask stride is block width
static uint8_t* get_ii_mask(BlockSize bsize, InterIntraMode ii_mode) {
    return smooth_ii_masks[bsize][ii_mode];
}

void svt_aom_combine_interintra_highbd(InterIntraMode mode, uint8_t use_wedge_interintra, uint8_t wedge_index,
                                       uint8_t wedge_sign, BlockSize bsize, BlockSize plane_bsize, uint8_t* comppred8,
                                       int compstride, const uint8_t* interpred8, int interstride,
                                       const uint8_t* intrapred8, int intrastride, int bd) {
    const int bw = block_size_wide[plane_bsize];
    const int bh = block_size_high[plane_bsize];

    if (use_wedge_interintra) {
        if (svt_aom_is_interintra_wedge_used(bsize)) {
            const uint8_t* mask = svt_aom_get_contiguous_soft_mask(wedge_index, wedge_sign, bsize);
            const int      subh = 2 * mi_size_high[bsize] == bh;
            const int      subw = 2 * mi_size_wide[bsize] == bw;
            svt_aom_highbd_blend_a64_mask(comppred8,
                                          compstride,
                                          intrapred8,
                                          intrastride,
                                          interpred8,
                                          interstride,
                                          mask,
                                          block_size_wide[bsize],
                                          bw,
                                          bh,
                                          subw,
                                          subh,
                                          bd);
        }
        return;
    }

    uint8_t* mask = get_ii_mask(plane_bsize, mode);
    svt_aom_highbd_blend_a64_mask(
        comppred8, compstride, intrapred8, intrastride, interpred8, interstride, mask, bw, bw, bh, 0, 0, bd);
}

static const uint8_t* av1_get_compound_type_mask(const InterInterCompoundData* const comp_data, uint8_t* seg_mask,
                                                 BlockSize bsize) {
    assert(svt_aom_is_masked_compound_type(comp_data->type));
    (void)bsize;
    switch (comp_data->type) {
    case COMPOUND_WEDGE:
        return svt_aom_get_contiguous_soft_mask(comp_data->wedge_index, comp_data->wedge_sign, bsize);
    case COMPOUND_DIFFWTD:
        return seg_mask;
    default:
        assert(0);
        return NULL;
    }
}

void svt_aom_build_masked_compound_no_round(uint8_t* dst, int dst_stride, const CONV_BUF_TYPE* src0, int src0_stride,
                                            const CONV_BUF_TYPE* src1, int src1_stride,
                                            const InterInterCompoundData* const comp_data, uint8_t* seg_mask,
                                            BlockSize bsize, int h, int w, ConvolveParams* conv_params,
                                            uint8_t bit_depth, bool is_16bit) {
    // Derive subsampling from h and w passed in. May be refactored to
    // pass in subsampling factors directly.
    const int      subh = (2 << mi_size_high_log2[bsize]) == h;
    const int      subw = (2 << mi_size_wide_log2[bsize]) == w;
    const uint8_t* mask = av1_get_compound_type_mask(comp_data, seg_mask, bsize);

    if (is_16bit) {
        svt_aom_highbd_blend_a64_d16_mask(dst,
                                          dst_stride,
                                          src0,
                                          src0_stride,
                                          src1,
                                          src1_stride,
                                          mask,
                                          block_size_wide[bsize],
                                          w,
                                          h,
                                          subw,
                                          subh,
                                          conv_params,
                                          bit_depth);
    } else {
        svt_aom_lowbd_blend_a64_d16_mask(dst,
                                         dst_stride,
                                         src0,
                                         src0_stride,
                                         src1,
                                         src1_stride,
                                         mask,
                                         block_size_wide[bsize],
                                         w,
                                         h,
                                         subw,
                                         subh,
                                         conv_params);
    }
}

void svt_aom_find_ref_dv(Mv* ref_dv, const TileInfo* const tile, int mib_size, int mi_row, int mi_col) {
    (void)mi_col;
    if (mi_row - mib_size < tile->mi_row_start) {
        ref_dv->y = 0;
        ref_dv->x = -MI_SIZE * mib_size - INTRABC_DELAY_PIXELS;
    } else {
        ref_dv->y = -MI_SIZE * mib_size;
        ref_dv->x = 0;
    }
    ref_dv->y *= 8;
    ref_dv->x *= 8;
}
#if CONFIG_ENABLE_OBMC
int svt_av1_skip_u4x4_pred_in_obmc(BlockSize bsize, int dir, int subsampling_x, int subsampling_y) {
    assert(is_motion_variation_allowed_bsize(bsize));

    const BlockSize bsize_plane = get_plane_block_size(bsize, subsampling_x, subsampling_y);
    switch (bsize_plane) {
#if DISABLE_CHROMA_U8X8_OBMC
    case BLOCK_4X4:
    case BLOCK_8X4:
    case BLOCK_4X8:
        return 1;
        break;
#else
    case BLOCK_4X4:
    case BLOCK_8X4:
    case BLOCK_4X8:
        return dir == 0;
        break;
#endif
    default:
        return 0;
    }
}
#endif

#define MAX_MASK_VALUE (1 << WEDGE_WEIGHT_BITS)

/**
 * Computes SSE of a compound predictor constructed from 2 fundamental
 * predictors p0 and p1 using blending with mask.
 *
 * r1:  Residuals of p1.
 *      (source - p1)
 * d:   Difference of p1 and p0.
 *      (p1 - p0)
 * m:   The blending mask
 * N:   Number of pixels
 *
 * 'r1', 'd', and 'm' are contiguous.
 *
 * Computes:
 *  Sum((MAX_MASK_VALUE*r1 + mask*d)**2), which is equivalent to:
 *  Sum((mask*r0 + (MAX_MASK_VALUE-mask)*r1)**2),
 *    where r0 is (source - p0), and r1 is (source - p1), which is in turn
 *    is equivalent to:
 *  Sum((source*MAX_MASK_VALUE - (mask*p0 + (MAX_MASK_VALUE-mask)*p1))**2),
 *    which is the SSE of the residuals of the compound predictor scaled up by
 *    MAX_MASK_VALUE**2.
 *
 * Note that we clamp the partial term in the loop to 16 bits signed. This is
 * to facilitate equivalent SIMD implementation. It should have no effect if
 * residuals are within 16 - WEDGE_WEIGHT_BITS (=10) signed, which always
 * holds for 8 bit input, and on real input, it should hold practically always,
 * as residuals are expected to be small.
 */
uint64_t svt_av1_wedge_sse_from_residuals_c(const int16_t* r1, const int16_t* d, const uint8_t* m, int N) {
    uint64_t csse = 0;

    for (int i = 0; i < N; i++) {
        int32_t t = MAX_MASK_VALUE * r1[i] + m[i] * d[i];
        t         = clamp(t, INT16_MIN, INT16_MAX);
        csse += t * t;
    }
    return ROUND_POWER_OF_TWO(csse, 2 * WEDGE_WEIGHT_BITS);
}

void svt_aom_combine_interintra(InterIntraMode mode, int8_t use_wedge_interintra, int wedge_index, int wedge_sign,
                                BlockSize bsize, BlockSize plane_bsize, uint8_t* comppred, int compstride,
                                const uint8_t* interpred, int interstride, const uint8_t* intrapred, int intrastride) {
    const int bw = block_size_wide[plane_bsize];
    const int bh = block_size_high[plane_bsize];

    if (use_wedge_interintra) {
        if (svt_aom_is_interintra_wedge_used(bsize)) {
            const uint8_t* mask = svt_aom_get_contiguous_soft_mask(wedge_index, wedge_sign, bsize);
            const int      subw = 2 * mi_size_wide[bsize] == bw;
            const int      subh = 2 * mi_size_high[bsize] == bh;
            svt_aom_blend_a64_mask(comppred,
                                   compstride,
                                   intrapred,
                                   intrastride,
                                   interpred,
                                   interstride,
                                   mask,
                                   block_size_wide[bsize],
                                   bw,
                                   bh,
                                   subw,
                                   subh);
        }
        return;
    } else {
        uint8_t* mask = get_ii_mask(plane_bsize, mode);
        svt_aom_blend_a64_mask(
            comppred, compstride, intrapred, intrastride, interpred, interstride, mask, bw, bw, bh, 0, 0);
    }
}

void svt_aom_highbd_blend_a64_hmask_16bit_c(uint16_t* dst, uint32_t dst_stride, const uint16_t* src0,
                                            uint32_t src0_stride, const uint16_t* src1, uint32_t src1_stride,
                                            const uint8_t* mask, int w, int h, int bd) {
    (void)bd;

    assert(IMPLIES(src0 == dst, src0_stride == dst_stride));
    assert(IMPLIES(src1 == dst, src1_stride == dst_stride));

    assert(h >= 1);
    assert(w >= 1);
    assert(IS_POWER_OF_TWO(h));
    assert(IS_POWER_OF_TWO(w));

    assert(bd == 8 || bd == 10 || bd == 12);

    for (int i = 0; i < h; ++i) {
        for (int j = 0; j < w; ++j) {
            dst[i * dst_stride + j] = AOM_BLEND_A64(mask[j], src0[i * src0_stride + j], src1[i * src1_stride + j]);
        }
    }
}

uint64_t svt_aom_sum_squares_i16_c(const int16_t* src, uint32_t n) {
    uint64_t ss = 0;
    do {
        const int16_t v = *src++;
        ss += v * v;
    } while (--n);

    return ss;
}

// obmc_mask_N[overlap_position]
static const uint8_t obmc_mask_1[1]                      = {64};
DECLARE_ALIGNED(2, static const uint8_t, obmc_mask_2[2]) = {45, 64};

DECLARE_ALIGNED(4, static const uint8_t, obmc_mask_4[4]) = {39, 50, 59, 64};

static const uint8_t obmc_mask_8[8] = {36, 42, 48, 53, 57, 61, 64, 64};

static const uint8_t obmc_mask_16[16] = {34, 37, 40, 43, 46, 49, 52, 54, 56, 58, 60, 61, 64, 64, 64, 64};

static const uint8_t obmc_mask_32[32] = {33, 35, 36, 38, 40, 41, 43, 44, 45, 47, 48, 50, 51, 52, 53, 55,
                                         56, 57, 58, 59, 60, 60, 61, 62, 64, 64, 64, 64, 64, 64, 64, 64};

const uint8_t* svt_av1_get_obmc_mask(int length) {
    switch (length) {
    case 1:
        return obmc_mask_1;
    case 2:
        return obmc_mask_2;
    case 4:
        return obmc_mask_4;
    case 8:
        return obmc_mask_8;
    case 16:
        return obmc_mask_16;
    case 32:
        return obmc_mask_32;
    default:
        assert(0);
        return NULL;
    }
}

int16_t svt_aom_mode_context_analyzer(int16_t mode_context, const MvReferenceFrame* const rf) {
    static unsigned svt_aom_compound_mode_ctx_map[3][COMP_NEWMV_CTXS] = {
        {0, 1, 1, 1, 1},
        {1, 2, 3, 4, 4},
        {4, 4, 5, 6, 7},
    };

    if (rf[1] <= INTRA_FRAME) {
        return mode_context;
    }

    const unsigned newmv_ctx = mode_context & NEWMV_CTX_MASK;
    const unsigned refmv_ctx = (mode_context >> REFMV_OFFSET) & REFMV_CTX_MASK;
    assert((refmv_ctx >> 1) < 3);
    const unsigned comp_ctx = svt_aom_compound_mode_ctx_map[refmv_ctx >> 1][AOMMIN(newmv_ctx, COMP_NEWMV_CTXS - 1)];
    return comp_ctx;
}
