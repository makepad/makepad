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

#ifndef EbEntropyCodingObject_h
#define EbEntropyCodingObject_h

#include "definitions.h"
#include "cabac_context_model.h"
#include "bitstream_unit.h"
#include "object.h"
#ifdef __cplusplus
extern "C" {
#endif
typedef struct Bitstream {
    EbDctor              dctor;
    OutputBitstreamUnit* output_bitstream_ptr;
} Bitstream;

typedef struct EntropyCoder {
    EbDctor        dctor;
    FRAME_CONTEXT* fc; /* this frame entropy */
    AomWriter      ec_writer;
    EbPtr          ec_output_bitstream_ptr;
    uint64_t       ec_frame_size;
} EntropyCoder;

typedef struct EntropyTileInfo {
    EbDctor       dctor;
    EntropyCoder* ec;
    bool          entropy_coding_tile_done;
} EntropyTileInfo;

EbErrorType svt_aom_entropy_tile_info_ctor(EntropyTileInfo* entropy_tile_info_ptr, uint32_t buf_size);

EbErrorType svt_aom_bitstream_ctor(Bitstream* bitstream_ptr, uint32_t buffer_size);

void svt_aom_bitstream_reset(Bitstream* bitstream_ptr);

int svt_aom_bitstream_get_bytes_count(const Bitstream* bitstream_ptr);

// copy size bytes from bistream_ptr to dst
void svt_aom_bitstream_copy(const Bitstream* bitstream_ptr, void* dest, int size);

EbErrorType svt_aom_entropy_coder_ctor(EntropyCoder* ec, uint32_t buffer_size);

#ifdef __cplusplus
}
#endif
#endif // EbEntropyCodingObject_h
