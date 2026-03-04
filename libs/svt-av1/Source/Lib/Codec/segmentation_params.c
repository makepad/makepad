/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include "definitions.h"
#include "segmentation_params.h"

const int svt_aom_segmentation_feature_signed[SEG_LVL_MAX] = {1, 1, 1, 1, 1, 0, 0, 0};

const int svt_aom_segmentation_feature_bits[SEG_LVL_MAX] = {8, 6, 6, 6, 6, 3, 0, 0};

const int svt_aom_segmentation_feature_max[SEG_LVL_MAX] = {
    MAXQ, MAX_LOOP_FILTER, MAX_LOOP_FILTER, MAX_LOOP_FILTER, MAX_LOOP_FILTER, 7, 0, 0};
