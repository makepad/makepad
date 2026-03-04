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

#include "enc_dec_segments.h"

static void enc_dec_segments_dctor(EbPtr p) {
    EncDecSegments* obj = (EncDecSegments*)p;
    uint32_t        row_index;
    for (row_index = 0; row_index < obj->segment_max_row_count; ++row_index) {
        EB_DESTROY_MUTEX(obj->row_array[row_index].assignment_mutex);
    }
    EB_DESTROY_MUTEX(obj->dep_map.update_mutex);
    EB_FREE_ARRAY(obj->x_start_array);
    EB_FREE_ARRAY(obj->y_start_array);
    EB_FREE_ARRAY(obj->valid_sb_count_array);
    EB_FREE_ARRAY(obj->dep_map.dependency_map);
    EB_FREE_ARRAY(obj->row_array);

    EB_FREE_ARRAY(obj->dep_map.dependency_map);
    EB_FREE_ARRAY(obj->valid_sb_count_array);
    EB_FREE_ARRAY(obj->y_start_array);
    EB_FREE_ARRAY(obj->x_start_array);
}

EbErrorType svt_aom_enc_dec_segments_ctor(EncDecSegments* segments_ptr, uint32_t segment_col_count,
                                          uint32_t segment_row_count) {
    uint32_t row_index;

    segments_ptr->dctor = enc_dec_segments_dctor;

    segments_ptr->segment_max_row_count   = segment_row_count;
    segments_ptr->segment_max_band_count  = segment_row_count + segment_col_count;
    segments_ptr->segment_max_total_count = segments_ptr->segment_max_row_count * segments_ptr->segment_max_band_count;

    // Start Arrays
    EB_MALLOC_ARRAY(segments_ptr->x_start_array, segments_ptr->segment_max_total_count);

    EB_MALLOC_ARRAY(segments_ptr->y_start_array, segments_ptr->segment_max_total_count);

    EB_MALLOC_ARRAY(segments_ptr->valid_sb_count_array, segments_ptr->segment_max_total_count);

    // Dependency map
    EB_MALLOC_ARRAY(segments_ptr->dep_map.dependency_map, segments_ptr->segment_max_total_count);

    EB_CREATE_MUTEX(segments_ptr->dep_map.update_mutex);

    // Segment rows
    EB_MALLOC_ARRAY(segments_ptr->row_array, segments_ptr->segment_max_row_count);
    for (row_index = 0; row_index < segments_ptr->segment_max_row_count; ++row_index) {
        segments_ptr->row_array[row_index].assignment_mutex = NULL;
    }

    for (row_index = 0; row_index < segments_ptr->segment_max_row_count; ++row_index) {
        EB_CREATE_MUTEX(segments_ptr->row_array[row_index].assignment_mutex);
    }

    return EB_ErrorNone;
}

