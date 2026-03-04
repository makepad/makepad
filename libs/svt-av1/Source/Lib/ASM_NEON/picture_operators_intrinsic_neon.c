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
#include "definitions.h"
#include "mem_neon.h"
#include "transpose_neon.h"
#include "pack_unpack_c.h"

static inline void residual_kernel4xh_neon(const uint8_t* restrict input, const uint32_t input_stride,
                                           const uint8_t* restrict pred, const uint32_t pred_stride, int16_t* residual,
                                           const uint32_t residual_stride, uint32_t area_height) {
    do {
        uint8x8_t s = load_u8_4x2(input, input_stride);
        uint8x8_t p = load_u8_4x2(pred, pred_stride);

        int16x8_t diff = vreinterpretq_s16_u16(vsubl_u8(s, p));

        vst1_s16(residual + 0 * residual_stride, vget_low_s16(diff));
        vst1_s16(residual + 1 * residual_stride, vget_high_s16(diff));

        input += 2 * input_stride;
        pred += 2 * pred_stride;
        residual += 2 * residual_stride;
        area_height -= 2;
    } while (area_height != 0);
}

static inline void residual_kernel8xh_neon(const uint8_t* restrict input, const uint32_t input_stride,
                                           const uint8_t* restrict pred, const uint32_t pred_stride, int16_t* residual,
                                           const uint32_t residual_stride, uint32_t area_height) {
    do {
        const uint8x8_t in = vld1_u8(input);
        const uint8x8_t pr = vld1_u8(pred);
        const int16x8_t re = vreinterpretq_s16_u16(vsubl_u8(in, pr));
        vst1q_s16(residual, re);

        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
    } while (--area_height != 0);
}

static inline void residual_kernel16xh_neon(const uint8_t* restrict input, const uint32_t input_stride,
                                            const uint8_t* restrict pred, const uint32_t pred_stride, int16_t* residual,
                                            const uint32_t residual_stride, uint32_t area_height) {
    do {
        const uint8x16_t in = vld1q_u8(input);
        const uint8x16_t pr = vld1q_u8(pred);

        const int16x8_t diff0 = vreinterpretq_s16_u16(vsubl_u8(vget_low_u8(in), vget_low_u8(pr)));
        const int16x8_t diff1 = vreinterpretq_s16_u16(vsubl_u8(vget_high_u8(in), vget_high_u8(pr)));

        store_s16_8x2(residual, 8, diff0, diff1);

        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
    } while (--area_height != 0);
}

static inline void residual_kernel32x1_neon(const uint8_t* restrict input, const uint8_t* restrict pred,
                                            int16_t* residual) {
    const uint8x16_t in0 = vld1q_u8(input);
    const uint8x16_t in1 = vld1q_u8(input + 16);
    const uint8x16_t pr0 = vld1q_u8(pred);
    const uint8x16_t pr1 = vld1q_u8(pred + 16);

    const int16x8_t diff0_lo = vreinterpretq_s16_u16(vsubl_u8(vget_low_u8(in0), vget_low_u8(pr0)));
    const int16x8_t diff0_hi = vreinterpretq_s16_u16(vsubl_u8(vget_high_u8(in0), vget_high_u8(pr0)));
    const int16x8_t diff1_lo = vreinterpretq_s16_u16(vsubl_u8(vget_low_u8(in1), vget_low_u8(pr1)));
    const int16x8_t diff1_hi = vreinterpretq_s16_u16(vsubl_u8(vget_high_u8(in1), vget_high_u8(pr1)));

    store_s16_8x4(residual, 8, diff0_lo, diff0_hi, diff1_lo, diff1_hi);
}

static inline void residual_kernel32xh_neon(const uint8_t* restrict input, const uint32_t input_stride,
                                            const uint8_t* restrict pred, const uint32_t pred_stride, int16_t* residual,
                                            const uint32_t residual_stride, uint32_t area_height) {
    do {
        residual_kernel32x1_neon(input, pred, residual);

        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
    } while (--area_height != 0);
}

static inline void residual_kernel64xh_neon(const uint8_t* restrict input, const uint32_t input_stride,
                                            const uint8_t* restrict pred, const uint32_t pred_stride, int16_t* residual,
                                            const uint32_t residual_stride, uint32_t area_height) {
    do {
        residual_kernel32x1_neon(input, pred, residual);
        residual_kernel32x1_neon(input + 32, pred + 32, residual + 32);

        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
    } while (--area_height != 0);
}

static inline void residual_kernel128xh_neon(const uint8_t* restrict input, const uint32_t input_stride,
                                             const uint8_t* restrict pred, const uint32_t  pred_stride,
                                             int16_t* residual, const uint32_t residual_stride, uint32_t area_height) {
    do {
        for (int i = 0; i < 128; i += 32) {
            residual_kernel32x1_neon(input + i, pred + i, residual + i);
        }

        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
    } while (--area_height != 0);
}

