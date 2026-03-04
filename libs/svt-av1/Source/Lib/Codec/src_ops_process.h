/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef EbSourceBasedOperations_h
#define EbSourceBasedOperations_h

#include "definitions.h"

/***************************************
 * Extern Function Declaration
 ***************************************/
EbErrorType svt_aom_tpl_disp_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr, int index,
                                          int tasks_index);
void*       svt_aom_tpl_disp_kernel(void* input_ptr);
void        generate_lambda_scaling_factor(PictureParentControlSet* pcs, int64_t mc_dep_cost_base);
void        svt_aom_generate_r0beta(PictureParentControlSet* pcs);
EbErrorType svt_aom_source_based_operations_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                                         int tpl_index, int index);

void*        svt_aom_source_based_operations_kernel(void* input_ptr);
unsigned int svt_aom_get_perpixel_variance(const uint8_t* buf, uint32_t stride, const int block_size);
#endif // EbSourceBasedOperations_h
