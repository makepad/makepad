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

#include <stdlib.h>
#include <string.h>

#include "neighbor_arrays.h"
#include "utility.h"
#include "pic_operators.h"
#include "common_dsp_rtcd.h"
#define UNUSED(x) (void)(x)

static void neighbor_array_unit_dctor32(EbPtr p) {
    NeighborArrayUnit32* obj = (NeighborArrayUnit32*)p;
    EB_FREE(obj->left_array);
    EB_FREE(obj->top_array);
    EB_FREE(obj->top_left_array);
}

/*************************************************
 * Neighbor Array Unit Ctor
 *************************************************/
EbErrorType svt_aom_neighbor_array_unit_ctor32(NeighborArrayUnit32* na_unit_ptr, uint32_t max_picture_width,
                                               uint32_t max_picture_height, uint32_t unit_size,
                                               uint8_t granularity_normal, uint8_t granularity_top_left,
                                               uint8_t type_mask) {
    na_unit_ptr->dctor                     = neighbor_array_unit_dctor32;
    na_unit_ptr->unit_size                 = (uint8_t)(unit_size);
    na_unit_ptr->granularity_normal        = granularity_normal;
    na_unit_ptr->granularity_normal_log2   = (uint8_t)(svt_log2f(na_unit_ptr->granularity_normal));
    na_unit_ptr->granularity_top_left      = granularity_top_left;
    na_unit_ptr->granularity_top_left_log2 = (uint8_t)(svt_log2f(na_unit_ptr->granularity_top_left));
    na_unit_ptr->left_array_size           = (uint16_t)((type_mask & NEIGHBOR_ARRAY_UNIT_LEFT_MASK)
                                                            ? max_picture_height >> na_unit_ptr->granularity_normal_log2
                                                            : 0);
    na_unit_ptr->top_array_size            = (uint16_t)((type_mask & NEIGHBOR_ARRAY_UNIT_TOP_MASK)
                                                            ? max_picture_width >> na_unit_ptr->granularity_normal_log2
                                                            : 0);
    na_unit_ptr->top_left_array_size       = (uint16_t)((type_mask & NEIGHBOR_ARRAY_UNIT_TOPLEFT_MASK)
                                                            ? (max_picture_width + max_picture_height) >>
                                                          na_unit_ptr->granularity_top_left_log2
                                                            : 0);

    if (na_unit_ptr->left_array_size) {
        EB_MALLOC(na_unit_ptr->left_array, na_unit_ptr->unit_size * na_unit_ptr->left_array_size);
    }

    if (na_unit_ptr->top_array_size) {
        EB_MALLOC(na_unit_ptr->top_array, na_unit_ptr->unit_size * na_unit_ptr->top_array_size);
    }

    if (na_unit_ptr->top_left_array_size) {
        EB_MALLOC(na_unit_ptr->top_left_array, na_unit_ptr->unit_size * na_unit_ptr->top_left_array_size);
    }
    return EB_ErrorNone;
}

static void neighbor_array_unit_dctor(EbPtr p) {
    NeighborArrayUnit* obj = (NeighborArrayUnit*)p;
    EB_FREE(obj->left_array);
    EB_FREE(obj->top_array);
    EB_FREE(obj->top_left_array);
}

