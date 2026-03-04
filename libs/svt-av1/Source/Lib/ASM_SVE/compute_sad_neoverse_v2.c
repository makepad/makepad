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

#include <arm_neon.h>
#include <arm_sve.h>

#include "aom_dsp_rtcd.h"
#include "common_dsp_rtcd.h"
#include "compute_sad_c.h"
#include "compute_sad_neon_dotprod.h"
#include "compute_sad_sve.h"
#include "neon_sve_bridge.h"
#include "sum_neon.h"
#include "utility.h"

// This file implements a version of svt_sad_loop_kernel that is specifically
// tuned for the Neoverse V2 core.

static inline uint32x4_t sad8xhx4d_neoverse_v2(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                               uint32_t ref_stride, uint32_t h) {
    uint16x8_t sum[4];
    sum[0] = vdupq_n_u16(0);
    sum[1] = vdupq_n_u16(0);
    sum[2] = vdupq_n_u16(0);
    sum[3] = vdupq_n_u16(0);

    do {
        uint8x8_t        s   = vld1_u8(src);
        const uint8x16_t r   = vld1q_u8(ref);
        const uint8x8_t  r_l = vget_low_u8(r);
        const uint8x8_t  r_h = vget_high_u8(r);

        sum[0] = vabal_u8(sum[0], s, r_l);
        sum[1] = vabal_u8(sum[1], s, vext_u8(r_l, r_h, 1));
        sum[2] = vabal_u8(sum[2], s, vext_u8(r_l, r_h, 2));
        sum[3] = vabal_u8(sum[3], s, vext_u8(r_l, r_h, 3));

        src += src_stride;
        ref += ref_stride;
    } while (--h != 0);

    return horizontal_add_4d_u16x8(sum);
}

