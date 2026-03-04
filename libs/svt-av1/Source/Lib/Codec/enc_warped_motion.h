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

#ifndef EbEncWarpedMotion_h
#define EbEncWarpedMotion_h

#include "warped_motion.h"

#ifdef __cplusplus
extern "C" {
#endif

// Returns the error between the result of applying motion 'wm' to the frame
// described by 'ref' and the frame described by 'dst'.
int64_t svt_av1_warp_error(WarpedMotionParams* wm, const uint8_t* ref, int width, int height, int stride, uint8_t* dst,
                           int p_col, int p_row, int p_width, int p_height, int p_stride, int subsampling_x,
                           int subsampling_y, uint8_t chess_refn, int64_t best_error);

#ifdef __cplusplus
}
#endif
#endif // EbEncWarpedMotion_h
