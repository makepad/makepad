/*
* Copyright(c) 2022 Intel Corporation
*
* This source code is subject to the terms of the BSD 3-Clause Clear License and
* the Alliance for Open Media Patent License 1.0. If the BSD 3-Clause Clear License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#include <stdint.h>
#include <stdio.h>

#include "app_config.h"
#include "app_output_ivf.h"

#define AV1_FOURCC 0x31305641 // used for ivf header
#define IVF_STREAM_HEADER_SIZE 32
#define IVF_FRAME_HEADER_SIZE 12

static __inline void mem_put_le32(void* vmem, int32_t val) {
    uint8_t* mem = (uint8_t*)vmem;

    mem[0] = (uint8_t)((val >> 0) & 0xff);
    mem[1] = (uint8_t)((val >> 8) & 0xff);
    mem[2] = (uint8_t)((val >> 16) & 0xff);
    mem[3] = (uint8_t)((val >> 24) & 0xff);
}

static __inline void mem_put_le16(void* vmem, int32_t val) {
    uint8_t* mem = (uint8_t*)vmem;

    mem[0] = (uint8_t)((val >> 0) & 0xff);
    mem[1] = (uint8_t)((val >> 8) & 0xff);
}

void write_ivf_stream_header(EbConfig* app_cfg, int32_t length) {
    char header[IVF_STREAM_HEADER_SIZE] = {'D', 'K', 'I', 'F'};
    mem_put_le16(header + 4, 0); // version
    mem_put_le16(header + 6, 32); // header size
    mem_put_le32(header + 8, AV1_FOURCC); // fourcc
    mem_put_le16(header + 12, app_cfg->input_padded_width); // width
    mem_put_le16(header + 14, app_cfg->input_padded_height); // height
    mem_put_le32(header + 16, app_cfg->config.frame_rate_numerator); // rate
    mem_put_le32(header + 20, app_cfg->config.frame_rate_denominator); // scale
    mem_put_le32(header + 24, length); // length
    mem_put_le32(header + 28, 0); // unused
    fwrite(header, 1, IVF_STREAM_HEADER_SIZE, app_cfg->bitstream_file);
}

void write_ivf_frame_header(EbConfig* app_cfg, uint32_t byte_count) {
    char header[IVF_FRAME_HEADER_SIZE];

    mem_put_le32(&header[0], (int32_t)byte_count);
    mem_put_le32(&header[4], (int32_t)(app_cfg->ivf_count & 0xFFFFFFFF));
    mem_put_le32(&header[8], (int32_t)(app_cfg->ivf_count >> 32));

    app_cfg->ivf_count++;
    fwrite(header, 1, IVF_FRAME_HEADER_SIZE, app_cfg->bitstream_file);
}
