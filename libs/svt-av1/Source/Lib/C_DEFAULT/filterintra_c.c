/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

//#include "utility.h"
#include "common_utils.h"
#include "common_dsp_rtcd.h"
#define FILTER_INTRA_SCALE_BITS 4

DECLARE_ALIGNED(16, const int8_t, eb_av1_filter_intra_taps[FILTER_INTRA_MODES][8][8]) = {
    {
        {-6, 10, 0, 0, 0, 12, 0, 0},
        {-5, 2, 10, 0, 0, 9, 0, 0},
        {-3, 1, 1, 10, 0, 7, 0, 0},
        {-3, 1, 1, 2, 10, 5, 0, 0},
        {-4, 6, 0, 0, 0, 2, 12, 0},
        {-3, 2, 6, 0, 0, 2, 9, 0},
        {-3, 2, 2, 6, 0, 2, 7, 0},
        {-3, 1, 2, 2, 6, 3, 5, 0},
    },
    {
        {-10, 16, 0, 0, 0, 10, 0, 0},
        {-6, 0, 16, 0, 0, 6, 0, 0},
        {-4, 0, 0, 16, 0, 4, 0, 0},
        {-2, 0, 0, 0, 16, 2, 0, 0},
        {-10, 16, 0, 0, 0, 0, 10, 0},
        {-6, 0, 16, 0, 0, 0, 6, 0},
        {-4, 0, 0, 16, 0, 0, 4, 0},
        {-2, 0, 0, 0, 16, 0, 2, 0},
    },
    {
        {-8, 8, 0, 0, 0, 16, 0, 0},
        {-8, 0, 8, 0, 0, 16, 0, 0},
        {-8, 0, 0, 8, 0, 16, 0, 0},
        {-8, 0, 0, 0, 8, 16, 0, 0},
        {-4, 4, 0, 0, 0, 0, 16, 0},
        {-4, 0, 4, 0, 0, 0, 16, 0},
        {-4, 0, 0, 4, 0, 0, 16, 0},
        {-4, 0, 0, 0, 4, 0, 16, 0},
    },
    {
        {-2, 8, 0, 0, 0, 10, 0, 0},
        {-1, 3, 8, 0, 0, 6, 0, 0},
        {-1, 2, 3, 8, 0, 4, 0, 0},
        {0, 1, 2, 3, 8, 2, 0, 0},
        {-1, 4, 0, 0, 0, 3, 10, 0},
        {-1, 3, 4, 0, 0, 4, 6, 0},
        {-1, 2, 3, 4, 0, 4, 4, 0},
        {-1, 2, 2, 3, 4, 3, 3, 0},
    },
    {
        {-12, 14, 0, 0, 0, 14, 0, 0},
        {-10, 0, 14, 0, 0, 12, 0, 0},
        {-9, 0, 0, 14, 0, 11, 0, 0},
        {-8, 0, 0, 0, 14, 10, 0, 0},
        {-10, 12, 0, 0, 0, 0, 14, 0},
        {-9, 1, 12, 0, 0, 0, 12, 0},
        {-8, 0, 0, 12, 0, 1, 11, 0},
        {-7, 0, 0, 1, 12, 1, 9, 0},
    },
};

void svt_av1_filter_intra_predictor_c(uint8_t* dst, ptrdiff_t stride, TxSize tx_size, const uint8_t* above,
                                      const uint8_t* left, int32_t mode) {
    int       r, c;
    uint8_t   buffer[33][33];
    const int bw = tx_size_wide[tx_size];
    const int bh = tx_size_high[tx_size];

    assert(bw <= 32 && bh <= 32);

    // The initialization is just for silencing Jenkins static analysis warnings
    for (r = 0; r < bh + 1; ++r) {
        memset(buffer[r], 0, (bw + 1) * sizeof(buffer[0][0]));
    }

    for (r = 0; r < bh; ++r) {
        buffer[r + 1][0] = left[r];
    }
    svt_memcpy_c(buffer[0], &above[-1], (bw + 1) * sizeof(uint8_t));

    for (r = 1; r < bh + 1; r += 2) {
        for (c = 1; c < bw + 1; c += 4) {
            const uint8_t p0 = buffer[r - 1][c - 1];
            const uint8_t p1 = buffer[r - 1][c];
            const uint8_t p2 = buffer[r - 1][c + 1];
            const uint8_t p3 = buffer[r - 1][c + 2];
            const uint8_t p4 = buffer[r - 1][c + 3];
            const uint8_t p5 = buffer[r][c - 1];
            const uint8_t p6 = buffer[r + 1][c - 1];
            for (int k = 0; k < 8; ++k) {
                int r_offset                       = k >> 2;
                int c_offset                       = k & 0x03;
                buffer[r + r_offset][c + c_offset] = clip_pixel(ROUND_POWER_OF_TWO_SIGNED(
                    eb_av1_filter_intra_taps[mode][k][0] * p0 + eb_av1_filter_intra_taps[mode][k][1] * p1 +
                        eb_av1_filter_intra_taps[mode][k][2] * p2 + eb_av1_filter_intra_taps[mode][k][3] * p3 +
                        eb_av1_filter_intra_taps[mode][k][4] * p4 + eb_av1_filter_intra_taps[mode][k][5] * p5 +
                        eb_av1_filter_intra_taps[mode][k][6] * p6,
                    FILTER_INTRA_SCALE_BITS));
            }
        }
    }

    for (r = 0; r < bh; ++r) {
        svt_memcpy_c(dst, &buffer[r + 1][1], bw * sizeof(uint8_t));
        dst += stride;
    }
}
