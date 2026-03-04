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

#include <arm_neon.h>
#include <math.h>

#include "aom_dsp_rtcd.h"
#include "cdef.h"
#include "definitions.h"
#include "mem_neon.h"
#include "neon_sve_bridge.h"

static inline void mse_8xn_16bit_sve(const uint16_t* src, const uint16_t* dst, const int32_t dstride, uint64x2_t* sse,
                                     uint8_t height, uint8_t subsampling_factor) {
    do {
        const uint16x8_t s0 = vld1q_u16(src);
        const uint16x8_t s1 = vld1q_u16(src + subsampling_factor * 8);
        const uint16x8_t d0 = vld1q_u16(dst);
        const uint16x8_t d1 = vld1q_u16(dst + subsampling_factor * dstride);

        const uint16x8_t abs0 = vabdq_u16(s0, d0);
        const uint16x8_t abs1 = vabdq_u16(s1, d1);

        *sse = svt_udotq_u16(*sse, abs0, abs0);
        *sse = svt_udotq_u16(*sse, abs1, abs1);

        src += 8 * 2 * subsampling_factor;
        dst += 2 * subsampling_factor * dstride;
        height -= 2 * subsampling_factor;
    } while (height != 0);
}

static inline void mse_4xn_16bit_sve(const uint16_t* src, const uint16_t* dst, const int32_t dstride, uint64x2_t* sse,
                                     uint8_t height, uint8_t subsampling_factor) {
    do {
        const uint16x8_t s0 = load_u16_4x2(src, 4 * subsampling_factor);
        const uint16x8_t s1 = load_u16_4x2(src + 2 * 4 * subsampling_factor, 4 * subsampling_factor);
        const uint16x8_t d0 = load_u16_4x2(dst, dstride * subsampling_factor);
        const uint16x8_t d1 = load_u16_4x2(dst + 2 * dstride * subsampling_factor, dstride * subsampling_factor);

        const uint16x8_t abs0 = vabdq_u16(s0, d0);
        const uint16x8_t abs1 = vabdq_u16(s1, d1);
        *sse                  = svt_udotq_u16(*sse, abs0, abs0);
        *sse                  = svt_udotq_u16(*sse, abs1, abs1);

        src += 4 * 4 * subsampling_factor;
        dst += 4 * subsampling_factor * dstride;
        height -= 4 * subsampling_factor;
    } while (height != 0);
}

uint64_t svt_aom_compute_cdef_dist_16bit_sve(const uint16_t* dst, int32_t dstride, const uint16_t* src,
                                             const CdefList* dlist, int32_t cdef_count, BlockSize bsize,
                                             int32_t coeff_shift, uint8_t subsampling_factor) {
    uint64_t sum;
    int32_t  bi, bx, by;

    uint64x2_t mse64 = vdupq_n_u64(0);

    if (bsize == BLOCK_8X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            by = dlist[bi].by;
            bx = dlist[bi].bx;
            mse_8xn_16bit_sve(src, dst + (8 * by + 0) * dstride + 8 * bx, dstride, &mse64, 8, subsampling_factor);
            src += 8 * 8;
        }
    } else if (bsize == BLOCK_4X8) {
        for (bi = 0; bi < cdef_count; bi++) {
            by = dlist[bi].by;
            bx = dlist[bi].bx;
            mse_4xn_16bit_sve(src, dst + (8 * by + 0) * dstride + 4 * bx, dstride, &mse64, 8, subsampling_factor);
            src += 4 * 8;
        }
    } else if (bsize == BLOCK_8X4) {
        for (bi = 0; bi < cdef_count; bi++) {
            by = dlist[bi].by;
            bx = dlist[bi].bx;
            mse_8xn_16bit_sve(src, dst + 4 * by * dstride + 8 * bx, dstride, &mse64, 4, subsampling_factor);
            src += 8 * 4;
        }
    } else {
        assert(bsize == BLOCK_4X4);
        for (bi = 0; bi < cdef_count; bi++) {
            by = dlist[bi].by;
            bx = dlist[bi].bx;
            mse_4xn_16bit_sve(src, dst + 4 * by * dstride + 4 * bx, dstride, &mse64, 4, subsampling_factor);
            src += 4 * 4;
        }
    }

    sum = vaddvq_u64(mse64);

    return sum >> 2 * coeff_shift;
}
