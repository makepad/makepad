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

#ifndef EbPacketization_h
#define EbPacketization_h

#include "definitions.h"
#ifdef __cplusplus
extern "C" {
#endif

/**************************************
 * Extern Function Declarations
 **************************************/
EbErrorType svt_aom_packetization_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                               int rate_control_index, int demux_index, int me_port_index);

void* svt_aom_packetization_kernel(void* input_ptr);
// Release the pd_dpb and ref_pic_list at the end of the sequence
void release_references_eos(SequenceControlSet* scs);
#ifdef __cplusplus
}
#endif
#endif // EbPacketization_h
