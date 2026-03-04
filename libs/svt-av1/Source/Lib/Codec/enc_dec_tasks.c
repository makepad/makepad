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

#include <stdlib.h>

#include "enc_dec_tasks.h"

static EbErrorType enc_dec_tasks_ctor(EncDecTasks* context_ptr, EbPtr object_init_data_ptr) {
    (void)context_ptr;
    (void)object_init_data_ptr;

    return EB_ErrorNone;
}

EbErrorType svt_aom_enc_dec_tasks_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    EncDecTasks* obj;

    *object_dbl_ptr = NULL;
    EB_NEW(obj, enc_dec_tasks_ctor, object_init_data_ptr);
    *object_dbl_ptr = obj;

    return EB_ErrorNone;
}
