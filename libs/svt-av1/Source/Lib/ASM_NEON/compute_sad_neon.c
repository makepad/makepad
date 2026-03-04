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
#include "mcomp.h"
#include "mem_neon.h"
#include "sum_neon.h"
#include "utility.h"

static inline uint32_t sad40xh_neon(const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr, int ref_stride,
                                    int h) {
    uint32_t res1, res2;
    res1 = sad32xh_neon(src_ptr, src_stride, ref_ptr, ref_stride, h);
    res2 = sad8xh_neon(src_ptr + 32, src_stride, ref_ptr + 32, ref_stride, h);
    return (res1 + res2);
}

static inline uint32_t sad56xh_neon(const uint8_t* src_ptr, int src_stride, const uint8_t* ref_ptr, int ref_stride,
                                    int h) {
    uint32_t res1, res2;
    res1 = sad48xh_neon(src_ptr, src_stride, ref_ptr, ref_stride, h);
    res2 = sad8xh_neon(src_ptr + 48, src_stride, ref_ptr + 48, ref_stride, h);
    return (res1 + res2);
}

static inline uint32x4_t sadwxhx4d_large_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                              uint32_t ref_stride, uint32_t w, uint32_t h, uint32_t h_overflow) {
    uint32x4_t sum[4];
    uint16x8_t sum_lo[4], sum_hi[4];
    uint32_t   h_limit;

    sum[0] = vdupq_n_u32(0);
    sum[1] = vdupq_n_u32(0);
    sum[2] = vdupq_n_u32(0);
    sum[3] = vdupq_n_u32(0);

    h_limit = h > h_overflow ? h_overflow : h;

    uint32_t i = 0;
    do {
        sum_lo[0] = vdupq_n_u16(0);
        sum_lo[1] = vdupq_n_u16(0);
        sum_lo[2] = vdupq_n_u16(0);
        sum_lo[3] = vdupq_n_u16(0);
        sum_hi[0] = vdupq_n_u16(0);
        sum_hi[1] = vdupq_n_u16(0);
        sum_hi[2] = vdupq_n_u16(0);
        sum_hi[3] = vdupq_n_u16(0);
        do {
            const uint8_t*       loop_src       = src;
            const uint8_t*       loop_ref       = ref;
            const uint8_t* const loop_src_limit = loop_src + w;
            do {
                const uint8x16_t s0 = vld1q_u8(loop_src);
                sad16_neon(s0, vld1q_u8(loop_ref + 0), &sum_lo[0]);
                sad16_neon(s0, vld1q_u8(loop_ref + 1), &sum_lo[1]);
                sad16_neon(s0, vld1q_u8(loop_ref + 2), &sum_lo[2]);
                sad16_neon(s0, vld1q_u8(loop_ref + 3), &sum_lo[3]);

                const uint8x16_t s1 = vld1q_u8(loop_src + 16);
                sad16_neon(s1, vld1q_u8(loop_ref + 0 + 16), &sum_hi[0]);
                sad16_neon(s1, vld1q_u8(loop_ref + 1 + 16), &sum_hi[1]);
                sad16_neon(s1, vld1q_u8(loop_ref + 2 + 16), &sum_hi[2]);
                sad16_neon(s1, vld1q_u8(loop_ref + 3 + 16), &sum_hi[3]);

                loop_src += 32;
                loop_ref += 32;
            } while (loop_src < loop_src_limit);

            src += src_stride;
            ref += ref_stride;
        } while (++i < h_limit);

        sum[0] = vpadalq_u16(sum[0], sum_lo[0]);
        sum[0] = vpadalq_u16(sum[0], sum_hi[0]);
        sum[1] = vpadalq_u16(sum[1], sum_lo[1]);
        sum[1] = vpadalq_u16(sum[1], sum_hi[1]);
        sum[2] = vpadalq_u16(sum[2], sum_lo[2]);
        sum[2] = vpadalq_u16(sum[2], sum_hi[2]);
        sum[3] = vpadalq_u16(sum[3], sum_lo[3]);
        sum[3] = vpadalq_u16(sum[3], sum_hi[3]);

        h_limit += h_overflow;
    } while (i < h);

    return horizontal_add_4d_u32x4(sum);
}

static inline uint32x4_t sad128xhx4d_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                          uint32_t ref_stride, uint32_t h) {
    return sadwxhx4d_large_neon(src, src_stride, ref, ref_stride, 128, h, 32);
}

static inline uint32x4_t sad64xhx4d_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                         uint32_t ref_stride, uint32_t h) {
    return sadwxhx4d_large_neon(src, src_stride, ref, ref_stride, 64, h, 64);
}

static inline uint32x4_t sad32xhx4d_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                         uint32_t ref_stride, uint32_t h) {
    uint16x8_t sum_lo[4], sum_hi[4];

    const uint8x16_t s0_init = vld1q_u8(src);
    sum_lo[0]                = sad16_neon_init(s0_init, vld1q_u8(ref + 0));
    sum_lo[1]                = sad16_neon_init(s0_init, vld1q_u8(ref + 1));
    sum_lo[2]                = sad16_neon_init(s0_init, vld1q_u8(ref + 2));
    sum_lo[3]                = sad16_neon_init(s0_init, vld1q_u8(ref + 3));

    const uint8x16_t s1_init = vld1q_u8(src + 16);
    sum_hi[0]                = sad16_neon_init(s1_init, vld1q_u8(ref + 0 + 16));
    sum_hi[1]                = sad16_neon_init(s1_init, vld1q_u8(ref + 1 + 16));
    sum_hi[2]                = sad16_neon_init(s1_init, vld1q_u8(ref + 2 + 16));
    sum_hi[3]                = sad16_neon_init(s1_init, vld1q_u8(ref + 3 + 16));

    const uint8_t* const src_limit = src + src_stride * h;

    src += src_stride;
    ref += ref_stride;

    while (src < src_limit) {
        const uint8x16_t s0 = vld1q_u8(src);
        sad16_neon(s0, vld1q_u8(ref + 0), &sum_lo[0]);
        sad16_neon(s0, vld1q_u8(ref + 1), &sum_lo[1]);
        sad16_neon(s0, vld1q_u8(ref + 2), &sum_lo[2]);
        sad16_neon(s0, vld1q_u8(ref + 3), &sum_lo[3]);

        const uint8x16_t s1 = vld1q_u8(src + 16);
        sad16_neon(s1, vld1q_u8(ref + 0 + 16), &sum_hi[0]);
        sad16_neon(s1, vld1q_u8(ref + 1 + 16), &sum_hi[1]);
        sad16_neon(s1, vld1q_u8(ref + 2 + 16), &sum_hi[2]);
        sad16_neon(s1, vld1q_u8(ref + 3 + 16), &sum_hi[3]);

        src += src_stride;
        ref += ref_stride;
    }

    return horizontal_long_add_4d_u16x8(sum_lo, sum_hi);
}

