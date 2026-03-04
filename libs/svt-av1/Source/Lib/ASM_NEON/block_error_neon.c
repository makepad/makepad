/*
 * Copyright (c) 2019, Alliance for Open Media. All rights reserved.
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#include <arm_neon.h>

#include "aom_dsp_rtcd.h"
#include "definitions.h"
#include "mem_neon.h"

int64_t svt_av1_block_error_neon(const TranLow* coeff, const TranLow* dqcoeff, intptr_t block_size, int64_t* ssz) {
    int64x2_t error   = vdupq_n_s64(0);
    int64x2_t sqcoeff = vdupq_n_s64(0);

    assert(block_size >= 8);
    assert((block_size % 8) == 0);

    do {
        const int16x8_t c       = load_tran_low_to_s16q(coeff);
        const int16x8_t d       = load_tran_low_to_s16q(dqcoeff);
        const int16x8_t diff    = vsubq_s16(c, d);
        const int16x4_t diff_lo = vget_low_s16(diff);
        const int16x4_t diff_hi = vget_high_s16(diff);
        // diff is 15-bits, the squares 30, so we can store 2 in 31-bits before
        // accumulating them in 64-bits.
        const int32x4_t err0 = vmull_s16(diff_lo, diff_lo);
        const int32x4_t err1 = vmlal_s16(err0, diff_hi, diff_hi);
        const int64x2_t err2 = vaddl_s32(vget_low_s32(err1), vget_high_s32(err1));
        error                = vaddq_s64(error, err2);

        const int16x4_t coeff_lo = vget_low_s16(c);
        const int16x4_t coeff_hi = vget_high_s16(c);
        const int32x4_t sqcoeff0 = vmull_s16(coeff_lo, coeff_lo);
        const int32x4_t sqcoeff1 = vmlal_s16(sqcoeff0, coeff_hi, coeff_hi);
        const int64x2_t sqcoeff2 = vaddl_s32(vget_low_s32(sqcoeff1), vget_high_s32(sqcoeff1));
        sqcoeff                  = vaddq_s64(sqcoeff, sqcoeff2);

        coeff += 8;
        dqcoeff += 8;
        block_size -= 8;
    } while (block_size != 0);

    *ssz = vaddvq_s64(sqcoeff);
    return vaddvq_s64(error);
}
