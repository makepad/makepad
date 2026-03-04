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

#ifndef AOM_AOM_DSP_ARM_MEM_NEON_H_
#define AOM_AOM_DSP_ARM_MEM_NEON_H_

#include <arm_neon.h>
#include "definitions.h"
#include "convolve.h"

typedef int32_t tran_low_t;

static inline int get_filter_tap(const InterpFilterParams* const filter_params, int subpel_qn) {
    const int16_t* const filter = av1_get_interp_filter_subpel_kernel(*filter_params, subpel_qn & SUBPEL_MASK);
    if (filter_params->taps == 12) {
        return 12;
    }
    if (filter[0] | filter[7]) {
        return 8;
    }
    if (filter[1] | filter[6]) {
        return 6;
    }
    if (filter[2] | filter[5]) {
        return 4;
    }
    return 2;
}

static inline void store_u8_8x2(uint8_t* s, ptrdiff_t p, const uint8x8_t s0, const uint8x8_t s1) {
    vst1_u8(s, s0);
    vst1_u8(s + p, s1);
}

static inline uint8x16_t load_u8_8x2(const uint8_t* s, ptrdiff_t p) {
    return vcombine_u8(vld1_u8(s), vld1_u8(s + p));
}

/* These intrinsics require immediate values, so we must use #defines
   to enforce that. */
#define load_u8_4x1_lane(s, s0, lane)                                                                 \
    do {                                                                                              \
        *(s0) = vreinterpret_u8_u32(vld1_lane_u32((uint32_t*)(s), vreinterpret_u32_u8(*(s0)), lane)); \
    } while (0)

// Load four bytes into the low half of a uint8x8_t, zero the upper half.
static inline uint8x8_t load_u8_4x1(const uint8_t* p) {
    uint8x8_t ret = vdup_n_u8(0);
    load_u8_4x1_lane(p, &ret, 0);
    return ret;
}

static inline uint8x8_t load_unaligned_u8_4x1(const uint8_t* buf) {
    uint32_t   a;
    uint32x2_t a_u32;

    memcpy(&a, buf, 4);
    a_u32 = vdup_n_u32(0);
    a_u32 = vset_lane_u32(a, a_u32, 0);
    return vreinterpret_u8_u32(a_u32);
}

// Load two blocks of 32-bits into a single vector.
static inline uint8x8_t load_u8x4_strided_x2(uint8_t* src, ptrdiff_t stride) {
    uint8x8_t ret = vdup_n_u8(0);
    load_u8_4x1_lane(src, &ret, 0);
    src += stride;
    load_u8_4x1_lane(src, &ret, 1);
    return ret;
}

#undef load_u8_4x1_lane

static inline void load_u8_8x8(const uint8_t* s, ptrdiff_t p, uint8x8_t* const s0, uint8x8_t* const s1,
                               uint8x8_t* const s2, uint8x8_t* const s3, uint8x8_t* const s4, uint8x8_t* const s5,
                               uint8x8_t* const s6, uint8x8_t* const s7) {
    *s0 = vld1_u8(s);
    s += p;
    *s1 = vld1_u8(s);
    s += p;
    *s2 = vld1_u8(s);
    s += p;
    *s3 = vld1_u8(s);
    s += p;
    *s4 = vld1_u8(s);
    s += p;
    *s5 = vld1_u8(s);
    s += p;
    *s6 = vld1_u8(s);
    s += p;
    *s7 = vld1_u8(s);
}

static inline void load_u8_8x7(const uint8_t* s, ptrdiff_t p, uint8x8_t* const s0, uint8x8_t* const s1,
                               uint8x8_t* const s2, uint8x8_t* const s3, uint8x8_t* const s4, uint8x8_t* const s5,
                               uint8x8_t* const s6) {
    *s0 = vld1_u8(s);
    s += p;
    *s1 = vld1_u8(s);
    s += p;
    *s2 = vld1_u8(s);
    s += p;
    *s3 = vld1_u8(s);
    s += p;
    *s4 = vld1_u8(s);
    s += p;
    *s5 = vld1_u8(s);
    s += p;
    *s6 = vld1_u8(s);
}

static inline void load_u8_8x4(const uint8_t* s, const ptrdiff_t p, uint8x8_t* const s0, uint8x8_t* const s1,
                               uint8x8_t* const s2, uint8x8_t* const s3) {
    *s0 = vld1_u8(s);
    s += p;
    *s1 = vld1_u8(s);
    s += p;
    *s2 = vld1_u8(s);
    s += p;
    *s3 = vld1_u8(s);
}

static inline void load_u8_8x3(const uint8_t* s, const ptrdiff_t p, uint8x8_t* const s0, uint8x8_t* const s1,
                               uint8x8_t* const s2) {
    *s0 = vld1_u8(s);
    s += p;
    *s1 = vld1_u8(s);
    s += p;
    *s2 = vld1_u8(s);
}

static inline void load_u16_4x4(const uint16_t* s, const ptrdiff_t p, uint16x4_t* const s0, uint16x4_t* const s1,
                                uint16x4_t* const s2, uint16x4_t* const s3) {
    *s0 = vld1_u16(s);
    s += p;
    *s1 = vld1_u16(s);
    s += p;
    *s2 = vld1_u16(s);
    s += p;
    *s3 = vld1_u16(s);
}

static inline void load_u16_4x6(const uint16_t* s, ptrdiff_t p, uint16x4_t* const s0, uint16x4_t* const s1,
                                uint16x4_t* const s2, uint16x4_t* const s3, uint16x4_t* const s4,
                                uint16x4_t* const s5) {
    *s0 = vld1_u16(s);
    s += p;
    *s1 = vld1_u16(s);
    s += p;
    *s2 = vld1_u16(s);
    s += p;
    *s3 = vld1_u16(s);
    s += p;
    *s4 = vld1_u16(s);
    s += p;
    *s5 = vld1_u16(s);
}

static inline void load_u16_4x7(const uint16_t* s, ptrdiff_t p, uint16x4_t* const s0, uint16x4_t* const s1,
                                uint16x4_t* const s2, uint16x4_t* const s3, uint16x4_t* const s4, uint16x4_t* const s5,
                                uint16x4_t* const s6) {
    *s0 = vld1_u16(s);
    s += p;
    *s1 = vld1_u16(s);
    s += p;
    *s2 = vld1_u16(s);
    s += p;
    *s3 = vld1_u16(s);
    s += p;
    *s4 = vld1_u16(s);
    s += p;
    *s5 = vld1_u16(s);
    s += p;
    *s6 = vld1_u16(s);
}