EbErrorType svt_aom_neighbor_array_unit_ctor(NeighborArrayUnit* na_unit_ptr, uint32_t max_picture_width,
                                             uint32_t max_picture_height, uint32_t unit_size,
                                             uint8_t granularity_normal, uint8_t granularity_top_left,
                                             uint8_t type_mask) {
    na_unit_ptr->dctor                     = neighbor_array_unit_dctor;
    na_unit_ptr->unit_size                 = (uint8_t)(unit_size);
    na_unit_ptr->granularity_normal        = granularity_normal;
    na_unit_ptr->granularity_normal_log2   = (uint8_t)(svt_log2f(na_unit_ptr->granularity_normal));
    na_unit_ptr->granularity_top_left      = granularity_top_left;
    na_unit_ptr->granularity_top_left_log2 = (uint8_t)(svt_log2f(na_unit_ptr->granularity_top_left));

    na_unit_ptr->max_pic_h = max_picture_height;

    na_unit_ptr->left_array_size     = (uint16_t)((type_mask & NEIGHBOR_ARRAY_UNIT_LEFT_MASK)
                                                      ? max_picture_height >> na_unit_ptr->granularity_normal_log2
                                                      : 0);
    na_unit_ptr->top_array_size      = (uint16_t)((type_mask & NEIGHBOR_ARRAY_UNIT_TOP_MASK)
                                                      ? max_picture_width >> na_unit_ptr->granularity_normal_log2
                                                      : 0);
    na_unit_ptr->top_left_array_size = (uint16_t)((type_mask & NEIGHBOR_ARRAY_UNIT_TOPLEFT_MASK)
                                                      ? (max_picture_width + max_picture_height) >>
                                                          na_unit_ptr->granularity_top_left_log2
                                                      : 0);

    if (na_unit_ptr->left_array_size) {
        EB_MALLOC(na_unit_ptr->left_array, na_unit_ptr->unit_size * na_unit_ptr->left_array_size);
    }
    if (na_unit_ptr->top_array_size) {
        EB_MALLOC(na_unit_ptr->top_array, na_unit_ptr->unit_size * na_unit_ptr->top_array_size);
    }
    if (na_unit_ptr->top_left_array_size) {
        EB_MALLOC(na_unit_ptr->top_left_array, na_unit_ptr->unit_size * na_unit_ptr->top_left_array_size);
    }
    return EB_ErrorNone;
}

/*************************************************
 * Neighbor Array Unit Reset
 *************************************************/

void svt_aom_neighbor_array_unit_reset32(NeighborArrayUnit32* na_unit_ptr) {
    if (na_unit_ptr->left_array) {
        svt_memset(na_unit_ptr->left_array, ~0, na_unit_ptr->unit_size * na_unit_ptr->left_array_size);
    }

    if (na_unit_ptr->top_array) {
        svt_memset(na_unit_ptr->top_array, ~0, na_unit_ptr->unit_size * na_unit_ptr->top_array_size);
    }

    if (na_unit_ptr->top_left_array) {
        svt_memset(na_unit_ptr->top_left_array, ~0, na_unit_ptr->unit_size * na_unit_ptr->top_left_array_size);
    }

    return;
}

void svt_aom_neighbor_array_unit_reset(NeighborArrayUnit* na_unit_ptr) {
    if (na_unit_ptr->left_array) {
        svt_memset(na_unit_ptr->left_array, ~0, na_unit_ptr->unit_size * na_unit_ptr->left_array_size);
    }

    if (na_unit_ptr->top_array) {
        svt_memset(na_unit_ptr->top_array, ~0, na_unit_ptr->unit_size * na_unit_ptr->top_array_size);
    }

    if (na_unit_ptr->top_left_array) {
        svt_memset(na_unit_ptr->top_left_array, ~0, na_unit_ptr->unit_size * na_unit_ptr->top_left_array_size);
    }

    return;
}

/*************************************************
 * Neighbor Array Unit Get Top Index
 *************************************************/
static uint32_t get_neighbor_array_unit_top_left_index_32(NeighborArrayUnit32* na_unit_ptr, int32_t loc_x,
                                                          int32_t loc_y) {
    return na_unit_ptr->left_array_size + (loc_x >> na_unit_ptr->granularity_top_left_log2) -
        (loc_y >> na_unit_ptr->granularity_top_left_log2);
}

uint32_t svt_aom_get_neighbor_array_unit_top_left_index(NeighborArrayUnit* na_unit_ptr, int32_t loc_x, int32_t loc_y) {
    return na_unit_ptr->left_array_size + (loc_x >> na_unit_ptr->granularity_top_left_log2) -
        (loc_y >> na_unit_ptr->granularity_top_left_log2);
}

