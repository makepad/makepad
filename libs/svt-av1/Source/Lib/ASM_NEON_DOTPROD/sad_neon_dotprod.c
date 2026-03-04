/*
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

#include "aom_dsp_rtcd.h"
#include "mem_neon.h"
#include "sad_neon_dotprod.h"
#include "sum_neon.h"

void svt_ext_sad_calculation_8x8_16x16_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                    uint32_t ref_stride, uint32_t* p_best_sad_8x8,
                                                    uint32_t* p_best_sad_16x16, uint32_t* p_best_mv8x8,
                                                    uint32_t* p_best_mv16x16, uint32_t mv, uint32_t* p_sad16x16,
                                                    uint32_t* p_sad8x8, bool sub_sad) {
    uint32_t   sad16x16;
    uint32x4_t sad;
    uint32x4_t best_sad_vec = vld1q_u32(p_best_sad_8x8);
    uint32x4_t best_mv_vec  = vld1q_u32(p_best_mv8x8);
    uint32x4_t mv_vec       = vdupq_n_u32(mv);

    if (sub_sad) {
        uint32x4_t sad01 = sad_8x4_dual_neon(
            src + 0 * src_stride, 2 * src_stride, ref + 0 * ref_stride, 2 * ref_stride, 2);
        uint32x4_t sad23 = sad_8x4_dual_neon(
            src + 8 * src_stride, 2 * src_stride, ref + 8 * ref_stride, 2 * ref_stride, 2);

        sad = vpaddq_u32(sad01, sad23);
    } else {
        uint32x4_t sad01 = sad_8x8_dual_neon(src + 0 * src_stride, src_stride, ref + 0 * ref_stride, ref_stride, 1);
        uint32x4_t sad23 = sad_8x8_dual_neon(src + 8 * src_stride, src_stride, ref + 8 * ref_stride, ref_stride, 1);

        sad = vpaddq_u32(sad01, sad23);
    }

    uint32x4_t cmp = vcltq_u32(sad, best_sad_vec);
    best_sad_vec   = vbslq_u32(cmp, sad, best_sad_vec);
    best_mv_vec    = vbslq_u32(cmp, mv_vec, best_mv_vec);

    sad16x16 = vaddvq_u32(sad);
    if (sad16x16 < p_best_sad_16x16[0]) {
        p_best_sad_16x16[0] = sad16x16;
        p_best_mv16x16[0]   = mv;
    }

    *p_sad16x16 = sad16x16;
    vst1q_u32(p_sad8x8, sad);
    vst1q_u32(p_best_sad_8x8, best_sad_vec);
    vst1q_u32(p_best_mv8x8, best_mv_vec);
}

static inline void svt_ext_eight_sad_calculation_8x8_16x16_sub_sad_1_neon_dotprod(
    uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride, uint32_t mv, uint32_t start_16x16_pos,
    uint32_t* p_best_sad_8x8, uint32_t* p_best_sad_16x16, uint32_t* p_best_mv8x8, uint32_t* p_best_mv16x16,
    uint32_t p_eight_sad16x16[16][8]) {
    const uint32_t start_8x8_pos = 4 * start_16x16_pos;

    p_best_sad_8x8 += start_8x8_pos;
    p_best_mv8x8 += start_8x8_pos;
    p_best_sad_16x16 += start_16x16_pos;
    p_best_mv16x16 += start_16x16_pos;
    uint32_t   x_mv         = _MVXT(mv);
    uint32_t   y_mv         = _MVYT(mv);
    uint32x4_t best_sad_vec = vld1q_u32(p_best_sad_8x8);
    uint32x4_t best_mv_vec  = vld1q_u32(p_best_mv8x8);

    uint32_t src_stride_sub = 2 * src_stride;
    uint32_t ref_stride_sub = 2 * ref_stride;
    for (int search_index = 0; search_index < 8; search_index++) {
        uint32_t   tmp_mv = (y_mv << 16) | ((x_mv + search_index) & 0xFFFF);
        uint32x4_t mv_vec = vdupq_n_u32(tmp_mv);

        uint32x4_t sad01 = sad_8x4_dual_neon(src, src_stride_sub, ref + search_index, ref_stride_sub, 2);

        uint32x4_t sad23 = sad_8x4_dual_neon(
            src + 8 * src_stride, src_stride_sub, ref + 8 * ref_stride + search_index, ref_stride_sub, 2);

        uint32x4_t sad = vpaddq_u32(sad01, sad23);

        uint32x4_t cmp = vcltq_u32(sad, best_sad_vec);
        best_sad_vec   = vbslq_u32(cmp, sad, best_sad_vec);
        best_mv_vec    = vbslq_u32(cmp, mv_vec, best_mv_vec);

        uint32_t sad16x16 = p_eight_sad16x16[start_16x16_pos][search_index] = vaddvq_u32(sad);

        if (sad16x16 < p_best_sad_16x16[0]) {
            p_best_sad_16x16[0] = sad16x16;
            p_best_mv16x16[0]   = tmp_mv;
        }
    }
    vst1q_u32(p_best_sad_8x8, best_sad_vec);
    vst1q_u32(p_best_mv8x8, best_mv_vec);
}

static inline void svt_ext_eight_sad_calculation_8x8_16x16_sub_sad_0_neon_dotprod(
    uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride, uint32_t mv, uint32_t start_16x16_pos,
    uint32_t* p_best_sad_8x8, uint32_t* p_best_sad_16x16, uint32_t* p_best_mv8x8, uint32_t* p_best_mv16x16,
    uint32_t p_eight_sad16x16[16][8]) {
    const uint32_t start_8x8_pos = 4 * start_16x16_pos;

    p_best_sad_8x8 += start_8x8_pos;
    p_best_mv8x8 += start_8x8_pos;
    p_best_sad_16x16 += start_16x16_pos;
    p_best_mv16x16 += start_16x16_pos;
    uint32_t   x_mv         = _MVXT(mv);
    uint32_t   y_mv         = _MVYT(mv);
    uint32x4_t best_sad_vec = vld1q_u32(p_best_sad_8x8);
    uint32x4_t best_mv_vec  = vld1q_u32(p_best_mv8x8);

    for (int search_index = 0; search_index < 8; search_index++) {
        uint32_t   tmp_mv = (y_mv << 16) | ((x_mv + search_index) & 0xFFFF);
        uint32x4_t mv_vec = vdupq_n_u32(tmp_mv);
        uint32x4_t sad01  = sad_8x8_dual_neon(src, src_stride, ref + search_index, ref_stride, 1);
        uint32x4_t sad23  = sad_8x8_dual_neon(
            src + 8 * src_stride, src_stride, ref + 8 * ref_stride + search_index, ref_stride, 1);

        uint32x4_t sad = vpaddq_u32(sad01, sad23);
        uint32x4_t cmp = vcltq_u32(sad, best_sad_vec);
        best_sad_vec   = vbslq_u32(cmp, sad, best_sad_vec);
        best_mv_vec    = vbslq_u32(cmp, mv_vec, best_mv_vec);

        uint32_t sad16x16 = p_eight_sad16x16[start_16x16_pos][search_index] = vaddvq_u32(sad);
        if (sad16x16 < p_best_sad_16x16[0]) {
            p_best_sad_16x16[0] = sad16x16;
            p_best_mv16x16[0]   = tmp_mv;
        }
    }
    vst1q_u32(p_best_sad_8x8, best_sad_vec);
    vst1q_u32(p_best_mv8x8, best_mv_vec);
}

void svt_ext_all_sad_calculation_8x8_16x16_neon_dotprod(uint8_t* src, uint32_t src_stride, uint8_t* ref,
                                                        uint32_t ref_stride, uint32_t mv, uint32_t* p_best_sad_8x8,
                                                        uint32_t* p_best_sad_16x16, uint32_t* p_best_mv8x8,
                                                        uint32_t* p_best_mv16x16, uint32_t p_eight_sad16x16[16][8],
                                                        uint32_t p_eight_sad8x8[64][8], bool sub_sad) {
    (void)p_eight_sad8x8;
    static const char offsets[16] = {0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15};

    //---- 16x16 : 0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15
    if (sub_sad) {
        for (int y = 0; y < 4; y++) {
            for (int x = 0; x < 4; x++) {
                const uint32_t block_index           = 16 * y * src_stride + 16 * x;
                const uint32_t search_position_index = 16 * y * ref_stride + 16 * x;
                svt_ext_eight_sad_calculation_8x8_16x16_sub_sad_1_neon_dotprod(src + block_index,
                                                                               src_stride,
                                                                               ref + search_position_index,
                                                                               ref_stride,
                                                                               mv,
                                                                               offsets[4 * y + x],
                                                                               p_best_sad_8x8,
                                                                               p_best_sad_16x16,
                                                                               p_best_mv8x8,
                                                                               p_best_mv16x16,
                                                                               p_eight_sad16x16);
            }
        }
    } else {
        for (int y = 0; y < 4; y++) {
            for (int x = 0; x < 4; x++) {
                const uint32_t block_index           = 16 * y * src_stride + 16 * x;
                const uint32_t search_position_index = 16 * y * ref_stride + 16 * x;
                svt_ext_eight_sad_calculation_8x8_16x16_sub_sad_0_neon_dotprod(src + block_index,
                                                                               src_stride,
                                                                               ref + search_position_index,
                                                                               ref_stride,
                                                                               mv,
                                                                               offsets[4 * y + x],
                                                                               p_best_sad_8x8,
                                                                               p_best_sad_16x16,
                                                                               p_best_mv8x8,
                                                                               p_best_mv16x16,
                                                                               p_eight_sad16x16);
            }
        }
    }
}
