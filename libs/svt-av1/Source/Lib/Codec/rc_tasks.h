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

#ifndef EbRateControlTasks_h
#define EbRateControlTasks_h

#include "definitions.h"
#include "sys_resource_manager.h"
#include "object.h"

/**************************************
 * Tasks Types
 **************************************/
typedef enum RateControlTaskTypes {
    RC_INPUT,
    RC_PACKETIZATION_FEEDBACK_RESULT,
    RC_INPUT_SUPERRES_RECODE,
    RC_INVALID_TASK
} RateControlTaskTypes;

/**************************************
 * Process Results
 **************************************/
typedef struct RateControlTasks {
    EbDctor              dctor;
    RateControlTaskTypes task_type;
    EbObjectWrapper*     pcs_wrapper;
} RateControlTasks;

typedef struct RateControlTasksInitData {
    int32_t junk;
} RateControlTasksInitData;

/**************************************
 * Extern Function Declarations
 **************************************/
EbErrorType svt_aom_rate_control_tasks_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);

#endif // EbRateControlTasks_h
