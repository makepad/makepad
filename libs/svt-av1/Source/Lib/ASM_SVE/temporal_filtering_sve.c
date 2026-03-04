/*
 * Copyright (c) 2025, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <assert.h>
#include <arm_neon.h>
#include <arm_sve.h>
#include "neon_sve_bridge.h"

#include "definitions.h"
#include "mem_neon.h"
#include "temporal_filtering.h"
#include "utility.h"

static void process_block_hbd_sve(int h, int w, uint16_t* buff_hbd_start, uint32_t* accum, uint16_t* count,
                                  uint32_t stride) {
    do {
        int width = w;
        do {
            // buff_lbd_start[pos] = (uint8_t)OD_DIVU(accum[k] + (count[k] >> 1), count[k]);
            // buff_lbd_start[pos] = (uint8_t)((accum[k] + (count[k] >> 1))/ count[k]);
            uint32x4_t accum0     = vld1q_u32(accum);
            uint32x4_t accum1     = vld1q_u32(accum + 4);
            uint16x8_t count_u16  = vld1q_u16(count);
            uint32x4_t count0_u32 = vmovl_u16(vget_low_u16(count_u16));
            uint32x4_t count1_u32 = vmovl_u16(vget_high_u16(count_u16));

            // accum[k] + (count[k] >> 1)
            accum0 = vsraq_n_u32(accum0, count0_u32, 1);
            accum1 = vsraq_n_u32(accum1, count1_u32, 1);

            uint32x4_t d0  = svt_div_u32(accum0, count0_u32);
            uint32x4_t d1  = svt_div_u32(accum1, count1_u32);
            uint16x8_t d01 = vcombine_u16(vmovn_u32(d0), vmovn_u32(d1));

            vst1q_u16(buff_hbd_start, d01);

            accum += 8;
            count += 8;
            buff_hbd_start += 8;
            width -= 8;
        } while (width != 0);
        buff_hbd_start += stride;
    } while (--h != 0);
}

static void process_block_lbd_sve(int h, int w, uint8_t* buff_lbd_start, uint32_t* accum, uint16_t* count,
                                  uint32_t stride) {
    do {
        int width = w;
        do {
            // buff_lbd_start[pos] = (uint8_t)OD_DIVU(accum[k] + (count[k] >> 1), count[k]);
            // buff_lbd_start[pos] = (uint8_t)((accum[k] + (count[k] >> 1))/ count[k]);
            uint32x4_t accum0     = vld1q_u32(accum);
            uint32x4_t accum1     = vld1q_u32(accum + 4);
            uint16x8_t count_u16  = vld1q_u16(count);
            uint32x4_t count0_u32 = vmovl_u16(vget_low_u16(count_u16));
            uint32x4_t count1_u32 = vmovl_u16(vget_high_u16(count_u16));

            // accum[k] + (count[k] >> 1)
            accum0 = vsraq_n_u32(accum0, count0_u32, 1);
            accum1 = vsraq_n_u32(accum1, count1_u32, 1);

            uint32x4_t d0  = svt_div_u32(accum0, count0_u32);
            uint32x4_t d1  = svt_div_u32(accum1, count1_u32);
            uint16x8_t d01 = vcombine_u16(vmovn_u32(d0), vmovn_u32(d1));

            svuint16_t d01_sve = svset_neonq_u16(svundef_u16(), d01);
            svst1b(svptrue_pat_b8(SV_VL16), buff_lbd_start, d01_sve);

            accum += 8;
            count += 8;
            buff_lbd_start += 8;
            width -= 8;
        } while (width != 0);
        buff_lbd_start += stride;
    } while (--h != 0);
}

void svt_aom_get_final_filtered_pixels_sve(MeContext* me_ctx, EbByte* src_center_ptr_start,
                                           uint16_t** altref_buffer_highbd_start, uint32_t** accum, uint16_t** count,
                                           const uint32_t* stride, int blk_y_src_offset, int blk_ch_src_offset,
                                           uint16_t blk_width_ch, uint16_t blk_height_ch, bool is_highbd) {
    assert(blk_width_ch % 16 == 0);
    assert(BW % 16 == 0);

    if (!is_highbd) {
        //Process luma
        process_block_lbd_sve(
            BH, BW, &src_center_ptr_start[C_Y][blk_y_src_offset], accum[C_Y], count[C_Y], stride[C_Y] - BW);
        // Process chroma
        if (me_ctx->tf_chroma) {
            process_block_lbd_sve(blk_height_ch,
                                  blk_width_ch,
                                  &src_center_ptr_start[C_U][blk_ch_src_offset],
                                  accum[C_U],
                                  count[C_U],
                                  stride[C_U] - blk_width_ch);
            process_block_lbd_sve(blk_height_ch,
                                  blk_width_ch,
                                  &src_center_ptr_start[C_V][blk_ch_src_offset],
                                  accum[C_V],
                                  count[C_V],
                                  stride[C_V] - blk_width_ch);
        }
    } else {
        // Process luma
        process_block_hbd_sve(
            BH, BW, &altref_buffer_highbd_start[C_Y][blk_y_src_offset], accum[C_Y], count[C_Y], stride[C_Y] - BW);
        // Process chroma
        if (me_ctx->tf_chroma) {
            process_block_hbd_sve(blk_height_ch,
                                  blk_width_ch,
                                  &altref_buffer_highbd_start[C_U][blk_ch_src_offset],
                                  accum[C_U],
                                  count[C_U],
                                  stride[C_U] - blk_width_ch);
            process_block_hbd_sve(blk_height_ch,
                                  blk_width_ch,
                                  &altref_buffer_highbd_start[C_V][blk_ch_src_offset],
                                  accum[C_V],
                                  count[C_V],
                                  stride[C_V] - blk_width_ch);
        }
    }
}
