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

#ifndef EbPictureAnalysisResults_h
#define EbPictureAnalysisResults_h

#include "definitions.h"
#include "sys_resource_manager.h"
#include "object.h"

/**************************************
 * Process Results
 **************************************/
typedef struct PictureAnalysisResults {
    EbDctor          dctor;
    EbObjectWrapper* pcs_wrapper;
} PictureAnalysisResults;

typedef struct PictureAnalysisResultInitData {
    int32_t junk;
} PictureAnalysisResultInitData;

/**************************************
 * Extern Function Declarations
 **************************************/
EbErrorType svt_aom_picture_analysis_result_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);

#endif //EbPictureAnalysisResults_h