static inline void load_u16_4x8(const uint16_t* s, ptrdiff_t p, uint16x4_t* const s0, uint16x4_t* const s1,
                                uint16x4_t* const s2, uint16x4_t* const s3, uint16x4_t* const s4, uint16x4_t* const s5,
                                uint16x4_t* const s6, uint16x4_t* const s7) {
    *s0 = vld1_u16(s);
    s += p;
    *s1 = vld1_u16(s);
    s += p;
    *s2 = vld1_u16(s);
    s += p;
    *s3 = vld1_u16(s);
    s += p;
    *s4 = vld1_u16(s);
    s += p;
    *s5 = vld1_u16(s);
    s += p;
    *s6 = vld1_u16(s);
    s += p;
    *s7 = vld1_u16(s);
}

static inline void load_u16_4x14(const uint16_t* s, ptrdiff_t p, uint16x4_t* const s0, uint16x4_t* const s1,
                                 uint16x4_t* const s2, uint16x4_t* const s3, uint16x4_t* const s4, uint16x4_t* const s5,
                                 uint16x4_t* const s6, uint16x4_t* const s7, uint16x4_t* const s8, uint16x4_t* const s9,
                                 uint16x4_t* const s10, uint16x4_t* const s11, uint16x4_t* const s12,
                                 uint16x4_t* const s13) {
    *s0 = vld1_u16(s);
    s += p;
    *s1 = vld1_u16(s);
    s += p;
    *s2 = vld1_u16(s);
    s += p;
    *s3 = vld1_u16(s);
    s += p;
    *s4 = vld1_u16(s);
    s += p;
    *s5 = vld1_u16(s);
    s += p;
    *s6 = vld1_u16(s);
    s += p;
    *s7 = vld1_u16(s);
    s += p;
    *s8 = vld1_u16(s);
    s += p;
    *s9 = vld1_u16(s);
    s += p;
    *s10 = vld1_u16(s);
    s += p;
    *s11 = vld1_u16(s);
    s += p;
    *s12 = vld1_u16(s);
    s += p;
    *s13 = vld1_u16(s);
}

static inline void load_s16_8x2(const int16_t* s, const ptrdiff_t p, int16x8_t* const s0, int16x8_t* const s1) {
    *s0 = vld1q_s16(s);
    s += p;
    *s1 = vld1q_s16(s);
}

static inline void load_u16_8x2(const uint16_t* s, const ptrdiff_t p, uint16x8_t* const s0, uint16x8_t* const s1) {
    *s0 = vld1q_u16(s);
    s += p;
    *s1 = vld1q_u16(s);
}

static inline void load_u16_8x4(const uint16_t* s, const ptrdiff_t p, uint16x8_t* const s0, uint16x8_t* const s1,
                                uint16x8_t* const s2, uint16x8_t* const s3) {
    *s0 = vld1q_u16(s);
    s += p;
    *s1 = vld1q_u16(s);
    s += p;
    *s2 = vld1q_u16(s);
    s += p;
    *s3 = vld1q_u16(s);
}

static inline void load_s16_4x12(const int16_t* s, ptrdiff_t p, int16x4_t* const s0, int16x4_t* const s1,
                                 int16x4_t* const s2, int16x4_t* const s3, int16x4_t* const s4, int16x4_t* const s5,
                                 int16x4_t* const s6, int16x4_t* const s7, int16x4_t* const s8, int16x4_t* const s9,
                                 int16x4_t* const s10, int16x4_t* const s11) {
    *s0 = vld1_s16(s);
    s += p;
    *s1 = vld1_s16(s);
    s += p;
    *s2 = vld1_s16(s);
    s += p;
    *s3 = vld1_s16(s);
    s += p;
    *s4 = vld1_s16(s);
    s += p;
    *s5 = vld1_s16(s);
    s += p;
    *s6 = vld1_s16(s);
    s += p;
    *s7 = vld1_s16(s);
    s += p;
    *s8 = vld1_s16(s);
    s += p;
    *s9 = vld1_s16(s);
    s += p;
    *s10 = vld1_s16(s);
    s += p;
    *s11 = vld1_s16(s);
}

static inline void load_s16_4x11(const int16_t* s, ptrdiff_t p, int16x4_t* const s0, int16x4_t* const s1,
                                 int16x4_t* const s2, int16x4_t* const s3, int16x4_t* const s4, int16x4_t* const s5,
                                 int16x4_t* const s6, int16x4_t* const s7, int16x4_t* const s8, int16x4_t* const s9,
                                 int16x4_t* const s10) {
    *s0 = vld1_s16(s);
    s += p;
    *s1 = vld1_s16(s);
    s += p;
    *s2 = vld1_s16(s);
    s += p;
    *s3 = vld1_s16(s);
    s += p;
    *s4 = vld1_s16(s);
    s += p;
    *s5 = vld1_s16(s);
    s += p;
    *s6 = vld1_s16(s);
    s += p;
    *s7 = vld1_s16(s);
    s += p;
    *s8 = vld1_s16(s);
    s += p;
    *s9 = vld1_s16(s);
    s += p;
    *s10 = vld1_s16(s);
}

static inline void load_u16_4x11(const uint16_t* s, ptrdiff_t p, uint16x4_t* const s0, uint16x4_t* const s1,
                                 uint16x4_t* const s2, uint16x4_t* const s3, uint16x4_t* const s4, uint16x4_t* const s5,
                                 uint16x4_t* const s6, uint16x4_t* const s7, uint16x4_t* const s8, uint16x4_t* const s9,
                                 uint16x4_t* const s10) {
    *s0 = vld1_u16(s);
    s += p;
    *s1 = vld1_u16(s);
    s += p;
    *s2 = vld1_u16(s);
    s += p;
    *s3 = vld1_u16(s);
    s += p;
    *s4 = vld1_u16(s);
    s += p;
    *s5 = vld1_u16(s);
    s += p;
    *s6 = vld1_u16(s);
    s += p;
    *s7 = vld1_u16(s);
    s += p;
    *s8 = vld1_u16(s);
    s += p;
    *s9 = vld1_u16(s);
    s += p;
    *s10 = vld1_u16(s);
}

static inline void load_s16_4x8(const int16_t* s, ptrdiff_t p, int16x4_t* const s0, int16x4_t* const s1,
                                int16x4_t* const s2, int16x4_t* const s3, int16x4_t* const s4, int16x4_t* const s5,
                                int16x4_t* const s6, int16x4_t* const s7) {
    *s0 = vld1_s16(s);
    s += p;
    *s1 = vld1_s16(s);
    s += p;
    *s2 = vld1_s16(s);
    s += p;
    *s3 = vld1_s16(s);
    s += p;
    *s4 = vld1_s16(s);
    s += p;
    *s5 = vld1_s16(s);
    s += p;
    *s6 = vld1_s16(s);
    s += p;
    *s7 = vld1_s16(s);
}

