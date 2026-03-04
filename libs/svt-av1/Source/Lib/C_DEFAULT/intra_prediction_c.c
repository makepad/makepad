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
#include "definitions.h"

void svt_av1_upsample_intra_edge_high_c(uint16_t* p, int32_t sz, int32_t bd) {
    // interpolate half-sample positions
    assert(sz <= MAX_UPSAMPLE_SZ);

    uint16_t in[MAX_UPSAMPLE_SZ + 3];
    // copy p[-1..(sz-1)] and extend first and last samples
    in[0] = p[-1];
    in[1] = p[-1];
    for (int32_t i = 0; i < sz; i++) {
        in[i + 2] = p[i];
    }
    in[sz + 2] = p[sz - 1];

    // interpolate half-sample edge positions
    p[-2] = in[0];
    for (int32_t i = 0; i < sz; i++) {
        int32_t s    = -in[i] + (9 * in[i + 1]) + (9 * in[i + 2]) - in[i + 3];
        s            = (s + 8) >> 4;
        s            = clip_pixel_highbd(s, bd);
        p[2 * i - 1] = (uint16_t)s;
        p[2 * i]     = in[i + 2];
    }
}

void svt_av1_upsample_intra_edge_c(uint8_t* p, int32_t sz) {
    // interpolate half-sample positions
    assert(sz <= MAX_UPSAMPLE_SZ);

    uint8_t in[MAX_UPSAMPLE_SZ + 3];
    // copy p[-1..(sz-1)] and extend first and last samples
    in[0] = p[-1];
    in[1] = p[-1];
    for (int32_t i = 0; i < sz; i++) {
        in[i + 2] = p[i];
    }
    in[sz + 2] = p[sz - 1];

    // interpolate half-sample edge positions
    p[-2] = in[0];
    for (int32_t i = 0; i < sz; i++) {
        int32_t s    = -in[i] + (9 * in[i + 1]) + (9 * in[i + 2]) - in[i + 3];
        s            = clip_pixel((s + 8) >> 4);
        p[2 * i - 1] = (uint8_t)s;
        p[2 * i]     = in[i + 2];
    }
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
// Directional prediction, zone 3: 180 < angle < 270
void svt_av1_highbd_dr_prediction_z3_c(uint16_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t* above,
                                       const uint16_t* left, int32_t upsample_left, int32_t dx, int32_t dy,
                                       int32_t bd) {
    (void)above;
    (void)dx;
    (void)bd;
    assert(dx == 1);
    assert(dy > 0);

    const int32_t max_base_y = (bw + bh - 1) << upsample_left;
    const int32_t frac_bits  = 6 - upsample_left;
    const int32_t base_inc   = 1 << upsample_left;
    for (int32_t c = 0, y = dy; c < bw; ++c, y += dy) {
        int32_t base  = y >> frac_bits;
        int32_t shift = ((y << upsample_left) & 0x3F) >> 1;

        for (int32_t r = 0; r < bh; ++r, base += base_inc) {
            if (base < max_base_y) {
                int32_t val         = left[base] * (32 - shift) + left[base + 1] * shift;
                val                 = ROUND_POWER_OF_TWO(val, 5);
                dst[r * stride + c] = (uint16_t)clip_pixel_highbd(val, bd);
            } else {
                for (; r < bh; ++r) {
                    dst[r * stride + c] = left[max_base_y];
                }
                break;
            }
        }
    }
}
#endif