void svt_aom_update_recon_neighbor_array(NeighborArrayUnit* na_unit_ptr, uint8_t* src_ptr_top, uint8_t* src_ptr_left,
                                         uint32_t pic_origin_x, uint32_t pic_origin_y, uint32_t block_width,
                                         uint32_t block_height) {
    uint8_t* dst_ptr;

    dst_ptr = na_unit_ptr->top_array +
        get_neighbor_array_unit_top_index(na_unit_ptr, pic_origin_x) * na_unit_ptr->unit_size;
    svt_memcpy(dst_ptr, src_ptr_top, block_width);

    dst_ptr = na_unit_ptr->left_array +
        get_neighbor_array_unit_left_index(na_unit_ptr, pic_origin_y) * na_unit_ptr->unit_size;
    svt_memcpy(dst_ptr, src_ptr_left, block_height);

    //na_unit_ptr->top_left_array[ (MAX_PICTURE_HEIGHT_SIZE>>is_chroma) + pic_origin_x - pic_origin_y] = srcPtr2[block_height-1];

    /*
        //   Top-left Neighbor Array
        //
        //    4-5--6--7--------------
        //    3 \      \
        //    2  \      \
        //    1   \      \
        //    |\   xxxxxx7
        //    | \  x     6
        //    |  \ x     5
        //    |   \1x2x3x4
        //    |
        //
        //  The top-left neighbor array is updated with the reversed samples
        //    from the right column and bottom row of the source block
        //
        // Index = org_x - org_y
        */

    uint32_t idx;

    uint8_t* read_ptr;

    int32_t  dst_step;
    int32_t  read_step;
    uint32_t count;

    read_ptr = src_ptr_top; //+ ((block_height - 1) * stride);

    // Copy bottom row
    dst_ptr =
        //    topLeftArray_chkn+
        na_unit_ptr->top_left_array +
        svt_aom_get_neighbor_array_unit_top_left_index(na_unit_ptr, pic_origin_x, pic_origin_y + (block_height - 1)) *
            na_unit_ptr->unit_size;

    svt_memcpy(dst_ptr, read_ptr, block_width);

    // Reset read_ptr to the right-column
    read_ptr = src_ptr_left; // + (block_width - 1);

    // Copy right column
    dst_ptr =
        //  topLeftArray_chkn+
        na_unit_ptr->top_left_array +
        svt_aom_get_neighbor_array_unit_top_left_index(na_unit_ptr, pic_origin_x + (block_width - 1), pic_origin_y) *
            na_unit_ptr->unit_size;

    dst_step  = -1;
    read_step = 1; //stride;
    count     = block_height;

    for (idx = 0; idx < count; ++idx) {
        *dst_ptr = *read_ptr;

        dst_ptr += dst_step;
        read_ptr += read_step;
    }

    return;
}

void svt_aom_update_recon_neighbor_array16bit(NeighborArrayUnit* na_unit_ptr, uint16_t* src_ptr_top,
                                              uint16_t* src_ptr_left, uint32_t pic_origin_x, uint32_t pic_origin_y,
                                              uint32_t block_width, uint32_t block_height) {
    uint16_t* dst_ptr;
    dst_ptr = (uint16_t*)(na_unit_ptr->top_array +
                          get_neighbor_array_unit_top_index(na_unit_ptr, pic_origin_x) * na_unit_ptr->unit_size);
    svt_memcpy(dst_ptr, src_ptr_top, block_width * sizeof(uint16_t));

    dst_ptr = (uint16_t*)(na_unit_ptr->left_array +
                          get_neighbor_array_unit_left_index(na_unit_ptr, pic_origin_y) * na_unit_ptr->unit_size);
    svt_memcpy(dst_ptr, src_ptr_left, block_height * sizeof(uint16_t));

    //   Top-left Neighbor Array
    uint32_t  idx;
    uint16_t* read_ptr = src_ptr_top;
    int32_t   dst_step;
    int32_t   read_step;
    uint32_t  count;

    // Copy bottom row
    dst_ptr = (uint16_t*)(na_unit_ptr->top_left_array +
                          svt_aom_get_neighbor_array_unit_top_left_index(
                              na_unit_ptr, pic_origin_x, pic_origin_y + (block_height - 1)) *
                              na_unit_ptr->unit_size);
    svt_memcpy(dst_ptr, read_ptr, block_width * sizeof(uint16_t));

    // Reset read_ptr to the right-column
    read_ptr = src_ptr_left;

    // Copy right column
    dst_ptr = (uint16_t*)(na_unit_ptr->top_left_array +
                          svt_aom_get_neighbor_array_unit_top_left_index(
                              na_unit_ptr, pic_origin_x + (block_width - 1), pic_origin_y) *
                              na_unit_ptr->unit_size);

    dst_step  = -1;
    read_step = 1;
    count     = block_height;
    for (idx = 0; idx < count; ++idx) {
        *dst_ptr = *read_ptr;
        dst_ptr += dst_step;
        read_ptr += read_step;
    }

    return;
}

/*************************************************
 * Neighbor Array Sample Update
 *************************************************/
