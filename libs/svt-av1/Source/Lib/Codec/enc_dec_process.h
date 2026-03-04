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

#ifndef EbEncDecProcess_h
#define EbEncDecProcess_h

#include "definitions.h"
#include "md_process.h"
#include "sys_resource_manager.h"
#include "pic_buffer_desc.h"
#include "mode_decision.h"
#include "enc_inter_prediction.h"
#include "entropy_coding.h"
#include "reference_object.h"
#include "neighbor_arrays.h"
#include "coding_unit.h"
#include "object.h"

#ifdef __cplusplus
extern "C" {
#endif

/**************************************
     * Enc Dec Context
     **************************************/
typedef struct EncDecContext {
    EbFifo*              mode_decision_input_fifo_ptr;
    EbFifo*              enc_dec_output_fifo_ptr;
    EbFifo*              enc_dec_feedback_fifo_ptr;
    EbFifo*              picture_demux_output_fifo_ptr; // to picture-manager
    ModeDecisionContext* md_ctx;
    const BlockGeom*     blk_geom;
    // Coding Unit Workspace---------------------------
    EbPictureBufferDesc* input_samples;
    EbPictureBufferDesc* input_sample16bit_buffer;
    // temporary buffers for decision making of LF (LPF_PICK_FROM_FULL_IMAGE).
    // Since recon switches between reconPtr and referencePtr, the temporary buffers sizes used the referencePtr's which has padding,...
    uint32_t pic_fast_lambda[2];
    uint32_t pic_full_lambda[2];

    //  Context Variables---------------------------------
    BlkStruct* blk_ptr;
    //const CodedBlockStats                *cu_stats;
    uint16_t blk_org_x; // within the picture
    uint16_t blk_org_y; // within the picture
    uint32_t sb_index;
    uint8_t  txb_itr;
    bool     is_16bit; //enable 10 bit encode in CL
    uint32_t bit_depth;
    uint64_t tot_intra_coded_area;
    uint64_t tot_skip_coded_area;
    uint64_t tot_hp_coded_area;
    uint64_t tot_cnt_zero_mv;
    uint64_t three_quad_energy;

    uint16_t coded_area_sb;
    uint16_t coded_area_sb_uv;
    uint16_t coded_area_sb_update;
    uint16_t coded_area_sb_uv_update;

    uint8_t md_skip_blk;

    uint16_t tile_group_index;
    uint16_t tile_index;
    uint32_t coded_sb_count;
} EncDecContext;

/**************************************
 * Extern Function Declarations
 **************************************/
EbErrorType svt_aom_enc_dec_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr, int index,
                                         int tasks_index);

void* svt_aom_mode_decision_kernel(void* input_ptr);

#ifdef __cplusplus
}
#endif
#endif // EbEncDecProcess_h
