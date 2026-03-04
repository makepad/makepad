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

#include <stdlib.h>
#include <string.h>

#include "definitions.h"
#include "intra_prediction.h"
#include "md_process.h"
#include "common_dsp_rtcd.h"

// clang-format off
// Weights are quadratic from '1' to '1 / BlockSize', scaled by
// 2^sm_weight_log2_scale.
const int32_t sm_weight_log2_scale = 8;
const uint8_t sm_weight_arrays[2 * MAX_BLOCK_DIM] = {
    // Unused, because we always offset by bs, which is at least 2.
    0, 0,
    // bs = 2
    255, 128,
    // bs = 4
    255, 149, 85, 64,
    // bs = 8
    255, 197, 146, 105, 73, 50, 37, 32,
    // bs = 16
    255, 225, 196, 170, 145, 123, 102, 84, 68, 54, 43, 33, 26, 20, 17, 16,
    // bs = 32
    255, 240, 225, 210, 196, 182, 169, 157, 145, 133, 122, 111, 101, 92, 83, 74,
    66, 59, 52, 45, 39, 34, 29, 25, 21, 17, 14, 12, 10, 9, 8, 8,
    // bs = 64
    255, 248, 240, 233, 225, 218, 210, 203, 196, 189, 182, 176, 169, 163, 156,
    150, 144, 138, 133, 127, 121, 116, 111, 106, 101, 96, 91, 86, 82, 77, 73, 69,
    65, 61, 57, 54, 50, 47, 44, 41, 38, 35, 32, 29, 27, 25, 22, 20, 18, 16, 15,
    13, 12, 10, 9, 8, 7, 6, 6, 5, 5, 4, 4, 4,
};

DECLARE_ALIGNED(32, uint8_t, base_mask[33][32]) = {
    {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0,
     0,    0,    0,    0,    0,    0,    0,    0, 0, 0, 0, 0, 0, 0, 0, 0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0,
     0,    0,    0,    0,    0,    0,    0,    0,    0, 0, 0, 0, 0, 0, 0, 0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0,
     0,    0,    0,    0,    0,    0,    0,    0,    0,    0, 0, 0, 0, 0, 0, 0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0,
     0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0, 0, 0, 0, 0, 0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0,
     0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0, 0, 0, 0, 0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0,
     0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0, 0, 0, 0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0,
     0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0, 0, 0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0,
     0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0, 0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,
     0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0xff, 0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0xff, 0xff, 0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,    0,    0,    0,    0,    0,    0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,    0,    0,    0,    0,    0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,    0,    0,    0,    0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,    0,    0,    0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,    0,    0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,    0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,    0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,    0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,    0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0},
    {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
     0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff},
};

DECLARE_ALIGNED(16, uint8_t, even_odd_mask_x[8][16]) = {{0, 2, 4, 6, 8, 10, 12, 14, 1, 3, 5, 7, 9, 11, 13, 15},
                                                        {0, 1, 3, 5, 7, 9, 11, 13, 0, 2, 4, 6, 8, 10, 12, 14},
                                                        {0, 0, 2, 4, 6, 8, 10, 12, 0, 0, 3, 5, 7, 9, 11, 13},
                                                        {0, 0, 0, 3, 5, 7, 9, 11, 0, 0, 0, 4, 6, 8, 10, 12},
                                                        {0, 0, 0, 0, 4, 6, 8, 10, 0, 0, 0, 0, 5, 7, 9, 11},
                                                        {0, 0, 0, 0, 0, 5, 7, 9, 0, 0, 0, 0, 0, 6, 8, 10},
                                                        {0, 0, 0, 0, 0, 0, 6, 8, 0, 0, 0, 0, 0, 0, 7, 9},
                                                        {0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 8}};
// clang-format on

// Some basic checks on weights for smooth predictor.
#define sm_weights_sanity_checks(weights_w, weights_h, weights_scale, pred_scale) \
    do {                                                                          \
        assert(weights_w[0] < weights_scale);                                     \
        assert(weights_h[0] < weights_scale);                                     \
        assert(weights_scale - weights_w[bw - 1] < weights_scale);                \
        assert(weights_scale - weights_h[bh - 1] < weights_scale);                \
        assert(pred_scale < 31);                                                  \
    } while (0) // ensures no overflow when calculating predictor.

int svt_aom_is_smooth(const BlockModeInfo* block_mi, int plane) {
    if (plane == 0) {
        const PredictionMode mode = block_mi->mode;
        return (mode == SMOOTH_PRED || mode == SMOOTH_V_PRED || mode == SMOOTH_H_PRED);
    } else {
        // uv_mode is not set for inter blocks, so need to explicitly
        // detect that case.
        if (is_inter_block(block_mi)) {
            return 0;
        }

        const UvPredictionMode uv_mode = block_mi->uv_mode;
        return (uv_mode == UV_SMOOTH_PRED || uv_mode == UV_SMOOTH_V_PRED || uv_mode == UV_SMOOTH_H_PRED);
    }
}

int32_t svt_aom_use_intra_edge_upsample(int32_t bs0, int32_t bs1, int32_t delta, int32_t type) {
    const int32_t d      = abs(delta);
    const int32_t blk_wh = bs0 + bs1;
    if (d <= 0 || d >= 40) {
        return 0;
    }
    return type ? (blk_wh <= 8) : (blk_wh <= 16);
}

#define INTRA_EDGE_FILT 3
#define INTRA_EDGE_TAPS 5

void svt_av1_filter_intra_edge_c(uint8_t* p, int32_t sz, int32_t strength) {
    if (!strength) {
        return;
    }

    static const int32_t kernel[INTRA_EDGE_FILT][INTRA_EDGE_TAPS] = {{0, 4, 8, 4, 0}, {0, 5, 6, 5, 0}, {2, 4, 4, 4, 2}};

    const int32_t filt = strength - 1;
    uint8_t       edge[129];

    svt_memcpy(edge, p, sz * sizeof(*p));
    for (int32_t i = 1; i < sz; i++) {
        int32_t s = 0;
        for (int32_t j = 0; j < INTRA_EDGE_TAPS; j++) {
            int32_t k = i - 2 + j;
            k         = (k < 0) ? 0 : k;
            k         = (k > sz - 1) ? sz - 1 : k;
            s += edge[k] * kernel[filt][j];
        }
        s    = (s + 8) >> 4;
        p[i] = (uint8_t)s;
    }
}

int32_t svt_aom_intra_edge_filter_strength(int32_t bs0, int32_t bs1, int32_t delta, int32_t type) {
    const int32_t d        = abs(delta);
    int32_t       strength = 0;

    const int32_t blk_wh = bs0 + bs1;
    if (type == 0) {
        if (blk_wh <= 8) {
            if (d >= 56) {
                strength = 1;
            }
        } else if (blk_wh <= 12) {
            if (d >= 40) {
                strength = 1;
            }
        } else if (blk_wh <= 16) {
            if (d >= 40) {
                strength = 1;
            }
        } else if (blk_wh <= 24) {
            if (d >= 8) {
                strength = 1;
            }
            if (d >= 16) {
                strength = 2;
            }
            if (d >= 32) {
                strength = 3;
            }
        } else if (blk_wh <= 32) {
            if (d >= 1) {
                strength = 1;
            }
            if (d >= 4) {
                strength = 2;
            }
            if (d >= 32) {
                strength = 3;
            }
        } else {
            if (d >= 1) {
                strength = 3;
            }
        }
    } else {
        if (blk_wh <= 8) {
            if (d >= 40) {
                strength = 1;
            }
            if (d >= 64) {
                strength = 2;
            }
        } else if (blk_wh <= 16) {
            if (d >= 20) {
                strength = 1;
            }
            if (d >= 48) {
                strength = 2;
            }
        } else if (blk_wh <= 24) {
            if (d >= 4) {
                strength = 3;
            }
        } else {
            if (d >= 1) {
                strength = 3;
            }
        }
    }
    return strength;
}

// clang-format off
static const uint16_t eb_dr_intra_derivative[90] = {
    // More evenly spread out angles and limited to 10-bit
    // Values that are 0 will never be used
    //                    Approx angle
    0,    0, 0,        //
    1023, 0, 0,        // 3, ...
    547,  0, 0,        // 6, ...
    372,  0, 0, 0, 0,  // 9, ...
    273,  0, 0,        // 14, ...
    215,  0, 0,        // 17, ...
    178,  0, 0,        // 20, ...
    151,  0, 0,        // 23, ... (113 & 203 are base angles)
    132,  0, 0,        // 26, ...
    116,  0, 0,        // 29, ...
    102,  0, 0, 0,     // 32, ...
    90,   0, 0,        // 36, ...
    80,   0, 0,        // 39, ...
    71,   0, 0,        // 42, ...
    64,   0, 0,        // 45, ... (45 & 135 are base angles)
    57,   0, 0,        // 48, ...
    51,   0, 0,        // 51, ...
    45,   0, 0, 0,     // 54, ...
    40,   0, 0,        // 58, ...
    35,   0, 0,        // 61, ...
    31,   0, 0,        // 64, ...
    27,   0, 0,        // 67, ... (67 & 157 are base angles)
    23,   0, 0,        // 70, ...
    19,   0, 0,        // 73, ...
    15,   0, 0, 0, 0,  // 76, ...
    11,   0, 0,        // 81, ...
    7,    0, 0,        // 84, ...
    3,    0, 0,        // 87, ...
};
// clang-format on

// Get the shift (up-scaled by 256) in Y w.r.t a unit change in X.
// If angle > 0 && angle < 90, dy = 1;
// If angle > 90 && angle < 180, dy = (int32_t)(256 * t);
// If angle > 180 && angle < 270, dy = -((int32_t)(256 * t));

#define divide_round(value, bits) (((value) + (1 << ((bits) - 1))) >> (bits))

static INLINE uint16_t get_dy(int32_t angle) {
    if (angle > 90 && angle < 180) {
        return eb_dr_intra_derivative[angle - 90];
    } else if (angle > 180 && angle < 270) {
        return eb_dr_intra_derivative[270 - angle];
    } else {
        // In this case, we are not really going to use dy. We may return any value.
        return 1;
    }
}

// Get the shift (up-scaled by 256) in X w.r.t a unit change in Y.
// If angle > 0 && angle < 90, dx = -((int32_t)(256 / t));
// If angle > 90 && angle < 180, dx = (int32_t)(256 / t);
// If angle > 180 && angle < 270, dx = 1;
static INLINE uint16_t get_dx(int32_t angle) {
    if (angle > 0 && angle < 90) {
        return eb_dr_intra_derivative[angle];
    } else if (angle > 90 && angle < 180) {
        return eb_dr_intra_derivative[180 - angle];
    } else {
        // In this case, we are not really going to use dx. We may return any value.
        return 1;
    }
}

// Directional prediction, zone 3: 180 < angle < 270
void svt_av1_dr_prediction_z3_c(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                                const uint8_t* left, int32_t upsample_left, int32_t dx, int32_t dy) {
    (void)above;
    (void)dx;

    assert(dx == 1);
    assert(dy > 0);

    const int32_t max_base_y = (bw + bh - 1) << upsample_left;
    const int32_t frac_bits  = 6 - upsample_left;
    const int32_t base_inc   = 1 << upsample_left;
    for (int32_t c = 0, y = dy; c < bw; ++c, y += dy) {
        int32_t base = y >> frac_bits, shift = ((y << upsample_left) & 0x3F) >> 1;

        for (int32_t r = 0; r < bh; ++r, base += base_inc) {
            if (base < max_base_y) {
                int32_t val;
                val                 = left[base] * (32 - shift) + left[base + 1] * shift;
                val                 = ROUND_POWER_OF_TWO(val, 5);
                dst[r * stride + c] = (uint8_t)clip_pixel_highbd(val, 8);
            } else {
                for (; r < bh; ++r) {
                    dst[r * stride + c] = left[max_base_y];
                }
                break;
            }
        }
    }
}

void svt_av1_dr_prediction_z1_c(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                                const uint8_t* left, int32_t upsample_above, int32_t dx, int32_t dy) {
    (void)left;
    (void)dy;
    assert(dy == 1);
    assert(dx > 0);

    const int32_t max_base_x = ((bw + bh) - 1) << upsample_above;
    const int32_t frac_bits  = 6 - upsample_above;
    const int32_t base_inc   = 1 << upsample_above;
    for (int32_t r = 0, x = dx; r < bh; ++r, dst += stride, x += dx) {
        int32_t base = x >> frac_bits, shift = ((x << upsample_above) & 0x3F) >> 1;

        if (base >= max_base_x) {
            for (int32_t i = r; i < bh; ++i) {
                memset(dst, above[max_base_x], bw * sizeof(dst[0]));
                dst += stride;
            }
            return;
        }

        for (int32_t c = 0; c < bw; ++c, base += base_inc) {
            if (base < max_base_x) {
                int32_t val;
                val    = above[base] * (32 - shift) + above[base + 1] * shift;
                val    = ROUND_POWER_OF_TWO(val, 5);
                dst[c] = (uint8_t)clip_pixel_highbd(val, 8);
            } else {
                dst[c] = above[max_base_x];
            }
        }
    }
}

// Directional prediction, zone 2: 90 < angle < 180
void svt_av1_dr_prediction_z2_c(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                                const uint8_t* left, int32_t upsample_above, int32_t upsample_left, int32_t dx,
                                int32_t dy) {
    assert(dx > 0);
    assert(dy > 0);

    const int32_t min_base_x  = -(1 << upsample_above);
    const int32_t frac_bits_x = 6 - upsample_above;
    const int32_t frac_bits_y = 6 - upsample_left;
    const int32_t base_inc_x  = 1 << upsample_above;
    for (int32_t r = 0, x = -dx; r < bh; ++r, x -= dx, dst += stride) {
        int32_t val;
        int32_t base1 = x >> frac_bits_x;
        int32_t y     = (r << 6) - dy;
        for (int32_t c = 0; c < bw; ++c, base1 += base_inc_x, y -= dy) {
            if (base1 >= min_base_x) {
                int32_t shift1 = ((x * (1 << upsample_above)) & 0x3F) >> 1;
                val            = above[base1] * (32 - shift1) + above[base1 + 1] * shift1;
                val            = ROUND_POWER_OF_TWO(val, 5);
            } else {
                int32_t base2 = y >> frac_bits_y;
                assert(base2 >= -(1 << upsample_left));
                int32_t shift2 = ((y * (1 << upsample_left)) & 0x3F) >> 1;
                val            = left[base2] * (32 - shift2) + left[base2 + 1] * shift2;
                val            = ROUND_POWER_OF_TWO(val, 5);
            }
            dst[c] = (uint8_t)clip_pixel_highbd(val, 8);
        }
    }
}

/************************************************************************************************
* svt_cfl_luma_subsampling_420_lbd_c
* Subsample luma samples to match chroma size. Low bit depth and C
************************************************************************************************/
void svt_cfl_luma_subsampling_420_lbd_c(const uint8_t* input, int32_t input_stride, int16_t* output_q3, int32_t width,
                                        int32_t height) {
    for (int32_t j = 0; j < height; j += 2) {
        for (int32_t i = 0; i < width; i += 2) {
            const int32_t bot = i + input_stride;
            output_q3[i >> 1] = (input[i] + input[i + 1] + input[bot] + input[bot + 1]) << 1;
        }
        input += input_stride << 1;
        output_q3 += CFL_BUF_LINE;
    }
}

/************************************************************************************************
* svt_cfl_luma_subsampling_420_hbd_c
* Subsample luma samples to match chroma size. High bit depth and C
************************************************************************************************/
void svt_cfl_luma_subsampling_420_hbd_c(const uint16_t* input, int32_t input_stride, int16_t* output_q3, int32_t width,
                                        int32_t height) {
    for (int32_t j = 0; j < height; j += 2, input += input_stride << 1, output_q3 += CFL_BUF_LINE) {
        for (int32_t i = 0; i < width; i += 2) {
            const int32_t bot = i + input_stride;
            output_q3[i >> 1] = (input[i] + input[i + 1] + input[bot] + input[bot + 1]) << 1;
        }
    }
}

/************************************************************************************************
* svt_subtract_average_c
* Calculate the DC value by averaging over all sample. Subtract DC value to get AC values In C
************************************************************************************************/
void svt_subtract_average_c(int16_t* pred_buf_q3, int32_t width, int32_t height, int32_t round_offset,
                            int32_t num_pel_log2) {
    int32_t  sum_q3   = 0;
    int16_t* pred_buf = pred_buf_q3;
    for (int32_t j = 0; j < height; j++) {
        // assert(pred_buf_q3 + tx_width <= cfl->pred_buf_q3 + CFL_BUF_SQUARE);
        for (int32_t i = 0; i < width; i++) {
            sum_q3 += pred_buf[i];
        }
        pred_buf += CFL_BUF_LINE;
    }
    const int32_t avg_q3 = (sum_q3 + round_offset) >> num_pel_log2;
    // Loss is never more than 1/2 (in Q3)
    // assert(abs((avg_q3 * (1 << num_pel_log2)) - sum_q3) <= 1 << num_pel_log2 >>
    //       1);
    for (int32_t j = 0; j < height; j++) {
        for (int32_t i = 0; i < width; i++) {
            pred_buf_q3[i] -= (int16_t)(avg_q3);
        }
        pred_buf_q3 += CFL_BUF_LINE;
    }
}

CFL_SUB_AVG_FN(c)

