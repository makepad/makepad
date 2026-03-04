/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

/*********************************
 * Includes
 *********************************/

#include "pack_unpack_c.h"
#include "utility.h"
#include "intra_prediction.h"
#include "aom_dsp_rtcd.h"
#include "pic_operators.h"

/*******************************************
* Residual Kernel 16bit
Computes the residual data
*******************************************/
void svt_residual_kernel16bit_c(uint16_t* input, uint32_t input_stride, uint16_t* pred, uint32_t pred_stride,
                                int16_t* residual, uint32_t residual_stride, uint32_t area_width,
                                uint32_t area_height) {
    uint32_t row_index = 0;

    while (row_index < area_height) {
        uint32_t column_index = 0;
        while (column_index < area_width) {
            residual[column_index] = ((int16_t)input[column_index]) - ((int16_t)pred[column_index]);
            ++column_index;
        }

        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
        ++row_index;
    }

    return;
}

/*******************************************
* Residual Kernel
Computes the residual data
*******************************************/
void svt_residual_kernel8bit_c(uint8_t* input, uint32_t input_stride, uint8_t* pred, uint32_t pred_stride,
                               int16_t* residual, uint32_t residual_stride, uint32_t area_width, uint32_t area_height) {
    uint32_t row_index = 0;

    while (row_index < area_height) {
        uint32_t column_index = 0;
        while (column_index < area_width) {
            residual[column_index] = ((int16_t)input[column_index]) - ((int16_t)pred[column_index]);
            ++column_index;
        }

        input += input_stride;
        pred += pred_stride;
        residual += residual_stride;
        ++row_index;
    }

    return;
}

/*******************************************
* Picture Full Distortion
*  Used in the Full Mode Decision Loop for the only case of a MVP-SKIP candidate
*******************************************/

void svt_full_distortion_kernel32_bits_c(int32_t* coeff, uint32_t coeff_stride, int32_t* recon_coeff,
                                         uint32_t recon_coeff_stride, uint64_t distortion_result[DIST_CALC_TOTAL],
                                         uint32_t area_width, uint32_t area_height) {
    uint32_t row_index             = 0;
    uint64_t residual_distortion   = 0;
    uint64_t prediction_distortion = 0;

    while (row_index < area_height) {
        uint32_t column_index = 0;
        while (column_index < area_width) {
            residual_distortion += (int64_t)SQR((int64_t)(coeff[column_index]) - (recon_coeff[column_index]));
            prediction_distortion += (int64_t)SQR((int64_t)(coeff[column_index]));
            ++column_index;
        }

        coeff += coeff_stride;
        recon_coeff += recon_coeff_stride;
        ++row_index;
    }

    distortion_result[DIST_CALC_RESIDUAL]   = residual_distortion;
    distortion_result[DIST_CALC_PREDICTION] = prediction_distortion;
}

uint64_t svt_full_distortion_kernel16_bits_c(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                             uint8_t* pred, int32_t pred_offset, uint32_t pred_stride,
                                             uint32_t area_width, uint32_t area_height) {
    uint32_t row_index      = 0;
    uint64_t sse_distortion = 0;

    uint16_t* input_16bit = (uint16_t*)input;
    uint16_t* pred_16bit  = (uint16_t*)pred;
    input_16bit += input_offset;
    pred_16bit += pred_offset;

    while (row_index < area_height) {
        uint32_t column_index = 0;
        while (column_index < area_width) {
            sse_distortion += (int64_t)SQR((int64_t)(input_16bit[column_index]) - (pred_16bit[column_index]));
            ++column_index;
        }
        input_16bit += input_stride;
        pred_16bit += pred_stride;
        ++row_index;
    }

    return sse_distortion;
}

/*******************************************
* Picture Distortion Full Kernel CbfZero
*******************************************/
void svt_full_distortion_kernel_cbf_zero32_bits_c(int32_t* coeff, uint32_t coeff_stride,
                                                  uint64_t distortion_result[DIST_CALC_TOTAL], uint32_t area_width,
                                                  uint32_t area_height) {
    uint32_t row_index             = 0;
    uint64_t prediction_distortion = 0;

    while (row_index < area_height) {
        uint32_t column_index = 0;
        while (column_index < area_width) {
            prediction_distortion += (int64_t)SQR((int64_t)(coeff[column_index]));
            ++column_index;
        }

        coeff += coeff_stride;
        ++row_index;
    }

    distortion_result[DIST_CALC_RESIDUAL]   = prediction_distortion;
    distortion_result[DIST_CALC_PREDICTION] = prediction_distortion;
}

