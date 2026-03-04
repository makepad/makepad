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

#include <stdlib.h>
#include "enc_handle.h"
#include "dlf_process.h"
#include "enc_dec_results.h"
#include "reference_object.h"
#include "deblocking_filter.h"
#include "definitions.h"
#include "sequence_control_set.h"
#include "pcs.h"
#include "aom_dsp_rtcd.h"
#include "pic_operators.h"

static void dlf_context_dctor(EbPtr p) {
    EbThreadContext* thread_ctx = (EbThreadContext*)p;
    DlfContext*      obj        = (DlfContext*)thread_ctx->priv;
    EB_FREE_ARRAY(obj);
}

/******************************************************
 * Dlf Context Constructor
 ******************************************************/
EbErrorType svt_aom_dlf_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr, int index) {
    DlfContext* context_ptr;
    EB_CALLOC_ARRAY(context_ptr, 1);
    thread_ctx->priv  = context_ptr;
    thread_ctx->dctor = dlf_context_dctor;

    // Input/Output System Resource Manager FIFOs
    context_ptr->dlf_input_fifo_ptr = svt_system_resource_get_consumer_fifo(
        enc_handle_ptr->enc_dec_results_resource_ptr, index);
    context_ptr->dlf_output_fifo_ptr = svt_system_resource_get_producer_fifo(enc_handle_ptr->dlf_results_resource_ptr,
                                                                             index);
    return EB_ErrorNone;
}

/******************************************************
 * Dlf Kernel
 ******************************************************/
