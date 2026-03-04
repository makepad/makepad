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

#include "definitions.h"
#include "av1_structs.h"

#ifndef EbDeblockingCommon_h
#define EbDeblockingCommon_h
#ifdef __cplusplus
extern "C" {
#endif

typedef enum EdgeDir { VERT_EDGE = 0, HORZ_EDGE = 1, NUM_EDGE_DIRS } EdgeDir;

typedef struct Av1DeblockingParameters {
    // length of the filter applied to the outer edge
    uint32_t filter_length;
    // deblocking limits
    const uint8_t* lim;
    const uint8_t* mblim;
    const uint8_t* hev_thr;
} Av1DeblockingParameters;

static const int32_t mode_lf_lut[] = {
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // INTRA_MODES
    1, 1, 0, 1, // INTER_MODES (GLOBALMV == 0)
    1, 1, 1, 1, 1, 1, 0, 1 // INTER_COMPOUND_MODES (GLOBAL_GLOBALMV == 0)
};

uint8_t svt_aom_get_filter_level_delta_lf(FrameHeader* frm_hdr, const int32_t dir_idx, int32_t plane,
                                          int32_t* sb_delta_lf, uint8_t seg_id, PredictionMode pred_mode,
                                          MvReferenceFrame ref_frame_0);

static INLINE int32_t is_inter_block_no_intrabc(MvReferenceFrame ref_frame_0) {
    return /*is_intrabc_block(mbmi) ||*/ ref_frame_0 > INTRA_FRAME;
}

void svt_av1_loop_filter_frame_init(FrameHeader* frm_hdr, LoopFilterInfoN* lf_info, int32_t plane_start,
                                    int32_t plane_end);

void svt_aom_update_sharpness(LoopFilterInfoN* lfi, int32_t sharpness_lvl);

#ifdef __cplusplus
}
#endif
#endif // EbDeblockingCommon_h
