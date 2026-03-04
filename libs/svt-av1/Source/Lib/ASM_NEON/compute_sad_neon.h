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

#ifndef COMPUTE_SAD_NEON_H
#define COMPUTE_SAD_NEON_H

#include <arm_neon.h>

#include "aom_dsp_rtcd.h"
#include "mem_neon.h"
#include "sum_neon.h"
#include "utility.h"

#if __GNUC__
#define svt_ctzll(id, x) id = (unsigned long)__builtin_ctzll(x)
#elif defined(_MSC_VER)
#include <intrin.h>

#define svt_ctzll(id, x) _BitScanForward64(&id, x)
#endif

/* Find the position of the first occurrence of 'value' in the vector 'x'.
 * Returns the position (index) of the first occurrence of 'value' in the vector 'x'. */
static inline uint16_t findposq_u32(uint32x4_t x, uint32_t value) {
    uint32x4_t val_mask = vdupq_n_u32(value);

    /* Pack the information in the lower 64 bits of the register by considering only alternate
     * 16-bit lanes. */
    uint16x4_t is_one = vmovn_u32(vceqq_u32(x, val_mask));

    /* Get the lower 64 bits from the 128-bit register. */
    uint64_t idx = vget_lane_u64(vreinterpret_u64_u16(is_one), 0);

    /* Calculate the position as an index, dividing by 16 to account for 16-bit lanes. */
    uint64_t res;
    svt_ctzll(res, idx);
    return res >> 4;
}

static inline void update_best_sad_u32(uint32x4_t sad4, uint64_t* best_sad, int16_t* x_search_center,
                                       int16_t* y_search_center, int16_t x_search_index, int16_t y_search_index) {
    /* Find the minimum SAD value out of the 4 search spaces. */
    uint64_t temp_sad = vminvq_u32(sad4);

    if (temp_sad < *best_sad) {
        *best_sad        = temp_sad;
        *x_search_center = (int16_t)(x_search_index + findposq_u32(sad4, temp_sad));
        *y_search_center = y_search_index;
    }
}

/* Find the position of the first occurrence of 'value' in the vector 'x'.
 * Returns the position (index) of the first occurrence of 'value' in the vector 'x'. */
static inline uint16_t findposq_u16(uint16x8_t x, uint16_t value) {
    uint16x8_t val_mask = vdupq_n_u16(value);

    /* Pack the information in the lower 64 bits of the register by considering only alternate
     * 8-bit lanes. */
    uint8x8_t is_one = vmovn_u16(vceqq_u16(x, val_mask));

    /* Get the lower 64 bits from the 128-bit register. */
    uint64_t idx = vget_lane_u64(vreinterpret_u64_u8(is_one), 0);

    /* Calculate the position as an index, dividing by 8 to account for 8-bit lanes. */
    uint64_t res;
    svt_ctzll(res, idx);
    return res >> 3;
}

static inline void update_best_sad_u16(uint16x8_t sad8, uint64_t* best_sad, int16_t* x_search_center,
                                       int16_t* y_search_center, int16_t x_search_index, int16_t y_search_index) {
    /* Find the minimum SAD value out of the 8 search spaces. */
    uint64_t temp_sad = vminvq_u16(sad8);

    if (temp_sad < *best_sad) {
        *best_sad        = temp_sad;
        *x_search_center = (int16_t)(x_search_index + findposq_u16(sad8, temp_sad));
        *y_search_center = y_search_index;
    }
}

static inline void update_best_sad(uint64_t temp_sad, uint64_t* best_sad, int16_t* x_search_center,
                                   int16_t* y_search_center, int16_t x_search_index, int16_t y_search_index) {
    if (temp_sad < *best_sad) {
        *best_sad        = temp_sad;
        *x_search_center = x_search_index;
        *y_search_center = y_search_index;
    }
}

/* Return a uint16x8 vector with 'n' lanes filled with 0 and the others filled with 65535
 * The valid range for 'n' is 0 to 7 */
static inline uint16x8_t prepare_maskq_u16(uint16_t n) {
    uint64_t mask    = UINT64_MAX;
    mask             = mask << (8 * n);
    uint8x16_t mask8 = vcombine_u8(vcreate_u8(mask), vdup_n_u8(0));
    return vreinterpretq_u16_u8(vzip1q_u8(mask8, mask8));
}

