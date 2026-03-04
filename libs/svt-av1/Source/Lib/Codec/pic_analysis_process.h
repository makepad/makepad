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

#ifndef EbPictureAnalysis_h
#define EbPictureAnalysis_h

#include "definitions.h"

#include "pcs.h"
#include "sequence_control_set.h"

/***************************************
 * Extern Function Declaration
 ***************************************/
EbErrorType svt_aom_picture_analysis_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                                  int index);

void* svt_aom_picture_analysis_kernel(void* input_ptr);

void svt_aom_downsample_filtering_input_picture(PictureParentControlSet* pcs, EbPictureBufferDesc* input_padded_pic,
                                                EbPictureBufferDesc* quarter_picture_ptr,
                                                EbPictureBufferDesc* sixteenth_picture_ptr);

void svt_aom_pad_input_pictures(SequenceControlSet* scs, EbPictureBufferDesc* input_pic);

#endif // EbPictureAnalysis_h