void svt_aom_enc_dec_segments_init(EncDecSegments* segments_ptr, uint32_t segColCount, uint32_t segRowCount,
                                   uint32_t pic_width_sb, uint32_t pic_height_sb) {
    segColCount = (segColCount < pic_width_sb) ? segColCount : pic_width_sb;
    segRowCount = (segRowCount < pic_height_sb) ? segRowCount : pic_height_sb;
    segRowCount = (segRowCount < segments_ptr->segment_max_row_count) ? segRowCount
                                                                      : segments_ptr->segment_max_row_count;

    segments_ptr->sb_row_count       = pic_height_sb;
    segments_ptr->sb_band_count      = BAND_TOTAL_COUNT(pic_height_sb, pic_width_sb);
    segments_ptr->segment_row_count  = segRowCount;
    segments_ptr->segment_band_count = BAND_TOTAL_COUNT(segRowCount, segColCount);
    segments_ptr->segment_ttl_count  = segments_ptr->segment_row_count * segments_ptr->segment_band_count;

    //EB_MEMSET(segments_ptr->inputMap.inputDependencyMap, 0, sizeof(uint16_t) * segments_ptr->segment_ttl_count);
    EB_MEMSET(segments_ptr->valid_sb_count_array, 0, sizeof(uint16_t) * segments_ptr->segment_ttl_count);
    EB_MEMSET(segments_ptr->x_start_array, -1, sizeof(uint16_t) * segments_ptr->segment_ttl_count);
    EB_MEMSET(segments_ptr->y_start_array, -1, sizeof(uint16_t) * segments_ptr->segment_ttl_count);

    // Initialize the per-SB input availability map & Start Arrays
    for (unsigned y = 0; y < pic_height_sb; ++y) {
        for (unsigned x = 0; x < pic_width_sb; ++x) {
            unsigned band_index    = BAND_INDEX(x, y, segments_ptr->segment_band_count, segments_ptr->sb_band_count);
            unsigned row_index     = ROW_INDEX(y, segments_ptr->segment_row_count, segments_ptr->sb_row_count);
            unsigned segment_index = SEGMENT_INDEX(row_index, band_index, segments_ptr->segment_band_count);

            //++segments_ptr->inputMap.inputDependencyMap[segment_index];
            ++segments_ptr->valid_sb_count_array[segment_index];
            segments_ptr->x_start_array[segment_index] = (segments_ptr->x_start_array[segment_index] == (uint16_t)-1)
                ? (uint16_t)x
                : segments_ptr->x_start_array[segment_index];
            segments_ptr->y_start_array[segment_index] = (segments_ptr->y_start_array[segment_index] == (uint16_t)-1)
                ? (uint16_t)y
                : segments_ptr->y_start_array[segment_index];
        }
    }

    // Initialize the row-based controls
    for (unsigned row_index = 0; row_index < segments_ptr->segment_row_count; ++row_index) {
        unsigned y = ((row_index * segments_ptr->sb_row_count) + (segments_ptr->segment_row_count - 1)) /
            segments_ptr->segment_row_count;
        unsigned y_last = ((((row_index + 1) * segments_ptr->sb_row_count) + (segments_ptr->segment_row_count - 1)) /
                           segments_ptr->segment_row_count) -
            1;
        unsigned band_index = BAND_INDEX(0, y, segments_ptr->segment_band_count, segments_ptr->sb_band_count);

        segments_ptr->row_array[row_index].starting_seg_index = (uint16_t)SEGMENT_INDEX(
            row_index, band_index, segments_ptr->segment_band_count);
        band_index = BAND_INDEX(
            pic_width_sb - 1, y_last, segments_ptr->segment_band_count, segments_ptr->sb_band_count);
        segments_ptr->row_array[row_index].ending_seg_index = (uint16_t)SEGMENT_INDEX(
            row_index, band_index, segments_ptr->segment_band_count);
        segments_ptr->row_array[row_index].current_seg_index = segments_ptr->row_array[row_index].starting_seg_index;
    }

    // Initialize the per-segment dependency map
    EB_MEMSET(segments_ptr->dep_map.dependency_map, 0, sizeof(uint8_t) * segments_ptr->segment_ttl_count);
    for (unsigned row_index = 0; row_index < segments_ptr->segment_row_count; ++row_index) {
        for (unsigned segment_index = segments_ptr->row_array[row_index].starting_seg_index;
             segment_index <= segments_ptr->row_array[row_index].ending_seg_index;
             ++segment_index) {
            // Check that segment is valid
            if (segments_ptr->valid_sb_count_array[segment_index]) {
                // Right Neighbor
                if (segment_index < segments_ptr->row_array[row_index].ending_seg_index) {
                    ++segments_ptr->dep_map.dependency_map[segment_index + 1];
                }
                // Bottom Neighbor
                if (row_index < segments_ptr->segment_row_count - 1 &&
                    segment_index + segments_ptr->segment_band_count >=
                        segments_ptr->row_array[row_index + 1].starting_seg_index) {
                    ++segments_ptr->dep_map.dependency_map[segment_index + segments_ptr->segment_band_count];
                }
            }
        }
    }

    return;
}
