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

#ifndef EbPictureResults_h
#define EbPictureResults_h

#include "sys_resource_manager.h"
#include "object.h"

/**************************************
 * Enums
 **************************************/
typedef enum EbPicType {
    EB_PIC_INVALID        = 0,
    EB_PIC_INPUT          = 1,
    EB_PIC_REFERENCE      = 2,
    EB_PIC_FEEDBACK       = 3,
    EB_PIC_SUPERRES_INPUT = 4
} EbPicType;

/**************************************
 * Picture Demux Results
 **************************************/
typedef struct PictureDemuxResults {
    EbDctor   dctor;
    EbPicType picture_type;

    // Only valid for input pictures
    EbObjectWrapper* pcs_wrapper;

    // Only valid for reference pictures
    EbObjectWrapper*           ref_pic_wrapper;
    struct SequenceControlSet* scs;
    uint64_t                   picture_number;
    uint64_t                   decode_order;
} PictureDemuxResults;

typedef struct PictureResultInitData {
    int32_t junk;
} PictureResultInitData;

/**************************************
 * Extern Function Declarations
 **************************************/
EbErrorType svt_aom_picture_results_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);
#endif //EbPictureResults_h