void svt_aom_picture_full_distortion32_bits_single(int32_t* coeff, int32_t* recon_coeff, uint32_t stride,
                                                   uint32_t bwidth, uint32_t bheight, uint64_t* distortion,
                                                   uint32_t cnt_nz_coeff) {
    distortion[0] = 0;
    distortion[1] = 0;

    if (cnt_nz_coeff) {
        svt_full_distortion_kernel32_bits(coeff, stride, recon_coeff, stride, distortion, bwidth, bheight);
    } else {
        svt_full_distortion_kernel_cbf_zero32_bits(coeff, stride, distortion, bwidth, bheight);
    }
}

void svt_aom_un_pack2d(uint16_t* in16_bit_buffer, uint32_t in_stride, uint8_t* out8_bit_buffer, uint32_t out8_stride,
                       uint8_t* outn_bit_buffer, uint32_t outn_stride, uint32_t width, uint32_t height) {
    if (((width & 3) == 0) && ((height & 1) == 0)) {
        svt_aom_un_pack2d_16_bit_src_mul4(
            in16_bit_buffer, in_stride, out8_bit_buffer, outn_bit_buffer, out8_stride, outn_stride, width, height);
    } else {
        svt_enc_msb_un_pack2_d(
            in16_bit_buffer, in_stride, out8_bit_buffer, outn_bit_buffer, out8_stride, outn_stride, width, height);
    }
}

void svt_aom_pack2d_src(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer, uint32_t inn_stride,
                        uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t width, uint32_t height) {
    if (((width & 3) == 0) && ((height & 1) == 0)) {
        svt_pack2d_16_bit_src_mul4(
            in8_bit_buffer, in8_stride, inn_bit_buffer, out16_bit_buffer, inn_stride, out_stride, width, height);
    } else {
        svt_enc_msb_pack2_d(
            in8_bit_buffer, in8_stride, inn_bit_buffer, out16_bit_buffer, inn_stride, out_stride, width, height);
    }
}

void svt_aom_compressed_pack_sb(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                                uint32_t inn_stride, uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t width,
                                uint32_t height) {
    svt_compressed_packmsb(
        in8_bit_buffer, in8_stride, inn_bit_buffer, inn_stride, out16_bit_buffer, out_stride, width, height);
}

// Copies the source image into the destination image and updates the
// destination's UMV borders.
// Note: The frames are assumed to be identical in size.
void svt_aom_yv12_copy_y_c(const Yv12BufferConfig* src_ybc, Yv12BufferConfig* dst_ybc) {
    int32_t        row;
    const uint8_t* src = src_ybc->y_buffer;
    uint8_t*       dst = dst_ybc->y_buffer;

    if (src_ybc->flags & YV12_FLAG_HIGHBITDEPTH) {
        const uint16_t* src16 = CONVERT_TO_SHORTPTR(src);
        uint16_t*       dst16 = CONVERT_TO_SHORTPTR(dst);
        for (row = 0; row < src_ybc->y_height; ++row) {
            svt_memcpy(dst16, src16, src_ybc->y_width * sizeof(uint16_t));
            src16 += src_ybc->y_stride;
            dst16 += dst_ybc->y_stride;
        }
        return;
    }

    for (row = 0; row < src_ybc->y_height; ++row) {
        svt_memcpy(dst, src, src_ybc->y_width);
        src += src_ybc->y_stride;
        dst += dst_ybc->y_stride;
    }
}

void svt_aom_yv12_copy_u_c(const Yv12BufferConfig* src_bc, Yv12BufferConfig* dst_bc) {
    int32_t        row;
    const uint8_t* src = src_bc->u_buffer;
    uint8_t*       dst = dst_bc->u_buffer;

    if (src_bc->flags & YV12_FLAG_HIGHBITDEPTH) {
        const uint16_t* src16 = CONVERT_TO_SHORTPTR(src);
        uint16_t*       dst16 = CONVERT_TO_SHORTPTR(dst);
        for (row = 0; row < src_bc->uv_height; ++row) {
            svt_memcpy(dst16, src16, src_bc->uv_width * sizeof(uint16_t));
            src16 += src_bc->uv_stride;
            dst16 += dst_bc->uv_stride;
        }
        return;
    }

    for (row = 0; row < src_bc->uv_height; ++row) {
        svt_memcpy(dst, src, src_bc->uv_width);
        src += src_bc->uv_stride;
        dst += dst_bc->uv_stride;
    }
}

