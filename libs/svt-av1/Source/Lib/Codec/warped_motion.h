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

#ifndef EbWarpedMotion_h
#define EbWarpedMotion_h

#include "block_structures.h"

#ifdef __cplusplus
extern "C" {
#endif

// Bits of precision used for the model
#define WARPEDMODEL_PREC_BITS 16
#define WARPEDMODEL_ROW3HOMO_PREC_BITS 16

#define WARPEDMODEL_TRANS_CLAMP (128 << WARPEDMODEL_PREC_BITS)
#define WARPEDMODEL_NONDIAGAFFINE_CLAMP (1 << (WARPEDMODEL_PREC_BITS - 3))
#define WARPEDMODEL_ROW3HOMO_CLAMP (1 << (WARPEDMODEL_PREC_BITS - 2))

// Bits of subpel precision for warped interpolation
#define WARPEDPIXEL_PREC_BITS 6
#define WARPEDPIXEL_PREC_SHIFTS (1 << WARPEDPIXEL_PREC_BITS)

#define WARP_PARAM_REDUCE_BITS 6

#define WARPEDDIFF_PREC_BITS (WARPEDMODEL_PREC_BITS - WARPEDPIXEL_PREC_BITS)

extern const int16_t svt_aom_warped_filter[WARPEDPIXEL_PREC_SHIFTS * 3 + 1][8];

EB_ALIGN(16)
static const uint8_t warp_pad_left[14][16] = {
    {1, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15},
    {2, 2, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15},
    {3, 3, 3, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15},
    {4, 4, 4, 4, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15},
    {5, 5, 5, 5, 5, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15},
    {6, 6, 6, 6, 6, 6, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15},
    {7, 7, 7, 7, 7, 7, 7, 7, 8, 9, 10, 11, 12, 13, 14, 15},
    {8, 8, 8, 8, 8, 8, 8, 8, 8, 9, 10, 11, 12, 13, 14, 15},
    {9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 10, 11, 12, 13, 14, 15},
    {10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 11, 12, 13, 14, 15},
    {11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11, 12, 13, 14, 15},
    {12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 13, 14, 15},
    {13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 13, 14, 15},
    {14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 14, 15},
};

EB_ALIGN(16)
static const uint8_t warp_pad_right[14][16] = {{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 14},
                                               {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 13, 13},
                                               {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 12, 12, 12},
                                               {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 11, 11, 11, 11},
                                               {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 10, 10, 10, 10, 10},
                                               {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9, 9, 9, 9},
                                               {0, 1, 2, 3, 4, 5, 6, 7, 8, 8, 8, 8, 8, 8, 8, 8},
                                               {0, 1, 2, 3, 4, 5, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7},
                                               {0, 1, 2, 3, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6},
                                               {0, 1, 2, 3, 4, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5},
                                               {0, 1, 2, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4},
                                               {0, 1, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3},
                                               {0, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2},
                                               {0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1}};

void svt_av1_warp_plane(WarpedMotionParams* wm, int use_hbd, int bd, const uint8_t* ref, const uint8_t* ref_2b,
                        int width, int height, int stride, uint8_t* pred, int p_col, int p_row, int p_width,
                        int p_height, int p_stride, int subsampling_x, int subsampling_y, ConvolveParams* conv_params);

bool svt_find_projection(int np, int* pts1, int* pts2, BlockSize bsize, const Mv mv, WarpedMotionParams* wm_params,
                         int mi_row, int mi_col);

int svt_get_shear_params(WarpedMotionParams* wm);

void svt_warp_plane(WarpedMotionParams* wm, const uint8_t* const ref, int width, int height, int stride, uint8_t* pred,
                    int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x, int subsampling_y,
                    ConvolveParams* conv_params);

uint8_t svt_aom_select_samples(const Mv mv, int* pts, int* pts_inref, int len, BlockSize bsize);

#ifdef __cplusplus
}
#endif
#endif // EbWarpedMotion_h
