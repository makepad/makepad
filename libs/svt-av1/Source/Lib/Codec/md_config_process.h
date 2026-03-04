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

#ifndef EbModeDecisionConfigurationProcess_h
#define EbModeDecisionConfigurationProcess_h

#include "definitions.h"
#include "mode_decision.h"
#include "sys_resource_manager.h"
#include "md_rate_estimation.h"
#include "rc_process.h"
#include "sequence_control_set.h"
#include "object.h"
#include "inv_transforms.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ModeDecisionConfigurationContext {
    EbFifo* rate_control_input_fifo_ptr;
    EbFifo* mode_decision_configuration_output_fifo_ptr;
} ModeDecisionConfigurationContext;

/**************************************
 * Extern Function Declarations
 **************************************/
EbErrorType svt_aom_mode_decision_configuration_context_ctor(EbThreadContext*   thread_ctx,
                                                             const EbEncHandle* enc_handle_ptr, int input_index,
                                                             int output_index);

void svt_av1_build_quantizer(PictureParentControlSet* pcs, EbBitDepth bit_depth, int32_t y_dc_delta_q,
                             int32_t u_dc_delta_q, int32_t u_ac_delta_q, int32_t v_dc_delta_q, int32_t v_ac_delta_q,
                             Quants* const quants, Dequants* const deq);

void* svt_aom_mode_decision_configuration_kernel(void* input_ptr);

#ifdef __cplusplus
}
#endif
#endif // EbModeDecisionConfigurationProcess_h
