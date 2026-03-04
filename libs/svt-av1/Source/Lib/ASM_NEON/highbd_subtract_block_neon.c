/*
 * Copyright (c) 2022, Alliance for Open Media. All rights reserved.
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at www.aomedia.org/license/software. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at www.aomedia.org/license/patent.
 */

#include <arm_neon.h>
#include <stddef.h>

#include "aom_dsp_rtcd.h"

void svt_aom_highbd_subtract_block_neon(int rows, int cols, int16_t* diff, ptrdiff_t diff_stride, const uint8_t* src8,
                                        ptrdiff_t src_stride, const uint8_t* pred8, ptrdiff_t pred_stride, int bd) {
    (void)bd;
    uint16_t* src  = (uint16_t*)src8;
    uint16_t* pred = (uint16_t*)pred8;

    if (cols > 16) {
        do {
            int c = 0;
            do {
                const uint16x8_t v_src_00  = vld1q_u16(&src[c + 0]);
                const uint16x8_t v_pred_00 = vld1q_u16(&pred[c + 0]);
                const uint16x8_t v_diff_00 = vsubq_u16(v_src_00, v_pred_00);

                const uint16x8_t v_src_08  = vld1q_u16(&src[c + 8]);
                const uint16x8_t v_pred_08 = vld1q_u16(&pred[c + 8]);
                const uint16x8_t v_diff_08 = vsubq_u16(v_src_08, v_pred_08);

                vst1q_s16(&diff[c + 0], vreinterpretq_s16_u16(v_diff_00));
                vst1q_s16(&diff[c + 8], vreinterpretq_s16_u16(v_diff_08));
                c += 16;
            } while (c < cols);

            diff += diff_stride;
            pred += pred_stride;
            src += src_stride;
        } while (--rows != 0);
    } else if (cols > 8) {
        do {
            const uint16x8_t v_src_00  = vld1q_u16(&src[0]);
            const uint16x8_t v_pred_00 = vld1q_u16(&pred[0]);
            const uint16x8_t v_diff_00 = vsubq_u16(v_src_00, v_pred_00);

            const uint16x8_t v_src_08  = vld1q_u16(&src[8]);
            const uint16x8_t v_pred_08 = vld1q_u16(&pred[8]);
            const uint16x8_t v_diff_08 = vsubq_u16(v_src_08, v_pred_08);

            vst1q_s16(&diff[0], vreinterpretq_s16_u16(v_diff_00));
            vst1q_s16(&diff[8], vreinterpretq_s16_u16(v_diff_08));

            diff += diff_stride;
            pred += pred_stride;
            src += src_stride;
        } while (--rows != 0);
    } else if (cols > 4) {
        do {
            const uint16x8_t v_src_r0  = vld1q_u16(&src[0]);
            const uint16x8_t v_src_r1  = vld1q_u16(&src[src_stride]);
            const uint16x8_t v_pred_r0 = vld1q_u16(&pred[0]);
            const uint16x8_t v_pred_r1 = vld1q_u16(&pred[pred_stride]);

            const uint16x8_t v_diff_r0 = vsubq_u16(v_src_r0, v_pred_r0);
            const uint16x8_t v_diff_r1 = vsubq_u16(v_src_r1, v_pred_r1);

            vst1q_s16(&diff[0], vreinterpretq_s16_u16(v_diff_r0));
            vst1q_s16(&diff[diff_stride], vreinterpretq_s16_u16(v_diff_r1));

            diff += diff_stride << 1;
            pred += pred_stride << 1;
            src += src_stride << 1;
            rows -= 2;
        } while (rows != 0);
    } else {
        do {
            const uint16x4_t v_src_r0  = vld1_u16(&src[0]);
            const uint16x4_t v_src_r1  = vld1_u16(&src[src_stride]);
            const uint16x4_t v_pred_r0 = vld1_u16(&pred[0]);
            const uint16x4_t v_pred_r1 = vld1_u16(&pred[pred_stride]);

            const uint16x4_t v_diff_r0 = vsub_u16(v_src_r0, v_pred_r0);
            const uint16x4_t v_diff_r1 = vsub_u16(v_src_r1, v_pred_r1);

            vst1_s16(&diff[0], vreinterpret_s16_u16(v_diff_r0));
            vst1_s16(&diff[diff_stride], vreinterpret_s16_u16(v_diff_r1));

            diff += diff_stride << 1;
            pred += pred_stride << 1;
            src += src_stride << 1;
            rows -= 2;
        } while (rows != 0);
    }
}
