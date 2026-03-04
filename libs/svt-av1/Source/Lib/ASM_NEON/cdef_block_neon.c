/*
 * Copyright (c) 2024, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */
#include <arm_neon.h>

#include "definitions.h"
#include "sum_neon.h"

#if __GNUC__
#define svt_clzll(id, x) id = (unsigned long)__builtin_clzll(x)
#elif defined(_MSC_VER)
#include <intrin.h>

#define svt_clzll(id, x) _BitScanReverse64(&id, x)
#endif

void svt_aom_copy_rect8_8bit_to_16bit_neon(uint16_t* dst, int32_t dstride, const uint8_t* src, int32_t sstride,
                                           int32_t v, int32_t h) {
    int32_t i, j;
    for (i = 0; i < v; i++) {
        for (j = 0; j < (h & ~0x7); j += 8) {
            const uint8x8_t row = vld1_u8(&src[i * sstride + j]);
            vst1q_u16(&dst[i * dstride + j], vmovl_u8(row));
        }
        for (; j < h; j++) {
            dst[i * dstride + j] = src[i * sstride + j];
        }
    }
}

// partial A is a 16-bit vector of the form:
// [x8 x7 x6 x5 x4 x3 x2 x1] and partial B has the form:
// [0  y1 y2 y3 y4 y5 y6 y7].
// This function computes (x1^2+y1^2)*C1 + (x2^2+y2^2)*C2 + ...
// (x7^2+y2^7)*C7 + (x8^2+0^2)*C8 where the C1..C8 constants are in const1
// and const2.
static inline uint32x4_t fold_mul_and_sum_neon(int16x8_t partiala, int16x8_t partialb, uint32x4_t const1,
                                               uint32x4_t const2) {
    // Reverse partial B.
    // pattern = { 12 13 10 11 8 9 6 7 4 5 2 3 0 1 14 15 }.
    uint8x16_t pattern = vreinterpretq_u8_u64(vcombine_u64(vcreate_u64((uint64_t)0x07060908 << 32 | 0x0b0a0d0c),
                                                           vcreate_u64((uint64_t)0x0f0e0100 << 32 | 0x03020504)));

    partialb = vreinterpretq_s16_s8(vqtbl1q_s8(vreinterpretq_s8_s16(partialb), pattern));

    // Square and add the corresponding x and y values.
    int32x4_t cost_lo = vmull_s16(vget_low_s16(partiala), vget_low_s16(partiala));
    cost_lo           = vmlal_s16(cost_lo, vget_low_s16(partialb), vget_low_s16(partialb));
    int32x4_t cost_hi = vmull_s16(vget_high_s16(partiala), vget_high_s16(partiala));
    cost_hi           = vmlal_s16(cost_hi, vget_high_s16(partialb), vget_high_s16(partialb));

    // Multiply by constant.
    uint32x4_t cost = vmulq_u32(vreinterpretq_u32_s32(cost_lo), const1);
    cost            = vmlaq_u32(cost, vreinterpretq_u32_s32(cost_hi), const2);
    return cost;
}

