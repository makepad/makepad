/*
 * Copyright (c) 2017, Alliance for Open Media. All rights reserved
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

// pre: predictor being evaluated
// wsrc: target weighted prediction (has been *4096 to keep precision)
// mask: 2d weights (scaled by 4096)
static INLINE unsigned int obmc_sad(const uint8_t* pre, int pre_stride, const int32_t* wsrc, const int32_t* mask,
                                    int width, int height) {
    int          y, x;
    unsigned int sad = 0;

    for (y = 0; y < height; y++) {
        for (x = 0; x < width; x++) {
            sad += ROUND_POWER_OF_TWO(abs(wsrc[x] - pre[x] * mask[x]), 12);
        }

        pre += pre_stride;
        wsrc += width;
        mask += width;
    }

    return sad;
}

#define OBMCSADMxN(m, n)                                                                \
    unsigned int svt_aom_obmc_sad##m##x##n##_c(                                         \
        const uint8_t* ref, int ref_stride, const int32_t* wsrc, const int32_t* mask) { \
        return obmc_sad(ref, ref_stride, wsrc, mask, m, n);                             \
    }

/* clang-format off */
OBMCSADMxN(128, 128)
OBMCSADMxN(128, 64)
OBMCSADMxN(64, 128)
OBMCSADMxN(64, 64)
OBMCSADMxN(64, 32)
OBMCSADMxN(32, 64)
OBMCSADMxN(32, 32)
OBMCSADMxN(32, 16)
OBMCSADMxN(16, 32)
OBMCSADMxN(16, 16)
OBMCSADMxN(16, 8)
OBMCSADMxN(8, 16)
OBMCSADMxN(8, 8)
OBMCSADMxN(8, 4)
OBMCSADMxN(4, 8)
OBMCSADMxN(4, 4)
OBMCSADMxN(4, 16)
OBMCSADMxN(16, 4)
OBMCSADMxN(8, 32)
OBMCSADMxN(32, 8)
OBMCSADMxN(16, 64)
OBMCSADMxN(64, 16)
    /* clang-format on */
