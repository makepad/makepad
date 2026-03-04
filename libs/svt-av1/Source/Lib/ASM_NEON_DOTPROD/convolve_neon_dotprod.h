/*
 * Copyright (c) 2025, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#ifndef CONVOLVE_NEON_DOTPROD_H_
#define CONVOLVE_NEON_DOTPROD_H_

#include "definitions.h"

DECLARE_ALIGNED(16, extern const uint8_t, svt_kDotProdPermuteTbl[48]);
DECLARE_ALIGNED(16, extern const uint8_t, svt_kDotProdMergeBlockTbl[48]);

#endif // CONVOLVE_NEON_DOTPROD_H_
