/*
 * Copyright (c) 2025 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
 * Copyright (c) 2025, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#ifndef COMPUTE_SAD_NEON_DOTPROD_H
#define COMPUTE_SAD_NEON_DOTPROD_H

#include <arm_neon.h>

#include "aom_dsp_rtcd.h"
#include "common_dsp_rtcd.h"
#include "compute_sad_c.h"
#include "compute_sad_neon.h"
#include "sum_neon.h"
#include "utility.h"

static inline unsigned int sadwxh_neon_dotprod(const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr,
                                               int ref_stride, int w, int h) {
    // Only two accumulators are required for optimal instruction throughput of
    // the ABD, UDOT sequence on CPUs with either 2 or 4 Neon pipes.
    uint32x4_t sum[2] = {vdupq_n_u32(0), vdupq_n_u32(0)};

    do {
        int j = 0;
        do {
            uint8x16_t s0, s1, r0, r1, diff0, diff1;

            s0     = vld1q_u8(src_ptr + j);
            r0     = vld1q_u8(ref_ptr + j);
            diff0  = vabdq_u8(s0, r0);
            sum[0] = vdotq_u32(sum[0], diff0, vdupq_n_u8(1));

            s1     = vld1q_u8(src_ptr + j + 16);
            r1     = vld1q_u8(ref_ptr + j + 16);
            diff1  = vabdq_u8(s1, r1);
            sum[1] = vdotq_u32(sum[1], diff1, vdupq_n_u8(1));

            j += 32;
        } while (j < w);

        src_ptr += src_stride;
        ref_ptr += ref_stride;
    } while (--h != 0);

    return vaddvq_u32(vaddq_u32(sum[0], sum[1]));
}

static inline unsigned int sad32xh_neon_dotprod(const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr,
                                                int ref_stride, int h) {
    return sadwxh_neon_dotprod(src_ptr, src_stride, ref_ptr, ref_stride, 32, h);
}

static inline unsigned int sad64xh_neon_dotprod(const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr,
                                                int ref_stride, int h) {
    return sadwxh_neon_dotprod(src_ptr, src_stride, ref_ptr, ref_stride, 64, h);
}

static inline void sad16_neon_dotprod(uint8x16_t src, uint8x16_t ref, uint32x4_t* const sad_sum) {
    uint8x16_t abs_diff = vabdq_u8(src, ref);
    *sad_sum            = vdotq_u32(*sad_sum, abs_diff, vdupq_n_u8(1));
}

static inline uint32x4_t sadwxhx4d_large_neon_dotprod(const uint8_t* src, int src_stride, const uint8_t* ref,
                                                      int ref_stride, int w, int h) {
    uint32x4_t sum_lo[4] = {vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0)};
    uint32x4_t sum_hi[4] = {vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0)};
    uint32x4_t sum[4];

    do {
        int            j       = w;
        const uint8_t* ref_ptr = ref;
        const uint8_t* src_ptr = src;

        uint8x16_t r0 = vld1q_u8(ref_ptr);

        do {
            const uint8x16_t s0 = vld1q_u8(src_ptr);
            const uint8x16_t s1 = vld1q_u8(src_ptr + 16);

            const uint8x16_t r1 = vld1q_u8(ref_ptr + 16);
            const uint8x16_t r2 = vld1q_u8(ref_ptr + 32);

            sad16_neon_dotprod(s0, r0, &sum_lo[0]);
            sad16_neon_dotprod(s0, vextq_u8(r0, r1, 1), &sum_lo[1]);
            sad16_neon_dotprod(s0, vextq_u8(r0, r1, 2), &sum_lo[2]);
            sad16_neon_dotprod(s0, vextq_u8(r0, r1, 3), &sum_lo[3]);

            sad16_neon_dotprod(s1, r1, &sum_lo[0]);
            sad16_neon_dotprod(s1, vextq_u8(r1, r2, 1), &sum_hi[1]);
            sad16_neon_dotprod(s1, vextq_u8(r1, r2, 2), &sum_hi[2]);
            sad16_neon_dotprod(s1, vextq_u8(r1, r2, 3), &sum_hi[3]);

            j -= 32;
            ref_ptr += 32;
            src_ptr += 32;

            r0 = r2;
        } while (j != 0);

        src += src_stride;
        ref += ref_stride;
    } while (--h != 0);

    sum[0] = vaddq_u32(sum_lo[0], sum_hi[0]);
    sum[1] = vaddq_u32(sum_lo[1], sum_hi[1]);
    sum[2] = vaddq_u32(sum_lo[2], sum_hi[2]);
    sum[3] = vaddq_u32(sum_lo[3], sum_hi[3]);

    return horizontal_add_4d_u32x4(sum);
}

static inline uint32x4_t sad64xhx4d_neon_dotprod(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                                 uint32_t ref_stride, uint32_t h) {
    return sadwxhx4d_large_neon_dotprod(src, src_stride, ref, ref_stride, 64, h);
}

static inline uint32x4_t sad32xhx4d_neon_dotprod(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride,
                                                 int h) {
    return sadwxhx4d_large_neon_dotprod(src, src_stride, ref, ref_stride, 32, h);
}

static inline uint32x4_t sad16xhx4d_neon_dotprod(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride,
                                                 int h) {
    uint32x4_t sum[4] = {vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0)};

    do {
        const uint8x16_t r0 = vld1q_u8(ref);
        const uint8x16_t r1 = vld1q_u8(ref + 16);
        const uint8x16_t s  = vld1q_u8(src);

        sad16_neon_dotprod(s, r0, &sum[0]);
        sad16_neon_dotprod(s, vextq_u8(r0, r1, 1), &sum[1]);
        sad16_neon_dotprod(s, vextq_u8(r0, r1, 2), &sum[2]);
        sad16_neon_dotprod(s, vextq_u8(r0, r1, 3), &sum[3]);

        src += src_stride;
        ref += ref_stride;
    } while (--h != 0);

    return horizontal_add_4d_u32x4(sum);
}

static inline unsigned int sad16xh_neon_dotprod(const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr,
                                                int ref_stride, int h) {
    uint32x4_t sum[2] = {vdupq_n_u32(0), vdupq_n_u32(0)};

    do {
        uint8x16_t s0, r0, diff0;

        s0     = vld1q_u8(src_ptr);
        r0     = vld1q_u8(ref_ptr);
        diff0  = vabdq_u8(s0, r0);
        sum[0] = vdotq_u32(sum[0], diff0, vdupq_n_u8(1));

        src_ptr += src_stride;
        ref_ptr += ref_stride;

        uint8x16_t s1, r1, diff1;

        s1     = vld1q_u8(src_ptr);
        r1     = vld1q_u8(ref_ptr);
        diff1  = vabdq_u8(s1, r1);
        sum[1] = vdotq_u32(sum[1], diff1, vdupq_n_u8(1));

        src_ptr += src_stride;
        ref_ptr += ref_stride;

        h -= 2;
    } while (h > 1);

    if (h) {
        uint8x16_t s0, r0, diff0;

        s0     = vld1q_u8(src_ptr);
        r0     = vld1q_u8(ref_ptr);
        diff0  = vabdq_u8(s0, r0);
        sum[0] = vdotq_u32(sum[0], diff0, vdupq_n_u8(1));
    }

    return vaddvq_u32(vaddq_u32(sum[0], sum[1]));
}

static inline void svt_sad_loop_kernel16xh_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                        uint32_t ref_stride, uint32_t block_height, uint64_t* best_sad,
                                                        int16_t* x_search_center, int16_t* y_search_center,
                                                        uint32_t src_stride_raw, uint8_t skip_search_line,
                                                        int16_t search_area_width, int16_t search_area_height) {
    int16_t y_search_start = 0;
    int16_t y_search_step  = 1;

    if (block_height <= 16 && skip_search_line) {
        ref += src_stride_raw;
        src_stride_raw *= 2;
        y_search_start = 1;
        y_search_step  = 2;
    }

    for (int16_t y_search_index = y_search_start; y_search_index < search_area_height;
         y_search_index += y_search_step) {
        for (int16_t x_search_index = 0; x_search_index < search_area_width; x_search_index += 8) {
            /* Get the SAD of 8 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sad16xhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index, ref_stride, block_height);
            uint32x4_t sad4_1 = sad16xhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index + 4, ref_stride, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(sad4_1, best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel16xh_small_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                              uint32_t ref_stride, uint32_t block_height,
                                                              uint64_t* best_sad, int16_t* x_search_center,
                                                              int16_t* y_search_center, uint32_t src_stride_raw,
                                                              uint8_t skip_search_line, int16_t search_area_width,
                                                              int16_t search_area_height) {
    int16_t y_search_start = 0;
    int16_t y_search_step  = 1;

    if (block_height <= 16 && skip_search_line) {
        ref += src_stride_raw;
        src_stride_raw *= 2;
        y_search_start = 1;
        y_search_step  = 2;
    }

    for (int16_t y_search_index = y_search_start; y_search_index < search_area_height;
         y_search_index += y_search_step) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sad16xhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search space aligned along the width and store it in 'temp_sad'. */
            uint64_t temp_sad = sad16xh_neon_dotprod(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel32xh_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                        uint32_t ref_stride, uint32_t block_height, uint64_t* best_sad,
                                                        int16_t* x_search_center, int16_t* y_search_center,
                                                        uint32_t src_stride_raw, int16_t search_area_width,
                                                        int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (int16_t x_search_index = 0; x_search_index < search_area_width; x_search_index += 8) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sad32xhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index, ref_stride, block_height);
            uint32x4_t sad4_1 = sad32xhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index + 4, ref_stride, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(sad4_1, best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel32xh_small_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                              uint32_t ref_stride, uint32_t block_height,
                                                              uint64_t* best_sad, int16_t* x_search_center,
                                                              int16_t* y_search_center, uint32_t src_stride_raw,
                                                              int16_t search_area_width, int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sad32xhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search space aligned along the width and store it in 'temp_sad'. */
            uint64_t temp_sad = sad32xh_neon_dotprod(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel64xh_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                        uint32_t ref_stride, uint32_t block_height, uint64_t* best_sad,
                                                        int16_t* x_search_center, int16_t* y_search_center,
                                                        uint32_t src_stride_raw, int16_t search_area_width,
                                                        int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (int16_t x_search_index = 0; x_search_index < search_area_width; x_search_index += 8) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sad64xhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index, ref_stride, block_height);
            uint32x4_t sad4_1 = sad64xhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index + 4, ref_stride, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(sad4_1, best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel64xh_small_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                              uint32_t ref_stride, uint32_t block_height,
                                                              uint64_t* best_sad, int16_t* x_search_center,
                                                              int16_t* y_search_center, uint32_t src_stride_raw,
                                                              int16_t search_area_width, int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sad64xhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search space aligned along the width and store it in 'temp_sad'. */
            uint64_t temp_sad = sad64xh_neon_dotprod(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        ref += src_stride_raw;
    }
}

// Permute table to gather four elements of each of the 8 ref blocks so we can do sad8d in two operations.
DECLARE_ALIGNED(16, static const uint8_t, kPermTable4xh[32]) = {0, 1, 2, 3, 1, 2, 3, 4, 2, 3, 4, 5, 3, 4, 5, 6,
                                                                4, 5, 6, 7, 5, 6, 7, 8, 6, 7, 8, 9, 7, 8, 9, 10};

static inline uint16x8_t sad4xhx8d_neon_dotprod(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                                uint32_t ref_stride, uint32_t h) {
    /* Initialize 'sum' to store the sum of absolute differences (SAD) of 8 search spaces. */
    uint32x4_t   sum0     = vdupq_n_u32(0);
    uint32x4_t   sum1     = vdupq_n_u32(0);
    uint8x16x2_t perm_tbl = vld1q_u8_x2(kPermTable4xh);

    do {
        uint8x16_t src0 = vreinterpretq_u8_u32(vld1q_dup_u32((const uint32_t*)src));

        uint8x16_t ref0 = vqtbl1q_u8(vld1q_u8(ref), perm_tbl.val[0]);
        uint8x16_t ref1 = vqtbl1q_u8(vld1q_u8(ref), perm_tbl.val[1]);

        uint8x16_t abs0 = vabdq_u8(src0, ref0);
        uint8x16_t abs1 = vabdq_u8(src0, ref1);

        sum0 = vdotq_u32(sum0, abs0, vdupq_n_u8(1));
        sum1 = vdotq_u32(sum1, abs1, vdupq_n_u8(1));

        src += src_stride;
        ref += ref_stride;
    } while (--h != 0);

    return vcombine_u16(vmovn_u32(sum0), vmovn_u32(sum1));
}

// clang-format off
static const uint16_t kMask16Bit[16] = {
    0, 0, 0, 0, 0, 0, 0, 0, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff
};
// clang-format on

static inline void svt_sad_loop_kernel4xh_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                       uint32_t ref_stride, uint32_t block_height, uint64_t* best_sad,
                                                       int16_t* x_search_center, int16_t* y_search_center,
                                                       uint32_t src_stride_raw, int16_t search_area_width,
                                                       int16_t search_area_height) {
    uint32_t   leftover      = search_area_width & 7;
    uint16x8_t leftover_mask = vld1q_u16(kMask16Bit + 8 - leftover);

    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 8; x_search_index += 8) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint16x8_t sad8 = sad4xhx8d_neon_dotprod(src, src_stride, ref + x_search_index, ref_stride, block_height);

            /* Update 'best_sad'. */
            update_best_sad_u16(sad8, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        if (leftover) {
            /* Get the SAD of 8 search spaces aligned along the width and store it in 'sad8'. */
            uint16x8_t sad8 = sad4xhx8d_neon_dotprod(src, src_stride, ref + x_search_index, ref_stride, block_height);

            /* Set undesired lanes to maximum value. */
            sad8 = vorrq_u16(sad8, leftover_mask);

            /* Update 'best_sad'. */
            update_best_sad_u16(sad8, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline uint16x8_t sad6xhx8d_neon_dotprod(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                                uint32_t ref_stride, uint32_t h) {
    /* Initialize 'sum' to store the sum of absolute differences (SAD) of 8 search spaces. */
    uint32x4_t   sum0      = vdupq_n_u32(0);
    uint32x4_t   sum1      = vdupq_n_u32(0);
    uint16x8_t   sum       = vdupq_n_u16(0);
    uint8x16x2_t perm_tbl  = vld1q_u8_x2(kPermTable4xh);
    uint8x16_t   perm_tbl2 = vld1q_u8(kPermTable2xh + 32);

    do {
        uint8x16_t src0 = vreinterpretq_u8_u32(vld1q_dup_u32((const uint32_t*)src));
        uint8x16_t r    = vld1q_u8(ref);

        /* First four elements. */
        uint8x16_t ref0 = vqtbl1q_u8(r, perm_tbl.val[0]);
        uint8x16_t ref1 = vqtbl1q_u8(r, perm_tbl.val[1]);

        uint8x16_t abs0 = vabdq_u8(src0, ref0);
        uint8x16_t abs1 = vabdq_u8(src0, ref1);

        sum0 = vdotq_u32(sum0, abs0, vdupq_n_u8(1));
        sum1 = vdotq_u32(sum1, abs1, vdupq_n_u8(1));

        /* Last two elements. */
        uint8x16_t src1 = vreinterpretq_u8_u16(vld1q_dup_u16((const uint16_t*)(src + 4)));
        uint8x16_t ref2 = vqtbl1q_u8(r, perm_tbl2);
        uint8x16_t abs2 = vabdq_u8(src1, ref2);
        sum             = vpadalq_u8(sum, abs2);

        src += src_stride;
        ref += ref_stride;
    } while (--h != 0);

    return vaddq_u16(sum, vcombine_u16(vmovn_u32(sum0), vmovn_u32(sum1)));
}

static inline void svt_sad_loop_kernel6xh_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                       uint32_t ref_stride, uint32_t block_height, uint64_t* best_sad,
                                                       int16_t* x_search_center, int16_t* y_search_center,
                                                       uint32_t src_stride_raw, int16_t search_area_width,
                                                       int16_t search_area_height) {
    uint32_t   leftover      = search_area_width & 7;
    uint16x8_t leftover_mask = vld1q_u16(kMask16Bit + 8 - leftover);

    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 8; x_search_index += 8) {
            /* Get the SAD of 8 search spaces aligned along the width and store it in 'sad8'. */
            uint16x8_t sad8 = sad6xhx8d_neon_dotprod(src, src_stride, ref + x_search_index, ref_stride, block_height);

            /* Update 'best_sad'. */
            update_best_sad_u16(sad8, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        if (leftover) {
            /* Get the SAD of 8 search spaces aligned along the width and store it in 'sad8'. */
            uint16x8_t sad8 = sad6xhx8d_neon_dotprod(src, src_stride, ref + x_search_index, ref_stride, block_height);

            /* Set undesired lanes to maximum value. */
            sad8 = vorrq_u16(sad8, leftover_mask);

            /* Update 'best_sad'. */
            update_best_sad_u16(sad8, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline void sad12xhx8d_neon_dotprod(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride,
                                           int h, uint32x4_t* res) {
    uint16x8_t sum[8]  = {vdupq_n_u16(0),
                          vdupq_n_u16(0),
                          vdupq_n_u16(0),
                          vdupq_n_u16(0),
                          vdupq_n_u16(0),
                          vdupq_n_u16(0),
                          vdupq_n_u16(0),
                          vdupq_n_u16(0)};
    uint32x4_t sum4[2] = {vdupq_n_u32(0), vdupq_n_u32(0)};

    uint8x16x2_t perm_tbl = vld1q_u8_x2(kPermTable4xh);

    do {
        /* First eight elements. */
        uint8x8_t        s   = vld1_u8(src);
        const uint8x16_t r0  = vld1q_u8(ref);
        const uint8x8_t  r_l = vget_low_u8(r0);
        const uint8x8_t  r_h = vget_high_u8(r0);

        sum[0] = vabal_u8(sum[0], s, r_l);
        sum[1] = vabal_u8(sum[1], s, vext_u8(r_l, r_h, 1));
        sum[2] = vabal_u8(sum[2], s, vext_u8(r_l, r_h, 2));
        sum[3] = vabal_u8(sum[3], s, vext_u8(r_l, r_h, 3));
        sum[4] = vabal_u8(sum[4], s, vext_u8(r_l, r_h, 4));
        sum[5] = vabal_u8(sum[5], s, vext_u8(r_l, r_h, 5));
        sum[6] = vabal_u8(sum[6], s, vext_u8(r_l, r_h, 6));
        sum[7] = vabal_u8(sum[7], s, vext_u8(r_l, r_h, 7));

        /* Last four elements. */
        uint8x16_t src0 = vreinterpretq_u8_u32(vld1q_dup_u32((const uint32_t*)(src + 8)));
        uint8x16_t r    = vld1q_u8(ref + 8);

        uint8x16_t ref0 = vqtbl1q_u8(r, perm_tbl.val[0]);
        uint8x16_t ref1 = vqtbl1q_u8(r, perm_tbl.val[1]);

        uint8x16_t abs0 = vabdq_u8(src0, ref0);
        uint8x16_t abs1 = vabdq_u8(src0, ref1);

        sum4[0] = vdotq_u32(sum4[0], abs0, vdupq_n_u8(1));
        sum4[1] = vdotq_u32(sum4[1], abs1, vdupq_n_u8(1));

        src += src_stride;
        ref += ref_stride;
    } while (--h != 0);

    uint32x4_t sum0_u32 = horizontal_add_4d_u16x8(&sum[0]);
    uint32x4_t sum1_u32 = horizontal_add_4d_u16x8(&sum[4]);

    res[0] = vaddq_u32(sum0_u32, sum4[0]);
    res[1] = vaddq_u32(sum1_u32, sum4[1]);
}

// clang-format off
static const uint32_t kMask32Bit[16] = {
             0,          0,          0,          0,          0,          0,          0,          0,
    0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff,
};
// clang-format on

static inline void svt_sad_loop_kernel12xh_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                        uint32_t ref_stride, uint32_t block_height, uint64_t* best_sad,
                                                        int16_t* x_search_center, int16_t* y_search_center,
                                                        uint32_t src_stride_raw, int16_t search_area_width,
                                                        int16_t search_area_height) {
    uint32_t   leftover = search_area_width & 7;
    uint32_t   mask_idx = 8 - leftover;
    uint32x4_t leftover_mask[2];
    leftover_mask[0] = vld1q_u32(kMask32Bit + mask_idx);
    leftover_mask[1] = vld1q_u32(kMask32Bit + mask_idx + 4);

    for (int y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        /* To accumulate the SAD of 8 search spaces */
        uint32x4_t sad8[2];

        int x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 8; x_search_index += 8) {
            /* Get the SAD of 8 search spaces aligned along the width and store it in 'sad8'. */
            sad12xhx8d_neon_dotprod(src, src_stride, ref + x_search_index, ref_stride, block_height, sad8);

            /* Update 'best_sad'. */
            update_best_sad_u32(sad8[0], best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(
                sad8[1], best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }

        if (leftover) {
            /* Get the SAD of 8 search spaces aligned along the width and store it in 'sad8'. */
            sad12xhx8d_neon_dotprod(src, src_stride, ref + x_search_index, ref_stride, block_height, sad8);

            /* Set undesired lanes to maximum value. */
            sad8[0] = vorrq_u32(sad8[0], leftover_mask[0]);
            sad8[1] = vorrq_u32(sad8[1], leftover_mask[1]);

            /* Update 'best_sad'. */
            update_best_sad_u32(sad8[0], best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(
                sad8[1], best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline uint32x4_t sad24xhx4d_neon_dotprod(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride,
                                                 int h) {
    uint32x4_t sum[4]     = {vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0)};
    uint16x8_t sum_u16[4] = {vdupq_n_u16(0), vdupq_n_u16(0), vdupq_n_u16(0), vdupq_n_u16(0)};

    do {
        const uint8x16_t s  = vld1q_u8(src);
        const uint8x16_t r0 = vld1q_u8(ref);
        const uint8x16_t r1 = vld1q_u8(ref + 16);

        sad16_neon_dotprod(s, r0, &sum[0]);
        sad16_neon_dotprod(s, vextq_u8(r0, r1, 1), &sum[1]);
        sad16_neon_dotprod(s, vextq_u8(r0, r1, 2), &sum[2]);
        sad16_neon_dotprod(s, vextq_u8(r0, r1, 3), &sum[3]);

        const uint8x8_t s16  = vld1_u8(src + 16);
        const uint8x8_t r1_l = vget_low_u8(r1);
        const uint8x8_t r1_h = vget_high_u8(r1);
        sum_u16[0]           = vabal_u8(sum_u16[0], s16, r1_l);
        sum_u16[1]           = vabal_u8(sum_u16[1], s16, vext_u8(r1_l, r1_h, 1));
        sum_u16[2]           = vabal_u8(sum_u16[2], s16, vext_u8(r1_l, r1_h, 2));
        sum_u16[3]           = vabal_u8(sum_u16[3], s16, vext_u8(r1_l, r1_h, 3));

        src += src_stride;
        ref += ref_stride;
    } while (--h != 0);

    sum[0] = vpadalq_u16(sum[0], sum_u16[0]);
    sum[1] = vpadalq_u16(sum[1], sum_u16[1]);
    sum[2] = vpadalq_u16(sum[2], sum_u16[2]);
    sum[3] = vpadalq_u16(sum[3], sum_u16[3]);

    return horizontal_add_4d_u32x4(sum);
}

static inline unsigned int sad24xh_neon_dotprod(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride,
                                                int h) {
    uint32x4_t sum[2]     = {vdupq_n_u32(0), vdupq_n_u32(0)};
    uint16x8_t sum_u16[2] = {vdupq_n_u16(0), vdupq_n_u16(0)};

    do {
        const uint8x16_t s0 = vld1q_u8(src);
        const uint8x16_t s1 = vld1q_u8(src + src_stride);
        sad16_neon_dotprod(s0, vld1q_u8(ref), &sum[0]);
        sad16_neon_dotprod(s1, vld1q_u8(ref + ref_stride), &sum[1]);

        const uint8x8_t s16_0 = vld1_u8(src + 16);
        const uint8x8_t s16_1 = vld1_u8(src + src_stride + 16);
        sum_u16[0]            = vabal_u8(sum_u16[0], s16_0, vld1_u8(ref + 16));
        sum_u16[1]            = vabal_u8(sum_u16[1], s16_1, vld1_u8(ref + ref_stride + 16));

        src += 2 * src_stride;
        ref += 2 * ref_stride;

        h -= 2;
    } while (h > 1);

    if (h) {
        const uint8x16_t s0 = vld1q_u8(src);
        sad16_neon_dotprod(s0, vld1q_u8(ref), &sum[0]);

        const uint8x8_t s16_0 = vld1_u8(src + 16);
        sum_u16[0]            = vabal_u8(sum_u16[0], s16_0, vld1_u8(ref + 16));
    }

    sum[0] = vpadalq_u16(sum[0], sum_u16[0]);
    sum[1] = vpadalq_u16(sum[1], sum_u16[1]);

    return vaddvq_u32(vaddq_u32(sum[0], sum[1]));
}

static inline void svt_sad_loop_kernel24xh_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                        uint32_t ref_stride, uint32_t block_height, uint64_t* best_sad,
                                                        int16_t* x_search_center, int16_t* y_search_center,
                                                        uint32_t src_stride_raw, int16_t search_area_width,
                                                        int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (int16_t x_search_index = 0; x_search_index <= search_area_width - 8; x_search_index += 8) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sad24xhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index, ref_stride, block_height);
            uint32x4_t sad4_1 = sad24xhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index + 4, ref_stride, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(sad4_1, best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }
        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel24xh_small_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                              uint32_t ref_stride, uint32_t block_height,
                                                              uint64_t* best_sad, int16_t* x_search_center,
                                                              int16_t* y_search_center, uint32_t src_stride_raw,
                                                              int16_t search_area_width, int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4 = sad24xhx4d_neon_dotprod(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad_u32(sad4, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search spaces aligned along the width and store it in 'temp_sad'. */
            uint64_t temp_sad = sad24xh_neon_dotprod(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        ref += src_stride_raw;
    }
}

static inline uint32x4_t sad48xhx4d_neon_dotprod(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                                 uint32_t ref_stride, uint32_t h) {
    uint32x4_t sad4_0 = sad32xhx4d_neon_dotprod(src + 0, src_stride, ref + 0, ref_stride, h);
    uint32x4_t sad4_1 = sad16xhx4d_neon_dotprod(src + 32, src_stride, ref + 32, ref_stride, h);
    return vaddq_u32(sad4_0, sad4_1);
}

static inline uint32_t sad48xh_neon_dotprod(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride,
                                            int h) {
    uint32x4_t sum[3] = {vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0)};

    do {
        const uint8x16_t s0 = vld1q_u8(src);
        sad16_neon_dotprod(s0, vld1q_u8(ref + 0), &sum[0]);

        const uint8x16_t s1 = vld1q_u8(src + 16);
        sad16_neon_dotprod(s1, vld1q_u8(ref + 0 + 16), &sum[1]);

        const uint8x16_t s2 = vld1q_u8(src + 32);
        sad16_neon_dotprod(s2, vld1q_u8(ref + 0 + 32), &sum[2]);

        src += src_stride;
        ref += ref_stride;
    } while (--h != 0);

    sum[0] = vaddq_u32(sum[0], sum[1]);
    sum[0] = vaddq_u32(sum[0], sum[2]);

    return vaddvq_u32(sum[0]);
}

static inline void svt_sad_loop_kernel48xh_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                        uint32_t ref_stride, uint32_t block_height, uint64_t* best_sad,
                                                        int16_t* x_search_center, int16_t* y_search_center,
                                                        uint32_t src_stride_raw, int16_t search_area_width,
                                                        int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (int16_t x_search_index = 0; x_search_index <= search_area_width - 8; x_search_index += 8) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sad48xhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index, ref_stride, block_height);
            uint32x4_t sad4_1 = sad48xhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index + 4, ref_stride, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(sad4_1, best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }
        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel48xh_small_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                              uint32_t ref_stride, uint32_t block_height,
                                                              uint64_t* best_sad, int16_t* x_search_center,
                                                              int16_t* y_search_center, uint32_t src_stride_raw,
                                                              int16_t search_area_width, int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4 = sad48xhx4d_neon_dotprod(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad_u32(sad4, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search spaces aligned along the width and store it in 'temp_sad'. */
            uint64_t temp_sad = sad48xh_neon_dotprod(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        ref += src_stride_raw;
    }
}

static inline uint32_t sad_anywxh_neon_dotprod(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                               uint32_t ref_stride, uint32_t width, uint32_t height) {
    uint16x8_t sum_u16 = vdupq_n_u16(0);
    uint32x4_t sum_u32 = vdupq_n_u32(0);
    uint32_t   sum     = 0;

    do {
        int w = width;

        const uint8_t* src_ptr = src;
        const uint8_t* ref_ptr = ref;

        while (w >= 16) {
            const uint8x16_t s = vld1q_u8(src_ptr);
            sad16_neon_dotprod(s, vld1q_u8(ref_ptr), &sum_u32);

            src_ptr += 16;
            ref_ptr += 16;
            w -= 16;
        }

        if (w >= 8) {
            const uint8x8_t s = vld1_u8(src_ptr);
            sum_u16           = vabal_u8(sum_u16, s, vld1_u8(ref_ptr));
            src_ptr += 8;
            ref_ptr += 8;
            w -= 8;
        }

        if (w >= 4) {
            const uint8x8_t s = load_u8_4x1(src_ptr);
            sum_u16           = vabal_u8(sum_u16, s, load_u8_4x1(ref_ptr));
            src_ptr += 4;
            ref_ptr += 4;
            w -= 4;
        }

        while (--w >= 0) {
            sum += EB_ABS_DIFF(src_ptr[w], ref_ptr[w]);
        }

        src += src_stride;
        ref += ref_stride;
    } while (--height != 0);

    sum_u32 = vpadalq_u16(sum_u32, sum_u16);
    return sum + vaddvq_u32(sum_u32);
}

#endif // COMPUTE_SAD_NEON_DOTPROD_H
