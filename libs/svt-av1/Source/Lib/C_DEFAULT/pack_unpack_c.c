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

#include "pack_unpack_c.h"
#include "EbSvtAv1.h"

/************************************************
* pack 8 and 2 bit 2D data into 10 bit data
************************************************/
void svt_enc_msb_pack2_d(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                         uint16_t* out16_bit_buffer, uint32_t inn_stride, uint32_t out_stride, uint32_t width,
                         uint32_t height) {
    uint64_t j, k;
    uint16_t out_pixel;
    uint8_t  n_bit_pixel;

    //SIMD hint: use _mm_unpacklo_epi8 +_mm_unpackhi_epi8 to do the concatenation

    for (j = 0; j < height; j++) {
        for (k = 0; k < width; k++) {
            out_pixel   = (in8_bit_buffer[k + j * in8_stride]) << 2;
            n_bit_pixel = (inn_bit_buffer[k + j * inn_stride] >> 6) & 3;

            out16_bit_buffer[k + j * out_stride] = out_pixel | n_bit_pixel;
        }
    }
}

/************************************************
* pack 8 and 2 bit 2D data into 10 bit data
2bit data storage : 4 2bit-pixels in one byte
************************************************/
void svt_compressed_packmsb_c(uint8_t* in8_bit_buffer, uint32_t in8_stride, uint8_t* inn_bit_buffer,
                              uint32_t inn_stride, uint16_t* out16_bit_buffer, uint32_t out_stride, uint32_t width,
                              uint32_t height) {
    uint64_t row, k_idx;
    uint16_t out_pixel;
    uint8_t  n_bit_pixel;
    uint8_t  four_2bit_pels;

    for (row = 0; row < height; row++) {
        for (k_idx = 0; k_idx < width / 4; k_idx++) {
            four_2bit_pels = inn_bit_buffer[k_idx + row * inn_stride];

            n_bit_pixel = (four_2bit_pels >> 6) & 3;

            out_pixel                                          = in8_bit_buffer[k_idx * 4 + 0 + row * in8_stride] << 2;
            out16_bit_buffer[k_idx * 4 + 0 + row * out_stride] = out_pixel | n_bit_pixel;

            n_bit_pixel                                        = (four_2bit_pels >> 4) & 3;
            out_pixel                                          = in8_bit_buffer[k_idx * 4 + 1 + row * in8_stride] << 2;
            out16_bit_buffer[k_idx * 4 + 1 + row * out_stride] = out_pixel | n_bit_pixel;

            n_bit_pixel                                        = (four_2bit_pels >> 2) & 3;
            out_pixel                                          = in8_bit_buffer[k_idx * 4 + 2 + row * in8_stride] << 2;
            out16_bit_buffer[k_idx * 4 + 2 + row * out_stride] = out_pixel | n_bit_pixel;

            n_bit_pixel                                        = (four_2bit_pels >> 0) & 3;
            out_pixel                                          = in8_bit_buffer[k_idx * 4 + 3 + row * in8_stride] << 2;
            out16_bit_buffer[k_idx * 4 + 3 + row * out_stride] = out_pixel | n_bit_pixel;
        }
    }
}

