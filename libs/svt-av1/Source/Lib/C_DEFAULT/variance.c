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

#include <assert.h>
#include <stdlib.h>
#include <string.h>

#include "pcs.h"
#include "convolve.h"
#include "aom_dsp_rtcd.h"
#include "inter_prediction.h"

// Applies a 1-D 2-tap bilinear filter to the source block in either horizontal
// or vertical direction to produce the filtered output block. Used to implement
// the first-pass of 2-D separable filter.
//
// Produces int16_t output to retain precision for the next pass. Two filter
// taps should sum to FILTER_WEIGHT. pixel_step defines whether the filter is
// applied horizontally (pixel_step = 1) or vertically (pixel_step = stride).
// It defines the offset required to move from one input to the next.
static void aom_var_filter_block2d_bil_first_pass_c(const uint8_t* a, uint16_t* b, unsigned int src_pixels_per_line,
                                                    unsigned int pixel_step, unsigned int output_height,
                                                    unsigned int output_width, const uint8_t* filter) {
    unsigned int i, j;

    for (i = 0; i < output_height; ++i) {
        for (j = 0; j < output_width; ++j) {
            b[j] = ROUND_POWER_OF_TWO((int)a[0] * filter[0] + (int)a[pixel_step] * filter[1], FILTER_BITS);

            ++a;
        }

        a += src_pixels_per_line - output_width;
        b += output_width;
    }
}

// Applies a 1-D 2-tap bilinear filter to the source block in either horizontal
// or vertical direction to produce the filtered output block. Used to implement
// the second-pass of 2-D separable filter.
//
// Requires 16-bit input as produced by filter_block2d_bil_first_pass. Two
// filter taps should sum to FILTER_WEIGHT. pixel_step defines whether the
// filter is applied horizontally (pixel_step = 1) or vertically
// (pixel_step = stride). It defines the offset required to move from one input
// to the next. Output is 8-bit.
static void aom_var_filter_block2d_bil_second_pass_c(const uint16_t* a, uint8_t* b, unsigned int src_pixels_per_line,
                                                     unsigned int pixel_step, unsigned int output_height,
                                                     unsigned int output_width, const uint8_t* filter) {
    unsigned int i, j;

    for (i = 0; i < output_height; ++i) {
        for (j = 0; j < output_width; ++j) {
            b[j] = ROUND_POWER_OF_TWO((int)a[0] * filter[0] + (int)a[pixel_step] * filter[1], FILTER_BITS);
            ++a;
        }

        a += src_pixels_per_line - output_width;
        b += output_width;
    }
}