/* Return a uint32x4 vector with 'n' lanes filled with 0 and the others filled with 4294967295
 * The valid range for 'n' is 0 to 4 */
static inline uint32x4_t prepare_maskq_u32(uint16_t n) {
    uint64_t mask    = UINT64_MAX;
    mask             = n < 4 ? (mask << (16 * n)) : 0;
    uint16x8_t mask8 = vcombine_u16(vcreate_u16(mask), vdup_n_u16(0));
    return vreinterpretq_u32_u16(vzip1q_u16(mask8, mask8));
}

static inline uint32_t sad8xh_neon(const uint8_t* src_ptr, uint32_t src_stride, const uint8_t* ref_ptr,
                                   uint32_t ref_stride, uint32_t h) {
    uint16x8_t sum = vdupq_n_u16(0);
    do {
        uint8x8_t s = vld1_u8(src_ptr);
        uint8x8_t r = vld1_u8(ref_ptr);

        sum = vabal_u8(sum, s, r);

        src_ptr += src_stride;
        ref_ptr += ref_stride;
    } while (--h != 0);

    return vaddlvq_u16(sum);
}

static inline uint32x4_t sad8xhx4d_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
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

static inline void svt_sad_loop_kernel8xh_neon(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                               uint32_t block_height, uint64_t* best_sad, int16_t* x_search_center,
                                               int16_t* y_search_center, uint32_t src_stride_raw,
                                               int16_t search_area_width, int16_t search_area_height) {
    for (int y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        int16_t x_search_index;
        for (x_search_index = 0; x_search_index <= search_area_width - 4; x_search_index += 4) {
            /* Get the SAD of 4 search spaces aligned along the width and store it in 'sad4'. */
            uint32x4_t sad4 = sad8xhx4d_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);
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

static inline uint16x8_t sad16_neon_init(uint8x16_t src, uint8x16_t ref) {
    const uint8x16_t abs_diff = vabdq_u8(src, ref);
    return vpaddlq_u8(abs_diff);
}

static inline void sad16_neon(uint8x16_t src, uint8x16_t ref, uint16x8_t* const sad_sum) {
    const uint8x16_t abs_diff = vabdq_u8(src, ref);
    *sad_sum                  = vpadalq_u8(*sad_sum, abs_diff);
}

static inline uint32_t sad128xh_neon(const uint8_t* src_ptr, uint32_t src_stride, const uint8_t* ref_ptr,
                                     uint32_t ref_stride, uint32_t h) {
    // We use 8 accumulators to prevent overflow for large values of 'h', as well
    // as enabling optimal UADALP instruction throughput on CPUs that have either
    // 2 or 4 Neon pipes.
    uint16x8_t sum[8];
    uint8x16_t s0, s1, s2, s3, s4, s5, s6, s7;
    uint8x16_t r0, r1, r2, r3, r4, r5, r6, r7;
    uint8x16_t diff0, diff1, diff2, diff3, diff4, diff5, diff6, diff7;
    uint32x4_t sum_u32;
    uint32_t   i;

    for (i = 0; i < 8; i++) {
        sum[i] = vdupq_n_u16(0);
    }

    i = h;
    do {
        s0     = vld1q_u8(src_ptr);
        r0     = vld1q_u8(ref_ptr);
        diff0  = vabdq_u8(s0, r0);
        sum[0] = vpadalq_u8(sum[0], diff0);

        s1     = vld1q_u8(src_ptr + 16);
        r1     = vld1q_u8(ref_ptr + 16);
        diff1  = vabdq_u8(s1, r1);
        sum[1] = vpadalq_u8(sum[1], diff1);

        s2     = vld1q_u8(src_ptr + 32);
        r2     = vld1q_u8(ref_ptr + 32);
        diff2  = vabdq_u8(s2, r2);
        sum[2] = vpadalq_u8(sum[2], diff2);

        s3     = vld1q_u8(src_ptr + 48);
        r3     = vld1q_u8(ref_ptr + 48);
        diff3  = vabdq_u8(s3, r3);
        sum[3] = vpadalq_u8(sum[3], diff3);

        s4     = vld1q_u8(src_ptr + 64);
        r4     = vld1q_u8(ref_ptr + 64);
        diff4  = vabdq_u8(s4, r4);
        sum[4] = vpadalq_u8(sum[4], diff4);

        s5     = vld1q_u8(src_ptr + 80);
        r5     = vld1q_u8(ref_ptr + 80);
        diff5  = vabdq_u8(s5, r5);
        sum[5] = vpadalq_u8(sum[5], diff5);

        s6     = vld1q_u8(src_ptr + 96);
        r6     = vld1q_u8(ref_ptr + 96);
        diff6  = vabdq_u8(s6, r6);
        sum[6] = vpadalq_u8(sum[6], diff6);

        s7     = vld1q_u8(src_ptr + 112);
        r7     = vld1q_u8(ref_ptr + 112);
        diff7  = vabdq_u8(s7, r7);
        sum[7] = vpadalq_u8(sum[7], diff7);

        src_ptr += src_stride;
        ref_ptr += ref_stride;
    } while (--i != 0);

    sum_u32 = vpaddlq_u16(sum[0]);
    sum_u32 = vpadalq_u16(sum_u32, sum[1]);
    sum_u32 = vpadalq_u16(sum_u32, sum[2]);
    sum_u32 = vpadalq_u16(sum_u32, sum[3]);
    sum_u32 = vpadalq_u16(sum_u32, sum[4]);
    sum_u32 = vpadalq_u16(sum_u32, sum[5]);
    sum_u32 = vpadalq_u16(sum_u32, sum[6]);
    sum_u32 = vpadalq_u16(sum_u32, sum[7]);

    return vaddvq_u32(sum_u32);
}

static inline uint32_t sad64xh_neon(const uint8_t* src_ptr, uint32_t src_stride, const uint8_t* ref_ptr,
                                    uint32_t ref_stride, uint32_t h) {
    uint16x8_t sum[4];
    uint8x16_t s0, s1, s2, s3, r0, r1, r2, r3;
    uint8x16_t diff0, diff1, diff2, diff3;
    uint32x4_t sum_u32;
    uint32_t   i;

    sum[0] = vdupq_n_u16(0);
    sum[1] = vdupq_n_u16(0);
    sum[2] = vdupq_n_u16(0);
    sum[3] = vdupq_n_u16(0);

    i = h;
    do {
        s0     = vld1q_u8(src_ptr);
        r0     = vld1q_u8(ref_ptr);
        diff0  = vabdq_u8(s0, r0);
        sum[0] = vpadalq_u8(sum[0], diff0);

        s1     = vld1q_u8(src_ptr + 16);
        r1     = vld1q_u8(ref_ptr + 16);
        diff1  = vabdq_u8(s1, r1);
        sum[1] = vpadalq_u8(sum[1], diff1);

        s2     = vld1q_u8(src_ptr + 32);
        r2     = vld1q_u8(ref_ptr + 32);
        diff2  = vabdq_u8(s2, r2);
        sum[2] = vpadalq_u8(sum[2], diff2);

        s3     = vld1q_u8(src_ptr + 48);
        r3     = vld1q_u8(ref_ptr + 48);
        diff3  = vabdq_u8(s3, r3);
        sum[3] = vpadalq_u8(sum[3], diff3);

        src_ptr += src_stride;
        ref_ptr += ref_stride;
    } while (--i != 0);

    sum_u32 = vpaddlq_u16(sum[0]);
    sum_u32 = vpadalq_u16(sum_u32, sum[1]);
    sum_u32 = vpadalq_u16(sum_u32, sum[2]);
    sum_u32 = vpadalq_u16(sum_u32, sum[3]);

    return vaddvq_u32(sum_u32);
}

static inline uint32_t sad32xh_neon(const uint8_t* src_ptr, uint32_t src_stride, const uint8_t* ref_ptr,
                                    uint32_t ref_stride, uint32_t h) {
    uint16x8_t sum[2];
    uint8x16_t s0, r0, s1, r1, diff0, diff1;
    uint32_t   i;

    sum[0] = vdupq_n_u16(0);
    sum[1] = vdupq_n_u16(0);

    i = h;
    do {
        s0     = vld1q_u8(src_ptr);
        r0     = vld1q_u8(ref_ptr);
        diff0  = vabdq_u8(s0, r0);
        sum[0] = vpadalq_u8(sum[0], diff0);

        s1     = vld1q_u8(src_ptr + 16);
        r1     = vld1q_u8(ref_ptr + 16);
        diff1  = vabdq_u8(s1, r1);
        sum[1] = vpadalq_u8(sum[1], diff1);

        src_ptr += src_stride;
        ref_ptr += ref_stride;
    } while (--i != 0);

    return vaddlvq_u16(vaddq_u16(sum[0], sum[1]));
}

static inline uint32_t sad16xh_neon(const uint8_t* src_ptr, uint32_t src_stride, const uint8_t* ref_ptr,
                                    uint32_t ref_stride, uint32_t h) {
    uint16x8_t sum;
    uint8x16_t s, r, diff;
    uint32_t   i;

    sum = vdupq_n_u16(0);

    i = h;
    do {
        s = vld1q_u8(src_ptr);
        r = vld1q_u8(ref_ptr);

        diff = vabdq_u8(s, r);
        sum  = vpadalq_u8(sum, diff);

        src_ptr += src_stride;
        ref_ptr += ref_stride;
    } while (--i != 0);

    return vaddlvq_u16(sum);
}

static inline uint32_t sad4xh_neon(const uint8_t* src_ptr, uint32_t src_stride, const uint8_t* ref_ptr,
                                   uint32_t ref_stride, uint32_t h) {
    uint16x8_t sum;
    uint8x8_t  s, r;
    uint32_t   i;

    sum = vdupq_n_u16(0);
    i   = h / 2;
    do {
        s = load_u8_4x2(src_ptr, src_stride);
        r = load_u8_4x2(ref_ptr, ref_stride);

        sum = vabal_u8(sum, s, r); // add and accumulate

        src_ptr += 2 * src_stride;
        ref_ptr += 2 * ref_stride;
    } while (--i != 0);
    return vaddlvq_u16(sum);
}

static inline uint32_t sad24xh_neon(const uint8_t* src_ptr, uint32_t src_stride, const uint8_t* ref_ptr,
                                    uint32_t ref_stride, uint32_t h) {
    uint32_t temp_sad;
    temp_sad = sad16xh_neon(src_ptr + 0, src_stride, ref_ptr + 0, ref_stride, h);
    temp_sad += sad8xh_neon(src_ptr + 16, src_stride, ref_ptr + 16, ref_stride, h);
    return temp_sad;
}

static inline uint32_t sad48xh_neon(const uint8_t* src_ptr, uint32_t src_stride, const uint8_t* ref_ptr,
                                    uint32_t ref_stride, uint32_t h) {
    uint32_t temp_sad;
    temp_sad = sad32xh_neon(src_ptr + 0, src_stride, ref_ptr + 0, ref_stride, h);
    temp_sad += sad16xh_neon(src_ptr + 32, src_stride, ref_ptr + 32, ref_stride, h);
    return temp_sad;
}

DECLARE_ALIGNED(16, static const uint8_t, kPermTable2xh[48]) = {0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6,  6,  7,  7,  8,
                                                                2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8,  8,  9,  9,  10,
                                                                4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12};

static inline uint16x8_t sad4xhx8d_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                        uint32_t ref_stride, uint32_t h) {
    /* Initialize 'res' to store the sum of absolute differences (SAD) of 8 search spaces. */
    uint16x8_t   res      = vdupq_n_u16(0);
    uint8x16x2_t perm_tbl = vld1q_u8_x2(kPermTable2xh);

    do {
        uint8x16_t src0 = vreinterpretq_u8_u16(vld1q_dup_u16((const uint16_t*)src));
        uint8x16_t src1 = vreinterpretq_u8_u16(vld1q_dup_u16((const uint16_t*)(src + 2)));

        uint8x16_t ref0 = vqtbl1q_u8(vld1q_u8(ref), perm_tbl.val[0]);
        uint8x16_t ref1 = vqtbl1q_u8(vld1q_u8(ref), perm_tbl.val[1]);

        uint8x16_t abs0 = vabdq_u8(src0, ref0);
        uint8x16_t abs1 = vabdq_u8(src1, ref1);

        res = vpadalq_u8(res, abs0);
        res = vpadalq_u8(res, abs1);

        src += src_stride;
        ref += ref_stride;
    } while (--h != 0);
    return res;
}

static inline uint16x8_t sad6xhx8d_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref,
                                        uint32_t ref_stride, uint32_t h) {
    /* Initialize 'res' to store the sum of absolute differences (SAD) of 8 search spaces. */
    uint16x8_t   res      = vdupq_n_u16(0);
    uint8x16x3_t perm_tbl = vld1q_u8_x3(kPermTable2xh);

    do {
        uint8x16_t src0 = vreinterpretq_u8_u16(vld1q_dup_u16((const uint16_t*)src));
        uint8x16_t src1 = vreinterpretq_u8_u16(vld1q_dup_u16((const uint16_t*)(src + 2)));
        uint8x16_t src2 = vreinterpretq_u8_u16(vld1q_dup_u16((const uint16_t*)(src + 4)));

        uint8x16_t ref0 = vqtbl1q_u8(vld1q_u8(ref), perm_tbl.val[0]);
        uint8x16_t ref1 = vqtbl1q_u8(vld1q_u8(ref), perm_tbl.val[1]);
        uint8x16_t ref2 = vqtbl1q_u8(vld1q_u8(ref), perm_tbl.val[2]);

        uint8x16_t abs0 = vabdq_u8(src0, ref0);
        uint8x16_t abs1 = vabdq_u8(src1, ref1);
        uint8x16_t abs2 = vabdq_u8(src2, ref2);

        res = vpadalq_u8(res, abs0);
        res = vpadalq_u8(res, abs1);
        res = vpadalq_u8(res, abs2);

        src += src_stride;
        ref += ref_stride;
    } while (--h != 0);
    return res;
}