// clang-format off
const uint8_t extend_modes[INTRA_MODES] = {
    NEED_ABOVE | NEED_LEFT, // DC
    NEED_ABOVE, // V
    NEED_LEFT, // H
    NEED_ABOVE | NEED_ABOVERIGHT, // D45
    NEED_LEFT | NEED_ABOVE | NEED_ABOVELEFT, // D135
    NEED_LEFT | NEED_ABOVE | NEED_ABOVELEFT, // D113
    NEED_LEFT | NEED_ABOVE | NEED_ABOVELEFT, // D157
    NEED_LEFT | NEED_BOTTOMLEFT, // D203
    NEED_ABOVE | NEED_ABOVERIGHT, // D67
    NEED_LEFT | NEED_ABOVE, // SMOOTH
    NEED_LEFT | NEED_ABOVE, // SMOOTH_V
    NEED_LEFT | NEED_ABOVE, // SMOOTH_H
    NEED_LEFT | NEED_ABOVE | NEED_ABOVELEFT, // PAETH
};

// Tables to store if the top-right reference pixels are available. The flags
// are represented with bits, packed into 8-bit integers. E.g., for the 32x32
// blocks in a 128x128 superblock, the index of the "o" block is 10 (in raster
// order), so its flag is stored at the 3rd bit of the 2nd entry in the table,
// i.e. (table[10 / 8] >> (10 % 8)) & 1.
//       . . . .
//       . . . .
//       . . o .
//       . . . .
static uint8_t has_tr_4x4[128] = {
    255, 255, 255, 255, 85,  85,  85,  85,  119, 119, 119, 119, 85,  85,  85,  85,  127, 127, 127, 127, 85,  85,
    85,  85,  119, 119, 119, 119, 85,  85,  85,  85,  255, 127, 255, 127, 85,  85,  85,  85,  119, 119, 119, 119,
    85,  85,  85,  85,  127, 127, 127, 127, 85,  85,  85,  85,  119, 119, 119, 119, 85,  85,  85,  85,  255, 255,
    255, 127, 85,  85,  85,  85,  119, 119, 119, 119, 85,  85,  85,  85,  127, 127, 127, 127, 85,  85,  85,  85,
    119, 119, 119, 119, 85,  85,  85,  85,  255, 127, 255, 127, 85,  85,  85,  85,  119, 119, 119, 119, 85,  85,
    85,  85,  127, 127, 127, 127, 85,  85,  85,  85,  119, 119, 119, 119, 85,  85,  85,  85,
};
static uint8_t has_tr_4x8[64] = {
    255, 255, 255, 255, 119, 119, 119, 119, 127, 127, 127, 127, 119, 119, 119, 119, 255, 127, 255, 127, 119, 119,
    119, 119, 127, 127, 127, 127, 119, 119, 119, 119, 255, 255, 255, 127, 119, 119, 119, 119, 127, 127, 127, 127,
    119, 119, 119, 119, 255, 127, 255, 127, 119, 119, 119, 119, 127, 127, 127, 127, 119, 119, 119, 119,
};
static uint8_t has_tr_8x4[64] = {
    255, 255, 0,   0,   85,  85,  0,  0,  119, 119, 0,   0,   85,  85,  0,  0,  127, 127, 0,   0,   85, 85,
    0,   0,   119, 119, 0,   0,   85, 85, 0,   0,   255, 127, 0,   0,   85, 85, 0,   0,   119, 119, 0,  0,
    85,  85,  0,   0,   127, 127, 0,  0,  85,  85,  0,   0,   119, 119, 0,  0,  85,  85,  0,   0,
};
static uint8_t has_tr_8x8[32] = {
    255, 255, 85, 85, 119, 119, 85, 85, 127, 127, 85, 85, 119, 119, 85, 85,
    255, 127, 85, 85, 119, 119, 85, 85, 127, 127, 85, 85, 119, 119, 85, 85,
};
static uint8_t has_tr_8x16[16] = {
    255,
    255,
    119,
    119,
    127,
    127,
    119,
    119,
    255,
    127,
    119,
    119,
    127,
    127,
    119,
    119,
};
static uint8_t has_tr_16x8[16] = {
    255,
    0,
    85,
    0,
    119,
    0,
    85,
    0,
    127,
    0,
    85,
    0,
    119,
    0,
    85,
    0,
};
static uint8_t has_tr_16x16[8] = {
    255,
    85,
    119,
    85,
    127,
    85,
    119,
    85,
};
static uint8_t has_tr_16x32[4]   = {255, 119, 127, 119};
static uint8_t has_tr_32x16[4]   = {15, 5, 7, 5};
static uint8_t has_tr_32x32[2]   = {95, 87};
static uint8_t has_tr_32x64[1]   = {127};
static uint8_t has_tr_64x32[1]   = {19};
static uint8_t has_tr_64x64[1]   = {7};
static uint8_t has_tr_64x128[1]  = {3};
static uint8_t has_tr_128x64[1]  = {1};
static uint8_t has_tr_128x128[1] = {1};
static uint8_t has_tr_4x16[32]   = {
    255, 255, 255, 255, 127, 127, 127, 127, 255, 127, 255, 127, 127, 127, 127, 127,
    255, 255, 255, 127, 127, 127, 127, 127, 255, 127, 255, 127, 127, 127, 127, 127,
};
static uint8_t has_tr_16x4[32] = {
    255, 0, 0, 0, 85, 0, 0, 0, 119, 0, 0, 0, 85, 0, 0, 0, 127, 0, 0, 0, 85, 0, 0, 0, 119, 0, 0, 0, 85, 0, 0, 0,
};
static uint8_t has_tr_8x32[8] = {
    255,
    255,
    127,
    127,
    255,
    127,
    127,
    127,
};
static uint8_t has_tr_32x8[8] = {
    15,
    0,
    5,
    0,
    7,
    0,
    5,
    0,
};
static uint8_t has_tr_16x64[2] = {255, 127};
static uint8_t has_tr_64x16[2] = {3, 1};

static const uint8_t* const has_tr_tables[BLOCK_SIZES_ALL] = {
    // 4X4
    has_tr_4x4,
    // 4X8,       8X4,            8X8
    has_tr_4x8,
    has_tr_8x4,
    has_tr_8x8,
    // 8X16,      16X8,           16X16
    has_tr_8x16,
    has_tr_16x8,
    has_tr_16x16,
    // 16X32,     32X16,          32X32
    has_tr_16x32,
    has_tr_32x16,
    has_tr_32x32,
    // 32X64,     64X32,          64X64
    has_tr_32x64,
    has_tr_64x32,
    has_tr_64x64,
    // 64x128,    128x64,         128x128
    has_tr_64x128,
    has_tr_128x64,
    has_tr_128x128,
    // 4x16,      16x4,            8x32
    has_tr_4x16,
    has_tr_16x4,
    has_tr_8x32,
    // 32x8,      16x64,           64x16
    has_tr_32x8,
    has_tr_16x64,
    has_tr_64x16};

static uint8_t has_tr_vert_8x8[32] = {
    255, 255, 0, 0, 119, 119, 0, 0, 127, 127, 0, 0, 119, 119, 0, 0,
    255, 127, 0, 0, 119, 119, 0, 0, 127, 127, 0, 0, 119, 119, 0, 0,
};
static uint8_t has_tr_vert_16x16[8] = {
    255,
    0,
    119,
    0,
    127,
    0,
    119,
    0,
};
static uint8_t has_tr_vert_32x32[2] = {15, 7};
static uint8_t has_tr_vert_64x64[1] = {3};

// The _vert_* tables are like the ordinary tables above, but describe the
// order we visit square blocks when doing a PARTITION_VERT_A or
// PARTITION_VERT_B. This is the same order as normal except for on the last
// split where we go vertically (TL, BL, TR, BR). We treat the rectangular block
// as a pair of squares, which means that these tables work correctly for both
// mixed vertical partition types.
//
// There are tables for each of the square sizes. Vertical rectangles (like
// BLOCK_16X32) use their respective "non-vert" table
static const uint8_t* const has_tr_vert_tables[BLOCK_SIZES] = {
    // 4X4
    NULL,
    // 4X8,      8X4,         8X8
    has_tr_4x8,
    NULL,
    has_tr_vert_8x8,
    // 8X16,     16X8,        16X16
    has_tr_8x16,
    NULL,
    has_tr_vert_16x16,
    // 16X32,    32X16,       32X32
    has_tr_16x32,
    NULL,
    has_tr_vert_32x32,
    // 32X64,    64X32,       64X64
    has_tr_32x64,
    NULL,
    has_tr_vert_64x64,
    // 64x128,   128x64,      128x128
    has_tr_64x128,
    NULL,
    has_tr_128x128};
// clang-format on

static const uint8_t* get_has_tr_table(PartitionType partition, BlockSize bsize) {
    const uint8_t* ret = NULL;
    // If this is a mixed vertical partition, look up bsize in orders_vert.
    if (partition == PARTITION_VERT_A || partition == PARTITION_VERT_B) {
        assert(bsize < BLOCK_SIZES);
        ret = has_tr_vert_tables[bsize];
    } else {
        ret = has_tr_tables[bsize];
    }
    assert(ret);
    return ret;
}

int32_t svt_aom_intra_has_top_right(BlockSize sb_size, BlockSize bsize, int32_t mi_row, int32_t mi_col,
                                    int32_t top_available, int32_t right_available, PartitionType partition,
                                    TxSize txsz, int32_t row_off, int32_t col_off, int32_t ss_x, int32_t ss_y) {
    if (!top_available || !right_available) {
        return 0;
    }

    const int32_t bw_unit              = block_size_wide[bsize] >> tx_size_wide_log2[0];
    const int32_t plane_bw_unit        = AOMMAX(bw_unit >> ss_x, 1);
    const int32_t top_right_count_unit = eb_tx_size_wide_unit[txsz];

    if (row_off > 0) { // Just need to check if enough pixels on the right.
        if (block_size_wide[bsize] > block_size_wide[BLOCK_64X64]) {
            // Special case: For 128x128 blocks, the transform unit whose
            // top-right corner is at the center of the block does in fact have
            // pixels available at its top-right corner.
            if (row_off == mi_size_high[BLOCK_64X64] >> ss_y &&
                col_off + top_right_count_unit == mi_size_wide[BLOCK_64X64] >> ss_x) {
                return 1;
            }
            const int32_t plane_bw_unit_64 = mi_size_wide[BLOCK_64X64] >> ss_x;
            const int32_t col_off_64       = col_off % plane_bw_unit_64;
            return col_off_64 + top_right_count_unit < plane_bw_unit_64;
        }
        return col_off + top_right_count_unit < plane_bw_unit;
    } else {
        // All top-right pixels are in the block above, which is already available.
        if (col_off + top_right_count_unit < plane_bw_unit) {
            return 1;
        }

        const int32_t bw_in_mi_log2 = mi_size_wide_log2[bsize];
        const int32_t bh_in_mi_log2 = mi_size_high_log2[bsize];
        const int32_t sb_mi_size    = mi_size_high[sb_size];
        const int32_t blk_row_in_sb = (mi_row & (sb_mi_size - 1)) >> bh_in_mi_log2;
        const int32_t blk_col_in_sb = (mi_col & (sb_mi_size - 1)) >> bw_in_mi_log2;

        // Top row of superblock: so top-right pixels are in the top and/or
        // top-right superblocks, both of which are already available.
        if (blk_row_in_sb == 0) {
            return 1;
        }

        // Rightmost column of superblock (and not the top row): so top-right pixels
        // fall in the right superblock, which is not available yet.
        if (((blk_col_in_sb + 1) << bw_in_mi_log2) >= sb_mi_size) {
            return 0;
        }
        // General case (neither top row nor rightmost column): check if the
        // top-right block is coded before the current block.
        const int32_t this_blk_index = ((blk_row_in_sb + 0) << (MAX_MIB_SIZE_LOG2 - bw_in_mi_log2)) + blk_col_in_sb + 0;
        const int32_t idx1           = this_blk_index / 8;
        const int32_t idx2           = this_blk_index % 8;
        const uint8_t* has_tr_table  = get_has_tr_table(partition, bsize);
        return (has_tr_table[idx1] >> idx2) & 1;
    }
}

// clang-format off
// Similar to the has_tr_* tables, but store if the bottom-left reference
// pixels are available.
static uint8_t has_bl_4x4[128] = {
    84, 85, 85, 85, 16, 17, 17, 17, 84, 85, 85, 85, 0,  1,  1,  1,  84, 85, 85, 85, 16, 17, 17, 17, 84, 85,
    85, 85, 0,  0,  1,  0,  84, 85, 85, 85, 16, 17, 17, 17, 84, 85, 85, 85, 0,  1,  1,  1,  84, 85, 85, 85,
    16, 17, 17, 17, 84, 85, 85, 85, 0,  0,  0,  0,  84, 85, 85, 85, 16, 17, 17, 17, 84, 85, 85, 85, 0,  1,
    1,  1,  84, 85, 85, 85, 16, 17, 17, 17, 84, 85, 85, 85, 0,  0,  1,  0,  84, 85, 85, 85, 16, 17, 17, 17,
    84, 85, 85, 85, 0,  1,  1,  1,  84, 85, 85, 85, 16, 17, 17, 17, 84, 85, 85, 85, 0,  0,  0,  0,
};
static uint8_t has_bl_4x8[64] = {
    16, 17, 17, 17, 0, 1, 1, 1, 16, 17, 17, 17, 0, 0, 1, 0, 16, 17, 17, 17, 0, 1, 1, 1, 16, 17, 17, 17, 0, 0, 0, 0,
    16, 17, 17, 17, 0, 1, 1, 1, 16, 17, 17, 17, 0, 0, 1, 0, 16, 17, 17, 17, 0, 1, 1, 1, 16, 17, 17, 17, 0, 0, 0, 0,
};
static uint8_t has_bl_8x4[64] = {
    254, 255, 84,  85,  254, 255, 16,  17,  254, 255, 84,  85,  254, 255, 0,   1,   254, 255, 84,  85,  254, 255,
    16,  17,  254, 255, 84,  85,  254, 255, 0,   0,   254, 255, 84,  85,  254, 255, 16,  17,  254, 255, 84,  85,
    254, 255, 0,   1,   254, 255, 84,  85,  254, 255, 16,  17,  254, 255, 84,  85,  254, 255, 0,   0,
};
static uint8_t has_bl_8x8[32] = {
    84, 85, 16, 17, 84, 85, 0, 1, 84, 85, 16, 17, 84, 85, 0, 0,
    84, 85, 16, 17, 84, 85, 0, 1, 84, 85, 16, 17, 84, 85, 0, 0,
};
static uint8_t has_bl_8x16[16] = {
    16,
    17,
    0,
    1,
    16,
    17,
    0,
    0,
    16,
    17,
    0,
    1,
    16,
    17,
    0,
    0,
};
static uint8_t has_bl_16x8[16] = {
    254,
    84,
    254,
    16,
    254,
    84,
    254,
    0,
    254,
    84,
    254,
    16,
    254,
    84,
    254,
    0,
};
static uint8_t has_bl_16x16[8] = {
    84,
    16,
    84,
    0,
    84,
    16,
    84,
    0,
};
static uint8_t has_bl_16x32[4]   = {16, 0, 16, 0};
static uint8_t has_bl_32x16[4]   = {78, 14, 78, 14};
static uint8_t has_bl_32x32[2]   = {4, 4};
static uint8_t has_bl_32x64[1]   = {0};
static uint8_t has_bl_64x32[1]   = {34};
static uint8_t has_bl_64x64[1]   = {0};
static uint8_t has_bl_64x128[1]  = {0};
static uint8_t has_bl_128x64[1]  = {0};
static uint8_t has_bl_128x128[1] = {0};
static uint8_t has_bl_4x16[32]   = {
    0, 1, 1, 1, 0, 0, 1, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0, 1, 0, 0, 1, 1, 1, 0, 0, 0, 0,
};
static uint8_t has_bl_16x4[32] = {
    254, 254, 254, 84, 254, 254, 254, 16, 254, 254, 254, 84, 254, 254, 254, 0,
    254, 254, 254, 84, 254, 254, 254, 16, 254, 254, 254, 84, 254, 254, 254, 0,
};
static uint8_t has_bl_8x32[8] = {
    0,
    1,
    0,
    0,
    0,
    1,
    0,
    0,
};
static uint8_t has_bl_32x8[8] = {
    238,
    78,
    238,
    14,
    238,
    78,
    238,
    14,
};
static uint8_t has_bl_16x64[2] = {0, 0};
static uint8_t has_bl_64x16[2] = {42, 42};

static const uint8_t* const has_bl_tables[BLOCK_SIZES_ALL] = {
    // 4X4
    has_bl_4x4,
    // 4X8,         8X4,         8X8
    has_bl_4x8,
    has_bl_8x4,
    has_bl_8x8,
    // 8X16,        16X8,        16X16
    has_bl_8x16,
    has_bl_16x8,
    has_bl_16x16,
    // 16X32,       32X16,       32X32
    has_bl_16x32,
    has_bl_32x16,
    has_bl_32x32,
    // 32X64,       64X32,       64X64
    has_bl_32x64,
    has_bl_64x32,
    has_bl_64x64,
    // 64x128,      128x64,      128x128
    has_bl_64x128,
    has_bl_128x64,
    has_bl_128x128,
    // 4x16,        16x4,        8x32
    has_bl_4x16,
    has_bl_16x4,
    has_bl_8x32,
    // 32x8,        16x64,       64x16
    has_bl_32x8,
    has_bl_16x64,
    has_bl_64x16};