static INLINE const InterpFilterParams* av1_get_filter(int subpel_search) {
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
void svt_aom_upsampled_pred_c(MacroBlockD* xd, const struct AV1Common* const cm, int mi_row, int mi_col,
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
        svt_aom_convolve8_horiz_c(ref, ref_stride, comp_pred, width, kernel, 16, NULL, -1, width, height);
    } else if (!subpel_x_q3) {
        const int16_t* const kernel = av1_get_interp_filter_subpel_kernel(*filter, subpel_y_q3 << 1);
        svt_aom_convolve8_vert_c(ref, ref_stride, comp_pred, width, NULL, -1, kernel, 16, width, height);
    } else {
        DECLARE_ALIGNED(16, uint8_t, temp[((MAX_SB_SIZE * 2 + 16) + 16) * MAX_SB_SIZE]);
        const int16_t* const kernel_x            = av1_get_interp_filter_subpel_kernel(*filter, subpel_x_q3 << 1);
        const int16_t* const kernel_y            = av1_get_interp_filter_subpel_kernel(*filter, subpel_y_q3 << 1);
        const int            intermediate_height = (((height - 1) * 8 + subpel_y_q3) >> 3) + filter->taps;
        assert(intermediate_height <= (MAX_SB_SIZE * 2 + 16) + 16);
        svt_aom_convolve8_horiz_c(ref - ref_stride * ((filter->taps >> 1) - 1),
                                  ref_stride,
                                  temp,
                                  MAX_SB_SIZE,
                                  kernel_x,
                                  16,
                                  NULL,
                                  -1,
                                  width,
                                  intermediate_height);
        svt_aom_convolve8_vert_c(temp + MAX_SB_SIZE * ((filter->taps >> 1) - 1),
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

// functions are from deleted file, associated with this macro
// Moved from EbComputeVariance_C.c
static void variance_c(const uint8_t* a, int a_stride, const uint8_t* b, int b_stride, int w, int h, uint32_t* sse,
                       int* sum) {
    int i, j;

    *sum = 0;
    *sse = 0;

    for (i = 0; i < h; ++i) {
        for (j = 0; j < w; ++j) {
            const int diff = a[j] - b[j];
            *sum += diff;
            *sse += diff * diff;
        }

        a += a_stride;
        b += b_stride;
    }
}

// Moved from EbComputeVariance_C.c
// TODO: use or implement a simd version of this
uint32_t svt_aom_variance_highbd_c(const uint16_t* a, int a_stride, const uint16_t* b, int b_stride, int w, int h,
                                   uint32_t* sse) {
    int i, j;

    int sad = 0;
    *sse    = 0;

    for (i = 0; i < h; ++i) {
        for (j = 0; j < w; ++j) {
            const int diff = a[j] - b[j];
            sad += diff;
            *sse += diff * diff;
        }

        a += a_stride;
        b += b_stride;
    }

    return *sse - ((int64_t)sad * sad) / (w * h);
}

// Moved from EbComputeVariance_C.c
#define VAR(W, H)                                                                        \
    uint32_t svt_aom_variance##W##x##H##_c(                                              \
        const uint8_t* a, int a_stride, const uint8_t* b, int b_stride, uint32_t* sse) { \
        int sum;                                                                         \
        variance_c(a, a_stride, b, b_stride, W, H, sse, &sum);                           \
        return *sse - (uint32_t)(((int64_t)sum * sum) / (W * H));                        \
    }

#define SUBPIX_VAR(W, H)                                                                                           \
    uint32_t svt_aom_sub_pixel_variance##W##x##H##_c(                                                              \
        const uint8_t* a, int a_stride, int xoffset, int yoffset, const uint8_t* b, int b_stride, uint32_t* sse) { \
        uint16_t fdata3[(H + 1) * W];                                                                              \
        uint8_t  temp2[H * W];                                                                                     \
                                                                                                                   \
        aom_var_filter_block2d_bil_first_pass_c(a, fdata3, a_stride, 1, H + 1, W, bilinear_filters_2t[xoffset]);   \
        aom_var_filter_block2d_bil_second_pass_c(fdata3, temp2, W, W, H, W, bilinear_filters_2t[yoffset]);         \
                                                                                                                   \
        return svt_aom_variance##W##x##H##_c(temp2, W, b, b_stride, sse);                                          \
    }

/* All the variance are available in the same sizes. */
#define VARIANCES(W, H) \
    VAR(W, H)           \
    SUBPIX_VAR(W, H)
VARIANCES(128, 128)
VARIANCES(128, 64)
VARIANCES(64, 128)
VARIANCES(64, 64)
VARIANCES(64, 32)
VARIANCES(32, 64)
VARIANCES(32, 32)
VARIANCES(32, 16)
VARIANCES(16, 32)
VARIANCES(16, 16)
VARIANCES(16, 8)
VARIANCES(8, 16)
VARIANCES(8, 8)
VARIANCES(8, 4)
VARIANCES(4, 8)
VARIANCES(4, 4)
VARIANCES(4, 16)
VARIANCES(16, 4)
VARIANCES(8, 32)
VARIANCES(32, 8)
VARIANCES(16, 64)
VARIANCES(64, 16)

static INLINE void obmc_variance(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask, int w,
                                 int h, unsigned int* sse, int* sum) {
    int i, j;

    *sse = 0;
    *sum = 0;

    for (i = 0; i < h; i++) {
        for (j = 0; j < w; j++) {
            int diff = ROUND_POWER_OF_TWO_SIGNED(wsrc[j] - pre[j] * mask[j], 12);
            *sum += diff;
            *sse += diff * diff;
        }

        pre += pre_stride;
        wsrc += w;
        mask += w;
    }
}

#define OBMC_VAR(W, H)                                                                                     \
    unsigned int svt_aom_obmc_variance##W##x##H##_c(                                                       \
        const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask, unsigned int* sse) { \
        int sum;                                                                                           \
        obmc_variance(pre, pre_stride, wsrc, mask, W, H, sse, &sum);                                       \
        return *sse - (unsigned int)(((int64_t)sum * sum) / (W * H));                                      \
    }

