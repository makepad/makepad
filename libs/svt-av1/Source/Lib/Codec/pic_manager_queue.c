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
#include "pic_manager_queue.h"

EbErrorType svt_aom_input_queue_entry_ctor(InputQueueEntry* entry_ptr) {
    (void)entry_ptr;
    return EB_ErrorNone;
}

static void reference_queue_entry_dctor(EbPtr p) {
    UNUSED(p);
}

EbErrorType svt_aom_reference_queue_entry_ctor(ReferenceQueueEntry* entry_ptr) {
    entry_ptr->dctor          = reference_queue_entry_dctor;
    entry_ptr->picture_number = ~0u;
    entry_ptr->is_valid       = 0;
    return EB_ErrorNone;
}
