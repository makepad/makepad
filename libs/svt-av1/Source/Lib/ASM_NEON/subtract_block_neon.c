/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved.
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

void svt_aom_subtract_block_neon(int rows, int cols, int16_t* diff, ptrdiff_t diff_stride, const uint8_t* src,
                                 ptrdiff_t src_stride, const uint8_t* pred, ptrdiff_t pred_stride) {
    int r, c;
    if (cols > 16) {
        r = rows;
        do {
            c = 0;
            do {
                const uint8x16_t v_src_00     = vld1q_u8(&src[c + 0]);
                const uint8x16_t v_src_16     = vld1q_u8(&src[c + 16]);
                const uint8x16_t v_pred_00    = vld1q_u8(&pred[c + 0]);
                const uint8x16_t v_pred_16    = vld1q_u8(&pred[c + 16]);
                const uint16x8_t v_diff_lo_00 = vsubl_u8(vget_low_u8(v_src_00), vget_low_u8(v_pred_00));
                const uint16x8_t v_diff_hi_00 = vsubl_u8(vget_high_u8(v_src_00), vget_high_u8(v_pred_00));
                const uint16x8_t v_diff_lo_16 = vsubl_u8(vget_low_u8(v_src_16), vget_low_u8(v_pred_16));
                const uint16x8_t v_diff_hi_16 = vsubl_u8(vget_high_u8(v_src_16), vget_high_u8(v_pred_16));
                vst1q_s16(&diff[c + 0], vreinterpretq_s16_u16(v_diff_lo_00));
                vst1q_s16(&diff[c + 8], vreinterpretq_s16_u16(v_diff_hi_00));
                vst1q_s16(&diff[c + 16], vreinterpretq_s16_u16(v_diff_lo_16));
                vst1q_s16(&diff[c + 24], vreinterpretq_s16_u16(v_diff_hi_16));
                c += 32;
            } while (c < cols);
            diff += diff_stride;
            pred += pred_stride;
            src += src_stride;
        } while (--r != 0);
    } else if (cols > 8) {
        r = rows;
        do {
            const uint8x16_t v_src     = vld1q_u8(&src[0]);
            const uint8x16_t v_pred    = vld1q_u8(&pred[0]);
            const uint16x8_t v_diff_lo = vsubl_u8(vget_low_u8(v_src), vget_low_u8(v_pred));
            const uint16x8_t v_diff_hi = vsubl_u8(vget_high_u8(v_src), vget_high_u8(v_pred));
            vst1q_s16(&diff[0], vreinterpretq_s16_u16(v_diff_lo));
            vst1q_s16(&diff[8], vreinterpretq_s16_u16(v_diff_hi));
            diff += diff_stride;
            pred += pred_stride;
            src += src_stride;
        } while (--r != 0);
    } else if (cols > 4) {
        r = rows;
        do {
            const uint8x8_t  v_src  = vld1_u8(&src[0]);
            const uint8x8_t  v_pred = vld1_u8(&pred[0]);
            const uint16x8_t v_diff = vsubl_u8(v_src, v_pred);
            vst1q_s16(&diff[0], vreinterpretq_s16_u16(v_diff));
            diff += diff_stride;
            pred += pred_stride;
            src += src_stride;
        } while (--r != 0);
    } else {
        r = rows;
        do {
            c = 0;
            do {
                diff[c] = src[c] - pred[c];
            } while (++c < cols);
            diff += diff_stride;
            pred += pred_stride;
            src += src_stride;
        } while (--r != 0);
    }
}
