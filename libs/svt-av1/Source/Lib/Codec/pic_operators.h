/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2019, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef EbPictureOperators_h
#define EbPictureOperators_h

#include "picture_operators_c.h"
#include "definitions.h"
#include "pic_buffer_desc.h"
#include "svt_log.h"
#include "common_dsp_rtcd.h"
#ifdef __cplusplus
extern "C" {
#endif
void svt_aom_picture_full_distortion32_bits_single(int32_t* coeff, int32_t* recon_coeff, uint32_t stride,
                                                   uint32_t bwidth, uint32_t bheight, uint64_t* distortion,
                                                   uint32_t cnt_nz_coeff);
//Residual Data

void svt_aom_compressed_pack_sb(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                                uint32_t inn_stride, uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t width,
                                uint32_t height);

void svt_aom_pack2d_src(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer, uint32_t inn_stride,
                        uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t width, uint32_t height);

void svt_aom_un_pack2d(uint16_t* in16_bit_buffer, uint32_t in_stride, uint8_t* out8_bit_buffer, uint32_t out8_stride,
                       uint8_t* outn_bit_buffer, uint32_t outn_stride, uint32_t width, uint32_t height);

static INLINE void memset16bit(uint16_t* in_ptr, uint16_t value, uint64_t num_of_elements) {
    uint64_t i;

    for (i = 0; i < num_of_elements; i++) {
        in_ptr[i] = value;
    }
}

static INLINE void memset32bit(uint32_t* in_ptr, uint32_t value, uint64_t num_of_elements) {
    uint64_t i;

    for (i = 0; i < num_of_elements; i++) {
        in_ptr[i] = value;
    }
}

void svt_full_distortion_kernel_cbf_zero32_bits_c(int32_t* coeff, uint32_t coeff_stride,
                                                  uint64_t distortion_result[DIST_CALC_TOTAL], uint32_t area_width,
                                                  uint32_t area_height);

void svt_full_distortion_kernel32_bits_c(int32_t* coeff, uint32_t coeff_stride, int32_t* recon_coeff,
                                         uint32_t recon_coeff_stride, uint64_t distortion_result[DIST_CALC_TOTAL],
                                         uint32_t area_width, uint32_t area_height);

uint64_t svt_full_distortion_kernel16_bits_c(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                             uint8_t* pred, int32_t pred_offset, uint32_t pred_stride,
                                             uint32_t area_width, uint32_t area_height);

void svt_residual_kernel16bit_c(uint16_t* input, uint32_t input_stride, uint16_t* pred, uint32_t pred_stride,
                                int16_t* residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height);

void svt_residual_kernel8bit_c(uint8_t* input, uint32_t input_stride, uint8_t* pred, uint32_t pred_stride,
                               int16_t* residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height);

void svt_aom_generate_padding(EbByte src_pic, uint32_t src_stride, uint32_t original_src_width,
                              uint32_t original_src_height, uint32_t padding_width, uint32_t padding_height);

void svt_aom_generate_padding_compressed_10bit(
    EbByte   src_pic, //output paramter, pointer to the source picture to be padded.
    uint32_t src_stride, //input paramter, the stride of the source picture to be padded.
    uint32_t original_src_width, //input paramter, the width of the source picture which excludes the padding.
    uint32_t original_src_height, //input paramter, the height of the source picture which excludes the padding.
    uint32_t padding_width, //input paramter, the padding width.
    uint32_t padding_height); //input paramter, the padding height.

void svt_aom_generate_padding16_bit(uint16_t* src_pic, uint32_t src_stride, uint32_t original_src_width,
                                    uint32_t original_src_height, uint32_t padding_width, uint32_t padding_height);

void pad_input_picture(EbByte src_pic, uint32_t src_stride, uint32_t original_src_width, uint32_t original_src_height,
                       uint32_t pad_right, uint32_t pad_bottom);

void svt_aom_pad_input_picture_16bit(uint16_t* src_pic, uint32_t src_stride, uint32_t original_src_width,
                                     uint32_t original_src_height, uint32_t pad_right, uint32_t pad_bottom);

void svt_aom_pack_2d_pic(EbPictureBufferDesc* input_picture, uint16_t* packed[3]);
void svt_aom_convert_pic_8bit_to_16bit(EbPictureBufferDesc* src_8bit, EbPictureBufferDesc* dst_16bit, uint16_t ss_x,
                                       uint16_t ss_y);
void svt_aom_copy_buffer_info(EbPictureBufferDesc* src_ptr, EbPictureBufferDesc* dst_ptr);
void svt_aom_yv12_copy_y_c(const Yv12BufferConfig* src_ybc, Yv12BufferConfig* dst_ybc);
void svt_aom_yv12_copy_u_c(const Yv12BufferConfig* src_bc, Yv12BufferConfig* dst_bc);
void svt_aom_yv12_copy_v_c(const Yv12BufferConfig* src_bc, Yv12BufferConfig* dst_bc);
void svt_aom_pack_highbd_pic(const EbPictureBufferDesc* pic_ptr, uint16_t* buffer_16bit[3], uint32_t ss_x,
                             uint32_t ss_y, bool include_padding);
void svt_aom_unpack_highbd_pic(uint16_t* buffer_highbd[3], EbPictureBufferDesc* pic_ptr, uint32_t ss_x, uint32_t ss_y,
                               bool include_padding);

static inline void svt_av1_picture_copy_y(EbPictureBufferDesc* src, uint32_t src_origin_index, EbPictureBufferDesc* dst,
                                          uint32_t dst_origin_index, uint32_t area_width, uint32_t area_height,
                                          bool hbd) {
    if (hbd) {
        svt_av1_copy_wxh_16bit((uint16_t*)src->buffer_y + src_origin_index,
                               src->stride_y,
                               (uint16_t*)dst->buffer_y + dst_origin_index,
                               dst->stride_y,
                               area_height,
                               area_width);
    } else {
        svt_av1_copy_wxh_8bit(src->buffer_y + src_origin_index,
                              src->stride_y,
                              dst->buffer_y + dst_origin_index,
                              dst->stride_y,
                              area_height,
                              area_width);
    }
}

static inline void svt_av1_picture_copy_cb(EbPictureBufferDesc* src, uint32_t src_origin_index,
                                           EbPictureBufferDesc* dst, uint32_t dst_origin_index, uint32_t area_width,
                                           uint32_t area_height, bool hbd) {
    if (hbd) {
        svt_av1_copy_wxh_16bit((uint16_t*)src->buffer_cb + src_origin_index,
                               src->stride_cb,
                               (uint16_t*)dst->buffer_cb + dst_origin_index,
                               dst->stride_cb,
                               area_height,
                               area_width);
    } else {
        svt_av1_copy_wxh_8bit(src->buffer_cb + src_origin_index,
                              src->stride_cb,
                              dst->buffer_cb + dst_origin_index,
                              dst->stride_cb,
                              area_height,
                              area_width);
    }
}

static inline void svt_av1_picture_copy_cr(EbPictureBufferDesc* src, uint32_t src_origin_index,
                                           EbPictureBufferDesc* dst, uint32_t dst_origin_index, uint32_t area_width,
                                           uint32_t area_height, bool hbd) {
    if (hbd) {
        svt_av1_copy_wxh_16bit((uint16_t*)src->buffer_cr + src_origin_index,
                               src->stride_cr,
                               (uint16_t*)dst->buffer_cr + dst_origin_index,
                               dst->stride_cr,
                               area_height,
                               area_width);
    } else {
        svt_av1_copy_wxh_8bit(src->buffer_cr + src_origin_index,
                              src->stride_cr,
                              dst->buffer_cr + dst_origin_index,
                              dst->stride_cr,
                              area_height,
                              area_width);
    }
}

#ifdef __cplusplus
}
#endif
#endif // EbPictureOperators_h
