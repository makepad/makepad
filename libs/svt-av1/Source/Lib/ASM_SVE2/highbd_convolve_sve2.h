/*
 * Copyright (c) 2023, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#ifndef HIGHBD_CONVOLVE_SVE2_H
#define HIGHBD_CONVOLVE_SVE2_H

#include <arm_neon.h>

#include "neon_sve_bridge.h"
#include "neon_sve2_bridge.h"
#include "definitions.h"

DECLARE_ALIGNED(16, extern const uint16_t, svt_kHbdDotProdMergeBlockTbl[24]);

static inline void svt_tbl2x4_s16(int16x8_t t0[4], int16x8_t t1[4], uint16x8_t tbl, int16x8_t res[4]) {
    res[0] = svt_tbl2_s16(t0[0], t1[0], tbl);
    res[1] = svt_tbl2_s16(t0[1], t1[1], tbl);
    res[2] = svt_tbl2_s16(t0[2], t1[2], tbl);
    res[3] = svt_tbl2_s16(t0[3], t1[3], tbl);
}

static inline void svt_tbl2x2_s16(int16x8_t t0[2], int16x8_t t1[2], uint16x8_t tbl, int16x8_t res[2]) {
    res[0] = svt_tbl2_s16(t0[0], t1[0], tbl);
    res[1] = svt_tbl2_s16(t0[1], t1[1], tbl);
}

#endif // HIGHBD_CONVOLVE_SVE2_H