void svt_aom_yv12_copy_v_c(const Yv12BufferConfig* src_bc, Yv12BufferConfig* dst_bc) {
    int32_t        row;
    const uint8_t* src = src_bc->v_buffer;
    uint8_t*       dst = dst_bc->v_buffer;

    if (src_bc->flags & YV12_FLAG_HIGHBITDEPTH) {
        const uint16_t* src16 = CONVERT_TO_SHORTPTR(src);
        uint16_t*       dst16 = CONVERT_TO_SHORTPTR(dst);
        for (row = 0; row < src_bc->uv_height; ++row) {
            svt_memcpy(dst16, src16, src_bc->uv_width * sizeof(uint16_t));
            src16 += src_bc->uv_stride;
            dst16 += dst_bc->uv_stride;
        }
        return;
    }

    for (row = 0; row < src_bc->uv_height; ++row) {
        svt_memcpy(dst, src, src_bc->uv_width);
        src += src_bc->uv_stride;
        dst += dst_bc->uv_stride;
    }
}

/** svt_aom_generate_padding()
        is used to pad the target picture. The horizontal padding happens first and then the vertical padding.
 */
void svt_aom_generate_padding(
    EbByte   src_pic, //output paramter, pointer to the source picture to be padded.
    uint32_t src_stride, //input paramter, the stride of the source picture to be padded.
    uint32_t original_src_width, //input paramter, the width of the source picture which excludes the padding.
    uint32_t original_src_height, //input paramter, the height of the source picture which excludes the padding.
    uint32_t padding_width, //input paramter, the padding width.
    uint32_t padding_height) //input paramter, the padding height.
{
    uint32_t vertical_idx = original_src_height;
    EbByte   temp_src_pic0;
    EbByte   temp_src_pic1;
    EbByte   temp_src_pic2;
    EbByte   temp_src_pic3;

    if (!src_pic) {
        SVT_ERROR("padding NULL pointers\n");
        return;
    }

    temp_src_pic0 = src_pic + padding_width + padding_height * src_stride;
    while (vertical_idx) {
        // horizontal padding
        svt_memset(temp_src_pic0 - padding_width, *temp_src_pic0, padding_width);
        svt_memset(temp_src_pic0 + original_src_width, *(temp_src_pic0 + original_src_width - 1), padding_width);

        temp_src_pic0 += src_stride;
        --vertical_idx;
    }

    // vertical padding
    vertical_idx  = padding_height;
    temp_src_pic0 = src_pic + padding_height * src_stride;
    temp_src_pic1 = src_pic + (padding_height + original_src_height - 1) * src_stride;
    temp_src_pic2 = temp_src_pic0;
    temp_src_pic3 = temp_src_pic1;
    while (vertical_idx) {
        // top part data copy
        temp_src_pic2 -= src_stride;
        svt_memcpy(temp_src_pic2, temp_src_pic0, sizeof(uint8_t) * src_stride); // uint8_t to be modified
        // bottom part data copy
        temp_src_pic3 += src_stride;
        svt_memcpy(temp_src_pic3, temp_src_pic1, sizeof(uint8_t) * src_stride); // uint8_t to be modified
        --vertical_idx;
    }

    return;
}

