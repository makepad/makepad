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

#include "aom_dsp_rtcd.h"
#include "coding_unit.h"
#include "definitions.h"
#include "filter.h"
#include "inter_prediction.h"

static inline const InterpFilterParams* av1_get_filter(int subpel_search) {
    assert(subpel_search >= USE_2_TAPS);

    switch (subpel_search) {
    case USE_2_TAPS:
        return &av1_interp_filter_params_list[BILINEAR];
    case USE_4_TAPS:
        return &av1_interp_4tap[EIGHTTAP_REGULAR];
    case USE_8_TAPS:
        return &av1_interp_filter_params_list[EIGHTTAP_REGULAR];
    default:
        assert(0);
        return NULL;
    }
}

// Get pred block from up-sampled reference.
void svt_aom_upsampled_pred_neon(MacroBlockD* xd, const struct AV1Common* const cm, int mi_row, int mi_col,
                                 const Mv* const mv, uint8_t* comp_pred, int width, int height, int subpel_x_q3,
                                 int subpel_y_q3, const uint8_t* ref, int ref_stride, int subpel_search) {
    (void)xd;
    (void)cm;
    (void)mi_row;
    (void)mi_col;
    (void)mv;
    const InterpFilterParams* filter = av1_get_filter(subpel_search);
    assert(filter != NULL);
    if (!subpel_x_q3 && !subpel_y_q3) {
        for (int i = 0; i < height; i++) {
            svt_memcpy(comp_pred, ref, width * sizeof(*comp_pred));
            comp_pred += width;
            ref += ref_stride;
        }
    } else if (!subpel_y_q3) {
        const int16_t* const kernel = av1_get_interp_filter_subpel_kernel(*filter, subpel_x_q3 << 1);
        svt_aom_convolve8_horiz(ref, ref_stride, comp_pred, width, kernel, 16, NULL, -1, width, height);
    } else if (!subpel_x_q3) {
        const int16_t* const kernel = av1_get_interp_filter_subpel_kernel(*filter, subpel_y_q3 << 1);
        svt_aom_convolve8_vert(ref, ref_stride, comp_pred, width, NULL, -1, kernel, 16, width, height);
    } else {
        DECLARE_ALIGNED(16, uint8_t, temp[((MAX_SB_SIZE * 2 + 16) + 16) * MAX_SB_SIZE]);
        const int16_t* const kernel_x            = av1_get_interp_filter_subpel_kernel(*filter, subpel_x_q3 << 1);
        const int16_t* const kernel_y            = av1_get_interp_filter_subpel_kernel(*filter, subpel_y_q3 << 1);
        const int            intermediate_height = (((height - 1) * 8 + subpel_y_q3) >> 3) + filter->taps;
        assert(intermediate_height <= (MAX_SB_SIZE * 2 + 16) + 16);
        svt_aom_convolve8_horiz(ref - ref_stride * ((filter->taps >> 1) - 1),
                                ref_stride,
                                temp,
                                MAX_SB_SIZE,
                                kernel_x,
                                16,
                                NULL,
                                -1,
                                width,
                                intermediate_height);
        svt_aom_convolve8_vert(temp + MAX_SB_SIZE * ((filter->taps >> 1) - 1),
                               MAX_SB_SIZE,
                               comp_pred,
                               width,
                               NULL,
                               -1,
                               kernel_y,
                               16,
                               width,
                               height);
    }
}