void svt_aom_neighbor_array_unit_sample_write(NeighborArrayUnit* na_unit_ptr, uint8_t* src_ptr, uint32_t stride,
                                              uint32_t src_origin_x, uint32_t src_origin_y, uint32_t pic_origin_x,
                                              uint32_t pic_origin_y, uint32_t block_width, uint32_t block_height,
                                              uint8_t neighbor_array_type_mask) {
    uint32_t idx;
    uint8_t* dst_ptr;
    uint8_t* read_ptr;

    int32_t  dst_step;
    int32_t  read_step;
    uint32_t count;

    // Adjust the Source ptr to start at the origin of the block being updated.
    src_ptr += ((src_origin_y * stride) + src_origin_x) * na_unit_ptr->unit_size;

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_TOP_MASK) {
        //
        //     ----------12345678---------------------  Top Neighbor Array
        //                ^    ^
        //                |    |
        //                |    |
        //               xxxxxxxx
        //               x      x
        //               x      x
        //               12345678
        //
        //  The top neighbor array is updated with the samples from the
        //    bottom row of the source block
        //
        //  Index = org_x

        // Adjust read_ptr to the bottom-row
        read_ptr = src_ptr + ((block_height - 1) * stride);

        dst_ptr = na_unit_ptr->top_array +
            get_neighbor_array_unit_top_index(na_unit_ptr, pic_origin_x) * na_unit_ptr->unit_size;

        dst_step  = na_unit_ptr->unit_size;
        read_step = na_unit_ptr->unit_size;
        count     = block_width;

        for (idx = 0; idx < count; ++idx) {
            *dst_ptr = *read_ptr;

            dst_ptr += dst_step;
            read_ptr += read_step;
        }
    }

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_LEFT_MASK) {
        //   Left Neighbor Array
        //
        //    |
        //    |
        //    1         xxxxxxx1
        //    2  <----  x      2
        //    3  <----  x      3
        //    4         xxxxxxx4
        //    |
        //    |
        //
        //  The left neighbor array is updated with the samples from the
        //    right column of the source block
        //
        //  Index = org_y

        // Adjust read_ptr to the right-column
        read_ptr = src_ptr + (block_width - 1);

        dst_ptr = na_unit_ptr->left_array +
            get_neighbor_array_unit_left_index(na_unit_ptr, pic_origin_y) * na_unit_ptr->unit_size;

        dst_step  = 1;
        read_step = stride;
        count     = block_height;

        for (idx = 0; idx < count; ++idx) {
            *dst_ptr = *read_ptr;

            dst_ptr += dst_step;
            read_ptr += read_step;
        }
    }

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_TOPLEFT_MASK) {
        /*
        //   Top-left Neighbor Array
        //
        //    4-5--6--7--------------
        //    3 \      \
        //    2  \      \
        //    1   \      \
        //    |\   xxxxxx7
        //    | \  x     6
        //    |  \ x     5
        //    |   \1x2x3x4
        //    |
        //
        //  The top-left neighbor array is updated with the reversed samples
        //    from the right column and bottom row of the source block
        //
        // Index = org_x - org_y
        */

        // Adjust read_ptr to the bottom-row
        read_ptr = src_ptr + ((block_height - 1) * stride);

        // Copy bottom row
        dst_ptr = na_unit_ptr->top_left_array +
            svt_aom_get_neighbor_array_unit_top_left_index(
                na_unit_ptr, pic_origin_x, pic_origin_y + (block_height - 1)) *
                na_unit_ptr->unit_size;

        svt_memcpy(dst_ptr, read_ptr, block_width);

        // Reset read_ptr to the right-column
        read_ptr = src_ptr + (block_width - 1);

        // Copy right column
        dst_ptr = na_unit_ptr->top_left_array +
            svt_aom_get_neighbor_array_unit_top_left_index(
                na_unit_ptr, pic_origin_x + (block_width - 1), pic_origin_y) *
                na_unit_ptr->unit_size;

        dst_step  = -1;
        read_step = stride;
        count     = block_height;

        for (idx = 0; idx < count; ++idx) {
            *dst_ptr = *read_ptr;

            dst_ptr += dst_step;
            read_ptr += read_step;
        }
    }

    return;
}

/*************************************************
 * Neighbor Array Sample Update for 16 bit case
 *************************************************/