static inline void load_s16_4x7(const int16_t* s, ptrdiff_t p, int16x4_t* const s0, int16x4_t* const s1,
                                int16x4_t* const s2, int16x4_t* const s3, int16x4_t* const s4, int16x4_t* const s5,
                                int16x4_t* const s6) {
    *s0 = vld1_s16(s);
    s += p;
    *s1 = vld1_s16(s);
    s += p;
    *s2 = vld1_s16(s);
    s += p;
    *s3 = vld1_s16(s);
    s += p;
    *s4 = vld1_s16(s);
    s += p;
    *s5 = vld1_s16(s);
    s += p;
    *s6 = vld1_s16(s);
}

static inline void load_s16_4x6(const int16_t* s, ptrdiff_t p, int16x4_t* const s0, int16x4_t* const s1,
                                int16x4_t* const s2, int16x4_t* const s3, int16x4_t* const s4, int16x4_t* const s5) {
    *s0 = vld1_s16(s);
    s += p;
    *s1 = vld1_s16(s);
    s += p;
    *s2 = vld1_s16(s);
    s += p;
    *s3 = vld1_s16(s);
    s += p;
    *s4 = vld1_s16(s);
    s += p;
    *s5 = vld1_s16(s);
}

static inline void load_s16_4x5(const int16_t* s, ptrdiff_t p, int16x4_t* const s0, int16x4_t* const s1,
                                int16x4_t* const s2, int16x4_t* const s3, int16x4_t* const s4) {
    *s0 = vld1_s16(s);
    s += p;
    *s1 = vld1_s16(s);
    s += p;
    *s2 = vld1_s16(s);
    s += p;
    *s3 = vld1_s16(s);
    s += p;
    *s4 = vld1_s16(s);
}

static inline void load_s16_4x3(const int16_t* s, ptrdiff_t p, int16x4_t* const s0, int16x4_t* const s1,
                                int16x4_t* const s2) {
    *s0 = vld1_s16(s);
    s += p;
    *s1 = vld1_s16(s);
    s += p;
    *s2 = vld1_s16(s);
}

static inline void load_u16_4x5(const uint16_t* s, const ptrdiff_t p, uint16x4_t* const s0, uint16x4_t* const s1,
                                uint16x4_t* const s2, uint16x4_t* const s3, uint16x4_t* const s4) {
    *s0 = vld1_u16(s);
    s += p;
    *s1 = vld1_u16(s);
    s += p;
    *s2 = vld1_u16(s);
    s += p;
    *s3 = vld1_u16(s);
    s += p;
    *s4 = vld1_u16(s);
}

static inline void load_u8_8x5(const uint8_t* s, ptrdiff_t p, uint8x8_t* const s0, uint8x8_t* const s1,
                               uint8x8_t* const s2, uint8x8_t* const s3, uint8x8_t* const s4) {
    *s0 = vld1_u8(s);
    s += p;
    *s1 = vld1_u8(s);
    s += p;
    *s2 = vld1_u8(s);
    s += p;
    *s3 = vld1_u8(s);
    s += p;
    *s4 = vld1_u8(s);
}

static inline void load_u16_8x5(const uint16_t* s, const ptrdiff_t p, uint16x8_t* const s0, uint16x8_t* const s1,
                                uint16x8_t* const s2, uint16x8_t* const s3, uint16x8_t* const s4) {
    *s0 = vld1q_u16(s);
    s += p;
    *s1 = vld1q_u16(s);
    s += p;
    *s2 = vld1q_u16(s);
    s += p;
    *s3 = vld1q_u16(s);
    s += p;
    *s4 = vld1q_u16(s);
}

static inline void load_s16_4x4(const int16_t* s, ptrdiff_t p, int16x4_t* const s0, int16x4_t* const s1,
                                int16x4_t* const s2, int16x4_t* const s3) {
    *s0 = vld1_s16(s);
    s += p;
    *s1 = vld1_s16(s);
    s += p;
    *s2 = vld1_s16(s);
    s += p;
    *s3 = vld1_s16(s);
}

static inline void store_u8_8x8(uint8_t* s, ptrdiff_t p, const uint8x8_t s0, const uint8x8_t s1, const uint8x8_t s2,
                                const uint8x8_t s3, const uint8x8_t s4, const uint8x8_t s5, const uint8x8_t s6,
                                const uint8x8_t s7) {
    vst1_u8(s, s0);
    s += p;
    vst1_u8(s, s1);
    s += p;
    vst1_u8(s, s2);
    s += p;
    vst1_u8(s, s3);
    s += p;
    vst1_u8(s, s4);
    s += p;
    vst1_u8(s, s5);
    s += p;
    vst1_u8(s, s6);
    s += p;
    vst1_u8(s, s7);
}

static inline void store_u8_8x4(uint8_t* s, ptrdiff_t p, const uint8x8_t s0, const uint8x8_t s1, const uint8x8_t s2,
                                const uint8x8_t s3) {
    vst1_u8(s, s0);
    s += p;
    vst1_u8(s, s1);
    s += p;
    vst1_u8(s, s2);
    s += p;
    vst1_u8(s, s3);
}

static inline void store_u8_8x16(uint8_t* s, ptrdiff_t p, const uint8x16_t s0, const uint8x16_t s1, const uint8x16_t s2,
                                 const uint8x16_t s3) {
    vst1q_u8(s, s0);
    s += p;
    vst1q_u8(s, s1);
    s += p;
    vst1q_u8(s, s2);
    s += p;
    vst1q_u8(s, s3);
}

static inline void store_u16_8x8(uint16_t* s, ptrdiff_t dst_stride, const uint16x8_t s0, const uint16x8_t s1,
                                 const uint16x8_t s2, const uint16x8_t s3, const uint16x8_t s4, const uint16x8_t s5,
                                 const uint16x8_t s6, const uint16x8_t s7) {
    vst1q_u16(s, s0);
    s += dst_stride;
    vst1q_u16(s, s1);
    s += dst_stride;
    vst1q_u16(s, s2);
    s += dst_stride;
    vst1q_u16(s, s3);
    s += dst_stride;
    vst1q_u16(s, s4);
    s += dst_stride;
    vst1q_u16(s, s5);
    s += dst_stride;
    vst1q_u16(s, s6);
    s += dst_stride;
    vst1q_u16(s, s7);
}