static uint8_t has_bl_vert_8x8[32] = {
    254, 255, 16, 17, 254, 255, 0, 1, 254, 255, 16, 17, 254, 255, 0, 0,
    254, 255, 16, 17, 254, 255, 0, 1, 254, 255, 16, 17, 254, 255, 0, 0,
};
static uint8_t has_bl_vert_16x16[8] = {
    254,
    16,
    254,
    0,
    254,
    16,
    254,
    0,
};
static uint8_t has_bl_vert_32x32[2] = {14, 14};
static uint8_t has_bl_vert_64x64[1] = {2};

// The _vert_* tables are like the ordinary tables above, but describe the
// order we visit square blocks when doing a PARTITION_VERT_A or
// PARTITION_VERT_B. This is the same order as normal except for on the last
// split where we go vertically (TL, BL, TR, BR). We treat the rectangular block
// as a pair of squares, which means that these tables work correctly for both
// mixed vertical partition types.
//
// There are tables for each of the square sizes. Vertical rectangles (like
// BLOCK_16X32) use their respective "non-vert" table
static const uint8_t* const has_bl_vert_tables[BLOCK_SIZES] = {
    // 4X4
    NULL,
    // 4X8,     8X4,         8X8
    has_bl_4x8,
    NULL,
    has_bl_vert_8x8,
    // 8X16,    16X8,        16X16
    has_bl_8x16,
    NULL,
    has_bl_vert_16x16,
    // 16X32,   32X16,       32X32
    has_bl_16x32,
    NULL,
    has_bl_vert_32x32,
    // 32X64,   64X32,       64X64
    has_bl_32x64,
    NULL,
    has_bl_vert_64x64,
    // 64x128,  128x64,      128x128
    has_bl_64x128,
    NULL,
    has_bl_128x128};
// clang-format on

static const uint8_t* get_has_bl_table(PartitionType partition, BlockSize bsize) {
    const uint8_t* ret = NULL;
    // If this is a mixed vertical partition, look up bsize in orders_vert.
    if (partition == PARTITION_VERT_A || partition == PARTITION_VERT_B) {
        assert(bsize < BLOCK_SIZES);
        ret = has_bl_vert_tables[bsize];
    } else {
        ret = has_bl_tables[bsize];
    }
    assert(ret);
    return ret;
}

int32_t svt_aom_intra_has_bottom_left(BlockSize sb_size, BlockSize bsize, int32_t mi_row, int32_t mi_col,
                                      int32_t bottom_available, int32_t left_available, PartitionType partition,
                                      TxSize txsz, int32_t row_off, int32_t col_off, int32_t ss_x, int32_t ss_y) {
    if (!bottom_available || !left_available) {
        return 0;
    }

    // Special case for 128x* blocks, when col_off is half the block width.
    // This is needed because 128x* superblocks are divided into 64x* blocks in
    // raster order
    if (block_size_wide[bsize] > block_size_wide[BLOCK_64X64] && col_off > 0) {
        const int32_t plane_bw_unit_64 = mi_size_wide[BLOCK_64X64] >> ss_x;
        const int32_t col_off_64       = col_off % plane_bw_unit_64;
        if (col_off_64 == 0) {
            // We are at the left edge of top-right or bottom-right 64x* block.
            const int32_t plane_bh_unit_64 = mi_size_high[BLOCK_64X64] >> ss_y;
            const int32_t row_off_64       = row_off % plane_bh_unit_64;
            const int32_t plane_bh_unit    = AOMMIN(mi_size_high[bsize] >> ss_y, plane_bh_unit_64);
            // Check if all bottom-left pixels are in the left 64x* block (which is
            // already coded).
            return row_off_64 + eb_tx_size_high_unit[txsz] < plane_bh_unit;
        }
    }

    if (col_off > 0) {
        // Bottom-left pixels are in the bottom-left block, which is not available.
        return 0;
    } else {
        const int32_t bh_unit                = block_size_high[bsize] >> tx_size_high_log2[0];
        const int32_t plane_bh_unit          = AOMMAX(bh_unit >> ss_y, 1);
        const int32_t bottom_left_count_unit = eb_tx_size_high_unit[txsz];

        // All bottom-left pixels are in the left block, which is already available.
        if (row_off + bottom_left_count_unit < plane_bh_unit) {
            return 1;
        }

        const int32_t bw_in_mi_log2 = mi_size_wide_log2[bsize];
        const int32_t bh_in_mi_log2 = mi_size_high_log2[bsize];
        const int32_t sb_mi_size    = mi_size_high[sb_size];
        const int32_t blk_row_in_sb = (mi_row & (sb_mi_size - 1)) >> bh_in_mi_log2;
        const int32_t blk_col_in_sb = (mi_col & (sb_mi_size - 1)) >> bw_in_mi_log2;

        // Leftmost column of superblock: so bottom-left pixels maybe in the left
        // and/or bottom-left superblocks. But only the left superblock is
        // available, so check if all required pixels fall in that superblock.
        if (blk_col_in_sb == 0) {
            const int32_t blk_start_row_off = blk_row_in_sb << (bh_in_mi_log2 + MI_SIZE_LOG2 - tx_size_wide_log2[0]) >>
                ss_y;
            const int32_t row_off_in_sb  = blk_start_row_off + row_off;
            const int32_t sb_height_unit = sb_mi_size >> ss_y;
            return row_off_in_sb + bottom_left_count_unit < sb_height_unit;
        }

        // Bottom row of superblock (and not the leftmost column): so bottom-left
        // pixels fall in the bottom superblock, which is not available yet.
        if (((blk_row_in_sb + 1) << bh_in_mi_log2) >= sb_mi_size) {
            return 0;
        }

        // General case (neither leftmost column nor bottom row): check if the
        // bottom-left block is coded before the current block.
        const int32_t this_blk_index = ((blk_row_in_sb + 0) << (MAX_MIB_SIZE_LOG2 - bw_in_mi_log2)) + blk_col_in_sb + 0;
        const int32_t idx1           = this_blk_index / 8;
        const int32_t idx2           = this_blk_index % 8;
        const uint8_t* has_bl_table  = get_has_bl_table(partition, bsize);
        return (has_bl_table[idx1] >> idx2) & 1;
    }
}

IntraPredFn svt_aom_eb_pred[INTRA_MODES][TX_SIZES_ALL];
IntraPredFn svt_aom_dc_pred[2][2][TX_SIZES_ALL];

IntraHighPredFn svt_aom_pred_high[INTRA_MODES][TX_SIZES_ALL];
IntraHighPredFn svt_aom_dc_pred_high[2][2][TX_SIZES_ALL];

static INLINE void dc_128_predictor(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                                    const uint8_t* left) {
    (void)above;
    (void)left;

    for (int32_t r = 0; r < bh; r++) {
        memset(dst, 128, bw);
        dst += stride;
    }
}

static INLINE void dc_left_predictor(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                                     const uint8_t* left) {
    int32_t sum = 0;
    (void)above;

    for (int32_t i = 0; i < bh; i++) {
        sum += left[i];
    }
    int32_t expected_dc = (sum + (bh >> 1)) / bh;

    for (int32_t r = 0; r < bh; r++) {
        memset(dst, expected_dc, bw);
        dst += stride;
    }
}

static INLINE void dc_top_predictor(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                                    const uint8_t* left) {
    int32_t sum = 0;
    (void)left;

    for (int32_t i = 0; i < bw; i++) {
        sum += above[i];
    }
    int32_t expected_dc = (sum + (bw >> 1)) / bw;

    for (int32_t r = 0; r < bh; r++) {
        memset(dst, expected_dc, bw);
        dst += stride;
    }
}

static INLINE void dc_predictor(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                                const uint8_t* left) {
    int32_t       sum   = 0;
    const int32_t count = bw + bh;

    for (int32_t i = 0; i < bw; i++) {
        sum += above[i];
    }
    for (int32_t i = 0; i < bh; i++) {
        sum += left[i];
    }
    int32_t expected_dc = (sum + (count >> 1)) / count;

    for (int32_t r = 0; r < bh; r++) {
        memset(dst, expected_dc, bw);
        dst += stride;
    }
}

static INLINE void v_predictor(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                               const uint8_t* left) {
    (void)left;

    for (int32_t r = 0; r < bh; r++) {
        svt_memcpy(dst, above, bw);
        dst += stride;
    }
}

static INLINE void h_predictor(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                               const uint8_t* left) {
    (void)above;

    for (int32_t r = 0; r < bh; r++) {
        memset(dst, left[r], bw);
        dst += stride;
    }
}

static INLINE void smooth_predictor(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                                    const uint8_t* left) {
    const uint8_t        below_pred   = left[bh - 1]; // estimated by bottom-left pixel
    const uint8_t        right_pred   = above[bw - 1]; // estimated by top-right pixel
    const uint8_t* const sm_weights_w = sm_weight_arrays + bw;
    const uint8_t* const sm_weights_h = sm_weight_arrays + bh;
    // scale = 2 * 2^sm_weight_log2_scale
    const int32_t  log2_scale = 1 + sm_weight_log2_scale;
    const uint16_t scale      = (1 << sm_weight_log2_scale);
    sm_weights_sanity_checks(sm_weights_w, sm_weights_h, scale, log2_scale + 1);
    for (int32_t r = 0; r < bh; ++r, dst += stride) {
        for (int32_t c = 0; c < bw; ++c) {
            const uint8_t pixels[]  = {above[c], below_pred, left[r], right_pred};
            const uint8_t weights[] = {sm_weights_h[r],
                                       (uint8_t)(scale - sm_weights_h[r]),
                                       sm_weights_w[c],
                                       (uint8_t)(scale - sm_weights_w[c])};
            uint32_t      this_pred = 0;
            assert(scale >= sm_weights_h[r] && scale >= sm_weights_w[c]);
            for (int i = 0; i < 4; ++i) {
                this_pred += weights[i] * pixels[i];
            }
            dst[c] = (uint8_t)divide_round(this_pred, log2_scale);
        }
    }
}

static INLINE void smooth_v_predictor(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                                      const uint8_t* left) {
    const uint8_t        below_pred = left[bh - 1]; // estimated by bottom-left pixel
    const uint8_t* const sm_weights = sm_weight_arrays + bh;
    // scale = 2^sm_weight_log2_scale
    const int32_t  log2_scale = sm_weight_log2_scale;
    const uint16_t scale      = (1 << sm_weight_log2_scale);
    sm_weights_sanity_checks(sm_weights, sm_weights, scale, log2_scale + 1);

    for (int32_t r = 0; r < bh; ++r, dst += stride) {
        for (int32_t c = 0; c < bw; ++c) {
            const uint8_t pixels[]  = {above[c], below_pred};
            const uint8_t weights[] = {sm_weights[r], (uint8_t)(scale - sm_weights[r])};
            uint32_t      this_pred = 0;
            assert(scale >= sm_weights[r]);
            for (unsigned i = 0; i < 2; ++i) {
                this_pred += weights[i] * pixels[i];
            }
            dst[c] = (uint8_t)divide_round(this_pred, log2_scale);
        }
    }
}

static INLINE void smooth_h_predictor(uint8_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint8_t* above,
                                      const uint8_t* left) {
    const uint8_t        right_pred = above[bw - 1]; // estimated by top-right pixel
    const uint8_t* const sm_weights = sm_weight_arrays + bw;
    // scale = 2^sm_weight_log2_scale
    const int32_t  log2_scale = sm_weight_log2_scale;
    const uint16_t scale      = (1 << sm_weight_log2_scale);
    sm_weights_sanity_checks(sm_weights, sm_weights, scale, log2_scale + 1);

    for (int32_t r = 0; r < bh; ++r, dst += stride) {
        for (int32_t c = 0; c < bw; ++c) {
            const uint8_t pixels[]  = {left[r], right_pred};
            const uint8_t weights[] = {sm_weights[c], (uint8_t)(scale - sm_weights[c])};
            uint32_t      this_pred = 0;
            assert(scale >= sm_weights[c]);
            for (unsigned i = 0; i < 2; ++i) {
                this_pred += weights[i] * pixels[i];
            }
            dst[c] = (uint8_t)divide_round(this_pred, log2_scale);
        }
    }
}

#undef DC_MULTIPLIER_1X2
#undef DC_MULTIPLIER_1X4

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
static INLINE void highbd_v_predictor(uint16_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t* above,
                                      const uint16_t* left, int32_t bd) {
    (void)left;
    (void)bd;
    for (int32_t r = 0; r < bh; r++) {
        svt_memcpy(dst, above, bw * sizeof(uint16_t));
        dst += stride;
    }
}

static INLINE void highbd_h_predictor(uint16_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t* above,
                                      const uint16_t* left, int32_t bd) {
    (void)above;
    (void)bd;
    for (int32_t r = 0; r < bh; r++) {
        svt_aom_memset16(dst, left[r], bw);
        dst += stride;
    }
}
#endif
static INLINE int abs_diff(int a, int b) {
    return (a > b) ? a - b : b - a;
}

static INLINE uint16_t paeth_predictor_single(uint16_t left, uint16_t top, uint16_t top_left) {
    const int base       = top + left - top_left;
    const int p_left     = abs_diff(base, left);
    const int p_top      = abs_diff(base, top);
    const int p_top_left = abs_diff(base, top_left);

    // Return nearest to base of left, top and top_left.
    return (p_left <= p_top && p_left <= p_top_left) ? left : (p_top <= p_top_left) ? top : top_left;
}

static INLINE void paeth_predictor(uint8_t* dst, ptrdiff_t stride, int bw, int bh, const uint8_t* above,
                                   const uint8_t* left) {
    const uint8_t ytop_left = above[-1];

    for (int r = 0; r < bh; ++r, dst += stride) {
        for (int c = 0; c < bw; ++c) {
            dst[c] = (uint8_t)paeth_predictor_single(left[r], above[c], ytop_left);
        }
    }
}

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
static INLINE void highbd_paeth_predictor(uint16_t* dst, ptrdiff_t stride, int bw, int bh, const uint16_t* above,
                                          const uint16_t* left, int bd) {
    const uint16_t ytop_left = above[-1];
    (void)bd;

    for (int r = 0; r < bh; ++r, dst += stride) {
        for (int c = 0; c < bw; ++c) {
            dst[c] = paeth_predictor_single(left[r], above[c], ytop_left);
        }
    }
}

static INLINE void highbd_smooth_predictor(uint16_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh,
                                           const uint16_t* above, const uint16_t* left, int32_t bd) {
    (void)bd;
    const uint16_t       below_pred   = left[bh - 1]; // estimated by bottom-left pixel
    const uint16_t       right_pred   = above[bw - 1]; // estimated by top-right pixel
    const uint8_t* const sm_weights_w = sm_weight_arrays + bw;
    const uint8_t* const sm_weights_h = sm_weight_arrays + bh;
    // scale = 2 * 2^sm_weight_log2_scale
    const int32_t  log2_scale = 1 + sm_weight_log2_scale;
    const uint16_t scale      = (1 << sm_weight_log2_scale);
    sm_weights_sanity_checks(sm_weights_w, sm_weights_h, scale, log2_scale + 2);
    for (int32_t r = 0; r < bh; ++r, dst += stride) {
        for (int32_t c = 0; c < bw; ++c) {
            const uint16_t pixels[]  = {above[c], below_pred, left[r], right_pred};
            const uint8_t  weights[] = {sm_weights_h[r],
                                        (uint8_t)(scale - sm_weights_h[r]),
                                        sm_weights_w[c],
                                        (uint8_t)(scale - sm_weights_w[c])};
            uint32_t       this_pred = 0;
            assert(scale >= sm_weights_h[r] && scale >= sm_weights_w[c]);
            for (int i = 0; i < 4; ++i) {
                this_pred += weights[i] * pixels[i];
            }
            dst[c] = (uint16_t)divide_round(this_pred, log2_scale);
        }
    }
}

static INLINE void highbd_smooth_v_predictor(uint16_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh,
                                             const uint16_t* above, const uint16_t* left, int32_t bd) {
    (void)bd;
    const uint16_t       below_pred = left[bh - 1]; // estimated by bottom-left pixel
    const uint8_t* const sm_weights = sm_weight_arrays + bh;
    // scale = 2^sm_weight_log2_scale
    const int32_t  log2_scale = sm_weight_log2_scale;
    const uint16_t scale      = (1 << sm_weight_log2_scale);
    sm_weights_sanity_checks(sm_weights, sm_weights, scale, log2_scale + sizeof(*dst));

    for (int32_t r = 0; r < bh; ++r, dst += stride) {
        for (int32_t c = 0; c < bw; ++c) {
            const uint16_t pixels[]  = {above[c], below_pred};
            const uint8_t  weights[] = {sm_weights[r], (uint8_t)(scale - sm_weights[r])};
            uint32_t       this_pred = 0;
            assert(scale >= sm_weights[r]);
            for (int i = 0; i < 2; ++i) {
                this_pred += weights[i] * pixels[i];
            }
            dst[c] = (uint16_t)divide_round(this_pred, log2_scale);
        }
    }
}

