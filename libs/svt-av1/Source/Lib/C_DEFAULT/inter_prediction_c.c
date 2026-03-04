/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

//#include "utility.h"
#include "definitions.h"

static void diffwtd_mask_d16(uint8_t* mask, int which_inverse, int mask_base, const CONV_BUF_TYPE* src0,
                             int src0_stride, const CONV_BUF_TYPE* src1, int src1_stride, int h, int w,
                             ConvolveParams* conv_params, int bd) {
    int round = 2 * FILTER_BITS - conv_params->round_0 - conv_params->round_1 + (bd - 8);
    int i, j, m, diff;
    for (i = 0; i < h; ++i) {
        for (j = 0; j < w; ++j) {
            diff            = abs(src0[i * src0_stride + j] - src1[i * src1_stride + j]);
            diff            = ROUND_POWER_OF_TWO(diff, round);
            m               = clamp(mask_base + (diff / DIFF_FACTOR), 0, AOM_BLEND_A64_MAX_ALPHA);
            mask[i * w + j] = which_inverse ? AOM_BLEND_A64_MAX_ALPHA - m : m;
        }
    }
}

void svt_av1_build_compound_diffwtd_mask_d16_c(uint8_t* mask, DIFFWTD_MASK_TYPE mask_type, const CONV_BUF_TYPE* src0,
                                               int src0_stride, const CONV_BUF_TYPE* src1, int src1_stride, int h,
                                               int w, ConvolveParams* conv_params, int bd) {
    switch (mask_type) {
    case DIFFWTD_38:
        diffwtd_mask_d16(mask, 0, 38, src0, src0_stride, src1, src1_stride, h, w, conv_params, bd);
        break;
    case DIFFWTD_38_INV:
        diffwtd_mask_d16(mask, 1, 38, src0, src0_stride, src1, src1_stride, h, w, conv_params, bd);
        break;
    default:
        assert(0);
    }
}
