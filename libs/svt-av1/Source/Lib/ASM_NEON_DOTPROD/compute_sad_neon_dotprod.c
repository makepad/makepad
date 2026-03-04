/*
 * Copyright (c) 2025 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
 * Copyright (c) 2023, Alliance for Open Media. All rights reserved
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
#include "common_dsp_rtcd.h"
#include "compute_sad_c.h"
#include "compute_sad_neon.h"
#include "compute_sad_neon_dotprod.h"
#include "mem_neon.h"
#include "sum_neon.h"
#include "utility.h"

DECLARE_ALIGNED(16, static const uint8_t, kPermTable[32]) = {0, 1, 2, 3, 1, 2, 3, 4, 2, 3, 4, 5, 3, 4, 5, 6,
                                                             4, 5, 6, 7, 5, 6, 7, 8, 6, 7, 8, 9, 7, 8, 9, 10};

static inline uint32x4_t sadwxhx4d_neon_dotprod(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                                uint32_t ref_stride, uint32_t width, uint32_t height) {
    uint32x4_t sum_u32[4] = {vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0)};
    uint16x8_t sum_u16[4] = {vdupq_n_u16(0), vdupq_n_u16(0), vdupq_n_u16(0), vdupq_n_u16(0)};
    uint32x4_t sum4       = vdupq_n_u32(0);
    uint16x8_t sum        = vdupq_n_u16(0);

    do {
        int w = width;

        const uint8_t* src_ptr = src;
        const uint8_t* ref_ptr = ref;

        while (w >= 16) {
            const uint8x16_t s = vld1q_u8(src_ptr);
            sad16_neon_dotprod(s, vld1q_u8(ref_ptr + 0), &sum_u32[0]);
            sad16_neon_dotprod(s, vld1q_u8(ref_ptr + 1), &sum_u32[1]);
            sad16_neon_dotprod(s, vld1q_u8(ref_ptr + 2), &sum_u32[2]);
            sad16_neon_dotprod(s, vld1q_u8(ref_ptr + 3), &sum_u32[3]);

            src_ptr += 16;
            ref_ptr += 16;
            w -= 16;
        }

        if (w >= 8) {
            const uint8x8_t s = vld1_u8(src_ptr);
            sum_u16[0]        = vabal_u8(sum_u16[0], s, vld1_u8(ref_ptr + 0));
            sum_u16[1]        = vabal_u8(sum_u16[1], s, vld1_u8(ref_ptr + 1));
            sum_u16[2]        = vabal_u8(sum_u16[2], s, vld1_u8(ref_ptr + 2));
            sum_u16[3]        = vabal_u8(sum_u16[3], s, vld1_u8(ref_ptr + 3));

            src_ptr += 8;
            ref_ptr += 8;
            w -= 8;
        }

        if (w >= 4) {
            uint8x16_t perm_tbl = vld1q_u8(kPermTable);
            uint8x16_t s        = vreinterpretq_u8_u32(vld1q_dup_u32((const uint32_t*)src_ptr));
            uint8x16_t r        = vqtbl1q_u8(vld1q_u8(ref_ptr), perm_tbl);
            uint8x16_t abs      = vabdq_u8(s, r);
            sum4                = vdotq_u32(sum4, abs, vdupq_n_u8(1));
            src_ptr += 4;
            ref_ptr += 4;
            w -= 4;
        }

        while (--w >= 0) {
            uint8x8_t s = vld1_dup_u8(src_ptr + w);
            uint8x8_t r = load_u8_4x1(ref_ptr + w);
            sum         = vabal_u8(sum, s, r);
        }

        src += src_stride;
        ref += ref_stride;
    } while (--height != 0);

    sum_u32[0] = vpadalq_u16(sum_u32[0], sum_u16[0]);
    sum_u32[1] = vpadalq_u16(sum_u32[1], sum_u16[1]);
    sum_u32[2] = vpadalq_u16(sum_u32[2], sum_u16[2]);
    sum_u32[3] = vpadalq_u16(sum_u32[3], sum_u16[3]);
    return vaddq_u32(vaddw_u16(sum4, vget_low_u16(sum)), horizontal_add_4d_u32x4(sum_u32));
}

static inline void svt_sad_loop_kernelwxh_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                       uint32_t ref_stride, uint32_t block_width, uint32_t block_height,
                                                       uint64_t* best_sad, int16_t* x_search_center,
                                                       int16_t* y_search_center, uint32_t src_stride_raw,
                                                       int16_t search_area_width, int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (int16_t x_search_index = 0; x_search_index < search_area_width; x_search_index += 8) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4_0 = sadwxhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index, ref_stride, block_width, block_height);
            uint32x4_t sad4_1 = sadwxhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index + 4, ref_stride, block_width, block_height);
            update_best_sad_u32(sad4_0, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(sad4_1, best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernelwxh_small_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                             uint32_t ref_stride, uint32_t block_width,
                                                             uint32_t block_height, uint64_t* best_sad,
                                                             int16_t* x_search_center, int16_t* y_search_center,
                                                             uint32_t src_stride_raw, int16_t search_area_width,
                                                             int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4 = sadwxhx4d_neon_dotprod(
                src, src_stride, ref + x_search_index, ref_stride, block_width, block_height);
            update_best_sad_u32(sad4, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search spaces aligned along the width and store it in 'temp_sad'. */
            uint64_t temp_sad = sad_anywxh_neon_dotprod(
                src, src_stride, ref + x_search_index, ref_stride, block_width, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        ref += src_stride_raw;
    }
}

void svt_sad_loop_kernel_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                      uint32_t block_height, uint32_t block_width, uint64_t* best_sad,
                                      int16_t* x_search_center, int16_t* y_search_center, uint32_t src_stride_raw,
                                      uint8_t skip_search_line, int16_t search_area_width, int16_t search_area_height) {
    *best_sad = UINT64_MAX;
    /* Most of the time search_area_width is a multiple of 8, so specialize for this case so that we run only sad4d. */
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
            svt_sad_loop_kernel8xh_neon(src,
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
            svt_sad_loop_kernel16xh_neon_dotprod(src,
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
            svt_sad_loop_kernel32xh_neon_dotprod(src,
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
            svt_sad_loop_kernel48xh_neon_dotprod(src,
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
            svt_sad_loop_kernel64xh_neon_dotprod(src,
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
            svt_sad_loop_kernelwxh_neon_dotprod(src,
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
            svt_sad_loop_kernel8xh_neon(src,
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
            svt_sad_loop_kernel16xh_small_neon_dotprod(src,
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
            svt_sad_loop_kernel32xh_small_neon_dotprod(src,
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
            svt_sad_loop_kernel48xh_small_neon_dotprod(src,
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
            svt_sad_loop_kernel64xh_small_neon_dotprod(src,
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
            svt_sad_loop_kernelwxh_small_neon_dotprod(src,
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
