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

#ifndef EbResourceCoordinationResults_h
#define EbResourceCoordinationResults_h

#include "definitions.h"
#include "sys_resource_manager.h"
#include "object.h"
#ifdef __cplusplus
extern "C" {
#endif

typedef struct InputCommand {
    EbDctor          dctor;
    EbObjectWrapper* eb_input_wrapper_ptr;
    EbObjectWrapper* y8b_wrapper;
} InputCommand;

/**************************************
 * Process Results
 **************************************/
typedef struct ResourceCoordinationResults {
    EbDctor          dctor;
    EbObjectWrapper* pcs_wrapper;
} ResourceCoordinationResults;

typedef struct ResourceCoordinationResultInitData {
    int32_t junk;
} ResourceCoordinationResultInitData;

/**************************************
     * Extern Function Declarations
     **************************************/
EbErrorType svt_aom_resource_coordination_result_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);

#ifdef __cplusplus
}
#endif
#endif //EbResourceCoordinationResults_h
