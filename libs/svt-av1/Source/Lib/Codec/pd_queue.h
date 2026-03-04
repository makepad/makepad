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

#ifndef EbPictureDecisionQueue_h
#define EbPictureDecisionQueue_h

#include "definitions.h"
#include "sys_resource_manager.h"
#include "pred_structure.h"
#include "object.h"

/************************************************
 * PA Reference Queue Entry
 ************************************************/
typedef struct PaReferenceEntry {
    EbDctor          dctor;
    EbObjectWrapper* input_object_ptr;
    EbObjectWrapper* y8b_wrapper;
    uint64_t         picture_number;
    // The entry will be valid when it represents a valid DPB entry.
    // This is used in case the DPB is accessed before being populated,
    // and for when the DPB is cleared at EOS.
    bool     is_valid;
    uint64_t decode_order;
    uint8_t  is_alt_ref;
} PaReferenceEntry;

EbErrorType svt_aom_pa_reference_queue_entry_ctor(PaReferenceEntry* entry_ptr);

#endif // EbPictureDecisionQueue_h
