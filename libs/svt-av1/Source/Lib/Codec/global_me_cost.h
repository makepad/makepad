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

#ifndef EbGlobalMotionEstimationCost_h
#define EbGlobalMotionEstimationCost_h

#include "definitions.h"

int svt_aom_gm_get_params_cost(const WarpedMotionParams* gm, const WarpedMotionParams* ref_gm, int allow_hp);

#endif