void svt_aom_generate_padding_compressed_10bit(
    EbByte   src_pic, //output paramter, pointer to the source picture to be padded.
    uint32_t src_stride, //input paramter, the stride of the source picture to be padded.
    uint32_t original_src_width, //input paramter, the width of the source picture which excludes the padding.
    uint32_t original_src_height, //input paramter, the height of the source picture which excludes the padding.
    uint32_t padding_width, //input paramter, the padding width.
    uint32_t padding_height) //input paramter, the padding height.
{
    if (!src_pic) {
        SVT_ERROR("padding NULL pointers\n");
        return;
    }
    EbByte temp_src_pic0 = src_pic + padding_width / 4 + padding_height * src_stride;

    for (uint32_t row = 0; row < original_src_height; row++) {
        const uint8_t left_pixel  = (temp_src_pic0[0] >> 6) & 0x03;
        const uint8_t right_pixel = temp_src_pic0[original_src_width / 4 - 1] & 0x03;

        EB_MEMSET(temp_src_pic0 - padding_width / 4, left_pixel * 0b01010101, padding_width / 4);
        EB_MEMSET(temp_src_pic0 + original_src_width / 4, right_pixel * 0b01010101, padding_width / 4);

        temp_src_pic0 += src_stride;
    }

    // vertical padding
    uint32_t vertical_idx = padding_height;
    temp_src_pic0         = src_pic + padding_height * src_stride;
    EbByte temp_src_pic1  = src_pic + (padding_height + original_src_height - 1) * src_stride;
    EbByte temp_src_pic2  = temp_src_pic0;
    EbByte temp_src_pic3  = temp_src_pic1;
    while (vertical_idx) {
        // top part data copy
        temp_src_pic2 -= src_stride;
        svt_memcpy(temp_src_pic2, temp_src_pic0, sizeof(uint8_t) * src_stride); // uint8_t to be modified
        // bottom part data copy
        temp_src_pic3 += src_stride;
        svt_memcpy(temp_src_pic3, temp_src_pic1, sizeof(uint8_t) * src_stride); // uint8_t to be modified
        --vertical_idx;
    }
}

/** svt_aom_generate_padding16_bit()
is used to pad the target picture. The horizontal padding happens first and then the vertical padding.
*/
// TODO: svt_aom_generate_padding() and generate_padding16() functions are not aligned, inputs according to comments are wrong
void svt_aom_generate_padding16_bit(
    uint16_t* src_pic, //output paramter, pointer to the source picture to be padded.
    uint32_t  src_stride, //input paramter, the stride of the source picture to be padded.
    uint32_t  original_src_width, //input paramter, the width of the source picture which excludes the padding.
    uint32_t  original_src_height, //input paramter, the height of the source picture which excludes the padding.
    uint32_t  padding_width, //input paramter, the padding width.
    uint32_t  padding_height) //input paramter, the padding height.
{
    uint32_t  vertical_idx = original_src_height;
    uint16_t* temp_src_pic0;
    uint16_t* temp_src_pic1;
    uint16_t* temp_src_pic2;
    uint16_t* temp_src_pic3;

    temp_src_pic0 = src_pic + padding_width + padding_height * src_stride;
    while (vertical_idx) {
        // horizontal padding
        //EB_MEMSET(temp_src_pic0 - padding_width, temp_src_pic0, padding_width);
        memset16bit(temp_src_pic0 - padding_width, temp_src_pic0[0], padding_width);
        memset16bit(temp_src_pic0 + original_src_width, (temp_src_pic0 + original_src_width - 1)[0], padding_width);

        temp_src_pic0 += src_stride;
        --vertical_idx;
    }

    // vertical padding
    vertical_idx  = padding_height;
    temp_src_pic0 = src_pic + padding_height * src_stride;
    temp_src_pic1 = src_pic + (padding_height + original_src_height - 1) * src_stride;
    temp_src_pic2 = temp_src_pic0;
    temp_src_pic3 = temp_src_pic1;
    while (vertical_idx) {
        // top part data copy
        temp_src_pic2 -= src_stride;
        svt_memcpy(temp_src_pic2, temp_src_pic0, sizeof(uint16_t) * src_stride);
        // bottom part data copy
        temp_src_pic3 += src_stride;
        svt_memcpy(temp_src_pic3, temp_src_pic1, sizeof(uint16_t) * src_stride);
        --vertical_idx;
    }

    return;
}

