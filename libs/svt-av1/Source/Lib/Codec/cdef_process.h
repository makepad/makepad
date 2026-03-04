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

#ifndef EbCdefProcess_h
#define EbCdefProcess_h

#include "sys_resource_manager.h"
#include "object.h"

/**************************************
 * Extern Function Declarations
 **************************************/
EbErrorType svt_aom_cdef_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr, int index);

void* svt_aom_cdef_kernel(void* input_ptr);

#endif
