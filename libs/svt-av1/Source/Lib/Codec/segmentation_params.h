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

#ifndef SVT_AV1_EBSEGMENTATIONPARAMS_H
#define SVT_AV1_EBSEGMENTATIONPARAMS_H
#include "object.h"

enum {
    SEG_LVL_ALT_Q, // Use alternate Quantizer
    SEG_LVL_ALT_LF_Y_V, // Use alternate loop filter value on y plane vertical
    SEG_LVL_ALT_LF_Y_H, // Use alternate loop filter value on y plane horizontal
    SEG_LVL_ALT_LF_U, // Use alternate loop filter value on u plane
    SEG_LVL_ALT_LF_V, // Use alternate loop filter value on v plane
    SEG_LVL_REF_FRAME, // Optional Segment reference frame
    SEG_LVL_SKIP, // Optional Segment (0,0) + skip mode
    SEG_LVL_GLOBALMV,
    SEG_LVL_MAX
} UENUM1BYTE(SEG_LVL_FEATURES);

typedef struct {
    EbDctor  dctor;
    uint8_t* data;
    uint32_t map_size;
} SegmentationNeighborMap;

typedef struct {
    /*!< 1: Indicates that this frame makes use of the segmentation tool
    *   0: Indicates that the frame does not use segmentation*/
    uint8_t segmentation_enabled;

    /*!< 1: Indicates that the segmentation map are updated during the decoding
     *      of this frame
     *   0: Indicates that the segmentation map from the previous frame is used*/
    uint8_t segmentation_update_map;

    /*!< 1: Indicates that the updates to the segmentation map are coded
     *      relative to the existing segmentation map
     *   0: Indicates that the new segmentation map is coded without reference
     *      to the existing segmentation map */
    uint8_t segmentation_temporal_update;

    /*!<1: Indicates that new parameters are about to be specified for each segment
     *  0: Indicates that the segmentation parameters should keep their existing
     *     values*/
    uint8_t segmentation_update_data;

    /*!< Specifies the feature data for a segment feature */
    int16_t feature_data[MAX_SEGMENTS][SEG_LVL_MAX];

    /*!< Specifies the feature enabled for a segment feature */
    int16_t feature_enabled[MAX_SEGMENTS][SEG_LVL_MAX];

    /*!< Specifies the feature enabled for a segment feature */
    int16_t seg_qm_level[MAX_SEGMENTS][SEG_LVL_MAX];

    /*!< Specifies the highest numbered segment id that has some enabled feature*/
    uint8_t last_active_seg_id;

    /*!< 1: Indicates that the segment id will be read before the skip syntax element
     *   0: Indicates that the skip syntax element will be read first */
    uint8_t seg_id_pre_skip;

    //qp-binning related
    int16_t variance_bin_edge[MAX_SEGMENTS];

} SegmentationParams;

extern const int svt_aom_segmentation_feature_bits[SEG_LVL_MAX];
extern const int svt_aom_segmentation_feature_signed[SEG_LVL_MAX];
extern const int svt_aom_segmentation_feature_max[SEG_LVL_MAX];

#endif //SVT_AV1_EBSEGMENTATIONPARAMS_H
