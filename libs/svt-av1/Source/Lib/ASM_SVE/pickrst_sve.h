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
#include <arm_sve.h>

#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "mem_neon.h"
#include "neon_sve_bridge.h"
#include "pickrst_neon.h"
#include "restoration.h"
#include "restoration_pick.h"
#include "sum_neon.h"
#include "transpose_neon.h"
#include "utility.h"

void compute_stats_win5_sve(const int16_t* const d, const int32_t d_stride, const int16_t* const s,
                            const int32_t s_stride, const int32_t width, const int32_t height, int64_t* const M,
                            int64_t* const H);

void compute_stats_win7_sve(const int16_t* const d, const int32_t d_stride, const int16_t* const s,
                            const int32_t s_stride, const int32_t width, const int32_t height, int64_t* const M,
                            int64_t* const H);
