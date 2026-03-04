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

#ifndef EbEncDecTasks_h
#define EbEncDecTasks_h

#include "definitions.h"
#include "sys_resource_manager.h"
#include "object.h"
#ifdef __cplusplus
extern "C" {
#endif
#define ENCDEC_TASKS_MDC_INPUT 0
#define ENCDEC_TASKS_ENCDEC_INPUT 1
#define ENCDEC_TASKS_CONTINUE 2
#define ENCDEC_TASKS_SUPERRES_INPUT 3

/**************************************
 * Process Results
 **************************************/
typedef struct EncDecTasks {
    EbDctor          dctor;
    EbObjectWrapper* pcs_wrapper;
    uint32_t         input_type;
    int16_t          enc_dec_segment_row;
    uint16_t         tile_group_index;
} EncDecTasks;

typedef struct EncDecTasksInitData {
    unsigned enc_dec_segment_row_count;
} EncDecTasksInitData;

/**************************************
     * Extern Function Declarations
     **************************************/
EbErrorType svt_aom_enc_dec_tasks_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr);

#ifdef __cplusplus
}
#endif
#endif // EbEncDecTasks_h
