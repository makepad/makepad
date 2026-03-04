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

#ifndef BLEND_A64_MASK_NEON_H
#define BLEND_A64_MASK_NEON_H

#include <arm_neon.h>
#include <assert.h>

#include "common_dsp_rtcd.h"
#include "definitions.h"
#include "mem_neon.h"

static inline uint8x8_t avg_blend_pairwise_u8x8_4(uint8x8_t a, uint8x8_t b, uint8x8_t c, uint8x8_t d) {
    uint8x8_t a_c = vpadd_u8(a, c);
    uint8x8_t b_d = vpadd_u8(b, d);
    return vrshr_n_u8(vqadd_u8(a_c, b_d), 2);
}

static inline uint16x8_t avg_blend_pairwise_long_u8x8_4(uint8x8_t a, uint8x8_t b, uint8x8_t c, uint8x8_t d) {
    uint8x8_t a_c = vpadd_u8(a, c);
    uint8x8_t b_d = vpadd_u8(b, d);
    return vrshrq_n_u16(vaddl_u8(a_c, b_d), 2);
}

static inline uint8x16_t avg_blend_pairwise_u8x16_4(uint8x16_t a, uint8x16_t b, uint8x16_t c, uint8x16_t d) {
    uint8x16_t a_c = vpaddq_u8(a, c);
    uint8x16_t b_d = vpaddq_u8(b, d);
    return vrshrq_n_u8(vqaddq_u8(a_c, b_d), 2);
}

#endif // BLEND_A64_MASK_NEON_H
