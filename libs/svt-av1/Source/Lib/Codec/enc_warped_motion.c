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

#include <stdlib.h>
#include "aom_dsp_rtcd.h"
#include "enc_warped_motion.h"
#include "bitstream_unit.h"
#include "definitions.h"
#include "convolve.h"

#define WARP_ERROR_BLOCK 32

static int64_t warp_error(WarpedMotionParams* wm, const uint8_t* const ref, int width, int height, int stride,
                          const uint8_t* const dst, int p_col, int p_row, int p_width, int p_height, int p_stride,
                          int subsampling_x, int subsampling_y, uint8_t chess_refn, int64_t best_error) {
    int64_t        gm_sumerr = 0;
    int            warp_w, warp_h;
    int            error_bsize_w = AOMMIN(p_width, WARP_ERROR_BLOCK);
    int            error_bsize_h = AOMMIN(p_height, WARP_ERROR_BLOCK);
    uint8_t        tmp[WARP_ERROR_BLOCK * WARP_ERROR_BLOCK];
    ConvolveParams conv_params   = get_conv_params(0, 0, 0, 8);
    conv_params.use_jnt_comp_avg = 0;

    int i_itr = 0;
    for (int i = p_row; i < p_row + p_height; i += WARP_ERROR_BLOCK) {
        int jstart = (i_itr & 1) ? p_col : p_col + WARP_ERROR_BLOCK;
        int jstep  = 2;

        if (chess_refn == 0) {
            jstart = p_col;
            jstep  = 1;
        }

        for (int j = jstart; j < p_col + p_width; j += jstep * WARP_ERROR_BLOCK) {
            // avoid warping extra 8x8 blocks in the padded region of the frame
            // when p_width and p_height are not multiples of WARP_ERROR_BLOCK
            warp_w = AOMMIN(error_bsize_w, p_col + p_width - j);
            warp_h = AOMMIN(error_bsize_h, p_row + p_height - i);
            svt_warp_plane(wm,
                           ref,
                           width,
                           height,
                           stride,
                           tmp,
                           j,
                           i,
                           warp_w,
                           warp_h,
                           WARP_ERROR_BLOCK,
                           subsampling_x,
                           subsampling_y,
                           &conv_params);

            gm_sumerr += svt_nxm_sad_kernel(tmp, WARP_ERROR_BLOCK, dst + j + i * p_stride, p_stride, warp_h, warp_w);

            if (gm_sumerr > best_error) {
                return gm_sumerr;
            }
        }

        i_itr++;
    }

    gm_sumerr = chess_refn ? gm_sumerr * 2 : gm_sumerr;

    return gm_sumerr;
}

int64_t svt_av1_warp_error(WarpedMotionParams* wm, const uint8_t* ref, int width, int height, int stride, uint8_t* dst,
                           int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x,
                           int subsampling_y, uint8_t chess_refn, int64_t best_error) {
    if (wm->wmtype <= AFFINE) {
        if (!svt_get_shear_params(wm)) {
            return 1;
        }
    }
    return warp_error(wm,
                      ref,
                      width,
                      height,
                      stride,
                      dst,
                      p_col,
                      p_row,
                      p_width,
                      p_height,
                      p_stride,
                      subsampling_x,
                      subsampling_y,
                      chess_refn,
                      best_error);
}