static inline void store_u16_4x12(uint16_t* s, ptrdiff_t dst_stride, const uint16x4_t s0, const uint16x4_t s1,
                                  const uint16x4_t s2, const uint16x4_t s3, const uint16x4_t s4, const uint16x4_t s5,
                                  const uint16x4_t s6, const uint16x4_t s7, const uint16x4_t s8, const uint16x4_t s9,
                                  const uint16x4_t s10, const uint16x4_t s11) {
    vst1_u16(s, s0);
    s += dst_stride;
    vst1_u16(s, s1);
    s += dst_stride;
    vst1_u16(s, s2);
    s += dst_stride;
    vst1_u16(s, s3);
    s += dst_stride;
    vst1_u16(s, s4);
    s += dst_stride;
    vst1_u16(s, s5);
    s += dst_stride;
    vst1_u16(s, s6);
    s += dst_stride;
    vst1_u16(s, s7);
    s += dst_stride;
    vst1_u16(s, s8);
    s += dst_stride;
    vst1_u16(s, s9);
    s += dst_stride;
    vst1_u16(s, s10);
    s += dst_stride;
    vst1_u16(s, s11);
}

static inline void store_u16_4x6(uint16_t* s, ptrdiff_t dst_stride, const uint16x4_t s0, const uint16x4_t s1,
                                 const uint16x4_t s2, const uint16x4_t s3, const uint16x4_t s4, const uint16x4_t s5) {
    vst1_u16(s, s0);
    s += dst_stride;
    vst1_u16(s, s1);
    s += dst_stride;
    vst1_u16(s, s2);
    s += dst_stride;
    vst1_u16(s, s3);
    s += dst_stride;
    vst1_u16(s, s4);
    s += dst_stride;
    vst1_u16(s, s5);
}

static inline void store_u16_4x4(uint16_t* s, ptrdiff_t dst_stride, const uint16x4_t s0, const uint16x4_t s1,
                                 const uint16x4_t s2, const uint16x4_t s3) {
    vst1_u16(s, s0);
    s += dst_stride;
    vst1_u16(s, s1);
    s += dst_stride;
    vst1_u16(s, s2);
    s += dst_stride;
    vst1_u16(s, s3);
}

static inline void store_u16_4x3(uint16_t* s, ptrdiff_t dst_stride, const uint16x4_t s0, const uint16x4_t s1,
                                 const uint16x4_t s2) {
    vst1_u16(s, s0);
    s += dst_stride;
    vst1_u16(s, s1);
    s += dst_stride;
    vst1_u16(s, s2);
}

static inline void store_u16_8x2(uint16_t* s, ptrdiff_t dst_stride, const uint16x8_t s0, const uint16x8_t s1) {
    vst1q_u16(s, s0);
    s += dst_stride;
    vst1q_u16(s, s1);
}

static inline void store_u16_8x3(uint16_t* s, ptrdiff_t dst_stride, const uint16x8_t s0, const uint16x8_t s1,
                                 const uint16x8_t s2) {
    vst1q_u16(s, s0);
    s += dst_stride;
    vst1q_u16(s, s1);
    s += dst_stride;
    vst1q_u16(s, s2);
}

static inline void store_u16_8x4(uint16_t* s, ptrdiff_t dst_stride, const uint16x8_t s0, const uint16x8_t s1,
                                 const uint16x8_t s2, const uint16x8_t s3) {
    vst1q_u16(s, s0);
    s += dst_stride;
    vst1q_u16(s, s1);
    s += dst_stride;
    vst1q_u16(s, s2);
    s += dst_stride;
    vst1q_u16(s, s3);
}

static inline void store_s16_8x8(int16_t* s, ptrdiff_t dst_stride, const int16x8_t s0, const int16x8_t s1,
                                 const int16x8_t s2, const int16x8_t s3, const int16x8_t s4, const int16x8_t s5,
                                 const int16x8_t s6, const int16x8_t s7) {
    vst1q_s16(s, s0);
    s += dst_stride;
    vst1q_s16(s, s1);
    s += dst_stride;
    vst1q_s16(s, s2);
    s += dst_stride;
    vst1q_s16(s, s3);
    s += dst_stride;
    vst1q_s16(s, s4);
    s += dst_stride;
    vst1q_s16(s, s5);
    s += dst_stride;
    vst1q_s16(s, s6);
    s += dst_stride;
    vst1q_s16(s, s7);
}

static inline void store_s16_4x8(int16_t* s, ptrdiff_t dst_stride, const int16x4_t s0, const int16x4_t s1,
                                 const int16x4_t s2, const int16x4_t s3, const int16x4_t s4, const int16x4_t s5,
                                 const int16x4_t s6, const int16x4_t s7) {
    vst1_s16(s, s0);
    s += dst_stride;
    vst1_s16(s, s1);
    s += dst_stride;
    vst1_s16(s, s2);
    s += dst_stride;
    vst1_s16(s, s3);
    s += dst_stride;
    vst1_s16(s, s4);
    s += dst_stride;
    vst1_s16(s, s5);
    s += dst_stride;
    vst1_s16(s, s6);
    s += dst_stride;
    vst1_s16(s, s7);
}

static inline void store_s16_4x4(int16_t* s, ptrdiff_t dst_stride, const int16x4_t s0, const int16x4_t s1,
                                 const int16x4_t s2, const int16x4_t s3) {
    vst1_s16(s, s0);
    s += dst_stride;
    vst1_s16(s, s1);
    s += dst_stride;
    vst1_s16(s, s2);
    s += dst_stride;
    vst1_s16(s, s3);
}

static inline void store_s16_8x2(int16_t* s, ptrdiff_t dst_stride, const int16x8_t s0, const int16x8_t s1) {
    vst1q_s16(s, s0);
    s += dst_stride;
    vst1q_s16(s, s1);
}

#define store_u16_2x1_lane(dst, src, lane)                           \
    do {                                                             \
        uint32_t a = vget_lane_u32(vreinterpret_u32_u16(src), lane); \
        memcpy(dst, &a, 4);                                          \
    } while (0)

#define store_s16_2x1_lane(dst, src, lane)                          \
    do {                                                            \
        int32_t a = vget_lane_s32(vreinterpret_s32_s16(src), lane); \
        memcpy(dst, &a, 4);                                         \
    } while (0)

#define store_u16_4x1_lane(dst, src, lane)                             \
    do {                                                               \
        uint64_t a = vgetq_lane_u64(vreinterpretq_u64_u16(src), lane); \
        memcpy(dst, &a, 8);                                            \
    } while (0)