static INLINE void highbd_smooth_h_predictor(uint16_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh,
                                             const uint16_t* above, const uint16_t* left, int32_t bd) {
    (void)bd;
    const uint16_t       right_pred = above[bw - 1]; // estimated by top-right pixel
    const uint8_t* const sm_weights = sm_weight_arrays + bw;
    // scale = 2^sm_weight_log2_scale
    const int32_t  log2_scale = sm_weight_log2_scale;
    const uint16_t scale      = (1 << sm_weight_log2_scale);
    sm_weights_sanity_checks(sm_weights, sm_weights, scale, log2_scale + sizeof(*dst));

    for (int32_t r = 0; r < bh; r++, dst += stride) {
        for (int32_t c = 0; c < bw; ++c) {
            const uint16_t pixels[]  = {left[r], right_pred};
            const uint8_t  weights[] = {sm_weights[c], (uint8_t)(scale - sm_weights[c])};
            uint32_t       this_pred = 0;
            assert(scale >= sm_weights[c]);
            for (int i = 0; i < 2; ++i) {
                this_pred += weights[i] * pixels[i];
            }
            dst[c] = (uint16_t)divide_round(this_pred, log2_scale);
        }
    }
}

static INLINE void highbd_dc_128_predictor(uint16_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh,
                                           const uint16_t* above, const uint16_t* left, int32_t bd) {
    (void)above;
    (void)left;

    for (int32_t r = 0; r < bh; r++) {
        svt_aom_memset16(dst, 128 << (bd - 8), bw);
        dst += stride;
    }
}

static INLINE void highbd_dc_left_predictor(uint16_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh,
                                            const uint16_t* above, const uint16_t* left, int32_t bd) {
    int32_t sum = 0;
    (void)above;
    (void)bd;

    for (int32_t i = 0; i < bh; i++) {
        sum += left[i];
    }
    int32_t expected_dc = (sum + (bh >> 1)) / bh;

    for (int32_t r = 0; r < bh; r++) {
        svt_aom_memset16(dst, expected_dc, bw);
        dst += stride;
    }
}

static INLINE void highbd_dc_top_predictor(uint16_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh,
                                           const uint16_t* above, const uint16_t* left, int32_t bd) {
    int32_t sum = 0;
    (void)left;
    (void)bd;

    for (int32_t i = 0; i < bw; i++) {
        sum += above[i];
    }
    int32_t expected_dc = (sum + (bw >> 1)) / bw;

    for (int32_t r = 0; r < bh; r++) {
        svt_aom_memset16(dst, expected_dc, bw);
        dst += stride;
    }
}

static INLINE void highbd_dc_predictor(uint16_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t* above,
                                       const uint16_t* left, int32_t bd) {
    int32_t       sum   = 0;
    const int32_t count = bw + bh;
    (void)bd;

    for (int32_t i = 0; i < bw; i++) {
        sum += above[i];
    }
    for (int32_t i = 0; i < bh; i++) {
        sum += left[i];
    }
    int32_t expected_dc = (sum + (count >> 1)) / count;

    for (int32_t r = 0; r < bh; r++) {
        svt_aom_memset16(dst, expected_dc, bw);
        dst += stride;
    }
}
#endif

#define intra_pred_sized(type, width, height)                                        \
    void svt_aom_##type##_predictor_##width##x##height##_c(                          \
        uint8_t* dst, ptrdiff_t stride, const uint8_t* above, const uint8_t* left) { \
        type##_predictor(dst, stride, width, height, above, left);                   \
    }

intra_pred_sized(dc, 4, 4);
intra_pred_sized(dc, 8, 8);
intra_pred_sized(dc, 16, 16);
intra_pred_sized(dc, 32, 32);
intra_pred_sized(dc, 64, 64);
intra_pred_sized(dc, 4, 8);
intra_pred_sized(dc, 4, 16);
intra_pred_sized(dc, 8, 4);
intra_pred_sized(dc, 8, 16);
intra_pred_sized(dc, 8, 32);
intra_pred_sized(dc, 16, 4);
intra_pred_sized(dc, 16, 8);
intra_pred_sized(dc, 16, 32);
intra_pred_sized(dc, 16, 64);
intra_pred_sized(dc, 32, 8);
intra_pred_sized(dc, 32, 16);
intra_pred_sized(dc, 32, 64);
intra_pred_sized(dc, 64, 16);
intra_pred_sized(dc, 64, 32);

intra_pred_sized(dc_128, 4, 4);
intra_pred_sized(dc_128, 8, 8);
intra_pred_sized(dc_128, 16, 16);
intra_pred_sized(dc_128, 32, 32);
intra_pred_sized(dc_128, 64, 64);
intra_pred_sized(dc_128, 4, 8);
intra_pred_sized(dc_128, 4, 16);
intra_pred_sized(dc_128, 8, 4);
intra_pred_sized(dc_128, 8, 16);
intra_pred_sized(dc_128, 8, 32);
intra_pred_sized(dc_128, 16, 4);
intra_pred_sized(dc_128, 16, 8);
intra_pred_sized(dc_128, 16, 32);
intra_pred_sized(dc_128, 16, 64);
intra_pred_sized(dc_128, 32, 8);
intra_pred_sized(dc_128, 32, 16);
intra_pred_sized(dc_128, 32, 64);
intra_pred_sized(dc_128, 64, 16);
intra_pred_sized(dc_128, 64, 32);

intra_pred_sized(dc_left, 4, 4);
intra_pred_sized(dc_left, 8, 8);
intra_pred_sized(dc_left, 16, 16);
intra_pred_sized(dc_left, 32, 32);
intra_pred_sized(dc_left, 64, 64);
intra_pred_sized(dc_left, 4, 8);
intra_pred_sized(dc_left, 4, 16);
intra_pred_sized(dc_left, 8, 4);
intra_pred_sized(dc_left, 8, 16);
intra_pred_sized(dc_left, 8, 32);
intra_pred_sized(dc_left, 16, 4);
intra_pred_sized(dc_left, 16, 8);
intra_pred_sized(dc_left, 16, 32);
intra_pred_sized(dc_left, 16, 64);
intra_pred_sized(dc_left, 32, 8);
intra_pred_sized(dc_left, 32, 16);
intra_pred_sized(dc_left, 32, 64);
intra_pred_sized(dc_left, 64, 16);
intra_pred_sized(dc_left, 64, 32);

intra_pred_sized(dc_top, 4, 4);
intra_pred_sized(dc_top, 8, 8);
intra_pred_sized(dc_top, 16, 16);
intra_pred_sized(dc_top, 32, 32);
intra_pred_sized(dc_top, 64, 64);
intra_pred_sized(dc_top, 4, 8);
intra_pred_sized(dc_top, 4, 16);
intra_pred_sized(dc_top, 8, 4);
intra_pred_sized(dc_top, 8, 16);
intra_pred_sized(dc_top, 8, 32);
intra_pred_sized(dc_top, 16, 4);
intra_pred_sized(dc_top, 16, 8);
intra_pred_sized(dc_top, 16, 32);
intra_pred_sized(dc_top, 16, 64);
intra_pred_sized(dc_top, 32, 8);
intra_pred_sized(dc_top, 32, 16);
intra_pred_sized(dc_top, 32, 64);
intra_pred_sized(dc_top, 64, 16);
intra_pred_sized(dc_top, 64, 32);
intra_pred_sized(v, 4, 4);
intra_pred_sized(v, 8, 8);
intra_pred_sized(v, 16, 16);
intra_pred_sized(v, 32, 32);
intra_pred_sized(v, 64, 64);
intra_pred_sized(v, 4, 8);
intra_pred_sized(v, 4, 16);
intra_pred_sized(v, 8, 4);
intra_pred_sized(v, 8, 16);
intra_pred_sized(v, 8, 32);
intra_pred_sized(v, 16, 4);
intra_pred_sized(v, 16, 8);
intra_pred_sized(v, 16, 32);
intra_pred_sized(v, 16, 64);
intra_pred_sized(v, 32, 8);
intra_pred_sized(v, 32, 16);
intra_pred_sized(v, 32, 64);
intra_pred_sized(v, 64, 16);
intra_pred_sized(v, 64, 32);
intra_pred_sized(h, 4, 4);
intra_pred_sized(h, 8, 8);
intra_pred_sized(h, 16, 16);
intra_pred_sized(h, 32, 32);
intra_pred_sized(h, 64, 64);
intra_pred_sized(h, 4, 8);
intra_pred_sized(h, 4, 16);
intra_pred_sized(h, 8, 4);
intra_pred_sized(h, 8, 16);
intra_pred_sized(h, 8, 32);
intra_pred_sized(h, 16, 4);
intra_pred_sized(h, 16, 8);
intra_pred_sized(h, 16, 32);
intra_pred_sized(h, 16, 64);
intra_pred_sized(h, 32, 8);
intra_pred_sized(h, 32, 16);
intra_pred_sized(h, 32, 64);
intra_pred_sized(h, 64, 16);
intra_pred_sized(h, 64, 32);
intra_pred_sized(smooth, 4, 4);
intra_pred_sized(smooth, 8, 8);
intra_pred_sized(smooth, 16, 16);
intra_pred_sized(smooth, 32, 32);
intra_pred_sized(smooth, 64, 64);
intra_pred_sized(smooth, 4, 8);
intra_pred_sized(smooth, 4, 16);
intra_pred_sized(smooth, 8, 4);
intra_pred_sized(smooth, 8, 16);
intra_pred_sized(smooth, 8, 32);
intra_pred_sized(smooth, 16, 4);
intra_pred_sized(smooth, 16, 8);
intra_pred_sized(smooth, 16, 32);
intra_pred_sized(smooth, 16, 64);
intra_pred_sized(smooth, 32, 8);
intra_pred_sized(smooth, 32, 16);
intra_pred_sized(smooth, 32, 64);
intra_pred_sized(smooth, 64, 16);
intra_pred_sized(smooth, 64, 32);
intra_pred_sized(smooth_h, 4, 4);
intra_pred_sized(smooth_h, 8, 8);
intra_pred_sized(smooth_h, 16, 16);
intra_pred_sized(smooth_h, 32, 32);
intra_pred_sized(smooth_h, 64, 64);
intra_pred_sized(smooth_h, 4, 8);
intra_pred_sized(smooth_h, 4, 16);
intra_pred_sized(smooth_h, 8, 4);
intra_pred_sized(smooth_h, 8, 16);
intra_pred_sized(smooth_h, 8, 32);
intra_pred_sized(smooth_h, 16, 4);
intra_pred_sized(smooth_h, 16, 8);
intra_pred_sized(smooth_h, 16, 32);
intra_pred_sized(smooth_h, 16, 64);
intra_pred_sized(smooth_h, 32, 8);
intra_pred_sized(smooth_h, 32, 16);
intra_pred_sized(smooth_h, 32, 64);
intra_pred_sized(smooth_h, 64, 16);
intra_pred_sized(smooth_h, 64, 32);
intra_pred_sized(smooth_v, 4, 4);
intra_pred_sized(smooth_v, 8, 8);
intra_pred_sized(smooth_v, 16, 16);
intra_pred_sized(smooth_v, 32, 32);
intra_pred_sized(smooth_v, 64, 64);
intra_pred_sized(smooth_v, 4, 8);
intra_pred_sized(smooth_v, 4, 16);
intra_pred_sized(smooth_v, 8, 4);
intra_pred_sized(smooth_v, 8, 16);
intra_pred_sized(smooth_v, 8, 32);
intra_pred_sized(smooth_v, 16, 4);
intra_pred_sized(smooth_v, 16, 8);
intra_pred_sized(smooth_v, 16, 32);
intra_pred_sized(smooth_v, 16, 64);
intra_pred_sized(smooth_v, 32, 8);
intra_pred_sized(smooth_v, 32, 16);
intra_pred_sized(smooth_v, 32, 64);
intra_pred_sized(smooth_v, 64, 16);
intra_pred_sized(smooth_v, 64, 32);
intra_pred_sized(paeth, 4, 4);
intra_pred_sized(paeth, 8, 8);
intra_pred_sized(paeth, 16, 16);
intra_pred_sized(paeth, 32, 32);
intra_pred_sized(paeth, 64, 64);
intra_pred_sized(paeth, 4, 8);
intra_pred_sized(paeth, 4, 16);
intra_pred_sized(paeth, 8, 4);
intra_pred_sized(paeth, 8, 16);
intra_pred_sized(paeth, 8, 32);
intra_pred_sized(paeth, 16, 4);
intra_pred_sized(paeth, 16, 8);
intra_pred_sized(paeth, 16, 32);
intra_pred_sized(paeth, 16, 64);
intra_pred_sized(paeth, 32, 8);
intra_pred_sized(paeth, 32, 16);
intra_pred_sized(paeth, 32, 64);
intra_pred_sized(paeth, 64, 16);
intra_pred_sized(paeth, 64, 32);
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
#define intra_pred_highbd_sized(type, width, height)                                                \
    void svt_aom_highbd_##type##_predictor_##width##x##height##_c(                                  \
        uint16_t* dst, ptrdiff_t stride, const uint16_t* above, const uint16_t* left, int32_t bd) { \
        highbd_##type##_predictor(dst, stride, width, height, above, left, bd);                     \
    };

intra_pred_highbd_sized(dc, 4, 4);
intra_pred_highbd_sized(dc, 8, 8);
intra_pred_highbd_sized(dc, 16, 16);
intra_pred_highbd_sized(dc, 32, 32);
intra_pred_highbd_sized(dc, 64, 64);
intra_pred_highbd_sized(dc, 4, 8);
intra_pred_highbd_sized(dc, 4, 16);
intra_pred_highbd_sized(dc, 8, 4);
intra_pred_highbd_sized(dc, 8, 16);
intra_pred_highbd_sized(dc, 8, 32);
intra_pred_highbd_sized(dc, 16, 4);
intra_pred_highbd_sized(dc, 16, 8);
intra_pred_highbd_sized(dc, 16, 32);
intra_pred_highbd_sized(dc, 16, 64);
intra_pred_highbd_sized(dc, 32, 8);
intra_pred_highbd_sized(dc, 32, 16);
intra_pred_highbd_sized(dc, 32, 64);
intra_pred_highbd_sized(dc, 64, 16);
intra_pred_highbd_sized(dc, 64, 32);

intra_pred_highbd_sized(dc_128, 4, 4);
intra_pred_highbd_sized(dc_128, 8, 8);
intra_pred_highbd_sized(dc_128, 16, 16);
intra_pred_highbd_sized(dc_128, 32, 32);
intra_pred_highbd_sized(dc_128, 64, 64);

intra_pred_highbd_sized(dc_128, 4, 8);
intra_pred_highbd_sized(dc_128, 4, 16);
intra_pred_highbd_sized(dc_128, 8, 4);
intra_pred_highbd_sized(dc_128, 8, 16);
intra_pred_highbd_sized(dc_128, 8, 32);
intra_pred_highbd_sized(dc_128, 16, 4);
intra_pred_highbd_sized(dc_128, 16, 8);
intra_pred_highbd_sized(dc_128, 16, 32);
intra_pred_highbd_sized(dc_128, 16, 64);
intra_pred_highbd_sized(dc_128, 32, 8);
intra_pred_highbd_sized(dc_128, 32, 16);
intra_pred_highbd_sized(dc_128, 32, 64);
intra_pred_highbd_sized(dc_128, 64, 16);
intra_pred_highbd_sized(dc_128, 64, 32);

intra_pred_highbd_sized(dc_left, 4, 4);
intra_pred_highbd_sized(dc_left, 8, 8);
intra_pred_highbd_sized(dc_left, 16, 16);
intra_pred_highbd_sized(dc_left, 32, 32);
intra_pred_highbd_sized(dc_left, 64, 64);

intra_pred_highbd_sized(dc_left, 4, 8);
intra_pred_highbd_sized(dc_left, 4, 16);
intra_pred_highbd_sized(dc_left, 8, 4);
intra_pred_highbd_sized(dc_left, 8, 16);
intra_pred_highbd_sized(dc_left, 8, 32);
intra_pred_highbd_sized(dc_left, 16, 4);
intra_pred_highbd_sized(dc_left, 16, 8);
intra_pred_highbd_sized(dc_left, 16, 32);
intra_pred_highbd_sized(dc_left, 16, 64);
intra_pred_highbd_sized(dc_left, 32, 8);
intra_pred_highbd_sized(dc_left, 32, 16);
intra_pred_highbd_sized(dc_left, 32, 64);
intra_pred_highbd_sized(dc_left, 64, 16);
intra_pred_highbd_sized(dc_left, 64, 32);

intra_pred_highbd_sized(dc_top, 4, 4);
intra_pred_highbd_sized(dc_top, 8, 8);
intra_pred_highbd_sized(dc_top, 16, 16);
intra_pred_highbd_sized(dc_top, 32, 32);
intra_pred_highbd_sized(dc_top, 64, 64);

intra_pred_highbd_sized(dc_top, 4, 8);
intra_pred_highbd_sized(dc_top, 4, 16);
intra_pred_highbd_sized(dc_top, 8, 4);
intra_pred_highbd_sized(dc_top, 8, 16);
intra_pred_highbd_sized(dc_top, 8, 32);
intra_pred_highbd_sized(dc_top, 16, 4);
intra_pred_highbd_sized(dc_top, 16, 8);
intra_pred_highbd_sized(dc_top, 16, 32);
intra_pred_highbd_sized(dc_top, 16, 64);
intra_pred_highbd_sized(dc_top, 32, 8);
intra_pred_highbd_sized(dc_top, 32, 16);
intra_pred_highbd_sized(dc_top, 32, 64);
intra_pred_highbd_sized(dc_top, 64, 16);
intra_pred_highbd_sized(dc_top, 64, 32);

intra_pred_highbd_sized(v, 4, 4);
intra_pred_highbd_sized(v, 8, 8);
intra_pred_highbd_sized(v, 16, 16);
intra_pred_highbd_sized(v, 32, 32);
intra_pred_highbd_sized(v, 64, 64);

