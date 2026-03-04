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

#ifndef EbPictureDecisionResults_h
#define EbPictureDecisionResults_h

#include "definitions.h"
#include "sys_resource_manager.h"
#include "object.h"
#include "pcs.h"

/**************************************
 * Process Results
 **************************************/
typedef struct PictureDecisionResults {
    EbDctor                     dctor;
    EbObjectWrapper*            pcs_wrapper;
    uint32_t                    segment_index;
    uint8_t                     task_type; // 0:ME   1:Temporal Filtering
    uint8_t                     lst0_cnt;
    uint8_t                     lst1_cnt;
    uint8_t                     tmp_layer_idx;
    uint8_t                     is_reference;
    uint8_t                     sc_class0;
    uint8_t                     sc_class1;
    uint8_t                     sc_class2;
    EbDownScaledBufDescPtrArray ref_ds[MAX_NUM_OF_REF_PIC_LIST][REF_LIST_MAX_DEPTH];
} PictureDecisionResults;

typedef struct PictureDecisionResultInitData {
    int32_t junk;
} PictureDecisionResultInitData;

/**************************************
 * Extern Function Declarations
 **************************************/
EbErrorType svt_aom_picture_decision_result_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);

#endif //EbPictureDecisionResults_h
