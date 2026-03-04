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
#include "packetization_reorder_queue.h"

static void packetization_reorder_entry_dctor(EbPtr p) {
    PacketizationReorderEntry* obj = (PacketizationReorderEntry*)p;
    EB_DELETE(obj->bitstream_ptr);
}

EbErrorType svt_aom_packetization_reorder_entry_ctor(PacketizationReorderEntry* entry_ptr, uint32_t picture_number) {
    entry_ptr->dctor          = packetization_reorder_entry_dctor;
    entry_ptr->picture_number = picture_number;
    //16 should enough for show existing frame
    EB_NEW(entry_ptr->bitstream_ptr, svt_aom_bitstream_ctor, 16);
    return EB_ErrorNone;
}
