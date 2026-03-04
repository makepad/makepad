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
#include <stdio.h>
#include "enc_handle.h"
#include "entropy_coding.h"
#include "ec_process.h"
#include "enc_dec_results.h"
#include "ec_results.h"
#include "rc_tasks.h"
#include "cabac_context_model.h"
#include "svt_log.h"
#include "common_dsp_rtcd.h"

static void rest_context_dctor(EbPtr p) {
    EbThreadContext*      thread_ctx = (EbThreadContext*)p;
    EntropyCodingContext* obj        = (EntropyCodingContext*)thread_ctx->priv;
    EB_FREE_ARRAY(obj);
}

/******************************************************
 * Enc Dec Context Constructor
 ******************************************************/
EbErrorType svt_aom_entropy_coding_context_ctor(EbThreadContext* thread_ctx, const EbEncHandle* enc_handle_ptr,
                                                int index) {
    EntropyCodingContext* context_ptr;
    EB_CALLOC_ARRAY(context_ptr, 1);
    thread_ctx->priv  = context_ptr;
    thread_ctx->dctor = rest_context_dctor;

    context_ptr->is_16bit = enc_handle_ptr->scs_instance->scs->static_config.encoder_bit_depth > EB_EIGHT_BIT;

    // Input/Output System Resource Manager FIFOs
    context_ptr->enc_dec_input_fifo_ptr = svt_system_resource_get_consumer_fifo(
        enc_handle_ptr->rest_results_resource_ptr, index);
    context_ptr->entropy_coding_output_fifo_ptr = svt_system_resource_get_producer_fifo(
        enc_handle_ptr->entropy_coding_results_resource_ptr, index);

    return EB_ErrorNone;
}

/***********************************************
 * Entropy Coding Reset Neighbor Arrays
 ***********************************************/