// This function computes the cost along directions 4, 5, 6, 7. (4 is diagonal
// down-right, 6 is vertical).
//
// For each direction the lines are shifted so that we can perform a
// basic sum on each vector element. For example, direction 5 is "south by
// southeast", so we need to add the pixels along each line i below:
//
// 0  1 2 3 4 5 6 7
// 0  1 2 3 4 5 6 7
// 8  0 1 2 3 4 5 6
// 8  0 1 2 3 4 5 6
// 9  8 0 1 2 3 4 5
// 9  8 0 1 2 3 4 5
// 10 9 8 0 1 2 3 4
// 10 9 8 0 1 2 3 4
//
// For this to fit nicely in vectors, the lines need to be shifted like so:
//        0 1 2 3 4 5 6 7
//        0 1 2 3 4 5 6 7
//      8 0 1 2 3 4 5 6
//      8 0 1 2 3 4 5 6
//    9 8 0 1 2 3 4 5
//    9 8 0 1 2 3 4 5
// 10 9 8 0 1 2 3 4
// 10 9 8 0 1 2 3 4
//
// In this configuration we can now perform SIMD additions to get the cost
// along direction 5. Since this won't fit into a single 128-bit vector, we use
// two of them to compute each half of the new configuration, and pad the empty
// spaces with zeros. Similar shifting is done for other directions, except
// direction 6 which is straightforward as it's the vertical direction.
static inline uint32x4_t compute_vert_directions_neon(int16x8_t lines[8], uint32_t cost[4]) {
    const int16x8_t zero = vdupq_n_s16(0);

    // Partial sums for lines 0 and 1.
    int16x8_t partial4a = vextq_s16(zero, lines[0], 1);
    partial4a           = vaddq_s16(partial4a, vextq_s16(zero, lines[1], 2));
    int16x8_t partial4b = vextq_s16(lines[0], zero, 1);
    partial4b           = vaddq_s16(partial4b, vextq_s16(lines[1], zero, 2));
    int16x8_t tmp       = vaddq_s16(lines[0], lines[1]);
    int16x8_t partial5a = vextq_s16(zero, tmp, 3);
    int16x8_t partial5b = vextq_s16(tmp, zero, 3);
    int16x8_t partial7a = vextq_s16(zero, tmp, 6);
    int16x8_t partial7b = vextq_s16(tmp, zero, 6);
    int16x8_t partial6  = tmp;

    // Partial sums for lines 2 and 3.
    partial4a = vaddq_s16(partial4a, vextq_s16(zero, lines[2], 3));
    partial4a = vaddq_s16(partial4a, vextq_s16(zero, lines[3], 4));
    partial4b = vaddq_s16(partial4b, vextq_s16(lines[2], zero, 3));
    partial4b = vaddq_s16(partial4b, vextq_s16(lines[3], zero, 4));
    tmp       = vaddq_s16(lines[2], lines[3]);
    partial5a = vaddq_s16(partial5a, vextq_s16(zero, tmp, 4));
    partial5b = vaddq_s16(partial5b, vextq_s16(tmp, zero, 4));
    partial7a = vaddq_s16(partial7a, vextq_s16(zero, tmp, 5));
    partial7b = vaddq_s16(partial7b, vextq_s16(tmp, zero, 5));
    partial6  = vaddq_s16(partial6, tmp);

    // Partial sums for lines 4 and 5.
    partial4a = vaddq_s16(partial4a, vextq_s16(zero, lines[4], 5));
    partial4a = vaddq_s16(partial4a, vextq_s16(zero, lines[5], 6));
    partial4b = vaddq_s16(partial4b, vextq_s16(lines[4], zero, 5));
    partial4b = vaddq_s16(partial4b, vextq_s16(lines[5], zero, 6));
    tmp       = vaddq_s16(lines[4], lines[5]);
    partial5a = vaddq_s16(partial5a, vextq_s16(zero, tmp, 5));
    partial5b = vaddq_s16(partial5b, vextq_s16(tmp, zero, 5));
    partial7a = vaddq_s16(partial7a, vextq_s16(zero, tmp, 4));
    partial7b = vaddq_s16(partial7b, vextq_s16(tmp, zero, 4));
    partial6  = vaddq_s16(partial6, tmp);

    // Partial sums for lines 6 and 7.
    partial4a = vaddq_s16(partial4a, vextq_s16(zero, lines[6], 7));
    partial4a = vaddq_s16(partial4a, lines[7]);
    partial4b = vaddq_s16(partial4b, vextq_s16(lines[6], zero, 7));
    tmp       = vaddq_s16(lines[6], lines[7]);
    partial5a = vaddq_s16(partial5a, vextq_s16(zero, tmp, 6));
    partial5b = vaddq_s16(partial5b, vextq_s16(tmp, zero, 6));
    partial7a = vaddq_s16(partial7a, vextq_s16(zero, tmp, 3));
    partial7b = vaddq_s16(partial7b, vextq_s16(tmp, zero, 3));
    partial6  = vaddq_s16(partial6, tmp);

    uint32x4_t const0 = vreinterpretq_u32_u64(
        vcombine_u64(vcreate_u64((uint64_t)420 << 32 | 840), vcreate_u64((uint64_t)210 << 32 | 280)));
    uint32x4_t const1 = vreinterpretq_u32_u64(
        vcombine_u64(vcreate_u64((uint64_t)140 << 32 | 168), vcreate_u64((uint64_t)105 << 32 | 120)));
    uint32x4_t const2 = vreinterpretq_u32_u64(vcombine_u64(vcreate_u64(0), vcreate_u64((uint64_t)210 << 32 | 420)));
    uint32x4_t const3 = vreinterpretq_u32_u64(
        vcombine_u64(vcreate_u64((uint64_t)105 << 32 | 140), vcreate_u64((uint64_t)105 << 32 | 105)));

    // Compute costs in terms of partial sums.
    int32x4_t partial6_s32 = vmull_s16(vget_low_s16(partial6), vget_low_s16(partial6));
    partial6_s32           = vmlal_s16(partial6_s32, vget_high_s16(partial6), vget_high_s16(partial6));

    uint32x4_t costs[4];
    costs[0] = fold_mul_and_sum_neon(partial4a, partial4b, const0, const1);
    costs[1] = fold_mul_and_sum_neon(partial5a, partial5b, const2, const3);
    costs[2] = vmulq_n_u32(vreinterpretq_u32_s32(partial6_s32), 105);
    costs[3] = fold_mul_and_sum_neon(partial7a, partial7b, const2, const3);

    costs[0] = horizontal_add_4d_u32x4(costs);
    vst1q_u32(cost, costs[0]);
    return costs[0];
}

