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

#ifndef EbEstimationProcess_h
#define EbEstimationProcess_h

#include "definitions.h"
#include "sequence_control_set.h"
#include "me_context.h"

/**************************************
 * Context
 **************************************/
typedef struct MotionEstimationContext {
    EbFifo*    picture_decision_results_input_fifo_ptr;
    EbFifo*    motion_estimation_results_output_fifo_ptr;
    MeContext* me_ctx;

    uint8_t* index_table0;
    uint8_t* index_table1;
} MotionEstimationContext_t;

typedef struct InLoopMeContext {
    EbFifo*    input_fifo_ptr;
    EbFifo*    output_fifo_ptr;
    MeContext* me_ctx;

    uint8_t* index_table0;
    uint8_t* index_table1;
} InLoopMeContext;

/***************************************
 * Extern Function Declaration
 ***************************************/
EbErrorType svt_aom_motion_estimation_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                                   int index);

void* svt_aom_motion_estimation_kernel(void* input_ptr);

void svt_aom_gm_pre_processor(PictureParentControlSet* pcs, PictureParentControlSet** pcs_list);

#endif // EbMotionEstimationProcess_h