void svt_residual_kernel8bit_neon(uint8_t* input, uint32_t input_stride, uint8_t* pred, uint32_t pred_stride,
                                  int16_t* residual, uint32_t residual_stride, uint32_t area_width,
                                  uint32_t area_height) {
    switch (area_width) {
    case 4: {
        residual_kernel4xh_neon(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;
    }

    case 8: {
        residual_kernel8xh_neon(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;
    }

    case 16: {
        residual_kernel16xh_neon(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;
    }

    case 32: {
        residual_kernel32xh_neon(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;
    }

    case 64: {
        residual_kernel64xh_neon(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;
    }

    default: // 128
    {
        residual_kernel128xh_neon(input, input_stride, pred, pred_stride, residual, residual_stride, area_height);
        break;
    }
    }
}

void svt_full_distortion_kernel32_bits_neon(int32_t* coeff, uint32_t coeff_stride, int32_t* recon_coeff,
                                            uint32_t recon_coeff_stride, uint64_t distortion_result[DIST_CALC_TOTAL],
                                            uint32_t area_width, uint32_t area_height) {
    int64x2_t residual_distortion = vdupq_n_s64(0);
    int64x2_t residual_prediction = vdupq_n_s64(0);

    do {
        int32_t* coeff_temp       = coeff;
        int32_t* recon_coeff_temp = recon_coeff;

        uint32_t col_count = area_width;
        do {
            int32x4_t x0 = vld1q_s32(coeff_temp);
            int32x4_t y0 = vld1q_s32(recon_coeff_temp);

            int32x2_t x_lo = vget_low_s32(x0);
            int32x2_t x_hi = vget_high_s32(x0);
            int32x2_t y_lo = vget_low_s32(y0);
            int32x2_t y_hi = vget_high_s32(y0);

            residual_prediction = vmlal_s32(residual_prediction, x_lo, x_lo);
            residual_prediction = vmlal_s32(residual_prediction, x_hi, x_hi);

            int32x2_t x_lo_sub = vsub_s32(x_lo, y_lo);
            int32x2_t x_hi_sub = vsub_s32(x_hi, y_hi);

            residual_distortion = vmlal_s32(residual_distortion, x_lo_sub, x_lo_sub);
            residual_distortion = vmlal_s32(residual_distortion, x_hi_sub, x_hi_sub);

            coeff_temp += 4;
            recon_coeff_temp += 4;
            col_count -= 4;
        } while (col_count != 0);

        coeff += coeff_stride;
        recon_coeff += recon_coeff_stride;
    } while (--area_height != 0);

    vst1q_s64((int64_t*)distortion_result, vpaddq_s64(residual_distortion, residual_prediction));
}

static inline void unpack_and_2bcompress_32_neon(uint16_t* in16b_buffer, uint8_t* out8b_buffer, uint8_t* out2b_buffer,
                                                 uint32_t width_rep) {
    const uint16x8_t ymm_00ff = vdupq_n_u16(0x00FF);
    const uint16x8_t msk_2b   = vdupq_n_u16(0x0003); //0000.0000.0000.0011

    const uint32x4_t msk0 = vdupq_n_u32(0x000000C0); //1100.0000
    const uint32x4_t msk1 = vdupq_n_u32(0x00000030); //0011.0000
    const uint32x4_t msk2 = vdupq_n_u32(0x0000000C); //0000.1100

    for (uint32_t w = 0; w < width_rep; w++) {
        const uint16x8_t in1 = vld1q_u16(in16b_buffer + w * 16);
        const uint16x8_t in2 = vld1q_u16(in16b_buffer + w * 16 + 8);

        const uint16x8_t tmp_2b1 = vandq_u16(in1, msk_2b); //0000.0011.1111.1111 -> 0000.0000.0000.0011
        const uint16x8_t tmp_2b2 = vandq_u16(in2, msk_2b);
        const uint32x4_t tmp_2b  = vreinterpretq_u32_u8(vcombine_u8(vqmovn_u16(tmp_2b1), vqmovn_u16(tmp_2b2)));

        const uint32x4_t ext0 = vshrq_n_u32(
            tmp_2b,
            24); //0000.0011.0000.0000.0000.0000.0000.0000 -> 0000.0000.0000.0000.0000.0000.0000.0011
        const uint32x4_t ext1 = vandq_u32(
            vshrq_n_u32(tmp_2b, 14),
            msk2); //0000.0000.0000.0011.0000.0000.0000.0000 -> 0000.0000.0000.0000.0000.0000.0000.1100
        const uint32x4_t ext2 = vandq_u32(
            vshrq_n_u32(tmp_2b, 4),
            msk1); //0000.0000.0000.0000.0000.0011.0000.0000 -> 0000.0000.0000.0000.0000.0000.0011.0000
        const uint32x4_t ext3 = vandq_u32(
            vshlq_n_u32(tmp_2b, 6),
            msk0); //0000.0000.0000.0000.0000.0000.0000.0011 -> 0000.0000.0000.0000.0000.0000.1100.0000

        const uint32x4_t ext0123 = vorrq_u32(vorrq_u32(ext0, ext1),
                                             vorrq_u32(ext2, ext3)); //0000.0000.0000.0000.0000.0000.1111.1111

        const uint32_t ext0123_packed32 = vget_lane_u32(
            vreinterpret_u32_u8(vqmovn_u16(vcombine_u16(vqmovn_u32(ext0123), vdup_n_u16(0)))), 0);
        *((uint32_t*)(out2b_buffer + w * 4)) = ext0123_packed32;

        const uint8x16_t out8_u8 = vcombine_u8(vqmovn_u16(vandq_u16(vshrq_n_u16(in1, 2), ymm_00ff)),
                                               vqmovn_u16(vandq_u16(vshrq_n_u16(in2, 2), ymm_00ff)));

        vst1q_u8(out8b_buffer + w * 16, out8_u8);
    }
}

static inline void svt_unpack_and_2bcompress_remainder(uint16_t* in16b_buffer, uint8_t* out8b_buffer,
                                                       uint8_t* out2b_buffer, uint32_t width) {
    uint32_t col;
    uint16_t in_pixel;
    uint8_t  tmp_pixel;

    uint32_t w_m4  = (width / 4) * 4;
    uint32_t w_rem = width - w_m4;

    for (col = 0; col < w_m4; col += 4) {
        uint8_t compressed_unpacked_pixel = 0;
        //+0
        in_pixel              = in16b_buffer[col + 0];
        out8b_buffer[col + 0] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 0) & 0xC0); //1100.0000

        //+1
        in_pixel              = in16b_buffer[col + 1];
        out8b_buffer[col + 1] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 2) & 0x30); //0011.0000

        //+2
        in_pixel              = in16b_buffer[col + 2];
        out8b_buffer[col + 2] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 4) & 0x0C); //0000.1100

        //+3
        in_pixel              = in16b_buffer[col + 3];
        out8b_buffer[col + 3] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 6) & 0x03); //0000.0011

        out2b_buffer[col / 4] = compressed_unpacked_pixel;
    }

    //we can have up to 3 pixels remaining
    if (w_rem > 0) {
        uint8_t compressed_unpacked_pixel = 0;
        //+0
        in_pixel              = in16b_buffer[col + 0];
        out8b_buffer[col + 0] = (uint8_t)(in_pixel >> 2);
        tmp_pixel             = (uint8_t)(in_pixel << 6);
        compressed_unpacked_pixel |= ((tmp_pixel >> 0) & 0xC0); //1100.0000

        if (w_rem > 1) {
            //+1
            in_pixel              = in16b_buffer[col + 1];
            out8b_buffer[col + 1] = (uint8_t)(in_pixel >> 2);
            tmp_pixel             = (uint8_t)(in_pixel << 6);
            compressed_unpacked_pixel |= ((tmp_pixel >> 2) & 0x30); //0011.0000
        }
        if (w_rem > 2) {
            //+2
            in_pixel              = in16b_buffer[col + 2];
            out8b_buffer[col + 2] = (uint8_t)(in_pixel >> 2);
            tmp_pixel             = (uint8_t)(in_pixel << 6);
            compressed_unpacked_pixel |= ((tmp_pixel >> 4) & 0x0C); //0000.1100
        }

        out2b_buffer[col / 4] = compressed_unpacked_pixel;
    }
}

