/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#ifndef AV1_COMMON_AV1_CONVOLVE_H_
#define AV1_COMMON_AV1_CONVOLVE_H_

#ifdef __cplusplus
extern "C" {
#endif

#include "definitions.h"
#include "filter.h"

#define ROUND0_BITS 3
#define COMPOUND_ROUND1_BITS 7
#define WIENER_ROUND0_BITS 3

#define WIENER_CLAMP_LIMIT(r0, bd) (1 << ((bd) + 1 + FILTER_BITS - r0))

typedef void (*AomConvolveFn)(const uint8_t* src, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                              int32_t h, const InterpFilterParams* filter_params_x,
                              const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                              const int32_t subpel_y_q4, ConvolveParams* conv_params);

typedef void (*aom_highbd_convolve_fn_t)(const uint16_t* src, int32_t src_stride, uint16_t* dst, int32_t dst_stride,
                                         int32_t w, int32_t h, const InterpFilterParams* filter_params_x,
                                         const InterpFilterParams* filter_params_y, const int32_t subpel_x_q4,
                                         const int32_t subpel_y_q4, ConvolveParams* conv_params, int32_t bd);

struct AV1Common;
struct scale_factors;

static INLINE ConvolveParams get_conv_params_no_round(int32_t ref, int32_t do_average, int32_t plane, ConvBufType* dst,
                                                      int32_t dst_stride, int32_t is_compound, int32_t bd) {
    (void)plane;
    (void)ref;
    ConvolveParams conv_params;
    // conv_params.ref = ref;
    conv_params.do_average = do_average;
    assert(IMPLIES(do_average, is_compound));
    conv_params.is_compound   = is_compound;
    conv_params.round_0       = ROUND0_BITS;
    conv_params.round_1       = is_compound ? COMPOUND_ROUND1_BITS : 2 * FILTER_BITS - conv_params.round_0;
    const int32_t intbufrange = bd + FILTER_BITS - conv_params.round_0 + 2;
    ASSERT(IMPLIES(bd < 12, intbufrange <= 16));
    if (intbufrange > 16) {
        conv_params.round_0 += intbufrange - 16;
        if (!is_compound) {
            conv_params.round_1 -= intbufrange - 16;
        }
    }
    conv_params.dst        = dst;
    conv_params.dst_stride = dst_stride;
    // conv_params.plane = plane;
    conv_params.use_jnt_comp_avg = 0;

    return conv_params;
}

static INLINE ConvolveParams get_conv_params(int32_t ref, int32_t do_average, int32_t plane, int32_t bd) {
    return get_conv_params_no_round(ref, do_average, plane, NULL, 0, 0, bd);
}

static INLINE ConvolveParams get_conv_params_wiener(int32_t bd) {
    ConvolveParams conv_params;
    (void)bd;
    conv_params.ref           = 0;
    conv_params.do_average    = 0;
    conv_params.is_compound   = 0;
    conv_params.round_0       = WIENER_ROUND0_BITS;
    conv_params.round_1       = 2 * FILTER_BITS - conv_params.round_0;
    const int32_t intbufrange = bd + FILTER_BITS - conv_params.round_0 + 2;
    ASSERT(IMPLIES(bd < 12, intbufrange <= 16));
    if (intbufrange > 16) {
        conv_params.round_0 += intbufrange - 16;
        conv_params.round_1 -= intbufrange - 16;
    }
    conv_params.dst        = NULL;
    conv_params.dst_stride = 0;
    conv_params.plane      = 0;
    // Initialization
    conv_params.fwd_offset            = 0;
    conv_params.bck_offset            = 0;
    conv_params.use_jnt_comp_avg      = 0;
    conv_params.use_dist_wtd_comp_avg = 0;
    return conv_params;
}

void av1_highbd_convolve_2d_facade(const uint8_t* src8, int32_t src_stride, uint8_t* dst, int32_t dst_stride, int32_t w,
                                   int32_t h, InterpFilters interp_filters, const int32_t subpel_x_q4,
                                   int32_t x_step_q4, const int32_t subpel_y_q4, int32_t y_step_q4, int32_t scaled,
                                   ConvolveParams* conv_params, const struct scale_factors* sf, int32_t bd);

#ifdef __cplusplus
} // extern "C"
#endif

#endif // AV1_COMMON_AV1_CONVOLVE_H_