intra_pred_highbd_sized(v, 4, 8);
intra_pred_highbd_sized(v, 4, 16);
intra_pred_highbd_sized(v, 8, 4);
intra_pred_highbd_sized(v, 8, 16);
intra_pred_highbd_sized(v, 8, 32);
intra_pred_highbd_sized(v, 16, 4);
intra_pred_highbd_sized(v, 16, 8);
intra_pred_highbd_sized(v, 16, 32);
intra_pred_highbd_sized(v, 16, 64);
intra_pred_highbd_sized(v, 32, 8);
intra_pred_highbd_sized(v, 32, 16);
intra_pred_highbd_sized(v, 32, 64);
intra_pred_highbd_sized(v, 64, 16);
intra_pred_highbd_sized(v, 64, 32);

intra_pred_highbd_sized(h, 4, 4);
intra_pred_highbd_sized(h, 8, 8);
intra_pred_highbd_sized(h, 16, 16);
intra_pred_highbd_sized(h, 32, 32);
intra_pred_highbd_sized(h, 64, 64);

intra_pred_highbd_sized(h, 4, 8);
intra_pred_highbd_sized(h, 4, 16);
intra_pred_highbd_sized(h, 8, 4);
intra_pred_highbd_sized(h, 8, 16);
intra_pred_highbd_sized(h, 8, 32);
intra_pred_highbd_sized(h, 16, 4);
intra_pred_highbd_sized(h, 16, 8);
intra_pred_highbd_sized(h, 16, 32);
intra_pred_highbd_sized(h, 16, 64);
intra_pred_highbd_sized(h, 32, 8);
intra_pred_highbd_sized(h, 32, 16);
intra_pred_highbd_sized(h, 32, 64);
intra_pred_highbd_sized(h, 64, 16);
intra_pred_highbd_sized(h, 64, 32);

intra_pred_highbd_sized(smooth, 4, 4);
intra_pred_highbd_sized(smooth, 8, 8);
intra_pred_highbd_sized(smooth, 16, 16);
intra_pred_highbd_sized(smooth, 32, 32);
intra_pred_highbd_sized(smooth, 64, 64);

intra_pred_highbd_sized(smooth, 4, 8);
intra_pred_highbd_sized(smooth, 4, 16);
intra_pred_highbd_sized(smooth, 8, 4);
intra_pred_highbd_sized(smooth, 8, 16);
intra_pred_highbd_sized(smooth, 8, 32);
intra_pred_highbd_sized(smooth, 16, 4);
intra_pred_highbd_sized(smooth, 16, 8);
intra_pred_highbd_sized(smooth, 16, 32);
intra_pred_highbd_sized(smooth, 16, 64);
intra_pred_highbd_sized(smooth, 32, 8);
intra_pred_highbd_sized(smooth, 32, 16);
intra_pred_highbd_sized(smooth, 32, 64);
intra_pred_highbd_sized(smooth, 64, 16);
intra_pred_highbd_sized(smooth, 64, 32);

intra_pred_highbd_sized(smooth_h, 4, 4);
intra_pred_highbd_sized(smooth_h, 8, 8);
intra_pred_highbd_sized(smooth_h, 16, 16);
intra_pred_highbd_sized(smooth_h, 32, 32);
intra_pred_highbd_sized(smooth_h, 64, 64);

intra_pred_highbd_sized(smooth_h, 4, 8);
intra_pred_highbd_sized(smooth_h, 4, 16);
intra_pred_highbd_sized(smooth_h, 8, 4);
intra_pred_highbd_sized(smooth_h, 8, 16);
intra_pred_highbd_sized(smooth_h, 8, 32);
intra_pred_highbd_sized(smooth_h, 16, 4);
intra_pred_highbd_sized(smooth_h, 16, 8);
intra_pred_highbd_sized(smooth_h, 16, 32);
intra_pred_highbd_sized(smooth_h, 16, 64);
intra_pred_highbd_sized(smooth_h, 32, 8);
intra_pred_highbd_sized(smooth_h, 32, 16);
intra_pred_highbd_sized(smooth_h, 32, 64);
intra_pred_highbd_sized(smooth_h, 64, 16);
intra_pred_highbd_sized(smooth_h, 64, 32);

intra_pred_highbd_sized(smooth_v, 4, 4);
intra_pred_highbd_sized(smooth_v, 8, 8);
intra_pred_highbd_sized(smooth_v, 16, 16);
intra_pred_highbd_sized(smooth_v, 32, 32);
intra_pred_highbd_sized(smooth_v, 64, 64);

intra_pred_highbd_sized(smooth_v, 4, 8);
intra_pred_highbd_sized(smooth_v, 4, 16);
intra_pred_highbd_sized(smooth_v, 8, 4);
intra_pred_highbd_sized(smooth_v, 8, 16);
intra_pred_highbd_sized(smooth_v, 8, 32);
intra_pred_highbd_sized(smooth_v, 16, 4);
intra_pred_highbd_sized(smooth_v, 16, 8);
intra_pred_highbd_sized(smooth_v, 16, 32);
intra_pred_highbd_sized(smooth_v, 16, 64);
intra_pred_highbd_sized(smooth_v, 32, 8);
intra_pred_highbd_sized(smooth_v, 32, 16);
intra_pred_highbd_sized(smooth_v, 32, 64);
intra_pred_highbd_sized(smooth_v, 64, 16);
intra_pred_highbd_sized(smooth_v, 64, 32);

intra_pred_highbd_sized(paeth, 4, 4);
intra_pred_highbd_sized(paeth, 8, 8);
intra_pred_highbd_sized(paeth, 16, 16);
intra_pred_highbd_sized(paeth, 32, 32);
intra_pred_highbd_sized(paeth, 64, 64);
intra_pred_highbd_sized(paeth, 4, 8);
intra_pred_highbd_sized(paeth, 4, 16);
intra_pred_highbd_sized(paeth, 8, 4);
intra_pred_highbd_sized(paeth, 8, 16);
intra_pred_highbd_sized(paeth, 8, 32);
intra_pred_highbd_sized(paeth, 16, 4);
intra_pred_highbd_sized(paeth, 16, 8);
intra_pred_highbd_sized(paeth, 16, 32);
intra_pred_highbd_sized(paeth, 16, 64);
intra_pred_highbd_sized(paeth, 32, 8);
intra_pred_highbd_sized(paeth, 32, 16);
intra_pred_highbd_sized(paeth, 32, 64);
intra_pred_highbd_sized(paeth, 64, 16);
intra_pred_highbd_sized(paeth, 64, 32);
#endif

static IntraPredFnC dc_pred_c[2][2];
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
static IntraHighBdPredFnC highbd_dc_pred_c[2][2];
#endif

void svt_aom_init_intra_dc_predictors_c_internal(void) {
    dc_pred_c[0][0] = dc_128_predictor;
    dc_pred_c[0][1] = dc_top_predictor;
    dc_pred_c[1][0] = dc_left_predictor;
    dc_pred_c[1][1] = dc_predictor;

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    highbd_dc_pred_c[0][0] = highbd_dc_128_predictor;
    highbd_dc_pred_c[0][1] = highbd_dc_top_predictor;
    highbd_dc_pred_c[1][0] = highbd_dc_left_predictor;
    highbd_dc_pred_c[1][1] = highbd_dc_predictor;
#endif
}

