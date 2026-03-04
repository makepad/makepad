/*
* Copyright(c) 2024 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <arm_neon.h>
#include <stdint.h>

#include "definitions.h"
#include "mem_neon.h"

void svt_enc_msb_un_pack2d_neon(uint16_t* in16_bit_buffer, uint32_t in_stride, uint8_t* out8_bit_buffer,
                                uint8_t* outn_bit_buffer, uint32_t out8_stride, uint32_t outn_stride, uint32_t width,
                                uint32_t height) {
    if (width % 16 == 0) {
        do {
            int       w        = width;
            uint16_t* in16_ptr = in16_bit_buffer;
            uint8_t*  out8_ptr = out8_bit_buffer;
            uint8_t*  outn_ptr = outn_bit_buffer;

            do {
                const uint16x8_t in0_lo = vld1q_u16(in16_ptr);
                const uint16x8_t in0_hi = vld1q_u16(in16_ptr + 8);
                const uint16x8_t in1_lo = vld1q_u16(in16_ptr + in_stride);
                const uint16x8_t in1_hi = vld1q_u16(in16_ptr + in_stride + 8);

                const uint8x16_t in0_u8 = vcombine_u8(vshrn_n_u16(in0_lo, 2), vshrn_n_u16(in0_hi, 2));
                const uint8x16_t in1_u8 = vcombine_u8(vshrn_n_u16(in1_lo, 2), vshrn_n_u16(in1_hi, 2));

                vst1q_u8(out8_ptr, in0_u8);
                vst1q_u8(out8_ptr + out8_stride, in1_u8);

                if (outn_bit_buffer) {
                    const uint16x8_t in0_lo_u2 = vshlq_n_u16(in0_lo, 6);
                    const uint16x8_t in0_hi_u2 = vshlq_n_u16(in0_hi, 6);
                    const uint16x8_t in1_lo_u2 = vshlq_n_u16(in1_lo, 6);
                    const uint16x8_t in1_hi_u2 = vshlq_n_u16(in1_hi, 6);
                    const uint8x16_t in0_u2    = vcombine_u8(vmovn_u16(in0_lo_u2), vmovn_u16(in0_hi_u2));
                    const uint8x16_t in1_u2    = vcombine_u8(vmovn_u16(in1_lo_u2), vmovn_u16(in1_hi_u2));

                    vst1q_u8(outn_ptr, in0_u2);
                    vst1q_u8(outn_ptr + outn_stride, in1_u2);

                    outn_ptr += 16;
                }

                in16_ptr += 16;
                out8_ptr += 16;
                w -= 16;
            } while (w != 0);

            in16_bit_buffer += 2 * in_stride;
            out8_bit_buffer += 2 * out8_stride;
            if (outn_bit_buffer) {
                outn_bit_buffer += 2 * outn_stride;
            }
            height -= 2;
        } while (height != 0);
    } else if (width == 4) {
        do {
            const uint16x8_t in0 = load_u16_4x2(in16_bit_buffer, in_stride);

            store_u8x4_strided_x2(out8_bit_buffer, out8_stride, vshrn_n_u16(in0, 2));

            if (outn_bit_buffer) {
                const uint16x8_t in0_u2 = vshlq_n_u16(in0, 6);
                store_u8x4_strided_x2(outn_bit_buffer, outn_stride, vmovn_u16(in0_u2));

                outn_bit_buffer += 2 * outn_stride;
            }

            out8_bit_buffer += 2 * out8_stride;
            in16_bit_buffer += 2 * in_stride;
            height -= 2;
        } while (height != 0);
    } else if (width == 8) {
        do {
            const uint16x8_t in0 = vld1q_u16(in16_bit_buffer);
            const uint16x8_t in1 = vld1q_u16(in16_bit_buffer + in_stride);

            vst1_u8(out8_bit_buffer, vshrn_n_u16(in0, 2));
            vst1_u8(out8_bit_buffer + out8_stride, vshrn_n_u16(in1, 2));

            if (outn_bit_buffer) {
                const uint16x8_t in0_u2 = vshlq_n_u16(in0, 6);
                const uint16x8_t in1_u2 = vshlq_n_u16(in1, 6);
                vst1_u8(outn_bit_buffer, vmovn_u16(in0_u2));
                vst1_u8(outn_bit_buffer + outn_stride, vmovn_u16(in1_u2));

                outn_bit_buffer += 2 * outn_stride;
            }

            out8_bit_buffer += 2 * out8_stride;
            in16_bit_buffer += 2 * in_stride;
            height -= 2;
        } while (height != 0);
    } else {
        do {
            int       w        = width;
            uint16_t* in16_ptr = in16_bit_buffer;
            uint8_t*  out8_ptr = out8_bit_buffer;
            uint8_t*  outn_ptr = outn_bit_buffer;

            while (w >= 16) {
                const uint16x8_t in0_lo = vld1q_u16(in16_ptr);
                const uint16x8_t in0_hi = vld1q_u16(in16_ptr + 8);
                const uint16x8_t in1_lo = vld1q_u16(in16_ptr + in_stride);
                const uint16x8_t in1_hi = vld1q_u16(in16_ptr + in_stride + 8);

                const uint8x16_t in0_u8 = vcombine_u8(vshrn_n_u16(in0_lo, 2), vshrn_n_u16(in0_hi, 2));
                const uint8x16_t in1_u8 = vcombine_u8(vshrn_n_u16(in1_lo, 2), vshrn_n_u16(in1_hi, 2));

                vst1q_u8(out8_ptr, in0_u8);
                vst1q_u8(out8_ptr + out8_stride, in1_u8);

                if (outn_bit_buffer) {
                    const uint16x8_t in0_lo_u2 = vshlq_n_u16(in0_lo, 6);
                    const uint16x8_t in0_hi_u2 = vshlq_n_u16(in0_hi, 6);
                    const uint16x8_t in1_lo_u2 = vshlq_n_u16(in1_lo, 6);
                    const uint16x8_t in1_hi_u2 = vshlq_n_u16(in1_hi, 6);
                    const uint8x16_t in0_u2    = vcombine_u8(vmovn_u16(in0_lo_u2), vmovn_u16(in0_hi_u2));
                    const uint8x16_t in1_u2    = vcombine_u8(vmovn_u16(in1_lo_u2), vmovn_u16(in1_hi_u2));

                    vst1q_u8(outn_ptr, in0_u2);
                    vst1q_u8(outn_ptr + outn_stride, in1_u2);

                    outn_ptr += 16;
                }

                in16_ptr += 16;
                out8_ptr += 16;
                w -= 16;
            }

            if (w >= 8) {
                const uint16x8_t in0 = vld1q_u16(in16_ptr);
                const uint16x8_t in1 = vld1q_u16(in16_ptr + in_stride);

                vst1_u8(out8_ptr, vshrn_n_u16(in0, 2));
                vst1_u8(out8_ptr + out8_stride, vshrn_n_u16(in1, 2));

                if (outn_bit_buffer) {
                    const uint16x8_t in0_u2 = vshlq_n_u16(in0, 6);
                    const uint16x8_t in1_u2 = vshlq_n_u16(in1, 6);
                    vst1_u8(outn_ptr, vmovn_u16(in0_u2));
                    vst1_u8(outn_ptr + outn_stride, vmovn_u16(in1_u2));

                    outn_ptr += 8;
                }

                out8_ptr += 8;
                in16_ptr += 8;
                w -= 8;
            }

            if (w >= 4) {
                assert(w == 4);
                const uint16x8_t in0 = load_u16_4x2(in16_ptr, in_stride);

                store_u8x4_strided_x2(out8_ptr, out8_stride, vshrn_n_u16(in0, 2));

                if (outn_bit_buffer) {
                    const uint16x8_t in0_u2 = vshlq_n_u16(in0, 6);
                    store_u8x4_strided_x2(outn_ptr, outn_stride, vmovn_u16(in0_u2));

                    outn_ptr += 4;
                }

                out8_ptr += 4;
                in16_ptr += 4;
                w -= 4;
            }

            in16_bit_buffer += 2 * in_stride;
            out8_bit_buffer += 2 * out8_stride;
            if (outn_bit_buffer) {
                outn_bit_buffer += 2 * outn_stride;
            }
            height -= 2;
        } while (height != 0);
    }
}