/** pad_input_picture()
is used to pad the input picture in order to get . The horizontal padding happens first and then the vertical padding.
*/
void pad_input_picture(
    EbByte   src_pic, //output paramter, pointer to the source picture to be padded.
    uint32_t src_stride, //input paramter, the stride of the source picture to be padded.
    uint32_t original_src_width, //input paramter, the width of the source picture which excludes the padding.
    uint32_t original_src_height, //input paramter, the height of the source picture which excludes the padding.
    uint32_t pad_right, //input paramter, the padding right.
    uint32_t pad_bottom) //input paramter, the padding bottom.
{
    uint32_t vertical_idx;
    EbByte   temp_src_pic0;

    if (!src_pic) {
        SVT_ERROR("padding NULL pointers\n");
        return;
    }

    if (pad_right) {
        // Add padding @ the right
        vertical_idx  = original_src_height;
        temp_src_pic0 = src_pic;

        while (vertical_idx) {
            svt_memset(temp_src_pic0 + original_src_width, *(temp_src_pic0 + original_src_width - 1), pad_right);
            temp_src_pic0 += src_stride;
            --vertical_idx;
        }
    }

    if (pad_bottom) {
        EbByte temp_src_pic1;
        // Add padding @ the bottom
        vertical_idx  = pad_bottom;
        temp_src_pic0 = src_pic + (original_src_height - 1) * src_stride;
        temp_src_pic1 = temp_src_pic0;

        while (vertical_idx) {
            temp_src_pic1 += src_stride;
            svt_memcpy(temp_src_pic1, temp_src_pic0, sizeof(uint8_t) * (original_src_width + pad_right));
            --vertical_idx;
        }
    }

    return;
}

/** svt_aom_pad_input_picture_16bit()
is used to pad the input picture in order to get . The horizontal padding happens first and then the vertical padding.
*/
void svt_aom_pad_input_picture_16bit(
    uint16_t* src_pic, //output paramter, pointer to the source picture to be padded.
    uint32_t  src_stride, //input paramter, the stride of the source picture to be padded.
    uint32_t  original_src_width, //input paramter, the width of the source picture which excludes the padding.
    uint32_t  original_src_height, //input paramter, the height of the source picture which excludes the padding.
    uint32_t  pad_right, //input paramter, the padding right.
    uint32_t  pad_bottom) //input paramter, the padding bottom.
{
    uint32_t  vertical_idx;
    uint16_t* temp_src_pic0;

    if (pad_right) {
        // Add padding @ the right
        vertical_idx  = original_src_height;
        temp_src_pic0 = src_pic;

        while (vertical_idx) {
            memset16bit(temp_src_pic0 + original_src_width, *(temp_src_pic0 + original_src_width - 1), pad_right);
            temp_src_pic0 += src_stride;
            --vertical_idx;
        }
    }

    if (pad_bottom) {
        uint16_t* temp_src_pic1;
        // Add padding @ the bottom
        vertical_idx  = pad_bottom;
        temp_src_pic0 = (uint16_t*)(src_pic + (original_src_height - 1) * src_stride);
        temp_src_pic1 = temp_src_pic0;

        while (vertical_idx) {
            temp_src_pic1 += src_stride;
            svt_memcpy(temp_src_pic1, temp_src_pic0, sizeof(uint16_t) * (original_src_width + pad_right));
            --vertical_idx;
        }
    }

    return;
}

void svt_aom_pack_2d_pic(EbPictureBufferDesc* input_picture, uint16_t* packed[3]) {
    const uint32_t input_luma_offset = ((input_picture->org_y) * input_picture->stride_y) + (input_picture->org_x);
    const uint32_t input_bit_inc_luma_offset = ((input_picture->org_y) * input_picture->stride_bit_inc_y >> 2) +
        (input_picture->org_x >> 2);
    const uint32_t input_cb_offset = (((input_picture->org_y) >> 1) * input_picture->stride_cb) +
        ((input_picture->org_x) >> 1);
    const uint32_t input_bit_inc_cb_offset = (((input_picture->org_y) >> 1) * input_picture->stride_bit_inc_cb >> 2) +
        ((input_picture->org_x >> 2) >> 1);
    const uint32_t input_cr_offset = (((input_picture->org_y) >> 1) * input_picture->stride_cr) +
        ((input_picture->org_x) >> 1);
    const uint32_t input_bit_inc_cr_offset = (((input_picture->org_y) >> 1) * input_picture->stride_bit_inc_cr >> 2) +
        ((input_picture->org_x >> 2) >> 1);

    svt_aom_compressed_pack_sb(input_picture->buffer_y + input_luma_offset,
                               input_picture->stride_y,
                               input_picture->buffer_bit_inc_y + input_bit_inc_luma_offset,
                               input_picture->stride_bit_inc_y >> 2,
                               (uint16_t*)packed[0],
                               input_picture->stride_y,
                               input_picture->width,
                               input_picture->height);

    svt_aom_compressed_pack_sb(input_picture->buffer_cb + input_cb_offset,
                               input_picture->stride_cr,
                               input_picture->buffer_bit_inc_cb + input_bit_inc_cb_offset,
                               input_picture->stride_bit_inc_cr >> 2,
                               (uint16_t*)packed[1],
                               input_picture->stride_cr,
                               input_picture->width >> 1,
                               input_picture->height >> 1);

    svt_aom_compressed_pack_sb(input_picture->buffer_cr + input_cr_offset,
                               input_picture->stride_cr,
                               input_picture->buffer_bit_inc_cr + input_bit_inc_cr_offset,
                               input_picture->stride_bit_inc_cr >> 2,
                               (uint16_t*)packed[2],
                               input_picture->stride_cr,
                               input_picture->width >> 1,
                               input_picture->height >> 1);
}

