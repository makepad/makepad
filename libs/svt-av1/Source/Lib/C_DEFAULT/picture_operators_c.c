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

#include "picture_operators_c.h"
#include <stdint.h>
#include <stdio.h>
#include "utility.h"
#include "common_dsp_rtcd.h"
#include "ac_bias.h"

/*********************************
* Picture Copy Kernel
*********************************/
void svt_memcpy_c(void* dst_ptr, void const* src_ptr, size_t size) {
    memcpy(dst_ptr, src_ptr, size);
}

void svt_memset_c(void* dst_ptr, int c, size_t size) {
    memset(dst_ptr, c, size);
}

void svt_aom_picture_copy_kernel(EbByte src, uint32_t src_stride, EbByte dst, uint32_t dst_stride, uint32_t area_width,
                                 uint32_t area_height,
                                 uint32_t bytes_per_sample) //=1 always)
{
    uint32_t       sample_count       = 0;
    const uint32_t sample_total_count = area_width * area_height;
    const uint32_t copy_length        = area_width * bytes_per_sample;

    src_stride *= bytes_per_sample;
    dst_stride *= bytes_per_sample;

    while (sample_count < sample_total_count) {
        svt_memcpy_c(dst, src, copy_length);
        src += src_stride;
        dst += dst_stride;
        sample_count += area_width;
    }

    return;
}

// C equivalents

uint64_t svt_spatial_full_distortion_kernel_c(uint8_t* input, uint32_t input_offset, uint32_t input_stride,
                                              uint8_t* recon, int32_t recon_offset, uint32_t recon_stride,
                                              uint32_t area_width, uint32_t area_height) {
    uint64_t spatial_distortion = 0;
    input += input_offset;
    recon += recon_offset;

    for (uint32_t row_index = 0; row_index < area_height; ++row_index) {
        uint32_t column_index = 0;
        while (column_index < area_width) {
            spatial_distortion += (int64_t)SQR((int64_t)(input[column_index]) - (recon[column_index]));
            ++column_index;
        }

        input += input_stride;
        recon += recon_stride;
    }
    return spatial_distortion;
}

static void hadamard_col4(const int16_t* src_diff, ptrdiff_t src_stride, int16_t* coeff) {
    int16_t b0 = (src_diff[0 * src_stride] + src_diff[1 * src_stride]) >> 1;
    int16_t b1 = (src_diff[0 * src_stride] - src_diff[1 * src_stride]) >> 1;
    int16_t b2 = (src_diff[2 * src_stride] + src_diff[3 * src_stride]) >> 1;
    int16_t b3 = (src_diff[2 * src_stride] - src_diff[3 * src_stride]) >> 1;

    coeff[0] = b0 + b2;
    coeff[1] = b1 + b3;
    coeff[2] = b0 - b2;
    coeff[3] = b1 - b3;
}

void svt_aom_hadamard_4x4_c(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff) {
    int      idx;
    int16_t  buffer[16];
    int16_t  buffer2[16];
    int16_t* tmp_buf = &buffer[0];
    for (idx = 0; idx < 4; ++idx) {
        // src_diff: 9 bit (8b), 13 bit (HBD)
        // dynamic range [-255, 255] (8b), [-4095, 4095] (HBD)
        hadamard_col4(src_diff, src_stride, tmp_buf);
        tmp_buf += 4;
        ++src_diff;
    }

    tmp_buf = &buffer[0];
    for (idx = 0; idx < 4; ++idx) {
        // tmp_buf: 10 bit (8b), 14 bit (HBD)
        // dynamic range [-510, 510] (8b), [-8190, 8190] (HBD)
        // buffer2: 11 bit (8b), 15 bit (HBD)
        // dynamic range [-1020, 1020] (8b), [-16380, 16380] (HBD)
        hadamard_col4(tmp_buf, 4, buffer2 + 4 * idx);

        ++tmp_buf;
    }

    // Extra transpose to match SSE2 behavior(i.e., svt_aom_hadamard_4x4_sse2).
    for (int i = 0; i < 4; i++) {
        for (int j = 0; j < 4; j++) {
            coeff[i * 4 + j] = (int32_t)buffer2[j * 4 + i];
        }
    }
}

