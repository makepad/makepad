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

#ifndef EbInitialRateControl_h
#define EbInitialRateControl_h

#include "definitions.h"
#include "sys_resource_manager.h"
#include "rc_process.h"

/***************************************
 * Extern Function Declaration
 ***************************************/
EbErrorType svt_aom_initial_rate_control_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                                      uint32_t ppcs_count);

void* svt_aom_initial_rate_control_kernel(void* input_ptr);
#endif // EbInitialRateControl_h