#define store_s16_4x1_lane(dst, src, lane)                            \
    do {                                                              \
        int64_t a = vgetq_lane_s64(vreinterpretq_s64_s16(src), lane); \
        memcpy(dst, &a, 8);                                           \
    } while (0)

// Store the low 32-bits from a single vector.
static inline void store_u16_2x1(uint16_t* dst, const uint16x4_t src) {
    store_u16_2x1_lane(dst, src, 0);
}

static inline void store_s16_8x4(int16_t* s, ptrdiff_t dst_stride, const int16x8_t s0, const int16x8_t s1,
                                 const int16x8_t s2, const int16x8_t s3) {
    vst1q_s16(s, s0);
    s += dst_stride;
    vst1q_s16(s, s1);
    s += dst_stride;
    vst1q_s16(s, s2);
    s += dst_stride;
    vst1q_s16(s, s3);
}

static inline void load_u8_8x11(const uint8_t* s, ptrdiff_t p, uint8x8_t* const s0, uint8x8_t* const s1,
                                uint8x8_t* const s2, uint8x8_t* const s3, uint8x8_t* const s4, uint8x8_t* const s5,
                                uint8x8_t* const s6, uint8x8_t* const s7, uint8x8_t* const s8, uint8x8_t* const s9,
                                uint8x8_t* const s10) {
    *s0 = vld1_u8(s);
    s += p;
    *s1 = vld1_u8(s);
    s += p;
    *s2 = vld1_u8(s);
    s += p;
    *s3 = vld1_u8(s);
    s += p;
    *s4 = vld1_u8(s);
    s += p;
    *s5 = vld1_u8(s);
    s += p;
    *s6 = vld1_u8(s);
    s += p;
    *s7 = vld1_u8(s);
    s += p;
    *s8 = vld1_u8(s);
    s += p;
    *s9 = vld1_u8(s);
    s += p;
    *s10 = vld1_u8(s);
}

static inline void load_s16_8x10(const int16_t* s, ptrdiff_t p, int16x8_t* const s0, int16x8_t* const s1,
                                 int16x8_t* const s2, int16x8_t* const s3, int16x8_t* const s4, int16x8_t* const s5,
                                 int16x8_t* const s6, int16x8_t* const s7, int16x8_t* const s8, int16x8_t* const s9) {
    *s0 = vld1q_s16(s);
    s += p;
    *s1 = vld1q_s16(s);
    s += p;
    *s2 = vld1q_s16(s);
    s += p;
    *s3 = vld1q_s16(s);
    s += p;
    *s4 = vld1q_s16(s);
    s += p;
    *s5 = vld1q_s16(s);
    s += p;
    *s6 = vld1q_s16(s);
    s += p;
    *s7 = vld1q_s16(s);
    s += p;
    *s8 = vld1q_s16(s);
    s += p;
    *s9 = vld1q_s16(s);
}

static inline void load_s16_8x11(const int16_t* s, ptrdiff_t p, int16x8_t* const s0, int16x8_t* const s1,
                                 int16x8_t* const s2, int16x8_t* const s3, int16x8_t* const s4, int16x8_t* const s5,
                                 int16x8_t* const s6, int16x8_t* const s7, int16x8_t* const s8, int16x8_t* const s9,
                                 int16x8_t* const s10) {
    *s0 = vld1q_s16(s);
    s += p;
    *s1 = vld1q_s16(s);
    s += p;
    *s2 = vld1q_s16(s);
    s += p;
    *s3 = vld1q_s16(s);
    s += p;
    *s4 = vld1q_s16(s);
    s += p;
    *s5 = vld1q_s16(s);
    s += p;
    *s6 = vld1q_s16(s);
    s += p;
    *s7 = vld1q_s16(s);
    s += p;
    *s8 = vld1q_s16(s);
    s += p;
    *s9 = vld1q_s16(s);
    s += p;
    *s10 = vld1q_s16(s);
}

static inline void load_s16_8x12(const int16_t* s, ptrdiff_t p, int16x8_t* const s0, int16x8_t* const s1,
                                 int16x8_t* const s2, int16x8_t* const s3, int16x8_t* const s4, int16x8_t* const s5,
                                 int16x8_t* const s6, int16x8_t* const s7, int16x8_t* const s8, int16x8_t* const s9,
                                 int16x8_t* const s10, int16x8_t* const s11) {
    *s0 = vld1q_s16(s);
    s += p;
    *s1 = vld1q_s16(s);
    s += p;
    *s2 = vld1q_s16(s);
    s += p;
    *s3 = vld1q_s16(s);
    s += p;
    *s4 = vld1q_s16(s);
    s += p;
    *s5 = vld1q_s16(s);
    s += p;
    *s6 = vld1q_s16(s);
    s += p;
    *s7 = vld1q_s16(s);
    s += p;
    *s8 = vld1q_s16(s);
    s += p;
    *s9 = vld1q_s16(s);
    s += p;
    *s10 = vld1q_s16(s);
    s += p;
    *s11 = vld1q_s16(s);
}

static inline void load_u16_8x11(const uint16_t* s, ptrdiff_t p, uint16x8_t* const s0, uint16x8_t* const s1,
                                 uint16x8_t* const s2, uint16x8_t* const s3, uint16x8_t* const s4, uint16x8_t* const s5,
                                 uint16x8_t* const s6, uint16x8_t* const s7, uint16x8_t* const s8, uint16x8_t* const s9,
                                 uint16x8_t* const s10) {
    *s0 = vld1q_u16(s);
    s += p;
    *s1 = vld1q_u16(s);
    s += p;
    *s2 = vld1q_u16(s);
    s += p;
    *s3 = vld1q_u16(s);
    s += p;
    *s4 = vld1q_u16(s);
    s += p;
    *s5 = vld1q_u16(s);
    s += p;
    *s6 = vld1q_u16(s);
    s += p;
    *s7 = vld1q_u16(s);
    s += p;
    *s8 = vld1q_u16(s);
    s += p;
    *s9 = vld1q_u16(s);
    s += p;
    *s10 = vld1q_u16(s);
}

static inline void load_s16_8x8(const int16_t* s, ptrdiff_t p, int16x8_t* const s0, int16x8_t* const s1,
                                int16x8_t* const s2, int16x8_t* const s3, int16x8_t* const s4, int16x8_t* const s5,
                                int16x8_t* const s6, int16x8_t* const s7) {
    *s0 = vld1q_s16(s);
    s += p;
    *s1 = vld1q_s16(s);
    s += p;
    *s2 = vld1q_s16(s);
    s += p;
    *s3 = vld1q_s16(s);
    s += p;
    *s4 = vld1q_s16(s);
    s += p;
    *s5 = vld1q_s16(s);
    s += p;
    *s6 = vld1q_s16(s);
    s += p;
    *s7 = vld1q_s16(s);
}

