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

#include "common_dsp_rtcd.h"
#include "definitions.h"

static inline int32x4_t k_means_multiply_add_neon(const int16x8_t a) {
    const int32x4_t l = vmull_s16(vget_low_s16(a), vget_low_s16(a));
    const int32x4_t h = vmull_s16(vget_high_s16(a), vget_high_s16(a));
    return vpaddq_s32(l, h);
}

void svt_av1_calc_indices_dim1_neon(const int* data, const int* centroids, uint8_t* indices, int n, int k) {
    int16x8_t cents[PALETTE_MAX_SIZE];

    int j = 0;
    do {
        cents[j] = vdupq_n_s16((int16_t)centroids[j]);
    } while (++j != k);

    do {
        const int16x8_t input0 = vcombine_s16(vmovn_s32(vld1q_s32(data)), vmovn_s32(vld1q_s32(data + 4)));
        const int16x8_t input1 = vcombine_s16(vmovn_s32(vld1q_s32(data + 8)), vmovn_s32(vld1q_s32(data + 12)));
        uint16x8_t      ind0   = vdupq_n_u16(0);
        uint16x8_t      ind1   = vdupq_n_u16(0);
        // Compute the distance to the first centroid.
        int16x8_t dist_min0 = vabdq_s16(input0, cents[0]);
        int16x8_t dist_min1 = vabdq_s16(input1, cents[0]);

        j = 1;
        do {
            // Compute the distance to the centroid.
            const int16x8_t dist0 = vabdq_s16(input0, cents[j]);
            const int16x8_t dist1 = vabdq_s16(input1, cents[j]);

            // Compare to the minimal one.
            const uint16x8_t cmp0 = vcgtq_s16(dist_min0, dist0);
            const uint16x8_t cmp1 = vcgtq_s16(dist_min1, dist1);

            dist_min0 = vminq_s16(dist_min0, dist0);
            dist_min1 = vminq_s16(dist_min1, dist1);

            const uint16x8_t index = vdupq_n_u16(j);
            ind0                   = vbslq_u16(cmp0, index, ind0);
            ind1                   = vbslq_u16(cmp1, index, ind1);
        } while (++j != k);

        vst1_u8(indices + 0, vmovn_u16(ind0));
        vst1_u8(indices + 8, vmovn_u16(ind1));

        indices += 16;
        data += 16;
        n -= 16;
    } while (n != 0);
}

void svt_av1_calc_indices_dim2_neon(const int* data, const int* centroids, uint8_t* indices, int n, int k) {
    int16x8_t cents[PALETTE_MAX_SIZE];

    int j = 0;
    do {
        int32x4_t cxcy_s32 = vreinterpretq_s32_u64(vld1q_dup_u64((const uint64_t*)&centroids[2 * j]));
        cents[j]           = vcombine_s16(vmovn_s32(cxcy_s32), vmovn_s32(cxcy_s32));
    } while (++j != k);

    do {
        const int16x8_t input0 = vcombine_s16(vmovn_s32(vld1q_s32(data)), vmovn_s32(vld1q_s32(data + 4)));
        const int16x8_t input1 = vcombine_s16(vmovn_s32(vld1q_s32(data + 8)), vmovn_s32(vld1q_s32(data + 12)));
        uint32x4_t      ind0   = vdupq_n_u32(0);
        uint32x4_t      ind1   = vdupq_n_u32(0);

        // Compute the distance to the first centroid.
        int16x8_t d0        = vsubq_s16(input0, cents[0]);
        int16x8_t d1        = vsubq_s16(input1, cents[0]);
        int32x4_t dist_min0 = k_means_multiply_add_neon(d0);
        int32x4_t dist_min1 = k_means_multiply_add_neon(d1);

        j = 1;
        do {
            // Compute the distance to the centroid.
            d0 = vsubq_s16(input0, cents[j]);
            d1 = vsubq_s16(input1, cents[j]);

            const int32x4_t dist0 = k_means_multiply_add_neon(d0);
            const int32x4_t dist1 = k_means_multiply_add_neon(d1);

            // Compare to the minimal one.
            const uint32x4_t cmp0 = vcgtq_s32(dist_min0, dist0);
            const uint32x4_t cmp1 = vcgtq_s32(dist_min1, dist1);

            dist_min0 = vminq_s32(dist_min0, dist0);
            dist_min1 = vminq_s32(dist_min1, dist1);

            const uint32x4_t index = vdupq_n_u32(j);
            ind0                   = vbslq_u32(cmp0, index, ind0);
            ind1                   = vbslq_u32(cmp1, index, ind1);
        } while (++j != k);

        // Cast to 8 bit and store.
        vst1_u8(indices, vmovn_u16(vcombine_u16(vmovn_u32(ind0), vmovn_u32(ind1))));

        data += 16;
        indices += 8;
        n -= 8;
    } while (n != 0);
}