void svt_unpack_and_2bcompress_neon(uint16_t* in16b_buffer, uint32_t in16b_stride, uint8_t* out8b_buffer,
                                    uint32_t out8b_stride, uint8_t* out2b_buffer, uint32_t out2b_stride, uint32_t width,
                                    uint32_t height) {
    if (width == 32) {
        for (uint32_t h = 0; h < height; h++) {
            unpack_and_2bcompress_32_neon(
                in16b_buffer + h * in16b_stride, out8b_buffer + h * out8b_stride, out2b_buffer + h * out2b_stride, 2);
        }
    } else if (width == 64) {
        for (uint32_t h = 0; h < height; h++) {
            unpack_and_2bcompress_32_neon(
                in16b_buffer + h * in16b_stride, out8b_buffer + h * out8b_stride, out2b_buffer + h * out2b_stride, 4);
        }
    } else {
        uint32_t offset_rem   = width & 0xfffffff0;
        uint32_t offset2b_rem = offset_rem >> 2;
        uint32_t remainder    = width & 0xf;
        for (uint32_t h = 0; h < height; h++) {
            unpack_and_2bcompress_32_neon(in16b_buffer + h * in16b_stride,
                                          out8b_buffer + h * out8b_stride,
                                          out2b_buffer + h * out2b_stride,
                                          width >> 4);
            if (remainder) {
                svt_unpack_and_2bcompress_remainder(in16b_buffer + h * in16b_stride + offset_rem,
                                                    out8b_buffer + h * out8b_stride + offset_rem,
                                                    out2b_buffer + h * out2b_stride + offset2b_rem,
                                                    remainder);
            }
        }
    }
}

static const uint8_t unpack_tbl[64] = {0,  0,  0,  0,  1,  1,  1,  1,  2,  2,  2,  2,  3,  3,  3,  3,
                                       4,  4,  4,  4,  5,  5,  5,  5,  6,  6,  6,  6,  7,  7,  7,  7,
                                       8,  8,  8,  8,  9,  9,  9,  9,  10, 10, 10, 10, 11, 11, 11, 11,
                                       12, 12, 12, 12, 13, 13, 13, 13, 14, 14, 14, 14, 15, 15, 15, 15};

static const int8_t shift[4] = {0, 2, 4, 6};

static inline void compressed_packmsb_8x2h(const uint8_t* in8_bit_buffer, uint32_t in8_stride,
                                           const uint8_t* inn_bit_buffer, uint32_t inn_stride,
                                           uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t height) {
    const uint8x8_t idx0_1   = vld1_u8(unpack_tbl + 0 * 8);
    const uint8x8_t idx2_3   = vld1_u8(unpack_tbl + 1 * 8);
    const int8x8_t  shift_s8 = vreinterpret_s8_s32(vld1_dup_s32((const int32_t*)shift));

    do {
        const uint8x8_t in_2_bit = load_u8_2x2(inn_bit_buffer, inn_stride);

        uint8x8_t ext0_7  = vtbl1_u8(in_2_bit, idx0_1);
        uint8x8_t ext8_15 = vtbl1_u8(in_2_bit, idx2_3);
        ext0_7            = vshl_u8(ext0_7, shift_s8);
        ext8_15           = vshl_u8(ext8_15, shift_s8);

        const uint8x16_t ext0_7q  = vcombine_u8(ext0_7, vdup_n_u8(0));
        const uint8x16_t ext8_15q = vcombine_u8(ext8_15, vdup_n_u8(0));

        const uint8x16_t in_8_bit0 = vcombine_u8(vld1_u8(in8_bit_buffer + 0 * in8_stride), vdup_n_u8(0));
        const uint8x16_t in_8_bit2 = vcombine_u8(vld1_u8(in8_bit_buffer + 1 * in8_stride), vdup_n_u8(0));

        const uint16x8_t concat0 = vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(ext0_7q, in_8_bit0)), 6);
        const uint16x8_t concat1 = vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(ext8_15q, in_8_bit2)), 6);

        vst1q_u16(out16_bit_buffer + 0 * out_stride, concat0);
        vst1q_u16(out16_bit_buffer + 1 * out_stride, concat1);

        in8_bit_buffer += 2 * in8_stride;
        inn_bit_buffer += 2 * inn_stride;
        out16_bit_buffer += 2 * out_stride;
        height -= 2;
    } while (height != 0);
}

static inline void compressed_packmsb_16x2h(const uint8_t* in8_bit_buffer, uint32_t in8_stride,
                                            const uint8_t* inn_bit_buffer, uint32_t inn_stride,
                                            uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t height) {
    const uint8x16_t idx0_3   = vld1q_u8(unpack_tbl + 0 * 16);
    const uint8x16_t idx4_7   = vld1q_u8(unpack_tbl + 1 * 16);
    const int8x16_t  shift_s8 = vreinterpretq_s8_s32(vld1q_dup_s32((const int32_t*)shift));

    do {
        const uint8x8_t  in_2_bit_lo = load_u8_4x2(inn_bit_buffer, inn_stride);
        const uint8x16_t in_2_bit    = vcombine_u8(in_2_bit_lo, vdup_n_u8(0));

        uint8x16_t ext0_15  = vqtbl1q_u8(in_2_bit, idx0_3);
        uint8x16_t ext16_31 = vqtbl1q_u8(in_2_bit, idx4_7);

        ext0_15  = vshlq_u8(ext0_15, shift_s8);
        ext16_31 = vshlq_u8(ext16_31, shift_s8);

        const uint8x16_t in_8_bit0 = vld1q_u8(in8_bit_buffer + 0 * in8_stride);
        const uint8x16_t in_8_bit2 = vld1q_u8(in8_bit_buffer + 1 * in8_stride);

        const uint16x8_t concat0 = vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(ext0_15, in_8_bit0)), 6);
        const uint16x8_t concat1 = vshrq_n_u16(vreinterpretq_u16_u8(vzip2q_u8(ext0_15, in_8_bit0)), 6);

        vst1q_u16(out16_bit_buffer + 0, concat0);
        vst1q_u16(out16_bit_buffer + 8, concat1);

        const uint16x8_t concat2 = vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(ext16_31, in_8_bit2)), 6);
        const uint16x8_t concat3 = vshrq_n_u16(vreinterpretq_u16_u8(vzip2q_u8(ext16_31, in_8_bit2)), 6);

        vst1q_u16(out16_bit_buffer + out_stride + 0, concat2);
        vst1q_u16(out16_bit_buffer + out_stride + 8, concat3);

        in8_bit_buffer += 2 * in8_stride;
        inn_bit_buffer += 2 * inn_stride;
        out16_bit_buffer += 2 * out_stride;
        height -= 2;
    } while (height != 0);
}

