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

#include "pd_reorder_queue.h"

EbErrorType svt_aom_picture_decision_reorder_entry_ctor(PictureDecisionReorderEntry* entry_ptr,
                                                        uint32_t                     picture_number) {
    entry_ptr->picture_number = picture_number;
    return EB_ErrorNone;
}