void svt_aom_convert_pic_8bit_to_16bit(EbPictureBufferDesc* src_8bit, EbPictureBufferDesc* dst_16bit, uint16_t ss_x,
                                       uint16_t ss_y) {
    //copy input from 8bit to 16bit
    uint8_t*  buffer_8bit;
    int32_t   stride_8bit;
    uint16_t* buffer_16bit;
    int32_t   stride_16bit;
    // Y
    buffer_16bit = (uint16_t*)(dst_16bit->buffer_y) + dst_16bit->org_x + dst_16bit->org_y * dst_16bit->stride_y;
    stride_16bit = dst_16bit->stride_y;
    buffer_8bit  = src_8bit->buffer_y + src_8bit->org_x + src_8bit->org_y * src_8bit->stride_y;
    stride_8bit  = src_8bit->stride_y;

    svt_convert_8bit_to_16bit(buffer_8bit, stride_8bit, buffer_16bit, stride_16bit, src_8bit->width, src_8bit->height);

    // Cb
    buffer_16bit = (uint16_t*)(dst_16bit->buffer_cb) + (dst_16bit->org_x >> ss_x) +
        (dst_16bit->org_y >> ss_y) * dst_16bit->stride_cb;
    stride_16bit = dst_16bit->stride_cb;
    buffer_8bit  = src_8bit->buffer_cb + (src_8bit->org_x >> ss_x) + (src_8bit->org_y >> ss_y) * src_8bit->stride_cb;
    stride_8bit  = src_8bit->stride_cb;

    svt_convert_8bit_to_16bit(
        buffer_8bit, stride_8bit, buffer_16bit, stride_16bit, src_8bit->width >> ss_x, src_8bit->height >> ss_y);

    // Cr
    buffer_16bit = (uint16_t*)(dst_16bit->buffer_cr) + (dst_16bit->org_x >> ss_x) +
        (dst_16bit->org_y >> ss_y) * dst_16bit->stride_cr;
    stride_16bit = dst_16bit->stride_cr;
    buffer_8bit  = src_8bit->buffer_cr + (src_8bit->org_x >> ss_x) + (src_8bit->org_y >> ss_y) * src_8bit->stride_cr;
    stride_8bit  = src_8bit->stride_cr;

    svt_convert_8bit_to_16bit(
        buffer_8bit, stride_8bit, buffer_16bit, stride_16bit, src_8bit->width >> ss_x, src_8bit->height >> ss_y);

    dst_16bit->width  = src_8bit->width;
    dst_16bit->height = src_8bit->height;
}

void svt_aom_copy_buffer_info(EbPictureBufferDesc* src_ptr, EbPictureBufferDesc* dst_ptr) {
    dst_ptr->width             = src_ptr->width;
    dst_ptr->height            = src_ptr->height;
    dst_ptr->max_width         = src_ptr->max_width;
    dst_ptr->max_height        = src_ptr->max_height;
    dst_ptr->stride_y          = src_ptr->stride_y;
    dst_ptr->stride_cb         = src_ptr->stride_cb;
    dst_ptr->stride_cr         = src_ptr->stride_cr;
    dst_ptr->org_x             = src_ptr->org_x;
    dst_ptr->origin_bot_y      = src_ptr->origin_bot_y;
    dst_ptr->org_y             = src_ptr->org_y;
    dst_ptr->stride_bit_inc_y  = src_ptr->stride_bit_inc_y;
    dst_ptr->stride_bit_inc_cb = src_ptr->stride_bit_inc_cb;
    dst_ptr->stride_bit_inc_cr = src_ptr->stride_bit_inc_cr;
    dst_ptr->luma_size         = src_ptr->luma_size;
    dst_ptr->chroma_size       = src_ptr->chroma_size;
}

