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
#include "pd_queue.h"

static void pa_reference_queue_entry_dctor(EbPtr p) {
    UNUSED(p);
}

EbErrorType svt_aom_pa_reference_queue_entry_ctor(PaReferenceEntry* entry_ptr) {
    entry_ptr->dctor    = pa_reference_queue_entry_dctor;
    entry_ptr->is_valid = 0;

    return EB_ErrorNone;
}