static inline uint32x4_t fold_mul_and_sum_pairwise_neon(int16x8_t partiala, int16x8_t partialb, int16x8_t partialc,
                                                        uint32x4_t const0) {
    // Reverse partial c.
    // pattern = { 10 11 8 9 6 7 4 5 2 3 0 1 12 13 14 15 }.
    uint8x16_t pattern = vreinterpretq_u8_u64(vcombine_u64(vcreate_u64((uint64_t)0x05040706 << 32 | 0x09080b0a),
                                                           vcreate_u64((uint64_t)0x0f0e0d0c << 32 | 0x01000302)));

    partialc = vreinterpretq_s16_s8(vqtbl1q_s8(vreinterpretq_s8_s16(partialc), pattern));

    int32x4_t partiala_s32 = vpaddlq_s16(partiala);
    int32x4_t partialb_s32 = vpaddlq_s16(partialb);
    int32x4_t partialc_s32 = vpaddlq_s16(partialc);

    partiala_s32 = vmulq_s32(partiala_s32, partiala_s32);
    partialb_s32 = vmulq_s32(partialb_s32, partialb_s32);
    partialc_s32 = vmulq_s32(partialc_s32, partialc_s32);

    partiala_s32 = vaddq_s32(partiala_s32, partialc_s32);

    uint32x4_t cost = vmulq_n_u32(vreinterpretq_u32_s32(partialb_s32), 105);
    cost            = vmlaq_u32(cost, vreinterpretq_u32_s32(partiala_s32), const0);
    return cost;
}

