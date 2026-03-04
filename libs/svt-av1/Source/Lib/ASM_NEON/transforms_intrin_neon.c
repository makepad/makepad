/*
 * Copyright(c) 2024 Intel Corporation
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#include "definitions.h"
#include "motion_estimation.h"

#include <arm_neon.h>

static inline void energy_computation_kernel_neon(const int32_t* const in, int64x2_t* const sum0,
                                                  int64x2_t* const sum1) {
    const int32x4_t input0 = vld1q_s32(in);
    const int32x4_t input1 = vld1q_s32(in + 4);
    *sum0                  = vmlal_s32(*sum0, vget_low_s32(input0), vget_low_s32(input0));
    *sum0                  = vmlal_s32(*sum0, vget_high_s32(input0), vget_high_s32(input0));
    *sum1                  = vmlal_s32(*sum1, vget_low_s32(input1), vget_low_s32(input1));
    *sum1                  = vmlal_s32(*sum1, vget_high_s32(input1), vget_high_s32(input1));
}

static inline uint64_t energy_computation_wxh_neon(const int32_t* in, const int stride, uint32_t width,
                                                   uint32_t height) {
    int64x2_t sum0 = vdupq_n_s64(0);
    int64x2_t sum1 = vdupq_n_s64(0);

    do {
        int            w      = width;
        const int32_t* in_ptr = in;
        do {
            energy_computation_kernel_neon(in_ptr + 0 * 8, &sum0, &sum1);
            energy_computation_kernel_neon(in_ptr + 1 * 8, &sum0, &sum1);

            in_ptr += 16;
            w -= 16;
        } while (w != 0);
        in += stride;
    } while (--height != 0);

    return vaddvq_s64(vaddq_s64(sum0, sum1));
}

uint64_t svt_handle_transform16x64_neon(int32_t* output) {
    //bottom 16x32 area.
    const uint64_t three_quad_energy = energy_computation_wxh_neon(output + 16 * 32, 16, 16, 32);
    return three_quad_energy;
}

uint64_t svt_handle_transform32x64_neon(int32_t* output) {
    //bottom 32x32 area.
    const uint64_t three_quad_energy = energy_computation_wxh_neon(output + 32 * 32, 32, 32, 32);
    return three_quad_energy;
}

uint64_t svt_handle_transform64x16_neon(int32_t* output) {
    // top - right 32x16 area.
    const uint64_t three_quad_energy = energy_computation_wxh_neon(output + 32, 64, 32, 16);
    // Re-pack non-zero coeffs in the first 32x16 indices.
    for (int32_t row = 1; row < 16; ++row) {
        memcpy(output + row * 32, output + row * 64, 32 * sizeof(*output));
    }

    return three_quad_energy;
}

uint64_t svt_handle_transform64x32_neon(int32_t* output) {
    // top - right 32x32 area.
    const uint64_t three_quad_energy = energy_computation_wxh_neon(output + 32, 64, 32, 32);
    // Re-pack non-zero coeffs in the first 32x32 indices.
    for (int32_t row = 1; row < 32; ++row) {
        memcpy(output + row * 32, output + row * 64, 32 * sizeof(*output));
    }

    return three_quad_energy;
}

uint64_t svt_handle_transform64x64_neon(int32_t* output) {
    uint64_t three_quad_energy;

    // top - right 32x32 area.
    three_quad_energy = energy_computation_wxh_neon(output + 32, 64, 32, 32);
    //bottom 64x32 area.
    three_quad_energy += energy_computation_wxh_neon(output + 32 * 64, 64, 64, 32);
    // Re-pack non-zero coeffs in the first 32x32 indices.
    for (int32_t row = 1; row < 32; ++row) {
        memcpy(output + row * 32, output + row * 64, 32 * sizeof(*output));
    }

    return three_quad_energy;
}

uint64_t svt_handle_transform16x64_N2_N4_neon(int32_t* output) {
    (void)output;
    return 0;
}

uint64_t svt_handle_transform32x64_N2_N4_neon(int32_t* output) {
    (void)output;
    return 0;
}

uint64_t svt_handle_transform64x16_N2_N4_neon(int32_t* output) {
    // Re-pack non-zero coeffs in the first 32x16 indices.
    for (int32_t row = 1; row < 16; ++row) {
        memcpy(output + row * 32, output + row * 64, 32 * sizeof(*output));
    }
    return 0;
}

uint64_t svt_handle_transform64x32_N2_N4_neon(int32_t* output) {
    // Re-pack non-zero coeffs in the first 32x32 indices.
    for (int32_t row = 1; row < 32; ++row) {
        memcpy(output + row * 32, output + row * 64, 32 * sizeof(*output));
    }
    return 0;
}

uint64_t svt_handle_transform64x64_N2_N4_neon(int32_t* output) {
    // Re-pack non-zero coeffs in the first 32x32 indices.
    for (int32_t row = 1; row < 32; ++row) {
        memcpy(output + row * 32, output + row * 64, 32 * sizeof(*output));
    }
    return 0;
}