static inline void svt_sad_loop_kernel8xh_neoverse_v2(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                      uint32_t ref_stride, uint32_t block_height, uint64_t* best_sad,
                                                      int16_t* x_search_center, int16_t* y_search_center,
                                                      uint32_t src_stride_raw, int16_t search_area_width,
                                                      int16_t search_area_height) {
    for (int y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4 = sad8xhx4d_neoverse_v2(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad_u32(sad4, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search spaces aligned along the width and store it in 'temp_sad'. */
            uint64_t temp_sad = sad8xh_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        ref += src_stride_raw;
    }
}

static inline uint32x4_t sad16xhx4d_neoverse_v2(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride,
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

static inline void svt_sad_loop_kernel16xh_neoverse_v2(uint8_t* src, uint32_t src_stride, uint8_t* ref,
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
            uint32x4_t sad4_0 = sad16xhx4d_neoverse_v2(src, src_stride, ref + x_search_index, ref_stride, block_height);
            uint32x4_t sad4_1 = sad16xhx4d_neoverse_v2(
                src, src_stride, ref + x_search_index + 4, ref_stride, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(sad4_1, best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel16xh_small_neoverse_v2(uint8_t* src, uint32_t src_stride, uint8_t* ref,
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
            uint32x4_t sad4_0 = sad16xhx4d_neoverse_v2(src, src_stride, ref + x_search_index, ref_stride, block_height);
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

static inline uint32x4_t sadwxhx4d_large_neoverse_v2(const uint8_t* src, int src_stride, const uint8_t* ref,
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

static inline uint32x4_t sad32xhx4d_neoverse_v2(const uint8_t* src, int src_stride, const uint8_t* ref, int ref_stride,
                                                int h) {
    return sadwxhx4d_large_neoverse_v2(src, src_stride, ref, ref_stride, 32, h);
}

static inline void svt_sad_loop_kernel32xh_neoverse_v2(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                       uint32_t ref_stride, uint32_t block_height, uint64_t* best_sad,
                                                       int16_t* x_search_center, int16_t* y_search_center,
                                                       uint32_t src_stride_raw, int16_t search_area_width,
                                                       int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (int16_t x_search_index = 0; x_search_index < search_area_width; x_search_index += 8) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sad32xhx4d_neoverse_v2(src, src_stride, ref + x_search_index, ref_stride, block_height);
            uint32x4_t sad4_1 = sad32xhx4d_neoverse_v2(
                src, src_stride, ref + x_search_index + 4, ref_stride, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(sad4_1, best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel32xh_small_neoverse_v2(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                             uint32_t ref_stride, uint32_t block_height,
                                                             uint64_t* best_sad, int16_t* x_search_center,
                                                             int16_t* y_search_center, uint32_t src_stride_raw,
                                                             int16_t search_area_width, int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sad32xhx4d_neoverse_v2(src, src_stride, ref + x_search_index, ref_stride, block_height);
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

static inline uint32x4_t sad64xhx4d_neoverse_v2(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                                uint32_t ref_stride, uint32_t h) {
    return sadwxhx4d_large_neoverse_v2(src, src_stride, ref, ref_stride, 64, h);
}

static inline void svt_sad_loop_kernel64xh_neoverse_v2(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                       uint32_t ref_stride, uint32_t block_height, uint64_t* best_sad,
                                                       int16_t* x_search_center, int16_t* y_search_center,
                                                       uint32_t src_stride_raw, int16_t search_area_width,
                                                       int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (int16_t x_search_index = 0; x_search_index < search_area_width; x_search_index += 8) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sad64xhx4d_neoverse_v2(src, src_stride, ref + x_search_index, ref_stride, block_height);
            uint32x4_t sad4_1 = sad64xhx4d_neoverse_v2(
                src, src_stride, ref + x_search_index + 4, ref_stride, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(sad4_1, best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel64xh_small_neoverse_v2(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                             uint32_t ref_stride, uint32_t block_height,
                                                             uint64_t* best_sad, int16_t* x_search_center,
                                                             int16_t* y_search_center, uint32_t src_stride_raw,
                                                             int16_t search_area_width, int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sad64xhx4d_neoverse_v2(src, src_stride, ref + x_search_index, ref_stride, block_height);
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

static inline uint32x4_t sad48xhx4d_neoverse_v2(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                                uint32_t ref_stride, uint32_t h) {
    uint32x4_t sad4_0 = sad32xhx4d_neoverse_v2(src + 0, src_stride, ref + 0, ref_stride, h);
    uint32x4_t sad4_1 = sad16xhx4d_neoverse_v2(src + 32, src_stride, ref + 32, ref_stride, h);
    return vaddq_u32(sad4_0, sad4_1);
}

static inline void svt_sad_loop_kernel48xh_neoverse_v2(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                       uint32_t ref_stride, uint32_t block_height, uint64_t* best_sad,
                                                       int16_t* x_search_center, int16_t* y_search_center,
                                                       uint32_t src_stride_raw, int16_t search_area_width,
                                                       int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (int16_t x_search_index = 0; x_search_index <= search_area_width - 8; x_search_index += 8) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sad48xhx4d_neoverse_v2(src, src_stride, ref + x_search_index, ref_stride, block_height);
            uint32x4_t sad4_1 = sad48xhx4d_neoverse_v2(
                src, src_stride, ref + x_search_index + 4, ref_stride, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(sad4_1, best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }
        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel48xh_small_neoverse_v2(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                             uint32_t ref_stride, uint32_t block_height,
                                                             uint64_t* best_sad, int16_t* x_search_center,
                                                             int16_t* y_search_center, uint32_t src_stride_raw,
                                                             int16_t search_area_width, int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4 = sad48xhx4d_neoverse_v2(src, src_stride, ref + x_search_index, ref_stride, block_height);
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

static inline uint32x4_t sadwxhx4d_neoverse_v2(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                               uint32_t ref_stride, uint32_t width, uint32_t height) {
    uint32x4_t sum_u32[4] = {vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0)};

    do {
        int w = width;

        const uint8_t* src_ptr = src;
        const uint8_t* ref_ptr = ref;

        uint8x16_t ref0 = vld1q_u8(ref_ptr);
        while (w >= 16) {
            const uint8x16_t ref1 = vld1q_u8(ref_ptr + 16);
            const uint8x16_t s    = vld1q_u8(src_ptr);

            sad16_neon_dotprod(s, ref0, &sum_u32[0]);
            sad16_neon_dotprod(s, vextq_u8(ref0, ref1, 1), &sum_u32[1]);
            sad16_neon_dotprod(s, vextq_u8(ref0, ref1, 2), &sum_u32[2]);
            sad16_neon_dotprod(s, vextq_u8(ref0, ref1, 3), &sum_u32[3]);

            src_ptr += 16;
            ref_ptr += 16;
            w -= 16;

            ref0 = ref1;
        }

        const svbool_t   p  = svwhilelt_b8_s32(0, width & 15);
        const uint8x16_t s  = svget_neonq_u8(svld1_u8(p, src_ptr));
        const uint8x16_t r0 = svget_neonq_u8(svld1_u8(p, ref_ptr + 0));
        const uint8x16_t r1 = svget_neonq_u8(svld1_u8(p, ref_ptr + 1));
        const uint8x16_t r2 = svget_neonq_u8(svld1_u8(p, ref_ptr + 2));
        const uint8x16_t r3 = svget_neonq_u8(svld1_u8(p, ref_ptr + 3));

        sad16_neon_dotprod(s, r0, &sum_u32[0]);
        sad16_neon_dotprod(s, r1, &sum_u32[1]);
        sad16_neon_dotprod(s, r2, &sum_u32[2]);
        sad16_neon_dotprod(s, r3, &sum_u32[3]);

        src += src_stride;
        ref += ref_stride;
    } while (--height != 0);

    return horizontal_add_4d_u32x4(sum_u32);
}

static inline void svt_sad_loop_kernelwxh_neoverse_v2(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                      uint32_t ref_stride, uint32_t block_width, uint32_t block_height,
                                                      uint64_t* best_sad, int16_t* x_search_center,
                                                      int16_t* y_search_center, uint32_t src_stride_raw,
                                                      int16_t search_area_width, int16_t search_area_height) {
    for (int y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (int x_search_index = 0; x_search_index < search_area_width; x_search_index += 8) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sadwxhx4d_neoverse_v2(
                src, src_stride, ref + x_search_index, ref_stride, block_width, block_height);
            uint32x4_t sad4_1 = sadwxhx4d_neoverse_v2(
                src, src_stride, ref + x_search_index + 4, ref_stride, block_width, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(sad4_1, best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernelwxh_small_neoverse_v2(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                            uint32_t ref_stride, uint32_t block_width,
                                                            uint32_t block_height, uint64_t* best_sad,
                                                            int16_t* x_search_center, int16_t* y_search_center,
                                                            uint32_t src_stride_raw, int16_t search_area_width,
                                                            int16_t search_area_height) {
    for (int y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4 = sadwxhx4d_neoverse_v2(
                src, src_stride, ref + x_search_index, ref_stride, block_width, block_height);
            update_best_sad_u32(sad4, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search spaces aligned along the width and store it in 'temp_sad'. */
            uint64_t temp_sad = sad_anywxh_sve(
                src, src_stride, ref + x_search_index, ref_stride, block_width, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        ref += src_stride_raw;
    }
}

void svt_sad_loop_kernel_neoverse_v2(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                     uint32_t block_height, uint32_t block_width, uint64_t* best_sad,
                                     int16_t* x_search_center, int16_t* y_search_center, uint32_t src_stride_raw,
                                     uint8_t skip_search_line, int16_t search_area_width, int16_t search_area_height) {
    *best_sad = UINT64_MAX;
    // Most of the time search_area_width is a multiple of 8, so specialize for this case so that we run only sad4d.
    if (search_area_width % 8 == 0) {
        switch (block_width) {
        case 4: {
            svt_sad_loop_kernel4xh_neon_dotprod(src,
                                                src_stride,
                                                ref,
                                                ref_stride,
                                                block_height,
                                                best_sad,
                                                x_search_center,
                                                y_search_center,
                                                src_stride_raw,
                                                search_area_width,
                                                search_area_height);
            break;
        }
        case 6: {
            svt_sad_loop_kernel6xh_neon_dotprod(src,
                                                src_stride,
                                                ref,
                                                ref_stride,
                                                block_height,
                                                best_sad,
                                                x_search_center,
                                                y_search_center,
                                                src_stride_raw,
                                                search_area_width,
                                                search_area_height);
            break;
        }
        case 8: {
            svt_sad_loop_kernel8xh_neoverse_v2(src,
                                               src_stride,
                                               ref,
                                               ref_stride,
                                               block_height,
                                               best_sad,
                                               x_search_center,
                                               y_search_center,
                                               src_stride_raw,
                                               search_area_width,
                                               search_area_height);
            break;
        }
        case 12: {
            svt_sad_loop_kernel12xh_neon_dotprod(src,
                                                 src_stride,
                                                 ref,
                                                 ref_stride,
                                                 block_height,
                                                 best_sad,
                                                 x_search_center,
                                                 y_search_center,
                                                 src_stride_raw,
                                                 search_area_width,
                                                 search_area_height);
            break;
        }
        case 16: {
            svt_sad_loop_kernel16xh_neoverse_v2(src,
                                                src_stride,
                                                ref,
                                                ref_stride,
                                                block_height,
                                                best_sad,
                                                x_search_center,
                                                y_search_center,
                                                src_stride_raw,
                                                skip_search_line,
                                                search_area_width,
                                                search_area_height);
            break;
        }
        case 24: {
            svt_sad_loop_kernel24xh_neon_dotprod(src,
                                                 src_stride,
                                                 ref,
                                                 ref_stride,
                                                 block_height,
                                                 best_sad,
                                                 x_search_center,
                                                 y_search_center,
                                                 src_stride_raw,
                                                 search_area_width,
                                                 search_area_height);
            break;
        }
        case 32: {
            svt_sad_loop_kernel32xh_neoverse_v2(src,
                                                src_stride,
                                                ref,
                                                ref_stride,
                                                block_height,
                                                best_sad,
                                                x_search_center,
                                                y_search_center,
                                                src_stride_raw,
                                                search_area_width,
                                                search_area_height);
            break;
        }
        case 48: {
            svt_sad_loop_kernel48xh_neoverse_v2(src,
                                                src_stride,
                                                ref,
                                                ref_stride,
                                                block_height,
                                                best_sad,
                                                x_search_center,
                                                y_search_center,
                                                src_stride_raw,
                                                search_area_width,
                                                search_area_height);
            break;
        }
        case 64: {
            svt_sad_loop_kernel64xh_neoverse_v2(src,
                                                src_stride,
                                                ref,
                                                ref_stride,
                                                block_height,
                                                best_sad,
                                                x_search_center,
                                                y_search_center,
                                                src_stride_raw,
                                                search_area_width,
                                                search_area_height);
            break;
        }
        default: {
            svt_sad_loop_kernelwxh_neoverse_v2(src,
                                               src_stride,
                                               ref,
                                               ref_stride,
                                               block_width,
                                               block_height,
                                               best_sad,
                                               x_search_center,
                                               y_search_center,
                                               src_stride_raw,
                                               search_area_width,
                                               search_area_height);
            break;
        }
        }

    } else {
        switch (block_width) {
        case 4: {
            svt_sad_loop_kernel4xh_neon_dotprod(src,
                                                src_stride,
                                                ref,
                                                ref_stride,
                                                block_height,
                                                best_sad,
                                                x_search_center,
                                                y_search_center,
                                                src_stride_raw,
                                                search_area_width,
                                                search_area_height);
            break;
        }
        case 6: {
            svt_sad_loop_kernel6xh_neon_dotprod(src,
                                                src_stride,
                                                ref,
                                                ref_stride,
                                                block_height,
                                                best_sad,
                                                x_search_center,
                                                y_search_center,
                                                src_stride_raw,
                                                search_area_width,
                                                search_area_height);
            break;
        }
        case 8: {
            svt_sad_loop_kernel8xh_neoverse_v2(src,
                                               src_stride,
                                               ref,
                                               ref_stride,
                                               block_height,
                                               best_sad,
                                               x_search_center,
                                               y_search_center,
                                               src_stride_raw,
                                               search_area_width,
                                               search_area_height);
            break;
        }
        case 12: {
            svt_sad_loop_kernel12xh_neon_dotprod(src,
                                                 src_stride,
                                                 ref,
                                                 ref_stride,
                                                 block_height,
                                                 best_sad,
                                                 x_search_center,
                                                 y_search_center,
                                                 src_stride_raw,
                                                 search_area_width,
                                                 search_area_height);
            break;
        }
        case 16: {
            svt_sad_loop_kernel16xh_small_neoverse_v2(src,
                                                      src_stride,
                                                      ref,
                                                      ref_stride,
                                                      block_height,
                                                      best_sad,
                                                      x_search_center,
                                                      y_search_center,
                                                      src_stride_raw,
                                                      skip_search_line,
                                                      search_area_width,
                                                      search_area_height);
            break;
        }
        case 24: {
            svt_sad_loop_kernel24xh_small_neon_dotprod(src,
                                                       src_stride,
                                                       ref,
                                                       ref_stride,
                                                       block_height,
                                                       best_sad,
                                                       x_search_center,
                                                       y_search_center,
                                                       src_stride_raw,
                                                       search_area_width,
                                                       search_area_height);
            break;
        }
        case 32: {
            svt_sad_loop_kernel32xh_small_neoverse_v2(src,
                                                      src_stride,
                                                      ref,
                                                      ref_stride,
                                                      block_height,
                                                      best_sad,
                                                      x_search_center,
                                                      y_search_center,
                                                      src_stride_raw,
                                                      search_area_width,
                                                      search_area_height);
            break;
        }
        case 48: {
            svt_sad_loop_kernel48xh_small_neoverse_v2(src,
                                                      src_stride,
                                                      ref,
                                                      ref_stride,
                                                      block_height,
                                                      best_sad,
                                                      x_search_center,
                                                      y_search_center,
                                                      src_stride_raw,
                                                      search_area_width,
                                                      search_area_height);
            break;
        }
        case 64: {
            svt_sad_loop_kernel64xh_small_neoverse_v2(src,
                                                      src_stride,
                                                      ref,
                                                      ref_stride,
                                                      block_height,
                                                      best_sad,
                                                      x_search_center,
                                                      y_search_center,
                                                      src_stride_raw,
                                                      search_area_width,
                                                      search_area_height);
            break;
        }
        default: {
            svt_sad_loop_kernelwxh_small_neoverse_v2(src,
                                                     src_stride,
                                                     ref,
                                                     ref_stride,
                                                     block_width,
                                                     block_height,
                                                     best_sad,
                                                     x_search_center,
                                                     y_search_center,
                                                     src_stride_raw,
                                                     search_area_width,
                                                     search_area_height);
            break;
        }
        }
    }
}