void svt_aom_pack_highbd_pic(const EbPictureBufferDesc* pic_ptr, uint16_t* buffer_16bit[3], uint32_t ss_x,
                             uint32_t ss_y, bool include_padding) {
    uint16_t width  = pic_ptr->stride_y;
    uint16_t height = (uint16_t)(pic_ptr->org_y * 2 + pic_ptr->height);

    svt_aom_assert_err(include_padding == 1, "not supporting OFF");

    uint32_t comp_stride_y = pic_ptr->stride_y / 4;

    svt_aom_compressed_pack_sb(pic_ptr->buffer_y,
                               pic_ptr->stride_y,
                               pic_ptr->buffer_bit_inc_y,
                               comp_stride_y,
                               buffer_16bit[0 /*Y*/],
                               pic_ptr->stride_y,
                               width,
                               height);

    uint32_t comp_stride_uv = pic_ptr->stride_cb / 4;
    if (buffer_16bit[1 /*U*/]) {
        svt_aom_compressed_pack_sb(pic_ptr->buffer_cb,
                                   pic_ptr->stride_cb,
                                   pic_ptr->buffer_bit_inc_cb,
                                   comp_stride_uv,
                                   buffer_16bit[1 /*U*/],
                                   pic_ptr->stride_cb,
                                   (width + ss_x) >> ss_x,
                                   (height + ss_y) >> ss_y);
    }
    if (buffer_16bit[2 /*V*/]) {
        svt_aom_compressed_pack_sb(pic_ptr->buffer_cr,
                                   pic_ptr->stride_cr,
                                   pic_ptr->buffer_bit_inc_cr,
                                   comp_stride_uv,
                                   buffer_16bit[2 /*V*/],
                                   pic_ptr->stride_cr,
                                   (width + ss_x) >> ss_x,
                                   (height + ss_y) >> ss_y);
    }
}

void svt_aom_unpack_highbd_pic(uint16_t* buffer_highbd[3], EbPictureBufferDesc* pic_ptr, uint32_t ss_x, uint32_t ss_y,
                               bool include_padding) {
    uint16_t width  = pic_ptr->stride_y;
    uint16_t height = (uint16_t)(pic_ptr->org_y * 2 + pic_ptr->height);

    svt_aom_assert_err(include_padding == 1, "not supporting OFF");

    uint32_t comp_stride_y  = pic_ptr->stride_y / 4;
    uint32_t comp_stride_uv = pic_ptr->stride_cb / 4;

    svt_unpack_and_2bcompress(buffer_highbd[0 /*Y*/],
                              pic_ptr->stride_y,
                              pic_ptr->buffer_y,
                              pic_ptr->stride_y,
                              pic_ptr->buffer_bit_inc_y,
                              comp_stride_y,
                              width,
                              height);

    if (buffer_highbd[1 /*U*/]) {
        svt_unpack_and_2bcompress(buffer_highbd[1 /*U*/],
                                  pic_ptr->stride_cb,
                                  pic_ptr->buffer_cb,
                                  pic_ptr->stride_cb,
                                  pic_ptr->buffer_bit_inc_cb,
                                  comp_stride_uv,
                                  (width + ss_x) >> ss_x,
                                  (height + ss_y) >> ss_y);
    }

    if (buffer_highbd[2 /*V*/]) {
        svt_unpack_and_2bcompress(buffer_highbd[2 /*V*/],
                                  pic_ptr->stride_cr,
                                  pic_ptr->buffer_cr,
                                  pic_ptr->stride_cr,
                                  pic_ptr->buffer_bit_inc_cr,
                                  comp_stride_uv,
                                  (width + ss_x) >> ss_x,
                                  (height + ss_y) >> ss_y);
    }
}
