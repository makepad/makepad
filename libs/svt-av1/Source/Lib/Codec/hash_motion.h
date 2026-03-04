/*
 * Copyright (c) 2016, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#ifndef AOM_AV1_ENCODER_HASH_MOTION_H_
#define AOM_AV1_ENCODER_HASH_MOTION_H_

#include "definitions.h"
#include "coding_unit.h"
#include "vector.h"
#include "pic_buffer_desc.h"

#ifdef __cplusplus
extern "C" {
#endif

// store a block's hash info.
// x and y are the position from the top left of the picture
// hash_value2 is used to store the second hash value
typedef struct _block_hash {
    int16_t  x;
    int16_t  y;
    uint32_t hash_value2;
} BlockHash;

typedef struct HashTable {
    Vector** p_lookup_table;
} HashTable;

void        svt_av1_hash_table_destroy(HashTable* p_hash_table);
EbErrorType svt_aom_rtime_alloc_svt_av1_hash_table_create(HashTable* p_hash_table);
int32_t     svt_av1_hash_table_count(const HashTable* p_hash_table, uint32_t hash_value);
Iterator    svt_av1_hash_get_first_iterator(HashTable* p_hash_table, uint32_t hash_value);
void        svt_av1_generate_block_2x2_hash_value(const Yv12BufferConfig* picture, uint32_t* pic_block_hash,
                                                  struct PictureControlSet* pcs);

void svt_av1_generate_block_hash_value(const Yv12BufferConfig* picture, int block_size, uint32_t* src_pic_block_hash,
                                       uint32_t* dst_pic_block_hash, struct PictureControlSet* pcs);
bool svt_aom_rtime_alloc_svt_av1_add_to_hash_map_by_row_with_precal_data(HashTable* p_hash_table, uint32_t* pic_hash,
                                                                         int pic_width, int pic_height, int block_size);

// check whether the block starts from (x_start, y_start) with the size of
// BlockSize x BlockSize has the same color in all rows

// check whether the block starts from (x_start, y_start) with the size of
// BlockSize x BlockSize has the same color in all columns
void svt_av1_get_block_hash_value(uint8_t* y_src, int stride, int block_size, uint32_t* hash_value1,
                                  uint32_t* hash_value2, int use_highbitdepth, struct PictureControlSet* pcs,
                                  struct IntraBcContext /*MACROBLOCK*/* x);

#ifdef __cplusplus
} // extern "C"
#endif

#endif // AOM_AV1_ENCODER_HASH_MOTION_H_