static inline void compressed_packmsb_32x2h(const uint8_t* in8_bit_buffer, uint32_t in8_stride,
                                            const uint8_t* inn_bit_buffer, uint32_t inn_stride,
                                            uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t height) {
    const uint8x16_t idx0_3   = vld1q_u8(unpack_tbl + 0 * 16);
    const uint8x16_t idx4_7   = vld1q_u8(unpack_tbl + 1 * 16);
    const uint8x16_t idx8_11  = vld1q_u8(unpack_tbl + 2 * 16);
    const uint8x16_t idx12_15 = vld1q_u8(unpack_tbl + 3 * 16);
    const int8x16_t  shift_s8 = vreinterpretq_s8_s32(vld1q_dup_s32((const int32_t*)shift));

    do {
        const uint8x16_t in_2_bit = load_u8_8x2(inn_bit_buffer, inn_stride);

        uint8x16_t ext0_15  = vqtbl1q_u8(in_2_bit, idx0_3);
        uint8x16_t ext16_31 = vqtbl1q_u8(in_2_bit, idx4_7);
        uint8x16_t ext32_47 = vqtbl1q_u8(in_2_bit, idx8_11);
        uint8x16_t ext48_63 = vqtbl1q_u8(in_2_bit, idx12_15);

        ext0_15  = vshlq_u8(ext0_15, shift_s8);
        ext16_31 = vshlq_u8(ext16_31, shift_s8);
        ext32_47 = vshlq_u8(ext32_47, shift_s8);
        ext48_63 = vshlq_u8(ext48_63, shift_s8);

        const uint8x16_t in_8_bit0 = vld1q_u8(in8_bit_buffer + 0);
        const uint8x16_t in_8_bit1 = vld1q_u8(in8_bit_buffer + 16);
        const uint8x16_t in_8_bit2 = vld1q_u8(in8_bit_buffer + in8_stride);
        const uint8x16_t in_8_bit3 = vld1q_u8(in8_bit_buffer + in8_stride + 16);

        const uint16x8_t concat00 = vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(ext0_15, in_8_bit0)), 6);
        const uint16x8_t concat01 = vshrq_n_u16(vreinterpretq_u16_u8(vzip2q_u8(ext0_15, in_8_bit0)), 6);
        const uint16x8_t concat02 = vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(ext16_31, in_8_bit1)), 6);
        const uint16x8_t concat03 = vshrq_n_u16(vreinterpretq_u16_u8(vzip2q_u8(ext16_31, in_8_bit1)), 6);

        vst1q_u16(out16_bit_buffer + 0, concat00);
        vst1q_u16(out16_bit_buffer + 8, concat01);
        vst1q_u16(out16_bit_buffer + 16, concat02);
        vst1q_u16(out16_bit_buffer + 24, concat03);

        const uint16x8_t concat10 = vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(ext32_47, in_8_bit2)), 6);
        const uint16x8_t concat11 = vshrq_n_u16(vreinterpretq_u16_u8(vzip2q_u8(ext32_47, in_8_bit2)), 6);
        const uint16x8_t concat12 = vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(ext48_63, in_8_bit3)), 6);
        const uint16x8_t concat13 = vshrq_n_u16(vreinterpretq_u16_u8(vzip2q_u8(ext48_63, in_8_bit3)), 6);

        vst1q_u16(out16_bit_buffer + out_stride + 0, concat10);
        vst1q_u16(out16_bit_buffer + out_stride + 8, concat11);
        vst1q_u16(out16_bit_buffer + out_stride + 16, concat12);
        vst1q_u16(out16_bit_buffer + out_stride + 24, concat13);

        in8_bit_buffer += 2 * in8_stride;
        inn_bit_buffer += 2 * inn_stride;
        out16_bit_buffer += 2 * out_stride;
        height -= 2;
    } while (height != 0);
}

static inline void compressed_packmsb_64xh(const uint8_t* in8_bit_buffer, uint32_t in8_stride,
                                           const uint8_t* inn_bit_buffer, uint32_t inn_stride,
                                           uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t height) {
    const uint8x16_t idx0_3   = vld1q_u8(unpack_tbl + 0 * 16);
    const uint8x16_t idx4_7   = vld1q_u8(unpack_tbl + 1 * 16);
    const uint8x16_t idx8_11  = vld1q_u8(unpack_tbl + 2 * 16);
    const uint8x16_t idx12_15 = vld1q_u8(unpack_tbl + 3 * 16);
    const int8x16_t  shift_s8 = vreinterpretq_s8_s32(vld1q_dup_s32((const int32_t*)shift));

    // one row per iteration
    do {
        const uint8x16_t in_2_bit = vld1q_u8(inn_bit_buffer);

        uint8x16_t ext0_15  = vqtbl1q_u8(in_2_bit, idx0_3);
        uint8x16_t ext16_31 = vqtbl1q_u8(in_2_bit, idx4_7);
        uint8x16_t ext32_47 = vqtbl1q_u8(in_2_bit, idx8_11);
        uint8x16_t ext48_63 = vqtbl1q_u8(in_2_bit, idx12_15);

        ext0_15  = vshlq_u8(ext0_15, shift_s8);
        ext16_31 = vshlq_u8(ext16_31, shift_s8);
        ext32_47 = vshlq_u8(ext32_47, shift_s8);
        ext48_63 = vshlq_u8(ext48_63, shift_s8);

        const uint8x16_t in_8_bit0 = vld1q_u8(in8_bit_buffer + 0);
        const uint8x16_t in_8_bit1 = vld1q_u8(in8_bit_buffer + 16);
        const uint8x16_t in_8_bit2 = vld1q_u8(in8_bit_buffer + 32);
        const uint8x16_t in_8_bit3 = vld1q_u8(in8_bit_buffer + 48);

        // (out_pixel | n_bit_pixel) concatenation
        const uint16x8_t concat00 = vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(ext0_15, in_8_bit0)), 6);
        const uint16x8_t concat01 = vshrq_n_u16(vreinterpretq_u16_u8(vzip2q_u8(ext0_15, in_8_bit0)), 6);
        const uint16x8_t concat02 = vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(ext16_31, in_8_bit1)), 6);
        const uint16x8_t concat03 = vshrq_n_u16(vreinterpretq_u16_u8(vzip2q_u8(ext16_31, in_8_bit1)), 6);

        vst1q_u16(out16_bit_buffer + 0, concat00);
        vst1q_u16(out16_bit_buffer + 8, concat01);
        vst1q_u16(out16_bit_buffer + 16, concat02);
        vst1q_u16(out16_bit_buffer + 24, concat03);

        // (out_pixel | n_bit_pixel) concatenation
        const uint16x8_t concat10 = vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(ext32_47, in_8_bit2)), 6);
        const uint16x8_t concat11 = vshrq_n_u16(vreinterpretq_u16_u8(vzip2q_u8(ext32_47, in_8_bit2)), 6);
        const uint16x8_t concat12 = vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(ext48_63, in_8_bit3)), 6);
        const uint16x8_t concat13 = vshrq_n_u16(vreinterpretq_u16_u8(vzip2q_u8(ext48_63, in_8_bit3)), 6);

        vst1q_u16(out16_bit_buffer + 32, concat10);
        vst1q_u16(out16_bit_buffer + 40, concat11);
        vst1q_u16(out16_bit_buffer + 48, concat12);
        vst1q_u16(out16_bit_buffer + 56, concat13);

        in8_bit_buffer += in8_stride;
        inn_bit_buffer += inn_stride;
        out16_bit_buffer += out_stride;
    } while (--height != 0);
}