static inline void load_u16_8x7(const uint16_t* s, ptrdiff_t p, uint16x8_t* const s0, uint16x8_t* const s1,
                                uint16x8_t* const s2, uint16x8_t* const s3, uint16x8_t* const s4, uint16x8_t* const s5,
                                uint16x8_t* const s6) {
    *s0 = vld1q_u16(s);
    s += p;
    *s1 = vld1q_u16(s);
    s += p;
    *s2 = vld1q_u16(s);
    s += p;
    *s3 = vld1q_u16(s);
    s += p;
    *s4 = vld1q_u16(s);
    s += p;
    *s5 = vld1q_u16(s);
    s += p;
    *s6 = vld1q_u16(s);
}

static inline void load_s16_8x7(const int16_t* s, ptrdiff_t p, int16x8_t* const s0, int16x8_t* const s1,
                                int16x8_t* const s2, int16x8_t* const s3, int16x8_t* const s4, int16x8_t* const s5,
                                int16x8_t* const s6) {
    *s0 = vld1q_s16(s);
    s += p;
    *s1 = vld1q_s16(s);
    s += p;
    *s2 = vld1q_s16(s);
    s += p;
    *s3 = vld1q_s16(s);
    s += p;
    *s4 = vld1q_s16(s);
    s += p;
    *s5 = vld1q_s16(s);
    s += p;
    *s6 = vld1q_s16(s);
}

static inline void load_s16_8x6(const int16_t* s, ptrdiff_t p, int16x8_t* const s0, int16x8_t* const s1,
                                int16x8_t* const s2, int16x8_t* const s3, int16x8_t* const s4, int16x8_t* const s5) {
    *s0 = vld1q_s16(s);
    s += p;
    *s1 = vld1q_s16(s);
    s += p;
    *s2 = vld1q_s16(s);
    s += p;
    *s3 = vld1q_s16(s);
    s += p;
    *s4 = vld1q_s16(s);
    s += p;
    *s5 = vld1q_s16(s);
}

static inline void load_s16_8x5(const int16_t* s, ptrdiff_t p, int16x8_t* const s0, int16x8_t* const s1,
                                int16x8_t* const s2, int16x8_t* const s3, int16x8_t* const s4) {
    *s0 = vld1q_s16(s);
    s += p;
    *s1 = vld1q_s16(s);
    s += p;
    *s2 = vld1q_s16(s);
    s += p;
    *s3 = vld1q_s16(s);
    s += p;
    *s4 = vld1q_s16(s);
}

static inline void load_s16_8x4(const int16_t* s, ptrdiff_t p, int16x8_t* const s0, int16x8_t* const s1,
                                int16x8_t* const s2, int16x8_t* const s3) {
    *s0 = vld1q_s16(s);
    s += p;
    *s1 = vld1q_s16(s);
    s += p;
    *s2 = vld1q_s16(s);
    s += p;
    *s3 = vld1q_s16(s);
}

static inline void load_s16_8x3(const int16_t* s, ptrdiff_t p, int16x8_t* const s0, int16x8_t* const s1,
                                int16x8_t* const s2) {
    *s0 = vld1q_s16(s);
    s += p;
    *s1 = vld1q_s16(s);
    s += p;
    *s2 = vld1q_s16(s);
}

#define load_u32_2x1_lane(v, p, lane)                           \
    do {                                                        \
        (v) = vld1_lane_u32((const uint32_t*)(p), (v), (lane)); \
    } while (0)
#define load_u32_4x1_lane(v, p, lane)                            \
    do {                                                         \
        (v) = vld1q_lane_u32((const uint32_t*)(p), (v), (lane)); \
    } while (0)

// Load 2 sets of 4 bytes.
static inline uint8x8_t load_u8_4x2(const uint8_t* buf, ptrdiff_t stride) {
    uint32_t a;
    memcpy(&a, buf, 4);
    buf += stride;
    uint32x2_t a_u32 = vdup_n_u32(a);
    memcpy(&a, buf, 4);
    a_u32 = vset_lane_u32(a, a_u32, 1);
    return vreinterpret_u8_u32(a_u32);
}

// Load 4 sets of 4 bytes.
static inline uint8x16_t load_u8_4x4(const uint8_t* buf, ptrdiff_t stride) {
    uint32_t   a;
    uint32x4_t a_u32;
    if (stride == 4) {
        return vld1q_u8(buf);
    }
    memcpy(&a, buf, 4);
    buf += stride;
    a_u32 = vdupq_n_u32(a);
    memcpy(&a, buf, 4);
    buf += stride;
    a_u32 = vsetq_lane_u32(a, a_u32, 1);
    memcpy(&a, buf, 4);
    buf += stride;
    a_u32 = vsetq_lane_u32(a, a_u32, 2);
    memcpy(&a, buf, 4);
    a_u32 = vsetq_lane_u32(a, a_u32, 3);
    return vreinterpretq_u8_u32(a_u32);
}

static inline uint8x8_t load_u8_2x2(const uint8_t* buf, ptrdiff_t stride) {
    uint16_t   a;
    uint16x4_t a_u16;

    memcpy(&a, buf, 2);
    buf += stride;
    a_u16 = vdup_n_u16(a);
    memcpy(&a, buf, 2);
    a_u16 = vset_lane_u16(a, a_u16, 1);
    return vreinterpret_u8_u16(a_u16);
}

static inline uint8x8_t load_u8_2x4(const uint8_t* buf, ptrdiff_t stride) {
    uint16_t   a;
    uint16x4_t a_u16;

    memcpy(&a, buf, 2);
    buf += stride;
    a_u16 = vdup_n_u16(a);
    memcpy(&a, buf, 2);
    a_u16 = vset_lane_u16(a, a_u16, 1);
    buf += stride;
    memcpy(&a, buf, 2);
    buf += stride;
    a_u16 = vset_lane_u16(a, a_u16, 2);
    memcpy(&a, buf, 2);
    a_u16 = vset_lane_u16(a, a_u16, 3);
    return vreinterpret_u8_u16(a_u16);
}

static inline uint8x8_t load_dup_u8_4x2(const uint8_t* buf) {
    uint32_t   a;
    uint32x2_t a_u32;

    memcpy(&a, buf, 4);
    a_u32 = vdup_n_u32(a);
    return vreinterpret_u8_u32(a_u32);
}