static void entropy_coding_reset_neighbor_arrays(PictureControlSet* pcs, uint16_t tile_idx) {
    svt_aom_neighbor_array_unit_reset(pcs->partition_context_na[tile_idx]);

    svt_aom_neighbor_array_unit_reset(pcs->luma_dc_sign_level_coeff_na[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->cb_dc_sign_level_coeff_na[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->cr_dc_sign_level_coeff_na[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->txfm_context_array[tile_idx]);
    svt_aom_neighbor_array_unit_reset(pcs->segmentation_id_pred_array[tile_idx]);
    return;
}

/**************************************************
 * Reset Entropy Coding Picture
 **************************************************/
static void reset_entropy_coding_picture(EntropyCodingContext* ctx, PictureControlSet* pcs, SequenceControlSet* scs) {
    PictureParentControlSet* ppcs     = pcs->ppcs;
    const uint16_t           tile_cnt = ppcs->av1_cm->tiles_info.tile_rows * ppcs->av1_cm->tiles_info.tile_cols;
    ctx->is_16bit                     = scs->static_config.encoder_bit_depth > EB_EIGHT_BIT;
    const FrameHeader* frm_hdr        = &ppcs->frm_hdr;
    // Asuming cb and cr offset to be the same for chroma QP in both slice and pps for lambda computation
    const uint32_t entropy_coding_qp = frm_hdr->quantization_params.base_q_idx;

    for (uint16_t tile_idx = 0; tile_idx < tile_cnt; tile_idx++) {
        ppcs->prev_qindex[tile_idx] = entropy_coding_qp;
    }
    if (frm_hdr->allow_intrabc) {
        assert(frm_hdr->delta_lf_params.delta_lf_present == 0);
    }
    if (frm_hdr->delta_lf_params.delta_lf_present) {
        ppcs->prev_delta_lf_from_base = 0;

        const int frame_lf_count = ppcs->monochrome == 0 ? FRAME_LF_COUNT : FRAME_LF_COUNT - 2;
        for (int lf_id = 0; lf_id < frame_lf_count; ++lf_id) {
            ppcs->prev_delta_lf[lf_id] = 0;
        }
    }

    // pass the ent
    for (uint16_t tile_idx = 0; tile_idx < tile_cnt; tile_idx++) {
        EntropyCoder*        ec                   = pcs->ec_info[tile_idx]->ec;
        OutputBitstreamUnit* output_bitstream_ptr = ec->ec_output_bitstream_ptr;
        //****************************************************************//
        ec->ec_writer.allow_update_cdf = !ppcs->large_scale_tile && !frm_hdr->disable_cdf_update;
        aom_start_encode(&ec->ec_writer, output_bitstream_ptr);
        // ADD Reset here
        const uint8_t primary_ref_frame = frm_hdr->primary_ref_frame;
        if (primary_ref_frame != PRIMARY_REF_NONE) {
            // primary ref stored as REF_FRAME_MINUS1, while get_list_idx/get_ref_frame_idx take arg of ref frame
            // Therefore, add 1 to the primary ref frame (e.g. LAST --> LAST_FRAME)
            const uint8_t      list_idx = get_list_idx(primary_ref_frame + 1);
            const uint8_t      ref_idx  = get_ref_frame_idx(primary_ref_frame + 1);
            EbReferenceObject* ref      = (EbReferenceObject*)pcs->ref_pic_ptr_array[list_idx][ref_idx]->object_ptr;
            svt_memcpy(ec->fc, &ref->frame_context, sizeof(FRAME_CONTEXT));
        } else {
            svt_aom_reset_entropy_coder(scs->enc_ctx, ec, entropy_coding_qp, pcs->slice_type);
        }

        entropy_coding_reset_neighbor_arrays(pcs, tile_idx);
    }
}

/* Entropy Coding */

/*********************************************************************************
 *
 * @brief
 *  The Entropy Coding process is responsible for producing an AV1 conformant bitstream for each
 *frame.
 *
 * @par Description:
 *  The entropy coder is a frame-based process and is based on multi-symbol arithmetic range coding.
 *  It takes as input the coding decisions and information for each block and produces as output the
 *bitstream for each frame.
 *
 * @param[in] Coding Decisions
 *  Coding decisions and information for each block.
 *
 * @param[out] bitstream
 *  Bitstream for each block
 *
 ********************************************************************************/
void* svt_aom_entropy_coding_kernel(void* input_ptr) {
    // Context & SCS & PCS
    EbThreadContext*      thread_ctx  = (EbThreadContext*)input_ptr;
    EntropyCodingContext* context_ptr = (EntropyCodingContext*)thread_ctx->priv;

    // Input
    EbObjectWrapper* rest_results_wrapper;

    // Output
    EbObjectWrapper*      entropy_coding_results_wrapper_ptr;
    EntropyCodingResults* entropy_coding_results_ptr;

    for (;;) {
        // Get Mode Decision Results
        EB_GET_FULL_OBJECT(context_ptr->enc_dec_input_fifo_ptr, &rest_results_wrapper);

        RestResults*        rest_results = (RestResults*)rest_results_wrapper->object_ptr;
        PictureControlSet*  pcs          = (PictureControlSet*)rest_results->pcs_wrapper->object_ptr;
        SequenceControlSet* scs          = pcs->scs;
        // SB Constants

        uint8_t sb_size = (uint8_t)scs->sb_size;

        uint8_t          sb_size_log2    = (uint8_t)svt_log2f(sb_size);
        uint32_t         pic_width_in_sb = (pcs->ppcs->aligned_width + sb_size - 1) >> sb_size_log2;
        uint16_t         tile_idx        = rest_results->tile_index;
        Av1Common* const cm              = pcs->ppcs->av1_cm;
        const uint16_t   tile_cnt        = cm->tiles_info.tile_rows * cm->tiles_info.tile_cols;
        const uint16_t   tile_col        = tile_idx % cm->tiles_info.tile_cols;
        const uint16_t   tile_row        = tile_idx / cm->tiles_info.tile_cols;
        const uint16_t   tile_sb_start_x = cm->tiles_info.tile_col_start_mi[tile_col] >> scs->seq_header.sb_size_log2;
        const uint16_t   tile_sb_start_y = cm->tiles_info.tile_row_start_mi[tile_row] >> scs->seq_header.sb_size_log2;

        uint16_t tile_width_in_sb = (cm->tiles_info.tile_col_start_mi[tile_col + 1] -
                                     cm->tiles_info.tile_col_start_mi[tile_col]) >>
            scs->seq_header.sb_size_log2;
        uint16_t tile_height_in_sb = (cm->tiles_info.tile_row_start_mi[tile_row + 1] -
                                      cm->tiles_info.tile_row_start_mi[tile_row]) >>
            scs->seq_header.sb_size_log2;

        bool frame_entropy_done = false;

        svt_block_on_mutex(pcs->entropy_coding_pic_mutex);
        if (pcs->entropy_coding_pic_reset_flag) {
            pcs->entropy_coding_pic_reset_flag = false;

            reset_entropy_coding_picture(context_ptr, pcs, scs);
        }
        svt_release_mutex(pcs->entropy_coding_pic_mutex);

        if (!svt_aom_is_pic_skipped(pcs->ppcs)) {
            for (uint32_t y_sb_index = 0; y_sb_index < tile_height_in_sb; ++y_sb_index) {
                for (uint32_t x_sb_index = 0; x_sb_index < tile_width_in_sb; ++x_sb_index) {
                    uint16_t    sb_index = (uint16_t)((x_sb_index + tile_sb_start_x) +
                                                   (y_sb_index + tile_sb_start_y) * pic_width_in_sb);
                    SuperBlock* sb_ptr   = pcs->sb_ptr_array[sb_index];

                    const uint32_t sb_origin_x = (x_sb_index + tile_sb_start_x) << sb_size_log2;
                    const uint32_t sb_origin_y = (y_sb_index + tile_sb_start_y) << sb_size_log2;
                    if (x_sb_index == 0 && y_sb_index == 0) {
                        svt_av1_reset_loop_restoration(context_ptr);
                        context_ptr->tok = pcs->tile_tok[tile_row][tile_col];
                    }

                    EbPictureBufferDesc* coeff_picture_ptr = pcs->ppcs->enc_dec_ptr->quantized_coeff[sb_index];
                    context_ptr->coded_area_sb             = 0;
                    context_ptr->coded_area_sb_uv          = 0;
                    svt_aom_write_modes_sb(context_ptr,
                                           sb_ptr,
                                           pcs,
                                           tile_idx,
                                           pcs->ec_info[tile_idx]->ec,
                                           coeff_picture_ptr,
                                           sb_ptr->ptree,
                                           sb_origin_y >> MI_SIZE_LOG2,
                                           sb_origin_x >> MI_SIZE_LOG2);
                }
            }
        }
        bool pic_ready = true;

        // Current tile ready
        svt_aom_encode_slice_finish(pcs->ec_info[tile_idx]->ec);

        svt_block_on_mutex(pcs->entropy_coding_pic_mutex);
        pcs->ec_info[tile_idx]->entropy_coding_tile_done = true;
        for (uint16_t i = 0; i < tile_cnt; i++) {
            if (pcs->ec_info[i]->entropy_coding_tile_done == false) {
                pic_ready = false;
                break;
            }
        }
        svt_release_mutex(pcs->entropy_coding_pic_mutex);
        if (pic_ready) {
            if (pcs->ppcs->superres_total_recode_loop == 0) {
                // Release the reference Pictures from both lists
                for (REF_FRAME_MINUS1 ref = LAST; ref < ALT + 1; ref++) {
                    const uint8_t list_idx = get_list_idx(ref + 1);
                    const uint8_t ref_idx  = get_ref_frame_idx(ref + 1);
                    if (pcs->ref_pic_ptr_array[list_idx][ref_idx] != NULL) {
                        svt_release_object(pcs->ref_pic_ptr_array[list_idx][ref_idx]);
                    }
                }

                //free palette data
                if (pcs->tile_tok[0][0]) {
                    EB_FREE_ARRAY(pcs->tile_tok[0][0]);
                }
            }
            frame_entropy_done = true;
        }

        if (frame_entropy_done) {
            if (pcs->ppcs->valid_qindex_area) {
                pcs->ppcs->avg_qp = ((pcs->ppcs->tot_qindex / pcs->ppcs->valid_qindex_area) + 2) >> 2;
            }
            // Get Empty Entropy Coding Results
            svt_get_empty_object(context_ptr->entropy_coding_output_fifo_ptr, &entropy_coding_results_wrapper_ptr);
            entropy_coding_results_ptr = (EntropyCodingResults*)entropy_coding_results_wrapper_ptr->object_ptr;
            entropy_coding_results_ptr->pcs_wrapper = rest_results->pcs_wrapper;

            // Post EntropyCoding Results
            svt_post_full_object(entropy_coding_results_wrapper_ptr);
        }

        // Release Mode Decision Results
        svt_release_object(rest_results_wrapper);
    }

    return NULL;
}
