/*
* Copyright(c) 2019 Intel Corporation
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef pack_unpack_c_h
#define pack_unpack_c_h
#ifdef __cplusplus
extern "C" {
#endif

#include "definitions.h"

void svt_unpack_and_2bcompress_c(uint16_t* in16b_buffer, uint32_t in16b_stride, uint8_t* out8b_buffer,
                                 uint32_t out8b_stride, uint8_t* out2b_buffer, uint32_t out2b_stride, uint32_t width,
                                 uint32_t height);

void svt_enc_msb_pack2_d(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                         uint16_t* out16_bit_buffer, uint32_t inn_stride, uint32_t out_stride, uint32_t width,
                         uint32_t height);

void svt_compressed_packmsb_c(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                              uint32_t inn_stride, uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t width,
                              uint32_t height);

void svt_enc_msb_un_pack2_d(uint16_t* in16_bit_buffer, uint32_t in_stride, uint8_t* out8_bit_buffer,
                            uint8_t* outn_bit_buffer, uint32_t out8_stride, uint32_t outn_stride, uint32_t width,
                            uint32_t height);

void svt_convert_8bit_to_16bit_c(uint8_t* src, uint32_t src_stride, uint16_t* dst, uint32_t dst_stride, uint32_t width,
                                 uint32_t height);

void svt_convert_16bit_to_8bit_c(uint16_t* src, uint32_t src_stride, uint8_t* dst, uint32_t dst_stride, uint32_t width,
                                 uint32_t height);
void svt_c_unpack_compressed_10bit(const uint8_t* inn_bit_buffer, uint32_t inn_stride, uint8_t* in_compn_bit_buffer,
                                   uint32_t out_stride, uint32_t height);
#ifdef __cplusplus
}
#endif
#endif // pack_unpack_c_h