static inline uint8x8_t load_dup_u8_2x4(const uint8_t* buf) {
    uint16_t   a;
    uint16x4_t a_u32;

    memcpy(&a, buf, 2);
    a_u32 = vdup_n_u16(a);
    return vreinterpret_u8_u16(a_u32);
}

static inline void load_u8_4x2x2(const uint8_t* buf, ptrdiff_t stride, uint8x8_t* tu0, uint8x8_t* tu1) {
    *tu0 = load_u8_4x2(buf, stride);
    buf += 2 * stride;
    *tu1 = load_u8_4x2(buf, stride);
}

static inline void load_u8_16x8(const uint8_t* s, ptrdiff_t p, uint8x16_t* const s0, uint8x16_t* const s1,
                                uint8x16_t* const s2, uint8x16_t* const s3, uint8x16_t* const s4, uint8x16_t* const s5,
                                uint8x16_t* const s6, uint8x16_t* const s7) {
    *s0 = vld1q_u8(s);
    s += p;
    *s1 = vld1q_u8(s);
    s += p;
    *s2 = vld1q_u8(s);
    s += p;
    *s3 = vld1q_u8(s);
    s += p;
    *s4 = vld1q_u8(s);
    s += p;
    *s5 = vld1q_u8(s);
    s += p;
    *s6 = vld1q_u8(s);
    s += p;
    *s7 = vld1q_u8(s);
}

static inline void load_u8_16x3(const uint8_t* s, ptrdiff_t p, uint8x16_t* const s0, uint8x16_t* const s1,
                                uint8x16_t* const s2) {
    *s0 = vld1q_u8(s);
    s += p;
    *s1 = vld1q_u8(s);
    s += p;
    *s2 = vld1q_u8(s);
}

static inline void load_u8_16x4(const uint8_t* s, ptrdiff_t p, uint8x16_t* const s0, uint8x16_t* const s1,
                                uint8x16_t* const s2, uint8x16_t* const s3) {
    *s0 = vld1q_u8(s);
    s += p;
    *s1 = vld1q_u8(s);
    s += p;
    *s2 = vld1q_u8(s);
    s += p;
    *s3 = vld1q_u8(s);
}

static inline void load_u8_16x5(const uint8_t* s, ptrdiff_t p, uint8x16_t* const s0, uint8x16_t* const s1,
                                uint8x16_t* const s2, uint8x16_t* const s3, uint8x16_t* const s4) {
    *s0 = vld1q_u8(s);
    s += p;
    *s1 = vld1q_u8(s);
    s += p;
    *s2 = vld1q_u8(s);
    s += p;
    *s3 = vld1q_u8(s);
    s += p;
    *s4 = vld1q_u8(s);
}

static inline void load_u16_8x8(const uint16_t* s, const ptrdiff_t p, uint16x8_t* s0, uint16x8_t* s1, uint16x8_t* s2,
                                uint16x8_t* s3, uint16x8_t* s4, uint16x8_t* s5, uint16x8_t* s6, uint16x8_t* s7) {
    *s0 = vld1q_u16(s);
    s += p;
    *s1 = vld1q_u16(s);
    s += p;
    *s2 = vld1q_u16(s);
    s += p;
    *s3 = vld1q_u16(s);
    s += p;
    *s4 = vld1q_u16(s);
    s += p;
    *s5 = vld1q_u16(s);
    s += p;
    *s6 = vld1q_u16(s);
    s += p;
    *s7 = vld1q_u16(s);
}

static inline void load_u16_16x4(const uint16_t* s, ptrdiff_t p, uint16x8_t* const s0, uint16x8_t* const s1,
                                 uint16x8_t* const s2, uint16x8_t* const s3, uint16x8_t* const s4, uint16x8_t* const s5,
                                 uint16x8_t* const s6, uint16x8_t* const s7) {
    *s0 = vld1q_u16(s);
    *s1 = vld1q_u16(s + 8);
    s += p;
    *s2 = vld1q_u16(s);
    *s3 = vld1q_u16(s + 8);
    s += p;
    *s4 = vld1q_u16(s);
    *s5 = vld1q_u16(s + 8);
    s += p;
    *s6 = vld1q_u16(s);
    *s7 = vld1q_u16(s + 8);
}

static inline uint16x4_t load_u16_2x2(const uint16_t* buf, ptrdiff_t stride) {
    uint32_t   a;
    uint32x2_t a_u32;

    memcpy(&a, buf, 4);
    buf += stride;
    a_u32 = vdup_n_u32(a);
    memcpy(&a, buf, 4);
    a_u32 = vset_lane_u32(a, a_u32, 1);
    return vreinterpret_u16_u32(a_u32);
}

static inline uint16x8_t load_u16_4x2(const uint16_t* buf, ptrdiff_t stride) {
    uint64_t   a;
    uint64x2_t a_u64;

    memcpy(&a, buf, 8);
    buf += stride;
    a_u64 = vdupq_n_u64(0);
    a_u64 = vsetq_lane_u64(a, a_u64, 0);
    memcpy(&a, buf, 8);
    a_u64 = vsetq_lane_u64(a, a_u64, 1);
    return vreinterpretq_u16_u64(a_u64);
}

static inline int16x8_t load_s16_4x2(const int16_t* buf, ptrdiff_t stride) {
    int64_t   a;
    int64x2_t a_s64;

    memcpy(&a, buf, 8);
    buf += stride;
    a_s64 = vdupq_n_s64(0);
    a_s64 = vsetq_lane_s64(a, a_s64, 0);
    memcpy(&a, buf, 8);
    a_s64 = vsetq_lane_s64(a, a_s64, 1);
    return vreinterpretq_s16_s64(a_s64);
}

static inline int32x4_t load_s32_2x2(int32_t* s, ptrdiff_t stride) {
    return vcombine_s32(vld1_s32(s), vld1_s32(s + stride));
}

static inline void load_s32_4x2(int32_t* s, ptrdiff_t p, int32x4_t* s1, int32x4_t* s2) {
    *s1 = vld1q_s32(s);
    s += p;
    *s2 = vld1q_s32(s);
}

static inline void load_s32_4x4(int32_t* s, ptrdiff_t p, int32x4_t* s1, int32x4_t* s2, int32x4_t* s3, int32x4_t* s4) {
    *s1 = vld1q_s32(s);
    s += p;
    *s2 = vld1q_s32(s);
    s += p;
    *s3 = vld1q_s32(s);
    s += p;
    *s4 = vld1q_s32(s);
}