void svt_compressed_packmsb_neon(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                                 uint32_t inn_stride, uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t width,
                                 uint32_t height) {
    if (width == 32) {
        compressed_packmsb_32x2h(
            in8_bit_buffer, in8_stride, inn_bit_buffer, inn_stride, out16_bit_buffer, out_stride, height);
    } else if (width == 64) {
        compressed_packmsb_64xh(
            in8_bit_buffer, in8_stride, inn_bit_buffer, inn_stride, out16_bit_buffer, out_stride, height);
    } else {
        while (width >= 64) {
            compressed_packmsb_64xh(
                in8_bit_buffer, in8_stride, inn_bit_buffer, inn_stride, out16_bit_buffer, out_stride, height);

            inn_bit_buffer += 16; // 4 elements per byte.
            in8_bit_buffer += 64;
            out16_bit_buffer += 64;
            width -= 64;
        }
        if (width >= 32) {
            compressed_packmsb_32x2h(
                in8_bit_buffer, in8_stride, inn_bit_buffer, inn_stride, out16_bit_buffer, out_stride, height);

            inn_bit_buffer += 8; // 4 elements per byte.
            in8_bit_buffer += 32;
            out16_bit_buffer += 32;
            width -= 32;
        }
        if (width >= 16) {
            compressed_packmsb_16x2h(
                in8_bit_buffer, in8_stride, inn_bit_buffer, inn_stride, out16_bit_buffer, out_stride, height);

            inn_bit_buffer += 4; // 4 elements per byte.
            in8_bit_buffer += 16;
            out16_bit_buffer += 16;
            width -= 16;
        }
        if (width >= 8) {
            compressed_packmsb_8x2h(
                in8_bit_buffer, in8_stride, inn_bit_buffer, inn_stride, out16_bit_buffer, out_stride, height);

            inn_bit_buffer += 2; // 4 elements per byte.
            in8_bit_buffer += 8;
            out16_bit_buffer += 8;
            width -= 8;
        }
        if (width) {
            svt_compressed_packmsb_c(
                in8_bit_buffer, in8_stride, inn_bit_buffer, inn_stride, out16_bit_buffer, out_stride, width, height);
        }
    }
}