// src_diff: first pass, 9 bit, dynamic range [-255, 255]
//           second pass, 12 bit, dynamic range [-2040, 2040]
static void hadamard_col8(const int16_t* src_diff, ptrdiff_t src_stride, int16_t* coeff) {
    int16_t b0 = src_diff[0 * src_stride] + src_diff[1 * src_stride];
    int16_t b1 = src_diff[0 * src_stride] - src_diff[1 * src_stride];
    int16_t b2 = src_diff[2 * src_stride] + src_diff[3 * src_stride];
    int16_t b3 = src_diff[2 * src_stride] - src_diff[3 * src_stride];
    int16_t b4 = src_diff[4 * src_stride] + src_diff[5 * src_stride];
    int16_t b5 = src_diff[4 * src_stride] - src_diff[5 * src_stride];
    int16_t b6 = src_diff[6 * src_stride] + src_diff[7 * src_stride];
    int16_t b7 = src_diff[6 * src_stride] - src_diff[7 * src_stride];

    int16_t c0 = b0 + b2;
    int16_t c1 = b1 + b3;
    int16_t c2 = b0 - b2;
    int16_t c3 = b1 - b3;
    int16_t c4 = b4 + b6;
    int16_t c5 = b5 + b7;
    int16_t c6 = b4 - b6;
    int16_t c7 = b5 - b7;

    coeff[0] = c0 + c4;
    coeff[7] = c1 + c5;
    coeff[3] = c2 + c6;
    coeff[4] = c3 + c7;
    coeff[2] = c0 - c4;
    coeff[6] = c1 - c5;
    coeff[1] = c2 - c6;
    coeff[5] = c3 - c7;
}

// The order of the output coeff of the hadamard is not important. For
// optimization purposes the final transpose may be skipped.
void svt_aom_hadamard_8x8_c(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff) {
    int      idx;
    int16_t  buffer[64];
    int16_t  buffer2[64];
    int16_t* tmp_buf = &buffer[0];
    for (idx = 0; idx < 8; ++idx) {
        hadamard_col8(src_diff, src_stride, tmp_buf); // src_diff: 9 bit
        // dynamic range [-255, 255]
        tmp_buf += 8;
        ++src_diff;
    }

    tmp_buf = &buffer[0];
    for (idx = 0; idx < 8; ++idx) {
        hadamard_col8(tmp_buf, 8, buffer2 + 8 * idx); // tmp_buf: 12 bit
        // dynamic range [-2040, 2040]
        // buffer2: 15 bit
        // dynamic range [-16320, 16320]
        ++tmp_buf;
    }

    for (idx = 0; idx < 64; ++idx) {
        coeff[idx] = (int32_t)buffer2[idx];
    }
}

static void hadamard_highbd_col8_first_pass(const int16_t* src_diff, ptrdiff_t src_stride, int16_t* coeff) {
    int16_t b0 = src_diff[0 * src_stride] + src_diff[1 * src_stride];
    int16_t b1 = src_diff[0 * src_stride] - src_diff[1 * src_stride];
    int16_t b2 = src_diff[2 * src_stride] + src_diff[3 * src_stride];
    int16_t b3 = src_diff[2 * src_stride] - src_diff[3 * src_stride];
    int16_t b4 = src_diff[4 * src_stride] + src_diff[5 * src_stride];
    int16_t b5 = src_diff[4 * src_stride] - src_diff[5 * src_stride];
    int16_t b6 = src_diff[6 * src_stride] + src_diff[7 * src_stride];
    int16_t b7 = src_diff[6 * src_stride] - src_diff[7 * src_stride];

    int16_t c0 = b0 + b2;
    int16_t c1 = b1 + b3;
    int16_t c2 = b0 - b2;
    int16_t c3 = b1 - b3;
    int16_t c4 = b4 + b6;
    int16_t c5 = b5 + b7;
    int16_t c6 = b4 - b6;
    int16_t c7 = b5 - b7;

    coeff[0] = c0 + c4;
    coeff[7] = c1 + c5;
    coeff[3] = c2 + c6;
    coeff[4] = c3 + c7;
    coeff[2] = c0 - c4;
    coeff[6] = c1 - c5;
    coeff[1] = c2 - c6;
    coeff[5] = c3 - c7;
}

// src_diff: 16 bit, dynamic range [-32760, 32760]
// coeff: 19 bit
static void hadamard_highbd_col8_second_pass(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff) {
    int32_t b0 = src_diff[0 * src_stride] + src_diff[1 * src_stride];
    int32_t b1 = src_diff[0 * src_stride] - src_diff[1 * src_stride];
    int32_t b2 = src_diff[2 * src_stride] + src_diff[3 * src_stride];
    int32_t b3 = src_diff[2 * src_stride] - src_diff[3 * src_stride];
    int32_t b4 = src_diff[4 * src_stride] + src_diff[5 * src_stride];
    int32_t b5 = src_diff[4 * src_stride] - src_diff[5 * src_stride];
    int32_t b6 = src_diff[6 * src_stride] + src_diff[7 * src_stride];
    int32_t b7 = src_diff[6 * src_stride] - src_diff[7 * src_stride];

    int32_t c0 = b0 + b2;
    int32_t c1 = b1 + b3;
    int32_t c2 = b0 - b2;
    int32_t c3 = b1 - b3;
    int32_t c4 = b4 + b6;
    int32_t c5 = b5 + b7;
    int32_t c6 = b4 - b6;
    int32_t c7 = b5 - b7;

    coeff[0] = c0 + c4;
    coeff[7] = c1 + c5;
    coeff[3] = c2 + c6;
    coeff[4] = c3 + c7;
    coeff[2] = c0 - c4;
    coeff[6] = c1 - c5;
    coeff[1] = c2 - c6;
    coeff[5] = c3 - c7;
}

