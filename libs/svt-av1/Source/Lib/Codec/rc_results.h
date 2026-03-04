/*
* Copyright (c) 2019, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef EbRateControlResults_h
#define EbRateControlResults_h

#include "definitions.h"
#include "sys_resource_manager.h"
#include "object.h"
#ifdef __cplusplus
extern "C" {
#endif
/**************************************
 * Process Results
 **************************************/
typedef struct RateControlResults {
    EbDctor          dctor;
    EbObjectWrapper* pcs_wrapper;
    bool             superres_recode;
} RateControlResults;

typedef struct RateControlResultsInitData {
    int32_t junk;
} RateControlResultsInitData;

/**************************************
 * Extern Function Declarations
 **************************************/
EbErrorType svt_aom_rate_control_results_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);

#ifdef __cplusplus
}
#endif
#endif // EbRateControlResults_h