// This function computes the cost along directions 0, 1, 2, 3. (0 means
// 45-degree up-right, 2 is horizontal).
//
// For direction 1 and 3 ("east northeast" and "east southeast") the shifted
// lines need three vectors instead of two. For direction 1 for example, we need
// to compute the sums along the line i below:
// 0 0 1 1 2 2 3  3
// 1 1 2 2 3 3 4  4
// 2 2 3 3 4 4 5  5
// 3 3 4 4 5 5 6  6
// 4 4 5 5 6 6 7  7
// 5 5 6 6 7 7 8  8
// 6 6 7 7 8 8 9  9
// 7 7 8 8 9 9 10 10
//
// Which means we need the following configuration:
// 0 0 1 1 2 2 3 3
//     1 1 2 2 3 3 4 4
//         2 2 3 3 4 4 5 5
//             3 3 4 4 5 5 6 6
//                 4 4 5 5 6 6 7 7
//                     5 5 6 6 7 7 8 8
//                         6 6 7 7 8 8 9 9
//                             7 7 8 8 9 9 10 10
//
// Three vectors are needed to compute this, as well as some extra pairwise
// additions.
static inline uint32x4_t compute_horiz_directions_neon(int16x8_t lines[8], uint32_t cost[4]) {
    const int16x8_t zero = vdupq_n_s16(0);

    // Compute diagonal directions (1, 2, 3).
    // Partial sums for lines 0 and 1.
    int16x8_t partial0a = lines[0];
    partial0a           = vaddq_s16(partial0a, vextq_s16(zero, lines[1], 7));
    int16x8_t partial0b = vextq_s16(lines[1], zero, 7);
    int16x8_t partial1a = vaddq_s16(lines[0], vextq_s16(zero, lines[1], 6));
    int16x8_t partial1b = vextq_s16(lines[1], zero, 6);
    int16x8_t partial3a = vextq_s16(lines[0], zero, 2);
    partial3a           = vaddq_s16(partial3a, vextq_s16(lines[1], zero, 4));
    int16x8_t partial3b = vextq_s16(zero, lines[0], 2);
    partial3b           = vaddq_s16(partial3b, vextq_s16(zero, lines[1], 4));

    // Partial sums for lines 2 and 3.
    partial0a = vaddq_s16(partial0a, vextq_s16(zero, lines[2], 6));
    partial0a = vaddq_s16(partial0a, vextq_s16(zero, lines[3], 5));
    partial0b = vaddq_s16(partial0b, vextq_s16(lines[2], zero, 6));
    partial0b = vaddq_s16(partial0b, vextq_s16(lines[3], zero, 5));
    partial1a = vaddq_s16(partial1a, vextq_s16(zero, lines[2], 4));
    partial1a = vaddq_s16(partial1a, vextq_s16(zero, lines[3], 2));
    partial1b = vaddq_s16(partial1b, vextq_s16(lines[2], zero, 4));
    partial1b = vaddq_s16(partial1b, vextq_s16(lines[3], zero, 2));
    partial3a = vaddq_s16(partial3a, vextq_s16(lines[2], zero, 6));
    partial3b = vaddq_s16(partial3b, vextq_s16(zero, lines[2], 6));
    partial3b = vaddq_s16(partial3b, lines[3]);

    // Partial sums for lines 4 and 5.
    partial0a           = vaddq_s16(partial0a, vextq_s16(zero, lines[4], 4));
    partial0a           = vaddq_s16(partial0a, vextq_s16(zero, lines[5], 3));
    partial0b           = vaddq_s16(partial0b, vextq_s16(lines[4], zero, 4));
    partial0b           = vaddq_s16(partial0b, vextq_s16(lines[5], zero, 3));
    partial1b           = vaddq_s16(partial1b, lines[4]);
    partial1b           = vaddq_s16(partial1b, vextq_s16(zero, lines[5], 6));
    int16x8_t partial1c = vextq_s16(lines[5], zero, 6);
    partial3b           = vaddq_s16(partial3b, vextq_s16(lines[4], zero, 2));
    partial3b           = vaddq_s16(partial3b, vextq_s16(lines[5], zero, 4));
    int16x8_t partial3c = vextq_s16(zero, lines[4], 2);
    partial3c           = vaddq_s16(partial3c, vextq_s16(zero, lines[5], 4));

    // Partial sums for lines 6 and 7.
    partial0a = vaddq_s16(partial0a, vextq_s16(zero, lines[6], 2));
    partial0a = vaddq_s16(partial0a, vextq_s16(zero, lines[7], 1));
    partial0b = vaddq_s16(partial0b, vextq_s16(lines[6], zero, 2));
    partial0b = vaddq_s16(partial0b, vextq_s16(lines[7], zero, 1));
    partial1b = vaddq_s16(partial1b, vextq_s16(zero, lines[6], 4));
    partial1b = vaddq_s16(partial1b, vextq_s16(zero, lines[7], 2));
    partial1c = vaddq_s16(partial1c, vextq_s16(lines[6], zero, 4));
    partial1c = vaddq_s16(partial1c, vextq_s16(lines[7], zero, 2));
    partial3b = vaddq_s16(partial3b, vextq_s16(lines[6], zero, 6));
    partial3c = vaddq_s16(partial3c, vextq_s16(zero, lines[6], 6));
    partial3c = vaddq_s16(partial3c, lines[7]);

    // Special case for direction 2 as it's just a sum along each line.
    int16x8_t lines03[4] = {lines[0], lines[1], lines[2], lines[3]};
    int16x8_t lines47[4] = {lines[4], lines[5], lines[6], lines[7]};
    int32x4_t partial2a  = horizontal_add_4d_s16x8(lines03);
    int32x4_t partial2b  = horizontal_add_4d_s16x8(lines47);

    uint32x4_t partial2a_u32 = vreinterpretq_u32_s32(vmulq_s32(partial2a, partial2a));
    uint32x4_t partial2b_u32 = vreinterpretq_u32_s32(vmulq_s32(partial2b, partial2b));

    uint32x4_t const0 = vreinterpretq_u32_u64(
        vcombine_u64(vcreate_u64((uint64_t)420 << 32 | 840), vcreate_u64((uint64_t)210 << 32 | 280)));
    uint32x4_t const1 = vreinterpretq_u32_u64(
        vcombine_u64(vcreate_u64((uint64_t)140 << 32 | 168), vcreate_u64((uint64_t)105 << 32 | 120)));
    uint32x4_t const2 = vreinterpretq_u32_u64(
        vcombine_u64(vcreate_u64((uint64_t)210 << 32 | 420), vcreate_u64((uint64_t)105 << 32 | 140)));

    uint32x4_t costs[4];
    costs[0] = fold_mul_and_sum_neon(partial0a, partial0b, const0, const1);
    costs[1] = fold_mul_and_sum_pairwise_neon(partial1a, partial1b, partial1c, const2);
    costs[2] = vaddq_u32(partial2a_u32, partial2b_u32);
    costs[2] = vmulq_n_u32(costs[2], 105);
    costs[3] = fold_mul_and_sum_pairwise_neon(partial3c, partial3b, partial3a, const2);

    costs[0] = horizontal_add_4d_u32x4(costs);
    vst1q_u32(cost, costs[0]);
    return costs[0];
}

