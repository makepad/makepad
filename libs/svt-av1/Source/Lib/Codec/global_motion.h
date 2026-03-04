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

#ifndef AOM_AV1_ENCODER_GLOBAL_MOTION_H_
#define AOM_AV1_ENCODER_GLOBAL_MOTION_H_

#include "definitions.h"
#include "pcs.h"
#include "sequence_control_set.h"

#ifdef __cplusplus
extern "C" {
#endif

#define MAX_CORNERS 4096
#define RANSAC_NUM_MOTIONS 1

typedef struct {
    int x, y;
    int rx, ry;
} Correspondence;

enum {
    GM_ERRORADV_TR_0,
    GM_ERRORADV_TR_1,
    GM_ERRORADV_TR_2,
    GM_ERRORADV_TR_TYPES,
} UENUM1BYTE(GM_ERRORADV_TYPE);

typedef struct {
    double params[MAX_PARAMDIM];
    int*   inliers;
    int    num_inliers;
} MotionModel;

void svt_av1_convert_model_to_params(const double* params, WarpedMotionParams* model);

int svt_av1_is_enough_erroradvantage(double best_erroradvantage, int params_cost, int erroradv_type);

// Returns the av1_warp_error between "dst" and the result of applying the
// motion params that result from fine-tuning "wm" to "ref". Note that "wm" is
// modified in place.
int64_t svt_av1_refine_integerized_param(GmControls* gm_ctrls, WarpedMotionParams* wm, TransformationType wmtype,
                                         uint8_t* ref, int r_width, int r_height, int r_stride, uint8_t* dst,
                                         int d_width, int d_height, int d_stride, int n_refinements, uint8_t chess_refn,
                                         int64_t best_frame_error, uint32_t pic_sad, int params_cost);

void gm_compute_correspondence(PictureParentControlSet* pcs, uint8_t* frm_buffer, int frm_width, int frm_height,
                               int frm_stride, int* frm_corners, int num_frm_corners, uint8_t* ref, int ref_stride,
                               Correspondence* correspondences, int* num_correspondences, uint8_t list_idx,
                               uint8_t ref_idx);
/*
  Computes "num_motions" candidate global motion parameters between two frames.
  The array "params_by_motion" should be length 6 * "num_motions". The ordering
  of each set of parameters is best described  by the homography:

        [x'     (m2 m3 m0   [x
    z .  y'  =   m4 m5 m1 *  y
         1]      0  0  1)    1]

  where m{i} represents the ith value in any given set of parameters.

  "num_inliers" should be length "num_motions", and will be populated with the
  number of inlier feature points for each motion. Params for which the
  num_inliers entry is 0 should be ignored by the caller.
*/
void determine_gm_params(TransformationType type, MotionModel* params_by_motion, int num_motions,
                         Correspondence* correspondences, int num_correspondences);
#ifdef __cplusplus
} // extern "C"
#endif
#endif // AOM_AV1_ENCODER_GLOBAL_MOTION_H_