#define OBMC_SUBPIX_VAR(W, H)                                                                                        \
    unsigned int svt_aom_obmc_sub_pixel_variance##W##x##H##_c(const uint8_t* pre,                                    \
                                                              int            pre_stride,                             \
                                                              int            xoffset,                                \
                                                              int            yoffset,                                \
                                                              const int32_t* wsrc,                                   \
                                                              const int32_t* mask,                                   \
                                                              unsigned int*  sse) {                                   \
        uint16_t fdata3[(H + 1) * W];                                                                                \
        uint8_t  temp2[H * W];                                                                                       \
                                                                                                                     \
        aom_var_filter_block2d_bil_first_pass_c(pre, fdata3, pre_stride, 1, H + 1, W, bilinear_filters_2t[xoffset]); \
        aom_var_filter_block2d_bil_second_pass_c(fdata3, temp2, W, W, H, W, bilinear_filters_2t[yoffset]);           \
                                                                                                                     \
        return svt_aom_obmc_variance##W##x##H##_c(temp2, W, wsrc, mask, sse);                                        \
    }

OBMC_VAR(4, 4)
OBMC_SUBPIX_VAR(4, 4)

OBMC_VAR(4, 8)
OBMC_SUBPIX_VAR(4, 8)

OBMC_VAR(8, 4)
OBMC_SUBPIX_VAR(8, 4)

OBMC_VAR(8, 8)
OBMC_SUBPIX_VAR(8, 8)

OBMC_VAR(8, 16)
OBMC_SUBPIX_VAR(8, 16)

OBMC_VAR(16, 8)
OBMC_SUBPIX_VAR(16, 8)

OBMC_VAR(16, 16)
OBMC_SUBPIX_VAR(16, 16)

OBMC_VAR(16, 32)
OBMC_SUBPIX_VAR(16, 32)

OBMC_VAR(32, 16)
OBMC_SUBPIX_VAR(32, 16)

OBMC_VAR(32, 32)
OBMC_SUBPIX_VAR(32, 32)

OBMC_VAR(32, 64)
OBMC_SUBPIX_VAR(32, 64)

OBMC_VAR(64, 32)
OBMC_SUBPIX_VAR(64, 32)

OBMC_VAR(64, 64)
OBMC_SUBPIX_VAR(64, 64)

OBMC_VAR(64, 128)
OBMC_SUBPIX_VAR(64, 128)

OBMC_VAR(128, 64)
OBMC_SUBPIX_VAR(128, 64)

OBMC_VAR(128, 128)
OBMC_SUBPIX_VAR(128, 128)

OBMC_VAR(4, 16)
OBMC_SUBPIX_VAR(4, 16)
OBMC_VAR(16, 4)
OBMC_SUBPIX_VAR(16, 4)
OBMC_VAR(8, 32)
OBMC_SUBPIX_VAR(8, 32)
OBMC_VAR(32, 8)
OBMC_SUBPIX_VAR(32, 8)
OBMC_VAR(16, 64)
OBMC_SUBPIX_VAR(16, 64)
OBMC_VAR(64, 16)
OBMC_SUBPIX_VAR(64, 16)

uint32_t svt_aom_highbd_mse16x16_c(const uint8_t* src_ptr, int32_t source_stride, const uint8_t* ref_ptr,
                                   int32_t recon_stride) {
    const uint16_t* a    = CONVERT_TO_SHORTPTR(src_ptr);
    const uint16_t* b    = CONVERT_TO_SHORTPTR(ref_ptr);
    uint64_t        tsse = 0;

    for (int i = 0; i < 16; ++i) {
        for (int j = 0; j < 16; ++j) {
            const int diff = a[j] - b[j];
            tsse += (uint32_t)(diff * diff);
        }
        a += source_stride;
        b += recon_stride;
    }
    return (uint32_t)tsse;
}