void svt_aom_neighbor_array_unit16bit_sample_write(NeighborArrayUnit* na_unit_ptr, uint16_t* src_ptr, uint32_t stride,
                                                   uint32_t src_origin_x, uint32_t src_origin_y, uint32_t pic_origin_x,
                                                   uint32_t pic_origin_y, uint32_t block_width, uint32_t block_height,
                                                   uint8_t neighbor_array_type_mask) {
    uint32_t  idx;
    uint16_t* dst_ptr;
    uint16_t* read_ptr;

    int32_t  dst_step;
    int32_t  read_step;
    uint32_t count;

    // Adjust the Source ptr to start at the origin of the block being updated.
    src_ptr += ((src_origin_y * stride) + src_origin_x) /*CHKN  * na_unit_ptr->unit_size*/;

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_TOP_MASK) {
        //
        //     ----------12345678---------------------  Top Neighbor Array
        //                ^    ^
        //                |    |
        //                |    |
        //               xxxxxxxx
        //               x      x
        //               x      x
        //               12345678
        //
        //  The top neighbor array is updated with the samples from the
        //    bottom row of the source block
        //
        //  Index = org_x

        // Adjust read_ptr to the bottom-row
        read_ptr = src_ptr + ((block_height - 1) * stride);

        dst_ptr = (uint16_t*)(na_unit_ptr->top_array) +
            get_neighbor_array_unit_top_index(na_unit_ptr,
                                              pic_origin_x); //CHKN * na_unit_ptr->unit_size;

        count = block_width;

        for (idx = 0; idx < count; ++idx) {
            *dst_ptr = *read_ptr;

            dst_ptr += 1;
            read_ptr += 1;
        }
    }

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_LEFT_MASK) {
        //   Left Neighbor Array
        //
        //    |
        //    |
        //    1         xxxxxxx1
        //    2  <----  x      2
        //    3  <----  x      3
        //    4         xxxxxxx4
        //    |
        //    |
        //
        //  The left neighbor array is updated with the samples from the
        //    right column of the source block
        //
        //  Index = org_y

        // Adjust read_ptr to the right-column
        read_ptr = src_ptr + (block_width - 1);

        dst_ptr = (uint16_t*)(na_unit_ptr->left_array) +
            get_neighbor_array_unit_left_index(na_unit_ptr,
                                               pic_origin_y); //CHKN * na_unit_ptr->unit_size;

        dst_step  = 1;
        read_step = stride;
        count     = block_height;

        for (idx = 0; idx < count; ++idx) {
            *dst_ptr = *read_ptr;

            dst_ptr += dst_step;
            read_ptr += read_step;
        }
    }

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_TOPLEFT_MASK) {
        /*
        //   Top-left Neighbor Array
        //
        //    4-5--6--7--------------
        //    3 \      \
        //    2  \      \
        //    1   \      \
        //    |\   xxxxxx7
        //    | \  x     6
        //    |  \ x     5
        //    |   \1x2x3x4
        //    |
        //
        //  The top-left neighbor array is updated with the reversed samples
        //    from the right column and bottom row of the source block
        //
        // Index = org_x - org_y
        */

        // Adjust read_ptr to the bottom-row
        read_ptr = src_ptr + ((block_height - 1) * stride);

        // Copy bottom row
        dst_ptr = (uint16_t*)(na_unit_ptr->top_left_array) +
            svt_aom_get_neighbor_array_unit_top_left_index(
                      na_unit_ptr, pic_origin_x, pic_origin_y + (block_height - 1));

        dst_step  = 1;
        read_step = 1;
        count     = block_width;

        for (idx = 0; idx < count; ++idx) {
            *dst_ptr = *read_ptr;

            dst_ptr += dst_step;
            read_ptr += read_step;
        }

        // Reset read_ptr to the right-column
        read_ptr = src_ptr + (block_width - 1);

        // Copy right column
        dst_ptr = (uint16_t*)(na_unit_ptr->top_left_array) +
            svt_aom_get_neighbor_array_unit_top_left_index(na_unit_ptr,
                                                           pic_origin_x + (block_width - 1),
                                                           pic_origin_y); //CHKN  * na_unit_ptr->unit_size;

        dst_step  = -1;
        read_step = stride;
        count     = block_height;

        for (idx = 0; idx < count; ++idx) {
            *dst_ptr = *read_ptr;

            dst_ptr += dst_step;
            read_ptr += read_step;
        }
    }

    return;
}

/*************************************************
 * Neighbor Array Unit Mode Write
 *************************************************/