/*static*/ void svt_aom_init_intra_predictors_internal(void) {
    svt_aom_eb_pred[V_PRED][TX_4X4]   = svt_aom_v_predictor_4x4;
    svt_aom_eb_pred[V_PRED][TX_8X8]   = svt_aom_v_predictor_8x8;
    svt_aom_eb_pred[V_PRED][TX_16X16] = svt_aom_v_predictor_16x16;
    svt_aom_eb_pred[V_PRED][TX_32X32] = svt_aom_v_predictor_32x32;
    svt_aom_eb_pred[V_PRED][TX_64X64] = svt_aom_v_predictor_64x64;
    svt_aom_eb_pred[V_PRED][TX_4X8]   = svt_aom_v_predictor_4x8;
    svt_aom_eb_pred[V_PRED][TX_4X16]  = svt_aom_v_predictor_4x16;

    svt_aom_eb_pred[V_PRED][TX_8X4]  = svt_aom_v_predictor_8x4;
    svt_aom_eb_pred[V_PRED][TX_8X16] = svt_aom_v_predictor_8x16;
    svt_aom_eb_pred[V_PRED][TX_8X32] = svt_aom_v_predictor_8x32;

    svt_aom_eb_pred[V_PRED][TX_16X4]  = svt_aom_v_predictor_16x4;
    svt_aom_eb_pred[V_PRED][TX_16X8]  = svt_aom_v_predictor_16x8;
    svt_aom_eb_pred[V_PRED][TX_16X32] = svt_aom_v_predictor_16x32;
    svt_aom_eb_pred[V_PRED][TX_16X64] = svt_aom_v_predictor_16x64;

    svt_aom_eb_pred[V_PRED][TX_32X8]  = svt_aom_v_predictor_32x8;
    svt_aom_eb_pred[V_PRED][TX_32X16] = svt_aom_v_predictor_32x16;
    svt_aom_eb_pred[V_PRED][TX_32X64] = svt_aom_v_predictor_32x64;

    svt_aom_eb_pred[V_PRED][TX_64X16] = svt_aom_v_predictor_64x16;
    svt_aom_eb_pred[V_PRED][TX_64X32] = svt_aom_v_predictor_64x32;

    svt_aom_eb_pred[H_PRED][TX_4X4]   = svt_aom_h_predictor_4x4;
    svt_aom_eb_pred[H_PRED][TX_8X8]   = svt_aom_h_predictor_8x8;
    svt_aom_eb_pred[H_PRED][TX_16X16] = svt_aom_h_predictor_16x16;
    svt_aom_eb_pred[H_PRED][TX_32X32] = svt_aom_h_predictor_32x32;
    svt_aom_eb_pred[H_PRED][TX_64X64] = svt_aom_h_predictor_64x64;

    svt_aom_eb_pred[H_PRED][TX_4X8]  = svt_aom_h_predictor_4x8;
    svt_aom_eb_pred[H_PRED][TX_4X16] = svt_aom_h_predictor_4x16;

    svt_aom_eb_pred[H_PRED][TX_8X4]  = svt_aom_h_predictor_8x4;
    svt_aom_eb_pred[H_PRED][TX_8X16] = svt_aom_h_predictor_8x16;
    svt_aom_eb_pred[H_PRED][TX_8X32] = svt_aom_h_predictor_8x32;

    svt_aom_eb_pred[H_PRED][TX_16X4]  = svt_aom_h_predictor_16x4;
    svt_aom_eb_pred[H_PRED][TX_16X8]  = svt_aom_h_predictor_16x8;
    svt_aom_eb_pred[H_PRED][TX_16X32] = svt_aom_h_predictor_16x32;
    svt_aom_eb_pred[H_PRED][TX_16X64] = svt_aom_h_predictor_16x64;

    svt_aom_eb_pred[H_PRED][TX_32X8]  = svt_aom_h_predictor_32x8;
    svt_aom_eb_pred[H_PRED][TX_32X16] = svt_aom_h_predictor_32x16;
    svt_aom_eb_pred[H_PRED][TX_32X64] = svt_aom_h_predictor_32x64;

    svt_aom_eb_pred[H_PRED][TX_64X16] = svt_aom_h_predictor_64x16;
    svt_aom_eb_pred[H_PRED][TX_64X32] = svt_aom_h_predictor_64x32;

    svt_aom_eb_pred[SMOOTH_PRED][TX_4X4]   = svt_aom_smooth_predictor_4x4;
    svt_aom_eb_pred[SMOOTH_PRED][TX_8X8]   = svt_aom_smooth_predictor_8x8;
    svt_aom_eb_pred[SMOOTH_PRED][TX_16X16] = svt_aom_smooth_predictor_16x16;
    svt_aom_eb_pred[SMOOTH_PRED][TX_32X32] = svt_aom_smooth_predictor_32x32;
    svt_aom_eb_pred[SMOOTH_PRED][TX_64X64] = svt_aom_smooth_predictor_64x64;

    svt_aom_eb_pred[SMOOTH_PRED][TX_4X8]  = svt_aom_smooth_predictor_4x8;
    svt_aom_eb_pred[SMOOTH_PRED][TX_4X16] = svt_aom_smooth_predictor_4x16;

    svt_aom_eb_pred[SMOOTH_PRED][TX_8X4]  = svt_aom_smooth_predictor_8x4;
    svt_aom_eb_pred[SMOOTH_PRED][TX_8X16] = svt_aom_smooth_predictor_8x16;
    svt_aom_eb_pred[SMOOTH_PRED][TX_8X32] = svt_aom_smooth_predictor_8x32;

    svt_aom_eb_pred[SMOOTH_PRED][TX_16X4]  = svt_aom_smooth_predictor_16x4;
    svt_aom_eb_pred[SMOOTH_PRED][TX_16X8]  = svt_aom_smooth_predictor_16x8;
    svt_aom_eb_pred[SMOOTH_PRED][TX_16X32] = svt_aom_smooth_predictor_16x32;
    svt_aom_eb_pred[SMOOTH_PRED][TX_16X64] = svt_aom_smooth_predictor_16x64;

    svt_aom_eb_pred[SMOOTH_PRED][TX_32X8]  = svt_aom_smooth_predictor_32x8;
    svt_aom_eb_pred[SMOOTH_PRED][TX_32X16] = svt_aom_smooth_predictor_32x16;
    svt_aom_eb_pred[SMOOTH_PRED][TX_32X64] = svt_aom_smooth_predictor_32x64;

    svt_aom_eb_pred[SMOOTH_PRED][TX_64X16] = svt_aom_smooth_predictor_64x16;
    svt_aom_eb_pred[SMOOTH_PRED][TX_64X32] = svt_aom_smooth_predictor_64x32;

    svt_aom_eb_pred[SMOOTH_V_PRED][TX_4X4]   = svt_aom_smooth_v_predictor_4x4;
    svt_aom_eb_pred[SMOOTH_V_PRED][TX_8X8]   = svt_aom_smooth_v_predictor_8x8;
    svt_aom_eb_pred[SMOOTH_V_PRED][TX_16X16] = svt_aom_smooth_v_predictor_16x16;
    svt_aom_eb_pred[SMOOTH_V_PRED][TX_32X32] = svt_aom_smooth_v_predictor_32x32;
    svt_aom_eb_pred[SMOOTH_V_PRED][TX_64X64] = svt_aom_smooth_v_predictor_64x64;

    svt_aom_eb_pred[SMOOTH_V_PRED][TX_4X8]  = svt_aom_smooth_v_predictor_4x8;
    svt_aom_eb_pred[SMOOTH_V_PRED][TX_4X16] = svt_aom_smooth_v_predictor_4x16;

    svt_aom_eb_pred[SMOOTH_V_PRED][TX_8X4]  = svt_aom_smooth_v_predictor_8x4;
    svt_aom_eb_pred[SMOOTH_V_PRED][TX_8X16] = svt_aom_smooth_v_predictor_8x16;
    svt_aom_eb_pred[SMOOTH_V_PRED][TX_8X32] = svt_aom_smooth_v_predictor_8x32;

    svt_aom_eb_pred[SMOOTH_V_PRED][TX_16X4]  = svt_aom_smooth_v_predictor_16x4;
    svt_aom_eb_pred[SMOOTH_V_PRED][TX_16X8]  = svt_aom_smooth_v_predictor_16x8;
    svt_aom_eb_pred[SMOOTH_V_PRED][TX_16X32] = svt_aom_smooth_v_predictor_16x32;
    svt_aom_eb_pred[SMOOTH_V_PRED][TX_16X64] = svt_aom_smooth_v_predictor_16x64;

    svt_aom_eb_pred[SMOOTH_V_PRED][TX_32X8]  = svt_aom_smooth_v_predictor_32x8;
    svt_aom_eb_pred[SMOOTH_V_PRED][TX_32X16] = svt_aom_smooth_v_predictor_32x16;
    svt_aom_eb_pred[SMOOTH_V_PRED][TX_32X64] = svt_aom_smooth_v_predictor_32x64;

    svt_aom_eb_pred[SMOOTH_V_PRED][TX_64X16] = svt_aom_smooth_v_predictor_64x16;
    svt_aom_eb_pred[SMOOTH_V_PRED][TX_64X32] = svt_aom_smooth_v_predictor_64x32;

    svt_aom_eb_pred[SMOOTH_H_PRED][TX_4X4]   = svt_aom_smooth_h_predictor_4x4;
    svt_aom_eb_pred[SMOOTH_H_PRED][TX_8X8]   = svt_aom_smooth_h_predictor_8x8;
    svt_aom_eb_pred[SMOOTH_H_PRED][TX_16X16] = svt_aom_smooth_h_predictor_16x16;
    svt_aom_eb_pred[SMOOTH_H_PRED][TX_32X32] = svt_aom_smooth_h_predictor_32x32;
    svt_aom_eb_pred[SMOOTH_H_PRED][TX_64X64] = svt_aom_smooth_h_predictor_64x64;

    svt_aom_eb_pred[SMOOTH_H_PRED][TX_4X8]  = svt_aom_smooth_h_predictor_4x8;
    svt_aom_eb_pred[SMOOTH_H_PRED][TX_4X16] = svt_aom_smooth_h_predictor_4x16;

    svt_aom_eb_pred[SMOOTH_H_PRED][TX_8X4]  = svt_aom_smooth_h_predictor_8x4;
    svt_aom_eb_pred[SMOOTH_H_PRED][TX_8X16] = svt_aom_smooth_h_predictor_8x16;
    svt_aom_eb_pred[SMOOTH_H_PRED][TX_8X32] = svt_aom_smooth_h_predictor_8x32;

    svt_aom_eb_pred[SMOOTH_H_PRED][TX_16X4]  = svt_aom_smooth_h_predictor_16x4;
    svt_aom_eb_pred[SMOOTH_H_PRED][TX_16X8]  = svt_aom_smooth_h_predictor_16x8;
    svt_aom_eb_pred[SMOOTH_H_PRED][TX_16X32] = svt_aom_smooth_h_predictor_16x32;
    svt_aom_eb_pred[SMOOTH_H_PRED][TX_16X64] = svt_aom_smooth_h_predictor_16x64;

    svt_aom_eb_pred[SMOOTH_H_PRED][TX_32X8]  = svt_aom_smooth_h_predictor_32x8;
    svt_aom_eb_pred[SMOOTH_H_PRED][TX_32X16] = svt_aom_smooth_h_predictor_32x16;
    svt_aom_eb_pred[SMOOTH_H_PRED][TX_32X64] = svt_aom_smooth_h_predictor_32x64;

    svt_aom_eb_pred[SMOOTH_H_PRED][TX_64X16] = svt_aom_smooth_h_predictor_64x16;
    svt_aom_eb_pred[SMOOTH_H_PRED][TX_64X32] = svt_aom_smooth_h_predictor_64x32;

    svt_aom_eb_pred[PAETH_PRED][TX_4X4]   = svt_aom_paeth_predictor_4x4;
    svt_aom_eb_pred[PAETH_PRED][TX_8X8]   = svt_aom_paeth_predictor_8x8;
    svt_aom_eb_pred[PAETH_PRED][TX_16X16] = svt_aom_paeth_predictor_16x16;
    svt_aom_eb_pred[PAETH_PRED][TX_32X32] = svt_aom_paeth_predictor_32x32;
    svt_aom_eb_pred[PAETH_PRED][TX_64X64] = svt_aom_paeth_predictor_64x64;

    svt_aom_eb_pred[PAETH_PRED][TX_4X8]  = svt_aom_paeth_predictor_4x8;
    svt_aom_eb_pred[PAETH_PRED][TX_4X16] = svt_aom_paeth_predictor_4x16;

    svt_aom_eb_pred[PAETH_PRED][TX_8X4]  = svt_aom_paeth_predictor_8x4;
    svt_aom_eb_pred[PAETH_PRED][TX_8X16] = svt_aom_paeth_predictor_8x16;
    svt_aom_eb_pred[PAETH_PRED][TX_8X32] = svt_aom_paeth_predictor_8x32;

    svt_aom_eb_pred[PAETH_PRED][TX_16X4]  = svt_aom_paeth_predictor_16x4;
    svt_aom_eb_pred[PAETH_PRED][TX_16X8]  = svt_aom_paeth_predictor_16x8;
    svt_aom_eb_pred[PAETH_PRED][TX_16X32] = svt_aom_paeth_predictor_16x32;
    svt_aom_eb_pred[PAETH_PRED][TX_16X64] = svt_aom_paeth_predictor_16x64;

    svt_aom_eb_pred[PAETH_PRED][TX_32X8]  = svt_aom_paeth_predictor_32x8;
    svt_aom_eb_pred[PAETH_PRED][TX_32X16] = svt_aom_paeth_predictor_32x16;
    svt_aom_eb_pred[PAETH_PRED][TX_32X64] = svt_aom_paeth_predictor_32x64;

    svt_aom_eb_pred[PAETH_PRED][TX_64X16] = svt_aom_paeth_predictor_64x16;
    svt_aom_eb_pred[PAETH_PRED][TX_64X32] = svt_aom_paeth_predictor_64x32;
    svt_aom_dc_pred[0][0][TX_4X4]         = svt_aom_dc_128_predictor_4x4;
    svt_aom_dc_pred[0][0][TX_8X8]         = svt_aom_dc_128_predictor_8x8;
    svt_aom_dc_pred[0][0][TX_16X16]       = svt_aom_dc_128_predictor_16x16;
    svt_aom_dc_pred[0][0][TX_32X32]       = svt_aom_dc_128_predictor_32x32;
    svt_aom_dc_pred[0][0][TX_64X64]       = svt_aom_dc_128_predictor_64x64;

    svt_aom_dc_pred[0][0][TX_4X8]  = svt_aom_dc_128_predictor_4x8;
    svt_aom_dc_pred[0][0][TX_4X16] = svt_aom_dc_128_predictor_4x16;

    svt_aom_dc_pred[0][0][TX_8X4]  = svt_aom_dc_128_predictor_8x4;
    svt_aom_dc_pred[0][0][TX_8X16] = svt_aom_dc_128_predictor_8x16;
    svt_aom_dc_pred[0][0][TX_8X32] = svt_aom_dc_128_predictor_8x32;

    svt_aom_dc_pred[0][0][TX_16X4]  = svt_aom_dc_128_predictor_16x4;
    svt_aom_dc_pred[0][0][TX_16X8]  = svt_aom_dc_128_predictor_16x8;
    svt_aom_dc_pred[0][0][TX_16X32] = svt_aom_dc_128_predictor_16x32;
    svt_aom_dc_pred[0][0][TX_16X64] = svt_aom_dc_128_predictor_16x64;

    svt_aom_dc_pred[0][0][TX_32X8]  = svt_aom_dc_128_predictor_32x8;
    svt_aom_dc_pred[0][0][TX_32X16] = svt_aom_dc_128_predictor_32x16;
    svt_aom_dc_pred[0][0][TX_32X64] = svt_aom_dc_128_predictor_32x64;

    svt_aom_dc_pred[0][0][TX_64X16] = svt_aom_dc_128_predictor_64x16;
    svt_aom_dc_pred[0][0][TX_64X32] = svt_aom_dc_128_predictor_64x32;

    svt_aom_dc_pred[0][1][TX_4X4]   = svt_aom_dc_top_predictor_4x4;
    svt_aom_dc_pred[0][1][TX_8X8]   = svt_aom_dc_top_predictor_8x8;
    svt_aom_dc_pred[0][1][TX_16X16] = svt_aom_dc_top_predictor_16x16;
    svt_aom_dc_pred[0][1][TX_32X32] = svt_aom_dc_top_predictor_32x32;
    svt_aom_dc_pred[0][1][TX_64X64] = svt_aom_dc_top_predictor_64x64;

    svt_aom_dc_pred[0][1][TX_4X8]  = svt_aom_dc_top_predictor_4x8;
    svt_aom_dc_pred[0][1][TX_4X16] = svt_aom_dc_top_predictor_4x16;

    svt_aom_dc_pred[0][1][TX_8X4]  = svt_aom_dc_top_predictor_8x4;
    svt_aom_dc_pred[0][1][TX_8X16] = svt_aom_dc_top_predictor_8x16;
    svt_aom_dc_pred[0][1][TX_8X32] = svt_aom_dc_top_predictor_8x32;

    svt_aom_dc_pred[0][1][TX_16X4]  = svt_aom_dc_top_predictor_16x4;
    svt_aom_dc_pred[0][1][TX_16X8]  = svt_aom_dc_top_predictor_16x8;
    svt_aom_dc_pred[0][1][TX_16X32] = svt_aom_dc_top_predictor_16x32;
    svt_aom_dc_pred[0][1][TX_16X64] = svt_aom_dc_top_predictor_16x64;

    svt_aom_dc_pred[0][1][TX_32X8]  = svt_aom_dc_top_predictor_32x8;
    svt_aom_dc_pred[0][1][TX_32X16] = svt_aom_dc_top_predictor_32x16;
    svt_aom_dc_pred[0][1][TX_32X64] = svt_aom_dc_top_predictor_32x64;

    svt_aom_dc_pred[0][1][TX_64X16] = svt_aom_dc_top_predictor_64x16;
    svt_aom_dc_pred[0][1][TX_64X32] = svt_aom_dc_top_predictor_64x32;

    svt_aom_dc_pred[1][0][TX_4X4]   = svt_aom_dc_left_predictor_4x4;
    svt_aom_dc_pred[1][0][TX_8X8]   = svt_aom_dc_left_predictor_8x8;
    svt_aom_dc_pred[1][0][TX_16X16] = svt_aom_dc_left_predictor_16x16;
    svt_aom_dc_pred[1][0][TX_32X32] = svt_aom_dc_left_predictor_32x32;
    svt_aom_dc_pred[1][0][TX_64X64] = svt_aom_dc_left_predictor_64x64;
    svt_aom_dc_pred[1][0][TX_4X8]   = svt_aom_dc_left_predictor_4x8;
    svt_aom_dc_pred[1][0][TX_4X16]  = svt_aom_dc_left_predictor_4x16;

    svt_aom_dc_pred[1][0][TX_8X4]  = svt_aom_dc_left_predictor_8x4;
    svt_aom_dc_pred[1][0][TX_8X16] = svt_aom_dc_left_predictor_8x16;
    svt_aom_dc_pred[1][0][TX_8X32] = svt_aom_dc_left_predictor_8x32;

    svt_aom_dc_pred[1][0][TX_16X4]  = svt_aom_dc_left_predictor_16x4;
    svt_aom_dc_pred[1][0][TX_16X8]  = svt_aom_dc_left_predictor_16x8;
    svt_aom_dc_pred[1][0][TX_16X32] = svt_aom_dc_left_predictor_16x32;
    svt_aom_dc_pred[1][0][TX_16X64] = svt_aom_dc_left_predictor_16x64;

    svt_aom_dc_pred[1][0][TX_32X8]  = svt_aom_dc_left_predictor_32x8;
    svt_aom_dc_pred[1][0][TX_32X16] = svt_aom_dc_left_predictor_32x16;
    svt_aom_dc_pred[1][0][TX_32X64] = svt_aom_dc_left_predictor_32x64;

    svt_aom_dc_pred[1][0][TX_64X16] = svt_aom_dc_left_predictor_64x16;
    svt_aom_dc_pred[1][0][TX_64X32] = svt_aom_dc_left_predictor_64x32;

    svt_aom_dc_pred[1][1][TX_4X4]   = svt_aom_dc_predictor_4x4;
    svt_aom_dc_pred[1][1][TX_8X8]   = svt_aom_dc_predictor_8x8;
    svt_aom_dc_pred[1][1][TX_16X16] = svt_aom_dc_predictor_16x16;
    svt_aom_dc_pred[1][1][TX_32X32] = svt_aom_dc_predictor_32x32;
    svt_aom_dc_pred[1][1][TX_64X64] = svt_aom_dc_predictor_64x64;
    svt_aom_dc_pred[1][1][TX_4X8]   = svt_aom_dc_predictor_4x8;
    svt_aom_dc_pred[1][1][TX_4X16]  = svt_aom_dc_predictor_4x16;

    svt_aom_dc_pred[1][1][TX_8X4]  = svt_aom_dc_predictor_8x4;
    svt_aom_dc_pred[1][1][TX_8X16] = svt_aom_dc_predictor_8x16;
    svt_aom_dc_pred[1][1][TX_8X32] = svt_aom_dc_predictor_8x32;

    svt_aom_dc_pred[1][1][TX_16X4]  = svt_aom_dc_predictor_16x4;
    svt_aom_dc_pred[1][1][TX_16X8]  = svt_aom_dc_predictor_16x8;
    svt_aom_dc_pred[1][1][TX_16X32] = svt_aom_dc_predictor_16x32;
    svt_aom_dc_pred[1][1][TX_16X64] = svt_aom_dc_predictor_16x64;

    svt_aom_dc_pred[1][1][TX_32X8]  = svt_aom_dc_predictor_32x8;
    svt_aom_dc_pred[1][1][TX_32X16] = svt_aom_dc_predictor_32x16;
    svt_aom_dc_pred[1][1][TX_32X64] = svt_aom_dc_predictor_32x64;

    svt_aom_dc_pred[1][1][TX_64X16] = svt_aom_dc_predictor_64x16;
    svt_aom_dc_pred[1][1][TX_64X32] = svt_aom_dc_predictor_64x32;

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
    svt_aom_pred_high[V_PRED][TX_4X4]   = svt_aom_highbd_v_predictor_4x4;
    svt_aom_pred_high[V_PRED][TX_8X8]   = svt_aom_highbd_v_predictor_8x8;
    svt_aom_pred_high[V_PRED][TX_16X16] = svt_aom_highbd_v_predictor_16x16;
    svt_aom_pred_high[V_PRED][TX_32X32] = svt_aom_highbd_v_predictor_32x32;
    svt_aom_pred_high[V_PRED][TX_64X64] = svt_aom_highbd_v_predictor_64x64;

    svt_aom_pred_high[V_PRED][TX_4X8]  = svt_aom_highbd_v_predictor_4x8;
    svt_aom_pred_high[V_PRED][TX_4X16] = svt_aom_highbd_v_predictor_4x16;

    svt_aom_pred_high[V_PRED][TX_8X4]  = svt_aom_highbd_v_predictor_8x4;
    svt_aom_pred_high[V_PRED][TX_8X16] = svt_aom_highbd_v_predictor_8x16;
    svt_aom_pred_high[V_PRED][TX_8X32] = svt_aom_highbd_v_predictor_8x32;

    svt_aom_pred_high[V_PRED][TX_16X4]  = svt_aom_highbd_v_predictor_16x4;
    svt_aom_pred_high[V_PRED][TX_16X8]  = svt_aom_highbd_v_predictor_16x8;
    svt_aom_pred_high[V_PRED][TX_16X32] = svt_aom_highbd_v_predictor_16x32;
    svt_aom_pred_high[V_PRED][TX_16X64] = svt_aom_highbd_v_predictor_16x64;

    svt_aom_pred_high[V_PRED][TX_32X8]  = svt_aom_highbd_v_predictor_32x8;
    svt_aom_pred_high[V_PRED][TX_32X16] = svt_aom_highbd_v_predictor_32x16;
    svt_aom_pred_high[V_PRED][TX_32X64] = svt_aom_highbd_v_predictor_32x64;

    svt_aom_pred_high[V_PRED][TX_64X16] = svt_aom_highbd_v_predictor_64x16;
    svt_aom_pred_high[V_PRED][TX_64X32] = svt_aom_highbd_v_predictor_64x32;

    svt_aom_pred_high[H_PRED][TX_4X4]   = svt_aom_highbd_h_predictor_4x4;
    svt_aom_pred_high[H_PRED][TX_8X8]   = svt_aom_highbd_h_predictor_8x8;
    svt_aom_pred_high[H_PRED][TX_16X16] = svt_aom_highbd_h_predictor_16x16;
    svt_aom_pred_high[H_PRED][TX_32X32] = svt_aom_highbd_h_predictor_32x32;
    svt_aom_pred_high[H_PRED][TX_64X64] = svt_aom_highbd_h_predictor_64x64;

    svt_aom_pred_high[H_PRED][TX_4X8]  = svt_aom_highbd_h_predictor_4x8;
    svt_aom_pred_high[H_PRED][TX_4X16] = svt_aom_highbd_h_predictor_4x16;

    svt_aom_pred_high[H_PRED][TX_8X4]  = svt_aom_highbd_h_predictor_8x4;
    svt_aom_pred_high[H_PRED][TX_8X16] = svt_aom_highbd_h_predictor_8x16;
    svt_aom_pred_high[H_PRED][TX_8X32] = svt_aom_highbd_h_predictor_8x32;

    svt_aom_pred_high[H_PRED][TX_16X4]  = svt_aom_highbd_h_predictor_16x4;
    svt_aom_pred_high[H_PRED][TX_16X8]  = svt_aom_highbd_h_predictor_16x8;
    svt_aom_pred_high[H_PRED][TX_16X32] = svt_aom_highbd_h_predictor_16x32;
    svt_aom_pred_high[H_PRED][TX_16X64] = svt_aom_highbd_h_predictor_16x64;

    svt_aom_pred_high[H_PRED][TX_32X8]  = svt_aom_highbd_h_predictor_32x8;
    svt_aom_pred_high[H_PRED][TX_32X16] = svt_aom_highbd_h_predictor_32x16;
    svt_aom_pred_high[H_PRED][TX_32X64] = svt_aom_highbd_h_predictor_32x64;

    svt_aom_pred_high[H_PRED][TX_64X16] = svt_aom_highbd_h_predictor_64x16;
    svt_aom_pred_high[H_PRED][TX_64X32] = svt_aom_highbd_h_predictor_64x32;

    svt_aom_pred_high[SMOOTH_PRED][TX_4X4]   = svt_aom_highbd_smooth_predictor_4x4;
    svt_aom_pred_high[SMOOTH_PRED][TX_8X8]   = svt_aom_highbd_smooth_predictor_8x8;
    svt_aom_pred_high[SMOOTH_PRED][TX_16X16] = svt_aom_highbd_smooth_predictor_16x16;
    svt_aom_pred_high[SMOOTH_PRED][TX_32X32] = svt_aom_highbd_smooth_predictor_32x32;
    svt_aom_pred_high[SMOOTH_PRED][TX_64X64] = svt_aom_highbd_smooth_predictor_64x64;

    svt_aom_pred_high[SMOOTH_PRED][TX_4X8]  = svt_aom_highbd_smooth_predictor_4x8;
    svt_aom_pred_high[SMOOTH_PRED][TX_4X16] = svt_aom_highbd_smooth_predictor_4x16;

    svt_aom_pred_high[SMOOTH_PRED][TX_8X4]  = svt_aom_highbd_smooth_predictor_8x4;
    svt_aom_pred_high[SMOOTH_PRED][TX_8X16] = svt_aom_highbd_smooth_predictor_8x16;
    svt_aom_pred_high[SMOOTH_PRED][TX_8X32] = svt_aom_highbd_smooth_predictor_8x32;

    svt_aom_pred_high[SMOOTH_PRED][TX_16X4]  = svt_aom_highbd_smooth_predictor_16x4;
    svt_aom_pred_high[SMOOTH_PRED][TX_16X8]  = svt_aom_highbd_smooth_predictor_16x8;
    svt_aom_pred_high[SMOOTH_PRED][TX_16X32] = svt_aom_highbd_smooth_predictor_16x32;
    svt_aom_pred_high[SMOOTH_PRED][TX_16X64] = svt_aom_highbd_smooth_predictor_16x64;

    svt_aom_pred_high[SMOOTH_PRED][TX_32X8]  = svt_aom_highbd_smooth_predictor_32x8;
    svt_aom_pred_high[SMOOTH_PRED][TX_32X16] = svt_aom_highbd_smooth_predictor_32x16;
    svt_aom_pred_high[SMOOTH_PRED][TX_32X64] = svt_aom_highbd_smooth_predictor_32x64;

    svt_aom_pred_high[SMOOTH_PRED][TX_64X16] = svt_aom_highbd_smooth_predictor_64x16;
    svt_aom_pred_high[SMOOTH_PRED][TX_64X32] = svt_aom_highbd_smooth_predictor_64x32;

    svt_aom_pred_high[SMOOTH_V_PRED][TX_4X4]   = svt_aom_highbd_smooth_v_predictor_4x4;
    svt_aom_pred_high[SMOOTH_V_PRED][TX_8X8]   = svt_aom_highbd_smooth_v_predictor_8x8;
    svt_aom_pred_high[SMOOTH_V_PRED][TX_16X16] = svt_aom_highbd_smooth_v_predictor_16x16;
    svt_aom_pred_high[SMOOTH_V_PRED][TX_32X32] = svt_aom_highbd_smooth_v_predictor_32x32;
    svt_aom_pred_high[SMOOTH_V_PRED][TX_64X64] = svt_aom_highbd_smooth_v_predictor_64x64;

    svt_aom_pred_high[SMOOTH_V_PRED][TX_4X8]  = svt_aom_highbd_smooth_v_predictor_4x8;
    svt_aom_pred_high[SMOOTH_V_PRED][TX_4X16] = svt_aom_highbd_smooth_v_predictor_4x16;

    svt_aom_pred_high[SMOOTH_V_PRED][TX_8X4]  = svt_aom_highbd_smooth_v_predictor_8x4;
    svt_aom_pred_high[SMOOTH_V_PRED][TX_8X16] = svt_aom_highbd_smooth_v_predictor_8x16;
    svt_aom_pred_high[SMOOTH_V_PRED][TX_8X32] = svt_aom_highbd_smooth_v_predictor_8x32;

    svt_aom_pred_high[SMOOTH_V_PRED][TX_16X4]  = svt_aom_highbd_smooth_v_predictor_16x4;
    svt_aom_pred_high[SMOOTH_V_PRED][TX_16X8]  = svt_aom_highbd_smooth_v_predictor_16x8;
    svt_aom_pred_high[SMOOTH_V_PRED][TX_16X32] = svt_aom_highbd_smooth_v_predictor_16x32;
    svt_aom_pred_high[SMOOTH_V_PRED][TX_16X64] = svt_aom_highbd_smooth_v_predictor_16x64;

    svt_aom_pred_high[SMOOTH_V_PRED][TX_32X8]  = svt_aom_highbd_smooth_v_predictor_32x8;
    svt_aom_pred_high[SMOOTH_V_PRED][TX_32X16] = svt_aom_highbd_smooth_v_predictor_32x16;
    svt_aom_pred_high[SMOOTH_V_PRED][TX_32X64] = svt_aom_highbd_smooth_v_predictor_32x64;

    svt_aom_pred_high[SMOOTH_V_PRED][TX_64X16] = svt_aom_highbd_smooth_v_predictor_64x16;
    svt_aom_pred_high[SMOOTH_V_PRED][TX_64X32] = svt_aom_highbd_smooth_v_predictor_64x32;

    svt_aom_pred_high[SMOOTH_H_PRED][TX_4X4]   = svt_aom_highbd_smooth_h_predictor_4x4;
    svt_aom_pred_high[SMOOTH_H_PRED][TX_8X8]   = svt_aom_highbd_smooth_h_predictor_8x8;
    svt_aom_pred_high[SMOOTH_H_PRED][TX_16X16] = svt_aom_highbd_smooth_h_predictor_16x16;
    svt_aom_pred_high[SMOOTH_H_PRED][TX_32X32] = svt_aom_highbd_smooth_h_predictor_32x32;
    svt_aom_pred_high[SMOOTH_H_PRED][TX_64X64] = svt_aom_highbd_smooth_h_predictor_64x64;

    svt_aom_pred_high[SMOOTH_H_PRED][TX_4X8]  = svt_aom_highbd_smooth_h_predictor_4x8;
    svt_aom_pred_high[SMOOTH_H_PRED][TX_4X16] = svt_aom_highbd_smooth_h_predictor_4x16;

    svt_aom_pred_high[SMOOTH_H_PRED][TX_8X4]  = svt_aom_highbd_smooth_h_predictor_8x4;
    svt_aom_pred_high[SMOOTH_H_PRED][TX_8X16] = svt_aom_highbd_smooth_h_predictor_8x16;
    svt_aom_pred_high[SMOOTH_H_PRED][TX_8X32] = svt_aom_highbd_smooth_h_predictor_8x32;

    svt_aom_pred_high[SMOOTH_H_PRED][TX_16X4]  = svt_aom_highbd_smooth_h_predictor_16x4;
    svt_aom_pred_high[SMOOTH_H_PRED][TX_16X8]  = svt_aom_highbd_smooth_h_predictor_16x8;
    svt_aom_pred_high[SMOOTH_H_PRED][TX_16X32] = svt_aom_highbd_smooth_h_predictor_16x32;
    svt_aom_pred_high[SMOOTH_H_PRED][TX_16X64] = svt_aom_highbd_smooth_h_predictor_16x64;

    svt_aom_pred_high[SMOOTH_H_PRED][TX_32X8]  = svt_aom_highbd_smooth_h_predictor_32x8;
    svt_aom_pred_high[SMOOTH_H_PRED][TX_32X16] = svt_aom_highbd_smooth_h_predictor_32x16;
    svt_aom_pred_high[SMOOTH_H_PRED][TX_32X64] = svt_aom_highbd_smooth_h_predictor_32x64;

    svt_aom_pred_high[SMOOTH_H_PRED][TX_64X16] = svt_aom_highbd_smooth_h_predictor_64x16;
    svt_aom_pred_high[SMOOTH_H_PRED][TX_64X32] = svt_aom_highbd_smooth_h_predictor_64x32;

    svt_aom_pred_high[PAETH_PRED][TX_4X4]   = svt_aom_highbd_paeth_predictor_4x4;
    svt_aom_pred_high[PAETH_PRED][TX_8X8]   = svt_aom_highbd_paeth_predictor_8x8;
    svt_aom_pred_high[PAETH_PRED][TX_16X16] = svt_aom_highbd_paeth_predictor_16x16;
    svt_aom_pred_high[PAETH_PRED][TX_32X32] = svt_aom_highbd_paeth_predictor_32x32;
    svt_aom_pred_high[PAETH_PRED][TX_64X64] = svt_aom_highbd_paeth_predictor_64x64;

    svt_aom_pred_high[PAETH_PRED][TX_4X8]  = svt_aom_highbd_paeth_predictor_4x8;
    svt_aom_pred_high[PAETH_PRED][TX_4X16] = svt_aom_highbd_paeth_predictor_4x16;

    svt_aom_pred_high[PAETH_PRED][TX_8X4]  = svt_aom_highbd_paeth_predictor_8x4;
    svt_aom_pred_high[PAETH_PRED][TX_8X16] = svt_aom_highbd_paeth_predictor_8x16;
    svt_aom_pred_high[PAETH_PRED][TX_8X32] = svt_aom_highbd_paeth_predictor_8x32;

    svt_aom_pred_high[PAETH_PRED][TX_16X4]  = svt_aom_highbd_paeth_predictor_16x4;
    svt_aom_pred_high[PAETH_PRED][TX_16X8]  = svt_aom_highbd_paeth_predictor_16x8;
    svt_aom_pred_high[PAETH_PRED][TX_16X32] = svt_aom_highbd_paeth_predictor_16x32;
    svt_aom_pred_high[PAETH_PRED][TX_16X64] = svt_aom_highbd_paeth_predictor_16x64;

    svt_aom_pred_high[PAETH_PRED][TX_32X8]  = svt_aom_highbd_paeth_predictor_32x8;
    svt_aom_pred_high[PAETH_PRED][TX_32X16] = svt_aom_highbd_paeth_predictor_32x16;
    svt_aom_pred_high[PAETH_PRED][TX_32X64] = svt_aom_highbd_paeth_predictor_32x64;

    svt_aom_pred_high[PAETH_PRED][TX_64X16] = svt_aom_highbd_paeth_predictor_64x16;
    svt_aom_pred_high[PAETH_PRED][TX_64X32] = svt_aom_highbd_paeth_predictor_64x32;
    svt_aom_dc_pred_high[0][0][TX_4X4]      = svt_aom_highbd_dc_128_predictor_4x4;
    svt_aom_dc_pred_high[0][0][TX_8X8]      = svt_aom_highbd_dc_128_predictor_8x8;
    svt_aom_dc_pred_high[0][0][TX_16X16]    = svt_aom_highbd_dc_128_predictor_16x16;
    svt_aom_dc_pred_high[0][0][TX_32X32]    = svt_aom_highbd_dc_128_predictor_32x32;
    svt_aom_dc_pred_high[0][0][TX_64X64]    = svt_aom_highbd_dc_128_predictor_64x64;

    svt_aom_dc_pred_high[0][0][TX_4X8]  = svt_aom_highbd_dc_128_predictor_4x8;
    svt_aom_dc_pred_high[0][0][TX_4X16] = svt_aom_highbd_dc_128_predictor_4x16;

    svt_aom_dc_pred_high[0][0][TX_8X4]  = svt_aom_highbd_dc_128_predictor_8x4;
    svt_aom_dc_pred_high[0][0][TX_8X16] = svt_aom_highbd_dc_128_predictor_8x16;
    svt_aom_dc_pred_high[0][0][TX_8X32] = svt_aom_highbd_dc_128_predictor_8x32;

    svt_aom_dc_pred_high[0][0][TX_16X4]  = svt_aom_highbd_dc_128_predictor_16x4;
    svt_aom_dc_pred_high[0][0][TX_16X8]  = svt_aom_highbd_dc_128_predictor_16x8;
    svt_aom_dc_pred_high[0][0][TX_16X32] = svt_aom_highbd_dc_128_predictor_16x32;
    svt_aom_dc_pred_high[0][0][TX_16X64] = svt_aom_highbd_dc_128_predictor_16x64;

    svt_aom_dc_pred_high[0][0][TX_32X8]  = svt_aom_highbd_dc_128_predictor_32x8;
    svt_aom_dc_pred_high[0][0][TX_32X16] = svt_aom_highbd_dc_128_predictor_32x16;
    svt_aom_dc_pred_high[0][0][TX_32X64] = svt_aom_highbd_dc_128_predictor_32x64;

    svt_aom_dc_pred_high[0][0][TX_64X16] = svt_aom_highbd_dc_128_predictor_64x16;
    svt_aom_dc_pred_high[0][0][TX_64X32] = svt_aom_highbd_dc_128_predictor_64x32;

    svt_aom_dc_pred_high[0][1][TX_4X4]   = svt_aom_highbd_dc_top_predictor_4x4;
    svt_aom_dc_pred_high[0][1][TX_8X8]   = svt_aom_highbd_dc_top_predictor_8x8;
    svt_aom_dc_pred_high[0][1][TX_16X16] = svt_aom_highbd_dc_top_predictor_16x16;
    svt_aom_dc_pred_high[0][1][TX_32X32] = svt_aom_highbd_dc_top_predictor_32x32;
    svt_aom_dc_pred_high[0][1][TX_64X64] = svt_aom_highbd_dc_top_predictor_64x64;

    svt_aom_dc_pred_high[0][1][TX_4X8]  = svt_aom_highbd_dc_top_predictor_4x8;
    svt_aom_dc_pred_high[0][1][TX_4X16] = svt_aom_highbd_dc_top_predictor_4x16;

    svt_aom_dc_pred_high[0][1][TX_8X4]  = svt_aom_highbd_dc_top_predictor_8x4;
    svt_aom_dc_pred_high[0][1][TX_8X16] = svt_aom_highbd_dc_top_predictor_8x16;
    svt_aom_dc_pred_high[0][1][TX_8X32] = svt_aom_highbd_dc_top_predictor_8x32;

    svt_aom_dc_pred_high[0][1][TX_16X4]  = svt_aom_highbd_dc_top_predictor_16x4;
    svt_aom_dc_pred_high[0][1][TX_16X8]  = svt_aom_highbd_dc_top_predictor_16x8;
    svt_aom_dc_pred_high[0][1][TX_16X32] = svt_aom_highbd_dc_top_predictor_16x32;
    svt_aom_dc_pred_high[0][1][TX_16X64] = svt_aom_highbd_dc_top_predictor_16x64;

    svt_aom_dc_pred_high[0][1][TX_32X8]  = svt_aom_highbd_dc_top_predictor_32x8;
    svt_aom_dc_pred_high[0][1][TX_32X16] = svt_aom_highbd_dc_top_predictor_32x16;
    svt_aom_dc_pred_high[0][1][TX_32X64] = svt_aom_highbd_dc_top_predictor_32x64;

    svt_aom_dc_pred_high[0][1][TX_64X16] = svt_aom_highbd_dc_top_predictor_64x16;
    svt_aom_dc_pred_high[0][1][TX_64X32] = svt_aom_highbd_dc_top_predictor_64x32;

    svt_aom_dc_pred_high[1][0][TX_4X4]   = svt_aom_highbd_dc_left_predictor_4x4;
    svt_aom_dc_pred_high[1][0][TX_8X8]   = svt_aom_highbd_dc_left_predictor_8x8;
    svt_aom_dc_pred_high[1][0][TX_16X16] = svt_aom_highbd_dc_left_predictor_16x16;
    svt_aom_dc_pred_high[1][0][TX_32X32] = svt_aom_highbd_dc_left_predictor_32x32;
    svt_aom_dc_pred_high[1][0][TX_64X64] = svt_aom_highbd_dc_left_predictor_64x64;

    svt_aom_dc_pred_high[1][0][TX_4X8]  = svt_aom_highbd_dc_left_predictor_4x8;
    svt_aom_dc_pred_high[1][0][TX_4X16] = svt_aom_highbd_dc_left_predictor_4x16;

    svt_aom_dc_pred_high[1][0][TX_8X4]  = svt_aom_highbd_dc_left_predictor_8x4;
    svt_aom_dc_pred_high[1][0][TX_8X16] = svt_aom_highbd_dc_left_predictor_8x16;
    svt_aom_dc_pred_high[1][0][TX_8X32] = svt_aom_highbd_dc_left_predictor_8x32;

    svt_aom_dc_pred_high[1][0][TX_16X4]  = svt_aom_highbd_dc_left_predictor_16x4;
    svt_aom_dc_pred_high[1][0][TX_16X8]  = svt_aom_highbd_dc_left_predictor_16x8;
    svt_aom_dc_pred_high[1][0][TX_16X32] = svt_aom_highbd_dc_left_predictor_16x32;
    svt_aom_dc_pred_high[1][0][TX_16X64] = svt_aom_highbd_dc_left_predictor_16x64;

    svt_aom_dc_pred_high[1][0][TX_32X8]  = svt_aom_highbd_dc_left_predictor_32x8;
    svt_aom_dc_pred_high[1][0][TX_32X16] = svt_aom_highbd_dc_left_predictor_32x16;
    svt_aom_dc_pred_high[1][0][TX_32X64] = svt_aom_highbd_dc_left_predictor_32x64;

    svt_aom_dc_pred_high[1][0][TX_64X16] = svt_aom_highbd_dc_left_predictor_64x16;
    svt_aom_dc_pred_high[1][0][TX_64X32] = svt_aom_highbd_dc_left_predictor_64x32;

    svt_aom_dc_pred_high[1][1][TX_4X4]   = svt_aom_highbd_dc_predictor_4x4;
    svt_aom_dc_pred_high[1][1][TX_8X8]   = svt_aom_highbd_dc_predictor_8x8;
    svt_aom_dc_pred_high[1][1][TX_16X16] = svt_aom_highbd_dc_predictor_16x16;
    svt_aom_dc_pred_high[1][1][TX_32X32] = svt_aom_highbd_dc_predictor_32x32;
    svt_aom_dc_pred_high[1][1][TX_64X64] = svt_aom_highbd_dc_predictor_64x64;

    svt_aom_dc_pred_high[1][1][TX_4X8]  = svt_aom_highbd_dc_predictor_4x8;
    svt_aom_dc_pred_high[1][1][TX_4X16] = svt_aom_highbd_dc_predictor_4x16;

    svt_aom_dc_pred_high[1][1][TX_8X4]  = svt_aom_highbd_dc_predictor_8x4;
    svt_aom_dc_pred_high[1][1][TX_8X16] = svt_aom_highbd_dc_predictor_8x16;
    svt_aom_dc_pred_high[1][1][TX_8X32] = svt_aom_highbd_dc_predictor_8x32;

    svt_aom_dc_pred_high[1][1][TX_16X4]  = svt_aom_highbd_dc_predictor_16x4;
    svt_aom_dc_pred_high[1][1][TX_16X8]  = svt_aom_highbd_dc_predictor_16x8;
    svt_aom_dc_pred_high[1][1][TX_16X32] = svt_aom_highbd_dc_predictor_16x32;
    svt_aom_dc_pred_high[1][1][TX_16X64] = svt_aom_highbd_dc_predictor_16x64;

    svt_aom_dc_pred_high[1][1][TX_32X8]  = svt_aom_highbd_dc_predictor_32x8;
    svt_aom_dc_pred_high[1][1][TX_32X16] = svt_aom_highbd_dc_predictor_32x16;
    svt_aom_dc_pred_high[1][1][TX_32X64] = svt_aom_highbd_dc_predictor_32x64;

    svt_aom_dc_pred_high[1][1][TX_64X16] = svt_aom_highbd_dc_predictor_64x16;
    svt_aom_dc_pred_high[1][1][TX_64X32] = svt_aom_highbd_dc_predictor_64x32;
#endif
}