/************************************************
* unpack  16bit  data to 8bit  + compressed-2bit
  2bit data storage : 4 2bit-pixels in one byte
  width is not necessarily multiple of 4
************************************************/
void svt_unpack_and_2bcompress_c(uint16_t* in16b_buffer, uint32_t in16b_stride, uint8_t* out8b_buffer,
                                 uint32_t out8b_stride, uint8_t* out2b_buffer, uint32_t out2b_stride, uint32_t width,
                                 uint32_t height) {
    uint32_t row, col;
    uint16_t in_pixel;
    uint8_t  tmp_pixel;

    uint32_t w_m4  = (width / 4) * 4;
    uint32_t w_rem = width - w_m4;

    for (row = 0; row < height; row++) {
        for (col = 0; col < w_m4; col += 4) {
            uint8_t compressed_unpacked_pixel = 0;

            //+0
            in_pixel                                   = in16b_buffer[col + 0 + row * in16b_stride];
            out8b_buffer[col + 0 + row * out8b_stride] = (uint8_t)(in_pixel >> 2);
            tmp_pixel                                  = (uint8_t)(in_pixel << 6);
            compressed_unpacked_pixel |= ((tmp_pixel >> 0) & 0xC0); //1100.0000

            //+1
            in_pixel                                   = in16b_buffer[col + 1 + row * in16b_stride];
            out8b_buffer[col + 1 + row * out8b_stride] = (uint8_t)(in_pixel >> 2);
            tmp_pixel                                  = (uint8_t)(in_pixel << 6);
            compressed_unpacked_pixel |= ((tmp_pixel >> 2) & 0x30); //0011.0000

            //+2
            in_pixel                                   = in16b_buffer[col + 2 + row * in16b_stride];
            out8b_buffer[col + 2 + row * out8b_stride] = (uint8_t)(in_pixel >> 2);
            tmp_pixel                                  = (uint8_t)(in_pixel << 6);
            compressed_unpacked_pixel |= ((tmp_pixel >> 4) & 0x0C); //0000.1100

            //+3
            in_pixel                                   = in16b_buffer[col + 3 + row * in16b_stride];
            out8b_buffer[col + 3 + row * out8b_stride] = (uint8_t)(in_pixel >> 2);
            tmp_pixel                                  = (uint8_t)(in_pixel << 6);
            compressed_unpacked_pixel |= ((tmp_pixel >> 6) & 0x03); //0000.0011

            out2b_buffer[col / 4 + row * out2b_stride] = compressed_unpacked_pixel;
        }

        //we can have up to 3 pixels remaining
        if (w_rem > 0) {
            uint8_t compressed_unpacked_pixel = 0;
            //+0
            in_pixel                                   = in16b_buffer[col + 0 + row * in16b_stride];
            out8b_buffer[col + 0 + row * out8b_stride] = (uint8_t)(in_pixel >> 2);
            tmp_pixel                                  = (uint8_t)(in_pixel << 6);
            compressed_unpacked_pixel |= ((tmp_pixel >> 0) & 0xC0); //1100.0000

            if (w_rem > 1) {
                //+1
                in_pixel                                   = in16b_buffer[col + 1 + row * in16b_stride];
                out8b_buffer[col + 1 + row * out8b_stride] = (uint8_t)(in_pixel >> 2);
                tmp_pixel                                  = (uint8_t)(in_pixel << 6);
                compressed_unpacked_pixel |= ((tmp_pixel >> 2) & 0x30); //0011.0000
            }
            if (w_rem > 2) {
                //+2
                in_pixel                                   = in16b_buffer[col + 2 + row * in16b_stride];
                out8b_buffer[col + 2 + row * out8b_stride] = (uint8_t)(in_pixel >> 2);
                tmp_pixel                                  = (uint8_t)(in_pixel << 6);
                compressed_unpacked_pixel |= ((tmp_pixel >> 4) & 0x0C); //0000.1100
            }

            out2b_buffer[col / 4 + row * out2b_stride] = compressed_unpacked_pixel;
        }
    }
}

/************************************************
* convert compressed packed 2bit data to uncompressed 8bit
data: 4 pixels in 1 byte to 4 1pixel-bytes
************************************************/
void svt_c_unpack_compressed_10bit(const uint8_t* inn_bit_buffer, uint32_t inn_stride, uint8_t* in_compn_bit_buffer,
                                   uint32_t out_stride, uint32_t height) {
    uint32_t row_index, col_index;

    for (row_index = 0; row_index < height; row_index++) {
        for (col_index = 0; col_index < out_stride; col_index++) {
            uint32_t data_byte = inn_bit_buffer[row_index * inn_stride + col_index / 4];

            uint8_t uncompressed_byte = 0;

            if (col_index % 4 == 0) {
                uncompressed_byte = data_byte & 0xC0;
            } else if (col_index % 4 == 1) {
                uncompressed_byte = (data_byte << 2) & 0xC0;
            } else if (col_index % 4 == 2) {
                uncompressed_byte = (data_byte << 4) & 0xC0;
            } else {
                uncompressed_byte = (data_byte << 6) & 0xC0;
            }

            in_compn_bit_buffer[row_index * out_stride + col_index] = uncompressed_byte;
        }
    }
}

/************************************************
* unpack 10 bit data into  8 and 2 bit 2D data
************************************************/
void svt_enc_msb_un_pack2_d(uint16_t* in16_bit_buffer, uint32_t in_stride, uint8_t* out8_bit_buffer,
                            uint8_t* outn_bit_buffer, uint32_t out8_stride, uint32_t outn_stride, uint32_t width,
                            uint32_t height) {
    uint64_t j, k;
    uint16_t in_pixel;
    uint8_t  tmp_pixel;
    for (j = 0; j < height; j++) {
        for (k = 0; k < width; k++) {
            in_pixel                             = in16_bit_buffer[k + j * in_stride];
            out8_bit_buffer[k + j * out8_stride] = (uint8_t)(in_pixel >> 2);
            if (outn_bit_buffer) {
                tmp_pixel                            = (uint8_t)(in_pixel << 6);
                outn_bit_buffer[k + j * outn_stride] = tmp_pixel;
            }
        }
    }
}

void svt_convert_8bit_to_16bit_c(uint8_t* src, uint32_t src_stride, uint16_t* dst, uint32_t dst_stride, uint32_t width,
                                 uint32_t height) {
    for (uint32_t j = 0; j < height; j++) {
        for (uint32_t k = 0; k < width; k++) {
            dst[k + j * dst_stride] = src[k + j * src_stride];
        }
    }
}

void svt_convert_16bit_to_8bit_c(uint16_t* src, uint32_t src_stride, uint8_t* dst, uint32_t dst_stride, uint32_t width,
                                 uint32_t height) {
    for (uint32_t j = 0; j < height; j++) {
        for (uint32_t k = 0; k < width; k++) {
            dst[k + j * dst_stride] = (uint8_t)(src[k + j * src_stride]);
        }
    }
}