void svt_aom_neighbor_array_unit_mode_write32(NeighborArrayUnit32* na_unit_ptr, uint32_t value, uint32_t org_x,
                                              uint32_t org_y, uint32_t block_width, uint32_t block_height,
                                              uint8_t neighbor_array_type_mask) {
    uint32_t  idx;
    uint32_t* dst_ptr;

    uint32_t count;
    uint32_t na_offset;
    uint32_t na_unit_size;

    na_unit_size = 1; //na_unit_ptr->unit_size;

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_TOP_MASK) {
        //
        //     ----------12345678---------------------  Top Neighbor Array
        //                ^    ^
        //                |    |
        //                |    |
        //               xxxxxxxx
        //               x      x
        //               x      x
        //               12345678
        //
        //  The top neighbor array is updated with the samples from the
        //    bottom row of the source block
        //
        //  Index = org_x

        na_offset = get_neighbor_array_unit_top_index32(na_unit_ptr, org_x);

        dst_ptr = na_unit_ptr->top_array + na_offset * na_unit_size;

        count = block_width >> na_unit_ptr->granularity_normal_log2;

        for (idx = 0; idx < count; ++idx) {
            memset32bit(dst_ptr, value, na_unit_size);

            dst_ptr += na_unit_size;
        }
    }

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_LEFT_MASK) {
        //   Left Neighbor Array
        //
        //    |
        //    |
        //    1         xxxxxxx1
        //    2  <----  x      2
        //    3  <----  x      3
        //    4         xxxxxxx4
        //    |
        //    |
        //
        //  The left neighbor array is updated with the samples from the
        //    right column of the source block
        //
        //  Index = org_y

        na_offset = get_neighbor_array_unit_left_index32(na_unit_ptr, org_y);

        dst_ptr = na_unit_ptr->left_array + na_offset * na_unit_size;

        count = block_height >> na_unit_ptr->granularity_normal_log2;

        for (idx = 0; idx < count; ++idx) {
            memset32bit(dst_ptr, value, na_unit_size);

            dst_ptr += na_unit_size;
        }
    }

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_TOPLEFT_MASK) {
        /*
        //   Top-left Neighbor Array
        //
        //    4-5--6--7------------
        //    3 \      \
        //    2  \      \
        //    1   \      \
        //    |\   xxxxxx7
        //    | \  x     6
        //    |  \ x     5
        //    |   \1x2x3x4
        //    |
        //
        //  The top-left neighbor array is updated with the reversed samples
        //    from the right column and bottom row of the source block
        //
        // Index = org_x - org_y
        */

        na_offset = get_neighbor_array_unit_top_left_index_32(na_unit_ptr, org_x, org_y + (block_height - 1));

        // Copy bottom-row + right-column
        // *Note - start from the bottom-left corner
        dst_ptr = na_unit_ptr->top_left_array + na_offset * na_unit_size;

        count = ((block_width + block_height) >> na_unit_ptr->granularity_top_left_log2) - 1;

        for (idx = 0; idx < count; ++idx) {
            memset32bit(dst_ptr, value, na_unit_size);

            dst_ptr += na_unit_size;
        }
    }

    return;
}

