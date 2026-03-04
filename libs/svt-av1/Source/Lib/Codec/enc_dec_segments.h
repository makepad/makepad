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

#ifndef EbEncDecSegments_h
#define EbEncDecSegments_h

#include "definitions.h"
#include "svt_threads.h"
#include "object.h"
#ifdef __cplusplus
extern "C" {
#endif
/**************************************
     * Defines
     **************************************/
#define ENCDEC_SEGMENTS_MAX_COL_COUNT 60
#define ENCDEC_SEGMENTS_MAX_ROW_COUNT 37
#define ENCDEC_SEGMENTS_MAX_BAND_COUNT ENCDEC_SEGMENTS_MAX_COL_COUNT + ENCDEC_SEGMENTS_MAX_ROW_COUNT

/**************************************
      * Macros
      **************************************/
#define BAND_TOTAL_COUNT(sb_row_total_count, sb_col_total_count) ((sb_row_total_count) + (sb_col_total_count) - 1)
#define ROW_INDEX(y_sb_index, segment_row_count, sb_row_total_count) \
    (((y_sb_index) * (segment_row_count)) / (sb_row_total_count))
#define BAND_INDEX(x_sb_index, y_sb_index, segment_band_count, sb_band_total_count) \
    ((((x_sb_index) + (y_sb_index)) * (segment_band_count)) / (sb_band_total_count))
#define SEGMENT_INDEX(row_index, band_index, segment_band_count) (((row_index) * (segment_band_count)) + (band_index))

/**************************************
       * Member definitions
       **************************************/
typedef struct EncDecSegDependencyMap {
    uint8_t* dependency_map;
    EbHandle update_mutex;
} EncDecSegDependencyMap;

typedef struct EncDecSegSegmentRow {
    uint16_t starting_seg_index;
    uint16_t ending_seg_index;
    uint16_t current_seg_index;
    EbHandle assignment_mutex;
} EncDecSegSegmentRow;

/**************************************
     * ENCDEC Segments
     **************************************/
typedef struct EncDecSegments {
    EbDctor                dctor;
    EncDecSegDependencyMap dep_map;
    EncDecSegSegmentRow*   row_array;

    uint16_t* x_start_array;
    uint16_t* y_start_array;
    uint16_t* valid_sb_count_array;

    uint32_t segment_band_count;
    uint32_t segment_row_count;
    uint32_t segment_ttl_count;
    uint32_t sb_band_count;
    uint32_t sb_row_count;

    uint32_t segment_max_band_count;
    uint32_t segment_max_row_count;
    uint32_t segment_max_total_count;
} EncDecSegments;

/**************************************
     * Extern Function Declarations
     **************************************/
EbErrorType svt_aom_enc_dec_segments_ctor(EncDecSegments* segments_ptr, uint32_t segment_col_count,
                                          uint32_t segment_row_count);

void svt_aom_enc_dec_segments_init(EncDecSegments* segments_ptr, uint32_t col_count, uint32_t row_count,
                                   uint32_t pic_width_sb, uint32_t pic_height_sb);
#ifdef __cplusplus
}
#endif
#endif // EbEncDecResults_h
