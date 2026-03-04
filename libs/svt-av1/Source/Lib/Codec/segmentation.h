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

#ifndef SVT_AV1_EBSEGMENTATION_H
#define SVT_AV1_EBSEGMENTATION_H

#include "definitions.h"
#include "pcs.h"
#include "sequence_control_set.h"
#include "rc_process.h"
#include "utility.h"

void svt_aom_apply_segmentation_based_quantization(PictureControlSet* pcs, SuperBlock* sb_ptr, BlkStruct* blk_ptr,
                                                   const BlockSize bsize, const int org_x, const int org_y);

void svt_aom_setup_segmentation(PictureControlSet* pcs, SequenceControlSet* scs);

void find_segment_qps(SegmentationParams* segmentation_params, PictureControlSet* pcs);

void temporally_update_qps(int32_t* segment_qp_ptr, int32_t* prev_segment_qp_ptr, bool temporal_update);

void calculate_segmentation_data(SegmentationParams* segmentation_params);

#endif //SVT_AV1_EBSEGMENTATIONS_H