void* svt_aom_dlf_kernel(void* input_ptr) {
    // Context & SCS & PCS
    EbThreadContext*    thread_ctx  = (EbThreadContext*)input_ptr;
    DlfContext*         context_ptr = (DlfContext*)thread_ctx->priv;
    PictureControlSet*  pcs;
    SequenceControlSet* scs;

    //// Input
    EbObjectWrapper* enc_dec_results_wrapper;
    EncDecResults*   enc_dec_results;

    //// Output
    EbObjectWrapper*   dlf_results_wrapper;
    struct DlfResults* dlf_results;

    // SB Loop variables
    for (;;) {
        // Get EncDec Results
        EB_GET_FULL_OBJECT(context_ptr->dlf_input_fifo_ptr, &enc_dec_results_wrapper);

        enc_dec_results               = (EncDecResults*)enc_dec_results_wrapper->object_ptr;
        pcs                           = (PictureControlSet*)enc_dec_results->pcs_wrapper->object_ptr;
        PictureParentControlSet* ppcs = pcs->ppcs;
        scs                           = pcs->scs;

        bool is_16bit = scs->is_16bit_pipeline;
        if (is_16bit && scs->static_config.encoder_bit_depth == EB_EIGHT_BIT) {
            svt_aom_convert_pic_8bit_to_16bit(pcs->ppcs->enhanced_pic,
                                              pcs->input_frame16bit,
                                              pcs->ppcs->scs->subsampling_x,
                                              pcs->ppcs->scs->subsampling_y);
            // convert 8-bit recon to 16-bit for it bypass encdec process
            if (pcs->pic_bypass_encdec) {
                EbPictureBufferDesc* recon_pic;
                EbPictureBufferDesc* recon_picture_16bit_ptr;
                svt_aom_get_recon_pic(pcs, &recon_pic, 0);
                svt_aom_get_recon_pic(pcs, &recon_picture_16bit_ptr, 1);
                svt_aom_convert_pic_8bit_to_16bit(
                    recon_pic, recon_picture_16bit_ptr, pcs->ppcs->scs->subsampling_x, pcs->ppcs->scs->subsampling_y);
            }
        }
        // Initialize dev to negative value to indicate it was not computed.
        // SB-based DLF does not compute the distortion
        pcs->zero_filt_sse             = -1;
        pcs->best_filt_sse             = -1;
        pcs->dlf_dist_dev              = -1;
        FrameHeader*   frm_hdr         = &pcs->ppcs->frm_hdr;
        bool           dlf_enable_flag = (bool)pcs->ppcs->dlf_ctrls.enabled;
        const uint16_t tg_count        = pcs->ppcs->tile_group_cols * pcs->ppcs->tile_group_rows;
        // Move sb level lf to here if tile_parallel
        if ((dlf_enable_flag && !pcs->ppcs->dlf_ctrls.sb_based_dlf) ||
            (dlf_enable_flag && pcs->ppcs->dlf_ctrls.sb_based_dlf && tg_count > 1)) {
            EbPictureBufferDesc* recon_buffer;
            svt_aom_get_recon_pic(pcs, &recon_buffer, is_16bit);
            svt_av1_loop_filter_init(pcs);
            svt_av1_pick_filter_level((EbPictureBufferDesc*)pcs->ppcs->enhanced_pic, pcs, LPF_PICK_FROM_FULL_IMAGE);
            if (pcs->zero_filt_sse == -1 &&
                (frm_hdr->loop_filter_params.filter_level[0] || frm_hdr->loop_filter_params.filter_level[1])) {
                pcs->zero_filt_sse = picture_sse_calculations(pcs, recon_buffer, /*plane*/ 0);
                if (pcs->best_filt_sse != -1 && pcs->zero_filt_sse <= pcs->best_filt_sse) {
                    frm_hdr->loop_filter_params.filter_level[0] = 0;
                    frm_hdr->loop_filter_params.filter_level[1] = 0;
                    frm_hdr->loop_filter_params.filter_level_u  = 0;
                    frm_hdr->loop_filter_params.filter_level_v  = 0;
                }
            }

            svt_av1_loop_filter_frame(recon_buffer, pcs, 0, 3);
            if (pcs->best_filt_sse == -1 &&
                (frm_hdr->loop_filter_params.filter_level[0] || frm_hdr->loop_filter_params.filter_level[1])) {
                pcs->best_filt_sse = picture_sse_calculations(pcs, recon_buffer, /*plane*/ 0);
            }
            pcs->dlf_dist_dev = pcs->zero_filt_sse == 0 ||
                    !(frm_hdr->loop_filter_params.filter_level[0] || frm_hdr->loop_filter_params.filter_level[1])
                ? 0
                : (int32_t)(1000 - ((1000 * pcs->best_filt_sse) / pcs->zero_filt_sse));
        }

        //pre-cdef prep
        {
            EbPictureBufferDesc* recon_pic;
            svt_aom_get_recon_pic(pcs, &recon_pic, is_16bit);

            Av1Common* cm = pcs->ppcs->av1_cm;
            if (ppcs->enable_restoration) {
                svt_aom_link_eb_to_aom_buffer_desc(
                    recon_pic, cm->frame_to_show, scs->max_input_pad_right, scs->max_input_pad_bottom, is_16bit);
                svt_av1_loop_restoration_save_boundary_lines(cm->frame_to_show, cm, 0);
            }

            if (scs->seq_header.cdef_level && pcs->ppcs->cdef_level) {
                const uint32_t offset_y  = recon_pic->org_x + recon_pic->org_y * recon_pic->stride_y;
                pcs->cdef_input_recon[0] = recon_pic->buffer_y + (offset_y << is_16bit);
                const uint32_t offset_cb = (recon_pic->org_x + recon_pic->org_y * recon_pic->stride_cb) >> 1;
                pcs->cdef_input_recon[1] = recon_pic->buffer_cb + (offset_cb << is_16bit);
                const uint32_t offset_cr = (recon_pic->org_x + recon_pic->org_y * recon_pic->stride_cr) >> 1;
                pcs->cdef_input_recon[2] = recon_pic->buffer_cr + (offset_cr << is_16bit);

                EbPictureBufferDesc* input_pic      = is_16bit ? pcs->input_frame16bit : pcs->ppcs->enhanced_pic;
                const uint32_t       input_offset_y = input_pic->org_x + input_pic->org_y * input_pic->stride_y;
                pcs->cdef_input_source[0]           = input_pic->buffer_y + (input_offset_y << is_16bit);
                const uint32_t input_offset_cb      = (input_pic->org_x + input_pic->org_y * input_pic->stride_cb) >> 1;
                pcs->cdef_input_source[1]           = input_pic->buffer_cb + (input_offset_cb << is_16bit);
                const uint32_t input_offset_cr      = (input_pic->org_x + input_pic->org_y * input_pic->stride_cr) >> 1;
                pcs->cdef_input_source[2]           = input_pic->buffer_cr + (input_offset_cr << is_16bit);
            }
        }

        pcs->cdef_segments_column_count = scs->cdef_segment_column_count;
        pcs->cdef_segments_row_count    = scs->cdef_segment_row_count;
        pcs->cdef_segments_total_count  = (uint16_t)(pcs->cdef_segments_column_count * pcs->cdef_segments_row_count);
        pcs->tot_seg_searched_cdef      = 0;
        uint32_t segment_index;

        for (segment_index = 0; segment_index < pcs->cdef_segments_total_count; ++segment_index) {
            // Get Empty DLF Results to Cdef
            svt_get_empty_object(context_ptr->dlf_output_fifo_ptr, &dlf_results_wrapper);
            dlf_results                = (struct DlfResults*)dlf_results_wrapper->object_ptr;
            dlf_results->pcs_wrapper   = enc_dec_results->pcs_wrapper;
            dlf_results->segment_index = segment_index;
            // Post DLF Results
            svt_post_full_object(dlf_results_wrapper);
        }

        // Release EncDec Results
        svt_release_object(enc_dec_results_wrapper);
    }

    return NULL;
}