static inline void sad12xhx8d_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref, uint32_t ref_stride,
                                   uint32_t h, uint32x4_t* res) {
    /* 'sad8' will store 8d SAD for block_width = 4 */
    uint16x8_t sad8;
    sad8   = sad4xhx8d_neon(src + 0, src_stride, ref + 0, ref_stride, h);
    res[0] = sad8xhx4d_neon(src + 4, src_stride, ref + 4, ref_stride, h);
    res[1] = sad8xhx4d_neon(src + 4, src_stride, ref + 8, ref_stride, h);
    res[0] = vaddw_u16(res[0], vget_low_u16(sad8));
    res[1] = vaddw_high_u16(res[1], sad8);
}

static inline void svt_sad_loop_kernel4xh_neon(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                               uint32_t block_height, uint64_t* best_sad, int16_t* x_search_center,
                                               int16_t* y_search_center, uint32_t src_stride_raw,
                                               int16_t search_area_width, int16_t search_area_height) {
    int16_t    x_search_index, y_search_index;
    uint16x8_t sad8;
    uint32_t   leftover      = search_area_width & 7;
    uint16x8_t leftover_mask = prepare_maskq_u16(leftover);

    for (y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (x_search_index = 0; x_search_index <= search_area_width - 8; x_search_index += 8) {
            /* Get the SAD of 8 search spaces aligned along the width and store it in 'sad8'. */
            sad8 = sad4xhx8d_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);

            /* Update 'best_sad' */
            update_best_sad_u16(sad8, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        if (leftover) {
            /* Get the SAD of 8 search spaces aligned along the width and store it in 'sad8'. */
            sad8 = sad4xhx8d_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);

            /* set undesired lanes to maximum value */
            sad8 = vorrq_u16(sad8, leftover_mask);

            /* Update 'best_sad' */
            update_best_sad_u16(sad8, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel6xh_neon(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                               uint32_t block_height, uint64_t* best_sad, int16_t* x_search_center,
                                               int16_t* y_search_center, uint32_t src_stride_raw,
                                               int16_t search_area_width, int16_t search_area_height) {
    int16_t    x_search_index, y_search_index;
    uint16x8_t sad8;
    uint32_t   leftover      = search_area_width & 7;
    uint16x8_t leftover_mask = prepare_maskq_u16(leftover);

    for (y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (x_search_index = 0; x_search_index <= search_area_width - 8; x_search_index += 8) {
            /* Get the SAD of 8 search spaces aligned along the width and store it in 'sad8'. */
            sad8 = sad6xhx8d_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);

            /* Update 'best_sad' */
            update_best_sad_u16(sad8, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        if (leftover) {
            /* Get the SAD of 8 search spaces aligned along the width and store it in 'sad8'. */
            sad8 = sad6xhx8d_neon(src, src_stride, ref + x_search_index, ref_stride, block_height);

            /* set undesired lanes to maximum value */
            sad8 = vorrq_u16(sad8, leftover_mask);

            /* Update 'best_sad' */
            update_best_sad_u16(sad8, best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline void svt_sad_loop_kernel12xh_neon(uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
                                                uint32_t block_height, uint64_t* best_sad, int16_t* x_search_center,
                                                int16_t* y_search_center, uint32_t src_stride_raw,
                                                int16_t search_area_width, int16_t search_area_height) {
    int16_t x_search_index, y_search_index;

    /* To accumulate the SAD of 8 search spaces */
    uint32x4_t sad8[2];

    uint32_t   leftover = search_area_width & 7;
    uint32x4_t leftover_mask[2];
    leftover_mask[0] = prepare_maskq_u32(MIN(leftover, 4));
    leftover_mask[1] = prepare_maskq_u32(leftover - MIN(leftover, 4));
    for (y_search_index = 0; y_search_index < search_area_height; y_search_index++) {
        for (x_search_index = 0; x_search_index <= search_area_width - 8; x_search_index += 8) {
            /* Get the SAD of 8 search spaces aligned along the width and store it in 'sad8'. */
            sad12xhx8d_neon(src, src_stride, ref + x_search_index, ref_stride, block_height, sad8);

            /* Update 'best_sad' */
            update_best_sad_u32(sad8[0], best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(
                sad8[1], best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }

        if (leftover) {
            /* Get the SAD of 8 search spaces aligned along the width and store it in 'sad8'. */
            sad12xhx8d_neon(src, src_stride, ref + x_search_index, ref_stride, block_height, sad8);

            /* set undesired lanes to maximum value */
            sad8[0] = vorrq_u32(sad8[0], leftover_mask[0]);
            sad8[1] = vorrq_u32(sad8[1], leftover_mask[1]);

            /* Update 'best_sad' */
            update_best_sad_u32(sad8[0], best_sad, x_search_center, y_search_center, x_search_index, y_search_index);
            update_best_sad_u32(
                sad8[1], best_sad, x_search_center, y_search_center, x_search_index + 4, y_search_index);
        }

        ref += src_stride_raw;
    }
}

static inline uint32_t sadwxh_neon(const uint8_t* src, uint32_t src_stride, const uint8_t* ref, uint32_t ref_stride,
                                   uint32_t width, uint32_t height) {
    uint32x4_t sum_u32 = vdupq_n_u32(0);
    uint32_t   sum     = 0;

    // We can accumulate 257 absolute differences in a 16-bit element before
    // it overflows. Given that we're accumulating in 8 16-bit elements we
    // therefore need width * height < (257 * 8). (This isn't quite true as some
    // elements in the tail loops have a different accumulator, but it's a good
    // enough approximation).
    uint32_t h_overflow = (257 * 8) / width;
    uint32_t h_limit    = h_overflow >= height ? height : h_overflow;

    uint32_t i = 0;
    do {
        uint16x8_t sum_u16 = vdupq_n_u16(0);

        do {
            int w = width;

            const uint8_t* src_ptr = src;
            const uint8_t* ref_ptr = ref;

            while (w >= 16) {
                const uint8x16_t s = vld1q_u8(src_ptr);
                sad16_neon(s, vld1q_u8(ref_ptr), &sum_u16);

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

            while (w != 0) {
                sum += EB_ABS_DIFF(src_ptr[w - 1], ref_ptr[w - 1]);

                w--;
            }
            src += src_stride;
            ref += ref_stride;
        } while (++i < h_limit);

        sum_u32 = vpadalq_u16(sum_u32, sum_u16);

        uint32_t h_inc = h_limit + h_overflow < height ? h_overflow : height - h_limit;
        h_limit += h_inc;
    } while (i < height);

    return sum + vaddvq_u32(sum_u32);
}

#endif // COMPUTE_SAD_NEON_H
