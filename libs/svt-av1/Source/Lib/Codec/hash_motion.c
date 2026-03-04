/*
 * Copyright (c) 2018, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#include "aom_dsp_rtcd.h"
#include "hash_motion.h"
#include "pcs.h"

static const int crc_bits                       = 16;
static const int block_size_bits                = 3;
static const int max_candidates_per_hash_bucket = 256;

static void hash_table_clear_all(HashTable* p_hash_table) {
    if (p_hash_table->p_lookup_table == NULL) {
        return;
    }
    int max_addr = 1 << (crc_bits + block_size_bits);
    for (int i = 0; i < max_addr; i++) {
        if (p_hash_table->p_lookup_table[i] != NULL) {
            svt_aom_vector_destroy(p_hash_table->p_lookup_table[i]);
            EB_FREE(p_hash_table->p_lookup_table[i]);
            p_hash_table->p_lookup_table[i] = NULL;
        }
    }
}

static void get_pixels_in_1d_char_array_by_block_2x2(uint8_t* y_src, int stride, uint8_t* p_pixels_in1D) {
    uint8_t* p_pel = y_src;
    int      index = 0;
    for (int i = 0; i < 2; i++) {
        for (int j = 0; j < 2; j++) {
            p_pixels_in1D[index++] = p_pel[j];
        }
        p_pel += stride;
    }
}

static void get_pixels_in_1d_short_array_by_block_2x2(uint16_t* y_src, int stride, uint16_t* p_pixels_in1D) {
    uint16_t* p_pel = y_src;
    int       index = 0;
    for (int i = 0; i < 2; i++) {
        for (int j = 0; j < 2; j++) {
            p_pixels_in1D[index++] = p_pel[j];
        }
        p_pel += stride;
    }
}

// the hash value (hash_value1 consists two parts, the first 3 bits relate to
// the block size and the remaining 16 bits are the crc values. This fuction
// is used to get the first 3 bits.
static int hash_block_size_to_index(int block_size) {
    switch (block_size) {
    case 4:
        return 0;
    case 8:
        return 1;
    case 16:
        return 2;
    case 32:
        return 3;
    case 64:
        return 4;
    case 128:
        return 5;
    default:
        return -1;
    }
}

static uint32_t get_identity_hash_value(const uint8_t a, const uint8_t b, const uint8_t c, const uint8_t d) {
    // The four input values add up to 32 bits, which is the size of the output.
    // Just pack those values as is.
    return ((uint32_t)a << 24) + ((uint32_t)b << 16) + ((uint32_t)c << 8) + ((uint32_t)d);
}

static uint32_t get_xor_hash_value_hbd(const uint16_t a, const uint16_t b, const uint16_t c, const uint16_t d) {
    uint32_t result;
    // Pack the lower 8 bits of each input value to the 32 bit output, then xor
    // with the upper 8 bits of each input value.
    result = ((uint32_t)(a & 0x00ff) << 24) + ((uint32_t)(b & 0x00ff) << 16) + ((uint32_t)(c & 0x00ff) << 8) +
        ((uint32_t)(d & 0x00ff));
    result ^= ((uint32_t)(a & 0xff00) << 16) + ((uint32_t)(b & 0xff00) << 8) + ((uint32_t)(c & 0xff00)) +
        ((uint32_t)(d & 0xff00) >> 8);
    return result;
}

void svt_av1_hash_table_destroy(HashTable* p_hash_table) {
    hash_table_clear_all(p_hash_table);
    EB_FREE_ARRAY(p_hash_table->p_lookup_table);
    p_hash_table->p_lookup_table = NULL;
}

EbErrorType svt_aom_rtime_alloc_svt_av1_hash_table_create(HashTable* p_hash_table) {
    EbErrorType err_code = EB_ErrorNone;
    ;

    if (p_hash_table->p_lookup_table != NULL) {
        hash_table_clear_all(p_hash_table);
        return err_code;
    }
    const int max_addr = 1 << (crc_bits + block_size_bits);
    EB_CALLOC_ARRAY(p_hash_table->p_lookup_table, max_addr);

    return err_code;
}

static bool hash_table_add_to_table(HashTable* p_hash_table, uint32_t hash_value, const BlockHash* curr_block_hash) {
    if (p_hash_table->p_lookup_table[hash_value] == NULL) {
        EB_MALLOC_OBJECT_NO_CHECK(p_hash_table->p_lookup_table[hash_value]);
        if (p_hash_table->p_lookup_table[hash_value] == NULL) {
            return false;
        }
        if (svt_aom_vector_setup(p_hash_table->p_lookup_table[hash_value], 10, sizeof(*curr_block_hash)) ==
            VECTOR_ERROR) {
            return false;
        }
    }
    // Place an upper bound each hash table bucket to up to 256 intrabc
    // block candidates, and ignore subsequent ones. Considering more can
    // unnecessarily slow down encoding for virtually no efficiency gain.
    if (svt_aom_vector_byte_size(p_hash_table->p_lookup_table[hash_value]) <
        max_candidates_per_hash_bucket * sizeof(*curr_block_hash)) {
        if (svt_aom_vector_push_back(p_hash_table->p_lookup_table[hash_value], (void*)curr_block_hash) ==
            VECTOR_ERROR) {
            return false;
        }
    }
    return true;
}

int32_t svt_av1_hash_table_count(const HashTable* p_hash_table, uint32_t hash_value) {
    if (p_hash_table->p_lookup_table[hash_value] == NULL) {
        return 0;
    } else {
        return (int32_t)(p_hash_table->p_lookup_table[hash_value]->size);
    }
}

Iterator svt_av1_hash_get_first_iterator(HashTable* p_hash_table, uint32_t hash_value) {
    assert(svt_av1_hash_table_count(p_hash_table, hash_value) > 0);
    return svt_aom_vector_begin(p_hash_table->p_lookup_table[hash_value]);
}

void svt_av1_generate_block_2x2_hash_value(const Yv12BufferConfig* picture, uint32_t* pic_block_hash,
                                           PictureControlSet* pcs) {
    const int width  = 2;
    const int height = 2;
    const int x_end  = picture->y_crop_width - width + 1;
    const int y_end  = picture->y_crop_height - height + 1;
    (void)pcs;
    if (picture->flags & YV12_FLAG_HIGHBITDEPTH) {
        uint16_t p[4];
        int      pos = 0;
        for (int y_pos = 0; y_pos < y_end; y_pos++) {
            for (int x_pos = 0; x_pos < x_end; x_pos++) {
                get_pixels_in_1d_short_array_by_block_2x2(
                    CONVERT_TO_SHORTPTR(picture->y_buffer) + y_pos * picture->y_stride + x_pos, picture->y_stride, p);
                // For HBD, we either have 40 or 48 bits of input data that the xor hash
                // reduce to 32 bits. We intentionally don't want to "discard" bits to
                // avoid any kind of biasing.
                pic_block_hash[pos] = get_xor_hash_value_hbd(p[0], p[1], p[2], p[3]);
                pos++;
            }
            pos += width - 1;
        }
    } else {
        uint8_t p[4];
        int     pos = 0;
        for (int y_pos = 0; y_pos < y_end; y_pos++) {
            for (int x_pos = 0; x_pos < x_end; x_pos++) {
                get_pixels_in_1d_char_array_by_block_2x2(
                    picture->y_buffer + y_pos * picture->y_stride + x_pos, picture->y_stride, p);
                // This 2x2 hash isn't used directly as a "key" for the hash table, so
                // we can afford to just copy the 4 8-bit pixel values as a single
                // 32-bit value directly. (i.e. there are no concerns of a lack of
                // uniform distribution)
                pic_block_hash[pos] = get_identity_hash_value(p[0], p[1], p[2], p[3]);
                pos++;
            }
            pos += width - 1;
        }
    }
}

void svt_av1_generate_block_hash_value(const Yv12BufferConfig* picture, int block_size, uint32_t* src_pic_block_hash,
                                       uint32_t* dst_pic_block_hash, PictureControlSet* pcs) {
    const int pic_width = picture->y_crop_width;
    const int x_end     = picture->y_crop_width - block_size + 1;
    const int y_end     = picture->y_crop_height - block_size + 1;

    const int src_size = block_size >> 1;

    uint32_t  p[4];
    const int length = sizeof(p);

    int pos = 0;
    for (int y_pos = 0; y_pos < y_end; y_pos++) {
        for (int x_pos = 0; x_pos < x_end; x_pos++) {
            p[0]                    = src_pic_block_hash[pos];
            p[1]                    = src_pic_block_hash[pos + src_size];
            p[2]                    = src_pic_block_hash[pos + src_size * pic_width];
            p[3]                    = src_pic_block_hash[pos + src_size * pic_width + src_size];
            dst_pic_block_hash[pos] = svt_av1_get_crc32c_value(&pcs->crc_calculator, (uint8_t*)p, length);

            pos++;
        }
        pos += block_size - 1;
    }
}

bool svt_aom_rtime_alloc_svt_av1_add_to_hash_map_by_row_with_precal_data(HashTable* p_hash_table, uint32_t* pic_hash,
                                                                         int pic_width, int pic_height,
                                                                         int block_size) {
    const int x_end = pic_width - block_size + 1;
    const int y_end = pic_height - block_size + 1;

    int add_value = hash_block_size_to_index(block_size);
    assert(add_value >= 0);
    add_value <<= crc_bits;
    const int crc_mask = (1 << crc_bits) - 1;
    int       step     = block_size;
    int       x_offset = 0;
    int       y_offset = 0;

    // Explore the entire frame hierarchically to add intrabc candidate blocks to
    // the hash table, by starting with coarser steps (the block size), towards
    // finer-grained steps until every candidate block has been considered.
    // The nested for loop goes through the pic_hash array column by column.

    // Doing a hierarchical block exploration helps maximize spatial dispersion
    // of the first and foremost candidate blocks while minimizing overlap between
    // them. This is helpful because we only keep up to 256 entries of the
    // same candidate block (located in different places), so we want those
    // entries to cover the biggest area of the image to encode to maximize coding
    // efficiency.

    // This is the coordinate exploration order example for an 8x8 region, with
    // block_size = 4. The top-left corner (x, y) coordinates of each candidate
    // block are shown below. There are 5 * 5 (25) candidate blocks.
    //    x  0  1  2  3  4  5  6  7
    //  y +------------------------
    //  0 |  1 10  5 13  3
    //  1 | 16 22 18 24 20
    //  2 |  7 11  9 14  8
    //  3 | 17 23 19 25 21
    //  4 |  2 12  6 15  4--------+
    //  5 |              | 4 x 4  |
    //  6 |              | block  |
    //  7 |              +--------+

    // Please note that due to the way block exploration works, the smallest step
    // used is 2 (i.e. no two adjacent blocks will be explored consecutively).
    // Also, the exploration is designed to visit each block candidate only once.
    while (step > 1) {
        for (int x_pos = x_offset; x_pos < x_end; x_pos += step) {
            for (int y_pos = y_offset; y_pos < y_end; y_pos += step) {
                const int pos = y_pos * pic_width + x_pos;
                BlockHash curr_block_hash;

                curr_block_hash.x = x_pos;
                curr_block_hash.y = y_pos;

                const uint32_t hash_value1  = (pic_hash[pos] & crc_mask) + add_value;
                curr_block_hash.hash_value2 = pic_hash[pos];

                if (!hash_table_add_to_table(p_hash_table, hash_value1, &curr_block_hash)) {
                    return false;
                }
            }
        }

        // Adjust offsets and step sizes with this state machine.
        // State 0 is needed because no blocks in pic_hash have been explored,
        // so exploration requires a way to account for blocks with both zero
        // x_offset and zero y_offset.
        // State 0 is always meant to be executed first, but the relative order of
        // states 1, 2 and 3 can be arbitrary, as long as no two adjacent blocks
        // are explored consecutively.
        if (x_offset == 0 && y_offset == 0) {
            // State 0 -> State 1: special case
            // This state transition will only execute when step == block_size
            x_offset = step / 2;
        } else if (x_offset == step / 2 && y_offset == 0) {
            // State 1 -> State 2
            x_offset = 0;
            y_offset = step / 2;
        } else if (x_offset == 0 && y_offset == step / 2) {
            // State 2 -> State 3
            x_offset = step / 2;
        } else {
            assert(x_offset == step / 2 && y_offset == step / 2);
            // State 3 -> State 1: We've fully explored all the coordinates for the
            // current step size, continue by halving the step size
            step /= 2;
            x_offset = step / 2;
            y_offset = 0;
        }
    }

    return true;
}

void svt_av1_get_block_hash_value(uint8_t* y_src, int stride, int block_size, uint32_t* hash_value1,
                                  uint32_t* hash_value2, int use_highbitdepth, struct PictureControlSet* pcs,
                                  IntraBcContext* x) {
    UNUSED(pcs);
    const int add_value = hash_block_size_to_index(block_size) << crc_bits;
    assert(add_value >= 0);
    const int crc_mask = (1 << crc_bits) - 1;

    // 2x2 subblock hash values in current CU
    int sub_block_in_width = (block_size >> 1);
    if (use_highbitdepth) {
        uint16_t  pixel_to_hash[4];
        uint16_t* y16_src = CONVERT_TO_SHORTPTR(y_src);
        for (int y_pos = 0; y_pos < block_size; y_pos += 2) {
            for (int x_pos = 0; x_pos < block_size; x_pos += 2) {
                int pos = (y_pos >> 1) * sub_block_in_width + (x_pos >> 1);
                get_pixels_in_1d_short_array_by_block_2x2(y16_src + y_pos * stride + x_pos, stride, pixel_to_hash);
                assert(pos < AOM_BUFFER_SIZE_FOR_BLOCK_HASH);
                // For HBD, we either have 40 or 48 bits of input data that the xor hash
                // reduce to 32 bits. We intentionally don't want to "discard" bits to
                // avoid any kind of biasing.
                x->hash_value_buffer[0][pos] = get_xor_hash_value_hbd(
                    pixel_to_hash[0], pixel_to_hash[1], pixel_to_hash[2], pixel_to_hash[3]);
            }
        }
    } else {
        uint8_t pixel_to_hash[4];
        for (int y_pos = 0; y_pos < block_size; y_pos += 2) {
            for (int x_pos = 0; x_pos < block_size; x_pos += 2) {
                int pos = (y_pos >> 1) * sub_block_in_width + (x_pos >> 1);
                get_pixels_in_1d_char_array_by_block_2x2(y_src + y_pos * stride + x_pos, stride, pixel_to_hash);
                assert(pos < AOM_BUFFER_SIZE_FOR_BLOCK_HASH);
                // This 2x2 hash isn't used directly as a "key" for the hash table, so
                // we can afford to just copy the 4 8-bit pixel values as a single
                // 32-bit value directly. (i.e. there are no concerns of a lack of
                // uniform distribution)
                x->hash_value_buffer[0][pos] = get_identity_hash_value(
                    pixel_to_hash[0], pixel_to_hash[1], pixel_to_hash[2], pixel_to_hash[3]);
            }
        }
    }

    int src_sub_block_in_width = sub_block_in_width;
    sub_block_in_width >>= 1;

    int src_idx = 0;
    int dst_idx = 1 - src_idx;

    // 4x4 subblock hash values to current block hash values
    uint32_t to_hash[4];
    for (int sub_width = 4; sub_width <= block_size; sub_width *= 2, src_idx = 1 - src_idx) {
        dst_idx = 1 - src_idx;

        int dst_pos = 0;
        for (int y_pos = 0; y_pos < sub_block_in_width; y_pos++) {
            for (int x_pos = 0; x_pos < sub_block_in_width; x_pos++) {
                int src_pos = (y_pos << 1) * src_sub_block_in_width + (x_pos << 1);

                assert(src_pos + 1 < AOM_BUFFER_SIZE_FOR_BLOCK_HASH);
                assert(src_pos + src_sub_block_in_width + 1 < AOM_BUFFER_SIZE_FOR_BLOCK_HASH);
                assert(dst_pos < AOM_BUFFER_SIZE_FOR_BLOCK_HASH);

                to_hash[0] = x->hash_value_buffer[src_idx][src_pos];
                to_hash[1] = x->hash_value_buffer[src_idx][src_pos + 1];
                to_hash[2] = x->hash_value_buffer[src_idx][src_pos + src_sub_block_in_width];
                to_hash[3] = x->hash_value_buffer[src_idx][src_pos + src_sub_block_in_width + 1];

                x->hash_value_buffer[dst_idx][dst_pos] = svt_av1_get_crc32c_value(
                    &x->crc_calculator, (uint8_t*)to_hash, sizeof(to_hash));
                dst_pos++;
            }
        }

        src_sub_block_in_width = sub_block_in_width;
        sub_block_in_width >>= 1;
    }

    *hash_value1 = (x->hash_value_buffer[dst_idx][0] & crc_mask) + add_value;
    *hash_value2 = x->hash_value_buffer[dst_idx][0];
}