void svt_enc_msb_pack2d_neon(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                             uint16_t* out16_bit_buffer, uint32_t inn_stride, uint32_t out_stride, uint32_t width,
                             uint32_t height) {
    uint32_t count_width, count_height;

    if (width == 4) {
        for (count_height = 0; count_height < height; count_height += 2) {
            vst1_u16(
                out16_bit_buffer,
                vshr_n_u16(vreinterpret_u16_u8(vzip1_u8(vreinterpret_u8_u32(vdup_n_u32(*(uint32_t*)(inn_bit_buffer))),
                                                        vreinterpret_u8_u32(vdup_n_u32(*(uint32_t*)(in8_bit_buffer))))),
                           6));
            vst1_u16(out16_bit_buffer + out_stride,
                     vshr_n_u16(vreinterpret_u16_u8(vzip1_u8(
                                    vreinterpret_u8_u32(vdup_n_u32(*(uint32_t*)(inn_bit_buffer + inn_stride))),
                                    vreinterpret_u8_u32(vdup_n_u32(*(uint32_t*)(in8_bit_buffer + in8_stride))))),
                                6));

            out16_bit_buffer += (out_stride << 1);
            in8_bit_buffer += (in8_stride << 1);
            inn_bit_buffer += (inn_stride << 1);
        }
    } else if (width == 8) {
        for (count_height = 0; count_height < height; count_height += 2) {
            vst1q_u16(
                out16_bit_buffer,
                vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(
                                vcombine_u8(vreinterpret_u8_u64(vld1_u64((uint64_t*)(inn_bit_buffer))), vdup_n_u8(0)),
                                vcombine_u8(vreinterpret_u8_u64(vld1_u64((uint64_t*)(in8_bit_buffer))), vdup_n_u8(0)))),
                            6));
            vst1q_u16(
                out16_bit_buffer + out_stride,
                vshrq_n_u16(vreinterpretq_u16_u8(vzip1q_u8(
                                vcombine_u8(vreinterpret_u8_u64(vld1_u64((uint64_t*)(inn_bit_buffer + inn_stride))),
                                            vdup_n_u8(0)),
                                vcombine_u8(vreinterpret_u8_u64(vld1_u64((uint64_t*)(in8_bit_buffer + in8_stride))),
                                            vdup_n_u8(0)))),
                            6));

            out16_bit_buffer += (out_stride << 1);
            in8_bit_buffer += (in8_stride << 1);
            inn_bit_buffer += (inn_stride << 1);
        }
    } else if (width == 16) {
        for (count_height = 0; count_height < height; count_height += 2) {
            const uint8x16_t inn_bit_buffer_lo = vld1q_u8(inn_bit_buffer);
            const uint8x16_t inn_bit_buffer_hi = vld1q_u8(inn_bit_buffer + inn_stride);
            const uint8x16_t in_8bit_buffer_lo = vld1q_u8(in8_bit_buffer);
            const uint8x16_t in_8bit_buffer_hi = vld1q_u8(in8_bit_buffer + in8_stride);

            const uint16x8_t out_pixel_1 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip1q_u8(inn_bit_buffer_lo, in_8bit_buffer_lo)), 6);
            const uint16x8_t out_pixel_2 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip2q_u8(inn_bit_buffer_lo, in_8bit_buffer_lo)), 6);
            const uint16x8_t out_pixel_3 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip1q_u8(inn_bit_buffer_hi, in_8bit_buffer_hi)), 6);
            const uint16x8_t out_pixel_4 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip2q_u8(inn_bit_buffer_hi, in_8bit_buffer_hi)), 6);

            vst1q_u16(out16_bit_buffer + 0, out_pixel_1);
            vst1q_u16(out16_bit_buffer + 8, out_pixel_2);
            vst1q_u16(out16_bit_buffer + out_stride + 0, out_pixel_3);
            vst1q_u16(out16_bit_buffer + out_stride + 8, out_pixel_4);

            in8_bit_buffer += (in8_stride << 1);
            inn_bit_buffer += (inn_stride << 1);
            out16_bit_buffer += (out_stride << 1);
        }
    } else if (width == 32) {
        for (count_height = 0; count_height < height; count_height += 2) {
            const uint8x16_t inn_bit_buffer_1 = vld1q_u8(inn_bit_buffer);
            const uint8x16_t inn_bit_buffer_2 = vld1q_u8(inn_bit_buffer + 16);
            const uint8x16_t inn_bit_buffer_3 = vld1q_u8(inn_bit_buffer + inn_stride);
            const uint8x16_t inn_bit_buffer_4 = vld1q_u8(inn_bit_buffer + inn_stride + 16);

            const uint8x16_t in_8bit_buffer1 = vld1q_u8(in8_bit_buffer);
            const uint8x16_t in_8bit_buffer2 = vld1q_u8(in8_bit_buffer + 16);
            const uint8x16_t in_8bit_buffer3 = vld1q_u8(in8_bit_buffer + in8_stride);
            const uint8x16_t in_8bit_buffer4 = vld1q_u8(in8_bit_buffer + in8_stride + 16);

            const uint16x8_t out_pixel_1 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip1q_u8(inn_bit_buffer_1, in_8bit_buffer1)), 6);
            const uint16x8_t out_pixel_2 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip2q_u8(inn_bit_buffer_1, in_8bit_buffer1)), 6);
            const uint16x8_t out_pixel_3 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip1q_u8(inn_bit_buffer_2, in_8bit_buffer2)), 6);
            const uint16x8_t out_pixel_4 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip2q_u8(inn_bit_buffer_2, in_8bit_buffer2)), 6);
            const uint16x8_t out_pixel_5 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip1q_u8(inn_bit_buffer_3, in_8bit_buffer3)), 6);
            const uint16x8_t out_pixel_6 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip2q_u8(inn_bit_buffer_3, in_8bit_buffer3)), 6);
            const uint16x8_t out_pixel_7 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip1q_u8(inn_bit_buffer_4, in_8bit_buffer4)), 6);
            const uint16x8_t out_pixel_8 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip2q_u8(inn_bit_buffer_4, in_8bit_buffer4)), 6);

            vst1q_u16(out16_bit_buffer + 0, out_pixel_1);
            vst1q_u16(out16_bit_buffer + 8, out_pixel_2);
            vst1q_u16(out16_bit_buffer + 16, out_pixel_3);
            vst1q_u16(out16_bit_buffer + 24, out_pixel_4);
            vst1q_u16(out16_bit_buffer + out_stride + 0, out_pixel_5);
            vst1q_u16(out16_bit_buffer + out_stride + 8, out_pixel_6);
            vst1q_u16(out16_bit_buffer + out_stride + 16, out_pixel_7);
            vst1q_u16(out16_bit_buffer + out_stride + 24, out_pixel_8);

            in8_bit_buffer += (in8_stride << 1);
            inn_bit_buffer += (inn_stride << 1);
            out16_bit_buffer += (out_stride << 1);
        }
    } else if (width == 64) {
        for (count_height = 0; count_height < height; ++count_height) {
            const uint8x16_t inn_bit_buffer_1 = vld1q_u8(inn_bit_buffer);
            const uint8x16_t inn_bit_buffer_2 = vld1q_u8(inn_bit_buffer + 16);
            const uint8x16_t inn_bit_buffer_3 = vld1q_u8(inn_bit_buffer + 32);
            const uint8x16_t inn_bit_buffer_4 = vld1q_u8(inn_bit_buffer + 48);

            const uint8x16_t in_8bit_buffer1 = vld1q_u8(in8_bit_buffer);
            const uint8x16_t in_8bit_buffer2 = vld1q_u8(in8_bit_buffer + 16);
            const uint8x16_t in_8bit_buffer3 = vld1q_u8(in8_bit_buffer + 32);
            const uint8x16_t in_8bit_buffer4 = vld1q_u8(in8_bit_buffer + 48);

            const uint16x8_t out_pixel_1 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip1q_u8(inn_bit_buffer_1, in_8bit_buffer1)), 6);
            const uint16x8_t out_pixel_2 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip2q_u8(inn_bit_buffer_1, in_8bit_buffer1)), 6);
            const uint16x8_t out_pixel_3 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip1q_u8(inn_bit_buffer_2, in_8bit_buffer2)), 6);
            const uint16x8_t out_pixel_4 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip2q_u8(inn_bit_buffer_2, in_8bit_buffer2)), 6);
            const uint16x8_t out_pixel_5 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip1q_u8(inn_bit_buffer_3, in_8bit_buffer3)), 6);
            const uint16x8_t out_pixel_6 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip2q_u8(inn_bit_buffer_3, in_8bit_buffer3)), 6);
            const uint16x8_t out_pixel_7 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip1q_u8(inn_bit_buffer_4, in_8bit_buffer4)), 6);
            const uint16x8_t out_pixel_8 = vshrq_n_u16(
                vreinterpretq_u16_u8(vzip2q_u8(inn_bit_buffer_4, in_8bit_buffer4)), 6);

            vst1q_u16(out16_bit_buffer + 0, out_pixel_1);
            vst1q_u16(out16_bit_buffer + 8, out_pixel_2);
            vst1q_u16(out16_bit_buffer + 16, out_pixel_3);
            vst1q_u16(out16_bit_buffer + 24, out_pixel_4);
            vst1q_u16(out16_bit_buffer + 32, out_pixel_5);
            vst1q_u16(out16_bit_buffer + 40, out_pixel_6);
            vst1q_u16(out16_bit_buffer + 48, out_pixel_7);
            vst1q_u16(out16_bit_buffer + 56, out_pixel_8);

            in8_bit_buffer += in8_stride;
            inn_bit_buffer += inn_stride;
            out16_bit_buffer += out_stride;
        }
    } else {
        uint32_t in_n_stride_diff = (inn_stride << 1) - width;
        uint32_t in_8_stride_diff = (in8_stride << 1) - width;
        uint32_t out_stride_diff  = (out_stride << 1) - width;

        if (!(width & 7)) {
            for (count_height = 0; count_height < height; count_height += 2) {
                for (count_width = 0; count_width < width; count_width += 8) {
                    vst1q_u16(
                        out16_bit_buffer,
                        vshrq_n_u16(
                            vreinterpretq_u16_u8(vzip1q_u8(
                                vcombine_u8(vreinterpret_u8_u64(vld1_u64((uint64_t*)(inn_bit_buffer))), vdup_n_u8(0)),
                                vcombine_u8(vreinterpret_u8_u64(vld1_u64((uint64_t*)(in8_bit_buffer))), vdup_n_u8(0)))),
                            6));
                    vst1q_u16(
                        out16_bit_buffer + out_stride,
                        vshrq_n_u16(
                            vreinterpretq_u16_u8(vzip1q_u8(
                                vcombine_u8(vreinterpret_u8_u64(vld1_u64((uint64_t*)(inn_bit_buffer + inn_stride))),
                                            vdup_n_u8(0)),
                                vcombine_u8(vreinterpret_u8_u64(vld1_u64((uint64_t*)(in8_bit_buffer + in8_stride))),
                                            vdup_n_u8(0)))),
                            6));

                    out16_bit_buffer += 8;
                    in8_bit_buffer += 8;
                    inn_bit_buffer += 8;
                }
                in8_bit_buffer += in_8_stride_diff;
                inn_bit_buffer += in_n_stride_diff;
                out16_bit_buffer += out_stride_diff;
            }
        } else {
            for (count_height = 0; count_height < height; count_height += 2) {
                for (count_width = 0; count_width < width; count_width += 4) {
                    vst1_u16(out16_bit_buffer,
                             vshr_n_u16(vreinterpret_u16_u8(
                                            vzip1_u8(vreinterpret_u8_u32(vdup_n_u32(*(uint32_t*)(inn_bit_buffer))),
                                                     vreinterpret_u8_u32(vdup_n_u32(*(uint32_t*)(in8_bit_buffer))))),
                                        6));
                    vst1_u16(
                        out16_bit_buffer + out_stride,
                        vshr_n_u16(vreinterpret_u16_u8(vzip1_u8(
                                       vreinterpret_u8_u32(vdup_n_u32(*(uint32_t*)(inn_bit_buffer + inn_stride))),
                                       vreinterpret_u8_u32(vdup_n_u32(*(uint32_t*)(in8_bit_buffer + in8_stride))))),
                                   6));

                    out16_bit_buffer += 4;
                    in8_bit_buffer += 4;
                    inn_bit_buffer += 4;
                }
                in8_bit_buffer += in_8_stride_diff;
                inn_bit_buffer += in_n_stride_diff;
                out16_bit_buffer += out_stride_diff;
            }
        }
    }
}