static inline uint32x4_t sad16xhx4d_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                         uint32_t ref_stride, uint32_t h) {
    uint16x8_t sum_u16[4];
    uint32x4_t sum_u32[4];

    const uint8x16_t s_init = vld1q_u8(src);
    sum_u16[0]              = sad16_neon_init(s_init, vld1q_u8(ref + 0));
    sum_u16[1]              = sad16_neon_init(s_init, vld1q_u8(ref + 1));
    sum_u16[2]              = sad16_neon_init(s_init, vld1q_u8(ref + 2));
    sum_u16[3]              = sad16_neon_init(s_init, vld1q_u8(ref + 3));

    const uint8_t* const src_limit = src + src_stride * h;

    src += src_stride;
    ref += ref_stride;

    while (src < src_limit) {
        const uint8x16_t s = vld1q_u8(src);
        sad16_neon(s, vld1q_u8(ref + 0), &sum_u16[0]);
        sad16_neon(s, vld1q_u8(ref + 1), &sum_u16[1]);
        sad16_neon(s, vld1q_u8(ref + 2), &sum_u16[2]);
        sad16_neon(s, vld1q_u8(ref + 3), &sum_u16[3]);

        src += src_stride;
        ref += ref_stride;
    }

    sum_u32[0] = vpaddlq_u16(sum_u16[0]);
    sum_u32[1] = vpaddlq_u16(sum_u16[1]);
    sum_u32[2] = vpaddlq_u16(sum_u16[2]);
    sum_u32[3] = vpaddlq_u16(sum_u16[3]);

    return horizontal_add_4d_u32x4(sum_u32);
}

static inline uint32x4_t sad24xhx4d_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                         uint32_t ref_stride, uint32_t h) {
    uint32x4_t sad4_0, sad4_1;
    sad4_0 = sad16xhx4d_neon(src + 0, src_stride, ref + 0, ref_stride, h);
    sad4_1 = sad8xhx4d_neon(src + 16, src_stride, ref + 16, ref_stride, h);
    return vaddq_u32(sad4_0, sad4_1);
}

static inline uint32x4_t sad48xhx4d_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                         uint32_t ref_stride, uint32_t h) {
    uint32x4_t sad4_0, sad4_1;
    sad4_0 = sad32xhx4d_neon(src + 0, src_stride, ref + 0, ref_stride, h);
    sad4_1 = sad16xhx4d_neon(src + 32, src_stride, ref + 32, ref_stride, h);
    return vaddq_u32(sad4_0, sad4_1);
}