void svt_aom_dr_predictor(uint8_t* dst, ptrdiff_t stride, TxSize tx_size, const uint8_t* above, const uint8_t* left,
                          int32_t upsample_above, int32_t upsample_left, int32_t angle) {
    const int32_t dx = get_dx(angle);
    const int32_t dy = get_dy(angle);
    const int32_t bw = tx_size_wide[tx_size];
    const int32_t bh = tx_size_high[tx_size];
    assert(angle > 0 && angle < 270);

    if (angle > 0 && angle < 90) {
        svt_av1_dr_prediction_z1(dst, stride, bw, bh, above, left, upsample_above, dx, dy);
    } else if (angle > 90 && angle < 180) {
        svt_av1_dr_prediction_z2(dst, stride, bw, bh, above, left, upsample_above, upsample_left, dx, dy);
    } else if (angle > 180 && angle < 270) {
        svt_av1_dr_prediction_z3(dst, stride, bw, bh, above, left, upsample_left, dx, dy);
    } else if (angle == 90) {
        svt_aom_eb_pred[V_PRED][tx_size](dst, stride, above, left);
    } else if (angle == 180) {
        svt_aom_eb_pred[H_PRED][tx_size](dst, stride, above, left);
    }
}

void filter_intra_edge_corner(uint8_t* p_above, uint8_t* p_left) {
    const int32_t kernel[3] = {5, 6, 5};

    int32_t s   = (p_left[0] * kernel[0]) + (p_above[-1] * kernel[1]) + (p_above[0] * kernel[2]);
    s           = (s + 8) >> 4;
    p_above[-1] = (uint8_t)s;
    p_left[-1]  = (uint8_t)s;
}