void svt_full_distortion_kernel_cbf_zero32_bits_neon(int32_t* coeff, uint32_t coeff_stride,
                                                     uint64_t distortion_result[DIST_CALC_TOTAL], uint32_t area_width,
                                                     uint32_t area_height) {
    uint64x2_t sum = vdupq_n_u64(0);

    uint32_t row_count = area_height;
    do {
        int32_t* coeff_temp = coeff;

        uint32_t col_count = area_width / 4;
        do {
            const int32x2_t x_lo = vld1_s32(coeff_temp + 0);
            const int32x2_t x_hi = vld1_s32(coeff_temp + 2);
            coeff_temp += 4;

            const uint64x2_t y_lo = vreinterpretq_u64_s64(vmull_s32(x_lo, x_lo));
            const uint64x2_t y_hi = vreinterpretq_u64_s64(vmull_s32(x_hi, x_hi));

            sum = vaddq_u64(sum, y_lo);
            sum = vaddq_u64(sum, y_hi);

        } while (--col_count);

        coeff += coeff_stride;
        row_count -= 1;
    } while (row_count > 0);

    const uint64x2_t temp2 = vextq_u64(sum, sum, 1);
    const uint64x2_t temp1 = vaddq_u64(sum, temp2);
    vst1q_u64(distortion_result, temp1);
}