// The order of the output coeff of the hadamard is not important. For
// optimization purposes the final transpose may be skipped.
void svt_aom_highbd_hadamard_8x8_c(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff) {
    int      idx;
    int16_t  buffer[64];
    int32_t  buffer2[64];
    int16_t* tmp_buf = &buffer[0];
    for (idx = 0; idx < 8; ++idx) {
        // src_diff: 13 bit
        // buffer: 16 bit, dynamic range [-32760, 32760]
        hadamard_highbd_col8_first_pass(src_diff, src_stride, tmp_buf);
        tmp_buf += 8;
        ++src_diff;
    }

    tmp_buf = &buffer[0];
    for (idx = 0; idx < 8; ++idx) {
        // buffer: 16 bit
        // buffer2: 19 bit, dynamic range [-262080, 262080]
        hadamard_highbd_col8_second_pass(tmp_buf, 8, buffer2 + 8 * idx);
        ++tmp_buf;
    }

    for (idx = 0; idx < 64; ++idx) {
        coeff[idx] = (int32_t)buffer2[idx];
    }
}

// In place 16x16 2D Hadamard transform
void svt_aom_hadamard_16x16_c(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff) {
    int idx;
    for (idx = 0; idx < 4; ++idx) {
        // src_diff: 9 bit, dynamic range [-255, 255]
        const int16_t* src_ptr = src_diff + (idx >> 1) * 8 * src_stride + (idx & 0x01) * 8;
        svt_aom_hadamard_8x8_c(src_ptr, src_stride, coeff + idx * 64);
    }

    // coeff: 15 bit, dynamic range [-16320, 16320]
    for (idx = 0; idx < 64; ++idx) {
        int32_t a0 = coeff[0];
        int32_t a1 = coeff[64];
        int32_t a2 = coeff[128];
        int32_t a3 = coeff[192];

        int32_t b0 = (a0 + a1) >> 1; // (a0 + a1): 16 bit, [-32640, 32640]
        int32_t b1 = (a0 - a1) >> 1; // b0-b3: 15 bit, dynamic range
        int32_t b2 = (a2 + a3) >> 1; // [-16320, 16320]
        int32_t b3 = (a2 - a3) >> 1;

        coeff[0]   = b0 + b2; // 16 bit, [-32640, 32640]
        coeff[64]  = b1 + b3;
        coeff[128] = b0 - b2;
        coeff[192] = b1 - b3;

        ++coeff;
    }
}

void svt_aom_hadamard_32x32_c(const int16_t* src_diff, ptrdiff_t src_stride, int32_t* coeff) {
    int idx;
    for (idx = 0; idx < 4; ++idx) {
        // src_diff: 9 bit, dynamic range [-255, 255]
        const int16_t* src_ptr = src_diff + (idx >> 1) * 16 * src_stride + (idx & 0x01) * 16;
        svt_aom_hadamard_16x16_c(src_ptr, src_stride, coeff + idx * 256);
    }

    // coeff: 16 bit, dynamic range [-32768, 32767]
    for (idx = 0; idx < 256; ++idx) {
        int32_t a0 = coeff[0];
        int32_t a1 = coeff[256];
        int32_t a2 = coeff[512];
        int32_t a3 = coeff[768];

        int32_t b0 = (a0 + a1) >> 2; // (a0 + a1): 17 bit, [-65536, 65535]
        int32_t b1 = (a0 - a1) >> 2; // b0-b3: 15 bit, dynamic range
        int32_t b2 = (a2 + a3) >> 2; // [-16384, 16383]
        int32_t b3 = (a2 - a3) >> 2;

        coeff[0]   = b0 + b2; // 16 bit, [-32768, 32767]
        coeff[256] = b1 + b3;
        coeff[512] = b0 - b2;
        coeff[768] = b1 - b3;

        ++coeff;
    }
}

void svt_av1_copy_wxh_8bit_c(uint8_t* src, uint32_t src_stride, uint8_t* dst, uint32_t dst_stride, uint32_t height,
                             uint32_t width) {
    for (uint32_t j = 0; j < height; j++) {
        svt_memcpy_c(dst + j * dst_stride, src + j * src_stride, width);
    }
}

void svt_av1_copy_wxh_16bit_c(uint16_t* src, uint32_t src_stride, uint16_t* dst, uint32_t dst_stride, uint32_t height,
                              uint32_t width) {
    for (uint32_t j = 0; j < height; j++) {
        svt_memcpy_c(dst + j * dst_stride, src + j * src_stride, width * 2);
    }
}
