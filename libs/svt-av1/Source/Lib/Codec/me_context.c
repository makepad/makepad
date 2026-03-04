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
#include <string.h>

#include "me_context.h"
#include "utility.h"

static void me_context_dctor(EbPtr p) {
    MeContext* obj = (MeContext*)p;

    EB_FREE_ARRAY(obj->p_eight_pos_sad16x16);
}

EbErrorType svt_aom_me_context_ctor(MeContext* object_ptr) {
    object_ptr->dctor = me_context_dctor;

    EB_MALLOC_ARRAY(object_ptr->p_eight_pos_sad16x16,
                    8 * 16); //16= 16 16x16 blocks in a SB.       8=8search points

    // Initialize Alt-Ref parameters
    object_ptr->me_type                     = ME_CLOSE_LOOP;
    object_ptr->num_of_list_to_search       = 1;
    object_ptr->num_of_ref_pic_to_search[0] = 0;
    object_ptr->num_of_ref_pic_to_search[1] = 0;

    return EB_ErrorNone;
}
