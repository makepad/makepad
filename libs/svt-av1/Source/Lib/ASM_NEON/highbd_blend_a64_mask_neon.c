/*
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
#include <assert.h>

#include "blend_a64_mask_neon.h"
#include "common_dsp_rtcd.h"
#include "definitions.h"
#include "mem_neon.h"

static inline uint16x8_t alpha_blend_a64_u16x8(uint16x8_t m, uint16x8_t a, uint16x8_t b) {
    uint16x8_t m_inv = vsubq_u16(vdupq_n_u16(AOM_BLEND_A64_MAX_ALPHA), m);

    uint32x4_t blend_u32_lo = vmull_u16(vget_low_u16(a), vget_low_u16(m));
    uint32x4_t blend_u32_hi = vmull_u16(vget_high_u16(a), vget_high_u16(m));

    blend_u32_lo = vmlal_u16(blend_u32_lo, vget_low_u16(b), vget_low_u16(m_inv));
    blend_u32_hi = vmlal_u16(blend_u32_hi, vget_high_u16(b), vget_high_u16(m_inv));

    uint16x4_t blend_u16_lo = vrshrn_n_u32(blend_u32_lo, AOM_BLEND_A64_ROUND_BITS);
    uint16x4_t blend_u16_hi = vrshrn_n_u32(blend_u32_hi, AOM_BLEND_A64_ROUND_BITS);

    return vcombine_u16(blend_u16_lo, blend_u16_hi);
}

void svt_aom_highbd_blend_a64_mask_neon(uint8_t* dst_8, uint32_t dst_stride, const uint8_t* src0_8,
                                        uint32_t src0_stride, const uint8_t* src1_8, uint32_t src1_stride,
                                        const uint8_t* mask, uint32_t mask_stride, int w, int h, int subw, int subh,
                                        int bd) {
    (void)bd;

    const uint16_t* src0 = (uint16_t*)src0_8;
    const uint16_t* src1 = (uint16_t*)src1_8;
    uint16_t*       dst  = (uint16_t*)dst_8;

    assert(IMPLIES(src0 == dst, src0_stride == dst_stride));
    assert(IMPLIES(src1 == dst, src1_stride == dst_stride));

    assert(h >= 1);
    assert(w >= 1);
    assert(IS_POWER_OF_TWO(h));
    assert(IS_POWER_OF_TWO(w));

    assert(bd == 8 || bd == 10);

    if ((subw | subh) == 0) {
        if (w >= 8) {
            do {
                int i = 0;
                do {
                    uint16x8_t m0 = vmovl_u8(vld1_u8(mask + i));
                    uint16x8_t s0 = vld1q_u16(src0 + i);
                    uint16x8_t s1 = vld1q_u16(src1 + i);

                    uint16x8_t blend = alpha_blend_a64_u16x8(m0, s0, s1);

                    vst1q_u16(dst + i, blend);
                    i += 8;
                } while (i < w);

                mask += mask_stride;
                src0 += src0_stride;
                src1 += src1_stride;
                dst += dst_stride;
            } while (--h != 0);
        } else {
            do {
                uint16x8_t m0 = vmovl_u8(load_u8_4x2(mask, mask_stride));
                uint16x8_t s0 = load_u16_4x2(src0, src0_stride);
                uint16x8_t s1 = load_u16_4x2(src1, src1_stride);

                uint16x8_t blend = alpha_blend_a64_u16x8(m0, s0, s1);

                store_u16x4_strided_x2(dst, dst_stride, blend);

                mask += 2 * mask_stride;
                src0 += 2 * src0_stride;
                src1 += 2 * src1_stride;
                dst += 2 * dst_stride;
                h -= 2;
            } while (h != 0);
        }
    } else if ((subw & subh) == 1) {
        if (w >= 8) {
            do {
                int i = 0;
                do {
                    uint8x8_t  m0 = vld1_u8(mask + 0 * mask_stride + 2 * i);
                    uint8x8_t  m1 = vld1_u8(mask + 1 * mask_stride + 2 * i);
                    uint8x8_t  m2 = vld1_u8(mask + 0 * mask_stride + 2 * i + 8);
                    uint8x8_t  m3 = vld1_u8(mask + 1 * mask_stride + 2 * i + 8);
                    uint16x8_t s0 = vld1q_u16(src0 + i);
                    uint16x8_t s1 = vld1q_u16(src1 + i);

                    uint16x8_t m_avg = avg_blend_pairwise_long_u8x8_4(m0, m1, m2, m3);

                    uint16x8_t blend = alpha_blend_a64_u16x8(m_avg, s0, s1);

                    vst1q_u16(dst + i, blend);

                    i += 8;
                } while (i < w);

                mask += 2 * mask_stride;
                src0 += src0_stride;
                src1 += src1_stride;
                dst += dst_stride;
            } while (--h != 0);
        } else {
            do {
                uint8x8_t  m0 = vld1_u8(mask + 0 * mask_stride);
                uint8x8_t  m1 = vld1_u8(mask + 1 * mask_stride);
                uint8x8_t  m2 = vld1_u8(mask + 2 * mask_stride);
                uint8x8_t  m3 = vld1_u8(mask + 3 * mask_stride);
                uint16x8_t s0 = load_u16_4x2(src0, src0_stride);
                uint16x8_t s1 = load_u16_4x2(src1, src1_stride);

                uint16x8_t m_avg = avg_blend_pairwise_long_u8x8_4(m0, m1, m2, m3);
                uint16x8_t blend = alpha_blend_a64_u16x8(m_avg, s0, s1);

                store_u16x4_strided_x2(dst, dst_stride, blend);

                mask += 4 * mask_stride;
                src0 += 2 * src0_stride;
                src1 += 2 * src1_stride;
                dst += 2 * dst_stride;
                h -= 2;
            } while (h != 0);
        }
    } else if (subw == 1 && subh == 0) {
        if (w >= 8) {
            do {
                int i = 0;

                do {
                    uint8x8_t  m0 = vld1_u8(mask + 2 * i);
                    uint8x8_t  m1 = vld1_u8(mask + 2 * i + 8);
                    uint16x8_t s0 = vld1q_u16(src0 + i);
                    uint16x8_t s1 = vld1q_u16(src1 + i);

                    uint16x8_t m_avg = vmovl_u8(vrshr_n_u8(vpadd_u8(m0, m1), 1));
                    uint16x8_t blend = alpha_blend_a64_u16x8(m_avg, s0, s1);

                    vst1q_u16(dst + i, blend);

                    i += 8;
                } while (i < w);

                mask += mask_stride;
                src0 += src0_stride;
                src1 += src1_stride;
                dst += dst_stride;
            } while (--h != 0);
        } else {
            do {
                uint8x8_t  m0 = vld1_u8(mask + 0 * mask_stride);
                uint8x8_t  m1 = vld1_u8(mask + 1 * mask_stride);
                uint16x8_t s0 = load_u16_4x2(src0, src0_stride);
                uint16x8_t s1 = load_u16_4x2(src1, src1_stride);

                uint16x8_t m_avg = vmovl_u8(vrshr_n_u8(vpadd_u8(m0, m1), 1));
                uint16x8_t blend = alpha_blend_a64_u16x8(m_avg, s0, s1);

                store_u16x4_strided_x2(dst, dst_stride, blend);

                mask += 2 * mask_stride;
                src0 += 2 * src0_stride;
                src1 += 2 * src1_stride;
                dst += 2 * dst_stride;
                h -= 2;
            } while (h != 0);
        }
    } else {
        if (w >= 8) {
            do {
                int i = 0;
                do {
                    uint8x8_t  m0 = vld1_u8(mask + 0 * mask_stride + i);
                    uint8x8_t  m1 = vld1_u8(mask + 1 * mask_stride + i);
                    uint16x8_t s0 = vld1q_u16(src0 + i);
                    uint16x8_t s1 = vld1q_u16(src1 + i);

                    uint16x8_t m_avg = vmovl_u8(vrhadd_u8(m0, m1));
                    uint16x8_t blend = alpha_blend_a64_u16x8(m_avg, s0, s1);

                    vst1q_u16(dst + i, blend);

                    i += 8;
                } while (i < w);

                mask += 2 * mask_stride;
                src0 += src0_stride;
                src1 += src1_stride;
                dst += dst_stride;
            } while (--h != 0);
        } else {
            do {
                uint8x8_t  m0_2 = load_u8_4x2(mask + 0 * mask_stride, 2 * mask_stride);
                uint8x8_t  m1_3 = load_u8_4x2(mask + 1 * mask_stride, 2 * mask_stride);
                uint16x8_t s0   = load_u16_4x2(src0, src0_stride);
                uint16x8_t s1   = load_u16_4x2(src1, src1_stride);

                uint16x8_t m_avg = vmovl_u8(vrhadd_u8(m0_2, m1_3));
                uint16x8_t blend = alpha_blend_a64_u16x8(m_avg, s0, s1);

                store_u16x4_strided_x2(dst, dst_stride, blend);

                mask += 4 * mask_stride;
                src0 += 2 * src0_stride;
                src1 += 2 * src1_stride;
                dst += 2 * dst_stride;
                h -= 2;
            } while (h != 0);
        }
    }
}

#define HBD_BLEND_A64_D16_MASK(bd, round0_bits)                                                                       \
    static inline uint16x8_t alpha_##bd##_blend_a64_d16_u16x8(                                                        \
        uint16x8_t m, uint16x8_t a, uint16x8_t b, int32x4_t round_offset) {                                           \
        const uint16x8_t m_inv = vsubq_u16(vdupq_n_u16(AOM_BLEND_A64_MAX_ALPHA), m);                                  \
                                                                                                                      \
        uint32x4_t blend_u32_lo = vmlal_u16(vreinterpretq_u32_s32(round_offset), vget_low_u16(m), vget_low_u16(a));   \
        uint32x4_t blend_u32_hi = vmlal_u16(vreinterpretq_u32_s32(round_offset), vget_high_u16(m), vget_high_u16(a)); \
                                                                                                                      \
        blend_u32_lo = vmlal_u16(blend_u32_lo, vget_low_u16(m_inv), vget_low_u16(b));                                 \
        blend_u32_hi = vmlal_u16(blend_u32_hi, vget_high_u16(m_inv), vget_high_u16(b));                               \
                                                                                                                      \
        uint16x4_t blend_u16_lo = vqrshrun_n_s32(                                                                     \
            vreinterpretq_s32_u32(blend_u32_lo),                                                                      \
            AOM_BLEND_A64_ROUND_BITS + 2 * FILTER_BITS - round0_bits - COMPOUND_ROUND1_BITS);                         \
        uint16x4_t blend_u16_hi = vqrshrun_n_s32(                                                                     \
            vreinterpretq_s32_u32(blend_u32_hi),                                                                      \
            AOM_BLEND_A64_ROUND_BITS + 2 * FILTER_BITS - round0_bits - COMPOUND_ROUND1_BITS);                         \
                                                                                                                      \
        uint16x8_t blend_u16 = vcombine_u16(blend_u16_lo, blend_u16_hi);                                              \
        blend_u16            = vminq_u16(blend_u16, vdupq_n_u16((1 << bd) - 1));                                      \
                                                                                                                      \
        return blend_u16;                                                                                             \
    }                                                                                                                 \
                                                                                                                      \
    static inline void highbd_##bd##_blend_a64_d16_mask_neon(uint16_t*            dst,                                \
                                                             uint32_t             dst_stride,                         \
                                                             const CONV_BUF_TYPE* src0,                               \
                                                             uint32_t             src0_stride,                        \
                                                             const CONV_BUF_TYPE* src1,                               \
                                                             uint32_t             src1_stride,                        \
                                                             const uint8_t*       mask,                               \
                                                             uint32_t             mask_stride,                        \
                                                             int                  w,                                  \
                                                             int                  h,                                  \
                                                             int                  subw,                               \
                                                             int                  subh) {                                              \
        const int offset_bits  = bd + 2 * FILTER_BITS - round0_bits;                                                  \
        int32_t   round_offset = (1 << (offset_bits - COMPOUND_ROUND1_BITS)) +                                        \
            (1 << (offset_bits - COMPOUND_ROUND1_BITS - 1));                                                          \
        int32x4_t offset = vdupq_n_s32(-(round_offset << AOM_BLEND_A64_ROUND_BITS));                                  \
                                                                                                                      \
        if ((subw | subh) == 0) {                                                                                     \
            if (w >= 8) {                                                                                             \
                do {                                                                                                  \
                    int i = 0;                                                                                        \
                    do {                                                                                              \
                        uint16x8_t m0 = vmovl_u8(vld1_u8(mask + i));                                                  \
                        uint16x8_t s0 = vld1q_u16(src0 + i);                                                          \
                        uint16x8_t s1 = vld1q_u16(src1 + i);                                                          \
                                                                                                                      \
                        uint16x8_t blend = alpha_##bd##_blend_a64_d16_u16x8(m0, s0, s1, offset);                      \
                                                                                                                      \
                        vst1q_u16(dst + i, blend);                                                                    \
                        i += 8;                                                                                       \
                    } while (i < w);                                                                                  \
                                                                                                                      \
                    mask += mask_stride;                                                                              \
                    src0 += src0_stride;                                                                              \
                    src1 += src1_stride;                                                                              \
                    dst += dst_stride;                                                                                \
                } while (--h != 0);                                                                                   \
            } else {                                                                                                  \
                do {                                                                                                  \
                    uint16x8_t m0 = vmovl_u8(load_u8_4x2(mask, mask_stride));                                         \
                    uint16x8_t s0 = load_u16_4x2(src0, src0_stride);                                                  \
                    uint16x8_t s1 = load_u16_4x2(src1, src1_stride);                                                  \
                                                                                                                      \
                    uint16x8_t blend = alpha_##bd##_blend_a64_d16_u16x8(m0, s0, s1, offset);                          \
                                                                                                                      \
                    store_u16x4_strided_x2(dst, dst_stride, blend);                                                   \
                                                                                                                      \
                    mask += 2 * mask_stride;                                                                          \
                    src0 += 2 * src0_stride;                                                                          \
                    src1 += 2 * src1_stride;                                                                          \
                    dst += 2 * dst_stride;                                                                            \
                    h -= 2;                                                                                           \
                } while (h != 0);                                                                                     \
            }                                                                                                         \
        } else if ((subw & subh) == 1) {                                                                              \
            if (w >= 8) {                                                                                             \
                do {                                                                                                  \
                    int i = 0;                                                                                        \
                    do {                                                                                              \
                        uint8x16_t m0 = vld1q_u8(mask + 0 * mask_stride + 2 * i);                                     \
                        uint8x16_t m1 = vld1q_u8(mask + 1 * mask_stride + 2 * i);                                     \
                        uint16x8_t s0 = vld1q_u16(src0 + i);                                                          \
                        uint16x8_t s1 = vld1q_u16(src1 + i);                                                          \
                                                                                                                      \
                        uint16x8_t m_avg = avg_blend_pairwise_long_u8x8_4(                                            \
                            vget_low_u8(m0), vget_low_u8(m1), vget_high_u8(m0), vget_high_u8(m1));                    \
                        uint16x8_t blend = alpha_##bd##_blend_a64_d16_u16x8(m_avg, s0, s1, offset);                   \
                                                                                                                      \
                        vst1q_u16(dst + i, blend);                                                                    \
                        i += 8;                                                                                       \
                    } while (i < w);                                                                                  \
                                                                                                                      \
                    mask += 2 * mask_stride;                                                                          \
                    src0 += src0_stride;                                                                              \
                    src1 += src1_stride;                                                                              \
                    dst += dst_stride;                                                                                \
                } while (--h != 0);                                                                                   \
            } else {                                                                                                  \
                do {                                                                                                  \
                    uint8x8_t  m0 = vld1_u8(mask + 0 * mask_stride);                                                  \
                    uint8x8_t  m1 = vld1_u8(mask + 1 * mask_stride);                                                  \
                    uint8x8_t  m2 = vld1_u8(mask + 2 * mask_stride);                                                  \
                    uint8x8_t  m3 = vld1_u8(mask + 3 * mask_stride);                                                  \
                    uint16x8_t s0 = load_u16_4x2(src0, src0_stride);                                                  \
                    uint16x8_t s1 = load_u16_4x2(src1, src1_stride);                                                  \
                                                                                                                      \
                    uint16x8_t m_avg = avg_blend_pairwise_long_u8x8_4(m0, m1, m2, m3);                                \
                    uint16x8_t blend = alpha_##bd##_blend_a64_d16_u16x8(m_avg, s0, s1, offset);                       \
                                                                                                                      \
                    store_u16x4_strided_x2(dst, dst_stride, blend);                                                   \
                                                                                                                      \
                    mask += 4 * mask_stride;                                                                          \
                    src0 += 2 * src0_stride;                                                                          \
                    src1 += 2 * src1_stride;                                                                          \
                    dst += 2 * dst_stride;                                                                            \
                    h -= 2;                                                                                           \
                } while (h != 0);                                                                                     \
            }                                                                                                         \
        } else if (subw == 1 && subh == 0) {                                                                          \
            if (w >= 8) {                                                                                             \
                do {                                                                                                  \
                    int i = 0;                                                                                        \
                    do {                                                                                              \
                        uint8x8_t  m0 = vld1_u8(mask + 2 * i);                                                        \
                        uint8x8_t  m1 = vld1_u8(mask + 2 * i + 8);                                                    \
                        uint16x8_t s0 = vld1q_u16(src0 + i);                                                          \
                        uint16x8_t s1 = vld1q_u16(src1 + i);                                                          \
                                                                                                                      \
                        uint16x8_t m_avg = vmovl_u8(vrshr_n_u8(vpadd_u8(m0, m1), 1));                                 \
                        uint16x8_t blend = alpha_##bd##_blend_a64_d16_u16x8(m_avg, s0, s1, offset);                   \
                                                                                                                      \
                        vst1q_u16(dst + i, blend);                                                                    \
                        i += 8;                                                                                       \
                    } while (i < w);                                                                                  \
                                                                                                                      \
                    mask += mask_stride;                                                                              \
                    src0 += src0_stride;                                                                              \
                    src1 += src1_stride;                                                                              \
                    dst += dst_stride;                                                                                \
                } while (--h != 0);                                                                                   \
            } else {                                                                                                  \
                do {                                                                                                  \
                    uint8x8_t  m0 = vld1_u8(mask + 0 * mask_stride);                                                  \
                    uint8x8_t  m1 = vld1_u8(mask + 1 * mask_stride);                                                  \
                    uint16x8_t s0 = load_u16_4x2(src0, src0_stride);                                                  \
                    uint16x8_t s1 = load_u16_4x2(src1, src1_stride);                                                  \
                                                                                                                      \
                    uint16x8_t m_avg = vmovl_u8(vrshr_n_u8(vpadd_u8(m0, m1), 1));                                     \
                    uint16x8_t blend = alpha_##bd##_blend_a64_d16_u16x8(m_avg, s0, s1, offset);                       \
                                                                                                                      \
                    store_u16x4_strided_x2(dst, dst_stride, blend);                                                   \
                                                                                                                      \
                    mask += 2 * mask_stride;                                                                          \
                    src0 += 2 * src0_stride;                                                                          \
                    src1 += 2 * src1_stride;                                                                          \
                    dst += 2 * dst_stride;                                                                            \
                    h -= 2;                                                                                           \
                } while (h != 0);                                                                                     \
            }                                                                                                         \
        } else {                                                                                                      \
            if (w >= 8) {                                                                                             \
                do {                                                                                                  \
                    int i = 0;                                                                                        \
                    do {                                                                                              \
                        uint8x8_t  m0 = vld1_u8(mask + 0 * mask_stride + i);                                          \
                        uint8x8_t  m1 = vld1_u8(mask + 1 * mask_stride + i);                                          \
                        uint16x8_t s0 = vld1q_u16(src0 + i);                                                          \
                        uint16x8_t s1 = vld1q_u16(src1 + i);                                                          \
                                                                                                                      \
                        uint16x8_t m_avg = vmovl_u8(vrhadd_u8(m0, m1));                                               \
                        uint16x8_t blend = alpha_##bd##_blend_a64_d16_u16x8(m_avg, s0, s1, offset);                   \
                                                                                                                      \
                        vst1q_u16(dst + i, blend);                                                                    \
                        i += 8;                                                                                       \
                    } while (i < w);                                                                                  \
                                                                                                                      \
                    mask += 2 * mask_stride;                                                                          \
                    src0 += src0_stride;                                                                              \
                    src1 += src1_stride;                                                                              \
                    dst += dst_stride;                                                                                \
                } while (--h != 0);                                                                                   \
            } else {                                                                                                  \
                do {                                                                                                  \
                    uint8x8_t  m0_2 = load_u8_4x2(mask + 0 * mask_stride, 2 * mask_stride);                           \
                    uint8x8_t  m1_3 = load_u8_4x2(mask + 1 * mask_stride, 2 * mask_stride);                           \
                    uint16x8_t s0   = load_u16_4x2(src0, src0_stride);                                                \
                    uint16x8_t s1   = load_u16_4x2(src1, src1_stride);                                                \
                                                                                                                      \
                    uint16x8_t m_avg = vmovl_u8(vrhadd_u8(m0_2, m1_3));                                               \
                    uint16x8_t blend = alpha_##bd##_blend_a64_d16_u16x8(m_avg, s0, s1, offset);                       \
                                                                                                                      \
                    store_u16x4_strided_x2(dst, dst_stride, blend);                                                   \
                                                                                                                      \
                    mask += 4 * mask_stride;                                                                          \
                    src0 += 2 * src0_stride;                                                                          \
                    src1 += 2 * src1_stride;                                                                          \
                    dst += 2 * dst_stride;                                                                            \
                    h -= 2;                                                                                           \
                } while (h != 0);                                                                                     \
            }                                                                                                         \
        }                                                                                                             \
    }

// 10 bitdepth
HBD_BLEND_A64_D16_MASK(10, ROUND0_BITS)
// 8 bitdepth
HBD_BLEND_A64_D16_MASK(8, ROUND0_BITS)

void svt_aom_highbd_blend_a64_d16_mask_neon(uint8_t* dst_8, uint32_t dst_stride, const CONV_BUF_TYPE* src0,
                                            uint32_t src0_stride, const CONV_BUF_TYPE* src1, uint32_t src1_stride,
                                            const uint8_t* mask, uint32_t mask_stride, int w, int h, int subw, int subh,
                                            ConvolveParams* conv_params, const int bd) {
    (void)conv_params;
    assert(h >= 1);
    assert(w >= 1);
    assert(IS_POWER_OF_TWO(h));
    assert(IS_POWER_OF_TWO(w));

    uint16_t* dst = (uint16_t*)dst_8;
    assert(IMPLIES(src0 == dst, src0_stride == dst_stride));
    assert(IMPLIES(src1 == dst, src1_stride == dst_stride));

    if (bd == 10) {
        highbd_10_blend_a64_d16_mask_neon(
            dst, dst_stride, src0, src0_stride, src1, src1_stride, mask, mask_stride, w, h, subw, subh);
    } else {
        highbd_8_blend_a64_d16_mask_neon(
            dst, dst_stride, src0, src0_stride, src1, src1_stride, mask, mask_stride, w, h, subw, subh);
    }
}

static inline uint16x4_t alpha_blend_a64_u16x4(uint16x4_t m, uint16x4_t a, uint16x4_t b) {
    const uint16x4_t m_inv = vsub_u16(vdup_n_u16(AOM_BLEND_A64_MAX_ALPHA), m);

    uint32x4_t blend_u16 = vmull_u16(m, a);

    blend_u16 = vmlal_u16(blend_u16, m_inv, b);

    return vrshrn_n_u32(blend_u16, AOM_BLEND_A64_ROUND_BITS);
}

void svt_aom_highbd_blend_a64_hmask_16bit_neon(uint16_t* dst, uint32_t dst_stride, const uint16_t* src0,
                                               uint32_t src0_stride, const uint16_t* src1, uint32_t src1_stride,
                                               const uint8_t* mask, int w, int h, int bd) {
    (void)bd;

    assert(IMPLIES(src0 == dst, src0_stride == dst_stride));
    assert(IMPLIES(src1 == dst, src1_stride == dst_stride));

    assert(h >= 1);
    assert(w >= 1);
    assert(IS_POWER_OF_TWO(h));
    assert(IS_POWER_OF_TWO(w));

    assert(bd == 8 || bd == 10 || bd == 12);

    if (w >= 8) {
        do {
            int i = 0;
            do {
                uint16x8_t m0 = vmovl_u8(vld1_u8(mask + i));
                uint16x8_t s0 = vld1q_u16(src0 + i);
                uint16x8_t s1 = vld1q_u16(src1 + i);

                uint16x8_t blend = alpha_blend_a64_u16x8(m0, s0, s1);

                vst1q_u16(dst + i, blend);
                i += 8;
            } while (i < w);

            src0 += src0_stride;
            src1 += src1_stride;
            dst += dst_stride;
        } while (--h != 0);
    } else if (w == 4) {
        const uint16x8_t m0 = vmovl_u8(load_dup_u8_4x2(mask));
        do {
            uint16x8_t s0 = load_u16_4x2(src0, src0_stride);
            uint16x8_t s1 = load_u16_4x2(src1, src1_stride);

            uint16x8_t blend = alpha_blend_a64_u16x8(m0, s0, s1);

            store_u16x4_strided_x2(dst, dst_stride, blend);

            src0 += 2 * src0_stride;
            src1 += 2 * src1_stride;
            dst += 2 * dst_stride;
            h -= 2;
        } while (h != 0);
    } else {
        assert(w == 2);
        const uint16x4_t m0 = vget_low_u16(vmovl_u8(load_dup_u8_2x4(mask)));
        do {
            uint16x4_t s0 = load_u16_2x2(src0, src0_stride);
            uint16x4_t s1 = load_u16_2x2(src1, src1_stride);

            uint16x4_t blend = alpha_blend_a64_u16x4(m0, s0, s1);

            store_u16x2_strided_x2(dst, dst_stride, blend);

            src0 += 2 * src0_stride;
            src1 += 2 * src1_stride;
            dst += 2 * dst_stride;
            h -= 2;
        } while (h != 0);
    }
}

void svt_aom_highbd_blend_a64_vmask_16bit_neon(uint16_t* dst, uint32_t dst_stride, const uint16_t* src0,
                                               uint32_t src0_stride, const uint16_t* src1, uint32_t src1_stride,
                                               const uint8_t* mask, int w, int h, int bd) {
    (void)bd;

    assert(IMPLIES(src0 == dst, src0_stride == dst_stride));
    assert(IMPLIES(src1 == dst, src1_stride == dst_stride));

    assert(h >= 1);
    assert(w >= 1);
    assert(IS_POWER_OF_TWO(h));
    assert(IS_POWER_OF_TWO(w));

    assert(bd == 8 || bd == 10 || bd == 12);

    if (w >= 8) {
        do {
            uint16x8_t m = vmovl_u8(vdup_n_u8(mask[0]));
            int        i = 0;
            do {
                uint16x8_t s0 = vld1q_u16(src0 + i);
                uint16x8_t s1 = vld1q_u16(src1 + i);

                uint16x8_t blend = alpha_blend_a64_u16x8(m, s0, s1);

                vst1q_u16(dst + i, blend);
                i += 8;
            } while (i < w);

            mask += 1;
            src0 += src0_stride;
            src1 += src1_stride;
            dst += dst_stride;
        } while (--h != 0);
    } else if (w == 4) {
        do {
            uint16x4_t m1 = vdup_n_u16((uint16_t)mask[0]);
            uint16x4_t m2 = vdup_n_u16((uint16_t)mask[1]);
            uint16x8_t m  = vcombine_u16(m1, m2);
            uint16x8_t s0 = load_u16_4x2(src0, src0_stride);
            uint16x8_t s1 = load_u16_4x2(src1, src1_stride);

            uint16x8_t blend = alpha_blend_a64_u16x8(m, s0, s1);

            store_u16x4_strided_x2(dst, dst_stride, blend);

            mask += 2;
            src0 += 2 * src0_stride;
            src1 += 2 * src1_stride;
            dst += 2 * dst_stride;
            h -= 2;
        } while (h != 0);
    } else {
        assert(w == 2);
        do {
            uint16x4_t m0    = vdup_n_u16(0);
            m0               = vld1_lane_u16((uint16_t*)mask, m0, 0);
            uint8x8_t m0_zip = vzip_u8(vreinterpret_u8_u16(m0), vreinterpret_u8_u16(m0)).val[0];
            m0               = vget_low_u16(vmovl_u8(m0_zip));
            uint16x4_t s0    = load_u16_2x2(src0, src0_stride);
            uint16x4_t s1    = load_u16_2x2(src1, src1_stride);

            uint16x4_t blend = alpha_blend_a64_u16x4(m0, s0, s1);

            store_u16x2_strided_x2(dst, dst_stride, blend);

            mask += 2;
            src0 += 2 * src0_stride;
            src1 += 2 * src1_stride;
            dst += 2 * dst_stride;
            h -= 2;
        } while (h != 0);
    }
}