static inline void load_s32_4x8(int32_t* s, ptrdiff_t p, int32x4_t* s0, int32x4_t* s1, int32x4_t* s2, int32x4_t* s3,
                                int32x4_t* s4, int32x4_t* s5, int32x4_t* s6, int32x4_t* s7) {
    *s0 = vld1q_s32(s);
    s += p;
    *s1 = vld1q_s32(s);
    s += p;
    *s2 = vld1q_s32(s);
    s += p;
    *s3 = vld1q_s32(s);
    s += p;
    *s4 = vld1q_s32(s);
    s += p;
    *s5 = vld1q_s32(s);
    s += p;
    *s6 = vld1q_s32(s);
    s += p;
    *s7 = vld1q_s32(s);
}

static inline void store_s32_4x4(int32_t* s, ptrdiff_t p, int32x4_t s1, int32x4_t s2, int32x4_t s3, int32x4_t s4) {
    vst1q_s32(s, s1);
    s += p;
    vst1q_s32(s, s2);
    s += p;
    vst1q_s32(s, s3);
    s += p;
    vst1q_s32(s, s4);
}

static inline void load_u32_4x4(uint32_t* s, ptrdiff_t p, uint32x4_t* s1, uint32x4_t* s2, uint32x4_t* s3,
                                uint32x4_t* s4) {
    *s1 = vld1q_u32(s);
    s += p;
    *s2 = vld1q_u32(s);
    s += p;
    *s3 = vld1q_u32(s);
    s += p;
    *s4 = vld1q_u32(s);
}

static inline void store_u32_4x2(uint32_t* s, ptrdiff_t p, uint32x4_t s1, uint32x4_t s2) {
    vst1q_u32(s, s1);
    s += p;
    vst1q_u32(s, s2);
}

static inline void store_u32_4x4(uint32_t* s, ptrdiff_t p, uint32x4_t s1, uint32x4_t s2, uint32x4_t s3, uint32x4_t s4) {
    vst1q_u32(s, s1);
    s += p;
    vst1q_u32(s, s2);
    s += p;
    vst1q_u32(s, s3);
    s += p;
    vst1q_u32(s, s4);
}

static inline void store_s32_8x4(int32_t* s, ptrdiff_t p, int32x4_t s0, int32x4_t s1, int32x4_t s2, int32x4_t s3,
                                 int32x4_t s4, int32x4_t s5, int32x4_t s6, int32x4_t s7) {
    vst1q_s32(s, s0);
    s += p;
    vst1q_s32(s, s1);
    s += p;
    vst1q_s32(s, s2);
    s += p;
    vst1q_s32(s, s3);
    s += p;
    vst1q_s32(s, s4);
    s += p;
    vst1q_s32(s, s5);
    s += p;
    vst1q_s32(s, s6);
    s += p;
    vst1q_s32(s, s7);
}

static inline int16x8_t load_tran_low_to_s16q(const tran_low_t* buf) {
    const int32x4_t v0 = vld1q_s32(buf);
    const int32x4_t v1 = vld1q_s32(buf + 4);
    const int16x4_t s0 = vmovn_s32(v0);
    const int16x4_t s1 = vmovn_s32(v1);
    return vcombine_s16(s0, s1);
}

static inline void store_s16q_to_tran_low(tran_low_t* buf, const int16x8_t a) {
    const int32x4_t v0 = vmovl_s16(vget_low_s16(a));
    const int32x4_t v1 = vmovl_s16(vget_high_s16(a));
    vst1q_s32(buf, v0);
    vst1q_s32(buf + 4, v1);
}

static inline void store_s16_to_tran_low(tran_low_t* buf, const int16x4_t a) {
    const int32x4_t v0 = vmovl_s16(a);
    vst1q_s32(buf, v0);
}

// The `lane` parameter here must be an immediate.
#define store_u8_2x1_lane(dst, src, lane)                           \
    do {                                                            \
        uint16_t a = vget_lane_u16(vreinterpret_u16_u8(src), lane); \
        memcpy(dst, &a, 2);                                         \
    } while (0)

#define store_u8_4x1_lane(dst, src, lane)                           \
    do {                                                            \
        uint32_t a = vget_lane_u32(vreinterpret_u32_u8(src), lane); \
        memcpy(dst, &a, 4);                                         \
    } while (0)

// Store the low 16-bits from a single vector.
static inline void store_u8_2x1(uint8_t* dst, const uint8x8_t src) {
    store_u8_2x1_lane(dst, src, 0);
}

// Store the low 32-bits from a single vector.
static inline void store_u8_4x1(uint8_t* dst, const uint8x8_t src) {
    store_u8_4x1_lane(dst, src, 0);
}

// Store two blocks of 32-bits from a single vector.
static inline void store_u8x4_strided_x2(uint8_t* dst, ptrdiff_t stride, uint8x8_t src) {
    store_u8_4x1_lane(dst, src, 0);
    dst += stride;
    store_u8_4x1_lane(dst, src, 1);
}

// Store two blocks of 16-bits from a single vector.
static inline void store_u8x2_strided_x2(uint8_t* dst, ptrdiff_t dst_stride, uint8x8_t src) {
    store_u8_2x1_lane(dst, src, 0);
    dst += dst_stride;
    store_u8_2x1_lane(dst, src, 1);
}

#undef store_u8_4x1_lane
#undef store_u8_2x1_lane

// Store two blocks of 64-bits from a single vector.
static inline void store_s16x4_strided_x2(int16_t* dst, ptrdiff_t dst_stride, int16x8_t src) {
    store_s16_4x1_lane(dst, src, 0);
    dst += dst_stride;
    store_s16_4x1_lane(dst, src, 1);
}

// Store two blocks of 32-bits from a single vector.
static inline void store_u16x2_strided_x2(uint16_t* dst, ptrdiff_t dst_stride, uint16x4_t src) {
    store_u16_2x1_lane(dst, src, 0);
    dst += dst_stride;
    store_u16_2x1_lane(dst, src, 1);
}

// Store two blocks of 32-bits from a single vector.
static inline void store_s16x2_strided_x2(int16_t* dst, ptrdiff_t dst_stride, int16x4_t src) {
    store_s16_2x1_lane(dst, src, 0);
    dst += dst_stride;
    store_s16_2x1_lane(dst, src, 1);
}

// Store two blocks of 64-bits from a single vector.
static inline void store_u16x4_strided_x2(uint16_t* dst, ptrdiff_t dst_stride, uint16x8_t src) {
    store_u16_4x1_lane(dst, src, 0);
    dst += dst_stride;
    store_u16_4x1_lane(dst, src, 1);
}

#undef store_u16_2x1_lane
#undef store_s16_2x1_lane
#undef store_u16_4x1_lane
#undef store_s16_4x1_lane

#endif // AOM_AOM_DSP_ARM_MEM_NEON_H_