static inline void svt_sad_loop_kernel16xh_neon(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                uint32_t block_height, uint64_t* best_sad, int16_t* x_search_center,
                                                int16_t* y_search_center, uint32_t src_stride_raw,
                                                uint8_t skip_search_line, int16_t search_area_width,
                                                int16_t search_area_height) {
    int16_t    x_search_index, y_search_index;
    uint32x4_t sad4;
    uint64_t   temp_sad;

    int y_search_start = 0;
    int y_search_step  = 1;

    if (block_height <= 16 && skip_search_line) {
        ref += src_stride_raw;
        src_stride_raw *= 2;
        y_search_start = 1;
        y_search_step  = 2;
    }

    for (y_search_index = y_search_start; y_search_index < search_area_height; y_search_index += y_search_step) {
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            sad4 = sad16xhx4d_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad_u32(sad4, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search spaces aligned along the width and store it in 'temp_sad'. */
            temp_sad = sad16xh_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel24xh_neon(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                uint32_t block_height, uint64_t* best_sad, int16_t* x_search_center,
                                                int16_t* y_search_center, uint32_t src_stride_raw,
                                                int16_t search_area_width, int16_t search_area_height) {
    int16_t    x_search_index, y_search_index;
    uint32x4_t sad4;
    uint64_t   temp_sad;

    for (y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            sad4 = sad24xhx4d_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad_u32(sad4, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search spaces aligned along the width and store it in 'temp_sad'. */
            temp_sad = sad24xh_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel32xh_neon(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                uint32_t block_height, uint64_t* best_sad, int16_t* x_search_center,
                                                int16_t* y_search_center, uint32_t src_stride_raw,
                                                int16_t search_area_width, int16_t search_area_height) {
    int16_t    x_search_index, y_search_index;
    uint32x4_t sad4;
    uint64_t   temp_sad;

    for (y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            sad4 = sad32xhx4d_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad_u32(sad4, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search spaces aligned along the width and store it in 'temp_sad'. */
            temp_sad = sad32xh_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel48xh_neon(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                uint32_t block_height, uint64_t* best_sad, int16_t* x_search_center,
                                                int16_t* y_search_center, uint32_t src_stride_raw,
                                                int16_t search_area_width, int16_t search_area_height) {
    int16_t    x_search_index, y_search_index;
    uint32x4_t sad4;
    uint64_t   temp_sad;

    for (y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            sad4 = sad48xhx4d_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad_u32(sad4, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search spaces aligned along the width and store it in 'temp_sad'. */
            temp_sad = sad48xh_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel64xh_neon(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                uint32_t block_height, uint64_t* best_sad, int16_t* x_search_center,
                                                int16_t* y_search_center, uint32_t src_stride_raw,
                                                int16_t search_area_width, int16_t search_area_height) {
    int16_t    x_search_index, y_search_index;
    uint32x4_t sad4;
    uint64_t   temp_sad;

    for (y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            sad4 = sad64xhx4d_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad_u32(sad4, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search spaces aligned along the width and store it in 'temp_sad'. */
            temp_sad = sad64xh_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel128xh_neon(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                 uint32_t block_height, uint64_t* best_sad, int16_t* x_search_center,
                                                 int16_t* y_search_center, uint32_t src_stride_raw,
                                                 int16_t search_area_width, int16_t search_area_height) {
    int16_t    x_search_index, y_search_index;
    uint32x4_t sad4;
    uint64_t   temp_sad;

    for (y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            sad4 = sad128xhx4d_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad_u32(sad4, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search spaces aligned along the width and store it in 'temp_sad'. */
            temp_sad = sad128xh_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        ref += src_stride_raw;
    }
}

DECLARE_ALIGNED(16, static const uint8_t, kPermTable4xh[16]) = {0, 1, 2, 3, 1, 2, 3, 4, 2, 3, 4, 5, 3, 4, 5, 6};

static inline uint32x4_t sadwxhx4d_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                        uint32_t ref_stride, uint32_t width, uint32_t height) {
    uint32x4_t sum_u32[4] = {vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0), vdupq_n_u32(0)};
    uint32x4_t sum4       = vdupq_n_u32(0);
    uint16x8_t sum        = vdupq_n_u16(0);

    // We can accumulate 257 absolute differences in a 16-bit element before
    // it overflows. Given that we're accumulating in 8 16-bit elements we
    // therefore need width * height < (257 * 8). (This isn't quite true as some
    // elements in the tail loops have a different accumulator, but it's a good
    // enough approximation).
    uint32_t h_overflow = (257 * 8) / width;
    uint32_t h_limit    = h_overflow >= height ? height : h_overflow;

    uint32_t i = 0;
    do {
        uint16x8_t sum_u16[4] = {vdupq_n_u16(0), vdupq_n_u16(0), vdupq_n_u16(0), vdupq_n_u16(0)};

        do {
            int w = width;

            const uint8_t* src_ptr = src;
            const uint8_t* ref_ptr = ref;

            while (w >= 16) {
                const uint8x16_t s = vld1q_u8(src_ptr);
                sad16_neon(s, vld1q_u8(ref_ptr + 0), &sum_u16[0]);
                sad16_neon(s, vld1q_u8(ref_ptr + 1), &sum_u16[1]);
                sad16_neon(s, vld1q_u8(ref_ptr + 2), &sum_u16[2]);
                sad16_neon(s, vld1q_u8(ref_ptr + 3), &sum_u16[3]);

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
                uint8x16_t perm_tbl = vld1q_u8(kPermTable4xh);
                uint8x16_t s        = vreinterpretq_u8_u32(vld1q_dup_u32((const uint32_t*)src_ptr));

                uint8x16_t r   = vqtbl1q_u8(vld1q_u8(ref_ptr), perm_tbl);
                uint8x16_t abs = vabdq_u8(s, r);

                uint16x8_t abs_u16 = vpaddlq_u8(abs);
                sum4               = vpadalq_u16(sum4, abs_u16);

                src_ptr += 4;
                ref_ptr += 4;
                w -= 4;
            }

            while (w != 0) {
                uint8x8_t s = vld1_dup_u8(src_ptr + w - 1);
                uint8x8_t r = load_u8_4x1(ref_ptr + w - 1);
                sum         = vabal_u8(sum, s, r);

                w--;
            }

            src += src_stride;
            ref += ref_stride;
        } while (++i < h_limit);

        sum_u32[0] = vpadalq_u16(sum_u32[0], sum_u16[0]);
        sum_u32[1] = vpadalq_u16(sum_u32[1], sum_u16[1]);
        sum_u32[2] = vpadalq_u16(sum_u32[2], sum_u16[2]);
        sum_u32[3] = vpadalq_u16(sum_u32[3], sum_u16[3]);

        uint32_t h_inc = h_limit + h_overflow < height ? h_overflow : height - h_limit;
        h_limit += h_inc;
    } while (i < height);

    return vaddq_u32(vaddw_u16(sum4, vget_low_u16(sum)), horizontal_add_4d_u32x4(sum_u32));
}

static inline void svt_sad_loop_kernelwxh_neon(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                               uint32_t block_width, uint32_t block_height, uint64_t* best_sad,
                                               int16_t* x_search_center, int16_t* y_search_center,
                                               uint32_t src_stride_raw, int16_t search_area_width,
                                               int16_t search_area_height) {
    for (int16_t y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4 = sadwxhx4d_neon(
                src, src_stride, ref + x_search_index, ref_stride, block_width, block_height);
            update_best_sad_u32(sad4, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        for (; x_search_index < search_area_width; x_search_index++) {
            /* Get the SAD of 1 search spaces aligned along the width and store it in 'temp_sad'. */
            uint64_t temp_sad = sadwxh_neon(
                src, src_stride, ref + x_search_index, ref_stride, block_width, block_height);
            update_best_sad(temp_sad, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }
        ref += src_stride_raw;
    }
}

void svt_sad_loop_kernel_neon(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                              uint32_t block_height, uint32_t block_width, uint64_t* best_sad, int16_t* x_search_center,
                              int16_t* y_search_center, uint32_t src_stride_raw, uint8_t skip_search_line,
                              int16_t search_area_width, int16_t search_area_height) {
    *best_sad = UINT64_MAX;
    switch (block_width) {
    case 4: {
        svt_sad_loop_kernel4xh_neon(src,
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
        svt_sad_loop_kernel6xh_neon(src,
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
        svt_sad_loop_kernel12xh_neon(src,
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
        svt_sad_loop_kernel16xh_neon(src,
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
        svt_sad_loop_kernel24xh_neon(src,
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
        svt_sad_loop_kernel32xh_neon(src,
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
        svt_sad_loop_kernel48xh_neon(src,
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
        svt_sad_loop_kernel64xh_neon(src,
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
    case 128: {
        svt_sad_loop_kernel128xh_neon(src,
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
        svt_sad_loop_kernelwxh_neon(src,
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

static inline uint32x4_t get_mv_cost_vector(const svt_mv_cost_param* mv_cost_params, int16_t row, int16_t col,
                                            int16_t mvx, int16_t mvy, int16_t search_position_start_x,
                                            int16_t search_position_start_y) {
    const Mv baseMv = {
        {(int16_t)(mvx + (search_position_start_x + col) * 8), (int16_t)(mvy + (search_position_start_y + row) * 8)}};
    const Mv mvs[4] = {{{baseMv.y, baseMv.x + 8 * 0}},
                       {{baseMv.y, baseMv.x + 8 * 1}},
                       {{baseMv.y, baseMv.x + 8 * 2}},
                       {{baseMv.y, baseMv.x + 8 * 3}}};
    uint32_t costs[4];
    costs[0] = (uint32_t)svt_aom_fp_mv_err_cost(&mvs[0], mv_cost_params);
    costs[1] = (uint32_t)svt_aom_fp_mv_err_cost(&mvs[1], mv_cost_params);
    costs[2] = (uint32_t)svt_aom_fp_mv_err_cost(&mvs[2], mv_cost_params);
    costs[3] = (uint32_t)svt_aom_fp_mv_err_cost(&mvs[3], mv_cost_params);

    return vld1q_u32(costs);
}

static inline void update_best_cost_u32(uint32x4_t cost4, uint32_t* best_cost, int16_t* x_search_center,
                                        int16_t* y_search_center, int16_t x_search_index, int16_t y_search_index,
                                        int16_t mvx, int16_t mvy) {
    uint64_t temp_cost;

    /* Find the minimum SAD value out of the 4 search spaces. */
    temp_cost = vminvq_u32(cost4);

    if (temp_cost < *best_cost) {
        *best_cost       = temp_cost;
        *x_search_center = mvx + (int16_t)(x_search_index + findposq_u32(cost4, temp_cost)) * 8;
        *y_search_center = mvy + y_search_index * 8;
    }
}

static inline void svt_pme_sad_loop_kernel4xh_neon(const svt_mv_cost_param* mv_cost_params, uint8_t* src,
                                                   uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                   uint32_t block_height, uint32_t* best_cost, int16_t* best_mvx,
                                                   int16_t* best_mvy, int16_t search_position_start_x,
                                                   int16_t search_position_start_y, int16_t search_area_width,
                                                   int16_t search_area_height, int16_t search_step, int16_t mvx,
                                                   int16_t mvy) {
    int16_t        i, j;
    const uint8_t *p_ref, *p_src;
    uint16x8_t     res8;
    uint32x4_t     cost4_low, cost4_high, res4_1, res4_2;

    for (i = 0; i < search_area_height; i += search_step) {
        for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
            p_src                = src;
            p_ref                = ref + j;
            res8                 = sad4xhx8d_neon(p_src, src_stride, p_ref, ref_stride, block_height);
            uint32x4_t res8_low  = vmovl_u16(vget_low_u16(res8));
            uint32x4_t res8_high = vmovl_u16(vget_high_u16(res8));
            cost4_low            = get_mv_cost_vector(
                mv_cost_params, i, j, mvx, mvy, search_position_start_x, search_position_start_y);
            cost4_high = get_mv_cost_vector(
                mv_cost_params, i, j + 4, mvx, mvy, search_position_start_x, search_position_start_y);
            res4_1 = vaddq_u32(res8_low, cost4_low);
            res4_2 = vaddq_u32(res8_high, cost4_high);
            update_best_cost_u32(res4_1,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
            update_best_cost_u32(res4_2,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + 4 + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
        }

        ref += search_step * ref_stride;
    }
}

static inline void svt_pme_sad_loop_kernel8xh_neon(const svt_mv_cost_param* mv_cost_params, uint8_t* src,
                                                   uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                   uint32_t block_height, uint32_t* best_cost, int16_t* best_mvx,
                                                   int16_t* best_mvy, int16_t search_position_start_x,
                                                   int16_t search_position_start_y, int16_t search_area_width,
                                                   int16_t search_area_height, int16_t search_step, int16_t mvx,
                                                   int16_t mvy) {
    int16_t        i, j;
    const uint8_t *p_ref, *p_src;
    uint32x4_t     sad4_low, sad4_high, cost4_low, cost4_high, res4_1, res4_2;

    for (i = 0; i < search_area_height; i += search_step) {
        for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
            p_src     = src;
            p_ref     = ref + j;
            sad4_low  = sad8xhx4d_neon(p_src, src_stride, p_ref, ref_stride, block_height);
            sad4_high = sad8xhx4d_neon(p_src, src_stride, p_ref + 4, ref_stride, block_height);

            cost4_low = get_mv_cost_vector(
                mv_cost_params, i, j, mvx, mvy, search_position_start_x, search_position_start_y);
            cost4_high = get_mv_cost_vector(
                mv_cost_params, i, j + 4, mvx, mvy, search_position_start_x, search_position_start_y);
            res4_1 = vaddq_u32(sad4_low, cost4_low);
            res4_2 = vaddq_u32(sad4_high, cost4_high);
            update_best_cost_u32(res4_1,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
            update_best_cost_u32(res4_2,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + 4 + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
        }

        ref += search_step * ref_stride;
    }
}

static inline void svt_pme_sad_loop_kernel16xh_neon(const svt_mv_cost_param* mv_cost_params, uint8_t* src,
                                                    uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                    uint32_t block_height, uint32_t* best_cost, int16_t* best_mvx,
                                                    int16_t* best_mvy, int16_t search_position_start_x,
                                                    int16_t search_position_start_y, int16_t search_area_width,
                                                    int16_t search_area_height, int16_t search_step, int16_t mvx,
                                                    int16_t mvy) {
    int16_t        i, j;
    const uint8_t *p_ref, *p_src;
    uint32x4_t     sad4_low, sad4_high, cost4_low, cost4_high, res4_1, res4_2;

    for (i = 0; i < search_area_height; i += search_step) {
        for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
            p_src     = src;
            p_ref     = ref + j;
            sad4_low  = sad16xhx4d_neon(p_src, src_stride, p_ref, ref_stride, block_height);
            sad4_high = sad16xhx4d_neon(p_src, src_stride, p_ref + 4, ref_stride, block_height);

            cost4_low = get_mv_cost_vector(
                mv_cost_params, i, j, mvx, mvy, search_position_start_x, search_position_start_y);
            cost4_high = get_mv_cost_vector(
                mv_cost_params, i, j + 4, mvx, mvy, search_position_start_x, search_position_start_y);
            res4_1 = vaddq_u32(sad4_low, cost4_low);
            res4_2 = vaddq_u32(sad4_high, cost4_high);
            update_best_cost_u32(res4_1,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
            update_best_cost_u32(res4_2,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + 4 + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
        }

        ref += search_step * ref_stride;
    }
}

static inline void svt_pme_sad_loop_kernel24xh_neon(const svt_mv_cost_param* mv_cost_params, uint8_t* src,
                                                    uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                    uint32_t block_height, uint32_t* best_cost, int16_t* best_mvx,
                                                    int16_t* best_mvy, int16_t search_position_start_x,
                                                    int16_t search_position_start_y, int16_t search_area_width,
                                                    int16_t search_area_height, int16_t search_step, int16_t mvx,
                                                    int16_t mvy) {
    int16_t        i, j;
    const uint8_t *p_ref, *p_src;
    uint32x4_t     sad4_low, sad4_high, cost4_low, cost4_high, res4_1, res4_2;

    for (i = 0; i < search_area_height; i += search_step) {
        for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
            p_src     = src;
            p_ref     = ref + j;
            sad4_low  = sad24xhx4d_neon(p_src, src_stride, p_ref, ref_stride, block_height);
            sad4_high = sad24xhx4d_neon(p_src, src_stride, p_ref + 4, ref_stride, block_height);

            cost4_low = get_mv_cost_vector(
                mv_cost_params, i, j, mvx, mvy, search_position_start_x, search_position_start_y);
            cost4_high = get_mv_cost_vector(
                mv_cost_params, i, j + 4, mvx, mvy, search_position_start_x, search_position_start_y);
            res4_1 = vaddq_u32(sad4_low, cost4_low);
            res4_2 = vaddq_u32(sad4_high, cost4_high);
            update_best_cost_u32(res4_1,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
            update_best_cost_u32(res4_2,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + 4 + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
        }

        ref += search_step * ref_stride;
    }
}

static inline void svt_pme_sad_loop_kernel32xh_neon(const svt_mv_cost_param* mv_cost_params, uint8_t* src,
                                                    uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                    uint32_t block_height, uint32_t* best_cost, int16_t* best_mvx,
                                                    int16_t* best_mvy, int16_t search_position_start_x,
                                                    int16_t search_position_start_y, int16_t search_area_width,
                                                    int16_t search_area_height, int16_t search_step, int16_t mvx,
                                                    int16_t mvy) {
    int16_t        i, j;
    const uint8_t *p_ref, *p_src;
    uint32x4_t     sad4_low, sad4_high, cost4_low, cost4_high, res4_1, res4_2;

    for (i = 0; i < search_area_height; i += search_step) {
        for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
            p_src     = src;
            p_ref     = ref + j;
            sad4_low  = sad32xhx4d_neon(p_src, src_stride, p_ref, ref_stride, block_height);
            sad4_high = sad32xhx4d_neon(p_src, src_stride, p_ref + 4, ref_stride, block_height);

            cost4_low = get_mv_cost_vector(
                mv_cost_params, i, j, mvx, mvy, search_position_start_x, search_position_start_y);
            cost4_high = get_mv_cost_vector(
                mv_cost_params, i, j + 4, mvx, mvy, search_position_start_x, search_position_start_y);
            res4_1 = vaddq_u32(sad4_low, cost4_low);
            res4_2 = vaddq_u32(sad4_high, cost4_high);
            update_best_cost_u32(res4_1,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
            update_best_cost_u32(res4_2,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + 4 + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
        }

        ref += search_step * ref_stride;
    }
}

static inline void svt_pme_sad_loop_kernel48xh_neon(const svt_mv_cost_param* mv_cost_params, uint8_t* src,
                                                    uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                    uint32_t block_height, uint32_t* best_cost, int16_t* best_mvx,
                                                    int16_t* best_mvy, int16_t search_position_start_x,
                                                    int16_t search_position_start_y, int16_t search_area_width,
                                                    int16_t search_area_height, int16_t search_step, int16_t mvx,
                                                    int16_t mvy) {
    int16_t        i, j;
    const uint8_t *p_ref, *p_src;
    uint32x4_t     sad4_low, sad4_high, cost4_low, cost4_high, res4_1, res4_2;

    for (i = 0; i < search_area_height; i += search_step) {
        for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
            p_src     = src;
            p_ref     = ref + j;
            sad4_low  = sad48xhx4d_neon(p_src, src_stride, p_ref, ref_stride, block_height);
            sad4_high = sad48xhx4d_neon(p_src, src_stride, p_ref + 4, ref_stride, block_height);

            cost4_low = get_mv_cost_vector(
                mv_cost_params, i, j, mvx, mvy, search_position_start_x, search_position_start_y);
            cost4_high = get_mv_cost_vector(
                mv_cost_params, i, j + 4, mvx, mvy, search_position_start_x, search_position_start_y);
            res4_1 = vaddq_u32(sad4_low, cost4_low);
            res4_2 = vaddq_u32(sad4_high, cost4_high);
            update_best_cost_u32(res4_1,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
            update_best_cost_u32(res4_2,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + 4 + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
        }

        ref += search_step * ref_stride;
    }
}

static inline void svt_pme_sad_loop_kernel64xh_neon(const svt_mv_cost_param* mv_cost_params, uint8_t* src,
                                                    uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                    uint32_t block_height, uint32_t* best_cost, int16_t* best_mvx,
                                                    int16_t* best_mvy, int16_t search_position_start_x,
                                                    int16_t search_position_start_y, int16_t search_area_width,
                                                    int16_t search_area_height, int16_t search_step, int16_t mvx,
                                                    int16_t mvy) {
    int16_t        i, j;
    const uint8_t *p_ref, *p_src;
    uint32x4_t     sad4_low, sad4_high, cost4_low, cost4_high, res4_1, res4_2;

    for (i = 0; i < search_area_height; i += search_step) {
        for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
            p_src     = src;
            p_ref     = ref + j;
            sad4_low  = sad64xhx4d_neon(p_src, src_stride, p_ref, ref_stride, block_height);
            sad4_high = sad64xhx4d_neon(p_src, src_stride, p_ref + 4, ref_stride, block_height);

            cost4_low = get_mv_cost_vector(
                mv_cost_params, i, j, mvx, mvy, search_position_start_x, search_position_start_y);
            cost4_high = get_mv_cost_vector(
                mv_cost_params, i, j + 4, mvx, mvy, search_position_start_x, search_position_start_y);

            res4_1 = vaddq_u32(sad4_low, cost4_low);
            res4_2 = vaddq_u32(sad4_high, cost4_high);
            update_best_cost_u32(res4_1,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
            update_best_cost_u32(res4_2,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + 4 + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
        }

        ref += search_step * ref_stride;
    }
}

static inline void svt_pme_sad_loop_kernel128xh_neon(const svt_mv_cost_param* mv_cost_params, uint8_t* src,
                                                     uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                     uint32_t block_height, uint32_t* best_cost, int16_t* best_mvx,
                                                     int16_t* best_mvy, int16_t search_position_start_x,
                                                     int16_t search_position_start_y, int16_t search_area_width,
                                                     int16_t search_area_height, int16_t search_step, int16_t mvx,
                                                     int16_t mvy) {
    int16_t        i, j;
    const uint8_t *p_ref, *p_src;
    uint32x4_t     sad4_low, sad4_high, cost4_low, cost4_high, res4_1, res4_2;

    for (i = 0; i < search_area_height; i += search_step) {
        for (j = 0; j <= search_area_width - 8; j += (8 + search_step - 1)) {
            p_src     = src;
            p_ref     = ref + j;
            sad4_low  = sad128xhx4d_neon(p_src, src_stride, p_ref, ref_stride, block_height);
            sad4_high = sad128xhx4d_neon(p_src, src_stride, p_ref + 4, ref_stride, block_height);

            cost4_low = get_mv_cost_vector(
                mv_cost_params, i, j, mvx, mvy, search_position_start_x, search_position_start_y);
            cost4_high = get_mv_cost_vector(
                mv_cost_params, i, j + 4, mvx, mvy, search_position_start_x, search_position_start_y);
            res4_1 = vaddq_u32(sad4_low, cost4_low);
            res4_2 = vaddq_u32(sad4_high, cost4_high);
            update_best_cost_u32(res4_1,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
            update_best_cost_u32(res4_2,
                                 best_cost,
                                 best_mvx,
                                 best_mvy,
                                 j + 4 + search_position_start_x,
                                 i + search_position_start_y,
                                 mvx,
                                 mvy);
        }

        ref += search_step * ref_stride;
    }
}

void svt_pme_sad_loop_kernel_neon(const svt_mv_cost_param* mv_cost_params, uint8_t* src, uint32_t src_stride,
                                  uint8_t* ref, uint32_t ref_stride, uint32_t block_height, uint32_t block_width,
                                  uint32_t* best_cost, int16_t* best_mvx, int16_t* best_mvy,
                                  int16_t search_position_start_x, int16_t search_position_start_y,
                                  int16_t search_area_width, int16_t search_area_height, int16_t search_step,
                                  int16_t mvx, int16_t mvy) {
    switch (block_width) {
    case 4: {
        svt_pme_sad_loop_kernel4xh_neon(mv_cost_params,
                                        src,
                                        src_stride,
                                        ref,
                                        ref_stride,
                                        block_height,
                                        best_cost,
                                        best_mvx,
                                        best_mvy,
                                        search_position_start_x,
                                        search_position_start_y,
                                        search_area_width,
                                        search_area_height,
                                        search_step,
                                        mvx,
                                        mvy);
        break;
    }
    case 8: {
        svt_pme_sad_loop_kernel8xh_neon(mv_cost_params,
                                        src,
                                        src_stride,
                                        ref,
                                        ref_stride,
                                        block_height,
                                        best_cost,
                                        best_mvx,
                                        best_mvy,
                                        search_position_start_x,
                                        search_position_start_y,
                                        search_area_width,
                                        search_area_height,
                                        search_step,
                                        mvx,
                                        mvy);
        break;
    }
    case 16: {
        svt_pme_sad_loop_kernel16xh_neon(mv_cost_params,
                                         src,
                                         src_stride,
                                         ref,
                                         ref_stride,
                                         block_height,
                                         best_cost,
                                         best_mvx,
                                         best_mvy,
                                         search_position_start_x,
                                         search_position_start_y,
                                         search_area_width,
                                         search_area_height,
                                         search_step,
                                         mvx,
                                         mvy);
        break;
    }
    case 24: {
        svt_pme_sad_loop_kernel24xh_neon(mv_cost_params,
                                         src,
                                         src_stride,
                                         ref,
                                         ref_stride,
                                         block_height,
                                         best_cost,
                                         best_mvx,
                                         best_mvy,
                                         search_position_start_x,
                                         search_position_start_y,
                                         search_area_width,
                                         search_area_height,
                                         search_step,
                                         mvx,
                                         mvy);
        break;
    }
    case 32: {
        svt_pme_sad_loop_kernel32xh_neon(mv_cost_params,
                                         src,
                                         src_stride,
                                         ref,
                                         ref_stride,
                                         block_height,
                                         best_cost,
                                         best_mvx,
                                         best_mvy,
                                         search_position_start_x,
                                         search_position_start_y,
                                         search_area_width,
                                         search_area_height,
                                         search_step,
                                         mvx,
                                         mvy);
        break;
    }
    case 48: {
        svt_pme_sad_loop_kernel48xh_neon(mv_cost_params,
                                         src,
                                         src_stride,
                                         ref,
                                         ref_stride,
                                         block_height,
                                         best_cost,
                                         best_mvx,
                                         best_mvy,
                                         search_position_start_x,
                                         search_position_start_y,
                                         search_area_width,
                                         search_area_height,
                                         search_step,
                                         mvx,
                                         mvy);
        break;
    }
    case 64: {
        svt_pme_sad_loop_kernel64xh_neon(mv_cost_params,
                                         src,
                                         src_stride,
                                         ref,
                                         ref_stride,
                                         block_height,
                                         best_cost,
                                         best_mvx,
                                         best_mvy,
                                         search_position_start_x,
                                         search_position_start_y,
                                         search_area_width,
                                         search_area_height,
                                         search_step,
                                         mvx,
                                         mvy);
        break;
    }
    case 128: {
        svt_pme_sad_loop_kernel128xh_neon(mv_cost_params,
                                          src,
                                          src_stride,
                                          ref,
                                          ref_stride,
                                          block_height,
                                          best_cost,
                                          best_mvx,
                                          best_mvy,
                                          search_position_start_x,
                                          search_position_start_y,
                                          search_area_width,
                                          search_area_height,
                                          search_step,
                                          mvx,
                                          mvy);
        break;
    }
    default: {
        svt_pme_sad_loop_kernel_c(mv_cost_params,
                                  src,
                                  src_stride,
                                  ref,
                                  ref_stride,
                                  block_height,
                                  block_width,
                                  best_cost,
                                  best_mvx,
                                  best_mvy,
                                  search_position_start_x,
                                  search_position_start_y,
                                  search_area_width,
                                  search_area_height,
                                  search_step,
                                  mvx,
                                  mvy);
        break;
    }
    }
}

uint32_t svt_nxm_sad_kernel_helper_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                        uint32_t ref_stride, uint32_t height, uint32_t width) {
    uint32_t res = 0;
    switch (width) {
    case 4: {
        res = sad4xh_neon(src, src_stride, ref, ref_stride, height);
        break;
    }
    case 8: {
        res = sad8xh_neon(src, src_stride, ref, ref_stride, height);
        break;
    }
    case 16: {
        res = sad16xh_neon(src, src_stride, ref, ref_stride, height);
        break;
    }
    case 24: {
        res = sad24xh_neon(src, src_stride, ref, ref_stride, height);
        break;
    }
    case 32: {
        res = sad32xh_neon(src, src_stride, ref, ref_stride, height);
        break;
    }
    case 40: {
        res = sad40xh_neon(src, src_stride, ref, ref_stride, height);
        break;
    }
    case 48: {
        res = sad48xh_neon(src, src_stride, ref, ref_stride, height);
        break;
    }
    case 56: {
        res = sad56xh_neon(src, src_stride, ref, ref_stride, height);
        break;
    }
    case 64: {
        res = sad64xh_neon(src, src_stride, ref, ref_stride, height);
        break;
    }
    case 128: {
        res = sad128xh_neon(src, src_stride, ref, ref_stride, height);
        break;
    }
    default: {
        res = svt_nxm_sad_kernel_helper_c(src, src_stride, ref, ref_stride, height, width);
        break;
    }
    }
    return res;
}

static inline void compute_4sad_neon(const uint32_t p_sad16x16[4][8], uint32_t* p_sad32x32, uint32x4_t* sad0,
                                     uint32x4_t* sad1) {
    uint32x4_t tmp0 = vaddq_u32(vld1q_u32(p_sad16x16[0]), vld1q_u32(p_sad16x16[1]));
    uint32x4_t tmp1 = vaddq_u32(vld1q_u32(p_sad16x16[2]), vld1q_u32(p_sad16x16[3]));
    *sad0           = vaddq_u32(tmp0, tmp1);

    tmp0  = vaddq_u32(vld1q_u32(p_sad16x16[0] + 4), vld1q_u32(p_sad16x16[1] + 4));
    tmp1  = vaddq_u32(vld1q_u32(p_sad16x16[2] + 4), vld1q_u32(p_sad16x16[3] + 4));
    *sad1 = vaddq_u32(tmp0, tmp1);

    store_u32_4x2(p_sad32x32, 4, *sad0, *sad1);
}

void svt_ext_eight_sad_calculation_32x32_64x64_neon(const uint32_t p_sad16x16[16][8], uint32_t* p_best_sad_32x32,
                                                    uint32_t* p_best_sad_64x64, uint32_t* p_best_mv32x32,
                                                    uint32_t* p_best_mv64x64, uint32_t mv, uint32_t p_sad32x32[4][8]) {
    uint32x4_t sad32_a1, sad32_a2, sad32_b1, sad32_b2, sad32_c1, sad32_c2, sad32_d1, sad32_d2;
    compute_4sad_neon(&p_sad16x16[0], &p_sad32x32[0][0], &sad32_a1, &sad32_a2);
    compute_4sad_neon(&p_sad16x16[4], &p_sad32x32[1][0], &sad32_b1, &sad32_b2);
    compute_4sad_neon(&p_sad16x16[8], &p_sad32x32[2][0], &sad32_c1, &sad32_c2);
    compute_4sad_neon(&p_sad16x16[12], &p_sad32x32[3][0], &sad32_d1, &sad32_d2);

    DECLARE_ALIGNED(32, uint32_t, p_sad64x64[8]);
    uint32x4_t tmp0 = vaddq_u32(sad32_a1, sad32_b1);
    uint32x4_t tmp1 = vaddq_u32(sad32_c1, sad32_d1);
    vst1q_u32(p_sad64x64, vaddq_u32(tmp0, tmp1));
    tmp0 = vaddq_u32(sad32_a2, sad32_b2);
    tmp1 = vaddq_u32(sad32_c2, sad32_d2);
    vst1q_u32((p_sad64x64 + 4), vaddq_u32(tmp0, tmp1));

    DECLARE_ALIGNED(32, uint32_t, computed_idx[8]);
    uint32x4_t       search_idx = vmovl_u16(vcreate_u16(0x0003000200010000));
    const uint16x8_t mv_sse     = vreinterpretq_u16_u32(vdupq_n_u32(mv));
    uint32x4_t       new_mv_sse = vreinterpretq_u32_u16(vaddq_u16(vreinterpretq_u16_u32(search_idx), mv_sse));
    vst1q_u32(computed_idx, new_mv_sse);

    search_idx = vmovl_u16(vcreate_u16(0x0007000600050004));
    new_mv_sse = vreinterpretq_u32_u16(vaddq_u16(vreinterpretq_u16_u32(search_idx), mv_sse));
    vst1q_u32(computed_idx + 4, new_mv_sse);

    for (int i = 0; i < 4; i++) {
        for (int j = 0; j < 8; j++) {
            if (p_sad32x32[i][j] < p_best_sad_32x32[i]) {
                p_best_sad_32x32[i] = p_sad32x32[i][j];
                p_best_mv32x32[i]   = computed_idx[j];
            }
        }
    }

    for (int j = 0; j < 8; j++) {
        if (p_sad64x64[j] < p_best_sad_64x64[0]) {
            p_best_sad_64x64[0] = p_sad64x64[j];
            p_best_mv64x64[0]   = computed_idx[j];
        }
    }
}

void svt_ext_sad_calculation_32x32_64x64_neon(uint32_t* p_sad16x16, uint32_t* p_best_sad_32x32,
                                              uint32_t* p_best_sad_64x64, uint32_t* p_best_mv32x32,
                                              uint32_t* p_best_mv64x64, uint32_t mv, uint32_t* p_sad32x32) {
    uint32x4_t sad4d[4];
    load_u32_4x4(p_sad16x16, 4, &sad4d[0], &sad4d[1], &sad4d[2], &sad4d[3]);

    uint32x4_t sad = horizontal_add_4d_u32x4(sad4d);
    vst1q_u32(p_sad32x32, sad);

    uint32x4_t best_sad = vld1q_u32(p_best_sad_32x32);

    uint32x4_t comp = vcltq_u32(sad, best_sad);

    best_sad = vbslq_u32(comp, sad, best_sad);
    vst1q_u32(p_best_sad_32x32, best_sad);

    uint32x4_t best_mv = vld1q_u32(p_best_mv32x32);
    uint32x4_t mv_u32  = vdupq_n_u32(mv);

    best_mv = vbslq_u32(comp, mv_u32, best_mv);
    vst1q_u32(p_best_mv32x32, best_mv);

    uint32_t sad64x64 = vaddvq_u32(sad);
    if (sad64x64 < p_best_sad_64x64[0]) {
        p_best_sad_64x64[0] = sad64x64;
        p_best_mv64x64[0]   = mv;
    }
}

static inline uint32_t highbd_sad4xh_neon(uint16_t* src_ptr, int src_stride, uint16_t* ref_ptr, int ref_stride, int h) {
    assert(h % 2 == 0);
    uint32x4_t sum[2] = {vdupq_n_u32(0), vdupq_n_u32(0)};

    h >>= 1;
    do {
        uint16x4_t s0 = vld1_u16(src_ptr);
        uint16x4_t r0 = vld1_u16(ref_ptr);
        sum[0]        = vabal_u16(sum[0], s0, r0);

        uint16x4_t s1 = vld1_u16(src_ptr + src_stride);
        uint16x4_t r1 = vld1_u16(ref_ptr + ref_stride);
        sum[1]        = vabal_u16(sum[1], s1, r1);

        src_ptr += 2 * src_stride;
        ref_ptr += 2 * ref_stride;
    } while (--h != 0);

    sum[0] = vaddq_u32(sum[0], sum[1]);
    return vaddvq_u32(sum[0]);
}

static inline uint32_t highbd_sad8xh_neon(uint16_t* src_ptr, int src_stride, uint16_t* ref_ptr, int ref_stride, int h) {
    assert(h % 2 == 0);
    uint32x4_t sum[2] = {vdupq_n_u32(0), vdupq_n_u32(0)};

    h >>= 1;
    do {
        uint16x8_t s0    = vld1q_u16(src_ptr);
        uint16x8_t r0    = vld1q_u16(ref_ptr);
        uint16x8_t diff0 = vabdq_u16(s0, r0);
        sum[0]           = vpadalq_u16(sum[0], diff0);

        uint16x8_t s1    = vld1q_u16(src_ptr + src_stride);
        uint16x8_t r1    = vld1q_u16(ref_ptr + ref_stride);
        uint16x8_t diff1 = vabdq_u16(s1, r1);
        sum[1]           = vpadalq_u16(sum[1], diff1);

        src_ptr += 2 * src_stride;
        ref_ptr += 2 * ref_stride;
    } while (--h != 0);

    sum[0] = vaddq_u32(sum[0], sum[1]);
    return vaddvq_u32(sum[0]);
}

static inline uint32_t highbd_sad16xh_neon(uint16_t* src_ptr, uint32_t src_stride, uint16_t* ref_ptr,
                                           uint32_t ref_stride, int h) {
    // 'h_overflow' is the number of 16-wide rows we can process before 16-bit
    // accumulators overflow. After hitting this limit accumulate into 32-bit
    // elements. 65535 / 1023 ~= 64, so 64 16-wide rows using two accumulators.
    const int h_overflow = 64;
    int       h_limit    = h < h_overflow ? h : h_overflow;

    uint32x4_t sum_u32 = vdupq_n_u32(0);

    do {
        uint16x8_t sum_u16[2] = {vdupq_n_u16(0), vdupq_n_u16(0)};

        int i = h_limit;
        do {
            uint16x8_t s0 = vld1q_u16(src_ptr);
            uint16x8_t r0 = vld1q_u16(ref_ptr);
            sum_u16[0]    = vabaq_u16(sum_u16[0], s0, r0);

            uint16x8_t s1 = vld1q_u16(src_ptr + 8);
            uint16x8_t r1 = vld1q_u16(ref_ptr + 8);
            sum_u16[1]    = vabaq_u16(sum_u16[1], s1, r1);

            src_ptr += src_stride;
            ref_ptr += ref_stride;
        } while (--i != 0);

        sum_u32 = vpadalq_u16(sum_u32, sum_u16[0]);
        sum_u32 = vpadalq_u16(sum_u32, sum_u16[1]);

        h -= h_limit;
        h_limit = h < h_limit ? h : h_limit;
    } while (h != 0);

    return vaddvq_u32(sum_u32);
}

static inline uint32_t highbd_sad24nxh_neon(uint16_t* src_ptr, uint32_t src_stride, uint16_t* ref_ptr,
                                            uint32_t ref_stride, int w, int h, const int h_overflow) {
    int h_limit = h < h_overflow ? h : h_overflow;

    uint32x4_t sum_u32 = vdupq_n_u32(0);

    do {
        uint16x8_t sum_u16[3] = {vdupq_n_u16(0), vdupq_n_u16(0), vdupq_n_u16(0)};

        int i = h_limit;
        do {
            int j = 0;
            do {
                uint16x8_t s0 = vld1q_u16(src_ptr + j + 0);
                uint16x8_t r0 = vld1q_u16(ref_ptr + j + 0);
                sum_u16[0]    = vabaq_u16(sum_u16[0], s0, r0);

                uint16x8_t s1 = vld1q_u16(src_ptr + j + 8);
                uint16x8_t r1 = vld1q_u16(ref_ptr + j + 8);
                sum_u16[1]    = vabaq_u16(sum_u16[1], s1, r1);

                uint16x8_t s2 = vld1q_u16(src_ptr + j + 16);
                uint16x8_t r2 = vld1q_u16(ref_ptr + j + 16);
                sum_u16[2]    = vabaq_u16(sum_u16[2], s2, r2);

                j += 24;
            } while (j < w);

            src_ptr += src_stride;
            ref_ptr += ref_stride;
        } while (--i != 0);

        sum_u32 = vpadalq_u16(sum_u32, sum_u16[0]);
        sum_u32 = vpadalq_u16(sum_u32, sum_u16[1]);
        sum_u32 = vpadalq_u16(sum_u32, sum_u16[2]);

        h -= h_limit;
        h_limit = h < h_limit ? h : h_limit;
    } while (h != 0);

    return vaddvq_u32(sum_u32);
}

static inline uint32_t highbd_sad24xh_neon(uint16_t* src_ptr, uint32_t src_stride, uint16_t* ref_ptr,
                                           uint32_t ref_stride, int h) {
    // 'h_overflow' is the number of 24-wide rows we can process before 16-bit
    // accumulators overflow. After hitting this limit accumulate into 32-bit
    // elements. 65535 / 1023 ~= 64, so 64 24-wide rows using three accumulators.
    const int h_overflow = 64;
    return highbd_sad24nxh_neon(src_ptr, src_stride, ref_ptr, ref_stride, 24, h, h_overflow);
}

static inline uint32_t highbd_sad48xh_neon(uint16_t* src_ptr, uint32_t src_stride, uint16_t* ref_ptr,
                                           uint32_t ref_stride, int h) {
    // 'h_overflow' is the number of 48-wide rows we can process before 16-bit
    // accumulators overflow. After hitting this limit accumulate into 32-bit
    // elements. 65535 / 1023 ~= 64, so 32 48-wide rows using three accumulators.
    const int h_overflow = 32;
    return highbd_sad24nxh_neon(src_ptr, src_stride, ref_ptr, ref_stride, 48, h, h_overflow);
}

static inline uint32_t highbd_sadwxh_neon(uint16_t* src_ptr, uint32_t src_stride, uint16_t* ref_ptr,
                                          uint32_t ref_stride, int w, int h, const int h_overflow) {
    assert(w % 32 == 0);
    int h_limit = h < h_overflow ? h : h_overflow;

    uint32x4_t sum_u32 = vdupq_n_u32(0);

    do {
        uint16x8_t sum_u16[4] = {vdupq_n_u16(0), vdupq_n_u16(0), vdupq_n_u16(0), vdupq_n_u16(0)};

        int i = h_limit;
        do {
            int j = 0;
            do {
                uint16x8_t s0 = vld1q_u16(src_ptr + j + 0);
                uint16x8_t r0 = vld1q_u16(ref_ptr + j + 0);
                sum_u16[0]    = vabaq_u16(sum_u16[0], s0, r0);

                uint16x8_t s1 = vld1q_u16(src_ptr + j + 8);
                uint16x8_t r1 = vld1q_u16(ref_ptr + j + 8);
                sum_u16[1]    = vabaq_u16(sum_u16[1], s1, r1);

                uint16x8_t s2 = vld1q_u16(src_ptr + j + 16);
                uint16x8_t r2 = vld1q_u16(ref_ptr + j + 16);
                sum_u16[2]    = vabaq_u16(sum_u16[2], s2, r2);

                uint16x8_t s3 = vld1q_u16(src_ptr + j + 24);
                uint16x8_t r3 = vld1q_u16(ref_ptr + j + 24);
                sum_u16[3]    = vabaq_u16(sum_u16[3], s3, r3);

                j += 32;
            } while (j < w);

            src_ptr += src_stride;
            ref_ptr += ref_stride;
        } while (--i != 0);

        sum_u32 = vpadalq_u16(sum_u32, sum_u16[0]);
        sum_u32 = vpadalq_u16(sum_u32, sum_u16[1]);
        sum_u32 = vpadalq_u16(sum_u32, sum_u16[2]);
        sum_u32 = vpadalq_u16(sum_u32, sum_u16[3]);

        h -= h_limit;
        h_limit = h < h_limit ? h : h_limit;
    } while (h != 0);

    return vaddvq_u32(sum_u32);
}

static inline uint32_t highbd_sad32xh_neon(uint16_t* src_ptr, uint32_t src_stride, uint16_t* ref_ptr,
                                           uint32_t ref_stride, int h) {
    // 'h_overflow' is the number of 32-wide rows we can process before 16-bit
    // accumulators overflow. After hitting this limit accumulate into 32-bit
    // elements. 65535 / 1023 ~= 64, so 64 32-wide rows using four accumulators.
    const int h_overflow = 64;
    return highbd_sadwxh_neon(src_ptr, src_stride, ref_ptr, ref_stride, 32, h, h_overflow);
}

static inline uint32_t highbd_sad64xh_neon(uint16_t* src_ptr, uint32_t src_stride, uint16_t* ref_ptr,
                                           uint32_t ref_stride, int h) {
    // 'h_overflow' is the number of 64-wide rows we can process before 16-bit
    // accumulators overflow. After hitting this limit accumulate into 32-bit
    // elements. 65535 / 1023 ~= 64, so 32 64-wide rows using four accumulators.
    const int h_overflow = 32;
    return highbd_sadwxh_neon(src_ptr, src_stride, ref_ptr, ref_stride, 64, h, h_overflow);
}

static inline uint32_t highbd_sad128xh_neon(uint16_t* src_ptr, uint32_t src_stride, uint16_t* ref_ptr,
                                            uint32_t ref_stride, int h) {
    // 'h_overflow' is the number of 128-wide rows we can process before 16-bit
    // accumulators overflow. After hitting this limit accumulate into 32-bit
    // elements. 65535 / 1023 ~= 64, so 16 128-wide rows using four accumulators.
    const int h_overflow = 16;
    return highbd_sadwxh_neon(src_ptr, src_stride, ref_ptr, ref_stride, 128, h, h_overflow);
}

uint32_t svt_aom_sad_16b_kernel_neon(uint16_t* src, uint32_t src_stride, uint16_t* ref, uint32_t ref_stride,
                                     uint32_t height, uint32_t width) {
    assert(width % 4 == 0);

    switch (width) {
    case 4:
        return highbd_sad4xh_neon(src, src_stride, ref, ref_stride, height);
    case 8:
        return highbd_sad8xh_neon(src, src_stride, ref, ref_stride, height);
    case 16:
        return highbd_sad16xh_neon(src, src_stride, ref, ref_stride, height);
    case 24:
        return highbd_sad24xh_neon(src, src_stride, ref, ref_stride, height);
    case 32:
        return highbd_sad32xh_neon(src, src_stride, ref, ref_stride, height);
    case 48:
        return highbd_sad48xh_neon(src, src_stride, ref, ref_stride, height);
    case 64:
        return highbd_sad64xh_neon(src, src_stride, ref, ref_stride, height);
    case 128:
        return highbd_sad128xh_neon(src, src_stride, ref, ref_stride, height);
    }

    uint32_t offset       = 0;
    uint32_t acc          = 0;
    uint32_t width_offset = width & ~31;
    if (width_offset) {
        const int h_overflow = 64 / (width_offset / 16);
        acc += highbd_sadwxh_neon(src, src_stride, ref, ref_stride, width_offset, height, h_overflow);
        width -= width_offset;
        offset += width_offset;
    }
    if (width >= 16) {
        acc += highbd_sad16xh_neon(src + offset, src_stride, ref + offset, ref_stride, height);
        width -= 16;
        offset += 16;
    }
    if (width >= 8) {
        acc += highbd_sad8xh_neon(src + offset, src_stride, ref + offset, ref_stride, height);
        width -= 8;
        offset += 8;
    }
    if (width) {
        acc += highbd_sad4xh_neon(src + offset, src_stride, ref + offset, ref_stride, height);
    }

    return acc;
}