uint8_t svt_aom_cdef_find_dir_neon(const uint16_t* img, int32_t stride, int32_t* var, int32_t coeff_shift) {
    uint32_t  cost[8];
    uint32_t  best_cost = 0;
    int       best_dir  = 0;
    int16x8_t lines[8];
    for (int i = 0; i < 8; i++) {
        uint16x8_t s = vld1q_u16(&img[i * stride]);
        lines[i]     = vreinterpretq_s16_u16(vsubq_u16(vshlq_u16(s, vdupq_n_s16(-coeff_shift)), vdupq_n_u16(128)));
    }

    // Compute "mostly vertical" directions.
    uint32x4_t cost47 = compute_vert_directions_neon(lines, cost + 4);

    // Compute "mostly horizontal" directions.
    uint32x4_t cost03 = compute_horiz_directions_neon(lines, cost);

    // Find max cost as well as its index to get best_dir.
    // The max cost needs to be propagated in the whole vector to find its
    // position in the original cost vectors cost03 and cost47.
    uint32x4_t cost07     = vmaxq_u32(cost03, cost47);
    best_cost             = vmaxvq_u32(cost07);
    uint32x4_t   max_cost = vdupq_n_u32(best_cost);
    uint8x16x2_t costs    = {
        {vreinterpretq_u8_u32(vceqq_u32(max_cost, cost03)), vreinterpretq_u8_u32(vceqq_u32(max_cost, cost47))}};
    // idx = { 28, 24, 20, 16, 12, 8, 4, 0 };
    uint8x8_t idx = vreinterpret_u8_u64(vcreate_u64(0x0004080c1014181cULL));
    // Get the lowest 8 bit of each 32-bit elements and reverse them.
    uint8x8_t     tbl = vqtbl2_u8(costs, idx);
    uint64_t      a   = vget_lane_u64(vreinterpret_u64_u8(tbl), 0);
    unsigned long id;
    svt_clzll(id, a);
    best_dir = id >> 3;

    // Difference between the optimal variance and the variance along the
    // orthogonal direction. Again, the sum(x^2) terms cancel out.
    *var = best_cost - cost[(best_dir + 4) & 7];
    // We'd normally divide by 840, but dividing by 1024 is close enough
    // for what we're going to do with this.
    *var >>= 10;
    return best_dir;
}

void svt_aom_cdef_find_dir_dual_neon(const uint16_t* img1, const uint16_t* img2, int stride, int32_t* var_out_1st,
                                     int32_t* var_out_2nd, int32_t coeff_shift, uint8_t* out_dir_1st_8x8,
                                     uint8_t* out_dir_2nd_8x8) {
    // Process first 8x8.
    *out_dir_1st_8x8 = svt_aom_cdef_find_dir_neon(img1, stride, var_out_1st, coeff_shift);

    // Process second 8x8.
    *out_dir_2nd_8x8 = svt_aom_cdef_find_dir_neon(img2, stride, var_out_2nd, coeff_shift);
}
