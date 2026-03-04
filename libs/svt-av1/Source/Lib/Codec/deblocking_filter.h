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

#include "neighbor_arrays.h"
#include "enc_dec_process.h"
#include "dlf_process.h"
#include "deblocking_common.h"
#include "common_dsp_rtcd.h"
#ifndef EbDeblockingFilter_h
#define EbDeblockingFilter_h
#ifdef __cplusplus
extern "C" {
#endif

typedef enum LpfPickMethod {
    // Try the full image with different values.
    LPF_PICK_FROM_FULL_IMAGE,
    // Try a small portion of the image with different values.
    LPF_PICK_FROM_SUBIMAGE,
    // Estimate the level based on quantizer and frame type
    LPF_PICK_FROM_Q,
    // Pick 0 to disable LPF if LPF was enabled last frame
    LPF_PICK_MINIMAL_LPF
} LpfPickMethod;

/* assorted LoopFilter functions which get used elsewhere */
struct AV1Common;
struct macroblockd;
struct AV1LfSyncData;

void svt_av1_loop_filter_init(PictureControlSet* pcs);

void svt_aom_loop_filter_sb(EbPictureBufferDesc* frame_buffer, //reconpicture,
                            //Yv12BufferConfig *frame_buffer,
                            PictureControlSet* pcs, int32_t mi_row, int32_t mi_col, int32_t plane_start,
                            int32_t plane_end, uint8_t last_col);

void svt_av1_loop_filter_frame(
        EbPictureBufferDesc *frame_buffer,//reconpicture,
        //Yv12BufferConfig *frame_buffer,
        PictureControlSet *pcs,
        /*MacroBlockD *xd,*/ int32_t plane_start, int32_t plane_end/*,
        int32_t partial_frame*/);
uint64_t picture_sse_calculations(PictureControlSet* pcs, EbPictureBufferDesc* recon_ptr, int32_t plane);

EbErrorType svt_av1_pick_filter_level(EbPictureBufferDesc* srcBuffer, // source input
                                      PictureControlSet* pcs, LpfPickMethod method);
void        svt_av1_pick_filter_level_by_q(PictureControlSet* pcs, uint8_t qindex, int32_t* filter_level);

void svt_av1_filter_block_plane_vert(const PictureControlSet* const pcs, const int32_t plane,
                                     const MacroblockdPlane* const plane_ptr, const uint32_t mi_row,
                                     const uint32_t mi_col);

void svt_av1_filter_block_plane_horz(const PictureControlSet* const pcs, const int32_t plane,
                                     const MacroblockdPlane* const plane_ptr, const uint32_t mi_row,
                                     const uint32_t mi_col);

#ifdef __cplusplus
}
#endif
#endif
