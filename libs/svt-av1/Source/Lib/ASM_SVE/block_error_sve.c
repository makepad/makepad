/*
 * Copyright (c) 2023, Alliance for Open Media. All rights reserved.
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#include <arm_sve.h>

#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "mem_neon.h"
#include "neon_sve_bridge.h"

int64_t svt_av1_block_error_sve(const tran_low_t* coeff, const tran_low_t* dqcoeff, intptr_t block_size, int64_t* ssz) {
    int64x2_t error[2]   = {vdupq_n_s64(0), vdupq_n_s64(0)};
    int64x2_t sqcoeff[2] = {vdupq_n_s64(0), vdupq_n_s64(0)};

    assert(block_size >= 16);
    assert(block_size % 16 == 0);

    do {
        const int16x8_t c0 = load_tran_low_to_s16q(coeff);
        const int16x8_t c1 = load_tran_low_to_s16q(coeff + 8);
        const int16x8_t d0 = load_tran_low_to_s16q(dqcoeff);
        const int16x8_t d1 = load_tran_low_to_s16q(dqcoeff + 8);

        const int16x8_t diff0 = vsubq_s16(c0, d0);
        const int16x8_t diff1 = vsubq_s16(c1, d1);

        error[0]   = svt_sdotq_s16(error[0], diff0, diff0);
        error[1]   = svt_sdotq_s16(error[1], diff1, diff1);
        sqcoeff[0] = svt_sdotq_s16(sqcoeff[0], c0, c0);
        sqcoeff[1] = svt_sdotq_s16(sqcoeff[1], c1, c1);

        coeff += 16;
        dqcoeff += 16;
        block_size -= 16;
    } while (block_size != 0);

    *ssz = vaddvq_s64(vaddq_s64(sqcoeff[0], sqcoeff[1]));
    return vaddvq_s64(vaddq_s64(error[0], error[1]));
}
