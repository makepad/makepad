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

#ifndef AOM_AV1_ENCODER_RANSAC_H_
#define AOM_AV1_ENCODER_RANSAC_H_

#include <stdio.h>
#include <stdlib.h>
#include <math.h>
#include <memory.h>

#include "global_motion.h"

#define MIN_INLIER_PROB 0.1

static const double kIdentityParams[MAX_PARAMDIM] = {0.0, 0.0, 1.0, 0.0, 0.0, 1.0};

typedef struct {
    int    num_inliers;
    double sse;
    int*   inlier_indices;
} RANSAC_MOTION;

typedef bool (*FindTransformationFunc)(const Correspondence* points, const int* indices, int num_indices,
                                       double* params);
typedef void (*ScoreModelFunc)(const double* mat, const Correspondence* points, int num_points, RANSAC_MOTION* model);

// vtable-like structure which stores all of the information needed by RANSAC
// for a particular model type
typedef struct {
    FindTransformationFunc find_transformation;
    ScoreModelFunc         score_model;

    // The minimum number of points which can be passed to find_transformation
    // to generate a model.
    //
    // This should be set as small as possible. This is due to an observation
    // from section 4 of "Optimal Ransac" by A. Hast, J. Nysjö and
    // A. Marchetti (https://dspace5.zcu.cz/bitstream/11025/6869/1/Hast.pdf):
    // using the minimum possible number of points in the initial model maximizes
    // the chance that all of the selected points are inliers.
    //
    // That paper proposes a method which can deal with models which are
    // contaminated by outliers, which helps in cases where the inlier fraction
    // is low. However, for our purposes, global motion only gives significant
    // gains when the inlier fraction is high.
    //
    // So we do not use the method from this paper, but we do find that
    // minimizing the number of points used for initial model fitting helps
    // make the best use of the limited number of models we consider.
    int minpts;
} RansacModelInfo;

bool svt_aom_ransac(const Correspondence* matched_points, int npoints, TransformationType type,
                    MotionModel* motion_models, int num_desired_motions, bool* mem_alloc_failed);
#endif // AOM_AV1_ENCODER_RANSAC_H_