void svt_aom_neighbor_array_unit_mode_write(NeighborArrayUnit* na_unit_ptr, uint8_t* value, uint32_t org_x,
                                            uint32_t org_y, uint32_t block_width, uint32_t block_height,
                                            uint8_t neighbor_array_type_mask) {
    uint32_t idx, j;
    uint8_t* dst_ptr;

    uint32_t count;
    uint32_t na_offset;
    uint32_t na_unit_size;

    na_unit_size = na_unit_ptr->unit_size;

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_TOP_MASK) {
        //
        //     ----------12345678---------------------  Top Neighbor Array
        //                ^    ^
        //                |    |
        //                |    |
        //               xxxxxxxx
        //               x      x
        //               x      x
        //               12345678
        //
        //  The top neighbor array is updated with the samples from the
        //    bottom row of the source block
        //
        //  Index = org_x

        na_offset = get_neighbor_array_unit_top_index(na_unit_ptr, org_x);

        dst_ptr = na_unit_ptr->top_array + na_offset * na_unit_size;

        count = block_width >> na_unit_ptr->granularity_normal_log2;

        for (idx = 0; idx < count; ++idx) {
            /* svt_memcpy less that 10 bytes*/
            for (j = 0; j < na_unit_size; ++j) {
                dst_ptr[j] = value[j];
            }

            dst_ptr += na_unit_size;
        }
    }

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_LEFT_MASK) {
        //   Left Neighbor Array
        //
        //    |
        //    |
        //    1         xxxxxxx1
        //    2  <----  x      2
        //    3  <----  x      3
        //    4         xxxxxxx4
        //    |
        //    |
        //
        //  The left neighbor array is updated with the samples from the
        //    right column of the source block
        //
        //  Index = org_y

        na_offset = get_neighbor_array_unit_left_index(na_unit_ptr, org_y);

        dst_ptr = na_unit_ptr->left_array + na_offset * na_unit_size;

        count = block_height >> na_unit_ptr->granularity_normal_log2;

        for (idx = 0; idx < count; ++idx) {
            /* svt_memcpy less that 10 bytes*/
            for (j = 0; j < na_unit_size; ++j) {
                dst_ptr[j] = value[j];
            }

            dst_ptr += na_unit_size;
        }
    }

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_TOPLEFT_MASK) {
        /*
        //   Top-left Neighbor Array
        //
        //    4-5--6--7------------
        //    3 \      \
        //    2  \      \
        //    1   \      \
        //    |\   xxxxxx7
        //    | \  x     6
        //    |  \ x     5
        //    |   \1x2x3x4
        //    |
        //
        //  The top-left neighbor array is updated with the reversed samples
        //    from the right column and bottom row of the source block
        //
        // Index = org_x - org_y
        */

        na_offset = svt_aom_get_neighbor_array_unit_top_left_index(na_unit_ptr, org_x, org_y + (block_height - 1));

        // Copy bottom-row + right-column
        // *Note - start from the bottom-left corner
        dst_ptr = na_unit_ptr->top_left_array + na_offset * na_unit_size;

        count = ((block_width + block_height) >> na_unit_ptr->granularity_top_left_log2) - 1;

        for (idx = 0; idx < count; ++idx) {
            /* svt_memcpy less that 10 bytes*/
            for (j = 0; j < na_unit_size; ++j) {
                dst_ptr[j] = value[j];
            }

            dst_ptr += na_unit_size;
        }
    }

    return;
}

void svt_aom_copy_neigh_arr(NeighborArrayUnit* na_src, NeighborArrayUnit* na_dst, uint32_t org_x, uint32_t org_y,
                            uint32_t bw, uint32_t bh, uint8_t neighbor_array_type_mask) {
    uint32_t idx;
    uint8_t *dst_ptr, *src_ptr;

    uint32_t count;
    uint32_t na_offset;
    uint32_t na_unit_size;

    UNUSED(idx);
    na_unit_size = na_src->unit_size;

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_TOP_MASK) {
        na_offset = get_neighbor_array_unit_top_index(na_src, org_x);
        src_ptr   = na_src->top_array + na_offset * na_unit_size;
        dst_ptr   = na_dst->top_array + na_offset * na_unit_size;
        count     = bw >> na_src->granularity_normal_log2;

        svt_memcpy(dst_ptr, src_ptr, na_unit_size * count);
    }

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_LEFT_MASK) {
        na_offset = get_neighbor_array_unit_left_index(na_src, org_y);
        src_ptr   = na_src->left_array + na_offset * na_unit_size;
        dst_ptr   = na_dst->left_array + na_offset * na_unit_size;
        count     = bh >> na_src->granularity_normal_log2;

        svt_memcpy(dst_ptr, src_ptr, na_unit_size * count);
    }

    if (neighbor_array_type_mask & NEIGHBOR_ARRAY_UNIT_TOPLEFT_MASK) {
        /*
        //   Top-left Neighbor Array
        //
        //    4-5--6--7------------
        //    3 \      \
        //    2  \      \
        //    1   \      \
        //    |\   xxxxxx7
        //    | \  x     6
        //    |  \ x     5
        //    |   \1x2x3x4
        //    |
        //
        //  The top-left neighbor array is updated with the reversed samples
        //    from the right column and bottom row of the source block
        //
        // Index = org_x - org_y
        */

        na_offset = svt_aom_get_neighbor_array_unit_top_left_index(na_src, org_x, org_y + (bh - 1));

        // Copy bottom-row + right-column
        // *Note - start from the bottom-left corner
        src_ptr = na_src->top_left_array + na_offset * na_unit_size;
        dst_ptr = na_dst->top_left_array + na_offset * na_unit_size;

        count = ((bw + bh) >> na_src->granularity_top_left_log2) - 1;

        svt_memcpy(dst_ptr, src_ptr, na_unit_size * count);
    }

    return;
}
