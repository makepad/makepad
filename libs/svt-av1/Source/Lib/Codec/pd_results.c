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

#include "pd_results.h"

static EbErrorType picture_decision_result_ctor(PictureDecisionResults* object_ptr, EbPtr object_init_data_ptr) {
    (void)object_ptr;
    (void)object_init_data_ptr;

    return EB_ErrorNone;
}

EbErrorType svt_aom_picture_decision_result_creator(EbPtr* object_dbl_ptr, EbPtr object_init_data_ptr) {
    PictureDecisionResults* obj;

    *object_dbl_ptr = NULL;
    EB_NEW(obj, picture_decision_result_ctor, object_init_data_ptr);
    *object_dbl_ptr = obj;

    return EB_ErrorNone;
}
