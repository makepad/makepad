/*
* Copyright(c) 2019 Intel Corporation
* Copyright (c) 2016, Alliance for Open Media. All rights reserved
*
* This source code is subject to the terms of the BSD 2 Clause License and
* the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
* was not distributed with this source code in the LICENSE file, you can
* obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
* Media Patent License 1.0 was not distributed with this source code in the
* PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
*/

#ifndef EbGlobalMotionEstimation_h
#define EbGlobalMotionEstimation_h

#include "pic_buffer_desc.h"
#include "me_context.h"

void                    svt_aom_global_motion_estimation(PictureParentControlSet* pcs, EbPictureBufferDesc* input_pic);
void                    svt_aom_upscale_wm_params(WarpedMotionParams* wm_params, uint8_t scale_factor);
extern MvReferenceFrame svt_get_ref_frame_type(uint8_t list, uint8_t ref_idx);
#endif // EbGlobalMotionEstimation_h