/******************************************************************************************************
                                       svt_residual_kernel16bit_neon
******************************************************************************************************/
void svt_residual_kernel16bit_neon(uint16_t* input, uint32_t input_stride, uint16_t* pred, uint32_t pred_stride,
                                   int16_t* residual, uint32_t residual_stride, uint32_t area_width,
                                   uint32_t area_height) {
    if (area_width == 4) {
        for (uint32_t height = 0; height < area_height; height += 2) {
            const uint16x4_t residual64_0 = vsub_u16(vld1_u16(input), vld1_u16(pred));
            const uint16x4_t residual64_1 = vsub_u16(vld1_u16((input + input_stride)), vld1_u16((pred + pred_stride)));

            vst1_s16(residual, vreinterpret_s16_u16(residual64_0));
            vst1_s16((residual + residual_stride), vreinterpret_s16_u16(residual64_1));

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
    } else if (area_width == 8) {
        for (uint32_t height = 0; height < area_height; height += 2) {
            const uint16x8_t residual0 = vsubq_u16(vld1q_u16(input), vld1q_u16(pred));
            const uint16x8_t residual1 = vsubq_u16(vld1q_u16((input + input_stride)), vld1q_u16((pred + pred_stride)));

            vst1q_s16(residual, vreinterpretq_s16_u16(residual0));
            vst1q_s16((residual + residual_stride), vreinterpretq_s16_u16(residual1));

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
    } else if (area_width == 16) {
        for (uint32_t height = 0; height < area_height; height += 2) {
            const uint16x8_t residual0 = vsubq_u16(vld1q_u16(input), vld1q_u16(pred));
            const uint16x8_t residual1 = vsubq_u16(vld1q_u16((input + 8)), vld1q_u16((pred + 8)));
            const uint16x8_t residual2 = vsubq_u16(vld1q_u16((input + input_stride)), vld1q_u16((pred + pred_stride)));
            const uint16x8_t residual3 = vsubq_u16(vld1q_u16((input + input_stride + 8)),
                                                   vld1q_u16((pred + pred_stride + 8)));

            vst1q_s16(residual, vreinterpretq_s16_u16(residual0));
            vst1q_s16((residual + 8), vreinterpretq_s16_u16(residual1));
            vst1q_s16((residual + residual_stride), vreinterpretq_s16_u16(residual2));
            vst1q_s16((residual + residual_stride + 8), vreinterpretq_s16_u16(residual3));

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
    } else if (area_width == 32) {
        for (uint32_t height = 0; height < area_height; height += 2) {
            vst1q_s16(residual, vreinterpretq_s16_u16(vsubq_u16(vld1q_u16(input), vld1q_u16(pred))));
            vst1q_s16((residual + 8), vreinterpretq_s16_u16(vsubq_u16(vld1q_u16((input + 8)), vld1q_u16((pred + 8)))));
            vst1q_s16((residual + 16),
                      vreinterpretq_s16_u16(vsubq_u16(vld1q_u16((input + 16)), vld1q_u16((pred + 16)))));
            vst1q_s16((residual + 24),
                      vreinterpretq_s16_u16(vsubq_u16(vld1q_u16((input + 24)), vld1q_u16((pred + 24)))));

            vst1q_s16(
                (residual + residual_stride),
                vreinterpretq_s16_u16(vsubq_u16(vld1q_u16((input + input_stride)), vld1q_u16((pred + pred_stride)))));
            vst1q_s16((residual + residual_stride + 8),
                      vreinterpretq_s16_u16(
                          vsubq_u16(vld1q_u16((input + input_stride + 8)), vld1q_u16((pred + pred_stride + 8)))));
            vst1q_s16((residual + residual_stride + 16),
                      vreinterpretq_s16_u16(
                          vsubq_u16(vld1q_u16((input + input_stride + 16)), vld1q_u16((pred + pred_stride + 16)))));
            vst1q_s16((residual + residual_stride + 24),
                      vreinterpretq_s16_u16(
                          vsubq_u16(vld1q_u16((input + input_stride + 24)), vld1q_u16((pred + pred_stride + 24)))));

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
    } else if (area_width == 64) { // Branch was not tested because the encoder had max txb_size of 32
        for (uint32_t height = 0; height < area_height; height += 2) {
            vst1q_s16(residual, vreinterpretq_s16_u16(vsubq_u16(vld1q_u16(input), vld1q_u16(pred))));
            vst1q_s16((residual + 8), vreinterpretq_s16_u16(vsubq_u16(vld1q_u16((input + 8)), vld1q_u16((pred + 8)))));
            vst1q_s16((residual + 16),
                      vreinterpretq_s16_u16(vsubq_u16(vld1q_u16((input + 16)), vld1q_u16((pred + 16)))));
            vst1q_s16((residual + 24),
                      vreinterpretq_s16_u16(vsubq_u16(vld1q_u16((input + 24)), vld1q_u16((pred + 24)))));
            vst1q_s16((residual + 32),
                      vreinterpretq_s16_u16(vsubq_u16(vld1q_u16((input + 32)), vld1q_u16((pred + 32)))));
            vst1q_s16((residual + 40),
                      vreinterpretq_s16_u16(vsubq_u16(vld1q_u16((input + 40)), vld1q_u16((pred + 40)))));
            vst1q_s16((residual + 48),
                      vreinterpretq_s16_u16(vsubq_u16(vld1q_u16((input + 48)), vld1q_u16((pred + 48)))));
            vst1q_s16((residual + 56),
                      vreinterpretq_s16_u16(vsubq_u16(vld1q_u16((input + 56)), vld1q_u16((pred + 56)))));

            vst1q_s16(
                (residual + residual_stride),
                vreinterpretq_s16_u16(vsubq_u16(vld1q_u16((input + input_stride)), vld1q_u16((pred + pred_stride)))));
            vst1q_s16((residual + residual_stride + 8),
                      vreinterpretq_s16_u16(
                          vsubq_u16(vld1q_u16((input + input_stride + 8)), vld1q_u16((pred + pred_stride + 8)))));
            vst1q_s16((residual + residual_stride + 16),
                      vreinterpretq_s16_u16(
                          vsubq_u16(vld1q_u16((input + input_stride + 16)), vld1q_u16((pred + pred_stride + 16)))));
            vst1q_s16((residual + residual_stride + 24),
                      vreinterpretq_s16_u16(
                          vsubq_u16(vld1q_u16((input + input_stride + 24)), vld1q_u16((pred + pred_stride + 24)))));
            vst1q_s16((residual + residual_stride + 32),
                      vreinterpretq_s16_u16(
                          vsubq_u16(vld1q_u16((input + input_stride + 32)), vld1q_u16((pred + pred_stride + 32)))));
            vst1q_s16((residual + residual_stride + 40),
                      vreinterpretq_s16_u16(
                          vsubq_u16(vld1q_u16((input + input_stride + 40)), vld1q_u16((pred + pred_stride + 40)))));
            vst1q_s16((residual + residual_stride + 48),
                      vreinterpretq_s16_u16(
                          vsubq_u16(vld1q_u16((input + input_stride + 48)), vld1q_u16((pred + pred_stride + 48)))));
            vst1q_s16((residual + residual_stride + 56),
                      vreinterpretq_s16_u16(
                          vsubq_u16(vld1q_u16((input + input_stride + 56)), vld1q_u16((pred + pred_stride + 56)))));

            input += input_stride << 1;
            pred += pred_stride << 1;
            residual += residual_stride << 1;
        }
    } else {
        const uint32_t input_stride_diff    = 2 * input_stride - area_width;
        const uint32_t pred_stride_diff     = 2 * pred_stride - area_width;
        const uint32_t residual_stride_diff = 2 * residual_stride - area_width;

        if (!(area_width & 7)) {
            for (uint32_t height = 0; height < area_height; height += 2) {
                for (uint32_t width = 0; width < area_width; width += 8) {
                    vst1q_s16(residual, vreinterpretq_s16_u16(vsubq_u16(vld1q_u16(input), vld1q_u16(pred))));
                    vst1q_s16((residual + residual_stride),
                              vreinterpretq_s16_u16(
                                  vsubq_u16(vld1q_u16((input + input_stride)), vld1q_u16((pred + pred_stride)))));

                    input += 8;
                    pred += 8;
                    residual += 8;
                }
                input    = input + input_stride_diff;
                pred     = pred + pred_stride_diff;
                residual = residual + residual_stride_diff;
            }
        } else {
            for (uint32_t height = 0; height < area_height; height += 2) {
                for (uint32_t width = 0; width < area_width; width += 4) {
                    vst1_s16(residual,
                             vreinterpret_s16_u16(vget_low_u16(vsubq_u16(vld1q_u16(input), vld1q_u16(pred)))));
                    vst1_s16((residual + residual_stride),
                             vreinterpret_s16_u16(vget_low_u16(
                                 vsubq_u16(vld1q_u16((input + input_stride)), vld1q_u16((pred + pred_stride))))));

                    input += 4;
                    pred += 4;
                    residual += 4;
                }
                input += input_stride_diff;
                pred += pred_stride_diff;
                residual += residual_stride_diff;
            }
        }
    }
}