// Directional prediction, zone 1: 0 < angle < 90
#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_av1_highbd_dr_prediction_z1_c(uint16_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t* above,
                                       const uint16_t* left, int32_t upsample_above, int32_t dx, int32_t dy,
                                       int32_t bd) {
    (void)left;
    (void)dy;
    (void)bd;
    assert(dy == 1);
    assert(dx > 0);

    const int32_t max_base_x = ((bw + bh) - 1) << upsample_above;
    const int32_t frac_bits  = 6 - upsample_above;
    const int32_t base_inc   = 1 << upsample_above;
    for (int32_t r = 0, x = dx; r < bh; ++r, dst += stride, x += dx) {
        int32_t       base  = x >> frac_bits;
        const int32_t shift = ((x << upsample_above) & 0x3F) >> 1;

        if (base >= max_base_x) {
            for (int32_t i = r; i < bh; ++i) {
                svt_aom_memset16(dst, above[max_base_x], bw);
                dst += stride;
            }
            return;
        }

        for (int32_t c = 0; c < bw; ++c, base += base_inc) {
            if (base < max_base_x) {
                int32_t val = above[base] * (32 - shift) + above[base + 1] * shift;
                val         = ROUND_POWER_OF_TWO(val, 5);
                dst[c]      = (uint16_t)clip_pixel_highbd(val, bd);
            } else {
                dst[c] = above[max_base_x];
            }
        }
    }
}

// Directional prediction, zone 2: 90 < angle < 180
void svt_av1_highbd_dr_prediction_z2_c(uint16_t* dst, ptrdiff_t stride, int32_t bw, int32_t bh, const uint16_t* above,
                                       const uint16_t* left, int32_t upsample_above, int32_t upsample_left, int32_t dx,
                                       int32_t dy, int32_t bd) {
    (void)bd;
    assert(dx > 0);
    assert(dy > 0);

    const int32_t min_base_x  = -(1 << upsample_above);
    const int32_t frac_bits_x = 6 - upsample_above;
    const int32_t frac_bits_y = 6 - upsample_left;
    for (int32_t r = 0; r < bh; ++r, dst += stride) {
        for (int32_t c = 0; c < bw; ++c) {
            int32_t y    = r + 1;
            int32_t x    = (c << 6) - y * dx;
            int32_t base = x >> frac_bits_x;
            int32_t val;
            if (base >= min_base_x) {
                const int32_t shift = ((x * (1 << upsample_above)) & 0x3F) >> 1;
                val                 = above[base] * (32 - shift) + above[base + 1] * shift;
                val                 = ROUND_POWER_OF_TWO(val, 5);
            } else {
                x                   = c + 1;
                y                   = (r << 6) - x * dy;
                base                = y >> frac_bits_y;
                const int32_t shift = ((y * (1 << upsample_left)) & 0x3F) >> 1;
                val                 = left[base] * (32 - shift) + left[base + 1] * shift;
                val                 = ROUND_POWER_OF_TWO(val, 5);
            }
            dst[c] = clip_pixel_highbd(val, bd);
        }
    }
}

void svt_aom_highbd_dr_predictor(uint16_t* dst, ptrdiff_t stride, TxSize tx_size, const uint16_t* above,
                                 const uint16_t* left, int32_t upsample_above, int32_t upsample_left, int32_t angle,
                                 int32_t bd) {
    const int32_t dx = get_dx(angle);
    const int32_t dy = get_dy(angle);
    const int32_t bw = tx_size_wide[tx_size];
    const int32_t bh = tx_size_high[tx_size];
    assert(angle > 0 && angle < 270);

    if (angle > 0 && angle < 90) {
        svt_av1_highbd_dr_prediction_z1(dst, stride, bw, bh, above, left, upsample_above, dx, dy, bd);
    } else if (angle > 90 && angle < 180) {
        svt_av1_highbd_dr_prediction_z2(dst, stride, bw, bh, above, left, upsample_above, upsample_left, dx, dy, bd);
    } else if (angle > 180 && angle < 270) {
        svt_av1_highbd_dr_prediction_z3(dst, stride, bw, bh, above, left, upsample_left, dx, dy, bd);
    } else if (angle == 90) {
        svt_aom_pred_high[V_PRED][tx_size](dst, stride, above, left, bd);
    } else if (angle == 180) {
        svt_aom_pred_high[H_PRED][tx_size](dst, stride, above, left, bd);
    }
}

void svt_av1_filter_intra_edge_high_c(uint16_t* p, int32_t sz, int32_t strength) {
    if (!strength) {
        return;
    }

    const int32_t kernel[INTRA_EDGE_FILT][INTRA_EDGE_TAPS] = {{0, 4, 8, 4, 0}, {0, 5, 6, 5, 0}, {2, 4, 4, 4, 2}};
    const int32_t filt                                     = strength - 1;
    uint16_t      edge[129];

    svt_memcpy_c(edge, p, sz * sizeof(*p));
    for (int32_t i = 1; i < sz; i++) {
        int32_t s = 0;
        for (unsigned j = 0; j < INTRA_EDGE_TAPS; j++) {
            int32_t k = i - 2 + j;
            k         = (k < 0) ? 0 : k;
            k         = (k > sz - 1) ? sz - 1 : k;
            s += edge[k] * kernel[filt][j];
        }
        s    = (s + 8) >> 4;
        p[i] = (uint16_t)s;
    }
}

void filter_intra_edge_corner_high(uint16_t* p_above, uint16_t* p_left) {
    const int32_t kernel[3] = {5, 6, 5};

    int32_t s   = (p_left[0] * kernel[0]) + (p_above[-1] * kernel[1]) + (p_above[0] * kernel[2]);
    s           = (s + 8) >> 4;
    p_above[-1] = (uint16_t)s;
    p_left[-1]  = (uint16_t)s;
}
#endif

/*static INLINE*/ BlockSize svt_aom_scale_chroma_bsize(BlockSize bsize, int32_t subsampling_x, int32_t subsampling_y) {
    BlockSize bs = bsize;
    switch (bsize) {
    case BLOCK_4X4:
        if (subsampling_x == 1 && subsampling_y == 1) {
            bs = BLOCK_8X8;
        } else if (subsampling_x == 1) {
            bs = BLOCK_8X4;
        } else if (subsampling_y == 1) {
            bs = BLOCK_4X8;
        }
        break;
    case BLOCK_4X8:
        if (subsampling_x == 1 && subsampling_y == 1) {
            bs = BLOCK_8X8;
        } else if (subsampling_x == 1) {
            bs = BLOCK_8X8;
        } else if (subsampling_y == 1) {
            bs = BLOCK_4X8;
        }
        break;
    case BLOCK_8X4:
        if (subsampling_x == 1 && subsampling_y == 1) {
            bs = BLOCK_8X8;
        } else if (subsampling_x == 1) {
            bs = BLOCK_8X4;
        } else if (subsampling_y == 1) {
            bs = BLOCK_8X8;
        }
        break;
    case BLOCK_4X16:
        if (subsampling_x == 1 && subsampling_y == 1) {
            bs = BLOCK_8X16;
        } else if (subsampling_x == 1) {
            bs = BLOCK_8X16;
        } else if (subsampling_y == 1) {
            bs = BLOCK_4X16;
        }
        break;
    case BLOCK_16X4:
        if (subsampling_x == 1 && subsampling_y == 1) {
            bs = BLOCK_16X8;
        } else if (subsampling_x == 1) {
            bs = BLOCK_16X4;
        } else if (subsampling_y == 1) {
            bs = BLOCK_16X8;
        }
        break;
    default:
        break;
    }
    return bs;
}

////////////########...........Recurssive intra prediction starting...........#########

#if CONFIG_ENABLE_HIGH_BIT_DEPTH
void svt_aom_highbd_filter_intra_predictor(uint16_t* dst, ptrdiff_t stride, TxSize tx_size, const uint16_t* above,
                                           const uint16_t* left, int mode, int bd) {
    uint16_t  buffer[33][33];
    const int bw = tx_size_wide[tx_size];
    const int bh = tx_size_high[tx_size];

    assert(bw <= 32 && bh <= 32);

    // The initialization is just for silencing Jenkins static analysis warnings
    for (int r = 0; r < bh + 1; ++r) {
        memset(buffer[r], 0, (bw + 1) * sizeof(buffer[0][0]));
    }

    for (int r = 0; r < bh; ++r) {
        buffer[r + 1][0] = left[r];
    }
    svt_memcpy(buffer[0], &above[-1], (bw + 1) * sizeof(buffer[0][0]));

    for (int r = 1; r < bh + 1; r += 2) {
        for (int c = 1; c < bw + 1; c += 4) {
            const uint16_t p0 = buffer[r - 1][c - 1];
            const uint16_t p1 = buffer[r - 1][c];
            const uint16_t p2 = buffer[r - 1][c + 1];
            const uint16_t p3 = buffer[r - 1][c + 2];
            const uint16_t p4 = buffer[r - 1][c + 3];
            const uint16_t p5 = buffer[r][c - 1];
            const uint16_t p6 = buffer[r + 1][c - 1];
            for (unsigned k = 0; k < 8; ++k) {
                int r_offset                       = k >> 2;
                int c_offset                       = k & 0x03;
                buffer[r + r_offset][c + c_offset] = clip_pixel_highbd(
                    ROUND_POWER_OF_TWO_SIGNED(
                        eb_av1_filter_intra_taps[mode][k][0] * p0 + eb_av1_filter_intra_taps[mode][k][1] * p1 +
                            eb_av1_filter_intra_taps[mode][k][2] * p2 + eb_av1_filter_intra_taps[mode][k][3] * p3 +
                            eb_av1_filter_intra_taps[mode][k][4] * p4 + eb_av1_filter_intra_taps[mode][k][5] * p5 +
                            eb_av1_filter_intra_taps[mode][k][6] * p6,
                        FILTER_INTRA_SCALE_BITS),
                    bd);
            }
        }
    }

    for (int r = 0; r < bh; ++r) {
        svt_memcpy(dst, &buffer[r + 1][1], bw * sizeof(dst[0]));
        dst += stride;
    }
}
#endif
void svt_aom_filter_intra_edge(uint8_t mode, uint16_t max_frame_width, uint16_t max_frame_height, int32_t p_angle,
                               int32_t cu_origin_x, int32_t cu_origin_y, uint8_t* above_row, uint8_t* left_col) {
    const int mb_stride       = (max_frame_width + 15) / 16;
    const int mb_height       = (max_frame_height + 15) / 16;
    const int txwpx           = tx_size_wide[TX_16X16];
    const int txhpx           = tx_size_high[TX_16X16];
    const int need_right      = p_angle < 90;
    const int need_bottom     = p_angle > 180;
    int       need_left       = extend_modes[mode] & NEED_LEFT;
    int       need_above      = extend_modes[mode] & NEED_ABOVE;
    int       need_above_left = extend_modes[mode] & NEED_ABOVELEFT;
    //force to 0 for neighbor may not be ready at segment boundary
    // int ab_sm = 0; // (cu_origin_y > 0 && (ois_mb_results_ptr - mb_stride)) ? is_smooth_luma((ois_mb_results_ptr - mb_stride)->intra_mode) : 0;
    // int le_sm = 0; // (cu_origin_x > 0 && (ois_mb_results_ptr - 1)) ? is_smooth_luma((ois_mb_results_ptr - 1)->intra_mode) : 0;
    const int filt_type = 0; // (ab_sm || le_sm) ? 1 : 0
    int       n_top_px  = cu_origin_y > 0 ? AOMMIN(txwpx, (mb_stride * 16 - cu_origin_x + txwpx)) : 0;
    int       n_left_px = cu_origin_x > 0 ? AOMMIN(txhpx, (mb_height * 16 - cu_origin_y + txhpx)) : 0;

    if (av1_is_directional_mode((PredictionMode)mode)) {
        if (p_angle <= 90) {
            need_above = 1, need_left = 0, need_above_left = 1;
        } else if (p_angle < 180) {
            need_above = 1, need_left = 1, need_above_left = 1;
        } else {
            need_above = 0, need_left = 1, need_above_left = 1;
        }
    }

    if (p_angle != 90 && p_angle != 180) {
        const int ab_le = need_above_left ? 1 : 0;
        if (need_above && need_left && (txwpx + txhpx >= 24)) {
            filter_intra_edge_corner(above_row, left_col);
        }
        if (need_above && n_top_px > 0) {
            const int strength = svt_aom_intra_edge_filter_strength(txwpx, txhpx, p_angle - 90, filt_type);
            const int n_px     = n_top_px + ab_le + (need_right ? txhpx : 0);
            svt_av1_filter_intra_edge(above_row - ab_le, n_px, strength);
        }
        if (need_left && n_left_px > 0) {
            const int strength = svt_aom_intra_edge_filter_strength(txhpx, txwpx, p_angle - 180, filt_type);
            const int n_px     = n_left_px + ab_le + (need_bottom ? txwpx : 0);
            svt_av1_filter_intra_edge(left_col - ab_le, n_px, strength);
        }
    }
    int upsample_above = svt_aom_use_intra_edge_upsample(txwpx, txhpx, p_angle - 90, filt_type);
    if (need_above && upsample_above) {
        const int n_px = txwpx + (need_right ? txhpx : 0);
        svt_av1_upsample_intra_edge(above_row, n_px);
    }
    int upsample_left = svt_aom_use_intra_edge_upsample(txhpx, txwpx, p_angle - 180, filt_type);
    if (need_left && upsample_left) {
        const int n_px = txhpx + (need_bottom ? txwpx : 0);
        svt_av1_upsample_intra_edge(left_col, n_px);
    }
    return;
}

EbErrorType svt_aom_intra_prediction_open_loop_mb(int32_t p_angle, uint8_t ois_intra_mode, uint32_t src_origin_x,
                                                  uint32_t src_origin_y, TxSize tx_size, uint8_t* above_row,
                                                  uint8_t* left_col, uint8_t* dst, uint32_t dst_stride)

{
    EbErrorType    return_error = EB_ErrorNone;
    PredictionMode mode         = ois_intra_mode;
    const int32_t  is_dr_mode   = av1_is_directional_mode(mode);

    if (is_dr_mode) {
        svt_aom_dr_predictor(dst, dst_stride, tx_size, above_row, left_col, 0, 0, p_angle);
    } else {
        // predict
        if (mode == DC_PRED) {
            svt_aom_dc_pred[src_origin_x > 0][src_origin_y > 0][tx_size](dst, dst_stride, above_row, left_col);
        } else {
            svt_aom_eb_pred[mode][tx_size](dst, dst_stride, above_row, left_col);
        }
    }
    return return_error;
}
